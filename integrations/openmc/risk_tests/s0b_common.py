"""Pure helpers for the S0b diagnostics (docs/notes/CAD_TRANSPORT_DIAGNOSTICS_S0B.md). Standard library only.

Everything here is a rule or a statistic the frozen protocol fixes: figure of merit and its uncertainty, z-scores, the D1
build rule, the G5 selection, the D5a ratio rule and single-step Bateman activity, the D5c seed-spread rule, and the
machine-time ledger.
"""
from __future__ import annotations

import json
import math
from pathlib import Path
import time

R_LIMIT_D2 = 0.05
FOM_SIGMA_Z = 1.0
Z_UNBIASED = 3.0
D2_R_LIMIT = 0.1  # above this the analog reference is too coarse to judge bias (gains become lower bounds)
D1_SPEEDUP_MIN = 3.0
D1_LOST_PER_HISTORY = 1.0e-6
D1_SIGMA = 3.0
D5A_SIGMA = 3.0
D5A_MIN_DEVIATION = 0.02
D5C_FACTOR = 3.0
D4_TBR_LIMIT = 0.02
D4_A_LIMIT = 0.20
SECONDS_PER_JULIAN_YEAR = 365.25 * 86400.0


# ---------------------------------------------------------------------------
# Figure of merit

def fom(r: float | None, seconds: float) -> float | None:
    """FOM = 1 / (R^2 T)."""
    if r is None or r <= 0.0 or seconds <= 0.0:
        return None
    return 1.0 / (r * r * seconds)


def fom_relative_sigma(n_batches: int) -> float:
    """Relative uncertainty of R^2, hence of the FOM, from n batches: sqrt(2 / (n - 1))."""
    if n_batches < 2:
        raise ValueError("at least two batches are needed")
    return math.sqrt(2.0 / (n_batches - 1))


def fom_with_sigma(r: float | None, seconds: float, n_batches: int) -> dict:
    value = fom(r, seconds)
    if value is None:
        return {"fom": None, "sigma": None, "relative_sigma": None, "batches": n_batches}
    rel = fom_relative_sigma(n_batches)
    return {"fom": value, "sigma": value * rel, "relative_sigma": rel, "batches": n_batches}


def gain_with_sigma(f: dict, ref: dict) -> dict:
    """Gain f/ref with the two relative sigmas added in quadrature."""
    if f["fom"] is None or ref["fom"] is None or ref["fom"] <= 0.0:
        return {"gain": None, "sigma": None}
    g = f["fom"] / ref["fom"]
    return {"gain": g, "sigma": g * math.hypot(f["relative_sigma"], ref["relative_sigma"])}


def improved_over(f: dict, ref: dict) -> bool:
    """G5's 'improved': FOM higher than the reference by more than the combined 1 sigma of the two FOMs."""
    if f["fom"] is None or ref["fom"] is None:
        return False
    return (f["fom"] - ref["fom"]) > FOM_SIGMA_Z * math.hypot(f["sigma"], ref["sigma"])


def z_score(mean_a: float, se_a: float, mean_b: float, se_b: float) -> float | None:
    sd = math.hypot(se_a, se_b)
    return (mean_a - mean_b) / sd if sd > 0.0 else None


def unbiased(z: float | None, limit: float = Z_UNBIASED) -> bool | None:
    """Agreement with the analog reference within 3 sigma; None when no z is available (NOT_EVALUATED)."""
    return None if z is None else abs(z) <= limit


def ratio_sigma(a: float, sa: float, b: float, sb: float) -> float | None:
    """Standard error of a/b for independent a and b."""
    if a == 0.0 or b == 0.0:
        return None
    return abs(a / b) * math.hypot(sa / a, sb / b)


def ratio_of_means_sigma(num: list[float], den: list[float]) -> dict:
    """Ratio of batch means with the delta-method standard error, using the batch covariance (num and den may be correlated)."""
    n = len(num)
    if n != len(den) or n < 2:
        raise ValueError("need two equal-length series of at least two batches")
    mn, md = math.fsum(num) / n, math.fsum(den) / n
    vn = math.fsum((x - mn) ** 2 for x in num) / (n - 1)
    vd = math.fsum((x - md) ** 2 for x in den) / (n - 1)
    cnd = math.fsum((x - mn) * (y - md) for x, y in zip(num, den)) / (n - 1)
    if md == 0.0:
        return {"ratio": None, "std_error": None, "num_mean": mn, "den_mean": md}
    f = mn / md
    var = (vn - 2.0 * f * cnd + f * f * vd) / (md * md) / n
    return {"ratio": f, "std_error": math.sqrt(max(var, 0.0)), "num_mean": mn, "den_mean": md}


# ---------------------------------------------------------------------------
# D1: ray tracer

def d1_rule(dd: dict, moab: dict) -> dict:
    """D1: adopt double-down if speed-up >= 3x, lost <= 1e-6 per history, and (a) and TBR agree within 3 sigma.

    dd and moab each carry rate (active particles/s), histories, lost, a {mean, std_error}, tbr {mean, std_error}.
    """
    speedup = dd["rate"] / moab["rate"] if moab["rate"] > 0.0 else None
    lost_rate = dd["lost"] / dd["histories"] if dd["histories"] > 0 else None
    za = z_score(dd["a"]["mean"], dd["a"]["std_error"], moab["a"]["mean"], moab["a"]["std_error"])
    zt = z_score(dd["tbr"]["mean"], dd["tbr"]["std_error"], moab["tbr"]["mean"], moab["tbr"]["std_error"])
    checks = {
        "speedup_ok": speedup is not None and speedup >= D1_SPEEDUP_MIN,
        "lost_ok": lost_rate is not None and lost_rate <= D1_LOST_PER_HISTORY,
        "a_ok": za is not None and abs(za) <= D1_SIGMA,
        "tbr_ok": zt is not None and abs(zt) <= D1_SIGMA,
    }
    adopt = all(checks.values())
    return {"speedup": speedup, "lost_per_history": lost_rate, "z_a": za, "z_tbr": zt, **checks,
            "adopt_double_down": adopt, "selected": "openmc0153dd" if adopt else "w003env",
            "rule": "speed-up >= 3x, lost <= 1e-6 per history, (a) and TBR within 3 sigma of w003env"}


# ---------------------------------------------------------------------------
# D3: G5 selection and G* selection

def select_g5(fom_g0: dict, g1: dict | None, g2: dict | None, g3: dict | None, g4: dict | None,
              unbiased_flags: dict | None = None) -> dict:
    """G5 per the protocol: best of G1/G2 (mesh and source shape) if either improved on G0, plus G3 and G4 if each improved.

    Each argument is a fom_with_sigma dict (None if the configuration was not run). 'improved' is FOM above G0's by more than the
    combined 1 sigma. A configuration whose (a) disagrees with D2 by more than 3 sigma (unbiased_flags[name] is False) is not eligible.
    Returns the choices and the arithmetic.
    """
    flags = unbiased_flags or {}
    arith = {}
    cands = {"G1": g1, "G2": g2, "G3": g3, "G4": g4}
    eligible = {}
    for name, f in cands.items():
        if f is None or f["fom"] is None:
            arith[name] = {"fom": None, "improved": False, "reason": "not run or no FOM"}
            continue
        diff = f["fom"] - fom_g0["fom"]
        comb = math.hypot(f["sigma"], fom_g0["sigma"])
        imp = diff > FOM_SIGMA_Z * comb
        ok = flags.get(name, True) is not False
        arith[name] = {"fom": f["fom"], "sigma": f["sigma"], "g0_fom": fom_g0["fom"], "g0_sigma": fom_g0["sigma"],
                       "difference": diff, "combined_sigma": comb, "improved": bool(imp), "unbiased": flags.get(name),
                       "eligible": bool(imp and ok)}
        if imp and ok:
            eligible[name] = f["fom"]
    mesh_pick = max((n for n in ("G1", "G2") if n in eligible), key=lambda n: eligible[n], default=None)
    return {"mesh_and_shape": mesh_pick,
            "max_split_1000": "G3" in eligible,
            "checkpoints": "G4" in eligible,
            "regenerate": mesh_pick is not None,
            "identical_to_g0": mesh_pick is None and "G3" not in eligible and "G4" not in eligible,
            "arithmetic": arith}


def best_unbiased(gains: dict[str, dict], unbiased_flags: dict[str, bool | None], d2_relative_error: float | None = None) -> dict:
    """G*: the largest gain among configurations whose (a) agrees with D2 within 3 sigma. NOT_EVALUATED flags are not eligible.

    When D2's R(a) exceeds R_LIMIT every gain is a labelled lower bound and unbiasedness is NOT_EVALUATED (as in S0), so G* is
    the largest gain among all configurations that have one, with "lower_bound": true.
    """
    if d2_relative_error is not None and d2_relative_error > D2_R_LIMIT:
        ok = {k: v["gain"] for k, v in gains.items() if v.get("gain") is not None}
        lower_bound = True
    else:
        ok = {k: v["gain"] for k, v in gains.items() if v.get("gain") is not None and unbiased_flags.get(k) is True}
        lower_bound = False
    if not ok:
        return {"best": None, "gain": None, "lower_bound": lower_bound, "note": "no unbiased configuration with a gain"}
    best = max(ok, key=ok.get)
    band = "G* >= 100" if ok[best] >= 100.0 else ("10 <= G* < 100" if ok[best] >= 10.0 else "G* < 10")
    return {"best": best, "gain": ok[best], "band": band, "lower_bound": lower_bound}


# ---------------------------------------------------------------------------
# D4: laptop preview rule

def d4_rule(runs: list[dict]) -> dict:
    """PERF-025 keeps its numbers if both runs give TBR R <= 2 % and (a) R <= 20 %."""
    rows = []
    for r in runs:
        rows.append({"tbr_r": r["tbr_r"], "a_r": r["a_r"],
                     "tbr_ok": r["tbr_r"] is not None and r["tbr_r"] <= D4_TBR_LIMIT,
                     "a_ok": r["a_r"] is not None and r["a_r"] <= D4_A_LIMIT})
    keeps = bool(rows) and all(x["tbr_ok"] and x["a_ok"] for x in rows)
    return {"runs": rows, "perf_025_keeps_numbers": keeps}


# ---------------------------------------------------------------------------
# D5a: reaction-pathway audit

def bateman_single_step(production_rate: float, decay_constant: float, t_irradiation_s: float, t_cooling_s: float) -> dict:
    """Atoms of a radionuclide produced at a constant rate (atoms/s) for t_irradiation, then left to decay for t_cooling.

    N(T) = P (1 - exp(-lam T)) / lam; N(T + tau) = N(T) exp(-lam tau). The activity is lam N. Production-rate loss and
    daughters are ignored (one step).
    """
    lam = decay_constant
    if lam <= 0.0:
        raise ValueError("decay constant must be positive")
    n_end = production_rate * (-math.expm1(-lam * t_irradiation_s)) / lam
    n_cool = n_end * math.exp(-lam * t_cooling_s)
    return {"atoms_end_irradiation": n_end, "atoms_cooled": n_cool, "activity_cooled": lam * n_cool,
            "time_correction": (-math.expm1(-lam * t_irradiation_s)) * math.exp(-lam * t_cooling_s)}


def d5a_rule(d1s: float, d1s_se: float, ref: float, ref_se: float) -> dict:
    """A nuclide is a defect if D1S/reference differs from 1 by more than 3 sigma and more than 2 %."""
    if ref == 0.0:
        return {"ratio": None, "sigma": None, "defect": None, "reason": "reference is zero"}
    ratio = d1s / ref
    sigma = ratio_sigma(d1s, d1s_se, ref, ref_se) if d1s != 0.0 else d1s_se / abs(ref)
    dev = abs(ratio - 1.0)
    defect = bool(dev > D5A_SIGMA * sigma and dev > D5A_MIN_DEVIATION)
    return {"ratio": ratio, "sigma": sigma, "deviation": dev, "defect": defect}


def h1_confirmed(ratio: float, ratio_sigma_: float, fraction: float, fraction_sigma: float) -> dict:
    """H1: 1 - ratio equals the partial-MT fraction within 3 sigma."""
    diff = (1.0 - ratio) - fraction
    sigma = math.hypot(ratio_sigma_, fraction_sigma)
    return {"one_minus_ratio": 1.0 - ratio, "fraction": fraction, "difference": diff, "sigma": sigma,
            "confirmed": bool(abs(diff) <= D5A_SIGMA * sigma) if sigma > 0.0 else bool(diff == 0.0)}


def shares(values: dict[str, float]) -> dict[str, float]:
    total = sum(values.values())
    return {k: (v / total if total else 0.0) for k, v in values.items()}


def nuclides_above(share_a: dict[str, float], share_b: dict[str, float], threshold: float = 0.01) -> list[str]:
    """Radionuclides giving at least `threshold` of either method's photon leakage."""
    names = set(share_a) | set(share_b)
    return sorted(n for n in names if share_a.get(n, 0.0) >= threshold or share_b.get(n, 0.0) >= threshold)


# ---------------------------------------------------------------------------
# D5c: R2S seed spread

def seed_spread(totals: list[float]) -> dict:
    n = len(totals)
    if n < 2:
        raise ValueError("need at least two seeds")
    mean = math.fsum(totals) / n
    sd = math.sqrt(math.fsum((x - mean) ** 2 for x in totals) / (n - 1))
    return {"n": n, "mean": mean, "std_dev": sd, "relative_std_dev": sd / mean if mean else None}


def d5c_rule(totals: list[float], photon_r: float) -> dict:
    """If the seed-to-seed relative standard deviation exceeds 3x the photon-step R, S0's R2S uncertainty is understated."""
    s = seed_spread(totals)
    understated = s["relative_std_dev"] is not None and s["relative_std_dev"] > D5C_FACTOR * photon_r
    return {**s, "photon_step_r": photon_r, "threshold": D5C_FACTOR * photon_r, "uncertainty_understated": bool(understated)}


# ---------------------------------------------------------------------------
# Log parsing

def _first_number(text: str, pattern: str) -> float | None:
    import re
    m = re.search(pattern, text)
    return float(m.group(1).replace(",", "")) if m else None


def _parse_random_ray_block(text: str) -> dict:
    import re
    out = {
        "iterations": _first_number(text, r"Total Iterations\s*=\s*([\d.eE+-]+)"),
        "rays_per_iteration": _first_number(text, r"Number of Rays per Iteration\s*=\s*([\d.eE+-]+)"),
        "source_regions": _first_number(text, r"Source Regions \(SRs\)\s*=\s*([\d.eE+-]+)"),
        "source_regions_with_external_source": _first_number(text, r"SRs Containing External Sources\s*=\s*([\d.eE+-]+)"),
        "avg_intersections_per_iteration_per_sr": _first_number(text, r"Avg per Iteration per SR\s*=\s*([\d.eE+-]+)"),
        "avg_sr_miss_rate_percent": _first_number(text, r"Avg SR Miss Rate per Iteration\s*=\s*([\d.eE+-]+)%"),
        "energy_groups": _first_number(text, r"Energy Groups\s*=\s*([\d.eE+-]+)"),
        "simulation_seconds": _first_number(text, r"Total simulation time\s*=\s*([\d.eE+-]+) seconds"),
        "adjoint_mode_on": bool(re.search(r"Adjoint Flux Mode\s*=\s*ON", text)),
    }
    m = re.search(r"Source Shape\s*=\s*(\w+)", text)
    out["source_shape"] = m.group(1) if m else None
    return out


def parse_random_ray_log(text: str) -> dict:
    """Statistics OpenMC prints after each random-ray solve (forward, then adjoint): one dict per SIMULATION STATISTICS block."""
    blocks = text.split("SIMULATION STATISTICS")[1:]
    solves = [_parse_random_ray_block(b) for b in blocks]
    return {"solves": solves, "forward": next((x for x in solves if not x["adjoint_mode_on"]), None),
            "adjoint": next((x for x in solves if x["adjoint_mode_on"]), None), "double_down": "DOUBLE-DOWN" in text.upper()}


def parse_transport_log(text: str) -> dict:
    """Timing and tracer lines of a continuous-energy run: initialization, cross-section reading, simulation, rate, tracer."""
    return {
        "initialization_s": _first_number(text, r"Total time for initialization\s*=\s*([\d.eE+-]+) seconds"),
        "reading_cross_sections_s": _first_number(text, r"Reading cross sections\s*=\s*([\d.eE+-]+) seconds"),
        "simulation_s": _first_number(text, r"Total time in simulation\s*=\s*([\d.eE+-]+) seconds"),
        "transport_only_s": _first_number(text, r"Time in transport only\s*=\s*([\d.eE+-]+) seconds"),
        "active_batches_s": _first_number(text, r"Time in active batches\s*=\s*([\d.eE+-]+) seconds"),
        "elapsed_s": _first_number(text, r"Total time elapsed\s*=\s*([\d.eE+-]+) seconds"),
        "rate_active_particles_per_s": _first_number(text, r"Calculation Rate \(active\)\s*=\s*([\d.eE+-]+) particles/second"),
        "rate_particles_per_s": _first_number(text, r"Calculation Rate\s*=\s*([\d.eE+-]+) particles/second"),
        "leakage_fraction": _first_number(text, r"Leakage Fraction\s*=\s*([\d.eE+-]+)"),
        "double_down": "DOUBLE-DOWN" in text.upper(),
        "geometry_loaded_line": ("Loading file" in text),
    }


# ---------------------------------------------------------------------------
# Machine ledger

def read_loadavg() -> str:
    try:
        return Path("/proc/loadavg").read_text(encoding="utf-8").strip()
    except OSError:
        return ""


def ledger_add(path: Path, item: str, label: str, wall_s: float, **extra) -> dict:
    """Append a run to the S0b ledger (several jobs can finish together, so the file is locked). Wall time is job wall time."""
    import fcntl

    path = Path(path)
    with open(str(path) + ".lock", "w", encoding="utf-8") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        data = json.loads(path.read_text(encoding="utf-8"))
        entry = {"item": item, "label": label, "wall_s": wall_s, "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), **extra}
        data["entries"].append(entry)
        data["job_wall_total_s"] = sum(e["wall_s"] for e in data["entries"])
        data["elapsed_since_start_s"] = time.time() - data["start_epoch"]
        path.write_text(json.dumps(data, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    return data
