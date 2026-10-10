#!/usr/bin/env python3
"""S0b result tables from the run records (standard library only). Applies the protocol's mechanical rules; no verdict wording.

    s0b_analyse.py d1 --dd D1/dd.json --moab D1/moab.json --out D1/result.json
    s0b_analyse.py d3 --root D3 --d2 D2/analog.json --out D3/table.json [--configs G0,G1,...]
    s0b_analyse.py g5 --root D3 --d2 D2/analog.json --out D3/g5.json
    s0b_analyse.py d4 --run1 D4/run1.json --run2 D4/run2.json --out D4/result.json
    s0b_analyse.py d5c --seeds R2S_S0/r2s_record.json,... --out D5C/result.json
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0b_common as C  # noqa: E402


def load(path: Path):
    return json.loads(Path(path).read_text(encoding="utf-8")) if Path(path).is_file() else None


def time_file(log: Path) -> dict:
    """Wall seconds and peak RSS (MB) from a run.sh log (`/usr/bin/time -v` output)."""
    out = {"wall_seconds": None, "peak_rss_mb": None}
    if not Path(log).is_file():
        return out
    text = Path(log).read_text(encoding="utf-8", errors="replace")
    m = re.search(r"Elapsed \(wall clock\) time \(h:mm:ss or m:ss\): ([\d:.]+)", text)
    if m:
        seconds = 0.0
        for part in m.group(1).split(":"):
            seconds = seconds * 60.0 + float(part)
        out["wall_seconds"] = seconds
    m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", text)
    if m:
        out["peak_rss_mb"] = int(m.group(1)) / 1024.0
    return out


def d1_inputs(rec: dict) -> dict:
    return {"rate": rec["log"]["rate_active_particles_per_s"], "histories": rec["response"]["histories"],
            "lost": rec["lost"]["restart_files"], "a": rec["response"]["a"], "tbr": rec["response"]["tbr"]}


def d1(args) -> int:
    dd, moab = load(args.dd), load(args.moab)
    out = {"rule": C.d1_rule(d1_inputs(dd), d1_inputs(moab)), "builds": {}}
    for name, rec in (("openmc0153dd", dd), ("w003env", moab)):
        out["builds"][name] = {
            "particles_per_s_active": rec["log"]["rate_active_particles_per_s"], "initialization_s": rec["log"]["initialization_s"],
            "reading_cross_sections_s": rec["log"]["reading_cross_sections_s"], "a": rec["response"]["a"], "tbr": rec["response"]["tbr"],
            "lost_particle_restart_files": rec["lost"]["restart_files"], "histories": rec["response"]["histories"],
            "peak_rss_mb_children": rec["peak_rss_mb_children"], "double_down_in_log": rec["log"]["double_down"],
            "wall_seconds_openmc": rec["run"]["wall_seconds"], "loadavg_start": rec["loadavg_start"], "loadavg_end": rec["loadavg_end"],
            "seed": rec["seed"], "executable": rec["run"]["executable"]}
    Path(args.out).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(out["rule"], indent=1))
    return 0


def d3_rows(root: Path, d2: dict, configs: list[str]) -> dict:
    ref = d2["response"]
    ref_fom = d2["fom_a"]
    rows = {}
    for name in configs:
        prod = load(Path(root) / name / "produce.json")
        gen = load(Path(root) / name / "generate.json")
        row = {"config": name}
        if gen:
            rr = gen.get("random_ray_log_stats") or {}
            fwd, adj = rr.get("forward") or {}, rr.get("adjoint") or {}
            w = gen.get("windows") or {}
            row["generation"] = {
                "wall_seconds_random_ray_step": gen["steps"]["random_ray_fw_cadis"]["seconds"] if "random_ray_fw_cadis" in gen["steps"] else None,
                "mgxs_seconds": gen["steps"]["mgxs"]["seconds"] if "mgxs" in gen["steps"] else None,
                "peak_rss_mb_children": gen.get("peak_rss_mb_children"),
                "source_regions": fwd.get("source_regions"), "source_regions_with_external_source_adjoint": adj.get("source_regions_with_external_source"),
                "avg_sr_miss_rate_percent_forward_adjoint": [fwd.get("avg_sr_miss_rate_percent"), adj.get("avg_sr_miss_rate_percent")],
                "window_cells": w.get("window_cells"), "fraction_positive": w.get("fraction_positive"),
                "fraction_above_1e-9_of_max": w.get("fraction_above_1e-9_of_max"), "fraction_positive_top_group": w.get("fraction_positive_top_group"),
                "lower_max": w.get("lower_max"), "lower_min_positive": w.get("lower_min_positive"), "upper_over_lower": w.get("upper_over_lower"),
                "survival_ratio": w.get("survival_ratio"), "max_split": w.get("max_split"), "mesh_dimension": w.get("mesh_dimension"),
                "parameters": gen.get("parameters"), "group_edges_ev": gen.get("group_edges_ev"), "seed": gen.get("generation_seed"),
                "weight_windows_sha256": gen.get("weight_windows_sha256")}
        if prod and "response" in prod:
            r = prod["response"]
            fom = prod["fom_a"]
            gain = C.gain_with_sigma(fom, ref_fom)
            z = C.z_score(r["a"]["mean"], r["a"]["std_error"], ref["a"]["mean"], ref["a"]["std_error"])
            row["production"] = {
                "R_a": r["a"]["relative_error"], "T_seconds": r["seconds_to_statepoint"], "batches": r["batches"], "histories": r["histories"],
                "histories_per_second": r["histories_per_second"], "FOM_a": fom["fom"], "FOM_sigma": fom["sigma"], "gain_vs_D2": gain["gain"],
                "gain_sigma": gain["sigma"], "a_mean": r["a"]["mean"], "a_std_error": r["a"]["std_error"], "z_vs_D2": z,
                "unbiased": C.unbiased(z) if ref["a"]["relative_error"] is not None and ref["a"]["relative_error"] <= 0.1 else None,
                "tbr": r["tbr"], "lost_restart_files": prod["lost"]["restart_files"], "seed": prod["seed"],
                "windows": prod["windows"], "checkpoints_surface_and_collision": prod["checkpoints_surface_and_collision"],
                "max_split_override": prod["max_split_override"], "statepoint_sha256": r["statepoint_sha256"],
                "loadavg_start": prod["loadavg_start"], "loadavg_end": prod["loadavg_end"], "peak_rss_mb_children": prod["peak_rss_mb_children"],
                "affinity_cpus": prod["affinity_cpus"]}
        rows[name] = row
    return rows


def d3(args) -> int:
    d2 = load(args.d2)
    configs = args.configs.split(",")
    rows = d3_rows(args.root, d2, configs)
    out = {"d2": {"R_a": d2["response"]["a"]["relative_error"], "T_seconds": d2["response"]["seconds_to_statepoint"],
                  "histories": d2["response"]["histories"], "FOM_a": d2["fom_a"]["fom"], "FOM_sigma": d2["fom_a"]["sigma"]},
           "rows": rows}
    flags = {k: v["production"]["unbiased"] for k, v in rows.items() if "production" in v}
    gains = {k: {"gain": v["production"]["gain_vs_D2"]} for k, v in rows.items() if "production" in v}
    out["g_star"] = C.best_unbiased(gains, flags)
    Path(args.out).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(f"D2: R(a) {out['d2']['R_a']:.4f}  T {out['d2']['T_seconds']:.0f} s  FOM {out['d2']['FOM_a']:.4g}")
    for k, v in rows.items():
        p = v.get("production")
        if p:
            print(f"{k}: R {p['R_a']:.4f}  T {p['T_seconds']:.0f}  hist {p['histories']}  hist/s {p['histories_per_second']:.1f}  FOM {p['FOM_a']:.4g} +- {p['FOM_sigma']:.2g}"
                  f"  gain {p['gain_vs_D2']:.3g} +- {p['gain_sigma']:.2g}  z {p['z_vs_D2']}  unbiased {p['unbiased']}")
    print("G*:", out["g_star"])
    return 0


def g5(args) -> int:
    d2 = load(args.d2)
    rows = d3_rows(args.root, d2, ["G0", "G1", "G2", "G3", "G4"])
    foms = {}
    flags = {}
    for k, v in rows.items():
        if "production" in v:
            foms[k] = {"fom": v["production"]["FOM_a"], "sigma": v["production"]["FOM_sigma"]}
            flags[k] = v["production"]["unbiased"]
    sel = C.select_g5(foms["G0"], foms.get("G1"), foms.get("G2"), foms.get("G3"), foms.get("G4"), unbiased_flags=flags)
    Path(args.out).write_text(json.dumps(sel, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(sel, indent=1))
    return 0


def d4(args) -> int:
    runs = []
    for p in (args.run1, args.run2):
        r = load(p)
        runs.append({"tbr_r": r["response"]["tbr"]["relative_error"], "a_r": r["response"]["a"]["relative_error"],
                     "histories": r["response"]["histories"], "seconds_to_last_statepoint": r["response"]["seconds_to_statepoint"]})
    out = C.d4_rule(runs)
    Path(args.out).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=1))
    return 0


def d5c(args) -> int:
    import numpy as np

    totals, rs = [], []
    for spec in args.seeds.split(","):
        rec_path = Path(spec)
        z = np.load(rec_path.parent / "dose_time_2.npz")
        m, s = z["mean"], z["std"]
        totals.append(float(m.sum()))
        rs.append(float(np.sqrt((s ** 2).sum()) / m.sum()))
    out = {"totals_pSv_cm_per_s": totals, "photon_step_R_each": rs, "rule": C.d5c_rule(totals, sum(rs) / len(rs))}
    out["rule_with_largest_R"] = C.d5c_rule(totals, max(rs))
    Path(args.out).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=1))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("d1")
    p.add_argument("--dd", required=True, type=Path)
    p.add_argument("--moab", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    for name in ("d3", "g5"):
        p = sub.add_parser(name)
        p.add_argument("--root", required=True, type=Path)
        p.add_argument("--d2", required=True, type=Path)
        p.add_argument("--out", required=True, type=Path)
        p.add_argument("--configs", default="G0,G1,G2,G3,G4,G5,G6")
    p = sub.add_parser("d4")
    p.add_argument("--run1", required=True, type=Path)
    p.add_argument("--run2", required=True, type=Path)
    p.add_argument("--out", required=True, type=Path)
    p = sub.add_parser("d5c")
    p.add_argument("--seeds", required=True)
    p.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()
    return {"d1": d1, "d3": d3, "g5": g5, "d4": d4, "d5c": d5c}[args.cmd](args)


if __name__ == "__main__":
    raise SystemExit(main())
