#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Strict intake and MCNP-card mapping for the IAEA FNS-TOF benchmark."""
from __future__ import annotations

import csv
import hashlib
import math
import re
from pathlib import Path

SOURCE_COMMIT = "24b9efe3f2084a51eb1e83d9a08823ada5b9fe02"
SOURCE_URL = "https://github.com/IAEA-NDS/open-benchmarks"
BASE = "jade_open_benchmarks"
INPUTS = f"{BASE}/inputs/FNS-TOF"
EXPERIMENTS = f"{BASE}/exp_results/FNS-TOF"
LICENSE = "CC-BY-4.0"
ANGLES = (0.0, 12.2, 24.9, 41.8, 66.8)
LOWER_MEASUREMENT_LIMIT_MEV = 0.1
ASSEMBLIES = (
    "Be-5", "Be-15", "C-5", "C-20", "C-40", "Fe-5", "Fe-20", "Fe-40", "Fe-60",
    "N-20", "O-20", "Pb-5", "Pb-20", "Pb-40",
)


class IntakeError(ValueError):
    pass


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def finite(value: str, field: str) -> float:
    try:
        result = float(value)
    except (TypeError, ValueError) as exc:
        raise IntakeError(f"{field} must be numeric") from exc
    if not math.isfinite(result):
        raise IntakeError(f"{field} must be finite")
    return result


def input_deck(text: str) -> dict:
    """Extract exactly the source histogram, F5 locations/FM factors, and E0 tops.

    This is intentionally not a general MCNP parser. It rejects unsupported deck
    structures instead of silently guessing their meaning.
    """
    physical = [line.split("$", 1)[0].rstrip() for line in text.splitlines()]
    lines = []
    source_lines = []
    for line in physical:
        # MCNP continuation lines start in columns 1-5 with blanks. Preserve all
        # numeric data lines for source and energy cards; drop blank/comment lines.
        if not line.strip() or line.lstrip().startswith("c ") or line.lstrip() == "c":
            continue
        lines.append(line.strip())
        source_lines.append(line)

    cells = []
    surfaces = []
    data_start = None
    surface_block = False
    for i, line in enumerate(lines):
        if re.match(r"^mode\s+n\b", line, re.I):
            data_start = i
            break
        tokens = line.split()
        if tokens and tokens[0].isdigit():
            # MCNP permits a surface transformation number between the surface
            # id and mnemonic (e.g. `6 1 cz 5.235`). Cell material IDs and
            # densities are numeric, so recognize supported surface mnemonics
            # in the first three post-id columns instead of guessing by position.
            if any(token.lower() in {"pz", "cz", "so", "px", "py", "s", "cx", "cy", "sz"}
                   for token in tokens[1:4]):
                surface_block = True
                surfaces.append(line)
            elif not surface_block and cells and not source_lines[i][:5].strip():
                # In MCNP fixed-format cell blocks, a blank first five columns
                # denotes continuation. Preserve it on the preceding card so
                # importance/material identity cannot be lost or misread as a
                # second cell whose ID happens to be the first surface number.
                cells[-1] += " " + line
            else:
                cells.append(line)
    if data_start is None or not cells or not surfaces:
        raise IntakeError("deck does not contain the expected cell/surface/data blocks")
    sdef = [line for line in lines[data_start:] if re.match(r"^sdef\b", line, re.I)]
    if len(sdef) != 1:
        raise IntakeError("deck must have exactly one SDEF card")
    smatch = re.fullmatch(r"sdef\s+erg=d1\s+pos=([-+0-9.eE]+)\s+([-+0-9.eE]+)\s+([-+0-9.eE]+)", sdef[0], re.I)
    if not smatch:
        raise IntakeError("only the published point-source SDEF ERG=D1 POS=x y z form is supported")
    source_position = [finite(smatch.group(i), "SDEF point") for i in (1, 2, 3)]

    # MCNP vertical data cards use a leading # to declare multiple data-card
    # names whose paired data follow by column. See the pinned primary manual
    # cited by README.md. This is native SI1/SP1 syntax, not a reconstruction.
    cards: dict[str, list[list[float]]] = {}
    current = None
    source_table = False
    for line in lines[data_start:]:
        if re.match(r"^#\s*si1\s+sp1\s*$", line, re.I):
            if source_table or "si1" in cards:
                raise IntakeError("deck has duplicated or mixed SI1/SP1 source definitions")
            source_table = True
            cards["si1"], cards["sp1"] = [[]], [[]]
            current = "paired_source"
            continue
        if current == "paired_source":
            if re.match(r"^[a-z*][a-z0-9*]*\s", line, re.I):
                current = None
            else:
                values = [finite(v, "paired SI1/SP1 source table") for v in line.split()]
                if len(values) != 2:
                    raise IntakeError("every paired SI1/SP1 row must contain exactly two numbers")
                cards["si1"][0].append(values[0])
                cards["sp1"][0].append(values[1])
                continue
        match = re.match(r"^(si|sp)(\d+)\s*(?:([hla])\s*)?(.*)$", line, re.I)
        ematch = re.match(r"^e0\s*(.*)$", line, re.I)
        if match:
            key = f"{match.group(1).lower()}{match.group(2)}"
            option, tail = (match.group(3) or "h").lower(), match.group(4)
            if option != "h":
                raise IntakeError(f"{key} uses unsupported non-histogram option {option!r}")
            current = key
            cards[current] = [[]]
            if tail.strip():
                cards[current][0].extend(finite(v, key) for v in tail.split())
        elif ematch:
            current = "e0"
            cards[current] = [[]]
            if ematch.group(1).strip():
                cards[current][0].extend(finite(v, current) for v in ematch.group(1).split())
        elif current is not None:
            # New non-SI/SP/E0 cards terminate the current card's continuation.
            if re.match(r"^[a-z*][a-z0-9*]*\s", line, re.I):
                current = None
            else:
                cards[current][0].extend(finite(v, current) for v in line.split())

    if "si1" not in cards or "sp1" not in cards or "e0" not in cards:
        raise IntakeError("deck must define SI1, SP1 and E0")
    source_edges = cards["si1"][0]
    source_weights = cards["sp1"][0]
    energy_tops = cards["e0"][0]
    if len(source_edges) != len(source_weights) or len(source_edges) < 3:
        raise IntakeError("SI1/SP1 must have equal-length histogram boundary and probability arrays")
    if source_weights[0] != 0.0:
        raise IntakeError("MCNP H source histogram first-bin probability must be zero")
    if any(b <= a for a, b in zip(source_edges, source_edges[1:])):
        raise IntakeError("SI1 edges must increase strictly")
    if any(b <= a for a, b in zip(energy_tops, energy_tops[1:])):
        raise IntakeError("E0 bin tops must increase strictly")
    if any(w < 0 for w in source_weights):
        raise IntakeError("SP1 probabilities cannot be negative")
    total = math.fsum(source_weights[1:])
    if total <= 0 or not math.isfinite(total):
        raise IntakeError("SI1/SP1 nonzero histogram probabilities must have positive finite sum")

    points = {}
    labels = {}
    multipliers = {}
    for line in lines[data_start:]:
        match = re.match(r"^f(5|15|25|35|45):n\s+([-+0-9.eE]+)\s+([-+0-9.eE]+)\s+([-+0-9.eE]+)\s+1\s+nd\s*$", line, re.I)
        if match:
            tally = int(match.group(1))
            points[tally] = [finite(match.group(i), f"F{tally} position") for i in (2, 3, 4)]
            continue
        match = re.match(r"^fc(5|15|25|35|45)\s+.*?at\s+([0-9.]+)\s+degrees?\s*$", line, re.I)
        if match:
            labels[int(match.group(1))] = finite(match.group(2), "FC angle")
            continue
        match = re.match(r"^fm(5|15|25|35|45)\s+([-+0-9.eE]+)\s*$", line, re.I)
        if match:
            multipliers[int(match.group(1))] = finite(match.group(2), "FM")
    expected = {5, 15, 25, 35, 45}
    present = set(points)
    if not present or not present <= expected or set(multipliers) != present or set(labels) != present:
        raise IntakeError(f"deck must define matching supported point tallies, FC labels and FM cards; got {sorted(points)} / {sorted(labels)} / {sorted(multipliers)}")
    if len(energy_tops) < 2:
        raise IntakeError("E0 must contain at least two increasing boundaries")
    return {
        "source_histogram": {"edges_mev": source_edges, "bin_probabilities": source_weights,
                             "weight_sum": total, "first_zero_bin": True,
                             "SI_option": "H default; histogram bin boundaries",
                             "SP_option": "D default; bin probabilities which need not be normalized"},
        "source_position_cm": source_position,
        "fine_energy_upper_edges_mev": energy_tops,
        "detectors": [
            {"tally": t, "angle_deg": labels[t], "position_cm": points[t], "mcnp_fm": multipliers[t]}
            for t in sorted(present)
        ],
        "source_table_is_literal_input": True,
        "source_table_syntax": "MCNP vertical data card '# SI1 SP1' with paired columns",
        "cell_cards": cells,
        "surface_cards": surfaces,
    }


def read_rows(data: bytes, path: str) -> list[dict]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise IntakeError(f"{path} is not UTF-8 CSV") from exc
    rows = list(csv.DictReader(text.splitlines()))
    if not rows:
        raise IntakeError(f"{path} has no data rows")
    required = {"Value", "Error", "Energy", "Angle"}
    if not required <= set(rows[0]):
        raise IntakeError(f"{path} lacks required columns {sorted(required - set(rows[0]))}")
    result = []
    has_absolute_error = "abs err" in rows[0]
    for index, row in enumerate(rows):
        value = finite(row["Value"], f"{path} row {index} Value")
        error = finite(row["Error"], f"{path} row {index} Error")
        if value < 0 or error < 0:
            raise IntakeError(f"{path} row {index} has negative value or relative error")
        absolute = finite(row["abs err"], f"{path} row {index} abs err") if has_absolute_error else value * error
        if absolute < 0 or not math.isfinite(value * error):
            raise IntakeError(f"{path} row {index} has invalid absolute error")
        if has_absolute_error and not math.isclose(absolute, value * error, rel_tol=1e-5, abs_tol=1e-15):
            raise IntakeError(f"{path} row {index} absolute error does not match Value*Error")
        result.append({"value": value, "error_relative": error, "energy": row["Energy"],
                       "error_absolute": absolute,
                       "absolute_error_source": "abs err" if has_absolute_error else "Value*Error derived from source Error column",
                       "angle": row["Angle"], "row": index})
    return result


def energy_bins(rows: list[dict], fine_edges: list[float] | None, fine: bool) -> list[dict]:
    if fine:
        edges = [float(r["energy"]) for r in rows]
        if edges != fine_edges:
            raise IntakeError("fine CSV Energy entries do not match MCNP E0 edges exactly")
        bins = []
        for i, row in enumerate(rows):
            lo = 0.0 if i == 0 else edges[i - 1]
            hi = edges[i]
            bins.append({"lo_mev": lo, "hi_mev": hi, "edge_convention": "CSV Energy is the MCNP E0 upper edge; first bin is 0 to first edge"})
        return bins
    bins = []
    for row in rows:
        match = re.fullmatch(r"\s*([0-9.eE+-]+)\s*-\s*([0-9.eE+-]+)\s*", row["energy"])
        if not match:
            raise IntakeError(f"coarse energy range {row['energy']!r} is malformed")
        lo, hi = (finite(v, "coarse energy edge") for v in match.groups())
        if lo < 0 or hi <= lo:
            raise IntakeError(f"coarse energy range {row['energy']!r} is invalid")
        bins.append({"lo_mev": lo, "hi_mev": hi, "edge_convention": "CSV energy range bounds; response is per unit lethargy"})
    return bins


def tally_energy_edges(bins: list[dict]) -> list[float]:
    """Return OpenMC EnergyFilter edges, preserving gap and edge semantics."""
    edges = [bins[0]["lo_mev"]]
    for item in bins:
        if not math.isclose(edges[-1], item["lo_mev"], rel_tol=0, abs_tol=1e-12):
            raise IntakeError("response bins are not contiguous; cannot create one OpenMC energy filter")
        edges.append(item["hi_mev"])
    return edges


def lethargy_width(lo: float, hi: float) -> float:
    # JADE's pinned `by_lethargy` processing prepends 1e-10 MeV as its
    # documented numerical "zero" before taking adjacent-bin log ratios.
    if lo < 0 or hi <= 0 or hi <= lo:
        raise IntakeError("lethargy width needs nonnegative lower and greater positive upper energy edges")
    effective_lo = 1.0e-10 if lo == 0 else lo
    return math.log(hi / effective_lo)


def openmc_histogram(edges_mev: list[float], bin_weights: list[float]) -> dict:
    """Translate MCNP H-bin probabilities to OpenMC histogram PDF densities."""
    if len(edges_mev) != len(bin_weights) or len(edges_mev) < 3:
        raise IntakeError("source histogram needs matching edge and weight arrays with at least three entries")
    if any(not math.isfinite(x) for x in edges_mev) or any(b <= a for a, b in zip(edges_mev, edges_mev[1:])):
        raise IntakeError("source histogram edges must be finite and strictly increasing")
    if any(not math.isfinite(p) or p < 0 for p in bin_weights) or bin_weights[0] != 0:
        raise IntakeError("source histogram weights must be finite, nonnegative, and start with the MCNP zero bin")
    total = math.fsum(bin_weights[1:])
    if total <= 0:
        raise IntakeError("source histogram has zero probability")
    density = [p / total / (b - a) for a, b, p in zip(edges_mev[:-1], edges_mev[1:], bin_weights[1:], strict=True)]
    if not math.isclose(math.fsum(p * (b - a) for p, a, b in zip(density, edges_mev[:-1], edges_mev[1:], strict=True)), 1.0,
                        rel_tol=1e-12, abs_tol=1e-12):
        raise IntakeError("translated OpenMC source histogram does not integrate to one")
    # OpenMC Tabular requires x and p arrays of equal length. Under histogram
    # interpolation the final p entry has no associated interval and is unused.
    density_per_ev = [value / 1.0e6 for value in density]
    return {"edges_ev": [x * 1.0e6 for x in edges_mev], "pdf_per_ev": density_per_ev + [0.0],
            "interpolation": "histogram", "source_weight_sum": total,
            "translation": "MCNP SI1/SP1 paired table: skip zero-probability first bin; normalize SP weights; convert bin masses to per-eV densities"}


def fm_geometric_factor(detector: dict, source_position: list[float]) -> dict:
    """Cross-check the deck FM factor against the primary JADE L²/As relation."""
    point = detector["position_cm"]
    length = math.sqrt(math.fsum((point[i] - source_position[i]) ** 2 for i in range(3)))
    area_cm2 = 0.2304 * length - 84.16
    if length <= 0 or area_cm2 <= 0:
        raise IntakeError("detector flight length does not yield a positive JADE effective area")
    calculated = length * length / area_cm2
    relative_difference = abs(detector["mcnp_fm"] - calculated) / calculated
    return {"length_cm_from_SDEF_point": length, "effective_area_cm2_from_JADE_relation": area_cm2,
            "L2_over_As": calculated, "deck_FM": detector["mcnp_fm"],
            "relative_difference": relative_difference}


def qualify_halvings(levels: list[dict], tolerance: float = 0.01, max_rel_mcse: float = 0.02) -> dict:
    """Apply the frozen approximation gate to paired-radius per-bin run levels.

    A level is `{"radius_cm": r, "bins": {id: {"value": x, "u_mc": s}}}`.
    Qualification needs two consecutive half-radius transitions where every
    nonzero bin differs by <= 1% and each endpoint's relative MC standard error
    is <= 2%. Zero/rare bins never disappear; if their relative error is not
    bounded they make the transition inconclusive.
    """
    if len(levels) < 3:
        return {"qualified": False, "why": "three radii are required for two successive halvings"}
    passed = 0
    details = []
    for prior, current in zip(levels, levels[1:]):
        r0, r1 = prior["radius_cm"], current["radius_cm"]
        if not (r0 > 0 and math.isclose(r1 * 2.0, r0, rel_tol=1e-12, abs_tol=1e-15)):
            return {"qualified": False, "why": "radius sequence must halve exactly at every level", "transitions": details}
        b0, b1 = prior["bins"], current["bins"]
        if set(b0) != set(b1) or not b0:
            return {"qualified": False, "why": "all response bins must be present at each radius", "transitions": details}
        max_change, observed_rel_se, unresolved = 0.0, 0.0, []
        for key in sorted(b0):
            a, b = b0[key], b1[key]
            x0, x1 = float(a["value"]), float(b["value"])
            s0, s1 = float(a["u_mc"]), float(b["u_mc"])
            if not all(math.isfinite(v) and v >= 0 for v in (x0, x1, s0, s1)):
                raise IntakeError(f"{key}: radius-level estimates/errors must be finite and nonnegative")
            for v, s, label in ((x0, s0, "prior"), (x1, s1, "current")):
                if v == 0 or s / v > max_rel_mcse:
                    unresolved.append(f"{key} ({label} radius, relative MCSE unresolved)")
                else:
                    observed_rel_se = max(observed_rel_se, s / v)
            if x0 > 0 and x1 > 0:
                max_change = max(max_change, abs(x1 - x0) / max(abs(x0), abs(x1)))
        transition_ok = not unresolved and max_change <= tolerance
        details.append({"from_radius_cm": r0, "to_radius_cm": r1, "max_relative_change": max_change,
                        "max_relative_mcse_limit": max_rel_mcse,
                        "max_relative_mcse_observed": observed_rel_se if not unresolved else None,
                        "qualified": transition_ok,
                        "unresolved": unresolved})
        passed = passed + 1 if transition_ok else 0
    return {"qualified": passed >= 2, "transitions": details,
            "why": None if passed >= 2 else "two consecutive halvings did not satisfy all-bin change and MCSE limits"}
