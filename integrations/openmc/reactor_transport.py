#!/usr/bin/env python3
"""Execute one bounded-input FARIS/OpenMC fixed-source transport job.

The Rust model owns inputs and normalization. This adapter builds OpenMC CSG,
executes the pinned solver, and copies raw per-source means and standard errors
into the strict transport artifact without source-rate scaling. It also records
the Monte Carlo covariance between scalar responses from per-batch statepoints.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import random
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ET

SCHEMA = "faris-openmc-input/v0.1"
ARTIFACT_SCHEMA = "faris-transport-artifact/v0.2"
TEMP_TOLERANCE_K = 0.1
OPENMC_LABEL_TOLERANCE_K = 1.0
# Full transport energy coverage for the authored source and its secondary
# photons; the spectrum-bin sum is checked against integrated neutron flux.
SPECTRUM_EDGES_EV = [0.0, 1.0e3, 1.0e4, 1.0e5, 1.0e6, 2.0e6, 5.0e6, 1.0e7, 1.41e7, 2.0e7, 1.0e9]
# Opt-in activation spectra: the 709 fispact-709 group boundaries (eV, ascending) of ACTINV's
# TENDL-2025 library, tallied per component beside the full-energy spectra above.
ACTIVATION_709_EDGES_EV = [
    1e-05, 1.047129e-05, 1.096478e-05, 1.148154e-05, 1.202264e-05, 1.258925e-05, 1.318257e-05,
    1.380384e-05, 1.44544e-05, 1.513561e-05, 1.584893e-05, 1.659587e-05, 1.737801e-05, 1.819701e-05,
    1.905461e-05, 1.995262e-05, 2.089296e-05, 2.187762e-05, 2.290868e-05, 2.398833e-05, 2.511886e-05,
    2.630268e-05, 2.754229e-05, 2.884032e-05, 3.019952e-05, 3.162278e-05, 3.311311e-05, 3.467369e-05,
    3.630781e-05, 3.801894e-05, 3.981072e-05, 4.168694e-05, 4.365158e-05, 4.570882e-05, 4.786301e-05,
    5.011872e-05, 5.248075e-05, 5.495409e-05, 5.754399e-05, 6.025596e-05, 6.309573e-05, 6.606934e-05,
    6.91831e-05, 7.24436e-05, 7.585776e-05, 7.943282e-05, 8.317638e-05, 8.709636e-05, 9.120108e-05,
    9.549926e-05, 0.0001, 0.0001047129, 0.0001096478, 0.0001148154, 0.0001202264, 0.0001258925,
    0.0001318257, 0.0001380384, 0.000144544, 0.0001513561, 0.0001584893, 0.0001659587, 0.0001737801,
    0.0001819701, 0.0001905461, 0.0001995262, 0.0002089296, 0.0002187762, 0.0002290868, 0.0002398833,
    0.0002511886, 0.0002630268, 0.0002754229, 0.0002884032, 0.0003019952, 0.0003162278, 0.0003311311,
    0.0003467369, 0.0003630781, 0.0003801894, 0.0003981072, 0.0004168694, 0.0004365158, 0.0004570882,
    0.0004786301, 0.0005011872, 0.0005248075, 0.0005495409, 0.0005754399, 0.0006025596, 0.0006309573,
    0.0006606934, 0.000691831, 0.000724436, 0.0007585776, 0.0007943282, 0.0008317638, 0.0008709636,
    0.0009120108, 0.0009549926, 0.001, 0.001047129, 0.001096478, 0.001148154, 0.001202264, 0.001258925,
    0.001318257, 0.001380384, 0.00144544, 0.001513561, 0.001584893, 0.001659587, 0.001737801,
    0.001819701, 0.001905461, 0.001995262, 0.002089296, 0.002187762, 0.002290868, 0.002398833,
    0.002511886, 0.002630268, 0.002754229, 0.002884032, 0.003019952, 0.003162278, 0.003311311,
    0.003467369, 0.003630781, 0.003801894, 0.003981072, 0.004168694, 0.004365158, 0.004570882,
    0.004786301, 0.005011872, 0.005248075, 0.005495409, 0.005754399, 0.006025596, 0.006309573,
    0.006606934, 0.00691831, 0.00724436, 0.007585776, 0.007943282, 0.008317638, 0.008709636,
    0.009120108, 0.009549926, 0.01, 0.01047129, 0.01096478, 0.01148154, 0.01202264, 0.01258925,
    0.01318257, 0.01380384, 0.0144544, 0.01513561, 0.01584893, 0.01659587, 0.01737801, 0.01819701,
    0.01905461, 0.01995262, 0.02089296, 0.02187762, 0.02290868, 0.02398833, 0.02511886, 0.02630268,
    0.02754229, 0.02884032, 0.03019952, 0.03162278, 0.03311311, 0.03467369, 0.03630781, 0.03801894,
    0.03981072, 0.04168694, 0.04365158, 0.04570882, 0.04786301, 0.05011872, 0.05248075, 0.05495409,
    0.05754399, 0.06025596, 0.06309573, 0.06606934, 0.0691831, 0.0724436, 0.07585776, 0.07943282,
    0.08317638, 0.08709636, 0.09120108, 0.09549926, 0.1, 0.1047129, 0.1096478, 0.1148154, 0.1202264,
    0.1258925, 0.1318257, 0.1380384, 0.144544, 0.1513561, 0.1584893, 0.1659587, 0.1737801, 0.1819701,
    0.1905461, 0.1995262, 0.2089296, 0.2187762, 0.2290868, 0.2398833, 0.2511886, 0.2630268, 0.2754229,
    0.2884032, 0.3019952, 0.3162278, 0.3311311, 0.3467369, 0.3630781, 0.3801894, 0.3981072, 0.4168694,
    0.4365158, 0.4570882, 0.4786301, 0.5011872, 0.5248075, 0.5495409, 0.5754399, 0.6025596, 0.6309573,
    0.6606934, 0.691831, 0.724436, 0.7585776, 0.7943282, 0.8317638, 0.8709636, 0.9120108, 0.9549926,
    1.0, 1.047129, 1.096478, 1.148154, 1.202264, 1.258925, 1.318257, 1.380384, 1.44544, 1.513561,
    1.584893, 1.659587, 1.737801, 1.819701, 1.905461, 1.995262, 2.089296, 2.187762, 2.290868, 2.398833,
    2.511886, 2.630268, 2.754229, 2.884032, 3.019952, 3.162278, 3.311311, 3.467369, 3.630781, 3.801894,
    3.981072, 4.168694, 4.365158, 4.570882, 4.786301, 5.011872, 5.248075, 5.495409, 5.754399, 6.025596,
    6.309573, 6.606934, 6.91831, 7.24436, 7.585776, 7.943282, 8.317638, 8.709636, 9.120108, 9.549926,
    10.0, 10.47129, 10.96478, 11.48154, 12.02264, 12.58925, 13.18257, 13.80384, 14.4544, 15.13561,
    15.84893, 16.59587, 17.37801, 18.19701, 19.05461, 19.95262, 20.89296, 21.87762, 22.90868, 23.98833,
    25.11886, 26.30268, 27.54229, 28.84032, 30.19952, 31.62278, 33.11311, 34.67369, 36.30781, 38.01894,
    39.81072, 41.68694, 43.65158, 45.70882, 47.86301, 50.11872, 52.48075, 54.95409, 57.54399, 60.25596,
    63.09573, 66.06934, 69.1831, 72.4436, 75.85776, 79.43282, 83.17638, 87.09636, 91.20108, 95.49926,
    100.0, 104.7129, 109.6478, 114.8154, 120.2264, 125.8925, 131.8257, 138.0384, 144.544, 151.3561,
    158.4893, 165.9587, 173.7801, 181.9701, 190.5461, 199.5262, 208.9296, 218.7762, 229.0868, 239.8833,
    251.1886, 263.0268, 275.4229, 288.4032, 301.9952, 316.2278, 331.1311, 346.7369, 363.0781, 380.1894,
    398.1072, 416.8694, 436.5158, 457.0882, 478.6301, 501.1872, 524.8075, 549.5409, 575.4399, 602.5596,
    630.9573, 660.6934, 691.831, 724.436, 758.5776, 794.3282, 831.7638, 870.9636, 912.0108, 954.9926,
    1000.0, 1047.129, 1096.478, 1148.154, 1202.264, 1258.925, 1318.257, 1380.384, 1445.44, 1513.561,
    1584.893, 1659.587, 1737.801, 1819.701, 1905.461, 1995.262, 2089.296, 2187.762, 2290.868, 2398.833,
    2511.886, 2630.268, 2754.229, 2884.032, 3019.952, 3162.278, 3311.311, 3467.369, 3630.781, 3801.894,
    3981.072, 4168.694, 4365.158, 4570.882, 4786.301, 5011.872, 5248.075, 5495.409, 5754.399, 6025.596,
    6309.573, 6606.934, 6918.31, 7244.36, 7585.776, 7943.282, 8317.638, 8709.636, 9120.108, 9549.926,
    10000.0, 10471.29, 10964.78, 11481.54, 12022.64, 12589.25, 13182.57, 13803.84, 14454.4, 15135.61,
    15848.93, 16595.87, 17378.01, 18197.01, 19054.61, 19952.62, 20892.96, 21877.62, 22908.68, 23988.33,
    25118.86, 26302.68, 27542.29, 28840.32, 30199.52, 31622.78, 33113.11, 34673.69, 36307.81, 38018.94,
    39810.72, 41686.94, 43651.58, 45708.82, 47863.01, 50118.72, 52480.75, 54954.09, 57543.99, 60255.96,
    63095.73, 66069.34, 69183.1, 72443.6, 75857.76, 79432.82, 83176.38, 87096.36, 91201.08, 95499.26,
    100000.0, 104712.9, 109647.8, 114815.4, 120226.4, 125892.5, 131825.7, 138038.4, 144544.0, 151356.1,
    158489.3, 165958.7, 173780.1, 181970.1, 190546.1, 199526.2, 208929.6, 218776.2, 229086.8, 239883.3,
    251188.6, 263026.8, 275422.9, 288403.2, 301995.2, 316227.8, 331131.1, 346736.9, 363078.1, 380189.4,
    398107.2, 416869.4, 436515.8, 457088.2, 478630.1, 501187.2, 524807.5, 549540.9, 575439.9, 602559.6,
    630957.3, 660693.4, 691831.0, 724436.0, 758577.6, 794328.2, 831763.8, 870963.6, 912010.8, 954992.6,
    1000000.0, 1047129.0, 1096478.0, 1148154.0, 1202264.0, 1258925.0, 1318257.0, 1380384.0, 1445440.0,
    1513561.0, 1584893.0, 1659587.0, 1737801.0, 1819701.0, 1905461.0, 1995262.0, 2089296.0, 2187762.0,
    2290868.0, 2398833.0, 2511886.0, 2630268.0, 2754229.0, 2884032.0, 3019952.0, 3162278.0, 3311311.0,
    3467369.0, 3630781.0, 3801894.0, 3981072.0, 4168694.0, 4365158.0, 4570882.0, 4786301.0, 5011872.0,
    5248075.0, 5495409.0, 5754399.0, 6025596.0, 6309573.0, 6606934.0, 6918310.0, 7244360.0, 7585776.0,
    7943282.0, 8317638.0, 8709636.0, 9120108.0, 9549926.0, 10000000.0, 10200000.0, 10400000.0,
    10600000.0, 10800000.0, 11000000.0, 11200000.0, 11400000.0, 11600000.0, 11800000.0, 12000000.0,
    12200000.0, 12400000.0, 12600000.0, 12800000.0, 13000000.0, 13200000.0, 13400000.0, 13600000.0,
    13800000.0, 14000000.0, 14200000.0, 14400000.0, 14600000.0, 14800000.0, 15000000.0, 15200000.0,
    15400000.0, 15600000.0, 15800000.0, 16000000.0, 16200000.0, 16400000.0, 16600000.0, 16800000.0,
    17000000.0, 17200000.0, 17400000.0, 17600000.0, 17800000.0, 18000000.0, 18200000.0, 18400000.0,
    18600000.0, 18800000.0, 19000000.0, 19200000.0, 19400000.0, 19600000.0, 19800000.0, 20000000.0,
    21000000.0, 22000000.0, 23000000.0, 24000000.0, 25000000.0, 26000000.0, 27000000.0, 28000000.0,
    29000000.0, 30000000.0, 32000000.0, 34000000.0, 36000000.0, 38000000.0, 40000000.0, 42000000.0,
    44000000.0, 46000000.0, 48000000.0, 50000000.0, 52000000.0, 54000000.0, 56000000.0, 58000000.0,
    60000000.0, 65000000.0, 70000000.0, 75000000.0, 80000000.0, 90000000.0, 100000000.0, 110000000.0,
    120000000.0, 130000000.0, 140000000.0, 150000000.0, 160000000.0, 180000000.0, 200000000.0,
    240000000.0, 280000000.0, 320000000.0, 360000000.0, 400000000.0, 440000000.0, 480000000.0,
    520000000.0, 560000000.0, 600000000.0, 640000000.0, 680000000.0, 720000000.0, 760000000.0,
    800000000.0, 840000000.0, 880000000.0, 920000000.0, 960000000.0, 1000000000.0
]
ACTIVATION_SPECTRA_STRUCTURE = "fispact-709"
MAX_SOLVER_LOG_BYTES = 4 * 1024 * 1024
INTEGRATED_RSE_REVIEW_GOAL = 0.05
LOCAL_RSE_REVIEW_GOAL = 0.10
COVARIANCE_METHOD = "batch-means-sample-covariance/v1"
BATCH_VALUES_SCHEMA = "faris-transport-batch-values/v0.1"
BATCH_VALUES_FILE = "transport-batch-values.json"
BATCH_MEAN_REL_TOLERANCE = 1.0e-12
BATCH_STD_REL_TOLERANCE = 1.0e-9


def domain_key(domain: dict) -> str:
    return json.dumps(domain, sort_keys=True)


def torus_region_volume_m3(major_radius_m: float, inner_m: float, outer_m: float, region: dict) -> float:
    """Exact volume of a region of the full torus shell between minor radii inner_m < outer_m.

    A shell point has cylindrical radius R = R0 + r cos(theta) and volume
    element dV = R r dr dtheta dphi = (R0 + r cos(theta)) r dr dtheta dphi. The
    outboard half (R >= R0, cos(theta) > 0) and inboard half (R < R0) integrate
    over a half annulus each:
        int (R0 + r cos(theta)) r dr dtheta
          = R0 pi (b^2 - a^2) / 2  +/-  (b^3 - a^3) / 3 * int cos(theta) dtheta
          = R0 pi (b^2 - a^2) / 2  +/-  2 (b^3 - a^3) / 3,
    plus for outboard and minus for inboard. Multiplying by the toroidal extent
    gives the region volume: 2 pi for a half, 2 w for the port sector of half
    width w, 2 pi - 2 w for the outboard half without it. The two halves sum to
    the full torus 2 pi^2 R0 (b^2 - a^2).
    """
    a, b = inner_m, outer_m
    half_annulus = major_radius_m * math.pi * (b * b - a * a) / 2.0
    skew = 2.0 * (b ** 3 - a ** 3) / 3.0
    inboard, outboard = half_annulus - skew, half_annulus + skew
    kind = region["kind"]
    if kind == "inboard_half":
        return 2.0 * math.pi * inboard
    if kind == "outboard_half":
        w = region.get("excluding_sector_half_width_rad")
        return 2.0 * math.pi * outboard if w is None else (2.0 * math.pi - 2.0 * w) * outboard
    if kind == "port_sector":
        return 2.0 * region["half_width_rad"] * outboard
    raise ValueError(f"unsupported component region kind: {kind}")


def region_contains(region: dict, major_radius_cm: float, x_cm: float, z_cm: float) -> bool:
    """Region membership of a point by its cylindrical radius and toroidal angle.

    The torus axis is y, the toroidal angle is atan2(z, x), and sectors are
    centred on angle 0 (the +x axis, the centre of the outboard port prism).
    """
    radius = math.hypot(x_cm, z_cm)
    angle = abs(math.atan2(z_cm, x_cm))
    kind = region["kind"]
    if kind == "inboard_half":
        return radius < major_radius_cm
    if radius < major_radius_cm:
        return False
    if kind == "outboard_half":
        w = region.get("excluding_sector_half_width_rad")
        return w is None or angle > w
    if kind == "port_sector":
        return angle <= region["half_width_rad"]
    raise ValueError(f"unsupported component region kind: {kind}")


def build_region_universe(openmc, material, name: str, major_radius_cm: float, regions: list[dict]):
    """Partition a shell cell into region sub-cells inside a nested universe.

    Returns (universe, {region key: sub-cell}). The parent component cell keeps
    its identity, name and port cut and is filled with this universe, so every
    existing component tally (a CellFilter on the parent matches at any nesting
    depth) is unchanged, and each region is one cell for a scalar CellFilter.

    OpenMC's CylindricalMesh is always about the z axis while the torus axis is
    y, and a mesh filter carries a translation but no rotation, so region bins
    cannot be taken from a cylindrical mesh without changing the model
    orientation. Region cells are used instead, with these surfaces:
    a y-axis cylinder of radius R0 (inboard inside, outboard outside) and two
    planes through the torus axis at +/- the sector half width. The sector is
    the wedge between the planes (the half-planes are less than pi apart, so
    their intersection is exactly the wedge); the rest of the outboard half is
    its complement, so no toroidal wrap-around needs special handling.
    """
    widths = {w for r in regions if (w := (r.get("half_width_rad") if r["kind"] == "port_sector" else r.get("excluding_sector_half_width_rad"))) is not None}
    require(len(widths) <= 1, f"{name}: regions must share one sector half width")
    cylinder = openmc.YCylinder(x0=0.0, z0=0.0, r=major_radius_cm)
    inboard = openmc.Cell(name=f"{name}/inboard-half", fill=material, region=-cylinder)
    by_key = {domain_key({"kind": "inboard_half"}): inboard}
    outboard = openmc.Cell(name=f"{name}/outboard-half", fill=material, region=+cylinder)
    by_key[domain_key({"kind": "outboard_half"})] = outboard
    if widths:
        w = next(iter(widths))
        require(0.0 < w < math.pi / 2.0, f"{name}: sector half width must be in (0, pi/2)")
        # f(x, z) = z cos(phi) - x sin(phi) > 0 is counter-clockwise of the ray at angle phi.
        lower = openmc.Plane(a=math.sin(w), b=0.0, c=math.cos(w), d=0.0)
        upper = openmc.Plane(a=-math.sin(w), b=0.0, c=math.cos(w), d=0.0)
        wedge = +lower & -upper
        sector = openmc.Cell(name=f"{name}/port-sector", fill=material, region=wedge)
        remainder = openmc.Cell(name=f"{name}/outboard-excluding-port-sector", fill=material, region=~wedge)
        outboard.fill = openmc.Universe(cells=[sector, remainder])
        by_key[domain_key({"kind": "port_sector", "half_width_rad": w})] = sector
        by_key[domain_key({"kind": "outboard_half", "excluding_sector_half_width_rad": w})] = remainder
    else:
        outboard.fill = material
    return openmc.Universe(cells=[inboard, outboard]), by_key


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def per_batch_values(cumulative_sums: list[float]) -> list[float]:
    """Per-batch tally value x_b = sum_b - sum_(b-1), with sum_0 = 0."""
    previous = 0.0
    values = []
    for total in cumulative_sums:
        values.append(total - previous)
        previous = total
    return values


def verify_batch_values(response_id: str, values: list[float], mean: float, std_dev: float) -> None:
    """Check batch values against OpenMC's own final mean and standard deviation.

    OpenMC 0.15.3 (openmc/tallies.py, Tally.mean and Tally.std_dev) defines
    mean = sum / n_realizations and
    std_dev = sqrt((sum_sq / n - mean**2) / (n - 1)) for nonzero means, where
    sum and sum_sq are the accumulated per-batch tally value and its square.
    Hence, for per-batch values x_b, mean = mean(x_b) and
    std_dev = sqrt(var(x_b, ddof=1) / n). A statepoint written after batch b
    holds sum over batches 1..b, so consecutive differences recover x_b. The
    covariance below reuses exactly that estimator, so any disagreement means
    the assumption above does not hold and the run must not be trusted.
    OpenMC reports std_dev = 0 for an exactly zero mean; that case is checked
    only for the mean.
    """
    n = len(values)
    require(n >= 2, f"response {response_id}: at least 2 batches are required")
    require(all(math.isfinite(v) for v in values), f"response {response_id}: non-finite batch value")
    batch_mean = math.fsum(values) / n
    scale = abs(mean) if mean != 0.0 else max(abs(v) for v in values)
    require(
        abs(batch_mean - mean) <= BATCH_MEAN_REL_TOLERANCE * scale,
        f"response {response_id}: mean of batch values {batch_mean!r} differs from OpenMC mean {mean!r}",
    )
    if mean == 0.0:
        return
    variance = math.fsum((v - batch_mean) ** 2 for v in values) / (n - 1)
    batch_se = math.sqrt(variance / n)
    require(
        abs(batch_se - std_dev) <= BATCH_STD_REL_TOLERANCE * max(std_dev, batch_se),
        f"response {response_id}: batch standard error {batch_se!r} differs from OpenMC std_dev {std_dev!r}",
    )


def batch_mean_covariance(columns: list[list[float]]) -> list[list[float]]:
    """Covariance of the batch-mean estimators: sample covariance (ddof=1) / n."""
    require(columns, "no responses for covariance")
    n = len(columns[0])
    require(n >= 2 and all(len(c) == n for c in columns), "covariance needs equal columns of at least 2 batches")
    means = [math.fsum(c) / n for c in columns]
    size = len(columns)
    matrix = [[0.0] * size for _ in range(size)]
    for i in range(size):
        for j in range(i, size):
            total = math.fsum((columns[i][b] - means[i]) * (columns[j][b] - means[j]) for b in range(n))
            matrix[i][j] = matrix[j][i] = total / (n - 1) / n
    return matrix


def read_batch_sums(openmc, out: Path, n_batches: int, tally_names: list[str]) -> dict[str, list[float]]:
    """Read cumulative scalar tally sums from every per-batch statepoint.

    Intermediate statepoints are deleted as soon as they are read; only the
    final one remains, as the rest of the worker and its checks expect.
    """
    files = {}
    for path in out.glob("statepoint.*.h5"):
        files[int(path.name.split(".")[1])] = path
    require(sorted(files) == list(range(1, n_batches + 1)), "per-batch statepoints do not cover batches 1..N exactly")
    sums = {name: [] for name in tally_names}
    try:
        for batch in range(1, n_batches + 1):
            with openmc.StatePoint(str(files[batch])) as sp:
                require(int(sp.current_batch) == batch and int(sp.n_realizations) == batch, f"statepoint for batch {batch} has unexpected realization count")
                for name in tally_names:
                    values = sp.get_tally(name=name).sum.ravel()
                    require(len(values) == 1, f"{name} did not produce one scalar in batch {batch}")
                    sums[name].append(float(values[0]))
            if batch < n_batches:
                files[batch].unlink()
    finally:
        for batch in range(1, n_batches):
            files[batch].unlink(missing_ok=True)
    return sums


def sampling_precision_report(request: dict, tallies: list[dict], volumes: list[dict]) -> dict:
    """Report predeclared exploratory precision goals, never a physics verdict."""
    by_id = {t["response_id"]: t for t in tallies}
    volume_by_domain = {json.dumps(v["domain"], sort_keys=True): v for v in volumes}
    checks = []

    def add(response_id: str, goal: float, quantity: str, include_volume_error: bool) -> None:
        tally = by_id.get(response_id)
        if tally is None:
            return
        score = request["responses"]
        definition = next(r for r in score if r["id"] == response_id)
        mean = float(tally["mean"])
        se = float(tally["standard_error"])
        relative = None if mean == 0.0 else se / abs(mean)
        if include_volume_error and mean > 0.0:
            volume = volume_by_domain[json.dumps(definition["domain"], sort_keys=True)]
            relative = math.hypot(relative or 0.0, float(volume["standard_error"]) / float(volume["value"]))
        checks.append({
            "response_id": response_id,
            "quantity": quantity,
            "target_relative_standard_error": goal,
            "observed_relative_standard_error": relative,
            "met": relative is not None and relative <= goal,
        })

    by_definition = {r["id"]: r for r in request["responses"]}
    for response_id, definition in by_definition.items():
        score = definition["score"]
        domain = definition["domain"]
        if response_id == "total-tritium-production":
            add(response_id, INTEGRATED_RSE_REVIEW_GOAL, "whole-model tritium production", False)
        elif score["kind"] == "heating" and score["particle_scope"] == "total":
            if domain["kind"] == "whole_model":
                add(response_id, INTEGRATED_RSE_REVIEW_GOAL, "whole-model deposited heating", False)
            elif domain["kind"] == "component":
                add(response_id, INTEGRATED_RSE_REVIEW_GOAL, "integrated component deposited heating", False)
                add(response_id, LOCAL_RSE_REVIEW_GOAL, "volume-averaged component deposited heating", True)
        elif score["kind"] == "flux" and (domain["kind"] == "mesh" or domain.get("component_id") == "magnets"):
            add(response_id, LOCAL_RSE_REVIEW_GOAL, "magnet or mesh neutron flux", True)
        elif score["kind"] == "flux_above" and domain["kind"] in ("component", "component_region"):
            add(response_id, LOCAL_RSE_REVIEW_GOAL, "region-average fast neutron flux", True)
    return {
        "plan_id": "faris-exploratory-precision-goals/v0.1",
        "purpose": "numerical sampling review only; not a physics or design acceptance test",
        "estimator": "one-standard-error relative to the response mean; zero means have undefined RSE and fail the check",
        "integrated_goal": INTEGRATED_RSE_REVIEW_GOAL,
        "local_goal": LOCAL_RSE_REVIEW_GOAL,
        "checks": checks,
        "all_goals_met": bool(checks) and all(item["met"] for item in checks),
    }


def read_input(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    require(data.get("schema_version") == SCHEMA, f"input schema must be {SCHEMA}")
    for key in ("manifest", "physics", "request", "sampling", "cross_sections", "openmc_executable"):
        require(key in data, f"missing top-level field: {key}")
    return data


def _terminate_solver(process: subprocess.Popen[bytes], grace_seconds: float = 5.0) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_solver_streaming(command: list[str], cwd: Path, env: dict[str, str], log_limit: int) -> dict:
    """Tee raw solver bytes to the Rust worker pipes and bounded log files.

    Rust owns cancellation, timeout, process-group cleanup, and its independent
    output cap. This layer retains at most ``log_limit`` bytes per stream and
    terminates OpenMC if that local retained-log cap is exceeded.
    """
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    exceeded = threading.Event()
    tee_failed = threading.Event()
    stream_state: dict[str, dict] = {
        "stdout": {"bytes_seen": 0, "bytes_saved": 0, "truncated": False, "error": None},
        "stderr": {"bytes_seen": 0, "bytes_saved": 0, "truncated": False, "error": None},
    }

    def pump(name: str, source, parent_stream, log_path: Path) -> None:
        state = stream_state[name]
        try:
            with log_path.open("wb") as log:
                while True:
                    # BufferedReader.read(n) may wait for n bytes or EOF;
                    # read1 forwards whatever is currently available so Rust
                    # sees output promptly and can enforce cancellation/limits.
                    chunk = source.read1(64 * 1024)
                    if not chunk:
                        break
                    state["bytes_seen"] += len(chunk)
                    remaining = max(0, log_limit - state["bytes_saved"])
                    saved = chunk[:remaining]
                    if saved:
                        log.write(saved)
                        state["bytes_saved"] += len(saved)
                    if len(saved) != len(chunk):
                        state["truncated"] = True
                        exceeded.set()
                    try:
                        parent_stream.write(chunk)
                        parent_stream.flush()
                    except (BrokenPipeError, OSError) as error:
                        state["error"] = f"parent log pipe: {error}"
                        tee_failed.set()
                log.flush()
        except Exception as error:
            state["error"] = f"log capture: {type(error).__name__}: {error}"
            tee_failed.set()

    require(process.stdout is not None and process.stderr is not None, "failed to create solver output pipes")
    stdout_thread = threading.Thread(
        target=pump, args=("stdout", process.stdout, sys.stdout.buffer, cwd / "openmc.stdout.log"), daemon=True
    )
    stderr_thread = threading.Thread(
        target=pump, args=("stderr", process.stderr, sys.stderr.buffer, cwd / "openmc.stderr.log"), daemon=True
    )
    stdout_thread.start()
    stderr_thread.start()
    stop_reason = None
    while process.poll() is None:
        if exceeded.is_set():
            stop_reason = f"solver log exceeded {log_limit} bytes on at least one stream"
            _terminate_solver(process)
            break
        if tee_failed.is_set():
            stop_reason = "failed to forward or retain solver output"
            _terminate_solver(process)
            break
        time.sleep(0.02)
    return_code = process.wait()
    stdout_thread.join()
    stderr_thread.join()
    if stop_reason is None and exceeded.is_set():
        stop_reason = f"solver log exceeded {log_limit} bytes on at least one stream"
    if stop_reason is None and tee_failed.is_set():
        stop_reason = "failed to forward or retain solver output"
    for name in ("stdout", "stderr"):
        path = cwd / f"openmc.{name}.log"
        stream_state[name]["file"] = path.name
        stream_state[name]["sha256"] = sha256(path)
    return {
        "return_code": return_code,
        "stop_reason": stop_reason,
        "log_limit_bytes_per_stream": log_limit,
        "streams": stream_state,
        "capture_policy": "bounded local prefix; complete raw bytes tee to parent Rust job output pipes",
    }


def check_data_identity(physics: dict, xml_path: Path, openmc, coupled_heating_required: bool) -> tuple[str, list[str], dict[str, float], float, dict[str, str], dict[str, object]]:
    require(xml_path.is_file(), f"cross_sections XML does not exist: {xml_path}")
    xml_hash = sha256(xml_path)
    selection = physics["nuclear_data"]
    root = ET.parse(xml_path).getroot()
    neutron = {}
    photon = {}
    for library in root.findall("library"):
        if library.get("type") == "neutron":
            for nuclide in library.get("materials", "").split():
                neutron[nuclide] = library.get("path")
        elif library.get("type") == "photon":
            for element in library.get("materials", "").split():
                photon[element] = library.get("path")
    recipes = [m["recipe"] for m in physics["materials"] if m["recipe"]["kind"] == "nuclide_mixture"]
    required = sorted({n["nuclide"] for recipe in recipes for n in recipe["nuclides"]})
    actual_temps = {}
    runtime_labels = set()
    photon_hashes = {}
    photon_physics = {"atomic_relaxation_available": None, "atomic_relaxation_enabled": False, "electron_treatment": None}
    if selection.get("state") == "inventory":
        data_root = xml_path.parent
        declared = {}
        declared_photon = {}
        for item in selection.get("files", []):
            p = (data_root / item["relative_path"]).resolve()
            require(p.is_relative_to(data_root.resolve()), "nuclear-data path escaped selected library root")
            require(p.is_file(), f"selected nuclear-data file missing: {item['relative_path']}")
            require(p.stat().st_size == item["size_bytes"], f"nuclear-data size changed: {item['file_id']}")
            require(item["sha256"] == f"sha256:{sha256(p)}", f"nuclear-data hash changed: {item['file_id']}")
            for nuc in item["nuclides"]:
                if "continuous_energy_neutron_transport" in item["capabilities"]:
                    declared[nuc] = (item, p)
                if "photon_transport" in item["capabilities"]:
                    element = re.match(r"[A-Z][a-z]?", nuc)
                    require(element is not None, f"invalid photon-element isotope name: {nuc}")
                    declared_photon[element.group(0)] = (item, p)
        for nuclide in required:
            require(nuclide in declared, f"selected nuclear-data inventory omits {nuclide}")
            item, file_path = declared[nuclide]
            require(nuclide in neutron, f"cross_sections XML has no neutron entry for {nuclide}")
            require(item["relative_path"] == neutron[nuclide], f"XML/data-inventory path mismatch for {nuclide}")
            if coupled_heating_required:
                require("heating" in item["capabilities"], f"MT=301 heating capability is not declared for {nuclide}")
            data = openmc.data.IncidentNeutron.from_hdf5(str(file_path))
            if coupled_heating_required:
                require(301 in data.reactions, f"MT=301 heating coefficients missing for {nuclide}")
            stored_temperatures = [float(kT) / openmc.data.K_BOLTZMANN for kT in data.kTs]
            require(len(stored_temperatures) == len(item["temperatures_k"]), f"audited temperature count changed for {nuclide}")
            require(all(any(abs(a-b) <= 1.0e-8 for b in item["temperatures_k"]) for a in stored_temperatures), f"audited stored temperatures changed for {nuclide}")
            actual_temps[nuclide] = min(stored_temperatures)
            runtime_labels.update(data.temperatures)
    else:
        for nuclide in required:
            require(nuclide in neutron, f"cross_sections XML has no neutron entry for {nuclide}")
            file_path = (xml_path.parent / neutron[nuclide]).resolve(strict=True)
            data = openmc.data.IncidentNeutron.from_hdf5(str(file_path))
            actual_temps[nuclide] = min(float(kT) / openmc.data.K_BOLTZMANN for kT in data.kTs)
            if coupled_heating_required:
                require(301 in data.reactions, f"MT=301 heating coefficients missing for {nuclide}")
            runtime_labels.update(data.temperatures)
    used_elements = sorted({re.match(r"[A-Z][a-z]?", nuclide).group(0) for nuclide in required}) if coupled_heating_required else []
    for element in used_elements:
        require(element in photon, f"cross_sections XML has no photon atomic data entry for {element}")
        if selection.get("state") == "inventory":
            require(element in declared_photon, f"selected nuclear-data inventory omits photon data for {element}")
            item, file_path = declared_photon[element]
            require(item["relative_path"] == photon[element], f"photon XML/data-inventory path mismatch for {element}")
            require(not coupled_heating_required or "atomic_relaxation" in item["capabilities"], f"atomic-relaxation shell map capability is not declared for {element}")
        else:
            file_path = (xml_path.parent / photon[element]).resolve(strict=True)
        data = openmc.data.IncidentPhoton.from_hdf5(str(file_path))
        require(data.name == element and bool(data.reactions), f"photon data unreadable/incomplete for {element}")
        relaxation = data.atomic_relaxation
        # Require binding-energy and electron-count records for every
        # photoelectric shell. Low-Z elements can legitimately have no
        # relaxation transitions; an empty object/map for every shell is
        # the unsafe condition that crashed with the FENDL photon files.
        import h5py
        with h5py.File(file_path, "r") as library:
            shells = set(library[element]["subshells"].keys())
        populated = bool(
            relaxation is not None
            and shells
            and shells <= set(relaxation.binding_energy)
            and shells <= set(relaxation.num_electrons)
        )
        require(populated, f"atomic-relaxation binding/electron shell map incomplete for {element}")
        photon_physics.setdefault("atomic_relaxation_available_by_element", {})[element] = {
            "shell_map_complete": populated,
            "photoelectric_shell_count": len(shells),
            "atomic_relaxation_shell_count": len(relaxation.binding_energy) if relaxation else 0,
            "transition_shell_count": len(relaxation.transitions) if relaxation else 0,
        }
        photon_hashes[element] = sha256(file_path)
        if coupled_heating_required:
            populated_flags = [item["shell_map_complete"] for item in photon_physics["atomic_relaxation_available_by_element"].values()]
            photon_physics["atomic_relaxation_available"] = bool(populated_flags) and all(populated_flags)
            photon_physics["atomic_relaxation_enabled"] = photon_physics["atomic_relaxation_available"]
            photon_physics["electron_treatment"] = "led"
    require(len(runtime_labels) == 1, "this adapter requires a common labeled neutron-data temperature across its recipes")
    label = next(iter(runtime_labels))
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)K", label)
    require(match is not None, f"unsupported neutron data temperature label: {label}")
    runtime_temp = float(match.group(1))
    for nuclide in required:
        require(nuclide in actual_temps, f"no stored temperature found for {nuclide}")
    for recipe in recipes:
        target = recipe["nuclear_data_temperature_k"]
        for nuclide in (n["nuclide"] for n in recipe["nuclides"]):
            require(abs(actual_temps[nuclide] - target) <= TEMP_TOLERANCE_K, f"{nuclide} stored numeric temperature is outside {TEMP_TOLERANCE_K} K of requested {target}")
    require(abs(runtime_temp - min(actual_temps.values())) < 1.0, "rounded OpenMC data label is inconsistent with exact stored temperatures")
    return xml_hash, required, actual_temps, runtime_temp, photon_hashes, photon_physics


def audit_geometry_ownership(
    geometry, major_radius_m, plasma_minor_radius_m, gap_m,
    first_inner_radius_m, components, component_cells, plasma_cell,
    clearance_cell, materials, scenario_sha256, variant_id,
):
    """Probe actual OpenMC CSG ownership before solving; this is geometry-only."""
    probes = []
    toroidal_angles = (math.pi / 2.0, math.pi, 3.0 * math.pi / 2.0)
    cross_section_angles = (0.0, math.pi / 2.0, math.pi, 3.0 * math.pi / 2.0)

    def probe(label, radial_m, phi, theta, expected_name, expected_cell, expected_material_id):
        ring_radius_cm = 100.0 * (major_radius_m + radial_m * math.cos(theta))
        point_cm = [ring_radius_cm * math.cos(phi), 100.0 * radial_m * math.sin(theta),
                    ring_radius_cm * math.sin(phi)]
        path = geometry.find(point_cm)
        # The find path alternates universes and cells; a cell nested in
        # region universes is owned by its outermost cell, and its material is
        # that of the innermost one.
        cells_on_path = [c for c in (path if isinstance(path, (list, tuple)) else [path]) if hasattr(c, "region")]
        observed = cells_on_path[0] if cells_on_path else None
        leaf = cells_on_path[-1] if cells_on_path else None
        expected_fill = materials[expected_material_id]
        observed_fill = None if leaf is None else leaf.fill
        expected_material_name = None if expected_fill is None else expected_fill.name
        observed_material_name = None if observed_fill is None else observed_fill.name
        expected_material_openmc_id = None if expected_fill is None else expected_fill.id
        observed_material_openmc_id = None if observed_fill is None else observed_fill.id
        match = (
            observed is not None
            and observed.id == expected_cell.id
            and observed.name == expected_name
            and observed_material_name == expected_material_name
            and observed_material_openmc_id == expected_material_openmc_id
        )
        probes.append({
            "probe_id": label,
            "point_xyz_cm": point_cm,
            "minor_radius_m": radial_m,
            "toroidal_angle_rad": phi,
            "cross_section_angle_rad": theta,
            "expected_cell_name": expected_name,
            "expected_cell_id": expected_cell.id,
            "observed_cell_name": None if observed is None else observed.name,
            "observed_cell_id": None if observed is None else observed.id,
            "expected_material_id": expected_material_id,
            "expected_openmc_material_name": expected_material_name,
            "expected_openmc_material_id": expected_material_openmc_id,
            "observed_openmc_material_name": observed_material_name,
            "observed_openmc_material_id": observed_material_openmc_id,
            "status": "PASS" if match else "FAIL",
        })

    plasma_delta = min(1.0e-4, plasma_minor_radius_m / 10.0)
    for phi in toroidal_angles:
        probe(f"plasma-interior-phi-{phi:.8f}", plasma_minor_radius_m - plasma_delta,
              phi, 0.0, "plasma-source-domain", plasma_cell, "void")

    clearance_ok = []
    if gap_m > 0.0:
        delta = min(1.0e-4, gap_m / 4.0)
        for phi in toroidal_angles:
            for label, radius in (("near-plasma", plasma_minor_radius_m + delta),
                                  ("near-first-wall", first_inner_radius_m - delta)):
                probe_id = f"clearance-{label}-phi-{phi:.8f}"
                probe(probe_id, radius, phi, 0.0, "plasma-first-wall-clearance",
                      clearance_cell, "void")
                clearance_ok.append(probes[-1]["status"] == "PASS")

    for component in components:
        cid = component["id"]
        inner = float(component["inner_minor_radius_m"])
        outer = float(component["outer_minor_radius_m"])
        delta = min(1.0e-4, (outer - inner) / 4.0)
        cell = component_cells[cid]
        material_id = component["material_id"]
        for phi in toroidal_angles:
            for theta in cross_section_angles:
                probe(f"{cid}-mid-phi-{phi:.8f}-theta-{theta:.8f}",
                      (inner + outer) / 2.0, phi, theta, cid, cell, material_id)
            for label, radius in (("near-inner", inner + delta),
                                  ("near-outer", outer - delta)):
                probe(f"{cid}-{label}-phi-{phi:.8f}", radius, phi, 0.0,
                      cid, cell, material_id)

    failed = sum(p["status"] != "PASS" for p in probes)
    return {
        "schema_version": "faris-openmc-geometry-ownership-audit/v0.1",
        "method": "OpenMC Geometry.find actual cell/material ownership at analytic probes; zero transport histories",
        "scenario_sha256": scenario_sha256,
        "variant_id": variant_id,
        "status": "PASS" if failed == 0 else "FAIL",
        "checks_are_geometry_only": True,
        "scientific_qualification": "NOT_EVALUATED",
        "plasma_radius_m": plasma_minor_radius_m,
        "declared_plasma_to_first_wall_clearance_m": gap_m,
        "first_wall_inner_radius_m": first_inner_radius_m,
        "clearance_status": "PASS" if gap_m == 0.0 or all(clearance_ok) else "FAIL",
        "toroidal_probe_directions_rad": list(toroidal_angles),
        "cross_section_probe_directions_rad": list(cross_section_angles),
        "probe_count": len(probes),
        "failed_probe_count": failed,
        "probes": probes,
    }


def compose(inp: dict, out: Path, openmc, data_runtime_temperature: float, photon_physics: dict):
    manifest = inp["manifest"]
    physics = inp["physics"]
    request = inp["request"]
    sampling = inp["sampling"]
    variant = next((v for v in manifest["variants"] if v["id"] == physics["variant_id"]), None)
    require(variant is not None, "physics variant missing from manifest")
    require(request["scenario_id"] == manifest["scenario_id"] and request["scenario_sha256"] == manifest["source_sha256"], "request/manifest identity mismatch")
    require(request["variant_id"] == physics["variant_id"], "request/physics variant mismatch")
    require(request["fusion_power_mw"] == manifest["fusion_power_mw"], "request fusion power differs from manifest")
    require(physics["scenario_sha256"] == manifest["source_sha256"], "physics/manifest hash mismatch")
    require(physics["source"]["spatial_distribution"] == "uniform_circular_plasma_torus", "unsupported spatial source")
    require(physics["source"]["angular_distribution"] == "isotropic" and physics["source"]["energy_distribution"] == "monoenergetic", "unsupported source recipe")
    require(all(physics["source"][k] == request["source"][k] for k in ("energy_per_reaction_ev", "neutron_energy_ev", "neutrons_per_reaction")), "source/request mismatch")
    require(sampling["batches"] >= 2, "at least 2 batches are required: response covariance is estimated from batch-resolved tallies")
    require(sampling["threads"] >= 1 and sampling["batches"] >= 1 and sampling["particles_per_batch"] >= 1, "sampling values must be positive integers")

    material_defs = {m["id"]: m["recipe"] for m in physics["materials"]}
    assignments = {a["component_id"]: a["material_id"] for a in physics["component_assignments"]}
    require(len(assignments) == len(variant["components"]), "every variant component must have one physics assignment")
    materials = {}
    for material_id, recipe in material_defs.items():
        if recipe["kind"] == "void":
            # OpenMC represents a geometric void with a null-filled cell, not
            # a zero-density Material (which is rejected by the solver).
            materials[material_id] = None
            continue
        else:
            mat = openmc.Material(name=material_id)
            for nuclide in recipe["nuclides"]:
                mat.add_nuclide(nuclide["nuclide"], nuclide["atom_fraction"], percent_type="ao")
            mat.set_density("kg/m3", recipe["density_kg_m3"])
            # OpenMC CE groups are addressed by their rounded label (e.g.
            # 294K); the exact HDF5 kT value is separately checked above.
            mat.temperature = data_runtime_temperature
        materials[material_id] = mat

    R = float(manifest["major_radius_m"])
    plasma_minor = float(manifest["plasma_minor_radius_m"])
    require(bool(variant["components"]), "variant has no components")
    gap = float(variant["components"][0]["inner_minor_radius_m"]) - plasma_minor
    require(gap >= 0.0, "first component begins inside the plasma minor radius")
    require(math.isfinite(R) and R > 0 and plasma_minor > 0, "invalid torus dimensions")
    scale = 100.0
    plasma_surface = openmc.YTorus(a=R * scale, b=plasma_minor * scale, c=plasma_minor * scale)
    first_inner = plasma_minor + gap
    first_inner_surface = openmc.YTorus(
        a=R * scale, b=first_inner * scale, c=first_inner * scale
    )
    require("void" in materials, "explicit void material required for plasma and clearance")
    plasma = openmc.Cell(name="plasma-source-domain", fill=materials["void"], region=-plasma_surface)
    # Keep an unperforated geometry with independent cells for the volume
    # sampling control below. The transport geometry is cut by the port; using
    # that geometry alone would classify every in-port point as port void and
    # falsely report zero removed component volume.
    control_cells = [openmc.Cell(name="plasma-source-domain", fill=materials["void"], region=-plasma_surface)]
    penetration = manifest.get("penetration")
    port_region = None
    if penetration is not None:
        require(penetration.get("kind") == "outboard_rectangular_prism", "unsupported penetration geometry kind")
        bounds = penetration["bounds_m"]
        minimum = [float(v) for v in bounds["minimum_xyz_m"]]
        maximum = [float(v) for v in bounds["maximum_xyz_m"]]
        require(len(minimum) == len(maximum) == 3 and all(math.isfinite(minimum[i]) and math.isfinite(maximum[i]) and minimum[i] < maximum[i] for i in range(3)), "invalid rectangular penetration bounds")
        require(minimum[0] > R + plasma_minor, "penetration intersects the idealized plasma source volume")
        xlo, ylo_box, zlo = (openmc.XPlane(x0=minimum[0] * scale), openmc.YPlane(y0=minimum[1] * scale), openmc.ZPlane(z0=minimum[2] * scale))
        xhi, yhi_box, zhi = (openmc.XPlane(x0=maximum[0] * scale), openmc.YPlane(y0=maximum[1] * scale), openmc.ZPlane(z0=maximum[2] * scale))
        port_region = +xlo & -xhi & +ylo_box & -yhi_box & +zlo & -zhi
        require(penetration["fill_material_id"] in materials, "penetration fill material is absent")
    surfaces = []
    cells = [plasma]
    clearance_cell = None
    if gap > 0.0:
        # Model the declared plasma-to-first-wall clearance explicitly. The
        # material component's volume and port intersection start at its own
        # inner radius, not at the plasma boundary.
        clearance_cell = openmc.Cell(
            name="plasma-first-wall-clearance",
            fill=materials["void"],
            region=+plasma_surface & -first_inner_surface,
        )
        cells.append(clearance_cell)
        control_cells.append(
            openmc.Cell(
                name="plasma-first-wall-clearance",
                fill=materials["void"],
                region=+plasma_surface & -first_inner_surface,
            )
        )
    component_cells = {}
    component_filters = {}
    region_domains = {}
    for response in request["responses"]:
        if response["domain"]["kind"] == "component_region":
            require(response["score"]["kind"] == "flux_above", "component regions support only flux-above scores")
            region_domains.setdefault(response["domain"]["component_id"], {})[domain_key(response["domain"])] = response["domain"]
    region_cell_by_domain = {}
    for component in variant["components"]:
        rid = component["id"]
        require(rid in assignments and assignments[rid] == component["material_id"], f"material assignment mismatch for {rid}")
        require(component["material_id"] in materials, f"material missing for component {rid}")
        inner = float(component["inner_minor_radius_m"])
        outer = float(component["outer_minor_radius_m"])
        require(math.isclose(inner, first_inner if not surfaces else float(surfaces[-1].c) / scale, abs_tol=1e-8), f"non-contiguous radial geometry before {rid}")
        inner_surface = first_inner_surface if not surfaces else surfaces[-1]
        surf = openmc.YTorus(a=R * scale, b=outer * scale, c=outer * scale)
        control_region = +inner_surface & -surf
        control_cells.append(openmc.Cell(name=rid, fill=materials[component["material_id"]], region=control_region))
        # Build a second CSG node tree so the cut applied below cannot alias
        # or mutate the unperforated control cell's region expression.
        region = +inner_surface & -surf
        if penetration is not None and rid in penetration["affected_component_ids"]:
            region &= ~port_region
        cell = openmc.Cell(name=rid, fill=materials[component["material_id"]], region=region)
        if rid in region_domains:
            require(materials[component["material_id"]] is not None, f"component {rid} is void and has no regions")
            region_universe, by_region = build_region_universe(
                openmc, materials[component["material_id"]], rid, R * scale,
                [d["region"] for d in region_domains[rid].values()],
            )
            cell.fill = region_universe
            for key, domain in region_domains[rid].items():
                region_cell_by_domain[key] = by_region[domain_key(domain["region"])]
        cells.append(cell)
        surfaces.append(surf)
        component_cells[rid] = cell
        component_filters[rid] = openmc.CellFilter(cell)
    outer_minor = float(variant["components"][-1]["outer_minor_radius_m"])
    require(outer_minor < R, "outer torus minor radius must remain below major radius")
    outer_surface = surfaces[-1]
    outer_surface.boundary_type = "vacuum"
    if penetration is not None:
        port_cell = openmc.Cell(
            name=penetration["id"],
            fill=materials[penetration["fill_material_id"]],
            region=port_region & +plasma_surface & -outer_surface,
        )
        cells.append(port_cell)
    outside = openmc.Cell(name="outside-torus-void", fill=materials["void"], region=+outer_surface)
    cells.append(outside)
    root = openmc.Universe(cells=cells)
    geometry = openmc.Geometry(root)
    control_cells.append(openmc.Cell(name="outside-torus-void", fill=materials["void"], region=+outer_surface))
    unperforated_geometry = openmc.Geometry(openmc.Universe(cells=control_cells))
    geometry_ownership_audit = audit_geometry_ownership(
        geometry,
        R,
        plasma_minor,
        gap,
        first_inner,
        variant["components"],
        component_cells,
        plasma,
        clearance_cell,
        materials,
        request["scenario_sha256"],
        physics["variant_id"],
    )
    require(
        geometry_ownership_audit["status"] == "PASS",
        "pre-transport OpenMC geometry ownership audit failed",
    )

    penetration_volume_audit = None
    region_volume_cm3 = {}
    component_volume_cm3 = {}
    component_volume_se_cm3 = {component_id: 0.0 for component_id in component_cells}
    penetration = manifest.get("penetration")
    if penetration is not None:
        bounds = penetration["bounds_m"]
        minimum = [float(v) for v in bounds["minimum_xyz_m"]]
        maximum = [float(v) for v in bounds["maximum_xyz_m"]]
        require(len(minimum) == len(maximum) == 3 and all(minimum[i] < maximum[i] for i in range(3)), "invalid rectangular penetration bounds")
        samples = int(inp.get("penetration_volume_samples", 1_000_000))
        require(100_000 <= samples <= 20_000_000, "penetration volume sampling must be 100k..20M points")
        rng = random.Random(int(inp.get("penetration_volume_seed", 913_731_507)))
        box_volume_m3 = math.prod(maximum[i] - minimum[i] for i in range(3))
        inside = {component_id: 0 for component_id in component_cells}
        confirmed_port_void = {component_id: 0 for component_id in component_cells}
        # Port-removed points per region domain, classified by position (cm).
        # Each region is checked on its own: no region is assumed to contain
        # or to miss the port without counting the removed points inside it.
        region_inside = {key: 0 for domains in region_domains.values() for key in domains}
        centre_phi = math.atan2((minimum[2] + maximum[2]) / 2.0, (minimum[0] + maximum[0]) / 2.0)
        require(abs(centre_phi) < 1.0e-6, "component regions are defined about toroidal angle 0; the penetration is not centred there")
        for _ in range(samples):
            point = [100.0 * rng.uniform(minimum[i], maximum[i]) for i in range(3)]
            original_path = unperforated_geometry.find(point)
            original_cell = (
                original_path[-1]
                if isinstance(original_path, (list, tuple)) and original_path
                else (None if isinstance(original_path, (list, tuple)) else original_path)
            )
            if original_cell is not None and original_cell.name in inside:
                component_id = original_cell.name
                inside[component_id] += 1
                if component_id in penetration["affected_component_ids"]:
                    for key, domain in region_domains.get(component_id, {}).items():
                        if region_contains(domain["region"], R * scale, point[0], point[2]):
                            region_inside[key] += 1
                    final_path = geometry.find(point)
                    final_cell = final_path[-1] if isinstance(final_path, (list, tuple)) else final_path
                    require(final_cell is not None and final_cell.name == penetration["id"],
                            f"port geometry did not map removed {component_id} points to the explicit void cell")
                    confirmed_port_void[component_id] += 1
        for component_id in penetration["affected_component_ids"]:
            require(component_id in component_cells, f"penetration lists unknown component: {component_id}")
            p = inside[component_id] / samples
            estimate_m3 = box_volume_m3 * p
            error_m3 = box_volume_m3 * math.sqrt(p * (1.0 - p) / samples)
            full = next(c for c in variant["components"] if c["id"] == component_id)["full_torus_volume_m3"]
            component_volume_cm3[component_id] = (full - estimate_m3) * 1.0e6
            require(component_volume_cm3[component_id] > 0, f"penetration removed all of component {component_id}")
            component_volume_se_cm3[component_id] = error_m3 * 1.0e6
        for domains in region_domains.values():
            for key, domain in domains.items():
                component_id = domain["component_id"]
                a_m, b_m = (next(c for c in variant["components"] if c["id"] == component_id)[k] for k in ("inner_minor_radius_m", "outer_minor_radius_m"))
                full = torus_region_volume_m3(R, a_m, b_m, domain["region"])
                if component_id in penetration["affected_component_ids"]:
                    p = region_inside[key] / samples
                    removed = box_volume_m3 * p
                    error = box_volume_m3 * math.sqrt(p * (1.0 - p) / samples)
                    region_volume_cm3[key] = ((full - removed) * 1.0e6, error * 1.0e6)
                    require(full - removed > 0, f"penetration removed all of region {key}")
                else:
                    region_volume_cm3[key] = (full * 1.0e6, 0.0)
        penetration_volume_audit = {
            "scenario_sha256": request["scenario_sha256"],
            "variant_id": physics["variant_id"],
            "method": "uniform_point_classification_in_unperforated_control_geometry_plus_final_port_void_confirmation",
            "seed": int(inp.get("penetration_volume_seed", 913_731_507)),
            "samples": samples,
            "box_volume_m3": box_volume_m3,
            "intersection_estimates_m3": {key: box_volume_m3 * count / samples for key, count in inside.items()},
            "intersection_standard_errors_m3": {key: box_volume_m3 * math.sqrt((count / samples) * (1.0 - count / samples) / samples) for key, count in inside.items()},
            "cell_counts": inside,
            "region_removed_counts": region_inside,
            "region_removed_estimates_m3": {key: box_volume_m3 * count / samples for key, count in region_inside.items()},
            "region_removed_standard_errors_m3": {key: box_volume_m3 * math.sqrt((count / samples) * (1.0 - count / samples) / samples) for key, count in region_inside.items()},
            "final_port_void_confirmation_counts_by_component": confirmed_port_void,
            "fractional_volume_standard_errors_are_binomial": True,
            "independent_of_Rust_midpoint_quadrature": True,
            "not_a_physical_validation": True,
        }
    else:
        for c in variant["components"]:
            component_volume_cm3[c["id"]] = float(c["full_torus_volume_m3"]) * 1.0e6
        for domains in region_domains.values():
            for key, domain in domains.items():
                c = next(c for c in variant["components"] if c["id"] == domain["component_id"])
                region_volume_cm3[key] = (torus_region_volume_m3(R, c["inner_minor_radius_m"], c["outer_minor_radius_m"], domain["region"]) * 1.0e6, 0.0)

    low = -(R + outer_minor) * scale
    high = (R + outer_minor) * scale
    ylow, yhigh = -outer_minor * scale, outer_minor * scale
    source = openmc.IndependentSource(
        space=openmc.stats.Box((low, ylow, low), (high, yhigh, high)),
        angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([request["source"]["neutron_energy_ev"]], [1.0]),
        particle="neutron",
        constraints={"domains": [plasma], "rejection_strategy": "resample"},
    )
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.batches = int(sampling["batches"])
    # One statepoint per batch gives batch-resolved scalar tallies for the
    # response covariance; the worker deletes all but the final statepoint.
    settings.statepoint = {"batches": list(range(1, int(sampling["batches"]) + 1))}
    settings.particles = int(sampling["particles_per_batch"])
    settings.seed = int(sampling["seed"])
    coupled_heating_required = any(r["score"]["kind"] == "heating" for r in request["responses"])
    settings.photon_transport = coupled_heating_required
    # If photoatomic data lacks relaxation transitions, explicitly omit the
    # fluorescence/Auger cascade rather than allow OpenMC 0.15.3 to index an
    # empty shell map. This approximation is recorded in the worker receipt.
    settings.atomic_relaxation = bool(photon_physics.get("atomic_relaxation_enabled", False))
    settings.electron_treatment = "led"
    settings.source = source
    # HDF kT is checked against the requested target at 0.1 K. OpenMC 0.15.3
    # resolves continuous-energy tables through rounded labels (e.g. 294K),
    # so this separate 1 K window only admits that label rounding at runtime.
    settings.temperature = {"default": data_runtime_temperature, "method": "nearest", "tolerance": OPENMC_LABEL_TOLERANCE_K}

    particle_filters = {
        "neutron": openmc.ParticleFilter("neutron"),
        "photon": openmc.ParticleFilter("photon"),
        "electron": openmc.ParticleFilter("electron"),
        "positron": openmc.ParticleFilter("positron"),
    }
    spectrum_tallies = {}
    spectrum_energy_filter = openmc.EnergyFilter(SPECTRUM_EDGES_EV)
    for component_id in component_cells:
        for particle in (("neutron", "photon") if coupled_heating_required else ("neutron",)):
            spectrum = openmc.Tally(name=f"spectrum-{particle}-{component_id}")
            spectrum.filters = [component_filters[component_id], particle_filters[particle], spectrum_energy_filter]
            spectrum.scores = ["flux"]
            spectrum.estimator = "tracklength"
            spectrum_tallies[(component_id, particle)] = spectrum
    if request.get("activation_spectra") is not None:
        require(request["activation_spectra"] == ACTIVATION_SPECTRA_STRUCTURE, "unsupported activation_spectra group structure")
        activation_filter = openmc.EnergyFilter(ACTIVATION_709_EDGES_EV)
        for component_id in component_cells:
            activation = openmc.Tally(name=f"spectrum-{ACTIVATION_SPECTRA_STRUCTURE}-{component_id}")
            activation.filters = [component_filters[component_id], particle_filters["neutron"], activation_filter]
            activation.scores = ["flux"]
            activation.estimator = "tracklength"
            spectrum_tallies[(component_id, ACTIVATION_SPECTRA_STRUCTURE)] = activation
    tallies = openmc.Tallies(list(spectrum_tallies.values()))
    response_by_tally = {}
    energy_filters = {}
    for index, response in enumerate(request["responses"], start=1):
        domain = response["domain"]
        score = response["score"]
        tally = openmc.Tally(name=f"response-{response['id']}")
        if domain["kind"] == "component_region":
            require(domain["component_id"] in component_cells, f"response references unknown component {domain['component_id']}")
            tally.filters = [openmc.CellFilter(region_cell_by_domain[domain_key(domain)])]
        elif domain["kind"] == "component":
            comp = domain["component_id"]
            require(comp in component_cells, f"response references unknown component {comp}")
            tally.filters = [component_filters[comp]]
        elif domain["kind"] == "whole_model":
            pass
        elif domain["kind"] == "mesh":
            # The full mesh tally is constructed once below; per-bin definitions map to its bins.
            continue
        else:
            raise ValueError(f"unsupported response domain {domain['kind']}")
        kind = score["kind"]
        if kind == "flux":
            tally.filters.append(particle_filters["neutron"])
            tally.scores = ["flux"]
        elif kind == "flux_above":
            # Neutron track length above the bound, up to the top of the
            # spectrum range; scalar per cell, so its per-batch values feed the
            # response covariance like every other scalar response.
            lower = float(score["energy_min_ev"])
            require(0.0 < lower < SPECTRUM_EDGES_EV[-1], "flux-above lower bound is outside the energy range")
            energy_filter = energy_filters.setdefault(lower, openmc.EnergyFilter([lower, SPECTRUM_EDGES_EV[-1]]))
            tally.filters.append(particle_filters["neutron"])
            tally.filters.append(energy_filter)
            tally.scores = ["flux"]
        elif kind == "reaction_rate":
            tally.filters.append(particle_filters["neutron"])
            tally.scores = [score["reaction"]]
        elif kind == "particle_production":
            tally.filters.append(particle_filters["neutron"])
            tally.scores = [score["score"]]
        elif kind == "heating":
            require(score.get("convention") == "heating", "only coupled OpenMC heating is available; heating-local/MT=901 is unsupported")
            scope = score.get("particle_scope")
            if scope == "neutron":
                tally.filters.append(particle_filters["neutron"])
            elif scope == "photon":
                tally.filters.append(particle_filters["photon"])
            elif scope == "electron":
                tally.filters.append(particle_filters["electron"])
            elif scope == "positron":
                tally.filters.append(particle_filters["positron"])
            elif scope != "total":
                raise ValueError(f"unsupported heating particle scope: {scope}")
            tally.scores = ["heating"]
            tally.estimator = "collision"
        else:
            raise ValueError(f"unsupported or unavailable score kind: {kind}")
        if kind != "heating":
            tally.estimator = "tracklength"
        tallies.append(tally)
        response_by_tally[tally.name] = response

    mesh_meta = inp.get("mesh")
    mesh_response_defs = [r for r in request["responses"] if r["domain"]["kind"] == "mesh"]
    mesh_tally = None
    mesh_index_audit = None
    if mesh_response_defs:
        require(mesh_meta is not None, "mesh-domain responses require mesh input metadata")
        dims = tuple(int(v) for v in mesh_meta["dimensions"])
        require(len(dims) == 3 and math.prod(dims) <= 32768, "mesh dimensions exceed adapter bound")
        mesh = openmc.RegularMesh()
        mesh.dimension = dims
        mesh.lower_left = [100.0 * v for v in mesh_meta["lower_left_m"]]
        mesh.upper_right = [100.0 * v for v in mesh_meta["upper_right_m"]]
        api_indices = list(mesh.indices)
        expected_indices = [
            (x, y, z)
            for z in range(1, dims[2] + 1)
            for y in range(1, dims[1] + 1)
            for x in range(1, dims[0] + 1)
        ]
        require(api_indices == expected_indices, "OpenMC RegularMesh.indices order disagrees with requested x-fast bin order")
        require(api_indices[0] == (1, 1, 1), "OpenMC mesh bin zero is not index (1,1,1)")
        tested_rollovers = {"bin_0": list(api_indices[0])}
        if dims[0] > 1:
            require(api_indices[1] == (2, 1, 1), "OpenMC mesh bin 1 does not increment x first")
            tested_rollovers["bin_1"] = list(api_indices[1])
        if dims[1] > 1:
            require(api_indices[dims[0]] == (1, 2, 1), "OpenMC mesh x-row rollover does not increment y")
            tested_rollovers["bin_nx"] = list(api_indices[dims[0]])
        if dims[2] > 1:
            require(api_indices[dims[0] * dims[1]] == (1, 1, 2), "OpenMC mesh plane rollover does not increment z")
            tested_rollovers["bin_nx_times_ny"] = list(api_indices[dims[0] * dims[1]])
        mesh_index_audit = {
            "api": "openmc.RegularMesh.indices",
            "ordering": "x-fastest, then y, then z; 1-based tuple indices correspond to zero-based flat bin i + nx*(j + ny*k)",
            "first_index_tuples": [list(v) for v in api_indices[: min(4, len(api_indices))]],
            "tested_rollovers": tested_rollovers,
            "assertion": "PASS",
        }
        mesh_tally = openmc.Tally(name="spatial-neutron-flux")
        mesh_tally.filters = [openmc.MeshFilter(mesh), particle_filters["neutron"]]
        mesh_tally.scores = ["flux"]
        mesh_tally.estimator = "tracklength"
        tallies.append(mesh_tally)
        expected = {int(r["domain"]["bin"]): r for r in mesh_response_defs}
        require(set(expected) == set(range(math.prod(dims))), "mesh response bins must exactly cover the declared mesh")
        require(all(r["domain"]["mesh_id"] == mesh_meta["id"] for r in mesh_response_defs), "mesh ID mismatch")

    model = openmc.Model(geometry=geometry, materials=openmc.Materials([m for m in materials.values() if m is not None]), settings=settings, tallies=tallies)
    model.export_to_xml(directory=str(out))
    # Exact analytic full-torus volume for component shells and the whole modeled outer torus.
    volumes_cm3 = {}
    for c in variant["components"]:
        a, b = c["inner_minor_radius_m"], c["outer_minor_radius_m"]
        volumes_cm3[c["id"]] = component_volume_cm3[c["id"]]
        expected_m3 = 2.0 * math.pi**2 * R * (b*b-a*a)
        require(math.isclose(float(c["full_torus_volume_m3"]), expected_m3, rel_tol=1.0e-12, abs_tol=1.0e-12), f"manifest torus volume mismatch for {c['id']}")
    whole_minor = plasma_minor + gap + sum(float(c["thickness_m"]) for c in variant["components"])
    whole_volume_cm3 = 2.0 * math.pi**2 * R * whole_minor**2 * 1.0e6
    mesh_bin_volume_cm3 = None
    if mesh_meta:
        widths = [(mesh_meta["upper_right_m"][i]-mesh_meta["lower_left_m"][i])/mesh_meta["dimensions"][i] for i in range(3)]
        mesh_bin_volume_cm3 = math.prod(widths)*1.0e6
    return model, response_by_tally, mesh_tally, expected if mesh_tally else {}, volumes_cm3, component_volume_se_cm3, whole_volume_cm3, mesh_bin_volume_cm3, component_cells, spectrum_tallies, plasma, mesh_index_audit, penetration_volume_audit, geometry_ownership_audit, region_volume_cm3


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    out = args.output_dir.expanduser().resolve()
    require(not out.exists(), "output directory must be fresh; refusing overwrite")
    out.mkdir(parents=True)
    record = {"execution_status": "NOT_STARTED", "scientific_status": "NOT_EVALUATED"}
    solver_output = None
    statepoint_identity = None
    exported_xml_hashes = {}
    mesh_index_audit = None
    penetration_volume_audit = None
    geometry_ownership_audit = None
    try:
        inp = read_input(args.input.expanduser().resolve(strict=True))
        request = inp["request"]
        exe = Path(inp["openmc_executable"]).expanduser().resolve(strict=True)
        xml = Path(inp["cross_sections"]).expanduser().resolve(strict=True)
        require(os.access(exe, os.X_OK), "OpenMC executable is not executable")
        data_digest = inp["nuclear_data_digest"]
        require(data_digest.startswith("sha256:") and len(data_digest) == 71, "nuclear_data_digest must be a sha256-prefixed 64-hex digest")
        import openmc
        require(openmc.__version__ == "0.15.3", f"expected OpenMC 0.15.3, got {openmc.__version__}")
        coupled_heating_required = any(r["score"]["kind"] == "heating" for r in inp["request"]["responses"])
        xml_hash, required_nuclides, actual_data_temps, runtime_data_temperature, photon_data_hashes, photon_physics = check_data_identity(inp["physics"], xml, openmc, coupled_heating_required)
        # Set only the path; all scored quantities remain raw, per source neutron.
        os.environ["OPENMC_CROSS_SECTIONS"] = str(xml)
        model, response_by_tally, mesh_tally, mesh_defs, volumes_cm3, component_volume_se_cm3, whole_cm3, mesh_bin_cm3, component_cells, spectrum_tallies, plasma, mesh_index_audit, penetration_volume_audit, geometry_ownership_audit, region_volumes_cm3 = compose(inp, out, openmc, runtime_data_temperature, photon_physics)
        geometry_ownership_audit["input_sha256"] = sha256(args.input)
        exported_xml = sorted(out.glob("*.xml"))
        require(exported_xml, "OpenMC model export produced no XML inputs")
        exported_xml_hashes = {path.name: sha256(path) for path in exported_xml}
        command = [str(exe)]
        run_env = os.environ.copy()
        run_env["OMP_NUM_THREADS"] = str(int(inp["sampling"]["threads"]))
        run_env.setdefault("OPENBLAS_NUM_THREADS", "1")
        solver_output = run_solver_streaming(command, out, run_env, MAX_SOLVER_LOG_BYTES)
        require(solver_output["stop_reason"] is None, solver_output["stop_reason"] or "solver output forwarding failed")
        require(solver_output["return_code"] == 0, f"OpenMC exited {solver_output['return_code']}; inspect bounded output logs")
        log = (out / "openmc.stdout.log").read_bytes() + b"\n" + (out / "openmc.stderr.log").read_bytes()
        require(not re.search(rb"(?i)particle\s+\d+\s+was lost|lost particles?", log), "OpenMC reported lost particles")
        n_batches = int(inp["sampling"]["batches"])
        batch_sums = read_batch_sums(openmc, out, n_batches, list(response_by_tally))
        statepoints = sorted(out.glob("statepoint.*.h5"))
        require(statepoints, "OpenMC produced no statepoint")
        require(len(statepoints) == 1, "expected exactly one final statepoint")
        statepoint_identity = {"filename": statepoints[-1].name, "size_bytes": statepoints[-1].stat().st_size, "sha256": sha256(statepoints[-1])}
        with openmc.StatePoint(str(statepoints[-1])) as sp:
            expected_version = tuple(int(part) for part in openmc.__version__.split(".")[:3])
            observed_version = tuple(int(part) for part in sp.version)
            require(observed_version == expected_version == (0, 15, 3), f"statepoint version {observed_version} does not match requested OpenMC {expected_version}")
            require(sp.run_mode == "fixed source", f"statepoint run mode is {sp.run_mode!r}, not fixed source")
            require(int(sp.seed) == int(inp["sampling"]["seed"]), "statepoint seed differs from request")
            require(int(sp.n_batches) == int(inp["sampling"]["batches"]), "statepoint n_batches differs from request")
            require(int(sp.current_batch) == int(inp["sampling"]["batches"]), "statepoint current_batch is incomplete")
            require(int(sp.n_realizations) == int(inp["sampling"]["batches"]), "statepoint realization count differs from requested batches")
            require(int(sp.n_particles) == int(inp["sampling"]["particles_per_batch"]), "statepoint particles per batch differs from request")
            raw_tallies = []
            batch_columns = {}
            for name, response in response_by_tally.items():
                tally = sp.get_tally(name=name)
                expected_estimator = "collision" if response["score"]["kind"] == "heating" else "tracklength"
                require(tally.estimator == expected_estimator, f"response {response['id']} estimator is {tally.estimator!r}; expected {expected_estimator!r}")
                means = tally.mean.ravel()
                errors = tally.std_dev.ravel()
                require(len(means) == 1 and len(errors) == 1, f"response {response['id']} did not produce one scalar")
                mean, se = float(means[0]), float(errors[0])
                batch_values = per_batch_values(batch_sums[name])
                verify_batch_values(response["id"], batch_values, mean, se)
                batch_columns[response["id"]] = batch_values
                signed_heating = response["score"]["kind"] == "heating"
                require(math.isfinite(mean) and (signed_heating or mean >= 0) and math.isfinite(se) and se >= 0, f"response {response['id']} has an invalid mean or standard error")
                domain = response["domain"]
                if domain["kind"] == "component":
                    volume = volumes_cm3[domain["component_id"]]
                elif domain["kind"] == "component_region":
                    volume = region_volumes_cm3[domain_key(domain)][0]
                elif domain["kind"] == "whole_model":
                    volume = whole_cm3
                else:
                    raise ValueError("unsupported response domain")
                score = response["score"]["kind"]
                unit = "ev_per_source" if score == "heating" else ("cm_per_source" if score in ("flux", "flux_above") else ("particles_per_source" if score == "particle_production" else "events_per_source"))
                raw_tallies.append({"response_id": response["id"], "estimator": tally.estimator, "unit": unit, "mean": mean, "standard_error": se})
            covariance_ids = list(batch_columns)
            covariance = batch_mean_covariance([batch_columns[i] for i in covariance_ids])
            batch_values_doc = {
                "schema_version": BATCH_VALUES_SCHEMA,
                "method": COVARIANCE_METHOD,
                "input_sha256": sha256(args.input),
                "n_batches": n_batches,
                "response_ids": covariance_ids,
                "unit": "raw tally units per source neutron, per batch (difference of consecutive cumulative statepoint sums)",
                "values": batch_columns,
            }
            (out / BATCH_VALUES_FILE).write_text(json.dumps(batch_values_doc, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            response_covariance = {
                "method": COVARIANCE_METHOD,
                "batches": n_batches,
                "response_ids": covariance_ids,
                "raw_per_source": [v for row in covariance for v in row],
                "batch_values_file": BATCH_VALUES_FILE,
                "batch_values_sha256": sha256(out / BATCH_VALUES_FILE),
            }
            if mesh_tally is not None:
                tally = sp.get_tally(name=mesh_tally.name)
                require(tally.estimator == "tracklength", "spatial flux estimator is not explicitly tracklength")
                means = tally.mean.ravel(order="F")
                errors = tally.std_dev.ravel(order="F")
                require(len(means) == len(mesh_defs) and len(errors) == len(mesh_defs), "mesh tally bin count mismatch")
                for bin_id, response in sorted(mesh_defs.items()):
                    mean, se = float(means[bin_id]), float(errors[bin_id])
                    require(math.isfinite(mean) and mean >= 0 and math.isfinite(se) and se >= 0, f"mesh bin {bin_id} non-finite/negative")
                    raw_tallies.append({"response_id": response["id"], "estimator": tally.estimator, "unit": "cm_per_source", "mean": mean, "standard_error": se})
            spectra = []
            for (component_id, particle), expected_tally in spectrum_tallies.items():
                tally = sp.get_tally(name=expected_tally.name)
                if particle == ACTIVATION_SPECTRA_STRUCTURE:
                    require(tally.estimator == "tracklength", f"activation spectrum estimator is not explicitly tracklength for {component_id}")
                    means = tally.mean.ravel()
                    errors = tally.std_dev.ravel()
                    require(len(means) == len(ACTIVATION_709_EDGES_EV) - 1 and len(errors) == len(means), f"activation spectrum bin count mismatch for {component_id}")
                    require(all(math.isfinite(float(x)) and float(x) >= 0 for x in list(means) + list(errors)), f"activation spectrum invalid for {component_id}")
                    flux_raw = next(t for t in raw_tallies if t["response_id"] == f"{component_id}-flux")
                    require(sum(float(x) for x in means) <= flux_raw["mean"] * (1.0 + 1.0e-8), f"activation spectrum exceeds integrated flux for {component_id}")
                    spectra.append({"component_id": component_id, "particle": "neutron", "group_structure": ACTIVATION_SPECTRA_STRUCTURE, "estimator": tally.estimator, "unit": "cm_per_source_per_energy_bin", "energy_edges_ev": ACTIVATION_709_EDGES_EV, "mean_cm_per_source_per_bin": [float(x) for x in means], "standard_error_cm_per_source_per_bin": [float(x) for x in errors], "volume_cm3": volumes_cm3[component_id], "volume_standard_error_cm3": component_volume_se_cm3[component_id]})
                    continue
                require(tally.estimator == "tracklength", f"spectrum estimator is not explicitly tracklength for {particle}/{component_id}")
                means = tally.mean.ravel()
                errors = tally.std_dev.ravel()
                require(len(means) == len(SPECTRUM_EDGES_EV) - 1, f"spectrum bin count mismatch for {particle}/{component_id}")
                require(all(math.isfinite(float(x)) and float(x) >= 0 for x in means), f"spectrum mean invalid for {particle}/{component_id}")
                require(all(math.isfinite(float(x)) and float(x) >= 0 for x in errors), f"spectrum standard error invalid for {particle}/{component_id}")
                spectra.append({"component_id": component_id, "particle": particle, "estimator": tally.estimator, "unit": "cm_per_source_per_energy_bin", "energy_edges_ev": SPECTRUM_EDGES_EV, "mean_cm_per_source_per_bin": [float(x) for x in means], "standard_error_cm_per_source_per_bin": [float(x) for x in errors], "volume_cm3": volumes_cm3[component_id], "volume_standard_error_cm3": component_volume_se_cm3[component_id]})
                if particle == "neutron":
                    flux_response = next((r for r in request["responses"] if r["id"] == f"{component_id}-flux"), None)
                    require(flux_response is not None, f"missing integrated flux response for {component_id}")
                    flux_raw = next(t for t in raw_tallies if t["response_id"] == flux_response["id"])
                    require(abs(sum(float(x) for x in means) - flux_raw["mean"]) <= 1.0e-8 * max(abs(flux_raw["mean"]), 1.0e-30), f"full-range neutron spectrum does not sum to integrated flux for {component_id}")
            volumes = []
            for response in inp["request"]["responses"]:
                d = response["domain"]
                if d["kind"] == "component":
                    v = volumes_cm3[d["component_id"]]
                    se = component_volume_se_cm3[d["component_id"]]
                elif d["kind"] == "component_region":
                    v, se = region_volumes_cm3[domain_key(d)]
                elif d["kind"] == "whole_model":
                    v = whole_cm3
                    se = 0.0
                elif d["kind"] == "mesh":
                    v = mesh_bin_cm3
                    se = 0.0
                else:
                    raise ValueError(f"unsupported volume domain: {d['kind']}")
                if not any(item["domain"] == d for item in volumes):
                    volumes.append({"domain": d, "value": v, "standard_error": se, "unit": "cubic_centimetre"})
            artifact = {"schema_version": ARTIFACT_SCHEMA, "request": inp["request"], "solver": {"name": "OpenMC", "version": openmc.__version__, "digest": f"sha256:{sha256(exe)}"}, "nuclear_data": {"name": inp["physics"].get("nuclear_data", {}).get("name", "external cross_sections.xml"), "version": inp["physics"].get("nuclear_data", {}).get("version", "unselected-local-library"), "digest": data_digest}, "histories": int(sp.n_realizations)*int(inp["sampling"]["particles_per_batch"]), "volumes": volumes, "tallies": raw_tallies, "response_covariance": response_covariance}
            (out / "transport-artifact.json").write_text(json.dumps(artifact, indent=2, sort_keys=True)+"\n", encoding="utf-8")
            (out / "transport-spectra.json").write_text(json.dumps({"schema_version":"faris-transport-spectra/v0.1","request":request,"scenario_sha256":request["scenario_sha256"],"variant_id":request["variant_id"],"input_sha256":sha256(args.input),"solver":artifact["solver"],"nuclear_data":artifact["nuclear_data"],"histories":artifact["histories"],"spectra":spectra}, indent=2, sort_keys=True)+"\n", encoding="utf-8")
        after_export_xml_hashes = {path.name: sha256(path) for path in sorted(out.glob("*.xml"))}
        require(after_export_xml_hashes == exported_xml_hashes, "OpenMC export XML changed during solver execution")
        precision_report = sampling_precision_report(inp["request"], raw_tallies, volumes)
        record.update({"execution_status":"COMPLETED","scientific_status":"NOT_EVALUATED","histories":artifact["histories"],"solver":artifact["solver"],"nuclear_data":artifact["nuclear_data"],"photon_data_sha256":photon_data_hashes,"photon_physics":photon_physics,"penetration_volume_audit":penetration_volume_audit,"geometry_ownership_audit":geometry_ownership_audit,"sampling_precision":precision_report,"requested_nuclear_data_temperature_K":sorted({m["recipe"]["nuclear_data_temperature_k"] for m in inp["physics"]["materials"] if m["recipe"]["kind"] == "nuclide_mixture"}),"stored_nuclear_data_temperatures_K":sorted(set(actual_data_temps.values())),"openmc_data_group_temperature_label_K":runtime_data_temperature,"openmc_nearest_label_tolerance_K":OPENMC_LABEL_TOLERANCE_K,"openmc_statepoint_version":list(observed_version),"statepoint":statepoint_identity,"export_xml_sha256":exported_xml_hashes,"solver_output_capture":solver_output,"mesh_index_audit":mesh_index_audit,"transport_artifact":"transport-artifact.json","transport_artifact_sha256":sha256(out / "transport-artifact.json"),"transport_batch_values":BATCH_VALUES_FILE,"transport_batch_values_sha256":sha256(out / BATCH_VALUES_FILE),"response_covariance_method":COVARIANCE_METHOD,"transport_spectra":"transport-spectra.json","transport_spectra_sha256":sha256(out / "transport-spectra.json"),"responses":len(raw_tallies),"normalization":"RAW_PER_SOURCE_NEUTRON; no absolute source normalization in Python","lost_particle_check":"no lost-particle log indication; statepoint present"})
    except Exception as error:
        record.update({"execution_status":"FAILED","scientific_status":"NOT_EVALUATED","error":f"{type(error).__name__}: {error}"})
        if solver_output is not None:
            record["solver_output_capture"] = solver_output
        if statepoint_identity is not None:
            record["statepoint"] = statepoint_identity
        if exported_xml_hashes:
            record["export_xml_sha256"] = exported_xml_hashes
        if mesh_index_audit is not None:
            record["mesh_index_audit"] = mesh_index_audit
        if penetration_volume_audit is not None:
            record["penetration_volume_audit"] = penetration_volume_audit
        if geometry_ownership_audit is not None:
            record["geometry_ownership_audit"] = geometry_ownership_audit
        (out / "worker-result.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
        raise
    (out / "worker-result.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
