"""R3 scenario constants and pure helpers (standard library only).

Scored region (authored): a regular mesh of 10 cm cubes over the equatorial port duct and the adjacent plasma chamber.
The port sits at 0 degrees on the outboard midplane. Its mouth is the outer surface of the scrape-off layer at
R = 330 + 113 + 3 = 446 cm and the duct ends at the outer face of the TiH2 shield, R = 443 + 137 = 580 cm.
The mesh starts 100 cm in front of the mouth and covers the full port width and height plus 20 cm on each side.
"""
from __future__ import annotations

import math

import rm_m_spec as spec

SECONDS_PER_YEAR = 365.25 * 86400.0
SECONDS_PER_HOUR = 3600.0

PORT_MOUTH_X_CM = spec.MAJOR_RADIUS_CM + spec.MINOR_RADIUS_CM + 3.0
DUCT_END_X_CM = PORT_MOUTH_X_CM + 134.0
MESH_LOWER_CM = (PORT_MOUTH_X_CM - 100.0, -(spec.PORT_WIDTH_CM["value"] / 2 + 20.0), -(spec.PORT_HEIGHT_CM["value"] / 2 + 20.0))
MESH_VOXEL_CM = 10.0
MESH_DIMENSION = (24, 7, 7)
MESH_UPPER_CM = tuple(lo + n * MESH_VOXEL_CM for lo, n in zip(MESH_LOWER_CM, MESH_DIMENSION))

# One constant-power year, then decay; photon transport at the three cooling times.
TIMESTEPS = [(1.0, "a"), (1.0, "d"), (6.0, "d"), (23.0, "d")]
COOLING_LABELS = {2: "1 d", 3: "7 d", 4: "30 d"}
PHOTON_TIME_INDICES = [2, 3, 4]
COOLING_SECONDS = {2: 86400.0, 3: 7 * 86400.0, 4: 30 * 86400.0}
SOURCE_RATES = [spec.SOURCE_RATE_N_S, 0.0, 0.0, 0.0]

PSV_PER_USV = 1.0e6
RATIO_BAND = (0.85, 1.15)
VOXEL_R_LIMIT = 0.10
CONSERVATION_LIMIT = 1.0e-6
ACTIVATION_GROUPS = "VITAMIN-J-42"


def mesh_volume_cm3() -> float:
    n = MESH_DIMENSION[0] * MESH_DIMENSION[1] * MESH_DIMENSION[2]
    return n * MESH_VOXEL_CM ** 3


def dose_rate_usv_per_h(pSv_cm_per_s_per_voxel: float, voxel_volume_cm3: float = MESH_VOXEL_CM ** 3) -> float:
    """Volume-averaged dose rate in a voxel from a track-length-weighted tally (pSv.cm/s) divided by the voxel volume."""
    return pSv_cm_per_s_per_voxel / voxel_volume_cm3 * SECONDS_PER_HOUR / PSV_PER_USV


def ratio_summary(d1s: list[float], r2s: list[float], d1s_r: list[float], r2s_r: list[float], limit: float = VOXEL_R_LIMIT) -> dict:
    """Distribution of D1S/R2S over voxels where both methods have relative error <= limit and a positive value."""
    ratios = []
    for a, b, ra, rb in zip(d1s, r2s, d1s_r, r2s_r):
        if a > 0.0 and b > 0.0 and ra is not None and rb is not None and ra <= limit and rb <= limit:
            ratios.append(a / b)
    out = {"voxels_used": len(ratios), "voxels_total": len(d1s), "limit_r": limit}
    if not ratios:
        out.update({"median": None, "p05": None, "p95": None})
        return out
    ratios.sort()

    def pct(p):
        k = (len(ratios) - 1) * p / 100.0
        lo, hi = int(math.floor(k)), int(math.ceil(k))
        return ratios[lo] + (ratios[hi] - ratios[lo]) * (k - lo)

    out.update({"median": pct(50.0), "p05": pct(5.0), "p95": pct(95.0)})
    return out


def band_check(ratio: float | None, band: tuple[float, float] = RATIO_BAND) -> bool:
    return ratio is not None and band[0] <= ratio <= band[1]


def conservation(source_strength: float, inventory_emission: float, limit: float = CONSERVATION_LIMIT) -> dict:
    """ACT-023: the decay-photon source handed to transport must equal the inventory's photon emission to `limit`."""
    if inventory_emission == 0.0:
        rel = 0.0 if source_strength == 0.0 else math.inf
    else:
        rel = abs(source_strength - inventory_emission) / abs(inventory_emission)
    return {"source_photons_per_s": source_strength, "inventory_photons_per_s": inventory_emission,
            "relative_difference": float(rel), "limit": limit, "pass": bool(rel <= limit)}


def top_contributors(values: dict[str, float], n: int = 5) -> list[dict]:
    total = sum(values.values())
    ranked = sorted(values.items(), key=lambda kv: kv[1], reverse=True)[:n]
    return [{"nuclide": k, "value": v, "fraction": (v / total if total else None)} for k, v in ranked]
