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

Stage S1b adds the commands behind import checks 6 to 8. Each runs as a bounded
job under the FARIS runner and writes a JSON report (and, where it makes
geometry, `faris-facets/v1` files, see faris_facets.py):

    convert          cad_to_dagmc, one material tag per solid, imprint on; records
                     tolerances, versions, time, peak memory, hashes; exports the
                     h5m as facets (check 6)
    export-facets    an existing h5m to facets
    tessellate       every solid meshed on its own with BRepMesh_IncrementalMesh, not
                     imprinted, as facets with the unimprinted flag (candidate pairs
                     for the overlap test); each solid's mesh is checked for closure
    overlap-booleans OCCT BRepAlgoAPI_Common on candidate solid pairs, in worker
                     processes (GEO-021 before imprinting)
    deviation        triangle centroids and edge midpoints (each edge once) against the owning solid:
                     closed form on plane, cylinder, cone, sphere and torus faces,
                     OpenCASCADE projection on the rest (GEO-032, faris_deviation.py)
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import math
import os
import re
import resource
import sys
import time
from pathlib import Path

# One thread per process for numpy's BLAS: the heavy commands use worker processes.
for _variable in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ.setdefault(_variable, "1")

sys.path.insert(0, str(Path(__file__).resolve().parent))

SCHEMA = "faris-step-inspect/v1"
HELPER_VERSION = "3"
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


# ---------------------------------------------------------------------------
# Stage S1b commands.

CONVERSION_SCHEMA = "faris-conversion/v1"
TESSELLATION_SCHEMA = "faris-tessellation/v1"
BOOLEANS_SCHEMA = "faris-overlap-booleans/v1"
DEVIATION_SCHEMA = "faris-deviation/v1"


def peak_rss_mb() -> float:
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0


def tool_versions() -> dict:
    from importlib.metadata import PackageNotFoundError, version

    out = {}
    for label, dist in [
        ("cad_to_dagmc", "cad_to_dagmc"),
        ("mesher", "cad_to_dagmc_mesher"),
        ("cadquery", "cadquery"),
        ("ocp", "cadquery_ocp"),
        ("gmsh", "gmsh"),
        ("h5py", "h5py"),
        ("numpy", "numpy"),
    ]:
        try:
            out[label] = version(dist)
        except PackageNotFoundError:
            out[label] = None
    base = versions()
    out["occt"] = base["occt"]
    out["python"] = base["python"]
    out["interpreter"] = base["interpreter"]
    out["helper"] = HELPER_VERSION
    return out


@contextlib.contextmanager
def quiet(log_path: Path):
    """Send everything written to file descriptors 1 and 2, including by native code, to a log.

    The mesher prints a line per surface; the runner caps captured output, so the
    chatter goes to a file the report names.
    """
    sys.stdout.flush()
    sys.stderr.flush()
    saved = (os.dup(1), os.dup(2))
    log = os.open(str(log_path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    os.dup2(log, 1)
    os.dup2(log, 2)
    try:
        yield
    finally:
        sys.stdout.flush()
        sys.stderr.flush()
        os.dup2(saved[0], 1)
        os.dup2(saved[1], 2)
        for fd in (*saved, log):
            os.close(fd)


def log_tail(path: Path, lines: int = 12) -> list[str]:
    try:
        return path.read_text(errors="replace").splitlines()[-lines:]
    except OSError:
        return []


def _solids_of(shape) -> list:
    from OCP.TopAbs import TopAbs_SOLID
    from OCP.TopExp import TopExp_Explorer

    solids = []
    explorer = TopExp_Explorer(shape, TopAbs_SOLID)
    while explorer.More():
        solids.append(explorer.Current())
        explorer.Next()
    return solids


def load_solids(step: Path, cache: Path | None = None):
    """The STEP solids in read order (the order step-inspect numbers them).

    Reading a large STEP file takes about a minute (RM-M: 52 s) and every helper command needs
    the solids, so with a `cache` directory the solids are kept there as one BREP, keyed by the
    STEP file's sha256 and the helper version, and read back in about a second.
    """
    from OCP.BRep import BRep_Builder
    from OCP.BRepTools import BRepTools
    from OCP.TopoDS import TopoDS_Compound, TopoDS_Shape

    key = None
    if cache is not None:
        brep, meta = cache / "solids.brep", cache / "solids.json"
        key = {"step_sha256": sha256_file(step), "helper": HELPER_VERSION}
        if brep.is_file() and meta.is_file():
            try:
                recorded = json.loads(meta.read_text())
            except ValueError:
                recorded = {}
            if all(recorded.get(k) == v for k, v in key.items()) and recorded.get("brep_bytes") == brep.stat().st_size:
                shape = TopoDS_Shape()
                if BRepTools.Read_s(shape, str(brep), BRep_Builder()):
                    solids = _solids_of(shape)
                    if len(solids) == recorded.get("solids"):
                        return solids
    _reader, solids, _named = read_step(step)
    if cache is not None:
        cache.mkdir(parents=True, exist_ok=True)
        compound, builder = TopoDS_Compound(), BRep_Builder()
        builder.MakeCompound(compound)
        for solid in solids:
            builder.Add(compound, solid)
        temporary = brep.with_suffix(".tmp")
        if BRepTools.Write_s(compound, str(temporary)):
            temporary.replace(brep)
            meta.write_text(json.dumps({**key, "solids": len(solids), "brep_bytes": brep.stat().st_size}) + "\n")
    return solids


def convert(args) -> int:
    """Check 6: cad_to_dagmc as a bounded job, one tag per solid, imprint on."""
    import faris_facets

    out_dir = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    log = out_dir / "convert.log"
    tags = sorted(json.loads(args.tags.read_text()), key=lambda e: e["step_index"])
    if [e["step_index"] for e in tags] != list(range(len(tags))):
        print("faris_cad: the tag list must cover step indices 0..n-1 once each", file=sys.stderr)
        return 2
    h5m = out_dir / "model.h5m"
    timing = {}
    started = time.monotonic()
    scale_to_cm = args.shape_unit_m * 100.0
    # cad_to_dagmc's own pre-imprint overlap scan duplicates GEO-021 (check 6's pre-imprint booleans, which
    # cover every candidate pair) and stops after 60 s; on RM-M it spent its budget and checked part of the
    # pairs. FARIS relies on GEO-021, so the duplicate scan is given no time.
    os.environ["YAMM_OVERLAP_BUDGET_S"] = "0"
    with quiet(log):
        import cad_to_dagmc
        import cadquery as cq

        t = time.monotonic()
        solids = load_solids(args.step, args.shape_cache)
        timing["read_step"] = time.monotonic() - t
        if len(solids) != len(tags):
            print(f"STEP holds {len(solids)} solids but {len(tags)} tags were given", file=sys.stderr)
            return 3
        model = cad_to_dagmc.CadToDagmc()
        for entry in tags:
            solid = cq.Shape.cast(solids[entry["step_index"]])
            model.add_cadquery_object(cq.Workplane("XY").add(solid), material_tags=[entry["tag"]])
        t = time.monotonic()
        model.export_dagmc_h5m_file(
            filename=str(h5m),
            implicit_complement_material_tag=args.implicit_complement,
            scale_factor=scale_to_cm,
            imprint=args.imprint_threads,
            meshing_backend="cad-to-dagmc-mesher",
            tolerance=args.chord_cm,
            angular_tolerance=args.angular,
        )
        timing["convert"] = time.monotonic() - t
    conversion_peak = peak_rss_mb()
    t = time.monotonic()
    exported = faris_facets.export_h5m(h5m, out_dir / "model", 0.01)
    timing["export_facets"] = time.monotonic() - t
    timing["total"] = time.monotonic() - started
    manifest = exported["manifest"]
    report = {
        "schema": CONVERSION_SCHEMA,
        "step": {"file_name": args.step.name, "sha256": sha256_file(args.step)},
        "tolerances": {
            "chord_cm": args.chord_cm,
            "angular_rad": args.angular,
            "scale_factor_to_cm": scale_to_cm,
            "imprint_threads": args.imprint_threads,
            "cad_to_dagmc_overlap_scan": "off; GEO-021 covers it",
            "implicit_complement_tag": args.implicit_complement,
        },
        "tags": tags,
        "versions": tool_versions(),
        "timing_seconds": timing,
        "peak_rss_mb": max(conversion_peak, peak_rss_mb()),
        "peak_rss_mb_after_conversion": conversion_peak,
        "h5m": {"file": h5m.name, "sha256": sha256_file(h5m), "bytes": h5m.stat().st_size},
        "facets": {
            "bin": Path(exported["bin"]).name,
            "json": Path(exported["json"]).name,
            "bin_sha256": manifest["binary_sha256"],
        },
        "counts": manifest["counts"],
        "log_tail": log_tail(log),
    }
    args.report.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


def export_facets_command(args) -> int:
    import faris_facets

    exported = faris_facets.export_h5m(args.h5m, args.out_base, args.scale_to_m)
    print(json.dumps({"bin": exported["bin"], "json": exported["json"]}))
    return 0


# Welding the per-face nodes of a solid into shared vertices: nodes within this many shape units
# (nanometres for millimetres) are one vertex. Edge nodes are shared by construction, so this
# only joins what OpenCASCADE already put at the same place.
WELD = 1.0e-6

_SHARED: dict = {}


def _mesh_solid(job) -> dict:
    """Mesh one solid with BRepMesh_IncrementalMesh and return it as welded vertices and faces.

    The solid is meshed in this process only; shapes are shared with the parent by fork.
    """
    import numpy as np
    from OCP.BRep import BRep_Tool
    from OCP.BRepMesh import BRepMesh_IncrementalMesh
    from OCP.TopAbs import TopAbs_FACE, TopAbs_REVERSED
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopLoc import TopLoc_Location
    from OCP.TopoDS import TopoDS

    index, chord, angular = job
    solid = _SHARED["solids"][index]
    started = time.monotonic()
    mesher = BRepMesh_IncrementalMesh(solid, chord, False, angular, False)
    if not mesher.IsDone():
        return {"index": index, "error": "OpenCASCADE could not mesh the solid"}
    mesh_seconds = time.monotonic() - started
    nodes, faces, missing = [], [], []
    offset = 0
    explorer = TopExp_Explorer(solid, TopAbs_FACE)
    number = 0
    while explorer.More():
        number += 1
        face = TopoDS.Face_s(explorer.Current())
        explorer.Next()
        location = TopLoc_Location()
        triangulation = BRep_Tool.Triangulation_s(face, location)
        if triangulation is None or triangulation.NbTriangles() == 0:
            missing.append(number)
            continue
        transform = None if location.IsIdentity() else location.Transformation()
        points = []
        for i in range(1, triangulation.NbNodes() + 1):
            p = triangulation.Node(i)
            if transform is not None:
                p = p.Transformed(transform)
            points.append((p.X(), p.Y(), p.Z()))
        triangles = np.array([triangulation.Triangle(i).Get() for i in range(1, triangulation.NbTriangles() + 1)], dtype=np.int64) - 1
        if face.Orientation() == TopAbs_REVERSED:
            triangles = triangles[:, [0, 2, 1]]
        nodes.append(np.array(points, dtype=np.float64))
        faces.append((number, triangles + offset))
        offset += len(points)
    result = {"index": index, "missing_faces": missing, "mesh_seconds": mesh_seconds, "faces": 0}
    if not faces:
        result["error"] = "the solid has no meshed face"
        return result
    allnodes = np.concatenate(nodes)
    keys = np.round(allnodes / WELD).astype(np.int64)
    _unique, first, inverse = np.unique(keys, axis=0, return_index=True, return_inverse=True)
    inverse = inverse.reshape(-1)
    result["vertices"] = allnodes[first]
    welded, dropped = [], 0
    for number, triangles in faces:
        t = inverse[triangles]
        keep = (t[:, 0] != t[:, 1]) & (t[:, 1] != t[:, 2]) & (t[:, 0] != t[:, 2])
        dropped += int((~keep).sum())
        welded.append((number, t[keep]))
    result["faces"] = welded
    result["collapsed_triangles"] = dropped
    result["seconds"] = time.monotonic() - started
    return result


def open_edge_report(triangles) -> tuple[int, int, int | None]:
    """(edges used once in their direction without a partner, edges used more than once, first bad triangle).

    A mesh is closed and consistently oriented when every directed edge is used exactly once and
    its reverse exactly once.
    """
    import numpy as np

    a = triangles[:, [0, 1, 2]].ravel()
    b = triangles[:, [1, 2, 0]].ravel()
    width = int(triangles.max()) + 1
    forward = a * width + b
    backward = b * width + a
    codes, counts = np.unique(forward, return_counts=True)
    repeated = int((counts > 1).sum())
    partnered = np.isin(backward, codes)
    unpaired = int((~partnered).sum())
    first = int(np.nonzero(~partnered | np.isin(forward, codes[counts > 1]))[0][0]) // 3 if (unpaired or repeated) else None
    return unpaired, repeated, first


def tessellate(args) -> int:
    """Mesh every solid on its own with BRepMesh_IncrementalMesh and export it as unimprinted facets.

    A solid is meshed alone: the converter's mesher refuses an assembly whose solids overlap, and
    an overlap is what the candidate-pair search has to find from these meshes. Each solid's mesh
    must be closed; one that is not is reported as a problem and left as it is.
    """
    import multiprocessing

    import faris_facets
    import numpy as np

    out_dir = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    log = out_dir / "tessellate.log"
    tags = sorted(json.loads(args.tags.read_text()), key=lambda e: e["step_index"])
    scale_to_cm = args.shape_unit_m * 100.0
    chord = args.chord_cm / scale_to_cm  # shape units
    started = time.monotonic()
    timing = {}
    with quiet(log):
        t = time.monotonic()
        solids = load_solids(args.step, args.shape_cache)
        timing["read_step"] = time.monotonic() - t
        if len(solids) != len(tags):
            print(f"STEP holds {len(solids)} solids but {len(tags)} tags were given", file=sys.stderr)
            return 3
        _SHARED["solids"] = [solids[e["step_index"]] for e in tags]
        jobs = [(k, chord, args.angular) for k in range(len(tags))]
        t = time.monotonic()
        if args.workers > 1:
            with multiprocessing.get_context("fork").Pool(args.workers) as pool:
                meshed = pool.map(_mesh_solid, jobs, chunksize=1)
        else:
            meshed = [_mesh_solid(job) for job in jobs]
        timing["mesh"] = time.monotonic() - t
    problems = []
    vertex_blocks, blocks, surfaces, surface_ids = [], [], [], []
    offset, first = 0, 0
    collapsed = {}
    for entry, mesh in zip(tags, meshed):
        volume = entry["step_index"] + 1
        label = f"solid {entry['step_index']} ({entry['tag']})"
        if "error" in mesh:
            problems.append(f"{label}: {mesh['error']}")
            continue
        if mesh["missing_faces"]:
            problems.append(f"{label}: {len(mesh['missing_faces'])} face(s) have no tessellation (first: face {mesh['missing_faces'][0]})")
        all_triangles = np.concatenate([t for _n, t in mesh["faces"]])
        vertices = mesh["vertices"] * args.shape_unit_m
        unpaired, repeated, bad = open_edge_report(all_triangles)
        if unpaired or repeated:
            at = vertices[all_triangles[bad]].mean(axis=0)
            problems.append(
                f"{label}: the tessellation is not closed: {unpaired} edge(s) have no partner triangle on the other side and "
                f"{repeated} edge(s) are used more than once in one direction (first near x={at[0]:.6g} y={at[1]:.6g} z={at[2]:.6g} m)"
            )
        if mesh["collapsed_triangles"]:
            collapsed[label] = mesh["collapsed_triangles"]
        p = vertices[all_triangles]
        volume_m3 = float(np.einsum("ij,ij->i", p[:, 0], np.cross(p[:, 1], p[:, 2])).sum() / 6.0)
        if volume_m3 <= 0.0:
            problems.append(f"{label} has a faceted volume of {volume_m3:g} (not positive), so its triangles are not oriented outward")
        vertex_blocks.append(vertices)
        for number, triangles in mesh["faces"]:
            if len(triangles) == 0:
                continue
            blocks.append((triangles + offset).astype(np.uint32))
            surfaces.append((first, len(triangles), volume, 0))
            surface_ids.append(number)
            first += len(triangles)
        offset += len(vertices)
    if not blocks:
        print("faris_cad: no solid could be meshed", file=sys.stderr)
        return 4
    vertices = np.concatenate(vertex_blocks)
    triangles = np.concatenate(blocks)
    per_volume = faris_facets.volumes_from_surfaces(surfaces, len(tags))
    volumes = [
        {"index": e["step_index"] + 1, "global_id": e["step_index"] + 1, "material_tag": "mat:" + e["tag"], "triangles": per_volume[e["step_index"]]}
        for e in tags
    ]
    binary = faris_facets.pack_facets(vertices, triangles, surfaces, len(tags), faris_facets.FLAG_UNIMPRINTED)
    manifest = faris_facets.make_manifest(binary, volumes, surface_ids, "step", sha256_file(args.step), len(vertices), len(triangles), len(surfaces), faris_facets.FLAG_UNIMPRINTED)
    bin_path, json_path = faris_facets.write_pair(out_dir / "solids", binary, manifest)
    timing["total"] = time.monotonic() - started
    report = {
        "schema": TESSELLATION_SCHEMA,
        "versions": tool_versions(),
        "method": "BRepMesh_IncrementalMesh on each solid alone, nodes welded by position, closedness checked per solid",
        "tolerances": {"chord_cm": args.chord_cm, "angular_rad": args.angular, "scale_factor_to_cm": scale_to_cm},
        "timing_seconds": timing,
        "peak_rss_mb": peak_rss_mb(),
        "facets": {"bin": bin_path.name, "json": json_path.name, "bin_sha256": manifest["binary_sha256"]},
        "counts": manifest["counts"],
        "problems": problems,
        "collapsed_triangles": collapsed,
        "log_tail": log_tail(log),
    }
    args.report.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


def _volume_m3(shape, shape_unit_m: float) -> float:
    from OCP.BRepGProp import BRepGProp
    from OCP.GProp import GProp_GProps

    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, props, VOLUME_EPS, False, False)
    return props.Mass() * shape_unit_m**3


def _common_worker(solids, a: int, b: int, shape_unit_m: float, connection) -> None:
    """One boolean in its own process; the answer travels back on a pipe."""
    try:
        from OCP.BRepAlgoAPI import BRepAlgoAPI_Common
        from OCP.BRepCheck import BRepCheck_Analyzer
        from OCP.TopAbs import TopAbs_SOLID
        from OCP.TopExp import TopExp_Explorer

        started = time.monotonic()
        operation = BRepAlgoAPI_Common(solids[a], solids[b])
        if not operation.IsDone():
            connection.send({"status": "undecided", "reason": "OpenCASCADE reported an error in the boolean intersection"})
            return
        result = operation.Shape()
        if result.IsNull():
            connection.send({"status": "decided", "common_volume_m3": 0.0, "seconds": time.monotonic() - started})
            return
        explorer = TopExp_Explorer(result, TopAbs_SOLID)
        if not explorer.More():
            connection.send({"status": "decided", "common_volume_m3": 0.0, "seconds": time.monotonic() - started})
            return
        if not BRepCheck_Analyzer(result).IsValid():
            connection.send({"status": "undecided", "reason": "the boolean intersection returned an invalid shape"})
            return
        connection.send({"status": "decided", "common_volume_m3": _volume_m3(result, shape_unit_m), "seconds": time.monotonic() - started})
    except BaseException as error:  # a crash in a worker must not look like "no overlap"
        connection.send({"status": "undecided", "reason": f"{type(error).__name__}: {error}"})
    finally:
        connection.close()


def overlap_booleans(args) -> int:
    """GEO-021 before imprinting: OCCT's exact intersection on candidate pairs only."""
    import multiprocessing

    pairs = json.loads(args.pairs.read_text())
    solids = load_solids(args.step, args.shape_cache)
    context = multiprocessing.get_context("fork")
    started = time.monotonic()
    results: dict[int, dict] = {}
    running: dict[int, tuple] = {}
    queue = list(range(len(pairs)))

    def launch(index: int) -> None:
        a, b = pairs[index]
        parent, child = context.Pipe(duplex=False)
        process = context.Process(target=_common_worker, args=(solids, a, b, args.shape_unit_m, child))
        process.start()
        child.close()
        running[index] = (process, parent, time.monotonic())

    while queue or running:
        while queue and len(running) < max(1, args.workers):
            launch(queue.pop(0))
        for index in list(running):
            process, connection, since = running[index]
            answer = None
            if connection.poll(0.05):
                try:
                    answer = connection.recv()
                except EOFError:
                    answer = {"status": "undecided", "reason": "the worker process ended without an answer"}
            elif not process.is_alive():
                answer = {"status": "undecided", "reason": f"the worker process ended without an answer (exit {process.exitcode})"}
            elif time.monotonic() - since > args.pair_timeout:
                process.kill()
                answer = {"status": "undecided", "reason": f"the boolean intersection did not finish within {args.pair_timeout:g} s"}
            if answer is not None:
                process.join(5)
                if process.is_alive():
                    process.kill()
                connection.close()
                results[index] = {"a": pairs[index][0], "b": pairs[index][1], **answer}
                del running[index]
    report = {
        "schema": BOOLEANS_SCHEMA,
        "versions": tool_versions(),
        "pairs": [results[i] for i in range(len(pairs))],
        "timing_seconds": {"total": time.monotonic() - started},
        "workers": args.workers,
    }
    args.report.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


def deviation(args) -> int:
    """GEO-032: the largest distance from triangle centroids and edge midpoints to the owning solid.

    Faces on a plane, cylinder, cone, sphere or torus are measured in closed form (faris_deviation),
    every other face by OpenCASCADE on an even stride of at most `--occt-points` points per solid.
    The report gives each solid's coverage, so the result reads "maximum over all points" or
    "sampled maximum".
    """
    import numpy as np

    import faris_deviation

    mapping = json.loads(args.map.read_text())  # [{"volume": 1, "step_index": 0}, ...]
    data = args.facets.read_bytes()
    nv, nt, ns = np.frombuffer(data, dtype="<u8", count=3, offset=16)
    offset = 48
    vertices = np.frombuffer(data, dtype="<f8", count=3 * int(nv), offset=offset).reshape(-1, 3)
    offset += 24 * int(nv)
    triangles = np.frombuffer(data, dtype="<u4", count=3 * int(nt), offset=offset).reshape(-1, 3)
    offset += 12 * int(nt)
    surfaces = np.frombuffer(data, dtype=[("first", "<u8"), ("count", "<u8"), ("fwd", "<u4"), ("rev", "<u4")], count=int(ns), offset=offset)
    started = time.monotonic()
    timing = {}
    solids = load_solids(args.step, args.shape_cache)
    timing["read_step"] = time.monotonic() - started
    to_shape = 1.0 / args.shape_unit_m
    point_sets, faces = [], []
    t = time.monotonic()
    for entry in mapping:
        volume, index = entry["volume"], entry["step_index"]
        owned = np.nonzero((surfaces["fwd"] == volume) | (surfaces["rev"] == volume))[0]
        picks = [np.arange(int(surfaces["first"][i]), int(surfaces["first"][i]) + int(surfaces["count"][i])) for i in owned]
        corners = triangles[np.concatenate(picks)]
        p = vertices[corners] * to_shape  # (n, 3 corners, 3)
        # Edge midpoints once per edge: neighbouring triangles share them.
        edges = np.sort(np.concatenate([corners[:, [0, 1]], corners[:, [1, 2]], corners[:, [2, 0]]]), axis=1)
        edges = np.unique(edges, axis=0)
        mids = (vertices[edges[:, 0]] + vertices[edges[:, 1]]) / 2.0 * to_shape
        point_sets.append(np.concatenate([p.mean(axis=1), mids]))
        faces.append(faris_deviation.SolidFaces(solids[index]))
    timing["faces"] = time.monotonic() - t
    t = time.monotonic()
    rows, phases = faris_deviation.measure(faces, point_sets, workers=args.workers, occt_points=args.occt_points, block=args.block_points)
    timing["measure"] = time.monotonic() - t
    timing.update({f"measure_{k}": v for k, v in phases.items()})
    per_solid = []
    for entry, row, solid in zip(mapping, rows, faces):
        out = {
            "volume": entry["volume"],
            "step_index": entry["step_index"],
            "points": row["points"],
            "points_available": row["points_available"],
            "points_closed_form": row["points_closed_form"],
            "points_occt": row["points_occt"],
            "points_occt_needed": row["points_occt_needed"],
            "occt_calls": row["occt_calls"],
            "faces": len(solid),
            "faces_closed_form": int(solid.analytic.sum()),
            "max_distance_m": row["worst"] * args.shape_unit_m,
        }
        if row["at"] is not None:
            out["at_m"] = [c * args.shape_unit_m for c in row["at"]]
        per_solid.append(out)
    timing["total"] = time.monotonic() - started
    report = {
        "schema": DEVIATION_SCHEMA,
        "versions": tool_versions(),
        "method": (
            "closed-form distance to plane, cylinder, cone, sphere and torus faces (accepted when the foot point is in the face, "
            "by OpenCASCADE's face classifier); BRepExtrema_DistShapeShape for other faces and for points whose foot point is outside the face"
        ),
        "solids": per_solid,
        "timing_seconds": timing,
        "workers": args.workers,
        "occt_points": args.occt_points,
        "peak_rss_mb": peak_rss_mb(),
    }
    args.report.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    inspect = sub.add_parser("step-inspect", help="report a STEP file's hash, unit and solids")
    inspect.add_argument("step", type=Path)
    inspect.add_argument("--out", type=Path, required=True)
    inspect.add_argument("--assume-unit", choices=["m", "cm", "mm"], default=None,
                         help="length unit to use when the file declares none (the design file's cad.length_unit)")

    def add_step_arguments(sub_parser, report=True):
        sub_parser.add_argument("step", type=Path)
        sub_parser.add_argument("--shape-unit-m", type=float, required=True,
                                help="metres per unit of the shapes as OpenCASCADE reads them (1e-3 for a file that declares a unit)")
        sub_parser.add_argument("--shape-cache", type=Path, default=None,
                                help="a directory that keeps the STEP solids as BREP, keyed by the STEP sha256, so later commands skip reading the STEP file")
        if report:
            sub_parser.add_argument("--report", type=Path, required=True)

    conv = sub.add_parser("convert", help="cad_to_dagmc with one material tag per solid; exports facets (check 6)")
    add_step_arguments(conv)
    conv.add_argument("--tags", type=Path, required=True, help='JSON list of {"step_index": n, "tag": "<solid id>"}')
    conv.add_argument("--out-dir", type=Path, required=True)
    conv.add_argument("--chord-cm", type=float, default=0.1)
    conv.add_argument("--angular", type=float, default=0.1)
    conv.add_argument("--imprint-threads", type=int, default=4)
    conv.add_argument("--implicit-complement", default="void")

    exp = sub.add_parser("export-facets", help="export a DAGMC h5m as faris-facets/v1")
    exp.add_argument("h5m", type=Path)
    exp.add_argument("--out-base", type=Path, required=True)
    exp.add_argument("--scale-to-m", type=float, default=0.01)

    tess = sub.add_parser("tessellate", help="mesh every solid on its own (not imprinted) as unimprinted facets")
    add_step_arguments(tess)
    tess.add_argument("--tags", type=Path, required=True)
    tess.add_argument("--out-dir", type=Path, required=True)
    tess.add_argument("--chord-cm", type=float, default=0.1)
    tess.add_argument("--angular", type=float, default=0.1)
    tess.add_argument("--workers", type=int, default=3, help="solids meshed at once, in worker processes")

    booleans = sub.add_parser("overlap-booleans", help="OCCT boolean intersection of candidate solid pairs")
    add_step_arguments(booleans)
    booleans.add_argument("--pairs", type=Path, required=True, help="JSON list of [step_index, step_index]")
    booleans.add_argument("--workers", type=int, default=3)
    booleans.add_argument("--pair-timeout", type=float, default=300.0)

    dev = sub.add_parser("deviation", help="faceting deviation of an imprinted model from the CAD solids (GEO-032)")
    add_step_arguments(dev)
    dev.add_argument("--facets", type=Path, required=True, help="the imprinted model's .facets.bin")
    dev.add_argument("--map", type=Path, required=True, help='JSON list of {"volume": n, "step_index": m}')
    dev.add_argument("--workers", type=int, default=3)
    dev.add_argument("--block-points", type=int, default=512, help="points measured together in closed form")
    dev.add_argument("--occt-points", type=int, default=2000,
                     help="points per solid measured by OpenCASCADE, on an even stride (0: all of them)")

    args = parser.parse_args(argv)
    if args.command == "export-facets":
        return export_facets_command(args)
    if not args.step.is_file():
        print(f"faris_cad: STEP file not found: {args.step}", file=sys.stderr)
        return 2
    if args.command == "convert":
        return convert(args)
    if args.command == "tessellate":
        return tessellate(args)
    if args.command == "overlap-booleans":
        return overlap_booleans(args)
    if args.command == "deviation":
        return deviation(args)
    report = inspect_step(args.step, args.assume_unit)
    args.out.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
