#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Validation runner and evaluator for the maintenance-coupling result (docs/notes/MAINTENANCE_COUPLING_VALIDATION.md).

  configs   write one driver configuration per variant (B, V1, V2-low, V2-high, V3, V4, V5a-V5d) from the amended
            run's configuration:
              maintenance_validation.py configs --base A2_CONFIG.json --out DIR [--v4-runs DIR]
            Each variant gets DIR/<variant>/config.json, the derived assumptions file where the variant changes
            one, and variant.json (what changed, SHA-256 of every derived file). The driver is
            scripts/maintenance_coupling_test.py; this script never runs it, ACTINV or any transport.
  evaluate  read DIR/<variant>/result.json for every variant that exists and apply the protocol's claim rules
            (C1-C4) and verdict labels (ROBUST, FRAGILE, INCOMPLETE):
              maintenance_validation.py evaluate --dir DIR --reference references/maintenance-coupling-test-a2.json
                                                 --out references/maintenance-coupling-validation.json
            Refuses unless the protocol's body SHA-256 (the text before its amendments heading) is the recorded one.
            If the baseline variant B does not reproduce the amended run exactly, no other variant is evaluated.

Exit status: 0 done, 2 bad input or refused.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import sys
from pathlib import Path

SCHEMA = "faris-maintenance-validation/v0.1"
REPO = Path(__file__).resolve().parent.parent
DEFAULT_PROTOCOL = REPO / "docs" / "notes" / "MAINTENANCE_COUPLING_VALIDATION.md"
PROTOCOL_BODY_SHA256 = "60fa7ab7acf6b4245e1f31046c86ba3870ab8407c71b209e7fcfc3da91804423"
AMENDMENTS_HEADING = b"## Amendments"

DAY_S = 86400.0
MONTH_DAYS = 30.4375
BLANKET_COOLDOWN_S = 30.0 * DAY_S
V2_WORK_MONTHS = {"V2-low": 3.2, "V2-high": 5.9}
PHOTON_RESPONSE = "/home/connoravila/nuclear-data/photon-response/nist-xcom-all.json"
BARE = "bare_lower_bound"
WITH_IMPURITIES = "specification_maximum_impurities"
CENTRAL_W = "0.5"
ARRANGEMENTS = ("no-port/reference", "no-port/breeder", "port/reference", "port/breeder")
ARRANGEMENT_KEYS = ("scenario", "physics", "history_run", "spectrum_run")
# V4 run folder name -> arrangement; control means no port
V4_RUNS = {"control-reference": "no-port/reference", "control-breeder": "no-port/breeder",
           "port-reference": "port/reference", "port-breeder": "port/breeder"}
VARIANTS = ("B", "V1", "V2-low", "V2-high", "V3", "V4", "V5a", "V5b", "V5c", "V5d")
BLANKET_COMPONENT = "blanket"
MAGNET_COMPONENT = "magnets"
V5_FACTORS = {"V5a": ("blanket", 0.8), "V5b": ("blanket", 1.2), "V5c": ("magnet", 0.8), "V5d": ("magnet", 1.2)}
D1_GAP = 0.02
C4_MIN_RATIO = 2.0
C4_CONTRAST = "breeder-minus-reference, no port"
CLAIMS = ("C1", "C2", "C3", "C4")
HOLDS, FAILS, NOT_EVALUATED = "HOLDS", "FAILS", "NOT_EVALUATED"
# local paths replaced in the output, after the validation directory itself
PATH_PLACEHOLDERS = (
    ("/home/connoravila/.local/bin/actinv", "<actinv>"),
    ("/home/connoravila/Documents/actinv/actinv-data", "<actinv-data>"),
    ("/home/connoravila/nuclear-data", "<nuclear-data>"),
)


class Refused(Exception):
    """Bad input, or an evaluation the protocol does not allow (exit status 2)."""


def sha256_file(path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_json(path, what):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError) as err:
        raise Refused(f"cannot read {what} {path}: {err}") from err


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def protocol_body_sha256(path: Path) -> str | None:
    """SHA-256 of the protocol text before its amendments heading, or None."""
    data = path.read_bytes()
    index = data.find(AMENDMENTS_HEADING)
    return hashlib.sha256(data[:index]).hexdigest() if index >= 0 else None


def check_protocol(path: Path) -> str:
    got = protocol_body_sha256(path) if path.is_file() else None
    if got != PROTOCOL_BODY_SHA256:
        raise Refused(f"protocol file {path} has body SHA-256 {got} (text before its amendments), "
                      f"expected {PROTOCOL_BODY_SHA256}; refusing to evaluate")
    return got


# ------------------------------------------------------------------ configs --

def derive_assumptions(base: dict, variant: str) -> tuple[dict, str] | None:
    """(derived assumptions, what changed) for a variant that changes the assumptions file, else None."""
    if variant in V2_WORK_MONTHS:
        work_s = V2_WORK_MONTHS[variant] * MONTH_DAYS * DAY_S
        duration = BLANKET_COOLDOWN_S + work_s
        doc = copy.deepcopy(base)
        entries = [e for e in doc["service_limits"] if e["component_id"] == BLANKET_COMPONENT]
        if not entries:
            raise Refused("the assumptions file has no blanket service_limits entry")
        for entry in entries:
            entry["replacement_duration_s"] = duration
        what = (f"every blanket service_limits replacement_duration_s = 30 d cooldown + {V2_WORK_MONTHS[variant]} months "
                f"work ({MONTH_DAYS} d per month) = {duration} s; class_w blanket = work / duration")
        return doc, what
    if variant in V5_FACTORS:
        cls, factor = V5_FACTORS[variant]
        component = BLANKET_COMPONENT if cls == "blanket" else MAGNET_COMPONENT
        doc = copy.deepcopy(base)
        entries = [e for e in doc["service_limits"] if e["component_id"] == component]
        if len(entries) != (1 if cls == "blanket" else 3):
            raise Refused(f"expected {1 if cls == 'blanket' else 3} {component} service_limits entries, found {len(entries)}")
        for entry in entries:
            entry["limit"] = entry["limit"] * factor
        return doc, f"every {component} service_limits limit x {factor}"
    return None


def make_configs(base_path: Path, out_dir: Path, v4_runs: Path | None) -> list[str]:
    base_path = Path(base_path).resolve()
    out_dir = Path(out_dir).resolve()
    base = load_json(base_path, "base config")
    here = base_path.parent

    def res(v):
        return str(v if os.path.isabs(v) else (here / v).resolve())

    for key in ("faris", "actinv", "data_dir", "assumptions", "arrangements"):
        if not base.get(key):
            raise Refused(f"base config has no {key!r}")
    arrangements = {}
    for name in ARRANGEMENTS:
        case = base["arrangements"].get(name)
        if not isinstance(case, dict) or any(k not in case for k in ARRANGEMENT_KEYS):
            raise Refused(f"base config arrangement {name!r} needs {', '.join(ARRANGEMENT_KEYS)}")
        arrangements[name] = {k: res(case[k]) for k in ARRANGEMENT_KEYS}
    assumptions_path = Path(res(base["assumptions"]))
    assumptions = load_json(assumptions_path, "assumptions")
    spectrum_runs = None
    if v4_runs is None:
        v4_error = "V4 needs --v4-runs, the folder holding the new spectrum runs"
    else:
        spectrum_runs = {}
        v4_error = None
        for run_name, arrangement in V4_RUNS.items():
            run = Path(v4_runs).resolve() / run_name / "run.json"
            if not run.is_file():
                v4_error = f"V4 spectrum run not found: {run}"
                break
            spectrum_runs[arrangement] = str(run)
    if v4_error:
        raise Refused(v4_error)
    cache = out_dir / "decay-cache"

    written = []
    for variant in VARIANTS:
        vdir = out_dir / variant
        cfg = {k: base[k] for k in ("protocol",) if k in base}
        cfg.update({
            "faris": res(base["faris"]), "actinv": res(base["actinv"]), "data_dir": res(base["data_dir"]),
            "assumptions": str(assumptions_path),
            "arrangements": copy.deepcopy(arrangements),
            "sweep": {}, "w_values": [0.5], "f_values": [1.0],
            "allow_reduced_grid": True, "amendment": 2,
            "output_dir": str(vdir / "out"), "result": str(vdir / "result.json"),
            "decay_cache": str(cache),
        })
        if "protocol" in cfg:
            cfg["protocol"] = res(cfg["protocol"])
        changes, derived = [], {}
        if variant == "V1":
            cfg["governing_quantity"] = "dose"
            cfg["photon_response"] = PHOTON_RESPONSE
            changes.append('governing_quantity "dose"; photon_response ' + PHOTON_RESPONSE)
        elif variant == "V3":
            cfg["impurities"] = str(REPO / "scenarios" / "arc-inspired" / "impurities" / "specification-maximum.json")
            changes.append("impurities: specification-maximum file; claims use the impurity variant of the result")
        elif variant == "V4":
            for arrangement, run in spectrum_runs.items():
                cfg["arrangements"][arrangement]["spectrum_run"] = run
            changes.append("spectrum_run of each arrangement replaced by the new run: "
                           + ", ".join(f"{n} -> {a}" for n, a in V4_RUNS.items()))
        made = derive_assumptions(assumptions, variant)
        if made is not None:
            doc, what = made
            path = vdir / "assumptions.json"
            write_json(path, doc)
            cfg["assumptions"] = str(path)
            derived["assumptions.json"] = sha256_file(path)
            changes.append(what)
        if variant in V2_WORK_MONTHS:
            work_s = V2_WORK_MONTHS[variant] * MONTH_DAYS * DAY_S
            cfg["class_w"] = {BLANKET_COMPONENT: work_s / (BLANKET_COOLDOWN_S + work_s)}
            changes.append(f"class_w {cfg['class_w']}")
        write_json(vdir / "config.json", cfg)
        write_json(vdir / "variant.json", {"variant": variant, "changes": changes or ["nothing else changed"],
                                           "derived_files": derived})
        written.append(variant)
    return written


# ----------------------------------------------------------------- evaluate --

def case_not_evaluated(name: str, case: dict | None, what: str) -> str | None:
    """A reason naming the case if it is not usable, else None."""
    if case is None:
        return f"{what} {name} is missing from the result"
    if case.get("status") != "EVALUATED":
        return f"{what} {name}: {case.get('reason') or 'status ' + str(case.get('status'))}"
    return None


def lifetime(case: dict) -> float:
    return case["history"]["lifetime_net_electricity_mwh"]


def rel_gap(a: float, b: float) -> float:
    big = max(abs(a), abs(b))
    return abs(a - b) / big if big > 0 else 0.0


def pair_claim(computed_cases: dict, fixed: dict, reference: str, breeder: str, ports: bool) -> dict:
    """The breeder arrangement ends up ahead of the reference one under the computed model, and was behind under the fixed one."""
    out = {"outcome": NOT_EVALUATED}
    for name in (reference, breeder):
        reason = case_not_evaluated(name, computed_cases.get(name), "computed case") or \
                 case_not_evaluated(name, fixed.get(name), "fixed case")
        if reason:
            out["reason"] = reason
            return out
    f_ref, f_br = lifetime(fixed[reference]), lifetime(fixed[breeder])
    c_ref, c_br = lifetime(computed_cases[reference]), lifetime(computed_cases[breeder])
    gap = rel_gap(c_ref, c_br)
    out["fixed_order"] = [breeder, reference] if f_br > f_ref else [reference, breeder]
    out["computed_order"] = [breeder, reference] if c_br > c_ref else [reference, breeder]
    out["computed_gap_fraction"] = gap
    if ports:
        out["replacement_counts"] = {
            name: {"fixed": len(fixed[name]["history"]["replacements"]),
                   "computed": len(computed_cases[name]["history"]["replacements"])}
            for name in (reference, breeder)}
    if not f_ref > f_br:
        out.update(outcome=FAILS, reason=f"no swap: fixed order already has {out['fixed_order'][0]} first"
                                         if f_br > f_ref else "no swap: the fixed lifetimes are equal")
    elif not c_br > c_ref:
        out.update(outcome=FAILS, reason=f"no swap: computed order keeps {reference} ahead of {breeder}")
    elif not gap > D1_GAP:
        out.update(outcome=FAILS, reason=f"computed gap {gap:.4f} is not above {D1_GAP}")
    else:
        out["outcome"] = HOLDS
    return out


def d3_claims(decisions: dict) -> tuple[dict, dict]:
    d3 = decisions.get("D3")
    if not isinstance(d3, dict):
        gone = {"outcome": NOT_EVALUATED, "reason": "the result has no D3 decision"}
        return gone, dict(gone)
    c3 = {"outcome": NOT_EVALUATED, "reason": f"D3 not evaluated: {d3.get('reason', 'no reason recorded')}"}
    if d3.get("status") == "EVALUATED":
        c3 = {"outcome": HOLDS if d3.get("changed") else FAILS}
        if not d3.get("changed"):
            c3["reason"] = "no contrast leaves the band"
    rows = [r for r in d3.get("contrasts", []) if r.get("contrast") == C4_CONTRAST]
    if not rows:
        return c3, {"outcome": NOT_EVALUATED, "reason": f"D3 has no contrast named {C4_CONTRAST!r}"}
    row = rows[0]
    if row.get("status") != "EVALUATED":
        return c3, {"outcome": NOT_EVALUATED, "reason": "a downtime needed by this contrast is missing"}
    c4 = {"fixed_difference_s": row["fixed_difference_s"], "computed_difference_s": row["computed_difference_s"],
          "ratio_computed_over_fixed": row["ratio_computed_over_fixed"]}
    if not row.get("considered"):
        c4.update(outcome=NOT_EVALUATED, reason="the fixed difference is below the D3 minimum, so the ratio is not meaningful")
    elif row["ratio_computed_over_fixed"] >= C4_MIN_RATIO:
        c4["outcome"] = HOLDS
    else:
        c4.update(outcome=FAILS, reason=f"ratio {row['ratio_computed_over_fixed']:.4g} is below {C4_MIN_RATIO}")
    return c3, c4


def evaluate_claims(result: dict, claim_variant: str) -> dict:
    """C1-C4 from one variant's result (computed model at the central w, and its fixed model)."""
    computed = result.get("computed_model", {}).get(claim_variant)
    block = (computed or {}).get("w", {}).get(CENTRAL_W)
    if block is None:
        reason = f"the result has no computed model {claim_variant!r} at w {CENTRAL_W}"
        return {c: {"outcome": NOT_EVALUATED, "reason": reason} for c in CLAIMS}
    cases, fixed = block.get("cases", {}), result.get("fixed_model", {})
    c3, c4 = d3_claims(block.get("decisions", {}))
    return {"C1": pair_claim(cases, fixed, "no-port/reference", "no-port/breeder", False),
            "C2": pair_claim(cases, fixed, "port/reference", "port/breeder", True),
            "C3": c3, "C4": c4}


def compare(path: str, ref, got, diffs: list) -> None:
    if ref != got:
        diffs.append({"field": path, "reference": ref, "got": got})


def reproduce(result: dict, reference: dict) -> dict:
    """Exact (==) comparison of the B result against the amended run, for the four arrangements at w 0.5."""
    diffs: list = []
    ref_block = reference["computed_model"][BARE]["w"][CENTRAL_W]
    got_block = result.get("computed_model", {}).get(BARE, {}).get("w", {}).get(CENTRAL_W)
    if got_block is None:
        diffs.append({"field": f"computed_model.{BARE}.w.{CENTRAL_W}", "reference": "present", "got": "missing"})
    else:
        for name in ARRANGEMENTS:
            ref_case, got_case = ref_block["cases"][name], got_block.get("cases", {}).get(name, {})
            compare(f"cases.{name}.durations_s", ref_case["durations_s"], got_case.get("durations_s"), diffs)
            for key in ("lifetime_net_electricity_mwh", "total_replacement_downtime_s"):
                compare(f"cases.{name}.history.{key}", ref_case["history"][key], got_case.get("history", {}).get(key), diffs)
        for decision in ("D1", "D3"):
            compare(f"decisions.{decision}", ref_block["decisions"][decision], got_block.get("decisions", {}).get(decision), diffs)
    for name in ARRANGEMENTS:
        compare(f"fixed_model.{name}.history", reference["fixed_model"][name]["history"],
                result.get("fixed_model", {}).get(name, {}).get("history"), diffs)
    return {"reproduced": not diffs, "differences": diffs}


def summarize(variant_claims: dict) -> dict:
    """ROBUST if a claim HOLDS in every variant, FRAGILE if it FAILS in any, else INCOMPLETE."""
    out = {}
    for claim in CLAIMS:
        outcomes = {v: variant_claims[v][claim]["outcome"] for v in VARIANTS}
        fails = [v for v, o in outcomes.items() if o == FAILS]
        missing = [v for v, o in outcomes.items() if o == NOT_EVALUATED]
        label = "FRAGILE" if fails else ("INCOMPLETE" if missing else "ROBUST")
        out[claim] = {"label": label, "holds_in": [v for v, o in outcomes.items() if o == HOLDS],
                      "fails_in": fails, "not_evaluated_in": missing}
    return out


def sanitise(value, replacements):
    if isinstance(value, str):
        for old, new in replacements:
            value = value.replace(old, new)
        return value
    if isinstance(value, list):
        return [sanitise(v, replacements) for v in value]
    if isinstance(value, dict):
        return {sanitise(k, replacements): sanitise(v, replacements) for k, v in value.items()}
    return value


def evaluate(run_dir: Path, reference_path: Path, protocol: Path = DEFAULT_PROTOCOL) -> dict:
    run_dir = Path(run_dir).resolve()
    body_sha = check_protocol(protocol)
    reference = load_json(reference_path, "reference run")
    variants, claims = {}, {}
    baseline = None
    for variant in VARIANTS:
        vdir = run_dir / variant
        record = {"variant": load_json(vdir / "variant.json", "variant.json") if (vdir / "variant.json").is_file() else None}
        result_path = vdir / "result.json"
        if not result_path.is_file():
            record.update(result_sha256=None, status="not run")
            reason = "not run"
            claims[variant] = {c: {"outcome": NOT_EVALUATED, "reason": reason} for c in CLAIMS}
        elif variant != "B" and baseline is not None and not baseline["reproduced"]:
            record.update(result_sha256=sha256_file(result_path), status="not evaluated")
            reason = "the baseline did not reproduce the amended run"
            claims[variant] = {c: {"outcome": NOT_EVALUATED, "reason": reason} for c in CLAIMS}
        elif variant != "B" and baseline is None:
            record.update(result_sha256=sha256_file(result_path), status="not evaluated")
            reason = "the baseline was not run, so reproduction of the amended run is unconfirmed"
            claims[variant] = {c: {"outcome": NOT_EVALUATED, "reason": reason} for c in CLAIMS}
        else:
            result = load_json(result_path, f"{variant} result")
            record.update(result_sha256=sha256_file(result_path), status="evaluated")
            claim_variant = WITH_IMPURITIES if variant == "V3" else BARE
            record["claim_variant"] = claim_variant
            if variant == "B":
                baseline = reproduce(result, reference)
                record["reproduction"] = baseline
            if variant == "B" and not baseline["reproduced"]:
                reason = "the baseline did not reproduce the amended run"
                claims[variant] = {c: {"outcome": NOT_EVALUATED, "reason": reason} for c in CLAIMS}
            else:
                claims[variant] = evaluate_claims(result, claim_variant)
                block = result.get("computed_model", {}).get(claim_variant, {}).get("w", {}).get(CENTRAL_W, {})
                record["inputs_sha256"] = result.get("inputs_sha256")
                record["q_star"] = block.get("q_star")
                record["durations_s"] = {n: c.get("durations_s") for n, c in block.get("cases", {}).items()
                                         if n in ARRANGEMENTS}
                record["decisions"] = {d: block.get("decisions", {}).get(d) for d in ("D1", "D3")}
        record["claims"] = claims[variant]
        variants[variant] = record
    out = {
        "schema": SCHEMA,
        "protocol": {"path": "docs/notes/MAINTENANCE_COUPLING_VALIDATION.md", "body_sha256": body_sha},
        "reference": {"sha256": sha256_file(reference_path)},
        "baseline": baseline if baseline is not None else {"reproduced": None, "differences": [], "reason": "not run"},
        "variants": variants,
        "summary": summarize(claims),
    }
    return sanitise(out, [(str(run_dir), "<validation-dir>"), *PATH_PLACEHOLDERS])


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="command", required=True)
    p = sub.add_parser("configs", help="write one driver config per variant")
    p.add_argument("--base", required=True, help="the amended run's driver config")
    p.add_argument("--out", required=True, help="validation directory")
    p.add_argument("--v4-runs", help="folder holding control-reference, control-breeder, port-reference, port-breeder runs")
    p = sub.add_parser("evaluate", help="evaluate the claims from the variants' results")
    p.add_argument("--dir", required=True)
    p.add_argument("--reference", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--protocol", default=str(DEFAULT_PROTOCOL))
    args = ap.parse_args(argv)
    try:
        if args.command == "configs":
            written = make_configs(Path(args.base), Path(args.out), Path(args.v4_runs) if args.v4_runs else None)
            print("wrote configs for " + ", ".join(written))
        else:
            write_json(Path(args.out), evaluate(Path(args.dir), Path(args.reference), Path(args.protocol)))
            print(f"wrote {args.out}")
    except Refused as err:
        print(f"refused: {err}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
