#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Build a confined FENDL-neutron + validated ENDF photon data overlay."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import xml.etree.ElementTree as ET
from typing import Any

SCHEMA = "faris-fendl32-endfbvii1-overlay/v0.1"
ELEMENTS = ("H", "Li", "Be", "F", "Ti", "Fe", "Cu", "W")
MAX_DATA_FILES = 64
MAX_ONE_FILE = 256 * 1024 * 1024
MAX_TOTAL_BYTES = 4 * 1024 * 1024 * 1024


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def assemble(source_xml: Path, audit_path: Path, photon_dir: Path, output: Path) -> dict[str, Any]:
    source_xml = source_xml.expanduser().resolve(strict=True)
    audit_path = audit_path.expanduser().resolve(strict=True)
    photon_dir = photon_dir.expanduser().resolve(strict=True)
    audit = json.loads(audit_path.read_text(encoding="utf-8"))
    converted = json.loads((photon_dir / "conversion-provenance.json").read_text(encoding="utf-8"))
    if converted.get("schema_version") != "faris-endfb-vii1-photon-conversion/v0.1":
        raise ValueError("unsupported photon conversion provenance schema")
    if converted.get("openmc_python_api_version") != "0.15.3":
        raise ValueError("photon conversion must identify OpenMC 0.15.3")
    if output.exists():
        raise ValueError("output directory must be new and exclusive")

    source_root = source_xml.parent.resolve(strict=True)
    source_tree = ET.parse(source_xml).getroot()
    directory = source_tree.findtext("directory")
    if directory:
        library_root = Path(directory).expanduser()
        if not library_root.is_absolute():
            library_root = source_root / library_root
        library_root = library_root.resolve(strict=True)
    else:
        library_root = source_root
    neutron_map: dict[str, Path] = {}
    for item in source_tree.findall("library"):
        if item.get("type") != "neutron":
            continue
        rel = item.get("path")
        if not rel:
            continue
        source_file = Path(rel)
        candidate = source_file if source_file.is_absolute() else library_root / source_file
        resolved = candidate.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError(f"not a regular neutron HDF5 file: {candidate}")
        for name in item.get("materials", "").split():
            if name in neutron_map and neutron_map[name] != resolved:
                raise ValueError(f"input XML declares multiple neutron libraries for {name}")
            neutron_map[name] = resolved

    expected_nuclides = audit.get("neutron_library", {})
    selected = sorted(name for name, item in expected_nuclides.items()
                      if item.get("library_entry_present") and item.get("sha256"))
    if not selected or len(selected) > MAX_DATA_FILES or any(name not in neutron_map for name in selected):
        raise ValueError("source FENDL XML does not provide all bounded, audited FARIS nuclides")
    photon_records = {item.get("element"): item for item in converted.get("files", [])}
    if set(photon_records) != set(ELEMENTS):
        raise ValueError("conversion provenance must contain exactly the eight required photon elements")
    for element, record in photon_records.items():
        source = (photon_dir / record["relative_path"]).resolve(strict=True)
        if not source.is_file() or not source.is_relative_to(photon_dir):
            raise ValueError(f"converted photon file is unsafe or missing: {record['relative_path']}")
        if sha256(source) != record.get("sha256"):
            raise ValueError(f"converted photon HDF5 hash mismatch: {element}")

    output = output.absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.assembling-", dir=output.parent))
    try:
        neutron_out = stage / "neutron"
        photon_out = stage / "photon"
        neutron_out.mkdir()
        photon_out.mkdir()
        inventory = []
        total_bytes = 0
        for name in selected:
            source = neutron_map[name]
            expected_hash = expected_nuclides[name]["sha256"]
            source_hash = sha256(source)
            if source_hash != expected_hash:
                raise ValueError(f"selected FENDL data bytes do not match the existing FARIS audit for {name}")
            size = source.stat().st_size
            total_bytes += size
            if size > MAX_ONE_FILE or total_bytes > MAX_TOTAL_BYTES:
                raise ValueError("selected nuclear-data files exceed declared copy bounds")
            relative = f"neutron/{name}.h5"
            target = stage / relative
            shutil.copyfile(source, target)
            if sha256(target) != expected_hash:
                raise ValueError(f"copied FENDL file digest changed for {name}")
            inventory.append({"kind": "neutron", "material": name, "relative_path": relative,
                              "bytes": size, "sha256": expected_hash})
        if len(inventory) + len(ELEMENTS) > MAX_DATA_FILES:
            raise ValueError("overlay exceeds the file-count bound")
        for element in ELEMENTS:
            source = photon_dir / photon_records[element]["relative_path"]
            target = photon_out / f"{element}.h5"
            shutil.copyfile(source, target)
            inventory.append({"kind": "photon", "element": element,
                              "relative_path": f"photon/{element}.h5",
                              "bytes": target.stat().st_size, "sha256": sha256(target)})

        root = ET.Element("cross_sections")
        for item in inventory:
            material = item["material"] if item["kind"] == "neutron" else item["element"]
            ET.SubElement(root, "library", {
                "materials": material,
                "path": item["relative_path"],
                "type": item["kind"],
            })
        ET.indent(root, space="  ")
        xml_path = stage / "cross_sections.xml"
        ET.ElementTree(root).write(xml_path, encoding="utf-8", xml_declaration=True)
        provenance = {
            "schema_version": SCHEMA,
            "source_neutron_library": {
                "cross_sections_xml_sha256": sha256(source_xml),
                "publisher_or_distribution_claim": audit.get("provenance_and_license", {}).get("upstream_release_assertion"),
                "publisher_checksum": None,
                "provenance_status": "local source XML selected by operator; no acquisition receipt or publisher checksum is claimed",
            },
            "source_photon_conversion_sha256": sha256(photon_dir / "conversion-provenance.json"),
            "audit_used_to_pin_neutron_file_hashes_sha256": sha256(audit_path),
            "cross_sections_xml": {"relative_path": "cross_sections.xml", "sha256": sha256(xml_path)},
            "files": inventory,
            "notice": "This overlay reproduces exact identified local file bytes; it does not authenticate the FENDL acquisition or qualify the evaluations for reactor predictions.",
        }
        (stage / "provenance.json").write_text(json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        if output.exists():
            raise ValueError("output directory appeared while overlay was being assembled")
        stage.rename(output)
        return provenance
    except Exception:
        failed = output.with_name(f"{output.name}.assembly-incomplete")
        if not failed.exists():
            stage.rename(failed)
        else:
            shutil.rmtree(stage)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--neutron-cross-sections", required=True, type=Path)
    parser.add_argument("--audit", required=True, type=Path)
    parser.add_argument("--photon-conversion", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = assemble(args.neutron_cross_sections, args.audit, args.photon_conversion, args.output_dir)
    except Exception as exc:
        parser.exit(2, f"overlay assembly error: {type(exc).__name__}: {exc}\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
