"""R1 configurations, seeds, mesh definitions and figures of merit (standard library only).

The six configurations were declared before any R1 run and are not changed here:

    C1  1 group,    objective (a),     default windows
    C2  CASMO-8,    objective (a),     default windows
    C3  CASMO-25,   objective (a),     default windows
    C4  CASMO-8,    objective (a)+(b), default windows
    C5  CASMO-8,    objective (a),     upper/lower ratio 10 and survival ratio 5
    C6  CASMO-25,   objective (a)+(b), default windows

Response (a): TF fast flux (E > 0.1 MeV), integrated over all TF coil volumes.
Response (b): maximum over a cylindrical mesh on the inboard TF legs at the midplane,
5 cm radial x 10 cm vertical x full coil toroidal width.
"""
from __future__ import annotations

import math

import rm_m_spec as spec

# Group structures. CASMO-8 and CASMO-25 are openmc.mgxs.GROUP_STRUCTURES entries (eV, ascending).
# The lowest edge of the CASMO structures is 0; it is kept as is.
CASMO_8 = [0.0, 0.058, 0.14, 0.28, 0.625, 4.0, 5530.0, 821000.0, 20000000.0]
CASMO_25 = [0.0, 0.03, 0.058, 0.14, 0.28, 0.35, 0.625, 0.972, 1.02, 1.097, 1.15, 1.855, 4.0, 9.877, 15.968, 148.73,
            5530.0, 9118.0, 111000.0, 500000.0, 821000.0, 1353000.0, 2231000.0, 3679000.0, 6065500.0, 20000000.0]
ONE_GROUP = [1.0e-5, 2.0e7]
STRUCTURES = {"1": ONE_GROUP, "CASMO-8": CASMO_8, "CASMO-25": CASMO_25}

FAST_CUT_EV = 1.0e5
DEFAULT_RATIO = 5.0      # upper/lower bound ratio of generated windows (read from the generated file)
DEFAULT_SURVIVAL = 3.0   # openmc.WeightWindows default survival_ratio

CONFIGS = {
    "C1": {"structure": "1", "objective": "a", "upper_lower_ratio": None, "survival_ratio": None},
    "C2": {"structure": "CASMO-8", "objective": "a", "upper_lower_ratio": None, "survival_ratio": None},
    "C3": {"structure": "CASMO-25", "objective": "a", "upper_lower_ratio": None, "survival_ratio": None},
    "C4": {"structure": "CASMO-8", "objective": "a+b", "upper_lower_ratio": None, "survival_ratio": None},
    "C5": {"structure": "CASMO-8", "objective": "a", "upper_lower_ratio": 10.0, "survival_ratio": 5.0},
    "C6": {"structure": "CASMO-25", "objective": "a+b", "upper_lower_ratio": None, "survival_ratio": None},
}

# Generation seeds and production seeds are disjoint (NUC-053).
GENERATION_SEEDS = {"C1": 81150101, "C2": 81150102, "C3": 81150103, "C4": 81150104, "C5": 81150105, "C6": 81150106}
PRODUCTION_SEEDS = {"C1": 20261101, "C2": 20261102, "C3": 20261103, "C4": 20261104, "C5": 20261105, "C6": 20261106}
ANALOG_SEED = 20261100
MGXS_SEED = 81150100

PRODUCTION_BUDGET_S = 1800.0
ANALOG_BUDGET_S = 7200.0
R_LIMIT = 0.1
GAIN_TARGET = 100.0


def fast_response_edges(structure: str) -> list[float]:
    """Energy bins of the adjoint response: the groups whose lower edge is at or above 0.1 MeV.

    A multigroup tally must follow group edges, so 0.1 MeV itself is only reproduced when it is an edge.
    CASMO-8 gives E > 0.821 MeV and CASMO-25 gives E > 0.111 MeV; the one-group structure gives the whole range.
    The production tally always uses the exact E > 0.1 MeV.
    """
    edges = STRUCTURES[structure]
    if len(edges) == 2:
        return list(edges)
    keep = [e for e in edges if e >= FAST_CUT_EV]
    return keep


def objective_includes_b(config: str) -> bool:
    return CONFIGS[config]["objective"] == "a+b"


def disjoint_seeds() -> bool:
    gen = set(GENERATION_SEEDS.values()) | {MGXS_SEED}
    prod = set(PRODUCTION_SEEDS.values()) | {ANALOG_SEED}
    return not (gen & prod) and len(gen) == len(GENERATION_SEEDS) + 1 and len(prod) == len(PRODUCTION_SEEDS) + 1


# ---------------------------------------------------------------------------
# Response (b) mesh

PEAK_MESH = {"r_cm": (60.0, 135.0, 5.0), "z_cm": (-50.0, 50.0, 10.0)}


def peak_mesh_grids() -> dict:
    """Cylindrical grids of the (b) mesh. phi bins alternate coil and gap, starting with a coil; only coil bins score."""
    r0, r1, dr = PEAK_MESH["r_cm"]
    z0, z1, dz = PEAK_MESH["z_cm"]
    r_grid = [r0 + i * dr for i in range(int(round((r1 - r0) / dr)) + 1)]
    z_grid = [z0 + i * dz for i in range(int(round((z1 - z0) / dz)) + 1)]
    half = spec.TF_TOROIDAL_SPAN_DEG["value"] / 2.0
    # each coil's two edges in order; consecutive edges alternate coil bin, gap bin, so coil k is phi bin 2k
    phi = []
    for centre in spec.coil_centres_deg():
        phi += [centre - half, centre + half]
    coil_bins = [2 * k for k in range(len(phi) // 2)]
    return {"r_grid_cm": r_grid, "z_grid_cm": z_grid, "phi_grid_deg": phi, "coil_phi_bins": coil_bins}


# ---------------------------------------------------------------------------
# Metrics

def relative_error(mean: float, std_error: float) -> float | None:
    return std_error / mean if mean > 0.0 else None


def figure_of_merit(r: float | None, seconds: float) -> float | None:
    """FOM = 1 / (R^2 T)."""
    if r is None or r <= 0.0 or seconds <= 0.0:
        return None
    return 1.0 / (r * r * seconds)


def z_score(mean_a: float, se_a: float, mean_b: float, se_b: float) -> float | None:
    sd = math.hypot(se_a, se_b)
    return (mean_a - mean_b) / sd if sd > 0.0 else None


def gain(fom: float | None, fom_analog: float | None) -> float | None:
    if fom is None or fom_analog is None or fom_analog <= 0.0:
        return None
    return fom / fom_analog


def analog_status(r_analog: float | None) -> dict:
    """The analog FOM counts only if R <= 0.1 within the budget; otherwise the gain is a lower bound."""
    counts = r_analog is not None and r_analog <= R_LIMIT
    return {"analog_r": r_analog, "analog_fom_counts": counts, "gain_is_lower_bound": not counts,
            "unbiasedness": "evaluated" if counts else "NOT_EVALUATED"}


def r1_verdict(gains: dict[str, float | None], status: dict) -> dict:
    """PASS if the best gain on (a) is at least 100x; with an analog R above 0.1 the gain is a labelled lower bound."""
    valid = {k: v for k, v in gains.items() if v is not None}
    if not valid:
        return {"verdict": "NOT_EVALUATED", "best_config": None, "best_gain": None}
    best = max(valid, key=valid.get)
    passed = valid[best] >= GAIN_TARGET
    return {"verdict": "PASS" if passed else "FAIL", "best_config": best, "best_gain": valid[best],
            "gain_is_lower_bound": status["gain_is_lower_bound"]}


def summarise(productions: dict, analog: dict) -> dict:
    """Per-configuration FOM, gain and agreement against the analog reference.

    Each production record needs response.a {mean, std_error, relative_error}, response.seconds_to_statepoint and
    response.b (peak voxel) as written by r1_worker.production.
    """
    ra = analog["response"]
    r_analog = ra["a"]["relative_error"]
    t_analog = ra["seconds_to_statepoint"]
    fom_analog = figure_of_merit(r_analog, t_analog)
    status = analog_status(r_analog)
    rows, gains = {}, {}
    for name, rec in productions.items():
        resp = rec["response"]
        a = resp["a"]
        t = resp["seconds_to_statepoint"]
        fom = figure_of_merit(a["relative_error"], t)
        g = gain(fom, fom_analog)
        gains[name] = g
        rows[name] = {"R_a": a["relative_error"], "T_seconds": t, "histories": resp["histories"], "FOM_a": fom, "gain_vs_analog": g,
                      "a_mean": a["mean"], "a_std_error": a["std_error"],
                      "z_vs_analog": z_score(a["mean"], a["std_error"], ra["a"]["mean"], ra["a"]["std_error"]),
                      "b": resp["b"], "b_nonzero_voxels": resp.get("b_nonzero_voxels")}
    verdict = r1_verdict(gains, status)
    return {"analog": {"R_a": r_analog, "T_seconds": t_analog, "histories": ra["histories"], "FOM_a": fom_analog, "a_mean": ra["a"]["mean"],
                       "a_std_error": ra["a"]["std_error"], "b": ra["b"], **status},
            "configs": rows, "verdict": verdict}
