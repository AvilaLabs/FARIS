#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Analysis driver for the maintenance-coupling test (docs/notes/MAINTENANCE_COUPLING_TEST.md).

Executes the fixed protocol: for each duration model (fixed, computed), each
split w and each case it runs `faris history from-run`, builds ACTINV specs with
outages subdivided on the cooling grid, runs `actinv run`, reads decay heat
versus time, derives replacement durations from the cooldown of the governing
components, iterates the coupling, and evaluates decisions D1-D4 and the
verdict by the protocol's rules. It refuses to run if the protocol file's
SHA-256 differs from the recorded one.

Config (JSON, paths relative to the config file):
  faris, actinv        CLI executables
  data_dir             ACTINV data root (as build_activation_inputs.py --data-dir)
  assumptions          demountable-magnet operating assumptions
  impurities           optional; adds the secondary impurity activation variant
  output_dir           new directory for every run
  result               output JSON (default references/maintenance-coupling-test.json)
  protocol             default docs/notes/MAINTENANCE_COUPLING_TEST.md in this repository
  arrangements         {"no-port/reference"|"no-port/breeder"|"port/reference"|"port/breeder":
                         {"scenario", "physics", "history_run", "spectrum_run"}}
  sweep                {"<blanket thickness label>": {"scenario", "physics", "history_run", "spectrum_run"}} (7 points)
                       history_run is the recorded run used for `faris history from-run` and as the main run for
                       activation (its component flux sets the scale); spectrum_run is the new 709-group run whose
                       spectrum shapes are passed as build_activation_inputs.py --spectrum-run
  allow_reduced_grid   optional; permit fewer sweep points / w / f values (tests only)
  amendment            optional; 2 applies Amendment 2 of the protocol (a full decay curve per event and governing
                       component from its own ACTINV run, 10 iterations, a pre-run equivalence check). Absent: the
                       first run's behaviour, unchanged.

  decay_cache          optional; folder of the content-addressed <sha>.points.json files (default
                       <output_dir>/decay-cache); shared between runs whose specs are byte-identical
  class_w              optional; {class: w} replacing the grid w for that class (calibration target, work time and
                       the cross-check's implied duration); each 0 < w < 1
  governing_quantity   optional; "heat" (default) or "dose": the quantity whose cooldown sets the durations. "dose"
                       calibrates q* on the contact gamma dose proxy and needs photon_response; the other quantity
                       is then the cross-check
  photon_response      optional; actinv-photon-response-1 JSON: every spec is built with --photon-response and
                       --actinv-outputs heat,dose. Absent: heat-only specs, unchanged.
  continuation_method  optional; "full" (default) runs every continuation spec as built, so a component installation's
                       whole history is repeated once per shutdown. "restart" (needs amendment 2) runs each
                       installation's irradiation history once (the trunk, heat only) and starts a short cooling run
                       per shutdown from the trunk's inventory at that shutdown; the curves are the same physics at a
                       fraction of the cost (scripts/restart_equivalence.py compares the two). Trunk results are
                       cached as <sha>.trunk.json beside the points files.
  An empty sweep ({}) with allow_reduced_grid runs no sweep case and D2 is NOT_EVALUATED.

`--equivalence-check-only` runs just the Amendment 2 equivalence check and prints its numbers.

Exit status: 0 result written, 2 bad input or refused.
"""
from __future__ import annotations

import argparse
import bisect
import codecs
import copy
import hashlib
import importlib.util
import json
import math
import os
import shutil
import subprocess
import sys
from collections import OrderedDict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SCRIPT_VERSION = "1"
SCHEMA = "faris-maintenance-coupling-result/v1"
REPO = Path(__file__).resolve().parent.parent
BUILD_SCRIPT = Path(__file__).with_name("build_activation_inputs.py")
# The protocol as committed (4359bd6), and its fixed body: everything before
# "## Amendments". Dated amendments may be appended; the body may not change.
PROTOCOL_SHA256 = "6a6c94e443b6ec578111644db88f5781c144d600c94dba37b7abd56bfd78a687"
PROTOCOL_BODY_SHA256 = "f287acd51542ffa4bd5d78eaad07912e35dacc6b4f63d14ce12616eb5a3440f2"
AMENDMENTS_HEADING = b"## Amendments"


def protocol_body_sha256(path: Path) -> str | None:
    """SHA-256 of the protocol text before its amendments heading, or None."""
    data = path.read_bytes()
    index = data.find(AMENDMENTS_HEADING)
    return hashlib.sha256(data[:index]).hexdigest() if index >= 0 else None
DEFAULT_PROTOCOL = REPO / "docs" / "notes" / "MAINTENANCE_COUPLING_TEST.md"
DEFAULT_RESULT = REPO / "references" / "maintenance-coupling-test.json"

DAY_S = 86400.0
COOLING_MIN_S = 3600.0
COOLING_MAX_S = 365.0 * DAY_S
COOLING_POINTS = 40
W_VALUES = (0.25, 0.5, 0.75)
CENTRAL_W = 0.5
F_VALUES = (0.5, 0.6, 0.7, 0.8, 0.9, 1.0)
MAX_ITERATIONS = 5
MAX_ITERATIONS_A2 = 10
AMENDMENT_2 = 2
EQUIVALENCE_TOL = 1e-6
EQUIVALENCE_OUTAGES = 3
EQUIVALENCE_COMPONENT = "first-wall"
CONVERGENCE_S = DAY_S
D1_GAP = 0.02
D2_GAP = 0.01
D3_BAND = (0.8, 1.25)
D3_MIN_FIXED_S = 30.0 * DAY_S
D4_GAIN = 0.01
D4_MAX_F = 0.9
TOL_S = 1.0
POINTS_SCHEMA = "faris-mct-actinv-points/v0.1"
RUN_MARKER = "run.json"
BARE = "bare_lower_bound"
WITH_IMPURITIES = "specification_maximum_impurities"
ARRANGEMENTS = ("no-port/reference", "no-port/breeder", "port/reference", "port/breeder")
CALIBRATION_CASE = "port/reference"
SWEEP_POINTS = 7
CLASSES = {
    "magnet": {"component_id": "magnets", "governing": ["first-wall", "blanket", "shield", "vessel"]},
    "blanket": {"component_id": "blanket", "governing": ["first-wall", "blanket"]},
}
BLANKET_CLASS = "blanket"
# The four 0.1 contrasts for D3: (label, minuend, subtrahend).
CONTRASTS = (
    ("breeder-minus-reference, no port", "no-port/breeder", "no-port/reference"),
    ("breeder-minus-reference, port", "port/breeder", "port/reference"),
    ("port-minus-no-port, reference", "port/reference", "no-port/reference"),
    ("port-minus-no-port, breeder", "port/breeder", "no-port/breeder"),
)


class Refused(Exception):
    """Bad input, or a run the protocol does not allow (exit status 2)."""


class ToolError(Exception):
    """An external tool failed."""


def sha256_file(path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_json(path, what):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as err:
        raise Refused(f"cannot read {what} {path}: {err}") from err


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


_BUILD = None


def build_module():
    """scripts/build_activation_inputs.py as a module, for its installation and schedule rules."""
    global _BUILD
    if _BUILD is None:
        spec = importlib.util.spec_from_file_location("build_activation_inputs", BUILD_SCRIPT)
        _BUILD = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(_BUILD)
    return _BUILD


def not_evaluated(reason: str, **extra) -> dict:
    return {"status": "NOT_EVALUATED", "reason": reason, **extra}


# ------------------------------------------------------------ cooling curves --

def cooling_grid_s() -> list[float]:
    """40 log-spaced times, 1 hour to 365 days, endpoints exact."""
    ratio = COOLING_MAX_S / COOLING_MIN_S
    grid = [COOLING_MIN_S * ratio ** (i / (COOLING_POINTS - 1)) for i in range(COOLING_POINTS)]
    grid[0], grid[-1] = COOLING_MIN_S, COOLING_MAX_S
    return grid


def interpolate(points: list[tuple[float, float]], t: float) -> float:
    """Value at t between sorted (t, value) points: linear in ln t, and in ln value when both ends are positive."""
    times = [p[0] for p in points]
    i = min(max(bisect.bisect_right(times, t) - 1, 0), len(points) - 2)
    (t0, v0), (t1, v1) = points[i], points[i + 1]
    if t <= t0:
        return v0
    if t >= t1:
        return v1
    frac = (math.log(t) - math.log(t0)) / (math.log(t1) - math.log(t0))
    if v0 > 0 and v1 > 0:
        return math.exp(math.log(v0) + frac * (math.log(v1) - math.log(v0)))
    return v0 + frac * (v1 - v0)


def combined_curve(series: dict, weights: dict, max_s: float = COOLING_MAX_S) -> list:
    """Curve of the governing set versus time since shutdown.

    series: {component: [(seconds since shutdown, value)]}, sorted, seconds > 0.
    weights: {component: volume_m3}. The curve is sum of values / sum of volumes.
    Defined on the common time range of every component, up to max_s.
    """
    if not series:
        raise ValueError("no components")
    lows = [s[0][0] for s in series.values()]
    highs = [s[-1][0] for s in series.values()]
    lo, hi = max(lows), min(min(highs), max_s * (1 + 1e-9))
    times = sorted({t for s in series.values() for t, _ in s if lo * (1 - 1e-12) <= t <= hi * (1 + 1e-12)})
    if not times:
        raise ValueError("components share no cooling-time range")
    total_volume = sum(weights[c] for c in series)
    curve = []
    for t in times:
        curve.append((t, sum(interpolate(series[c], t) for c in series) / total_volume))
    return curve


def cooldown(curve: list, qstar: float, max_s: float = COOLING_MAX_S) -> dict:
    """Time until the curve first reaches or falls below q* (log-linear between points).

    No crossing and the curve reaches max_s: NOT_EVALUATED. No crossing and the
    curve ends earlier (the outage after the shutdown is shorter than that):
    the window length, flagged window_limited, a lower bound.
    """
    if not curve:
        return not_evaluated("no decay curve")
    if curve[0][1] <= qstar:
        return {"status": "EVALUATED", "cooldown_s": curve[0][0], "window_limited": False,
                "note": "at or below q* at the first sample; cooldown is that sample time (the grid starts at 1 hour)"}
    for (t0, q0), (t1, q1) in zip(curve, curve[1:]):
        if q1 <= qstar:
            if q0 > 0 and q1 > 0 and qstar > 0:
                frac = (math.log(qstar) - math.log(q0)) / (math.log(q1) - math.log(q0))
                t = math.exp(math.log(t0) + frac * (math.log(t1) - math.log(t0)))
            else:
                t = t0 + (q0 - qstar) / (q0 - q1) * (t1 - t0)
            return {"status": "EVALUATED", "cooldown_s": t, "window_limited": False}
    end = curve[-1][0]
    if end >= max_s * (1 - 1e-9):
        return not_evaluated("decay curve never falls below q* within 365 days", curve_end_s=end)
    return {"status": "EVALUATED", "cooldown_s": end, "window_limited": True,
            "note": "no crossing inside the outage window; cooldown is at least the window length"}


def calibrate(curve: list, target_cooldown_s: float) -> dict:
    """q* = the curve value at the target cooldown; the cooldown it gives must come back."""
    if not curve:
        return not_evaluated("no decay curve at the calibration event")
    if not curve[0][0] * (1 - 1e-9) <= target_cooldown_s <= curve[-1][0] * (1 + 1e-9):
        return not_evaluated(
            f"calibration cooldown {target_cooldown_s / DAY_S:.3f} d is outside the curve "
            f"[{curve[0][0] / DAY_S:.3f}, {curve[-1][0] / DAY_S:.3f}] d")
    qstar = interpolate(curve, target_cooldown_s)
    back = cooldown(curve, qstar)
    if back["status"] != "EVALUATED" or abs(back["cooldown_s"] - target_cooldown_s) > max(TOL_S, 1e-6 * target_cooldown_s):
        return not_evaluated("decay curve is not monotone at the calibration event; q* does not reproduce the target cooldown",
                             q_star_candidate=qstar)
    return {"status": "EVALUATED", "q_star": qstar, "target_cooldown_s": target_cooldown_s}


# ------------------------------------------------------------------ history --

def history_end_s(history: dict) -> float:
    if history.get("outcome") == "horizon_completed":
        return float(history["assumptions"]["horizon_s"])
    return float(history["snapshots"][-1]["time_s"])


def replacement_events(history: dict, classes: dict) -> list[dict]:
    """Replacements per class in time order, k counted per component: {class, component, k, start_s, end_s|None}."""
    out = []
    for cls, spec in classes.items():
        component = spec["component_id"]
        events = sorted((e for e in history["events"] if e.get("component_id") == component
                         and e["kind"] in ("replacement_started", "replacement_completed")),
                        key=lambda e: (e["time_s"], e["order"]))
        k, open_start = 0, None
        for e in events:
            if e["kind"] == "replacement_started":
                k += 1
                open_start = {"class": cls, "component": component, "k": k, "start_s": float(e["time_s"]), "end_s": None}
                out.append(open_start)
            elif open_start is not None:
                open_start["end_s"] = float(e["time_s"])
                open_start = None
    return sorted(out, key=lambda e: (e["start_s"], e["component"]))


def total_downtime_s(history: dict, classes: dict) -> float:
    end = history_end_s(history)
    return sum(((e["end_s"] if e["end_s"] is not None else end) - e["start_s"])
               for e in replacement_events(history, classes))


def lifetime_net_mwh(history: dict):
    last = history["snapshots"][-1]
    return last.get("cumulative_net_electricity_mwh")


def fixed_durations_s(assumptions: dict, classes: dict) -> dict:
    out = {}
    for cls, spec in classes.items():
        values = {lim.get("replacement_duration_s") for lim in assumptions["service_limits"]
                  if lim["component_id"] == spec["component_id"] and lim.get("class") == "replaceable"}
        if len(values) != 1 or None in values:
            raise Refused(f"component {spec['component_id']} needs one replacement_duration_s over its replaceable limits")
        out[cls] = float(values.pop())
    return out


def derive_assumptions(base: dict, classes: dict, f: float, used: dict) -> dict:
    """Base assumptions with the blanket limit scaled by f and replacement_durations_s per component."""
    derived = copy.deepcopy(base)
    blanket = classes[BLANKET_CLASS]["component_id"]
    for lim in derived["service_limits"]:
        if lim["component_id"] == blanket and f != 1.0:
            lim["limit"] = lim["limit"] * f
        durations = used.get(lim["component_id"])
        if durations:
            lim["replacement_durations_s"] = list(durations)
    return derived


# ------------------------------------------------------------------- ACTINV --

RESULT_KEPT = ("ms", "pruned_states", "total_states")
READ_CHUNK = 8 << 20


def point_of(step: dict) -> list:
    return [step["t_s"], step["heat_W_per_g"]["total"], step.get("flux", 0.0),
            (step.get("photon_source") or {}).get("contact_gamma_air_dose_proxy_Gy_h")]


def scan_result(path: Path, chunk: int = READ_CHUNK, keep_steps: set | None = None) -> dict:
    """Read an ACTINV result one top-level member and one step at a time.

    With photon outputs a long schedule's result runs to gigabytes, far beyond what json.loads can hold, and only
    four numbers per step are kept. Values are decoded by the same json decoder, so they equal json.loads's.
    Returns the kept top-level members, the step points, and the file's SHA-256 and size. With keep_steps (0-based
    indices into the result's steps) it also returns "kept_steps", {index: the whole step} for those steps.
    """
    decoder = json.JSONDecoder()
    digest, size = hashlib.sha256(), 0
    text = codecs.getincrementaldecoder("utf-8")()
    state = {"buf": "", "pos": 0, "eof": False}
    fh = open(path, "rb")

    def more() -> bool:
        nonlocal size
        if state["eof"]:
            return False
        data = fh.read(chunk)
        digest.update(data)
        size += len(data)
        if not data:
            state["eof"] = True
            state["buf"] += text.decode(b"", final=True)
            return False
        if state["pos"] > chunk:
            state["buf"], state["pos"] = state["buf"][state["pos"]:], 0
        state["buf"] += text.decode(data)
        return True

    def peek() -> str:
        while True:
            buf, pos = state["buf"], state["pos"]
            while pos < len(buf) and buf[pos] in " \t\r\n":
                pos += 1
            state["pos"] = pos
            if pos < len(buf):
                return buf[pos]
            if not more():
                raise ValueError(f"{path}: result ends early")

    def expect(char: str) -> None:
        if peek() != char:
            raise ValueError(f"{path}: expected {char!r} at character {state['pos']}")
        state["pos"] += 1

    def value():
        peek()
        while True:
            try:
                got, end = decoder.raw_decode(state["buf"], state["pos"])
                # a number cut by the buffer's end decodes short (4987. -> 4987); accept a value only when a
                # delimiter follows it, which a cut number never has
                if (end < len(state["buf"]) and state["buf"][end] in " \t\r\n,]}:") or state["eof"]:
                    state["pos"] = end
                    return got
            except json.JSONDecodeError:
                pass
            if not more():
                got, end = decoder.raw_decode(state["buf"], state["pos"])
                state["pos"] = end
                return got

    kept, points, n_steps, kept_steps = {}, [], 0, {}
    try:
        expect("{")
        while peek() != "}":
            key = value()
            expect(":")
            if key == "steps":
                expect("[")
                while peek() != "]":
                    step = value()
                    points.append(point_of(step))
                    if keep_steps is not None and n_steps in keep_steps:
                        kept_steps[n_steps] = step
                    n_steps += 1
                    if peek() == ",":
                        state["pos"] += 1
                state["pos"] += 1
            elif key in RESULT_KEPT:
                kept[key] = value()
            else:
                value()
            if peek() == ",":
                state["pos"] += 1
        state["pos"] += 1
        while more():
            pass
        if state["buf"][state["pos"]:].strip():
            raise ValueError(f"{path}: text after the result")
    finally:
        fh.close()
    got = {"kept": kept, "points": points, "n_steps": n_steps, "sha256": digest.hexdigest(), "bytes": size}
    if keep_steps is not None:
        got["kept_steps"] = kept_steps
    return got


def write_points(result_path: Path, points_path: Path) -> None:
    """Compact the full ACTINV result into a points file (atomically), then delete the result.

    A heat-only result can still be hundreds of MB for a long schedule, and one with photon outputs gigabytes, so it
    is read a step at a time (scan_result) and not kept.
    """
    got = scan_result(result_path)
    doc = {
        "schema": POINTS_SCHEMA,
        "result_sha256": got["sha256"], "result_bytes": got["bytes"],
        "n_steps": got["n_steps"],
        "ms": got["kept"].get("ms"),
        "pruned_states": got["kept"].get("pruned_states"),
        "total_states": got["kept"].get("total_states"),
        "steps": got["points"],
    }
    tmp = points_path.with_name(points_path.name + ".tmp")
    tmp.write_text(json.dumps(doc) + "\n", encoding="utf-8")
    os.replace(tmp, points_path)
    result_path.unlink()


def read_activation(spec_dir: Path, variant: str) -> dict:
    """{component: [{install_s, remove_s, volume_m3, points: [(abs_s, heat_W, dose|None, flux)]}]} for one variant."""
    data: dict = {}
    for prov_path in sorted(spec_dir.glob(f"*__{variant}.provenance.json")):
        prov = json.loads(prov_path.read_text(encoding="utf-8"))
        if "continuation_of_shutdown_s" in prov:
            continue
        stored = json.loads((spec_dir / prov["spec_file"].replace(".spec.json", ".points.json")).read_text(encoding="utf-8"))
        install_s, remove_s = prov["installation_interval_s"]
        points = []
        for t_s, heat_per_g, flux, dose in stored["steps"]:
            points.append((install_s + t_s, heat_per_g * prov["mass_g"], dose, flux))
        data.setdefault(prov["component"], []).append({
            "install_s": install_s, "remove_s": remove_s, "volume_m3": prov["volume_m3"], "points": points,
            "library": prov.get("library", {}),
        })
    return data


# ------------------------------------------------------ restart continuation --

TRUNK_SCHEMA = "faris-mct-actinv-trunk/v0.1"
CONTINUATION_METHODS = ("full", "restart")
ELEMENT_SYMBOLS = (
    "H He Li Be B C N O F Ne Na Mg Al Si P S Cl Ar K Ca Sc Ti V Cr Mn Fe Co Ni Cu Zn Ga Ge As Se Br Kr "
    "Rb Sr Y Zr Nb Mo Tc Ru Rh Pd Ag Cd In Sn Sb Te I Xe Cs Ba La Ce Pr Nd Pm Sm Eu Gd Tb Dy Ho Er Tm Yb Lu "
    "Hf Ta W Re Os Ir Pt Au Hg Tl Pb Bi Po At Rn Fr Ra Ac Th Pa U Np Pu Am Cm Bk Cf Es Fm Md No Lr "
    "Rf Db Sg Bh Hs Mt Ds Rg Cn Nh Fl Mc Lv Ts Og").split()
assert len(ELEMENT_SYMBOLS) == 118


def nuclide_key(z: int, a: int, liso: int) -> str:
    """ACTINV composition key: symbol + mass number, plus m<LISO> for an isomeric state (LISO >= 1)."""
    if not 1 <= z <= len(ELEMENT_SYMBOLS):
        raise ToolError(f"no element symbol for Z = {z}")
    return f"{ELEMENT_SYMBOLS[z - 1]}{a}" + (f"m{liso}" if liso >= 1 else "")


def schedule_end_times(schedule: list) -> list:
    """Cumulative seconds at the end of every schedule step."""
    out, cursor = [], 0.0
    for step in schedule:
        cursor += float(step["dt"].split()[0])
        out.append(cursor)
    return out


def restart_groups(provs: list, spec_dir: Path, n_cooling: int) -> list:
    """Group continuation specs by (component, installation) into a trunk and the shutdowns it serves.

    The trunk is the latest continuation's spec with its cooling tail dropped and only heat requested. Every other
    spec of the group must equal it apart from the title and the schedule, and its irradiation schedule must be a
    prefix of the trunk's; the trunk step that ends that prefix gives the inventory at its shutdown.
    """
    grouped: dict = {}
    for prov in provs:
        if "continuation_of_shutdown_s" not in prov:
            raise ToolError(f"{prov.get('spec_file')}: not a continuation spec")
        grouped.setdefault((prov["component"], prov["installation_index"]), []).append(prov)
    groups = []
    for (component, installation), members in sorted(grouped.items()):
        members = sorted(members, key=lambda p: p["continuation_of_shutdown_s"])
        loaded = []
        for prov in members:
            spec = json.loads((spec_dir / prov["spec_file"]).read_text(encoding="utf-8"))
            n_irr = prov["schedule"]["irradiation_step_count"]
            if n_irr < 1 or len(spec["schedule"]) != n_irr + n_cooling:
                raise ToolError(f"{prov['spec_file']}: the schedule is not {n_irr} irradiation steps plus the "
                                f"{n_cooling}-step cooling grid")
            loaded.append((prov, spec, n_irr))
        last_prov, last_spec, last_n = loaded[-1]
        trunk_schedule = last_spec["schedule"][:last_n]
        rest = {k: v for k, v in last_spec.items() if k not in ("schedule", "title")}
        shutdowns = []
        for prov, spec, n_irr in loaded:
            if {k: v for k, v in spec.items() if k not in ("schedule", "title")} != rest:
                raise ToolError(f"{prov['spec_file']}: differs from {last_prov['spec_file']} outside its schedule; "
                                "cannot restart from a shared trunk")
            if spec["schedule"][:n_irr] != trunk_schedule[:n_irr]:
                raise ToolError(f"{prov['spec_file']}: its irradiation schedule is not a prefix of "
                                f"{last_prov['spec_file']}'s")
            shutdowns.append({"prov": prov, "spec": spec, "step_index": n_irr - 1})
        trunk = copy.deepcopy(last_spec)
        trunk["title"] = f"FARIS {component} installation {installation} restart trunk"
        trunk["schedule"] = trunk_schedule
        trunk.setdefault("options", {})["outputs"] = ["heat"]
        groups.append({"component": component, "installation": installation, "trunk": trunk,
                       "trunk_file": last_prov["spec_file"].replace(".spec.json", ".trunk.spec.json"),
                       "shutdowns": shutdowns})
    return groups


def trunk_step_record(step: dict) -> dict:
    return {"t_s": step["t_s"],
            "inventory": [{"Z": e["Z"], "A": e["A"], "LISO": e["LISO"], "atoms_per_g": e["atoms_per_g"]}
                          for e in step["inventory"]],
            "n_states_below_floor": step.get("n_states_below_floor"),
            "heat_bound_from_below_floor_W_per_g": step.get("heat_bound_from_below_floor_W_per_g")}


def cooling_spec(spec: dict, record: dict, n_cooling: int) -> dict:
    """The continuation spec restarted from a trunk step: its inventory as the material, only the cooling tail."""
    composition = {}
    for e in sorted(record["inventory"], key=lambda e: (e["Z"], e["A"], e["LISO"])):
        if e["atoms_per_g"] > 0:
            composition[nuclide_key(e["Z"], e["A"], e["LISO"])] = e["atoms_per_g"]
    if not composition:
        raise ToolError(f"{spec['title']}: the trunk inventory is empty at the shutdown")
    out = copy.deepcopy(spec)
    out["material"] = {"mass_g": spec["material"]["mass_g"], "basis": "atoms_per_g", "composition": composition}
    out["schedule"] = spec["schedule"][-n_cooling:]
    # Zero flux puts ACTINV's automatic mode into trace activation, which holds the initial material constant; the
    # restart material is the radioactive inventory itself, so it must evolve.
    out["options"] = {**spec["options"], "mode": "coupled"}
    return out


def cooling_curve(stored: dict, grid: list, mass_g: float, name: str, exact: bool = False) -> list:
    """[(tau, heat_W, dose|None, 0.0)] from the last len(grid) steps of a points file; the steps must be zero-flux and
    span the cooling grid (exact: and be nothing else)."""
    tail = stored["steps"][-len(grid):]
    span = tail[-1][0] - tail[0][0]
    if (len(tail) != len(grid) or any(s[2] != 0 for s in tail)
            or abs(span - (grid[-1] - grid[0])) > 1e-6 * grid[-1] or (exact and len(stored["steps"]) != len(grid))):
        raise ToolError(f"{name}: the last {len(grid)} steps are not the cooling grid")
    return [(tau, s[1] * mass_g, s[3], 0.0) for tau, s in zip(grid, tail)]


class DecayCurves:
    """Amendment 2: per (event start, governing component), the cooling part of that component installation's
    continuation run: {"volume_m3", "points": [(seconds since the shutdown, heat_W, dose|None, flux)]}."""

    def __init__(self, curves: dict | None = None):
        self.curves = curves or {}


def missing_dose(component: str, points: list, tau: float) -> dict:
    """NOT_EVALUATED for a point without a contact dose: no dose anywhere means heat-only specs, else a gap."""
    if all(p[2] is None for p in points):
        return not_evaluated(f"{component} has no contact gamma dose: its ACTINV specs have no photon "
                             "response and run with heat-only outputs")
    return not_evaluated(f"{component} has no contact gamma dose at {tau:.0f} s after the shutdown: "
                         "the ACTINV result lacks the value at that point")


def event_series(activation, event: dict, governing: list, quantity: str):
    """({component: [(tau, value)]}, {component: volume}) or a NOT_EVALUATED dict."""
    series, volumes = {}, {}
    if isinstance(activation, DecayCurves):
        for component in governing:
            entry = activation.curves.get((event["start_s"], component))
            if entry is None:
                return not_evaluated(f"no ACTINV continuation run of {component} for the shutdown at {event['start_s']:.0f} s")
            pts = []
            for tau, heat, dose, _flux in entry["points"]:
                value = heat if quantity == "heat" else (None if dose is None else dose * entry["volume_m3"])
                if value is None:
                    return missing_dose(component, entry["points"], tau)
                pts.append((tau, value))
            series[component] = pts
            volumes[component] = entry["volume_m3"]
        return series, volumes
    for component in governing:
        installs = [i for i in activation.get(component, [])
                    if i["install_s"] < event["start_s"] - TOL_S + 1e-9 and event["start_s"] <= i["remove_s"] + TOL_S]
        if not installs:
            return not_evaluated(f"no ACTINV run of {component} covers the shutdown at {event['start_s']:.0f} s")
        inst = installs[-1]
        pts = []
        for t, heat, dose, flux in inst["points"]:
            tau = t - event["start_s"]
            if tau <= TOL_S:
                continue
            if flux > 0 or tau > COOLING_MAX_S * (1 + 1e-9):
                break  # irradiation resumed (or a year passed): the decay curve after the shutdown ends here
            value = heat if quantity == "heat" else (None if dose is None else dose * inst["volume_m3"])
            if value is None:
                return missing_dose(component, inst["points"], tau)
            pts.append((tau, value))
        if not pts:
            return not_evaluated(f"{component} has no decay points after the shutdown at {event['start_s']:.0f} s")
        series[component] = pts
        volumes[component] = inst["volume_m3"]
    return series, volumes


def event_curve(activation: dict, event: dict, governing: list, quantity: str = "heat"):
    got = event_series(activation, event, governing, quantity)
    if isinstance(got, dict):
        return got
    series, volumes = got
    try:
        # heat is W: the sum over the set divided by the total volume is W/m^3. Dose series carry dose x volume,
        # so the same division is the volume-weighted mean contact dose.
        return combined_curve(series, volumes)
    except ValueError as err:
        return not_evaluated(str(err))


# ------------------------------------------------------------------- runner --

CACHE_ENTRIES = 8  # histories are ~20 MB of JSON each; the rest are re-read from disk


def cache_get(cache: OrderedDict, key):
    if key in cache:
        cache.move_to_end(key)
        return cache[key]
    return None


def cache_put(cache: OrderedDict, key, value) -> None:
    cache[key] = value
    cache.move_to_end(key)
    while len(cache) > CACHE_ENTRIES:
        cache.popitem(last=False)


def complete_spec_dir(spec_dir: Path) -> bool:
    """build_activation_inputs.py writes manifest.json last; a folder without it, or whose
    validation did not all pass, is an interrupted build."""
    manifest = spec_dir / "manifest.json"
    if not manifest.is_file():
        return False
    try:
        record = json.loads(manifest.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return False
    return all(r.get("ok") for r in record.get("validation", [])) and not record.get("placeholder_spectrum_components")


class Runner:
    """Runs histories and activation for (case, f, durations), keeping a few in memory.

    Everything is also on disk under the output directory, so an interrupted run resumes:
    a history is reused when its assumptions file is byte-identical to the derived one,
    and an activation folder when its build completed; ACTINV runs resume from points files.
    """

    def __init__(self, cfg: dict, out_dir: Path, base_assumptions: dict, classes: dict):
        self.cfg, self.out, self.base, self.classes = cfg, out_dir, base_assumptions, classes
        self._histories: OrderedDict = OrderedDict()
        self._activations: OrderedDict = OrderedDict()
        self.library: dict = {}
        self.amend2 = cfg.get("amendment") == AMENDMENT_2
        self.max_iterations = MAX_ITERATIONS_A2 if self.amend2 else MAX_ITERATIONS
        self._cases: dict = {}
        self.continuation_runs = 0
        self.actinv_workers = 1  # concurrent ACTINV runs for decay continuations; set by --actinv-workers
        self.continuation_cache_hits = 0
        self.continuation_method = cfg.get("continuation_method") or "full"
        self.trunk_runs = self.trunk_cache_hits = 0
        self.restart_cooling_runs = self.restart_cooling_cache_hits = 0
        self.restart_info: dict = {}  # (component, installation, shutdown_s) -> the trunk's below-floor figures
        self.decay_cache = Path(cfg["decay_cache"]) if cfg.get("decay_cache") else out_dir / "decay-cache"
        self.class_w = dict(cfg.get("class_w") or {})
        self.governing = cfg.get("governing_quantity") or "heat"
        self.photon_response = cfg.get("photon_response")

    def _photon_argv(self) -> list:
        """Builder arguments for the photon response: none for a heat-only run, so its specs are unchanged."""
        if not self.photon_response:
            return ["--actinv-outputs", "heat"]
        return ["--photon-response", self.photon_response, "--actinv-outputs", "heat,dose"]

    def _dir(self, case_name: str, f: float, used: dict) -> Path:
        key = hashlib.sha256(json.dumps(used, sort_keys=True).encode()).hexdigest()[:10]
        slug = case_name.replace("/", "__")
        return self.out / "cases" / slug / f"f{f}" / ("fixed" if not used else f"d{key}")

    def _run(self, argv: list, what: str, env=None):
        proc = subprocess.run([str(a) for a in argv], capture_output=True, text=True, env=env, check=False)
        if proc.returncode != 0:
            raise ToolError(f"{what} failed (exit {proc.returncode}): {(proc.stdout + proc.stderr).strip()[-600:]}")
        return proc

    def history(self, case_name: str, case: dict, f: float, used: dict) -> dict:
        key = (case_name, f, json.dumps(used, sort_keys=True))
        cached = cache_get(self._histories, key)
        if cached is not None:
            return cached
        folder = self._dir(case_name, f, used)
        derived = derive_assumptions(self.base, self.classes, f, used)
        derived_text = json.dumps(derived, indent=2, sort_keys=True) + "\n"
        assumptions_path = folder / "assumptions.json"
        history_path = folder / "history.json"
        data = None
        if (assumptions_path.is_file() and history_path.is_file()
                and assumptions_path.read_text(encoding="utf-8") == derived_text):
            try:
                data = json.loads(history_path.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                data = None  # interrupted write: recalculate
        if data is None:
            folder.mkdir(parents=True, exist_ok=True)
            history_path.unlink(missing_ok=True)
            assumptions_path.write_text(derived_text, encoding="utf-8")
            self._run([self.cfg["faris"], "history", "from-run", "--scenario", case["scenario"],
                       "--run", case["history_run"], "--assumptions", assumptions_path, "--output", history_path],
                      f"faris history from-run ({case_name})")
            data = json.loads(history_path.read_text(encoding="utf-8"))
        entry = {"dir": folder, "assumptions": assumptions_path, "history_path": history_path, "data": data}
        cache_put(self._histories, key, entry)
        return entry

    def activation(self, case_name: str, case: dict, f: float, used: dict, variant: str) -> dict:
        key = (case_name, f, json.dumps(used, sort_keys=True), variant)
        cached = cache_get(self._activations, key)
        if cached is not None:
            return cached
        if self.amend2:
            act = self.decay_curves(case_name, case, f, used, variant)
            cache_put(self._activations, key, act)
            return act
        hist = self.history(case_name, case, f, used)
        spec_dir = hist["dir"] / "activation"
        if spec_dir.exists() and not complete_spec_dir(spec_dir):
            shutil.rmtree(spec_dir)
        if not spec_dir.exists():
            argv = [sys.executable, BUILD_SCRIPT, "--run", case["history_run"], "--spectrum-run", case["spectrum_run"],
                    "--scenario", case["scenario"],
                    "--physics", case["physics"], "--history", hist["history_path"],
                    "--data-dir", self.cfg["data_dir"], "--cooling-grid",
                    ",".join(f"{t!r}s" for t in cooling_grid_s()), "--subdivide-outages",
                    *self._photon_argv(), "--actinv", self.cfg["actinv"], "--output-dir", spec_dir]
            if self.cfg.get("impurities"):
                argv += ["--impurities", self.cfg["impurities"]]
            self._run(argv, f"build_activation_inputs ({case_name})")
        self.run_specs(spec_dir, variant)
        act = read_activation(spec_dir, variant)
        for installs in act.values():
            if installs and installs[0].get("library") and not self.library:
                self.library = installs[0]["library"]
        cache_put(self._activations, key, act)
        return act

    def run_specs(self, spec_dir: Path, variant: str) -> None:
        """One ACTINV run per spec that has no points file yet; each leaves only its points file."""
        env = dict(os.environ, ACTINV_DATA_DIR=str(Path(self.cfg["data_dir"]).resolve()))
        for spec in sorted(spec_dir.glob(f"*__{variant}.spec.json")):
            result = spec.with_name(spec.name.replace(".spec.json", ".result.json"))
            points = spec.with_name(spec.name.replace(".spec.json", ".points.json"))
            if points.exists():
                continue
            result.unlink(missing_ok=True)  # a full result without a points file is not trusted
            self._run([self.cfg["actinv"], "run", spec, result], f"actinv run {spec.name}", env=env)
            write_points(result, points)

    def _build_specs(self, case_name: str, case: dict, hist: dict, spec_dir: Path, extra: list, what: str,
                     components=None) -> None:
        argv = [sys.executable, BUILD_SCRIPT, "--run", case["history_run"], "--spectrum-run", case["spectrum_run"],
                "--scenario", case["scenario"], "--physics", case["physics"], "--history", hist["history_path"],
                "--data-dir", self.cfg["data_dir"], "--cooling-grid", ",".join(f"{t!r}s" for t in cooling_grid_s()),
                *self._photon_argv(), "--actinv", self.cfg["actinv"], "--output-dir", spec_dir, *extra]
        for component in components or []:
            argv += ["--component", component]
        if self.cfg.get("impurities"):
            argv += ["--impurities", self.cfg["impurities"]]
        self._run(argv, f"{what} ({case_name})")

    def _actinv_points(self, spec: Path, points: Path) -> None:
        """Run ACTINV on a spec and leave only a points file (atomically)."""
        env = dict(os.environ, ACTINV_DATA_DIR=str(Path(self.cfg["data_dir"]).resolve()))
        result = spec.with_name(spec.name.replace(".spec.json", ".result.json"))
        result.unlink(missing_ok=True)
        self._run([self.cfg["actinv"], "run", spec, result], f"actinv run {spec.name}", env=env)
        points.parent.mkdir(parents=True, exist_ok=True)
        write_points(result, points)

    def decay_curves(self, case_name: str, case: dict, f: float, used: dict, variant: str) -> DecayCurves:
        """Amendment 2: one continuation run per (event, governing component installation), via the content cache."""
        hist = self.history(case_name, case, f, used)
        self._cases[case_name] = case
        build = build_module()
        needs, entries = [], {}
        for e in replacement_events(hist["data"], self.classes):
            for component in self.classes[e["class"]]["governing"]:
                found = [k for k, (a, b) in enumerate(build.installations(hist["data"], component), start=1)
                         if a < e["start_s"] - TOL_S + 1e-9 and e["start_s"] <= b + TOL_S]
                if not found:
                    continue
                needs.append((e["start_s"], component, found[-1]))
                entries[(component, found[-1], e["start_s"])] = {
                    "component": component, "installation": found[-1], "shutdown_s": e["start_s"]}
        spec_dir = hist["dir"] / "decay"
        grid = cooling_grid_s()
        by_key = {}
        if entries:
            times_text = json.dumps(sorted(entries.values(), key=lambda x: (x["shutdown_s"], x["component"])),
                                    indent=2) + "\n"
            times_path = hist["dir"] / "decay-times.json"
            if spec_dir.exists() and not (complete_spec_dir(spec_dir) and times_path.is_file()
                                          and times_path.read_text(encoding="utf-8") == times_text):
                shutil.rmtree(spec_dir)
            if not spec_dir.exists():
                times_path.write_text(times_text, encoding="utf-8")
                self._build_specs(case_name, case, hist, spec_dir,
                                  ["--decay-continuations", times_path, "--continuations-only"],
                                  "build_activation_inputs", sorted({c for _, c, _ in needs}))
            provs = [json.loads(p.read_text(encoding="utf-8"))
                     for p in sorted(spec_dir.glob(f"*__{variant}.provenance.json"))]
            if self.continuation_method == "restart":
                if not self.library:
                    self.library = next((p["library"] for p in provs if p.get("library")), {})
                by_key = self.restart_curves(provs, spec_dir)
            else:
                # Run the continuations not yet in the cache, several at once; each writes only its own files.
                missing: dict = {}
                for prov in provs:
                    spec = spec_dir / prov["spec_file"]
                    sha = hashlib.sha256(spec.read_bytes()).hexdigest()
                    if (self.decay_cache / f"{sha}.points.json").exists():
                        self.continuation_cache_hits += 1
                    elif sha in missing:
                        self.continuation_cache_hits += 1
                    else:
                        missing[sha] = spec
                with ThreadPoolExecutor(max_workers=max(1, self.actinv_workers)) as pool:
                    jobs = [pool.submit(self._actinv_points, spec, self.decay_cache / f"{sha}.points.json")
                            for sha, spec in sorted(missing.items())]
                    for job in jobs:
                        job.result()
                self.continuation_runs += len(missing)
                for prov in provs:
                    spec = spec_dir / prov["spec_file"]
                    sha = hashlib.sha256(spec.read_bytes()).hexdigest()
                    points = self.decay_cache / f"{sha}.points.json"
                    stored = json.loads(points.read_text(encoding="utf-8"))
                    tail = stored["steps"][-len(grid):]
                    span = tail[-1][0] - tail[0][0]
                    if (len(tail) != len(grid) or any(s[2] != 0 for s in tail)
                            or abs(span - (grid[-1] - grid[0])) > 1e-6 * grid[-1]):
                        raise ToolError(f"{spec.name}: the last {len(grid)} steps are not the cooling grid")
                    if not self.library and prov.get("library"):
                        self.library = prov["library"]
                    key = (prov["component"], prov["installation_index"], int(round(prov["continuation_of_shutdown_s"])))
                    by_key[key] = {"volume_m3": prov["volume_m3"],
                                   "points": [(tau, s[1] * prov["mass_g"], s[3], 0.0) for tau, s in zip(grid, tail)]}
        curves = {}
        for start_s, component, k in needs:
            got = by_key.get((component, k, int(round(start_s))))
            if got is not None:
                curves[(start_s, component)] = got
        return DecayCurves(curves)

    def _actinv_trunk(self, spec: Path, trunk_path: Path, needed: set) -> None:
        """Run a trunk and keep, atomically, the inventory at the needed steps (merged with any already cached)."""
        env = dict(os.environ, ACTINV_DATA_DIR=str(Path(self.cfg["data_dir"]).resolve()))
        result = spec.with_name(spec.name.replace(".spec.json", ".result.json"))
        result.unlink(missing_ok=True)
        self._run([self.cfg["actinv"], "run", spec, result], f"actinv run {spec.name}", env=env)
        got = scan_result(result, keep_steps=needed)
        absent = sorted(needed - set(got["kept_steps"]))
        if absent:
            raise ToolError(f"{spec.name}: the result has {got['n_steps']} steps; none at index {absent}")
        steps = {}
        if trunk_path.exists():
            steps = json.loads(trunk_path.read_text(encoding="utf-8")).get("steps", {})
        steps.update({str(i): trunk_step_record(step) for i, step in got["kept_steps"].items()})
        trunk_path.parent.mkdir(parents=True, exist_ok=True)
        tmp = trunk_path.with_name(trunk_path.name + ".tmp")
        tmp.write_text(json.dumps({"schema": TRUNK_SCHEMA, "n_steps": got["n_steps"], "steps": steps}) + "\n",
                       encoding="utf-8")
        os.replace(tmp, trunk_path)
        result.unlink()

    def restart_curves(self, provs: list, spec_dir: Path, cache_dir: Path | None = None) -> dict:
        """One heat-only trunk per component installation, then a cooling run per shutdown restarted from the trunk's
        inventory there. Returns {(component, installation, shutdown_s): {"volume_m3", "points"}} like the full method;
        the trunk's below-floor figures per curve are left in self.restart_info."""
        cache = cache_dir or self.decay_cache
        grid = cooling_grid_s()
        groups = restart_groups(provs, spec_dir, len(grid))
        restart_dir = spec_dir / "restart"
        restart_dir.mkdir(parents=True, exist_ok=True)
        trunks = {}  # sha -> (spec path, needed step indices)
        for g in groups:
            path = restart_dir / g["trunk_file"]
            path.write_text(json.dumps(g["trunk"], indent=2) + "\n", encoding="utf-8")
            g["sha"] = hashlib.sha256(path.read_bytes()).hexdigest()
            g["path"] = path
            needed = {sd["step_index"] for sd in g["shutdowns"]}
            held = set(trunks[g["sha"]][1]) if g["sha"] in trunks else set()
            trunks[g["sha"]] = (path, held | needed)
        todo = {}
        for sha, (path, needed) in sorted(trunks.items()):
            trunk_path = cache / f"{sha}.trunk.json"
            have = set()
            if trunk_path.exists():
                have = {int(i) for i in json.loads(trunk_path.read_text(encoding="utf-8")).get("steps", {})}
            if needed <= have:
                self.trunk_cache_hits += 1
            else:
                todo[sha] = (path, needed | have)
        with ThreadPoolExecutor(max_workers=max(1, self.actinv_workers)) as pool:
            jobs = [pool.submit(self._actinv_trunk, path, cache / f"{sha}.trunk.json", needed)
                    for sha, (path, needed) in todo.items()]
            for job in jobs:
                job.result()
        self.trunk_runs += len(todo)
        missing: dict = {}
        planned = []
        for g in groups:
            doc = json.loads((cache / f"{g['sha']}.trunk.json").read_text(encoding="utf-8"))
            ends = schedule_end_times(g["trunk"]["schedule"])
            for sd in g["shutdowns"]:
                prov, index = sd["prov"], sd["step_index"]
                record = doc["steps"][str(index)]
                if abs(record["t_s"] - ends[index]) > 1e-6 * ends[index]:
                    raise ToolError(f"{g['trunk_file']}: step {index} ends at {record['t_s']!r} s, "
                                    f"the schedule says {ends[index]!r} s")
                spec = cooling_spec(sd["spec"], record, len(grid))
                path = restart_dir / prov["spec_file"].replace(".spec.json", ".restart.spec.json")
                path.write_text(json.dumps(spec, indent=2) + "\n", encoding="utf-8")
                sha = hashlib.sha256(path.read_bytes()).hexdigest()
                planned.append((prov, path, sha, record))
                if (cache / f"{sha}.points.json").exists() or sha in missing:
                    self.restart_cooling_cache_hits += 1
                else:
                    missing[sha] = path
        with ThreadPoolExecutor(max_workers=max(1, self.actinv_workers)) as pool:
            jobs = [pool.submit(self._actinv_points, path, cache / f"{sha}.points.json")
                    for sha, path in sorted(missing.items())]
            for job in jobs:
                job.result()
        self.restart_cooling_runs += len(missing)
        by_key = {}
        for prov, path, sha, record in planned:
            stored = json.loads((cache / f"{sha}.points.json").read_text(encoding="utf-8"))
            key = (prov["component"], prov["installation_index"], int(round(prov["continuation_of_shutdown_s"])))
            by_key[key] = {"volume_m3": prov["volume_m3"],
                           "points": cooling_curve(stored, grid, prov["mass_g"], path.name, exact=True)}
            self.restart_info[key] = {
                "n_states_below_floor": record["n_states_below_floor"],
                "heat_bound_from_below_floor_W_per_g": record["heat_bound_from_below_floor_W_per_g"]}
        return by_key

    def equivalence_check(self, case: dict, variant: str = BARE) -> dict:
        """Amendment 2 item 5: heat at the end of the first outages of one installation, with the outage as a
        single zero-flux step and subdivided on the cooling grid. Refuses above EQUIVALENCE_TOL relative."""
        hist = self.history(CALIBRATION_CASE, case, 1.0, {})
        base = self.out / "equivalence-check"
        stem = f"{EQUIVALENCE_COMPONENT}__inst001__{variant}"
        paths = {}
        for label, extra in (("single", []), ("subdivided", ["--subdivide-outages"])):
            spec_dir = base / label
            if spec_dir.exists() and not complete_spec_dir(spec_dir):
                shutil.rmtree(spec_dir)
            if not spec_dir.exists():
                self._build_specs(CALIBRATION_CASE, case, hist, spec_dir, extra,
                                  "build_activation_inputs (equivalence check)", [EQUIVALENCE_COMPONENT])
            paths[label] = spec_dir
        single = json.loads((paths["single"] / f"{stem}.spec.json").read_text(encoding="utf-8"))
        prov = json.loads((paths["single"] / f"{stem}.provenance.json").read_text(encoding="utf-8"))
        schedule = single["schedule"][:prov["schedule"]["irradiation_step_count"]]
        ends, cursor = [], 0.0
        for index, step in enumerate(schedule):
            cursor += float(step["dt"].split()[0])
            if step["flux"] == 0.0:
                ends.append((index, cursor))
        ends = ends[:EQUIVALENCE_OUTAGES]
        if not ends:
            raise Refused("equivalence check: the first installation of the first wall has no outage to compare")
        sub = json.loads((paths["subdivided"] / f"{stem}.spec.json").read_text(encoding="utf-8"))
        sub_times, cursor = [], 0.0
        for step in sub["schedule"]:
            cursor += float(step["dt"].split()[0])
            sub_times.append(cursor)
        sub_index = {}
        for _, end in ends:
            near = [i for i, t in enumerate(sub_times) if abs(t - end) <= max(1e-3, 1e-9 * end)]
            if len(near) != 1:
                raise Refused(f"equivalence check: the subdivided schedule has no step ending at {end!r} s")
            sub_index[end] = near[0]
        heats = {}
        for label, doc, last in (("single", single, ends[-1][0]), ("subdivided", sub, sub_index[ends[-1][1]])):
            cut = {**doc, "schedule": doc["schedule"][:last + 1]}
            cut_spec = base / f"{label}-first-{len(ends)}-outages.spec.json"
            cut_spec.write_text(json.dumps(cut, indent=2) + "\n", encoding="utf-8")
            points = cut_spec.with_name(cut_spec.name.replace(".spec.json", ".points.json"))
            if not points.exists():
                self._actinv_points(cut_spec, points)
            heats[label] = json.loads(points.read_text(encoding="utf-8"))["steps"]
        rows, worst = [], 0.0
        for number, (index, end) in enumerate(ends, start=1):
            a = heats["single"][index][1]
            b = heats["subdivided"][sub_index[end]][1]
            rel = abs(a - b) / max(abs(a), abs(b)) if max(abs(a), abs(b)) > 0 else 0.0
            worst = max(worst, rel)
            rows.append({"outage": number, "end_since_installation_s": end, "heat_W_per_g_single_step": a,
                         "heat_W_per_g_subdivided": b, "relative_difference": rel})
        record = {"component": EQUIVALENCE_COMPONENT, "installation": 1, "tolerance_relative": EQUIVALENCE_TOL,
                  "outages": rows, "max_relative_difference": worst, "passed": worst <= EQUIVALENCE_TOL}
        write_json(base / "result.json", record)
        if not record["passed"]:
            raise Refused("equivalence check failed: a single zero-flux step and the subdivided outage differ by more "
                          f"than {EQUIVALENCE_TOL:g} relative: " + json.dumps(rows))
        return record

    def spectrum_error_of_replaced(self, case: dict, out: dict) -> None:
        """Amendment 2: a replaced component that never governs (the magnets) has no continuation sidecar, so its
        spectrum-error summary is computed by the builder's own functions from the same two run records."""
        missing = [spec["component_id"] for spec in self.classes.values() if spec["component_id"] not in out]
        if not missing:
            return
        build = build_module()
        run, spectrum_run = load_json(case["history_run"], "run record"), load_json(case["spectrum_run"], "spectrum run")
        bounds = build.library_info(Path(self.cfg["data_dir"]))["bounds_eV"]
        for component in missing:
            total, _ = build.total_neutron_flux_cm2(run, component)
            details = build.scaled_spectrum_from_run(spectrum_run, sha256_file(case["spectrum_run"]), component, bounds, total)
            if details is not None:
                out[component] = {
                    "spectrum_run_sha256": details.get("spectrum_run_sha256"),
                    "scale_factor": details.get("scale_factor_main_total_over_spectrum_run_total"),
                    **(details.get("spectrum_relative_error") or {}),
                }

    def spectrum_error(self, case_name: str, f: float, variant: str) -> dict:
        """Per component, the spectrum-error summary from the provenance sidecars of the first iteration."""
        spec_dir = self._dir(case_name, f, {}) / ("decay" if self.amend2 else "activation")
        out: dict = {}
        for prov_path in sorted(spec_dir.glob(f"*__{variant}.provenance.json")):
            prov = json.loads(prov_path.read_text(encoding="utf-8"))
            if prov["component"] in out:
                continue
            details = prov["spectrum"]["details"]
            out[prov["component"]] = {
                "spectrum_run_sha256": details.get("spectrum_run_sha256"),
                "scale_factor": details.get("scale_factor_main_total_over_spectrum_run_total"),
                **(details.get("spectrum_relative_error") or {}),
            }
        if self.amend2 and case_name in self._cases:
            self.spectrum_error_of_replaced(self._cases[case_name], out)
        return out


# -------------------------------------------------------------- calibration --

def class_split(runner, cls: str, w: float) -> float:
    """The work share of the authored duration for a class: its class_w entry, else the grid w."""
    return getattr(runner, "class_w", {}).get(cls, w)


def governing_quantity(runner) -> str:
    return getattr(runner, "governing", "heat")


def calibrate_class_thresholds(runner: Runner, cases: dict, w: float, fixed_s: dict, variant: str) -> dict:
    """q* per class from the first replacement of that class in port/reference, on the governing quantity.

    Both heat and dose are calibrated; the governing one (heat by default) gives entry["status"] and entry["q_star"],
    the other is kept for the cross-check.
    Uses the fixed-duration timeline (iteration 0), where that event's duration is exactly the authored one.
    """
    name = CALIBRATION_CASE
    hist = runner.history(name, cases[name], 1.0, {})
    act = runner.activation(name, cases[name], 1.0, {}, variant)
    events = replacement_events(hist["data"], runner.classes)
    out = {}
    for cls, spec in runner.classes.items():
        mine = [e for e in events if e["class"] == cls]
        if not mine:
            out[cls] = not_evaluated(f"no {cls} replacement in {name} at the fixed durations")
            continue
        first = mine[0]
        target = (1.0 - class_split(runner, cls, w)) * fixed_s[cls]
        entry = {"event": {"component": first["component"], "start_s": first["start_s"]}}
        curve = event_curve(act, first, spec["governing"], "heat")
        cal = curve if isinstance(curve, dict) else calibrate(curve, target)
        entry["heat"] = cal
        dose_curve = event_curve(act, first, spec["governing"], "dose")
        entry["dose"] = dose_curve if isinstance(dose_curve, dict) else calibrate(dose_curve, target)
        gov = entry[governing_quantity(runner)]
        entry.update(gov if gov["status"] != "EVALUATED" else {"status": "EVALUATED", "q_star": gov["q_star"]})
        out[cls] = entry
    return out


# ---------------------------------------------------------------- iteration --

def used_duration(used: dict, component: str, k: int, fixed: float) -> float:
    lst = used.get(component, [])
    return float(lst[k - 1]) if k <= len(lst) else fixed


def coupled_case(runner: Runner, name: str, case: dict, f: float, thresholds: dict, w: float,
                 fixed_s: dict, variant: str) -> dict:
    """History -> activation -> durations, until no duration changes by more than 1 day (at most 5 iterations; 10 under Amendment 2)."""
    used: dict = {}
    iterations = []
    limit = getattr(runner, "max_iterations", MAX_ITERATIONS)
    quantity = governing_quantity(runner)
    for number in range(1, limit + 1):
        hist = runner.history(name, case, f, used)
        act = runner.activation(name, case, f, used, variant)
        horizon = history_end_s(hist["data"])
        events = replacement_events(hist["data"], runner.classes)
        out: dict = {}
        details, change, window_limited = [], 0.0, False
        for e in events:
            spec = runner.classes[e["class"]]
            th = thresholds[e["class"]]
            if th["status"] != "EVALUATED":
                return not_evaluated(f"calibration unavailable for {e['class']}: {th['reason']}", iterations=iterations)
            curve = event_curve(act, e, spec["governing"], quantity)
            cd = curve if isinstance(curve, dict) else cooldown(curve, th["q_star"])
            if cd["status"] != "EVALUATED":
                details.append({**e, **cd})
                iterations.append({"iteration": number, "durations_in_s": used, "events": details})
                return not_evaluated(
                    f"iteration {number}: {e['component']} replacement {e['k']} at {e['start_s']:.0f} s: {cd['reason']}",
                    iterations=iterations)
            work = class_split(runner, e["class"], w) * fixed_s[e["class"]]
            new = work + cd["cooldown_s"]
            old = used_duration(used, e["component"], e["k"], fixed_s[e["class"]])
            remaining = horizon - e["start_s"]
            delta = abs(min(new, remaining) - min(old, remaining))
            change = max(change, delta)
            window_limited = window_limited or cd["window_limited"]
            out.setdefault(e["component"], []).append(new)
            details.append({"component": e["component"], "class": e["class"], "k": e["k"], "start_s": e["start_s"],
                            "work_s": work, "cooldown_s": cd["cooldown_s"], "window_limited": cd["window_limited"],
                            "duration_used_s": old, "duration_computed_s": new})
        iterations.append({"iteration": number, "durations_in_s": used, "durations_out_s": out,
                           "max_change_s": change, "window_limited": window_limited, "events": details,
                           "history": str(hist["history_path"])})
        if change <= CONVERGENCE_S and not window_limited:
            result = {"status": "EVALUATED", "converged_at_iteration": number, "iterations": iterations,
                      "durations_s": used, "history": summarize_history(hist["data"], runner.classes),
                      "assumptions": str(hist["assumptions"])}
            if quantity == "heat":
                result["dose_cross_check"] = dose_cross_check(runner, act, hist["data"], thresholds, w, fixed_s, used)
            else:
                result["cross_check"] = {"quantity": "heat", "events": quantity_cross_check(
                    runner, act, hist["data"], thresholds, w, fixed_s, "heat")}
            return result
        used = out
    last_two = [it.get("durations_out_s") for it in iterations[-2:]]
    return not_evaluated(f"no convergence in {limit} iterations (last change "
                         f"{iterations[-1]['max_change_s'] / DAY_S:.3f} d)",
                         iterations=iterations, last_two_duration_sets_s=last_two)


def quantity_cross_check(runner: Runner, act: dict, history: dict, thresholds: dict, w: float, fixed_s: dict,
                         quantity: str) -> list:
    """Cooldowns on the quantity that does not govern, calibrated the same way, beside the governing durations.
    Not used by any decision."""
    out = []
    for e in replacement_events(history, runner.classes):
        th = thresholds[e["class"]].get(quantity, not_evaluated(f"no {quantity} calibration"))
        if th["status"] != "EVALUATED":
            out.append({"component": e["component"], "k": e["k"], "status": "NOT_EVALUATED", "reason": th["reason"]})
            continue
        curve = event_curve(act, e, runner.classes[e["class"]]["governing"], quantity)
        cd = curve if isinstance(curve, dict) else cooldown(curve, th["q_star"])
        if cd["status"] != "EVALUATED":
            out.append({"component": e["component"], "k": e["k"], **cd})
            continue
        out.append({"component": e["component"], "k": e["k"], "status": "EVALUATED",
                    "cooldown_s": cd["cooldown_s"],
                    "implied_duration_s": class_split(runner, e["class"], w) * fixed_s[e["class"]] + cd["cooldown_s"],
                    "window_limited": cd["window_limited"]})
    return out


def dose_cross_check(runner: Runner, act: dict, history: dict, thresholds: dict, w: float, fixed_s: dict, used: dict) -> list:
    """Contact-dose cooldowns, calibrated the same way, beside the heat-based durations. Not used by any decision."""
    return quantity_cross_check(runner, act, history, thresholds, w, fixed_s, "dose")


def summarize_history(history: dict, classes: dict) -> dict:
    return {
        "outcome": history.get("outcome"),
        "lifetime_net_electricity_mwh": lifetime_net_mwh(history),
        "total_replacement_downtime_s": total_downtime_s(history, classes),
        "replacements": [{k: e[k] for k in ("component", "k", "start_s", "end_s")} for e in replacement_events(history, classes)],
    }


def fixed_case(runner: Runner, name: str, case: dict, f: float) -> dict:
    hist = runner.history(name, case, f, {})
    return {"status": "EVALUATED", "history": summarize_history(hist["data"], runner.classes),
            "assumptions": str(hist["assumptions"])}


# ---------------------------------------------------------------- decisions --

def rel_gap(a: float, b: float) -> float:
    """Difference as a fraction of the larger value (by magnitude)."""
    big = max(abs(a), abs(b))
    return abs(a - b) / big if big > 0 else 0.0


def ranking(values: dict) -> list:
    return sorted(values, key=lambda k: (-values[k], k))


def decide_d1(fixed: dict, computed: dict) -> dict:
    """Ranking of the arrangements by lifetime net electricity; changes if a swap exists and every swapped pair gaps > 2 %."""
    if any(v is None for v in (*fixed.values(), *computed.values())) or set(fixed) != set(computed):
        return not_evaluated("a lifetime net electricity is missing")
    rf, rc = ranking(fixed), ranking(computed)
    swapped = []
    names = sorted(fixed)
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            if (fixed[a] > fixed[b]) != (computed[a] > computed[b]) and fixed[a] != fixed[b] and computed[a] != computed[b]:
                swapped.append({"pair": [a, b], "gap_fraction_computed": rel_gap(computed[a], computed[b])})
    changed = rf != rc and bool(swapped) and all(s["gap_fraction_computed"] > D1_GAP for s in swapped)
    return {"status": "EVALUATED", "changed": changed, "fixed_ranking": rf, "computed_ranking": rc,
            "swapped_pairs": swapped, "threshold_fraction": D1_GAP}


def decide_d2(fixed: dict, computed: dict) -> dict:
    """Highest-lifetime sweep point; changes if it differs and the two points differ by > 1 % under the computed model."""
    if any(v is None for v in (*fixed.values(), *computed.values())) or set(fixed) != set(computed):
        return not_evaluated("a lifetime net electricity is missing")
    best_f, best_c = ranking(fixed)[0], ranking(computed)[0]
    gap = rel_gap(computed[best_f], computed[best_c])
    return {"status": "EVALUATED", "changed": best_f != best_c and gap > D2_GAP, "fixed_optimum": best_f,
            "computed_optimum": best_c, "gap_fraction_computed": gap, "threshold_fraction": D2_GAP}


def decide_d3(fixed: dict, computed: dict) -> dict:
    """Downtime contrasts; changes if computed/fixed is outside [0.8, 1.25] where the fixed difference is >= 30 days."""
    rows, missing = [], False
    for label, a, b in CONTRASTS:
        vals = [fixed.get(a), fixed.get(b), computed.get(a), computed.get(b)]
        if any(v is None for v in vals):
            missing = True
            rows.append({"contrast": label, "status": "NOT_EVALUATED"})
            continue
        df, dc = fixed[a] - fixed[b], computed[a] - computed[b]
        considered = abs(df) >= D3_MIN_FIXED_S
        ratio = dc / df if df != 0 else None
        changed = considered and (ratio < D3_BAND[0] or ratio > D3_BAND[1])
        rows.append({"contrast": label, "status": "EVALUATED", "fixed_difference_s": df, "computed_difference_s": dc,
                     "ratio_computed_over_fixed": ratio, "considered": considered, "changed": changed})
    if missing and not any(r.get("changed") for r in rows):
        return not_evaluated("a downtime is missing", contrasts=rows)
    return {"status": "EVALUATED", "changed": any(r.get("changed") for r in rows), "contrasts": rows,
            "band": list(D3_BAND), "min_fixed_difference_s": D3_MIN_FIXED_S}


def decide_d4(fixed: dict, computed: dict) -> dict:
    """Best blanket-replacement fraction f; changes if the computed best is <= 0.9 and gains > 1 % over f = 1.0."""
    if any(v is None for v in (*fixed.values(), *computed.values())) or 1.0 not in computed or set(fixed) != set(computed):
        return not_evaluated("a lifetime net electricity is missing")

    def best(values):
        return max(values, key=lambda f: (values[f], f))

    bc, bf = best(computed), best(fixed)
    base = computed[1.0]
    gain = (computed[bc] - base) / abs(base) if base != 0 else float("inf")
    return {"status": "EVALUATED", "changed": bc <= D4_MAX_F and gain > D4_GAIN, "computed_best_f": bc,
            "fixed_best_f": bf, "gain_over_f1_fraction": gain, "threshold_fraction": D4_GAIN, "max_best_f": D4_MAX_F}


def verdict(decisions_by_w: dict) -> dict:
    """MATERIAL / NOT MATERIAL / MIXED by the protocol; NOT_EVALUATED when no change stands and something is unevaluated.

    decisions_by_w: {w: {"D1": {...}, ...}}.
    """
    changed_at = {}
    unevaluated = []
    for w, decisions in decisions_by_w.items():
        for name, d in decisions.items():
            if d["status"] != "EVALUATED":
                unevaluated.append({"w": w, "decision": name})
            elif d["changed"]:
                changed_at.setdefault(name, []).append(w)
    central = [n for n, ws in changed_at.items() if str(CENTRAL_W) in ws]
    if central:
        label = "MATERIAL"
    elif changed_at:
        label = "MIXED"
    elif unevaluated:
        label = "NOT_EVALUATED"
    else:
        label = "NOT MATERIAL"
    return {"verdict": label, "changed_at_w": {k: sorted(v) for k, v in sorted(changed_at.items())},
            "changed_at_central_w": sorted(central), "not_evaluated": unevaluated}


# --------------------------------------------------------------------- main --

def lifetimes(results: dict) -> dict:
    return {k: (v["history"]["lifetime_net_electricity_mwh"] if v["status"] == "EVALUATED" else None) for k, v in results.items()}


def downtimes(results: dict) -> dict:
    return {k: (v["history"]["total_replacement_downtime_s"] if v["status"] == "EVALUATED" else None) for k, v in results.items()}


def with_spectrum_error(runner: Runner, name: str, f: float, variant: str, result: dict) -> dict:
    """Attach the first iteration's per-component spectrum-error summary (from the provenance sidecars)."""
    return {**result, "spectrum_error": runner.spectrum_error(name, f, variant)}


def analyse_variant(runner: Runner, cases: dict, sweep: dict, ws, fs, variant: str, fixed_results: dict,
                    fixed_s: dict) -> dict:
    out = {"variant": variant, "w": {}}
    port_ref = cases[CALIBRATION_CASE]
    decisions_by_w = {}
    # The central w alone can settle MATERIAL, so it runs first; each w's decisions are also written as soon as
    # they exist. The order does not change any value, and the result lists w in protocol order.
    for w in sorted(ws, key=lambda value: value != CENTRAL_W):
        thresholds = calibrate_class_thresholds(runner, cases, w, fixed_s, variant)
        computed = {}
        for name, case in {**cases, **{f"sweep/{k}": v for k, v in sweep.items()}}.items():
            computed[name] = with_spectrum_error(
                runner, name, 1.0, variant, coupled_case(runner, name, case, 1.0, thresholds, w, fixed_s, variant))
        for f in fs:
            key = f"f/{f}"
            computed[key] = computed[CALIBRATION_CASE] if f == 1.0 else with_spectrum_error(
                runner, CALIBRATION_CASE, f, variant,
                coupled_case(runner, CALIBRATION_CASE, port_ref, f, thresholds, w, fixed_s, variant))
        arr = {k: computed[k] for k in ARRANGEMENTS}
        swp = {k: computed[f"sweep/{k}"] for k in sweep}
        fres = {f: computed[f"f/{f}"] for f in fs}
        fx_arr = {k: fixed_results[k] for k in ARRANGEMENTS}
        fx_swp = {k: fixed_results[f"sweep/{k}"] for k in sweep}
        fx_f = {f: fixed_results[f"f/{f}"] for f in fs}
        decisions = {
            "D1": decide_d1(lifetimes(fx_arr), lifetimes(arr)),
            "D2": decide_d2(lifetimes(fx_swp), lifetimes(swp)) if sweep else not_evaluated("the allocation sweep was not run"),
            "D3": decide_d3(downtimes(fx_arr), downtimes(arr)),
            "D4": decide_d4({f: v for f, v in lifetimes(fx_f).items()}, {f: v for f, v in lifetimes(fres).items()}),
        }
        decisions_by_w[w] = decisions
        out["w"][str(w)] = {"w": w, "q_star": thresholds, "cases": computed, "decisions": decisions}
        write_json(runner.out / f"interim-{variant}-w{w}.json",
                   {"note": "interim: decisions for one w; the verdict needs every w", "w": w,
                    "decisions": decisions, "central_w": CENTRAL_W})
    out["verdict"] = verdict({str(w): decisions_by_w[w] for w in ws})
    return out


def run_all(cfg: dict, out_dir: Path, actinv_workers: int = 1) -> dict:
    base = load_json(cfg["assumptions"], "assumptions")
    classes = copy.deepcopy(CLASSES)
    classes.update(cfg.get("classes", {}))
    fixed_s = fixed_durations_s(base, classes)
    cases = {k: cfg["arrangements"][k] for k in ARRANGEMENTS}
    sweep = cfg["sweep"]
    ws = tuple(cfg.get("w_values", W_VALUES))
    fs = tuple(cfg.get("f_values", F_VALUES))
    runner = Runner(cfg, out_dir, base, classes)
    runner.actinv_workers = actinv_workers
    equivalence = runner.equivalence_check(cases[CALIBRATION_CASE]) if runner.amend2 else None
    fixed = {}
    for name, case in {**cases, **{f"sweep/{k}": v for k, v in sweep.items()}}.items():
        fixed[name] = fixed_case(runner, name, case, 1.0)
    for f in fs:
        fixed[f"f/{f}"] = fixed[CALIBRATION_CASE] if f == 1.0 else fixed_case(runner, CALIBRATION_CASE, cases[CALIBRATION_CASE], f)
    variants = {BARE: analyse_variant(runner, cases, sweep, ws, fs, BARE, fixed, fixed_s)}
    if cfg.get("impurities"):
        variants[WITH_IMPURITIES] = analyse_variant(runner, cases, sweep, ws, fs, WITH_IMPURITIES, fixed, fixed_s)
        variants[WITH_IMPURITIES]["role"] = "secondary; reported, not used for the verdict"
    variants[BARE]["role"] = "primary; the verdict"
    body = {"fixed": fixed, "fixed_durations_s": fixed_s, "variants": variants, "library": runner.library,
            "w_values": list(ws), "f_values": list(fs), "classes": classes, "max_iterations": runner.max_iterations}
    if runner.amend2:
        body["amendment"] = AMENDMENT_2
        body["equivalence_check"] = equivalence
        body["decay_continuations"] = {"runs": runner.continuation_runs, "cache_hits": runner.continuation_cache_hits}
        if runner.continuation_method == "restart":
            body["decay_continuations"].update({
                "method": "restart", "trunk_runs": runner.trunk_runs, "trunk_cache_hits": runner.trunk_cache_hits,
                "restart_cooling_runs": runner.restart_cooling_runs,
                "restart_cooling_cache_hits": runner.restart_cooling_cache_hits})
    return body


def check_config(cfg: dict) -> None:
    for key in ("faris", "actinv", "data_dir", "assumptions", "output_dir", "arrangements", "sweep"):
        if key not in cfg:
            raise Refused(f"config lacks {key}")
    if cfg.get("continuation_method", "full") not in CONTINUATION_METHODS:
        raise Refused(f"continuation_method must be one of {list(CONTINUATION_METHODS)}, "
                      f"not {cfg.get('continuation_method')!r}")
    if cfg.get("continuation_method") == "restart" and cfg.get("amendment") != AMENDMENT_2:
        raise Refused("continuation_method \"restart\" needs \"amendment\": 2 (it replaces the decay continuations)")
    if cfg.get("amendment") not in (None, AMENDMENT_2):
        raise Refused(f"unknown amendment {cfg.get('amendment')!r}; only 2 exists")
    if set(cfg["arrangements"]) != set(ARRANGEMENTS):
        raise Refused(f"arrangements must be exactly {list(ARRANGEMENTS)}")
    for name, case in {**cfg["arrangements"], **{f"sweep/{k}": v for k, v in cfg["sweep"].items()}}.items():
        for key in ("scenario", "physics", "history_run", "spectrum_run"):
            if not Path(case.get(key, "")).is_file():
                raise Refused(f"{name}: {key} file not found: {case.get(key)}")
    if not cfg.get("allow_reduced_grid"):
        if len(cfg["sweep"]) != SWEEP_POINTS:
            raise Refused(f"the protocol sweep has {SWEEP_POINTS} points")
        if tuple(cfg.get("w_values", W_VALUES)) != W_VALUES or tuple(cfg.get("f_values", F_VALUES)) != F_VALUES:
            raise Refused("w and f grids are fixed by the protocol")
    elif 1.0 not in cfg.get("f_values", F_VALUES):
        raise Refused("f_values must include 1.0")
    if cfg.get("governing_quantity", "heat") not in ("heat", "dose"):
        raise Refused(f"governing_quantity must be \"heat\" or \"dose\", not {cfg.get('governing_quantity')!r}")
    if cfg.get("photon_response") and not Path(cfg["photon_response"]).is_file():
        raise Refused(f"photon_response file not found: {cfg['photon_response']}")
    if cfg.get("governing_quantity") == "dose" and not cfg.get("photon_response"):
        raise Refused("governing_quantity \"dose\" needs photon_response (no contact dose without a photon response)")
    class_w = cfg.get("class_w")
    if class_w is not None:
        if not isinstance(class_w, dict):
            raise Refused("class_w must be a mapping of class to w")
        known = set(CLASSES) | set(cfg.get("classes", {}))
        for cls, value in class_w.items():
            if cls not in known:
                raise Refused(f"class_w names unknown class {cls!r}; classes are {sorted(known)}")
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0.0 < value < 1.0:
                raise Refused(f"class_w[{cls}] = {value!r}: each w must be a number with 0 < w < 1")


def resolve_paths(cfg: dict, base: Path) -> dict:
    def res(v):
        return str(v if os.path.isabs(v) else (base / v)) if isinstance(v, str) else v

    out = dict(cfg)
    for key in ("faris", "actinv", "data_dir", "assumptions", "impurities", "output_dir", "result", "protocol",
                "decay_cache", "photon_response"):
        if out.get(key):
            out[key] = res(out[key])
    out["arrangements"] = {k: {kk: res(vv) for kk, vv in v.items()} for k, v in cfg["arrangements"].items()}
    out["sweep"] = {k: {kk: res(vv) for kk, vv in v.items()} for k, v in cfg["sweep"].items()}
    return out


def actinv_identity(cfg: dict) -> dict:
    proc = subprocess.run([cfg["actinv"], "--version"], capture_output=True, text=True, check=False)
    return {"path": cfg["actinv"], "sha256": sha256_file(cfg["actinv"]), "version": (proc.stdout + proc.stderr).strip()}


def input_hashes(cfg: dict, config_path: Path) -> dict:
    cases = {**{f"arrangement:{k}": v for k, v in cfg["arrangements"].items()},
             **{f"sweep:{k}": v for k, v in cfg["sweep"].items()}}
    # every file of a case is hashed: scenario, physics, the history run and the spectrum run
    return {
        "config": sha256_file(config_path),
        "assumptions": sha256_file(cfg["assumptions"]),
        "impurities": sha256_file(cfg["impurities"]) if cfg.get("impurities") else None,
        **({"photon_response": sha256_file(cfg["photon_response"])} if cfg.get("photon_response") else {}),
        "cases": {k: {kk: sha256_file(vv) for kk, vv in v.items()} for k, v in sorted(cases.items())},
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--config", required=True)
    ap.add_argument("--actinv-workers", type=int, default=1,
                    help="concurrent ACTINV runs for Amendment 2 decay continuations (does not change any value)")
    ap.add_argument("--resume", action="store_true",
                    help="continue an interrupted run in its existing output directory (same config only)")
    ap.add_argument("--equivalence-check-only", action="store_true",
                    help="run only the Amendment 2 equivalence check (config needs \"amendment\": 2) and print its numbers")
    args = ap.parse_args(argv)
    config_path = Path(args.config).resolve()
    try:
        cfg = resolve_paths(load_json(config_path, "config"), config_path.parent)
        protocol = Path(cfg.get("protocol") or DEFAULT_PROTOCOL)
        got = protocol_body_sha256(protocol) if protocol.is_file() else None
        if got != PROTOCOL_BODY_SHA256:
            raise Refused(f"protocol file {protocol} has body SHA-256 {got} (text before its amendments), "
                          f"expected {PROTOCOL_BODY_SHA256}; refusing to run")
        check_config(cfg)
        if cfg.get("amendment") == AMENDMENT_2 and b"### Amendment 2" not in protocol.read_bytes():
            raise Refused(f"protocol file {protocol} has no Amendment 2; refusing to run with \"amendment\": 2")
        if args.equivalence_check_only and cfg.get("amendment") != AMENDMENT_2:
            raise Refused("--equivalence-check-only needs \"amendment\": 2 in the config")
        out_dir = Path(cfg["output_dir"])
        result_path = Path(cfg.get("result") or DEFAULT_RESULT)
        if result_path.exists() and not args.equivalence_check_only:
            raise Refused(f"result file {result_path} already exists")
        marker_path = out_dir / RUN_MARKER
        config_sha = sha256_file(config_path)
        if args.resume:
            marker = load_json(marker_path, "run marker") if marker_path.is_file() else None
            if not isinstance(marker, dict) or marker.get("config_sha256") != config_sha:
                raise Refused(f"cannot resume: {marker_path} is missing or names another config")
            marker["resumes"] = int(marker.get("resumes", 0)) + 1
        else:
            if out_dir.exists():
                raise Refused(f"output directory {out_dir} already exists")
            out_dir.mkdir(parents=True)
            marker = {"config_sha256": config_sha, "resumes": 0}
        write_json(marker_path, marker)
        if args.equivalence_check_only:
            base = load_json(cfg["assumptions"], "assumptions")
            classes = copy.deepcopy(CLASSES)
            classes.update(cfg.get("classes", {}))
            record = Runner(cfg, out_dir, base, classes).equivalence_check(cfg["arrangements"][CALIBRATION_CASE])
            print(json.dumps(record, indent=2, sort_keys=True))
            return 0
        body = run_all(cfg, out_dir, args.actinv_workers)
    except Refused as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    except ToolError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2
    primary = body["variants"][BARE]
    doc = {
        "schema": SCHEMA,
        "script": {"name": "scripts/maintenance_coupling_test.py", "version": SCRIPT_VERSION},
        "protocol": {"path": str(protocol), "committed_sha256": PROTOCOL_SHA256, "body_sha256": PROTOCOL_BODY_SHA256,
                     "file_sha256": sha256_file(protocol)},
        "inputs_sha256": input_hashes(cfg, config_path),
        "run": {"resumes": marker["resumes"]},
        "tools": {"faris": {"path": cfg["faris"], "sha256": sha256_file(cfg["faris"])}, "actinv": actinv_identity(cfg),
                  "activation_library": body["library"]},
        "parameters": {
            "cooling_grid_s": cooling_grid_s(), "w_values": body["w_values"], "f_values": body["f_values"],
            "max_iterations": body["max_iterations"], "convergence_s": CONVERGENCE_S,
            "fixed_durations_s": body["fixed_durations_s"], "classes": body["classes"],
            "thresholds": {"D1_gap": D1_GAP, "D2_gap": D2_GAP, "D3_band": list(D3_BAND), "D3_min_fixed_s": D3_MIN_FIXED_S,
                           "D4_gain": D4_GAIN, "D4_max_f": D4_MAX_F},
            "allow_reduced_grid": bool(cfg.get("allow_reduced_grid")),
        },
        "fixed_model": body["fixed"],
        "computed_model": body["variants"],
        "verdict": primary["verdict"],
    }
    if cfg.get("class_w"):
        doc["parameters"]["class_w"] = dict(cfg["class_w"])
    if cfg.get("governing_quantity") == "dose":
        doc["parameters"]["governing_quantity"] = "dose"
    if cfg.get("continuation_method") == "restart":
        doc["parameters"]["continuation_method"] = "restart"
    if body.get("amendment"):
        doc["amendment"] = body["amendment"]
        doc["equivalence_check"] = body["equivalence_check"]
        doc["decay_continuations"] = body["decay_continuations"]
    write_json(result_path, doc)
    print(f"verdict {primary['verdict']['verdict']}; wrote {result_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
