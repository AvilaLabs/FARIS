#!/usr/bin/env python3
"""Expand design-file materials to nuclide vectors and audit them against the library.

Run with the OpenMC environment (openmc and h5py), as a separate process under
the FARIS bounded-job runner:

    material_audit.py --request request.json --cross-sections cross_sections.xml --out audit.json

The request lists each material either as a ready nuclide vector (catalog
materials) or as a recipe (elements, nuclides, optional isotope splits, atom or
weight basis). Natural abundances come from OpenMC's own expansion
(openmc.Material.add_element), the source the FARIS material baseline used. The
expanded atom-fraction vector of every material is reported, and the union of
nuclides goes through audit_library.build_audit, which must sit beside this
file. The audit reads files; it does not qualify nuclear data.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

SCHEMA = "faris-material-audit/v1"


def element_of(nuclide: str) -> str:
    """Symbol of a nuclide name such as Fe56 or Ag110_m1."""
    return re.match(r"[A-Za-z]+", nuclide).group(0)


def expand_recipe(openmc, material: dict) -> dict[str, float]:
    """Atom fractions (sum 1) of a recipe, by nuclide, using OpenMC's expansion."""
    basis = "wo" if material["basis"] == "weight" else "ao"
    mat = openmc.Material()
    for component in material["components"]:
        fraction = float(component["fraction"])
        isotopes = component.get("isotopes")
        if component.get("nuclide"):
            mat.add_nuclide(component["nuclide"], fraction, percent_type=basis)
        elif isotopes:
            # isotope values are atom fractions of the element
            if basis == "wo":
                masses = {n: x * openmc.data.atomic_mass(n) for n, x in isotopes.items()}
                total = sum(masses.values())
                for nuclide, mass in masses.items():
                    mat.add_nuclide(nuclide, fraction * mass / total, percent_type="wo")
            else:
                for nuclide, x in isotopes.items():
                    mat.add_nuclide(nuclide, fraction * x, percent_type="ao")
        else:
            mat.add_element(component["element"], fraction, percent_type=basis)
    mat.set_density("g/cm3", 1.0)  # fractions are normalised below; the value does not matter
    densities = mat.get_nuclide_atom_densities()
    # OpenMC returns name -> atoms/b-cm (older versions: name -> (name, atoms/b-cm))
    values = {name: (v[1] if isinstance(v, tuple) else float(v)) for name, v in densities.items()}
    total = sum(values.values())
    return {name: values[name] / total for name in sorted(values)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--cross-sections", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        import openmc
    except ImportError as error:
        print(f"material_audit: needs the OpenMC environment: {error}", file=sys.stderr)
        return 3
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import audit_library

    request = json.loads(args.request.read_text(encoding="utf-8"))
    report: dict = {"schema": SCHEMA, "openmc": openmc.__version__, "materials": [], "nuclides": {}, "photon_elements": {}}
    wanted: set[str] = set()
    for material in request["materials"]:
        item = {"id": material["id"], "nuclides": None, "error": None}
        try:
            if material["kind"] == "nuclides":
                item["nuclides"] = dict(sorted(material["atom_fractions"].items()))
            else:
                item["nuclides"] = expand_recipe(openmc, material)
            wanted.update(item["nuclides"])
        except Exception as error:  # an unknown element is a finding, not a crash
            item["error"] = f"{type(error).__name__}: {error}"
        report["materials"].append(item)
    elements = sorted({element_of(n) for n in wanted})
    if wanted:
        audit = audit_library.build_audit(args.cross_sections, sorted(wanted), elements)
        report["cross_sections_sha256"] = audit["cross_sections_xml"]["sha256"]
        for name, info in audit["neutron_library"].items():
            report["nuclides"][name] = {
                "present": bool(info.get("library_entry_present")),
                "readable": bool(info.get("readable_by_h5py")) and bool(info.get("readable_by_openmc_data_api")),
                "temperatures_k": info.get("temperatures_k", []),
                "error": info.get("error") or (None if info.get("library_entry_present") else "no entry in cross_sections.xml"),
            }
        for name, info in audit["photon_atomic_library"].items():
            report["photon_elements"][name] = {
                "present": bool(info.get("library_entry_present")),
                "readable": bool(info.get("readable_by_h5py")),
            }
    args.out.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
