"""Pure helpers for the S0c stand-in-model tests (docs/notes/CAD_TRANSPORT_S0C.md). Standard library only.

The P1 slab (inboard radial build of RM-M as layers normal to x), the source-region count, the window floor, the
resolution rule (delta*), the floor rule, the Q2 preview rule, the H2 rule for Q3, and the time-box ledger. Statistics
(FOM, gain, z, ratio sigma) come from s0b_common and are not redefined here.
"""
from __future__ import annotations

import json
import math
from pathlib import Path
import time

import rm_m_spec as spec
import s0b_common as C

TIME_BOX_S = 3600.0
Q2_BUDGET_S = 360.0
Q3_BUDGET_S = 1200.0

# Q1
SPACINGS_CM = [25.0, 10.0, 5.0, 2.5]
SLAB_WIDTH_CM = 100.0          # y and z, reflective
SLAB_TOTAL_CM = 200.0          # x from 0 (plasma end) to 200 (beyond the TF leg); both x ends vacuum
ANALOG_R_STOP = 0.05
ANALOG_BUDGET_S = 600.0
PRODUCTION_S = 120.0
FLOOR_REL = 1.0e-8
MAX_SPLIT = 1000
FIRST_WALL_FAST_CUT_EV = 1.0e5
SOURCE_ENERGY_EV = 14.1e6
RM_M_BASE_CM = 25.0
Z_UNBIASED = C.Z_UNBIASED

# Seeds: new, disjoint from S0 (8115xxxx, 2026xxxx) and S0b (900xxxxx)
SEEDS = {"mgxs": 91010000, "analog": 91020000,
         "gen": {25.0: 91030025, 10.0: 91030010, 5.0: 91030005, 2.5: 91030002},
         "prod": {25.0: 91040025, 10.0: 91040010, 5.0: 91040005, 2.5: 91040002, "floor": 91040099},
         "q2": 91060001, "q3_d1s": 91070001, "q3_neutron": 91070010, "q3_photon": 91070020}

# Q3 (P3)
P3_THICKNESS_CM = 40.0
P3_DETECTOR_CM = 10.0
P3_VOXELS_CM = [20.0, 10.0, 2.0]
P3_CELL_CM = 2.0
P3_COOLING_INDEX = 2
P3_TIMESTEPS = [(1.0, "a"), (1.0, "d")]
P3_SOURCE_RATES = [1.0, 0.0]
H2_SIGMA = 3.0
H2_MIN_EXCESS = 0.05
H2_NEAR_ONE = 0.05


# ---------------------------------------------------------------------------
# P1 slab

PLASMA_LAYER_NAME = "plasma_source"


def p1_layers() -> list[dict]:
    """Layers of P1 from the plasma end: RM-M's inboard radial build (rm_m_spec.LAYERS 'in' values, gap, TF leg).

    The plasma-side layer is vacuum and carries the source; its thickness fills the 200 cm slab. Each layer has name, tag
    (material, 'void' for vacuum), thickness_cm, x0_cm and x1_cm.
    """
    built = [(layer["name"], layer["tag"], layer["in"]["value"]) for layer in spec.LAYERS]
    built.append(("gap_to_tf", "void", spec.GAP_TO_TF["value"]))
    built.append(("tf_inboard_leg", "tf_coil", spec.TF_RADIAL_CM["value"]))
    build_total = sum(t for _, _, t in built)
    plasma = SLAB_TOTAL_CM - build_total
    if plasma <= 0.0:
        raise ValueError("the radial build does not fit the slab")
    out, x = [], 0.0
    for name, tag, t in [(PLASMA_LAYER_NAME, "void", plasma)] + built:
        out.append({"name": name, "tag": tag, "thickness_cm": t, "x0_cm": x, "x1_cm": x + t})
        x += t
    return out


def layer_edges(layers: list[dict]) -> list[float]:
    return [layers[0]["x0_cm"]] + [layer["x1_cm"] for layer in layers]


def mesh_dimension(total_cm: float, spacing_cm: float) -> int:
    n = round(total_cm / spacing_cm)
    if abs(n * spacing_cm - total_cm) > 1.0e-9:
        raise ValueError(f"spacing {spacing_cm} does not divide {total_cm}")
    return int(n)


def source_region_count(layer_edges_cm: list[float], spacing_cm: float, total_cm: float | None = None) -> int:
    """Source regions of a slab with x layers split by a regular x mesh (one cell in y and z): the pieces of the union of the
    layer boundaries and the mesh edges that have positive length."""
    total = layer_edges_cm[-1] if total_cm is None else total_cm
    n = mesh_dimension(total, spacing_cm)
    cuts = sorted({round(e, 9) for e in layer_edges_cm} | {round(i * spacing_cm, 9) for i in range(n + 1)})
    return len(cuts) - 1


def source_region_estimate_3d(volumes_cm3: list[float], spacing_cm: float, cells_per_volume_edge_fudge: float = 1.0) -> float:
    """Rough source-region count of a 3-D mesh of the given spacing over domains of the given volumes (volume / cell volume,
    each domain at least one region). A lower bound: regions cut by domain boundaries add to it."""
    cell = spacing_cm ** 3
    return sum(max(1.0, v / cell * cells_per_volume_edge_fudge) for v in volumes_cm3)


# ---------------------------------------------------------------------------
# Window floor and range

def apply_floor(lower: list[float], upper: list[float], rel: float = FLOOR_REL) -> dict:
    """Raise every positive lower bound below rel x (largest lower bound) to that value; upper bounds scale by the same factor.

    Non-positive bounds are OpenMC's 'no window' marker and are left alone (counted). Returns the new lists and the counts.
    """
    if len(lower) != len(upper):
        raise ValueError("lower and upper bounds differ in length")
    top = max(lower, default=0.0)
    floor = rel * top
    new_lo, new_up, raised, nonpos = [], [], 0, 0
    for lo, up in zip(lower, upper):
        if lo <= 0.0:
            nonpos += 1
            new_lo.append(lo)
            new_up.append(up)
        elif lo < floor:
            raised += 1
            new_lo.append(floor)
            new_up.append(up * (floor / lo))
        else:
            new_lo.append(lo)
            new_up.append(up)
    return {"lower": new_lo, "upper": new_up, "floor_value": floor, "raised": raised, "non_positive": nonpos, "cells": len(lower)}


def window_range(lower: list[float]) -> dict:
    pos = [v for v in lower if v > 0.0]
    if not pos:
        return {"min_positive": None, "max": None, "dynamic_range": None, "fraction_positive": 0.0}
    return {"min_positive": min(pos), "max": max(pos), "dynamic_range": max(pos) / min(pos), "fraction_positive": len(pos) / len(lower)}


# ---------------------------------------------------------------------------
# Q1 rules

def extrapolate_cost(spacings_cm: list[float], seconds: list[float], target_cm: float) -> float:
    """Cost at a finer spacing from a power law in the number of mesh cells, fitted through the last two measured points."""
    if len(spacings_cm) < 2 or len(spacings_cm) != len(seconds):
        raise ValueError("need two measured points")
    (s0, s1), (t0, t1) = spacings_cm[-2:], seconds[-2:]
    p = math.log(t1 / t0) / math.log(s0 / s1)
    return t1 * (s1 / target_cm) ** p


def row_unbiased(a_mean: float, a_se: float, ref_mean: float, ref_se: float) -> dict:
    z = C.z_score(a_mean, a_se, ref_mean, ref_se)
    return {"z": z, "unbiased": C.unbiased(z)}


def delta_star(rows: list[dict]) -> dict:
    """Resolution needed: the coarsest spacing whose FOM is within 1 sigma (combined) of the best FOM in the sweep.

    rows: {spacing, fom, sigma, unbiased}. Configurations that are not unbiased (False) are excluded; None counts as not
    evaluated and is excluded too.
    """
    ok = [r for r in rows if r.get("unbiased") is True and r.get("fom") is not None]
    if not ok:
        return {"delta_star_cm": None, "best_spacing_cm": None, "note": "no unbiased configuration with a FOM"}
    best = max(ok, key=lambda r: r["fom"])
    within = []
    for r in ok:
        gap = best["fom"] - r["fom"]
        comb = math.hypot(best["sigma"], r["sigma"]) if r is not best else 0.0
        if gap <= C.FOM_SIGMA_Z * comb or r is best:
            within.append(r["spacing"])
    return {"delta_star_cm": max(within), "best_spacing_cm": best["spacing"], "best_fom": best["fom"], "within_one_sigma_of_best": sorted(within),
            "excluded": [r["spacing"] for r in rows if r not in ok]}


def floor_rule(fom_ref: dict, fom_floor: dict, unbiased: bool | None) -> dict:
    """The floor is adopted if it raises the FOM by more than the combined 1 sigma and stays unbiased."""
    improved = C.improved_over(fom_floor, fom_ref)
    return {"improved": bool(improved), "unbiased": unbiased, "adopted": bool(improved and unbiased is True)}


def rm_m_confirmation_required(delta_star_cm: float | None, base_cm: float = RM_M_BASE_CM) -> bool:
    return delta_star_cm is not None and delta_star_cm < base_cm


# ---------------------------------------------------------------------------
# Q2

def q2_rule(runs: list[dict]) -> dict:
    """If both warm previews give TBR R <= 2 % and (a) R <= 20 %, the PERF-025 numbers stand with the preview defined in a
    loaded session (cold start reported separately); otherwise the measured errors and the breakdown go to change control."""
    r = C.d4_rule(runs)
    return {**r, "outcome": "keep numbers, preview defined in a loaded session" if r["perf_025_keeps_numbers"] else "to change control"}


# ---------------------------------------------------------------------------
# Q3 H2

def ratio_row(num: float, num_r: float, den: float, den_r: float) -> dict:
    """Ratio num/den from values and their relative errors."""
    if den == 0.0 or num == 0.0:
        return {"ratio": None, "sigma": None}
    sigma = C.ratio_sigma(num, num_r * num, den, den_r * den)
    return {"ratio": num / den, "sigma": sigma}


def h2_rule(mesh: dict[float, dict], cell: dict) -> dict:
    """H2 rule. mesh: voxel cm -> {ratio, sigma} of mesh R2S / D1S; cell: {ratio, sigma} of cell-based R2S / D1S.

    Confirmed: the 10 cm ratio exceeds 1 by more than 3 sigma and 5 %, the ratio falls toward 1 as voxels shrink
    (r20 > r10 > r2), and the 2 cm mesh and the cell-based ratios are each within 5 % of 1.
    Rejected: the 10 cm ratio is within 5 % of 1, or the trend is not monotone. Otherwise the rule gives neither.
    """
    r20, r10, r2 = (mesh[v]["ratio"] for v in (20.0, 10.0, 2.0))
    if None in (r20, r10, r2, cell["ratio"]):
        return {"outcome": "NOT_EVALUATED", "reason": "a ratio is missing"}
    s10 = mesh[10.0]["sigma"]
    excess = r10 - 1.0
    high = excess > H2_SIGMA * s10 and excess > H2_MIN_EXCESS
    monotone = r20 > r10 > r2
    near = abs(r10 - 1.0) <= H2_NEAR_ONE + 1e-12
    fine_ok = abs(r2 - 1.0) <= H2_NEAR_ONE + 1e-12
    cell_ok = abs(cell["ratio"] - 1.0) <= H2_NEAR_ONE + 1e-12
    detail = {"r20": r20, "r10": r10, "r2": r2, "r_cell": cell["ratio"], "excess_10": excess, "sigma_10": s10,
              "ten_cm_high": bool(high), "monotone_falling": bool(monotone), "ten_cm_within_5pct_of_1": bool(near),
              "two_cm_within_5pct": bool(fine_ok), "cell_within_5pct": bool(cell_ok)}
    if high and monotone and fine_ok and cell_ok:
        return {"outcome": "CONFIRMED", **detail}
    if near or not monotone:
        return {"outcome": "REJECTED", **detail}
    return {"outcome": "NEITHER", **detail}


def subset_standard_error(subset_values: list[float], n_total: int) -> dict:
    """Standard error of the mean of n_total independent batches estimated from the spread of a subset of them.

    se = sd(subset) / sqrt(n_total); the relative error of the sd estimate is about 1 / sqrt(2 (k - 1)) for k values.
    """
    k = len(subset_values)
    if k < 2 or n_total < k:
        raise ValueError("need at least two subset values and n_total >= k")
    m = math.fsum(subset_values) / k
    sd = math.sqrt(math.fsum((v - m) ** 2 for v in subset_values) / (k - 1))
    return {"sd": sd, "std_error": sd / math.sqrt(n_total), "subset": k, "n_total": n_total, "relative_error_of_sd": 1.0 / math.sqrt(2.0 * (k - 1))}


# ---------------------------------------------------------------------------
# Time box ledger

def ledger_init(path: Path, cap_s: float = TIME_BOX_S) -> dict:
    data = {"cap_s": cap_s, "entries": [], "job_wall_total_s": 0.0, "start_epoch": time.time(),
            "start_iso": time.strftime("%Y-%m-%dT%H:%M:%S%z")}
    Path(path).write_text(json.dumps(data, indent=1) + "\n", encoding="utf-8")
    return data


def budget_left(ledger_path: Path) -> float:
    data = json.loads(Path(ledger_path).read_text(encoding="utf-8"))
    return data.get("cap_s", TIME_BOX_S) - sum(e["wall_s"] for e in data["entries"])


def fits(estimate_s: float, left_s: float, reserve_s: float = 0.0) -> bool:
    return estimate_s + reserve_s <= left_s
