#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Build one hash-bound FARIS manifest per public FNS-TOF assembly.

    python3 validation/cases/fns-tof-jade-v1/build_manifest.py \
      --upstream /path/to/open-benchmarks --out validation/cases

No files are copied from the upstream checkout. The compact reference tables
are included in each generated manifest under the upstream CC-BY-4.0 terms.
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
sys.path.insert(0, str(HERE.parents[2]))
from validation.manifest import SCHEMA, check_manifest  # noqa: E402

from benchmark import (  # noqa: E402
    ASSEMBLIES, BASE, EXPERIMENTS, INPUTS, LICENSE, LOWER_MEASUREMENT_LIMIT_MEV,
    SOURCE_COMMIT, SOURCE_URL, IntakeError, energy_bins, fm_geometric_factor,
    input_deck, read_rows, sha256_bytes,
)

JADE_COMMIT = "5ed62bdabbfb1d77a02d98db486916f5b47ec86d"
JADE_RAW_POSTPROCESS = f"https://github.com/JADE-V-V/JADE/blob/{JADE_COMMIT}/src/jade/resources/default_cfg/benchmarks_pp/raw/mcnp/FNS-TOF.yaml"
MCNP_MANUAL = "https://mcnp.lanl.gov/pdf_files/TechReport_2022_LANL_LA-UR-22-30006Rev.1_KuleszaAdamsEtAl.pdf"
JADE_BENCHMARK_DOC = "https://jade-a-nuclear-data-libraries-vv-tool.readthedocs.io/en/latest/benchmarks/experimental.html#fns-time-of-flight-experiment"
JADE_LETHARGY_CODE = f"https://github.com/JADE-V-V/JADE/blob/{JADE_COMMIT}/src/jade/post/manipulate_tally.py"
MAX_SOURCE_FILE_BYTES = 2_000_000


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout.strip()


def file_record(root: Path, rel: str, role: str) -> dict:
    path = root / rel
    if not path.is_file():
        raise IntakeError(f"pinned source file is missing: {rel}")
    if path.stat().st_size > MAX_SOURCE_FILE_BYTES:
        raise IntakeError(f"pinned source file exceeds the {MAX_SOURCE_FILE_BYTES}-byte intake limit: {rel}")
    data = path.read_bytes()
    return {"path_in_source": rel, "role": role, "bytes": len(data), "sha256": sha256_bytes(data)}


def angle_filename(angle: float) -> str:
    return f"{angle:g} deg.csv"


def build_one(upstream: Path, assembly: str) -> dict:
    if assembly not in ASSEMBLIES:
        raise IntakeError(f"unknown assembly {assembly!r}")
    deck_rel = f"{INPUTS}/FNS-TOF_{assembly}/mcnp/FNS-TOF_{assembly}.i"
    deck_path = upstream / deck_rel
    if not deck_path.is_file():
        raise IntakeError(f"no MCNP input for {assembly}: {deck_rel}")
    if deck_path.stat().st_size > MAX_SOURCE_FILE_BYTES:
        raise IntakeError(f"MCNP input exceeds the {MAX_SOURCE_FILE_BYTES}-byte intake limit: {deck_rel}")
    deck_bytes = deck_path.read_bytes()
    parsed = input_deck(deck_bytes.decode("utf-8"))
    by_angle = {round(d["angle_deg"], 1): d for d in parsed["detectors"]}
    if len(by_angle) != len(parsed["detectors"]):
        raise IntakeError(f"{assembly}: duplicate F5 detector angles")

    source_files = [file_record(upstream, deck_rel, "MCNP geometry, material, source, detector and tally input")]
    metadata_rel = f"{INPUTS}/benchmark_metadata.json"
    source_files.append(file_record(upstream, metadata_rel, "benchmark metadata"))
    source_files.append(file_record(upstream, "LICENSE", "source repository license"))
    detectors = []
    table_map = []
    angles_in_tables = set()
    for angle in sorted(by_angle):
        angle_slug = f"{angle:g}deg"
        for coarse in (False, True):
            prefix = "Coarse leakage flux" if coarse else "Neutron leakage flux"
            filename = f"FNS-TOF_{assembly} {prefix} at {angle_filename(angle)}"
            rel = f"{EXPERIMENTS}/{filename}"
            path = upstream / rel
            if not path.is_file():
                continue
            data = path.read_bytes()
            rows = read_rows(data, rel)
            if any(row["angle"].strip() != f"{angle:g} deg" for row in rows):
                raise IntakeError(f"{rel}: row Angle values do not match the FC tally label")
            bins = energy_bins(rows, parsed["fine_energy_upper_edges_mev"] if not coarse else None, not coarse)
            tally = by_angle[angle]
            if not coarse and len(rows) != len(parsed["fine_energy_upper_edges_mev"]):
                raise IntakeError(f"{rel}: expected every E0 bin, found {len(rows)}")
            angles_in_tables.add(angle)
            file_index = len(source_files)
            source_files.append(file_record(upstream, rel, "measured leakage spectrum table" if not coarse else "measured coarse leakage spectrum table"))
            records = []
            for i, (row, ebin) in enumerate(zip(rows, bins, strict=True)):
                value = row["value"]
                u_reported = row["error_absolute"]
                energy_slug = f"{ebin['hi_mev']:.9g}".replace(".", "p") if not coarse else f"{i:02d}"
                detector_id = f"a{angle_slug}-{'coarse' if coarse else 'fine'}-{energy_slug}"
                detector = {
                    "id": detector_id,
                    "response_class": "neutron_leakage_spectrum",
                    "location": {"tally_number": tally["tally"], "angle_deg": angle},
                    "bin": ebin,
                    "reference": {"value": value, "u": u_reported},
                }
                if ebin["hi_mev"] <= LOWER_MEASUREMENT_LIMIT_MEV:
                    detector["blocked"] = {
                        "why": "the JADE FNS-TOF benchmark description states measurements begin above 100 keV; this full source table row lies below that experimental support",
                        "next_step": "preserve the row, but score only bins wholly within the documented measured energy support",
                    }
                elif ebin["lo_mev"] < LOWER_MEASUREMENT_LIMIT_MEV < ebin["hi_mev"]:
                    detector["blocked"] = {
                        "why": "this response bin straddles the JADE FNS-TOF experimental lower limit of 100 keV",
                        "next_step": "preserve the bin and do not score it until the original data provide a justified edge split or acceptance rule",
                    }
                detectors.append(detector)
                records.append({"detector_id": detector_id, "bin_index": i, "lo_mev": ebin["lo_mev"], "hi_mev": ebin["hi_mev"]})
            table_map.append({"path_in_source": rel, "sha256": sha256_bytes(data), "angle_deg": angle,
                              "tally_number": tally["tally"], "mcnp_FM": tally["mcnp_fm"],
                              "geometry_factor_check": fm_geometric_factor(tally, parsed["source_position_cm"]),
                              "coarse": coarse, "detector_ids": records, "source_file_index": file_index})

    if not detectors:
        raise IntakeError(f"{assembly}: no reference tables match the supplied MCNP deck")
    angles_without_data = sorted(set(by_angle) - angles_in_tables)
    source = {
        "url": SOURCE_URL, "commit": SOURCE_COMMIT,
        "retrieved": "2026-11-09",
        "fetch": f"git clone {SOURCE_URL} open-benchmarks && git -C open-benchmarks checkout {SOURCE_COMMIT}",
        "original_experiment": "JAEA/FNS DT-neutron time-of-flight leakage angular spectrum benchmark, as packaged by IAEA-NDS/open-benchmarks and described in JADE",
        "definition_url": JADE_BENCHMARK_DOC,
        "manual_url": MCNP_MANUAL,
        "manual_source_cards_section": "MCNP 6.3.0 Theory & User Manual, section 5.8.2–5.8.3, pp. 393–394: SI defaults to H; SP defaults to D; the first histogram SP entry is zero and bin probabilities need not be normalized.",
        "JADE_postprocessor": {"url": JADE_RAW_POSTPROCESS, "commit": JADE_COMMIT,
                               "sha256": "b1fba4bc5a0288bee1966292cfdc9c7318148c1bac8ce9fe2211475c8de36d6f",
                               "lethargy_transform_url": JADE_LETHARGY_CODE,
                               "lethargy_transform_sha256": "54d2256ef98d79993d3ce7e08b43794b492627abbe0caee9c0c77ffc0459db89",
                               "zero_lower_edge_convention": "JADE by_lethargy prepends 1e-10 MeV as the numerical zero before dividing by ln(E_hi/E_lo); this exact pinned convention is also used for OpenMC tallies"},
    }
    manifest = {
        "schema": SCHEMA,
        "case_id": f"fns-tof-{assembly.lower()}",
        "title": f"JAEA FNS-TOF {assembly.replace('-', ' ')} assembly leakage angular spectrum",
        "evidence_class": "experiment",
        "source": source,
        "license": {"spdx": LICENSE,
                    "attribution": f"JAEA FNS benchmark data, IAEA Nuclear Data Section open-benchmarks, CC BY 4.0, commit {SOURCE_COMMIT}; source files are enumerated and SHA256-pinned below.",
                    "terms": "The upstream repository states its input files and experimental data are licensed CC BY 4.0. Source artifacts remain external; measurement rows copied into the manifest are attributed to that repository and JAEA FNS."},
        "files": source_files,
        "files_are_external_not_vendored": True,
        "normalisation": {
            "status": "confirmed", "blocks_scoring": False,
            "quantity": "FNS measured angular leakage neutron flux per source neutron per unit lethargy at the center of the assembly rear surface, for each detector angle",
            "units": "neutrons per source neutron per unit lethargy per steradian (as reduced by the detector-collimator effective area and solid angle)",
            "source_normalisation": "per source neutron; MCNP SI1/SP1 vertical histogram distribution weights are normalized to their sum by source sampling",
            "location": "five point detector locations from MCNP F5/F15/F25/F35/F45 input cards, joined only by their FC angle labels; missing supported locations are not synthesized",
            "reaction_or_particle": "neutrons",
            "energy_integration": "MCNP E0 upper-edge bins; output is integrated per energy bin then divided by the lethargy width ln(E_hi/E_lo); coarse table ranges are preserved independently",
            "basis": "JADE's pinned FNS-TOF raw-MCNP postprocessor applies the lethargy conversion to each point detector output and does not apply a second geometric multiplier. Its pinned by_lethargy implementation prepends 1e-10 MeV as the numerical lower edge for the initial bin and divides values by each adjacent ln(E_hi/E_lo). The pinned input deck FM card values numerically match the JADE definition L^2/A_s within 0.4% when L is computed from the SDEF target point to the F5 point and A_s=0.2304*L-84.16 cm^2; OpenMC applies the exact per-tally FM value from the input exactly once before dividing by the pinned JADE lethargy width. No additional L^2/A_s postfactor is applied. The JADE narrative sentence that says no FM card is present is retained as a documented source inconsistency, but the exact input card and JADE processing map determine this adapter's calculation path.",
            "mcnp_fm_applied_once": True,
            "JADE_docs_source_inconsistency": "The narrative says no FM card was used; every pinned FNS-TOF deck contains per-detector FM cards. Those factors match L^2/A_s; JADE's actual raw postprocessor does not apply a second factor.",
        },
        "uncertainty": {"components": [{"name": "CSV Error", "kind": "unspecified",
                                         "meaning": "Fine tables publish an abs err column and it is checked against Value*Error (all 9,750 checked fine rows match); coarse tables publish only Error, so u is arithmetically derived as Value*Error. Source path, SHA256 and row number are retained in adapter.tables. Neither column's confidence level, systematic decomposition, or energy-bin covariance is specified."}],
                        "note": "Numerical reference uncertainties are retained without a confidence interpretation; covariance is unknown, so compatibility remains INCONCLUSIVE."},
        "compatibility": {"declared": "2026-11-09", "k": 2.0,
                          "k_basis": "authored policy; comparison remains inconclusive while the source error/covariance meaning is unknown",
                          "covariance": {"kind": "unknown", "basis": "the experimental data do not publish cross-bin covariance or a complete measurement uncertainty model"}},
        "response_classes": {"neutron_leakage_spectrum": {"unit": "neutrons per source neutron per unit lethargy per steradian",
                                                           "description": "Angular leakage spectrum values from the public FNS-TOF data tables; full source rows are retained, with under-range and straddling bins explicitly blocked."}},
        "detectors": detectors,
        "not_covered": [
            "physical validation of FARIS OpenMC results; the finite-volume estimate is a point-detector approximation and remains unqualified until the predeclared convergence rule passes",
            "independent MCNP-to-OpenMC geometry/material equivalence review of openmc_mcnp_adapter; surface-boundary and sampled detector-containment checks are necessary guards, not converter verification",
            "measurement covariance or confidence interpretation not given in the public CSV",
            "bins below or straddling the documented 100 keV lower measurement bound (retained and blocked)",
            "angles for which a corresponding point tally/table pair is absent from the pinned source package",
            "other FNS integral response types (activation, heating, reaction rates) and other materials not in this assembly",
        ],
        "qualified_range": {"materials": [assembly.split("-")[0]],
                            "geometry_class": "the exact assembly defined by the hash-pinned MCNP input (conversion identity recorded per run)",
                            "spectrum_class": "the exact SI1/SP1 point-source histogram in the MCNP deck",
                            "parameters": [], "cooling_time": "not applicable"},
        "adapter": {
            "source_deck": deck_rel,
            "geometry_and_material_translation": "openmc_mcnp_adapter.mcnp_to_model (source, settings and tallies replaced by the FARIS adapter); converter is not methodically V&V'd per its upstream README, so execution identity is sealed and physical equivalence is not claimed",
            "source_histogram": parsed["source_histogram"],
            "source_position_cm": parsed["source_position_cm"],
            "detectors": parsed["detectors"],
            "tables": table_map,
            "missing_table_angles_deg": angles_without_data,
            "finite_point_estimator": {"method": "volume-averaged scalar flux in spherical vacuum cells centered at exact MCNP detector points",
                                        "initial_radius_cm": 2.5,
                                        "radii_cm": [2.5, 1.25, 0.625],
                                        "frozen_qualification": {"successive_halvings_required": 2,
                                                                 "max_change_each_bin": 0.01,
                                                                 "max_relative_mcse_each_endpoint": 0.02,
                                                                 "zero_or_rare_bins": "must be retained and their relative MCSE resolved; otherwise whole qualification is INCONCLUSIVE",
                                                                 "pass_does_not_claim_physical_validation": True}},
            "scoring_gate": "compatibility covariance is unknown; row-level C/E stays INCONCLUSIVE even after the numerical point approximation passes its convergence check. Independent converter geometry/material equivalence review is also outstanding; convergence and descriptive C/E do not establish physical validation",
            "converter_verification": {"status": "NOT_EVALUATED",
                                       "why": "the public converter declares that it has not had methodical verification; this package has no independent MCNP/OpenMC geometry-material equivalence dataset",
                                       "next_step": "compare converted cells/material compositions/surfaces against the pinned input with a qualified independent checker before any physical-validation claim"},
        },
    }
    # Detect duplicate or missing table mappings without relying on file order.
    ids = [d["id"] for d in detectors]
    if len(ids) != len(set(ids)):
        raise IntakeError(f"{assembly}: detector IDs are not unique")
    if not any(d["response_class"] == "neutron_leakage_spectrum" for d in detectors):
        raise IntakeError(f"{assembly}: no declared response class")
    return manifest


def render_manifest(manifest: dict) -> str:
    """Keep top-level metadata readable while emitting each response as one row."""
    lines = ["{"]
    keys = list(manifest)
    for key_index, key in enumerate(keys):
        value = manifest[key]
        comma = "," if key_index + 1 < len(keys) else ""
        encoded_key = json.dumps(key, ensure_ascii=False)
        if key == "detectors":
            lines.append(f"  {encoded_key}: [")
            for index, detector in enumerate(value):
                row_comma = "," if index + 1 < len(value) else ""
                lines.append("    " + json.dumps(detector, ensure_ascii=False, separators=(",", ":"), allow_nan=False) + row_comma)
            lines.append(f"  ]{comma}")
        else:
            encoded_value = json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False)
            indented = "\n".join("  " + line for line in encoded_value.splitlines())
            lines.append(f"  {encoded_key}: {indented}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", type=Path, required=True, help="clean IAEA-NDS/open-benchmarks checkout at the pinned commit")
    parser.add_argument("--out", type=Path, default=HERE.parent, help="cases root; writes <case_id>/manifest.json (generated manifests, not upstream data)")
    parser.add_argument("--assembly", choices=ASSEMBLIES, help="generate one assembly; default generates every input/table pair")
    args = parser.parse_args()
    upstream = args.upstream.expanduser().resolve()
    if git(upstream, "status", "--porcelain"):
        raise SystemExit("upstream source checkout has local modifications; use a clean pinned checkout")
    commit = git(upstream, "rev-parse", "HEAD")
    if commit != SOURCE_COMMIT:
        raise SystemExit(f"upstream commit must be {SOURCE_COMMIT}, got {commit}")
    args.out.mkdir(parents=True, exist_ok=True)
    for assembly in ([args.assembly] if args.assembly else ASSEMBLIES):
        manifest = build_one(upstream, assembly)
        check_manifest(manifest)
        output = args.out / manifest["case_id"] / "manifest.json"
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(render_manifest(manifest), encoding="utf-8")
        print(f"wrote {output} ({len(manifest['detectors'])} responses, {len(manifest['files'])} pinned files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
