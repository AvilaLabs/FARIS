#!/usr/bin/env python3
"""Transport worker of `faris design transport`: a checked design's DAGMC geometry, run to a target error.

    design_transport.py --phase run     --request request.json --work-dir DIR
    design_transport.py --phase mgxs    --request request.json --work-dir DIR
    design_transport.py --phase windows --request request.json --work-dir DIR
    design_transport.py --phase volumes --request request.json --work-dir DIR

The request (`faris-design-transport-request/v3`) is written by the engine from a design file
that passed `faris design check`:

    h5m                 path of the converted geometry; every solid is tagged with its solid id
    cross_sections      cross_sections.xml of the audited library
    solids              [{solid_id, volume_id, material_id}], material_id null for empty space
    materials           [{material_id, nuclide_atom_fractions, density_kg_m3, temperature_k}]
    plasma              the design file's plasma block (SI)
    chamber_solid_id    the solid the source sits in
    response            {solid_ids, energy_min_ev}: flux above the threshold in these solids
                        (any solids that are not empty space; 0 for the flux at all energies);
                        with peak_voxel_cm [dr, dz] also the highest flux inside each solid
    sampling            {target_relative_error, particles_per_batch, min_batches, max_batches,
                         max_seconds, seed, threads}
    variance_reduction  "none" or "windows"

`run` scores the track-length flux above the threshold in each response solid and stops at the
first batch (of at least min_batches) where the relative standard error of the sum over the solids is at or below the
target, or at the batch or time limit. The error of the sum comes from the per-batch sums,
so correlation between solids is included. Neutrons below the threshold are not followed:
they cannot score. Results are per source neutron (cm); the engine converts them to a flux.

With "windows", `mgxs` and `windows` run first and `run` uses what they leave in the work
directory. `mgxs` condenses the library to 17 groups, ten of them above 0.1 MeV, on the
materials of the design (OpenMC 0.15.3). `windows` solves the adjoint problem of the
response by random ray on a 25 cm grid and writes weight windows from it (CADIS; this needs
OpenMC 0.16 or later). `run` then splits and roulettes on those windows, which it hands to OpenMC as an HDF5
file (WORK/solver/windows.h5), never as XML text: on a large machine the bounds are hundreds of megabytes. If the OpenMC
library `run` loads was built with the aiming patch, it also looks for duct openings
that neutrons cross on their way from the source to the response without a collision, and aims at them (streaming_aim.py); otherwise the windows are
used as they are. Either way the result says which mode ran and why. This is the recipe of
docs/notes/CAD_TRANSPORT_S0F.md (pilot p1-fr, protocols 4 and 5).

With `peak_voxel_cm`, `volumes` runs before `run`. The peak is taken over pieces of each solid:
rings of dr by dz about the z axis, each cut by the solid, so a piece spans the solid's full
toroidal width (null for dr or dz: its full width in that direction too). `volumes` measures
the volume of every piece by ray tracing; `run` scores the same pieces and reports, per solid,
its leading piece: the one with the highest flux after two standard errors are taken off, so
a piece that one rare score landed in does not lead. Slivers at a solid's edge (under a quarter
of its widest toroidal span) are not ranked.
The run then stops only when the piece that leads at that batch is within twice the target as well
(10 % for the default 5 %): one piece of a few litres takes many more histories than the mean.

Progress goes to standard output as one JSON object per line. The result file is
WORK/solver/result.json (`faris-design-transport/v3`). openmc is imported lazily so the pure
helpers import without it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import shutil
import sys
import time
from pathlib import Path

from batch_statistics import VOV_LIMIT, RunningSum  # noqa: F401  (the tests reach these through this module)
from window_generator import (  # noqa: F401  (the tests and the other steps reach these through this module)
    FILLER, FILLER_DENSITY_KG_M3, GROUP_EDGES_EV, MAX_SPLIT, MGXS, MGXS_SEED, RANDOM_RAY, SOURCE_ENERGY_EV,
    WINDOW_CELL_CM, WINDOW_ID, WINDOW_MESH_ID, WINDOW_SEED, apply_windows, condense_multigroup, discard,
    domain_source, export_model, filler_material, group_edges, label_windows, library_entries, response_lines, run_cli,
    sha256_file, solve_windows, window_file_name, window_grid)

REQUEST_SCHEMA = "faris-design-transport-request/v3"
RESULT_SCHEMA = "faris-design-transport/v3"
ENERGY_MAX_EV = 2.0e7
TALLY_ID = 1
OUTER_ID = 10_000_000
AIM_THRESHOLD_EV = 1.0e5     # the aiming helpers were built and measured for this threshold only
# A roulette survivor may be at most this many times heavier than the particle it replaces (patched OpenMC,
# OPENMC_WW_ROULETTE_FACTOR; stock OpenMC ignores it). MCNP limits one split or roulette to 5 (MXSPLN on the WWP
# card). Without it a particle beside a port mouth, where coarse windows are too high, survived 210 times heavier
# and then dominated a batch (FARIS-010).
ROULETTE_FACTOR = 5.0
PROGRESS_SECONDS = 5.0
# A neutron the geometry cannot place is lost. Above this share of the histories the answer is not trusted.
LOST_FRACTION_LIMIT = 1.0e-6
LOST_LOCATIONS = 10          # how many lost neutrons have their position recorded
# The peak inside a solid: rings of dr by dz about the machine axis, each cut by the solid.
PEAK_TALLY_ID = 2
PEAK_MESH_ID = 9001
PEAK_RAYS = 2_000_000        # rays that measure how much of each ring lies in each solid
PEAK_TARGET_FACTOR = 2.0     # the highest piece is held to this many times the target on the mean
PEAK_MIN_FILL = 0.25         # a piece spanning under this share of the solid's widest toroidal angle is a sliver
PEAK_SIGMAS = 2.0            # the leading piece is the highest after this many standard errors are taken off


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
    need(sampling["seed"] not in (MGXS_SEED, WINDOW_SEED), "this seed is reserved for the weight-window steps")
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
    voxel = response.get("peak_voxel_cm")
    if voxel is not None:
        need(len(voxel) == 2 and all(v is None or v >= 1.0 for v in voxel),
             "peak_voxel_cm must be [dr, dz] with each at least 1 cm or null")
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


def ring_edges(low: float, high: float, step: float | None) -> list[float]:
    """Edges from `low` to at least `high` in steps of `step`; one bin when `step` is None."""
    if step is None:
        return [float(low), float(high)]
    count = max(1, math.ceil((high - low) / step - 1.0e-9))
    return [low + i * step for i in range(count + 1)]


def ranked_pieces(volumes, r_edges, dr_dz_cells: int | None = None, min_fill: float = PEAK_MIN_FILL):
    """Which pieces of each solid are compared (numpy): `volumes[solid, z, r]` in cm3 to a mask of that shape.

    A ring's volume grows with its radius, so volumes are compared as the toroidal angle each piece spans
    (volume over mid-radius). A piece spanning under `min_fill` of the solid's widest is a sliver at the
    solid's edge: its volume is poorly known and its score is noise.
    """
    import numpy as np

    edges = np.asarray(r_edges, dtype=float)
    span = volumes / (0.5 * (edges[:-1] + edges[1:]))
    widest = span.reshape(span.shape[0], -1).max(axis=1)[:, None, None]
    return (span > 0.0) & (span >= min_fill * widest)


def leading_piece(scored, volumes, ranked, batches: int):
    """Relative error of the leading piece over all solids (numpy), or None when unknown.

    `scored[piece, solid]` holds (sum, sum of squares) of the batch values; `volumes[solid, piece]` (cm3) and
    `ranked[solid, piece]` are flat over the pieces. The leading piece is chosen as in `peak_of`.
    """
    import numpy as np

    if batches < 2 or not ranked.any():
        return None
    mean = scored[:, :, 0].T / batches
    error = np.sqrt(np.maximum(scored[:, :, 1].T / batches - mean * mean, 0.0) / (batches - 1))
    safe = np.where(ranked, volumes, 1.0)
    bound = np.where(ranked, (mean - PEAK_SIGMAS * error) / safe, -np.inf)
    if not bound.max() > 0.0:
        bound = np.where(ranked, mean / safe, -np.inf)
    solid, piece = np.unravel_index(int(np.argmax(bound)), bound.shape)
    if mean[solid, piece] <= 0.0:
        return None
    return float(error[solid, piece] / mean[solid, piece])


def peak_of(sums, squares, volumes, ranked, batches: int) -> dict | None:
    """The leading piece of one solid, from each piece's sum of batch values, sum of their squares, volume
    (cm3) and whether it is ranked.

    The leader is the piece whose flux is highest after PEAK_SIGMAS standard errors are taken off: the highest
    flux the run has resolved. Ranking by the mean alone would pick whichever piece a rare large score landed
    in. With no piece resolved above zero the highest mean leads. Returns the flat index, mean, standard error
    and volume, or None with no ranked piece.
    """
    best, best_key = None, None
    for index, (total, square, volume, keep) in enumerate(zip(sums, squares, volumes, ranked)):
        if not keep:
            continue
        mean = total / batches
        error = None
        if batches >= 2:
            error = math.sqrt(max(square / batches - mean * mean, 0.0) / (batches - 1))
        bound = (mean - PEAK_SIGMAS * (error or 0.0)) / volume if error is not None else 0.0
        key = (bound > 0.0, bound if bound > 0.0 else mean / volume)
        if best is None or key > best_key:
            best, best_key = {"index": index, "mean": mean, "standard_error": error, "volume_cm3": volume}, key
    return best


def lost_too_many(lost: int | None, histories: int) -> bool:
    """True when the lost share of the histories is above the limit. One loss in under a million histories is
    above the share but is not judged until the run is long enough to tell."""
    return bool(lost) and histories >= 1.0 / LOST_FRACTION_LIMIT and lost > LOST_FRACTION_LIMIT * histories


def lost_count(dll) -> int | None:
    """OpenMC's count of lost particles, or None when this build does not expose it."""
    import ctypes

    try:
        return int(ctypes.c_int.in_dll(dll, "n_lost_particles").value)
    except (ValueError, AttributeError):
        return None


def lost_positions(directory: Path) -> list[list[float]]:
    """Where the first lost neutrons were, from the particle files OpenMC wrote in `directory` (cm)."""
    import h5py

    out = []
    for path in sorted(Path(directory).glob("particle_*.h5"))[:LOST_LOCATIONS]:
        try:
            with h5py.File(path, "r") as f:
                out.append([float(v) for v in f["xyz"][()]])
        except (OSError, KeyError):
            continue
    return out


# A batch more than this many times the median of the batches before it is recorded with its per-piece scores
# (after the first few batches): the trace of a rare heavy history.
HEAVY_BATCH_FACTOR = 10.0
HEAVY_BATCH_AFTER = 5


def stop_reason(stats: RunningSum, sampling: dict, elapsed: float, peak: tuple | None = None) -> str | None:
    """Why the run stops after this batch, or None to continue. The target is checked first.

    `peak` is None when no peak is asked for, else a one-item tuple holding the relative error of the highest
    piece (None when unknown): it must then be within PEAK_TARGET_FACTOR times the target as well.
    """
    rel, target = stats.relative_error, sampling["target_relative_error"]
    peak_met = peak is None or (peak[0] is not None and peak[0] <= PEAK_TARGET_FACTOR * target)
    vov = stats.variance_of_variance
    settled = vov is None or vov <= VOV_LIMIT
    if stats.n >= sampling["min_batches"] and rel is not None and rel <= target and peak_met and settled:
        return "target_reached"
    if stats.n >= sampling["max_batches"]:
        return "history_limit"
    if elapsed >= sampling["max_seconds"]:
        return "time_limit"
    return None


def explain(reason: str, stats: RunningSum, sampling: dict, windows: bool = False,
            peak: tuple | None = None) -> tuple[str, str]:
    """(why, next_step) for a run that stopped before its target. `peak` as for `stop_reason`."""
    rel = stats.relative_error
    seen = "no nonzero score was recorded" if rel is None else f"the error reached {100.0 * rel:.1f} %"
    if rel is not None and peak is not None:
        if peak[0] is None:
            seen += " on the mean and is unknown for the highest piece"
        else:
            seen += (f" on the mean and {100.0 * peak[0]:.1f} % on the highest piece (held to "
                     f"{100.0 * PEAK_TARGET_FACTOR * sampling['target_relative_error']:g} %)")
            rel = max(rel, peak[0] / PEAK_TARGET_FACTOR)
    target = 100.0 * sampling["target_relative_error"]
    other = "" if windows else ", or run with weight windows"
    if rel is None:
        more = f"Raise the history and time limits by a large factor{other}."
    else:
        factor = (rel / sampling["target_relative_error"]) ** 2
        more = f"About {factor:.1f} times as many histories are needed for {target:g} %: raise the limits{other}."
    limit = "history limit" if reason == "history_limit" else "time limit"
    vov = stats.variance_of_variance
    if vov is not None and vov > VOV_LIMIT:
        seen += (f", and a few batches hold much of the sum (variance of the variance {vov:.2f}, above "
                 f"{VOV_LIMIT:g}): a rare heavy history, so the error itself is not yet reliable")
        more += " If it recurs, report the model: the weight windows may let too heavy a history through."
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
        materials[FILLER] = filler_material(openmc, len(entries) + 1)
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
    return domain_source(openmc, openmc.Cell(cell_id=volume_ids(request)[request["chamber_solid_id"]]))


def phase_mgxs(request: dict, work: Path) -> dict:
    """Condense the library to the response's group structure on the design's materials. Writes WORK/mgxs/mgxs.h5."""
    import openmc

    edges = group_edges(float(request["response"]["energy_min_ev"]))
    geometry = geometry_copy(request, work / "geometry" / "model-rr.h5m", random_ray=True)
    model, _, _ = build_model(openmc, request, geometry, random_ray=True)
    return condense_multigroup(openmc, model, edges, point_source(openmc, request), work / "mgxs",
                               int(request["sampling"]["threads"]))


def phase_windows(request: dict, work: Path) -> dict:
    """Random-ray adjoint of the response and CADIS weight windows from it. Writes WORK/windows/filled.h5."""
    import openmc

    geometry = work / "geometry" / "model-rr.h5m"
    if not geometry.is_file():
        geometry_copy(request, geometry, random_ray=True)
    model, universe, materials = build_model(openmc, request, geometry, random_ray=True)
    box = universe.bounding_box
    record = solve_windows(
        openmc, model, universe, point_source(openmc, request),
        [materials[m] for m in response_materials(request)], float(request["response"]["energy_min_ev"]),
        window_grid(list(box.lower_left), list(box.upper_right)), work / "mgxs" / "mgxs.h5", work / "windows",
        int(request["sampling"]["threads"]))
    geometry.unlink()
    return record


def phase_volumes(request: dict, work: Path) -> dict:
    """Measure the volume of every ring piece of every response solid. Writes WORK/peak/volumes.npz."""
    import numpy as np
    import openmc

    out_dir = work / "peak"
    out_dir.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    # The checked h5m tags each solid with its own id: one stand-in material per solid tells the solids apart.
    numbers = {s["solid_id"]: n for n, s in enumerate(request["solids"], start=1)}
    stand_ins = []
    for solid_id, number in numbers.items():
        material = openmc.Material(material_id=number, name=solid_id)
        material.add_nuclide("H1", 1.0, percent_type="ao")
        material.set_density("kg/m3", FILLER_DENSITY_KG_M3)
        stand_ins.append(material)
    universe = openmc.DAGMCUniverse(str(request["h5m"]))
    box = universe.bounding_box
    reach = max(math.hypot(x, y) for x in (box.lower_left[0], box.upper_right[0])
                for y in (box.lower_left[1], box.upper_right[1]))
    dr, dz = request["response"]["peak_voxel_cm"]
    r_edges = ring_edges(0.0, math.ceil(reach), dr)
    z_edges = ring_edges(math.floor(box.lower_left[2]), math.ceil(box.upper_right[2]), dz)
    # The rays start on the box around the mesh, so the model must reach past it. Ids clear of the h5m's.
    half, low, high = r_edges[-1] + 20.0, z_edges[0] - 20.0, z_edges[-1] + 20.0
    sides = [openmc.XPlane(-half, surface_id=OUTER_ID + 1), openmc.XPlane(half, surface_id=OUTER_ID + 2),
             openmc.YPlane(-half, surface_id=OUTER_ID + 3), openmc.YPlane(half, surface_id=OUTER_ID + 4),
             openmc.ZPlane(low, surface_id=OUTER_ID + 5), openmc.ZPlane(high, surface_id=OUTER_ID + 6)]
    for side in sides:
        side.boundary_type = "vacuum"
    region = +sides[0] & -sides[1] & +sides[2] & -sides[3] & +sides[4] & -sides[5]
    model = openmc.Model(geometry=openmc.Geometry([openmc.Cell(cell_id=OUTER_ID, fill=universe, region=region)]),
                         materials=openmc.Materials(stand_ins))
    model.materials.cross_sections = str(request["cross_sections"])
    model.settings.run_mode = "fixed source"
    model.settings.particles, model.settings.batches = 10, 1
    model.settings.source = openmc.IndependentSource()
    mesh = openmc.CylindricalMesh(r_grid=r_edges, z_grid=z_edges, phi_grid=[0.0, 2.0 * math.pi])
    here = os.getcwd()
    os.chdir(out_dir)
    try:
        measured = mesh.material_volumes(model, n_samples=PEAK_RAYS, max_materials=min(len(numbers), 64),
                                         output=False, args=["--threads", str(int(request["sampling"]["threads"]))])
    finally:
        os.chdir(here)
    solids = request["response"]["solid_ids"]
    shape = (len(z_edges) - 1, len(r_edges) - 1)
    volumes = np.zeros((len(solids),) + shape)
    for row, solid_id in enumerate(solids):
        volumes[row] = np.asarray(measured[numbers[solid_id]], dtype=float).reshape(shape)
        if not volumes[row].sum() > 0.0:
            raise RuntimeError(f"no volume was found for the solid {solid_id}")
    # Keep only the rows and columns a response solid reaches.
    used_z = np.nonzero(volumes.sum(axis=(0, 2)) > 0.0)[0]
    used_r = np.nonzero(volumes.sum(axis=(0, 1)) > 0.0)[0]
    z0, z1, r0, r1 = used_z[0], used_z[-1] + 1, used_r[0], used_r[-1] + 1
    target = out_dir / "volumes.npz"
    np.savez_compressed(target, volumes_cm3=volumes[:, z0:z1, r0:r1], r_edges_cm=np.array(r_edges[r0:r1 + 1]),
                        z_edges_cm=np.array(z_edges[z0:z1 + 1]))
    discard(out_dir, {"volumes.npz"})
    record = {
        "openmc": openmc.__version__, "rays": PEAK_RAYS, "voxel_cm": [dr, dz], "solid_ids": solids,
        "solid_volume_cm3": [float(v.sum()) for v in volumes],
        "volumes_sha256": sha256_file(target), "seconds": time.monotonic() - started,
    }
    (out_dir / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    return record




CAPPED_WINDOWS = "windows-capped.h5"   # in WORK/aim: the run's windows after aiming lowered them along the line of sight


def reuse_aim(model_xml: Path, work: Path, supported: bool) -> dict | None:
    """A plan kept from an earlier run on the same geometry, materials, response and windows (WORK/aim-kept), if
    it was made for the same kind of OpenMC build. An aimed plan's capped windows file is put beside this run's."""
    kept = work / "aim-kept"
    if not (kept / "plan.json").is_file():
        return None
    plan = json.loads((kept / "plan.json").read_text())
    if plan.get("library_supported") != supported:
        return None
    if plan["mode"] == "aimed":
        if not (kept / CAPPED_WINDOWS).is_file():
            return None
        aimed = work / "aim" / "model-aimed.xml"
        aimed.parent.mkdir(parents=True, exist_ok=True)
        # The aimed model differs from this run's only in the windows file it is given when it runs.
        aimed.write_text(model_xml.read_text())
        shutil.copyfile(kept / CAPPED_WINDOWS, work / "aim" / CAPPED_WINDOWS)
        plan["model"] = str(aimed)
        plan["windows"] = str(work / "aim" / CAPPED_WINDOWS)
    plan["reused"] = True
    return plan


def plan_aim(request: dict, model_xml: Path, work: Path, material_numbers: dict) -> dict:
    """Ask streaming_aim.py whether this run can aim; always returns a plan with its reason and next step.
    The plan and an aimed plan's capped windows file (WORK/aim/windows-capped.h5) are left in WORK/aim for the
    engine to keep."""
    import openmc.lib

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import streaming_aim

    library = Path(openmc.lib._dll._name)
    supported = streaming_aim.aiming_supported(library)
    response = response_materials(request)
    if float(request["response"]["energy_min_ev"]) != AIM_THRESHOLD_EV:
        return streaming_aim.windows_only(
            "Aiming is available for a response above 0.1 MeV only, and this response has another threshold.",
            "Nothing to do: the weight windows apply as they are.")
    if len(response) != 1:
        return streaming_aim.windows_only(
            "The response solids are made of more than one material, and aiming handles one.",
            "Nothing to do: the weight windows apply as they are.")
    kept = reuse_aim(model_xml, work, supported)
    if kept is not None:
        return kept
    try:
        plan = streaming_aim.plan(model_xml, work / "aim", library, material_numbers[response[0]], keep_runs=False)
    except Exception as error:  # the search is an accelerator: its failure must not stop the run
        return streaming_aim.windows_only(
            f"The search for a streaming channel failed ({type(error).__name__}: {error}).",
            "Results are unaffected; the weight windows apply as they are.")
    plan["library_supported"] = supported
    plan["reused"] = False
    (work / "aim" / "plan.json").write_text(json.dumps(plan, indent=1))
    return plan


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
    if float(response["energy_min_ev"]) > 0.0:
        # A neutron below the threshold cannot score and is not followed.
        settings.cutoff = {"energy_neutron": float(response["energy_min_ev"])}
    settings.output = {"tallies": False, "summary": False}
    settings.sourcepoint = {"write": False}
    settings.statepoint = {"batches": []}
    # Lost neutrons are counted and judged here, with a result that says so, rather than by an abort.
    settings.max_lost_particles = 2_000_000_000
    settings.rel_max_lost_particles = 0.5
    settings.max_write_lost_particles = LOST_LOCATIONS

    solids = response["solid_ids"]
    ids = volume_ids(request)
    tally = openmc.Tally(tally_id=TALLY_ID, name="response-fast-flux")
    tally.filters = [
        openmc.CellFilter([ids[s] for s in solids]),
        openmc.EnergyFilter([float(response["energy_min_ev"]), ENERGY_MAX_EV]),
    ]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    tallies = [tally]
    pieces = None
    if response.get("peak_voxel_cm") is not None:
        pieces_record = json.loads((work / "peak" / "record.json").read_text())
        if (sha256_file(work / "peak" / "volumes.npz") != pieces_record["volumes_sha256"]
                or pieces_record["solid_ids"] != solids or pieces_record["voxel_cm"] != response["peak_voxel_cm"]):
            raise RuntimeError("the piece volumes do not match their record or this request")
        pieces = np.load(work / "peak" / "volumes.npz")
        rings = openmc.CylindricalMesh(mesh_id=PEAK_MESH_ID, r_grid=pieces["r_edges_cm"], z_grid=pieces["z_edges_cm"],
                                       phi_grid=[0.0, 2.0 * math.pi])
        peak_tally = openmc.Tally(tally_id=PEAK_TALLY_ID, name="response-peak")
        peak_tally.filters = [openmc.MeshFilter(rings)] + list(tally.filters)
        peak_tally.scores = ["flux"]
        peak_tally.estimator = "tracklength"
        tallies.append(peak_tally)
    model.tallies = openmc.Tallies(tallies)

    mode = {"mode": "analog", "reason": "No weight windows were asked for.",
            "next_step": "Run with weight windows to reach the target sooner.", "env": {}}
    windows_record = None
    aim_seconds = 0.0
    model_xml = out_dir / "model.xml"
    if request["variance_reduction"] == "windows":
        os.environ["OPENMC_WW_ROULETTE_FACTOR"] = f"{ROULETTE_FACTOR:g}"
        filled = work / "windows" / "filled.h5"
        windows_record = json.loads((work / "windows" / "record.json").read_text())
        if sha256_file(filled) != windows_record["filled_sha256"]:
            raise RuntimeError("the weight windows do not match their record")
        window_mesh = apply_windows(openmc, model, filled, out_dir)
        export_model(model, model_xml, window_mesh)
        numbers = {name: material.id for name, material in materials.items()}
        aim_started = time.monotonic()
        mode = plan_aim(request, model_xml, work, numbers)
        aim_seconds = time.monotonic() - aim_started
        if mode["mode"] == "aimed":
            model_xml.unlink()
            model_xml = Path(mode["model"])
            # The aimed model is this run's model; point it at the capped windows.
            name = window_file_name(Path(mode["windows"]), out_dir)
            text = re.sub(r"<weight_windows_file>.*?</weight_windows_file>",
                          f"<weight_windows_file>{name}</weight_windows_file>", model_xml.read_text(), count=1,
                          flags=re.S)
            model_xml.write_text(text)
            os.environ.update(mode["env"])
    else:
        model.export_to_model_xml(model_xml)

    here = os.getcwd()
    os.chdir(out_dir)
    stats = RunningSum()
    per_solid = [RunningSum() for _ in solids]
    peaks = [None for _ in solids]
    lost = None
    leading = None
    piece_volumes = piece_ranked = None
    if pieces is not None:
        piece_volumes = pieces["volumes_cm3"].reshape(len(solids), -1)
        piece_ranked = ranked_pieces(pieces["volumes_cm3"], pieces["r_edges_cm"]).reshape(len(solids), -1)
    started = time.monotonic()
    reason = None
    try:
        openmc.lib.init(["--threads", str(int(sampling["threads"])), str(model_xml)])
        try:
            openmc.lib.simulation_init()
            handle = openmc.lib.tallies[TALLY_ID]
            previous = np.zeros(len(solids))
            previous_pieces = None
            last_report = started
            # Every batch's value per solid, written as the run goes (a cancelled run keeps it): the evidence for
            # judging the run's statistics, and where a batch far above the others came from.
            batch_log = (out_dir / "batches.csv").open("w")
            batch_log.write("batch,total," + ",".join(str(s) for s in solids) + "\n")
            while reason is None:
                openmc.lib.next_batch()
                # Column 1 holds the sum of the batch values so far; its increase is this batch's value.
                sums = np.array(handle.results[:, 0, 1], dtype=float)
                batch = sums - previous
                previous = sums
                heavy = stats.n > HEAVY_BATCH_AFTER and float(batch.sum()) > HEAVY_BATCH_FACTOR * float(
                    np.median(stats.values)) > 0.0
                stats.add(float(batch.sum()))
                for accumulator, value in zip(per_solid, batch):
                    accumulator.add(float(value))
                batch_log.write(f"{stats.n},{float(batch.sum())!r}," + ",".join(repr(float(v)) for v in batch) + "\n")
                batch_log.flush()
                elapsed = time.monotonic() - started
                if pieces is not None:
                    # Mesh bins run over r, then z; the solids follow as the next filter.
                    scored = np.array(openmc.lib.tallies[PEAK_TALLY_ID].results[:, 0, 1:3], dtype=float)
                    scored = scored.reshape(-1, len(solids), 2)
                    if heavy and previous_pieces is not None:
                        # This batch's score in every piece, to find where the heavy history landed.
                        np.savez_compressed(out_dir / f"heavy-batch-{stats.n}.npz",
                                            pieces=scored[:, :, 0] - previous_pieces, solids=np.array(solids))
                    previous_pieces = scored[:, :, 0].copy()
                    leading = (leading_piece(scored, piece_volumes, piece_ranked, stats.n),)
                reason = stop_reason(stats, sampling, elapsed, leading)
                lost = lost_count(openmc.lib._dll)
                if lost_too_many(lost, stats.n * int(sampling["particles_per_batch"])):
                    reason = "lost_particles"
                if reason is not None or time.monotonic() - last_report >= PROGRESS_SECONDS:
                    last_report = time.monotonic()
                    print(json.dumps({
                        "progress": {
                            "batches": stats.n,
                            "histories": stats.n * int(sampling["particles_per_batch"]),
                            "relative_error": stats.relative_error,
                            "peak_relative_error": leading[0] if leading else None,
                            "elapsed_seconds": elapsed,
                        }}), flush=True)
            batch_log.close()
            if pieces is not None:
                # Every piece's sums, kept beside the result: the evidence behind the peak.
                np.savez_compressed(out_dir / "pieces.npz", sums=scored[:, :, 0], squares=scored[:, :, 1],
                                    batches=stats.n)
                volumes = pieces["volumes_cm3"]
                r_edges, z_edges = pieces["r_edges_cm"], pieces["z_edges_cm"]
                for row in range(len(solids)):
                    peak = peak_of(scored[:, row, 0], scored[:, row, 1], piece_volumes[row], piece_ranked[row],
                                   stats.n)
                    if peak is not None:
                        iz, ir = divmod(peak.pop("index"), volumes.shape[2])
                        peak["r_cm"] = [float(r_edges[ir]), float(r_edges[ir + 1])]
                        peak["z_cm"] = [float(z_edges[iz]), float(z_edges[iz + 1])]
                        peak["pieces_ranked"] = int(piece_ranked[row].sum())
                    peaks[row] = peak
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
        "variance_of_variance": stats.variance_of_variance,
        "estimator": "tracklength",
        "unit": "cm_per_source",
        "energy_min_ev": response["energy_min_ev"],
        "total": {"mean": stats.mean, "standard_error": stats.standard_error},
        "solids": [{"solid_id": s, "mean": a.mean, "standard_error": a.standard_error, "peak": peak}
                   for s, a, peak in zip(solids, per_solid, peaks)],
        "peak_voxel_cm": response.get("peak_voxel_cm"),
        "lost_particles": lost,
        "lost_positions_cm": lost_positions(out_dir) if lost else [],
        "variance_reduction": {
            "mode": mode["mode"], "reason": mode["reason"], "next_step": mode["next_step"],
            "targets": [t["name"] for t in mode.get("targets", [])],
            "windows_sha256": windows_record["filled_sha256"] if windows_record else None,
            "aim_seconds": aim_seconds,
            "aim_reused": bool(mode.get("reused", False)),
        },
        "why": None,
        "next_step": None,
    }
    if reason == "lost_particles":
        result["why"] = (f"{lost} of {result['histories']} neutrons could not be placed in the geometry, above the "
                         f"limit of one in a million; the answer is not trusted.")
        result["next_step"] = ("Look at the model where the neutrons were lost (positions are in the report) and "
                               "run `faris design check` again: a gap or overlap there is the usual cause.")
    elif reason != "target_reached":
        result["why"], result["next_step"] = explain(reason, stats, sampling, mode["mode"] != "analog", leading)
    target = out_dir / "result.json"
    with target.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=1)
        stream.write("\n")
    return result


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--phase", choices=("run", "mgxs", "windows", "volumes"), required=True)
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
    {"run": phase_run, "mgxs": phase_mgxs, "windows": phase_windows, "volumes": phase_volumes}[args.phase](request, work)
    return 0


if __name__ == "__main__":
    sys.exit(main())
