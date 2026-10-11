#!/usr/bin/env python3
"""S0e breakdown analysis of E0 (descriptive, no rule). Standard library only.

    s0e_analyse.py --e0 E0/produce.json --model-dir D --out breakdown-s0e.json [--md breakdown-s0e.md]

From E0's production record (its `breakdown` block): each TF coil's share of response (a) and its relative error; the (R, Z)
map's share of (a) in four regions (inboard leg R < 129 cm, outboard leg R > 580 cm, top and bottom: the rest, split by the
sign of Z); and the largest-variance bins by sigma_i^2 / sum(sigma_j^2). The coil next to the port (centres at 10 and 350
degrees) is compared with the mean of the other 16. Protocol: docs/notes/CAD_TRANSPORT_S0E.md, "Breakdown analysis".
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0e_common as E  # noqa: E402

INBOARD_R_CM = 129.0
OUTBOARD_R_CM = 580.0
TOP_N = 10
CAVEAT = ("The variance share sigma_i^2 / sum(sigma_j^2) ignores the covariance between bins from one history "
          "(neighbouring bins are tallied from the same particles), so it is an approximate indication of where the "
          "uncertainty sits, not a decomposition of the variance of the total.")
REGION_RULE = ("Bins are assigned by their edges, on the TF material only: a bin whose lower R edge is below 129 cm is "
               "inboard (the TF material is there only inside 128 cm); a bin whose upper R edge is above 580 cm is outboard "
               "(581 to 645 cm); every other bin is top (Z centre above 0) or bottom. The 120-140 cm bin straddles 129 cm "
               "and is counted inboard whole, so it also holds the first centimetres of the top and bottom legs.")


def coil_angles(roles: list[dict], centres_deg: list[float], tol_deg: float = 1.0) -> dict:
    """Angle of each coil volume from its centroid, checked against the model card's coil_centres_deg by index."""
    out, ok = [], True
    by_name = {r["name"]: r for r in roles}
    for k, name in enumerate(E.COIL_NAMES):
        x, y = by_name[name]["centroid_cm"][:2]
        angle = math.degrees(math.atan2(y, x)) % 360.0
        diff = abs((angle - centres_deg[k] + 180.0) % 360.0 - 180.0)
        out.append({"name": name, "centroid_angle_deg": angle, "card_centre_deg": centres_deg[k], "difference_deg": diff})
        ok = ok and diff <= tol_deg
    return {"coils": out, "mapping_confirmed": ok}


def coil_table(per_coil: list[dict], centres_deg: list[float]) -> dict:
    """Shares, relative errors, variance shares and the port-adjacent comparison."""
    total = math.fsum(c["mean"] for c in per_coil)
    var_total = math.fsum(c["std_error"] ** 2 for c in per_coil)
    rows = []
    for k, c in enumerate(sorted(per_coil, key=lambda c: c["name"])):
        rows.append({"name": c["name"], "centre_deg": centres_deg[k], "mean": c["mean"], "std_error": c["std_error"],
                     "share": c["mean"] / total if total > 0 else None,
                     "relative_error": c["std_error"] / c["mean"] if c["mean"] > 0 else None,
                     "variance_share": c["std_error"] ** 2 / var_total if var_total > 0 else None})
    near = [r for r in rows if any(abs((r["centre_deg"] - p + 180.0) % 360.0 - 180.0) < 1e-9 for p in E.PORT_ADJACENT_DEG)]
    other = [r for r in rows if r not in near]
    mean_near = math.fsum(r["share"] for r in near) / len(near) if near else None
    mean_other = math.fsum(r["share"] for r in other) / len(other) if other else None
    return {"coils": rows, "total": total, "port_adjacent": [r["name"] for r in near],
            "mean_share_port_adjacent": mean_near, "mean_share_other": mean_other,
            "ratio_port_adjacent_to_other": mean_near / mean_other if mean_near is not None and mean_other else None,
            "mean_variance_share_port_adjacent": math.fsum(r["variance_share"] for r in near) / len(near) if near else None,
            "mean_variance_share_other": math.fsum(r["variance_share"] for r in other) / len(other) if other else None}


def region_of(r_lo: float, r_hi: float, z_lo: float, z_hi: float) -> str:
    if r_lo < INBOARD_R_CM:
        return "inboard"
    if r_hi > OUTBOARD_R_CM:
        return "outboard"
    return "top" if (z_lo + z_hi) / 2.0 > 0 else "bottom"


def map_table(rz: dict, total_a: float | None = None) -> dict:
    """Region shares and the largest-variance bins from the (R, Z) map (R fastest, then Z)."""
    r, z = rz["r_edges_cm"], rz["z_edges_cm"]
    nr, nz = len(r) - 1, len(z) - 1
    if len(rz["mean"]) != nr * nz or len(rz["std_error"]) != nr * nz:
        raise ValueError("map size does not match its edges")
    var_total = math.fsum(s * s for s in rz["std_error"])
    mean_total = math.fsum(rz["mean"])
    regions = {k: {"mean": 0.0, "variance": 0.0, "bins": 0} for k in ("inboard", "outboard", "top", "bottom")}
    bins = []
    for iz in range(nz):
        for ir in range(nr):
            m, s = rz["mean"][iz * nr + ir], rz["std_error"][iz * nr + ir]
            reg = region_of(r[ir], r[ir + 1], z[iz], z[iz + 1])
            if m > 0 or s > 0:
                regions[reg]["bins"] += 1
            regions[reg]["mean"] += m
            regions[reg]["variance"] += s * s
            bins.append({"r_cm": [r[ir], r[ir + 1]], "z_cm": [z[iz], z[iz + 1]], "region": reg, "mean": m, "std_error": s,
                         "share": m / mean_total if mean_total > 0 else None,
                         "variance_share": s * s / var_total if var_total > 0 else None})
    for v in regions.values():
        v["share"] = v["mean"] / mean_total if mean_total > 0 else None
        v["variance_share"] = v["variance"] / var_total if var_total > 0 else None
    bins.sort(key=lambda b: -(b["variance_share"] or 0.0))
    return {"map_total": mean_total, "regions": regions, "largest_variance_bins": bins[:TOP_N],
            "map_total_vs_response_a": (mean_total / total_a if total_a else None), "region_rule": REGION_RULE}


def analyse(e0: dict, roles: list[dict], centres_deg: list[float]) -> dict:
    """The whole breakdown from E0's production record, the roles and the model card's coil centres."""
    if "breakdown" not in e0:
        raise ValueError("the E0 record has no breakdown block (was it run with --breakdown?)")
    b = e0["breakdown"]
    a = (e0.get("response") or {}).get("a") or {}
    coils = coil_table(b["per_coil"], centres_deg)
    return {"source": "E0", "label": e0.get("label"), "response_a": a, "angles": coil_angles(roles, centres_deg),
            "coil_sum_vs_response_a": (coils["total"] / a["mean"]) if a.get("mean") else None,
            "coils": coils, "map": map_table(b["rz_map"], a.get("mean")), "caveat": CAVEAT}


def fmt(x, digits=3) -> str:
    return "n/a" if x is None else f"{x:.{digits}g}"


def markdown(result: dict) -> str:
    c, m = result["coils"], result["map"]
    lines = ["# S0e breakdown of response (a) from E0", "",
             f"Coil index to angle: {'confirmed' if result['angles']['mapping_confirmed'] else 'NOT confirmed'} against the "
             "coil centroids. Port-adjacent coils: " + ", ".join(c["port_adjacent"]) + ".", "",
             "| coil | centre (deg) | share | relative error | variance share |", "| --- | --- | --- | --- | --- |"]
    for r in c["coils"]:
        lines.append(f"| {r['name']} | {r['centre_deg']:g} | {fmt(r['share'])} | {fmt(r['relative_error'])} | {fmt(r['variance_share'])} |")
    lines += ["", f"Mean share, port-adjacent coils: {fmt(c['mean_share_port_adjacent'])}; other 16: {fmt(c['mean_share_other'])}; "
              f"ratio {fmt(c['ratio_port_adjacent_to_other'])}.", "",
              "| region | share of (a) | variance share | bins with flux |", "| --- | --- | --- | --- |"]
    for name in ("inboard", "outboard", "top", "bottom"):
        v = m["regions"][name]
        lines.append(f"| {name} | {fmt(v['share'])} | {fmt(v['variance_share'])} | {v['bins']} |")
    lines += ["", "| R (cm) | Z (cm) | region | share | variance share |", "| --- | --- | --- | --- | --- |"]
    for b in m["largest_variance_bins"]:
        lines.append(f"| {b['r_cm'][0]:g}-{b['r_cm'][1]:g} | {b['z_cm'][0]:g} to {b['z_cm'][1]:g} | {b['region']} | "
                     f"{fmt(b['share'])} | {fmt(b['variance_share'])} |")
    lines += ["", m["region_rule"], "", result["caveat"], ""]
    return "\n".join(lines)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--e0", required=True, type=Path, help="E0's produce.json")
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--md", type=Path, help="markdown table (default: next to --out)")
    args = ap.parse_args(argv)
    e0 = json.loads(args.e0.read_text(encoding="utf-8"))
    roles = json.loads((args.model_dir / "rm_m_roles.json").read_text(encoding="utf-8"))["solids"]
    card = json.loads((args.model_dir / "rm_m_model_card.json").read_text(encoding="utf-8"))
    centres = card["tf_coils"]["coil_centres_deg"]["value"]
    result = analyse(e0, roles, centres)
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    md = args.md or args.out.with_suffix(".md")
    md.write_text(markdown(result), encoding="utf-8")
    print(markdown(result))
    return 0 if result["angles"]["mapping_confirmed"] else 1


if __name__ == "__main__":
    sys.exit(main())
