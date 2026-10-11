"""Shared constants and rules for stage S0e (docs/notes/CAD_TRANSPORT_S0E.md).

The rule functions are pure so test_s0e.py can check them without OpenMC. The OpenMC helpers take the module as an
argument, as elsewhere in this folder, so either installed version can call them.
"""
from __future__ import annotations

import hashlib
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0b_common as C  # noqa: E402
import s0d_common as D  # noqa: E402

CAP_S = 75 * 60.0            # machine time for the whole stage (jobs only; waiting for a quiet machine is not counted)
PRODUCTION_S = D.PRODUCTION_S
MAX_SPLIT = D.MAX_SPLIT
Z_UNBIASED = C.Z_UNBIASED
H_SIGMA = 3.0                # H5 and H6: gain above E0's by more than this many combined sigma ...
H_MIN_FACTOR = 1.2           # ... and by at least this factor
CUTOFF_EV = 1.0e5            # E1 and E3: settings.cutoff['energy_neutron']
QUIET_LOAD = 2.0             # a production starts only below this 1-minute load average
CONTENDED_LOAD = 6.0         # end-of-run load above this is reported as "contended"
BUILD_LOCK = Path("/home/connoravila/.cache/avila-night/faris-wt/cargo.lock")
NUC050_TARGET = D.NUC050_TARGET

# Disjoint from S0 (2026xxxx, 8115xxxx), S0b (9001xxxx-9010xxxx), S0c (9101xxxx-9107xxxx) and S0d (9011xxxx, 9012xxxx).
SEEDS = {"gen": {"smoke": 90130000, "E2": 90130002},
         "prod": {"E0": 90140000, "E1": 90140001, "E2": 90140002, "E3": 90140003}}

# Reference windows for E0, E1: S0d T2's file, unchanged.
T2_WINDOWS_REL = "s0d/t2/gen/weight_windows.h5"

# Estimates (seconds) used only to decide whether an item fits the cap; protocol "Estimate and footprint".
ESTIMATE_S = {"smoke": 180.0, "E0": 660.0, "E1": 660.0, "E3": 660.0, "E2": 1200.0}


# ---------------------------------------------------------------------------
# The cylindrical mesh for H6 ("The cylindrical mesh for H6")

def _equal_cells(a: float, b: float, n: int) -> list[float]:
    """Edges after `a` up to and including `b` in n equal cells."""
    return [a + (b - a) * i / n for i in range(1, n + 1)]


def cylindrical_edges() -> dict:
    """R, Z (cm) and phi (rad) edges of the H6 mesh, as the protocol lists them.

    R: 0, 32, 64, 96, 128, 129; TiH2 129-180 in 10 equal cells; 183, 186; FLiBe in 2 equal cells to 206; 213, 214, 217;
    plasma 217-443 in 9 equal cells; 446, 447, 454; FLiBe 454-554 in 4 equal cells; 557, 560; TiH2 560-580 in 4 equal
    cells; 581; TF coil 581-645 in 4 equal cells; 670, 700. Z: -400 to 400 in 25 cm steps. phi: 0 to 360 deg in 5 deg steps.
    """
    r = [0.0, 32.0, 64.0, 96.0, 128.0, 129.0]
    r += _equal_cells(129.0, 180.0, 10) + [183.0, 186.0]
    r += _equal_cells(186.0, 206.0, 2) + [213.0, 214.0, 217.0]
    r += _equal_cells(217.0, 443.0, 9) + [446.0, 447.0, 454.0]
    r += _equal_cells(454.0, 554.0, 4) + [557.0, 560.0]
    r += _equal_cells(560.0, 580.0, 4) + [581.0]
    r += _equal_cells(581.0, 645.0, 4) + [670.0, 700.0]
    z = [-400.0 + 25.0 * i for i in range(33)]
    phi = [math.radians(5.0 * i) for i in range(73)]
    return {"r_cm": r, "z_cm": z, "phi_rad": phi}


def edges_sha256(edges: dict) -> str:
    """sha256 of a canonical JSON of the edge lists (sorted keys, no spaces, floats rounded to 9 decimals)."""
    canonical = {k: [round(float(x), 9) for x in v] for k, v in edges.items()}
    text = json.dumps(canonical, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def mesh_shape(edges: dict) -> tuple[int, int, int]:
    return len(edges["r_cm"]) - 1, len(edges["z_cm"]) - 1, len(edges["phi_rad"]) - 1


def build_cylindrical_mesh(openmc, name: str = "ww-mesh"):
    """The H6 CylindricalMesh (origin on the machine axis) and its record; one mesh for source regions and windows."""
    edges = cylindrical_edges()
    mesh = openmc.CylindricalMesh(r_grid=edges["r_cm"], z_grid=edges["z_cm"], phi_grid=edges["phi_rad"],
                                  origin=(0.0, 0.0, 0.0), name=name)
    nr, nz, nphi = mesh_shape(edges)
    record = {"mesh_type": "cylindrical", "origin": [0.0, 0.0, 0.0], "dimension_r_z_phi": [nr, nz, nphi],
              "bins": nr * nz * nphi, "edges_sha256": edges_sha256(edges), "edges": edges}
    return mesh, record


# ---------------------------------------------------------------------------
# Breakdown tallies ("Breakdown tallies, in every production run")

N_COILS = 18
COIL_NAMES = [f"tf_coil_{k:02d}" for k in range(N_COILS)]
PORT_ADJACENT_DEG = (10.0, 350.0)


def map_edges() -> dict:
    """(R, Z) map: R 0-700 cm and Z -400 to 400 cm in 20 cm bins, one phi bin over the full turn."""
    return {"r_cm": [20.0 * i for i in range(36)], "z_cm": [-400.0 + 20.0 * i for i in range(41)],
            "phi_rad": [0.0, 2.0 * math.pi]}


def coil_volume_ids(roles: list[dict]) -> list[tuple[str, int]]:
    """[(tf_coil_NN, volume id)] in coil order; the roles file names the 18 TF coil volumes."""
    by_name = {r["name"]: int(r["volume_id"]) for r in roles}
    missing = [n for n in COIL_NAMES if n not in by_name]
    if missing:
        raise ValueError(f"roles file lacks TF coil volumes: {missing}")
    return [(n, by_name[n]) for n in COIL_NAMES]


def breakdown_tallies(openmc, roles: list[dict], tf_material, fast_cut_ev: float = 1.0e5, top_ev: float = 2.0e7):
    """The two breakdown tallies: per-coil cell filter, and the (R, Z) map with the TF material filter. Flux above 0.1 MeV."""
    coils = openmc.Tally(name="tf_coil_cells")
    coils.filters = [openmc.CellFilter([vid for _, vid in coil_volume_ids(roles)]), openmc.EnergyFilter([fast_cut_ev, top_ev])]
    coils.scores = ["flux"]
    e = map_edges()
    mesh = openmc.CylindricalMesh(r_grid=e["r_cm"], z_grid=e["z_cm"], phi_grid=e["phi_rad"], origin=(0.0, 0.0, 0.0),
                                  name="breakdown-map")
    rz = openmc.Tally(name="tf_rz_map")
    rz.filters = [openmc.MeshFilter(mesh), openmc.MaterialFilter([tf_material]), openmc.EnergyFilter([fast_cut_ev, top_ev])]
    rz.scores = ["flux"]
    return [coils, rz]


def read_breakdown(sp, roles: list[dict]) -> dict:
    """Breakdown tallies from an open StatePoint: per-coil (name, volume id, mean, std_error), and the (R, Z) map.

    The map's means are flattened with R fastest, then Z (one phi bin), as OpenMC orders mesh bins.
    """
    coil_tally = sp.get_tally(name="tf_coil_cells")
    ids = [int(b) for b in coil_tally.filters[0].bins]
    name_of = {vid: n for n, vid in coil_volume_ids(roles)}
    means, sds = coil_tally.mean.ravel().tolist(), coil_tally.std_dev.ravel().tolist()
    coils = [{"name": name_of[i], "volume_id": i, "mean": m, "std_error": s} for i, m, s in zip(ids, means, sds)]
    coils.sort(key=lambda c: c["name"])
    rz = sp.get_tally(name="tf_rz_map")
    e = map_edges()
    return {"per_coil": coils, "rz_map": {"r_edges_cm": e["r_cm"], "z_edges_cm": e["z_cm"], "order": "R fastest, then Z",
                                          "mean": rz.mean.ravel().tolist(), "std_error": rz.std_dev.ravel().tolist()},
            "unit": "cm per source neutron"}


# ---------------------------------------------------------------------------
# Rules ("Rules"). rows map E0..E3 to {"gain", "sigma", "unbiased"}; a missing row was not run.

def unbiased(z: float | None) -> bool | None:
    """|z| of (a) against D2 <= 3 (S0b's rule, as S0d)."""
    return C.unbiased(z, Z_UNBIASED)


def _has_gain(r: dict | None) -> bool:
    return r is not None and r.get("gain") is not None and r.get("sigma") is not None


def beats_reference(row: dict, ref: dict) -> bool:
    """Gain above the reference's by more than 3 combined sigma and by a factor of at least 1.2."""
    return (D.exceeds(row["gain"], row["sigma"], ref["gain"], ref["sigma"], H_SIGMA)
            and ref["gain"] > 0 and row["gain"] / ref["gain"] >= H_MIN_FACTOR)


def h5_rule(e0: dict | None, e1: dict | None) -> dict:
    """H5 (cut-off). Confirmed when E1 is unbiased and beats E0; rejected when unbiased and not; biased when E1 is not unbiased."""
    if not _has_gain(e0) or not _has_gain(e1) or e1.get("unbiased") is None:
        return {"outcome": "NOT_EVALUATED"}
    if e1["unbiased"] is not True:
        return {"outcome": "biased", "note": "E1 is not unbiased: falsifies fact 3; the cut-off is not adopted until explained"}
    return {"outcome": "H5 confirmed" if beats_reference(e1, e0) else "H5 rejected"}


def h6_rule(e0: dict | None, e2: dict | None) -> dict:
    """H6 (aligned mesh). Confirmed when E2 is unbiased and beats E0; rejected otherwise; NOT_EVALUATED when E2 has no gain."""
    if not _has_gain(e0) or not _has_gain(e2):
        return {"outcome": "NOT_EVALUATED"}
    if e2.get("unbiased") is not True:
        return {"outcome": "H6 rejected", "note": "E2 is not unbiased" if e2.get("unbiased") is False else "E2 has no bias verdict"}
    return {"outcome": "H6 confirmed" if beats_reference(e2, e0) else "H6 rejected"}


def e3_required(h5: dict | None, h6: dict | None) -> bool:
    """E3 runs only if H5 and H6 are both confirmed."""
    return bool(h5 and h6 and h5.get("outcome") == "H5 confirmed" and h6.get("outcome") == "H6 confirmed")


def nuc050_band(gstar: float | None) -> str:
    return D.nuc050_band(gstar)


def g_star(rows: dict[str, dict]) -> dict:
    """The largest gain among unbiased E0-E3, and the NUC-050 band it falls in."""
    usable = [k for k in ("E0", "E1", "E2", "E3") if _has_gain(rows.get(k)) and rows[k].get("unbiased") is True]
    best = max(usable, key=lambda k: rows[k]["gain"], default=None)
    value = rows[best]["gain"] if best else None
    return {"best": best, "g_star": value, "nuc050": nuc050_band(value)}


def repeatability_z(e0: dict | None, t2_gain: float | None, t2_sigma: float | None) -> float | None:
    """z of the difference between E0's gain and S0d T2's (E0 against T2); reported only, no rule depends on it."""
    if not _has_gain(e0) or t2_gain is None or t2_sigma is None:
        return None
    sd = math.hypot(e0["sigma"], t2_sigma)
    return (e0["gain"] - t2_gain) / sd if sd > 0 else None


# ---------------------------------------------------------------------------
# Quiet machine and load

def load1(loadavg_text: str) -> float | None:
    """The 1-minute load average from /proc/loadavg text."""
    try:
        return float(loadavg_text.split()[0])
    except (IndexError, ValueError):
        return None


def quiet_now(load_1min: float | None, lock_free: bool) -> bool:
    """A production starts only below the load limit and with no cargo build holding the lock."""
    return load_1min is not None and load_1min < QUIET_LOAD and lock_free


def contended(loadavg_end: str) -> bool:
    """The run is reported contended when the end-of-run 1-minute load exceeds 6.0."""
    v = load1(loadavg_end)
    return v is not None and v > CONTENDED_LOAD


def fits(used_s: float, estimate_s: float, cap_s: float = CAP_S) -> bool:
    return used_s + estimate_s <= cap_s
