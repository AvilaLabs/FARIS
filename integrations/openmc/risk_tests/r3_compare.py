#!/usr/bin/env python3
"""R3 comparison of D1S and mesh-based R2S (numpy only).

    r3_compare.py --r2s DIR --d1s DIR --out results-r3.json

Reads the per-voxel dose arrays both scripts wrote (pSv.cm/s per voxel, 10 cm voxels) and applies the protocol rules:
the ratio D1S/R2S of the total over the mesh at each cooling time against 0.85 to 1.15, the voxel-ratio distribution
over voxels where both methods have a relative error of at most 10 %, and the R2S source conservation (ACT-023).
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402


def total(mean: np.ndarray, std: np.ndarray) -> dict:
    s = float(mean.sum())
    e = float(np.sqrt((std ** 2).sum()))
    return {"sum_pSv_cm_per_s": s, "std_error": e, "relative_error": (e / s if s > 0 else None),
            "mean_dose_rate_uSv_per_h": sc.dose_rate_usv_per_h(s, sc.mesh_volume_cm3()), "voxels_nonzero": int((mean > 0).sum())}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--r2s", required=True, type=Path)
    ap.add_argument("--d1s", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()
    r2s_rec = json.loads((args.r2s / "r2s_record.json").read_text())
    d1s_rec = json.loads((args.d1s / "d1s_record.json").read_text())
    result = {"schema": "faris-r3/v1", "protocol": "docs/notes/CAD_TRANSPORT_RISK_TESTS.md#r3", "openmc": "0.15.3",
              "mesh": r2s_rec["mesh"], "unit": "pSv.cm/s summed over voxels (track length x ICRP-116 AP coefficient x source rate)",
              "ratio_band": sc.RATIO_BAND, "cooling_times": {}}
    all_in_band = True
    for idx, label in sc.COOLING_LABELS.items():
        a = np.load(args.d1s / f"dose_time_{idx}.npz")
        b = np.load(args.r2s / f"dose_time_{idx}.npz")
        ta, tb = total(a["mean"], a["std"]), total(b["mean"], b["std"])
        ratio = ta["sum_pSv_cm_per_s"] / tb["sum_pSv_cm_per_s"] if tb["sum_pSv_cm_per_s"] > 0 else None
        ratio_err = None
        if ratio and ta["relative_error"] is not None and tb["relative_error"] is not None:
            ratio_err = ratio * float(np.hypot(ta["relative_error"], tb["relative_error"]))
        ra = np.where(a["mean"] > 0, a["std"] / np.where(a["mean"] > 0, a["mean"], 1.0), np.inf).ravel()
        rb = np.where(b["mean"] > 0, b["std"] / np.where(b["mean"] > 0, b["mean"], 1.0), np.inf).ravel()
        voxel = sc.ratio_summary(a["mean"].ravel().tolist(), b["mean"].ravel().tolist(), ra.tolist(), rb.tolist())
        voxel["voxels_d1s_r_le_10"] = int((ra <= sc.VOXEL_R_LIMIT).sum())
        voxel["voxels_r2s_r_le_10"] = int((rb <= sc.VOXEL_R_LIMIT).sum())
        in_band = sc.band_check(ratio)
        all_in_band = all_in_band and in_band
        entry = {"d1s": ta, "r2s": tb, "ratio_d1s_over_r2s": ratio, "ratio_std_error": ratio_err, "in_band": in_band,
                 "voxel_ratio": voxel, "r2s_conservation": r2s_rec.get("conservation", {}).get(label)}
        if not in_band:
            entry["top5_nuclides"] = {
                "r2s_photon_power_MeV_per_s": r2s_rec["top_photon_power_nuclides_MeV_per_s"][label][:5],
                "d1s_dose_by_parent_pSv_cm_per_s": d1s_rec["dose_by_parent_nuclide_pSv_cm_per_s"][label][:5]}
        result["cooling_times"][label] = entry
    cons = [e["r2s_conservation"] for e in result["cooling_times"].values() if e["r2s_conservation"]]
    result["conservation_pass_as_built"] = all(c["pass"] for c in cons) if cons else None
    result["conservation_pass_unclipped"] = all(c["unclipped_source_check"]["pass"] for c in cons) if cons else None
    result["verdict"] = {"ratio_band": "PASS" if all_in_band else "FAIL",
                         "conservation": "PASS" if result["conservation_pass_as_built"] else "FAIL"}
    result["runs"] = {"r2s": {"steps": r2s_rec["steps"], "settings": r2s_rec["settings"], "workarounds": r2s_rec["workarounds"],
                              "photon": {k: {kk: vv for kk, vv in v.items() if kk != "conservation"} for k, v in r2s_rec["photon"].items()}},
                      "d1s": {k: d1s_rec.get(k) for k in ("batches", "histories", "particles_per_batch", "seconds_to_statepoint", "n_radionuclides",
                                                          "seed", "settings", "workarounds", "warnings", "peak_rss_mb", "run")}}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: {"ratio": v["ratio_d1s_over_r2s"], "in_band": v["in_band"], "voxels": v["voxel_ratio"]["voxels_used"]}
                      for k, v in result["cooling_times"].items()}, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
