#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Tests for scripts/read_study_file.py, the clean-room reader of FARIS study files (INT-001, PRV-053).

The archived fixtures in crates/faris-study/tests/fixtures/ were written by the Rust writer; the Rust test
`every_archived_fixture_opens_verifies_and_matches_its_summary` checks the Rust reader against the summary
committed beside each file, and this test checks the independent reader against the same summaries. The
documents inside the files are also validated against the schemas in docs/schemas/ (Rust checks the
keywords without patterns; this test checks the patterns too).
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import re
import shutil
import tempfile
import unittest
import warnings
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates" / "faris-study" / "tests" / "fixtures"
SCHEMAS = ROOT / "docs" / "schemas"

spec = importlib.util.spec_from_file_location("read_study_file", ROOT / "scripts" / "read_study_file.py")
reader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reader)

HAS_ZSTD = hasattr(zipfile, "ZIP_ZSTANDARD")
needs_zstd = unittest.skipUnless(HAS_ZSTD, "this Python cannot decompress zstd zip entries (needs 3.14+)")


def fixtures() -> list[Path]:
    return sorted(FIXTURES.glob("*.faris"))


def uses_zstd(path: Path) -> bool:
    with zipfile.ZipFile(path) as z:
        return any(i.compress_type == 93 for i in z.infolist())


def load_schema(name: str) -> dict:
    return json.loads((SCHEMAS / name).read_text())


def validate(schema: dict, root: dict, value, at: str = "") -> list[str]:
    """The JSON Schema 2020-12 keywords the published schemas use."""
    if "$ref" in schema:
        node = root
        for part in schema["$ref"][2:].split("/"):
            node = node[part]
        return validate(node, root, value, at)
    out: list[str] = []
    kinds = {"object": dict, "array": list, "string": str, "boolean": bool, "null": type(None)}

    def is_type(name):
        if name == "integer":
            return isinstance(value, int) and not isinstance(value, bool)
        if name == "number":
            return isinstance(value, (int, float)) and not isinstance(value, bool)
        return isinstance(value, kinds[name]) and not (name == "integer" and isinstance(value, bool))

    types = schema.get("type")
    if types is not None and not any(is_type(t) for t in ([types] if isinstance(types, str) else types)):
        out.append(f"{at}: not of type {types}")
    if "const" in schema and value != schema["const"]:
        out.append(f"{at}: not {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        out.append(f"{at}: {value!r} not in {schema['enum']}")
    if "pattern" in schema and isinstance(value, str) and not re.search(schema["pattern"], value):
        out.append(f"{at}: {value!r} does not match {schema['pattern']}")
    if "minimum" in schema and isinstance(value, (int, float)) and value < schema["minimum"]:
        out.append(f"{at}: below {schema['minimum']}")
    if "oneOf" in schema and sum(not validate(s, root, value, at) for s in schema["oneOf"]) != 1:
        out.append(f"{at}: does not match exactly one oneOf option")
    if "not" in schema and not validate(schema["not"], root, value, at):
        out.append(f"{at}: matches a schema it must not match")
    if isinstance(value, dict):
        out += [f"{at}: missing {k}" for k in schema.get("required", []) if k not in value]
        for key, child in value.items():
            if key in schema.get("properties", {}):
                out += validate(schema["properties"][key], root, child, f"{at}/{key}")
            elif schema.get("additionalProperties") is False:
                out.append(f"{at}/{key}: unknown property")
            elif isinstance(schema.get("additionalProperties"), dict):
                out += validate(schema["additionalProperties"], root, child, f"{at}/{key}")
    if isinstance(value, list) and "items" in schema:
        for i, child in enumerate(value):
            out += validate(schema["items"], root, child, f"{at}/{i}")
    return out


def check(name: str, value) -> list[str]:
    schema = load_schema(name)
    return validate(schema, schema, value)


def run(*args) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = reader.main([str(a) for a in args])
    return code, out.getvalue(), err.getvalue()


def rewrite(src: Path, dst: Path, edit, extra=()) -> None:
    """Copies a study file entry by entry, stored; `edit(name, bytes)` returns new bytes or None to drop."""
    with zipfile.ZipFile(src) as source, zipfile.ZipFile(dst, "w", zipfile.ZIP_STORED) as target:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            for info in source.infolist():
                data = edit(info.filename, source.read(info))
                if data is not None:
                    target.writestr(zipfile.ZipInfo(info.filename), data)
            for name, data in extra:
                target.writestr(zipfile.ZipInfo(name), data)


class ArchivedFixtures(unittest.TestCase):
    def test_the_set_is_large_enough(self):
        self.assertGreaterEqual(len(fixtures()), 20)
        for path in fixtures():
            self.assertTrue(path.with_suffix(".summary.json").exists(), path.name)
            self.assertLess(path.stat().st_size, 1_000_000, path.name)

    def test_the_reader_is_short(self):
        lines = (ROOT / "scripts" / "read_study_file.py").read_text().count("\n")
        self.assertLessEqual(lines, 300)

    def test_every_fixture_is_verified_and_agrees_with_the_committed_summary(self):
        ran = 0
        for path in fixtures():
            if uses_zstd(path) and not HAS_ZSTD:
                continue
            with self.subTest(path.name):
                committed = json.loads(path.with_suffix(".summary.json").read_text())
                code, out, err = run(path, "--summary")
                self.assertEqual((code, err), (0, ""), err)
                self.assertEqual(json.loads(out), committed)
                self.assertEqual(check("faris-study-summary-1.schema.json", committed), [])
                ran += 1
        if not ran:
            self.skipTest("no fixture can be read by this Python")

    @needs_zstd
    def test_every_fixture_is_read_by_the_independent_reader(self):
        self.assertTrue(uses_zstd(FIXTURES / "10-both-arrangements-and-sweep-and-assumptions.faris"))
        for path in fixtures():
            with self.subTest(path.name):
                self.assertEqual(run(path)[0], 0)

    @needs_zstd
    def test_documents_inside_the_files_validate_against_the_schemas(self):
        ensembles = stores = 0
        for path in fixtures():
            with self.subTest(path.name):
                study = reader.Study(str(path))
                self.assertEqual(check("faris-study-manifest-1.schema.json", study.manifest), [])
                for record in study.manifest.get("ensembles", []):
                    blob = json.loads(study.verify_blob(record["blob"]))
                    self.assertEqual(check("faris-history-ensemble-1.schema.json", blob), [])
                    ensembles += 1
                if "evidence-store/store.json" in study.by_name:
                    index = json.loads(study.read(study.by_name["evidence-store/store.json"], 1 << 26))
                    self.assertEqual(check("faris-evidence-store-index-1.schema.json", index), [])
                    stores += 1
        self.assertGreaterEqual(ensembles, 4)
        self.assertGreaterEqual(stores, 1)

    def test_the_schemas_reject_bad_documents(self):
        good = reader.Study(str(FIXTURES / "01-hand-built-minimal.faris")).manifest
        self.assertEqual(check("faris-study-manifest-1.schema.json", good), [])
        for change in (
            lambda m: m.update(format="faris-study/2"),
            lambda m: m["blobs"][0].update(sha256="ABC"),
            lambda m: m["blobs"][0].update(encoding="array-f32-le"),
            lambda m: m.update(assumptions="not a digest"),
            lambda m: m.update(layers={"evidence": {"mode": "packed", "archives": []},
                                       "evidence_store": {"mode": "packed", "trees": []}}),
            lambda m: m.update(sweep=[{"name": "../x", "schema_version": "v", "notice": "", "files": {}}]),
        ):
            bad = json.loads(json.dumps(good))
            change(bad)
            self.assertNotEqual(check("faris-study-manifest-1.schema.json", bad), [], bad)

    def test_records_and_blobs_can_be_dumped(self):
        path = FIXTURES / "01-hand-built-minimal.faris"
        code, out, _ = run(path, "--records")
        self.assertEqual((code, json.loads(out)), (0, []))
        sha = json.loads(path.with_suffix(".summary.json").read_text())["blob_sha256"][0]
        stream = io.TextIOWrapper(io.BytesIO(), encoding="utf-8")
        with contextlib.redirect_stdout(stream):
            self.assertEqual(reader.main([str(path), "--blob", sha]), 0)
        self.assertEqual(stream.buffer.getvalue(), b'{"x":1}\n')
        self.assertEqual(run(path, "--blob", "0" * 64)[0], 1)

    @needs_zstd
    def test_records_of_a_full_study(self):
        path = FIXTURES / "10-both-arrangements-and-sweep-and-assumptions.faris"
        records = json.loads(run(path, "--records")[1])
        self.assertEqual(sorted(r["group"] for r in records), ["control", "port", "port", "sweep"])
        self.assertEqual(json.loads(path.with_suffix(".summary.json").read_text())["record_count"], 4)


class FailClosed(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.src = FIXTURES / "01-hand-built-minimal.faris"
        self.sha = json.loads(self.src.with_suffix(".summary.json").read_text())["blob_sha256"][0]

    def refused(self, dst: Path, needle: str):
        code, _, err = run(dst)
        self.assertEqual(code, 1, err)
        self.assertIn("REFUSED", err)
        self.assertIn(needle, err)

    def edit(self, edit, extra=(), src=None) -> Path:
        dst = self.tmp / "x.faris"
        rewrite(src or self.src, dst, edit, extra)
        return dst

    def test_the_unchanged_copy_reads(self):
        self.assertEqual(run(self.edit(lambda n, d: d))[0], 0)

    def test_an_altered_blob_is_named(self):
        dst = self.edit(lambda n, d: b'{"x":2}\n' if n.startswith("blobs/") else d)
        self.refused(dst, self.sha)

    def manifest_edit(self, change):
        def edit(name, data):
            if name != "manifest.json":
                return data
            manifest = json.loads(data)
            change(manifest)
            return json.dumps(manifest).encode()
        return self.edit(edit)

    def test_unknown_major_version_is_refused_and_named(self):
        self.refused(self.manifest_edit(lambda m: m.update(format="faris-study/2")), "faris-study/2")

    def test_a_newer_minor_with_unknown_fields_is_read(self):
        dst = self.manifest_edit(lambda m: m.update(format="faris-study/1.9", future={"a": 1}))
        self.assertEqual(run(dst)[0], 0)

    def test_unknown_encoding_is_refused(self):
        self.refused(self.manifest_edit(lambda m: m["blobs"][0].update(encoding="array-f64-le")), "encoding")

    def test_a_wrong_recorded_size_is_refused(self):
        self.refused(self.manifest_edit(lambda m: m["blobs"][0].update(bytes=3)), "bytes")

    def test_a_reference_to_a_missing_blob_is_refused(self):
        self.refused(self.manifest_edit(lambda m: m.update(assumptions="0" * 64)), "does not hold")

    def test_unexpected_entry_and_unlisted_blob_are_refused(self):
        self.refused(self.edit(lambda n, d: d, [("notes.txt", b"x")]), "unexpected entry")
        self.refused(self.edit(lambda n, d: d, [("blobs/" + "1" * 64, b"x")]), "not listed")

    def test_a_missing_blob_entry_is_refused(self):
        self.refused(self.edit(lambda n, d: None if n.startswith("blobs/") else d), "missing from the file")

    def test_a_duplicate_entry_is_refused(self):
        self.refused(self.edit(lambda n, d: d, [("manifest.json", b"{}")]), "duplicate")

    def test_mimetype_must_be_first_and_right(self):
        self.refused(self.edit(lambda n, d: b"text/plain" if n == "mimetype" else d), "mimetype")
        dst = self.tmp / "order.faris"
        with zipfile.ZipFile(self.src) as source, zipfile.ZipFile(dst, "w", zipfile.ZIP_STORED) as target:
            for info in reversed(source.infolist()):
                target.writestr(zipfile.ZipInfo(info.filename), source.read(info))
        self.refused(dst, "first entry")

    def test_not_a_zip_is_refused(self):
        junk = self.tmp / "junk.faris"
        junk.write_bytes(b"not a zip file")
        self.refused(junk, "zip")

    @needs_zstd
    def test_a_damaged_evidence_store_blob_is_refused(self):
        src = FIXTURES / "21-evidence-store-packed.faris"
        names = [n for n in zipfile.ZipFile(src).namelist() if n.endswith(".xz")]
        dst = self.edit(lambda n, d: d + b"x" if n == names[0] else d, src=src)
        self.refused(dst, "store blob")
        dst = self.edit(lambda n, d: None if n == names[0] else d, src=src)
        self.refused(dst, "evidence store")

    @needs_zstd
    def test_store_entries_without_a_packed_layer_are_refused(self):
        src = FIXTURES / "20-evidence-store-referenced.faris"
        dst = self.edit(lambda n, d: d, [("evidence-store/store.json", b"{}")], src=src)
        self.refused(dst, "evidence store")


class WithoutZstd(unittest.TestCase):
    """A Python that cannot decompress zstd says so (exit 2) instead of failing obscurely."""

    def test_the_reader_names_the_problem_and_the_fix(self):
        path = FIXTURES / "10-both-arrangements-and-sweep-and-assumptions.faris"
        if not uses_zstd(path):
            self.skipTest("this fixture holds no zstd entry")
        had = hasattr(zipfile, "ZIP_ZSTANDARD")
        saved = getattr(zipfile, "ZIP_ZSTANDARD", None)
        if had:
            del zipfile.ZIP_ZSTANDARD
        try:
            code, _, err = run(path)
        finally:
            if had:
                zipfile.ZIP_ZSTANDARD = saved
        self.assertEqual(code, 2)
        self.assertIn("Python 3.14", err)


if __name__ == "__main__":
    unittest.main()
