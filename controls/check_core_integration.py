#!/usr/bin/env python3
"""Exercise the real Core executable, with its own primary semantic fixtures.

This checks software interpretation and FARIS declaration failures. It does not
qualify nuclear data or turn fixture verdicts into reactor verdicts. The Core
checkout supplies the fixtures; no external fixture text is redistributed here.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def digest(path):
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def invoke(core, arguments):
    process = subprocess.run([str(core), *map(str, arguments)], capture_output=True,
                             text=True, timeout=30, check=False)
    if len(process.stdout) + len(process.stderr) > 4 * 1024 * 1024:
        raise RuntimeError("Core control log exceeded its bound")
    report = json.loads(process.stdout)
    if process.returncode not in (0, 1):
        raise RuntimeError(f"Core execution failed: {process.stderr}")
    return {"exit_code": process.returncode, "report": report,
            "stderr": process.stderr}


def run(core, source, case):
    fixture_root = source / "fixtures/semantic-core/campaigns"
    index_path = fixture_root / "campaign-cases.v1.json"
    index = json.loads(index_path.read_text())
    names = {
        "campaign.le.within.pass", "campaign.le.exceeds.fail",
        "campaign.le.crossing.inconclusive", "campaign.parent-missing.not_evaluated",
        "campaign.snapshot-mismatch.rejected",
        "campaign.model-mismatch.quarantine", "campaign.inverted-bounds.quarantine",
        "campaign.require-qualification.unqualified.not_evaluated",
    }
    records = []
    for fixture in index["fixtures"]:
        if fixture["fixture_id"] not in names:
            continue
        paths = {name: (fixture_root / fixture[name]).resolve()
                 for name in ("contract", "registry", "claims")}
        result = invoke(core, ["evaluate", "--contract", paths["contract"],
                              "--registry", paths["registry"], "--claims", paths["claims"]])
        expected = fixture["expected"]
        report = result["report"]
        if report["status"] != expected["status"]:
            raise AssertionError(f"{fixture['fixture_id']}: wrong campaign status")
        actual_verdicts = {v["requirement_id"]: v["verdict"]["status"]
                           for v in report.get("verdicts", [])}
        expected_verdicts = {v["requirement_id"]: v["status"]
                             for v in expected.get("verdicts", [])}
        if actual_verdicts != expected_verdicts:
            raise AssertionError(f"{fixture['fixture_id']}: wrong verdict or omitted state")
        records.append({"test": fixture["fixture_id"], "scope": "Core software fixture",
                        "inputs": {k: digest(v) for k, v in paths.items()}, **result})
    if {r["test"] for r in records} != names:
        raise AssertionError("Pinned Core fixture set is incomplete")
    observed = {v["verdict"]["status"] for r in records
                for v in r["report"].get("verdicts", [])}
    if not {"pass", "fail", "inconclusive", "not_evaluated"} <= observed:
        raise AssertionError("Did not observe all four genuine Core states")

    if case is not None:
        contract = json.loads((case / "contract.json").read_text())
        registry = case / "registry.json"
        mutations = [
            ("wrong-unit", lambda c: c["requirements"][0]["limit"].update(unit="MW")),
            ("overlapping-binding", lambda c: c["workflow"][1]["bindings"].append(
                json.loads(json.dumps(c["workflow"][1]["bindings"][-1])))),
            ("missing-bound-input", lambda c: c["workflow"][1]["bindings"][-1]["source"].update(input_id="missing-physics")),
        ]
        with tempfile.TemporaryDirectory(prefix="faris-core-controls-") as temporary:
            for name, mutate in mutations:
                changed = json.loads(json.dumps(contract))
                mutate(changed)
                path = Path(temporary) / f"{name}.json"
                path.write_text(json.dumps(changed, indent=2) + "\n")
                result = invoke(core, ["compile", "--contract", path, "--registry", registry])
                report = result["report"]
                if report["status"] != "rejected" or not report.get("findings"):
                    raise AssertionError(f"{name}: Core did not reject with actionable findings")
                records.append({"test": name, "scope": "FARIS declaration mutation",
                                "contract_sha256": digest(path), "registry_sha256": digest(registry), **result})
    return {"schema_version": "faris-core-integration-controls/v0.1",
            "scope": "Software semantic controls only; no physical qualification",
            "core_executable_sha256": digest(core), "core_fixture_index_sha256": digest(index_path),
            "tests": records, "passed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core", type=Path, required=True)
    parser.add_argument("--core-source", type=Path, required=True)
    parser.add_argument("--case", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = run(args.core.resolve(), args.core_source.resolve(), args.case)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as file:
        json.dump(result, file, indent=2)
        file.write("\n")
    print(f"Passed {len(result['tests'])} actual Core controls; {args.output}")


if __name__ == "__main__":
    main()
