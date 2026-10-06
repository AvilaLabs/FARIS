import bisect
import contextlib
import hashlib
import importlib.util
import io
import json
import math
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "build_activation_inputs", Path(__file__).with_name("build_activation_inputs.py")
)
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)

HORIZON = 1000.0


def event(time_s, kind, component=None, order=0):
    return {"time_s": time_s, "order": order, "kind": kind, "component_id": component}


def make_history(operating, replacements=(), power=1.0, horizon=HORIZON):
    """operating: [(start, end)]; replacements: [(component, started, completed)]."""
    events = []
    for a, b in operating:
        events.append(event(a, "operation_started"))
        events.append(event(b, "operation_stopped"))
    for component, started, completed in replacements:
        events.append(event(started, "replacement_started", component))
        events.append(event(completed, "replacement_completed", component))
    times = sorted({0.0, horizon, *(t for a, b in operating for t in (a, b)), *range(0, int(horizon), 100)})
    snapshots = []
    starts = [a for a, _ in sorted(operating)]
    ordered = sorted(operating)
    for t in times:
        k = bisect.bisect_right(starts, t) - 1
        on = k >= 0 and ordered[k][0] <= t < ordered[k][1]
        snapshots.append({"time_s": float(t), "operating": on, "power_fraction": power if on else 0.0})
    return {
        "outcome": "horizon_completed",
        "assumptions": {"horizon_s": horizon, "operation": [{"start_s": 0.0, "end_s": horizon, "power_fraction": power}]},
        "driving_rates": {"scenario_sha256": None, "transport_artifact_sha256": None},
        "events": events,
        "snapshots": snapshots,
    }


def write_npy_bounds(count=710):
    bounds = [1e-5 * (1e14 ** (i / (count - 1))) for i in range(count)]
    header = "{'descr': '<f8', 'fortran_order': False, 'shape': (%d,), }" % count
    header += " " * ((64 - (10 + len(header) + 1) % 64) % 64) + "\n"
    return bounds, b"\x93NUMPY\x01\x00" + struct.pack("<H", len(header)) + header.encode() + struct.pack(f"<{count}d", *bounds)


def run_record(scenario_hash, with_709=False, bounds=None):
    results = [{"response_id": "magnets-flux", "domain": {"kind": "component", "component_id": "magnets"},
                "score": {"kind": "flux"}, "mean": 1.0e13, "volume_m3": 2.0}]
    spectra = None
    if with_709:
        spectra = [{"component_id": "magnets", "particle": "neutron", "energy_edges_ev": [0.0] + bounds[1:],
                    "mean_per_square_metre_second": [1.0e13 / 709] * 709}]
    return {"scenario_sha256": scenario_hash, "raw_artifact_sha256": None,
            "normalized": {"results": results}, "normalized_spectra": spectra}


def physics_doc():
    return {
        "materials": [
            {"id": "cu", "recipe": {"kind": "nuclide_mixture", "density_kg_m3": 9000.0,
                                    "nuclides": [{"nuclide": "Cu63", "atom_fraction": 0.7},
                                                 {"nuclide": "Cu65", "atom_fraction": 0.3}]}},
            {"id": "gap", "recipe": {"kind": "void"}},
        ],
        "component_assignments": [{"component_id": "magnets", "material_id": "cu"},
                                  {"component_id": "gap", "material_id": "gap"}],
    }


class Fixture:
    def __init__(self, root: Path, history, with_709=False):
        self.root = root
        self.bounds, npy = write_npy_bounds()
        activation = root / "data" / BUILD.CATALOG_VERSION / "activation"
        activation.mkdir(parents=True)
        with zipfile.ZipFile(activation / f"{BUILD.LIBRARY_ID}.npz", "w") as archive:
            archive.writestr("bounds.npy", npy)
        (activation / f"{BUILD.LIBRARY_ID}_index.json").write_text(json.dumps({"sha256_npz": "ab" * 32}))
        self.scenario = root / "scenario.json"
        self.scenario.write_text('{"id": "s"}\n')
        scenario_hash = hashlib.sha256(self.scenario.read_bytes()).hexdigest()
        history["driving_rates"]["scenario_sha256"] = scenario_hash
        self.run = root / "run.json"
        self.run.write_text(json.dumps(run_record(scenario_hash, with_709, self.bounds)))
        self.physics = root / "physics.json"
        self.physics.write_text(json.dumps(physics_doc()))
        self.history = root / "history.json"
        self.history.write_text(json.dumps(history))

    def argv(self, out="out", *extra):
        return ["--run", str(self.run), "--scenario", str(self.scenario), "--physics", str(self.physics),
                "--history", str(self.history), "--data-dir", str(self.root / "data"),
                "--output-dir", str(self.root / out), "--skip-validate", *extra]

    def main(self, out="out", *extra):
        with contextlib.redirect_stderr(io.StringIO()) as err, contextlib.redirect_stdout(io.StringIO()):
            code = BUILD.main(self.argv(out, *extra))
        return code, err.getvalue()


def specs_in(directory: Path, label=BUILD.BARE):
    return sorted(directory.glob(f"*__{label}.spec.json"))


class ScheduleTests(unittest.TestCase):
    def test_installations_split_at_replacements(self):
        history = make_history([(0, 1000)], replacements=[("magnets", 300.0, 400.0), ("magnets", 700.0, 750.0)])
        self.assertEqual(BUILD.installations(history, "magnets"),
                         [(0.0, 300.0), (400.0, 700.0), (750.0, HORIZON)])
        self.assertEqual(BUILD.installations(history, "other"), [(0.0, HORIZON)])

    def test_zero_flux_gaps_and_clipping_at_removal(self):
        history = make_history([(100, 300), (500, 900)], replacements=[("magnets", 600.0, 650.0)])
        power = BUILD.PowerSeries(history)
        intervals = BUILD.operating_intervals(history)
        steps, facts = BUILD.build_steps(intervals, power, 0.0, 600.0)
        self.assertEqual(steps, [(100.0, 0.0), (200.0, 1.0), (200.0, 0.0), (100.0, 1.0)])
        self.assertEqual(facts["operating_pieces"], 2)
        steps, _ = BUILD.build_steps(intervals, power, 650.0, HORIZON)
        self.assertEqual(steps, [(250.0, 1.0), (100.0, 0.0)])

    # Verifies: ACT-009
    def test_arbitrary_piecewise_history_keeps_flux_scaling_and_duration_per_segment(self):
        history = make_history([(0, 100), (150, 400), (410, 500), (600, 700)], power=0.5)
        steps, facts = BUILD.build_steps(BUILD.operating_intervals(history), BUILD.PowerSeries(history), 0.0, HORIZON)
        self.assertEqual(steps, [(100.0, 0.5), (50.0, 0.0), (250.0, 0.5), (10.0, 0.0), (90.0, 0.5),
                                 (100.0, 0.0), (100.0, 0.5), (300.0, 0.0)])
        self.assertAlmostEqual(sum(dt for dt, _ in steps), HORIZON)
        self.assertEqual(facts["max_power_fraction_deviation_from_snapshots"], 0.0)
        written = BUILD.schedule_json(steps)
        self.assertEqual(written[0], {"dt": "100.0 s", "flux": 0.5})

    # Verifies: ACT-009
    def test_ten_thousand_or_more_segments_are_accepted(self):
        pulses = [(10.0 * i, 10.0 * i + 4.0) for i in range(12000)]
        horizon = 10.0 * 12000
        history = make_history(pulses, horizon=horizon)
        steps, facts = BUILD.build_steps(BUILD.operating_intervals(history), BUILD.PowerSeries(history), 0.0, horizon)
        self.assertGreaterEqual(len(steps), 10_000)
        self.assertEqual(facts["steps_before_merging"], len(steps))
        self.assertEqual(len(BUILD.schedule_json(steps)), len(steps))

    def test_adjacent_identical_multipliers_merge_and_counts_are_recorded(self):
        # two operating intervals back to back (a restart with no time between) merge into one step
        history = make_history([(0, 200), (200, 500)])
        steps, facts = BUILD.build_steps(BUILD.operating_intervals(history), BUILD.PowerSeries(history), 0.0, HORIZON)
        self.assertEqual(steps, [(500.0, 1.0), (500.0, 0.0)])
        self.assertEqual((facts["steps_before_merging"], facts["steps_after_merging"]), (3, 2))

    def test_varying_power_uses_time_weighted_mean_and_reports_deviation(self):
        history = make_history([(0, 400)])
        for s in history["snapshots"]:
            if s["operating"]:
                s["power_fraction"] = 1.0 if s["time_s"] < 300 else 0.6
        steps, facts = BUILD.build_steps(BUILD.operating_intervals(history), BUILD.PowerSeries(history), 0.0, 400.0)
        self.assertAlmostEqual(steps[0][1], (300 * 1.0 + 100 * 0.6) / 400)
        self.assertAlmostEqual(facts["max_power_fraction_deviation_from_snapshots"], 0.9 - 0.6)

    def test_cooling_grid_cumulative_times_are_exact(self):
        grid = BUILD.parse_cooling_grid(BUILD.DEFAULT_COOLING)
        wanted = [1.0, 3600.0, 86400.0, 604800.0, 30 * 86400.0, BUILD.YEAR_S, 10 * BUILD.YEAR_S, 100 * BUILD.YEAR_S]
        self.assertEqual([t for _, t in grid], wanted)
        steps = BUILD.cooling_steps(grid)
        total, cumulative = 0.0, []
        for dt, multiplier in steps:
            self.assertEqual(multiplier, 0.0)
            total += dt
            cumulative.append(total)
        for got, want in zip(cumulative, wanted):
            self.assertAlmostEqual(got, want, delta=1e-6 * want)
        with self.assertRaises(BUILD.InputError):
            BUILD.parse_cooling_grid("1h,1s")


class SpecTests(unittest.TestCase):
    def test_refuses_without_709_group_spectrum_unless_flagged(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]))
            code, err = fx.main("refused")
            self.assertEqual(code, 3)
            self.assertIn("no 709-group spectrum in this run record; the 0.2 transport adds it", err)
            self.assertFalse((Path(d) / "refused").exists())
            code, _ = fx.main("placeholder", "--allow-placeholder-spectrum")
            self.assertEqual(code, 0)
            (spec_path,) = specs_in(Path(d) / "placeholder")
            spec = json.loads(spec_path.read_text())
            prov = json.loads(spec_path.with_name(spec_path.name.replace(".spec.", ".provenance.")).read_text())
            self.assertEqual(prov["spectrum"]["source"], BUILD.PLACEHOLDER)
            self.assertEqual(len(spec["spectrum"]["flux_per_group"]), 709)
            self.assertAlmostEqual(sum(spec["spectrum"]["flux_per_group"]), 1.0e13 * 1e-4, delta=1e-6)
            self.assertAlmostEqual(spec["material"]["mass_g"], 2.0 * 9000.0 * 1000.0)
            self.assertEqual(prov["schedule"]["total_step_count"], 1 + 8)
            self.assertEqual(sorted(p.name for p in (Path(d) / "placeholder").glob("gap*")), [])

    def test_uses_a_709_group_spectrum_when_the_run_has_one(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
            self.assertEqual(fx.main("real")[0], 0)
            (spec_path,) = specs_in(Path(d) / "real")
            spec = json.loads(spec_path.read_text())
            self.assertAlmostEqual(spec["spectrum"]["flux_per_group"][0], 1.0e13 / 709 * 1e-4)
            prov = json.loads(spec_path.with_name(spec_path.name.replace(".spec.", ".provenance.")).read_text())
            self.assertNotEqual(prov["spectrum"]["source"], BUILD.PLACEHOLDER)

    def test_refuses_an_existing_output_directory(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
            (Path(d) / "taken").mkdir()
            (Path(d) / "taken" / "keep.txt").write_text("x")
            code, err = fx.main("taken")
            self.assertEqual(code, 2)
            self.assertIn("already exists", err)
            self.assertEqual([p.name for p in (Path(d) / "taken").iterdir()], ["keep.txt"])

    def test_one_spec_per_installation_with_provenance_hashes(self):
        history = make_history([(0, 1000)], replacements=[("magnets", 400.0, 450.0)])
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), history, with_709=True)
            self.assertEqual(fx.main("two")[0], 0)
            specs = specs_in(Path(d) / "two")
            self.assertEqual(len(specs), 2)
            prov = json.loads(specs[1].with_name(specs[1].name.replace(".spec.", ".provenance.")).read_text())
            self.assertEqual(prov["installation_index"], 2)
            self.assertEqual(prov["installation_interval_s"], [450.0, 1000.0])
            self.assertEqual(prov["input_sha256"]["run_record"], hashlib.sha256(fx.run.read_bytes()).hexdigest())
            self.assertIsNone(prov["input_sha256"]["impurities"])
            self.assertEqual(prov["label"], BUILD.BARE)

    def test_impurity_variant_keeps_bare_ratios_and_labels_both_specs(self):
        impurities = {"cu": [{"element": "Co", "ppm": 100, "citation": "test"},
                             {"element": "Ag", "wt_fraction": 0.0005, "citation": "test"}]}
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
            path = Path(d) / "imp.json"
            path.write_text(json.dumps(impurities))
            self.assertEqual(fx.main("imp", "--impurities", str(path))[0], 0)
            bare = json.loads(specs_in(Path(d) / "imp", BUILD.BARE)[0].read_text())["material"]
            full = json.loads(specs_in(Path(d) / "imp", BUILD.WITH_IMPURITIES)[0].read_text())["material"]
            self.assertAlmostEqual(full["composition"]["Cu63"] / full["composition"]["Cu65"],
                                   bare["composition"]["Cu63"] / bare["composition"]["Cu65"], places=12)
            self.assertEqual(bare["mass_g"], full["mass_g"])
            # mass fractions: bare part is 1 - sum(w), impurities as stated
            mass = {k: v * (BUILD.ATOMIC_WEIGHT[k] if k in BUILD.ATOMIC_WEIGHT else BUILD.nuclide_mass_number(k))
                    for k, v in full["composition"].items()}
            total = sum(mass.values())
            self.assertAlmostEqual(total, 1.0, places=12)
            self.assertAlmostEqual(mass["Co"] / total, 100e-6, places=12)
            self.assertAlmostEqual(mass["Ag"] / total, 0.0005, places=12)
            prov = json.loads(specs_in(Path(d) / "imp", BUILD.WITH_IMPURITIES)[0].with_name(
                "magnets__inst001__specification_maximum_impurities.provenance.json").read_text())
            self.assertEqual(prov["label"], BUILD.WITH_IMPURITIES)
            self.assertEqual(prov["impurities"]["entries"][0]["citation"], "test")
            self.assertIsNotNone(prov["input_sha256"]["impurities"])

    def test_impurity_overlapping_a_transport_element_is_refused(self):
        bare = {"Cu63": 0.7, "Cu65": 0.3}
        parsed = BUILD.parse_impurities([{"element": "Cu", "ppm": 5, "citation": "c"}], "cu")
        with self.assertRaises(BUILD.InputError):
            BUILD.composition_with_impurities(bare, parsed)
        with self.assertRaises(BUILD.InputError):
            BUILD.parse_impurities([{"element": "Co", "ppm": 5}], "cu")  # no citation

    def test_spectrum_edges_must_match_the_library(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
            run = json.loads(fx.run.read_text())
            run["normalized_spectra"][0]["energy_edges_ev"][5] *= 1.01
            fx.run.write_text(json.dumps(run))
            code, err = fx.main("bad")
            self.assertEqual(code, 2)
            self.assertIn("differ from the fispact-709 boundaries", err)

    def test_subdividing_outages_splits_zero_flux_steps_on_the_grid_and_keeps_cumulative_times(self):
        grid = BUILD.parse_cooling_grid("10s,100s,1000s")
        steps = [(50.0, 1.0), (500.0, 0.0), (20.0, 0.5), (5.0, 0.0)]
        got = BUILD.subdivide_zero_flux(steps, grid)
        self.assertEqual(got, [(50.0, 1.0), (10.0, 0.0), (90.0, 0.0), (400.0, 0.0), (20.0, 0.5), (5.0, 0.0)])
        self.assertAlmostEqual(sum(dt for dt, _ in got), sum(dt for dt, _ in steps))

    def test_default_schedule_is_unsubdivided_and_flag_subdivides_every_outage(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 300), (400, 1000)]), with_709=True)
            self.assertEqual(fx.main("plain")[0], 0)
            self.assertEqual(fx.main("sub", "--subdivide-outages", "--cooling-grid", "10s,50s,200s,1000s")[0], 0)
            plain = json.loads(specs_in(Path(d) / "plain")[0].read_text())["schedule"]
            sub = json.loads(specs_in(Path(d) / "sub")[0].read_text())["schedule"]
            self.assertEqual(len(plain), 3 + len(BUILD.parse_cooling_grid(BUILD.DEFAULT_COOLING)))
            # the 100 s gap becomes 10 s + 40 s + 50 s; then the four cooling-grid steps follow the last irradiation
            self.assertEqual([x["dt"] for x in sub[:5]], ["300.0 s", "10.0 s", "40.0 s", "50.0 s", "600.0 s"])
            self.assertEqual(len(sub), 5 + 4)


def spectrum_run_record(base_run, scale=3.0, rel_error=0.1):
    """A second run record: same scenario and physics, a shaped 709-group spectrum with errors."""
    bounds = base_run["bounds"]
    means = [(1.0 + (i % 7)) * scale for i in range(709)]
    means[300] = 2000.0 * scale
    return {
        "scenario_sha256": base_run["scenario_sha256"], "physics_sha256": "phys",
        "normalized": {"results": []},
        "normalized_spectra": [{
            "component_id": "magnets", "particle": "neutron", "energy_edges_ev": [0.0] + bounds[1:],
            "mean_per_square_metre_second": means,
            "standard_error_per_square_metre_second": [m * rel_error for m in means],
        }],
    }


class SpectrumRunTests(unittest.TestCase):
    def fixture(self, d, **spec_kwargs):
        fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
        run = json.loads(fx.run.read_text())
        run["physics_sha256"] = "phys"
        fx.run.write_text(json.dumps(run))
        fx.spectrum_run = Path(d) / "spectrum-run.json"
        fx.spectrum_run.write_text(json.dumps(spectrum_run_record({**run, "bounds": fx.bounds}, **spec_kwargs)))
        return fx

    def test_mismatched_scenario_or_physics_is_refused(self):
        for field in ("scenario_sha256", "physics_sha256"):
            with tempfile.TemporaryDirectory() as d:
                fx = self.fixture(d)
                other = json.loads(fx.spectrum_run.read_text())
                other[field] = "0" * 64
                fx.spectrum_run.write_text(json.dumps(other))
                code, err = fx.main("bad", "--spectrum-run", str(fx.spectrum_run))
                self.assertEqual(code, 2)
                self.assertIn(field, err)
                self.assertFalse((Path(d) / "bad").exists())

    def test_scaling_makes_the_group_sum_equal_the_main_run_flux_and_records_provenance(self):
        with tempfile.TemporaryDirectory() as d:
            fx = self.fixture(d)
            self.assertEqual(fx.main("scaled", "--spectrum-run", str(fx.spectrum_run))[0], 0)
            (spec_path,) = specs_in(Path(d) / "scaled")
            spec = json.loads(spec_path.read_text())
            groups = spec["spectrum"]["flux_per_group"]
            self.assertAlmostEqual(sum(groups), 1.0e13 * 1e-4, delta=1e-9 * 1e9)
            raw = json.loads(fx.spectrum_run.read_text())["normalized_spectra"][0]["mean_per_square_metre_second"]
            self.assertAlmostEqual(groups[1] / groups[0], raw[1] / raw[0], places=12)
            prov = json.loads(spec_path.with_name(spec_path.name.replace(".spec.", ".provenance.")).read_text())
            details = prov["spectrum"]["details"]
            self.assertEqual(details["spectrum_run_sha256"], hashlib.sha256(fx.spectrum_run.read_bytes()).hexdigest())
            self.assertEqual(prov["input_sha256"]["spectrum_run_record"], details["spectrum_run_sha256"])
            self.assertAlmostEqual(details["scale_factor_main_total_over_spectrum_run_total"],
                                   1.0e13 * 1e-4 / (sum(raw) * 1e-4), places=9)
            err = details["spectrum_relative_error"]
            self.assertAlmostEqual(err["flux_weighted_mean_relative_error"], 0.1, places=12)
            self.assertAlmostEqual(err["max_relative_error_groups_ge_1pct"], 0.1, places=12)

    def test_error_summary_flux_weighting_and_one_percent_cut(self):
        means = [0.0] * 709
        errors = [0.0] * 709
        means[0], errors[0] = 98.0, 9.8      # 10 % error, carries most of the flux
        means[1], errors[1] = 1.5, 0.15      # 10 %
        means[2], errors[2] = 0.5, 0.5       # 100 % but below 1 % of the flux
        run = {"normalized_spectra": [{"component_id": "c", "particle": "neutron",
                                       "mean_per_square_metre_second": means,
                                       "standard_error_per_square_metre_second": errors}]}
        got = BUILD.spectrum_error_summary(run, "c")
        self.assertAlmostEqual(got["flux_weighted_mean_relative_error"], (9.8 + 0.15 + 0.5) / 100.0)
        self.assertAlmostEqual(got["max_relative_error_groups_ge_1pct"], 0.1)
        self.assertEqual(got["groups_ge_1pct_of_flux"], 2)

    def test_without_the_option_outputs_carry_no_spectrum_run_fields(self):
        with tempfile.TemporaryDirectory() as d:
            fx = self.fixture(d)
            self.assertEqual(fx.main("plain")[0], 0)
            (spec_path,) = specs_in(Path(d) / "plain")
            prov = json.loads(spec_path.with_name(spec_path.name.replace(".spec.", ".provenance.")).read_text())
            self.assertNotIn("spectrum_run_record", prov["input_sha256"])
            self.assertEqual(sorted(prov["spectrum"]["details"]), ["zero_lower_edge_replaced_by_library_floor"])
            self.assertAlmostEqual(json.loads(spec_path.read_text())["spectrum"]["flux_per_group"][0], 1.0e13 / 709 * 1e-4)


class OutputsOptionTests(unittest.TestCase):
    OLD_OPTIONS = {"mode": "auto", "prune": "rate", "bmin_atoms_per_g": 1e-8, "temperature_K": 293.6, "cram_order": 16}

    def test_outputs_reach_every_spec_and_provenance_and_default_specs_are_unchanged(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)], replacements=[("magnets", 400.0, 450.0)]), with_709=True)
            self.assertEqual(fx.main("plain")[0], 0)
            self.assertEqual(fx.main("heat", "--actinv-outputs", "heat")[0], 0)
            plain_specs, heat_specs = specs_in(Path(d) / "plain"), specs_in(Path(d) / "heat")
            self.assertEqual(len(plain_specs), 2)
            for plain, heat in zip(plain_specs, heat_specs):
                before, after = json.loads(plain.read_text()), json.loads(heat.read_text())
                self.assertEqual(before["options"], self.OLD_OPTIONS)  # no outputs key without the option
                self.assertEqual(after["options"], {**self.OLD_OPTIONS, "outputs": ["heat"]})
                after["options"] = before["options"]
                self.assertEqual(before, after)  # nothing else changes
                prov_name = plain.name.replace(".spec.", ".provenance.")
                plain_prov = json.loads((plain.parent / prov_name).read_text())
                heat_prov = json.loads((heat.parent / prov_name).read_text())
                self.assertNotIn("actinv_outputs", plain_prov)
                self.assertEqual(heat_prov["actinv_outputs"], ["heat"])

    def test_outputs_are_validated_against_the_allowed_set(self):
        self.assertEqual(BUILD.parse_outputs("heat, dose"), ["heat", "dose"])
        for bad in ("heat,bogus", ""):
            with self.assertRaises(BUILD.InputError):
                BUILD.parse_outputs(bad)
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
            code, err = fx.main("bad", "--actinv-outputs", "heat,bogus")
            self.assertEqual(code, 2)
            self.assertIn("bogus", err)
            self.assertFalse((Path(d) / "bad").exists())


class PhotonResponseOptionTests(unittest.TestCase):
    def test_the_response_reaches_every_spec_and_provenance_and_specs_are_otherwise_unchanged(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)], replacements=[("magnets", 400.0, 450.0)]), with_709=True)
            response = Path(d) / "response.json"
            response.write_text('{"schema": "actinv-photon-response-1"}\n')
            self.assertEqual(fx.main("plain")[0], 0)
            self.assertEqual(fx.main("photon", "--photon-response", str(response))[0], 0)
            plain_specs, photon_specs = specs_in(Path(d) / "plain"), specs_in(Path(d) / "photon")
            self.assertEqual(len(plain_specs), 2)
            block = {"response": {"path": str(response.resolve()),
                                  "sha256": hashlib.sha256(response.read_bytes()).hexdigest()}}
            for plain, photon in zip(plain_specs, photon_specs):
                before, after = json.loads(plain.read_text()), json.loads(photon.read_text())
                self.assertNotIn("photon", before)
                self.assertEqual(after.pop("photon"), block)
                self.assertEqual(before, after)
                prov_name = plain.name.replace(".spec.", ".provenance.")
                self.assertNotIn("photon_response", json.loads((plain.parent / prov_name).read_text()))
                self.assertEqual(json.loads((photon.parent / prov_name).read_text())["photon_response"], block["response"])

    def test_an_unreadable_response_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), make_history([(0, 1000)]), with_709=True)
            code, err = fx.main("bad", "--photon-response", str(Path(d) / "missing.json"))
            self.assertEqual(code, 2)
            self.assertIn("photon response", err)
            self.assertFalse((Path(d) / "bad").exists())


class ContinuationTests(unittest.TestCase):
    """--decay-continuations: one spec per (installation, shutdown), history to the shutdown, then cooling."""

    def history(self):
        # magnets installation 1 is [0, 700]: 300 s on, 100 s off, 300 s on; installation 2 is [760, 1000]
        return make_history([(0, 300), (400, 700), (800, 1000)], replacements=[("magnets", 700.0, 760.0)])

    def times(self, d, entries):
        path = Path(d) / "times.json"
        path.write_text(json.dumps(entries))
        return str(path)

    def test_the_prefix_is_not_subdivided_and_the_cooling_grid_is_appended(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), self.history(), with_709=True)
            times = self.times(d, [{"component": "magnets", "installation": 1, "shutdown_s": 700.0},
                                   {"component": "magnets", "installation": 1, "shutdown_s": 300.0}])
            code, err = fx.main("out", "--decay-continuations", times, "--subdivide-outages",
                                "--cooling-grid", "1h,1d,30d", "--actinv-outputs", "heat")
            self.assertEqual(code, 0, err)
            out = Path(d) / "out"
            late = out / "magnets__inst001__cont700__bare_lower_bound.spec.json"
            early = out / "magnets__inst001__cont300__bare_lower_bound.spec.json"
            spec = json.loads(late.read_text())
            self.assertEqual([s["flux"] for s in spec["schedule"]], [1.0, 0.0, 1.0, 0.0, 0.0, 0.0])
            self.assertEqual([s["dt"] for s in spec["schedule"][:3]], ["300.0 s", "100.0 s", "300.0 s"])  # outage whole
            cooling = [float(s["dt"].split()[0]) for s in spec["schedule"][3:]]
            self.assertEqual([sum(cooling[:i + 1]) for i in range(3)], [3600.0, 86400.0, 30 * 86400.0])
            self.assertEqual(spec["options"]["outputs"], ["heat"])
            self.assertEqual(json.loads(early.read_text())["schedule"][0], {"dt": "300.0 s", "flux": 1.0})
            self.assertEqual(len(json.loads(early.read_text())["schedule"]), 4)
            prov = json.loads(late.with_name(late.name.replace(".spec.", ".provenance.")).read_text())
            self.assertEqual(prov["continuation_of_shutdown_s"], 700.0)
            self.assertEqual(prov["spec_file"], late.name)
            self.assertEqual(prov["installation_interval_s"], [0.0, 700.0])
            manifest = json.loads((out / "manifest.json").read_text())
            self.assertIn(late.name, manifest["specs"])
            # the installation's own full spec is still written, and its sidecar has no continuation field
            self.assertTrue((out / "magnets__inst001__bare_lower_bound.spec.json").exists())
            full = json.loads((out / "magnets__inst001__bare_lower_bound.provenance.json").read_text())
            self.assertNotIn("continuation_of_shutdown_s", full)

    def test_continuations_only_skips_the_full_specs_but_keeps_sidecars_with_the_library(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), self.history(), with_709=True)
            times = self.times(d, [{"component": "magnets", "installation": 2, "shutdown_s": 1000.0}])
            code, err = fx.main("out", "--decay-continuations", times, "--continuations-only")
            self.assertEqual(code, 0, err)
            names = sorted(p.name for p in (Path(d) / "out").iterdir())
            self.assertEqual(names, ["magnets__inst002__cont1000__bare_lower_bound.provenance.json",
                                     "magnets__inst002__cont1000__bare_lower_bound.spec.json", "manifest.json"])
            prov = json.loads((Path(d) / "out" / names[0]).read_text())
            self.assertEqual(prov["library"]["library_sha256"], "ab" * 32)
            self.assertEqual(prov["spectrum"]["groups"], 709)

    def test_a_shutdown_that_is_not_a_step_boundary_is_refused_loudly(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), self.history(), with_709=True)
            for shutdown, text in ((350.0, "step boundary"), (900.0, "after the installation ends"),
                                   (0.0, "bad decay-continuations entry")):
                times = self.times(d, [{"component": "magnets", "installation": 1, "shutdown_s": shutdown}])
                code, err = fx.main(f"out{int(shutdown)}", "--decay-continuations", times)
                self.assertEqual(code, 2, shutdown)
                self.assertIn(text, err)
                self.assertFalse((Path(d) / f"out{int(shutdown)}").exists())
            for entry, text in (({"component": "magnets", "installation": 5, "shutdown_s": 300.0}, "not in the history"),
                                ({"component": "gap", "installation": 1, "shutdown_s": 300.0}, "not among")):
                code, err = fx.main("bad", "--decay-continuations", self.times(d, [entry]))
                self.assertEqual((code, text in err), (2, True), entry)
            code, err = fx.main("bad", "--continuations-only")
            self.assertEqual((code, "needs --decay-continuations" in err), (2, True))

    def test_without_the_options_the_output_is_byte_identical_to_the_earlier_script(self):
        # digests of the files (the temporary folder name masked) made by the script before this option existed
        golden = {0: "b1fbe488af99f53b2104478385257bd2e2fe22f5b027d8bf4457487590422a29",
                  5: "dc5ce7c6362d26283bb6300ad8e9c07c0ff20a5d5298852af28afde0627e1682"}
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), self.history(), with_709=True)
            for extra in ([], ["--subdivide-outages", "--actinv-outputs", "heat", "--cooling-grid", "1h,1d,30d"]):
                out = f"o{len(extra)}"
                self.assertEqual(fx.main(out, *extra)[0], 0)
                digest = hashlib.sha256()
                for path in sorted((Path(d) / out).iterdir()):
                    digest.update(path.name.encode())
                    digest.update(path.read_text().replace(d, "<d>").encode())
                self.assertEqual(digest.hexdigest(), golden[len(extra)])

    def test_the_full_specs_are_unchanged_when_continuations_are_added(self):
        with tempfile.TemporaryDirectory() as d:
            fx = Fixture(Path(d), self.history(), with_709=True)
            self.assertEqual(fx.main("plain")[0], 0)
            times = self.times(d, [{"component": "magnets", "installation": 1, "shutdown_s": 300.0}])
            self.assertEqual(fx.main("more", "--decay-continuations", times)[0], 0)
            for path in (Path(d) / "plain").iterdir():
                if path.name != "manifest.json":
                    self.assertEqual(path.read_bytes(), (Path(d) / "more" / path.name).read_bytes(), path.name)


if __name__ == "__main__":
    unittest.main()
