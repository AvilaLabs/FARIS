#!/usr/bin/env python3
"""S0c: read the run records, apply the protocol's rules, print the tables and write one results JSON.

    s0c_analyse.py [--root ~/.cache/avila-night/cad-risk-tests/s0c] [--out results-s0c.json]

Standard library only. Missing records are reported as NOT_EVALUATED, never guessed.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0b_common as C  # noqa: E402
import s0c_common as S  # noqa: E402


def load(path: Path):
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


def q1(root: Path) -> dict:
    analog = load(root / "q1" / "analog.json")
    out = {"analog": None, "rows": [], "floor": None}
    if analog is None or "response" not in analog:
        out["status"] = "NOT_EVALUATED: no analog record"
        return out
    ar = analog["response"]
    a_fom = analog["fom_a"]
    out["analog"] = {"R": ar["relative_error"], "mean": ar["mean"], "std_error": ar["std_error"], "T_s": ar["seconds_to_statepoint"], "batches": ar["batches"],
                     "histories": ar["histories"], "FOM": a_fom["fom"], "FOM_sigma": a_fom["sigma"], "stopped_by": analog["run"]["stopped_by"],
                     "reached_R_0.05": bool(ar["relative_error"] is not None and ar["relative_error"] <= S.ANALOG_R_STOP)}

    def row(label, spacing, gen, prod, floor=False):
        r = {"label": label, "spacing": spacing, "floor": floor}
        if gen:
            fwd = (gen.get("random_ray_log_stats") or {}).get("forward") or {}
            w = gen.get("windows") or {}
            r.update({"gen_seconds": gen["run"]["wall_seconds"], "gen_rss_mb": gen.get("peak_rss_mb_children"),
                      "source_regions": fwd.get("source_regions"), "source_regions_expected": gen.get("expected_source_regions"),
                      "window_min": w.get("min_positive"), "window_max": w.get("max"), "window_dynamic_range": w.get("dynamic_range"),
                      "window_fraction_positive": w.get("fraction_positive"), "window_cells": w.get("cells")})
        if prod and "response" in prod:
            pr = prod["response"]
            f = prod["fom_a"]
            g = C.gain_with_sigma(f, a_fom)
            u = S.row_unbiased(pr["mean"], pr["std_error"], ar["mean"], ar["std_error"])
            r.update({"R": pr["relative_error"], "mean": pr["mean"], "std_error": pr["std_error"], "T_s": pr["seconds_to_statepoint"], "batches": pr["batches"],
                      "histories": pr["histories"], "FOM": f["fom"], "spacing_fom_sigma": f["sigma"], "sigma": f["sigma"], "gain": g["gain"], "gain_sigma": g["sigma"],
                      "z": u["z"], "unbiased": u["unbiased"], "_fom": f, "floor_result": (prod.get("windows") or {}).get("floor_result"),
                      "lost": (prod.get("lost") or {}).get("restart_files")})
        else:
            why = (prod or {}).get("error") or "no production record"
            r.update({"R": None, "FOM": None, "unbiased": None, "note": f"NOT_EVALUATED: {why} (production {S.PRODUCTION_S:.0f} s)"})
        return r

    for d in S.SPACINGS_CM:
        tag = f"{d:g}"
        out["rows"].append(row(f"{tag} cm", d, load(root / "q1" / f"gen_{tag}.json"), load(root / "q1" / f"prod_{tag}.json")))
    measured = [(r["spacing"], r["gen_seconds"]) for r in out["rows"] if r.get("gen_seconds")]
    for r in out["rows"]:
        if r.get("gen_seconds") is None and len(measured) >= 2:
            est = S.extrapolate_cost([m[0] for m in measured], [m[1] for m in measured], r["spacing"])
            r["note"] = (f"NOT_EVALUATED: did not fit the time box; generation measured at " + ", ".join(f"{m[1]:.0f} s ({m[0]:g} cm)" for m in measured)
                         + f"; estimated {est:.0f} s generation + {S.PRODUCTION_S + 20:.0f} s production")
            r["estimated_generation_s"] = est
    ds = S.delta_star([{"spacing": r["spacing"], "fom": r.get("FOM"), "sigma": r.get("sigma"), "unbiased": r.get("unbiased")} for r in out["rows"]])
    out["delta_star"] = ds
    fl_prod = load(root / "q1" / "prod_floor.json")
    if fl_prod is not None and ds.get("delta_star_cm") is not None:
        d = ds["delta_star_cm"]
        base = next(r for r in out["rows"] if r["spacing"] == d)
        fr = row(f"floor at {d:g} cm", d, None, fl_prod, floor=True)
        out["floor"] = {"row": fr, "rule": S.floor_rule(base["_fom"], fr["_fom"], fr["unbiased"]) if "_fom" in fr and "_fom" in base else None,
                        "against_spacing": d}
    if out["floor"] is None and ds.get("delta_star_cm") is not None:
        gen = next((r for r in out["rows"] if r["spacing"] == ds["delta_star_cm"]), {})
        top, low = gen.get("window_max"), gen.get("window_min")
        if top is not None and low is not None:
            floor_value = S.FLOOR_REL * top
            out["floor"] = {"production_run": False, "floor_value": floor_value, "window_min_positive": low, "cells_raised_expected": bool(low < floor_value),
                            "against_spacing": ds["delta_star_cm"],
                            "note": "no lower bound is below the floor: the floored windows equal the unfloored ones" if low >= floor_value else "floor applies; production not run"}
    out["rm_m_confirmation_required"] = S.rm_m_confirmation_required(ds.get("delta_star_cm"))
    for r in out["rows"] + ([out["floor"]["row"]] if out["floor"] and "row" in out["floor"] else []):
        r.pop("_fom", None)
    return out


def q2(root: Path) -> dict:
    rec = load(root / "q2" / "q2.json")
    ps = root / "q2.ps"
    return {"record": rec, "process_snapshots": ps.read_text(encoding="utf-8") if ps.is_file() else None} if rec else {"status": "NOT_EVALUATED: no record"}


def q3(root: Path) -> dict:
    d1s = load(root / "q3" / "d1s" / "d1s.json")
    out = {"d1s": d1s.get("result") if d1s else None, "rows": []}
    if not d1s or "result" not in d1s:
        out["status"] = "NOT_EVALUATED: no D1S record"
        return out
    ref = d1s["result"]
    mesh, cell = {}, None
    for label, vox in (("mesh20", 20.0), ("mesh10", 10.0), ("mesh2", 2.0), ("cell2", None)):
        rec = load(root / "q3" / label / "r2s.json")
        if not rec or "result" not in rec:
            out["rows"].append({"label": label, "status": "NOT_EVALUATED"})
            continue
        res = rec["result"]
        rn = res.get("neutron_step_relative_error_estimate")
        r_photon = res["photon_step_relative_error"]
        r_comb = res.get("combined_relative_error_estimate") or r_photon
        ratio = S.ratio_row(res["photons_per_s"], r_comb, ref["photons_per_s"], ref["relative_error"])
        row = {"label": label, "photons_per_s": res["photons_per_s"], "R_photon": r_photon, "R_neutron_estimate": rn, "R_combined": r_comb,
               "ratio_to_d1s": ratio["ratio"], "ratio_sigma": ratio["sigma"], "seconds": rec.get("total_seconds")}
        out["rows"].append(row)
        if vox is None:
            cell = {"ratio": ratio["ratio"], "sigma": ratio["sigma"]}
        else:
            mesh[vox] = {"ratio": ratio["ratio"], "sigma": ratio["sigma"]}
    if len(mesh) == 3 and cell is not None:
        out["h2"] = S.h2_rule(mesh, cell)
    else:
        out["h2"] = {"outcome": "NOT_EVALUATED", "reason": "not every configuration has a result"}
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--root", type=Path, default=Path.home() / ".cache/avila-night/cad-risk-tests/s0c")
    ap.add_argument("--out", type=Path)
    ap.add_argument("--references", type=Path, help="also write the committed record: results, run-record hashes and the protocol hash")
    args = ap.parse_args()
    res = {"q1": q1(args.root), "q2": q2(args.root), "q3": q3(args.root), "ledger": load(args.root / "ledger.json")}
    if args.references:
        import hashlib

        def digest(path: Path) -> str:
            return hashlib.sha256(path.read_bytes()).hexdigest()

        records = {str(p.relative_to(args.root)): digest(p) for p in sorted(args.root.rglob("*.json")) if "smoke" not in p.parts and p.name != "results-s0c.json"
                   and p.stat().st_size < 5_000_000}
        protocol = Path(__file__).resolve().parents[3] / "docs" / "notes" / "CAD_TRANSPORT_S0C.md"
        ref = {"schema": "faris-cad-transport-s0c/v1", "protocol": "docs/notes/CAD_TRANSPORT_S0C.md", "protocol_sha256": digest(protocol),
               "record_sha256": records, "results": res}
        args.references.write_text(json.dumps(ref, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    out = args.out or args.root / "results-s0c.json"
    out.write_text(json.dumps(res, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    a = res["q1"].get("analog")
    if a:
        print(f"analog: R {a['R']:.4f}  T {a['T_s']:.0f} s  FOM {a['FOM']:.4g} +- {a['FOM_sigma']:.2g}  stopped by {a['stopped_by']}")
    for r in res["q1"]["rows"] + ([res["q1"]["floor"]["row"]] if (res["q1"].get("floor") or {}).get("row") else []):
        def f(k, fmt="{:.4g}"):
            return fmt.format(r[k]) if r.get(k) is not None else "-"
        print(f"{r['label']:<16} SR {f('source_regions','{:.0f}')} gen {f('gen_seconds','{:.0f}')} s {f('gen_rss_mb','{:.0f}')} MB  win {f('window_min')}..{f('window_max')}  "
              f"R {f('R')} FOM {f('FOM')} +- {f('sigma','{:.2g}')} gain {f('gain')} +- {f('gain_sigma','{:.2g}')} z {f('z','{:+.2f}')} unbiased {r.get('unbiased')}")
    print("delta*", res["q1"].get("delta_star"), "floor", (res["q1"].get("floor") or {}).get("rule"))
    for r in res["q3"]["rows"]:
        print(r)
    print("H2", res["q3"].get("h2"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
