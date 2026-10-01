import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("verify_recorded_demo.py")
SPEC = importlib.util.spec_from_file_location("verify_recorded_demo", SCRIPT)
VERIFY = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(VERIFY)
sys.path.insert(0, str(SCRIPT.parent))
from port_geometry_contract import validate_ownership_audits


def write(path: Path, value: bytes | str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value.encode() if isinstance(value, str) else value)


def make_package(root: Path, faris: Path, core: Path) -> None:
    pairs = []
    for pair in ("control", "port"):
        scenario = root / pair / "scenario.json"
        scenario_data = {"id": pair, "pair": pair}
        if pair == "port":
            scenario_data["penetration"] = {"kind": "outboard_rectangular_prism"}
        write(scenario, json.dumps(scenario_data) + "\n")
        scenario_sha = VERIFY.digest(scenario).removeprefix("sha256:")
        arrangements = []
        for variant in ("reference", "breeder-emphasis"):
            case = root / pair / "cases" / variant
            workspace = root / pair / "core-workspaces" / variant
            report = case / "execution-report.json"
            write(case / "case.marker", "case material\n")
            write(workspace / "receipt.json", "receipt material\n")
            write(report, "report material\n")
            descriptor_rel = f"saved-study-{pair}-{variant}.json"
            descriptor = {"case_directory": str(case.relative_to(root)),
                          "execution_report": str(report.relative_to(root)),
                          "execution_workspace": str(workspace.relative_to(root))}
            descriptor_path = root / descriptor_rel
            write(descriptor_path, json.dumps(descriptor, indent=2) + "\n")
            inspection = {
                "schema_version": "faris-saved-case-inspection/v0.2",
                "record_integrity": "UNSIGNED_IDENTITY_REVALIDATED",
                "scenario_sha256": f"sha256:{scenario_sha}",
                "variant_id": variant,
                "case_id": f"{pair}-{variant}-case",
                "execution_status": "executed",
                "binding_status": "verified",
                "compiler_id": "avila.core/compiler-rust@0.1.0",
                "semantic_profile": "avila.core/semantic/0.2-draft",
                "compiler_executable_sha256": "sha256:" + "b" * 64,
                "core_executable_sha256": "sha256:" + "b" * 64,
                "requirement_verdicts": [{"status": "not_evaluated"}],
                "steps": [{"step_id": "transport"}],
                "verified_receipt_count": 1,
            }
            inspection_rel = f"{pair}/inspections/{variant}.json"
            write(root / inspection_rel, json.dumps(inspection, indent=2, sort_keys=True) + "\n")
            run_record_hash = "sha256:" + "a" * 64
            arrangement = {
                "variant_id": variant,
                "core_execution_report": str(report.relative_to(root)),
                "run_record_sha256": run_record_hash,
                "scientific_qualification": "NOT_EVALUATED",
                "core_requirement_verdicts": ["not_evaluated"],
                "saved_study_descriptor": descriptor_rel,
                "saved_study_descriptor_sha256": VERIFY.digest(descriptor_path),
                "saved_case_inspection": inspection_rel,
                "saved_case_inspection_sha256": VERIFY.digest(root / inspection_rel),
            }
            if pair == "port":
                scenario_sha_raw = scenario_sha
                input_sha = "c" * 64
                worker_sha = "sha256:" + "d" * 64
                artifact_sha = "e" * 64
                run_record_sha = "sha256:" + "a" * 64
                volume = {"schema_version": "faris-independent-port-volume-check/v0.1",
                          "geometry_check": "PASS", "transport_volume_check": "PASS",
                          "scientific_qualification": "NOT_EVALUATED", "variant_id": variant,
                          "scenario_sha256": scenario_sha,
                          "run_record_sha256": run_record_hash.removeprefix("sha256:"),
                          "transport_artifact_sha256": artifact_sha,
                          "worker_result_sha256": worker_sha.removeprefix("sha256:"),
                          "input_sha256": input_sha}
                volume_rel = f"{pair}/geometry/{variant}-volume-check.json"
                write(root / volume_rel, json.dumps(volume) + "\n")
                arrangement["port_volume_report"] = {
                    "path": volume_rel, "sha256": VERIFY.digest(root / volume_rel)}
                phis = [1.5707963267948966, 3.141592653589793, 4.71238898038469]
                thetas = [0.0, 1.5707963267948966, 3.141592653589793, 4.71238898038469]
                probes = []
                def add_probe(probe_id, cell, material):
                    probes.append({
                        "probe_id": probe_id, "status": "PASS",
                        "expected_cell_name": cell, "observed_cell_name": cell,
                        "expected_cell_id": 1 if cell == "plasma-source-domain" else 2,
                        "observed_cell_id": 1 if cell == "plasma-source-domain" else 2,
                        "expected_material_id": material,
                        "expected_openmc_material_name": None if material == "void" else material,
                        "observed_openmc_material_name": None if material == "void" else material,
                        "expected_openmc_material_id": None if material == "void" else 1,
                        "observed_openmc_material_id": None if material == "void" else 1,
                        "point_xyz_cm": [1.0, 2.0, 3.0],
                    })
                for phi in phis:
                    p = f"{phi:.8f}"
                    add_probe(f"plasma-interior-phi-{p}", "plasma-source-domain", "void")
                    add_probe(f"clearance-near-plasma-phi-{p}", "plasma-first-wall-clearance", "void")
                    add_probe(f"clearance-near-first-wall-phi-{p}", "plasma-first-wall-clearance", "void")
                    add_probe(f"first-wall-near-inner-phi-{p}", "first-wall", "tungsten-natural")
                    add_probe(f"first-wall-near-outer-phi-{p}", "first-wall", "tungsten-natural")
                    for theta in thetas:
                        add_probe(f"first-wall-mid-phi-{p}-theta-{theta:.8f}",
                                  "first-wall", "tungsten-natural")
                ownership_audit = {
                    "schema_version": "faris-openmc-geometry-ownership-audit/v0.1",
                    "status": "PASS", "checks_are_geometry_only": True,
                    "scientific_qualification": "NOT_EVALUATED",
                    "method": "OpenMC Geometry.find actual cell/material ownership at analytic probes; zero transport histories",
                    "scenario_sha256": scenario_sha_raw, "variant_id": variant,
                    "input_sha256": input_sha, "plasma_radius_m": 1.0,
                    "declared_plasma_to_first_wall_clearance_m": 0.08,
                    "first_wall_inner_radius_m": 1.08, "clearance_status": "PASS",
                    "toroidal_probe_directions_rad": phis,
                    "cross_section_probe_directions_rad": thetas,
                    "probe_count": len(probes), "failed_probe_count": 0,
                    "probes": probes,
                }
                penetration = {
                    "scenario_sha256": scenario_sha_raw, "variant_id": variant,
                    "cell_counts": {"first-wall": 2},
                    "final_port_void_confirmation_counts_by_component": {"first-wall": 2},
                    "not_a_physical_validation": True,
                    "fractional_volume_standard_errors_are_binomial": True,
                    "independent_of_Rust_midpoint_quadrature": True,
                    "samples": 100,
                }
                own_rel = f"{pair}/geometry/{variant}-ownership-audit.json"
                ownership = {
                    "schema_version": "faris-packaged-port-geometry-ownership/v0.1",
                    "worker_result_sha256": worker_sha,
                    "run_record_sha256": run_record_hash,
                    "transport_artifact_sha256": artifact_sha,
                    "input_sha256": input_sha,
                    "scenario_sha256": scenario_sha_raw,
                    "variant_id": variant,
                    "component_materials": {"first-wall": "tungsten-natural"},
                    "geometry_ownership_audit": ownership_audit,
                    "penetration_volume_audit": penetration,
                }
                write(root / own_rel, json.dumps(ownership) + "\n")
                arrangement["port_geometry_ownership_report"] = {
                    "path": own_rel, "sha256": VERIFY.digest(root / own_rel),
                    "worker_result_sha256": worker_sha}
            else:
                arrangement["port_volume_report"] = None
                arrangement["port_geometry_ownership_report"] = None
            arrangements.append(arrangement)
        pairs.append({"scenario_id": pair, "scenario_path": f"{pair}/scenario.json",
                      "scenario_sha256": scenario_sha,
                      "feature": "finite_port" if pair == "port" else "feature_free_control",
                      "arrangements": arrangements})
    index = {"schema_version": "faris-recorded-demo-package/v0.3",
             "status": "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED",
             "faris_cli_sha256": VERIFY.digest(faris),
             "core_executable_sha256": VERIFY.digest(core),
             "scenario_pairs": pairs}
    inventory = []
    for path in sorted(root.rglob("*")):
        if path.is_file():
            inventory.append({"path": path.relative_to(root).as_posix(),
                              "bytes": path.stat().st_size, "sha256": VERIFY.digest(path)})
    index["files"] = inventory
    index_path = root / "package-index.json"
    write(index_path, json.dumps(index, indent=2) + "\n")
    write(root / "package-index.sha256", f"{VERIFY.digest(index_path)}  package-index.json\n")


class RecordedDemoPackageVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.package = self.root / "package"
        self.package.mkdir()
        self.faris = self.root / "faris-test"
        write(self.faris, "#!/usr/bin/env python3\nimport hashlib,json,sys\nfrom pathlib import Path\ncase=Path(sys.argv[sys.argv.index('--case')+1])\npair=case.parents[1].name\nscenario=case.parents[1]/'scenario.json'\nh=hashlib.sha256(scenario.read_bytes()).hexdigest()\nvariant=case.name\nprint(json.dumps({'schema_version':'faris-saved-case-inspection/v0.2','record_integrity':'UNSIGNED_IDENTITY_REVALIDATED','scenario_sha256':'sha256:'+h,'variant_id':variant,'case_id':pair+'-'+variant+'-case','execution_status':'executed','binding_status':'verified','compiler_id':'avila.core/compiler-rust@0.1.0','semantic_profile':'avila.core/semantic/0.2-draft','compiler_executable_sha256':'sha256:'+'b'*64,'core_executable_sha256':'sha256:'+'b'*64,'requirement_verdicts':[{'status':'not_evaluated'}],'steps':[{'step_id':'transport'}],'verified_receipt_count':1}))\n")
        self.faris.chmod(0o755)
        self.core = self.root / "core-test"
        write(self.core, "test core pin\n")
        make_package(self.package, self.faris, self.core)

    def tearDown(self):
        self.temporary.cleanup()

    def test_relocated_copy_rehashes_and_reopens_four_cases(self):
        relocated = self.root / "relocated" / "demo"
        shutil.copytree(self.package, relocated)
        result = VERIFY.verify_package(relocated, self.faris, self.core)
        self.assertEqual(result["inspected_saved_case_count"], 4)

    def test_tampered_copy_is_rejected_without_changing_source(self):
        original_hash = VERIFY.digest(self.package / "package-index.json")
        original_file = VERIFY.digest(self.package / "control" / "scenario.json")
        result = VERIFY.mutate_copy_for_negative_control(self.package, self.faris, self.core)
        self.assertEqual(result["tamper_control"], "EXPECTED_REJECTION")
        self.assertTrue(result["original_preserved"])
        self.assertEqual(VERIFY.digest(self.package / "package-index.json"), original_hash)
        self.assertEqual(VERIFY.digest(self.package / "control" / "scenario.json"), original_file)

    def test_cli_relocates_reopens_and_runs_tamper_negative_control(self):
        relocated = self.root / "relocated-cli" / "demo"
        original = VERIFY.digest(self.package / "control" / "scenario.json")
        result = subprocess.run(
            ["python3", str(SCRIPT), "--package", str(self.package), "--faris", str(self.faris),
             "--core", str(self.core), "--relocated-copy", str(relocated)],
            text=True, capture_output=True, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        self.assertEqual(output["inspected_saved_case_count"], 4)
        self.assertEqual(output["tamper_control"], "EXPECTED_REJECTION")
        self.assertTrue(relocated.is_dir())
        self.assertEqual(VERIFY.digest(self.package / "control" / "scenario.json"), original)

    def test_index_rejects_extra_unindexed_file_and_path_traversal(self):
        extra = self.package / "unexpected.txt"
        write(extra, "unindexed\n")
        with self.assertRaises(ValueError):
            VERIFY.verify_package(self.package, self.faris, self.core)
        with self.assertRaises(ValueError):
            VERIFY.safe_package_path(self.package.resolve(), "../outside")

    def test_port_geometry_contract_rejects_material_in_clearance(self):
        ownership_path = self.package / "port" / "geometry" / "reference-ownership-audit.json"
        ownership = json.loads(ownership_path.read_text())
        kwargs = {
            "scenario_sha256": ownership["scenario_sha256"],
            "variant_id": ownership["variant_id"],
            "input_sha256": ownership["input_sha256"],
            "component_materials": ownership["component_materials"],
        }
        validate_ownership_audits(ownership["geometry_ownership_audit"],
                                  ownership["penetration_volume_audit"], **kwargs)
        clearance = next(probe for probe in ownership["geometry_ownership_audit"]["probes"]
                         if probe["probe_id"].startswith("clearance-near-plasma-"))
        clearance["observed_openmc_material_name"] = "tungsten-natural"
        clearance["observed_openmc_material_id"] = 1
        with self.assertRaises(ValueError):
            validate_ownership_audits(ownership["geometry_ownership_audit"],
                                      ownership["penetration_volume_audit"], **kwargs)


if __name__ == "__main__":
    unittest.main()
