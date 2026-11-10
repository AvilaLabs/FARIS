#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pin JADE HCPB/WCLL TBM raw responses and write an external candidate manifest.

JADE's EUPL-1.2 CSVs and metadata stay outside this repository. The generated
manifest preserves the candidate values, coordinates, and published MC errors,
but every detector remains blocked from scoring until the producer input and
processed-library receipts are available and the mesh mapping is qualified.

Example (all downloads and generated output outside Git):

    python3 validation/cases/iter-tbm-jade-v1/build_manifest.py \\
      --case HCPB_TBM_1D --data-dir /tmp/hcpb-jade --upstream-root \\
      /path/to/iaea-open-benchmarks --fetch --out /tmp/hcpb-jade/manifest.json \\
      --retrieved 2026-11-09
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import sys
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT))
from validation.manifest import check_manifest  # noqa: E402

RESULTS_COMMIT = "e3b442cf8e5eb3f41c500535931ddb8a6b0919e7"
INPUTS_COMMIT = "872543644e2353ab238242a27c2ce975eb935f32"
RESULTS_BASE = f"https://raw.githubusercontent.com/JADE-V-V/JADE-RAW-RESULTS/{RESULTS_COMMIT}/"
INPUTS_BASE = f"https://raw.githubusercontent.com/IAEA-NDS/iaea-open-benchmarks/{INPUTS_COMMIT}/"
LICENSE_URL = f"https://github.com/JADE-V-V/JADE-RAW-RESULTS/blob/{RESULTS_COMMIT}/LICENSE"
INPUT_LICENSE_URL = f"https://github.com/IAEA-NDS/iaea-open-benchmarks/blob/{INPUTS_COMMIT}/LICENSE"
N_ROWS = 79
RULE_DECLARED = "2026-11-09"

RESPONSES = {
    "neutron_flux": {
        "file_label": "Neutron flux",
        "published_unit": "#/cm^3/source neutron (MCNP FC comment; output unit unverified)",
        "description": "Candidate neutron mesh flux unit is copied from the pinned deck FC comment; JADE conversion and output unit remain unverified.",
    },
    "neutron_heating": {
        "file_label": "Neutron heating",
        "published_unit": "MeV/cm^3/source neutron",
        "description": "Published neutron heating mesh response; this is not the photon or total-heating response.",
    },
    "photon_flux": {
        "file_label": "Photon flux",
        "published_unit": "#/cm^3/source neutron (MCNP FC comment; output unit unverified)",
        "description": "Candidate photon mesh flux unit is copied from the pinned deck FC comment; JADE conversion and output unit remain unverified.",
    },
    "photon_heating": {
        "file_label": "Photon heating",
        "published_unit": "MeV/cm^3/source neutron",
        "description": "Published photon heating mesh response; it is kept separate from neutron heating.",
    },
    "tritium_production": {
        "file_label": "Tritium Production",
        "published_unit": "atoms/cm^3/source neutron",
        "description": "Published tritium production mesh response.",
    },
}

CASE_SPECS = {
    "HCPB_TBM_1D": {
        "title": "ITER 1D HCPB Test Blanket Module response tables",
        "metadata_sha256": "985975a5aaf0a327f2ff1155542beeec5f8ae3242ec6e04bc81d54b3d62d4ad7",
        "metadata_bytes": 225,
        "input_files": {
            "metadata": ("jade_open_benchmarks/inputs/HCPB_TBM_1D/benchmark_metadata.json", "df6946f79ecf7391c53a4fcc89e88e21f019cee8b9468f2072d8ccda21f6a446", 126),
            "mcnp_deck": ("jade_open_benchmarks/inputs/HCPB_TBM_1D/HCPB_TBM_1D/mcnp/HCPB_TBM_1D.i", "be1acca82a449ebffbcd35473e50f4b8d8b657df50cce3628db61c3ed7e23d3a", 80612),
            "openmc_materials": ("jade_open_benchmarks/inputs/HCPB_TBM_1D/HCPB_TBM_1D/openmc/materials.xml", "d67e96af0550967884cf89c9f7d91344c1378f585f44e61f2493fbac92315092", 51065),
            "openmc_geometry": ("jade_open_benchmarks/inputs/HCPB_TBM_1D/HCPB_TBM_1D/openmc/geometry.xml", "5848490e4d66e6c351f8d3c0c17c7b8d7cf9e7cc6dba94ef4ededc26ae3c8f14", 20680),
        },
        "tallies": {
            "neutron_heating": 204,
            "photon_heating": 214,
            "tritium_production": 224,
            "neutron_flux": 234,
            "photon_flux": 244,
        },
        "csv": {
            "neutron_flux": ("1abb17caacb21ab8e3c6c208d744f58712b11946c85056ccf5cbd856021c6a90", 2412),
            "neutron_heating": ("d82d0e64b2679a43e35ce1b0311b90c92d0804468346daf23a8c0ebb2426a2d2", 2323),
            "photon_flux": ("3cdef2ec978d898e0e2f8663d95d8f162b64e03662de317d834f3b7b24c2c45f", 2425),
            "photon_heating": ("f3fad3e66fa6ae7f1219ca75ff4efaaf87459d660fa9cf649af14a3b4cec5ba2", 2323),
            "tritium_production": ("2fb11119b6ea8bbc588779a0f7d5e02c3f29b6a3691e15ce412f75016dea781f", 2311),
        },
    },
    "WCLL_TBM_1D": {
        "title": "ITER 1D WCLL Test Blanket Module response tables",
        "metadata_sha256": "6f2560949983f54e1d783a4f6a21845ffc6450ae3ca6b23e32a702b42231b88c",
        "metadata_bytes": 225,
        "input_files": {
            "metadata": ("jade_open_benchmarks/inputs/WCLL_TBM_1D/benchmark_metadata.json", "53b399ff8beff3b60114e6211960dcac386eb542ecf8404f37c79bfccc91cab1", 126),
            "mcnp_deck": ("jade_open_benchmarks/inputs/WCLL_TBM_1D/WCLL_TBM_1D/mcnp/WCLL_TBM_1D.i", "fb1f37e5d3169ded8d61bb6d12350d1cc58d7da2b828427bab915284124be1ef", 89605),
            "openmc_materials": ("jade_open_benchmarks/inputs/WCLL_TBM_1D/WCLL_TBM_1D/openmc/materials.xml", "116255ca80c919418932f3762ae3dbeba82f1eac2631e93c9a44de32cece07b6", 54654),
            "openmc_geometry": ("jade_open_benchmarks/inputs/WCLL_TBM_1D/WCLL_TBM_1D/openmc/geometry.xml", "2bd5b5f87ad0f41c5bc77409af0df34f1f10888929812b6a718ac61f6997dbe6", 20676),
        },
        "tallies": {
            "neutron_heating": 214,
            "photon_heating": 224,
            "tritium_production": 234,
            "neutron_flux": 244,
            "photon_flux": 254,
        },
        "csv": {
            "neutron_flux": ("638b87e694aa47090c029e57312c463435a216d3a9f995b2109b93a5ba6dd2b0", 2392),
            "neutron_heating": ("1f6c045fd976e20fc72ab4e3a19d697bcf83568816d69efdf6a5e9d688c2b5fa", 2282),
            "photon_flux": ("380db683db52f235a2df81fb3d00db621712637a5a02106eef0c0de71f3525d0", 2398),
            "photon_heating": ("b18f7e7c1d41ff1c8dc6a935a51a276804455c06339fdb1a3a567ede267d45d6", 2302),
            "tritium_production": ("9e1deafaa2cfe977944cb72b035bb1c0b64332d43b44a2cb00b4ee34cb4d15e5", 2279),
        },
    },
}

RESPONSE_LABELS = {spec["file_label"]: key for key, spec in RESPONSES.items()}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def result_path(case: str, response: str) -> str:
    label = RESPONSES[response]["file_label"]
    return f"ROOT/_mcnp_-_FENDL 3.2b_/{case}/{case} {label}.csv"


def metadata_path(case: str) -> str:
    return f"ROOT/_mcnp_-_FENDL 3.2b_/{case}/metadata.json"


def fetch(url: str, expected_sha256: str, expected_bytes: int, destination: Path) -> bytes:
    """Fetch bounded pinned UTF-8 data and atomically place it outside Git."""
    with urllib.request.urlopen(url, timeout=30) as response:
        length = response.headers.get("Content-Length")
        if length is not None and int(length) > expected_bytes:
            raise ValueError(f"source exceeds pinned byte limit {expected_bytes}: {url}")
        data = response.read(expected_bytes + 1)
    if len(data) != expected_bytes or sha256(data) != expected_sha256:
        raise ValueError(f"pinned bytes mismatch for {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    tmp = destination.with_name(destination.name + ".tmp")
    tmp.write_bytes(data)
    tmp.replace(destination)
    return data


def verify_bytes(data: bytes, expected_sha256: str, expected_bytes: int, label: str) -> None:
    if len(data) != expected_bytes or sha256(data) != expected_sha256:
        raise ValueError(f"{label} does not match its pinned upstream bytes")


def parse_metadata(data: bytes, case: str) -> dict:
    spec = CASE_SPECS[case]
    verify_bytes(data, spec["metadata_sha256"], spec["metadata_bytes"], "JADE metadata")
    try:
        metadata = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError("invalid JADE metadata JSON") from exc
    expected = {
        "benchmark_name": case,
        "benchmark_version": "1.0",
        "code": "mcnp",
        "code_version": "6.2",
        "library": "FENDL 3.2b",
        "jade_run_version": "3.0.1",
        "jade_version": "3.1.1.dev181+g8162787",
    }
    for key, value in expected.items():
        if metadata.get(key) != value:
            raise ValueError(f"JADE metadata {key} is {metadata.get(key)!r}, expected {value!r}")
    return metadata


def parse_table(data: bytes, label: str) -> list[dict]:
    """Parse JADE Cor A,Value,Error rows; retain zero-score precision as unknown."""
    try:
        text = data.decode("utf-8-sig")
        reader = csv.DictReader(text.splitlines())
        if reader.fieldnames != ["Cor A", "Value", "Error"]:
            raise ValueError("expected exact CSV columns Cor A,Value,Error")
        rows = list(reader)
    except (UnicodeDecodeError, csv.Error) as exc:
        raise ValueError(f"{label}: invalid UTF-8 CSV: {exc}") from exc
    if len(rows) != N_ROWS:
        raise ValueError(f"{label}: expected exactly {N_ROWS} response rows, got {len(rows)}")
    out = []
    seen = set()
    for line_number, row in enumerate(rows, start=2):
        try:
            coordinate = float(row["Cor A"])
            value = float(row["Value"])
            relative_se = float(row["Error"])
        except (TypeError, ValueError) as exc:
            raise ValueError(f"{label}:{line_number}: invalid coordinate/value/relative error") from exc
        if not all(math.isfinite(x) for x in (coordinate, value, relative_se)):
            raise ValueError(f"{label}:{line_number}: non-finite data")
        if coordinate < 0 or coordinate in seen:
            raise ValueError(f"{label}:{line_number}: Cor A coordinate must be unique and non-negative")
        if value < 0 or relative_se < 0:
            raise ValueError(f"{label}:{line_number}: value and relative error must be non-negative")
        if value == 0 and relative_se != 0:
            raise ValueError(f"{label}:{line_number}: zero mean with nonzero relative error is ambiguous")
        seen.add(coordinate)
        if value == 0 and relative_se == 0:
            absolute_se = None
            precision = "unknown_zero_score"
        else:
            absolute_se = value * relative_se
            if not math.isfinite(absolute_se):
                raise ValueError(f"{label}:{line_number}: derived standard error is non-finite")
            precision = "published_relative_standard_error"
        out.append({
            "cor_a_cm": coordinate,
            "value": value,
            "relative_standard_error": relative_se if precision != "unknown_zero_score" else None,
            "standard_error": absolute_se,
            "precision_status": precision,
        })
    if out[0]["cor_a_cm"] != 830.0 or out[-1]["cor_a_cm"] != 1084.2:
        raise ValueError(f"{label}: Cor A must span the pinned 830.0 to 1084.2 cm range")
    if any(b["cor_a_cm"] <= a["cor_a_cm"] for a, b in zip(out, out[1:])):
        raise ValueError(f"{label}: Cor A values must be strictly increasing")
    return out


def parse_mcnp_mesh(deck: bytes, case: str) -> dict:
    """Verify the pinned input's source and mesh/tally cards, without running MCNP."""
    try:
        text = deck.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("MCNP input is not UTF-8") from exc
    text = "\n".join(line for line in text.splitlines() if not re.match(r"\s*[cC](?:\s|$)", line))
    required_source = (r"(?im)^\s*sdef\s+erg=d1\s+cel=51\s+x=d2\s+y=d3\s+z=d4\s+wgt=1\s*$",
                       r"(?im)^\s*sp1\s+-6\s*$",
                       r"(?im)^\s*mode\s+n\s+p\s*$")
    if not all(re.search(pattern, text) for pattern in required_source):
        raise ValueError("MCNP deck does not match the recorded unit-weight Muir DT source and coupled n/p mode")
    meshes = {}
    for response, tally in CASE_SPECS[case]["tallies"].items():
        particle = "p" if response in ("photon_flux", "photon_heating") else "n"
        mesh = re.search(
            rf"(?ims)^\s*fmesh{tally}\s*:\s*{particle}\b(?P<body>.*?)(?=^\s*(?:fc\d+|fmesh\d+|fm\d+|f\d+|print)\b|\Z)",
            text,
        )
        if not mesh:
            raise ValueError(f"missing expected {particle} FMESH tally {tally} for {response}")
        body = mesh.group("body")
        if not re.search(r"\bGEOM\s*=\s*cyl\b", body, re.I):
            raise ValueError(f"FMESH{tally} is not cylindrical")
        if not re.search(r"\bIMESH\s*=\s*830(?:\.0?)?\s*,\s*1084\.2\s+IINTS\s*=\s*1\s*,\s*78\b", body, re.I):
            raise ValueError(f"FMESH{tally} does not use the pinned 830.0..1084.2 cm / 78-interval mesh")
        meshes[response] = tally
    return {"source_law": "MCNP SDEF erg=d1, SP1 -6 Muir velocity Gaussian DT at 10 keV; unit source weight; cell 51",
            "mesh": "cylindrical radial mesh; MCNP IINTS 1,78 defines one 0..830 cm bin and 78 bins from 830..1084.2 cm (79 total); JADE Cor A row-to-bin mapping remains unverified",
            "mesh_bin_count": 79,
            "mesh_intervals": [{"lower_cm": 0.0, "upper_cm": 830.0, "bins": 1},
                               {"lower_cm": 830.0, "upper_cm": 1084.2, "bins": 78}],
            "tallies": meshes}


def verify_upstream_inputs(root: Path, case: str) -> dict:
    """Verify input bundle bytes at a local pinned checkout; never fetch nuclear data."""
    spec = CASE_SPECS[case]
    found = {}
    for role, (relative, expected_hash, expected_bytes) in spec["input_files"].items():
        path = root / relative
        try:
            data = path.read_bytes()
        except OSError as exc:
            raise ValueError(f"missing upstream input {relative}: {exc}") from exc
        verify_bytes(data, expected_hash, expected_bytes, f"upstream {role}")
        found[role] = {"path_in_source": relative, "sha256": expected_hash, "bytes": expected_bytes}
        if role == "mcnp_deck":
            found[role]["checks"] = parse_mcnp_mesh(data, case)
        elif role == "metadata":
            metadata = json.loads(data.decode("utf-8"))
            if metadata.get("name") != case or metadata.get("version") != {"mcnp": "1.0", "openmc": "1.0", "serpent": "1.0"}:
                raise ValueError(f"upstream metadata is not the expected {case} 1.0 bundle")
    return {"repository": "IAEA-NDS/open-benchmarks", "commit": INPUTS_COMMIT,
            "license": "CC-BY-4.0", "license_url": INPUT_LICENSE_URL,
            "files": found,
            "producer_linkage": "The raw-results metadata does not bind this input bundle hash to the producing MCNP run. The exact JADE source commit is 81627873432afd120c714fe039b9f778758d894d; its MCNP output parser delegates mesh conversion to F4Enix (dependency lower bound >=0.8.1), and the exact F4Enix version and per-case raw-processing configuration are absent from the result receipt."}


def input_library_block(case: str) -> dict:
    """Describe exact limitations established from deck suffixes and local audits."""
    if case == "HCPB_TBM_1D":
        missing_fendl32 = ["As75"]
        suffix_inventory = {"31c": "bulk isotopes; xsdir mapping unavailable", "21c": "natural-element ZAIDs; xsdir mapping unavailable", "70c": "He4, As75 and O16; xsdir mapping unavailable"}
    else:
        missing_fendl32 = ["As75", "Pd102", "Pd104", "Pd105", "Pd106", "Pd108", "Pd110",
                           "Te120", "Te122", "Te123", "Te124", "Te125", "Te126", "Te128", "Te130"]
        suffix_inventory = {"31c": "bulk isotopes; xsdir mapping unavailable", "21c": "natural-element ZAIDs; xsdir mapping unavailable", "70c": "He4, As75, Pd102/104/105/106/108/110, Te120/122/123/124/125/126/128/130 and O16; xsdir mapping unavailable"}
    return {
        "producer_metadata_library_label": "FENDL 3.2b",
        "input_deck_zaid_suffixes": suffix_inventory,
        "exact_producer_xsdir_or_zaid_to_file_receipt": None,
        "local_openmc_fendl32_h5_coverage": {
            "library_catalog_sha256": "bc67ab6dd3e31d0f6443bb12bdc06c352f55ab43de9de84dc7a20c51d4204674",
            "general_purpose_neutron_nuclides": 192,
            "materials_xml_nuclides_not_in_catalog": missing_fendl32,
            "qualification": "Local candidate is labeled FENDL-3.2, not FENDL-3.2b; its acquisition receipt and data terms are unresolved.",
        },
        "local_tendl2025_openmc_candidate": {
            "inventory": 2843,
            "processing_and_library_identity": "UNQUALIFIED candidate; do not use as a supplement or relabel as FENDL-3.2b",
            "materials_missing_from_candidate_catalog": ["H1", "H2", "He3", "He4"],
        },
        "eligibility": "NOT_EVALUATED: raw output says FENDL 3.2b but producer xsdir and exact processed data are absent; suffixes alone do not establish fallback data identity.",
    }


def source_descriptor(case: str, retrieved: str) -> dict:
    return {
        "url": f"https://github.com/JADE-V-V/JADE-RAW-RESULTS/tree/{RESULTS_COMMIT}/ROOT/_mcnp_-_FENDL%203.2b_/{case}",
        "commit": RESULTS_COMMIT,
        "retrieved": retrieved,
        "producer": "JADE MCNP assessment; metadata states MCNP 6.2, FENDL 3.2b, JADE run 3.0.1, JADE post-processing 3.1.1.dev181+g8162787, benchmark 1.0.",
        "producer_input_and_data_receipt_gap": "No producer input hash, xsdir/ZAID mapping, processed library hash, executable hash, run seed, or history count is supplied by the published metadata.",
    }


def manifest_from_rows(case: str, tables: dict[str, list[dict]], retrieved: str,
                       upstream_inputs: dict | None = None) -> dict:
    """Build a candidate-rich but permanently score-blocked external manifest."""
    if set(tables) != set(RESPONSES):
        raise ValueError("all five response tables must be supplied exactly once")
    if any(len(tables[name]) != N_ROWS for name in RESPONSES):
        raise ValueError(f"every response table must retain exactly {N_ROWS} rows")
    coordinates = [r["cor_a_cm"] for r in tables["neutron_flux"]]
    for name, rows in tables.items():
        if [r["cor_a_cm"] for r in rows] != coordinates:
            raise ValueError(f"{name} Cor A coordinates do not exactly match neutron flux")

    spec = CASE_SPECS[case]
    idprefix = "hcpb" if case.startswith("HCPB") else "wcll"
    dets = []
    for response, rows in tables.items():
        for index, row in enumerate(rows):
            dets.append({
                "id": f"{response}:mesh-{index:03d}",
                "response_class": response,
                "reference": None,
                "reference_missing_why": "JADE table is a pinned candidate only; the exact producer input, xsdir/processed-data receipt, and 79-row mesh-to-OpenMC response mapping are not qualified.",
                "reference_missing_next_step": "Recover producer deck/xsdir/library/run receipts, verify all 79 mesh locations and units against the OpenMC adapter, and resolve every input nuclide without cross-library fallback.",
                "blocked": {
                    "why": "reference eligibility is NOT_EVALUATED because producer input/evaluation identity and mesh-bin semantics are incomplete",
                    "next_step": "Complete the producer receipt and response mapping review before permitting C/E scoring.",
                },
                "controlling_variable": {
                    "name": "JADE Cor A mesh coordinate; bin-center/edge meaning not established",
                    "value": row["cor_a_cm"],
                    "unit": "cm",
                },
                "candidate_reference": {
                    "value": row["value"],
                    "unit": RESPONSES[response]["published_unit"],
                    "relative_standard_error": row["relative_standard_error"],
                    "standard_error": row["standard_error"],
                    "precision_status": row["precision_status"],
                    "source_field": "JADE CSV Value and Error; Error is MCNP estimated relative standard error (one sigma)",
                    "mesh_index": index,
                },
            })
    file_records = []
    for response, (digest, size) in spec["csv"].items():
        file_records.append({
            "role": f"JADE raw {response} response table",
            "path_in_source": result_path(case, response),
            "sha256": digest,
            "bytes": size,
            "license": "EUPL-1.2",
        })
    file_records.append({
        "role": "JADE producer metadata",
        "path_in_source": metadata_path(case),
        "sha256": spec["metadata_sha256"],
        "bytes": spec["metadata_bytes"],
        "license": "EUPL-1.2",
    })
    manifest = {
        "schema": "faris.validation-case/1.0.0",
        "case_id": f"{idprefix}-tbm-jade-v1",
        "title": spec["title"] + " (candidate; NOT_EVALUATED)",
        "evidence_class": "code-to-code",
        "source": source_descriptor(case, retrieved),
        "license": {
            "spdx": "EUPL-1.2",
            "attribution": f"JADE-V-V/JADE-RAW-RESULTS, {case} MCNP/FENDL 3.2b response CSVs and metadata, pinned commit {RESULTS_COMMIT}; no response values are included in the repository.",
            "terms": LICENSE_URL,
        },
        "files": file_records,
        "files_are_external_not_vendored": True,
        "input_identity": upstream_inputs,
        "nuclear_data_identity": input_library_block(case),
        "normalisation": {
            "status": "unconfirmed",
            "blocks_scoring": True,
            "quantity": "five separate total response profiles from the published 1D TBM cylindrical mesh",
            "units": "units as listed in response_classes; no conversion is applied",
            "source_normalisation": "JADE/MCNP reports tallies per source neutron; deck source weight is 1. Exact produced-run input and source receipt remain absent.",
            "location": "The pinned MCNP deck defines 79 radial bins: one from 0..830 cm and 78 from 830..1084.2 cm. The JADE output contains 79 Cor A rows ranging 830.0..1084.2 cm, but row-to-bin coordinate/volume semantics remain unverified.",
            "reaction_or_particle": "separate neutron, photon, and tritium response classes; neutron and photon heating remain distinct",
            "energy_integration": "total response as defined by the source deck; no energy-binned table is supplied",
            "basis": "JADE computational benchmark TBM documentation and the pinned upstream MCNP input cards; output-to-input and output-unit identity still require a producer receipt.",
            "why": "The source deck's FC comments label mesh flux #/cm^3/source, while MCNP mesh flux is conventionally volume-averaged fluence in #/cm^2/source. JADE's pinned parser delegates meshtal conversion to F4Enix, but its exact version and the per-case raw-processing configuration are not in the receipt; the CSV unit/normalization and Cor A row mapping therefore remain unresolved.",
            "next_step": "Recover the exact producing mctal/meshtal, per-case JADE raw-processing configuration, and F4Enix version; then confirm units and all 79 Cor A row meanings against the pinned input and OpenMC adapter.",
        },
        "uncertainty": {
            "components": [{"name": "published MCNP Monte Carlo relative standard error", "kind": "statistical", "source_field": "CSV Error"}],
            "note": "For nonzero scores, candidate standard error is Value × Error. A Value=0, Error=0 row is recorded with precision_status=unknown_zero_score and standard_error=null; it is not treated as zero uncertainty. No model, nuclear-data, or cross-code covariance uncertainty is supplied.",
        },
        "compatibility": {
            "declared": RULE_DECLARED,
            "k": 2.0,
            "k_basis": "authored: coverage factor 2; not used while detectors are blocked",
            "covariance": {"kind": "unknown", "basis": "The JADE source does not provide covariance between its response classes or between JADE and a future FARIS run."},
        },
        "response_classes": {
            name: {"unit": response["published_unit"], "description": response["description"]}
            for name, response in RESPONSES.items()
        },
        "detectors": dets,
        "not_covered": [
            "physical validation against measurements; these are code-to-code outputs",
            "exact producer input deck, xsdir ZAID mapping, processed-data hashes, executable identity, run seed and history count",
            "exact radial mesh row-to-cell/volume mapping and flux dimensional normalization",
            "cross-code covariance and model/nuclear-data uncertainty",
            "any response or criterion verdict; no requirement is marked Met",
        ],
        "qualified_range": {
            "materials": [f"{case} materials as identified by the pinned IAEA OpenMC/MCNP inputs; producer data mapping not verified"],
            "geometry_class": "ITER 1D cylindrical outboard TBM with adjacent shielding; 79 Cor A result rows",
            "spectrum_class": "MCNP source card SP1 -6 Muir velocity-Gaussian DT spectrum at 10 keV, unit-weight source; producer-run link absent",
            "parameters": [],
            "cooling_time": "not applicable (prompt transport)",
        },
        "literature_context": [],
    }
    return check_manifest(manifest)


def registration_manifest(case: str) -> dict:
    """Committed value-free registration: 79 row slots for each of five responses."""
    empty_tables = {
        response: [{"cor_a_cm": float(index), "value": 0.0,
                    "relative_standard_error": None, "standard_error": None,
                    "precision_status": "unfetched"} for index in range(N_ROWS)]
        for response in RESPONSES
    }
    m = manifest_from_rows(case, empty_tables, RULE_DECLARED)
    for det in m["detectors"]:
        det["candidate_reference"] = {
            "value": None,
            "precision_status": "external pinned CSV not vendored; fetch through build_manifest.py",
            "mesh_index": int(det["id"].rsplit("-", 1)[1]),
        }
        det["controlling_variable"] = {
            "name": "row ordinal in JADE Cor A table; coordinate remains external",
            "value": int(det["id"].rsplit("-", 1)[1]),
            "unit": "index",
        }
    return check_manifest(m)


def build_external_manifest(case: str, data_dir: Path, upstream_root: Path,
                            retrieved: str, should_fetch: bool = False) -> dict:
    if case not in CASE_SPECS:
        raise ValueError(f"unknown case {case!r}")
    spec = CASE_SPECS[case]
    data_dir.mkdir(parents=True, exist_ok=True)
    metadata_file = data_dir / "metadata.json"
    if should_fetch:
        fetch(RESULTS_BASE + urllib.request.pathname2url(metadata_path(case)),
              spec["metadata_sha256"], spec["metadata_bytes"], metadata_file)
    metadata = parse_metadata(metadata_file.read_bytes(), case)
    input_receipt = verify_upstream_inputs(upstream_root, case)
    tables = {}
    for response, (digest, size) in spec["csv"].items():
        path = data_dir / f"{case} {RESPONSES[response]['file_label']}.csv"
        url = RESULTS_BASE + urllib.request.pathname2url(result_path(case, response))
        if should_fetch:
            fetch(url, digest, size, path)
        data = path.read_bytes()
        verify_bytes(data, digest, size, f"JADE {case} {response} CSV")
        tables[response] = parse_table(data, response)
    manifest = manifest_from_rows(case, tables, retrieved, input_receipt)
    manifest["producer_metadata"] = metadata
    return check_manifest(manifest)


def _inside(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--case", choices=sorted(CASE_SPECS), required=True)
    ap.add_argument("--data-dir", type=Path, required=True, help="external cache for JADE CSVs and metadata")
    ap.add_argument("--upstream-root", type=Path, required=True, help="pinned IAEA-NDS/open-benchmarks checkout")
    ap.add_argument("--fetch", action="store_true", help="download pinned JADE CSVs and metadata to --data-dir")
    ap.add_argument("--out", type=Path, required=True, help="write the generated candidate manifest outside Git")
    ap.add_argument("--retrieved", required=True, help="retrieval date YYYY-MM-DD")
    args = ap.parse_args()
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", args.retrieved):
        ap.error("--retrieved must use YYYY-MM-DD")
    for label, path in (("data directory", args.data_dir), ("manifest output", args.out)):
        if _inside(path, REPO_ROOT):
            ap.error(f"{label} must stay outside the Git checkout to avoid vendoring JADE raw results")
    try:
        manifest = build_external_manifest(args.case, args.data_dir, args.upstream_root,
                                           args.retrieved, args.fetch)
    except (OSError, ValueError) as exc:
        ap.error(str(exc))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote NOT_EVALUATED candidate manifest with {len(manifest['detectors'])} blocked rows; raw results remain external")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
