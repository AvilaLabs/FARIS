# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Build and audit a separate, explicitly unqualified R3 CE review index.

This driver does not promote a review composite to a production data basis.
It preserves the downloaded indexes and HDF5 files, adds only the four known
gaps, checks every chain-reachable nuclide/reaction in the requested 294 K
library, and records the evaluation/processing mixture as a confound for D1S
comparisons. The full HDF5 audit is an explicit command because it hashes and
opens every chain-required data file.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import tempfile
import xml.etree.ElementTree as ET

import r3_activation_coverage as AC

SCHEMA = "faris-r3-review-composite/v1"
PLAN_SCHEMA = "faris-r3-review-composite-plan/v1"
AUDIT_SCHEMA = "faris-r3-review-composite-audit/v1"
MIN_ENERGY_EV = 20.0e6  # Existing R3 TF fast-flux response upper boundary.
TEMPERATURE = "294K"  # Existing RM-M OpenMC nearest-temperature setting.
TENDL_INDEX_SHA256 = "317b9d1674a78077a454eae8cde7212561ac46a96f65e66c2d4eca00b141ac94"
TENDL_ARCHIVE_SHA256 = "c35e1b26fa3c2f86d9eebc1767abac3bf82017354ccf11db347c6711b55aa1b3"
FENDL_H_SHA256 = {
    "H1": "2bf2db322a188e5c435675cd732dd36b10b995e8cc6e7d6bd55d6aaf712f9b4d",
    "H2": "57b09c44bfd8a696e46e864dce689afa380555c3a35841c1da92dbc009ce5890",
}
PILOT_NUCLIDES = ("Ne24", "S42")


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def _json_bytes(value: dict) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode()


def _atomic_new(path: Path, payload: bytes) -> None:
    """Publish a fresh file without replacing an earlier review artifact."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(tmp, path)
    except FileExistsError as exc:
        raise ValueError(f"refusing to overwrite existing review artifact: {path}") from exc
    finally:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass


def _load_json(path: Path) -> dict:
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected a JSON object: {path}")
    return value


def _resolve_index_path(index_path: Path, raw_path: str, directory: Path | None = None) -> Path:
    path = Path(raw_path)
    if not path.is_absolute():
        base = directory or index_path.parent
        path = base / path
    return path.resolve()


def _entry_map(index_path: Path) -> tuple[ET.Element, dict[str, tuple[ET.Element, Path]]]:
    root = ET.parse(index_path).getroot()
    directory_text = (root.findtext("directory") or "").strip()
    directory = Path(directory_text) if directory_text else index_path.parent
    if not directory.is_absolute():
        directory = index_path.parent / directory
    directory = directory.resolve()
    entries: dict[str, tuple[ET.Element, Path]] = {}
    for element in root.findall("library"):
        if element.get("type") != "neutron":
            continue
        names = (element.get("materials") or element.get("name") or "").split()
        raw = element.get("path")
        if not names or not raw:
            raise ValueError(f"malformed neutron library entry in {index_path}")
        resolved = _resolve_index_path(index_path, raw, directory)
        for name in names:
            if name in entries:
                raise ValueError(f"duplicate neutron index entry for {name}: {index_path}")
            entries[name] = (element, resolved)
    return root, entries


def _chain_requirements(chain_xml: Path, material_roots, reaction_mt: dict[str, int]) -> dict:
    nuclides, adjacency = AC._chain_graph(Path(chain_xml))
    roots = sorted(set(map(str, material_roots)))
    if not roots:
        raise ValueError("review audit needs exact material roots from the serialized R3 materials")
    absent_roots = sorted(set(roots) - set(nuclides))
    if absent_roots:
        raise ValueError(f"material roots are absent from the bound depletion chain: {absent_roots[:20]}")
    reachable = set(roots)
    pending = list(roots)
    while pending:
        parent = pending.pop()
        for child in adjacency[parent] - reachable:
            reachable.add(child)
            pending.append(child)
    required = sorted(set(roots) | {name for name in reachable if nuclides[name]})
    channels = {}
    unmapped = []
    for name in sorted(reachable):
        for reaction in sorted(nuclides[name]):
            mt = reaction_mt.get(reaction)
            if mt is None:
                unmapped.append({"nuclide": name, "reaction": reaction})
            else:
                channels.setdefault(name, []).append({"reaction": reaction, "mt": int(mt)})
    return {"roots": roots, "reachable": sorted(reachable), "required_nuclides": required,
            "channels_by_nuclide": channels, "unmapped_chain_reactions": unmapped}


def _validate_pilot(nuclide: str, pilot_dir: Path) -> dict:
    receipt_path, audit_path = pilot_dir / "receipt.json", pilot_dir / "audit.json"
    receipt, audit = _load_json(receipt_path), _load_json(audit_path)
    if receipt.get("nuclide") != nuclide or receipt.get("status") != "CONVERTED_UNQUALIFIED":
        raise ValueError(f"{nuclide} conversion receipt is not the expected unqualified pilot")
    if audit.get("status") != "STRUCTURAL_AND_DESCRIPTIVE_AUDIT_PASS":
        raise ValueError(f"{nuclide} structural/descriptive audit did not pass")
    if audit.get("qualification", "").startswith("not a physical validation") is False:
        raise ValueError(f"{nuclide} audit qualification limit is missing")
    source = Path(receipt["source_file"]).resolve()
    hdf = Path(receipt["hdf5"]).resolve()
    njoy = Path(receipt["njoy"]).resolve()
    ctest_log = njoy.parent / "Testing/Temporary/LastTest.log"
    for label, path, expected in (
        ("source", source, receipt.get("source_sha256")),
        ("HDF5", hdf, receipt.get("hdf5_sha256")),
        ("NJOY executable", njoy, receipt.get("njoy_sha256")),
    ):
        if not expected or not path.is_file() or sha256_file(path) != expected:
            raise ValueError(f"{nuclide} {label} provenance hash/path check failed")
    if not ctest_log.is_file():
        raise ValueError(f"{nuclide} pilot lacks the pinned NJOY CTest log")
    ctest_text = ctest_log.read_text(encoding="utf-8", errors="replace")
    ctest_passes = len(re.findall(r"(?m)^Test Passed\.$", ctest_text))
    if ctest_passes != 13 or "Test Failed." in ctest_text or "End testing:" not in ctest_text:
        raise ValueError(f"{nuclide} NJOY CTest receipt does not show all 13 selected tests passing")
    if audit.get("source_sha256") != receipt.get("source_sha256") or \
            audit.get("hdf5_sha256") != receipt.get("hdf5_sha256"):
        raise ValueError(f"{nuclide} structural audit does not bind the receipt source and output hashes")
    if receipt.get("openmc_version") != "0.15.3":
        raise ValueError(f"{nuclide} pilot was not serialized with the pinned OpenMC 0.15.3")
    import h5py
    audited_mts = {int(row["mt"]) for row in audit.get("xs_ranges", [])}
    receipt_mts = set(map(int, receipt.get("mts", [])))
    with h5py.File(hdf, "r") as handle:
        reaction_group = handle.get(f"{nuclide}/reactions")
        if reaction_group is None:
            raise ValueError(f"{nuclide} pilot HDF5 has no reaction groups")
        hdf_mts = {int(group.attrs["mt"]) for group in reaction_group.values()
                   if "mt" in group.attrs}
    # ACE audit receipts include total and absorption summary MTs 1 and 101;
    # OpenMC stores those as derived totals rather than standalone reaction groups.
    if not audited_mts or audited_mts != hdf_mts or receipt_mts - hdf_mts != {1, 101}:
        raise ValueError(f"{nuclide} HDF5 channels, structural audit, and ACE receipt MT sets disagree")
    energy_summary = audit.get("energy_max_ev", {}).get(TEMPERATURE)
    if energy_summary is None or float(energy_summary) < MIN_ENERGY_EV:
        raise ValueError(f"{nuclide} structural audit does not verify the required 294 K energy extent")
    if TEMPERATURE not in receipt.get("temperatures_K", []):
        raise ValueError(f"{nuclide} pilot receipt lacks required {TEMPERATURE} data")
    if float(receipt.get("max_energy_eV", 0.0)) < MIN_ENERGY_EV:
        raise ValueError(f"{nuclide} pilot receipt does not reach the R3 {MIN_ENERGY_EV:g} eV boundary")
    return {
        "nuclide": nuclide, "kind": "TENDL-2025 source processed by isolated NJOY DMIN pilot",
        "receipt_path": str(receipt_path), "receipt_sha256": sha256_file(receipt_path),
        "audit_path": str(audit_path), "audit_sha256": sha256_file(audit_path),
        "source_path": str(source), "source_sha256": receipt["source_sha256"],
        "source_bytes": receipt.get("source_bytes"), "njoy_path": str(njoy),
        "njoy_sha256": receipt["njoy_sha256"], "openmc_version": receipt.get("openmc_version"),
        "njoy_ctest_log_path": str(ctest_log.resolve()), "njoy_ctest_log_sha256": sha256_file(ctest_log),
        "njoy_ctest_selected_tests_passed": ctest_passes,
        "hdf5_path": str(hdf), "hdf5_sha256": receipt["hdf5_sha256"],
        "temperature_labels": receipt["temperatures_K"], "max_energy_eV": receipt["max_energy_eV"],
        "reaction_mts": sorted(map(int, receipt.get("mts", []))),
        "structural_audit_status": audit["status"],
        "audit_qualification": audit["qualification"],
        "data_quality": "UNQUALIFIED; structural/descriptive processing audit only",
    }


def build_review_index(*, candidate_index: Path, candidate_receipt: Path,
                       fendl_index: Path, pilot_roots: dict[str, Path],
                       output_dir: Path) -> dict:
    """Build a new four-override index while leaving all inputs byte-identical."""
    candidate_index, candidate_receipt = Path(candidate_index).resolve(), Path(candidate_receipt).resolve()
    fendl_index, output_dir = Path(fendl_index).resolve(), Path(output_dir).resolve()
    if output_dir.exists():
        raise ValueError(f"review output directory already exists: {output_dir}")
    candidate_claim = _load_json(candidate_receipt)
    if candidate_claim.get("sha256") != TENDL_ARCHIVE_SHA256 or \
            candidate_claim.get("zenodo_record") != 21534376:
        raise ValueError("TENDL candidate archive provenance differs from the pinned download receipt")
    candidate_index_hash = sha256_file(candidate_index)
    if candidate_index_hash != TENDL_INDEX_SHA256:
        raise ValueError("TENDL candidate cross_sections.xml differs from the pinned review copy")
    candidate_root, entries = _entry_map(candidate_index)
    fendl_root, fendl_entries = _entry_map(fendl_index)
    for name, expected in FENDL_H_SHA256.items():
        if name not in fendl_entries:
            raise ValueError(f"actual combined FENDL index lacks {name}")
        actual = fendl_entries[name][1]
        if not actual.is_file() or sha256_file(actual) != expected:
            raise ValueError(f"FENDL {name} path does not match the pinned FENDL-3.2 HDF5 identity")
    expected_missing = set(candidate_claim.get("known_missing", []))
    needed = {"H1", "H2", *PILOT_NUCLIDES}
    if not needed.issubset(expected_missing):
        raise ValueError("candidate download receipt no longer identifies all four expected gaps")
    if needed & set(entries):
        raise ValueError("one or more explicit review overrides collide with candidate index entries")

    supplements = {
        "H1": {"path": fendl_entries["H1"][1], "kind": "FENDL-3.2 H1/H2 source data", "sha256": FENDL_H_SHA256["H1"]},
        "H2": {"path": fendl_entries["H2"][1], "kind": "FENDL-3.2 H1/H2 source data", "sha256": FENDL_H_SHA256["H2"]},
    }
    pilot_provenance = {}
    for name in PILOT_NUCLIDES:
        pilot = _validate_pilot(name, Path(pilot_roots[name]).resolve())
        pilot_provenance[name] = pilot
        supplements[name] = {"path": Path(pilot["hdf5_path"]), "kind": pilot["kind"],
                             "sha256": pilot["hdf5_sha256"]}

    # New review output; candidate and FENDL source index bytes are not touched.
    output_dir.mkdir(parents=True)
    out_index = output_dir / "cross_sections.review.xml"
    root = ET.Element(candidate_root.tag, dict(candidate_root.attrib))
    directory = ET.SubElement(root, "directory")
    directory.text = str(candidate_index.parent)
    ordered = []
    for name, (element, path) in entries.items():
        attrs = dict(element.attrib)
        attrs["path"] = str(path)
        ordered.append((name, attrs))
    for name, spec in supplements.items():
        ordered.append((name, {"materials": name, "path": str(spec["path"]), "type": "neutron"}))
    for _, attrs in sorted(ordered, key=lambda item: item[0]):
        ET.SubElement(root, "library", attrs)
    xml_bytes = ET.tostring(root, encoding="utf-8", xml_declaration=True, short_empty_elements=True)
    _atomic_new(out_index, xml_bytes + b"\n")

    # Validate one exact mapping per nuclide; no best-effort aliasing or overwrite.
    composite_root, composite_entries = _entry_map(out_index)
    if set(composite_entries) != set(entries) | needed:
        raise ValueError("constructed review index changed the set of candidate or explicit supplemental nuclides")
    for name, (_, original_path) in entries.items():
        if composite_entries[name][1] != original_path:
            raise ValueError(f"candidate material mapping changed while composing review index: {name}")
    for name, spec in supplements.items():
        if composite_entries[name][1] != Path(spec["path"]).resolve():
            raise ValueError(f"supplemental mapping changed while composing review index: {name}")

    report = {
        "schema": SCHEMA,
        "status": "REVIEW_COMPOSITE_UNQUALIFIED",
        "transport_eligible": False,
        "index_path": str(out_index), "index_sha256": sha256_file(out_index),
        "index_bytes": out_index.stat().st_size,
        "inputs": {
            "tendl_archive_receipt": {"path": str(candidate_receipt), "sha256": sha256_file(candidate_receipt),
                                      "claimed_archive_sha256": candidate_claim["sha256"],
                                      "archive_hash_verification": "receipt claim only; archive not rehashed by prepare; required extracted HDF5 files are hashed during audit",
                                      "license_claim": candidate_claim.get("license"),
                                      "creator_claim": candidate_claim.get("provider_creator_claim")},
            "tendl_original_index": {"path": str(candidate_index), "sha256": candidate_index_hash,
                                     "entry_count": len(entries)},
            "fendl_original_index": {"path": str(fendl_index), "sha256": sha256_file(fendl_index),
                                     "h1_h2_source": "FENDL-3.2 local HDF5; hashes pinned to the actual indexed files"},
        },
        "supplemental_overrides": {
            "H1": {"source": "FENDL-3.2 actual HDF5", "path": str(supplements["H1"]["path"]),
                   "sha256": supplements["H1"]["sha256"],
                   "evaluation_limit": "FENDL evaluation; source identity is exact, evaluation is not matched to TENDL",
                   "processing_limit": "local source metadata does not identify an independently reproducible processor",
                   "reuse_limit": "no redistribution or license permission is inferred"},
            "H2": {"source": "FENDL-3.2 actual HDF5", "path": str(supplements["H2"]["path"]),
                   "sha256": supplements["H2"]["sha256"],
                   "evaluation_limit": "FENDL evaluation; source identity is exact, evaluation is not matched to TENDL",
                   "processing_limit": "local source metadata does not identify an independently reproducible processor",
                   "reuse_limit": "no redistribution or license permission is inferred"},
            **pilot_provenance,
        },
        "evaluation_comparison": {
            "status": "UNMATCHED_TO_R3_D1S_BASIS",
            "explanation": "The R3 D1S basis remains the project combined FENDL-3.2/ENDF-B-VII.1 index. This review composite uses TENDL-2025 for its candidate entries, FENDL-3.2 H1/H2 overrides, and TENDL Ne24/S42 pilot conversions. It cannot support a matched-data VAL043 claim.",
        },
        "limits": [
            "This review composite never yields READY_FOR_R2S_TRANSPORT and is not wired into the production R3 gate.",
            "TENDL download receipt identifies an archive/provider claim; it does not establish per-file evaluation processing provenance or full-library qualification.",
            "Ne24/S42 pilots passed structural/descriptive audits only; their NJOY DMIN production path is explicitly distinct from the archive HDF5 conversion path.",
            "HDF5 identities establish exact bytes only. This composite has mixed evaluation provenance and remains scientifically unqualified.",
        ],
    }
    plan = {"schema": PLAN_SCHEMA, "index_review": report, "created_for": "offline full-chain CE presence audit only"}
    _atomic_new(output_dir / "review-plan.json", _json_bytes(plan))
    return plan


def audit_review_index(*, review_index: Path, chain_xml: Path, materials_xml: Path,
                       reaction_mt: dict[str, int], output_path: Path,
                       minimum_energy_ev: float = MIN_ENERGY_EV,
                       temperature: str = TEMPERATURE) -> dict:
    """Hash and structurally inspect all chain-required HDF5 channels once each."""
    import h5py
    import numpy as np

    review_index, chain_xml, materials_xml = map(Path, (review_index, chain_xml, materials_xml))
    if output_path.exists():
        raise ValueError(f"refusing to overwrite existing review audit: {output_path}")
    chain_hash_before = sha256_file(chain_xml)
    materials_hash_before = sha256_file(materials_xml)
    index_hash_before = sha256_file(review_index)
    roots = AC.actual_material_nuclides(materials_xml)
    req = _chain_requirements(chain_xml, roots, reaction_mt)
    index_root, indexed = _entry_map(review_index)
    missing_entries = sorted(set(req["required_nuclides"]) - set(indexed))
    file_results = []
    missing_channels, malformed = [], []
    for name in req["required_nuclides"]:
        entry = indexed.get(name)
        if entry is None:
            continue
        _, path = entry
        if not path.is_file():
            malformed.append({"nuclide": name, "reason": "indexed HDF5 file is absent", "path": str(path)})
            continue
        digest = sha256_file(path)
        stat_before = path.stat()
        row = {"nuclide": name, "path": str(path), "sha256": digest, "bytes": path.stat().st_size,
               "required_temperature": temperature, "minimum_required_energy_eV": minimum_energy_ev}
        try:
            with h5py.File(path, "r") as handle:
                filetype = handle.attrs.get("filetype")
                if isinstance(filetype, bytes):
                    filetype = filetype.decode("ascii", errors="replace")
                version = np.asarray(handle.attrs.get("version", []), dtype=np.int64).ravel()
                if filetype != "data_neutron" or version.size < 1 or version[0] != 3 or name not in handle:
                    raise ValueError("wrong OpenMC neutron HDF5 filetype/version or isotope root group missing")
                row["openmc_data_hdf_version"] = version.tolist()
                group = handle[name]
                temperatures = set(group.get("energy", {}).keys()) if "energy" in group else set()
                row["available_temperatures"] = sorted(temperatures)
                grid_size = None
                if temperature not in temperatures:
                    row["energy_coverage"] = "MISSING_REQUIRED_TEMPERATURE"
                    malformed.append({"nuclide": name, "reason": f"energy grid lacks {temperature}"})
                else:
                    grid = np.asarray(group["energy"][temperature][()], dtype=np.float64)
                    if grid.ndim != 1 or grid.size < 2 or not np.all(np.isfinite(grid)) or np.any(np.diff(grid) <= 0.0):
                        raise ValueError(f"{temperature} energy grid is not finite and strictly increasing")
                    row["energy_min_eV"], row["energy_max_eV"] = float(grid[0]), float(grid[-1])
                    grid_size = int(grid.size)
                    row["energy_coverage"] = "PASS" if grid[-1] >= minimum_energy_ev else "INSUFFICIENT_MAX_ENERGY"
                    if grid[-1] < minimum_energy_ev:
                        malformed.append({"nuclide": name, "reason": f"energy grid ends below {minimum_energy_ev:g} eV"})
                reactions = group.get("reactions")
                if reactions is None:
                    raise ValueError("nuclide has no reactions group")
                channel_rows = []
                for channel in req["channels_by_nuclide"].get(name, []):
                    mt = channel["mt"]
                    tag = f"reaction_{mt:03d}"
                    reaction = reactions.get(tag)
                    if reaction is None:
                        missing_channels.append({"nuclide": name, "reaction": channel["reaction"], "mt": mt,
                                                 "reason": "reaction group absent"})
                        continue
                    if int(reaction.attrs.get("mt", -1)) != mt:
                        missing_channels.append({"nuclide": name, "reaction": channel["reaction"], "mt": mt,
                                                 "reason": "reaction group MT attribute disagrees"})
                        continue
                    temp_group = reaction.get(temperature)
                    xs = temp_group.get("xs") if temp_group is not None else None
                    if xs is None:
                        missing_channels.append({"nuclide": name, "reaction": channel["reaction"], "mt": mt,
                                                 "reason": f"{temperature} cross-section dataset absent"})
                        continue
                    values = np.asarray(xs[()], dtype=np.float64)
                    if values.ndim != 1 or values.size == 0 or (grid_size is not None and values.size > grid_size) or \
                            not np.all(np.isfinite(values)) or np.any(values < 0.0):
                        missing_channels.append({"nuclide": name, "reaction": channel["reaction"], "mt": mt,
                                                 "reason": "cross-section values have invalid length or are not finite nonnegative 1-D data"})
                        continue
                    channel_rows.append({"reaction": channel["reaction"], "mt": mt, "temperature": temperature,
                                         "points": int(values.size)})
                row["required_reaction_channels"] = channel_rows
                row["required_reaction_channel_count"] = len(channel_rows)
        except (OSError, KeyError, TypeError, ValueError) as exc:
            malformed.append({"nuclide": name, "reason": f"HDF5 structural inspection failed: {type(exc).__name__}: {exc}"})
        if sha256_file(path) != digest:
            malformed.append({"nuclide": name, "reason": "indexed HDF5 bytes changed during audit"})
        stat_after = path.stat()
        if stat_after.st_size != stat_before.st_size:
            malformed.append({"nuclide": name, "reason": "indexed HDF5 size changed during audit"})
        file_results.append(row)

    if sha256_file(chain_xml) != chain_hash_before or sha256_file(materials_xml) != materials_hash_before or \
            sha256_file(review_index) != index_hash_before:
        raise ValueError("chain, serialized materials, or review index changed during CE coverage audit")
    complete = not (missing_entries or missing_channels or malformed or req["unmapped_chain_reactions"])
    report = {
        "schema": AUDIT_SCHEMA,
        "status": "STRUCTURAL_CE_PRESENCE_PASS_UNQUALIFIED" if complete else "INCOMPLETE",
        "transport_eligible": False,
        "chain_path": str(chain_xml.resolve()), "chain_sha256": chain_hash_before,
        "materials_path": str(materials_xml.resolve()), "materials_sha256": materials_hash_before,
        "index_path": str(review_index.resolve()), "index_sha256": index_hash_before,
        "material_root_nuclides": req["roots"], "reachable_nuclide_count": len(req["reachable"]),
        "required_nuclides": req["required_nuclides"],
        "required_reaction_channel_count": sum(map(len, req["channels_by_nuclide"].values())),
        "required_temperature": temperature, "minimum_required_energy_eV": minimum_energy_ev,
        "reviewed_file_count": len(file_results), "reviewed_files": file_results,
        "missing_index_entries": missing_entries, "missing_reaction_channels": missing_channels,
        "malformed_or_insufficient_files": malformed,
        "unmapped_chain_reactions": req["unmapped_chain_reactions"],
        "qualification": "Presence, temperature, energy extent, and structural checks only. This does not qualify evaluation quality, source processing equivalence, covariance, URR treatment, depletion data, or VAL043.",
        "evaluation_comparison_status": "UNMATCHED_TO_R3_D1S_BASIS",
    }
    _atomic_new(output_path, _json_bytes(report))
    return report


def _default_paths() -> dict[str, Path]:
    root = Path(__file__).resolve().parents[3]
    return {
        "root": root,
        "candidate_index": root / "data/raw/tendl-2025-openmc-candidate/hdf5/cross_sections.xml",
        "candidate_receipt": root / "data/raw/tendl-2025-openmc-candidate/download-receipt.json",
        "fendl_index": root / "data/raw/combined-fendl32-endfbvii1/cross_sections.xml",
        "chain": Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml",
        "materials": root / "runs/continuation/s0g-rm-m/materials.xml",
        "pilots": root / "runs/continuation",
    }


def main(argv=None) -> int:
    defaults = _default_paths()
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepare = sub.add_parser("prepare", help="write new review index and provenance plan; no full HDF5 scan")
    prepare.add_argument("--out-dir", type=Path, required=True)
    prepare.add_argument("--candidate-index", type=Path, default=defaults["candidate_index"])
    prepare.add_argument("--candidate-receipt", type=Path, default=defaults["candidate_receipt"])
    prepare.add_argument("--fendl-index", type=Path, default=defaults["fendl_index"])
    prepare.add_argument("--pilot-root", type=Path, default=defaults["pilots"])
    audit = sub.add_parser("audit", help="hash and inspect every required HDF5 file and reaction channel")
    audit.add_argument("--review-index", type=Path, required=True)
    audit.add_argument("--chain", type=Path, default=defaults["chain"])
    audit.add_argument("--materials", type=Path, default=defaults["materials"])
    audit.add_argument("--out", type=Path, required=True)
    audit.add_argument("--minimum-energy-ev", type=float, default=MIN_ENERGY_EV)
    audit.add_argument("--temperature", default=TEMPERATURE)
    args = parser.parse_args(argv)
    if args.command == "prepare":
        plan = build_review_index(
            candidate_index=args.candidate_index, candidate_receipt=args.candidate_receipt,
            fendl_index=args.fendl_index,
            pilot_roots={name: args.pilot_root / f"tendl-gap-{name}-dminpilot-20261109" for name in PILOT_NUCLIDES},
            output_dir=args.out_dir)
        print(json.dumps({"status": plan["index_review"]["status"],
                          "index": plan["index_review"]["index_path"],
                          "plan": str(args.out_dir / "review-plan.json")}, indent=2))
        return 0
    if not math.isfinite(args.minimum_energy_ev) or args.minimum_energy_ev <= 0.0:
        parser.error("--minimum-energy-ev must be finite and positive")
    try:
        from openmc.data import REACTION_MT
    except ImportError as exc:
        parser.error(f"audit requires the pinned OpenMC Python environment for REACTION_MT: {exc}")
    report = audit_review_index(review_index=args.review_index, chain_xml=args.chain,
                                materials_xml=args.materials, reaction_mt=REACTION_MT,
                                output_path=args.out, minimum_energy_ev=args.minimum_energy_ev,
                                temperature=args.temperature)
    print(json.dumps({"status": report["status"], "transport_eligible": False,
                      "reviewed_files": report["reviewed_file_count"],
                      "channels": report["required_reaction_channel_count"],
                      "report": str(args.out)}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
