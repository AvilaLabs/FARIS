"""Acceptance and R6 rule evaluation for the CAD transport risk tests.

Pure Python (standard library only). Each function applies a rule fixed in
docs/notes/CAD_TRANSPORT_RISK_TESTS.md and returns a dict with the numbers it
used, so a result can be audited without re-running anything.
"""
from __future__ import annotations

import math

VOLUME_TOLERANCE = 0.005      # GEO-030: faceted vs CAD volume per solid
LOST_PER_HISTORY_LIMIT = 1.0e-6  # GEO-025
LOST_MIN_HISTORIES = 1_000_000
R6_SIGMA = 3.0


def volume_comparison(solids: list[dict], tolerance: float = VOLUME_TOLERANCE) -> dict:
    """Per-solid faceted vs CAD volume. Each solid needs name, cad_volume_cm3 and faceted_volume_cm3."""
    rows = []
    for s in solids:
        cad = float(s["cad_volume_cm3"])
        faceted = float(s["faceted_volume_cm3"])
        if cad <= 0.0:
            raise ValueError(f"{s['name']}: CAD volume must be positive")
        rel = faceted / cad - 1.0
        rows.append({"name": s["name"], "cad_volume_cm3": cad, "faceted_volume_cm3": faceted,
                     "relative_error": rel, "pass": abs(rel) <= tolerance})
    worst = max(rows, key=lambda r: abs(r["relative_error"])) if rows else None
    return {
        "rule": f"|faceted/CAD - 1| <= {tolerance} for every solid (GEO-030)",
        "tolerance": tolerance,
        "solids": len(rows),
        "failing": [r["name"] for r in rows if not r["pass"]],
        "worst": {"name": worst["name"], "relative_error": worst["relative_error"]} if worst else None,
        "pass": bool(rows) and all(r["pass"] for r in rows),
        "rows": rows,
    }


def lost_particle_check(lost: int, histories: int) -> dict:
    """GEO-025: lost particles per history <= 1e-6, over at least 1e6 histories."""
    if histories <= 0 or lost < 0:
        raise ValueError("histories must be positive and lost non-negative")
    rate = lost / histories
    enough = histories >= LOST_MIN_HISTORIES
    return {
        "rule": f"lost/histories <= {LOST_PER_HISTORY_LIMIT} over at least {LOST_MIN_HISTORIES} histories (GEO-025)",
        "lost": lost,
        "histories": histories,
        "lost_per_history": rate,
        "enough_histories": enough,
        "pass": enough and rate <= LOST_PER_HISTORY_LIMIT,
    }


def source_site_check(inside: int, total: int) -> dict:
    """SRC-018: every sampled source site lies in the plasma volume."""
    if total <= 0 or not 0 <= inside <= total:
        raise ValueError("need 0 <= inside <= total and total > 0")
    return {
        "rule": "100 % of sampled source sites inside the plasma volume (SRC-018)",
        "sites": total,
        "inside": inside,
        "fraction_inside": inside / total,
        "pass": inside == total,
    }


def agree_within_sigma(mean_a: float, sd_a: float, mean_b: float, sd_b: float, sigmas: float = R6_SIGMA) -> dict:
    """Two estimates agree if |a - b| <= sigmas * sqrt(sd_a^2 + sd_b^2)."""
    if sd_a < 0 or sd_b < 0:
        raise ValueError("standard deviations must be non-negative")
    combined = math.hypot(sd_a, sd_b)
    diff = mean_a - mean_b
    z = None if combined == 0.0 else abs(diff) / combined
    return {"difference": diff, "combined_sd": combined, "z": z,
            "agree": (diff == 0.0) if combined == 0.0 else abs(diff) <= sigmas * combined}


def r6_choice(steps_a: dict, steps_b: dict, flux_a: tuple[float, float], flux_b: tuple[float, float]) -> dict:
    """Apply the R6 rule. A is OpenMC 0.15.3, B is 0.16.0.

    steps_x maps each R1-R3 step name to {"ok": bool, "workarounds": [str, ...]}.
    Choose B (0.16.0) only if
      1. every step runs on B, with no workaround that A does not also need, and
      2. the analog TF fast flux agrees within 3 sigma.
    Otherwise choose A (0.15.3). The reasons are returned in order.
    """
    reasons = []
    names = sorted(set(steps_a) | set(steps_b))
    for name in names:
        b = steps_b.get(name)
        a = steps_a.get(name, {"ok": False, "workarounds": []})
        if b is None:
            reasons.append(f"step {name}: not run on 0.16.0")
            continue
        if not b["ok"]:
            reasons.append(f"step {name}: failed on 0.16.0")
            continue
        extra = sorted(set(b["workarounds"]) - set(a["workarounds"]))
        if extra:
            reasons.append(f"step {name}: 0.16.0 needs workarounds beyond 0.15.3: {extra}")
    rule1 = not reasons
    agreement = agree_within_sigma(flux_a[0], flux_a[1], flux_b[0], flux_b[1])
    rule2 = agreement["agree"]
    if not rule2:
        reasons.append(f"analog TF fast flux differs by {agreement['z']:.2f} combined sigma (limit {R6_SIGMA})")
    chosen = "0.16.0" if (rule1 and rule2) else "0.15.3"
    if chosen == "0.16.0":
        reasons.append("both conditions hold: every step runs on 0.16.0 with no extra workaround, and the analog flux agrees")
    return {"rule1_steps_clean": rule1, "rule2_flux_agrees": rule2, "flux_agreement": agreement,
            "choice": chosen, "reasons": reasons}
