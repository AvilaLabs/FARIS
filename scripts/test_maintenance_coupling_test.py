import contextlib
import hashlib
import importlib.util
import io
import json
import math
import os
import stat
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


MC = load("maintenance_coupling_test", "maintenance_coupling_test.py")
TB = load("test_build_activation_inputs_helpers", "test_build_activation_inputs.py")
BUILD = TB.BUILD
DAY = MC.DAY_S

FAKE_FARIS = r'''#!{python}
import json, sys
args = sys.argv[1:]
assert args[:2] == ["history", "from-run"], args
opts = dict(zip(args[2::2], args[3::2]))
run = json.load(open(opts["--run"]))
A = json.load(open(opts["--assumptions"]))
fake = run["fake"]
H = A["horizon_s"]
lim = {{l["component_id"]: l for l in A["service_limits"]}}
f = lim["blanket"]["limit"] / 1e26
periods = {{"magnets": fake["magnet_period_s"], "blanket": fake["blanket_period_s"] * f}}

def dur(c, k):
    lst = lim[c].get("replacement_durations_s") or []
    return lst[k - 1] if k <= len(lst) else lim[c]["replacement_duration_s"]

events, ops, order = [], [], [0]
def ev(t, kind, comp=None):
    order[0] += 1
    events.append({{"time_s": t, "order": order[0], "kind": kind, "component_id": comp, "mass_kg": None, "note": ""}})

t, since, count = 0.0, {{c: 0.0 for c in periods}}, {{c: 0 for c in periods}}
ev(0.0, "operation_started")
op_start = 0.0
while True:
    nxt = min(periods[c] - since[c] for c in periods)
    if t + nxt >= H:
        ops.append((op_start, H))
        break
    t += nxt
    for c in periods:
        since[c] += nxt
    due = [c for c in periods if periods[c] - since[c] <= 1e-6]
    ops.append((op_start, t))
    ev(t, "operation_stopped")
    ends = []
    for c in due:
        count[c] += 1
        d = dur(c, count[c])
        ev(t, "replacement_started", c)
        if t + d <= H:
            ev(t + d, "replacement_completed", c)
        since[c] = 0.0
        ends.append(t + d)
    resume = max(ends)
    if resume >= H:
        break
    t = resume
    op_start = t
    ev(t, "operation_started")
events.sort(key=lambda e: (e["time_s"], e["order"]))
op_seconds = sum(b - a for a, b in ops)
points = {{0.0: True, H: False}}
for a, b in ops:
    points[a] = True
    points.setdefault(b, False)
snaps = []
for time in sorted(points):
    on = any(a <= time < b for a, b in ops)
    snaps.append({{"time_s": time, "operating": on, "power_fraction": 1.0 if on else 0.0,
                  "cumulative_net_electricity_mwh": None}})
snaps[-1]["cumulative_net_electricity_mwh"] = fake["net_mw"] * op_seconds / 3600.0
import hashlib
sha = hashlib.sha256(open(opts["--scenario"], "rb").read()).hexdigest()
out = {{"schema_version": "fake", "outcome": "horizon_completed", "assumptions": A,
       "driving_rates": {{"scenario_sha256": sha, "transport_artifact_sha256": run["raw_artifact_sha256"]}},
       "events": events, "snapshots": snaps}}
import os
assert not os.path.exists(opts["--output"])
json.dump(out, open(opts["--output"], "w"))
'''

FAKE_ACTINV = r'''#!{python}
import json, os, sys
a = sys.argv[1:]
if a[0] == "--version":
    print("actinv 1.4.0-fake")
    sys.exit(0)
if a[0] == "validate":
    sys.exit(0)
spec = json.load(open(a[1]))
model = json.load(open(os.environ["FAKE_ACTINV_MODEL"]))
words = spec["title"].split()
comp = words[1]
amp = model["amp"][comp] * spec["spectrum"]["total"] / 1e9
p = model.get("power", 1.0)
t, last, steps = 0.0, 0.0, []
for i, s in enumerate(spec["schedule"]):
    t += float(s["dt"].split()[0])
    if s["flux"] > 0:
        last, heat = t, amp
    else:
        heat = amp * (max(t - last, 3600.0) / 3600.0) ** (-p)
    dose = heat * 10.0 if model.get("dose") else None
    steps.append({{"step": i + 1, "t_s": t, "flux": s["flux"], "heat_W_per_g": {{"total": heat}},
                  "photon_source": {{"contact_gamma_air_dose_proxy_Gy_h": dose}}}})
json.dump({{"steps": steps}}, open(a[2], "w"))
'''


def write_exe(path: Path, text: str):
    path.write_text(text.format(python=sys.executable))
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


class Rig:
    """Synthetic inputs: fake faris and actinv, a fake library, per-case run records."""

    COMPONENTS = ["first-wall", "blanket", "shield", "vessel", "magnets"]

    def __init__(self, root: Path, case_params: dict, sweep_labels=("0.30", "0.40"), f_values=(0.8, 1.0),
                 w_values=(0.5,), model=None, horizon_y=5.0, protocol_text=None):
        self.root = root
        write_exe(root / "faris", FAKE_FARIS)
        write_exe(root / "actinv", FAKE_ACTINV)
        (root / "model.json").write_text(json.dumps(model or {"amp": {c: 1e-9 for c in self.COMPONENTS}, "power": 1.0}))
        os.environ["FAKE_ACTINV_MODEL"] = str(root / "model.json")
        bounds, npy = TB.write_npy_bounds()
        activation = root / "data" / BUILD.CATALOG_VERSION / "activation"
        activation.mkdir(parents=True)
        with zipfile.ZipFile(activation / f"{BUILD.LIBRARY_ID}.npz", "w") as archive:
            archive.writestr("bounds.npy", npy)
        (activation / f"{BUILD.LIBRARY_ID}_index.json").write_text(json.dumps({"sha256_npz": "ab" * 32}))
        (root / "scenario.json").write_text('{"id": "synthetic"}\n')
        scenario_hash = hashlib.sha256((root / "scenario.json").read_bytes()).hexdigest()
        physics = {
            "materials": [{"id": "cu", "recipe": {"kind": "nuclide_mixture", "density_kg_m3": 9000.0,
                                                  "nuclides": [{"nuclide": "Cu63", "atom_fraction": 0.7},
                                                               {"nuclide": "Cu65", "atom_fraction": 0.3}]}}],
            "component_assignments": [{"component_id": c, "material_id": "cu"} for c in self.COMPONENTS],
        }
        (root / "physics.json").write_text(json.dumps(physics))
        physics_hash = hashlib.sha256((root / "physics.json").read_bytes()).hexdigest()
        year = 365.25 * DAY
        base = json.loads(Path(MC.REPO / "scenarios/arc-inspired/demountable-magnet-assumptions.json").read_text())
        base["horizon_s"] = horizon_y * year
        base["operation"][0]["end_s"] = horizon_y * year
        base["planned_outages"] = []
        base["service_limits"] = [l for l in base["service_limits"] if l["response_id"] != "magnets-outboard-fast-flux"
                                  and l["response_id"] != "magnets-port-sector-fast-flux"]
        (root / "assumptions.json").write_text(json.dumps(base))
        self.horizon_s = horizon_y * year

        def case(name, params):
            safe = name.replace("/", "_")
            factor = params.get("flux_factor", 1.0)
            # the recorded run sets the flux magnitude; the new run only supplies the spectrum shape (and a
            # different total, which the scaling must remove)
            results = [{"response_id": f"{c}-flux", "domain": {"kind": "component", "component_id": c},
                        "score": {"kind": "flux"}, "mean": factor * 1e13, "volume_m3": 1.0} for c in self.COMPONENTS]
            run = {"scenario_sha256": scenario_hash, "physics_sha256": physics_hash,
                   "raw_artifact_sha256": f"artifact-{safe}",
                   "normalized": {"results": results}, "normalized_spectra": None,
                   "fake": {"net_mw": params.get("net_mw", 100.0),
                            "magnet_period_s": params.get("magnet_period_y", 2.0) * year,
                            "blanket_period_s": params.get("blanket_period_y", 1.0) * year}}
            (root / f"{safe}.run.json").write_text(json.dumps(run))
            shape = [1.0] * 709
            shape[300] = 800.0
            total = params.get("spectrum_scale", 0.7) * 1e13
            means = [total * v / sum(shape) for v in shape]
            spectra = [{"component_id": c, "particle": "neutron", "energy_edges_ev": [0.0] + bounds[1:],
                        "mean_per_square_metre_second": means,
                        "standard_error_per_square_metre_second": [m * 0.05 for m in means]} for c in self.COMPONENTS]
            spectrum_run = {"scenario_sha256": params.get("spectrum_scenario_sha256", scenario_hash),
                            "physics_sha256": physics_hash, "normalized": {"results": []},
                            "normalized_spectra": spectra}
            (root / f"{safe}.spectrum-run.json").write_text(json.dumps(spectrum_run))
            return {"scenario": "scenario.json", "physics": "physics.json",
                    "history_run": f"{safe}.run.json", "spectrum_run": f"{safe}.spectrum-run.json"}

        arrangements = {n: case(n, case_params.get(n, {})) for n in MC.ARRANGEMENTS}
        sweep = {k: case(f"sweep/{k}", case_params.get(f"sweep/{k}", {})) for k in sweep_labels}
        protocol = root / "protocol.md"
        protocol.write_bytes(MC.DEFAULT_PROTOCOL.read_bytes() if protocol_text is None else protocol_text)
        self.config = {
            "faris": "faris", "actinv": "actinv", "data_dir": "data", "assumptions": "assumptions.json",
            "output_dir": "runs", "result": "result.json", "protocol": "protocol.md",
            "arrangements": arrangements, "sweep": sweep, "allow_reduced_grid": True,
            "w_values": list(w_values), "f_values": list(f_values),
        }
        self.config_path = root / "config.json"
        self.write_config()

    def write_config(self):
        self.config_path.write_text(json.dumps(self.config))

    def run(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = MC.main(["--config", str(self.config_path)])
        return code, err.getvalue()

    def result(self):
        return json.loads((self.root / "result.json").read_text())


def power_curve(amp, p=1.0, start=3600.0, n=40, end=365 * DAY):
    grid = MC.cooling_grid_s()
    return [(t, amp * (t / 3600.0) ** (-p)) for t in grid]


class CurveTests(unittest.TestCase):
    def test_grid_is_forty_log_spaced_points_from_one_hour_to_a_year(self):
        g = MC.cooling_grid_s()
        self.assertEqual(len(g), 40)
        self.assertEqual(g[0], 3600.0)
        self.assertEqual(g[-1], 365 * DAY)
        ratios = [b / a for a, b in zip(g, g[1:])]
        self.assertLess(max(ratios) - min(ratios), 1e-9)

    def test_cooldown_interpolates_log_linearly_between_points(self):
        curve = [(10.0, 100.0), (100.0, 10.0), (1000.0, 1.0)]
        self.assertAlmostEqual(MC.cooldown(curve, 100.0 / math.sqrt(10))["cooldown_s"], 10 * math.sqrt(10), places=6)
        # not a power law between the points: the value at the log-midpoint is the geometric mean
        curve = [(10.0, 100.0), (1000.0, 1.0)]
        self.assertAlmostEqual(MC.cooldown(curve, 10.0)["cooldown_s"], 100.0, places=6)
        self.assertAlmostEqual(MC.interpolate(curve, 100.0), 10.0, places=9)

    def test_cooldown_reports_first_crossing_and_edge_cases(self):
        curve = [(10.0, 100.0), (100.0, 10.0)]
        self.assertEqual(MC.cooldown(curve, 100.0)["cooldown_s"], 10.0)  # already at q* at the first sample
        self.assertEqual(MC.cooldown(curve, 500.0)["cooldown_s"], 10.0)
        window = MC.cooldown(curve, 1.0)
        self.assertTrue(window["window_limited"])
        self.assertEqual(window["cooldown_s"], 100.0)
        full = [(3600.0, 10.0), (365 * DAY, 5.0)]
        ne = MC.cooldown(full, 1.0)
        self.assertEqual(ne["status"], "NOT_EVALUATED")
        self.assertIn("365 days", ne["reason"])

    def test_combined_curve_is_total_heat_over_total_volume_on_the_common_range(self):
        series = {"a": [(10.0, 100.0), (100.0, 10.0)], "b": [(10.0, 50.0), (50.0, 20.0), (1000.0, 1.0)]}
        curve = MC.combined_curve(series, {"a": 2.0, "b": 3.0})
        self.assertEqual([t for t, _ in curve], [10.0, 50.0, 100.0])
        self.assertAlmostEqual(curve[0][1], 150.0 / 5.0)
        self.assertAlmostEqual(curve[1][1], (MC.interpolate(series["a"], 50.0) + 20.0) / 5.0)

    def test_calibration_hits_the_target_cooldown(self):
        curve = power_curve(1e3)
        for target in (30 * DAY, 60 * DAY, 90 * DAY):
            cal = MC.calibrate(curve, target)
            self.assertEqual(cal["status"], "EVALUATED")
            self.assertAlmostEqual(MC.cooldown(curve, cal["q_star"])["cooldown_s"], target, delta=1.0)

    def test_calibration_refuses_a_target_outside_the_curve_and_a_non_monotone_curve(self):
        short = [(3600.0, 10.0), (10 * DAY, 1.0)]
        self.assertEqual(MC.calibrate(short, 60 * DAY)["status"], "NOT_EVALUATED")
        bump = [(1.0 * DAY, 10.0), (2.0 * DAY, 1.0), (3.0 * DAY, 8.0), (4.0 * DAY, 0.5)]
        self.assertEqual(MC.calibrate(bump, 3.0 * DAY)["status"], "NOT_EVALUATED")


class DecisionTests(unittest.TestCase):
    ARR = {"no-port/reference": 100.0, "no-port/breeder": 99.0, "port/reference": 110.0, "port/breeder": 105.0}

    def test_d1_swap_threshold_is_two_percent_of_the_larger_value(self):
        fixed = dict(self.ARR)
        for gap, expect in ((0.019, False), (0.021, True)):
            # port/breeder overtakes port/reference by `gap` of the larger value; the others keep their order
            larger = 110.0
            computed = {"no-port/reference": 60.0, "no-port/breeder": 59.0, "port/reference": larger * (1 - gap),
                        "port/breeder": larger}
            d = MC.decide_d1(fixed, computed)
            self.assertEqual(d["changed"], expect, gap)
            self.assertEqual(d["swapped_pairs"][0]["pair"], ["port/breeder", "port/reference"])
        same = MC.decide_d1(fixed, dict(fixed))
        self.assertFalse(same["changed"])
        self.assertEqual(same["swapped_pairs"], [])

    def test_d1_needs_every_swapped_pair_to_clear_the_threshold(self):
        fixed = {"a": 100.0, "b": 90.0, "c": 80.0}
        computed = {"a": 80.0, "b": 99.0, "c": 100.0}  # a/b swap (gap ~19 %), a/c swap and b/c swap (1 %)
        self.assertFalse(MC.decide_d1(fixed, computed)["changed"])
        computed = {"a": 80.0, "b": 100.0, "c": 90.0}  # a/b and a/c swap by > 2 %, b/c keeps order
        self.assertTrue(MC.decide_d1(fixed, computed)["changed"])

    def test_d2_optimum_must_move_by_more_than_one_percent(self):
        fixed = {"0.30": 10.0, "0.40": 12.0, "0.50": 11.0}
        for gap, expect in ((0.009, False), (0.011, True)):
            computed = {"0.30": 10.0, "0.40": 12.0 * (1 - gap), "0.50": 12.0}
            d = MC.decide_d2(fixed, computed)
            self.assertEqual(d["changed"], expect, gap)
            self.assertEqual((d["fixed_optimum"], d["computed_optimum"]), ("0.40", "0.50"))
        same = MC.decide_d2(fixed, {"0.30": 1.0, "0.40": 5.0, "0.50": 4.0})
        self.assertFalse(same["changed"])

    def downtime(self, fixed_diff_days, ratio):
        fixed = {"no-port/reference": 100 * DAY, "no-port/breeder": 100 * DAY, "port/reference": 100 * DAY,
                 "port/breeder": (100 + fixed_diff_days) * DAY}
        computed = dict(fixed)
        computed["port/breeder"] = 100 * DAY + fixed_diff_days * DAY * ratio
        return fixed, computed

    def test_d3_ratio_band_and_the_thirty_day_floor(self):
        row = lambda d: [r for r in d["contrasts"] if r["contrast"] == "breeder-minus-reference, port"][0]
        for ratio, expect in ((0.79, True), (0.81, False), (1.24, False), (1.26, True)):
            d = MC.decide_d3(*self.downtime(40, ratio))
            self.assertEqual(row(d)["changed"], expect, ratio)
            self.assertEqual(d["changed"], expect)
        small = MC.decide_d3(*self.downtime(29, 3.0))  # fixed difference below 30 days: not considered
        self.assertFalse(row(small)["considered"])
        self.assertFalse(small["changed"])
        self.assertTrue(row(MC.decide_d3(*self.downtime(30, 3.0)))["considered"])

    def test_d3_sign_reversal_is_a_change(self):
        fixed, computed = self.downtime(40, -0.5)
        self.assertTrue(MC.decide_d3(fixed, computed)["changed"])

    def test_d4_needs_best_f_at_most_point_nine_and_a_gain_over_one_percent(self):
        fixed = {0.8: 90.0, 0.9: 95.0, 1.0: 100.0}
        for gain, expect in ((1.009, False), (1.011, True)):
            d = MC.decide_d4(fixed, {0.8: 90.0, 0.9: 100.0 * gain, 1.0: 100.0})
            self.assertEqual(d["changed"], expect, gain)
            self.assertEqual(d["computed_best_f"], 0.9)
        d = MC.decide_d4(fixed, {0.8: 90.0, 0.9: 95.0, 1.0: 100.0})
        self.assertFalse(d["changed"])
        self.assertEqual(d["computed_best_f"], 1.0)
        self.assertEqual(MC.decide_d4(fixed, {0.8: None, 0.9: 1.0, 1.0: 1.0})["status"], "NOT_EVALUATED")

    def test_missing_values_make_a_decision_not_evaluated(self):
        self.assertEqual(MC.decide_d1(dict(self.ARR), {**self.ARR, "port/breeder": None})["status"], "NOT_EVALUATED")
        self.assertEqual(MC.decide_d2({"a": 1.0}, {"a": None})["status"], "NOT_EVALUATED")

    def test_verdict_material_not_material_and_mixed(self):
        ev = lambda changed: {"status": "EVALUATED", "changed": changed}
        quiet = {n: ev(False) for n in ("D1", "D2", "D3", "D4")}
        self.assertEqual(MC.verdict({"0.25": quiet, "0.5": quiet, "0.75": quiet})["verdict"], "NOT MATERIAL")
        central = {**quiet, "D2": ev(True)}
        v = MC.verdict({"0.25": quiet, "0.5": central, "0.75": quiet})
        self.assertEqual(v["verdict"], "MATERIAL")
        self.assertEqual(v["changed_at_central_w"], ["D2"])
        v = MC.verdict({"0.25": central, "0.5": quiet, "0.75": central})
        self.assertEqual(v["verdict"], "MIXED")
        self.assertEqual(v["changed_at_w"], {"D2": ["0.25", "0.75"]})
        unknown = {**quiet, "D4": {"status": "NOT_EVALUATED", "reason": "x"}}
        v = MC.verdict({"0.25": quiet, "0.5": unknown, "0.75": quiet})
        self.assertEqual(v["verdict"], "NOT_EVALUATED")
        self.assertEqual(v["not_evaluated"], [{"w": "0.5", "decision": "D4"}])
        self.assertEqual(MC.verdict({"0.25": quiet, "0.5": {**unknown, "D1": ev(True)}, "0.75": quiet})["verdict"], "MATERIAL")


class HistoryHelperTests(unittest.TestCase):
    def test_events_are_counted_per_component_and_downtime_sums_started_to_completed(self):
        history = TB.make_history([(0, 100), (200, 400)], replacements=[("blanket", 100, 200), ("magnets", 100, 180)],
                                  horizon=500)
        evs = MC.replacement_events(history, MC.CLASSES)
        self.assertEqual([(e["component"], e["k"], e["start_s"], e["end_s"]) for e in evs],
                         [("blanket", 1, 100.0, 200.0), ("magnets", 1, 100.0, 180.0)])
        self.assertEqual(MC.total_downtime_s(history, MC.CLASSES), 180.0)

    def test_derived_assumptions_write_durations_to_every_limit_of_a_component_and_scale_the_blanket(self):
        base = json.loads((MC.REPO / "scenarios/arc-inspired/demountable-magnet-assumptions.json").read_text())
        derived = MC.derive_assumptions(base, MC.CLASSES, 0.7, {"magnets": [1.0, 2.0]})
        for lim in derived["service_limits"]:
            if lim["component_id"] == "magnets":
                self.assertEqual(lim["replacement_durations_s"], [1.0, 2.0])
            else:
                self.assertNotIn("replacement_durations_s", lim)
        blanket = [l for l in derived["service_limits"] if l["component_id"] == "blanket"][0]
        self.assertAlmostEqual(blanket["limit"], 0.7e26)
        self.assertEqual(MC.fixed_durations_s(base, MC.CLASSES), {"magnet": 120 * DAY, "blanket": 60 * DAY})


class RefusalTests(unittest.TestCase):
    def test_a_protocol_whose_body_was_edited_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {}, protocol_text=MC.DEFAULT_PROTOCOL.read_bytes().replace(b"2 %", b"3 %", 1))
            code, err = rig.run()
            self.assertEqual(code, 2)
            self.assertIn("refusing to run", err)
            self.assertFalse((Path(d) / "runs").exists())

    def test_the_recorded_protocol_body_matches_the_file(self):
        self.assertEqual(MC.protocol_body_sha256(MC.DEFAULT_PROTOCOL), MC.PROTOCOL_BODY_SHA256)

    def test_an_appended_amendment_is_accepted_but_a_body_edit_is_refused(self):
        body = MC.DEFAULT_PROTOCOL.read_bytes()
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "protocol.md"
            path.write_bytes(body + b"\n### Amendment 99\n\nAppended.\n")
            self.assertEqual(MC.protocol_body_sha256(path), MC.PROTOCOL_BODY_SHA256)
            path.write_bytes(body.replace(b"2 %", b"3 %", 1))
            self.assertNotEqual(MC.protocol_body_sha256(path), MC.PROTOCOL_BODY_SHA256)

    def test_an_existing_output_directory_or_result_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {})
            (Path(d) / "runs").mkdir()
            code, err = rig.run()
            self.assertEqual((code, "already exists" in err), (2, True))
            (Path(d) / "runs").rmdir()
            (Path(d) / "result.json").write_text("{}")
            code, err = rig.run()
            self.assertEqual((code, "already exists" in err), (2, True))

    def test_the_protocol_grid_is_enforced_unless_reduced_explicitly(self):
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {})
            del rig.config["allow_reduced_grid"]
            rig.write_config()
            code, err = rig.run()
            self.assertEqual(code, 2)
            self.assertIn("7 points", err)


class StubRunner:
    """History and activation made in memory: one magnet replacement whose cooldown depends on the durations used."""

    classes = {"magnet": {"component_id": "magnets", "governing": ["first-wall"]},
               "blanket": {"component_id": "blanket", "governing": ["first-wall", "blanket"]}}

    def __init__(self, cooldown_of_used):
        self.cooldown_of_used = cooldown_of_used
        self.horizon = 1e8
        self.start = 1e7

    def history(self, name, case, f, used):
        d = used.get("magnets", [120 * DAY])[0]
        events = [TB.event(self.start, "replacement_started", "magnets", 1),
                  TB.event(self.start + d, "replacement_completed", "magnets", 2)]
        snaps = [{"time_s": 0.0, "operating": True, "power_fraction": 1.0},
                 {"time_s": self.horizon, "operating": True, "power_fraction": 1.0, "cumulative_net_electricity_mwh": 5.0}]
        return {"data": {"outcome": "horizon_completed", "assumptions": {"horizon_s": self.horizon},
                         "events": events, "snapshots": snaps},
                "assumptions": Path("a.json"), "history_path": Path("h.json")}

    def activation(self, name, case, f, used, variant):
        k = self.cooldown_of_used(used.get("magnets", [120 * DAY])[0])
        grid = MC.cooling_grid_s()
        points = [(self.start + t, k / t * 3600.0 / 3600.0, None, 0.0) for t in grid]  # q(tau) = k / tau; q* = 1
        return {"first-wall": [{"install_s": 0.0, "remove_s": self.horizon, "volume_m3": 1.0, "points": points}]}


class CouplingLoopTests(unittest.TestCase):
    TH = {"magnet": {"status": "EVALUATED", "q_star": 1.0}, "blanket": {"status": "EVALUATED", "q_star": 1.0}}

    def case(self, cooldown_of_used):
        runner = StubRunner(cooldown_of_used)
        return MC.coupled_case(runner, "x", {}, 1.0, self.TH, 0.5, {"magnet": 120 * DAY, "blanket": 60 * DAY}, MC.BARE)

    def test_a_fixed_point_converges_when_no_duration_moves_by_more_than_a_day(self):
        res = self.case(lambda d: 60 * DAY)  # cooldown independent of the durations; duration = 60 + 60 = fixed
        self.assertEqual(res["status"], "EVALUATED")
        self.assertEqual(res["converged_at_iteration"], 1)
        res = self.case(lambda d: 90 * DAY)  # duration 150 d at iteration 1, unchanged at iteration 2
        self.assertEqual(res["converged_at_iteration"], 2)
        self.assertAlmostEqual(res["durations_s"]["magnets"][0], 150 * DAY, delta=1.0)

    def test_the_one_day_tolerance_is_inclusive_at_the_boundary(self):
        # iteration 1 moves 120 d -> 160 d; iteration 2 then moves by `delta`
        for delta, converged_at in ((0.99 * DAY, 2), (1.01 * DAY, 3)):
            res = self.case(lambda d, delta=delta: 100 * DAY if d <= 120 * DAY else 100 * DAY + delta)
            self.assertEqual(res["converged_at_iteration"], converged_at, delta / DAY)

    def test_non_convergence_is_not_evaluated_with_the_last_two_duration_sets(self):
        # duration 120 d -> cooldown 140 d (new 200 d); 200 d -> cooldown 40 d (new 100 d); and so on
        res = self.case(lambda d: 140 * DAY if d <= 150 * DAY else 40 * DAY)
        self.assertEqual(res["status"], "NOT_EVALUATED")
        self.assertIn("no convergence in 5 iterations", res["reason"])
        self.assertEqual(len(res["iterations"]), 5)
        a, b = res["last_two_duration_sets_s"]
        self.assertAlmostEqual(a["magnets"][0], 100 * DAY, delta=1.0)
        self.assertAlmostEqual(b["magnets"][0], 200 * DAY, delta=1.0)

    def test_a_curve_beyond_a_year_ends_the_case_with_the_reason(self):
        res = self.case(lambda d: 400 * DAY)
        self.assertEqual(res["status"], "NOT_EVALUATED")
        self.assertIn("365 days", res["reason"])


class EndToEndTests(unittest.TestCase):
    """Fake faris and actinv; the decay curve is amp x (t / 1 h)^-1 per component, amp scaled by the run's spectrum."""

    def test_uniform_physics_reproduces_the_fixed_durations_and_is_not_material(self):
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {"port/breeder": {"net_mw": 99.0}}, w_values=(0.5,))
            code, err = rig.run()
            self.assertEqual(code, 0, err)
            res = rig.result()
            self.assertEqual(res["verdict"]["verdict"], "NOT MATERIAL")
            self.assertEqual(res["protocol"]["body_sha256"], MC.PROTOCOL_BODY_SHA256)
            self.assertEqual(res["tools"]["actinv"]["version"], "actinv 1.4.0-fake")
            self.assertEqual(res["tools"]["activation_library"]["library_sha256"], "ab" * 32)
            w = res["computed_model"][MC.BARE]["w"]["0.5"]
            case = w["cases"]["port/reference"]
            self.assertEqual(case["status"], "EVALUATED")
            self.assertEqual(case["converged_at_iteration"], 1)
            # calibration: the first replacement of each class has exactly the fixed duration
            first = {}
            for e in case["iterations"][0]["events"]:
                first.setdefault(e["class"], e)
            self.assertAlmostEqual(first["magnet"]["duration_computed_s"], 120 * DAY, delta=1.0)
            self.assertAlmostEqual(first["blanket"]["duration_computed_s"], 60 * DAY, delta=1.0)
            self.assertAlmostEqual(first["magnet"]["work_s"], 60 * DAY, delta=1e-6)
            # amp 1e-9 W/g x 9e6 g per component, 1 m3 each: q(tau) = 9e-3 / (tau / 1 h) W/m3 per component volume
            self.assertAlmostEqual(w["q_star"]["magnet"]["q_star"], 9e-3 / (60 * DAY / 3600.0), delta=1e-9)
            self.assertAlmostEqual(w["q_star"]["blanket"]["q_star"], 9e-3 / (30 * DAY / 3600.0), delta=1e-9)
            for name, c in w["cases"].items():
                self.assertEqual(c["status"], "EVALUATED", name)
                for e in c["iterations"][-1]["events"]:
                    self.assertAlmostEqual(e["duration_computed_s"], e["duration_used_s"], delta=1.0)
            # derived assumptions record per-component durations and every input has a hash
            self.assertEqual(len(res["inputs_sha256"]["cases"]), 6)
            self.assertTrue(all(len(h) == 64 for v in res["inputs_sha256"]["cases"].values() for h in v.values()))
            self.assertEqual(list(res), sorted(res))

    def test_hotter_arrangement_gets_longer_durations_that_change_downtime_contrasts(self):
        params = {
            "port/breeder": {"flux_factor": 1.8, "blanket_period_y": 0.8, "net_mw": 100.0},
            "no-port/breeder": {"flux_factor": 1.8, "blanket_period_y": 0.8},
        }
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), params, w_values=(0.25, 0.5), f_values=(1.0,), sweep_labels=("0.30", "0.40"))
            code, err = rig.run()
            self.assertEqual(code, 0, err)
            res = rig.result()
            central = res["computed_model"][MC.BARE]["w"]["0.5"]
            hot = central["cases"]["port/breeder"]
            self.assertEqual(hot["status"], "EVALUATED")
            # cooldown scales with the heat amplitude: 1.8 x 60 d; duration = work 60 d + cooldown
            durations = hot["durations_s"]["magnets"]
            self.assertAlmostEqual(durations[0], 60 * DAY + 1.8 * 60 * DAY, delta=60.0)
            self.assertAlmostEqual(hot["durations_s"]["blanket"][0], 30 * DAY + 1.8 * 30 * DAY, delta=60.0)
            self.assertEqual(hot["converged_at_iteration"], 2)  # iteration 1 used the fixed durations
            self.assertEqual(central["cases"]["port/reference"]["converged_at_iteration"], 1)
            self.assertTrue(central["decisions"]["D3"]["changed"])
            self.assertEqual(res["verdict"]["verdict"], "MATERIAL")
            self.assertIn("D3", res["verdict"]["changed_at_central_w"])
            # w = 0.25: the cooldown (162 d) outruns the 120 d outage window at first; the window grows until it is seen
            early = res["computed_model"][MC.BARE]["w"]["0.25"]["cases"]["port/breeder"]
            self.assertEqual(early["status"], "EVALUATED")
            self.assertGreater(early["converged_at_iteration"], 2)
            self.assertTrue(early["iterations"][0]["window_limited"])
            self.assertAlmostEqual(early["durations_s"]["magnets"][0], 30 * DAY + 1.8 * 90 * DAY, delta=60.0)

    def test_result_carries_both_run_hashes_and_the_spectrum_error_summary(self):
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {}, f_values=(1.0,), sweep_labels=("0.30",))
            self.assertEqual(rig.run()[0], 0)
            res = rig.result()
            hashes = res["inputs_sha256"]["cases"]["arrangement:port/reference"]
            root = Path(d)
            self.assertEqual(hashes["history_run"], hashlib.sha256((root / "port_reference.run.json").read_bytes()).hexdigest())
            self.assertEqual(hashes["spectrum_run"],
                             hashlib.sha256((root / "port_reference.spectrum-run.json").read_bytes()).hexdigest())
            self.assertNotEqual(hashes["history_run"], hashes["spectrum_run"])
            case = res["computed_model"][MC.BARE]["w"]["0.5"]["cases"]["port/reference"]
            self.assertEqual(sorted(case["spectrum_error"]), sorted(Rig.COMPONENTS))
            err = case["spectrum_error"]["magnets"]
            self.assertEqual(err["spectrum_run_sha256"], hashes["spectrum_run"])
            self.assertAlmostEqual(err["scale_factor"], 1.0 / 0.7, places=9)
            self.assertAlmostEqual(err["flux_weighted_mean_relative_error"], 0.05, places=9)
            self.assertAlmostEqual(err["max_relative_error_groups_ge_1pct"], 0.05, places=9)

    def test_a_spectrum_run_from_another_scenario_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {"port/breeder": {"spectrum_scenario_sha256": "0" * 64}}, f_values=(1.0,),
                      sweep_labels=("0.30",))
            code, err = rig.run()
            self.assertEqual(code, 2)
            self.assertIn("scenario_sha256", err)

    def test_derived_assumptions_carry_the_computed_durations(self):
        params = {"port/breeder": {"flux_factor": 1.5}}
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), params, f_values=(1.0,), sweep_labels=("0.30",))
            self.assertEqual(rig.run()[0], 0)
            res = rig.result()
            case = res["computed_model"][MC.BARE]["w"]["0.5"]["cases"]["port/breeder"]
            derived = json.loads(Path(case["assumptions"]).read_text())
            magnets = [l for l in derived["service_limits"] if l["component_id"] == "magnets"]
            self.assertTrue(all(l["replacement_durations_s"] == magnets[0]["replacement_durations_s"] for l in magnets))
            self.assertAlmostEqual(magnets[0]["replacement_durations_s"][0], 60 * DAY + 1.5 * 60 * DAY, delta=60.0)

    def test_a_curve_that_never_falls_below_q_star_in_a_year_is_not_evaluated(self):
        model = {"amp": {c: 1e-9 for c in Rig.COMPONENTS}, "power": 0.05}
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {"port/breeder": {"flux_factor": 3.0}}, f_values=(1.0,), sweep_labels=("0.30",),
                      model=model)
            self.assertEqual(rig.run()[0], 0)
            w = rig.result()["computed_model"][MC.BARE]["w"]["0.5"]
            self.assertEqual(w["cases"]["port/reference"]["status"], "EVALUATED")
            hot = w["cases"]["port/breeder"]
            self.assertEqual(hot["status"], "NOT_EVALUATED")

    def test_contact_dose_cross_check_is_calibrated_the_same_way_and_reported_apart(self):
        model = {"amp": {c: 1e-9 for c in Rig.COMPONENTS}, "power": 1.0, "dose": True}
        with tempfile.TemporaryDirectory() as d:
            rig = Rig(Path(d), {"port/breeder": {"flux_factor": 1.5}}, f_values=(1.0,), sweep_labels=("0.30",),
                      model=model)
            self.assertEqual(rig.run()[0], 0)
            w = rig.result()["computed_model"][MC.BARE]["w"]["0.5"]
            magnet = lambda case: [c for c in w["cases"][case]["dose_cross_check"] if c["component"] == "magnets"][0]
            self.assertEqual(magnet("port/reference")["status"], "EVALUATED")
            self.assertAlmostEqual(magnet("port/reference")["implied_duration_s"], 120 * DAY, delta=60.0)
            self.assertAlmostEqual(magnet("port/breeder")["implied_duration_s"], 60 * DAY + 1.5 * 60 * DAY, delta=60.0)
        with tempfile.TemporaryDirectory() as d:  # no dose in the results: reported as such
            rig = Rig(Path(d), {}, f_values=(1.0,), sweep_labels=("0.30",))
            self.assertEqual(rig.run()[0], 0)
            ref = rig.result()["computed_model"][MC.BARE]["w"]["0.5"]["cases"]["port/reference"]["dose_cross_check"][0]
            self.assertEqual(ref["status"], "NOT_EVALUATED")


if __name__ == "__main__":
    unittest.main()
