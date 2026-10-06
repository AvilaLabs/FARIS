#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Build ACTINV problem specs for FARIS components (0.2 activation test run).

For every non-void component, and every installation of it in an operating
history (from time zero or a replacement_completed event to the next
replacement_started event or the history end), write one `actinv-spec-1`
problem file and a provenance sidecar. With an impurity file a second spec per
installation adds the listed impurities. The script never runs ACTINV's solver;
it only calls `actinv validate` on each written spec.

Formats used (ACTINV docs/guide/specification.md):
  material  basis "atom_fraction" with explicit nuclide keys ("Required inputs",
            "Material bases"); mass_g is the component mass.
  spectrum  structure "fispact-709", 709 group-integrated fluxes in
            n cm^-2 s^-1, ascending energy (descending: false); "Projectile
            and spectrum".
  schedule  ordered {"dt": "<seconds> s", "flux": multiplier} pairs; a zero
            multiplier is an exact decay-only gap ("Options and result").

Exit status: 0 success, 1 validation failure, 2 bad input, 3 no 709-group
spectrum in the run record (and no --allow-placeholder-spectrum).
"""
from __future__ import annotations

import argparse
import bisect
import hashlib
import json
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import zipfile
from pathlib import Path

SCRIPT_VERSION = "1"
SPEC_FORMAT = "actinv-spec-1"
CATALOG_VERSION = "v1.1.0"
LIBRARY_ID = "tendl-2025-neutron-709g"
LIBRARY_BUNDLE = "tendl-2025-neutron"
DEFAULT_DATA_DIR = "/home/connoravila/Documents/actinv/actinv-data"
DEFAULT_COOLING = "1s,1h,1d,1w,30d,1y,10y,100y"
GROUPS = 709
YEAR_S = 365.25 * 86400.0
UNIT_S = {"s": 1.0, "min": 60.0, "h": 3600.0, "d": 86400.0, "w": 7 * 86400.0, "y": YEAR_S}
NO_SPECTRUM_MESSAGE = "no 709-group spectrum in this run record; the 0.2 transport adds it"
BARE = "bare_lower_bound"
WITH_IMPURITIES = "specification_maximum_impurities"
PLACEHOLDER = "placeholder_flat_lethargy_not_physics"
# options.outputs values ACTINV accepts (its default is every output)
ACTINV_OUTPUTS = ("inventory", "activity", "heat", "photons", "dose", "pathways", "radiological", "damage",
                  "ledger", "certificate", "audit")
ACTINV_DEFAULT = Path.home() / ".local" / "bin" / "actinv"

# Standard atomic weights (g/mol), conventional values, hydrogen to uranium.
ATOMIC_WEIGHT = dict(zip(
    "H He Li Be B C N O F Ne Na Mg Al Si P S Cl Ar K Ca Sc Ti V Cr Mn Fe Co Ni Cu Zn Ga Ge As Se Br Kr "
    "Rb Sr Y Zr Nb Mo Tc Ru Rh Pd Ag Cd In Sn Sb Te I Xe Cs Ba La Ce Pr Nd Pm Sm Eu Gd Tb Dy Ho Er Tm Yb "
    "Lu Hf Ta W Re Os Ir Pt Au Hg Tl Pb Bi Po At Rn Fr Ra Ac Th Pa U".split(),
    [1.008, 4.0026, 6.94, 9.0122, 10.81, 12.011, 14.007, 15.999, 18.998, 20.180, 22.990, 24.305, 26.982,
     28.085, 30.974, 32.06, 35.45, 39.948, 39.098, 40.078, 44.956, 47.867, 50.942, 51.996, 54.938, 55.845,
     58.933, 58.693, 63.546, 65.38, 69.723, 72.630, 74.922, 78.971, 79.904, 83.798, 85.468, 87.62, 88.906,
     91.224, 92.906, 95.95, 98.0, 101.07, 102.91, 106.42, 107.87, 112.41, 114.82, 118.71, 121.76, 127.60,
     126.90, 131.29, 132.91, 137.33, 138.91, 140.12, 140.91, 144.24, 145.0, 150.36, 151.96, 157.25, 158.93,
     162.50, 164.93, 167.26, 168.93, 173.05, 174.97, 178.49, 180.95, 183.84, 186.21, 190.23, 192.22, 195.08,
     196.97, 200.59, 204.38, 207.2, 208.98, 209.0, 210.0, 222.0, 223.0, 226.0, 227.0, 232.04, 231.04, 238.03],
))
NUCLIDE = re.compile(r"^([A-Z][a-z]?)(\d{1,3})(m\d*)?$")


class InputError(Exception):
    """Bad or inconsistent input (exit status 2)."""


class NoSpectrum(Exception):
    """The run record has no 709-group neutron spectrum (exit status 3)."""


def sha256_file(path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_json(path, what):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as err:
        raise InputError(f"cannot read {what} {path}: {err}") from err


def parse_outputs(text: str) -> list[str]:
    """'heat' -> ['heat']; every entry must be in ACTINV's allowed set."""
    items = [x.strip() for x in text.split(",")]
    bad = [x for x in items if x not in ACTINV_OUTPUTS]
    if not items or bad:
        raise InputError(f"bad --actinv-outputs entry {bad or text!r}; allowed: {', '.join(ACTINV_OUTPUTS)}")
    return items


def parse_cooling_grid(text: str) -> list[tuple[str, float]]:
    """'1s,1h,30d' -> [('1s', 1.0), ('1h', 3600.0), ('30d', 2592000.0)], strictly increasing."""
    grid = []
    for item in text.split(","):
        match = re.fullmatch(r"\s*([0-9]*\.?[0-9]+(?:[eE][+-]?\d+)?)\s*(s|min|h|d|w|y)\s*", item)
        if not match:
            raise InputError(f"bad cooling-grid entry {item!r}; use e.g. 1s, 1h, 1d, 1w, 30d, 1y")
        grid.append((item.strip(), float(match.group(1)) * UNIT_S[match.group(2)]))
    if not grid or any(b[1] <= a[1] for a, b in zip(grid, grid[1:])) or grid[0][1] <= 0:
        raise InputError("cooling grid must be positive and strictly increasing")
    return grid


# ---------------------------------------------------------------- library --

def read_library_bounds(npz_path: Path) -> list[float]:
    """The 710 group boundaries (eV, ascending) stored in the activation library."""
    with zipfile.ZipFile(npz_path) as archive, archive.open("bounds.npy") as handle:
        if handle.read(6) != b"\x93NUMPY":
            raise InputError(f"{npz_path}: bounds.npy is not an npy array")
        major = handle.read(2)[0]
        size = struct.unpack("<H" if major == 1 else "<I", handle.read(2 if major == 1 else 4))[0]
        header = handle.read(size).decode("latin1")
        if "'<f8'" not in header or "'fortran_order': False" not in header:
            raise InputError(f"{npz_path}: unexpected bounds dtype {header.strip()}")
        raw = handle.read()
    count = len(raw) // 8
    bounds = list(struct.unpack(f"<{count}d", raw[:count * 8]))
    if count != GROUPS + 1:
        raise InputError(f"{npz_path}: {count} boundaries, expected {GROUPS + 1}")
    return bounds


def library_info(data_dir: Path) -> dict:
    activation = data_dir / CATALOG_VERSION / "activation"
    npz, index = activation / f"{LIBRARY_ID}.npz", activation / f"{LIBRARY_ID}_index.json"
    if not npz.is_file() or not index.is_file():
        raise InputError(f"library {LIBRARY_ID} not found under {activation}; run `actinv data fetch`")
    meta = load_json(index, "library index")
    return {
        "catalog_version": CATALOG_VERSION,
        "bundle": LIBRARY_BUNDLE,
        "artifact": LIBRARY_ID,
        "library_sha256": meta["sha256_npz"],
        "index_sha256": sha256_file(index),
        "group_boundary_sha256": meta.get("group_boundary_sha256"),
        "bounds_eV": read_library_bounds(npz),
    }


# --------------------------------------------------------------- material --

def nuclide_mass_number(name: str) -> int:
    match = NUCLIDE.match(name)
    if not match:
        raise InputError(f"not an explicit nuclide name: {name}")
    return int(match.group(2))


def element_of(name: str) -> str:
    match = re.match(r"^([A-Z][a-z]?)", name)
    if not match or match.group(1) not in ATOMIC_WEIGHT:
        raise InputError(f"unknown element in {name!r}")
    return match.group(1)


def parse_impurities(entries, material_id: str) -> list[dict]:
    """Normalise one material's impurity list to {key, kind, mass_fraction, citation}."""
    out, seen = [], set()
    for entry in entries:
        keys = [k for k in ("element", "nuclide") if k in entry]
        amounts = [k for k in ("wt_fraction", "ppm") if k in entry]
        if len(keys) != 1 or len(amounts) != 1 or not str(entry.get("citation", "")).strip():
            raise InputError(
                f"impurity entry for {material_id} needs one of element/nuclide, one of wt_fraction/ppm "
                f"and a citation: {entry}"
            )
        kind = keys[0]
        key = entry[kind]
        if kind == "element":
            if key not in ATOMIC_WEIGHT:
                raise InputError(f"unknown impurity element {key!r} for {material_id}")
        else:
            nuclide_mass_number(key)
        fraction = float(entry["wt_fraction"]) if amounts[0] == "wt_fraction" else float(entry["ppm"]) * 1e-6
        if not (0.0 < fraction < 1.0) or not math.isfinite(fraction):
            raise InputError(f"impurity {key} for {material_id} has mass fraction {fraction}; need 0 < w < 1")
        if key in seen:
            raise InputError(f"duplicate impurity {key} for {material_id}")
        seen.add(key)
        out.append({"key": key, "kind": kind, "mass_fraction": fraction, "citation": entry["citation"]})
    if sum(i["mass_fraction"] for i in out) >= 1.0:
        raise InputError(f"impurity mass fractions of {material_id} sum to 1 or more")
    return out


def bare_composition(recipe: dict) -> dict[str, float]:
    atoms = {n["nuclide"]: float(n["atom_fraction"]) for n in recipe["nuclides"]}
    total = sum(atoms.values())
    if not atoms or total <= 0:
        raise InputError("nuclide_mixture has no atoms")
    return {k: v / total for k, v in atoms.items()}


def composition_with_impurities(bare: dict[str, float], impurities: list[dict]) -> tuple[dict, dict]:
    """Atom-fraction composition of (1 - sum w) bare material plus the impurities.

    The bare nuclides keep their mutual atom ratios exactly; the bare part of
    the mass is 1 - sum(w), with its mean atomic mass taken from the nuclide
    mass numbers. Impurity atoms per gram are w / (standard atomic weight or
    mass number). Returns (composition, report).
    """
    for imp in impurities:
        element = imp["key"] if imp["kind"] == "element" else element_of(imp["key"])
        clash = [k for k in bare if element_of(k) == element or k == imp["key"]]
        if clash:
            raise InputError(
                f"impurity {imp['key']} overlaps the transport nuclide(s) {clash}; "
                "ACTINV cannot take an element and its isotopes together, list only other elements"
            )
    mean_mass = sum(frac * nuclide_mass_number(k) for k, frac in bare.items())
    w_total = sum(i["mass_fraction"] for i in impurities)
    bare_atoms = (1.0 - w_total) / mean_mass
    composition = {k: frac * bare_atoms for k, frac in bare.items()}
    for imp in impurities:
        mass = ATOMIC_WEIGHT[imp["key"]] if imp["kind"] == "element" else float(nuclide_mass_number(imp["key"]))
        composition[imp["key"]] = imp["mass_fraction"] / mass
    return composition, {
        "impurity_mass_fraction_total": w_total,
        "bare_mass_fraction": 1.0 - w_total,
        "bare_mean_mass_number": mean_mass,
    }


def component_volume_m3(run: dict, component_id: str) -> tuple[float, str]:
    """Component volume from the run record's normalized results (`volume_m3`).

    Every result entry of the component carries the same volume; disagreement
    beyond 1e-9 relative is an error.
    """
    volumes = [
        r["volume_m3"] for r in run["normalized"]["results"]
        if r.get("domain", {}).get("kind") == "component"
        and r["domain"].get("component_id") == component_id and r.get("volume_m3") is not None
    ]
    if not volumes:
        raise InputError(f"no volume_m3 for component {component_id} in the run record")
    if max(volumes) - min(volumes) > 1e-9 * max(volumes):
        raise InputError(f"component {component_id} has inconsistent volumes in the run record")
    return float(volumes[0]), "run.normalized.results[].volume_m3 (domain.kind=component)"


# --------------------------------------------------------------- spectrum --

def total_neutron_flux_cm2(run: dict, component_id: str) -> tuple[float, str]:
    """Component neutron flux (n cm^-2 s^-1) from the component-average flux response."""
    for r in run["normalized"]["results"]:
        if (r.get("domain", {}).get("kind") == "component" and r["domain"].get("component_id") == component_id
                and r.get("score", {}).get("kind") == "flux"):
            return float(r["mean"]) * 1e-4, f"run.normalized.results[{r['response_id']}].mean"
    for s in run.get("normalized_spectra") or []:
        if s["component_id"] == component_id and s["particle"] == "neutron":
            return sum(s["mean_per_square_metre_second"]) * 1e-4, "sum of normalized_spectra"
    raise InputError(f"no neutron flux for component {component_id} in the run record")


def find_709_spectrum(run: dict, component_id: str, bounds: list[float]) -> dict | None:
    """The component's 709-group neutron spectrum in n cm^-2 s^-1, or None.

    The edges must equal the library's boundaries to 1e-12 relative; a leading
    0.0 edge (FARIS tallies start at zero) stands for the library's lowest edge.
    """
    for s in run.get("normalized_spectra") or []:
        if s["component_id"] != component_id or s["particle"] != "neutron":
            continue
        values = s["mean_per_square_metre_second"]
        if len(values) != GROUPS:
            continue
        edges = list(s["energy_edges_ev"])
        if len(edges) != GROUPS + 1:
            raise InputError(f"{component_id}: 709 values but {len(edges)} energy edges")
        floor_replaced = edges[0] == 0.0
        if floor_replaced:
            edges[0] = bounds[0]
        bad = [i for i, (a, b) in enumerate(zip(edges, bounds)) if abs(a - b) > 1e-12 * b]
        if bad:
            raise InputError(f"{component_id}: spectrum edges differ from the fispact-709 boundaries at {bad[:5]}")
        return {
            "flux_per_group": [v * 1e-4 for v in values],
            "source": "run.normalized_spectra (709 groups; n m^-2 s^-1 converted to n cm^-2 s^-1)",
            "zero_lower_edge_replaced_by_library_floor": floor_replaced,
        }
    return None


def spectrum_error_summary(run: dict, component_id: str) -> dict:
    """Relative sampling error of the component's 709-group neutron spectrum.

    Per group the relative error is standard error / mean over groups with a
    positive mean. `flux_weighted_mean_relative_error` weights each group by its
    share of the flux (so it is sum(standard error) / sum(mean)).
    `max_relative_error_groups_ge_1pct` is the largest relative error among the
    groups carrying at least 1 % of the flux.
    """
    for s in run.get("normalized_spectra") or []:
        if s["component_id"] != component_id or s["particle"] != "neutron" or len(s["mean_per_square_metre_second"]) != GROUPS:
            continue
        means = s["mean_per_square_metre_second"]
        errors = s.get("standard_error_per_square_metre_second")
        if errors is None or len(errors) != GROUPS:
            raise InputError(f"{component_id}: the spectrum run has no per-group standard errors")
        total = sum(means)
        if not total > 0:
            raise InputError(f"{component_id}: the spectrum run's neutron spectrum has no flux")
        major = [e / m for m, e in zip(means, errors) if m > 0 and m >= 0.01 * total]
        return {
            "flux_weighted_mean_relative_error": sum(e for m, e in zip(means, errors) if m > 0) / total,
            "max_relative_error_groups_ge_1pct": max(major) if major else None,
            "groups_ge_1pct_of_flux": len(major),
        }
    raise InputError(f"no 709-group neutron spectrum for component {component_id} in the spectrum run")


def scaled_spectrum_from_run(spectrum_run: dict, spectrum_run_sha: str, component_id: str,
                             bounds: list[float], target_total_cm2: float) -> dict | None:
    """The spectrum run's shape, normalised and scaled so its sum is `target_total_cm2`."""
    found = find_709_spectrum(spectrum_run, component_id, bounds)
    if found is None:
        return None
    shape = found["flux_per_group"]
    run_total = sum(shape)
    if not run_total > 0:
        raise InputError(f"{component_id}: the spectrum run's neutron spectrum has no flux")
    scale = target_total_cm2 / run_total
    unit = [v / run_total for v in shape]
    return {
        **found,
        "flux_per_group": [u * target_total_cm2 for u in unit],
        "source": found["source"] + "; shape from --spectrum-run, scaled to the main run's component flux",
        "spectrum_run_sha256": spectrum_run_sha,
        "scale_factor_main_total_over_spectrum_run_total": scale,
        "spectrum_run_total_flux_cm2_s": run_total,
        "scaled_to_total_flux_cm2_s": target_total_cm2,
        "spectrum_relative_error": spectrum_error_summary(spectrum_run, component_id),
    }


def placeholder_spectrum(total_cm2: float, bounds: list[float]) -> list[float]:
    """Flat per unit lethargy over the 709 groups, scaled to the total flux. Not physics."""
    weights = [math.log(bounds[i + 1] / bounds[i]) for i in range(GROUPS)]
    scale = total_cm2 / sum(weights)
    return [w * scale for w in weights]


# --------------------------------------------------------------- schedule --

def history_end_s(history: dict) -> float:
    if history.get("outcome") == "horizon_completed":
        return float(history["assumptions"]["horizon_s"])
    return float(history["snapshots"][-1]["time_s"])


def operating_intervals(history: dict) -> list[tuple[float, float]]:
    """Source-on intervals from operation_started / operation_stopped events."""
    events = sorted(history["events"], key=lambda e: (e["time_s"], e["order"]))
    end = history_end_s(history)
    intervals, start = [], None
    for e in events:
        if e["kind"] == "operation_started":
            if start is not None:
                raise InputError(f"operation_started at {e['time_s']} while already operating")
            start = e["time_s"]
        elif e["kind"] == "operation_stopped":
            if start is None:
                raise InputError(f"operation_stopped at {e['time_s']} while not operating")
            if e["time_s"] > start:
                intervals.append((start, e["time_s"]))
            start = None
    if start is not None and end > start:
        intervals.append((start, end))
    return intervals


def installations(history: dict, component_id: str) -> list[tuple[float, float]]:
    """(install_s, remove_s) pairs: from 0 or replacement_completed to replacement_started or the end."""
    end = history_end_s(history)
    events = sorted(
        (e for e in history["events"]
         if e.get("component_id") == component_id and e["kind"] in ("replacement_started", "replacement_completed")),
        key=lambda e: (e["time_s"], e["order"]),
    )
    out, start = [], 0.0
    for e in events:
        if e["kind"] == "replacement_started":
            if start is None:
                raise InputError(f"{component_id}: replacement_started at {e['time_s']} while removed")
            out.append((start, e["time_s"]))
            start = None
        else:
            if start is not None:
                raise InputError(f"{component_id}: replacement_completed at {e['time_s']} while installed")
            start = e["time_s"]
    if start is not None:
        out.append((start, end))
    return out


class PowerSeries:
    """Snapshot power fractions, used to give each operating piece its multiplier."""

    def __init__(self, history: dict):
        snaps = history["snapshots"]
        self.times = [s["time_s"] for s in snaps]
        self.power = [s["power_fraction"] for s in snaps]
        self.operating = [s["operating"] for s in snaps]
        self.fallback = history["assumptions"].get("operation", [])

    def piece(self, a: float, b: float) -> tuple[float, float, int]:
        """(multiplier, max absolute deviation from the snapshot series, snapshots used) over [a, b).

        Rule: the time-weighted mean of the snapshot power fractions in [a, b),
        each snapshot holding until the next one (or b). When the series is
        constant the multiplier equals it with zero deviation. With no snapshot
        inside the piece, the authored operation window covering it is used.
        """
        lo, hi = bisect.bisect_left(self.times, a), bisect.bisect_left(self.times, b)
        idx = [i for i in range(lo, hi) if self.operating[i]]
        if not idx:
            for window in self.fallback:
                if window["start_s"] <= a and b <= window["end_s"]:
                    return float(window["power_fraction"]), 0.0, 0
            raise InputError(f"no power fraction for the operating piece [{a}, {b})")
        stamps = [self.times[i] for i in idx]
        weights = []
        for k, i in enumerate(idx):
            span_start = a if k == 0 else stamps[k]
            span_end = stamps[k + 1] if k + 1 < len(idx) else b
            weights.append((span_end - span_start, self.power[i]))
        total = sum(w for w, _ in weights)
        if total <= 0:
            return float(self.power[idx[0]]), 0.0, len(idx)
        mean = sum(w * p for w, p in weights) / total
        return mean, max(abs(self.power[i] - mean) for i in idx), len(idx)


def build_steps(intervals, power: PowerSeries, install_s: float, remove_s: float) -> tuple[list[tuple[float, float]], dict]:
    """Irradiation steps [(duration_s, multiplier)] for one installation, plus lumping facts.

    Operating intervals are clipped to the installation, scaled by their power
    multiplier; the time between them is a zero-flux step. Steps before merging
    are one per operating piece and per gap; adjacent steps with an identical
    multiplier are then merged exactly (no approximation).
    """
    raw, max_dev, pieces = [], 0.0, 0
    cursor = install_s
    for a, b in intervals:
        a, b = max(a, install_s), min(b, remove_s)
        if b <= a:
            continue
        if a > cursor:
            raw.append((a - cursor, 0.0))
        multiplier, deviation, _ = power.piece(a, b)
        raw.append((b - a, multiplier))
        max_dev, pieces, cursor = max(max_dev, deviation), pieces + 1, b
    if remove_s > cursor:
        raw.append((remove_s - cursor, 0.0))
    merged: list[list[float]] = []
    for dt, m in raw:
        if merged and merged[-1][1] == m:
            merged[-1][0] += dt
        else:
            merged.append([dt, m])
    steps = [(dt, m) for dt, m in merged]
    return steps, {
        "steps_before_merging": len(raw),
        "steps_after_merging": len(steps),
        "operating_pieces": pieces,
        "max_power_fraction_deviation_from_snapshots": max_dev,
    }


def cooling_steps(grid: list[tuple[str, float]]) -> list[tuple[float, float]]:
    """Zero-flux steps whose cumulative times are exactly the grid times."""
    steps, previous = [], 0.0
    for _, t in grid:
        steps.append((t - previous, 0.0))
        previous = t
    return steps


def subdivide_zero_flux(steps, grid: list[tuple[str, float]]) -> list[tuple[float, float]]:
    """Split every zero-flux step on the cooling grid, measured from the step's start.

    A step of length L becomes pieces ending at each grid time below L and at L
    itself, so ACTINV reports the decay curve inside the outage. Irradiation
    steps are unchanged. Cumulative times of the step ends are preserved.
    """
    out: list[tuple[float, float]] = []
    for dt, m in steps:
        if m != 0.0:
            out.append((dt, m))
            continue
        previous = 0.0
        for _, t in grid:
            if t >= dt * (1.0 - 1e-12):
                break
            out.append((t - previous, 0.0))
            previous = t
        out.append((dt - previous, 0.0))
    return out


def schedule_json(steps) -> list[dict]:
    return [{"dt": f"{float(dt)!r} s", "flux": float(m)} for dt, m in steps]


# ------------------------------------------------------------------- specs --

def build_spec(title, composition, mass_g, flux_per_group, library, schedule, outputs=None) -> dict:
    spec = {
        "spec": SPEC_FORMAT,
        "title": title,
        "projectile": "neutron",
        "library": {"path": f"catalog:{LIBRARY_ID}", "sha256": library["library_sha256"]},
        "decay": {"primary": "catalog:endfb-viii-0-decay", "fallback": "catalog:jeff-3-3-decay"},
        "material": {"mass_g": mass_g, "basis": "atom_fraction", "composition": composition},
        "spectrum": {
            "structure": "fispact-709",
            "flux_per_group": flux_per_group,
            "total": sum(flux_per_group),
            "descending": False,
        },
        "schedule": schedule,
        "options": {"mode": "auto", "prune": "rate", "bmin_atoms_per_g": 1e-8, "temperature_K": 293.6, "cram_order": 16},
    }
    if outputs is not None:
        spec["options"]["outputs"] = list(outputs)
    return spec


def write_json(path: Path, value) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def plan(args) -> dict:
    """Read and check every input; return the work list. Raises InputError / NoSpectrum."""
    run = load_json(args.run, "run record")
    scenario_hash = sha256_file(args.scenario)
    history = load_json(args.history, "history")
    physics = load_json(args.physics, "physics file")
    load_json(args.scenario, "scenario")
    if not args.allow_input_mismatch:
        for label, got in (("run record", run.get("scenario_sha256")),
                           ("history", history.get("driving_rates", {}).get("scenario_sha256"))):
            if got != scenario_hash:
                raise InputError(f"the {label} was made from a different scenario file (hash {got})")
        if history.get("driving_rates", {}).get("transport_artifact_sha256") != run.get("raw_artifact_sha256"):
            raise InputError("the history was not made from this run record's transport artifact")
    spectrum_run = None
    if args.spectrum_run:
        spectrum_run = load_json(args.spectrum_run, "spectrum run record")
        for field in ("scenario_sha256", "physics_sha256"):
            if spectrum_run.get(field) is None or spectrum_run.get(field) != run.get(field):
                raise InputError(f"the spectrum run's {field} differs from the main run's; refusing to mix runs")
    impurities_doc = load_json(args.impurities, "impurities file") if args.impurities else None
    library = library_info(Path(args.data_dir))
    materials = {m["id"]: m["recipe"] for m in physics["materials"]}
    assignment = {a["component_id"]: a["material_id"] for a in physics["component_assignments"]}
    solid = [c for c, m in assignment.items() if materials[m]["kind"] == "nuclide_mixture"]
    wanted = args.component or solid
    for c in wanted:
        if c not in assignment:
            raise InputError(f"component {c} is not in the physics file")
        if c not in solid:
            raise InputError(f"component {c} is void; nothing to activate")
    grid = parse_cooling_grid(args.cooling_grid)
    outputs = parse_outputs(args.actinv_outputs) if args.actinv_outputs else None
    intervals, power = operating_intervals(history), PowerSeries(history)
    items, missing = [], []
    for component in wanted:
        material_id = assignment[component]
        recipe = materials[material_id]
        bare = bare_composition(recipe)
        volume, volume_source = component_volume_m3(run, component)
        mass_g = volume * float(recipe["density_kg_m3"]) * 1000.0
        total_cm2, total_source = total_neutron_flux_cm2(run, component)
        if spectrum_run is not None:
            found = scaled_spectrum_from_run(spectrum_run, sha256_file(args.spectrum_run), component,
                                             library["bounds_eV"], total_cm2)
        else:
            found = find_709_spectrum(run, component, library["bounds_eV"])
        if found is None:
            missing.append(component)
            if not args.allow_placeholder_spectrum:
                continue
            found = {
                "flux_per_group": placeholder_spectrum(total_cm2, library["bounds_eV"]),
                "source": PLACEHOLDER,
                "scaled_to": f"{total_source} = {total_cm2:.6e} n cm^-2 s^-1",
                "zero_lower_edge_replaced_by_library_floor": False,
            }
        variants = [(BARE, bare, None)]
        if impurities_doc is not None:
            listed = impurities_doc.get(material_id)
            if listed:
                parsed = parse_impurities(listed, material_id)
                composition, report = composition_with_impurities(bare, parsed)
                variants.append((WITH_IMPURITIES, composition, {"entries": parsed, **report}))
        for number, (start, end) in enumerate(installations(history, component), start=1):
            steps, lumping = build_steps(intervals, power, start, end)
            if args.subdivide_outages:
                steps = subdivide_zero_flux(steps, grid)
                lumping = {**lumping, "outages_subdivided_on_cooling_grid": True, "steps_after_subdivision": len(steps)}
            items.append({
                "component": component, "material_id": material_id, "installation": number,
                "install_s": start, "remove_s": end, "steps": steps, "lumping": lumping,
                "mass_g": mass_g, "volume_m3": volume, "volume_source": volume_source,
                "density_kg_m3": float(recipe["density_kg_m3"]), "spectrum": found, "total_flux_cm2": total_cm2,
                "variants": variants,
            })
    if missing and not args.allow_placeholder_spectrum:
        raise NoSpectrum(NO_SPECTRUM_MESSAGE)
    return {"run": run, "library": library, "grid": grid, "items": items, "missing": missing, "outputs": outputs}


def hashes(args) -> dict:
    out = {
        "run_record": sha256_file(args.run),
        "scenario": sha256_file(args.scenario),
        "physics": sha256_file(args.physics),
        "history": sha256_file(args.history),
        "impurities": sha256_file(args.impurities) if args.impurities else None,
    }
    if args.spectrum_run:
        out["spectrum_run_record"] = sha256_file(args.spectrum_run)
    return out


def write_outputs(args, work: dict) -> list[Path]:
    out = Path(args.output_dir)
    out.mkdir(parents=True)
    library, grid, input_hashes = work["library"], work["grid"], hashes(args)
    cooling = cooling_steps(grid)
    specs = []
    for item in work["items"]:
        irradiation = item["steps"]
        steps = irradiation + cooling
        for label, composition, impurity_report in item["variants"]:
            stem = f"{item['component']}__inst{item['installation']:03d}__{label}"
            title = f"FARIS {item['component']} installation {item['installation']} ({label})"
            spec = build_spec(title, composition, item["mass_g"], item["spectrum"]["flux_per_group"],
                              library, schedule_json(steps), work["outputs"])
            spec_path = out / f"{stem}.spec.json"
            write_json(spec_path, spec)
            write_json(out / f"{stem}.provenance.json", {
                "script": "scripts/build_activation_inputs.py", "script_version": SCRIPT_VERSION,
                "spec_file": spec_path.name, "spec_sha256": sha256_file(spec_path),
                "label": label,
                **({"actinv_outputs": work["outputs"]} if work["outputs"] is not None else {}),
                "input_sha256": input_hashes,
                "component": item["component"], "material_id": item["material_id"],
                "installation_index": item["installation"],
                "installation_interval_s": [item["install_s"], item["remove_s"]],
                "mass_g": item["mass_g"], "volume_m3": item["volume_m3"], "volume_source": item["volume_source"],
                "density_kg_m3": item["density_kg_m3"],
                "composition_basis": "atom_fraction", "impurities": impurity_report,
                "spectrum": {
                    "source": item["spectrum"]["source"], "groups": GROUPS,
                    "total_flux_cm2_s": sum(item["spectrum"]["flux_per_group"]),
                    "details": {k: v for k, v in item["spectrum"].items() if k not in ("flux_per_group", "source")},
                },
                "schedule": {
                    **item["lumping"],
                    "lumping_rule": "operating piece = time-weighted mean of snapshot power_fraction; "
                                    "gaps are zero-flux; adjacent identical multipliers merged exactly",
                    "total_irradiation_time_s": sum(dt for dt, m in irradiation if m > 0),
                    "full_power_equivalent_time_s": sum(dt * m for dt, m in irradiation),
                    "irradiation_step_count": item["lumping"]["steps_after_merging"],
                    "cooling_grid": [{"label": lab, "cumulative_s": t} for lab, t in grid],
                    "year_s": YEAR_S, "total_step_count": len(steps),
                },
                "data_dir": str(Path(args.data_dir)), "library": {k: v for k, v in library.items() if k != "bounds_eV"},
            })
            specs.append(spec_path)
    return specs


def validate_specs(specs: list[Path], actinv: str, data_dir: str) -> list[dict]:
    env = dict(os.environ, ACTINV_DATA_DIR=str(Path(data_dir).resolve()))
    results = []
    for spec in specs:
        proc = subprocess.run([actinv, "validate", str(spec)], capture_output=True, text=True, env=env, check=False)
        results.append({"spec": spec.name, "ok": proc.returncode == 0,
                        "message": (proc.stdout + proc.stderr).strip()[-400:]})
    return results


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--run", required=True, help="FARIS transport run record JSON")
    p.add_argument("--scenario", required=True)
    p.add_argument("--physics", required=True)
    p.add_argument("--history", required=True, help="output of `faris history from-run`")
    p.add_argument("--spectrum-run", help="second run record (same scenario and physics) whose 709-group spectra "
                   "are normalised and scaled to --run's component flux")
    p.add_argument("--component", action="append", help="component id (repeatable; default all non-void)")
    p.add_argument("--data-dir", default=DEFAULT_DATA_DIR, help="ACTINV data root (the folder above v1.1.0)")
    p.add_argument("--impurities", help="JSON: material_id -> [{element|nuclide, wt_fraction|ppm, citation}]")
    p.add_argument("--cooling-grid", default=DEFAULT_COOLING)
    p.add_argument("--subdivide-outages", action="store_true",
                   help="split every zero-flux step on the cooling grid so the decay curve after each shutdown is in the run")
    p.add_argument("--actinv-outputs", help="comma-separated options.outputs for every spec (default: ACTINV's "
                   f"own default, every output); allowed: {', '.join(ACTINV_OUTPUTS)}")
    p.add_argument("--output-dir", required=True, help="new directory; must not exist")
    p.add_argument("--allow-placeholder-spectrum", action="store_true",
                   help="write a flat-lethargy placeholder when the run has no 709-group spectrum (plumbing tests only)")
    p.add_argument("--allow-input-mismatch", action="store_true", help="skip the scenario/artifact hash agreement checks")
    p.add_argument("--actinv", default=str(ACTINV_DEFAULT) if ACTINV_DEFAULT.exists() else (shutil.which("actinv") or "actinv"))
    p.add_argument("--skip-validate", action="store_true", help="do not run `actinv validate`")
    return p


def main(argv=None) -> int:
    args = parser().parse_args(argv)
    if Path(args.output_dir).exists():
        print(f"error: output directory {args.output_dir} already exists", file=sys.stderr)
        return 2
    try:
        work = plan(args)
    except NoSpectrum as err:
        print(str(err), file=sys.stderr)
        return 3
    except (InputError, KeyError) as err:
        print(f"error: {err!r}" if isinstance(err, KeyError) else f"error: {err}", file=sys.stderr)
        return 2
    specs = write_outputs(args, work)
    results = [] if args.skip_validate else validate_specs(specs, args.actinv, args.data_dir)
    failed = [r for r in results if not r["ok"]]
    write_json(Path(args.output_dir) / "manifest.json", {
        "script_version": SCRIPT_VERSION, "specs": [s.name for s in specs],
        "placeholder_spectrum_components": work["missing"], "validation": results,
    })
    print(f"wrote {len(specs)} specs to {args.output_dir}; validated {len(results) - len(failed)}/{len(results)}")
    for r in failed:
        print(f"validation FAILED: {r['spec']}: {r['message']}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
