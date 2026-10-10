#!/usr/bin/env python3
"""Sample neutron source sites from the design file's `plasma` block (check 9, SRC-018).

Run with the OpenMC environment, as a separate process under the FARIS bounded-job
runner:

    source_sites.py --request request.json --out-base sites

The request carries the plasma block in SI units (the design file's fields) and the
number of sites and the seed. Sites are drawn with `openmc-plasma-source`'s
`tokamak_source` through OpenMC's own source machinery (`sample_external_source`),
so the sites are the ones a transport run would start from. The geometry OpenMC is
initialised with is a single empty sphere: FARIS tests the sites against the model
itself, with its own exact point-in-volume, and OpenMC never sees the model.

Writes `<out-base>.bin` (N x 3 little-endian float64, metres) and `<out-base>.json`
(schema "faris-source-sites/v1": versions, count, seed, the sha256 of the binary).
`openmc_plasma_source` is found on PYTHONPATH; FARIS never installs it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
import time
from pathlib import Path

SCHEMA = "faris-source-sites/v1"
HELPER_VERSION = "1"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


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


def sample(request: dict, timing: dict):
    import numpy as np
    import openmc
    import openmc.lib
    from openmc_plasma_source import tokamak_source

    started = time.monotonic()
    source = tokamak_source(**tokamak_arguments(request["plasma"]))
    timing["build_source"] = time.monotonic() - started
    sphere = openmc.Sphere(r=1.0e7, boundary_type="vacuum")
    model = openmc.Model()
    model.geometry = openmc.Geometry([openmc.Cell(region=-sphere)])
    model.settings.run_mode = "fixed source"
    model.settings.batches = 1
    model.settings.particles = 10
    model.settings.source = [source]
    here = os.getcwd()
    with tempfile.TemporaryDirectory() as scratch:
        # OpenMC insists on a cross-section file with one entry even when the model has no
        # material; the entry is never read.
        stub = Path(scratch) / "cross_sections.xml"
        stub.write_text('<?xml version="1.0"?>\n<cross_sections>\n  <library materials="H1" path="stub.h5" type="neutron"/>\n</cross_sections>\n')
        os.environ["OPENMC_CROSS_SECTIONS"] = str(stub)
        model.export_to_model_xml(scratch)
        os.chdir(scratch)
        try:
            openmc.lib.init(["--threads", "1"])
            try:
                started = time.monotonic()
                sites = openmc.lib.sample_external_source(int(request["count"]), prn_seed=int(request["seed"]))
                points = np.array([s.r for s in sites], dtype=np.float64) * 0.01
                timing["sample"] = time.monotonic() - started
            finally:
                openmc.lib.finalize()
        finally:
            os.chdir(here)
    return points, {"openmc": openmc.__version__}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--out-base", type=Path, required=True)
    args = parser.parse_args(argv)
    request = json.loads(args.request.read_text())
    timing: dict = {}
    points, versions = sample(request, timing)
    import importlib.metadata as metadata

    try:
        versions["openmc_plasma_source"] = metadata.version("openmc_plasma_source")
    except metadata.PackageNotFoundError:
        versions["openmc_plasma_source"] = None
    versions["python"] = sys.version.split()[0]
    data = points.astype("<f8").tobytes()
    bin_path = args.out_base.with_name(args.out_base.name + ".bin")
    bin_path.write_bytes(data)
    report = {
        "schema": SCHEMA,
        "helper_version": HELPER_VERSION,
        "versions": versions,
        "count": int(len(points)),
        "seed": int(request["seed"]),
        "unit": "m",
        "timing_seconds": timing,
        "bin_sha256": sha256_bytes(data),
    }
    json_path = args.out_base.with_name(args.out_base.name + ".json")
    json_path.write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
