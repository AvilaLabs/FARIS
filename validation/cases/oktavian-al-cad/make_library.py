#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Write the cross_sections.xml this case runs with: FENDL-3.2 neutron data and ENDF/B-VII.1 photon data.

    python3 make_library.py --fendl DIR --photon DIR --out cross_sections.xml

--fendl is the FENDL-3.2 HDF5 directory (neutron/ and its cross_sections.xml); --photon the ENDF/B-VII.1 OpenMC
HDF5 directory (photon/ and its cross_sections.xml). Entries point at the files where they are (absolute paths);
nothing is copied. The photon files of the FENDL-3.2 HDF5 directory are not used: OpenMC 0.15.3 stops with a
segmentation fault in sample_photon_reaction on every one tried (Al, Si, Cr, Fe, Ni, Cu, 1 MeV photons in a
sphere of one element), while the ENDF/B-VII.1 photon files run.
"""
from __future__ import annotations

import argparse
import xml.etree.ElementTree as ET
from pathlib import Path


def build(fendl: Path, photon: Path) -> str:
    root = ET.Element("cross_sections")
    for source, kind in ((fendl, "neutron"), (photon, "photon")):
        for lib in ET.parse(source / "cross_sections.xml").getroot().findall("library"):
            if lib.get("type") == kind:
                ET.SubElement(root, "library", materials=lib.get("materials"), path=str((source / lib.get("path")).resolve()), type=kind)
    ET.indent(root)
    return "<?xml version='1.0' encoding='UTF-8'?>\n" + ET.tostring(root, encoding="unicode") + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--fendl", type=Path, required=True)
    ap.add_argument("--photon", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    args.out.write_text(build(args.fendl.expanduser(), args.photon.expanduser()), encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
