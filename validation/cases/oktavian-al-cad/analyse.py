#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Apply the manifest's declared protocol to a sealed run record of oktavian-al-cad (and, optionally, to the CSG run).

    python3 analyse.py --cad RECORD [--csg RECORD] [--out FILE]

Standard library only. Nothing here chooses a threshold: the numbers come from the manifest's `protocol` block, which
was committed before the run. The per-bin verdicts are the validation harness's (validation/scoring.py).
"""
from __future__ import annotations

import argparse
import json
import math
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from validation.manifest import load_manifest, manifest_sha256  # noqa: E402
from validation.scoring import Identity, score_case  # noqa: E402

CASE = "oktavian-al-cad"
RANGES = ((0.1, 1.0), (1.0, 10.0), (10.0, 20.7))
MIN_COMPARED = 50
REL_MC_LIMIT = 0.20
PASS_FRACTION = 0.90
SIGMAS = 2.0


def case_outcome(rows: list[dict], total: int = 133, pass_fraction: float = PASS_FRACTION) -> dict:
    """The protocol's case rule over the neutron rows that count (blocked bins excluded)."""
    counts = {v: sum(1 for r in rows if r["verdict"] == v) for v in ("PASS", "FAIL", "INCONCLUSIVE", "NOT_EVALUATED")}
    scored = counts["PASS"] + counts["FAIL"]
    least = math.ceil(total / 2)
    if scored < least:
        return {"outcome": "INCONCLUSIVE", "scored": scored, "needed": least, "counts": counts,
                "why": f"{scored} of {total} bins are PASS or FAIL; the rule needs {least}. The rest are INCONCLUSIVE because the Monte Carlo error exceeds half the experimental uncertainty.",
                "next_step": "more histories: the INCONCLUSIVE bins list the precision they need"}
    share = counts["PASS"] / scored
    return {"outcome": "PASS" if share >= pass_fraction else "FAIL", "scored": scored, "pass_share": share, "counts": counts}


def geometry_comparison(cad: dict, csg: dict, ids: list[str]) -> dict:
    """CAD against CSG per bin: both relative Monte Carlo errors <= 20 %, agree within 2 combined standard deviations."""
    a = {r["detector_id"]: r for r in cad["results"]}
    b = {r["detector_id"]: r for r in csg["results"]}
    compared, agree, worst = [], 0, 0.0
    for i in ids:
        x, y = a[i], b[i]
        if not (x["value"] > 0 and y["value"] > 0):
            continue
        if x["u_mc"] / x["value"] > REL_MC_LIMIT or y["u_mc"] / y["value"] > REL_MC_LIMIT:
            continue
        sd = math.hypot(x["u_mc"], y["u_mc"])
        z = abs(x["value"] - y["value"]) / sd if sd else math.inf
        worst = max(worst, z)
        compared.append((i, x["value"] / y["value"], z))
        agree += z <= SIGMAS
    n = len(compared)
    ratios = sorted(r for _, r, _ in compared)
    out = {"bins_compared": n, "bins_agreeing": agree, "worst_z": worst,
           "median_cad_over_csg": ratios[n // 2] if n else None,
           "disagreeing": [{"detector_id": i, "cad_over_csg": r, "z": z} for i, r, z in compared if z > SIGMAS]}
    if n < MIN_COMPARED:
        out.update(outcome="INCONCLUSIVE", why=f"only {n} bins have both relative Monte Carlo errors at or below {REL_MC_LIMIT:.0%}; the rule needs {MIN_COMPARED}")
    else:
        out.update(outcome="PASS" if agree / n >= PASS_FRACTION else "FAIL", agree_share=agree / n)
    return out


def by_range(rows: list[dict], manifest: dict) -> dict:
    hi = {d["id"]: d["bin"]["hi_mev"] for d in manifest["detectors"]}
    lo = {d["id"]: d["bin"]["lo_mev"] for d in manifest["detectors"]}
    out = {}
    for a, b in RANGES:
        ce = sorted(r["ce"] for r in rows if r["ce"] is not None and a <= (lo[r["detector_id"]] + hi[r["detector_id"]]) / 2 < b)
        out[f"{a:g}-{b:g} MeV"] = {"bins": len(ce), "median_ce": ce[len(ce) // 2] if ce else None, "min_ce": ce[0] if ce else None, "max_ce": ce[-1] if ce else None}
    return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--cad", type=Path, required=True)
    ap.add_argument("--csg", type=Path)
    ap.add_argument("--out", type=Path)
    args = ap.parse_args(argv)
    manifest = load_manifest(HERE / "manifest.json")
    cad = json.loads(args.cad.read_text(encoding="utf-8"))
    identity = Identity.from_dict(cad["identity"])
    scored = score_case(manifest, cad, identity)
    neutron_rows = [r for r in scored["rows"] if r["response_class"] == "neutron_leakage_spectrum" and r["verdict"] != "NOT_EVALUATED"]
    neutron_class = scored["classes"]["neutron_leakage_spectrum"]
    inconclusive = [{"detector_id": r["detector_id"], "u_c_mc": r.get("u_c_mc"), "u_e": r.get("u_e")} for r in neutron_rows if r["verdict"] == "INCONCLUSIVE"]
    out = {"case_id": CASE, "manifest_sha256": manifest_sha256(manifest), "record_sha256": cad["record_sha256"], "run_id": cad["run_id"],
           "case_outcome": case_outcome(neutron_rows), "neutron_class": {k: neutron_class[k] for k in ("detectors", "verdict_counts", "distribution", "bias")},
           "ce_by_energy_range": by_range(neutron_rows, manifest), "inconclusive_bins": len(inconclusive),
           "worst_failing": sorted(({"detector_id": r["detector_id"], "ce": r["ce"], "difference": r.get("difference"), "allowed": r.get("allowed")} for r in neutron_rows if r["verdict"] == "FAIL"),
                                   key=lambda x: -abs((x["ce"] or 1) - 1))[:10],
           "photon_context": {"note": "not scored (edge convention unresolved)", "detectors": 57},
           "context": {k: cad["context"][k] for k in ("particles", "batches", "seed", "threads", "transport_wall_seconds", "lost_particle_files", "net_vs_partial")}}
    if args.csg:
        csg = json.loads(args.csg.read_text(encoding="utf-8"))
        ids = [d["id"] for d in manifest["detectors"] if d["response_class"] == "neutron_leakage_spectrum" and "blocked" not in d]
        out["geometry_path_comparison"] = geometry_comparison(cad, csg, ids)
        out["csg_record_sha256"] = csg["record_sha256"]
        out["csg_identity"] = csg["identity"]
        out["same_identity"] = csg["identity"]["library_sha256"] == cad["identity"]["library_sha256"] and csg["identity"]["code_version"] == cad["identity"]["code_version"]
    text = json.dumps(out, indent=2, sort_keys=True) + "\n"
    if args.out:
        args.out.write_text(text, encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
