#!/usr/bin/env python3
"""CAD helper for FARIS: reads a STEP file and reports what the design file must match.

Run with the CAD environment (cadquery 2.8 and its OCP/OpenCASCADE), as a
separate process under the FARIS bounded-job runner:

    faris_cad.py step-inspect MODEL.step --out inspect.json [--assume-unit mm]

The report (schema "faris-step-inspect/v1") holds the STEP hash, the declared
length unit, and for every solid in read order its name, volume, centroid and
bounding box in metres. It also holds the model bounding-box diagonal and the
versions that produced the numbers. Nothing is repaired: an unreadable file, a
disagreement between the two unit readers or several different length units
are reported as problems and the Rust side stops the import.

The unit is read two independent ways: OpenCASCADE's STEP reader, and a text
parse of the file's LENGTH_UNIT entities. The solid order is the read order of
OpenCASCADE's STEP reader, which is the order cadquery's importers give.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import time
from pathlib import Path

SCHEMA = "faris-step-inspect/v1"
HELPER_VERSION = "1"
# Adaptive-integration tolerance for volume and centroid; the reached error
# estimate is reported per solid.
VOLUME_EPS = 1.0e-7

# OpenCASCADE's STEP reader converts a file with a declared length unit into
# its internal unit (millimetres, set explicitly below); a file with no declared
# unit is read unscaled. So shape numbers are millimetres for a declared file
# and the design file's cad.length_unit otherwise.
INTERNAL_UNIT = "MM"
INTERNAL_FACTOR = 1.0e-3

# Exact factors to metres. Label -> factor.
UNIT_FACTORS = {"mm": 1.0e-3, "cm": 1.0e-2, "m": 1.0, "inch": 0.0254}
SI_PREFIX = {
    "$": 1.0, ".MILLI.": 1.0e-3, ".CENTI.": 1.0e-2, ".MICRO.": 1.0e-6, ".KILO.": 1.0e3,
    ".DECI.": 1.0e-1, ".NANO.": 1.0e-9,
}
# OpenCASCADE's names for the length units (STEPControl_Reader.FileUnits).
OCP_NAMES = {
    "millimetre": 1.0e-3, "centimetre": 1.0e-2, "metre": 1.0, "kilometre": 1.0e3,
    "micrometre": 1.0e-6, "inch": 0.0254, "foot": 0.3048, "mile": 1609.344,
    "mil": 2.54e-5, "microinch": 2.54e-8,
}
# Names OpenCASCADE gives a solid when the file carries none.
SYNTHETIC_NAME = re.compile(r"^Open CASCADE STEP translator [0-9.]+ [0-9.]+$")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def unit_label(factor: float) -> str | None:
    """Label for an exact known factor, else None."""
    for label, value in UNIT_FACTORS.items():
        if math.isclose(factor, value, rel_tol=1e-12, abs_tol=0.0):
            return label
    return None


def describe_factor(factor: float) -> str:
    return unit_label(factor) or f"{factor:g} m"


def _entity_text(text: str, number: str, near: int | None = None) -> str | None:
    """Body of entity #number. Units sit beside the context that names them, so
    search a window around `near` first; a whole-file search of a large file is slow."""
    pattern = re.compile(r"(?m)^#" + number + r"\s*=\s*([^;]*);")
    if near is not None:
        low = max(0, near - 20000)
        match = pattern.search(text, low, near + 20000)
        if match:
            return match.group(1)
    match = pattern.search(text)
    return match.group(1) if match else None


def _statement_factor(text: str, statement: str, near: int | None = None, depth: int = 0) -> tuple[float | None, str | None]:
    """Metres per unit of one LENGTH_UNIT statement body, and a problem text if unreadable."""
    si = re.search(r"SI_UNIT\s*\(\s*(\$|\.[A-Z]+\.)\s*,\s*\.METRE\.\s*\)", statement)
    if si:
        prefix = si.group(1)
        if prefix not in SI_PREFIX:
            return None, f"unknown SI prefix {prefix}"
        return SI_PREFIX[prefix], None
    conv = re.search(r"CONVERSION_BASED_UNIT\s*\(\s*'([^']*)'\s*,\s*#(\d+)\s*\)", statement)
    if conv and depth < 3:
        measure = _entity_text(text, conv.group(2), near)
        if measure is None:
            return None, f"conversion unit '{conv.group(1)}' references a missing entity"
        value = re.search(r"LENGTH_MEASURE_WITH_UNIT\s*\(\s*(?:LENGTH_MEASURE\s*\(\s*)?([-+0-9.eEdD]+)\s*\)?\s*,\s*#(\d+)", measure)
        if not value:
            return None, f"conversion unit '{conv.group(1)}' has no readable length measure"
        base = _entity_text(text, value.group(2), near)
        if base is None:
            return None, f"conversion unit '{conv.group(1)}' references a missing base unit"
        base_factor, problem = _statement_factor(text, base, near, depth + 1)
        if base_factor is None:
            return None, problem
        return float(value.group(1).replace("D", "E").replace("d", "e")) * base_factor, None
    return None, "length unit is neither SI_UNIT nor CONVERSION_BASED_UNIT"


def text_length_units(path: Path) -> tuple[list[float], list[str]]:
    """Distinct length-unit factors (metres) the file's contexts declare, and problems.

    Only units named by a GLOBAL_UNIT_ASSIGNED_CONTEXT count; the base unit a
    conversion-based unit points at (for example the millimetre under an inch)
    is not a declared unit of the file.
    """
    text = path.read_bytes().decode("latin-1")
    factors: list[float] = []
    problems: list[str] = []
    cache: dict[str, str | None] = {}
    for context in re.finditer(r"GLOBAL_UNIT_ASSIGNED_CONTEXT\s*\(\s*\(([^)]*)\)\s*\)", text):
        for number in re.findall(r"#(\d+)", context.group(1)):
            if number not in cache:
                cache[number] = _entity_text(text, number, context.start())
            statement = cache[number]
            if statement is None or not re.search(r"LENGTH_UNIT\s*\(\s*\)", statement):
                continue
            factor, problem = _statement_factor(text, statement, context.start())
            if factor is None:
                problems.append(problem or "length unit unreadable")
            elif not any(math.isclose(factor, seen, rel_tol=1e-12) for seen in factors):
                factors.append(factor)
    return sorted(factors), sorted(set(problems))


def ocp_length_units(reader) -> tuple[list[float], list[str]]:
    from OCP.TColStd import TColStd_SequenceOfAsciiString

    lengths = TColStd_SequenceOfAsciiString()
    angles = TColStd_SequenceOfAsciiString()
    solid_angles = TColStd_SequenceOfAsciiString()
    reader.FileUnits(lengths, angles, solid_angles)
    factors: list[float] = []
    problems: list[str] = []
    for i in range(1, lengths.Length() + 1):
        name = lengths.Value(i).ToCString()
        if not name:
            continue  # the reader lists an empty name for a context with no declared unit
        factor = OCP_NAMES.get(name.lower())
        if factor is None:
            problems.append(f"OpenCASCADE reports the unit '{name}', which FARIS does not recognise")
        elif not any(math.isclose(factor, seen, rel_tol=1e-12) for seen in factors):
            factors.append(factor)
    return sorted(factors), sorted(set(problems))


def read_step(path: Path):
    """Open the STEP file with names. Returns (reader, solids, names_by_partner)."""
    from OCP.STEPCAFControl import STEPCAFControl_Reader
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDataStd import TDataStd_Name
    from OCP.TDF import TDF_LabelSequence
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopAbs import TopAbs_SOLID
    from OCP.TopExp import TopExp_Explorer
    from OCP.XCAFApp import XCAFApp_Application
    from OCP.XCAFDoc import XCAFDoc_DocumentTool
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.Interface import Interface_Static

    Interface_Static.SetCVal_s("xstep.cascade.unit", INTERNAL_UNIT)
    application = XCAFApp_Application.GetApplication_s()
    document = TDocStd_Document(TCollection_ExtendedString("XmlOcaf"))
    application.NewDocument(TCollection_ExtendedString("MDTV-XCAF"), document)
    reader = STEPCAFControl_Reader()
    reader.SetNameMode(True)
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        raise RuntimeError("OpenCASCADE could not read the STEP file")
    if not reader.Transfer(document):
        raise RuntimeError("OpenCASCADE could not transfer the STEP file's shapes")
    shape = reader.Reader().OneShape()
    solids = []
    explorer = TopExp_Explorer(shape, TopAbs_SOLID)
    while explorer.More():
        solids.append(explorer.Current())
        explorer.Next()
    tool = XCAFDoc_DocumentTool.ShapeTool_s(document.Main())
    labels = TDF_LabelSequence()
    tool.GetShapes(labels)
    named = []
    for i in range(1, labels.Length() + 1):
        label = labels.Value(i)
        attribute = TDataStd_Name()
        if label.FindAttribute(TDataStd_Name.GetID_s(), attribute):
            named.append((tool.GetShape_s(label), attribute.Get().ToExtString()))
    return reader, solids, named


def solid_name(solid, named) -> str | None:
    for shape, name in named:
        if shape.ShapeType() == solid.ShapeType() and solid.IsPartner(shape):
            if not name.strip() or SYNTHETIC_NAME.match(name):
                return None
            return name
    return None


def measure(solid, factor: float | None) -> dict:
    """Volume, centroid and bounding box of one solid, in metres (None when no unit)."""
    from OCP.Bnd import Bnd_Box
    from OCP.BRepBndLib import BRepBndLib
    from OCP.BRepCheck import BRepCheck_Analyzer
    from OCP.BRepGProp import BRepGProp
    from OCP.GProp import GProp_GProps

    props = GProp_GProps()
    error = BRepGProp.VolumeProperties_s(solid, props, VOLUME_EPS, False, False)
    centre = props.CentreOfMass()
    box = Bnd_Box()
    BRepBndLib.AddOptimal_s(solid, box, False, False)
    low = box.CornerMin()
    high = box.CornerMax()
    native = {
        "volume": props.Mass(),
        "centroid": [centre.X(), centre.Y(), centre.Z()],
        "min": [low.X(), low.Y(), low.Z()],
        "max": [high.X(), high.Y(), high.Z()],
    }
    out = {
        "gprop_relative_error_estimate": error,
        "occt_valid": bool(BRepCheck_Analyzer(solid).IsValid()),
        "_native": native,
    }
    if factor is None:
        out.update(cad_volume_m3=None, centroid_m=None, bbox_min_m=None, bbox_max_m=None)
    else:
        out.update(
            cad_volume_m3=native["volume"] * factor**3,
            centroid_m=[v * factor for v in native["centroid"]],
            bbox_min_m=[v * factor for v in native["min"]],
            bbox_max_m=[v * factor for v in native["max"]],
        )
    return out


def versions() -> dict:
    import OCP

    try:
        import cadquery

        cadquery_version = cadquery.__version__
    except Exception:  # cadquery is not needed to read the file
        cadquery_version = None
    ocp_version = getattr(OCP, "__version__", None)
    return {
        "python": sys.version.split()[0],
        "interpreter": sys.executable,
        "cadquery": cadquery_version,
        "ocp": ocp_version,
        "occt": ".".join(ocp_version.split(".")[:2]) if ocp_version else None,
        "helper": HELPER_VERSION,
    }


def inspect_step(path: Path, assume_unit: str | None) -> dict:
    report: dict = {
        "schema": SCHEMA,
        "versions": versions(),
        "step": {"file_name": path.name, "sha256": sha256_file(path), "size_bytes": path.stat().st_size},
        "unit": None,
        "solids": [],
        "model_bbox_diagonal_m": None,
        "volume_method": f"BRepGProp.VolumeProperties adaptive, eps={VOLUME_EPS:g}",
        "problems": [],
        "timing_seconds": {},
    }
    started = time.monotonic()
    text_factors, text_problems = text_length_units(path)
    timing = report["timing_seconds"]
    timing["unit_text_parse"] = time.monotonic() - started
    started = time.monotonic()
    try:
        reader, solids, named = read_step(path)
    except Exception as error:
        report["problems"].append(f"STEP read failed: {type(error).__name__}: {error}")
        report["unit"] = {"declared": None, "factor_to_m": None, "used": None, "text_units": [describe_factor(f) for f in text_factors], "ocp_units": [], "problems": text_problems}
        return report
    timing["read_and_transfer"] = time.monotonic() - started
    ocp_factors, ocp_problems = ocp_length_units(reader.Reader())
    unit_problems = list(text_problems) + list(ocp_problems)
    declared = None
    factor = None
    if len(text_factors) > 1 or len(ocp_factors) > 1:
        unit_problems.append(
            "the file declares several different length units: text "
            f"{[describe_factor(f) for f in text_factors]}, OpenCASCADE {[describe_factor(f) for f in ocp_factors]}"
        )
    elif len(text_factors) != len(ocp_factors) or any(
        not math.isclose(a, b, rel_tol=1e-12) for a, b in zip(text_factors, ocp_factors)
    ):
        unit_problems.append(
            "the two unit readers disagree: text parse "
            f"{[describe_factor(f) for f in text_factors] or 'no unit'}, OpenCASCADE "
            f"{[describe_factor(f) for f in ocp_factors] or 'no unit'}"
        )
    elif text_factors:
        factor = text_factors[0]
        declared = describe_factor(factor)
    used = None
    used_factor = None  # metres per shape unit
    if not unit_problems:
        if declared is not None:
            used, used_factor = declared, INTERNAL_FACTOR
        elif assume_unit is not None:
            used, used_factor = assume_unit, UNIT_FACTORS[assume_unit]
    report["unit"] = {
        "declared": declared,
        "factor_to_m": factor,
        "used": used,
        "used_is_assumed": declared is None and used is not None,
        "text_units": [describe_factor(f) for f in text_factors],
        "ocp_units": [describe_factor(f) for f in ocp_factors],
        "problems": unit_problems,
    }
    lows, highs = [], []
    started = time.monotonic()
    for index, solid in enumerate(solids):
        item = measure(solid, used_factor)
        native = item.pop("_native")
        lows.append(native["min"])
        highs.append(native["max"])
        report["solids"].append({"step_index": index, "step_name": solid_name(solid, named), **item})
    timing["measure_solids"] = time.monotonic() - started
    if used_factor is not None and lows:
        low = [min(v[i] for v in lows) for i in range(3)]
        high = [max(v[i] for v in highs) for i in range(3)]
        report["model_bbox_min_m"] = [v * used_factor for v in low]
        report["model_bbox_max_m"] = [v * used_factor for v in high]
        report["model_bbox_diagonal_m"] = math.dist(low, high) * used_factor
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    inspect = sub.add_parser("step-inspect", help="report a STEP file's hash, unit and solids")
    inspect.add_argument("step", type=Path)
    inspect.add_argument("--out", type=Path, required=True)
    inspect.add_argument("--assume-unit", choices=["m", "cm", "mm"], default=None,
                         help="length unit to use when the file declares none (the design file's cad.length_unit)")
    args = parser.parse_args(argv)
    if not args.step.is_file():
        print(f"faris_cad: STEP file not found: {args.step}", file=sys.stderr)
        return 2
    report = inspect_step(args.step, args.assume_unit)
    args.out.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
