"""OpenMC side of RM-M: library identity, materials, DAGMC model, tokamak source and tallies.

Imported by accept_rm_m.py and r6_worker.py, which run under the OpenMC
interpreter (0.15.3 or 0.16.0). Materials reuse the FARIS material baseline
(references/demo-input-spec.json: the nuclide vectors and densities the
reactor adapter builds with `add_nuclide(..., percent_type="ao")` and
`set_density("kg/m3", ...)`). The library check mirrors the adapter's
cross_sections.xml identity check. openmc and openmc_plasma_source are
imported lazily so the module imports without them.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path
import xml.etree.ElementTree as ET

import rm_m_spec as spec

REPO = Path(__file__).resolve().parents[3]
AUDIT = REPO / "references" / "openmc-library-audit.json"
DEMO_SPEC = REPO / "references" / "demo-input-spec.json"
# The audited library root is git-ignored data; the default is this workstation's copy.
FARIS_XS = Path(os.environ.get("FARIS_CROSS_SECTIONS", "/home/connoravila/Documents/Avila-Labs/project-faris/data/raw/combined-fendl32-endfbvii1/cross_sections.xml"))
DATA_TEMPERATURE_K = 294.0
PLASMA_ID_NAME = "plasma"
SPHERE_SURFACE_ID = 100000

# Plasma profile parameters for openmc-plasma-source: authored (the shape and power are published).
PLASMA_PROFILE = {
    "mode": "H",
    "ion_density_centre": 1.5e20,       # m-3, authored
    "ion_density_peaking_factor": 1.0,
    "ion_density_pedestal": 1.1e20,
    "ion_density_separatrix": 0.4e20,
    "ion_temperature_centre": 2.0e4,    # eV, authored
    "ion_temperature_peaking_factor": 1.5,
    "ion_temperature_beta": 1.5,
    "ion_temperature_pedestal": 4.0e3,
    "ion_temperature_separatrix": 1.0e2,
    "pedestal_radius": 0.9 * spec.MINOR_RADIUS_CM,
    "shafranov_factor": 0.0,
}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def library_identity(xml_path: Path = FARIS_XS) -> dict:
    """Identity of the FARIS audited library: XML digest against the audit, and the nuclides RM-M needs."""
    xml_path = Path(xml_path)
    audit = json.loads(AUDIT.read_text(encoding="utf-8"))
    expected = audit["cross_sections_xml"]["sha256"]
    observed = sha256(xml_path)
    root = ET.parse(xml_path).getroot()
    neutron, photon = {}, {}
    for library in root.findall("library"):
        target = neutron if library.get("type") == "neutron" else photon
        for name in library.get("materials", "").split():
            target[name] = library.get("path")
    needed = sorted(needed_nuclides())
    missing = [n for n in needed if n not in neutron]
    return {
        "cross_sections_xml": str(xml_path),
        "sha256": observed,
        "audited_sha256": expected,
        "matches_audit": observed == expected,
        "library": "FENDL-3.2 neutron (294 K) + ENDF/B-VII.1 photon overlay (FARIS audited)",
        "nuclides_needed": needed,
        "nuclides_missing": missing,
        "photon_elements_available": sorted(photon),
        "usable": observed == expected and not missing,
    }


def material_recipes() -> dict:
    """tag -> {"density_g_cm3", "atom_fractions"} from the FARIS baseline plus the authored RM-M additions."""
    baseline = {m["id"]: m for m in json.loads(DEMO_SPEC.read_text(encoding="utf-8"))["materials"]}

    def from_baseline(material_id):
        m = baseline[material_id]
        recipe = m["recipe"]
        fractions = recipe.get("nuclide_atom_fractions") or recipe["nuclide_atom_fractions_across_all_atoms"]
        return {"density_g_cm3": m["density_kg_m3"] / 1000.0, "atom_fractions": dict(fractions), "source": material_id}

    fe = from_baseline("pure-iron-surrogate")
    cu = from_baseline("copper-magnet-response-surrogate")
    # Winding pack: 45.9 % Cu + 8.0 % REBCO (as Cu, authored) + 46.1 % steel, by volume.
    f_cu = spec.WP_FRACTIONS["copper"]["value"] + spec.WP_FRACTIONS["rebco"]["value"]
    f_fe = spec.WP_FRACTIONS["steel"]["value"]
    rho = f_cu * cu["density_g_cm3"] + f_fe * fe["density_g_cm3"]
    # atom density per cm3 (relative units): rho/A per isotope; use the mean atomic mass of each element.
    atoms_cu = f_cu * cu["density_g_cm3"] / 63.546
    atoms_fe = f_fe * fe["density_g_cm3"] / 55.845
    total = atoms_cu + atoms_fe
    wp = {n: x * atoms_cu / total for n, x in cu["atom_fractions"].items()}
    for n, x in fe["atom_fractions"].items():
        wp[n] = wp.get(n, 0.0) + x * atoms_fe / total
    # Coil: winding pack with the case steel, by volume. Pack fraction from the authored 2 cm case wall on a 64 cm leg.
    f_pack = (spec.TF_RADIAL_CM["value"] - 2.0 * spec.TF_CASE_WALL_CM["value"]) / spec.TF_RADIAL_CM["value"]
    rho_coil = f_pack * rho + (1.0 - f_pack) * fe["density_g_cm3"]
    # per-cm3 atom weights: use A = 55.845 for Fe and the pack's mean A implied by its atoms (cu/fe mix)
    a_pack = (atoms_cu * 63.546 + atoms_fe * 55.845) / (atoms_cu + atoms_fe)
    n_pack = f_pack * rho / a_pack
    n_case = (1.0 - f_pack) * fe["density_g_cm3"] / 55.845
    coil = {n: x * n_pack / (n_pack + n_case) for n, x in wp.items()}
    for n, x in fe["atom_fractions"].items():
        coil[n] = coil.get(n, 0.0) + x * n_case / (n_pack + n_case)
    return {
        "tf_coil": {"density_g_cm3": rho_coil, "atom_fractions": coil, "source": "authored: pack and case steel homogenised"},
        "tungsten": from_baseline("tungsten-natural"),
        "structure": fe,
        "flibe": from_baseline("flibe-cold-solid"),
        "beryllium": {"density_g_cm3": 1.85, "atom_fractions": {"Be9": 1.0}, "source": "authored (docs/DEMO_INPUT_SPEC.md)"},
        "tih2": from_baseline("ti-hydride-shield-surrogate"),
        "thermal_insulation": {"density_g_cm3": 1.0, "atom_fractions": dict(fe["atom_fractions"]), "source": "authored: Fe at 1.0 g/cm3"},
        "filler": {"density_g_cm3": 1.0e-3, "atom_fractions": {"H1": 1.0}, "source": "authored: random-ray stand-in for void"},
    }


def needed_nuclides() -> set:
    return {n for r in material_recipes().values() for n in r["atom_fractions"]}


def build_materials(openmc, tags):
    """openmc.Material for each non-void tag in `tags`, named by tag (DAGMC matches by name)."""
    recipes = material_recipes()
    out = {}
    for tag in sorted(set(tags)):
        if tag == "void":
            continue
        r = recipes[tag]
        mat = openmc.Material(name=tag)
        for nuclide, fraction in r["atom_fractions"].items():
            mat.add_nuclide(nuclide, fraction, percent_type="ao")
        mat.set_density("g/cm3", r["density_g_cm3"])
        mat.temperature = DATA_TEMPERATURE_K
        out[tag] = mat
    return out


def load_roles(model_dir: Path, stem: str = "rm_m") -> list[dict]:
    return json.loads((Path(model_dir) / f"{stem}_roles.json").read_text(encoding="utf-8"))["solids"]


def role_ids(roles: list[dict]) -> dict:
    ids = {r["name"]: int(r["volume_id"]) for r in roles}
    return {"plasma": ids["plasma"], "tf": sorted(v for k, v in ids.items() if k.startswith("tf_coil_")), "all": ids}


def build_model(openmc, h5m: Path, roles: list[dict], rr: bool = False, mgxs_tags: bool = False):
    """Geometry (DAGMC universe in a vacuum sphere), materials and library; no source or tallies."""
    h5m = Path(h5m)
    tags = {r.get("h5m_material_tag") or r["tag"] for r in roles}
    materials = build_materials(openmc, tags | ({"filler"} if rr else set()))
    dag = openmc.DAGMCUniverse(str(h5m))
    sphere = openmc.Sphere(surface_id=SPHERE_SURFACE_ID, r=spec.BOUNDING_SPHERE_RADIUS_CM["value"], boundary_type="vacuum")
    outer = openmc.Cell(cell_id=10000, fill=dag, region=-sphere)
    geometry = openmc.Geometry([outer])
    model = openmc.Model(geometry=geometry, materials=openmc.Materials(list(materials.values())))
    model.materials.cross_sections = str(FARIS_XS)
    return model, dag


def tokamak_mesh_source(openmc, plasma_cell_id: int | None, rate: float = spec.SOURCE_RATE_N_S, mesh_resolution=(100, 100)):
    """openmc-plasma-source tokamak source for the protocol's plasma, scaled to `rate` n/s.

    With a plasma cell id the source carries a cell constraint with resampling, so
    sampled sites outside the cell (voxels straddle the plasma boundary) are rejected.
    Returns (source, unscaled_rate_n_s).
    """
    from openmc_plasma_source import tokamak_source

    source = tokamak_source(
        major_radius=spec.MAJOR_RADIUS_CM, minor_radius=spec.MINOR_RADIUS_CM,
        elongation=spec.ELONGATION, triangularity=spec.TRIANGULARITY,
        mesh_resolution=mesh_resolution, **PLASMA_PROFILE)
    unscaled = float(source.strength)
    for element in source.sources.ravel():
        element.strength = float(element.strength) * rate / unscaled
    source.strength = rate
    if plasma_cell_id is not None:
        source.constraints = {"domains": [openmc.Cell(cell_id=plasma_cell_id)], "rejection_strategy": "resample"}
    return source, unscaled


def tf_fast_flux_tallies(openmc, tf_ids: list[int], name: str = "tf_fast_flux"):
    """Response (a): flux above 0.1 MeV in every TF coil cell, one bin per cell (summed by the caller)."""
    tally = openmc.Tally(name=name)
    tally.filters = [openmc.CellFilter(tf_ids), openmc.EnergyFilter([1.0e5, 2.0e7])]
    tally.scores = ["flux"]
    return tally


# ---------------------------------------------------------------------------
# Running OpenMC (continuous energy, analog or with weight windows)

def batch_statistics(per_batch: list[float]) -> dict:
    """Mean, standard error and relative error of a response from its per-batch values."""
    n = len(per_batch)
    if n < 2:
        raise ValueError("at least two batches are needed")
    mean = math.fsum(per_batch) / n
    var = math.fsum((x - mean) ** 2 for x in per_batch) / (n - 1)
    se = math.sqrt(var / n)
    return {"batches": n, "mean": mean, "std_error": se, "relative_error": (se / mean) if mean else None}


def cumulative_to_batches(cumulative: list[float]) -> list[float]:
    previous = 0.0
    out = []
    for total in cumulative:
        out.append(total - previous)
        previous = total
    return out


def run_openmc_cli(run_dir: Path, threads: int, log_name: str = "openmc.out") -> dict:
    """Run the `openmc` executable of the current interpreter's environment in run_dir."""
    import resource
    import subprocess
    import sys
    import time

    exe = Path(sys.executable).parent / "openmc"
    env = dict(os.environ, OMP_NUM_THREADS=str(threads))
    before = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    t0 = time.time()
    with (Path(run_dir) / log_name).open("w", encoding="utf-8") as log:
        proc = subprocess.run([str(exe), "--threads", str(threads)], cwd=run_dir, stdout=log, stderr=subprocess.STDOUT, env=env)
    wall = time.time() - t0
    after = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    return {"returncode": proc.returncode, "wall_seconds": wall, "executable": str(exe),
            "peak_rss_mb": max(after, before) / 1024.0}


def count_lost_particles(run_dir: Path, log_name: str = "openmc.out") -> dict:
    """Lost particles from the particle restart files OpenMC writes and from the log."""
    run_dir = Path(run_dir)
    files = sorted(p.name for p in run_dir.glob("particle_*.h5"))
    text = (run_dir / log_name).read_text(encoding="utf-8", errors="replace")
    warned = sum(1 for line in text.splitlines() if "lost" in line.lower() and "particle" in line.lower())
    return {"restart_files": len(files), "log_lines_mentioning_lost": warned, "files": files[:20]}


def read_tf_flux(openmc, run_dir: Path, batches: int, tally_name: str = "tf_fast_flux") -> dict:
    """Response (a) from per-batch statepoints: sum over the TF cells, per source particle (cm).

    Intermediate statepoints are deleted; the final one stays.
    """
    run_dir = Path(run_dir)
    cumulative, per_cell_final = [], None
    for b in range(1, batches + 1):
        path = run_dir / f"statepoint.{b:0{len(str(batches))}d}.h5"  # OpenMC zero-pads to the width of the batch count
        with openmc.StatePoint(str(path)) as sp:
            tally = sp.get_tally(name=tally_name)
            cumulative.append(float(tally.sum.sum()))
            if b == batches:
                per_cell_final = (tally.mean.ravel().tolist(), tally.std_dev.ravel().tolist())
        if b < batches:
            path.unlink()
    stats = batch_statistics(cumulative_to_batches(cumulative))
    stats["per_cell_mean"], stats["per_cell_std_dev"] = per_cell_final
    return stats


def run_analog(openmc, model_dir: Path, run_dir: Path, histories: int, batches: int, seed: int, threads: int,
               max_lost: int = 10000, weight_windows_file: Path | None = None, h5m_name: str = "rm_m.h5m") -> dict:
    """One continuous-energy neutron run of RM-M with the tokamak source and the TF fast-flux tally.

    With weight_windows_file the windows are applied; otherwise the run is analog.
    """
    model_dir, run_dir = Path(model_dir), Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    roles = load_roles(model_dir)
    ids = role_ids(roles)
    model, _ = build_model(openmc, model_dir / h5m_name, roles)
    source, unscaled = tokamak_mesh_source(openmc, ids["plasma"])
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches = batches
    settings.particles = histories // batches
    settings.seed = seed
    settings.source = [source]
    settings.photon_transport = False
    settings.max_lost_particles = max_lost
    settings.rel_max_lost_particles = 0.5
    settings.statepoint = {"batches": list(range(1, batches + 1))}
    settings.temperature = {"default": DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    settings.output = {"summary": False, "tallies": False}
    if weight_windows_file is not None:
        settings.weight_windows = openmc.hdf5_to_wws(str(weight_windows_file))
        settings.weight_windows_on = True
    model.tallies = openmc.Tallies([tf_fast_flux_tallies(openmc, ids["tf"])])
    model.export_to_model_xml(str(run_dir))
    run = run_openmc_cli(run_dir, threads)
    result = {"run_dir": str(run_dir), "openmc_version": openmc.__version__, "histories": histories, "batches": batches,
              "particles_per_batch": histories // batches, "seed": seed, "threads": threads,
              "weight_windows": str(weight_windows_file) if weight_windows_file else None,
              "source_unscaled_rate_n_s": unscaled, "run": run}
    if run["returncode"] == 0:
        result["lost"] = count_lost_particles(run_dir)
        result["tf_fast_flux"] = read_tf_flux(openmc, run_dir, batches)
    return result
