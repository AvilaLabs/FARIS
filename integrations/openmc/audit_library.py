#!/usr/bin/env python3
"""Read-only inventory of selected OpenMC continuous-energy library files.

Run this with an environment containing OpenMC and h5py, for example:

    python audit_library.py --cross-sections /path/to/cross_sections.xml

The default nuclide set covers the documented M1 cold-data candidate. No
library files are changed or copied. The audit records file identity and data
presence; it does not qualify nuclear data or establish a transport result.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

DEFAULT_NUCLIDES = [
    "Be9",
    "Cr50",
    "Cr52",
    "Cr53",
    "Cr54",
    "Cu63",
    "Cu65",
    "Fe54",
    "Fe56",
    "Fe57",
    "Fe58",
    "F19",
    "H1",
    "H2",
    "Li6",
    "Li7",
    "Ni58",
    "Ni60",
    "Ni61",
    "Ni62",
    "Ni64",
    "Ti46",
    "Ti47",
    "Ti48",
    "Ti49",
    "Ti50",
    "W180",
    "W182",
    "W183",
    "W184",
    "W186",
]

DEFAULT_PHOTON_ELEMENTS = ["Be", "Cr", "Cu", "Fe", "F", "H", "Li", "Ni", "Ti", "W"]
REACTION_MTS = (301, 901, 444)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def library_map(root: ET.Element, kind: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for node in root.findall("library"):
        if node.get("type") != kind:
            continue
        relpath = node.get("path")
        if not relpath:
            continue
        for name in node.get("materials", "").split():
            result[name] = relpath
    return result


def audit_neutron(path: Path, h5py: Any, openmc: Any) -> dict[str, Any]:
    item: dict[str, Any] = {
        "relative_path": None,
        "sha256": None,
        "size_bytes": None,
        "readable_by_h5py": False,
        "readable_by_openmc_data_api": False,
        "temperatures": [],
        "temperatures_k": [],
        "reactions": {},
        "h3_production_route": None,
        "secondary_photon_products": None,
        "error": None,
    }
    try:
        item["sha256"] = sha256_file(path)
        item["size_bytes"] = path.stat().st_size
        with h5py.File(path, "r") as library:
            nuclide = next(iter(library.keys()))
            group = library[nuclide]
            item["readable_by_h5py"] = True
            item["nuclide_name_in_hdf5"] = nuclide
            # OpenMC stores kT in eV. Keep its human-readable rounded group
            # label separate from the numeric temperature used for matching.
            item["temperatures"] = sorted(group["kTs"].keys())
            item["temperatures_k"] = sorted(
                float(value[()]) / openmc.data.K_BOLTZMANN
                for value in group["kTs"].values()
            )
            reactions = group["reactions"]
            for mt in REACTION_MTS:
                key = f"reaction_{mt:03d}"
                if key in reactions:
                    rx = reactions[key]
                    item["reactions"][str(mt)] = {
                        "present": True,
                        "label": _decode(rx.attrs.get("label")),
                        "redundant": bool(rx.attrs.get("redundant", False)),
                        "temperature_groups": sorted(
                            key for key in rx.keys() if key.endswith("K")
                        ),
                    }
                else:
                    item["reactions"][str(mt)] = {"present": False}
            if nuclide in ("Li6", "Li7"):
                key = "reaction_205"
                if key in reactions:
                    rx = reactions[key]
                    data_group = rx["294K"]
                    xs = data_group.get("xs")
                    xs_values = xs[()] if xs is not None else []
                    item["h3_production_route"] = {
                        "data_present": True,
                        "label": _decode(rx.attrs.get("label")),
                        "redundant_derived_reaction": bool(rx.attrs.get("redundant", False)),
                        "reaction_105_n_t_present": "reaction_105" in reactions,
                        "reaction_105_n_t_q_value_eV": (
                            float(reactions["reaction_105"].attrs.get("Q_value", 0.0))
                            if "reaction_105" in reactions
                            else None
                        ),
                        "temperature_groups": sorted(
                            name for name in rx.keys() if name.endswith("K")
                        ),
                        "294K_xs_grid_points": len(xs_values),
                        "294K_xs_positive_grid_points": int((xs_values > 0.0).sum()) if len(xs_values) else 0,
                        "interpretation": "FENDL stores the derived total tritium-product reaction (n,Xt), MT205. OpenMC maps its H3-production score to this reaction class; presence is not a run-level scoring verification or a nuclear-data validation.",
                    }
                else:
                    item["h3_production_route"] = {"data_present": False}
        data = openmc.data.IncidentNeutron.from_hdf5(str(path))
        item["readable_by_openmc_data_api"] = True
        photon_products = 0
        photon_product_reactions = 0
        photon_products_without_distribution = 0
        for reaction in data.reactions.values():
            photons = [product for product in reaction.products if product.particle == "photon"]
            n_photon = len(photons)
            photon_products += n_photon
            photon_product_reactions += int(n_photon > 0)
            photon_products_without_distribution += sum(
                product.distribution is None for product in photons
            )
        item["secondary_photon_products"] = {
            "photon_product_records": photon_products,
            "reactions_with_photon_products": photon_product_reactions,
            "products_without_energy_or_angle_distribution": photon_products_without_distribution,
            "note": "Parsed secondary photon product records from this neutron HDF5 file; coupled transport capability still requires an actual OpenMC run and response check.",
        }
    except Exception as error:  # preserve failed inspection as data, not a false pass
        item["error"] = f"{type(error).__name__}: {error}"
    return item


def _decode(value: Any) -> str | None:
    if value is None:
        return None
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    if hasattr(value, "decode"):
        try:
            return value.decode("utf-8", errors="replace")
        except Exception:
            pass
    return str(value)


def audit_photon(path: Path, h5py: Any, openmc: Any) -> dict[str, Any]:
    result: dict[str, Any] = {
        "relative_path": None,
        "sha256": None,
        "size_bytes": None,
        "readable_by_h5py": False,
        "readable_by_openmc_data_api": False,
        "atomic_relaxation_object_present": False,
        "atomic_relaxation_populated": False,
        "atomic_relaxation_shell_count": 0,
        "photoelectric_shell_count": 0,
        "atomic_relaxation_transition_shell_count": 0,
        "top_level_groups": [],
        "error": None,
    }
    try:
        result["sha256"] = sha256_file(path)
        result["size_bytes"] = path.stat().st_size
        with h5py.File(path, "r") as library:
            result["top_level_groups"] = sorted(library.keys())
            result["readable_by_h5py"] = True
        data = openmc.data.IncidentPhoton.from_hdf5(str(path))
        result["readable_by_openmc_data_api"] = bool(data.reactions)
        relaxation = data.atomic_relaxation
        result["atomic_relaxation_object_present"] = relaxation is not None
        result["atomic_relaxation_shell_count"] = len(relaxation.binding_energy) if relaxation else 0
        with h5py.File(path, "r") as library:
            photoelectric_shells = set(library[data.name]["subshells"].keys())
        result["photoelectric_shell_count"] = len(photoelectric_shells)
        result["atomic_relaxation_transition_shell_count"] = len(relaxation.transitions) if relaxation else 0
        # Low-Z elements legitimately have no radiative/Auger transitions, but
        # still require binding-energy and electron-count records for each
        # photoelectric shell so OpenMC can build a valid shell map.
        result["atomic_relaxation_populated"] = bool(
            relaxation is not None
            and photoelectric_shells
            and photoelectric_shells <= set(relaxation.binding_energy)
            and photoelectric_shells <= set(relaxation.num_electrons)
        )
        if not result["readable_by_openmc_data_api"]:
            result["error"] = "OpenMC photon API returned no interaction reactions"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
    return result


def build_audit(cross_sections: Path, nuclides: list[str], photon_elements: list[str]) -> dict[str, Any]:
    try:
        import h5py
        import openmc
    except ImportError as error:
        raise SystemExit(f"Run with the selected OpenMC environment (needs openmc and h5py): {error}")

    cross_sections = cross_sections.resolve()
    if not cross_sections.is_file():
        raise SystemExit(f"cross_sections.xml not found: {cross_sections}")
    xml_root = ET.parse(cross_sections).getroot()
    neutron_files = library_map(xml_root, "neutron")
    photon_files = library_map(xml_root, "photon")
    thermal_files = library_map(xml_root, "thermal")
    data_root = cross_sections.parent

    inventory: dict[str, Any] = {}
    for nuclide in sorted(set(nuclides)):
        relpath = neutron_files.get(nuclide)
        if relpath is None:
            inventory[nuclide] = {"library_entry_present": False}
            continue
        path = data_root / relpath
        details = audit_neutron(path, h5py, openmc) if path.is_file() else {
            "readable_by_h5py": False,
            "readable_by_openmc_data_api": False,
            "error": "referenced file missing",
        }
        details["library_entry_present"] = True
        details["relative_path"] = relpath
        inventory[nuclide] = details

    photon_inventory: dict[str, Any] = {}
    for element in sorted(set(photon_elements)):
        relpath = photon_files.get(element)
        if relpath is None:
            photon_inventory[element] = {"library_entry_present": False}
            continue
        path = data_root / relpath
        details = audit_photon(path, h5py, openmc) if path.is_file() else {
            "readable_by_h5py": False,
            "error": "referenced file missing",
        }
        details["library_entry_present"] = True
        details["relative_path"] = relpath
        photon_inventory[element] = details

    target_temps = sorted(
        {
            temp
            for info in inventory.values()
            for temp in info.get("temperatures", [])
        }
    )
    return {
        "schema": "faris.openmc-library-audit/1.0.0",
        "purpose": "Read-only targeted identity, HDF5/API readability, temperature, reaction-score, secondary-photon, and populated atomic-relaxation inventory. Not a transport result, benchmark, or nuclear-data qualification.",
        "openmc": {"version": openmc.__version__},
        "cross_sections_xml": {
            "sha256": sha256_file(cross_sections),
            "filename": cross_sections.name,
        },
        "neutron_library": inventory,
        "photon_atomic_library": photon_inventory,
        "thermal_scattering_library_entries": thermal_files,
        "summary": {
            "requested_neutron_nuclides": sorted(set(nuclides)),
            "all_neutron_entries_present": all(v.get("library_entry_present", False) for v in inventory.values()),
            "all_target_neutron_files_hdf5_and_openmc_readable": all(v.get("readable_by_h5py", False) and v.get("readable_by_openmc_data_api", False) for v in inventory.values()),
            "all_target_nuclides_have_MT301": all(v.get("reactions", {}).get("301", {}).get("present", False) for v in inventory.values()),
            "all_target_nuclides_have_MT901": all(v.get("reactions", {}).get("901", {}).get("present", False) for v in inventory.values()),
            "all_target_nuclides_have_MT444": all(v.get("reactions", {}).get("444", {}).get("present", False) for v in inventory.values()),
            "Li6_Li7_MT205_data_present": all((inventory.get(n, {}).get("h3_production_route") or {}).get("data_present", False) for n in ("Li6", "Li7")),
            "target_neutron_temperature_groups": target_temps,
            "all_target_photon_atomic_files_readable": all(v.get("readable_by_h5py", False) for v in photon_inventory.values()),
            "all_target_photon_files_readable_by_openmc_data_api": all(v.get("readable_by_openmc_data_api", False) for v in photon_inventory.values()),
            "all_target_photon_atomic_relaxation_populated": all(v.get("atomic_relaxation_populated", False) for v in photon_inventory.values()),
            "all_target_nuclides_have_secondary_photon_product_records": all((v.get("secondary_photon_products") or {}).get("photon_product_records", 0) > 0 for v in inventory.values()),
            "thermal_scattering_library_entries_present": bool(thermal_files),
        },
        "provenance_and_license": {
            "upstream_release_assertion": "FENDL-3.2 HDF5 (local directory naming and cross_sections.xml; not authenticated against an acquisition receipt in this audit)",
            "publisher_checksum_or_acquisition_receipt": None,
            "redistribution_license_or_permission": None,
            "note": "Do not bundle or redistribute the local data based on this audit. Verify publisher/package provenance and terms separately."
        },
        "audit_runtime": {"python": sys.version.split()[0], "h5py": h5py.__version__},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cross-sections", type=Path, required=True, help="OpenMC cross_sections.xml")
    parser.add_argument("--nuclides", nargs="+", default=DEFAULT_NUCLIDES, help="Neutron nuclides to inspect (default: M1 candidate inventory)")
    parser.add_argument("--photon-elements", nargs="+", default=DEFAULT_PHOTON_ELEMENTS, help="Photon atomic-data elements to inspect")
    parser.add_argument("--output", type=Path, help="Write JSON to this path; stdout if omitted")
    args = parser.parse_args()
    result = build_audit(args.cross_sections, args.nuclides, args.photon_elements)
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    else:
        sys.stdout.write(encoded)


if __name__ == "__main__":
    main()
