#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Write the manifests of the OpenMC-Fusion-Benchmarks cases that wait for an upstream specification.

    python3 build_awaiting_manifests.py --root DIR --step-dir DIR

--root holds a clean checkout `openmc_fusion_benchmarks/` at the pinned commit (with its `lfs` submodule);
--step-dir holds the Git LFS objects of the three FNG streaming STEP files, each checked against the object
id in its pointer. A case here has no scored detector: every detector is NOT_EVALUATED with the reason and the
next step, and the manifest's `registration` block says exactly what is missing. Nothing is invented.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from validation.manifest import SCHEMA, check_manifest  # noqa: E402

OFB = "openmc_fusion_benchmarks"
OFB_URL = "https://github.com/eepeterson/openmc_fusion_benchmarks"
LEGACY_COMMIT = "c47fc573a7e2a9ca11a8958dc67155b1146adf51"
LEGACY_NOTE = (
    f"The older master branch of the same repository (commit {LEGACY_COMMIT}, layout before 2025-03-28) holds a CSG OpenMC model and an experiment.h5 for this experiment; "
    "develop deleted both (2025-03-28 and 2025-12-01) and the repository does not say where the measured values come from or under which terms they may be redistributed "
    "(the MIT licence of the repository covers its authors' own work, not third-party data). They are not registered here and no value from them is used.")
MIT_ATTRIBUTION = ("OpenMC-Fusion-Benchmarks, Copyright (c) 2025 MIT PSFC, and openmc_fusion_benchmarks-lfs, Copyright (c) 2025 Stefano Segantin, both MIT licence "
                   "(the permission notice is in the LICENSE files named under files). Only file hashes are recorded; no OFB content is copied.")

COMMON_MISSING = [
    "a benchmark specification in src/openmc_fusion_benchmarks/benchmarks/ (materials, source, tallies, settings): develop defines only oktavian_al, and benchmark_list.txt lists this experiment as planned",
    "the measured values with their uncertainties in a form FARIS can hash and cite (develop's results database has only oktavian_al)",
    "the licence under which the measured values may be redistributed",
]

CASES = {
    "fng-streaming-onaxis": {
        "title": "FNG ITER shield streaming mock-up, on-axis foil reaction rates (awaiting upstream specification)",
        "step": "lfs/benchmarks/fng_str_onaxis/fng_str_onaxis.step",
        "benchmark_list_item": "3. Frascati Neutron Generator (FNG) streaming benchmark",
        "classes": {"activation_foil_reaction_rate": ("reactions per atom per source neutron", "activation-foil reaction rates along the streaming channel axis (foil types and positions not registered)")},
        "extra_missing": ["the foil positions and materials, the source (FNG D-T neutron source energy and angular characteristics), and the shield material compositions, none of which are in the STEP file"],
        "data_terms": "The FNG experimental values are published in SINBAD (NEA licence: redistribution to third parties forbidden, outputs derived from the data restricted; docs/NEXT_RELEASE_PLAN.md, open decision 5). Whether OFB redistributes them under MIT is not stated; develop contains none.",
        "why_priority": "VAL-021 asks for the FNG streaming and magnet mock-up experiments; the STEP file is the only open CAD geometry of a streaming path with measured foil data.",
        "reference": "Segantin, Ebiwonjumi, Peterson, Fusion Science and Technology 81 (2024) 437-447, doi 10.1080/15361055.2024.2400762",
    },
    "fng-streaming-offaxis": {
        "title": "FNG ITER shield streaming mock-up, off-axis foil reaction rates (awaiting upstream specification)",
        "step": "lfs/benchmarks/fng_str_offaxis/fng_str_offaxis.step",
        "benchmark_list_item": "3. Frascati Neutron Generator (FNG) streaming benchmark",
        "classes": {"activation_foil_reaction_rate": ("reactions per atom per source neutron", "activation-foil reaction rates behind the shield off the channel axis (foil types and positions not registered)")},
        "extra_missing": ["the foil positions and materials, the FNG source, and the shield material compositions, none of which are in the STEP file"],
        "data_terms": "As fng-streaming-onaxis: SINBAD terms apply to the original values; OFB's redistribution terms are not stated.",
        "why_priority": "VAL-021, and the off-axis foils test the shield outside the line of sight of the channel.",
        "reference": "Segantin, Ebiwonjumi, Peterson, Fusion Science and Technology 81 (2024) 437-447, doi 10.1080/15361055.2024.2400762",
    },
    "fng-streaming-heating": {
        "title": "FNG ITER shield streaming mock-up, nuclear heating (awaiting upstream specification)",
        "step": "lfs/benchmarks/fng_str_heating/fng_str_heating.step",
        "benchmark_list_item": "3. Frascati Neutron Generator (FNG) streaming benchmark",
        "classes": {"nuclear_heating": ("not confirmed: no specification", "nuclear heating measured along the shield depth (detector type, positions and unit not registered)")},
        "extra_missing": ["the detector type, positions and response, the FNG source, and the shield compositions, none of which are in the STEP file"],
        "data_terms": "As fng-streaming-onaxis: SINBAD terms apply to the original values; OFB's redistribution terms are not stated.",
        "why_priority": "VAL-021, and nuclear heating is a quantity with no benchmark in the V&V matrix today.",
        "reference": "Segantin, Ebiwonjumi, Peterson, Fusion Science and Technology 81 (2024) 437-447, doi 10.1080/15361055.2024.2400762",
    },
    "fns-dogleg-duct": {
        "title": "JAEA FNS dogleg duct streaming experiment (awaiting upstream specification and geometry)",
        "step": None,
        "benchmark_list_item": "2. FNS dogleg duct streaming benchmark",
        "classes": {"neutron_spectrum_in_duct": ("neutrons per source neutron per unit lethargy", "neutron spectra measured inside the duct (detector positions not registered)"),
                    "activation_foil_reaction_rate": ("reactions per atom per source neutron", "dosimetry reaction rates along the duct (foil types and positions not registered)")},
        "extra_missing": ["any geometry: OFB-LFS has no STEP file for this experiment (only the FNG streaming and OKTAVIAN files)",
                          "the D-T source of the JAEA FNS facility and the shield compositions"],
        "data_terms": "The measured values are from JAEA (Tokai-mura, 2004); the legacy file in OFB cites doi 10.1080/00223131.2004.10875640 as its literature source (not read in this pass); if the values also come from SINBAD, NEA terms apply. Not checked.",
        "why_priority": "docs/notes/CAD_TRANSPORT_TEST_SET.md found that the RM-M dog-leg port duct has no void line from plasma to coil (0 of 20,016 rays), so neutrons there reach the response only through the duct without a collision or through thin material. A bent duct with measured spectra is the open experiment that tests exactly that path in a CAD-derived model; no other open case registered in FARIS has a bent streaming path.",
        "reference": "benchmark_list.txt item 2; legacy file attribute: doi 10.1080/00223131.2004.10875640",
    },
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout.strip()


def build(case_id: str, spec: dict, root: Path, step_dir: Path) -> dict:
    ofb = root / OFB
    if git(ofb, "status", "--porcelain"):
        raise SystemExit(f"{ofb} has local changes; use a clean checkout")
    commit, lfs_commit = git(ofb, "rev-parse", "HEAD"), git(ofb / "lfs", "rev-parse", "HEAD")
    files = []

    def add(role: str, rel: str, **extra) -> None:
        path = ofb / rel
        files.append({"role": role, "path_in_source": f"{OFB}/{rel}", "sha256": sha256(path), "bytes": path.stat().st_size, **extra})

    add("OFB list of benchmarks planned upstream", "benchmark_list.txt")
    add("OFB licence (MIT)", "LICENSE")
    if spec["step"]:
        pointer = ofb / spec["step"]
        oid = re.search(r"^oid sha256:([0-9a-f]{64})$", pointer.read_text(encoding="utf-8"), re.M).group(1)
        step = step_dir / pointer.name
        if sha256(step) != oid:
            raise SystemExit(f"{step} does not match the LFS object id {oid}")
        add("OFB Git LFS pointer of the STEP geometry (the file's own sha256 is lfs_object_sha256)", spec["step"], lfs_object_sha256=oid, lfs_object_bytes=step.stat().st_size, lfs_submodule_commit=lfs_commit)
        add("OFB-LFS licence (MIT)", "lfs/LICENSE")
    missing = COMMON_MISSING + spec["extra_missing"]
    classes = {name: {"unit": unit, "description": desc} for name, (unit, desc) in spec["classes"].items()}
    detectors = []
    for name, (_, desc) in spec["classes"].items():
        detectors.append({
            "id": f"{name.replace('_', '-')}-set", "response_class": name,
            "reference_missing_why": f"no measured values are registered: {COMMON_MISSING[1]}; the licence question is open ({spec['data_terms']})",
            "reference_missing_next_step": "wait for the upstream specification and results database entry, or obtain the measured values from their original source under its terms, then add detectors with reference value and uncertainty to this manifest"})
    return check_manifest({
        "schema": SCHEMA,
        "case_id": case_id,
        "title": spec["title"],
        "evidence_class": "experiment",
        "geometry_path": "CAD" if spec["step"] else "none",
        "source": {"url": OFB_URL, "commit": commit, "retrieved": "2026-10-14", "reference": spec["reference"], "lfs_commit": lfs_commit},
        "source_layout": "path_in_source is relative to a directory that holds the checkout openmc_fusion_benchmarks/",
        "license": {"spdx": "MIT", "attribution": MIT_ATTRIBUTION, "terms": "MIT for the OFB files hashed here. " + spec["data_terms"]},
        "files": files,
        "files_are_external_not_vendored": True,
        "registration": {
            "status": "awaiting-upstream-specification",
            "benchmark_list_item": spec["benchmark_list_item"],
            "missing": missing,
            "why_it_matters": spec["why_priority"],
            "legacy_inventory": LEGACY_NOTE,
            "last_checked": "2026-10-14 against OFB develop at the pinned commit",
        },
        "normalisation": {"status": "unconfirmed", "blocks_scoring": True,
                          "quantity": "not defined: no specification", "units": "not defined: no specification", "source_normalisation": "not defined: no specification",
                          "location": "not defined: no specification", "reaction_or_particle": "not defined: no specification",
                          "energy_integration": "not defined: no specification",
                          "basis": "OFB develop has no specification for this experiment, so no tally definition or normalisation exists to confirm.",
                          "why": "the case has no specification and no registered measured values",
                          "next_step": "register the specification and values, then confirm the normalisation from the original publication"},
        "uncertainty": {"components": [{"name": "none registered", "kind": "unspecified", "meaning": "no measured values are registered"}], "note": "No reference uncertainty exists because no reference value is registered."},
        "compatibility": {"declared": "2026-10-14", "k": 2.0, "k_basis": "authored default, to be reviewed when values exist",
                          "covariance": {"kind": "unknown", "basis": "no reference values and no calculation exist yet"}},
        "response_classes": classes,
        "detectors": detectors,
        "not_covered": ["everything: the case has no specification, no registered measured values and no FARIS run",
                        "the SINBAD licence terms for the original values are not settled (docs/NEXT_RELEASE_PLAN.md, open decision 5)"],
        "qualified_range": {"materials": ["not defined: no specification"], "geometry_class": "streaming path in a shield mock-up" if spec["step"] else "double-bend (dogleg) duct in a shield",
                            "spectrum_class": "D-T point-like source", "parameters": [], "cooling_time": "not applicable (prompt transport only)"},
        "literature_context": [],
    })


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", type=Path, required=True)
    ap.add_argument("--step-dir", type=Path, required=True)
    args = ap.parse_args()
    for case_id, spec in CASES.items():
        manifest = build(case_id, spec, args.root.expanduser(), args.step_dir.expanduser())
        out = HERE / case_id
        out.mkdir(exist_ok=True)
        (out / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"wrote {out / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
