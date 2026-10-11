"""Shared constants and rules for stage S0d (docs/notes/CAD_TRANSPORT_S0D.md).

The rule functions are pure so test_s0d.py can check them without OpenMC. The OpenMC helpers take the module as an
argument, as elsewhere in this folder, so either installed version can call them.
"""
from __future__ import annotations

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0b_common as C  # noqa: E402

TIME_BOX_S = 3600.0
PRODUCTION_S = 600.0
MAX_SPLIT = 1000
Z_UNBIASED = C.Z_UNBIASED
H4_SIGMA = 3.0
H4_MIN_FACTOR = 2.0
NUC050_TARGET = 100.0

# Disjoint from S0 (2026xxxx, 8115xxxx), S0b (9001xxxx-9010xxxx) and S0c (9101xxxx-9107xxxx).
SEEDS = {"gen": {"T0": 90110000, "T1": 90110001, "T2": 90110002, "T3": 90110003},
         "prod": {"T1": 90120001, "T2": 90120002, "T3": 90120003}}

# Standing references from S0b (references/cad-transport-diagnostics-s0b.json): D2 analog, G6 windows on 0.15.3.
G6_GAIN = 8.87
G6_GAIN_SIGMA = 0.62


def cadis_adjoint_source(openmc, material, response_edges_ev: list[float]):
    """T2's adjoint source: uniform over the material, equal strength in each response group.

    Response (a) is the flux above 0.1 MeV, so its response function is 1 in every group above the cut. Random ray maps
    a discrete energy to the group containing it, so one line at each group's geometric midpoint carries that group.
    """
    lines = group_midpoints(response_edges_ev)
    return openmc.IndependentSource(energy=openmc.stats.Discrete(lines, [1.0 / len(lines)] * len(lines)),
                                    particle="neutron", constraints={"domains": [material]}, strength=1.0)


def group_midpoints(edges_ev: list[float]) -> list[float]:
    """Geometric midpoint of each group between consecutive edges."""
    if len(edges_ev) < 2 or any(b <= a for a, b in zip(edges_ev, edges_ev[1:])) or edges_ev[0] <= 0.0:
        raise ValueError("edges must be positive and increasing, at least two")
    return [math.sqrt(a * b) for a, b in zip(edges_ev, edges_ev[1:])]


def cadis_description(response_edges_ev: list[float]) -> dict:
    return {"domain": "TF material (tf_coil)", "groups_ev": response_edges_ev,
            "lines_ev": group_midpoints(response_edges_ev), "probabilities": "equal", "strength": 1.0,
            "forward_weighting": "none (CADIS)"}


def window_ranges(openmc, path: str, response_edges_ev: list[float]) -> dict:
    """Lower-bound range overall and in the groups at or above the response cut."""
    import numpy as np

    ww = openmc.hdf5_to_wws(path)[0]
    lo = np.asarray(ww.lower_ww_bounds)
    bounds = list(ww.energy_bounds)
    # 0.16.0 stores bounds as (mesh..., groups); find the group axis by its length.
    axis = [i for i, n in enumerate(lo.shape) if n == len(bounds) - 1][-1]
    lo = np.moveaxis(lo, axis, 0).reshape(len(bounds) - 1, -1)
    cut = min(response_edges_ev)
    fast = [g for g in range(len(bounds) - 1) if bounds[g] >= cut * (1 - 1e-9)]
    out = {"overall": _range(lo.ravel()), "response_groups": _range(lo[fast].ravel()), "response_group_indices": fast,
           "per_group": [_range(lo[g]) for g in range(len(bounds) - 1)]}
    return out


def _range(values) -> dict:
    pos = values[values > 0]
    if pos.size == 0:
        return {"min_positive": None, "max": None, "decades": None, "fraction_positive": 0.0}
    return {"min_positive": float(pos.min()), "max": float(pos.max()),
            "decades": float(math.log10(pos.max() / pos.min())), "fraction_positive": float(pos.size / values.size)}


def exceeds(gain: float | None, sigma: float | None, ref: float, ref_sigma: float, z: float = H4_SIGMA) -> bool:
    """Gain above the reference by more than z combined sigma."""
    if gain is None or sigma is None:
        return False
    return gain - ref > z * math.hypot(sigma, ref_sigma)


def h4_rule(rows: dict[str, dict], g6_gain: float = G6_GAIN, g6_sigma: float = G6_GAIN_SIGMA) -> dict:
    """Apply S0d's rules. rows maps T1/T2/T3 to {"gain", "sigma", "unbiased"}; a missing row was not run."""
    def usable(label):
        r = rows.get(label)
        return r is not None and r.get("unbiased") is True and r.get("gain") is not None

    risers = [t for t in ("T1", "T2") if usable(t)
              and exceeds(rows[t]["gain"], rows[t]["sigma"], g6_gain, g6_sigma)
              and rows[t]["gain"] >= H4_MIN_FACTOR * g6_gain]
    t3 = rows.get("T3")
    t3_ran = t3 is not None and t3.get("gain") is not None
    t3_rises = t3_ran and exceeds(t3["gain"], t3["sigma"], g6_gain, g6_sigma)
    evaluated = [t for t in ("T1", "T2") if usable(t)]
    if risers and not t3_rises:
        outcome = "H4 confirmed"
    elif risers and t3_rises:
        outcome = "attribution not established"
    elif evaluated:
        outcome = "H4 rejected"
    else:
        outcome = "NOT_EVALUATED"
    best = max(evaluated, key=lambda t: rows[t]["gain"], default=None)
    gstar = rows[best]["gain"] if best else None
    return {"outcome": outcome, "risers": risers, "t3_ran": t3_ran, "t3_rises": t3_rises, "best": best,
            "g_star": gstar, "nuc050": nuc050_band(gstar)}


def t3_required(t1: dict | None, g6_gain: float = G6_GAIN, g6_sigma: float = G6_GAIN_SIGMA) -> bool:
    """T3 runs only if T1's gain differs from G6's by more than 3 sigma combined."""
    if not t1 or t1.get("gain") is None or t1.get("sigma") is None:
        return False
    return abs(t1["gain"] - g6_gain) > H4_SIGMA * math.hypot(t1["sigma"], g6_sigma)


def nuc050_band(gstar: float | None) -> str:
    if gstar is None:
        return "NOT_EVALUATED"
    if gstar >= NUC050_TARGET:
        return ">= 100: flag cleared on RM-M, subject to an S2 rerun at 1200 s"
    if gstar >= 10.0:
        return "10-100: flag and target kept; G* recorded for change control"
    return "< 10: flag and target kept; G* recorded for change control"


def same_bounds(a, b, rtol: float = 0.0) -> bool:
    """True when two window bound arrays are identical (rtol 0) or equal within rtol."""
    import numpy as np

    a, b = np.asarray(a), np.asarray(b)
    if a.size != b.size:
        return False
    a, b = a.ravel(), b.ravel()
    return bool(np.array_equal(a, b) if rtol == 0.0 else np.allclose(a, b, rtol=rtol, atol=0.0))
