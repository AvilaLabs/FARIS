# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Every adapter declares itself under the common contract (requirement AUTO-060)."""
import copy
import json
import re
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCHEMA = REPO / "docs" / "schemas" / "faris-adapter-1.schema.json"
DOC = REPO / "docs" / "ADAPTER_CONTRACT.md"
DECLARATIONS = REPO / "integrations" / "adapters"


def validate(value, schema, path="$"):
    """The keywords the adapter schema uses; any other keyword is an error, so the schema cannot outgrow this."""
    known = {"$schema", "$id", "title", "description", "type", "properties", "required", "additionalProperties",
             "items", "minItems", "minLength", "enum", "const", "uniqueItems"}
    unknown = set(schema) - known
    if unknown:
        raise AssertionError(f"{path}: schema keyword {sorted(unknown)} is not supported by this validator")
    problems = []
    if "const" in schema and value != schema["const"]:
        problems.append(f"{path}: not {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        problems.append(f"{path}: {value!r} is not one of {schema['enum']}")
    kind = schema.get("type")
    python = {"object": dict, "array": list, "string": str}.get(kind)
    if python and not isinstance(value, python):
        return problems + [f"{path}: not a {kind}"]
    if isinstance(value, str) and len(value) < schema.get("minLength", 0):
        problems.append(f"{path}: too short")
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                problems.append(f"{path}: missing {key}")
        props = schema.get("properties", {})
        for key, sub in value.items():
            if key in props:
                problems += validate(sub, props[key], f"{path}.{key}")
            elif schema.get("additionalProperties") is False:
                problems.append(f"{path}: unexpected {key}")
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            problems.append(f"{path}: fewer than {schema['minItems']} items")
        if schema.get("uniqueItems") and len(set(map(json.dumps, value))) != len(value):
            problems.append(f"{path}: repeated items")
        for i, item in enumerate(value):
            problems += validate(item, schema.get("items", {}), f"{path}[{i}]")
    return problems


def declarations():
    return {p.stem: json.loads(p.read_text(encoding="utf-8")) for p in sorted(DECLARATIONS.glob("*.json"))}


class AdapterContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
        cls.doc = DOC.read_text(encoding="utf-8")

    # Verifies: AUTO-060
    def test_every_declaration_validates_against_the_schema(self):
        found = declarations()
        self.assertTrue(found)
        for name, declaration in found.items():
            self.assertEqual(validate(declaration, self.schema), [], name)
            self.assertEqual(declaration["name"], name)

    # Verifies: AUTO-060
    def test_every_tool_the_cli_lists_has_a_declaration_and_no_other_does(self):
        source = (REPO / "crates" / "faris-cli" / "src" / "adapters.rs").read_text(encoding="utf-8")
        tools = re.search(r"pub const TOOLS: \[&str; \d+\] = \[([^\]]*)\]", source).group(1)
        self.assertEqual(sorted(re.findall(r'"([^"]+)"', tools)), sorted(declarations()))

    # Verifies: AUTO-060
    def test_declared_entry_points_exist_and_the_contract_version_is_in_the_document(self):
        for name, declaration in declarations().items():
            for path in declaration["entry_points"]:
                self.assertTrue((REPO / path).exists(), f"{name}: {path}")
            self.assertIn(declaration["contract"], self.doc)

    # Verifies: AUTO-060
    def test_the_document_covers_every_required_part_of_the_contract(self):
        for part in ("Inputs", "Outputs and units", "Receipts", "Cancellation", "Failure modes", "Versions"):
            self.assertIn(f"**{part}.**", self.doc)
        for field in self.schema["required"]:
            self.assertIn(f"`{field}`", self.doc)

    # Verifies: AUTO-060
    def test_the_validator_rejects_a_declaration_with_a_part_missing_or_wrong(self):
        good = declarations()["openmc"]
        for change in (lambda d: d.pop("cancellation"), lambda d: d.update(contract="faris-adapter-contract/v2"),
                       lambda d: d["tool_versions"].update(check="ignores"), lambda d: d["outputs"][0].update(label="measured"),
                       lambda d: d["receipt"].append("tool_sha256"), lambda d: d.update(extra=1), lambda d: d.update(failures=[])):
            broken = copy.deepcopy(good)
            change(broken)
            self.assertTrue(validate(broken, self.schema))


if __name__ == "__main__":
    unittest.main()
