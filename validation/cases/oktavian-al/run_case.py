#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""OKTAVIAN aluminium sphere: build and run the OpenMC model, write a sealed run record.

Subcommands:

  plan   print what a run would do, check the upstream file hashes and the nuclear
         data coverage; reads files only (no OpenMC import, no CPU)
  build  export the OpenMC XML files into a fresh directory; no transport
  run    build, run OpenMC transport and write <case>.run.json (needs --i-have-the-cpu)

The model uses the geometry of the published OpenMC input with the materials and the
D-T source histogram of the published MCNP input (the published OpenMC materials are
pure Cr50 and pure Al27; see the manifest's known_upstream_defects). A thin void shell
outside surface 6 stands in for the MCNP surface flux tally; the run record converts it
to the manifest's units (docs/VALIDATION.md). Upstream files are read from --upstream and
never copied into this repository.

`run` takes hours of CPU time at the default particle count; it refuses to start
without --i-have-the-cpu.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from validation.manifest import load_manifest, manifest_sha256, verify_files  # noqa: E402
from validation.scoring import RUN_KIND, seal_run_record  # noqa: E402

OPENMC_VERSION = "0.15.3"
INPUTS = "jade_open_benchmarks/inputs/Oktavian/Oktavian_Al"
DEFAULT_CROSS_SECTIONS = Path("~/Documents/Avila-Labs/fusion-energy-ledger/.tools/fendl-3.2-hdf5/fendl-3.2-hdf5/cross_sections.xml")
AUDIT = HERE.parents[2] / "references" / "openmc-library-audit.json"
SHELL_RADIUS_CM = 19.95  # surface 6, the MCNP tally surface
SHELL_THICKNESS_CM = 0.2
DEFAULT_PARTICLES = 20_000_000
DEFAULT_BATCHES = 100
DEFAULT_SEED = 1
SYMBOLS = ("n H He Li Be B C N O F Ne Na Mg Al Si P S Cl Ar K Ca Sc Ti V Cr Mn Fe Co Ni Cu Zn Ga Ge As Se Br Kr Rb Sr Y Zr Nb Mo Tc Ru Rh Pd Ag Cd In Sn Sb Te I Xe "
           "Cs Ba La Ce Pr Nd Pm Sm Eu Gd Tb Dy Ho Er Tm Yb Lu Hf Ta W Re Os Ir Pt Au Hg Tl Pb Bi").split()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"error: {message}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


# ---- pure helpers (tested without OpenMC) ------------------------------------------------

def zaid_to_nuclide(zaid: str) -> str:
    """'13027.41c' -> 'Al27'."""
    digits = zaid.split(".")[0]
    require(digits.isdigit() and len(digits) >= 4, f"cannot read nuclide id {zaid!r}")
    z, a = int(digits[:-3]), int(digits[-3:])
    require(1 <= z < len(SYMBOLS), f"atomic number {z} out of range in {zaid!r}")
    return f"{SYMBOLS[z]}{a}"


def _numbers(block: str) -> list[float]:
    return [float(x) for x in re.findall(r"[-+]?\d+\.?\d*(?:[eE][-+]?\d+)?", block)]


def parse_mcnp(text: str) -> dict:
    """The parts of the MCNP input the OpenMC model needs: materials, densities, source histogram, tally edges."""
    lines = [ln for ln in text.splitlines() if not ln.lstrip().startswith("c ") and ln.strip() != "c"]
    cell_density: dict[int, float] = {}
    for ln in lines:
        m = re.match(r"^\s*(\d+)\s+(\d+)\s+(-?\d+\.\d+)\s+\(", ln)
        if m and int(m.group(2)) > 0:
            cell_density[int(m.group(2))] = float(m.group(3))
    materials: dict[int, list[tuple[str, float]]] = {}
    current = None
    for ln in lines:
        m = re.match(r"^m(\d+)\s+(\S+)\s+(\S+)", ln)
        if m:
            current = int(m.group(1))
            materials[current] = [(zaid_to_nuclide(m.group(2)), float(m.group(3)))]
            continue
        m = re.match(r"^\s+(\d{4,6}\.\d+c)\s+(\S+)", ln)
        if m and current is not None:
            materials[current].append((zaid_to_nuclide(m.group(1)), float(m.group(2))))
        elif ln.strip() and not ln.startswith((" ", "\t")):
            current = None
    out_materials = {}
    for mid, comp in materials.items():
        signs = {v < 0 for _, v in comp}
        require(len(signs) == 1, f"material m{mid} mixes atom and weight fractions")
        out_materials[mid] = {"basis": "wo" if signs == {True} else "ao", "nuclides": [(n, abs(v)) for n, v in comp]}
    density = {mid: abs(cell_density[mid]) for mid in out_materials if mid in cell_density}
    require(set(density) == set(out_materials), "could not read a density for every material cell")
    require(all(cell_density[mid] < 0 for mid in density), "expected mass densities (negative) on the cell cards")

    si = re.search(r"si1\s+sp1\s*(.*?)(?:\nc\s*$|\Z)", text, re.S)
    require(si is not None, "source histogram si1/sp1 not found")
    nums = _numbers(si.group(1))
    require(len(nums) % 2 == 0 and len(nums) >= 4, "source histogram has an odd number of entries")
    bounds, probs = nums[0::2], nums[1::2]
    e21 = re.search(r"e21\s+(.*?)\ne41", text, re.S)
    e41 = re.search(r"e41\s+(.*?)\n(?:c|#)", text, re.S)
    require(e21 is not None and e41 is not None, "tally energy cards e21/e41 not found")
    return {"materials": out_materials, "mass_density_g_cm3": density, "source_bounds_mev": bounds, "source_sp": probs,
            "e21_mev": _numbers(e21.group(1)), "e41_mev": _numbers(e41.group(1))}


def histogram_for_openmc(bounds_mev: list[float], sp: list[float]) -> tuple[list[float], list[float]]:
    """MCNP histogram to OpenMC Tabular(histogram) x (eV) and p.

    MCNP gives the probability sp[i] of the interval (bounds[i-1], bounds[i]] (sp[0] is 0, on the lower bound).
    OpenMC's p[i] is a density on [x[i], x[i+1]), so p[i] = sp[i+1] / width; the last entry is unused.
    """
    require(len(bounds_mev) == len(sp) and sp[0] == 0.0, "MCNP histogram must start with a zero entry on the lower bound")
    require(all(b2 > b1 for b1, b2 in zip(bounds_mev, bounds_mev[1:])), "source bounds must increase")
    x = [b * 1.0e6 for b in bounds_mev]
    return x, [sp[i + 1] / (x[i + 1] - x[i]) for i in range(len(x) - 1)] + [0.0]


def shell_dilution(r1: float, t: float) -> float:
    """Volume-mean over the shell of a radial 1/r^2 flux, relative to its value at r1: 3 r1^2 t / (r2^3 - r1^3)."""
    r2 = r1 + t
    return 3.0 * r1 * r1 * t / (r2**3 - r1**3)


def to_manifest_units(shell_track_cm: float, shell_track_err: float, volume_cm3: float, dilution: float, area_cm2: float, width: float) -> tuple[float, float]:
    """Track length in the shell (particle-cm per source particle) to the manifest value and its absolute error."""
    factor = area_cm2 / (volume_cm3 * dilution * width)
    return shell_track_cm * factor, shell_track_err * factor


def cross_sections_entries(xml_path: Path) -> tuple[dict[str, str], dict[str, str]]:
    neutron, photon = {}, {}
    for lib in ET.parse(xml_path).getroot().findall("library"):
        target = neutron if lib.get("type") == "neutron" else photon if lib.get("type") == "photon" else None
        if target is not None:
            for name in lib.get("materials", "").split():
                target[name] = lib.get("path")
    return neutron, photon


def library_report(xml_path: Path, nuclides: list[str], audit_path: Path) -> dict:
    """Which required nuclides and elements the library has, and which the repository's audit covers."""
    neutron, photon = cross_sections_entries(xml_path)
    audited = set(json.loads(audit_path.read_text(encoding="utf-8"))["neutron_library"]) if audit_path.is_file() else set()
    elements = sorted({re.match(r"[A-Z][a-z]?", n).group(0) for n in nuclides})
    return {
        "cross_sections_sha256": sha256(xml_path),
        "missing_neutron": sorted(n for n in nuclides if n not in neutron),
        "missing_photon": sorted(e for e in elements if e not in photon),
        "not_in_audit": sorted(n for n in nuclides if n not in audited),
        "audit_file": str(audit_path), "elements": elements,
    }


def build_run_record(run_id: str, identity: dict, results: list[dict], context: dict) -> dict:
    return seal_run_record({"schema": "faris.validation-run/1.0.0", "kind": RUN_KIND, "case_id": "oktavian-al", "run_id": run_id,
                            "identity": identity, "results": results, "context": context})


# ---- plan / build / run -------------------------------------------------------------------

def load_inputs(upstream: Path, manifest: dict) -> dict:
    issues = verify_files(manifest, upstream)
    require(not issues, "upstream files differ from the manifest: " + "; ".join(issues))
    return parse_mcnp((upstream / INPUTS / "mcnp" / "Oktavian_Al.i").read_text(encoding="utf-8"))


def plan_document(args, manifest: dict, parsed: dict) -> dict:
    nuclides = sorted({n for m in parsed["materials"].values() for n, _ in m["nuclides"]})
    xml = Path(args.cross_sections).expanduser()
    lib = library_report(xml, nuclides, AUDIT) if xml.is_file() else {"error": f"{xml} not found"}
    doc = {
        "case_id": manifest["case_id"], "manifest_sha256": manifest_sha256(manifest), "upstream_commit": manifest["source"]["commit"],
        "settings": {"particles": args.particles, "batches": args.batches, "seed": args.seed, "threads": args.threads, "temperature_k": 293.6,
                     "photon_transport": True, "energy_cutoff_neutron_ev": 1.0e3, "run_mode": "fixed source"},
        "source": {"point": [0, 0, 0], "bins": len(parsed["source_sp"]) - 1, "range_mev": [parsed["source_bounds_mev"][0], parsed["source_bounds_mev"][-1]]},
        "materials": parsed["materials"], "mass_density_g_cm3": parsed["mass_density_g_cm3"], "nuclides": nuclides, "library": lib,
        "tallies": {"shell_cell": {"r1_cm": SHELL_RADIUS_CM, "r2_cm": SHELL_RADIUS_CM + SHELL_THICKNESS_CM, "dilution": shell_dilution(SHELL_RADIUS_CM, SHELL_THICKNESS_CM)},
                    "neutron_bins": len(parsed["e21_mev"]), "photon_bins": len(parsed["e41_mev"])},
        "expected_cost": {"basis": "unmeasured estimate, not a timing",
                          "text": "a 20 cm sphere with photon production typically costs about 1e-4 to 5e-4 CPU-seconds per history, so 2e7 histories is roughly 1 to 3 CPU-hours "
                                  "(under an hour on 4 threads). Time a 1e5-history run first. To reach u_C,MC <= 0.5 u_E near the 15 MeV peak (u_E about 0.24 %) several 1e6 "
                                  "histories contribute to that bin; fewer histories leave those bins INCONCLUSIVE."},
        "refuses_when": ["upstream file hashes differ from the manifest", "a required nuclide or photon element is missing from the library",
                         "a required nuclide is not covered by references/openmc-library-audit.json and no --audit-extension covers it", "--i-have-the-cpu is absent"],
    }
    return doc


def command_plan(args) -> int:
    manifest = load_manifest(HERE / "manifest.json")
    parsed = load_inputs(args.upstream.expanduser(), manifest)
    print(json.dumps(plan_document(args, manifest, parsed), indent=2, sort_keys=True))
    return 0


def check_library(args, parsed: dict) -> Path:
    xml = Path(args.cross_sections).expanduser().resolve(strict=True)
    nuclides = sorted({n for m in parsed["materials"].values() for n, _ in m["nuclides"]})
    rep = library_report(xml, nuclides, AUDIT)
    require(not rep["missing_neutron"], f"library lacks neutron data for {rep['missing_neutron']}")
    require(not rep["missing_photon"], f"library lacks photon data for {rep['missing_photon']}")
    uncovered = rep["not_in_audit"]
    if uncovered and args.audit_extension:
        extra = set(json.loads(Path(args.audit_extension).read_text(encoding="utf-8"))["neutron_library"])
        uncovered = [n for n in uncovered if n not in extra]
    require(not uncovered, f"{uncovered} are not in the audited library inventory ({AUDIT.name}). Audit them first: python3 integrations/openmc/audit_library.py "
                           f"--cross-sections {xml} --nuclides {' '.join(rep['not_in_audit'])} --output audit-extension.json, then pass --audit-extension")
    return xml


def build_model(openmc, upstream: Path, parsed: dict, args):
    materials = {}
    for mid, spec in parsed["materials"].items():
        mat = openmc.Material(material_id=mid, name=f"M{mid}")
        for name, frac in spec["nuclides"]:
            mat.add_nuclide(name, frac, spec["basis"])
        mat.set_density("g/cm3", parsed["mass_density_g_cm3"][mid])
        mat.temperature = 293.6
        materials[mid] = mat
    mats = openmc.Materials(list(materials.values()))
    geometry = openmc.Geometry.from_xml(str(upstream / INPUTS / "openmc" / "geometry.xml"), materials=mats)
    surfaces = geometry.get_all_surfaces()
    cells = geometry.get_all_cells()
    tally_surface, vacuum = surfaces[6], surfaces[7]
    outer = openmc.Sphere(surface_id=9, r=SHELL_RADIUS_CM + SHELL_THICKNESS_CM)
    shell = openmc.Cell(cell_id=7, name="flux shell", region=+tally_surface & -outer)
    cells[5].region = +outer & -vacuum
    geometry.root_universe.add_cell(shell)

    bounds, probs = histogram_for_openmc(parsed["source_bounds_mev"], parsed["source_sp"])
    energy = openmc.stats.Tabular(bounds, probs, interpolation="histogram")
    source = openmc.IndependentSource(space=openmc.stats.Point((0.0, 0.0, 0.0)), angle=openmc.stats.Isotropic(), energy=energy, particle="neutron")

    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.source = [source]
    settings.particles = args.particles // args.batches
    settings.batches = args.batches
    settings.seed = args.seed
    settings.photon_transport = True
    settings.cutoff = {"energy_neutron": 1.0e3}
    settings.temperature = {"method": "nearest", "tolerance": 10.0, "default": 293.6}

    n_edges = [0.0] + [e * 1.0e6 for e in parsed["e21_mev"]]
    p_edges = [e * 1.0e6 for e in parsed["e41_mev"]] + [11.0e6]  # the manifest's photon bins run to 11 MeV
    tallies = openmc.Tallies()
    shell_filter = openmc.CellFilter([shell])
    for name, particle, edges in (("neutron_shell_flux", "neutron", n_edges), ("photon_shell_flux", "photon", p_edges)):
        t = openmc.Tally(name=name)
        t.filters = [shell_filter, openmc.ParticleFilter([particle]), openmc.EnergyFilter(edges)]
        t.scores = ["flux"]
        tallies.append(t)
    return openmc.model.Model(geometry=geometry, materials=mats, settings=settings, tallies=tallies), shell


def command_build(args) -> int:
    manifest = load_manifest(HERE / "manifest.json")
    parsed = load_inputs(args.upstream.expanduser(), manifest)
    xml = check_library(args, parsed)
    import os
    os.environ["OPENMC_CROSS_SECTIONS"] = str(xml)
    openmc = import_openmc()
    out = fresh_dir(args.work_dir)
    model, _ = build_model(openmc, args.upstream.expanduser(), parsed, args)
    model.export_to_model_xml(str(out / "model.xml"))
    print(json.dumps({"exported": str(out / "model.xml"), "sha256": sha256(out / "model.xml")}, indent=2))
    return 0


def command_run(args) -> int:
    require(args.i_have_the_cpu, "run performs long OpenMC transport on several cores; pass --i-have-the-cpu to run it")
    manifest = load_manifest(HERE / "manifest.json")
    parsed = load_inputs(args.upstream.expanduser(), manifest)
    xml = check_library(args, parsed)
    import os
    os.environ["OPENMC_CROSS_SECTIONS"] = str(xml)
    openmc = import_openmc()
    out = fresh_dir(args.work_dir)
    model, shell = build_model(openmc, args.upstream.expanduser(), parsed, args)
    model.export_to_model_xml(str(out / "model.xml"))
    statepoint = model.run(cwd=str(out), threads=args.threads, output=True)
    with openmc.StatePoint(statepoint) as sp:
        neutron = sp.get_tally(name="neutron_shell_flux")
        photon = sp.get_tally(name="photon_shell_flux")
        n_mean, n_err = neutron.mean.ravel(), neutron.std_dev.ravel()
        p_mean, p_err = photon.mean.ravel(), photon.std_dev.ravel()
    r1, r2 = SHELL_RADIUS_CM, SHELL_RADIUS_CM + SHELL_THICKNESS_CM
    volume, dil = 4.0 / 3.0 * math.pi * (r2**3 - r1**3), shell_dilution(r1, SHELL_THICKNESS_CM)
    area = manifest["normalisation"]["area_cm2"]
    results = []
    for det in manifest["detectors"]:
        bin_ = det["bin"]
        if det["response_class"] == "neutron_leakage_spectrum":
            i = parsed["e21_mev"].index(bin_["hi_mev"]) + 1  # bin 0 is (0, e21[0]]
            width = math.log(bin_["hi_mev"] / bin_["lo_mev"])
            mean, err = n_mean[i], n_err[i]
        else:
            i = [round(e, 6) for e in parsed["e41_mev"] + [11.0]].index(round(bin_["lo_mev"], 6))
            width = bin_["hi_mev"] - bin_["lo_mev"]
            mean, err = p_mean[i], p_err[i]
        value, u = to_manifest_units(mean, err, volume, dil, area, width)
        results.append({"detector_id": det["id"], "value": value, "u_mc": u})
    identity = {"library_sha256": sha256(xml), "code_version": f"openmc-{openmc.__version__}", "adapter_sha256": sha256(Path(__file__))}
    context = {"particles": args.particles, "batches": args.batches, "seed": args.seed, "threads": args.threads, "manifest_sha256": manifest_sha256(manifest), "upstream_commit": manifest["source"]["commit"], "model_xml_sha256": sha256(out / "model.xml"),
               "shell_dilution": dil, "area_cm2": area}
    record = build_run_record(out.name, identity, results, context)
    path = out / "oktavian-al.run.json"
    path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {path}")
    return 0


def import_openmc():
    import openmc
    require(openmc.__version__ == OPENMC_VERSION, f"expected OpenMC {OPENMC_VERSION}, got {openmc.__version__}")
    return openmc


def fresh_dir(path: Path) -> Path:
    path = Path(path).expanduser().resolve()
    require(not path.exists(), f"{path} exists; refusing to overwrite")
    path.mkdir(parents=True)
    return path


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    for name, fn in (("plan", command_plan), ("build", command_build), ("run", command_run)):
        sp = sub.add_parser(name)
        sp.set_defaults(func=fn)
        sp.add_argument("--upstream", type=Path, required=True, help="clean checkout of IAEA-NDS/open-benchmarks at the manifest's commit")
        sp.add_argument("--cross-sections", type=Path, default=DEFAULT_CROSS_SECTIONS)
        sp.add_argument("--audit-extension", type=Path, help="audit_library.py output covering nuclides absent from the repository audit")
        sp.add_argument("--particles", type=int, default=DEFAULT_PARTICLES)
        sp.add_argument("--batches", type=int, default=DEFAULT_BATCHES)
        sp.add_argument("--seed", type=int, default=DEFAULT_SEED)
        sp.add_argument("--threads", type=int, default=4)
        if name != "plan":
            sp.add_argument("--work-dir", type=Path, required=True, help="fresh directory for the exported model and results")
        if name == "run":
            sp.add_argument("--i-have-the-cpu", action="store_true")
    return p


def main() -> int:
    args = parser().parse_args()
    require(args.particles > 0 and args.batches > 0 and args.particles % args.batches == 0, "particles must be a positive multiple of batches")
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
