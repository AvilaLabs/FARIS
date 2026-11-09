#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Transport worker of `faris design transport`: a checked design's DAGMC geometry, run to a target error.

    design_transport.py --phase run     --request request.json --work-dir DIR
    design_transport.py --phase mgxs    --request request.json --work-dir DIR
    design_transport.py --phase windows --request request.json --work-dir DIR
    design_transport.py --phase volumes --request request.json --work-dir DIR
    design_transport.py --phase heating_preflight --request request.json --work-dir DIR
    design_transport.py --phase heating --request request.json --work-dir DIR

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

`heating_preflight` checks coupled-heating data before geometry sampling. `heating` is a separate all-energy run:
it enables photon transport, atomic relaxation and the LED electron treatment and does not set a neutron energy
cutoff. Its total heating tally has no particle filter; neutron, photon, electron and positron tallies run alongside
it. Per-batch residuals retain their correlations for the integrated response and each sampled voxel.
It stops early only after 30 independent batches when the aggregate all-solid mean is within its authored relative
standard-error target (5 % by default), each authored solid's resolved peak is within twice that target (10 %),
and the aggregate batch variance-of-variance is at most 0.1. An unknown or unstable variance-of-variance never counts
as convergence; at a history or time bound the result remains inconclusive. Geometry volume error stays separate.

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
import stat
import sys
import time
from pathlib import Path
from xml.etree import ElementTree as ET

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
MAX_RECEIPT_FILE_BYTES = 12 * 1024**3
MAX_HEATING_VOXEL_CELLS = 32_768
MAX_HEATING_VOLUME_CELLS = 1_000_000
MAX_HEATING_HISTORIES = 2_000_000_000
MAX_HEATING_BATCHES = 100_000
HEATING_VOV_LIMIT = 0.1
HEATING_PEAK_TARGET_FACTOR = 2.0
MAX_HEATING_SECONDS = 86_400
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


def heating_mesh_cell_count(bounds, voxel_cm: list[float], solid_count: int) -> int:
    """Conservative mesh-bin × solid count before ray sampling or tally allocation."""
    dr, dz = voxel_cm
    coordinates = (*bounds.lower_left, *bounds.upper_right)
    if (len(voxel_cm) != 2 or any(not math.isfinite(float(value)) for value in coordinates)
            or any(not math.isfinite(float(step)) or step <= 0.0 for step in (dr, dz))
            or solid_count <= 0):
        raise ValueError("the heating peak mesh needs finite geometry bounds, positive voxel widths and solids")
    reach = max(math.hypot(x, y) for x in (bounds.lower_left[0], bounds.upper_right[0])
                for y in (bounds.lower_left[1], bounds.upper_right[1]))
    if not math.isfinite(reach):
        raise ValueError("the heating peak mesh has non-finite radial extent")
    r_count = max(1, math.ceil(math.ceil(reach) / dr - 1.0e-9))
    z_span = math.ceil(bounds.upper_right[2]) - math.floor(bounds.lower_left[2])
    z_count = max(1, math.ceil(z_span / dz - 1.0e-9))
    return r_count * z_count * solid_count


def heating_mesh_not_evaluated(cell_count: int, limit: int = MAX_HEATING_VOLUME_CELLS) -> str | None:
    if cell_count > limit:
        return (f"The requested peak mesh requires {cell_count:,} voxel-solid cells, above the bounded "
                f"limit of {limit:,}.")
    return None


def heating_preflight_next_step(reason: str) -> str:
    if reason.startswith("The requested peak mesh requires"):
        return "Increase the radial or vertical peak voxel size, or narrow the explicitly authored winding-pack solids, then rerun heating."
    if "planned source histories" in reason:
        return f"Reduce the heating history limit so the planned run stays at or below {MAX_HEATING_HISTORIES:,} source histories."
    if "batches" in reason and "bounded" in reason:
        return f"Increase histories per batch or reduce the heating batch limit to at most {MAX_HEATING_BATCHES:,}."
    if "time limits" in reason:
        return f"Set the heating time limit between 1 and {MAX_HEATING_SECONDS:,} seconds."
    return "Select a supported OpenMC runtime and library with MT=301 heating, photon data, and complete atomic-relaxation shell maps for every used element."


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


def peak_of_moments(means, standard_errors, volumes, ranked) -> dict | None:
    """Select the leading voxel from online per-batch means and standard errors."""
    best, best_key = None, None
    for index, (mean, error, volume, keep) in enumerate(zip(means, standard_errors, volumes, ranked)):
        if not keep:
            continue
        mean, error, volume = float(mean), float(error), float(volume)
        bound = (mean - PEAK_SIGMAS * error) / volume
        key = (bound > 0.0, bound if bound > 0.0 else mean / volume)
        if best is None or key > best_key:
            best, best_key = {"index": index, "mean": mean, "standard_error": error,
                              "volume_cm3": volume}, key
    return best


def heating_peak_relative_error(means_by_bin_solid, errors_by_bin_solid, volumes_by_solid, ranked_by_solid):
    """Maximum relative error of the resolved leading voxel in every authored solid (vectorized)."""
    import numpy as np

    means = np.asarray(means_by_bin_solid, dtype=float).T
    errors = np.asarray(errors_by_bin_solid, dtype=float).T
    volumes = np.asarray(volumes_by_solid, dtype=float)
    ranked = np.asarray(ranked_by_solid, dtype=bool)
    if means.shape != errors.shape or means.shape != volumes.shape or means.shape != ranked.shape:
        return None
    bound = (means - PEAK_SIGMAS * errors) / np.where(ranked, volumes, 1.0)
    resolved = ranked & (bound > 0.0)
    has_resolved = resolved.any(axis=1)
    mean_density = means / np.where(ranked, volumes, 1.0)
    keys = np.where(has_resolved[:, None], np.where(resolved, bound, -np.inf), mean_density)
    keys = np.where(ranked, keys, -np.inf)
    leaders = keys.argmax(axis=1)
    rows = np.arange(means.shape[0])
    peak_mean = means[rows, leaders]
    peak_error = errors[rows, leaders]
    if (not ranked.any(axis=1).all() or not np.isfinite(peak_mean).all()
            or (peak_mean <= 0.0).any() or not np.isfinite(peak_error).all() or (peak_error < 0.0).any()):
        return None
    return float(np.max(peak_error / peak_mean))


def lost_too_many(lost: int | None, histories: int) -> bool:
    """True when the lost share of the histories is above the limit. One loss in under a million histories is
    above the share but does not interrupt an in-progress run until it is long enough to tell."""
    return histories >= 1.0 / LOST_FRACTION_LIMIT and lost_fraction_exceeded(lost, histories)


def lost_fraction_exceeded(lost: int | None, histories: int) -> bool:
    """Whether a completed run's recorded lost share strictly exceeds the limit."""
    return bool(lost) and histories > 0 and lost > LOST_FRACTION_LIMIT * histories


def final_run_status(
    reason: str, lost: int | None, histories: int
) -> tuple[str, str | None, str | None]:
    """Apply the final loss guard and return status plus its fail-closed guidance, if needed."""
    if reason == "lost_particles" or lost_fraction_exceeded(lost, histories):
        why, next_step = lost_particle_guidance(lost or 0, histories)
        return "lost_particles", why, next_step
    return reason, None, None


def lost_particle_guidance(lost: int, histories: int) -> tuple[str, str]:
    """Explain a failed run without treating OpenMC's recorded birth points as loss locations."""
    why = (f"{lost} of {histories} neutrons could not be placed in the geometry, above the "
           "limit of one in a million; the answer is not trusted.")
    next_step = ("Review the design for gaps or overlaps and run `faris design check` again. "
                 "The reported particle positions are birth points, not where particles left the geometry.")
    return why, next_step


def lost_count(dll) -> int | None:
    """OpenMC's count of lost particles, or None when this build does not expose it."""
    import ctypes

    try:
        return int(ctypes.c_int.in_dll(dll, "n_lost_particles").value)
    except (ValueError, AttributeError):
        return None


def lost_positions(directory: Path) -> list[list[float]]:
    """Recorded birth positions of the first lost neutrons, from OpenMC particle files (cm)."""
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


def checked_heating_h5m_snapshot(request: dict, work: Path, purpose: str) -> tuple[Path, str]:
    """Copy the exact preflight geometry to a stable input and verify both copies by hash."""
    source = Path(request["h5m"])
    preflight_path = work / "solver" / "heating-preflight.json"
    preflight = json.loads(preflight_path.read_text())
    expected = preflight.get("h5m_sha256")
    if preflight.get("status") != "READY" or not isinstance(expected, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", expected):
        raise ValueError("the coupled-heating preflight has no valid checked H5M identity")
    if not sha256_file(source).lower() == expected.lower():
        raise ValueError("the checked H5M changed after coupled-heating preflight")
    snapshot = work / "geometry" / f"model-{purpose}-source.h5m"
    snapshot.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, snapshot)
    if not sha256_file(snapshot).lower() == expected.lower():
        raise ValueError("the checked H5M changed while its stable worker snapshot was copied")
    if not sha256_file(source).lower() == expected.lower():
        raise ValueError("the checked H5M changed while its stable worker snapshot was copied")
    return snapshot, expected


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
    selected = request["response"]["solid_ids"]
    selected_set = set(selected)
    if request.get("heating_mode"):
        ordered = [s for s in request["solids"] if s["solid_id"] in selected_set]
        ordered.extend(s for s in request["solids"] if s["solid_id"] not in selected_set)
    else:
        ordered = request["solids"]
    numbers = {s["solid_id"]: n for n, s in enumerate(ordered, start=1)}
    stand_ins = []
    for solid_id, number in numbers.items():
        material = openmc.Material(material_id=number, name=solid_id)
        material.add_nuclide("H1", 1.0, percent_type="ao")
        material.set_density("kg/m3", FILLER_DENSITY_KG_M3)
        stand_ins.append(material)
    volume_geometry = Path(request["h5m"])
    volume_h5m_sha256 = None
    if request.get("heating_mode"):
        volume_geometry, volume_h5m_sha256 = checked_heating_h5m_snapshot(request, work, "volume")
    universe = openmc.DAGMCUniverse(str(volume_geometry))
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
    volume_material_limit = min(len(selected), 64) if request.get("heating_mode") else min(len(numbers), 64)
    here = os.getcwd()
    os.chdir(out_dir)
    try:
        measured = mesh.material_volumes(model, n_samples=PEAK_RAYS, max_materials=volume_material_limit,
                                         output=False, args=["--threads", str(int(request["sampling"]["threads"]))])
    finally:
        os.chdir(here)
    if volume_h5m_sha256 is not None and not sha256_file(volume_geometry).lower() == volume_h5m_sha256.lower():
        raise RuntimeError("the stable H5M snapshot changed during winding-pack volume sampling")
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
    if volume_h5m_sha256 is not None:
        record["h5m_sha256"] = volume_h5m_sha256
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
            # The in-progress stop rule waits for enough histories to judge the fraction. A run that
            # stops early for convergence or a budget still gets the strict final-fraction check.
            lost = lost_count(openmc.lib._dll)
            reason, lost_why, lost_next_step = final_run_status(
                reason, lost, stats.n * int(sampling["particles_per_batch"])
            )
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
        "why": lost_why,
        "next_step": lost_next_step,
    }
    if reason not in ("lost_particles", "target_reached"):
        result["why"], result["next_step"] = explain(reason, stats, sampling, mode["mode"] != "analog", leading)
    target = out_dir / "result.json"
    with target.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=1)
        stream.write("\n")
    return result


def heating_batch_closure(total_batches: list[float], component_batches: list[list[float]]) -> tuple[float, float]:
    """Mean and standard error of total minus particle components, preserving batch covariance."""
    if len(total_batches) < 2 or any(len(values) != len(total_batches) for values in component_batches):
        raise ValueError("heating closure needs at least two aligned batches")
    residuals = [total - math.fsum(values[i] for values in component_batches)
                 for i, total in enumerate(total_batches)]
    mean = math.fsum(residuals) / len(residuals)
    variance = math.fsum((value - mean) ** 2 for value in residuals) / (len(residuals) - 1)
    return mean, math.sqrt(variance / len(residuals))


def heating_closure_quality(mean: float, standard_error: float, component_scale: float) -> dict:
    """Three-SE covariance-aware closure gate with an explicit floating-point floor."""
    # The residual is formed from the same batch samples, so its SE already includes
    # component covariance. Permit only roundoff when that uncertainty is exactly zero.
    tolerance = 64.0 * sys.float_info.epsilon * max(abs(component_scale), abs(mean), 1.0e-300)
    within = abs(mean) <= 3.0 * standard_error + tolerance
    return {"within_3se": within, "roundoff_tolerance_ev_per_source": tolerance}


def heating_peak_quality(mean: float, standard_error: float | None) -> tuple[bool, str | None]:
    """Require a positive peak with a reported relative Monte Carlo error no greater than 10%."""
    if not math.isfinite(mean) or mean <= 0.0:
        return False, "The sampled peak response is zero; relative heating uncertainty is unavailable."
    if standard_error is None or not math.isfinite(standard_error) or standard_error < 0.0:
        return False, "The sampled peak has no usable Monte Carlo uncertainty estimate."
    relative_error = standard_error / mean
    if relative_error > 0.10:
        return False, f"The sampled peak relative Monte Carlo standard error is {100.0 * relative_error:.2f}%, above 10%; heating is unconverged."
    return True, None


def heating_terminal_quality(status: int | None, batches: int, planned: int, stop_reason: str,
                             lost: int | None, histories: int) -> tuple[str, str | None]:
    """Validate C API completion and final lost-particle count before accepting scores."""
    if status not in (0, 1, 2):
        return "NOT_EVALUATED", f"OpenMC returned unknown terminal status {status!r}."
    if status == 2:
        return "NOT_EVALUATED", "OpenMC terminated with C API status 2; the heating tally is incomplete."
    if status == 1 and batches != planned:
        return "NOT_EVALUATED", f"OpenMC reported planned-batch completion after only {batches} of {planned} batches."
    if status == 0 and (stop_reason not in ("time_limit", "target_reached") or batches < 30):
        return "NOT_EVALUATED", "OpenMC stopped before a valid target or time-budget endpoint or before 30 batches."
    if lost is None:
        return "NOT_EVALUATED", "OpenMC did not expose the final lost-particle count."
    if lost < 0:
        return "NOT_EVALUATED", "OpenMC exposed an invalid negative lost-particle count."
    if histories <= 0 or lost / histories > LOST_FRACTION_LIMIT:
        return "INCONCLUSIVE", f"{lost} of {histories} source histories were lost, exceeding the strict one-in-a-million limit."
    return "CALCULATED", None


class VectorMoments:
    """Online population of aligned per-batch tally values with fixed-size Welford state."""

    def __init__(self, shape):
        import numpy as np

        self.count = 0
        self.mean = np.zeros(shape, dtype=float)
        self.m2 = np.zeros(shape, dtype=float)

    def add(self, values) -> None:
        import numpy as np

        values = np.asarray(values, dtype=float)
        self.count += 1
        delta = values - self.mean
        self.mean += delta / self.count
        self.m2 += delta * (values - self.mean)

    @property
    def standard_error(self):
        import numpy as np

        if self.count < 2:
            return np.full_like(self.mean, np.nan)
        return np.sqrt(np.maximum(self.m2, 0.0) / (self.count * (self.count - 1)))


class ScalarMoments:
    """Fixed-state scalar batch moments, including MCNP batch VOV."""

    def __init__(self):
        self.count = 0
        self.mean = self.m2 = self.m3 = self.m4 = 0.0

    def add(self, value: float) -> None:
        n = self.count + 1
        delta = value - self.mean
        delta_n = delta / n
        delta_n2 = delta_n * delta_n
        term = delta * delta_n * self.count
        old_m2, old_m3 = self.m2, self.m3
        self.m4 += (term * delta_n2 * (n * n - 3 * n + 3)
                    + 6 * delta_n2 * old_m2 - 4 * delta_n * old_m3)
        self.m3 += term * delta_n * (n - 2) - 3 * delta_n * old_m2
        self.m2 += term
        self.mean += delta_n
        self.count = n

    @property
    def standard_error(self) -> float | None:
        if self.count < 2 or not math.isfinite(self.m2) or self.m2 < 0.0:
            return None
        return math.sqrt(self.m2 / (self.count * (self.count - 1)))

    @property
    def relative_error(self) -> float | None:
        se = self.standard_error
        if se is None or not math.isfinite(self.mean) or self.mean <= 0.0:
            return None
        return se / self.mean

    @property
    def variance_of_variance(self) -> float | None:
        if self.count < 2 or not math.isfinite(self.m2) or self.m2 <= 0.0 or not math.isfinite(self.m4):
            return None
        value = self.m4 / (self.m2 * self.m2) - 1.0 / self.count
        if not math.isfinite(value) or value < -128.0 * sys.float_info.epsilon:
            return None
        return max(0.0, value)

def collect_heating_batches(lib, tallies: dict, voxel_tallies: dict, solid_count: int, bins: int,
                            max_batches: int, max_seconds: float, started: float,
                            target_relative_error: float | None = None, volumes_by_solid=None, ranked=None,
                            particles_per_batch: int | None = None):
    """Run C API batches using fixed-size online moments and exact same-batch closure residuals."""
    import numpy as np

    previous = {name: np.zeros(solid_count) for name in tallies}
    moments = {name: VectorMoments((solid_count,)) for name in tallies}
    voxel_previous = {name: np.zeros(bins * solid_count) for name in voxel_tallies}
    voxel_moments = {name: VectorMoments((bins, solid_count)) for name in voxel_tallies}
    component_names = tuple(name for name in tallies if name != "total")
    closure_moments = VectorMoments((solid_count,))
    voxel_closure_moments = VectorMoments((bins, solid_count))
    stop_reason, terminal_status, batches_done = "history_limit", None, 0
    aggregate = ScalarMoments()
    peak_relative_error = None
    for _ in range(max_batches):
        terminal_status = lib.next_batch()
        batches_done += 1
        batch_values = {}
        voxel_batch_values = {}
        for name in tallies:
            cumulative = np.asarray(lib.tallies[tallies[name].id].results[:, 0, 1], dtype=float)
            batch = cumulative - previous[name]
            previous[name] = cumulative.copy()
            batch_values[name] = batch
            moments[name].add(batch)
            voxel_cumulative = np.asarray(
                lib.tallies[voxel_tallies[name].id].results[:, 0, 1], dtype=float)
            voxel_batch = (voxel_cumulative - voxel_previous[name]).reshape(bins, solid_count)
            voxel_previous[name] = voxel_cumulative.copy()
            voxel_batch_values[name] = voxel_batch
            voxel_moments[name].add(voxel_batch)
        component_sum = np.zeros(solid_count)
        voxel_component_sum = np.zeros((bins, solid_count))
        for name in component_names:
            component_sum += batch_values[name]
            voxel_component_sum += voxel_batch_values[name]
        closure_moments.add(batch_values["total"] - component_sum)
        voxel_closure_moments.add(voxel_batch_values["total"] - voxel_component_sum)
        aggregate.add(math.fsum(float(value) for value in batch_values["total"]))
        peak_relative_error = None
        if volumes_by_solid is not None and ranked is not None:
            peak_relative_error = heating_peak_relative_error(
                voxel_moments["total"].mean,
                voxel_moments["total"].standard_error,
                volumes_by_solid,
                ranked,
            )
        vov = aggregate.variance_of_variance
        target_met = (target_relative_error is not None and batches_done >= 30
                      and aggregate.relative_error is not None
                and aggregate.relative_error <= target_relative_error
                and peak_relative_error is not None
                and peak_relative_error <= HEATING_PEAK_TARGET_FACTOR * target_relative_error
                and vov is not None and vov <= HEATING_VOV_LIMIT)
        timed_out = time.monotonic() - started >= max_seconds and batches_done < max_batches
        if target_met:
            stop_reason = "target_reached"
        elif timed_out:
            stop_reason = "time_limit"
        if particles_per_batch is not None and (
                batches_done % 10 == 0 or target_met or timed_out or terminal_status not in (0, None)):
            print(json.dumps({"progress": {
                "batches": batches_done,
                "histories": batches_done * particles_per_batch,
                "relative_error": aggregate.relative_error,
                "peak_relative_error": peak_relative_error,
                "variance_of_variance": vov,
                "elapsed_seconds": time.monotonic() - started,
            }}), flush=True)
        if target_met:
            break
        if timed_out:
            break
        if terminal_status not in (0, None):
            break
    return (moments, voxel_moments, closure_moments, voxel_closure_moments,
            aggregate, peak_relative_error, batches_done, stop_reason, terminal_status)


def normalize_heating_source(source) -> None:
    """Make the OpenMC source distribution sum to exactly one source neutron."""
    source_total = float(source.strength)
    if not math.isfinite(source_total) or source_total <= 0.0:
        raise ValueError("the coupled-heating source strength must be finite and positive")
    for element in source.sources.ravel():
        element.strength = float(element.strength) / source_total
    source.strength = 1.0


def heating_tally_filters(openmc, component: str, cell_filter, mesh=None):
    """Keep the total unfiltered and attach one particle filter to each component tally."""
    filters = [openmc.MeshFilter(mesh), cell_filter] if mesh is not None else [cell_filter]
    if component != "total":
        filters.append(openmc.ParticleFilter([component]))
    return filters


def validate_heating_request(request: dict) -> None:
    """Validate the bounded all-energy request independently from the neutron flux request."""
    if request.get("schema") != REQUEST_SCHEMA:
        raise ValueError(f"the request is not {REQUEST_SCHEMA}")
    sampling = request["sampling"]
    if (not math.isfinite(sampling.get("target_relative_error", float("nan")))
            or not 0.0 < sampling["target_relative_error"] < 1.0):
        raise ValueError("heating target_relative_error must be finite and between 0 and 1")
    if not (2 <= sampling["min_batches"] <= sampling["max_batches"]):
        raise ValueError("heating needs 2 <= min_batches <= max_batches")
    if sampling["max_batches"] > MAX_HEATING_BATCHES:
        raise ValueError(f"heating is bounded to {MAX_HEATING_BATCHES:,} batches; increase histories per batch")
    if sampling["particles_per_batch"] * sampling["max_batches"] > MAX_HEATING_HISTORIES:
        raise ValueError(f"heating is bounded to {MAX_HEATING_HISTORIES:,} planned source histories")
    if (sampling["particles_per_batch"] < 1 or not math.isfinite(sampling["max_seconds"])
            or sampling["max_seconds"] <= 0 or sampling["max_seconds"] > MAX_HEATING_SECONDS):
        raise ValueError(f"heating time limits must be positive and no greater than {MAX_HEATING_SECONDS:,} seconds")
    if not (1 <= sampling["threads"] <= 32):
        raise ValueError("heating threads must lie between 1 and 32")
    response = request["response"]
    if response.get("energy_min_ev") != 0.0:
        raise ValueError("coupled heating must include all neutron energies")
    voxel = response.get("peak_voxel_cm")
    if (not isinstance(voxel, list) or len(voxel) != 2
            or any(v is None or not math.isfinite(v) or v < 1.0 for v in voxel)):
        raise ValueError("heating peak voxels must have radial and vertical widths at least 1 cm")
    solids = {solid["solid_id"]: solid for solid in request["solids"]}
    if len(solids) != len(request["solids"]):
        raise ValueError("heating request repeats a solid identity")
    solid_ids = response["solid_ids"]
    if not solid_ids or len(set(solid_ids)) != len(solid_ids):
        raise ValueError("heating response must name distinct winding-pack solids")
    for solid_id in solid_ids:
        if solid_id not in solids or solids[solid_id].get("classification") != "winding_pack":
            raise ValueError(f"{solid_id}: response lacks explicit winding-pack identity")
        if solids[solid_id]["material_id"] is None:
            raise ValueError(f"{solid_id}: winding-pack response has no material")


def heating_data_identity(request: dict, openmc, h5py) -> dict:
    """Check the selected neutron and photon files before geometry sampling starts."""
    xml = Path(request["cross_sections"])
    root = ET.parse(xml).getroot()
    neutron, photon = {}, {}
    for library in root.findall("library"):
        if library.get("type") == "neutron":
            neutron.update({name: library.get("path") for name in library.get("materials", "").split()})
        elif library.get("type") == "photon":
            photon.update({name: library.get("path") for name in library.get("materials", "").split()})
    nuclides = sorted({n for material in request["materials"] for n in material["nuclide_atom_fractions"]})
    elements = sorted({re.match(r"[A-Z][a-z]?", n).group(0) for n in nuclides})
    identity = {"neutron_mt301": {}, "photon": {}, "atomic_relaxation": "enabled",
                "electron_treatment": "led",
                "electron_treatment_scope": "LED local energy deposition approximation; secondary-electron transport is not explicit."}
    for nuclide in nuclides:
        if nuclide not in neutron:
            raise ValueError(f"coupled heating needs neutron data for {nuclide}")
        path = (xml.parent / neutron[nuclide]).resolve(strict=True)
        data = openmc.data.IncidentNeutron.from_hdf5(str(path))
        if 301 not in data.reactions:
            raise ValueError(f"coupled heating needs MT=301 for {nuclide}")
        identity["neutron_mt301"][nuclide] = {"sha256": sha256_file(path), "mt301": True}
    for element in elements:
        if element not in photon:
            raise ValueError(f"coupled heating needs photon data for {element}")
        path = (xml.parent / photon[element]).resolve(strict=True)
        data = openmc.data.IncidentPhoton.from_hdf5(str(path))
        with h5py.File(path, "r") as library:
            shells = set(library[element]["subshells"].keys())
        relaxation = data.atomic_relaxation
        if (not shells or relaxation is None or shells - set(relaxation.binding_energy)
                or shells - set(relaxation.num_electrons)):
            raise ValueError(f"coupled heating needs complete atomic-relaxation shell data for {element}")
        identity["photon"][element] = {"sha256": sha256_file(path), "shell_count": len(shells)}
    return identity


def receipt_file_identity(path, label: str) -> dict:
    """Canonical identity for a bounded regular file, hashed incrementally."""
    canonical = Path(path).resolve(strict=True)
    digest = hashlib.sha256()
    bytes_read = 0
    with canonical.open("rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_RECEIPT_FILE_BYTES:
            raise ValueError(f"{label} is not a regular file within the {MAX_RECEIPT_FILE_BYTES}-byte identity bound")
        for block in iter(lambda: stream.read(1 << 20), b""):
            bytes_read += len(block)
            if bytes_read > MAX_RECEIPT_FILE_BYTES:
                raise ValueError(f"{label} exceeded the {MAX_RECEIPT_FILE_BYTES}-byte identity bound while hashing")
            digest.update(block)
    return {"path": str(canonical), "sha256": digest.hexdigest()}


def heating_runtime_identity(openmc) -> dict:
    """Record the selected Python, loaded OpenMC C API library, module version and worker bytes."""
    dll = getattr(openmc.lib, "_dll", None)
    library_path = getattr(dll, "_name", None)
    if not library_path:
        raise ValueError("the selected OpenMC Python package does not expose its loaded C API library path")
    return {
        "python": receipt_file_identity(sys.executable, "selected Python executable"),
        "openmc_version": str(openmc.__version__),
        "openmc_c_api_library": receipt_file_identity(library_path, "loaded OpenMC C API library"),
        "worker_helper": receipt_file_identity(__file__, "design transport worker helper"),
    }


def heating_execution_identity(run_h5m: Path, model_xml: Path) -> dict:
    """Identify the retagged geometry and exported model passed to the C API."""
    return {
        "run_h5m": receipt_file_identity(run_h5m, "retagged heating H5M"),
        "model_xml": receipt_file_identity(model_xml, "exported heating model XML"),
    }


def validate_coupled_openmc_support(openmc) -> None:
    """Refuse before volume work when this OpenMC build lacks the requested coupled settings."""
    parts = re.findall(r"\d+", openmc.__version__)
    if len(parts) < 3 or tuple(int(part) for part in parts[:3]) < (0, 15, 3):
        raise ValueError(f"coupled design heating needs OpenMC 0.15.3 or later; found {openmc.__version__}")
    settings = openmc.Settings()
    try:
        settings.photon_transport = True
        settings.atomic_relaxation = True
        settings.electron_treatment = "led"
    except (AttributeError, ValueError) as exc:
        raise ValueError(f"this OpenMC build does not support coupled photon, atomic-relaxation and LED settings: {exc}") from exc


def phase_heating_preflight(request: dict, work: Path) -> dict:
    """Return a structured NOT_EVALUATED report for missing coupled data before volume rays run."""
    import h5py
    import openmc

    target = work / "solver" / "heating-preflight.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    identity_hashes = {
        "cross_sections_sha256": sha256_file(request["cross_sections"]),
        "h5m_sha256": sha256_file(request["h5m"]),
    }
    runtime = None
    try:
        validate_heating_request(request)
        validate_coupled_openmc_support(openmc)
        universe = openmc.DAGMCUniverse(str(request["h5m"]))
        mesh_cells = heating_mesh_cell_count(
            universe.bounding_box, request["response"]["peak_voxel_cm"],
            len(request["response"]["solid_ids"]))
        mesh_error = heating_mesh_not_evaluated(mesh_cells, MAX_HEATING_VOLUME_CELLS)
        if mesh_error is not None:
            raise ValueError(mesh_error)
        runtime = heating_runtime_identity(openmc)
        identity = heating_data_identity(request, openmc, h5py)
        record = {"schema": "faris-design-heating-preflight/v1", "status": "READY",
                  **identity_hashes, "mesh_voxel_solid_count": mesh_cells,
                  "runtime_identity": runtime, "data_identity": identity}
    except (OSError, ValueError, KeyError, AttributeError) as exc:
        reason = str(exc)
        record = {"schema": "faris-design-heating-preflight/v1", "status": "NOT_EVALUATED",
                  **identity_hashes, "runtime_identity": runtime,
                  "why": reason, "next_step": heating_preflight_next_step(reason)}
    target.write_text(json.dumps(record, indent=1) + "\n")
    return record


def phase_heating(request: dict, work: Path) -> dict:
    """Separate coupled heating run. It deliberately has no neutron energy cutoff."""
    import h5py
    import numpy as np
    import openmc
    import openmc.lib
    from openmc_plasma_source import tokamak_source

    validate_heating_request(request)
    sampling = request["sampling"]
    solid_ids = request["response"]["solid_ids"]
    solids = {solid["solid_id"]: solid for solid in request["solids"]}

    def unavailable(reason: str, next_step: str, data_identity=None, runtime_identity=None,
                    execution_identity=None) -> dict:
        report = {"schema": "faris-design-heating/v1", "status": "NOT_EVALUATED", "why": reason,
                  "next_step": next_step, "normalization": "per_source_neutron",
                  "unit": "eV/source_neutron", "solids": []}
        if data_identity is not None:
            report["data_identity"] = data_identity
        if runtime_identity is not None:
            report["runtime_identity"] = runtime_identity
        if execution_identity is not None:
            report["execution_identity"] = execution_identity
        target = work / "solver" / "heating-result.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(report, indent=1) + "\n")
        return report

    if not solid_ids or any(solids[sid].get("classification") != "winding_pack" for sid in solid_ids):
        return unavailable("No authored winding-pack solid identity is present in the request.",
                           "Assign classification: winding_pack to the CAD solids that represent the winding pack, then rerun design check.")
    xml = Path(request["cross_sections"])
    runtime_identity = None
    try:
        runtime_identity = heating_runtime_identity(openmc)
        data_identity = heating_data_identity(request, openmc, h5py)
    except (OSError, ValueError, KeyError, AttributeError) as exc:
        return unavailable(str(exc), "Select a supported Python/OpenMC runtime and library with MT=301 heating, photon data, and complete atomic-relaxation shell maps for every used element.",
                           runtime_identity=runtime_identity)
    preflight = json.loads((work / "solver" / "heating-preflight.json").read_text())
    if (preflight.get("status") != "READY"
            or preflight.get("data_identity") != data_identity
            or preflight.get("runtime_identity") != runtime_identity
            or preflight.get("cross_sections_sha256") != sha256_file(xml)
            or preflight.get("h5m_sha256") != sha256_file(request["h5m"])):
        return unavailable("Coupled-heating nuclear data or geometry identity changed after preflight.",
                           "Keep the checked geometry, selected Python/OpenMC runtime and nuclear data unchanged, then rerun heating.",
                           data_identity, runtime_identity)
    mesh_cells = preflight.get("mesh_voxel_solid_count")
    if (not isinstance(mesh_cells, int) or mesh_cells <= 0
            or heating_mesh_not_evaluated(mesh_cells, MAX_HEATING_VOLUME_CELLS) is not None):
        return unavailable("The ready preflight has no valid bounded heating mesh identity.",
                           "Rerun coupled heating so the peak mesh is bounded before volume sampling.",
                           data_identity, runtime_identity)

    try:
        source_snapshot, expected_h5m_sha = checked_heating_h5m_snapshot(request, work, "heating")
    except (OSError, ValueError, KeyError) as exc:
        return unavailable(str(exc), "Keep the checked H5M unchanged and rerun coupled heating.",
                           data_identity, runtime_identity)

    geometry_request = {**request, "h5m": str(source_snapshot)}
    geometry = geometry_copy(geometry_request, work / "geometry" / "model-heating.h5m", random_ray=False)
    if not sha256_file(source_snapshot).lower() == expected_h5m_sha.lower():
        return unavailable("The stable checked H5M snapshot changed while the transport geometry was prepared.",
                           "Keep the run directory unchanged and rerun coupled heating.",
                           data_identity, runtime_identity)
    model, _, _ = build_model(openmc, request, geometry)
    source = tokamak_source(**tokamak_arguments(request["plasma"]))
    normalize_heating_source(source)
    settings = model.settings
    settings.source = [source]
    settings.photon_transport = True
    settings.atomic_relaxation = True
    settings.electron_treatment = "led"
    settings.particles = int(sampling["particles_per_batch"])
    settings.batches = int(sampling["max_batches"])
    seed_value = int(sampling["seed"])
    setattr(settings, "seed", seed_value)
    # No energy_neutron cutoff is set: this is a coupled, all-energy heat run.
    settings.output = {"tallies": False, "summary": False}
    settings.sourcepoint = {"write": False}
    settings.statepoint = {"batches": []}
    ids = volume_ids(request)
    cell_filter = openmc.CellFilter([ids[sid] for sid in solid_ids])
    peak_volume_record = json.loads((work / "peak" / "record.json").read_text())
    peak_path = work / "peak" / "volumes.npz"
    if (sha256_file(peak_path) != peak_volume_record["volumes_sha256"]
            or peak_volume_record["solid_ids"] != solid_ids
            or peak_volume_record["voxel_cm"] != request["response"]["peak_voxel_cm"]
            or peak_volume_record.get("h5m_sha256", "").lower() != preflight["h5m_sha256"].lower()):
        return unavailable("The sampled winding-pack volumes do not match the exact checked H5M, solids, or voxel widths in this heating request.",
                           "Keep the checked geometry and heating inputs unchanged, rerun the volume sampling, then rerun coupled heating.",
                           data_identity, runtime_identity)
    pieces = np.load(peak_path)
    volume_by_solid = pieces["volumes_cm3"].reshape(len(solid_ids), -1)
    bins, solid_count = volume_by_solid.shape[1], len(solid_ids)
    if bins * solid_count > MAX_HEATING_VOXEL_CELLS:
        return unavailable("The requested peak mesh exceeds the bounded heating tally size.",
                           "Use larger peak voxels or split the winding pack into fewer authored solids.",
                           data_identity, runtime_identity)
    mesh = openmc.CylindricalMesh(mesh_id=PEAK_MESH_ID, r_grid=pieces["r_edges_cm"],
                                  z_grid=pieces["z_edges_cm"], phi_grid=[0.0, 2.0 * math.pi])
    components = ("neutron", "photon", "electron", "positron")
    tallies, voxel_tallies, all_tallies = {}, {}, []
    for index, component in enumerate(components + ("total",)):
        tally = openmc.Tally(tally_id=100 + index, name=f"heating-{component}")
        tally.filters = heating_tally_filters(openmc, component, cell_filter)
        tally.nuclides = ["total"]
        tally.scores = ["heating"]
        tally.estimator = "collision"
        tallies[component] = tally
        voxel_tally = openmc.Tally(tally_id=110 + index, name=f"heating-voxel-{component}")
        voxel_tally.filters = heating_tally_filters(openmc, component, cell_filter, mesh)
        voxel_tally.nuclides = ["total"]
        voxel_tally.scores = ["heating"]
        voxel_tally.estimator = "collision"
        voxel_tallies[component] = voxel_tally
        all_tallies.extend((tally, voxel_tally))
    model.tallies = openmc.Tallies(all_tallies)
    model_xml = work / "solver" / "heating-model.xml"
    model_xml.parent.mkdir(parents=True, exist_ok=True)
    model.export_to_model_xml(model_xml)
    try:
        execution_identity = heating_execution_identity(geometry, model_xml)
    except (OSError, ValueError) as exc:
        return unavailable(str(exc), "Keep the retagged H5M and exported model XML as bounded regular files, then rerun heating.",
                           data_identity, runtime_identity)
    here = os.getcwd()
    os.chdir(model_xml.parent)
    batches = voxel_batches = None
    started = None
    batches_done = 0
    stop_reason = "history_limit"
    terminal_status = None
    aggregate = ScalarMoments()
    peak_relative_error = None
    try:
        openmc.lib.init(["--threads", str(int(sampling["threads"])), str(model_xml)])
        try:
            init_status = openmc.lib.simulation_init()
            if init_status not in (None, 0):
                terminal_status = init_status
            started = time.monotonic()
            if terminal_status is None:
                (batch_moments, voxel_moments, closure_moments, voxel_closure_moments,
                 aggregate, peak_relative_error, batches_done, stop_reason, terminal_status) = collect_heating_batches(
                    openmc.lib, tallies, voxel_tallies, len(solid_ids), bins,
                    int(sampling["max_batches"]), float(sampling["max_seconds"]), started,
                    float(sampling["target_relative_error"]), volume_by_solid,
                    ranked_pieces(pieces["volumes_cm3"], pieces["r_edges_cm"]).reshape(solid_count, -1),
                    int(sampling["particles_per_batch"]))
            openmc.lib.simulation_finalize()
        finally:
            openmc.lib.finalize()
    finally:
        os.chdir(here)
    try:
        after_run_identity = heating_execution_identity(geometry, model_xml)
    except (OSError, ValueError) as exc:
        return unavailable(str(exc), "Keep the retagged H5M and exported model XML unchanged while OpenMC runs, then rerun heating.",
                           data_identity, runtime_identity, execution_identity)
    if after_run_identity != execution_identity:
        return unavailable("The retagged H5M or exported model XML changed while the OpenMC C API run was active.",
                           "Keep both C API inputs unchanged for the full run and rerun coupled heating.",
                           data_identity, runtime_identity, execution_identity)
    histories = batches_done * int(sampling["particles_per_batch"])
    lost = lost_count(getattr(openmc.lib, "_dll", None))
    terminal_quality, terminal_why = heating_terminal_quality(
        terminal_status, batches_done, int(sampling["max_batches"]), stop_reason, lost, histories)
    if terminal_quality == "NOT_EVALUATED":
        result = unavailable(terminal_why or "The OpenMC heating run did not complete with a recognized status.",
                             "Use an OpenMC build with the supported C API terminal status and lost-particle counter, then rerun heating.",
                             data_identity, runtime_identity, execution_identity)
        result.update({"c_api_terminal_status": terminal_status, "lost_particles": lost,
                       "histories": histories, "batches": batches_done, "stop_reason": stop_reason,
                       "target_relative_error": float(sampling["target_relative_error"]),
                       "peak_target_relative_error": HEATING_PEAK_TARGET_FACTOR * float(sampling["target_relative_error"]),
                       "target_reached": stop_reason == "target_reached",
                       "mean_relative_error": aggregate.relative_error,
                       "peak_relative_error": peak_relative_error,
                       "variance_of_variance": aggregate.variance_of_variance})
        (work / "solver" / "heating-result.json").write_text(json.dumps(result, indent=1) + "\n")
        return result
    if batches_done < int(sampling["min_batches"]):
        return unavailable(f"The heating run stopped after {batches_done} batches, below the minimum {sampling['min_batches']} batches.",
                           "Increase the time limit and rerun coupled heating.", data_identity, runtime_identity,
                           execution_identity)
    result_solids = []
    for row, solid_id in enumerate(solid_ids):
        values = {}
        for name in tallies:
            mean = float(batch_moments[name].mean[row])
            standard_error = float(batch_moments[name].standard_error[row])
            values[name] = {"mean_ev_per_source": mean,
                            "standard_error_ev_per_source": standard_error}
        closure_mean = float(closure_moments.mean[row])
        closure_se = float(closure_moments.standard_error[row])
        closure_scale = abs(values["total"]["mean_ev_per_source"]) + math.fsum(
            abs(values[name]["mean_ev_per_source"]) for name in components)
        values["closure_residual"] = {"mean_ev_per_source": closure_mean, "standard_error_ev_per_source": closure_se,
                                      **heating_closure_quality(closure_mean, closure_se, closure_scale)}
        ranked = ranked_pieces(pieces["volumes_cm3"], pieces["r_edges_cm"]).reshape(solid_count, -1)[row]
        peak = peak_of_moments(voxel_moments["total"].mean[:, row],
                               voxel_moments["total"].standard_error[:, row],
                               volume_by_solid[row], ranked)
        peak_index = None
        if peak is not None:
            if peak["mean"] <= 0.0:
                # Keep the measured zero, while making its undefined relative uncertainty explicit.
                peak["standard_error"] = None
            index = peak.pop("index")
            peak_index = index
            iz, ir = divmod(index, pieces["volumes_cm3"].shape[2])
            peak["r_cm"] = [float(pieces["r_edges_cm"][ir]), float(pieces["r_edges_cm"][ir + 1])]
            peak["z_cm"] = [float(pieces["z_edges_cm"][iz]), float(pieces["z_edges_cm"][iz + 1])]
            peak["pieces_ranked"] = int(ranked.sum())
            peak["volume_estimator"] = {"method": "OpenMC DAGMC mesh.material_volumes ray sampling",
                                        "rays": int(peak_volume_record["rays"]),
                                        "sampling_relative_error": None,
                                        "sampling_uncertainty_reported": False}
            peak_index = index
            closure_mean = float(voxel_closure_moments.mean[peak_index, row])
            closure_se = float(voxel_closure_moments.standard_error[peak_index, row])
            peak_components = {}
            for name in components + ("total",):
                peak_components[name] = {
                    "mean_ev_per_source": float(voxel_moments[name].mean[peak_index, row]),
                    "standard_error_ev_per_source": float(voxel_moments[name].standard_error[peak_index, row]),
                }
            peak["components"] = peak_components
            closure_scale = abs(peak_components["total"]["mean_ev_per_source"]) + math.fsum(
                abs(peak_components[name]["mean_ev_per_source"]) for name in components)
            peak["closure_residual"] = {"mean_ev_per_source": closure_mean,
                                        "standard_error_ev_per_source": closure_se,
                                        **heating_closure_quality(closure_mean, closure_se, closure_scale)}
        values["peak"] = peak
        voxel_scores = []
        for index, volume_cm3 in enumerate(volume_by_solid[row]):
            if volume_cm3 <= 0.0:
                continue
            iz, ir = divmod(index, pieces["volumes_cm3"].shape[2])
            component_scores = {}
            for name in components + ("total",):
                component_scores[name] = {
                    "mean_ev_per_source": float(voxel_moments[name].mean[index, row]),
                    "standard_error_ev_per_source": float(voxel_moments[name].standard_error[index, row]),
                }
            residual_mean = float(voxel_closure_moments.mean[index, row])
            residual_se = float(voxel_closure_moments.standard_error[index, row])
            voxel_scores.append({
                "r_cm": [float(pieces["r_edges_cm"][ir]), float(pieces["r_edges_cm"][ir + 1])],
                "z_cm": [float(pieces["z_edges_cm"][iz]), float(pieces["z_edges_cm"][iz + 1])],
                "volume_cm3": float(volume_cm3),
                "components": component_scores,
                "closure_residual": {
                    "mean_ev_per_source": residual_mean,
                    "standard_error_ev_per_source": residual_se,
                    **heating_closure_quality(residual_mean, residual_se,
                        abs(component_scores["total"]["mean_ev_per_source"]) + math.fsum(
                            abs(component_scores[name]["mean_ev_per_source"]) for name in components)),
                },
                "is_peak": index == peak_index,
            })
        values["voxel_scores"] = voxel_scores
        result_solids.append({"solid_id": solid_id, **values})
    closure_failed = any(
        not score["closure_residual"]["within_3se"]
        for solid in result_solids
        for score in [solid, *solid["voxel_scores"], *([solid["peak"]] if solid["peak"] else [])])
    peak_reasons = []
    for solid in result_solids:
        if solid["peak"] is not None:
            _, quality_reason = heating_peak_quality(
                solid["peak"]["mean"], solid["peak"]["standard_error"])
            if quality_reason:
                peak_reasons.append(quality_reason)
    reasons = []
    vov = aggregate.variance_of_variance
    mean_relative_error = aggregate.relative_error
    if stop_reason != "target_reached":
        reasons.append(
            f"The heating run stopped at its {stop_reason.replace('_', ' ')} before the authored target "
            f"of {100 * sampling['target_relative_error']:g}% mean and "
            f"{100 * HEATING_PEAK_TARGET_FACTOR * sampling['target_relative_error']:g}% peak error was reached."
        )
    if vov is None:
        reasons.append("Heating batch variance-of-variance is unavailable; the target error cannot be accepted.")
    elif vov > HEATING_VOV_LIMIT:
        reasons.append(f"Heating batch variance-of-variance {vov:.3g} exceeds {HEATING_VOV_LIMIT:g}; the error is not reliable.")
    if mean_relative_error is None or mean_relative_error > sampling["target_relative_error"]:
        reasons.append("The aggregate heating mean has no usable error estimate at or below its authored target.")
    if peak_relative_error is None or peak_relative_error > HEATING_PEAK_TARGET_FACTOR * sampling["target_relative_error"]:
        reasons.append("At least one resolved per-solid heating peak has no usable error estimate at or below its authored target.")
    if closure_failed:
        reasons.append("At least one per-coil or per-voxel correlated batch closure residual exceeded three standard errors plus the explicit floating-point tolerance.")
    reasons.extend(peak_reasons)
    if terminal_quality == "INCONCLUSIVE" and terminal_why:
        reasons.append(terminal_why)
    result = {"schema": "faris-design-heating/v1", "status": "INCONCLUSIVE" if reasons else "CALCULATED",
              "why": " ".join(reasons) if reasons else None,
              "next_step": ("Increase histories or investigate the closure diagnostics and sampled peak uncertainty, then rerun coupled heating."
                            if reasons else None),
              "c_api_terminal_status": terminal_status, "lost_particles": lost,
              "openmc": openmc.__version__,
              "h5m_sha256": sha256_file(request["h5m"]), "cross_sections_sha256": sha256_file(xml),
              "runtime_identity": runtime_identity, "execution_identity": execution_identity,
              "histories": histories,
              "stop_reason": stop_reason,
              "batches": batches_done,
              "target_relative_error": float(sampling["target_relative_error"]),
              "peak_target_relative_error": HEATING_PEAK_TARGET_FACTOR * float(sampling["target_relative_error"]),
              "target_reached": stop_reason == "target_reached",
              "mean_relative_error": mean_relative_error,
              "peak_relative_error": peak_relative_error,
              "variance_of_variance": vov,
              "normalization": "per_source_neutron", "unit": "eV/source_neutron",
              "particle_components": list(components), "data_identity": data_identity,
              "solids": result_solids,
              "volume_basis": "Sampled cylindrical ring voxels; geometry sampling uncertainty is reported separately when available.",
              "limitations": ["OpenMC material-volume ray count is reported; a volume sampling standard error is not exposed by this adapter."]}
    (work / "solver" / "heating-result.json").write_text(json.dumps(result, indent=1) + "\n")
    return result


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--phase", choices=("run", "mgxs", "windows", "volumes", "heating_preflight", "heating"), required=True)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    request = json.loads(args.request.read_text(encoding="utf-8"))
    try:
        if args.phase in ("run", "mgxs", "windows") or (
            args.phase == "volumes" and "variance_reduction" in request
        ):
            validate_request(request)
        else:
            validate_heating_request(request)
    except (KeyError, TypeError, ValueError) as error:
        print(f"invalid request: {error}", file=sys.stderr)
        return 2
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    {"run": phase_run, "mgxs": phase_mgxs, "windows": phase_windows, "volumes": phase_volumes,
     "heating_preflight": phase_heating_preflight, "heating": phase_heating}[args.phase](request, work)
    return 0


if __name__ == "__main__":
    sys.exit(main())
