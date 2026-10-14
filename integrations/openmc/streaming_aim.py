#!/usr/bin/env python3
"""Aiming for streaming paths (NUC-055): plan a windowed run that aims at targets FARIS finds in the model.

    streaming_aim.py MODEL.xml WORK_DIR --library /path/to/libopenmc.so [--response-material 4]

`plan` returns one of two modes, always with the reason and the next step:

- "aimed": the model's windows are capped along the line of sight from the response outward, and the run aims its
  source and its elastic collisions at the duct exit and inlet. This needs an OpenMC built with
  patches/faris-openmc-0.15.3-aim.patch.
- "windows-only": the model runs as it is, with its own windows. Chosen when the OpenMC library lacks the patch,
  or when no part of the response is in direct line of sight of the source (a bent duct, or no duct).

Stock OpenMC stays supported: the patch is an accelerator, never a requirement. The method, its measurements and
its limits are in docs/notes/CAD_TRANSPORT_S0F.md (facts 13 to 15, protocol 5). The numerical routines are the
ones those protocols ran, in risk_tests/s0f_find_targets.py and risk_tests/s0f_los_windows.py; this module holds
the decisions around them and imports only the standard library until a plan is asked for.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

PATCH = "integrations/openmc/patches/faris-openmc-0.15.3-aim.patch"
MARKERS = (b"OPENMC_APERTURE_BIAS", b"OPENMC_APERTURE_ONE_SIDED", b"OPENMC_COLLISION_AIM", b"OPENMC_WW_SKIP_VOID")
SOURCE_BETA, COLLISION_BETA = 0.1, 0.2   # the mixtures protocols 2 to 5 ran with


def aiming_supported(library: Path | None) -> bool:
    """True if the OpenMC shared library was built with the aiming patch: it then holds every variable's name."""
    if library is None or not Path(library).is_file():
        return False
    data = Path(library).read_bytes()
    return all(m in data for m in MARKERS)


def windows_only(reason: str, next_step: str) -> dict:
    return {"mode": "windows-only", "reason": reason, "next_step": next_step, "env": {}, "targets": []}


def aim_env(aperture_spec: str) -> dict:
    """The variables an aimed run sets, exactly (protocol 5's F9)."""
    return {"OPENMC_WW_SKIP_VOID": "1", "OPENMC_APERTURE_BIAS": aperture_spec, "OPENMC_APERTURE_ONE_SIDED": "1",
            "OPENMC_COLLISION_AIM": f"{COLLISION_BETA:g} *"}


def choose(supported: bool, targets) -> dict | None:
    """The fall-back plan, or None if an aimed run is possible. `targets` is None before the search."""
    if not supported:
        return windows_only(
            "This OpenMC build cannot aim particles at a duct.",
            f"Results are unaffected; a streaming response takes several times longer to converge. "
            f"To shorten it, build OpenMC 0.15.3 with {PATCH}.")
    if targets is None:
        return None
    names = [t[0] for t in targets]
    if not any(n.startswith("face") for n in names):
        return windows_only(
            "No part of the response is in direct line of sight of the source, so there is nothing to aim at.",
            "Nothing to do for a response behind a bent duct or a solid shield: the model's own windows apply.")
    if not any(n.startswith("exit") for n in names):
        return windows_only(
            "The response is in line of sight of the source, but no narrow channel leads to it.",
            "Nothing to do: the response is not behind a duct, and the model's own windows apply.")
    return None


def tidy(directory: Path) -> None:
    """Remove the bulky files of finished helper runs under `directory`: inputs, statepoints and summaries."""
    for pattern in ("model.xml", "statepoint.*.h5", "summary.h5", "tallies.out"):
        for path in Path(directory).rglob(pattern):
            path.unlink()


def plan(model: Path, work: Path, library: Path | None, response_material: int = 4,
         particles: int = 10_000_000, keep_runs: bool = True) -> dict:
    """Decide the mode for `model` (one XML file with its windows on mesh 9000) and prepare an aimed run if one
    is possible. Writes WORK/plan.json and, for an aimed run, WORK/model-aimed.xml. With `keep_runs` false the
    helper runs' bulky files are removed as soon as each step has read them."""
    work.mkdir(parents=True, exist_ok=True)
    out = choose(aiming_supported(library), None)
    if out is None:
        sys.path.insert(0, str(Path(__file__).resolve().parent / "risk_tests"))
        import s0f_find_targets as T
        import s0f_los_windows as W
        targets = T.find_targets(model, work / "targets", particles, 10.0, response_material, 1.0e7)
        if not keep_runs:
            tidy(work / "targets")
        out = choose(True, targets)
        if out is None:
            aimed = work / "model-aimed.xml"
            counts = W.cap_model(model, aimed, work / "visibility", targets)
            if not keep_runs:
                tidy(work / "visibility")
            out = {"mode": "aimed", "model": str(aimed), "env": aim_env(T.aperture_spec(targets, SOURCE_BETA)),
                   "reason": ("Neutrons reach the response through a duct without a collision (through the void "
                              "or thin material); the run aims at the duct's exit and inlet."),
                   "next_step": "Run the aimed model with the variables given, on the patched OpenMC.",
                   "targets": [{"name": n, "centre": list(map(float, c)), "u": list(map(float, u)),
                                "v": list(map(float, v)), "normal": list(map(float, nr))}
                               for n, c, u, v, nr in targets],
                   "windows_lowered": counts["lowered"]}
    (work / "plan.json").write_text(json.dumps(out, indent=1))
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("model", type=Path)
    ap.add_argument("work", type=Path)
    ap.add_argument("--library", type=Path, help="the libopenmc shared library the run will use")
    ap.add_argument("--response-material", type=int, default=4)
    args = ap.parse_args(argv)
    out = plan(args.model, args.work, args.library, args.response_material)
    print(f"{out['mode']}: {out['reason']} {out['next_step']}")
    for k, v in out["env"].items():
        print(f"{k}={v}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
