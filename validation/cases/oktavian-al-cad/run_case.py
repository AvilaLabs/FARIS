#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""OKTAVIAN aluminium sphere from the OpenMC-Fusion-Benchmarks STEP file, through FARIS's CAD-to-DAGMC conversion.

Subcommands:

  plan     print what a run would do and check every pinned file hash and the library coverage (no OpenMC, no CPU)
  convert  read the STEP file with faris_cad.py (step-inspect, then convert), find the tally surface in the
           faceted result, write the conversion directory
  run      build the OpenMC model on that conversion, run transport, write <case>.run.json (needs --i-have-the-cpu)

The model follows the OFB specification (materials, point source, energy histogram, tally energy edges, photon
transport) as written into the manifest's `model` block, with the deviations listed in the manifest's `protocol`
block. The tally is the outward partial current from the outer void volume across the DAGMC surface that borders the
implicit complement, summed over that whole surface; the net current is recorded as a check. Values are divided by the
lethargy width (neutrons) or the MeV width (photons) and by nothing else (docs/VALIDATION.md).

`convert` runs the CAD helper in the CAD Python (--cad-python); `run` needs the OpenMC 0.15.3 Python with DAGMC.
Neither step starts unless the pinned files hash as the manifest says.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "integrations" / "cad"))
from validation.manifest import load_manifest, manifest_sha256, verify_files  # noqa: E402
from validation.scoring import RUN_KIND, seal_run_record  # noqa: E402

CASE_ID = "oktavian-al-cad"
OPENMC_VERSION = "0.15.3"
FARIS_CAD = REPO / "integrations" / "cad" / "faris_cad.py"
AUDIT = REPO / "references" / "openmc-library-audit.json"
STEP_UNIT_M = 1.0e-3  # the STEP file declares millimetres (step-inspect reads it two ways)
TALLY_VOLUME_ID = 2  # the outer void sphere (OFB volume 2); checked against the faceted conversion
SPHERE_RADIUS_M = 1.0
AGREEMENT_SIGMAS = 3.0
DEFAULT_BATCHES = 100
DEFAULT_SEED = 1
DEFAULT_PARTICLES = 100 * 100_000


def _load(name: str, relative: str):
    spec = importlib.util.spec_from_file_location(name, HERE.parent / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CSG = _load("oktavian_al_run_case", "oktavian-al/run_case.py")  # parse helpers, library report, partial/net check
require = CSG.require
sha256 = CSG.sha256


# ---- pure helpers (tested without OpenMC) ------------------------------------------------

def source_histogram(edges_ev: list[float], bin_probability: list[float]) -> tuple[list[float], list[float]]:
    """OFB's per-bin probabilities to OpenMC Tabular(histogram) x (eV) and p (a density: probability / width; the last entry is unused)."""
    require(len(edges_ev) == len(bin_probability) + 1, "source needs one probability per energy interval")
    require(all(b > a for a, b in zip(edges_ev, edges_ev[1:])), "source edges must increase")
    return list(edges_ev), [p / (edges_ev[i + 1] - edges_ev[i]) for i, p in enumerate(bin_probability)] + [0.0]


def find_tally_surface(vertices, triangles, surfaces: list, volume_id: int, radius_m: float, tolerance: float = 0.01) -> dict:
    """The one faceted surface between `volume_id` and the implicit complement (volume 0), checked to be the sphere of `radius_m`.

    `surfaces` are (first_triangle, triangle_count, forward_volume, reverse_volume) in global-id order starting at 1.
    """
    bordering = [i + 1 for i, s in enumerate(surfaces) if {s[2], s[3]} == {volume_id, 0}]
    require(len(bordering) == 1, f"expected one surface between volume {volume_id} and the implicit complement, found {bordering}")
    sid = bordering[0]
    first, count = surfaces[sid - 1][0], surfaces[sid - 1][1]
    area = 0.0
    r_min, r_max = math.inf, 0.0
    for t in range(first, first + count):
        a, b, c = (vertices[k] for k in triangles[t])
        ab, ac = [b[i] - a[i] for i in range(3)], [c[i] - a[i] for i in range(3)]
        cross = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]]
        area += 0.5 * math.sqrt(sum(x * x for x in cross))
        for v in (a, b, c):
            r = math.sqrt(sum(x * x for x in v))
            r_min, r_max = min(r_min, r), max(r_max, r)
    exact = 4.0 * math.pi * radius_m ** 2
    require(abs(r_min - radius_m) <= tolerance * radius_m and abs(r_max - radius_m) <= tolerance * radius_m,
            f"surface {sid} is not a sphere of radius {radius_m} m (radius range {r_min:.4f} to {r_max:.4f} m)")
    require(abs(area / exact - 1.0) <= tolerance, f"surface {sid} area {area:.5f} m2 differs from the sphere's {exact:.5f} m2 by more than {tolerance:.0%}")
    return {"surface_id": sid, "volume_id": volume_id, "area_cm2": area * 1.0e4, "sphere_area_cm2": exact * 1.0e4, "radius_range_cm": [r_min * 100.0, r_max * 100.0]}


def result_rows(manifest: dict, net: dict, partial: dict) -> list[dict]:
    """Run-record results: the outward partial current of each detector's tally bin over its width, with its Monte Carlo error."""
    out = []
    for det in manifest["detectors"]:
        particle = "neutron" if det["response_class"] == "neutron_leakage_spectrum" else "photon"
        b = det["bin"]
        width = math.log(b["hi_mev"] / b["lo_mev"]) if particle == "neutron" else b["hi_mev"] - b["lo_mev"]
        mean, std = partial[particle]
        i = b["tally_bin"]
        out.append({"detector_id": det["id"], "value": mean[i] / width, "u_mc": std[i] / width})
    return out


def net_vs_partial(partial: list[float], partial_err: list[float], net: list[float], net_err: list[float], sigmas: float = AGREEMENT_SIGMAS) -> dict:
    """The absolute net current against the outward partial current (the sign of a DAGMC surface's net current follows the surface orientation)."""
    sign = sum(n for n in net)
    check = CSG.partial_net_agreement(partial, partial_err, [abs(n) for n in net], net_err, sigmas)
    check["net_sign"] = "negative" if sign < 0 else "positive"
    return check


# ---- plan / convert / run -----------------------------------------------------------------

def check_inputs(root: Path, step: Path, manifest: dict) -> None:
    issues = verify_files(manifest, root)
    require(not issues, "pinned files differ from the manifest: " + "; ".join(issues))
    pointer = next(f for f in manifest["files"] if f.get("lfs_object_sha256"))
    require(sha256(step) == pointer["lfs_object_sha256"], f"{step} does not match the LFS object id in the manifest")


def library(args, manifest: dict) -> tuple[Path, dict]:
    nuclides = sorted({n for m in manifest["model"]["materials"] for n in m["nuclides"]})
    xml = Path(args.cross_sections).expanduser().resolve(strict=True)
    rep = CSG.library_report(xml, nuclides, AUDIT)
    require(not rep["missing_neutron"], f"library lacks neutron data for {rep['missing_neutron']}")
    require(not rep["missing_photon"], f"library lacks photon data for {rep['missing_photon']}")
    uncovered = rep["not_in_audit"]
    if uncovered and args.audit_extension:
        extra = set(json.loads(Path(args.audit_extension).read_text(encoding="utf-8"))["neutron_library"])
        uncovered = [n for n in uncovered if n not in extra]
    require(not uncovered, f"{uncovered} are not in the audited library inventory; run integrations/openmc/audit_library.py and pass --audit-extension")
    return xml, rep


def command_plan(args) -> int:
    manifest = load_manifest(HERE / "manifest.json")
    check_inputs(args.root.expanduser(), args.step.expanduser(), manifest)
    xml, rep = library(args, manifest)
    print(json.dumps({"case_id": CASE_ID, "manifest_sha256": manifest_sha256(manifest), "library": rep, "settings": {
        "particles": args.particles, "batches": args.batches, "seed": args.seed, "threads": args.threads, "temperature_k": 293.6, "photon_transport": True,
        "energy_cutoff_neutron_ev": 1.0e3}, "protocol_histories": manifest["protocol"].get("histories")}, indent=2, sort_keys=True))
    return 0


def command_convert(args) -> int:
    manifest = load_manifest(HERE / "manifest.json")
    step = args.step.expanduser().resolve()
    check_inputs(args.root.expanduser(), step, manifest)
    out = fresh_dir(args.work_dir)
    cad = str(Path(args.cad_python).expanduser())
    inspect = out / "inspect.json"
    subprocess.run([cad, "-I", str(FARIS_CAD), "step-inspect", str(step), "--out", str(inspect)], check=True, capture_output=True)
    solids = json.loads(inspect.read_text(encoding="utf-8"))["solids"]
    want = [(s["step_index"], s["step_name"]) for s in manifest["model"]["step_solids"]]
    require([(s["step_index"], s["step_name"]) for s in solids] == want, f"STEP solids {[(s['step_index'], s['step_name']) for s in solids]} differ from the manifest's {want}")
    tags = out / "tags.json"
    tags.write_text(json.dumps([{"step_index": s["step_index"], "tag": s["tag"]} for s in manifest["model"]["step_solids"]]) + "\n", encoding="utf-8")
    cmd = [cad, "-I", str(FARIS_CAD), "convert", str(step), "--shape-unit-m", str(STEP_UNIT_M), "--tags", str(tags), "--out-dir", str(out),
           "--report", str(out / "report.json"), "--imprint-threads", str(args.threads), "--chord-cm", str(manifest["protocol"]["conversion"]["chord_cm"]),
           "--angular", str(manifest["protocol"]["conversion"]["angular_rad"])]
    started = time.monotonic()
    subprocess.run(cmd, check=True, capture_output=True)
    wall = time.monotonic() - started
    import faris_facets  # noqa: PLC0415  (standard library; never MOAB or OpenMC)
    facets = faris_facets.unpack_facets((out / "model.facets.bin").read_bytes())
    vertices, triangles = facets["vertices"], facets["triangles"]
    surface = find_tally_surface(vertices, triangles, list(facets["surfaces"]), TALLY_VOLUME_ID, SPHERE_RADIUS_M)
    (out / "tally_surface.json").write_text(json.dumps({**surface, "conversion_wall_seconds": wall}, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"conversion_dir": str(out), "wall_seconds": wall, "tally_surface": surface}, indent=2))
    return 0


def build_model(openmc, manifest: dict, h5m: Path, surface_id: int, args):
    spec = manifest["model"]
    materials = {}
    for m in spec["materials"]:
        mat = openmc.Material(material_id=m["id"], name=m["name"])
        for nuclide, fraction in m["nuclides"].items():
            mat.add_nuclide(nuclide, fraction, m["fraction_type"])
        mat.set_density("g/cm3", m["density_g_cm3"])
        mat.temperature = 293.6
        materials[m["name"]] = mat
    mats = openmc.Materials(list(materials.values()))
    universe = openmc.DAGMCUniverse(str(h5m))
    box = universe.bounding_box
    radius = 1.05 * max(math.sqrt(sum(c * c for c in corner)) for corner in (box.lower_left, box.upper_right))
    boundary = openmc.Sphere(surface_id=90000, r=radius, boundary_type="vacuum")
    geometry = openmc.Geometry([openmc.Cell(cell_id=90000, fill=universe, region=-boundary)])
    src = spec["source"]
    x, p = source_histogram(src["energy_edges_ev"], src["bin_probability"])
    source = openmc.IndependentSource(space=openmc.stats.Point(tuple(src["point_cm"])), angle=openmc.stats.Isotropic(),
                                      energy=openmc.stats.Tabular(x, p, interpolation="histogram"), particle="neutron")
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.source = [source]
    settings.particles = args.particles // args.batches
    settings.batches = args.batches
    settings.seed = args.seed
    settings.photon_transport = True
    settings.cutoff = {"energy_neutron": 1.0e3}
    settings.temperature = {"method": "nearest", "tolerance": 10.0, "default": 293.6}
    settings.output = {"tallies": False}
    edges = {"neutron": spec["tally_energy_edges_ev"]["neutron_leakage"], "photon": spec["tally_energy_edges_ev"]["photon_leakage"]}
    tallies = openmc.Tallies()
    from_cell = openmc.Cell(cell_id=TALLY_VOLUME_ID)  # a DAGMC volume keeps its id as the OpenMC cell id
    for particle in ("neutron", "photon"):
        for kind, extra in (("partial", [openmc.CellFromFilter([from_cell])]), ("net", [])):
            t = openmc.Tally(name=f"{particle}_{kind}_current")
            t.filters = [openmc.SurfaceFilter([surface_id]), *extra, openmc.ParticleFilter([particle]), openmc.EnergyFilter(edges[particle])]
            t.scores = ["current"]
            tallies.append(t)
    return openmc.model.Model(geometry=geometry, materials=mats, settings=settings, tallies=tallies)


def command_run(args) -> int:
    require(args.i_have_the_cpu, "run performs OpenMC transport; pass --i-have-the-cpu to run it")
    require(args.particles > 0 and args.batches > 1 and args.particles % args.batches == 0, "particles must be a positive multiple of batches (at least 2)")
    manifest = load_manifest(HERE / "manifest.json")
    step = args.step.expanduser().resolve()
    check_inputs(args.root.expanduser(), step, manifest)
    xml, _ = library(args, manifest)
    os.environ["OPENMC_CROSS_SECTIONS"] = str(xml)
    conv = Path(args.conversion_dir).expanduser().resolve(strict=True)
    report = json.loads((conv / "report.json").read_text(encoding="utf-8"))
    tally_surface = json.loads((conv / "tally_surface.json").read_text(encoding="utf-8"))
    h5m = conv / "model.h5m"
    require(sha256(h5m) == report["h5m"]["sha256"], "model.h5m differs from the conversion report")
    require(report["step"]["sha256"] == sha256(step), "the conversion was made from a different STEP file")
    import openmc  # noqa: PLC0415
    require(openmc.__version__ == OPENMC_VERSION, f"expected OpenMC {OPENMC_VERSION}, got {openmc.__version__}")
    out = fresh_dir(args.work_dir)
    model = build_model(openmc, manifest, h5m, tally_surface["surface_id"], args)
    model.export_to_model_xml(str(out / "model.xml"))
    started = time.monotonic()
    statepoint = model.run(cwd=str(out), threads=args.threads, output=True, openmc_exec=str(Path(sys.executable).parent / "openmc"))
    wall = time.monotonic() - started
    arrays = {}
    with openmc.StatePoint(statepoint) as sp:
        for name in ("neutron_partial_current", "neutron_net_current", "photon_partial_current", "photon_net_current"):
            t = sp.get_tally(name=name)
            arrays[name] = (list(map(float, t.mean.ravel())), list(map(float, t.std_dev.ravel())))
    lost = len(list(out.glob("particle_*.h5")))
    partial = {p: arrays[f"{p}_partial_current"] for p in ("neutron", "photon")}
    agreement = {p: net_vs_partial(*partial[p], *arrays[f"{p}_net_current"]) for p in ("neutron", "photon")}
    results = result_rows(manifest, None, partial)
    identity = {"library_sha256": sha256(xml), "code_version": f"openmc-{openmc.__version__}", "adapter_sha256": sha256(Path(__file__))}
    context = {"particles": args.particles, "batches": args.batches, "seed": args.seed, "threads": args.threads, "transport_wall_seconds": wall,
               "lost_particle_files": lost, "manifest_sha256": manifest_sha256(manifest), "source_commit": manifest["source"]["commit"],
               "step_sha256": sha256(step), "h5m_sha256": report["h5m"]["sha256"], "model_xml_sha256": sha256(out / "model.xml"),
               "conversion": {"timing_seconds": report["timing_seconds"], "peak_rss_mb": report["peak_rss_mb"], "counts": report["counts"], "tolerances": report["tolerances"],
                              "versions": report["versions"], "report_sha256": sha256(conv / "report.json")},
               "tally_surface": tally_surface, "scored": "outward partial current from DAGMC volume 2 across the tally surface",
               "net_vs_partial": agreement, "library": str(xml), "cross_sections_note": "FENDL-3.2 neutron files with ENDF/B-VII.1 photon files (make_library.py)",
               "partial_net_current": {p: {"partial": partial[p], "net": arrays[f"{p}_net_current"]} for p in ("neutron", "photon")}}
    record = seal_run_record({"schema": "faris.validation-run/1.0.0", "kind": RUN_KIND, "case_id": CASE_ID, "run_id": out.name,
                              "run_date": time.strftime("%Y-%m-%d"), "identity": identity, "results": results, "context": context})
    path = out / f"{CASE_ID}.run.json"
    path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {path}: transport {wall:.1f} s, {lost} lost-particle files")
    return 0


def fresh_dir(path: Path) -> Path:
    path = Path(path).expanduser().resolve()
    require(not path.exists(), f"{path} exists; refusing to overwrite")
    path.mkdir(parents=True)
    return path


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    for name, fn in (("plan", command_plan), ("convert", command_convert), ("run", command_run)):
        sp = sub.add_parser(name)
        sp.set_defaults(func=fn)
        sp.add_argument("--root", type=Path, required=True, help="directory holding openmc_fusion_benchmarks/ and open-benchmarks/ at the manifest's commits")
        sp.add_argument("--step", type=Path, required=True, help="the fetched Git LFS object oktavian_a.step")
        sp.add_argument("--threads", type=int, default=3)
        if name != "convert":
            sp.add_argument("--cross-sections", type=Path, required=True, help="cross_sections.xml from make_library.py")
            sp.add_argument("--audit-extension", type=Path, help="audit_library.py output covering nuclides absent from the repository audit")
            sp.add_argument("--particles", type=int, default=DEFAULT_PARTICLES)
            sp.add_argument("--batches", type=int, default=DEFAULT_BATCHES)
            sp.add_argument("--seed", type=int, default=DEFAULT_SEED)
        if name == "convert":
            sp.add_argument("--cad-python", type=Path, required=True, help="Python with cadquery and cad_to_dagmc (the CAD environment)")
        if name != "plan":
            sp.add_argument("--work-dir", type=Path, required=True, help="fresh directory")
        if name == "run":
            sp.add_argument("--conversion-dir", type=Path, required=True)
            sp.add_argument("--i-have-the-cpu", action="store_true")
    return p


def main() -> int:
    args = parser().parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
