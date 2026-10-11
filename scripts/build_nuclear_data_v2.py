#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Build FARIS's second local nuclear-data library (fendl32-endfbvii1-v2).

Sources (all local, nothing is downloaded):

* every neutron nuclide of the full FENDL-3.2 HDF5 directory, copied unchanged;
* ENDF/B-VII.1 photoatomic + atomic-relaxation data converted with OpenMC
  (IncidentPhoton.from_endf, export_to_hdf5) for every element that appears in
  a neutron nuclide of the new library;
* neutron nuclides of the fusion-materials-v1 set that FENDL-3.2 lacks, taken
  from ENDF/B-VIII.1 and listed one by one;
* a fixed list of ENDF/B-VIII.1 thermal-scattering tables.

Run with OpenMC 0.15.3 and h5py. The build is deterministic (no timestamps, sorted
order), writes into a staging directory, and refuses to replace an existing root
without --force. It never writes into any other library root.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "integrations" / "openmc"))

SCHEMA = "faris-nuclear-data-library-v2-provenance/v1"
OPENMC_VERSION = "0.15.3"
MARKER = ".faris-nuclear-data-v2"
DEFAULT_ROOT_NAME = "fendl32-endfbvii1-v2"
EXPECTED_MD5 = {
    "photoatomic": "5192f94e61f0b385cf536f448ffab4a4",
    "atomic_relaxation": "fddb6035e7f2b6931e51a58fc754bd10",
}
MAX_MEMBER_BYTES = 16 * 1024 * 1024

# ENDF/B-VIII.1 S(alpha,beta) tables for the fusion materials. They are used
# with FENDL-3.2 neutron data only when a material declares them.
THERMAL_TABLES = [
    "c_H_in_H2O",
    "c_D_in_D2O",
    "c_O_in_D2O",
    "c_Be",
    "c_Be_in_BeO",
    "c_O_in_BeO",
    "c_Be_in_FLiBe",
    "c_F_in_FLiBe",
    "c_Li_in_FLiBe",
    "c_Graphite",
    "c_H_in_7LiH",
    "c_Li_in_7LiH",
    "c_D_in_7LiD",
    "c_Li_in_7LiD",
    "c_Ca_in_CaH2",
    "c_H1_in_CaH2",
    "c_H2_in_CaH2",
    "c_H_in_ZrH2",
    "c_Zr_in_ZrH2",
    "c_H_in_CH2",
]
# Fusion materials for which ENDF/B-VIII.1 has no table; recorded, not hidden.
THERMAL_ABSENT_NOTES = {
    "TiH2": "no Ti-in-TiH2 or H-in-TiH2 table in ENDF/B-VIII.1",
    "natural Li hydride/deuteride": "tables exist for 7LiH and 7LiD only (Li-7 enriched); no natural-lithium table",
    "H2O oxygen": "ENDF/B-VIII.1 has no O-in-liquid-H2O table; oxygen in light water is free-gas",
    "PbLi, B4C, Li4SiO4, Li2TiO3, Be12Ti": "no table in the ENDF/B-VIII.1 set; free-gas treatment",
}

LICENCE_NOTES = {
    "fendl32": "FENDL-3.2 is distributed by the IAEA Nuclear Data Section under IAEA NDS terms of use. The terms were not re-read for this build; FARIS keeps the files local and Git-ignored and does not redistribute them.",
    "endfb_vii1": "ENDF/B-VII.1 is published by the NNDC, Brookhaven National Laboratory. The NNDC download page states no explicit licence; redistribution rights remain unresolved, as in docs/PHOTON_LIBRARY_ACQUISITION.md.",
    "endfb_viii1": "ENDF/B-VIII.1 (OpenMC's official HDF5 build) is published by the NNDC and distributed through OpenMC. No licence text was checked for this build; treated as the existing documentation treats NNDC data: local research use, no redistribution.",
    "established": "Byte identity of every file (SHA-256), the NNDC MD5 of the two ENDF/B-VII.1 archives, OpenMC readability (audit), the source of every entry.",
    "not_established": "Redistribution rights for any of the data; authentication of the FENDL-3.2 and ENDF/B-VIII.1 HDF5 acquisition (no publisher checksum was available to compare); suitability of any evaluation for a particular calculation.",
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def md5_file(path: Path) -> str:
    digest = hashlib.md5()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


# ---- pure parts ---------------------------------------------------------


def parse_cross_sections(xml_text: str) -> dict[str, dict[str, str]]:
    """kind -> {material name -> relative path} from an OpenMC cross_sections.xml."""
    result: dict[str, dict[str, str]] = {}
    for node in ET.fromstring(xml_text).findall("library"):
        kind = node.get("type")
        path = node.get("path")
        if not kind or not path:
            continue
        for name in node.get("materials", "").split():
            result.setdefault(kind, {})[name] = path
    return result


def element_of(nuclide: str) -> str:
    """Element symbol of an OpenMC nuclide name (Ta180_m1 -> Ta, H2 -> H)."""
    match = re.match(r"[A-Za-z]+", nuclide)
    if not match:
        raise ValueError(f"not a nuclide name: {nuclide!r}")
    return match.group()


def nuclide_sort_key(nuclide: str) -> tuple[str, int, str]:
    digits = re.search(r"\d+", nuclide)
    return (element_of(nuclide), int(digits.group()) if digits else 0, nuclide)


def plan_neutron(
    fendl: dict[str, str], viii: dict[str, str], needed: list[str]
) -> dict[str, Any]:
    """Pick the source library of every neutron nuclide.

    Everything in FENDL-3.2 is taken from FENDL-3.2. A needed nuclide absent
    from FENDL-3.2 comes from ENDF/B-VIII.1; one absent from both is a gap.
    """
    entries = {name: {"source_library": "FENDL-3.2", "source_relative_path": path} for name, path in fendl.items()}
    gap_filled = []
    gaps = []
    for name in sorted(set(needed), key=nuclide_sort_key):
        if name in fendl:
            continue
        if name in viii:
            entries[name] = {"source_library": "ENDF/B-VIII.1", "source_relative_path": viii[name]}
            gap_filled.append(name)
        else:
            gaps.append({"kind": "neutron_nuclide", "name": name, "reason": "absent from FENDL-3.2 and from ENDF/B-VIII.1 neutron HDF5"})
    return {
        "entries": dict(sorted(entries.items(), key=lambda kv: nuclide_sort_key(kv[0]))),
        "gap_filled": gap_filled,
        "gaps": gaps,
    }


def needed_elements_without_isotopes(elements: list[str], nuclides_in_library: set[str]) -> list[dict[str, str]]:
    present = {element_of(n) for n in nuclides_in_library}
    return [
        {"kind": "element", "name": el, "reason": "no isotope of this element in FENDL-3.2 or ENDF/B-VIII.1"}
        for el in sorted(set(elements) - present)
    ]


def photon_members(zip_names: list[str], directory: str) -> dict[str, tuple[int, str]]:
    """symbol -> (Z, member name) for members like photoat/photoat-026_Fe_000.endf."""
    result: dict[str, tuple[int, str]] = {}
    for name in zip_names:
        match = re.fullmatch(rf"{re.escape(directory)}/[A-Za-z_]+-(\d{{3}})_([A-Za-z]+)_000\.endf", name)
        if match:
            result[match.group(2)] = (int(match.group(1)), name)
    return result


def thermal_plan(viii_thermal: dict[str, str], wanted: list[str]) -> dict[str, Any]:
    present = {name: viii_thermal[name] for name in wanted if name in viii_thermal}
    missing = [
        {"kind": "thermal_table", "name": name, "reason": "absent from the ENDF/B-VIII.1 cross_sections.xml"}
        for name in wanted
        if name not in viii_thermal
    ]
    return {"tables": dict(sorted(present.items())), "gaps": missing}


def render_cross_sections(
    neutron: dict[str, str], photon: dict[str, str], thermal: dict[str, str]
) -> str:
    """Deterministic cross_sections.xml text with root-relative paths."""
    root = ET.Element("cross_sections")
    for name in sorted(neutron, key=nuclide_sort_key):
        ET.SubElement(root, "library", {"materials": name, "path": neutron[name], "type": "neutron"})
    for name in sorted(photon):
        ET.SubElement(root, "library", {"materials": name, "path": photon[name], "type": "photon"})
    for name in sorted(thermal):
        ET.SubElement(root, "library", {"materials": name, "path": thermal[name], "type": "thermal"})
    ET.indent(root, space="  ")
    return "<?xml version='1.0' encoding='UTF-8'?>\n" + ET.tostring(root, encoding="unicode") + "\n"


def validate_root(root: Path, force: bool) -> None:
    """Refuse an existing root unless --force and it is a v2 root."""
    if not root.exists():
        return
    if not force:
        raise SystemExit(f"{root} exists; refusing to overwrite without --force")
    if not (root / MARKER).is_file():
        raise SystemExit(f"{root} exists but is not a v2 library root (no {MARKER}); refusing even with --force")


# ---- build --------------------------------------------------------------


def read_member(archive: Path, member: str) -> bytes:
    with zipfile.ZipFile(archive) as bundle:
        info = bundle.getinfo(member)
        if info.is_dir() or info.file_size <= 0 or info.file_size > MAX_MEMBER_BYTES or ".." in Path(member).parts:
            raise ValueError(f"unsafe or excessive archive member {member}")
        data = bundle.read(info)
    if len(data) != info.file_size:
        raise ValueError(f"archive member size mismatch for {member}")
    return data


def convert_photon(photo_zip: Path, relax_zip: Path, elements: list[str], out_dir: Path, scratch: Path) -> tuple[list[dict[str, Any]], list[dict[str, str]]]:
    import h5py
    import openmc

    with zipfile.ZipFile(photo_zip) as z:
        photo = photon_members(z.namelist(), "photoat")
    with zipfile.ZipFile(relax_zip) as z:
        relax = photon_members(z.namelist(), "atomic_relax")
    out_dir.mkdir(parents=True)
    scratch.mkdir(parents=True)
    records: list[dict[str, Any]] = []
    gaps: list[dict[str, str]] = []
    for symbol in sorted(elements):
        if symbol not in photo or symbol not in relax:
            gaps.append({"kind": "photon_element", "name": symbol, "reason": "no photoatomic or atomic-relaxation record in the ENDF/B-VII.1 archives"})
            continue
        z, photo_member = photo[symbol]
        _, relax_member = relax[symbol]
        photo_endf = scratch / f"{symbol}-photoat.endf"
        relax_endf = scratch / f"{symbol}-relax.endf"
        photo_endf.write_bytes(read_member(photo_zip, photo_member))
        relax_endf.write_bytes(read_member(relax_zip, relax_member))
        target = out_dir / f"{symbol}.h5"
        openmc.data.IncidentPhoton.from_endf(str(photo_endf), str(relax_endf)).export_to_hdf5(str(target), mode="w")
        reopened = openmc.data.IncidentPhoton.from_hdf5(str(target))
        relaxation = reopened.atomic_relaxation
        with h5py.File(target, "r") as hdf:
            shells = set(hdf[reopened.name]["subshells"].keys())
        binding = set(relaxation.binding_energy) if relaxation else set()
        electrons = set(relaxation.num_electrons) if relaxation else set()
        if not reopened.reactions or not shells or relaxation is None or not shells <= binding or not shells <= electrons:
            raise RuntimeError(f"OpenMC round-trip validation failed for {symbol}")
        records.append({
            "element": symbol,
            "atomic_number": z,
            "relative_path": f"photon/{symbol}.h5",
            "source_members": {"photoatomic": photo_member, "atomic_relaxation": relax_member},
            "bytes": target.stat().st_size,
            "sha256": sha256_file(target),
            "photoelectric_shells": len(shells),
            "relaxation_binding_shells": len(binding),
            "relaxation_transition_shells": len(relaxation.transitions),
            "round_trip": "PASS",
        })
        photo_endf.unlink()
        relax_endf.unlink()
    shutil.rmtree(scratch)
    return records, gaps


def copy_verified(src: Path, dst: Path) -> dict[str, Any]:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    source_hash = sha256_file(src)
    copied_hash = sha256_file(dst)
    if source_hash != copied_hash:
        raise RuntimeError(f"copy of {src} changed its bytes")
    return {"bytes": dst.stat().st_size, "sha256": copied_hash}


def build(args: argparse.Namespace) -> dict[str, Any]:
    import openmc

    if openmc.__version__ != OPENMC_VERSION:
        raise SystemExit(f"requires OpenMC {OPENMC_VERSION}, got {openmc.__version__}")
    import audit_library

    fendl_xml = args.fendl_dir / "cross_sections.xml"
    viii_xml = args.endfb_viii_dir / "cross_sections.xml"
    fendl_maps = parse_cross_sections(fendl_xml.read_text(encoding="utf-8"))
    viii_maps = parse_cross_sections(viii_xml.read_text(encoding="utf-8"))
    for label, zip_path in (("photoatomic", args.photoatomic_zip), ("atomic_relaxation", args.atomic_relaxation_zip)):
        if md5_file(zip_path) != EXPECTED_MD5[label]:
            raise SystemExit(f"NNDC MD5 mismatch for {zip_path}; refusing to convert")

    selection = audit_library.fusion_materials_v1_selection(openmc.data.NATURAL_ABUNDANCE)
    plan = plan_neutron(fendl_maps["neutron"], viii_maps["neutron"], selection["nuclides"])
    thermal = thermal_plan(viii_maps.get("thermal", {}), THERMAL_TABLES)
    library_nuclides = set(plan["entries"])
    element_gaps = needed_elements_without_isotopes(selection["elements"], library_nuclides)
    photon_elements = sorted({element_of(n) for n in library_nuclides})

    root = args.output_dir.resolve()
    validate_root(root, args.force)
    stage = root.with_name(f".{root.name}.building")
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    try:
        neutron_records = []
        neutron_paths: dict[str, str] = {}
        for name, entry in plan["entries"].items():
            base = args.fendl_dir if entry["source_library"] == "FENDL-3.2" else args.endfb_viii_dir
            src = base / entry["source_relative_path"]
            rel = f"neutron/{name}.h5"
            record = {"nuclide": name, "relative_path": rel, **entry, **copy_verified(src, stage / rel)}
            neutron_records.append(record)
            neutron_paths[name] = rel
        photon_records, photon_gaps = convert_photon(
            args.photoatomic_zip, args.atomic_relaxation_zip, photon_elements, stage / "photon", stage / ".convert-scratch"
        )
        thermal_records = []
        for name, relpath in thermal["tables"].items():
            rel = f"thermal/{name}.h5"
            record = {"table": name, "relative_path": rel, "source_library": "ENDF/B-VIII.1", "source_relative_path": relpath, **copy_verified(args.endfb_viii_dir / relpath, stage / rel)}
            thermal_records.append(record)
        xml_text = render_cross_sections(
            neutron_paths,
            {r["element"]: r["relative_path"] for r in photon_records},
            {r["table"]: r["relative_path"] for r in thermal_records},
        )
        (stage / "cross_sections.xml").write_text(xml_text, encoding="utf-8")
        (stage / MARKER).write_text("FARIS nuclear-data library v2; built by scripts/build_nuclear_data_v2.py\n", encoding="utf-8")

        first_hashes: dict[str, str] = {}
        if args.first_provenance and args.first_provenance.is_file():
            first = json.loads(args.first_provenance.read_text(encoding="utf-8"))
            first_hashes = {f["element"]: f["sha256"] for f in first.get("photon_data", {}).get("files", [])}
        for record in photon_records:
            if record["element"] in first_hashes:
                record["matches_first_library_sha256"] = first_hashes[record["element"]] == record["sha256"]

        gap_list = plan["gaps"] + element_gaps + photon_gaps + thermal["gaps"]
        provenance = {
            "schema": SCHEMA,
            "not_for_git": True,
            "library_root_name": root.name,
            "build_script": "scripts/build_nuclear_data_v2.py",
            "openmc_version": openmc.__version__,
            "cross_sections_xml": {"relative_path": "cross_sections.xml", "sha256": hashlib.sha256(xml_text.encode("utf-8")).hexdigest(), "bytes": len(xml_text.encode("utf-8"))},
            "sources": {
                "fendl32_hdf5": {"directory": str(args.fendl_dir), "cross_sections_xml_sha256": sha256_file(fendl_xml), "neutron_entries": len(fendl_maps["neutron"]), "photon_entries_not_used": len(fendl_maps.get("photon", {}))},
                "endfb_viii1_hdf5": {"directory": str(args.endfb_viii_dir), "cross_sections_xml_sha256": sha256_file(viii_xml), "neutron_entries": len(viii_maps["neutron"]), "thermal_entries": len(viii_maps.get("thermal", {}))},
                "endfb_vii1_photon": {
                    "photoatomic_zip": {"path": str(args.photoatomic_zip), "md5": md5_file(args.photoatomic_zip), "sha256": sha256_file(args.photoatomic_zip), "official_md5": EXPECTED_MD5["photoatomic"]},
                    "atomic_relaxation_zip": {"path": str(args.atomic_relaxation_zip), "md5": md5_file(args.atomic_relaxation_zip), "sha256": sha256_file(args.atomic_relaxation_zip), "official_md5": EXPECTED_MD5["atomic_relaxation"]},
                },
            },
            "neutron_data": {
                "method": "byte-for-byte copy (not symlink, not hard link); SHA-256 of source and copy compared at build time",
                "rule": "all FENDL-3.2 neutron nuclides, plus nuclides of the fusion-materials-v1 set that FENDL-3.2 lacks, from ENDF/B-VIII.1",
                "count": len(neutron_records),
                "files": neutron_records,
            },
            "photon_data": {
                "evaluation": "ENDF/B-VII.1 photoatomic and atomic relaxation sublibraries (NNDC)",
                "converter": "openmc.data.IncidentPhoton.from_endf(photoatomic, relaxation); export_to_hdf5(path, mode='w'); re-opened with IncidentPhoton.from_hdf5",
                "elements_rule": "every element that appears in a neutron nuclide of this library",
                "count": len(photon_records),
                "files": photon_records,
            },
            "thermal_scattering_data": {
                "evaluation": "ENDF/B-VIII.1 thermal scattering (OpenMC HDF5 build), used with FENDL-3.2 neutron data",
                "assumption": "Combining a thermal-scattering library with a neutron evaluation from a different library is standard practice, but it is a stated assumption: the S(alpha,beta) tables were evaluated with ENDF/B-VIII.1 neutron data. Apply a table only when a material declares it.",
                "count": len(thermal_records),
                "files": thermal_records,
                "absent_for_fusion_materials": THERMAL_ABSENT_NOTES,
            },
            "gap_fill": {
                "rule": "A needed neutron nuclide that FENDL-3.2 lacks comes from ENDF/B-VIII.1 and is listed here; nothing is mixed silently.",
                "gap_filled_entries": [r for r in neutron_records if r["source_library"] != "FENDL-3.2"],
                "remaining_gaps": gap_list,
            },
            "nuclide_set": {"name": selection["name"], "elements": selection["elements"], "nuclide_count": len(selection["nuclides"])},
            "licence_and_redistribution": LICENCE_NOTES,
        }
        (stage / "provenance.json").write_text(json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        if root.exists():
            shutil.rmtree(root)
        stage.rename(root)
    except BaseException:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    return provenance


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fendl-dir", type=Path, required=True)
    parser.add_argument("--endfb-viii-dir", type=Path, required=True)
    parser.add_argument("--photoatomic-zip", type=Path, required=True)
    parser.add_argument("--atomic-relaxation-zip", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--first-provenance", type=Path, help="references/photon-library-provenance.json, to record photon hash matches")
    parser.add_argument("--provenance-out", type=Path, help="also write the provenance record here (the repository copy)")
    parser.add_argument("--force", action="store_true", help="replace an existing v2 root")
    args = parser.parse_args()
    provenance = build(args)
    if args.provenance_out:
        args.provenance_out.write_text(json.dumps(provenance, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    summary = {
        "neutron": provenance["neutron_data"]["count"],
        "photon": provenance["photon_data"]["count"],
        "thermal": provenance["thermal_scattering_data"]["count"],
        "gap_filled": [r["nuclide"] for r in provenance["gap_fill"]["gap_filled_entries"]],
        "remaining_gaps": provenance["gap_fill"]["remaining_gaps"],
    }
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
