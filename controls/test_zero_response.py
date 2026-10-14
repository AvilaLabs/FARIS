"""A tally that scored nothing is never counted as a met precision goal (NUC-019).

The worker's precision report gives a response with a zero mean no relative error and
marks its goal as not met, so a response that no history reached cannot pass as an
exact zero. The test needs only the worker's helper functions, not OpenMC.
"""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


WORKER = load_module("reactor_transport", ROOT / "integrations/openmc/reactor_transport.py")

REQUEST = {"responses": [
    {"id": "total-tritium-production", "domain": {"kind": "whole_model"},
     "score": {"kind": "particle_production", "particle": "tritium", "score": "H3-production"}},
    {"id": "heating-total-whole-model", "domain": {"kind": "whole_model"},
     "score": {"kind": "heating", "convention": "heating", "particle_scope": "total"}},
    {"id": "mesh-flux-0", "domain": {"kind": "mesh", "mesh_id": "m", "bin": 0}, "score": {"kind": "flux"}},
]}
VOLUMES = [{"domain": {"kind": "mesh", "mesh_id": "m", "bin": 0}, "value": 1.0, "standard_error": 0.0}]


def tally(response_id, mean, standard_error):
    return {"response_id": response_id, "mean": mean, "standard_error": standard_error}


def report(tallies):
    return WORKER.sampling_precision_report(REQUEST, tallies, VOLUMES)


class ZeroResponseTests(unittest.TestCase):
    # Verifies: NUC-019
    def test_a_response_with_no_scores_has_no_error_and_fails_its_goal(self):
        result = report([
            tally("total-tritium-production", 1.0, 0.001),
            tally("heating-total-whole-model", 2.0, 0.002),
            tally("mesh-flux-0", 0.0, 0.0),
        ])
        checks = {c["response_id"]: c for c in result["checks"]}
        zero = checks["mesh-flux-0"]
        self.assertIsNone(zero["observed_relative_standard_error"])
        self.assertFalse(zero["met"])
        self.assertTrue(checks["total-tritium-production"]["met"])
        # The run's overall goal is not met because of the empty response alone.
        self.assertFalse(result["all_goals_met"])
        self.assertIn("zero means have undefined RSE and fail the check", result["estimator"])

    # Verifies: NUC-019
    def test_a_zero_standard_error_does_not_make_a_zero_mean_pass(self):
        for mean in (0.0, -0.0):
            result = report([tally("total-tritium-production", mean, 0.0)])
            check = result["checks"][0]
            self.assertIsNone(check["observed_relative_standard_error"])
            self.assertFalse(check["met"])
            self.assertFalse(result["all_goals_met"])

    def test_the_same_goal_is_met_when_the_response_did_score(self):
        result = report([tally("total-tritium-production", 1.0, 0.001)])
        self.assertTrue(result["checks"][0]["met"])
        self.assertTrue(result["all_goals_met"])


if __name__ == "__main__":
    unittest.main()
