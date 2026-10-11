#!/usr/bin/env python3
"""S0e follow-up (Amendment 2): E2′ and, if the rules ask for it, E3′, inside a 45 min cap; then H6′ and G*′.

    s0e_followup.py --scratch ~/.cache/avila-night/cad-risk-tests/s0e

Reads S0e's E0, E1 and H5 from `results-s0e.json` in the scratch folder and writes `results-s0e-followup.json` beside it.
E2′ is S0e's E2 without the smoke step: full generation on the cylindrical mesh, then the production. Protocol:
docs/notes/CAD_TRANSPORT_S0E.md, Amendment 2.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s0e_common as S  # noqa: E402
import s0e_run as R  # noqa: E402

CAP_S = 45 * 60.0


def window_depth(gen_json: Path) -> dict:
    """The generation record's window depth (lowest positive bound, fraction within 1e-9 of the largest)."""
    try:
        w = json.loads(gen_json.read_text(encoding="utf-8")).get("windows") or {}
    except (OSError, ValueError):
        return {}
    return {k: w.get(k) for k in ("lower_min_positive", "lower_max", "fraction_above_1e-9_of_max", "fraction_positive")}


def e2_prime(scratch: Path, budget: R.Budget) -> dict:
    d = scratch / "e2p"
    d.mkdir(parents=True, exist_ok=True)
    gen = R.job(R.generation_argv(S.SEEDS["gen"]["E2"], d, False), d / "gen.log", budget.timeout(2400))
    budget.add("E2′", "generate", gen)
    windows = d / "gen/weight_windows.h5"
    depth = window_depth(d / "gen.json")
    if gen["returncode"] != 0 or not windows.is_file():
        return {"label": "E2′", "generate": gen, "window_depth": depth,
                **R.not_evaluated("E2′", budget, "generation failed; see gen.log and gen.json")}
    row = R.produce("E2", d, windows, False, budget, item="E2′")
    row.update(label="E2′", generate=gen, window_depth=depth)
    if row.get("gain") is None and not str(row.get("note", "")).startswith("NOT_EVALUATED"):
        row["note"] = "NOT_EVALUATED: the production wrote no usable statepoint (window depth recorded)"
    return row


def summarise(s0e: dict, rows: dict[str, dict], budget: R.Budget, extra: dict) -> dict:
    e0, e1 = s0e["rows"].get("E0"), s0e["rows"].get("E1")
    h5 = s0e["rule"]["H5"]
    h6p = S.h6_rule(e0, rows.get("E2′"))
    scored = {"E0": e0, "E1": e1, "E2": rows.get("E2′"), "E3": rows.get("E3′")}
    g = S.g_star(scored)
    best = {"E2": "E2′", "E3": "E3′"}.get(g["best"], g["best"])
    return {"rows": rows, "ledger": {"cap_s": budget.cap_s, "entries": budget.entries}, "job_wall_s": budget.used(),
            "rule": {"H5_from_S0e": h5, "H6′": h6p, "E3′_required": S.e3_required(h5, h6p),
                     "best": best, "g_star": g["g_star"], "nuc050": g["nuc050"]}, **extra}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--scratch", required=True, type=Path)
    args = ap.parse_args(argv)
    scratch = args.scratch
    s0e = json.loads((scratch / "results-s0e.json").read_text(encoding="utf-8"))
    budget = R.Budget(cap_s=CAP_S)
    rows: dict[str, dict] = {}
    extra = {"start_epoch": time.time(), "protocol": "docs/notes/CAD_TRANSPORT_S0E.md, Amendment 2"}

    def save() -> dict:
        result = summarise(s0e, rows, budget, extra)
        (scratch / "results-s0e-followup.json").write_text(
            json.dumps(result, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
        return result

    rows["E2′"] = e2_prime(scratch, budget)
    result = save()
    if not result["rule"]["E3′_required"]:
        rows["E3′"] = {"label": "E3′", "gain": None, "sigma": None, "unbiased": None,
                       "note": "not run: H5 and H6′ are not both confirmed (rule)"}
    elif not S.fits(budget.used(), S.ESTIMATE_S["E3"], budget.cap_s):
        rows["E3′"] = R.not_evaluated("E3′", budget, f"does not fit the cap (estimate {S.ESTIMATE_S['E3']:.0f} s)")
    else:
        d = scratch / "e3p"
        d.mkdir(parents=True, exist_ok=True)
        row = R.produce("E3", d, scratch / "e2p/gen/weight_windows.h5", True, budget, item="E3′")
        row["label"] = "E3′"
        rows["E3′"] = row
    result = save()
    print(json.dumps(result["rule"], indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
