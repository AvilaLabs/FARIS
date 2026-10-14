#!/usr/bin/env python3
"""Weight windows from a random-ray adjoint of a response: the one implementation behind `faris design
transport` (DAGMC geometry) and the reactor study's Production tier (CSG geometry).

The caller builds an OpenMC model with continuous-energy materials and says which cells or materials make the
response. Two steps follow, each run once and kept by the caller:

    condense_multigroup  the library condensed to a group structure on the model's materials (stochastic slab)
    solve_windows        random-ray adjoint of the response, and CADIS weight windows from it (OpenMC 0.16 or later)

The windows are written to an HDF5 file with FARIS's window and mesh ids (`label_windows`); a run names that file
in its model (`apply_windows`) and never carries the bounds as XML. Windows change only the variance: a particle
split or rouletted on them is scored with the weight it carries.

Random ray cannot cross true void. Empty space gets the filler (hydrogen at 1 mg/cm3), which the caller must put
in the model's materials; `fill_void` does it for CSG cells that have no fill.

openmc is imported lazily so the pure helpers import without it.
"""
from __future__ import annotations

import hashlib
import inspect
import json
import math
import os
import sys
import time
from pathlib import Path

WINDOW_MESH_ID = 9000        # streaming_aim.py reads the windows of the mesh with this id
WINDOW_ID = 1
FILLER = "faris-filler"      # random ray cannot cross true void; empty space gets hydrogen at 1 mg/cm3
FILLER_DENSITY_KG_M3 = 1.0
SOURCE_ENERGY_EV = 14.06e6
WINDOW_CELL_CM = 25.0
MAX_SPLIT = 1000
# The steps that make the windows have their own fixed seeds, so that windows can be reused between runs; a run
# may not use either seed, so it never shares a random stream with them (NUC-053).
MGXS_SEED, WINDOW_SEED = 811_500_991, 811_500_993
# 17 groups, ten above 0.1 MeV (S0f pilot p1-fr).
GROUP_EDGES_EV = [0.0, 0.058, 0.14, 0.28, 0.625, 4.0, 5530.0, 1.0e5, 5.0e5, 8.21e5, 1.353e6, 2.231e6, 3.679e6,
                  6.0655e6, 1.0e7, 1.3e7, 1.5e7, 2.0e7]
MGXS = {"method": "stochastic_slab", "correction": "P0", "nparticles": 20000, "batches": 60}
RANDOM_RAY = {"rays": 20000, "batches": 60, "inactive": 30, "distance_inactive_cm": 500.0,
              "distance_active_cm": 2500.0, "source_shape": "flat", "volume_estimator": "hybrid",
              "diagonal_stabilization_rho": 1.0, "sample_method": "prng"}


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


# ---------------------------------------------------------------------------
# Pure helpers (tested without OpenMC)

def library_entries(names: list[str], stored: list[str]) -> dict[str, str]:
    """The multigroup library's entry for each material. The library rewrites characters such as `-`, and OpenMC
    0.16 adds the material's id (`_3`) to the name, so names are compared with everything but letters and digits
    made equal and an optional trailing number ignored; each must match exactly one entry."""
    def plain(name: str) -> str:
        return "".join(c if c.isalnum() else "_" for c in name)

    def same(entry: str, name: str) -> bool:
        wanted, stored_name = plain(name), plain(entry)
        if stored_name == wanted:
            return True
        head, _, tail = stored_name.rpartition("_")
        return head == wanted and tail.isdigit()

    out = {}
    for name in names:
        hits = [entry for entry in stored if same(entry, name)]
        if len(hits) != 1:
            raise ValueError(f"the multigroup library has {len(hits)} entries for the material {name}")
        out[name] = hits[0]
    if len(set(out.values())) != len(out):
        raise ValueError("two materials share one entry of the multigroup library")
    return out


def group_edges(energy_min_ev: float) -> list[float]:
    """The group structure for a response above `energy_min_ev`: the standard edges, with the threshold added
    as an edge if it is not one already."""
    if energy_min_ev in GROUP_EDGES_EV:
        return list(GROUP_EDGES_EV)
    return sorted(GROUP_EDGES_EV + [float(energy_min_ev)])


def response_lines(edges_ev: list[float], energy_min_ev: float) -> list[float]:
    """One energy per group at or above the threshold, for the adjoint source: the geometric midpoint of the
    group, or the arithmetic one for a group that starts at zero. Random ray maps each to its group."""
    edges = edges_ev[edges_ev.index(energy_min_ev):]
    return [math.sqrt(a * b) if a > 0.0 else 0.5 * b for a, b in zip(edges, edges[1:])]


def window_grid(lower: list[float], upper: list[float], cell_cm: float = WINDOW_CELL_CM) -> tuple[list[float], list[float], list[int]]:
    """A box around the model, 5 cm clear on every side, in cells of about `cell_cm`."""
    lo = [math.floor(v - 5.0) for v in lower]
    hi = [math.ceil(v + 5.0) for v in upper]
    return [float(v) for v in lo], [float(v) for v in hi], [max(1, round((h - l) / cell_cm)) for l, h in zip(lo, hi)]


def label_windows(path: Path) -> None:
    """Give a weight-window file FARIS's window id, split limit and mesh id, in place. The window step does this
    to its own `filled.h5` before hashing it, so every run reads that file where it lies instead of a copy (on a
    large machine the file is over 100 MB, and a copy per run pushed the run directory past the job's disk cap).
    The window id in the group name is only a label: OpenMC numbers a window it loads itself."""
    import h5py

    with h5py.File(path, "r+") as stream:
        windows = stream["weight_windows"]
        group = windows[next(iter(windows))]
        label = f"weight_windows_{WINDOW_ID}"
        if group.name.rsplit("/", 1)[-1] != label:
            windows.move(group.name.rsplit("/", 1)[-1], label)
        windows.attrs["ids"] = [WINDOW_ID]
        windows.attrs["n_weight_windows"] = 1
        group = windows[label]
        group["max_split"][...] = MAX_SPLIT
        group["mesh"][...] = WINDOW_MESH_ID
        meshes = stream["meshes"]
        old = next(iter(meshes))
        if old != f"mesh {WINDOW_MESH_ID}":
            meshes.move(old, f"mesh {WINDOW_MESH_ID}")
        meshes[f"mesh {WINDOW_MESH_ID}"].attrs["id"] = WINDOW_MESH_ID
        meshes.attrs["ids"] = [WINDOW_MESH_ID]
        meshes.attrs["n_meshes"] = 1


def window_file_name(windows: Path, directory: Path) -> str:
    """How a run started in DIRECTORY names the weight-window file WINDOWS: a relative path, so the model does
    not depend on where the run directory lies."""
    return os.path.relpath(windows, directory)


def apply_windows(openmc, model, windows: Path, directory: Path):
    """Make the model read its weight windows from the file WINDOWS (labelled by `label_windows`) and return their
    mesh.

    The windows are not written into the model's XML: on a large machine their bounds are hundreds of megabytes
    of text that OpenMC must parse again at every run, and they are not copied either. OpenMC loads the bounds
    from the file and takes the mesh by the id stored in it, so the mesh must be in the model: pass the returned
    mesh to `export_model`. The model names the file relative to DIRECTORY, where the run must start."""
    import h5py

    with h5py.File(windows, "r") as stream:
        mesh = openmc.MeshBase.from_hdf5(stream["meshes"][f"mesh {WINDOW_MESH_ID}"])
    mesh.id = WINDOW_MESH_ID
    # a relative name: openmc would write the path of this process's directory, and the run starts in its own
    resolve = openmc.config["resolve_paths"]
    openmc.config["resolve_paths"] = False
    try:
        model.settings.weight_windows_file = Path(window_file_name(windows, directory))
    finally:
        openmc.config["resolve_paths"] = resolve
    model.settings.weight_windows_on = True
    model.settings.weight_window_checkpoints = {"collision": True, "surface": True}
    return mesh


def export_model(model, path: Path, window_mesh=None) -> None:
    """Write the model as one XML file, with the mesh of file-borne weight windows (from `apply_windows`) in its
    settings: the window file names the mesh by id and OpenMC looks it up among the model's meshes."""
    model.export_to_model_xml(str(path))
    if window_mesh is None:
        return
    import xml.etree.ElementTree as ET

    text = Path(path).read_text()
    element = window_mesh.to_xml_element()
    ET.indent(element, space="  ", level=2)
    at = text.index("</settings>")
    Path(path).write_text(text[:at] + "  " + ET.tostring(element, encoding="unicode") + "\n  " + text[at:])


# ---------------------------------------------------------------------------
# OpenMC

def repaired_totals(values: list[float]) -> tuple[list[float], int]:
    """A group's total cross sections with each one that is zero or negative set to the smallest positive one of the
    material. Random ray refuses such a value; it comes from noise in a nearly empty material (the filler) in
    groups the slab's few collisions miss. Returns (values, how many were repaired)."""
    positive = [v for v in values if v > 0.0]
    if len(positive) == len(values):
        return list(values), 0
    fill = min(positive) if positive else 1.0e-4
    return [v if v > 0.0 else fill for v in values], len(values) - len(positive)


def repair_library(path: Path) -> int:
    """Apply `repaired_totals` to every material and temperature of the multigroup library PATH, in place.
    Returns how many values were repaired."""
    import h5py

    repaired = 0
    with h5py.File(path, "r+") as stream:
        for material in stream.values():
            if not hasattr(material, "values"):
                continue
            for temperature in material.values():
                if "total" not in temperature:
                    continue
                values, count = repaired_totals([float(v) for v in temperature["total"][()]])
                if count:
                    temperature["total"][...] = values
                    repaired += count
    return repaired


def filler_material(openmc, material_id: int):
    material = openmc.Material(material_id=material_id, name=FILLER)
    material.add_nuclide("H1", 1.0, percent_type="ao")
    material.set_density("kg/m3", FILLER_DENSITY_KG_M3)
    return material


def fill_void(openmc, geometry, filler) -> int:
    """Give every CSG cell without a fill the filler, so that rays can cross it. Returns how many were filled."""
    count = 0
    for cell in geometry.get_all_cells().values():
        if cell.fill is None:
            cell.fill = filler
            count += 1
    return count


def domain_source(openmc, domain, energy_ev: float = SOURCE_ENERGY_EV):
    """Neutrons of one energy born uniformly in a cell: the source of the deterministic steps."""
    return openmc.IndependentSource(energy=openmc.stats.Discrete([energy_ev], [1.0]), particle="neutron",
                                    constraints={"domains": [domain]}, strength=1.0)


def discard(directory: Path, keep: set[str]) -> None:
    """Remove a finished step's files other than `keep`: the run directory has a size limit."""
    for path in Path(directory).iterdir():
        if path.is_file() and path.name not in keep:
            path.unlink()


def run_cli(directory: Path, threads: int, log_name: str) -> None:
    """Run the openmc program beside this interpreter in `directory`."""
    import subprocess

    exe = Path(sys.executable).parent / "openmc"
    with (directory / log_name).open("w") as log:
        done = subprocess.run([str(exe), "--threads", str(threads)], cwd=directory, stdout=log, stderr=subprocess.STDOUT)
    if done.returncode != 0:
        tail = (directory / log_name).read_text(errors="replace")[-2000:]
        raise RuntimeError(f"openmc exited {done.returncode} in {directory}:\n{tail}")


def condense_multigroup(openmc, model, edges: list[float], source, directory: Path, threads: int,
                        parameters: dict = MGXS) -> dict:
    """Condense the library to the group structure EDGES on the model's materials. Writes DIRECTORY/mgxs.h5 and
    returns its record. The model is changed (its materials become multigroup): build another to go on with.
    PARAMETERS are the slab run's (`MGXS`): fewer particles and batches make a noisier library and so rougher
    windows, never a biased result."""
    import openmc.mgxs

    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    model.settings.source = [source]
    model.settings.batches = parameters["batches"]
    model.settings.particles = parameters["nparticles"]
    model.settings.seed = MGXS_SEED
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(int(threads))
    target = directory / "mgxs.h5"
    # OpenMC 0.16 takes the slab run's settings as keywords and ignores the model's; 0.15.3 reads the model's
    # batches and the particle count argument.
    slab = {"particles": parameters["nparticles"], "batches": parameters["batches"],
            "inactive": parameters["batches"] // 2, "seed": MGXS_SEED}
    if any(p.kind is inspect.Parameter.VAR_KEYWORD
           for p in inspect.signature(model.convert_to_multigroup).parameters.values()):
        options = slab
    else:
        options = {"nparticles": parameters["nparticles"]}
    here = os.getcwd()
    os.chdir(directory)
    try:
        model.convert_to_multigroup(method=parameters["method"], groups=openmc.mgxs.EnergyGroups(edges),
                                    overwrite_mgxs_library=False, mgxs_path=str(target),
                                    correction=parameters["correction"], **options)
    finally:
        os.chdir(here)
    if not target.is_file():
        raise RuntimeError("the multigroup step wrote no library")
    discard(directory, keep={"mgxs.h5"})
    record = {"schema": "faris-design-mgxs/v1", "openmc": openmc.__version__, "parameters": parameters,
              "group_edges_ev": edges, "mgxs_sha256": sha256_file(target),
              "elapsed_seconds": time.monotonic() - started}
    (directory / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    return record


def solve_windows(openmc, model, universe, source, adjoint_domains: list, threshold_ev: float, bounds: tuple,
                  library: Path, directory: Path, threads: int) -> dict:
    """Random-ray adjoint of the flux above THRESHOLD_EV in ADJOINT_DOMAINS (cells or materials of the model) and
    CADIS weight windows from it. Writes DIRECTORY/filled.h5 (labelled) and returns the record.

    MODEL has continuous-energy materials, the filler among them where it has empty space; `universe` is the one
    that fills it (the DAGMC universe, or the root of a CSG model). LIBRARY is the file `condense_multigroup`
    wrote; its copy in DIRECTORY is the one the run reads. BOUNDS is (lower_left, upper_right, dimension) of the
    window mesh, from `window_grid`. Each material takes its macroscopic entry of the library under its own name,
    and ADJOINT_DOMAINS that are materials must be those of MODEL."""
    import shutil

    import h5py

    sys.path.insert(0, str(Path(__file__).resolve().parent / "risk_tests"))
    import s0f_common

    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    local = directory / "mgxs.h5"
    shutil.copyfile(library, local)
    repaired = repair_library(local)
    # Each material points at its macroscopic entry, under its own name.
    model.materials.cross_sections = str(local)
    with h5py.File(local, "r") as stored:
        entries = library_entries([m.name for m in model.materials], list(stored.keys()))
    for material in model.materials:
        material.set_density("macro", 1.0)
        material._nuclides = []
        material._sab = []
        material.add_macroscopic(entries[material.name])
    settings = model.settings
    settings.energy_mode = "multi-group"
    settings.source = [source]
    settings.batches, settings.inactive = RANDOM_RAY["batches"], RANDOM_RAY["inactive"]
    settings.particles = RANDOM_RAY["rays"]
    settings.seed = WINDOW_SEED
    settings.output = {"tallies": False, "summary": False}
    lower, upper, dimension = bounds
    mesh = openmc.RegularMesh(name="ww-mesh")
    mesh.lower_left, mesh.upper_right, mesh.dimension = lower, upper, dimension
    edges = group_edges(threshold_ev)
    lines = response_lines(edges, threshold_ev)
    # The response is the flux above the threshold: weight 1 in each group above it, uniform over its domains.
    adjoint = openmc.IndependentSource(
        energy=openmc.stats.Discrete(lines, [1.0 / len(lines)] * len(lines)), particle="neutron",
        constraints={"domains": list(adjoint_domains)}, strength=1.0)
    settings.random_ray = {
        "distance_inactive": RANDOM_RAY["distance_inactive_cm"], "distance_active": RANDOM_RAY["distance_active_cm"],
        "ray_source": openmc.IndependentSource(space=openmc.stats.Box(lower, upper)),
        "volume_estimator": RANDOM_RAY["volume_estimator"], "source_shape": RANDOM_RAY["source_shape"],
        "adjoint": False, "sample_method": RANDOM_RAY["sample_method"],
        "diagonal_stabilization_rho": RANDOM_RAY["diagonal_stabilization_rho"],
        "source_region_meshes": [(mesh, [openmc.Universe(universe_id=universe.id)])],
        "adjoint_source": [adjoint],
    }
    settings.weight_window_generators = [openmc.WeightWindowGenerator(
        mesh=mesh, energy_bounds=list(edges), particle_type="neutron", method="fw_cadis",
        max_realizations=RANDOM_RAY["batches"] - RANDOM_RAY["inactive"])]
    model.export_to_model_xml(str(directory))
    run_cli(directory, threads, "openmc.log")
    files = sorted(directory.glob("weight_windows*.h5"))
    if not files:
        raise RuntimeError("the random-ray run wrote no weight windows")
    filled = directory / "filled.h5"
    fill = s0f_common.fill_file(files[0], filled)
    label_windows(filled)
    record = {"schema": "faris-design-windows/v1", "openmc": openmc.__version__, "parameters": RANDOM_RAY,
              "method": "CADIS: random-ray adjoint of the response, bins without a value filled from neighbours",
              "mesh": {"lower_left": lower, "upper_right": upper, "dimension": dimension},
              "group_edges_ev": edges, "mgxs_sha256": sha256_file(local), "totals_repaired": repaired,
              "weight_windows_sha256": sha256_file(files[0]), "filled_sha256": sha256_file(filled),
              "bins_filled_per_group": fill["bins_filled_per_group"],
              "elapsed_seconds": time.monotonic() - started}
    (directory / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    discard(directory, keep={"filled.h5", "record.json", "openmc.log"})
    return record
