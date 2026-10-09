# SPDX-License-Identifier: AGPL-3.0-only
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from validation import manifest as M  # noqa: E402
from validation.fixtures import clone, manifest  # noqa: E402

CASES = Path(__file__).resolve().parent / "cases"
UPSTREAM = Path.home() / ".cache/avila-night/validation-data/open-benchmarks"


class ManifestValidation(unittest.TestCase):
    def problems(self, mutate) -> list[str]:
        m = clone(manifest())
        mutate(m)
        return M.validate_manifest(m)

    def test_fixture_is_valid(self):
        self.assertEqual(M.validate_manifest(manifest()), [])

    # Verifies: VAL-001
    def test_evidence_class_is_required_and_closed(self):
        self.assertTrue(any("evidence_class" in p for p in self.problems(lambda m: m.pop("evidence_class"))))
        self.assertTrue(any("evidence_class" in p for p in self.problems(lambda m: m.update(evidence_class="plant reproduction"))))
        for ok in ("verification", "code-to-code", "experiment"):
            self.assertEqual(self.problems(lambda m, ok=ok: m.update(evidence_class=ok)), [])

    def test_source_needs_a_pin_licence_and_hashes(self):
        self.assertTrue(any("pinned commit or a DOI" in p for p in self.problems(lambda m: m["source"].pop("commit"))))
        self.assertEqual(self.problems(lambda m: (m["source"].pop("commit"), m["source"].update(doi="10.1000/xyz123"))), [])
        self.assertTrue(any("40-character" in p for p in self.problems(lambda m: m["source"].update(commit="main"))))
        self.assertTrue(any("spdx" in p for p in self.problems(lambda m: m["license"].pop("spdx"))))
        self.assertTrue(any("attribution" in p for p in self.problems(lambda m: m["license"].pop("attribution"))))
        self.assertTrue(any("sha256" in p for p in self.problems(lambda m: m["files"][0].update(sha256="abc"))))

    def test_upstream_files_must_stay_external(self):
        self.assertTrue(any("external_not_vendored" in p for p in self.problems(lambda m: m.update(files_are_external_not_vendored=False))))

    # Verifies: VAL-027
    def test_rule_parameters_are_per_case_and_checked(self):
        self.assertTrue(any("compatibility.k" in p for p in self.problems(lambda m: m["compatibility"].update(k=0))))
        self.assertTrue(any("covariance.kind" in p for p in self.problems(lambda m: m["compatibility"]["covariance"].update(kind="guess"))))
        self.assertTrue(any("rho" in p for p in self.problems(lambda m: m["compatibility"]["covariance"].update(kind="correlation", rho=2))))
        self.assertTrue(any("covariance" in p for p in self.problems(lambda m: m["compatibility"].pop("covariance"))))

    # Verifies: VAL-029
    def test_case_record_fields_are_required(self):
        for key in ("quantity", "units", "source_normalisation", "location", "energy_integration"):
            self.assertTrue(any(key in p for p in self.problems(lambda m, key=key: m["normalisation"].pop(key))), key)
        self.assertTrue(any("uncertainty" in p for p in self.problems(lambda m: m.pop("uncertainty"))))

    def test_detector_errors_are_specific(self):
        self.assertTrue(any("duplicate" in p for p in self.problems(lambda m: m["detectors"][1].update(id="d0"))))
        self.assertTrue(any("not declared" in p for p in self.problems(lambda m: m["detectors"][0].update(response_class="heating"))))
        self.assertTrue(any("missing uncertainty" in p for p in self.problems(lambda m: m["detectors"][0]["reference"].update(u=None))))
        self.assertTrue(any("reference_missing_why" in p for p in self.problems(lambda m: m["detectors"][0].update(reference=None))))
        self.assertTrue(any("blocked" in p for p in self.problems(lambda m: m["detectors"][0].update(blocked={"why": "x"}))))

    # Verifies: VAL-075
    def test_not_covered_list_is_required(self):
        self.assertTrue(any("not_covered" in p for p in self.problems(lambda m: m.update(not_covered=[]))))
        self.assertTrue(any("not_covered" in p for p in self.problems(lambda m: m.pop("not_covered"))))

    # Verifies: VAL-070
    def test_qualified_range_fields_are_required(self):
        for key in ("materials", "geometry_class", "spectrum_class", "cooling_time"):
            self.assertTrue(any(key in p for p in self.problems(lambda m, key=key: m["qualified_range"].pop(key))), key)

    # Verifies: VAL-036
    def test_literature_context_must_be_labelled(self):
        self.assertEqual(self.problems(lambda m: m.update(literature_context=[{"kind": "literature", "label": "paper", "summary": "s"}])), [])
        self.assertTrue(any("literature" in p for p in self.problems(lambda m: m.update(literature_context=[{"kind": "faris-run", "label": "x"}]))))

    def test_error_lists_every_problem(self):
        m = clone(manifest())
        m.pop("title")
        m["compatibility"]["k"] = -1
        with self.assertRaises(M.ManifestError) as ctx:
            M.check_manifest(m)
        self.assertGreaterEqual(len(ctx.exception.problems), 2)
        self.assertIn("title", str(ctx.exception))

    def test_load_reports_unreadable_files(self):
        with tempfile.TemporaryDirectory() as d:
            bad = Path(d) / "m.json"
            bad.write_text("{not json", encoding="utf-8")
            with self.assertRaises(M.ManifestError):
                M.load_manifest(bad)
            with self.assertRaises(M.ManifestError):
                M.load_manifest(Path(d) / "absent.json")

    def test_verify_files_detects_changed_and_missing(self):
        import hashlib
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "in").mkdir()
            f = Path(d) / "in" / "a.xml"
            f.write_text("<a/>", encoding="utf-8")
            m = clone(manifest())
            m["files"][0]["sha256"] = hashlib.sha256(b"<a/>").hexdigest()
            self.assertEqual(M.verify_files(m, d), [])
            f.write_text("<b/>", encoding="utf-8")
            self.assertIn("differs", M.verify_files(m, d)[0])
            f.unlink()
            self.assertIn("missing", M.verify_files(m, d)[0])


class RegisteredCases(unittest.TestCase):
    def test_every_case_manifest_is_valid(self):
        paths = sorted(CASES.glob("*/manifest.json"))
        self.assertGreaterEqual(len(paths), 2)
        for path in paths:
            with self.subTest(case=path.parent.name):
                m = M.load_manifest(path)
                self.assertEqual(m["case_id"], path.parent.name)

    # Verifies: VAL-044
    def test_both_evidence_classes_are_covered_and_literature_is_context(self):
        classes = {M.load_manifest(p)["evidence_class"] for p in CASES.glob("*/manifest.json")}
        self.assertTrue({"experiment", "code-to-code"} <= classes)

    def test_iter_1d_uses_the_registered_identities(self):
        reg = json.loads((Path(__file__).resolve().parent.parent / "references" / "iter-1d-reference.json").read_text(encoding="utf-8"))
        m = M.load_manifest(CASES / "iter-1d" / "manifest.json")
        hashes = {f["path_in_source"].rsplit("/", 1)[1]: f["sha256"] for f in m["files"]}
        self.assertEqual(hashes["geometry.xml"], reg["input_identity"]["geometry_xml"]["sha256"])
        self.assertEqual(hashes["materials.xml"], reg["input_identity"]["materials_xml"]["sha256"])
        self.assertEqual(hashes["iter_1d_source.cpp"], reg["input_identity"]["iter_1d_source_cpp"]["sha256"])

    def test_oktavian_manifest_shape(self):
        m = M.load_manifest(CASES / "oktavian-al" / "manifest.json")
        self.assertEqual((m["license"]["spdx"], m["evidence_class"]), ("CC-BY-4.0", "experiment"))
        self.assertEqual(sum(1 for d in m["detectors"] if d["response_class"] == "neutron_leakage_spectrum"), 134)
        self.assertEqual(sum(1 for d in m["detectors"] if d["response_class"] == "photon_leakage_spectrum"), 57)
        self.assertTrue(all(c["kind"] == "literature" for c in m["literature_context"]))

    @unittest.skipUnless((UPSTREAM / ".git").exists(), "upstream checkout not present")
    def test_upstream_hashes_match_when_checkout_is_present(self):
        for case in ("oktavian-al", "iter-1d"):
            self.assertEqual(M.verify_files(M.load_manifest(CASES / case / "manifest.json"), UPSTREAM), [], case)


if __name__ == "__main__":
    unittest.main()
