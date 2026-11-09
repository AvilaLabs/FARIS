from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("compare.py")
SPEC = importlib.util.spec_from_file_location("lifecycle_peer_compare", MODULE_PATH)
compare = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(compare)


def write(path: Path, content: str | bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content.encode() if isinstance(content, str) else content)
    return path


def pinned(path: Path) -> dict[str, str]:
    return {"path": path.name, "sha256": compare.sha256(path)}


def source(kind="authored", reference="case assumptions"):
    return {"kind": kind, "reference": reference}


def quantity(value, unit, kind="authored"):
    return {"value": value, "unit": unit, "source": source(kind)}


def mapping_entry(disposition="matched", native_setting="native input x", note="matched as configured"):
    return {"disposition": disposition, "native_setting": native_setting, "note": note}


def mapping():
    groups = {name: mapping_entry("unmapped", None, "No native mapping is declared in this test fixture.") for name in
              ("planned_replacement", "pulse_ramp_and_recharge", "unplanned", "other")}
    unmap = lambda why: {"disposition": "unmapped", "native_field": None, "value": None, "unit": None, "note": why}
    native = {
        "plant_horizon_years": unmap("No native horizon mapping declared."),
        "net_electric_power_mw": unmap("No native net power mapping declared."),
        "parallel_remote_handling_systems": unmap("No native parallel RH mapping declared."),
        "component_lifetimes_years": [{"component": "blanket", "lifetime": unmap("No mapped lifetime field."),
                                       "replacement_duration": unmap("No mapped replacement field.")}],
    }
    return {"process": {"native_inputs": native, "downtime": dict(groups)},
            "bluemira": {"native_inputs": json.loads(json.dumps(native)), "downtime": dict(groups)}}


def manifest_fixture(root: Path) -> Path:
    faris_fixed = write(root / "fixed.json", '{"engine":"native"}\n')
    faris_computed = write(root / "computed.json", '{"engine":"native"}\n')
    process_input = write(root / "tokamak_IN.DAT", "* reviewed PROCESS input\n")
    blue_config = write(root / "bluemira-config.json", json.dumps({
        "lifecycle_config": {"A_global": 0.3}, "learning_strategy": {"kind": "uniform", "parameters": {}},
        "availability_strategy": {"kind": "lognormal", "parameters": {"sigma": 1.0}},
        "inputs": {}, "provenance": {"A_global": {"unit": "dimensionless", "kind": "authored", "reference": "fixture"}},
    }))
    manifest = {
        "schema": compare.SCHEMA,
        "comparison_id": "unit-test-only",
        "matched_inputs": {
            "plant_horizon_years": quantity(30, "year"),
            "net_electric_power_mw": quantity(500, "MW(e)"),
            "component_lifetimes_years": [{"component": "blanket", "lifetime": quantity(3, "year"),
                                             "replacement_duration_days": quantity(45, "day"), "source": source()}],
            "parallel_remote_handling_systems": quantity(2, "count"),
            "assumptions": [{"name": "duty_cycle", "value": "continuous", "unit": "text", "source": source()}],
        },
        "mapping": mapping(),
        "faris": {"fixed": dict(pinned(faris_fixed), scope="authoritative fixed-duration output"),
                  "computed": dict(pinned(faris_computed), scope="authoritative computed-duration output")},
        "peers": {
            "process": {"version": "3.4.2", "executable": "process", "input": pinned(process_input), "python": sys.executable},
            "bluemira": {"version": "2.14.0", "python": sys.executable, "config": pinned(blue_config), "rng_seed": 1234},
        },
        "limits": {"timeout_seconds": 120, "output_bytes": 1_000_000},
    }
    return write(root / "manifest.json", json.dumps(manifest, indent=2))


class PeerLifecycleManifestTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def test_manifest_hash_pins_each_input_and_keeps_faris_as_authority(self):
        path = manifest_fixture(self.root)
        manifest, files, limitations = compare.validate_manifest(path)
        self.assertIn("process.input", files)
        self.assertIn("bluemira.config", files)
        self.assertTrue(any("process" in item for item in limitations))
        self.assertEqual(manifest["schema"], compare.SCHEMA)

    def test_missing_provenance_and_omitted_downtime_mapping_fail(self):
        path = manifest_fixture(self.root)
        artifact = json.loads(path.read_text())
        artifact["matched_inputs"]["net_electric_power_mw"].pop("source")
        write(path, json.dumps(artifact))
        with self.assertRaisesRegex(ValueError, "value, unit and source"):
            compare.validate_manifest(path)

        artifact["matched_inputs"]["net_electric_power_mw"] = quantity(500, "MW(e)")
        artifact["mapping"]["process"]["downtime"].pop("other")
        write(path, json.dumps(artifact))
        with self.assertRaisesRegex(ValueError, "must explicitly map all downtime categories"):
            compare.validate_manifest(path)

    def test_unmapped_domains_are_reported_instead_of_assumed_zero(self):
        path = manifest_fixture(self.root)
        artifact = json.loads(path.read_text())
        artifact["mapping"]["bluemira"]["downtime"]["other"] = mapping_entry(
            "unmapped", None, "LifeCycle has no mapped representation for this downtime category."
        )
        write(path, json.dumps(artifact))
        _, _, limitations = compare.validate_manifest(path)
        self.assertTrue(any("bluemira.other" in item for item in limitations))

    def test_hash_mismatch_fails_before_any_peer_launch(self):
        path = manifest_fixture(self.root)
        artifact = json.loads(path.read_text())
        artifact["peers"]["process"]["input"]["sha256"] = "0" * 64
        write(path, json.dumps(artifact))
        with self.assertRaisesRegex(ValueError, "PROCESS input SHA-256 mismatch"):
            compare.validate_manifest(path)

    def test_native_input_mapping_is_checked_against_the_actual_input_file(self):
        path = manifest_fixture(self.root)
        artifact = json.loads(path.read_text())
        input_path = self.root / "tokamak_IN.DAT"
        write(input_path, "life_plant = 30.0 * plant horizon [year]\n")
        artifact["peers"]["process"]["input"] = pinned(input_path)
        artifact["mapping"]["process"]["native_inputs"]["plant_horizon_years"] = {
            "disposition": "matched", "native_field": "life_plant", "value": 30,
            "unit": "year", "note": "The reviewer verified the PROCESS input variable mapping.",
        }
        write(path, json.dumps(artifact))
        compare.validate_manifest(path)

        write(input_path, "life_plant = 29.0 * plant horizon [year]\n")
        artifact["peers"]["process"]["input"] = pinned(input_path)
        write(path, json.dumps(artifact))
        with self.assertRaisesRegex(ValueError, "does not match PROCESS IN.DAT"):
            compare.validate_manifest(path)

    def test_conditional_energy_normalization_uses_native_metric_basis(self):
        path = manifest_fixture(self.root)
        manifest, _, _ = compare.validate_manifest(path)
        result = compare.conditional_metrics(
            manifest,
            {"plant_availability": 0.5, "capacity_factor": 0.4, "plant_lifetime_years": 30.0},
            {"full_power_years": 12.0, "calendar_years": 30.0},
        )
        self.assertEqual(result["comparison_status"], "incomplete")
        self.assertAlmostEqual(result["PROCESS"]["net_electric_energy_mwh_e"], 500 * 0.4 * 30 * 8760)
        self.assertAlmostEqual(result["bluemira"]["net_electric_energy_mwh_e"], 500 * 12 * 8760)
        self.assertFalse(result["availability_comparison_allowed"])

    def test_peer_runner_does_not_overwrite_existing_output(self):
        output = self.root / "already-used"
        output.mkdir()
        write(output / "keep", "untouched")
        path = manifest_fixture(self.root)
        with self.assertRaisesRegex(ValueError, "not empty"):
            compare.run_manifest(path, output)

    def test_peer_output_limit_is_enforced_during_capture(self):
        script = write(self.root / "noisy.py", "print('x' * 100000)\n")
        with self.assertRaisesRegex(ValueError, "output limit"):
            compare.bounded_run([sys.executable, str(script)], cwd=self.root, timeout=5, output_bytes=10_000)


if __name__ == "__main__":
    unittest.main()
