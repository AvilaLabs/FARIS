#!/usr/bin/env python3
"""Transport worker of `faris design transport`: a checked design's DAGMC geometry, run to a target error.

    design_transport.py --request request.json --out-dir DIR

The request (`faris-design-transport-request/v1`) is written by the engine from a design file
that passed `faris design check`:

    h5m                 path of the converted geometry; every solid is tagged with its solid id
    cross_sections      cross_sections.xml of the audited library
    void_solid_ids      the solids that are empty space
    materials           [{solid_id, nuclide_atom_fractions, density_kg_m3, temperature_k}],
                        one entry per solid that is not void
    plasma              the design file's plasma block (SI)
    response            {solid_ids, energy_min_ev}: flux above the threshold in these solids
    sampling            {target_relative_error, particles_per_batch, min_batches, max_batches,
                         max_seconds, seed, threads}

It scores the track-length flux above the threshold in each response solid and stops at the
first batch where the relative standard error of the sum over the solids is at or below the
target, or at the batch or time limit. The error of the sum comes from the per-batch sums,
so correlation between solids is included. Results are per source neutron (cm); the engine
converts them to a flux with the source rate and the volumes.

Progress goes to standard output as one JSON object per line. The result file is
`result.json` (`faris-design-transport/v1`). openmc is imported lazily so the pure helpers
import without it.
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

REQUEST_SCHEMA = "faris-design-transport-request/v1"
RESULT_SCHEMA = "faris-design-transport/v1"
ENERGY_MAX_EV = 2.0e7
TALLY_ID = 1
OUTER_ID = 10_000_000
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
    sampling = request["sampling"]
    need(0.0 < sampling["target_relative_error"] < 1.0, "target_relative_error must lie between 0 and 1")
    need(sampling["particles_per_batch"] >= 1, "particles_per_batch must be at least 1")
    need(2 <= sampling["min_batches"] <= sampling["max_batches"], "need 2 <= min_batches <= max_batches")
    need(sampling["max_seconds"] > 0.0, "max_seconds must be positive")
    need(1 <= sampling["threads"] <= 32, "threads must lie between 1 and 32")
    solids = [m["solid_id"] for m in request["materials"]]
    need(len(set(solids)) == len(solids), "a solid has two materials")
    need(not set(solids) & set(request["void_solid_ids"]), "a solid is both void and filled")
    response = request["response"]
    need(response["solid_ids"], "the response names no solid")
    missing = sorted(set(response["solid_ids"]) - set(solids))
    need(not missing, f"response solids without a material: {', '.join(missing)}")
    need(0.0 <= response["energy_min_ev"] < ENERGY_MAX_EV, "energy_min_ev is outside the data range")
    for material in request["materials"]:
        need(material["density_kg_m3"] > 0.0, f"{material['solid_id']}: density must be positive")
        need(material["nuclide_atom_fractions"], f"{material['solid_id']}: no nuclides")


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


def explain(reason: str, stats: RunningSum, sampling: dict) -> tuple[str, str]:
    """(why, next_step) for a run that stopped before its target."""
    rel = stats.relative_error
    seen = "no nonzero score was recorded" if rel is None else f"the error reached {100.0 * rel:.1f} %"
    target = 100.0 * sampling["target_relative_error"]
    if rel is None:
        more = "Raise the history limit by a large factor, or generate weight windows for this response."
    else:
        factor = (rel / sampling["target_relative_error"]) ** 2
        more = (f"About {factor:.1f} times as many histories are needed for {target:g} %: raise the limit, "
                "or generate weight windows for this response.")
    limit = "history limit" if reason == "history_limit" else "time limit"
    return f"The run stopped at its {limit}; {seen} against a target of {target:g} %.", more


def void_names(values, void_solid_ids) -> list[int]:
    """Indices of the group names `mat:<solid id>` that belong to void solids."""
    wanted = {f"mat:{solid}".encode() for solid in void_solid_ids}
    return [i for i, raw in enumerate(values) if bytes(raw).split(b"\0", 1)[0] in wanted]


# ---------------------------------------------------------------------------
# OpenMC

def void_copy(h5m: Path, void_solid_ids: list[str], target: Path) -> Path:
    """A copy of the h5m whose void solids carry the tag `mat:void`, which OpenMC reads as vacuum.

    `faris design check` tags every solid with its own id so that each DAGMC volume can be
    matched to its solid; OpenMC would look for a material of that name. Only the group
    names of the void solids change; every solid named must be found.
    """
    import shutil

    import h5py

    shutil.copyfile(h5m, target)
    with h5py.File(target, "r+") as f:
        values = f["tstt"]["tags"]["NAME"]["values"]
        names = values[()]
        hits = void_names(names, void_solid_ids)
        if len(hits) != len(set(void_solid_ids)):
            raise ValueError(f"{len(set(void_solid_ids))} void solids are named but {len(hits)} tags were found in {h5m}")
        width = names.dtype.itemsize
        for i in hits:
            names[i] = b"mat:void".ljust(width, b"\0")
        values[...] = names
    return target


def build_model(openmc, request: dict, geometry: Path):
    from openmc_plasma_source import tokamak_source

    materials = {}
    for entry in sorted(request["materials"], key=lambda m: m["solid_id"]):
        material = openmc.Material(name=entry["solid_id"])
        for nuclide, fraction in sorted(entry["nuclide_atom_fractions"].items()):
            material.add_nuclide(nuclide, fraction, percent_type="ao")
        material.set_density("kg/m3", entry["density_kg_m3"])
        if entry.get("temperature_k") is not None:
            material.temperature = entry["temperature_k"]
        materials[entry["solid_id"]] = material
    universe = openmc.DAGMCUniverse(str(geometry))
    box = universe.bounding_box
    radius = 1.05 * max(math.sqrt(sum(c * c for c in corner)) for corner in (box.lower_left, box.upper_right))
    # Ids far above the DAGMC volumes and surfaces, which keep the numbers of the h5m.
    sphere = openmc.Sphere(surface_id=OUTER_ID, r=radius, boundary_type="vacuum")
    model = openmc.Model(geometry=openmc.Geometry([openmc.Cell(cell_id=OUTER_ID, fill=universe, region=-sphere)]),
                         materials=openmc.Materials(list(materials.values())))
    model.materials.cross_sections = str(request["cross_sections"])

    source = tokamak_source(**tokamak_arguments(request["plasma"]))
    total = float(source.strength)
    for element in source.sources.ravel():
        element.strength = float(element.strength) / total
    source.strength = 1.0

    sampling = request["sampling"]
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.source = [source]
    settings.particles = int(sampling["particles_per_batch"])
    settings.batches = int(sampling["max_batches"])
    settings.seed = int(sampling["seed"])
    settings.photon_transport = False
    settings.output = {"tallies": False, "summary": False}
    settings.sourcepoint = {"write": False}
    settings.statepoint = {"batches": []}

    response = request["response"]
    tally = openmc.Tally(tally_id=TALLY_ID, name="response-fast-flux")
    tally.filters = [
        openmc.MaterialFilter([materials[s] for s in response["solid_ids"]]),
        openmc.EnergyFilter([float(response["energy_min_ev"]), ENERGY_MAX_EV]),
    ]
    tally.scores = ["flux"]
    tally.estimator = "tracklength"
    model.tallies = openmc.Tallies([tally])
    return model


def run(request: dict, out_dir: Path) -> dict:
    import numpy as np
    import openmc
    import openmc.lib

    sampling, response = request["sampling"], request["response"]
    geometry = void_copy(Path(request["h5m"]), request["void_solid_ids"], out_dir / "model-void.h5m")
    model = build_model(openmc, request, geometry)
    model.export_to_model_xml(out_dir / "model.xml")
    here = os.getcwd()
    os.chdir(out_dir)
    stats = RunningSum()
    solids = response["solid_ids"]
    per_solid = [RunningSum() for _ in solids]
    started = time.monotonic()
    reason = None
    try:
        openmc.lib.init(["--threads", str(int(sampling["threads"])), "model.xml"])
        try:
            openmc.lib.simulation_init()
            tally = openmc.lib.tallies[TALLY_ID]
            previous = np.zeros(len(solids))
            last_report = started
            while reason is None:
                openmc.lib.next_batch()
                # Column 1 holds the sum of the batch values so far; its increase is this batch's value.
                sums = np.array(tally.results[:, 0, 1], dtype=float)
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
        "void_h5m_sha256": sha256_file(geometry),
        "model_xml_sha256": sha256_file(out_dir / "model.xml"),
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
        "why": None,
        "next_step": None,
    }
    if reason != "target_reached":
        result["why"], result["next_step"] = explain(reason, stats, sampling)
    return result


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    request = json.loads(args.request.read_text(encoding="utf-8"))
    try:
        validate_request(request)
    except (KeyError, TypeError, ValueError) as error:
        print(f"invalid request: {error}", file=sys.stderr)
        return 2
    out_dir = args.out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    result = run(request, out_dir)
    target = out_dir / "result.json"
    with target.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=1)
        stream.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
