#!/usr/bin/env python3
"""Weight windows for the reactor study's Production tier: the generator of `faris design transport`, on the
study's CSG torus.

    reactor_windows.py --input input.json --work-dir DIR [--threads N]

The input is the reactor input of `reactor_transport.py` (`faris-openmc-input/v0.1`). The windows are aimed at the
response the stop rule watches, the magnet fast flux: a random-ray adjoint of the flux above that response's
energy threshold in the component it names, then CADIS windows (window_generator.py, which the design transport
also uses). This needs OpenMC 0.16 or later; the run itself stays on the pinned OpenMC 0.15.3 and reads the
windows file `DIR/windows/filled.h5` (labelled with FARIS's window and mesh ids) where it lies.

The geometry is the run's own: `reactor_transport.compose` builds it, so the windows follow whatever the run
will transport (including the port). Empty space becomes the filler, because random ray cannot cross void.
Windows change only the variance, never the mean.

Progress goes to standard output as one JSON object per line; the record is DIR/windows/record.json.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import reactor_transport as RT  # noqa: E402
import window_generator as WG  # noqa: E402

RECORD_SCHEMA = "faris-reactor-windows/v1"
RESPONSE_ID = RT.STOP_RULE_RESPONSE_ID
# The slab run that condenses the library, lighter than the design transport's: the study's windows are made on the
# user's machine before a run that should take minutes. A noisier library gives rougher windows, never a biased result.
REACTOR_MGXS = dict(WG.MGXS, nparticles=10000, batches=60)


def say(**fields) -> None:
    print(json.dumps(fields), flush=True)


def window_response(request: dict) -> tuple[str, float]:
    """(component id, energy threshold in eV) of the response the windows are aimed at."""
    found = [r for r in request["responses"] if r["id"] == RESPONSE_ID]
    RT.require(len(found) == 1, f"the windows are aimed at {RESPONSE_ID}, which the request does not define")
    response = found[0]
    RT.require(response["score"]["kind"] == "flux_above", f"{RESPONSE_ID} is not a flux above an energy")
    RT.require(response["domain"]["kind"] in ("component", "component_region"),
               f"{RESPONSE_ID} is not scored in a component")
    return response["domain"]["component_id"], float(response["score"]["energy_min_ev"])


def data_temperature(physics: dict) -> float:
    """OpenMC's rounded label of the materials' data temperature (K), as the run uses it."""
    temperatures = {m["recipe"]["nuclear_data_temperature_k"] for m in physics["materials"]
                    if m["recipe"]["kind"] == "nuclide_mixture"}
    RT.require(len(temperatures) == 1, "the materials do not share one data temperature")
    return float(round(temperatures.pop()))


def build(openmc, inp: dict, scratch: Path):
    """The run's geometry and materials with empty space filled. Returns (model, component cell, plasma cell)."""
    request = inp["request"]
    component_id, _ = window_response(request)
    # Volume sampling of the port is for the run's receipt; the windows need only the geometry.
    inp = dict(inp, penetration_volume_samples=100_000)
    composed = RT.compose(inp, scratch, openmc, data_temperature(inp["physics"]), {"atomic_relaxation_enabled": False})
    model, component_cells, plasma = composed[0], composed[8], composed[10]
    RT.require(component_id in component_cells, f"the response names the unknown component {component_id}")
    model.materials.cross_sections = str(inp["cross_sections"])
    model.settings.photon_transport = False
    model.settings.statepoint = {}
    model.settings.trigger_active = False
    model.tallies = openmc.Tallies()
    top = max(m.id for m in model.materials)
    filler = WG.filler_material(openmc, top + 1)
    WG.fill_void(openmc, model.geometry, filler)
    model.materials.append(filler)
    return model, component_cells[component_id], plasma


def bounds(manifest: dict) -> tuple:
    """The window mesh: the box around the outer torus."""
    outer = max(c["outer_minor_radius_m"] for v in manifest["variants"] for c in v["components"])
    # rounded so that the last bit of a metre-to-centimetre conversion cannot move a mesh edge
    reach = round((float(manifest["major_radius_m"]) + outer) * 100.0, 6)
    half_height = round(outer * 100.0, 6)
    return WG.window_grid([-reach, -half_height, -reach], [reach, half_height, reach])


def make(inp: dict, work: Path, threads: int) -> dict:
    import openmc

    started = time.monotonic()
    # the slab models of the multigroup step are built inside OpenMC and find the library only this way
    os.environ["OPENMC_CROSS_SECTIONS"] = str(inp["cross_sections"])
    work.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="geometry-", dir=work))
    _, threshold = window_response(inp["request"])
    edges = WG.group_edges(threshold)
    say(step="mgxs")
    model, _, plasma = build(openmc, inp, scratch)
    mgxs = WG.condense_multigroup(openmc, model, edges, WG.domain_source(openmc, plasma), work / "mgxs", threads,
                                REACTOR_MGXS)
    say(step="windows")
    model, cell, plasma = build(openmc, inp, scratch)
    record = WG.solve_windows(openmc, model, model.geometry.root_universe, WG.domain_source(openmc, plasma), [cell],
                              threshold, bounds(inp["manifest"]), work / "mgxs" / "mgxs.h5", work / "windows", threads)
    for path in scratch.iterdir():
        path.unlink()
    scratch.rmdir()
    record.update({"schema": RECORD_SCHEMA, "response_id": RESPONSE_ID, "mgxs": mgxs,
                   "elapsed_seconds": time.monotonic() - started})
    (work / "windows" / "record.json").write_text(json.dumps(record, indent=1) + "\n")
    return record


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=1)
    args = parser.parse_args(argv)
    inp = json.loads(args.input.read_text(encoding="utf-8"))
    make(inp, args.work_dir.resolve(), max(1, args.threads))
    return 0


if __name__ == "__main__":
    sys.exit(main())
