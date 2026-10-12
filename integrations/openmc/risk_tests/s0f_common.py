"""Shared helpers and rules for stage S0f (docs/notes/CAD_TRANSPORT_S0F.md).

`fill_invalid` is pure Python so test_s0f.py can check it without numpy or OpenMC; `fill_file` applies it to a
weight_windows.h5 with h5py, imported only there.
"""
from __future__ import annotations

import math
from pathlib import Path


def neighbours(index: int, dims: tuple[int, int, int], periodic_axis: int | None = None) -> list[int]:
    """Face neighbours of a flat mesh index, in OpenMC's order (axis 0 fastest). `periodic_axis` wraps (a cylinder's phi)."""
    nx, ny, nz = dims
    i, rest = index % nx, index // nx
    j, k = rest % ny, rest // ny
    out = []
    for axis, (c, n) in enumerate(((i, nx), (j, ny), (k, nz))):
        for step in (-1, 1):
            m = c + step
            if axis == periodic_axis:
                if n < 3 and m % n == c:
                    continue
                m %= n
            elif not 0 <= m < n:
                continue
            p = [i, j, k]
            p[axis] = m
            out.append(p[0] + nx * (p[1] + ny * p[2]))
    return sorted(set(out))


def fill_invalid(values: list[float], dims: tuple[int, int, int], periodic_axis: int | None = None) -> tuple[list[float], int]:
    """Give every bin without a window (bound <= 0) the geometric mean of its valid face neighbours, in sweeps.

    Each sweep uses only the bins valid at its start, so the result does not depend on the order of the bins. Sweeps
    repeat until no invalid bin is left. A group with no valid bin, or none invalid, is returned unchanged.
    Returns (values, number of bins filled).
    """
    if len(values) != dims[0] * dims[1] * dims[2]:
        raise ValueError("values do not match the mesh dimensions")
    out = list(values)
    if all(v > 0 for v in out) or not any(v > 0 for v in out):
        return out, 0
    filled = 0
    todo = [n for n, v in enumerate(out) if v <= 0]
    while todo:
        updates = {}
        for n in todo:
            logs = [math.log(out[m]) for m in neighbours(n, dims, periodic_axis) if out[m] > 0]
            if logs:
                updates[n] = math.exp(sum(logs) / len(logs))
        if not updates:  # an invalid region with no valid bin anywhere around it (cannot happen when one bin is valid)
            raise ValueError("invalid bins with no valid neighbour")
        for n, v in updates.items():
            out[n] = v
        filled += len(updates)
        todo = [n for n in todo if n not in updates]
    return out, filled


def fill_file(src: Path, dst: Path) -> dict:
    """Copy a weight_windows.h5 and fill its invalid bins in every energy group that has any valid bin.

    Upper bounds are set to the file's own upper/lower ratio times the filled lower bound. Returns a summary.
    """
    import shutil

    import h5py

    shutil.copyfile(src, dst)
    with h5py.File(dst, "r+") as f:
        meshes = f["meshes"]
        mesh = meshes[list(meshes)[0]]
        if "r_grid" in mesh:
            dims = (len(mesh["r_grid"]) - 1, len(mesh["phi_grid"]) - 1, len(mesh["z_grid"]) - 1)
            periodic = 1
        else:
            dims = tuple(int(x) for x in mesh["dimension"][()])
            periodic = None
        group = f["weight_windows"][list(f["weight_windows"])[0]]
        lower = group["lower_ww_bounds"][()]
        upper = group["upper_ww_bounds"][()]
        ratios = sorted(float(u / l) for u, l in zip(upper.ravel(), lower.ravel()) if l > 0)
        ratio = ratios[len(ratios) // 2]
        per_group = []
        for e in range(lower.shape[0]):
            values, n = fill_invalid([float(v) for v in lower[e]], dims, periodic)
            if n:
                lower[e] = values
                upper[e] = [v * ratio for v in values]
            per_group.append(n)
        group["lower_ww_bounds"][...] = lower
        group["upper_ww_bounds"][...] = upper
    return {"source": str(src), "filled": str(dst), "dims": list(dims), "periodic_axis": periodic, "ratio": ratio,
            "bins_filled_per_group": per_group}


# ---------------------------------------------------------------------------
# S0f production: aperture-biased source against the current windows (protocol in CAD_TRANSPORT_S0F.md, "Protocol").

D2_FOM = 0.03318235249141742          # S0b D2 analog: FOM of (a) and its sigma (results-s0b.json)
D2_FOM_SIGMA = 0.0008498523078389429
D2_MEAN = 0.0010706697372987956       # S0b D2 analog (a), cm per source neutron, and its standard error
D2_STD_ERROR = 6.931317870952788e-05
Z_UNBIASED = 3.0
NUC050_TARGET = 100.0
H7_MIN_FACTOR = 1.2
VOV_LIMIT = 0.1
T_ONE_SIDED_95_DF3 = 2.353            # Student t, one-sided 95 %, 3 degrees of freedom (4 seeds)
APERTURE = "0.1 578 0 0 0 22 0 0 0 22"  # beta, centre, half-edge u, half-edge v (cm)
SEEDS = [77100001, 77100002, 77100003, 77100004]
PER_BATCH = 1000
BATCHES = 3000


def batch_stats(values: list[float], per_batch: int) -> dict:
    """Mean, relative error, variance per history (batch variance x per_batch / mean^2) and VOV of batch means.

    VOV = sum((x-m)^4) / (sum((x-m)^2))^2 - 1/n, as MCNP's, on batch means.
    """
    n = len(values)
    if n < 2:
        raise ValueError("at least two batches are needed")
    m = sum(values) / n
    d2 = [(x - m) ** 2 for x in values]
    s2 = sum(d2)
    var = s2 / (n - 1)
    if m <= 0.0 or s2 <= 0.0:
        return {"batches": n, "mean": m, "relative_error": None, "var_per_history": None, "vov": None}
    r = math.sqrt(var / n) / m
    vov = sum(x * x for x in d2) / (s2 * s2) - 1.0 / n
    return {"batches": n, "mean": m, "std_error": r * m, "relative_error": r, "var_per_history": var * per_batch / (m * m),
            "vov": vov}


def fom(relative_error: float | None, seconds: float) -> float | None:
    if relative_error is None or relative_error <= 0.0 or seconds <= 0.0:
        return None
    return 1.0 / (relative_error ** 2 * seconds)


def z_against_d2(mean: float, std_error: float) -> float:
    return (mean - D2_MEAN) / math.hypot(std_error, D2_STD_ERROR)


def seed_lower_bound(gains: list[float]) -> dict:
    """Mean of per-seed gains and its one-sided 95 % lower bound (Student t, 4 seeds)."""
    n = len(gains)
    if n != 4:
        raise ValueError("the protocol's bound is for 4 seeds")
    m = sum(gains) / n
    sd = math.sqrt(sum((g - m) ** 2 for g in gains) / (n - 1))
    return {"mean": m, "sd": sd, "lower95": m - T_ONE_SIDED_95_DF3 * sd / math.sqrt(n)}


def nuc050_outcome(gain: float | None, lower: float | None, unbiased: bool | None) -> str:
    """F1's NUC-050 band from its pooled gain G and conservative lower bound L."""
    if gain is None or lower is None or unbiased is None:
        return "NOT_EVALUATED"
    if unbiased is not True:
        return "biased: aperture biasing not adopted; investigate before any further use"
    if lower >= NUC050_TARGET:
        return ">= 100 at the 95 % lower bound: NUC-050 met on RM-M with a hand-placed aperture"
    if gain >= NUC050_TARGET:
        return ">= 100 at the point estimate only: more replicates before any claim"
    if gain >= 10.0:
        return "10-100: target kept; next lever is the far-wall scatter class"
    return "< 10: aperture biasing does not carry the gain seen in the diagnostic scan"


def h7_outcome(paired_ratios: list[float], pooled_ratio: float | None) -> str:
    """H7: F1 beats F0 when every paired per-seed FOM ratio exceeds 1 and the pooled ratio is at least 1.2."""
    if pooled_ratio is None or len(paired_ratios) != 4:
        return "NOT_EVALUATED"
    return "H7 confirmed" if all(r > 1.0 for r in paired_ratios) and pooled_ratio >= H7_MIN_FACTOR else "H7 rejected"


# Protocol 2 (CAD_TRANSPORT_S0F.md, "Protocol 2"): F4 adds collision aiming to F1; new seeds.
COLLISION_AIM = "0.2 *"               # beta for elastic collisions in every material
SEEDS2 = [77200001, 77200002, 77200003, 77200004]
# Protocol 3: F5 adds the duct inlet (x 446 cm, the duct's 15 cm half-widths) as a second aperture rectangle.
DUCT_INLET = "446 0 0 0 15 0 0 0 15"
# Protocol 3: F6 makes both rectangles one-sided (aimed at only from the plasma side); new seeds.
SEEDS3 = [77300001, 77300002, 77300003, 77300004]
# Protocol 4: F8 = F6 on reciprocity-capped windows; new seeds.
SEEDS4 = [77400001, 77400002, 77400003, 77400004]
CONFIG_ENV = {
    "F0": {},
    "F1": {"OPENMC_WW_SKIP_VOID": "1", "OPENMC_APERTURE_BIAS": APERTURE},
    "F4": {"OPENMC_WW_SKIP_VOID": "1", "OPENMC_APERTURE_BIAS": APERTURE, "OPENMC_COLLISION_AIM": COLLISION_AIM},
    "F5": {"OPENMC_WW_SKIP_VOID": "1", "OPENMC_APERTURE_BIAS": APERTURE + "; " + DUCT_INLET,
           "OPENMC_COLLISION_AIM": COLLISION_AIM},
    "F6": {"OPENMC_WW_SKIP_VOID": "1", "OPENMC_APERTURE_BIAS": APERTURE + "; " + DUCT_INLET,
           "OPENMC_APERTURE_ONE_SIDED": "1", "OPENMC_COLLISION_AIM": COLLISION_AIM},
}
# F7 = F6's variables on the line-of-sight-corrected windows (s0f_los_windows.py); same environment as F6.
CONFIG_ENV["F7"] = dict(CONFIG_ENV["F6"])
# F8 = F6's variables on windows capped by reciprocity runs from the response outward (fact 13).
CONFIG_ENV["F8"] = dict(CONFIG_ENV["F6"])
PATCH_VARIABLES = ("OPENMC_WW_SKIP_VOID", "OPENMC_APERTURE_BIAS", "OPENMC_SPLIT_BEFORE_COLLISION",
                   "OPENMC_COLLISION_AIM", "OPENMC_WW_ROULETTE_FACTOR", "OPENMC_APERTURE_ONE_SIDED")


def env_matches(config: str, env: dict) -> bool:
    """The FARIS patch variables set are exactly the configuration's."""
    want = CONFIG_ENV[config]
    return all(env.get(k) == want.get(k) for k in PATCH_VARIABLES)


def nuc050_outcome2(gain: float | None, lower: float | None, unbiased: bool | None) -> str:
    """Protocol 2: F4's NUC-050 band from its pooled gain G and conservative lower bound L."""
    if gain is None or lower is None or unbiased is None:
        return "NOT_EVALUATED"
    if unbiased is not True:
        return "biased: collision aiming not adopted; find the bias before any further use"
    if lower >= NUC050_TARGET:
        return ">= 100 at the 95 % lower bound: NUC-050 met on RM-M with a hand-placed aperture"
    if gain >= NUC050_TARGET:
        return ">= 100 at the point estimate only: more replicates before any claim"
    if gain >= 10.0:
        return "10-100: target kept; trace the top batches of F4 for the class that remains"
    return "< 10: collision aiming does not carry the gain seen in the diagnostic scan"


def h8_outcome(paired_ratios: list[float], pooled_ratio: float | None) -> str:
    """H8: F4 beats F0, by the same rule as H7."""
    return h7_outcome(paired_ratios, pooled_ratio).replace("H7", "H8")


def h9_outcome(paired_ratios: list[float], pooled_ratio: float | None) -> str:
    """H9: F6 beats F0, by the same rule as H7."""
    return h7_outcome(paired_ratios, pooled_ratio).replace("H7", "H9")


def nuc050_outcome3(gain: float | None, lower: float | None, unbiased: bool | None) -> str:
    """Protocol 3: F6's NUC-050 band, by protocol 2's rule."""
    return nuc050_outcome2(gain, lower, unbiased).replace("of F4", "of F6")


def h10_outcome(paired_ratios: list[float], pooled_ratio: float | None) -> str:
    """H10: F8 beats F0, by the same rule as H7."""
    return h7_outcome(paired_ratios, pooled_ratio).replace("H7", "H10")


def nuc050_outcome4(gain: float | None, lower: float | None, unbiased: bool | None) -> str:
    """Protocol 4: F8's NUC-050 band, by protocol 2's rule."""
    return nuc050_outcome2(gain, lower, unbiased).replace("of F4", "of F8").replace(
        "collision aiming not adopted", "capped windows with aiming not adopted")
