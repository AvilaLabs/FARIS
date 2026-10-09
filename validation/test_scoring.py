# SPDX-License-Identifier: AGPL-3.0-only
import math
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import scoring as S  # noqa: E402
from validation.fixtures import IDENT, clone, manifest, run_record  # noqa: E402

IND = {"kind": "independent"}


class CompatibilityRule(unittest.TestCase):
    # Verifies: VAL-027
    def test_exact_boundary_passes_and_just_beyond_fails(self):
        # u_c 3, u_e 4 -> combined 5 exactly; k = 1.
        self.assertEqual(S.compatibility(15.0, 10.0, 3.0, 4.0, 1.0, IND)["verdict"], S.PASS)
        self.assertEqual(S.compatibility(10.0, 15.0, 3.0, 4.0, 1.0, IND)["verdict"], S.PASS)
        self.assertEqual(S.compatibility(15.0 + 1e-9, 10.0, 3.0, 4.0, 1.0, IND)["verdict"], S.FAIL)

    # Verifies: VAL-027
    def test_k_scales_the_allowed_difference(self):
        self.assertEqual(S.compatibility(19.9, 10.0, 3.0, 4.0, 2.0, IND)["verdict"], S.PASS)
        self.assertEqual(S.compatibility(20.1, 10.0, 3.0, 4.0, 2.0, IND)["verdict"], S.FAIL)

    # Verifies: VAL-027
    def test_covariance_sign_changes_the_allowed_difference(self):
        # rho = +1: variance 9 + 16 - 24 = 1; rho = -1: variance 9 + 16 + 24 = 49.
        pos, neg = {"kind": "correlation", "rho": 1.0}, {"kind": "correlation", "rho": -1.0}
        self.assertEqual(S.compatibility(16.0, 10.0, 3.0, 4.0, 1.0, pos)["verdict"], S.FAIL)
        self.assertEqual(S.compatibility(16.0, 10.0, 3.0, 4.0, 1.0, neg)["verdict"], S.PASS)
        self.assertAlmostEqual(S.compatibility(16.0, 10.0, 3.0, 4.0, 1.0, neg)["allowed"], 7.0)
        self.assertAlmostEqual(S.compatibility(16.0, 10.0, 3.0, 4.0, 1.0, pos)["allowed"], 1.0)

    # Verifies: VAL-027
    def test_shared_normalisation_term_tightens_the_test(self):
        # u 2 and 2, values 12.5 and 10: independent allows 2 * sqrt(8) / 2 = 2.83; a shared 10 % factor
        # gives Cov = 0.01 * 12.5 * 10 = 1.25, variance 8 - 2.5 = 5.5, allowed 2.35.
        spec = {"kind": "shared_normalisation", "shared_relative_u": 0.1}
        self.assertEqual(S.compatibility(12.5, 10.0, 2.0, 2.0, 1.0, IND)["verdict"], S.PASS)
        self.assertEqual(S.compatibility(12.5, 10.0, 2.0, 2.0, 1.0, spec)["verdict"], S.FAIL)
        # Perfect common scale error cancels: variance 1 + 1 - 2 = 0 (round-off tolerated).
        self.assertEqual(S.compatibility(10.0, 10.0, 1.0, 1.0, 2.0, spec)["verdict"], S.PASS)

    # Verifies: VAL-027
    def test_unknown_covariance_is_inconclusive_never_pass(self):
        out = S.compatibility(10.0, 10.0, 1.0, 1.0, 2.0, {"kind": "unknown"})
        self.assertEqual(out["verdict"], S.INCONCLUSIVE)
        self.assertTrue(out["why"] and out["next_step"])

    # Verifies: VAL-027
    def test_missing_uncertainty_is_inconclusive(self):
        out = S.compatibility(10.0, 10.0, None, 1.0, 2.0, IND)
        self.assertEqual(out["verdict"], S.INCONCLUSIVE)
        self.assertTrue(out["why"] and out["next_step"])

    # Verifies: VAL-027
    def test_impossible_covariance_is_inconclusive(self):
        out = S.compatibility(10.0, 10.0, 1.0, 1.0, 2.0, {"kind": "explicit", "value": 5.0})
        self.assertEqual(out["verdict"], S.INCONCLUSIVE)

    def test_bad_inputs_raise(self):
        with self.assertRaises(S.ValidationError):
            S.compatibility(1, 1, 1, 1, 0.0, IND)
        with self.assertRaises(S.ValidationError):
            S.compatibility(1, 1, 1, 1, 2.0, {"kind": "correlation", "rho": 1.5})
        with self.assertRaises(S.ValidationError):
            S.compatibility(1, 1, 1, 1, 2.0, {"kind": "made-up"})

    # Verifies: VAL-028
    def test_monte_carlo_error_dominating_is_inconclusive(self):
        ok = S.compatibility(10.0, 10.0, 1.0, 1.0, 2.0, IND, u_c_mc=0.5)
        noisy = S.compatibility(10.0, 10.0, 1.0, 1.0, 2.0, IND, u_c_mc=0.51)
        self.assertEqual(ok["verdict"], S.PASS)
        self.assertEqual(noisy["verdict"], S.INCONCLUSIVE)
        self.assertIn("0.5", noisy["next_step"])


class Statistics(unittest.TestCase):
    ITEMS = [("a", 0.5), ("b", 1.0), ("c", 1.5), ("d", 2.0), ("e", 4.0)]

    # Verifies: VAL-030
    def test_distribution_matches_hand_values(self):
        d = S.distribution(self.ITEMS)
        self.assertEqual(d["count"], 5)
        self.assertAlmostEqual(d["mean"], 1.8)
        self.assertAlmostEqual(d["median"], 1.5)
        self.assertAlmostEqual(d["std_dev"], math.sqrt(7.3 / 4))
        self.assertAlmostEqual(d["p05"], 0.6)
        self.assertAlmostEqual(d["p95"], 3.6)
        self.assertEqual((d["min"], d["max"]), (0.5, 4.0))
        for key in ("count", "mean", "median", "std_dev", "p05", "p95", "min", "max"):
            self.assertIn(key, d)

    # Verifies: VAL-030
    def test_worst_case_is_named_and_first(self):
        d = S.distribution(self.ITEMS)
        self.assertEqual(d["worst"]["detector_id"], "e")
        self.assertEqual([w["detector_id"] for w in d["worst_first"]][:2], ["e", "d"])

    # Verifies: VAL-030
    def test_single_detector_has_no_standard_deviation(self):
        d = S.distribution([("only", 1.2)])
        self.assertIsNone(d["std_dev"])
        self.assertEqual(d["median"], 1.2)

    # Verifies: VAL-031
    def test_bias_recovers_a_known_value_and_interval(self):
        items = [(f"d{i}", math.exp(0.1)) for i in range(8)]
        b = S.bias(items)
        self.assertAlmostEqual(b["bias"], 0.1)
        self.assertAlmostEqual(b["ci95"][0], 0.1)
        self.assertAlmostEqual(b["ci95"][1], 0.1)
        self.assertEqual(b["status"], "estimated")

    # Verifies: VAL-031
    def test_bias_interval_brackets_the_mean_and_is_deterministic(self):
        items = [(f"d{i}", math.exp(0.05 * (i - 5))) for i in range(11)] + [("x", math.exp(0.3))]
        first, second, other = S.bias(items, seed=7), S.bias(items, seed=7), S.bias(items, seed=8)
        self.assertEqual(first, second)
        self.assertNotEqual(first["ci95"], other["ci95"])
        self.assertLess(first["ci95"][0], first["bias"])
        self.assertGreater(first["ci95"][1], first["bias"])
        self.assertEqual(first["seed"], 7)
        self.assertGreaterEqual(first["resamples"], 5000)

    # Verifies: VAL-031
    def test_fewer_than_five_detectors_is_labelled_too_few(self):
        b = S.bias([(f"d{i}", 1.1) for i in range(4)])
        self.assertEqual(b["status"], "too few to estimate")
        self.assertIsNone(b["ci95"])
        self.assertTrue(b["why"] and b["next_step"])
        self.assertEqual(S.bias([(f"d{i}", 1.1) for i in range(5)])["status"], "estimated")

    # Verifies: VAL-031
    def test_too_few_resamples_refused(self):
        with self.assertRaises(S.ValidationError):
            S.bias([(f"d{i}", 1.1) for i in range(6)], resamples=4999)


class Aggregation(unittest.TestCase):
    def rows(self, **change):
        base = {"response_class": "flux", "library_sha256": "l1", "code_version": "v1"}
        return [dict(base, detector_id=f"d{i}", ce=1.0 + 0.1 * i, **({k: v for k, v in change.items()} if i == 1 else {})) for i in range(3)]

    # Verifies: VAL-035
    def test_refuses_to_mix_classes_libraries_or_code_versions(self):
        for field, value in (("response_class", "dose"), ("library_sha256", "l2"), ("code_version", "v2")):
            with self.subTest(field=field), self.assertRaises(S.AggregationError):
                S.aggregate_ce(self.rows(**{field: value}))

    # Verifies: VAL-035
    def test_one_class_one_library_one_version_aggregates(self):
        agg = S.aggregate_ce(self.rows())
        self.assertEqual(agg["distribution"]["count"], 3)


class CaseScoring(unittest.TestCase):
    # Verifies: VAL-027, VAL-001
    def test_rows_carry_verdicts_and_evidence_class(self):
        m = manifest()
        calc = [d["reference"]["value"] for d in m["detectors"]]
        calc[2] += 1.0  # far outside k * sqrt(0.001^2 + 0.05^2)
        result = S.score_case(m, run_record(m, calc), IDENT)
        verdicts = {r["detector_id"]: r["verdict"] for r in result["rows"]}
        self.assertEqual(verdicts["d2"], S.FAIL)
        self.assertEqual(verdicts["d0"], S.PASS)
        self.assertTrue(all(r["evidence_class"] == "experiment" for r in result["rows"]))
        self.assertEqual(result["classes"]["flux"]["distribution"]["worst"]["detector_id"], "d2")

    # Verifies: VAL-036
    def test_literature_record_is_never_scored(self):
        m = manifest()
        lit = {"kind": "literature", "case_id": "fixture", "run_id": "paper", "identity": IDENT.as_dict(), "results": [], "record_sha256": "x"}
        with self.assertRaises(S.ValidationError):
            S.score_case(m, lit, IDENT)

    # Verifies: VAL-036
    def test_run_record_is_hash_bound(self):
        m = manifest()
        rec = run_record(m, [1.0] * 6)
        rec["results"][0]["value"] = 5.0  # edited after sealing
        with self.assertRaises(S.ValidationError):
            S.score_case(m, rec, IDENT)
        with self.assertRaises(S.ValidationError):
            S.score_case(m, run_record(m, [1.0] * 6, case_id="another-case"), IDENT)
        unsealed = {k: v for k, v in run_record(m, [1.0] * 6).items() if k != "record_sha256"}
        with self.assertRaises(S.ValidationError):
            S.score_case(m, unsealed, IDENT)

    # Verifies: VAL-037
    def test_changed_identity_marks_rows_stale(self):
        m = manifest()
        rec = run_record(m, [d["reference"]["value"] for d in m["detectors"]])
        for field, value in (("library_sha256", "e" * 64), ("code_version", "openmc-0.16.0"), ("adapter_sha256", "f" * 64)):
            current = S.Identity.from_dict({**IDENT.as_dict(), field: value})
            result = S.score_case(m, rec, current)
            with self.subTest(field=field):
                self.assertTrue(all(r["verdict"] == S.STALE for r in result["rows"]))
                self.assertIn(field, result["rows"][0]["why"])
                self.assertTrue(result["rows"][0]["next_step"])
                self.assertEqual(result["rows"][0]["library_sha256"], IDENT.library_sha256)
        self.assertTrue(all(r["verdict"] == S.PASS for r in S.score_case(m, rec, IDENT)["rows"]))

    # Verifies: VAL-037
    def test_rows_record_library_code_and_adapter(self):
        m = manifest()
        result = S.score_case(m, run_record(m, [1.0] * 6), IDENT)
        row = result["rows"][0]
        self.assertEqual((row["library_sha256"], row["code_version"], row["adapter_sha256"]), (IDENT.library_sha256, IDENT.code_version, IDENT.adapter_sha256))

    # Verifies: VAL-006
    def test_no_run_record_is_not_evaluated_with_reason(self):
        result = S.score_case(manifest(), None, IDENT)
        self.assertTrue(all(r["verdict"] == S.NOT_EVALUATED and r["why"] and r["next_step"] for r in result["rows"]))

    # Verifies: VAL-029
    def test_unconfirmed_normalisation_keeps_ce_but_blocks_pass(self):
        m = manifest(norm_status="inferred")
        result = S.score_case(m, run_record(m, [d["reference"]["value"] for d in m["detectors"]]), IDENT)
        self.assertTrue(all(r["verdict"] == S.INCONCLUSIVE and r["ce"] is not None for r in result["rows"]))
        self.assertIn("inferred", result["rows"][0]["why"])

    def test_blocked_detector_and_missing_reference(self):
        m = manifest(3)
        m["detectors"][0]["blocked"] = {"why": "bin straddles the limit", "next_step": "drop it"}
        m["detectors"][1]["reference"] = None
        m["detectors"][1]["reference_missing_why"] = "no value"
        m["detectors"][1]["reference_missing_next_step"] = "find one"
        result = S.score_case(m, run_record(m, [1.0, 1.1, 1.2]), IDENT)
        self.assertEqual([r["verdict"] for r in result["rows"]][:2], [S.NOT_EVALUATED, S.NOT_EVALUATED])
        self.assertEqual(result["rows"][0]["why"], "bin straddles the limit")

    def test_zero_reference_has_no_ce(self):
        m = manifest(values=[(0.0, 0.1), (1.0, 0.1)])
        result = S.score_case(m, run_record(m, [0.0, 1.0]), IDENT)
        self.assertEqual(result["rows"][0]["verdict"], S.NOT_EVALUATED)
        self.assertIsNone(result["rows"][0]["ce"])

    # Verifies: VAL-030, VAL-031
    def test_two_response_classes_are_kept_apart(self):
        m = manifest(6)
        for d in m["detectors"][3:]:
            d["response_class"] = "dose"
        result = S.score_case(m, run_record(m, [d["reference"]["value"] for d in m["detectors"]]), IDENT)
        self.assertEqual(set(result["classes"]), {"flux", "dose"})
        self.assertEqual(result["classes"]["flux"]["bias"]["status"], "too few to estimate")
        self.assertNotIn("overall", result)


if __name__ == "__main__":
    unittest.main()
