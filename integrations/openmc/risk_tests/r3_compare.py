#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""R3 comparison of D1S and mesh-based R2S (numpy only).

    r3_compare.py --r2s DIR --d1s DIR --out results-r3.json

Reads the per-voxel integrated dose arrays (pSv.cm3/s per voxel) and reports the central spatial ratios plus batch-aggregated
uncertainty when present. Legacy per-voxel standard errors are not summed because their bins share histories.
Unknown D1S/R2S covariance leaves the ratio comparison inconclusive; central in-band/out-of-band flags remain visible.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402


def total(mean: np.ndarray, aggregate: dict | None) -> dict:
    s = float(mean.sum())
    e = float(aggregate["standard_error"]) if aggregate else None
    if aggregate and not np.isclose(s, float(aggregate["mean"]), rtol=1.0e-10, atol=1.0e-14):
        raise ValueError("aggregate batch mean does not match the spatial tally sum")
    return {"sum_pSv_cm3_per_s": s, "std_error": e, "relative_error": (e / s if e is not None and s > 0 else None),
            "uncertainty_scope": aggregate.get("uncertainty_scope") if aggregate else "unavailable: legacy voxel standard errors do not preserve covariance across bins",
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
              "mesh": r2s_rec["mesh"], "unit": "pSv.cm3/s summed over voxels (track length x ICRP-116 AP coefficient x source rate)",
              "ratio_band": sc.RATIO_BAND, "cooling_times": {}}
    all_in_band = True
    for idx, label in sc.COOLING_LABELS.items():
        a = np.load(args.d1s / f"dose_time_{idx}.npz")
        b = np.load(args.r2s / f"dose_time_{idx}.npz")
        ta = total(a["mean"], d1s_rec.get("shutdown_dose", {}).get(label))
        tb = total(b["mean"], r2s_rec.get("shutdown_dose", {}).get(label))
        ratio = ta["sum_pSv_cm3_per_s"] / tb["sum_pSv_cm3_per_s"] if tb["sum_pSv_cm3_per_s"] > 0 else None
        positive = (a["mean"].ravel() > 0.0) & (b["mean"].ravel() > 0.0)
        voxel = sc.ratio_summary(a["mean"].ravel().tolist(), b["mean"].ravel().tolist(),
                                 [0.0] * positive.size, [0.0] * positive.size, limit=0.0)
        voxel.pop("limit_r", None)
        voxel["method"] = "central ratio for every voxel with positive values in both methods"
        voxel["uncertainty"] = "not assessed per voxel; parent-nuclide covariance is unavailable in marginal bins"
        in_band = sc.band_check(ratio)
        all_in_band = all_in_band and in_band
        entry = {"d1s": ta, "r2s": tb, "ratio_d1s_over_r2s": ratio, "ratio_std_error": None,
                 "central_ratio_in_band": in_band, "ratio_uncertainty": "unavailable: D1S/R2S covariance is unknown",
                 "voxel_ratio": voxel, "r2s_conservation": r2s_rec.get("conservation", {}).get(label)}
        if not in_band:
            entry["top5_nuclides"] = {
                "r2s_photon_power_MeV_per_s": r2s_rec["top_photon_power_nuclides_MeV_per_s"][label][:5],
                "d1s_dose_by_parent_pSv_cm_per_s": d1s_rec["dose_by_parent_nuclide_pSv_cm_per_s"][label][:5]}
        result["cooling_times"][label] = entry
    cons = [e["r2s_conservation"] for e in result["cooling_times"].values() if e["r2s_conservation"]]
    result["conservation_pass_as_built"] = all(c["pass"] for c in cons) if cons else None
    result["conservation_pass_unclipped"] = all(c["unclipped_source_check"]["pass"] for c in cons) if cons else None
    result["verdict"] = {"ratio_band": "INCONCLUSIVE: central band status is shown per point; aggregate uncertainty and method covariance are not established",
                         "central_ratio_band": "ALL_INSIDE" if all_in_band else "OUTSIDE_AT_LEAST_ONE",
                         "conservation": "PASS" if result["conservation_pass_as_built"] else "FAIL"}
    result["runs"] = {"r2s": {"steps": r2s_rec["steps"], "settings": r2s_rec["settings"], "workarounds": r2s_rec["workarounds"],
                              "photon": {k: {kk: vv for kk, vv in v.items() if kk != "conservation"} for k, v in r2s_rec["photon"].items()}},
                      "d1s": {k: d1s_rec.get(k) for k in ("batches", "histories", "particles_per_batch", "seconds_to_statepoint", "n_radionuclides",
                                                          "seed", "settings", "workarounds", "warnings", "peak_rss_mb", "run")}}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    print(json.dumps({k: {"ratio": v["ratio_d1s_over_r2s"], "in_band": v["central_ratio_in_band"], "voxels": v["voxel_ratio"]["voxels_used"]}
                      for k, v in result["cooling_times"].items()}, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
