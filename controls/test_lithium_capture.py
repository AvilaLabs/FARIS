import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "lithium_capture_control", ROOT / "integrations/openmc/lithium_capture_control.py"
)
CONTROL = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(CONTROL)

CHECK_SPEC = importlib.util.spec_from_file_location(
    "check_lithium_capture", Path(__file__).with_name("check_lithium_capture.py")
)
CHECK = importlib.util.module_from_spec(CHECK_SPEC)
assert CHECK_SPEC.loader is not None
CHECK_SPEC.loader.exec_module(CHECK)


class LithiumCaptureControlTests(unittest.TestCase):
    def test_ame_mass_defect_crosschecks_evaluated_q_values(self):
        q = CONTROL.expected_q_values()
        self.assertAlmostEqual(q["Li6_n_t_alpha_mass_defect_eV"], 4_783_471.7419, delta=0.1)
        self.assertAlmostEqual(q["Li6_n_gamma_Li7_mass_defect_eV"], 7_251_093.8806, delta=0.1)

    # Verifies: NUC-013
    def test_response_screen_passes_consistent_tally_moments(self):
        responses = {
            "h3_production": {"mean": 0.999, "standard_error": 0.001},
            "n_t_reactions": {"mean": 0.999, "standard_error": 0.001},
            "n_gamma_reactions": {"mean": 0.00004, "standard_error": 0.000001},
            "neutron_absorption": {"mean": 0.99904, "standard_error": 0.001},
            "heating_total": {"mean": 4_780_000.5, "standard_error": 1000.0},
            "heating_neutron": {"mean": 4_780_000.0, "standard_error": 1000.0},
            "heating_photon": {"mean": 0.2, "standard_error": 0.01},
            "heating_electron": {"mean": 0.3, "standard_error": 0.01},
            "heating_positron": {"mean": 0.0, "standard_error": 0.0},
        }
        result = CONTROL.validate_responses(
            responses,
            {"Li6_n_t_ENDF_Q_eV": 4_783_800.0, "Li6_n_gamma_ENDF_Q_eV": 7_250_600.0},
        )
        self.assertEqual(result["status"], "PASS")

    # Verifies: NUC-013
    def test_response_screen_rejects_inconsistent_h3_or_heat(self):
        responses = {
            "h3_production": {"mean": 0.7, "standard_error": 0.001},
            "n_t_reactions": {"mean": 0.999, "standard_error": 0.001},
            "n_gamma_reactions": {"mean": 0.00004, "standard_error": 0.000001},
            "neutron_absorption": {"mean": 0.99904, "standard_error": 0.001},
            "heating_total": {"mean": 2_000_000.0, "standard_error": 10.0},
            "heating_neutron": {"mean": 1_000_000.0, "standard_error": 10.0},
            "heating_photon": {"mean": 0.0, "standard_error": 0.0},
            "heating_electron": {"mean": 0.0, "standard_error": 0.0},
            "heating_positron": {"mean": 0.0, "standard_error": 0.0},
        }
        result = CONTROL.validate_responses(
            responses,
            {"Li6_n_t_ENDF_Q_eV": 4_783_800.0, "Li6_n_gamma_ENDF_Q_eV": 7_250_600.0},
        )
        self.assertEqual(result["status"], "FAIL")
        self.assertFalse(result["checks"]["tritium_particle_production_equals_n_t_reaction_events"]["within_heuristic"])

    def test_checker_requires_finite_nonnegative_response_and_unit(self):
        valid = {"responses": {"r": {"mean": 1.0, "standard_error": 0.1, "unit": "u"}}}
        self.assertEqual(CHECK.finite_response(valid, "r", "u"), (CHECK.Decimal("1.0"), CHECK.Decimal("0.1")))
        for bad in [
            {"responses": {"r": {"mean": -1, "standard_error": 0, "unit": "u"}}},
            {"responses": {"r": {"mean": 1, "standard_error": -1, "unit": "u"}}},
            {"responses": {"r": {"mean": "NaN", "standard_error": 0, "unit": "u"}}},
            {"responses": {"r": {"mean": 1, "standard_error": 0, "unit": "other"}}},
        ]:
            with self.assertRaises(ValueError):
                CHECK.finite_response(bad, "r", "u")


if __name__ == "__main__":
    unittest.main()
