# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import importlib.util
import math
import json
import subprocess
import sys
import unittest
from pathlib import Path
from validation.manifest import load_manifest
from validation import report as report_mod
from validation.matrix import load_registry

ROOT = Path(__file__).resolve().parent.parent
CASE = ROOT / "validation" / "cases" / "fns-tof-jade-v1"
UPSTREAM_CACHE = Path.home() / ".cache/avila-night/validation-data/open-benchmarks"
sys.path.insert(0, str(CASE))
sys.path.insert(0, str(ROOT))


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


B = load("fns_tof_benchmark", CASE / "benchmark.py")
R = load("fns_tof_run_case", CASE / "run_case.py")
BM = load("fns_tof_build_manifest", CASE / "build_manifest.py")


DECK = """FNS synthetic mapping fixture
c
1 1 -1.0 -1 imp:n=1
2 0 1 imp:n=1
1 so 1000
mode n
sdef erg=d1 pos=0 0 -5
# SI1 SP1
1.0 0.0
2.0 0.25
4.0 0.75
m1 4009.41c 1
f5:n 0 0 10 1 nd
fc5 Neutron leakage flux at 0 degrees
fm5 2.0
f25:n 2 0 10 1 nd
fc25 Neutron leakage flux at 24.9 degrees
fm25 2.1
e0 0.5
1.0
"""

DECK_FOUR = DECK.replace("f25:n 2 0 10", "f25:n 20 0 10").replace(
    "e0 0.5\n",
    "f35:n 40 0 10 1 nd\nfc35 Neutron leakage flux at 41.8 degrees\nfm35 2.2\n"
    "f45:n 60 0 10 1 nd\nfc45 Neutron leakage flux at 66.8 degrees\nfm45 2.3\ne0 0.5\n")


def test_manifest():
    return {"detectors": [
        {"id": "angle-0", "reference": {"value": 1.0}},
        {"id": "angle-24", "reference": {"value": 2.0}},
    ], "adapter": {"tables": [
        {"angle_deg": 0.0, "tally_number": 5, "mcnp_FM": 2.0, "coarse": False,
         "detector_ids": [{"detector_id": "angle-0", "bin_index": 0, "lo_mev": 0.1, "hi_mev": 0.5}]},
        {"angle_deg": 24.9, "tally_number": 25, "mcnp_FM": 2.1, "coarse": True,
         "detector_ids": [{"detector_id": "angle-24", "bin_index": 1, "lo_mev": 0.1, "hi_mev": 1.0}]},
    ]}}


class FnsTofHelpers(unittest.TestCase):
    def test_analytic_isotropic_sphere_flux_has_point_detector_limit(self):
        for radius in (2.5, 1.25, 0.625):
            distance = 700.0
            volume_average = R.isotropic_sphere_volume_average_cm2(distance, radius)
            point_flux = 1.0 / (4.0 * math.pi * distance * distance)
            self.assertLess(abs(volume_average / point_flux - 1.0), 1e-5)

        # Independent midpoint integration of the spherical-shell expression
        # checks the exact volume average away from the point-detector limit.
        distance, radius, steps = 10.0, 2.5, 20_000
        h = radius / steps
        integral = 0.0
        for i in range(steps):
            rho = (i + 0.5) * h
            integral += rho * math.log((distance + rho) / (distance - rho)) * h
        numeric = (2.0 * math.pi / distance * integral) / (
            4.0 * math.pi * R.sphere_volume_cm3(radius))
        exact = R.isotropic_sphere_volume_average_cm2(distance, radius)
        self.assertLess(abs(numeric / exact - 1.0), 1e-8)

    def test_aperture_mixture_preserves_unit_source_weight_and_event_probability(self):
        beta, physical_target_mass = 0.1, 0.037
        proposal_target_mass = (1.0 - beta) * physical_target_mass + beta
        target_weight = physical_target_mass / proposal_target_mass
        complement_proposal_mass = (1.0 - beta) * (1.0 - physical_target_mass)
        complement_weight = 1.0 / (1.0 - beta)
        self.assertAlmostEqual(proposal_target_mass * target_weight
                               + complement_proposal_mass * complement_weight, 1.0)
        self.assertAlmostEqual(proposal_target_mass * target_weight, physical_target_mass)
        self.assertGreater(beta, 0.0)
        self.assertLess(beta, 1.0)
        self.assertLessEqual(max(target_weight, complement_weight), 1.0 / (1.0 - beta))

    def test_four_point_apertures_cover_all_fns_fe5_detectors_with_finite_weight_bounds(self):
        parsed = B.input_deck(DECK_FOUR)
        plan = R.source_aperture_plan(parsed, 2.5, 0.1)
        self.assertEqual([row["mcnp_tally_number"] for row in plan["detector_apertures"]],
                         [5, 25, 35, 45])
        self.assertEqual(plan["environment"]["OPENMC_APERTURE_ONE_SIDED"], "1")
        self.assertIn("(1-beta)/(4*pi)", plan["proposal_law"])
        self.assertIn("4*pi*beta*aperture_pdf", plan["source_weight_correction"])
        bounds = plan["source_birth_weight_bounds"]
        self.assertGreater(bounds["lower_bound"], 0.0)
        self.assertLessEqual(bounds["lower_bound"], bounds["upper_bound"])
        self.assertAlmostEqual(bounds["upper_bound"], 1.0 / 0.9)
        self.assertTrue(all(row["proposal_isotropic_probability"] < row["finite_sphere_solid_angle_probability"]
                            for row in plan["detector_apertures"]))
        self.assertEqual(len(plan["environment"]["OPENMC_APERTURE_BIAS"].split(";")), 4)
        for radius in (2.5, 1.25, 0.625):
            radius_plan = R.source_aperture_plan(parsed, radius, 0.1)
            self.assertEqual([row["mcnp_tally_number"] for row in radius_plan["detector_apertures"]],
                             [5, 25, 35, 45])
        with self.assertRaisesRegex(ValueError, "strictly between"):
            R.source_aperture_plan(parsed, 2.5, 1.0)
        with self.assertRaisesRegex(ValueError, "requires all four"):
            R.source_aperture_plan(B.input_deck(DECK), 2.5, 0.1)

    @unittest.skipUnless(UPSTREAM_CACHE.is_dir(), "external pinned FNS checkout is not present")
    def test_pinned_fns_fe5_apertures_use_all_four_actual_deck_points(self):
        manifest = load_manifest(ROOT / "validation/cases/fns-tof-fe-5/manifest.json")
        manifest, deck_path, parsed_wrapper = R.source_inputs(UPSTREAM_CACHE, manifest)
        parsed = parsed_wrapper["deck"]
        mapping = R.validate_response_mapping(manifest, parsed)
        plan = R.source_aperture_plan(parsed, 2.5, 0.1)
        self.assertEqual(set(mapping), {detector["id"] for detector in manifest["detectors"]})
        self.assertEqual({row["tally"] for row in mapping.values()}, {5, 25, 35, 45})
        self.assertEqual({row["tally"] for row in parsed["detectors"]}, {5, 25, 35, 45})
        self.assertEqual({row["mcnp_tally_number"] for row in plan["detector_apertures"]},
                         {5, 25, 35, 45})
        for aperture, detector in zip(plan["detector_apertures"], parsed["detectors"], strict=True):
            self.assertEqual(aperture["center_cm"], detector["position_cm"])
        self.assertTrue(deck_path.is_file())

    def test_vacuum_aim_control_evaluation_requires_four_passing_zero_loss_responses(self):
        parsed = B.input_deck(DECK_FOUR)
        rows = []
        for point in parsed["detectors"]:
            distance = math.sqrt(math.fsum((point["position_cm"][i] - parsed["source_position_cm"][i]) ** 2
                                           for i in range(3)))
            expected = R.isotropic_sphere_volume_average_cm2(distance, 2.5)
            volume = R.sphere_volume_cm3(2.5)
            rows.append({"mcnp_tally_number": point["tally"],
                         "raw_tracklength_mean_particle_cm_per_source": expected * volume,
                         "raw_tracklength_std_dev_particle_cm_per_source": expected * volume * 0.01,
                         "sphere_volume_cm3": volume,
                         "analytic_flux_cm-2_per_source": expected})
        patch_statepoint = {"lost_particles": 0, "intentional_kill_events": 0}
        result = R.evaluate_vacuum_aim_control(rows, patch_statepoint)
        self.assertEqual(result["status"], "PASS")
        self.assertEqual([row["mcnp_tally_number"] for row in result["results"]], [5, 25, 35, 45])
        with self.assertRaisesRegex(ValueError, "all four"):
            R.evaluate_vacuum_aim_control(rows[:-1], patch_statepoint)
        with self.assertRaisesRegex(ValueError, "loss or intentional"):
            R.evaluate_vacuum_aim_control(rows, {"lost_particles": 1, "intentional_kill_events": 0})

    def test_actual_openmc_api_builds_vacuum_control_for_four_points_without_transport(self):
        try:
            import openmc
        except ImportError:
            self.skipTest("OpenMC Python API is not installed")
        parsed = B.input_deck(DECK_FOUR)
        model, detector_cells = R.vacuum_aim_control_model(openmc, parsed, 2.5, 10, 1000, 1701)
        self.assertEqual(set(detector_cells), {5, 25, 35, 45})
        self.assertEqual({t.name for t in model.tallies},
                         {"aim_vacuum_f5", "aim_vacuum_f25", "aim_vacuum_f35", "aim_vacuum_f45"})
        self.assertIsInstance(model.settings.source[0].angle, openmc.stats.Isotropic)
        self.assertEqual(model.settings.max_lost_particles, 1)
        for point in parsed["detectors"]:
            found = model.geometry.find(point["position_cm"])[-1]
            self.assertEqual(found.id, detector_cells[point["tally"]]["cell"].id)
            self.assertGreater(detector_cells[point["tally"]]["analytic_flux_cm-2_per_source"], 0.0)

    def test_aim_runtime_identity_requires_markers_in_selected_linked_library(self):
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            library = Path(temp) / "libopenmc.so"
            library.write_bytes(b"OPENMC_APERTURE_BIAS OPENMC_APERTURE_ONE_SIDED OPENMC_COLLISION_AIM")
            identity = R.aim_runtime_identity({"linked_library_path": str(library)})
            self.assertEqual(identity["linked_library_sha256"], R.hash_file(library))
            self.assertTrue(identity["collision_aim_available_but_disabled"])
            library.write_bytes(b"stock OpenMC")
            with self.assertRaisesRegex(ValueError, "lacks AIM patch support"):
                R.aim_runtime_identity({"linked_library_path": str(library)})

    def test_aim_control_receipt_binds_runtime_all_points_radii_plans_and_statepoints(self):
        import tempfile
        from unittest.mock import patch
        parsed = B.input_deck(DECK_FOUR)
        runtime = {"executable_sha256": "e" * 64, "linked_library_sha256": "l" * 64,
                   "openmc_python_module_sha256": "p" * 64, "cross_sections_xml_sha256": "x" * 64,
                   "aim_capability": {"patch_sha256": "a" * 64}}
        imp0_control = {"receipt_sha256": "i" * 64, "status": "PASS"}
        patch_statepoint = {"lost_particles": 0, "intentional_kill_events": 0,
                            "source_birth_kill_events": 0, "cell_entry_kill_events": 0,
                            "patch_version": 1, "current_batch": 10, "n_batches": 10,
                            "n_particles": 100, "n_realizations": 10, "solver_version": "0.15.3",
                            "source": "child statepoint datasets; imp0 kills and normal lost count are distinct"}
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            statepoint = root / "control.statepoint.h5"
            statepoint.write_bytes(b"fake statepoint identity; reader mocked")
            levels = []
            artifacts = []
            for radius in R.AIM_CONTROL_RADII_CM:
                results = [{"mcnp_tally_number": point["tally"],
                            "position_cm": point["position_cm"], "passed": True}
                           for point in parsed["detectors"]]
                levels.append({"radius_cm": radius, "status": "PASS", "results": results,
                               "source_aperture": R.source_aperture_plan(parsed, radius, 0.1),
                               "patch_statepoint": patch_statepoint})
                artifacts.append({"radius_cm": radius, "statepoint_path": str(statepoint),
                                  "statepoint_sha256": R.hash_file(statepoint)})
            receipt = {"schema": "faris.fns-aim-vacuum-control/1.0.0", "status": "PASS",
                       "case_id": R.AIM_SUPPORTED_CASE, "source_deck_sha256": "d" * 64,
                       "runtime_identity": runtime, "imp0_control_receipt": imp0_control,
                       "beta": 0.1, "radius_levels": levels,
                       "solver_artifacts": artifacts}
            receipt["receipt_sha256"] = R.canonical_json_sha256(receipt)
            path = root / "control.json"
            path.write_text(json.dumps(receipt))
            with patch.object(R, "read_imp0_statepoint", return_value=patch_statepoint):
                verified = R.verify_aim_control_receipt(path, runtime, R.AIM_SUPPORTED_CASE,
                                                        parsed, "d" * 64, 0.1, imp0_control)
                self.assertEqual(verified["status"], "PASS")
                bad = json.loads(path.read_text())
                bad["radius_levels"][0]["results"].pop()
                bad["receipt_sha256"] = R.canonical_json_sha256(bad, ("receipt_sha256",))
                path.write_text(json.dumps(bad))
                with self.assertRaisesRegex(ValueError, "every one of the four"):
                    R.verify_aim_control_receipt(path, runtime, R.AIM_SUPPORTED_CASE,
                                                 parsed, "d" * 64, 0.1, imp0_control)

    def test_analog_environment_clears_inherited_aim_and_collision_settings(self):
        env = R.solver_environment({"OPENMC_APERTURE_BIAS": "bad", "OPENMC_COLLISION_AIM": "bad",
                                   "OPENMC_WW_SKIP_VOID": "1", "KEEP": "yes"}, 1)
        self.assertNotIn("OPENMC_APERTURE_BIAS", env)
        self.assertNotIn("OPENMC_COLLISION_AIM", env)
        self.assertNotIn("OPENMC_WW_SKIP_VOID", env)
        self.assertEqual(env["KEEP"], "yes")

    def test_histogram_translation_has_equal_openmc_x_p_lengths_and_preserves_integral(self):
        translated = B.openmc_histogram([1.0, 2.0, 4.0], [0.0, 2.0, 6.0])
        self.assertEqual(len(translated["edges_ev"]), len(translated["pdf_per_ev"]))
        self.assertAlmostEqual(sum(p * (b - a) for a, b, p in zip(
            translated["edges_ev"][:-1], translated["edges_ev"][1:], translated["pdf_per_ev"][:-1])), 1.0)
        self.assertEqual(translated["pdf_per_ev"][-1], 0.0)

    def test_native_vertical_source_card_and_five_card_tally_mapping(self):
        parsed = B.input_deck(DECK)
        self.assertTrue(parsed["source_table_is_literal_input"])
        self.assertEqual([(d["tally"], d["angle_deg"]) for d in parsed["detectors"]], [(5, 0.0), (25, 24.9)])
        mapping = R.validate_response_mapping(test_manifest(), parsed)
        self.assertEqual(mapping["angle-0"]["tally"], 5)
        self.assertEqual(mapping["angle-24"]["tally"], 25)

    def test_published_absolute_error_is_preserved_and_inconsistent_rows_rejected(self):
        rows = B.read_rows(b"Energy,Value,Error,abs err,Angle\n1.0,2.0,0.1,0.2,0 deg\n", "fine.csv")
        self.assertEqual(rows[0]["error_absolute"], 0.2)
        self.assertEqual(rows[0]["absolute_error_source"], "abs err")
        with self.assertRaisesRegex(ValueError, r"does not match Value\*Error"):
            B.read_rows(b"Energy,Value,Error,abs err,Angle\n1.0,2.0,0.1,0.3,0 deg\n", "bad.csv")
        coarse = B.read_rows(b"Value,Error,Energy,Angle\n2.0,0.1,0.1 - 1.0,0 deg\n", "coarse.csv")
        self.assertEqual(coarse[0]["error_absolute"], 0.2)
        self.assertEqual(coarse[0]["absolute_error_source"], "Value*Error derived from source Error column")

    def test_mapping_rejects_positionally_plausible_but_wrong_tally_id(self):
        parsed = B.input_deck(DECK)
        manifest = test_manifest()
        manifest["adapter"]["tables"][0]["tally_number"] = 25
        with self.assertRaisesRegex(ValueError, "different tally ID"):
            R.validate_response_mapping(manifest, parsed)

    def test_mapping_rejects_missing_source_angle_instead_of_filling_by_order(self):
        parsed = B.input_deck(DECK)
        manifest = test_manifest()
        manifest["adapter"]["tables"][0]["angle_deg"] = 12.2
        with self.assertRaisesRegex(ValueError, "no exact FC-labeled tally"):
            R.validate_response_mapping(manifest, parsed)

    def test_mcnp_fm_is_applied_once_and_flux_is_per_unit_lethargy(self):
        radius = 0.625
        volume = (4.0 / 3.0) * math.pi * radius ** 3
        result = R.tally_value(3.0, 0.3, 2.0, 1.0, 2.0, volume, radius)
        self.assertAlmostEqual(result["value"], (3.0 / volume) * 2.0 / math.log(2.0))
        self.assertAlmostEqual(result["u_mc"], (0.3 / volume) * 2.0 / math.log(2.0))
        self.assertEqual(result["openmc_tracklength_mean_particle_cm_per_source"], 3.0)
        self.assertEqual(result["openmc_tracklength_std_dev_particle_cm_per_source"], 0.3)
        self.assertAlmostEqual(result["finite_volume_normalization"]["volume_cm3"], volume)
        self.assertAlmostEqual(
            result["finite_volume_normalization"]["flux_mean_particle_per_cm2_per_source_before_FM"],
            3.0 / volume)
        self.assertAlmostEqual(B.lethargy_width(0.0, 0.01), math.log(0.01 / 1e-10))

    def test_sphere_volume_is_analytic_and_tally_volume_must_match_radius(self):
        radius = 2.5
        expected_volume = 4.0 * math.pi * radius ** 3 / 3.0
        self.assertAlmostEqual(R.sphere_volume_cm3(radius), expected_volume, places=14)
        with self.assertRaisesRegex(ValueError, "differs from the analytic sphere volume"):
            R.tally_value(1.0, 0.1, 2.0, 1.0, 2.0, expected_volume / 2.0, radius)
        with self.assertRaisesRegex(ValueError, "radius must be positive and finite"):
            R.sphere_volume_cm3(0.0)

    def test_spatial_volume_normalization_is_radius_invariant_for_constant_flux(self):
        flux_density = 4.2
        radius_values = []
        for radius in (2.5, 1.25, 0.625):
            volume = R.sphere_volume_cm3(radius)
            row = R.tally_value(flux_density * volume, 0.05 * volume,
                                3.0, 0.1, 1.0, volume, radius)
            radius_values.append((row["value"], row["u_mc"]))
        for value, u_mc in radius_values[1:]:
            self.assertAlmostEqual(value, radius_values[0][0])
            self.assertAlmostEqual(u_mc, radius_values[0][1])

    def test_outer_sphere_must_be_preserved_as_vacuum_boundary(self):
        class Surface:
            boundary_type = "vacuum"
        class Geometry:
            def get_all_surfaces(self):
                return {1: Surface()}
        class Model:
            geometry = Geometry()
        parsed = B.input_deck(DECK)
        self.assertEqual(R.require_vacuum_outer_sphere(Model(), parsed), 1)
        Model.geometry.get_all_surfaces = lambda: {1: type("Reflective", (), {"boundary_type": "reflective"})()}
        with self.assertRaisesRegex(ValueError, "no verified imp0 cell marker"):
            R.require_vacuum_outer_sphere(Model(), parsed)
        Model.geometry.get_all_surfaces = lambda: {1: type("Transmission", (), {"boundary_type": "transmission"})()}
        mapping = {"cells": [{"action": "kill_on_entry_marker_required", "source_region_surface_ids": [1]}]}
        self.assertEqual(R.require_vacuum_outer_sphere(Model(), parsed, mapping), 1)

    def test_transformed_surface_cards_are_not_misclassified_as_cells(self):
        parsed = B.input_deck(DECK.replace("1 so 1000", "1 7 so 1000"))
        self.assertIn("1 7 so 1000", parsed["surface_cards"])

    def test_cell_id_allocator_sees_source_cells_removed_from_converted_universe(self):
        parsed = B.input_deck(DECK.replace("2 0 1 imp:n=1\n", "2 0 1 imp:n=1\n5 0 -1 imp:n=0\n"))
        self.assertEqual(R.mcnp_cell_ids(parsed), [1, 2, 5])

    def test_cell_continuations_are_joined_before_identity_and_importance_parsing(self):
        deck = DECK.replace("2 0 1 imp:n=1\n", "2 0 1 imp:n=1\n3 0 -1 :\n     2 -3 imp:n=1\n")
        parsed = B.input_deck(deck)
        self.assertEqual(R.mcnp_cell_ids(parsed), [1, 2, 3])
        self.assertIn("2 -3 imp:n=1", parsed["cell_cards"][2])
        self.assertEqual(R.mcnp_cell_semantics(parsed)[2]["neutron_importance"], 1.0)

    def test_cell_actions_mark_retained_imp0_and_accept_only_verified_vacuum_removals(self):
        class Cell:
            def __init__(self, fill=None):
                self.fill = fill
        class Surface:
            boundary_type = "vacuum"
        source = [
            {"mcnp_cell_id": 4, "material_id": 0, "neutron_importance": 0.0,
             "source_region_signature": ["(", "2", ":", "-3", ")", "-10"]},
            {"mcnp_cell_id": 5, "material_id": 0, "neutron_importance": 0.0,
             "source_region_signature": ["10"]},
        ]
        actions = R.verified_cell_actions(source, {4: "(2 : -3) -10", 5: "10"},
                                          {4: Cell()}, {10: Surface()})
        self.assertEqual([row["action"] for row in actions],
                         ["kill_on_entry_marker_required", "single_halfspace_vacuum_boundary"])
        with self.assertRaisesRegex(ValueError, "single-halfspace vacuum mapping"):
            R.verified_cell_actions([source[0]], {4: "(2 : -3) -10"}, {}, {10: Surface()})

    def test_patch_runtime_ldd_parser_requires_one_resolved_library(self):
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            library = Path(temp) / "libopenmc.so"
            library.write_bytes(b"metadata-only fixture")
            got = R.parse_ldd_openmc_library(f"libopenmc.so => {library} (0xabc)\n")
            self.assertEqual(got, library.resolve())
            with self.assertRaisesRegex(ValueError, "exactly one"):
                R.parse_ldd_openmc_library("libopenmc.so => not found\n")

    def test_selected_cli_runtime_probe_binds_exact_linked_library_capability(self):
        import tempfile
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            cli, lib = root / "openmc", root / "libopenmc.so"
            cli.write_bytes(b"cli"); lib.write_bytes(b"library")
            def runner(command, **_kwargs):
                if command[0] == "ldd":
                    return SimpleNamespace(returncode=0, stdout=f"libopenmc.so => {lib} (0xabc)\n", stderr="")
                return SimpleNamespace(returncode=0, stdout="1\n", stderr="")
            identity = R.selected_openmc_runtime_identity(cli, {}, runner=runner, python="python-test")
            self.assertEqual(identity["linked_library_sha256"], R.hash_file(lib))
            self.assertEqual(identity["patch_version"], 1)
            with self.assertRaisesRegex(ValueError, "LD_PRELOAD/LD_AUDIT"):
                R.selected_openmc_runtime_identity(cli, {"LD_PRELOAD": "/tmp/override.so"},
                                                   runner=runner, python="python-test")
            with self.assertRaisesRegex(ValueError, "LD_LIBRARY_PATH entries"):
                R.selected_openmc_runtime_identity(cli, {"LD_LIBRARY_PATH": "."},
                                                   runner=runner, python="python-test")
            def missing_patch(command, **_kwargs):
                if command[0] == "ldd":
                    return SimpleNamespace(returncode=0, stdout=f"libopenmc.so => {lib} (0xabc)\n", stderr="")
                return SimpleNamespace(returncode=1, stdout="", stderr="missing symbol")
            with self.assertRaisesRegex(ValueError, "lacks the FARIS"):
                R.selected_openmc_runtime_identity(cli, {}, runner=missing_patch, python="python-test")

    def test_patch_version_and_statepoint_integer_fields_fail_closed(self):
        class Dataset:
            def __init__(self, shape, data):
                self.shape = shape
                self.data = data
            def __getitem__(self, key):
                return self.data
        self.assertEqual(R.scalar_int_dataset(Dataset((), 7), "count"), 7)
        with self.assertRaisesRegex(ValueError, "integer"):
            R.scalar_int_dataset(Dataset((), 1.5), "count")
        R.require_imp0_patch_version(1)
        with self.assertRaisesRegex(ValueError, "expected 1"):
            R.require_imp0_patch_version(0)

    def test_imp0_statepoint_reader_uses_child_datasets_and_separates_lost_count(self):
        from unittest.mock import patch
        class Dataset:
            def __init__(self, value):
                self.shape = ()
                self.value = value
            def __getitem__(self, _key):
                return self.value
        class H5File(dict):
            attrs = {"openmc_version": [0, 15, 3]}
            def __enter__(self): return self
            def __exit__(self, *_args): return False
        fields = {
            R.FARIS_IMP0_PATCH_VERSION_DATASET: Dataset(1),
            R.FARIS_IMP0_PATCH_KILL_EVENTS_DATASET: Dataset(7),
            R.FARIS_IMP0_PATCH_BIRTH_KILL_DATASET: Dataset(2),
            R.FARIS_IMP0_PATCH_ENTRY_KILL_DATASET: Dataset(5),
            R.FARIS_OPENMC_LOST_DATASET: Dataset(0),
            R.STATEPOINT_CURRENT_BATCH_DATASET: Dataset(1),
            R.STATEPOINT_N_BATCHES_DATASET: Dataset(1),
            R.STATEPOINT_N_PARTICLES_DATASET: Dataset(1000),
            R.STATEPOINT_N_REALIZATIONS_DATASET: Dataset(1),
        }
        fake_h5py = type("H5py", (), {"File": staticmethod(lambda *_args, **_kwargs: H5File(fields))})
        with patch.dict(sys.modules, {"h5py": fake_h5py}):
            result = R.read_imp0_statepoint(Path("statepoint.fake"))
        self.assertEqual(result["intentional_kill_events"], 7)
        self.assertEqual(result["source_birth_kill_events"], 2)
        self.assertEqual(result["cell_entry_kill_events"], 5)
        self.assertEqual(result["lost_particles"], 0)
        self.assertEqual(result["n_particles"], 1000)
        self.assertEqual(result["n_realizations"], 1)
        self.assertEqual(result["solver_version"], "0.15.3")

    def test_imp0_control_rejects_particles_misreported_as_realizations(self):
        expected = {"current_batch": 1, "n_batches": 1, "n_particles": 1000,
                    "n_realizations": 1, "solver_version": "0.15.3"}
        R.require_imp0_control_statepoint_metadata(expected, "entry_neutron")
        mistaken = {**expected, "n_realizations": 1000}
        with self.assertRaisesRegex(ValueError, "n_realizations=1000, expected 1"):
            R.require_imp0_control_statepoint_metadata(mistaken, "entry_neutron")

    def test_importance_marker_injection_is_sidecar_and_xml_hash_bound(self):
        import tempfile
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            geometry = root / "geometry.xml"
            geometry.write_text("<?xml version='1.0'?>\n<geometry><cell id='4' region='-1'/></geometry>\n")
            source_sha = "a" * 64
            mapping = {"source_cell_id": 2, "source_birth_importance": 1.0,
                       "required_patch_marker_cell_ids": [4], "cells": [
                           {"mcnp_cell_id": 4, "openmc_cell_id": 4, "action": "kill_on_entry_marker_required"},
                           {"mcnp_cell_id": 5, "openmc_cell_id": None, "action": "single_halfspace_vacuum_boundary"}]}
            converter = {"source_commit": "deadbeef", "python_source_sha256": "b" * 64}
            receipt = R.inject_importance_xml_markers(geometry, root / "importance.json", "case",
                                                       source_sha, converter, mapping)
            self.assertEqual(receipt["marker_cell_ids"], [4])
            self.assertEqual(R.hash_file(geometry), receipt["final_geometry_sha256"])
            self.assertEqual(R.load_verified_importance_sidecar(root / "importance.json",
                                                                 receipt["sidecar_sha256"], source_sha)["case_id"], "case")
            with self.assertRaisesRegex(ValueError, "SHA256 mismatch"):
                R.load_verified_importance_sidecar(root / "importance.json", "0" * 64, source_sha)

    def test_imp0_control_receipt_must_match_selected_runtime_and_all_child_controls(self):
        import tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            case_rows = []
            expected = {"entry_neutron": (1000, 0, 1000, [1.0, 0.5, 0.0], 1.0),
                        "source_birth_neutron": (1000, 1000, 0, [0.0, 0.0, 0.0], 0.0),
                        "marked_photon": (0, 0, 0, [1.0, 0.5, 0.5], 1.0),
                        "unmarked_neutron": (0, 0, 0, [1.0, 0.5, 0.5], 1.0)}
            for name, (total, birth, entry, flux, current) in expected.items():
                sp = root / f"{name}.h5"
                sp.write_bytes(name.encode())
                case_rows.append({"case": {"name": name}, "statepoint": str(sp),
                                  "statepoint_sha256": R.hash_file(sp), "geometry_xml_sha256": "c" * 64,
                                  "statepoint_current_batch": 1, "statepoint_n_batches": 1,
                                  "statepoint_n_particles": 1000, "statepoint_n_realizations": 1,
                                  "solver_version": "0.15.3", "cell_flux_means_per_source": flux,
                                  "middle_surface_current_mean_per_source": current,
                                  "expected": {"kill": total, "birth": birth, "entry": entry,
                                               "flux": flux, "current": current},
                                  "diagnostics": {"faris_mcnp_imp0_patch_version": 1,
                                                  "faris_mcnp_imp0_kill_events": total,
                                                  "faris_mcnp_imp0_birth_kill_events": birth,
                                                  "faris_mcnp_imp0_entry_kill_events": entry,
                                                  "faris_openmc_n_lost_particles": 0}})
            cli, lib = root / "openmc", root / "libopenmc.so"
            cli.write_bytes(b"cli"); lib.write_bytes(b"lib")
            py_module, control_xs = root / "openmc.py", root / "cross_sections.xml"
            py_module.write_bytes(b"module"); control_xs.write_bytes(b"xs")
            runtime = {"executable_path": str(cli.resolve()), "executable_sha256": R.hash_file(cli),
                       "linked_library_path": str(lib.resolve()), "linked_library_sha256": R.hash_file(lib),
                       "patch_version": 1, "openmc_python_module_path": str(py_module.resolve()),
                       "openmc_python_module_sha256": R.hash_file(py_module),
                       "cross_sections_path": str(control_xs.resolve()),
                       "cross_sections_xml_sha256": R.hash_file(control_xs)}
            control = {"schema": "faris.openmc-mcnp-imp0-control/1.0.0", "status": "PASS",
                       "runtime": {"selected_cli_path": str(cli), "selected_cli_sha256": runtime["executable_sha256"],
                                   "selected_linked_library_path": str(lib),
                                   "selected_linked_library_sha256": runtime["linked_library_sha256"],
                                   "faris_patch_version_symbol": 1,
                                   "openmc_python_version": "0.15.3",
                                   "python_path": str(Path(sys.executable).resolve()),
                                   "python_sha256": R.hash_file(Path(sys.executable).resolve()),
                                   "openmc_python_module_path": str(py_module.resolve()),
                                   "openmc_python_module_sha256": R.hash_file(py_module),
                                   "cross_sections_path": str(control_xs.resolve()),
                                   "cross_sections_sha256": R.hash_file(control_xs)}, "cases": case_rows}
            control["receipt_sha256"] = R.imp0_control_receipt_sha256(control)
            path = root / "control.json"; path.write_text(json.dumps(control))
            def statepoint_reader(statepoint):
                total, birth, entry, _flux, _current = expected[statepoint.stem]
                return {"patch_version": 1, "intentional_kill_events": total,
                        "source_birth_kill_events": birth, "cell_entry_kill_events": entry,
                        "lost_particles": 0, "current_batch": 1, "n_batches": 1,
                        "n_particles": 1000, "n_realizations": 1, "solver_version": "0.15.3"}
            with patch.object(R, "read_imp0_statepoint", side_effect=statepoint_reader):
                self.assertEqual(R.verify_imp0_control_receipt(path, runtime)["status"], "PASS")
            alternate_xs = root / "other_cross_sections.xml"
            alternate_xs.write_bytes(b"other-xs")
            mismatched_runtime = {**runtime, "cross_sections_path": str(alternate_xs.resolve()),
                                  "cross_sections_xml_sha256": R.hash_file(alternate_xs)}
            with patch.object(R, "read_imp0_statepoint", side_effect=statepoint_reader):
                with self.assertRaisesRegex(ValueError, "different cross_sections.xml index"):
                    R.verify_imp0_control_receipt(path, mismatched_runtime)
            control["cases"][0]["statepoint_n_realizations"] = 1000
            control["receipt_sha256"] = R.imp0_control_receipt_sha256(control)
            path.write_text(json.dumps(control))
            with patch.object(R, "read_imp0_statepoint", side_effect=statepoint_reader):
                with self.assertRaisesRegex(ValueError, "statepoint_n_realizations differs"):
                    R.verify_imp0_control_receipt(path, runtime)
            control["cases"][0]["statepoint_n_realizations"] = 1
            control["cases"][0]["diagnostics"]["faris_mcnp_imp0_kill_events"] = 0
            control["receipt_sha256"] = R.imp0_control_receipt_sha256(control)
            path.write_text(json.dumps(control))
            with patch.object(R, "read_imp0_statepoint", side_effect=statepoint_reader):
                with self.assertRaisesRegex(ValueError, "kill-counter control failed"):
                    R.verify_imp0_control_receipt(path, runtime)

    def test_imp0_control_canonical_sha_detects_receipt_mutation(self):
        control = {"schema": "faris.openmc-mcnp-imp0-control/1.0.0", "status": "PASS", "cases": []}
        digest = R.imp0_control_receipt_sha256(control)
        control["receipt_sha256"] = digest
        self.assertEqual(R.imp0_control_receipt_sha256(control), digest)
        control["status"] = "FAILED"
        self.assertNotEqual(R.imp0_control_receipt_sha256(control), digest)

    def test_record_mapping_binds_source_f5_to_actual_openmc_tally_and_filter_ids(self):
        filters = [type("CellFilter", (), {"id": 81})(), type("EnergyFilter", (), {"id": 82})()]
        tallies = [type("Tally", (), {"name": "f25_coarse", "id": 29, "filters": filters})()]
        model = type("Model", (), {"tallies": tallies})()
        self.assertEqual(R.tally_identity_mapping(model), [{
            "mcnp_tally_number": 25, "energy_resolution": "coarse", "openmc_tally_id": 29,
            "filters": [{"openmc_filter_id": 81, "type": "CellFilter"},
                        {"openmc_filter_id": 82, "type": "EnergyFilter"}],
        }])

    def test_coarse_response_uses_direct_coarse_tally_std_dev(self):
        class Array:
            def __init__(self, value):
                self.value = value
            def reshape(self, *_shape):
                return self
            def tolist(self):
                return [self.value]

        class Tally:
            def __init__(self, name, mean, std_dev):
                self.name = name
                self.mean = Array(mean)
                self.std_dev = Array(std_dev)

        class StatePoint:
            def __init__(self, _path):
                self.tallies = type("Tallies", (), {"values": lambda _self: [
                    Tally("f5_fine", 1.0, 0.11), Tally("f25_coarse", 100.0, 0.07)]})()
            def __enter__(self):
                return self
            def __exit__(self, *_exc):
                return False

        fake_openmc = type("OpenMC", (), {"StatePoint": StatePoint})
        got = R.result_levels(fake_openmc, test_manifest(), B.input_deck(DECK)["detectors"],
                              Path("unused-statepoint"), 0.625)
        volume = R.sphere_volume_cm3(0.625)
        self.assertAlmostEqual(got["angle-24"]["u_mc"], (0.07 / volume) * 2.1 / math.log(10.0))
        self.assertEqual(got["angle-24"]["openmc_tracklength_mean_particle_cm_per_source"], 100.0)
        self.assertAlmostEqual(got["angle-24"]["finite_volume_normalization"]["volume_cm3"], volume)
        self.assertNotAlmostEqual(got["angle-24"]["u_mc"],
                                  math.sqrt(0.11**2 + 0.07**2) * 2.1 / math.log(10.0) / volume)

    def test_nonfinite_or_nonphysical_tallies_are_rejected(self):
        for values in ((math.nan, 0.1, 2.0, 1.0, 2.0), (1.0, math.inf, 2.0, 1.0, 2.0),
                       (1.0, -0.1, 2.0, 1.0, 2.0), (1.0, 0.1, -2.0, 1.0, 2.0)):
            with self.subTest(values=values), self.assertRaises(ValueError):
                radius = 0.625
                R.tally_value(*values, R.sphere_volume_cm3(radius), radius)
        with self.assertRaisesRegex(ValueError, "sphere geometry must be finite"):
            R.tally_value(1.0, 0.1, 2.0, 1.0, 2.0, math.inf, 0.625)

    def test_finite_volume_gate_requires_two_all_bin_successive_halvings(self):
        levels = [
            {"radius_cm": 2.5, "bins": {"a": {"value": 10.0, "u_mc": 0.1}, "b": {"value": 5.0, "u_mc": 0.05}}},
            {"radius_cm": 1.25, "bins": {"a": {"value": 10.02, "u_mc": 0.1}, "b": {"value": 5.01, "u_mc": 0.05}}},
            {"radius_cm": 0.625, "bins": {"a": {"value": 10.01, "u_mc": 0.1}, "b": {"value": 5.02, "u_mc": 0.05}}},
        ]
        self.assertTrue(B.qualify_halvings(levels)["qualified"])
        levels[-1]["bins"]["b"]["value"] = 5.2
        self.assertFalse(B.qualify_halvings(levels)["qualified"])

    def test_finite_volume_gate_keeps_rare_or_missing_rows_inconclusive(self):
        levels = [
            {"radius_cm": 2.5, "bins": {"rare": {"value": 0.0, "u_mc": 0.0}}},
            {"radius_cm": 1.25, "bins": {"rare": {"value": 1.0, "u_mc": 0.01}}},
            {"radius_cm": 0.625, "bins": {"rare": {"value": 1.0, "u_mc": 0.01}}},
        ]
        self.assertFalse(B.qualify_halvings(levels)["qualified"])
        levels[1]["bins"] = {}
        self.assertFalse(B.qualify_halvings(levels)["qualified"])

    def test_run_requires_explicit_cpu_consent_before_importing_solver(self):
        args = type("Args", (), {"i_have_the_cpu": False})()
        with self.assertRaisesRegex(ValueError, "--i-have-the-cpu"):
            R.command_run(args)

    def test_generated_public_manifests_keep_every_table_row_and_exact_tally_join(self):
        cases_root = ROOT / "validation" / "cases"
        manifests = sorted(cases_root.glob("fns-tof-*/manifest.json"))
        self.assertEqual(len(manifests), 14)
        for path in manifests:
            with self.subTest(manifest=path.name):
                self.assertLess(path.stat().st_size, 1_000_000)
                manifest = load_manifest(path)
                self.assertEqual(path.read_text(encoding="utf-8"), BM.render_manifest(manifest))
                self.assertEqual(path.parent.name, manifest["case_id"])
                points = manifest["adapter"]["detectors"]
                mapping = R.validate_response_mapping(manifest, {"detectors": points})
                self.assertEqual(set(mapping), {d["id"] for d in manifest["detectors"]})
                self.assertEqual(len(mapping), sum(len(t["detector_ids"]) for t in manifest["adapter"]["tables"]))
                self.assertTrue(manifest["files_are_external_not_vendored"])
                self.assertEqual(manifest["license"]["spdx"], "CC-BY-4.0")
                self.assertTrue(all(t["path_in_source"] for t in manifest["adapter"]["tables"]))
                self.assertTrue(all(t["geometry_factor_check"]["relative_difference"] < 0.004
                                    for t in manifest["adapter"]["tables"]))
                if manifest["case_id"] == "fns-tof-fe-60":
                    self.assertEqual(manifest["adapter"]["missing_table_angles_deg"], [66.8])
                if manifest["case_id"] == "fns-tof-c-5":
                    self.assertEqual(len({t["angle_deg"] for t in manifest["adapter"]["tables"]}), 4)

    def test_standard_discovery_registers_all_flux_rows_without_validating_them(self):
        discovered = [load_manifest(path) for path in report_mod.discover()]
        fns_ids = {m["case_id"] for m in discovered if m["case_id"].startswith("fns-tof-")}
        self.assertEqual(fns_ids, {path.parent.name for path in (ROOT / "validation" / "cases").glob("fns-tof-*/manifest.json")})
        suite = report_mod.build_suite(discovered, {}, None, registry=load_registry())
        rows = [row for row in suite["matrix"]["rows"] if row["benchmark_id"].startswith("fns-tof-")]
        self.assertEqual(len(rows), 14)
        self.assertTrue(all(row["status"] == "not validated" for row in rows))
        self.assertEqual({row["benchmark_id"].split("/")[0] for row in rows}, fns_ids)
        flux = next(q for q in load_registry()["quantities"] if q["id"] == "flux")
        registered = {b["case_id"] for b in flux["benchmarks"] if b["case_id"].startswith("fns-tof-")}
        self.assertEqual(registered, fns_ids)
        self.assertTrue(all("not validation" in b["scope"] and "two-halving" in b["scope"]
                            for b in flux["benchmarks"] if b["case_id"].startswith("fns-tof-")))

    def test_cli_help_is_callable_by_script_path(self):
        result = subprocess.run([sys.executable, str(CASE / "run_case.py"), "--help"],
                                check=True, capture_output=True, text=True)
        self.assertIn("three-radius transport qualification", result.stdout)

    def test_manifest_builder_help_is_callable_by_script_path(self):
        result = subprocess.run([sys.executable, str(CASE / "build_manifest.py"), "--help"],
                                check=True, capture_output=True, text=True)
        self.assertIn("clean IAEA-NDS/open-benchmarks checkout", result.stdout)


if __name__ == "__main__":
    unittest.main()
