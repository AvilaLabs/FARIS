#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Convert the cited NNDC ENDF/B-VII.1 photon archives to OpenMC HDF5.

Only the eight photoatomic/atomic-relaxation element pairs used by the current
FARIS reference materials are extracted. Nuclear-data files remain local and
are never included in the source repository.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import zipfile
from typing import Any

SCHEMA = "faris-endfb-vii1-photon-conversion/v0.1"
EXPECTED_MD5 = {
    "photoatomic": "5192f94e61f0b385cf536f448ffab4a4",
    "atomic_relaxation": "fddb6035e7f2b6931e51a58fc754bd10",
}
ELEMENTS = {
    "H": 1,
    "Li": 3,
    "Be": 4,
    "F": 9,
    "Ti": 22,
    "Fe": 26,
    "Cu": 29,
    "W": 74,
}
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024
MAX_MEMBER_BYTES = 8 * 1024 * 1024


def digest(path: Path, algorithm: str) -> str:
    hasher = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def read_member(archive: Path, member_name: str) -> bytes:
    with zipfile.ZipFile(archive) as bundle:
        try:
            info = bundle.getinfo(member_name)
        except KeyError as exc:
            raise ValueError(f"official archive lacks expected member {member_name}") from exc
        if info.is_dir() or info.file_size <= 0 or info.file_size > MAX_MEMBER_BYTES:
            raise ValueError(f"unsafe or excessive ENDF archive member {member_name}")
        if Path(member_name).name != member_name.split("/")[-1] or ".." in Path(member_name).parts:
            raise ValueError("refusing archive path traversal")
        with bundle.open(info) as stream:
            data = stream.read(MAX_MEMBER_BYTES + 1)
        if len(data) != info.file_size or len(data) > MAX_MEMBER_BYTES:
            raise ValueError(f"archive member size mismatch for {member_name}")
        return data


def convert(photo_zip: Path, relax_zip: Path, output: Path) -> dict[str, Any]:
    photo_zip = photo_zip.expanduser().resolve(strict=True)
    relax_zip = relax_zip.expanduser().resolve(strict=True)
    if photo_zip.stat().st_size > MAX_ARCHIVE_BYTES or relax_zip.stat().st_size > MAX_ARCHIVE_BYTES:
        raise ValueError("photon source archive exceeds the 32 MiB bound")
    observed = {"photoatomic": digest(photo_zip, "md5"), "atomic_relaxation": digest(relax_zip, "md5")}
    if observed != EXPECTED_MD5:
        raise ValueError("NNDC official MD5 mismatch; stop without converting modified archives")
    if output.exists():
        raise ValueError("output directory must be new and exclusive")
    parent = output.absolute().parent
    parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.converting-", dir=parent))
    try:
        try:
            import openmc
            import h5py
        except ImportError as exc:
            raise RuntimeError("run this script with OpenMC 0.15.3 and h5py's Python environment") from exc
        if openmc.__version__ != "0.15.3":
            raise RuntimeError(f"requires OpenMC Python API 0.15.3, got {openmc.__version__}")
        photon_dir = stage / "photon"
        photon_dir.mkdir()
        files = []
        for symbol, z in ELEMENTS.items():
            photo_member = f"photoat/photoat-{z:03d}_{symbol}_000.endf"
            relax_member = f"atomic_relax/atom-{z:03d}_{symbol}_000.endf"
            photo_data = read_member(photo_zip, photo_member)
            relax_data = read_member(relax_zip, relax_member)
            photo_endf = stage / f"{symbol}-photoat.endf"
            relax_endf = stage / f"{symbol}-relax.endf"
            photo_endf.write_bytes(photo_data)
            relax_endf.write_bytes(relax_data)
            result_path = photon_dir / f"{symbol}.h5"
            element = openmc.data.IncidentPhoton.from_endf(str(photo_endf), str(relax_endf))
            element.export_to_hdf5(str(result_path), mode="w")
            reopened = openmc.data.IncidentPhoton.from_hdf5(str(result_path))
            relaxation = reopened.atomic_relaxation
            with h5py.File(result_path, "r") as hdf:
                photo_shells = set(hdf[reopened.name]["subshells"].keys())
            binding_shells = set(relaxation.binding_energy) if relaxation else set()
            electron_shells = set(relaxation.num_electrons) if relaxation else set()
            transition_shells = set(relaxation.transitions) if relaxation else set()
            if (not reopened.reactions or not photo_shells or relaxation is None
                    or not photo_shells <= binding_shells or not photo_shells <= electron_shells):
                raise RuntimeError(f"OpenMC round-trip validation failed for {symbol}")
            record = {
                "element": symbol,
                "atomic_number": z,
                "relative_path": f"photon/{symbol}.h5",
                "bytes": result_path.stat().st_size,
                "sha256": digest(result_path, "sha256"),
                "photoatomic_reactions": len(reopened.reactions),
                "photoelectric_shells": len(photo_shells),
                "relaxation_binding_shells": len(binding_shells),
                "relaxation_transition_shells": len(transition_shells),
                "round_trip": "PASS",
            }
            files.append(record)
            photo_endf.unlink()
            relax_endf.unlink()

        import xml.etree.ElementTree as ET
        root = ET.Element("cross_sections")
        for item in files:
            ET.SubElement(root, "library", {
                "materials": item["element"],
                "path": item["relative_path"],
                "type": "photon",
            })
        ET.indent(root, space="  ")
        xml_path = stage / "photon-cross-sections.xml"
        ET.ElementTree(root).write(xml_path, encoding="utf-8", xml_declaration=True)
        conversion = {
            "schema_version": SCHEMA,
            "openmc_python_api_version": openmc.__version__,
            "converter": "openmc.data.IncidentPhoton.from_endf(photoatomic, relaxation); export_to_hdf5; re-open with IncidentPhoton.from_hdf5",
            "source": {
                "publisher": "National Nuclear Data Center, Brookhaven National Laboratory",
                "evaluation": "ENDF/B-VII.1 photoatomic and atomic-relaxation sublibraries",
                "photoatomic_zip": {"filename": photo_zip.name, "official_url": "https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-photoat.zip", "official_md5": EXPECTED_MD5["photoatomic"], "observed_md5": observed["photoatomic"], "sha256": digest(photo_zip, "sha256")},
                "atomic_relaxation_zip": {"filename": relax_zip.name, "official_url": "https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-atomic_relax.zip", "official_md5": EXPECTED_MD5["atomic_relaxation"], "observed_md5": observed["atomic_relaxation"], "sha256": digest(relax_zip, "sha256")},
                "license_statement": "NNDC archive page does not state a redistribution license; local research-use copy only pending rights clarification.",
            },
            "files": files,
            "photon_cross_sections_xml_sha256": digest(xml_path, "sha256"),
            "notice": "This metadata identifies converted files; it does not authenticate neutron data or qualify nuclear data for a reactor calculation.",
        }
        (stage / "conversion-provenance.json").write_text(json.dumps(conversion, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        # Do not retain downloaded ENDF records in the output directory.
        if output.exists():
            raise ValueError("output directory appeared while conversion was in progress")
        stage.rename(output)
        return conversion
    except Exception:
        failed = output.with_name(f"{output.name}.conversion-incomplete")
        if not failed.exists():
            stage.rename(failed)
        else:
            shutil.rmtree(stage)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--photoatomic-zip", required=True, type=Path)
    parser.add_argument("--atomic-relaxation-zip", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        record = convert(args.photoatomic_zip, args.atomic_relaxation_zip, args.output_dir)
    except Exception as exc:
        parser.exit(2, f"conversion error: {type(exc).__name__}: {exc}\n")
    print(json.dumps(record, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
