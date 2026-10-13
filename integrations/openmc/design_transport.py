#!/usr/bin/env python3
"""Transport worker of `faris design transport`: a checked design's DAGMC geometry, run to a target error.

    design_transport.py --phase run     --request request.json --work-dir DIR
    design_transport.py --phase mgxs    --request request.json --work-dir DIR
    design_transport.py --phase windows --request request.json --work-dir DIR

The request (`faris-design-transport-request/v2`) is written by the engine from a design file
that passed `faris design check`:

    h5m                 path of the converted geometry; every solid is tagged with its solid id
    cross_sections      cross_sections.xml of the audited library
    solids              [{solid_id, volume_id, material_id}], material_id null for empty space
    materials           [{material_id, nuclide_atom_fractions, density_kg_m3, temperature_k}]
    plasma              the design file's plasma block (SI)
    chamber_solid_id    the solid the source sits in
    response            {solid_ids, energy_min_ev}: flux above the threshold in these solids
    sampling            {target_relative_error, particles_per_batch, min_batches, max_batches,
                         max_seconds, seed, threads}
    variance_reduction  "none" or "windows"

`run` scores the track-length flux above the threshold in each response solid and stops at the
first batch where the relative standard error of the sum over the solids is at or below the
target, or at the batch or time limit. The error of the sum comes from the per-batch sums,
so correlation between solids is included. Neutrons below the threshold are not followed:
they cannot score. Results are per source neutron (cm); the engine converts them to a flux.

With "windows", `mgxs` and `windows` run first and `run` uses what they leave in the work
directory. `mgxs` condenses the library to 17 groups, ten of them above 0.1 MeV, on the
materials of the design (OpenMC 0.15.3). `windows` solves the adjoint problem of the
response by random ray on a 25 cm grid and writes weight windows from it (CADIS; this needs
OpenMC 0.16 or later). `run` then splits and roulettes on those windows. If the OpenMC
library `run` loads was built with the aiming patch, it also looks for a straight channel
from the source to the response and aims at it (streaming_aim.py); otherwise the windows are
used as they are. Either way the result says which mode ran and why. This is the recipe of
docs/notes/CAD_TRANSPORT_S0F.md (pilot p1-fr, protocols 4 and 5).

Progress goes to standard output as one JSON object per line. The result file is
WORK/solver/result.json (`faris-design-transport/v2`). openmc is imported lazily so the pure
helpers import without it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import sys
import time
from pathlib import Path

REQUEST_SCHEMA = "faris-design-transport-request/v2"
RESULT_SCHEMA = "faris-design-transport/v2"
ENERGY_MAX_EV = 2.0e7
TALLY_ID = 1
OUTER_ID = 10_000_000
WINDOW_MESH_ID = 9000        # streaming_aim.py reads the windows of the mesh with this id
WINDOW_ID = 1
FILLER = "faris-filler"      # random ray cannot cross true void; empty space gets hydrogen at 1 mg/cm3
FILLER_DENSITY_KG_M3 = 1.0
SOURCE_ENERGY_EV = 14.06e6
WINDOW_CELL_CM = 25.0
MAX_SPLIT = 1000
# The steps that make the windows never share a random stream with the run that uses them (NUC-053).
MGXS_SEED_OFFSET, WINDOW_SEED_OFFSET = 1_000_003, 2_000_003
# 17 groups, ten above 0.1 MeV (S0f pilot p1-fr).
GROUP_EDGES_EV = [0.0, 0.058, 0.14, 0.28, 0.625, 4.0, 5530.0, 1.0e5, 5.0e5, 8.21e5, 1.353e6, 2.231e6, 3.679e6,
                  6.0655e6, 1.0e7, 1.3e7, 1.5e7, 2.0e7]
MGXS = {"method": "stochastic_slab", "correction": "P0", "nparticles": 20000, "batches": 60}
RANDOM_RAY = {"rays": 20000, "batches": 60, "inactive": 30, "distance_inactive_cm": 500.0,
              "distance_active_cm": 2500.0, "source_shape": "flat", "volume_estimator": "hybrid",
              "diagonal_stabilization_rho": 1.0, "sample_method": "prng"}
PROGRESS_SECONDS = 5.0


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


# ---------------------------------------------------------------------------
# Pure helpers (tested without OpenMC)

def tokamak_arguments(plasma: dict) -> dict:
    """The design file's plasma block (SI) as `tokamak_source` arguments (lengths in cm)."""
    density, temperature = plasma["ion_density_m3"], plasma["ion_temperature_ev"]
    return {
        "major_radius": plasma["major_radius_m"] * 100.0,
        "minor_radius": plasma["minor_radius_m"] * 100.0,
        "elongation": plasma["elongation"],
        "triangularity": plasma["triangularity"],
        "mode": plasma["mode"],
        "ion_density_centre": density["centre"],
        "ion_density_peaking_factor": density["peaking_factor"],
        "ion_density_pedestal": density["pedestal"],
        "ion_density_separatrix": density["separatrix"],
        "ion_temperature_centre": temperature["centre"],
        "ion_temperature_peaking_factor": temperature["peaking_factor"],
        "ion_temperature_beta": temperature["beta"],
        "ion_temperature_pedestal": temperature["pedestal"],
        "ion_temperature_separatrix": temperature["separatrix"],
        "pedestal_radius": plasma["pedestal_radius_m"] * 100.0,
        "shafranov_factor": plasma["shafranov_shift_m"] * 100.0,
        "mesh_resolution": (100, 100),
        "fuel": plasma["fuel"],
    }


def validate_request(request: dict) -> None:
    def need(condition, message):
        if not condition:
            raise ValueError(message)

    need(request.get("schema") == REQUEST_SCHEMA, f"the request is not {REQUEST_SCHEMA}")
    need(request.get("variance_reduction") in ("none", "windows"), "variance_reduction must be none or windows")
    sampling = request["sampling"]
    need(0.0 < sampling["target_relative_error"] < 1.0, "target_relative_error must lie between 0 and 1")
    need(sampling["particles_per_batch"] >= 1, "particles_per_batch must be at least 1")
    need(2 <= sampling["min_batches"] <= sampling["max_batches"], "need 2 <= min_batches <= max_batches")
    need(sampling["max_seconds"] > 0.0, "max_seconds must be positive")
    need(1 <= sampling["threads"] <= 32, "threads must lie between 1 and 32")
    solids = {s["solid_id"]: s for s in request["solids"]}
    need(len(solids) == len(request["solids"]), "a solid is listed twice")
    volumes = [s["volume_id"] for s in request["solids"]]
    need(len(set(volumes)) == len(volumes), "two solids share a volume id")
    materials = {m["material_id"] for m in request["materials"]}
    need(len(materials) == len(request["materials"]), "a material is listed twice")
    need(FILLER not in materials, f"the material id {FILLER} is reserved")
    for solid in request["solids"]:
        need(solid["material_id"] is None or solid["material_id"] in materials,
             f"{solid['solid_id']}: its material is not listed")
    response = request["response"]
    need(response["solid_ids"], "the response names no solid")
    for solid_id in response["solid_ids"]:
        need(solid_id in solids, f"the response solid {solid_id} is not in the model")
        need(solids[solid_id]["material_id"] is not None, f"the response solid {solid_id} is empty space")
    need(len(set(response["solid_ids"])) == len(response["solid_ids"]), "a response solid is listed twice")
    need(0.0 <= response["energy_min_ev"] < ENERGY_MAX_EV, "energy_min_ev is outside the data range")
    need(request["chamber_solid_id"] in solids, "the plasma chamber solid is not in the model")
    for material in request["materials"]:
        need(material["density_kg_m3"] > 0.0, f"{material['material_id']}: density must be positive")
        need(material["nuclide_atom_fractions"], f"{material['material_id']}: no nuclides")


def retag_map(solids: list[dict], random_ray: bool) -> dict[bytes, bytes]:
    """h5m group names to rewrite: each solid's own tag becomes its material's.

    `faris design check` tags every solid with its own id so that each DAGMC volume can be matched to its solid.
    A run wants one material per design material. Empty space becomes `void`, which OpenMC reads as vacuum, or the
    filler for random ray, which cannot cross true void; the space outside every solid follows.
    """
    empty = FILLER if random_ray else "void"
    names = {f"mat:{s['solid_id']}".encode(): f"mat:{s['material_id'] or empty}".encode() for s in solids}
    if random_ray:
        names[b"mat:void_comp"] = f"mat:{FILLER}_comp".encode()
    return names


def retagged(values, names: dict[bytes, bytes], width: int) -> tuple[list[bytes], int]:
    """The NAME values with `names` applied, padded to `width`, and how many changed."""
    out, hits = [], 0
    for raw in values:
        name = bytes(raw).split(b"\0", 1)[0]
        new = names.get(name)
        if new is None:
            out.append(bytes(raw))
            continue
        if len(new) > width:
            raise ValueError(f"the tag {new.decode()} is longer than the {width} bytes an h5m name holds")
        out.append(new.ljust(width, b"\0"))
        hits += 1
    return out, hits


def library_entries(names: list[str], stored: list[str]) -> dict[str, str]:
    """The multigroup library's entry for each material. The library rewrites characters such as `-`, so names
    are compared with everything but letters and digits made equal; each must match exactly one entry."""
    def plain(name: str) -> str:
        return "".join(c if c.isalnum() else "_" for c in name)

    out = {}
    for name in names:
        hits = [entry for entry in stored if plain(entry) == plain(name)]
        if len(hits) != 1:
            raise ValueError(f"the multigroup library has {len(hits)} entries for the material {name}")
        out[name] = hits[0]
    if len(set(out.values())) != len(out):
        raise ValueError("two materials share one entry of the multigroup library")
    return out


def response_groups(edges_ev: list[float], energy_min_ev: float) -> list[float]:
    """The group edges at and above the response threshold, which must be one of them."""
    if energy_min_ev not in edges_ev:
        raise ValueError(f"the response threshold {energy_min_ev:g} eV is not an edge of the group structure")
    return edges_ev[edges_ev.index(energy_min_ev):]


def window_grid(lower: list[float], upper: list[float], cell_cm: float = WINDOW_CELL_CM) -> tuple[list[float], list[float], list[int]]:
    """A box around the model, 5 cm clear on every side, in cells of about `cell_cm`."""
    lo = [math.floor(v - 5.0) for v in lower]
    hi = [math.ceil(v + 5.0) for v in upper]
    return [float(v) for v in lo], [float(v) for v in hi], [max(1, round((h - l) / cell_cm)) for l, h in zip(lo, hi)]


class RunningSum:
    """Mean and standard error of per-batch values, one batch at a time."""

    def __init__(self):
        self.n, self.total, self.total_sq = 0, 0.0, 0.0

    def add(self, value: float) -> None:
        self.n += 1
        self.total += value
        self.total_sq += value * value

    @property
    def mean(self) -> float:
        return self.total / self.n if self.n else 0.0

    @property
    def standard_error(self) -> float | None:
        if self.n < 2:
            return None
        variance = (self.total_sq / self.n - self.mean ** 2) / (self.n - 1)
        return math.sqrt(max(variance, 0.0))

    @property
    def relative_error(self) -> float | None:
        se = self.standard_error
        if se is None or self.mean <= 0.0:
            return None
        return se / self.mean


def stop_reason(stats: RunningSum, sampling: dict, elapsed: float) -> str | None:
    """Why the run stops after this batch, or None to continue. The target is checked first."""
    rel = stats.relative_error
    if stats.n >= sampling["min_batches"] and rel is not None and rel <= sampling["target_relative_error"]:
        return "target_reached"
    if stats.n >= sampling["max_batches"]:
        return "history_limit"
    if elapsed >= sampling["max_seconds"]:
        return "time_limit"
    return None


def explain(reason: str, stats: RunningSum, sampling: dict, windows: bool = False) -> tuple[str, str]:
    """(why, next_step) for a run that stopped before its target."""
    rel = stats.relative_error
    seen = "no nonzero score was recorded" if rel is None else f"the error reached {100.0 * rel:.1f} %"
    target = 100.0 * sampling["target_relative_error"]
    other = "" if windows else ", or run with weight windows"
    if rel is None:
        more = f"Raise the history and time limits by a large factor{other}."
    else:
        factor = (rel / sampling["target_relative_error"]) ** 2
        more = f"About {factor:.1f} times as many histories are needed for {target:g} %: raise the limits{other}."
    limit = "history limit" if reason == "history_limit" else "time limit"
    return f"The run stopped at its {limit}; {seen} against a target of {target:g} %.", more


# ---------------------------------------------------------------------------
# OpenMC

def geometry_copy(request: dict, target: Path, random_ray: bool) -> Path:
    """A copy of the h5m with the tags of `retag_map`; every solid must be found."""
    import shutil

    import h5py

    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(request["h5m"], target)
    names = retag_map(request["solids"], random_ray)
    with h5py.File(target, "r+") as f:
        values = f["tstt"]["tags"]["NAME"]["values"]
        old = values[()]
        new, hits = retagged(old, names, old.dtype.itemsize)
        if hits != len(names):
            raise ValueError(f"{len(names)} tags were to be rewritten but {hits} were found in {request['h5m']}")
        for i, value in enumerate(new):
            old[i] = value
        values[...] = old
    return target


def build_model(openmc, request: dict, geometry: Path, random_ray: bool = False):
    """Geometry in a vacuum sphere and one material per design material. Returns (model, universe, materials)."""
    materials = {}
    entries = sorted(request["materials"], key=lambda m: m["material_id"])
    for number, entry in enumerate(entries, start=1):
        material = openmc.Material(material_id=number, name=entry["material_id"])
        for nuclide, fraction in sorted(entry["nuclide_atom_fractions"].items()):
            material.add_nuclide(nuclide, fraction, percent_type="ao")
        material.set_density("kg/m3", entry["density_kg_m3"])
        if entry.get("temperature_k") is not None:
            material.temperature = entry["temperature_k"]
        materials[entry["material_id"]] = material
    if random_ray:
        filler = openmc.Material(material_id=len(entries) + 1, name=FILLER)
        filler.add_nuclide("H1", 1.0, percent_type="ao")
        filler.set_density("kg/m3", FILLER_DENSITY_KG_M3)
        materials[FILLER] = filler
    universe = openmc.DAGMCUniverse(str(geometry))
    box = universe.bounding_box
    radius = 1.05 * max(math.sqrt(sum(c * c for c in corner)) for corner in (box.lower_left, box.upper_right))
    # Ids far above the DAGMC volumes and surfaces, which keep the numbers of the h5m.
    sphere = openmc.Sphere(surface_id=OUTER_ID, r=radius, boundary_type="vacuum")
    model = openmc.Model(geometry=openmc.Geometry([openmc.Cell(cell_id=OUTER_ID, fill=universe, region=-sphere)]),
                         materials=openmc.Materials(list(materials.values())))
    model.materials.cross_sections = str(request["cross_sections"])
    model.settings.run_mode = "fixed source"
    model.settings.photon_transport = False
    return model, universe, materials


def volume_ids(request: dict) -> dict:
    return {s["solid_id"]: int(s["volume_id"]) for s in request["solids"]}


def response_materials(request: dict) -> list[str]:
    by_solid = {s["solid_id"]: s["material_id"] for s in request["solids"]}
    return sorted({by_solid[s] for s in request["response"]["solid_ids"]})


def point_source(openmc, request: dict):
    """14.06 MeV neutrons born uniformly in the plasma chamber: the source of the deterministic steps."""
    chamber = openmc.Cell(cell_id=volume_ids(request)[request["chamber_solid_id"]])
    return openmc.IndependentSource(energy=openmc.stats.Discrete([SOURCE_ENERGY_EV], [1.0]), particle="neutron",
                                    constraints={"domains": [chamber]}, strength=1.0)


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


def phase_mgxs(request: dict, work: Path) -> dict:
    """Condense the library to GROUP_EDGES_EV on the design's materials. Writes WORK/mgxs/mgxs.h5."""
    import openmc
    import openmc.mgxs

    directory = work / "mgxs"
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    geometry = geometry_copy(request, work / "geometry" / "model-rr.h5m", random_ray=True)
    model, _, _ = build_model(openmc, request, geometry, random_ray=True)
    model.settings.source = [point_source(openmc, request)]
    model.settings.batches = MGXS["batches"]
    model.settings.particles = MGXS["nparticles"]
    model.settings.seed = int(request["sampling"]["seed"]) + MGXS_SEED_OFFSET
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(int(request["sampling"]["threads"]))
    target = directory / "mgxs.h5"
    here = os.getcwd()
    os.chdir(directory)
    try:
        model.convert_to_multigroup(method=MGXS["method"], groups=openmc.mgxs.EnergyGroups(GROUP_EDGES_EV),
                                    nparticles=MGXS["nparticles"], overwrite_mgxs_library=False,
                                    mgxs_path=str(target), correction=MGXS["correction"])
    finally:
        os.chdir(here)
    if not target.is_file():
        raise RuntimeError("the multigroup step wrote no library")
    discard(directory, keep={"mgxs.h5"})
    record = {"schema": "faris-design-mgxs/v1", "openmc": openmc.__version__, "parameters": MGXS,
              "group_edges_ev": GROUP_EDGES_EV, "mgxs_sha256": sha256_file(target),
              "elapsed_seconds": time.monotonic() - started}
    (directory / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    return record


def phase_windows(request: dict, work: Path) -> dict:
    """Random-ray adjoint of the response and CADIS weight windows from it. Writes WORK/windows/filled.h5."""
    import shutil

    import openmc

    sys.path.insert(0, str(Path(__file__).resolve().parent / "risk_tests"))
    import s0f_common

    directory = work / "windows"
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    threads = int(request["sampling"]["threads"])
    geometry = work / "geometry" / "model-rr.h5m"
    if not geometry.is_file():
        geometry_copy(request, geometry, random_ray=True)
    library = directory / "mgxs.h5"
    shutil.copyfile(work / "mgxs" / "mgxs.h5", library)
    model, universe, materials = build_model(openmc, request, geometry, random_ray=True)
    # Each material points at its macroscopic entry, under its own name.
    model.materials.cross_sections = str(library)
    import h5py

    with h5py.File(library, "r") as stored:
        entries = library_entries([m.name for m in model.materials], list(stored.keys()))
    for material in model.materials:
        material.set_density("macro", 1.0)
        material._nuclides = []
        material._sab = []
        material.add_macroscopic(entries[material.name])
    settings = model.settings
    settings.energy_mode = "multi-group"
    settings.source = [point_source(openmc, request)]
    settings.batches, settings.inactive = RANDOM_RAY["batches"], RANDOM_RAY["inactive"]
    settings.particles = RANDOM_RAY["rays"]
    settings.seed = int(request["sampling"]["seed"]) + WINDOW_SEED_OFFSET
    settings.output = {"tallies": False, "summary": False}
    box = universe.bounding_box
    lower, upper, dimension = window_grid(list(box.lower_left), list(box.upper_right))
    mesh = openmc.RegularMesh(name="ww-mesh")
    mesh.lower_left, mesh.upper_right, mesh.dimension = lower, upper, dimension
    groups = response_groups(GROUP_EDGES_EV, float(request["response"]["energy_min_ev"]))
    lines = [math.sqrt(a * b) for a, b in zip(groups, groups[1:])]
    # The response is the flux above the threshold: weight 1 in each group above it, uniform over its materials.
    adjoint = openmc.IndependentSource(
        energy=openmc.stats.Discrete(lines, [1.0 / len(lines)] * len(lines)), particle="neutron",
        constraints={"domains": [materials[m] for m in response_materials(request)]}, strength=1.0)
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
        mesh=mesh, energy_bounds=list(GROUP_EDGES_EV), particle_type="neutron", method="fw_cadis",
        max_realizations=RANDOM_RAY["batches"] - RANDOM_RAY["inactive"])]
    model.export_to_model_xml(str(directory))
    run_cli(directory, threads, "openmc.log")
    files = sorted(directory.glob("weight_windows*.h5"))
    if not files:
        raise RuntimeError("the random-ray run wrote no weight windows")
    filled = directory / "filled.h5"
    fill = s0f_common.fill_file(files[0], filled)
    record = {"schema": "faris-design-windows/v1", "openmc": openmc.__version__, "parameters": RANDOM_RAY,
              "method": "CADIS: random-ray adjoint of the response, bins without a value filled from neighbours",
              "mesh": {"lower_left": lower, "upper_right": upper, "dimension": dimension},
              "group_edges_ev": GROUP_EDGES_EV, "mgxs_sha256": sha256_file(library),
              "weight_windows_sha256": sha256_file(files[0]), "filled_sha256": sha256_file(filled),
              "bins_filled_per_group": fill["bins_filled_per_group"],
              "elapsed_seconds": time.monotonic() - started}
    (directory / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    discard(directory, keep={"filled.h5", "record.json", "openmc.log"})
    geometry.unlink()
    return record


def apply_windows(openmc, model, filled: Path) -> None:
    window = openmc.hdf5_to_wws(str(filled))[0]
    window.id, window.max_split = WINDOW_ID, MAX_SPLIT
    window.mesh.id = WINDOW_MESH_ID
    model.settings.weight_windows = [window]
    model.settings.weight_windows_on = True
    model.settings.weight_window_checkpoints = {"collision": True, "surface": True}


def plan_aim(request: dict, model_xml: Path, work: Path, material_numbers: dict) -> dict:
    """Ask streaming_aim.py whether this run can aim; always returns a plan with its reason and next step."""
    import openmc.lib

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import streaming_aim

    library = Path(openmc.lib._dll._name)
    response = response_materials(request)
    if len(response) != 1:
        return streaming_aim.windows_only(
            "The response solids are made of more than one material, and aiming handles one.",
            "Nothing to do: the weight windows apply as they are.")
    try:
        return streaming_aim.plan(model_xml, work / "aim", library, material_numbers[response[0]], keep_runs=False)
    except Exception as error:  # the search is an accelerator: its failure must not stop the run
        return streaming_aim.windows_only(
            f"The search for a streaming channel failed ({type(error).__name__}: {error}).",
            "Results are unaffected; the weight windows apply as they are.")


def phase_run(request: dict, work: Path) -> dict:
    import numpy as np
    import openmc
    import openmc.lib
    from openmc_plasma_source import tokamak_source

    out_dir = work / "solver"
    out_dir.mkdir(parents=True, exist_ok=True)
    sampling, response = request["sampling"], request["response"]
    geometry = geometry_copy(request, work / "geometry" / "model-mc.h5m", random_ray=False)
    model, _, materials = build_model(openmc, request, geometry)

    source = tokamak_source(**tokamak_arguments(request["plasma"]))
    total = float(source.strength)
    for element in source.sources.ravel():
        element.strength = float(element.strength) / total
    source.strength = 1.0
    settings = model.settings
    settings.source = [source]
    settings.particles = int(sampling["particles_per_batch"])
    settings.batches = int(sampling["max_batches"])
    settings.seed = int(sampling["seed"])
    # A neutron below the threshold cannot score and is not followed.
    settings.cutoff = {"energy_neutron": float(response["energy_min_ev"])}
    settings.output = {"tallies": False, "summary": False}
    settings.sourcepoint = {"write": False}
    settings.statepoint = {"batches": []}

    solids = response["solid_ids"]
    ids = volume_ids(request)
    tally = openmc.Tally(tally_id=TALLY_ID, name="response-fast-flux")
    tally.filters = [
        openmc.CellFilter([ids[s] for s in solids]),
        openmc.EnergyFilter([float(response["energy_min_ev"]), ENERGY_MAX_EV]),
    ]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    model.tallies = openmc.Tallies([tally])

    mode = {"mode": "analog", "reason": "No weight windows were asked for.",
            "next_step": "Run with weight windows to reach the target sooner.", "env": {}}
    windows_record = None
    aim_seconds = 0.0
    model_xml = out_dir / "model.xml"
    if request["variance_reduction"] == "windows":
        filled = work / "windows" / "filled.h5"
        windows_record = json.loads((work / "windows" / "record.json").read_text())
        if sha256_file(filled) != windows_record["filled_sha256"]:
            raise RuntimeError("the weight windows do not match their record")
        apply_windows(openmc, model, filled)
        model.export_to_model_xml(model_xml)
        numbers = {name: material.id for name, material in materials.items()}
        aim_started = time.monotonic()
        mode = plan_aim(request, model_xml, work, numbers)
        aim_seconds = time.monotonic() - aim_started
        if mode["mode"] == "aimed":
            model_xml.unlink()
            model_xml = Path(mode["model"])
            os.environ.update(mode["env"])
    else:
        model.export_to_model_xml(model_xml)

    here = os.getcwd()
    os.chdir(out_dir)
    stats = RunningSum()
    per_solid = [RunningSum() for _ in solids]
    started = time.monotonic()
    reason = None
    try:
        openmc.lib.init(["--threads", str(int(sampling["threads"])), str(model_xml)])
        try:
            openmc.lib.simulation_init()
            handle = openmc.lib.tallies[TALLY_ID]
            previous = np.zeros(len(solids))
            last_report = started
            while reason is None:
                openmc.lib.next_batch()
                # Column 1 holds the sum of the batch values so far; its increase is this batch's value.
                sums = np.array(handle.results[:, 0, 1], dtype=float)
                batch = sums - previous
                previous = sums
                stats.add(float(batch.sum()))
                for accumulator, value in zip(per_solid, batch):
                    accumulator.add(float(value))
                elapsed = time.monotonic() - started
                reason = stop_reason(stats, sampling, elapsed)
                if reason is not None or time.monotonic() - last_report >= PROGRESS_SECONDS:
                    last_report = time.monotonic()
                    print(json.dumps({
                        "progress": {
                            "batches": stats.n,
                            "histories": stats.n * int(sampling["particles_per_batch"]),
                            "relative_error": stats.relative_error,
                            "elapsed_seconds": elapsed,
                        }}), flush=True)
            openmc.lib.simulation_finalize()
        finally:
            openmc.lib.finalize()
    finally:
        os.chdir(here)
    elapsed = time.monotonic() - started
    result = {
        "schema": RESULT_SCHEMA,
        "openmc": openmc.__version__,
        "h5m_sha256": sha256_file(request["h5m"]),
        "run_h5m_sha256": sha256_file(geometry),
        "model_xml_sha256": sha256_file(model_xml),
        "seed": int(sampling["seed"]),
        "threads": int(sampling["threads"]),
        "particles_per_batch": int(sampling["particles_per_batch"]),
        "batches": stats.n,
        "histories": stats.n * int(sampling["particles_per_batch"]),
        "elapsed_seconds": elapsed,
        "stop_reason": reason,
        "target_relative_error": sampling["target_relative_error"],
        "target_reached": reason == "target_reached",
        "estimator": "tracklength",
        "unit": "cm_per_source",
        "energy_min_ev": response["energy_min_ev"],
        "total": {"mean": stats.mean, "standard_error": stats.standard_error},
        "solids": [{"solid_id": s, "mean": a.mean, "standard_error": a.standard_error}
                   for s, a in zip(solids, per_solid)],
        "variance_reduction": {
            "mode": mode["mode"], "reason": mode["reason"], "next_step": mode["next_step"],
            "targets": [t["name"] for t in mode.get("targets", [])],
            "windows_sha256": windows_record["filled_sha256"] if windows_record else None,
            "aim_seconds": aim_seconds,
        },
        "why": None,
        "next_step": None,
    }
    if reason != "target_reached":
        result["why"], result["next_step"] = explain(reason, stats, sampling, mode["mode"] != "analog")
    target = out_dir / "result.json"
    with target.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=1)
        stream.write("\n")
    return result


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--phase", choices=("run", "mgxs", "windows"), required=True)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    request = json.loads(args.request.read_text(encoding="utf-8"))
    try:
        validate_request(request)
    except (KeyError, TypeError, ValueError) as error:
        print(f"invalid request: {error}", file=sys.stderr)
        return 2
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    {"run": phase_run, "mgxs": phase_mgxs, "windows": phase_windows}[args.phase](request, work)
    return 0


if __name__ == "__main__":
    sys.exit(main())
