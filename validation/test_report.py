# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import report as R  # noqa: E402
from validation.manifest import load_manifest  # noqa: E402
from validation.fixtures import IDENT, clone, manifest, run_record  # noqa: E402
from validation.scoring import Identity, ValidationError  # noqa: E402


class Report(unittest.TestCase):
    def suite(self, with_run=True):
        m = manifest()
        records = {"fixture": run_record(m, [d["reference"]["value"] * 1.01 for d in m["detectors"]])} if with_run else {}
        return R.build_suite([m], records, IDENT if with_run else None)

    # Verifies: VAL-030
    def test_markdown_carries_the_distribution_and_worst_case(self):
        text = R.render_markdown(self.suite())
        for label in ("| count | mean | median | std dev | 5th | 95th | min | max |", "Worst case:", "Bias (mean log C/E)", "seed 20261009"):
            self.assertIn(label, text)

    # Verifies: VAL-075
    def test_markdown_lists_what_is_not_covered(self):
        self.assertIn("Not covered by this case:", R.render_markdown(self.suite()))

    # Verifies: VAL-056
    def test_coverage_comes_with_its_list(self):
        suite = self.suite(with_run=False)
        self.assertEqual(suite["coverage"]["scored"], 0)
        self.assertEqual(len(suite["coverage"]["unscored"]), 6)
        self.assertTrue(all(u["why"] and u["next_step"] for u in suite["coverage"]["unscored"]))

    def test_report_lint_is_clean_and_catches_missing_next_step(self):
        suite = self.suite()
        self.assertEqual(R.lint_report(suite), [])
        bad = clone(suite)
        bad["cases"][0]["rows"][0].update(verdict="INCONCLUSIVE", why="x", next_step=None)
        self.assertTrue(any("without why and next step" in p for p in R.lint_report(bad)))
        bad = clone(suite)
        bad["cases"][0]["rows"][0]["evidence_class"] = None
        self.assertTrue(any("evidence class" in p for p in R.lint_report(bad)))

    def test_run_record_needs_a_current_identity(self):
        m = manifest()
        with self.assertRaises(Exception):
            R.build_suite([m], {"fixture": run_record(m, [1.0] * 6)}, None)

    def _mixed_openmc_actinv_inputs(self):
        case_ids = ("oktavian-al", "fns-decayheat-fe-1996-5min")
        manifests = [load_manifest(R.CASES_DIR / case_id / "manifest.json") for case_id in case_ids]
        records = R.load_records(manifests, R.CASES_DIR)
        identities = {
            case_ids[0]: Identity(
                "ba27c6351df74a3a6367fdbb971d6c9772928db9837c19d1c70f90fb7595e607",
                "openmc-0.15.3",
                "4272ffd1d701de242fedf51b2ecf0d9c23e59ff4ca83179260dba660e79c911d",
            ),
            case_ids[1]: Identity(
                "82b9105d8f594a2644259ebca8ab098e43ab8df55e191a575864069a4c94eeef",
                "actinv-core 1.4.0",
                "500b987e290976df76d7f0a4fa1bc8edfd6221f386c606434d6a430daa57f9fb",
            ),
        }
        return case_ids, manifests, records, identities

    def test_current_identity_map_supports_mixed_openmc_and_actinv_records(self):
        case_ids, manifests, records, identities = self._mixed_openmc_actinv_inputs()
        suite = R.build_suite(manifests, records, None, current_by_case=identities)
        by_case = {case["case_id"]: case for case in suite["cases"]}
        self.assertEqual(by_case[case_ids[0]]["current_identity"], identities[case_ids[0]].as_dict())
        self.assertEqual(by_case[case_ids[1]]["current_identity"], identities[case_ids[1]].as_dict())
        self.assertFalse(any(row["verdict"] == "STALE" for case in suite["cases"] for row in case["rows"]))
        self.assertEqual(suite["provenance"]["current_identity_mode"], "per-case")
        self.assertEqual(len(suite["provenance"]["current_identity_mapping_sha256"]), 64)

    def test_changed_adapter_stales_only_its_case_in_mixed_identity_map(self):
        case_ids, manifests, records, identities = self._mixed_openmc_actinv_inputs()
        identities[case_ids[1]] = Identity(
            identities[case_ids[1]].library_sha256,
            identities[case_ids[1]].code_version,
            "600b987e290976df76d7f0a4fa1bc8edfd6221f386c606434d6a430daa57f9fb",
        )
        suite = R.build_suite(manifests, records, None, current_by_case=identities)
        stale_cases = {
            case["case_id"]
            for case in suite["cases"]
            if any(row["verdict"] == "STALE" for row in case["rows"])
        }
        self.assertEqual(stale_cases, {case_ids[1]})

    def test_identity_map_refuses_missing_unknown_duplicate_case_or_identity_keys(self):
        known = {"openmc-case", "actinv-case"}
        record_ids = {"openmc-case", "actinv-case"}
        identity = Identity("a" * 64, "code", "b" * 64)
        with self.assertRaisesRegex(ValidationError, "missing current identities"):
            R._validate_current_identity_map({"openmc-case": identity}, known, record_ids)
        fixture = manifest()
        with self.assertRaisesRegex(ValidationError, "missing current identities"):
            R.build_suite([fixture], {"fixture": run_record(fixture, [1.0] * 6)}, None, current_by_case={})
        with self.assertRaisesRegex(ValidationError, "unknown case ids"):
            R._validate_current_identity_map(
                {"openmc-case": identity, "actinv-case": identity, "unknown": identity},
                known,
                record_ids,
            )
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "current.json"
            ident = json.dumps(identity.as_dict())
            prefix = '{"schema":"' + R.CURRENT_IDENTITIES_SCHEMA + '","cases":{'
            suffix = ',"actinv-case":' + ident + '}}'
            path.write_text(
                prefix + '"openmc-case":' + ident + ',"openmc-case":' + ident + suffix,
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValidationError, "duplicate JSON key 'openmc-case'"):
                R.load_current_identity_map(path, known, record_ids)
            path.write_text(
                prefix + '"openmc-case":{"library_sha256":"' + "a" * 64
                + '","library_sha256":"' + "a" * 64
                + '","code_version":"code","adapter_sha256":"' + "b" * 64 + '"}' + suffix,
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValidationError, "duplicate JSON key 'library_sha256'"):
                R.load_current_identity_map(path, known, record_ids)

    def test_identity_map_report_provenance_binds_exact_input_bytes(self):
        case_ids, manifests, records, identities = self._mixed_openmc_actinv_inputs()
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "identities.json"
            raw = json.dumps(
                {
                    "schema": R.CURRENT_IDENTITIES_SCHEMA,
                    "cases": {case_id: identities[case_id].as_dict() for case_id in case_ids},
                },
                indent=2,
            ).encode("utf-8")
            path.write_bytes(raw)
            loaded = R.load_current_identity_map(path, set(case_ids), set(records))
            suite = R.build_suite(
                manifests,
                records,
                None,
                current_by_case=loaded.cases,
                current_identity_file_sha256=loaded.input_sha256,
            )
            self.assertEqual(loaded.input_sha256, hashlib.sha256(raw).hexdigest())
            self.assertEqual(suite["provenance"]["current_identity_file_sha256"], hashlib.sha256(raw).hexdigest())
            markdown = R.render_markdown(suite)
            self.assertIn(hashlib.sha256(raw).hexdigest(), markdown)

    def test_load_records_finds_nested_sealed_adapter_record_without_rewriting_it(self):
        m = {"case_id": "oktavian-al"}
        record = {"case_id": "oktavian-al", "run_id": "run_csg_final", "results": [{"value": 3.0}]}
        with tempfile.TemporaryDirectory() as d:
            nested = Path(d) / "oktavian-al-cad" / "records" / "csg-comparator"
            nested.mkdir(parents=True)
            path = nested / "oktavian-al.run.json"
            path.write_text(json.dumps(record), encoding="utf-8")
            loaded = R.load_records([m], Path(d))["oktavian-al"]
            self.assertEqual(loaded, record)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8")), record)

    def test_load_records_rejects_ambiguous_nested_records(self):
        m = {"case_id": "oktavian-al"}
        with tempfile.TemporaryDirectory() as d:
            for folder in ("a", "b"):
                nested = Path(d) / folder
                nested.mkdir()
                (nested / "oktavian-al.run.json").write_text(json.dumps({"case_id": "oktavian-al"}), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "multiple run records"):
                R.load_records([m], Path(d))

    # Verifies: VAL-091
    def test_one_command_writes_json_and_markdown_for_the_registered_cases(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(R.main(["report", "--out", d]), 0)
            suite = json.loads((Path(d) / "report.json").read_text(encoding="utf-8"))
            self.assertEqual({c["case_id"] for c in suite["cases"]}, {p.parent.name for p in R.discover()})
            self.assertTrue((Path(d) / "report.md").read_text(encoding="utf-8").startswith("# FARIS validation report"))
            self.assertEqual(suite["coverage"]["scored"], 0)

    # Verifies: VAL-091
    def test_report_carries_every_section_and_records_its_hash(self):
        with tempfile.TemporaryDirectory() as d, tempfile.TemporaryDirectory() as d2:
            record = Path(d) / "record.json"
            record.write_text(json.dumps({"version": "x"}), encoding="utf-8")
            self.assertEqual(R.main(["report", "--out", d, "--release-record", str(record)]), 0)
            self.assertEqual(R.main(["report", "--out", d2]), 0)
            suite = json.loads((Path(d) / "report.json").read_text(encoding="utf-8"))
            for key in ("methods", "matrix", "cases", "qualified_ranges", "known_issues", "last_run"):
                self.assertTrue(suite[key], key)
            self.assertTrue(all("distribution" in cls for c in suite["cases"] for cls in c["classes"].values()))
            text = (Path(d) / "report.md").read_text(encoding="utf-8")
            for heading in ("## Methods", "## V&V matrix", "## Known issues", "## Qualified ranges", "## Last run of each benchmark"):
                self.assertIn(heading, text)
            rec = json.loads(record.read_text(encoding="utf-8"))
            self.assertEqual(rec["version"], "x")
            self.assertEqual(rec["vv_report_sha256"], hashlib.sha256((Path(d) / "report.json").read_bytes()).hexdigest())
            self.assertIn(rec["vv_report_sha256"], (Path(d) / "report.sha256").read_text(encoding="utf-8"))
            # The same inputs give the same bytes, so the recorded hash identifies the report.
            self.assertEqual((Path(d) / "report.json").read_bytes(), (Path(d2) / "report.json").read_bytes())

    # Verifies: VAL-091
    def test_last_run_date_comes_from_the_run_record(self):
        m = manifest()
        record = run_record(m, [1.0] * 6)
        self.assertEqual(R.build_suite([m], {"fixture": record}, IDENT)["last_run"]["fixture"], "run date not recorded in the run record")
        from validation.scoring import seal_run_record
        dated = seal_run_record(dict(record, run_date="2026-10-12"))
        self.assertEqual(R.build_suite([m], {"fixture": dated}, IDENT)["last_run"]["fixture"], "2026-10-12")

    def test_registered_cases_with_a_synthetic_record_score_and_go_stale(self):
        m = load_manifest(R.CASES_DIR / "oktavian-al" / "manifest.json")
        calc = [d["reference"]["value"] for d in m["detectors"]]
        suite = R.build_suite([m], {"oktavian-al": run_record(m, calc, u_mc=1e-6)}, IDENT)
        counts = suite["cases"][0]["verdict_counts"]
        # The normalisation is confirmed; the photon bins and the first neutron bin stay blocked, so only the other 133 neutron bins may be scored.
        self.assertEqual(counts["PASS"] + counts["FAIL"] + counts["INCONCLUSIVE"], 133)
        self.assertEqual(counts["NOT_EVALUATED"], 58)
        self.assertEqual(R.lint_report(suite), [])


if __name__ == "__main__":
    unittest.main()
