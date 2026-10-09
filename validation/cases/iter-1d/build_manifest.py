#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Write manifest.json for the ITER_1D code-to-code case from references/iter-1d-reference.json
(the identities FARIS already registered, requirement VAL-044). No run is made and no
benchmark file is read from or copied into this repository.

    python3 build_manifest.py [--upstream DIR]   # DIR: open-benchmarks checkout, checked against the hashes
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT))
from validation.manifest import SCHEMA, check_manifest, verify_files  # noqa: E402

REGISTRATION = ROOT / "references" / "iter-1d-reference.json"
OPENMC_DIR = "jade_open_benchmarks/inputs/ITER_1D/ITER_1D/openmc"
WHY = "no provenance-qualified reference response set exists for ITER_1D (references/iter-1d-reference.json comparison status NOT_EVALUATED)"
NEXT = "obtain a reference run of a second code (MCNP or Serpent inputs exist upstream) or recover the producing inputs of the JADE statepoint, then add reference values with uncertainties"


def build(upstream: Path | None) -> dict:
    reg = json.loads(REGISTRATION.read_text(encoding="utf-8"))
    ident = reg["input_identity"]
    files = [
        {"role": "openmc geometry", "path_in_source": f"{OPENMC_DIR}/geometry.xml", "sha256": ident["geometry_xml"]["sha256"], "bytes": ident["geometry_xml"]["bytes"]},
        {"role": "openmc materials", "path_in_source": f"{OPENMC_DIR}/materials.xml", "sha256": ident["materials_xml"]["sha256"], "bytes": ident["materials_xml"]["bytes"]},
        {"role": "openmc source law (C++ text)", "path_in_source": f"{OPENMC_DIR}/iter_1d_source.cpp", "sha256": ident["iter_1d_source_cpp"]["sha256"], "bytes": ident["iter_1d_source_cpp"]["bytes"]},
        {"role": "openmc source build file", "path_in_source": f"{OPENMC_DIR}/CMakeLists.txt", "sha256": ident["cmakelists_txt"]["sha256"], "bytes": ident["cmakelists_txt"]["bytes"]},
        {"role": "benchmark metadata", "path_in_source": "jade_open_benchmarks/inputs/ITER_1D/benchmark_metadata.json", "sha256": ident["benchmark_metadata"]["sha256"]},
    ]
    commit = "1d9855b00e05c5d5e25a11026ac56940dc75b516"  # checkout whose file hashes match the registered identities (checked below when --upstream is given)
    manifest = {
        "schema": SCHEMA, "case_id": "iter-1d",
        "title": "ITER_1D neutron flux and absorption by material cell (code-to-code reference exercise)",
        "evidence_class": "code-to-code",
        "source": {"url": reg["benchmark"]["repository"], "commit": commit, "retrieved": "2026-10-09",
                   "registration": "references/iter-1d-reference.json", "local_version": reg["benchmark"]["local_version"]},
        "license": {"spdx": "CC-BY-4.0",
                    "attribution": "ITER_1D inputs from IAEA-NDS/open-benchmarks (IAEA Nuclear Data Section), CC BY 4.0. No upstream file is copied into this repository; identities are hashes.",
                    "terms": reg["benchmark"]["license"]},
        "files": files, "files_are_external_not_vendored": True,
        "normalisation": {
            "status": "confirmed", "quantity": "volume-average neutron flux and absorption rate per material cell",
            "units": "flux: particle/cm2 per source neutron; absorption: reactions/cm3 per source neutron",
            "source_normalisation": "per source neutron, unit-weight sites, 14.056 MeV normal source in void cell 51",
            "location": "97 material-filled cells (concentric z-cylinder annuli, exact analytic volumes)",
            "reaction_or_particle": "neutron; absorption scored analog, flux track-length",
            "energy_integration": "all energies",
            "basis": "references/iter-1d-reference.json response_plan and docs/ITER_1D_REFERENCE.md"},
        "uncertainty": {"components": [{"name": "Monte Carlo sampling of the calculation", "kind": "statistical"}],
                        "note": "No reference response set is registered, so no reference uncertainty exists yet."},
        "compatibility": {"declared": "2026-10-09", "k": 2.0, "k_basis": "authored: coverage factor 2 on the combined standard uncertainty",
                          "covariance": {"kind": "unknown", "basis": "no second code has been run; correlation between the two codes' results (same nuclear data, same source law) is not characterised"}},
        "response_classes": {
            "neutron_flux": {"unit": "particle/cm2 per source neutron", "description": "track-length flux averaged over each material cell"},
            "neutron_absorption": {"unit": "reactions/cm3 per source neutron", "description": "analog absorption rate density in each material cell"}},
        "detectors": [
            {"id": "neutron_flux:cells-97", "response_class": "neutron_flux", "reference": None, "reference_missing_why": WHY, "reference_missing_next_step": NEXT,
             "controlling_variable": {"name": "radial position", "value": None, "unit": "cm"}},
            {"id": "neutron_absorption:cells-97", "response_class": "neutron_absorption", "reference": None, "reference_missing_why": WHY, "reference_missing_next_step": NEXT,
             "controlling_variable": {"name": "radial position", "value": None, "unit": "cm"}}],
        "not_covered": [
            "any experimental comparison: ITER_1D is a computational benchmark, so agreement is verification evidence only and shares nuclear data and geometry with the reference code",
            "photon transport, nuclear heating and activation (the registered control is neutron-only)",
            "three-dimensional effects, ports and streaming (one-dimensional cylindrical model)",
            "temperature dependence (single 293.6 K evaluation)"],
        "qualified_range": {
            "materials": ["ITER bulk shield and vacuum vessel region materials as in the benchmark (97 material cells)"],
            "geometry_class": "one-dimensional concentric cylinder, reflective at z = +/-1000 cm, outer vacuum radius 1389.5 cm",
            "spectrum_class": "14.06 MeV D-T line source (normal, sigma 0.24 MeV) with moderated spectrum behind the first wall",
            "parameters": [{"name": "histories", "min": 10000, "max": 10000000, "unit": "histories"}],
            "cooling_time": "not applicable (prompt transport only)"},
        "literature_context": [],
    }
    if upstream is not None:
        issues = verify_files(manifest, upstream)
        head = subprocess.run(["git", "-C", str(upstream), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
        if issues or head != commit:
            raise SystemExit(f"upstream checkout does not match the registered identities (HEAD {head}): {issues}")
    return check_manifest(manifest)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--upstream", type=Path)
    ap.add_argument("--out", type=Path, default=HERE / "manifest.json")
    args = ap.parse_args()
    m = build(args.upstream.expanduser() if args.upstream else None)
    args.out.write_text(json.dumps(m, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
