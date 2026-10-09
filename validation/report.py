# SPDX-License-Identifier: AGPL-3.0-only
"""Per-case and suite validation reports, JSON and Markdown (requirements VAL-030, 035, 056, 091).

One command:

    python3 -m validation report --out DIR [--runs DIR] [--identity FILE] [--cases DIR]

Cases come from validation/cases/*/manifest.json. A run record named
<case_id>.run.json in --runs is scored against the manifest; a case without one
is reported NOT_EVALUATED with why and next step. The report never states a
single number across response classes, libraries or code versions (VAL-035).
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .manifest import load_manifest, manifest_sha256
from .scoring import Identity, ValidationError, score_case, DEFAULT_BOOTSTRAP_SEED

HERE = Path(__file__).resolve().parent
CASES_DIR = HERE / "cases"
SCHEMA = "faris.validation-report/1.0.0"


def discover(cases_dir: Path = CASES_DIR) -> list[Path]:
    return sorted(cases_dir.glob("*/manifest.json"))


def build_suite(manifests: list[dict], records: dict[str, dict], current: Identity | None, seed: int = DEFAULT_BOOTSTRAP_SEED) -> dict:
    cases = []
    for m in manifests:
        record = records.get(m["case_id"])
        if record is not None and current is None:
            raise ValidationError("a current identity (library, code version, adapter) is required to score a run record")
        result = score_case(m, record, current or Identity("none", "none", "none"), seed)
        result["manifest_sha256"] = manifest_sha256(m)
        result["title"] = m["title"]
        result["source"] = {"url": m["source"]["url"], "commit": m["source"].get("commit"), "doi": m["source"].get("doi")}
        result["license"] = m["license"]
        result["literature_context"] = m.get("literature_context", [])
        result["normalisation"] = {k: m["normalisation"][k] for k in ("status", "units")}
        cases.append(result)
    return {
        "schema": SCHEMA,
        "note": "C/E is reported per case and per response class. No figure combines classes, libraries or code versions.",
        "cases": cases,
        "coverage": coverage(cases),
    }


def coverage(cases: list[dict]) -> dict:
    """VAL-056: the ratio is never shown without the list of what is not covered."""
    total = sum(len(c["rows"]) for c in cases)
    scored = sum(1 for c in cases for r in c["rows"] if r["verdict"] in ("PASS", "FAIL"))
    return {
        "detectors": total, "scored": scored,
        "unscored": [{"case_id": c["case_id"], "detector_id": r["detector_id"], "verdict": r["verdict"], "why": r["why"], "next_step": r["next_step"]}
                     for c in cases for r in c["rows"] if r["verdict"] not in ("PASS", "FAIL")],
        "not_covered": {c["case_id"]: c["not_covered"] for c in cases},
    }


def _fmt(x, digits=4) -> str:
    return "-" if x is None else f"{x:.{digits}g}"


def render_markdown(suite: dict) -> str:
    lines = ["# FARIS validation report", "", suite["note"], ""]
    cov = suite["coverage"]
    lines += [f"Coverage: {cov['scored']} of {cov['detectors']} detectors have a PASS or FAIL verdict. The unscored detectors and what each case does not cover are listed below.", ""]
    for c in suite["cases"]:
        lines += [f"## {c['case_id']}: {c['title']}", "",
                  f"Evidence class: **{c['evidence_class']}**. Source: {c['source']['url']} ({c['source'].get('commit') or c['source'].get('doi')}). "
                  f"Licence: {c['license']['spdx']}. Rule: k = {c['k']}, covariance {c['covariance']['kind']}. Normalisation: {c['normalisation']['status']} ({c['normalisation']['units']}).",
                  "", "Verdicts: " + ", ".join(f"{k} {v}" for k, v in c["verdict_counts"].items() if v) + ".", ""]
        for name, cls in c["classes"].items():
            lines.append(f"### Response class {name}")
            lines.append("")
            counts = ", ".join(f"{k} {v}" for k, v in cls["verdict_counts"].items() if v)
            lines.append(f"{cls['detectors']} detectors ({counts}).")
            d = cls.get("distribution")
            if d is None:
                lines += ["", f"No C/E distribution: {cls['why']}. Next: {cls['next_step']}.", ""]
                continue
            lines += ["", "| count | mean | median | std dev | 5th | 95th | min | max |", "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
                      f"| {d['count']} | {_fmt(d['mean'])} | {_fmt(d['median'])} | {_fmt(d['std_dev'])} | {_fmt(d['p05'])} | {_fmt(d['p95'])} | {_fmt(d['min'])} | {_fmt(d['max'])} |", "",
                      f"Worst case: **{d['worst']['detector_id']}** with C/E {_fmt(d['worst']['ce'])}.", ""]
            b = cls["bias"]
            if b["status"] == "estimated":
                lines.append(f"Bias (mean log C/E): {_fmt(b['bias'])}, 95 % bootstrap interval [{_fmt(b['ci95'][0])}, {_fmt(b['ci95'][1])}], "
                             f"{b['resamples']} resamples, seed {b['seed']}.")
            else:
                lines.append(f"Bias: {b['status']}. {b['why']}. Next: {b['next_step']}.")
            lines.append("")
        stale = [r for r in c["rows"] if r["verdict"] == "STALE"]
        if stale:
            lines += [f"{len(stale)} rows are STALE: {stale[0]['why']}. Next: {stale[0]['next_step']}.", ""]
        waiting = [r for r in c["rows"] if r["verdict"] in ("NOT_EVALUATED", "INCONCLUSIVE")]
        if waiting:
            groups: dict[tuple, int] = {}
            for r in waiting:
                key = (r["verdict"], r["why"], r["next_step"])
                groups[key] = groups.get(key, 0) + 1
            lines += ["Not scored:", ""]
            for (verdict, why, nxt), n in groups.items():
                lines.append(f"- {n} detectors {verdict}: {why}. Next: {nxt}.")
            lines.append("")
        lines += ["Not covered by this case:", ""] + [f"- {x}" for x in c["not_covered"]] + [""]
        if c["literature_context"]:
            lines += ["Literature context (labelled, never scored):", ""] + [f"- {x['label']}: {x['summary']}" for x in c["literature_context"]] + [""]
    return "\n".join(lines).rstrip() + "\n"


def lint_report(suite: dict) -> list[str]:
    """VAL-035/036 lint: every class summary stands on its own; no cross-class or literature aggregate."""
    problems = []
    for c in suite["cases"]:
        for name, cls in c["classes"].items():
            if cls.get("distribution") and c["classes"][name]["detectors"] == 0:
                problems.append(f"{c['case_id']}/{name}: distribution without detectors")
        for r in c["rows"]:
            if r.get("evidence_class") not in ("verification", "code-to-code", "experiment"):
                problems.append(f"{c['case_id']}/{r['detector_id']}: row has no evidence class")
            if r["verdict"] in ("NOT_EVALUATED", "INCONCLUSIVE", "STALE") and not (r.get("why") and r.get("next_step")):
                problems.append(f"{c['case_id']}/{r['detector_id']}: {r['verdict']} without why and next step")
    for key in suite:
        if key in ("mean", "headline", "overall"):
            problems.append(f"suite carries an aggregate key {key!r}")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python3 -m validation", description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="command", required=True)
    rp = sub.add_parser("report", help="score every case and write report.json and report.md")
    rp.add_argument("--out", type=Path, required=True)
    rp.add_argument("--cases", type=Path, default=CASES_DIR)
    rp.add_argument("--runs", type=Path, help="directory with <case_id>.run.json sealed run records")
    rp.add_argument("--identity", type=Path, help="JSON with library_sha256, code_version, adapter_sha256 (the current identity)")
    rp.add_argument("--seed", type=int, default=DEFAULT_BOOTSTRAP_SEED)
    args = ap.parse_args(argv)
    manifests = [load_manifest(p) for p in discover(args.cases)]
    records = {}
    if args.runs:
        for m in manifests:
            path = args.runs / f"{m['case_id']}.run.json"
            if path.is_file():
                records[m["case_id"]] = json.loads(path.read_text(encoding="utf-8"))
    current = Identity.from_dict(json.loads(args.identity.read_text(encoding="utf-8"))) if args.identity else None
    suite = build_suite(manifests, records, current, args.seed)
    problems = lint_report(suite)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "report.json").write_text(json.dumps(suite, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    (args.out / "report.md").write_text(render_markdown(suite), encoding="utf-8")
    print(f"wrote {args.out / 'report.json'} and {args.out / 'report.md'}: {suite['coverage']['scored']} of {suite['coverage']['detectors']} detectors scored")
    return 0


if __name__ == "__main__":
    sys.exit(main())
