#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Fetch and verify JADE's published ITER_1D MCNP total-flux CSV and its
cell-map/tally inputs, then write an external manifest. Source files stay out
of the repository.

    python3 validation/cases/iter-1d-flux-jade-v1/build_manifest.py \
      --csv /tmp/ITER_1D-total-neutron-flux.csv --out /tmp/iter-1d-flux/manifest.json

Use --fetch to download the pinned file. It is verified against its recorded
SHA-256 before any response values are parsed.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import sys
import urllib.request
from pathlib import Path
from xml.etree import ElementTree

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT))
from validation.manifest import check_manifest

CASE_ID = "iter-1d-flux-jade-v1"
COMMIT = "dfc38c467c1a6d9c0f735081ada4fd644097d69e"
JADE_COMMIT = "735331b3f9f277acc728a0783498742124d68c97"
DECK_COMMIT = "10deb931cbc8f7c22185e4a00e98d260d0a7698d"
OPENMC_COMMIT = "1d9855b00e05c5d5e25a11026ac56940dc75b516"
RULE_DECLARED = "2026-11-09"
CSV_PATH = "ROOT/_mcnp_-_FENDL 3.2b_/ITER_1D/ITER_1D Total neutron flux.csv"
CSV_SHA256 = "b231fd670e067e549fd1fd542e8faa7dacedc0bf0e058544369123d54a1675d3"
CSV_BYTES = 2720
METADATA_PATH = "ROOT/_mcnp_-_FENDL 3.2b_/ITER_1D/metadata.json"
METADATA_SHA256 = "6fa3d6d7a257ff010649880b18621c9ff25b1ced3196c1288f4eb1e18ac36f8b"
METADATA_BYTES = 223
MAP_PATH = "src/jade/resources/default_cfg/benchmarks_pp/raw/mcnp/ITER_1D.yaml"
MAP_SHA256 = "5dacf7d32539a6baa219cc986424ea4777c972aa10311b06fd42bddc708a087e"
MAP_BYTES = 3303
DECK_PATH = "tests/dummy_structure/benchmark_templates/ITER_1D/ITER_1D/mcnp/ITER_1D.i"
DECK_SHA256 = "0982d96f563a7838d6b2f752a25a52fb01abcc894ee522aaa46c8b470f42f211"
DECK_BYTES = 46316
GEOMETRY_PATH = "jade_open_benchmarks/inputs/ITER_1D/ITER_1D/openmc/geometry.xml"
GEOMETRY_SHA256 = "80bef907daab030cf2175e54383c20a0cbf92b5b1a8aa26e9bc88b744fbb6228"
GEOMETRY_BYTES = 13938
SOURCE_PATH = "jade_open_benchmarks/inputs/ITER_1D/ITER_1D/openmc/iter_1d_source.cpp"
SOURCE_SHA256 = "06862214422bb3c09f93b0107f0b4badac1bda1796019f704ff4547a7f5c2779"
SOURCE_BYTES = 2191
FM_SCALE = 1.222e21
RAW_BASE = f"https://raw.githubusercontent.com/JADE-V-V/JADE-RAW-RESULTS/{COMMIT}/"
CSV_URL = RAW_BASE + urllib.request.pathname2url(CSV_PATH)
METADATA_URL = RAW_BASE + urllib.request.pathname2url(METADATA_PATH)
LICENSE_URL = f"https://github.com/JADE-V-V/JADE-RAW-RESULTS/blob/{COMMIT}/LICENSE"
JADE_LICENSE_URL = f"https://github.com/JADE-V-V/JADE/blob/{JADE_COMMIT}/LICENSE"
OPENMC_LICENSE_URL = f"https://github.com/IAEA-NDS/iaea-open-benchmarks/blob/{OPENMC_COMMIT}/LICENSE"
MAP_URL = f"https://raw.githubusercontent.com/JADE-V-V/JADE/{JADE_COMMIT}/{MAP_PATH}"
DECK_URL = f"https://raw.githubusercontent.com/JADE-V-V/JADE/{DECK_COMMIT}/{DECK_PATH}"
GEOMETRY_URL = f"https://raw.githubusercontent.com/IAEA-NDS/iaea-open-benchmarks/{OPENMC_COMMIT}/{GEOMETRY_PATH}"
SOURCE_URL = f"https://raw.githubusercontent.com/IAEA-NDS/iaea-open-benchmarks/{OPENMC_COMMIT}/{SOURCE_PATH}"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch(url: str, expected_sha256: str, destination: Path, max_bytes: int) -> bytes:
    """Fetch exact pinned bytes, verify first, then atomically place locally."""
    with urllib.request.urlopen(url, timeout=30) as response:
        declared_size = response.headers.get("Content-Length")
        if declared_size is not None and int(declared_size) > max_bytes:
            raise ValueError(f"source exceeds the {max_bytes}-byte download limit")
        data = response.read(max_bytes + 1)
    if len(data) > max_bytes:
        raise ValueError(f"source exceeds the {max_bytes}-byte download limit")
    actual = sha256(data)
    if actual != expected_sha256:
        raise ValueError(f"SHA-256 mismatch for {url}: expected {expected_sha256}, got {actual}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    tmp = destination.with_name(destination.name + ".tmp")
    tmp.write_bytes(data)
    tmp.replace(destination)
    return data


def parse_table(data: bytes) -> list[dict]:
    """Return radial response coordinates and one-sigma MCNP uncertainty."""
    try:
        text = data.decode("utf-8-sig")
        rows = list(csv.DictReader(text.splitlines()))
    except (UnicodeDecodeError, csv.Error) as exc:
        raise ValueError(f"invalid UTF-8 CSV: {exc}") from exc
    if not rows or list(rows[0]) != ["Cells", "Value", "Error"]:
        raise ValueError("expected CSV header Cells,Value,Error and at least one response")
    out, seen = [], set()
    for line, row in enumerate(rows, start=2):
        try:
            position = float(row["Cells"])
            value = float(row["Value"])
            relative_se = float(row["Error"])
        except (TypeError, ValueError) as exc:
            raise ValueError(f"line {line}: invalid radial coordinate/value/relative standard error") from exc
        if not math.isfinite(position) or position < 0 or position in seen:
            raise ValueError(f"line {line}: radial coordinate must be unique, finite and non-negative")
        if not math.isfinite(value) or value <= 0:
            raise ValueError(f"line {line}: flux value must be finite and positive")
        if not math.isfinite(relative_se) or relative_se < 0:
            raise ValueError(f"line {line}: relative standard error must be finite and non-negative")
        uncertainty = value * relative_se
        if not math.isfinite(uncertainty):
            raise ValueError(f"line {line}: derived standard uncertainty is not finite")
        seen.add(position)
        out.append({"position_cm": position, "value": value, "u": uncertainty,
                    "relative_standard_error": relative_se})
    if len(out) != 97:
        raise ValueError(f"expected the published 97-cell response set, found {len(out)} rows")
    return out


def parse_cell_radius_map(data: bytes) -> dict[float, int]:
    """Read JADE's named MCNP-cell-to-radial-coordinate mapping strictly."""
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("invalid UTF-8 cell map") from exc
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if "_cell_id_x:" in line), None)
    if start is None:
        raise ValueError("JADE cell map lacks _cell_id_x mapping")
    mapping: dict[float, int] = {}
    ids: set[int] = set()
    for line in lines[start + 1:]:
        m = __import__("re").fullmatch(r"\s+(\d+)\s*:\s*([0-9]+(?:\.[0-9]+)?)\s*(?:#.*)?", line)
        if not m:
            if mapping and line.strip() and not line.lstrip().startswith("#"):
                break
            continue
        cell, radius = int(m.group(1)), float(m.group(2))
        if cell in ids or radius in mapping:
            raise ValueError("cell map IDs and radial coordinates must be unique")
        ids.add(cell)
        mapping[radius] = cell
    if len(mapping) != 104:
        raise ValueError(f"expected 104 JADE cell-map entries, found {len(mapping)}")
    return mapping


def parse_tally_cells_and_scale(data: bytes) -> tuple[set[int], float]:
    """Extract F4 tally IDs and its FM multiplier from the pinned MCNP deck."""
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("invalid UTF-8 MCNP deck") from exc
    import re
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(r"^\s*f204:n\b", line, re.I)), None)
    fm = next((i for i, line in enumerate(lines) if re.match(r"^\s*fm204\b", line, re.I)), None)
    if start is None or fm is None or fm <= start:
        raise ValueError("deck must contain F204:N followed by FM204")
    card = " ".join(lines[start:fm]).replace("&", " ")
    card = re.sub(r"\$.*", "", card)
    ids = {int(token) for token in re.findall(r"(?<![A-Za-z0-9.])[0-9]+", card.split(":", 1)[1])}
    if len(ids) != 97:
        raise ValueError(f"expected 97 unique F204 tally cells, found {len(ids)}")
    fm_line = re.sub(r"\$.*", "", lines[fm]).split()
    if len(fm_line) != 2 or fm_line[0].lower() != "fm204":
        raise ValueError("FM204 must contain one explicit multiplier")
    try:
        scale = float(fm_line[1])
    except ValueError as exc:
        raise ValueError("invalid FM204 multiplier") from exc
    if not math.isfinite(scale) or scale <= 0:
        raise ValueError("FM204 multiplier must be finite and positive")
    return ids, scale


def map_rows_to_cells(rows: list[dict], cell_map: dict[float, int], tally_ids: set[int]) -> list[dict]:
    """Pair only exact published coordinates and deck tally cell IDs."""
    if len(rows) != 97 or len(tally_ids) != 97:
        raise ValueError("expected exactly 97 reference rows and tally cells")
    mapped = []
    for row in rows:
        radius = row["position_cm"]
        if radius not in cell_map:
            raise ValueError(f"unmapped JADE coordinate {radius:g} cm")
        cell_id = cell_map[radius]
        if cell_id not in tally_ids:
            raise ValueError(f"JADE coordinate {radius:g} maps to cell {cell_id}, absent from F204")
        mapped.append({**row, "cell_id": cell_id})
    if len({r["cell_id"] for r in mapped}) != 97 or {r["cell_id"] for r in mapped} != tally_ids:
        raise ValueError("JADE coordinates do not map one-to-one onto all F204 cells")
    return mapped


def parse_openmc_outer_radii(data: bytes) -> dict[int, float]:
    """Return each OpenMC cell's unique outer z-cylinder radius from its CSG region."""
    try:
        root = ElementTree.fromstring(data)
    except ElementTree.ParseError as exc:
        raise ValueError("invalid OpenMC geometry XML") from exc
    cylinders: dict[int, float] = {}
    for surface in root.findall("surface"):
        if surface.get("type") != "z-cylinder":
            continue
        try:
            sid = int(surface.attrib["id"])
            coeffs = [float(v) for v in surface.attrib["coeffs"].split()]
        except (KeyError, ValueError) as exc:
            raise ValueError("invalid z-cylinder surface in OpenMC geometry") from exc
        if len(coeffs) != 3 or not all(math.isfinite(v) for v in coeffs) or coeffs[2] <= 0:
            raise ValueError("OpenMC z-cylinder must have three finite coefficients and positive radius")
        if sid in cylinders:
            raise ValueError("duplicate OpenMC surface ID")
        cylinders[sid] = coeffs[2]
    out: dict[int, float] = {}
    import re
    for cell in root.findall("cell"):
        try:
            cid = int(cell.attrib["id"])
            region = cell.attrib["region"]
        except (KeyError, ValueError) as exc:
            raise ValueError("OpenMC cell lacks valid ID/region") from exc
        if cid in out:
            raise ValueError("duplicate OpenMC cell ID")
        signed_surfaces = [int(v) for v in re.findall(r"(?<![\w.])[-+]?\d+", region)]
        outer = [cylinders[-sid] for sid in signed_surfaces if sid < 0 and -sid in cylinders]
        if len(outer) == 1:
            out[cid] = outer[0]
    if not out:
        raise ValueError("no OpenMC cells with a unique negative z-cylinder boundary found")
    return out


def verify_openmc_mapping(rows: list[dict], cell_outer_radii: dict[int, float]) -> None:
    """Reject absent cells or a mismatch between the mapped coordinate and OpenMC outer radius."""
    for row in rows:
        cell = row["cell_id"]
        if cell not in cell_outer_radii:
            raise ValueError(f"mapped cell {cell} has no unique OpenMC outer radial boundary")
        if not math.isclose(row["position_cm"], cell_outer_radii[cell], rel_tol=0, abs_tol=1e-8):
            raise ValueError(f"mapped cell {cell} outer radius differs from JADE coordinate")


def build_manifest(data: bytes, retrieved: str, metadata_data: bytes | None = None,
                   map_data: bytes | None = None,
                   deck_data: bytes | None = None, geometry_data: bytes | None = None,
                   source_data: bytes | None = None) -> dict:
    if len(data) != CSV_BYTES or sha256(data) != CSV_SHA256:
        raise ValueError("CSV bytes do not match the pinned JADE artifact")
    if metadata_data is None or len(metadata_data) != METADATA_BYTES or sha256(metadata_data) != METADATA_SHA256:
        raise ValueError("JADE metadata bytes do not match the pinned source")
    if map_data is None or len(map_data) != MAP_BYTES or sha256(map_data) != MAP_SHA256:
        raise ValueError("JADE cell-map bytes do not match the pinned source")
    if deck_data is None or len(deck_data) != DECK_BYTES or sha256(deck_data) != DECK_SHA256:
        raise ValueError("MCNP deck bytes do not match the pinned source")
    if geometry_data is None or len(geometry_data) != GEOMETRY_BYTES or sha256(geometry_data) != GEOMETRY_SHA256:
        raise ValueError("OpenMC geometry bytes do not match the pinned source")
    if source_data is None or len(source_data) != SOURCE_BYTES or sha256(source_data) != SOURCE_SHA256:
        raise ValueError("OpenMC source implementation bytes do not match the pinned source")
    rows = parse_table(data)
    cell_map = parse_cell_radius_map(map_data)
    tally_ids, scale = parse_tally_cells_and_scale(deck_data)
    if not math.isclose(scale, FM_SCALE, rel_tol=0, abs_tol=1e6):
        raise ValueError("FM204 multiplier differs from the reviewed source-normalisation factor")
    mapped = map_rows_to_cells(rows, cell_map, tally_ids)
    verify_openmc_mapping(mapped, parse_openmc_outer_radii(geometry_data))
    return manifest_from_rows(mapped, retrieved, scale)


def manifest_from_rows(rows: list[dict], retrieved: str, scale: float = FM_SCALE) -> dict:
    """Create a case manifest from already parsed rows (testable without source data)."""
    if len(rows) != 97:
        raise ValueError(f"expected 97 response rows, received {len(rows)}")
    if not math.isfinite(scale) or scale <= 0:
        raise ValueError("source-normalisation factor must be finite and positive")
    if any("cell_id" not in r for r in rows):
        raise ValueError("each row must be mapped to a verified MCNP F4 cell ID")
    detectors = [{
        "id": f"neutron_flux:cell-{r['cell_id']}",
        "response_class": "neutron_flux",
        "reference": {"value": r["value"] / scale, "u": r["u"] / scale},
        "controlling_variable": {"name": "outer radial boundary of tallied cell", "value": r["position_cm"], "unit": "cm"},
        "source_uncertainty": {"kind": "MCNP relative standard error", "value": r["relative_standard_error"]},
        "blocked": {"why": "the pinned JADE coordinate maps to the outer radial boundary of this whole-cell F4 response, but MCNP -6 source-spectrum equivalence to FARIS has not been established; producer executable and processed nuclear-data identity are also absent; no C/E is reported",
                    "next_step": "qualify the MCNP -6 versus OpenMC Gaussian source treatment and verify producer executable/processed-library provenance before review of any unblocking"},
    } for r in rows]
    manifest = {
        "schema": "faris.validation-case/1.0.0",
        "case_id": CASE_ID,
        "title": "ITER_1D total neutron flux, JADE MCNP/FENDL-3.2b reference",
        "evidence_class": "code-to-code",
        "source": {"url": f"https://github.com/JADE-V-V/JADE-RAW-RESULTS/blob/{COMMIT}/{CSV_PATH}",
                   "commit": COMMIT, "retrieved": retrieved,
                   "producer": "JADE 4.1.1.dev256+g735331b3f post-processing; MCNP 6.2; FENDL 3.2b",
                   "producer_provenance_gap": "The published metadata does not bind this output to hashes of the producer's input deck, executable, processed nuclear data, or run settings."},
        "license": {"spdx": "EUPL-1.2", "attribution": "JADE-V-V/JADE-RAW-RESULTS, ITER_1D Total neutron flux.csv and metadata.json, pinned commit " + COMMIT,
                    "terms": LICENSE_URL},
        "files": [
            {"role": "published MCNP total neutron flux responses", "path_in_source": CSV_PATH, "sha256": CSV_SHA256, "bytes": CSV_BYTES, "license": "EUPL-1.2"},
            {"role": "JADE producer metadata", "path_in_source": METADATA_PATH, "sha256": METADATA_SHA256, "bytes": METADATA_BYTES, "license": "EUPL-1.2"},
            {"role": "JADE cell-coordinate map", "source_url": MAP_URL, "commit": JADE_COMMIT, "path_in_source": MAP_PATH, "sha256": MAP_SHA256, "bytes": MAP_BYTES, "license": "GPL-3.0-only", "license_url": JADE_LICENSE_URL},
            {"role": "JADE MCNP F4 tally deck", "source_url": DECK_URL, "commit": DECK_COMMIT, "path_in_source": DECK_PATH, "sha256": DECK_SHA256, "bytes": DECK_BYTES, "license": "GPL-3.0-only", "license_url": JADE_LICENSE_URL},
            {"role": "OpenMC benchmark geometry for cell-boundary cross-check", "source_url": GEOMETRY_URL, "commit": OPENMC_COMMIT, "path_in_source": GEOMETRY_PATH, "sha256": GEOMETRY_SHA256, "bytes": GEOMETRY_BYTES, "license": "CC-BY-4.0", "license_url": OPENMC_LICENSE_URL},
            {"role": "OpenMC benchmark source implementation for spectrum qualification", "source_url": SOURCE_URL, "commit": OPENMC_COMMIT, "path_in_source": SOURCE_PATH, "sha256": SOURCE_SHA256, "bytes": SOURCE_BYTES, "license": "CC-BY-4.0", "license_url": OPENMC_LICENSE_URL},
        ],
        "files_are_external_not_vendored": True,
        "normalisation": {"status": "confirmed", "blocks_scoring": False,
            "quantity": "whole-cell averaged F4 total neutron flux, divided by the explicit FM204 source multiplier",
            "units": "particle/cm2 per source neutron",
            "source_normalisation": f"published tally response divided by FM204 multiplier {scale:g}",
            "location": "97 MCNP tally cells; each CSV coordinate is the mapped cell's outer radial boundary, not a point response location",
            "reaction_or_particle": "neutron total flux", "energy_integration": "total flux tally; all-energy scope as documented by JADE",
            "basis": "JADE computational benchmark documentation and pinned result metadata; see docs/ITER_1D_REFERENCE.md"},
        "uncertainty": {"components": [{"name": "MCNP Monte Carlo sampling", "kind": "statistical", "source_field": "CSV Error (relative standard error)"}],
                        "note": "Reference standard uncertainty is value multiplied by the published relative Error. No model, nuclear-data, or experimental uncertainty is supplied or inferred."},
        "compatibility": {"declared": RULE_DECLARED, "k": 2.0, "k_basis": "authored: coverage factor 2 on combined standard uncertainty",
                          "covariance": {"kind": "unknown", "basis": "FARIS and JADE MCNP both use evaluated nuclear data and share benchmark geometry/source; cross-code covariance is not provided."}},
        "response_classes": {"neutron_flux": {"unit": "particle/cm2 per source neutron", "description": "Published JADE ITER_1D whole-cell F4 total neutron flux; mapped by cell ID and normalized by FM204."}},
        "detectors": detectors,
        "not_covered": ["experimental accuracy: this is code-to-code comparison and cannot establish physical validation",
            "neutron absorption, photon transport, heating, or activation; the published table contains only total neutron flux",
            "producer input/executable/processed-library identity and run settings, absent from the published result metadata",
            "equality of producer FENDL 3.2b data to the local OpenMC processed library, not established by the result metadata",
            "source-spectrum equivalence between MCNP -6 and the FARIS OpenMC Gaussian implementation, not established",
            "producer executable and processed-library hashes/settings, absent from published metadata"],
        "qualified_range": {"materials": ["ITER_1D benchmark materials; producer metadata identifies FENDL 3.2b"],
            "geometry_class": "ITER_1D one-dimensional cylindrical benchmark; JADE response rows mapped to MCNP cell IDs by pinned configuration and deck",
            "spectrum_class": "ITER_1D DT source; MCNP deck uses -6 Muir while FARIS worker source is Gaussian; equivalence requires qualification",
            "parameters": [], "cooling_time": "not applicable (prompt transport)"},
        "literature_context": [],
    }
    return check_manifest(manifest)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--csv", type=Path, required=True, help="external CSV path; downloaded source table is not vendored")
    ap.add_argument("--fetch", action="store_true", help="download the immutable pinned source to --csv")
    ap.add_argument("--out", type=Path, required=True, help="write manifest outside the Git checkout")
    ap.add_argument("--retrieved", required=True, help="retrieval date YYYY-MM-DD")
    args = ap.parse_args()
    repository = REPO_ROOT
    metadata_path = args.csv.with_name("metadata.json")
    map_path = args.csv.with_name("cell-radius-map.yaml")
    deck_path = args.csv.with_name("ITER_1D.i")
    geometry_path = args.csv.with_name("openmc-geometry.xml")
    source_path = args.csv.with_name("iter_1d_source.cpp")
    for label, path in (("CSV", args.csv), ("metadata", metadata_path), ("JADE map", map_path), ("MCNP deck", deck_path), ("OpenMC geometry", geometry_path), ("OpenMC source", source_path), ("manifest", args.out)):
        if path.expanduser().resolve().is_relative_to(repository):
            ap.error(f"{label} output/input must stay outside the Git checkout to avoid redistributing JADE result data")
    if args.fetch:
        fetch(CSV_URL, CSV_SHA256, args.csv, CSV_BYTES)
        fetch(METADATA_URL, METADATA_SHA256, metadata_path, METADATA_BYTES)
        fetch(MAP_URL, MAP_SHA256, map_path, MAP_BYTES)
        fetch(DECK_URL, DECK_SHA256, deck_path, DECK_BYTES)
        fetch(GEOMETRY_URL, GEOMETRY_SHA256, geometry_path, GEOMETRY_BYTES)
        fetch(SOURCE_URL, SOURCE_SHA256, source_path, SOURCE_BYTES)
    data = args.csv.read_bytes()
    manifest = build_manifest(data, args.retrieved, metadata_path.read_bytes(), map_path.read_bytes(),
                              deck_path.read_bytes(), geometry_path.read_bytes(), source_path.read_bytes())
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out} ({len(manifest['detectors'])} responses; reference data remain external)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
