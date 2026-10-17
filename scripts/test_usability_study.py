# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import contextlib
import io
import json
import math
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import usability_study as U  # noqa: E402

PATTERN = [(4, 2), (5, 1), (3, 2), (4, 2), (5, 1), (4, 2), (5, 1), (3, 2), (4, 1), (5, 2)]


def sus_csv(n: int) -> str:
    """n participants, each answering with a slight variation of one pattern (scores differ)."""
    lines = ["participant," + ",".join(f"q{i}" for i in range(1, 11))]
    for p in range(n):
        items = []
        for i, (hi, lo) in enumerate(PATTERN):
            base = hi if i % 2 == 0 else lo
            base = min(5, max(1, base + (1 if (p + i) % 7 == 0 else -1 if (p + i) % 5 == 0 and base > 1 else 0)))
            items.append(base)
        lines.append(f"P{p:02d}," + ",".join(map(str, items)))
    return "\n".join(lines) + "\n"


def tasks_csv(n: int) -> str:
    lines = ["participant,task,success,seconds,assisted"]
    for p in range(n):
        for t in ("T1", "T2"):
            ok = 0 if (p + (t == "T2")) % 5 == 0 else 1
            lines.append(f"P{p:02d},{t},{ok},{60 + 7 * p + (20 if t == 'T2' else 0)},0")
    return "\n".join(lines) + "\n"


EXCLUSIONS = json.dumps({"rules": [{"id": "X1", "column": "participant", "equals": "P99", "reason": "prior FARIS use (screening failed)"}]})


class Statistics(unittest.TestCase):
    # Verifies: UX-074
    def test_t_quantiles_match_the_tables(self):
        for df, expected in ((1, 12.7062), (9, 2.2622), (19, 2.0930), (29, 2.0452)):
            self.assertAlmostEqual(U.t_quantile(0.975, df), expected, places=3)

    # Verifies: UX-074
    def test_adjusted_wald_matches_a_hand_computation_and_stays_in_range(self):
        r = U.adjusted_wald(18, 20)
        z2 = U.Z95 ** 2
        p = (18 + z2 / 2) / (20 + z2)
        half = U.Z95 * math.sqrt(p * (1 - p) / (20 + z2))
        self.assertAlmostEqual(r["ci95_low"], p - half)
        self.assertAlmostEqual(r["ci95_high"], p + half)
        edge = U.adjusted_wald(20, 20)
        self.assertLessEqual(edge["ci95_high"], 1.0)
        self.assertLess(edge["ci95_low"], 1.0)
        self.assertGreater(U.adjusted_wald(0, 20)["ci95_high"], 0.0)
        with self.assertRaises(ValueError):
            U.adjusted_wald(3, 2)

    # Verifies: UX-074
    def test_time_summary_has_geometric_mean_median_p90_with_bootstrap_intervals_and_is_reproducible(self):
        times = [40, 55, 60, 62, 70, 75, 90, 110, 150, 300]
        a = U.time_summary(times, resamples=2000)
        self.assertEqual(a, U.time_summary(times, resamples=2000))
        self.assertAlmostEqual(a["geometric_mean"]["value"], math.exp(sum(math.log(t) for t in times) / 10))
        self.assertAlmostEqual(a["median"]["value"], 72.5)
        self.assertAlmostEqual(a["p90"]["value"], 150 + 0.1 * (300 - 150))
        for k in ("geometric_mean", "median", "p90"):
            self.assertLessEqual(a[k]["ci95_low"], a[k]["value"] + 1e-9)
            self.assertGreaterEqual(a[k]["ci95_high"], a[k]["value"] - 1e-9)
        with self.assertRaises(ValueError):
            U.time_summary([10, 0])

    # Verifies: UX-074
    def test_a_mean_sd_ci_on_known_values(self):
        s = U.mean_sd_ci([70, 80, 90])
        self.assertAlmostEqual(s["sd"], 10)
        self.assertAlmostEqual(s["ci95_high"] - s["mean"], 4.3027 * 10 / math.sqrt(3), places=2)


class Sus(unittest.TestCase):
    # Verifies: UX-002
    def test_sus_scoring_uses_the_standard_rule(self):
        self.assertEqual(U.sus_score([5, 1, 5, 1, 5, 1, 5, 1, 5, 1]), 100.0)
        self.assertEqual(U.sus_score([1, 5, 1, 5, 1, 5, 1, 5, 1, 5]), 0.0)
        self.assertEqual(U.sus_score([3] * 10), 50.0)
        with self.assertRaises(ValueError):
            U.sus_score([6] + [3] * 9)

    # Verifies: UX-002
    def test_every_sus_line_carries_n_mean_sd_and_ci_and_a_small_n_is_labelled_formative(self):
        small = U.sus_report([60, 70, 80, 75, 65])
        for part in ("n=5", "mean=70.0", "SD=7.9", "95% CI [", "formative, not a benchmark"):
            self.assertIn(part, small)
        twelve = U.sus_report([float(60 + i) for i in range(12)])
        self.assertIn("n=12", twelve)
        self.assertNotIn("formative", twelve)
        with self.assertRaises(ValueError):
            U.sus_report([80.0])  # a bare mean is refused

    # Verifies: UX-002
    def test_the_verdict_is_computed_and_needs_n_20(self):
        high = [80.0 + (i % 3) for i in range(20)]
        self.assertEqual(U.sus_verdict(high, "F1")["verdict"], "PASS")
        self.assertEqual(U.sus_verdict([60.0 + (i % 3) for i in range(20)], "F1")["verdict"], "FAIL")
        self.assertEqual(U.sus_verdict(high[:10], "F1")["verdict"], "INCONCLUSIVE")


class SusLint(unittest.TestCase):
    # Verifies: UX-002
    def test_a_bare_sus_mean_or_a_small_n_without_the_label_is_found(self):
        good = U.sus_report([60, 70, 80, 75, 65]) + "\n" + U.sus_report([float(60 + i) for i in range(20)])
        self.assertEqual(U.lint_sus_text(good), [])
        found = U.lint_sus_text("Mean SUS was 78.\nSUS n=5, mean=70.0, SD=7.9, 95% CI [60.2, 79.8]\nThe SUS section is next.\n")
        self.assertEqual(len(found), 2)
        self.assertIn("without n, mean, SD, 95% CI", found[0])
        self.assertIn("lacks the label", found[1])

    # Verifies: UX-002
    def test_the_report_generated_for_a_study_passes_its_own_lint(self):
        report = U.build_report(sus_csv(8), tasks_csv(8), EXCLUSIONS)
        self.assertIn(U.FORMATIVE_LABEL, report["sus"]["line"])
        self.assertEqual(U.lint_sus_text(report["sus"]["line"]), [])


class Store(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.d = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)
        self.raw = self.d / "raw"
        self.raw.mkdir()
        self.store = self.d / "store"
        self.files = []
        for name, text in (("sus.csv", sus_csv(20)), ("tasks.csv", tasks_csv(20)), ("exclusions.json", EXCLUSIONS)):
            (self.raw / name).write_text(text, encoding="utf-8")
            self.files.append(self.raw / name)

    def cli(self, *argv) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = U.main(list(argv))
        return code, out.getvalue()

    # Verifies: UX-075
    def test_registering_stores_by_hash_is_idempotent_and_refuses_changed_bytes(self):
        added = U.register(self.store, self.files)
        self.assertEqual({e["name"] for e in added}, {"sus.csv", "tasks.csv", "exclusions.json"})
        self.assertEqual(U.register(self.store, self.files), [])
        self.assertEqual(len(U.ledger_entries(self.store)), 3)
        self.assertEqual(U.verify(self.store), [])
        (self.raw / "sus.csv").write_text(sus_csv(21), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "append-only"):
            U.register(self.store, self.files)
        self.assertEqual(len(U.ledger_entries(self.store)), 3)

    # Verifies: UX-075
    def test_a_tampered_stored_file_fails_verification_and_blocks_the_report(self):
        U.register(self.store, self.files)
        digest = next(e["sha256"] for e in U.ledger_entries(self.store) if e["name"] == "tasks.csv")
        (self.store / "objects" / digest).write_text(tasks_csv(3), encoding="utf-8")
        self.assertTrue(any("no longer match" in p for p in U.verify(self.store)))
        self.assertEqual(self.cli("verify", "--store", str(self.store))[0], 1)
        with self.assertRaisesRegex(ValueError, "hash check"):
            U.report_from_store(self.store)

    # Verifies: UX-075, UX-074
    def test_the_report_is_regenerated_from_the_raw_files_and_a_typed_number_is_found(self):
        U.register(self.store, self.files)
        report_path = self.d / "report.json"
        self.assertEqual(self.cli("report", "--store", str(self.store), "--out", str(report_path))[0], 0)
        self.assertEqual(self.cli("check", "--store", str(self.store), "--report", str(report_path))[0], 0)
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertEqual(report["sus"]["n"], 20)
        self.assertIn("SUS n=20, mean=", report["sus"]["line"])
        self.assertIn(report["sus"]["verdict"]["verdict"], ("PASS", "FAIL"))
        self.assertEqual(sorted(report["tasks"]), ["T1", "T2"])
        report["sus"]["verdict"]["verdict"] = "PASS"
        report["sus"]["mean"] = 99.0
        report_path.write_text(U.render(report), encoding="utf-8")
        self.assertEqual(self.cli("check", "--store", str(self.store), "--report", str(report_path))[0], 1)

    # Verifies: UX-074
    def test_a_result_is_dropped_only_by_a_protocol_rule_and_every_drop_is_reported(self):
        extra = sus_csv(20) + "P99," + ",".join(["1"] * 10) + "\n"
        report = U.build_report(extra, tasks_csv(20) + "P99,T1,0,500,0\n", EXCLUSIONS)
        self.assertEqual(report["sus"]["n"], 20)
        self.assertEqual([d["source"] for d in report["drops"]], ["sus.csv", "tasks.csv"])
        self.assertTrue(all(d["rule"] == "X1" and d["reason"] for d in report["drops"]))
        kept = U.build_report(extra, tasks_csv(20) + "P99,T1,0,500,0\n", json.dumps({"rules": []}))
        self.assertEqual(kept["sus"]["n"], 21)
        self.assertEqual(kept["drops"], [])


if __name__ == "__main__":
    unittest.main()
