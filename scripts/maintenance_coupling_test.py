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

Exit status: 0 result written, 2 bad input or refused.
"""
from __future__ import annotations

import argparse
import bisect
import copy
import hashlib
import json
import math
import os
import shutil
import subprocess
import sys
from collections import OrderedDict
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

def write_points(result_path: Path, points_path: Path) -> None:
    """Compact the full ACTINV result into a points file (atomically), then delete the result.

    A heat-only result can still be hundreds of MB for a long schedule, so it is not kept.
    """
    raw = result_path.read_bytes()
    result = json.loads(raw)
    doc = {
        "schema": POINTS_SCHEMA,
        "result_sha256": hashlib.sha256(raw).hexdigest(), "result_bytes": len(raw),
        "n_steps": len(result["steps"]),
        "ms": result.get("ms"),
        "pruned_states": result.get("pruned_states"),
        "total_states": result.get("total_states"),
        "steps": [[s["t_s"], s["heat_W_per_g"]["total"], s.get("flux", 0.0),
                   (s.get("photon_source") or {}).get("contact_gamma_air_dose_proxy_Gy_h")] for s in result["steps"]],
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


def event_series(activation: dict, event: dict, governing: list, quantity: str):
    """({component: [(tau, value)]}, {component: volume}) or a NOT_EVALUATED dict."""
    series, volumes = {}, {}
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
                return not_evaluated(f"{component} has no contact gamma dose: its ACTINV specs have no photon "
                                     "response and run with heat-only outputs")
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
                    "--actinv-outputs", "heat", "--actinv", self.cfg["actinv"], "--output-dir", spec_dir]
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

    def spectrum_error(self, case_name: str, f: float, variant: str) -> dict:
        """Per component, the spectrum-error summary from the provenance sidecars of the first iteration."""
        spec_dir = self._dir(case_name, f, {}) / "activation"
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
        return out


# -------------------------------------------------------------- calibration --

def calibrate_class_thresholds(runner: Runner, cases: dict, w: float, fixed_s: dict, variant: str) -> dict:
    """q* (heat, and dose as a cross-check) per class from the first replacement of that class in port/reference.

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
        target = (1.0 - w) * fixed_s[cls]
        entry = {"event": {"component": first["component"], "start_s": first["start_s"]}}
        curve = event_curve(act, first, spec["governing"], "heat")
        cal = curve if isinstance(curve, dict) else calibrate(curve, target)
        entry["heat"] = cal
        dose_curve = event_curve(act, first, spec["governing"], "dose")
        entry["dose"] = dose_curve if isinstance(dose_curve, dict) else calibrate(dose_curve, target)
        entry.update(cal if cal["status"] != "EVALUATED" else {"status": "EVALUATED", "q_star": cal["q_star"]})
        out[cls] = entry
    return out


# ---------------------------------------------------------------- iteration --

def used_duration(used: dict, component: str, k: int, fixed: float) -> float:
    lst = used.get(component, [])
    return float(lst[k - 1]) if k <= len(lst) else fixed


def coupled_case(runner: Runner, name: str, case: dict, f: float, thresholds: dict, w: float,
                 fixed_s: dict, variant: str) -> dict:
    """History -> activation -> durations, until no duration changes by more than 1 day (at most 5 iterations)."""
    used: dict = {}
    iterations = []
    for number in range(1, MAX_ITERATIONS + 1):
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
            curve = event_curve(act, e, spec["governing"], "heat")
            cd = curve if isinstance(curve, dict) else cooldown(curve, th["q_star"])
            if cd["status"] != "EVALUATED":
                details.append({**e, **cd})
                iterations.append({"iteration": number, "durations_in_s": used, "events": details})
                return not_evaluated(
                    f"iteration {number}: {e['component']} replacement {e['k']} at {e['start_s']:.0f} s: {cd['reason']}",
                    iterations=iterations)
            work = w * fixed_s[e["class"]]
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
            return {"status": "EVALUATED", "converged_at_iteration": number, "iterations": iterations,
                    "durations_s": used, "history": summarize_history(hist["data"], runner.classes),
                    "dose_cross_check": dose_cross_check(runner, act, hist["data"], thresholds, w, fixed_s, used),
                    "assumptions": str(hist["assumptions"])}
        used = out
    last_two = [it.get("durations_out_s") for it in iterations[-2:]]
    return not_evaluated(f"no convergence in {MAX_ITERATIONS} iterations (last change "
                         f"{iterations[-1]['max_change_s'] / DAY_S:.3f} d)",
                         iterations=iterations, last_two_duration_sets_s=last_two)


def dose_cross_check(runner: Runner, act: dict, history: dict, thresholds: dict, w: float, fixed_s: dict, used: dict) -> list:
    """Contact-dose cooldowns, calibrated the same way, beside the heat-based durations. Not used by any decision."""
    out = []
    for e in replacement_events(history, runner.classes):
        th = thresholds[e["class"]].get("dose", not_evaluated("no dose calibration"))
        if th["status"] != "EVALUATED":
            out.append({"component": e["component"], "k": e["k"], "status": "NOT_EVALUATED", "reason": th["reason"]})
            continue
        curve = event_curve(act, e, runner.classes[e["class"]]["governing"], "dose")
        cd = curve if isinstance(curve, dict) else cooldown(curve, th["q_star"])
        if cd["status"] != "EVALUATED":
            out.append({"component": e["component"], "k": e["k"], **cd})
            continue
        out.append({"component": e["component"], "k": e["k"], "status": "EVALUATED",
                    "cooldown_s": cd["cooldown_s"], "implied_duration_s": w * fixed_s[e["class"]] + cd["cooldown_s"],
                    "window_limited": cd["window_limited"]})
    return out


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
    for w in ws:
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
            "D2": decide_d2(lifetimes(fx_swp), lifetimes(swp)),
            "D3": decide_d3(downtimes(fx_arr), downtimes(arr)),
            "D4": decide_d4({f: v for f, v in lifetimes(fx_f).items()}, {f: v for f, v in lifetimes(fres).items()}),
        }
        decisions_by_w[w] = decisions
        out["w"][str(w)] = {"w": w, "q_star": thresholds, "cases": computed, "decisions": decisions}
    out["verdict"] = verdict({str(w): d for w, d in decisions_by_w.items()})
    return out


def run_all(cfg: dict, out_dir: Path) -> dict:
    base = load_json(cfg["assumptions"], "assumptions")
    classes = copy.deepcopy(CLASSES)
    classes.update(cfg.get("classes", {}))
    fixed_s = fixed_durations_s(base, classes)
    cases = {k: cfg["arrangements"][k] for k in ARRANGEMENTS}
    sweep = cfg["sweep"]
    ws = tuple(cfg.get("w_values", W_VALUES))
    fs = tuple(cfg.get("f_values", F_VALUES))
    runner = Runner(cfg, out_dir, base, classes)
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
    return {"fixed": fixed, "fixed_durations_s": fixed_s, "variants": variants, "library": runner.library,
            "w_values": list(ws), "f_values": list(fs), "classes": classes}


def check_config(cfg: dict) -> None:
    for key in ("faris", "actinv", "data_dir", "assumptions", "output_dir", "arrangements", "sweep"):
        if key not in cfg:
            raise Refused(f"config lacks {key}")
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


def resolve_paths(cfg: dict, base: Path) -> dict:
    def res(v):
        return str(v if os.path.isabs(v) else (base / v)) if isinstance(v, str) else v

    out = dict(cfg)
    for key in ("faris", "actinv", "data_dir", "assumptions", "impurities", "output_dir", "result", "protocol"):
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
        "cases": {k: {kk: sha256_file(vv) for kk, vv in v.items()} for k, v in sorted(cases.items())},
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--config", required=True)
    ap.add_argument("--resume", action="store_true",
                    help="continue an interrupted run in its existing output directory (same config only)")
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
        out_dir = Path(cfg["output_dir"])
        result_path = Path(cfg.get("result") or DEFAULT_RESULT)
        if result_path.exists():
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
        body = run_all(cfg, out_dir)
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
            "max_iterations": MAX_ITERATIONS, "convergence_s": CONVERGENCE_S,
            "fixed_durations_s": body["fixed_durations_s"], "classes": body["classes"],
            "thresholds": {"D1_gap": D1_GAP, "D2_gap": D2_GAP, "D3_band": list(D3_BAND), "D3_min_fixed_s": D3_MIN_FIXED_S,
                           "D4_gain": D4_GAIN, "D4_max_f": D4_MAX_F},
            "allow_reduced_grid": bool(cfg.get("allow_reduced_grid")),
        },
        "fixed_model": body["fixed"],
        "computed_model": body["variants"],
        "verdict": primary["verdict"],
    }
    write_json(result_path, doc)
    print(f"verdict {primary['verdict']['verdict']}; wrote {result_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
