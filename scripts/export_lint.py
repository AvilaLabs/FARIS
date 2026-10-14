#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Lint over a study export folder (requirements INT-040, INT-044, PRV-010, PRV-012, UX-046, VIS-030, VIS-033, VIS-040,
DOC-062, VAL-084, VAL-090, DSN-085).

    python3 scripts/export_lint.py EXPORT_DIR [--strict]

EXPORT_DIR is the `<study name>-export` folder written by `faris study-file export` (docs/STUDY_EXPORT.md).
Findings of rules in ADVISORY are printed but fail only with --strict: they are gaps in the exporter that need a Rust
change (every one of them is found on the current exporter), listed here so that CI can start failing on them when the
exporter is fixed. Move a rule out of ADVISORY to enforce it. Exit status 1 on any failing finding.

  INT-040  (INT-040-units for the unit audit) every CSV is UTF-8 without a byte-order mark, LF line endings, starts with the research-screening statement,
           uses `.` as the decimal point; every numeric column names its unit (in the header, or by a `unit` column
           of the same table); the study hash and FARIS version are in the CSV header (advisory: they are in the manifest)
  PRV-010  every numeric column has a kind: a `<stem>_kind` column, a `kind`/`history_kind` column, or rows for it in
           uncertainty.csv; no kind other than calculated, authored, literature, conditional, not evaluated, partial
  PRV-012  a row whose state says not evaluated, no interval or not compared has a reason and a next step
  UX-046   no bare N/A, dash or NaN cell; empty cells are counted per file (advisory)
  VIS-030  every column group whose kind is calculated has a standard-error column
  VIS-033  a chart that draws an error depiction says what the interval includes (advisory)
  VIS-040  a chart has quantity-and-unit axis labels and names a kind label in its title or subtitle (advisory)
  INT-044  every file is listed in export-manifest.json with a matching full SHA-256; every PNG chart has an SVG sidecar
           that carries the statement; every file is documented in docs/STUDY_EXPORT.md
  DOC-062  the manifest and the user guide carry the design-model-not-a-twin statement (advisory for the export)
  VAL-090  no licensing, qualification or approval claim in any export text (the wording lint's rules)
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import re
import sys
from fnmatch import fnmatch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wording_lint  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
KINDS = {"calculated", "authored", "literature", "conditional", "not evaluated", "partial", "checked", "failed"}
NOT_EVALUATED = {"no interval", "not_evaluated", "not_calculated", "not_compared", "not evaluated", "not calculated"}
REASON_COLUMNS = {"why", "reason", "note", "basis_or_reason", "time_uncertainty", "what_would_settle_it"}
NEXT_COLUMNS = {"next_step", "what_would_settle_it"}
STATE_COLUMNS = {"state", "status", "ensemble_status"}
BARE = {"n/a", "na", "—", "-", "nan", "null", "unknown"}
# Column names that are dimensionless by their nature; any other numeric column must carry a unit.
DIMENSIONLESS = re.compile(r"(^|_)(seed|samples?|n|n_pairs|pairs|swaps|count|counts|fraction|rejected_draws|rank|version|"
                           r"histories|source_histories|ratio|index|id|flag|wilson95_(low|high)|ci95_(low|high)|"
                           r"value_number|replaceable|recorded|replacements)(_|$)|per_source_neutron|_fraction$|^sample$|_swaps")
UNIT_IN_NAME = re.compile(r"(_|^)(m|kg|mw|twh|y|years?|s|n_m2|n_m2_s|w|pct|percent|mev|ev|k|cm|a|t|kg_m3|mwh)(_|$)|_n_m2|_m$|_y$|_kg$|_twh$|_w$")
ADVISORY = {"INT-040-stamp", "INT-040-units", "PRV-010", "PRV-012", "UX-046", "UX-046-empty", "VIS-033", "VIS-040", "DOC-062"}
NUMBER = re.compile(r"^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$")
COMMA_DECIMAL = re.compile(r"^[+-]?\d+,\d+$")
TWIN_STATEMENT = re.compile(r"design model\b.{0,40}\bnot a (digital )?twin", re.I | re.S)


def research_statement(root: Path = ROOT) -> str:
    src = (root / "crates/faris-model/src/lib.rs").read_text(encoding="utf-8")
    m = re.search(r"RESEARCH_SCREENING_STATEMENT: &str =\s*((?:\"[^\"]*\"\s*)+);", src)
    return "".join(re.findall(r'"([^"]*)"', m.group(1)))


def read_csv(path: Path) -> tuple[list[str], list[str], list[list[str]], list[str]]:
    """(problems, comment lines, header, rows) of one export CSV."""
    raw = path.read_bytes()
    problems = []
    if raw.startswith(b"\xef\xbb\xbf"):
        problems.append("starts with a byte-order mark")
    if b"\r" in raw:
        problems.append("has CR characters (lines must end in LF)")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return ["is not valid UTF-8"], [], [], []
    comments = [line for line in text.splitlines() if line.startswith("# ")]
    rows = list(csv.reader(io.StringIO("\n".join(l for l in text.split("\n") if not l.startswith("# ")), newline="")))
    rows = [r for r in rows if r]
    return problems, comments, rows[0] if rows else [], rows[1:]


def numeric_columns(header: list[str], rows: list[list[str]]) -> list[int]:
    out = []
    for i, name in enumerate(header):
        cells = [r[i] for r in rows if i < len(r) and r[i] != ""]
        if cells and all(NUMBER.match(c) for c in cells):
            out.append(i)
    return out


def lint_csv(path: Path, statement: str, uncertainty_fields: dict[str, set[str]]) -> list[tuple[str, str]]:
    name = path.name
    findings: list[tuple[str, str]] = []
    problems, comments, header, rows = read_csv(path)
    for p in problems:
        findings.append(("INT-040", f"{name}: {p}"))
    if not comments or comments[0] != f"# {statement}":
        findings.append(("INT-040", f"{name}: the first line is not the research-screening statement"))
    if not header:
        return findings + [("INT-040", f"{name}: no header row")]
    if any("study" in h and "hash" in h for h in header) is False and not any(re.search(r"study.?(file.?)?(hash|sha)|faris.?version", c, re.I) for c in comments):
        findings.append(("INT-040-stamp", f"{name}: the header carries neither the study hash nor the FARIS version (they are in export-manifest.json)"))
    for r, row in enumerate(rows, 2):
        for c in row:
            if COMMA_DECIMAL.match(c):
                findings.append(("INT-040", f"{name} row {r}: decimal comma in {c!r}"))
                break
    numeric = numeric_columns(header, rows)
    has_unit_column = "unit" in header
    kind_columns = {h for h in header if h == "kind" or h.endswith("_kind")}
    table_kind = "kind" in header or "history_kind" in header
    covered = uncertainty_fields.get(name, set())
    for i in numeric:
        col = header[i]
        if col in kind_columns:
            continue
        if not (has_unit_column or UNIT_IN_NAME.search(col) or DIMENSIONLESS.search(col)):
            findings.append(("INT-040-units", f"{name}: numeric column {col!r} names no unit (add one, or a unit column)"))
        stem_kind = any(col.startswith(k[: -len("_kind")]) for k in kind_columns if k != "kind")
        if not (table_kind or stem_kind or col in covered or DIMENSIONLESS.search(col)):
            findings.append(("PRV-010", f"{name}: numeric column {col!r} has no kind label (a {col}_kind column, a kind column, or uncertainty.csv rows)"))
    for k in sorted(kind_columns):
        i = header.index(k)
        bad = sorted({r[i] for r in rows if i < len(r) and r[i] and r[i] not in KINDS})
        if bad:
            findings.append(("PRV-010", f"{name}: column {k!r} holds {bad}, which are not kind labels"))
        if k != "kind" and not k.startswith("history"):
            stem = k[: -len("_kind")]
            if any(r[i] == "calculated" for r in rows if i < len(r)) and not any(h.startswith(stem) and "standard_error" in h for h in header):
                findings.append(("VIS-030", f"{name}: {stem!r} is calculated but has no standard-error column"))
    for i, h in enumerate(header):
        if h in STATE_COLUMNS:
            reason = [header.index(c) for c in header if c in REASON_COLUMNS]
            nxt = [header.index(c) for c in header if c in NEXT_COLUMNS]
            for r, row in enumerate(rows, 2):
                if i < len(row) and row[i] in NOT_EVALUATED:
                    why = " ".join(row[j] for j in reason if j < len(row)).strip()
                    step = " ".join(row[j] for j in nxt if j < len(row)).strip() or ("next step" if re.search(r"next step", why, re.I) else "")
                    if len(why) < 10 or not step:
                        findings.append(("PRV-012", f"{name} row {r}: state {row[i]!r} without {'a reason' if len(why) < 10 else 'a next step'}"))
    for r, row in enumerate(rows, 2):
        for c in row:
            if c.strip().lower() in BARE:
                findings.append(("UX-046", f"{name} row {r}: bare placeholder cell {c!r}"))
                break
    empty = sorted({header[i] for r in rows for i in range(min(len(r), len(header))) if r[i] == ""})
    if empty:
        findings.append(("UX-046-empty", f"{name}: empty cells in {len(empty)} column(s): {', '.join(empty[:6])}{'...' if len(empty) > 6 else ''}"))
    if name == "caveats.csv":
        for r, row in enumerate(rows, 2):
            d = dict(zip(header, row))
            if len(d.get("why", "")) < 10 or len(d.get("what_would_settle_it", "")) < 10:
                findings.append(("PRV-012", f"{name} row {r}: caveat {d.get('item')!r} lacks why or what would settle it"))
    return findings


def svg_texts(path: Path) -> list[tuple[str, dict]]:
    out = []
    for m in re.finditer(r"<text([^>]*)>([^<]*)", path.read_text(encoding="utf-8")):
        text = re.sub(r"\s+", " ", m.group(2)).strip()
        if text:
            out.append((text, {"rotate": "rotate(" in m.group(1), "middle": 'text-anchor="middle"' in m.group(1)}))
    return out


def lint_chart(path: Path, statement: str) -> list[tuple[str, str]]:
    name = f"charts/{path.name}"
    texts = svg_texts(path)
    joined = " ".join(t for t, _ in texts)
    findings = []
    if statement not in joined:
        findings.append(("INT-044", f"{name}: the research-screening statement is not in the chart footer"))
    labels = [t for t, a in texts if a["middle"] and not NUMBER.match(t.replace("×10", "e").replace(" ", "")) and len(t) > 3]
    x_label = next((t for t, a in texts if a["middle"] and not a["rotate"] and re.search(r"[A-Za-z]{3}", t) and "(" in t or (a["middle"] and not a["rotate"] and re.search(r"[A-Za-z]{4}", t))), None)
    y_label = next((t for t, a in texts if a["rotate"]), None)
    for axis, label in (("x", x_label), ("y", y_label)):
        if label is None:
            findings.append(("VIS-040", f"{name}: no {axis}-axis label (quantity and unit)"))
        elif not re.search(r"\([^)]+\)", label):
            findings.append(("VIS-040", f"{name}: {axis}-axis label {label!r} has no (unit) or (dimensionless)"))
    head = " ".join(t for t, a in texts[:3]).lower()
    if not any(k in head for k in ("calculated", "authored", "literature", "conditional", "not evaluated")):
        findings.append(("VIS-040", f"{name}: title and subtitle name no kind label"))
    if re.search(r"±|\bSE\b|P5|band|error", joined) and not re.search(r"statistical only|sampling only|sampling uncertainty only|statistical, nuclear data", joined):
        findings.append(("VIS-033", f"{name}: draws an error depiction without saying it is statistical only or listing its sources"))
    del labels
    return findings


def lint_export(export: Path, root: Path = ROOT) -> list[tuple[str, str]]:
    statement = research_statement(root)
    findings: list[tuple[str, str]] = []
    manifest_path = export / "export-manifest.json"
    if not manifest_path.is_file():
        return [("INT-044", "export-manifest.json is missing")]
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("research_screening") != statement:
        findings.append(("INT-044", "export-manifest.json research_screening is not the statement"))
    for key in ("faris_version", "generated_utc", "files"):
        if not manifest.get(key):
            findings.append(("INT-044", f"export-manifest.json has no {key}"))
    if "study_file" not in manifest:
        findings.append(("INT-044", "export-manifest.json has no study_file stamp (null when unsaved)"))
    blob = json.dumps(manifest)
    if not TWIN_STATEMENT.search(blob):
        findings.append(("DOC-062", "export-manifest.json does not say that FARIS outputs come from a design model, not a twin"))
    listed = {f["path"]: f for f in manifest.get("files", [])}
    present = {str(p.relative_to(export)): p for p in export.rglob("*") if p.is_file() and p.name != "export-manifest.json"}
    for rel in sorted(set(present) - set(listed)):
        findings.append(("INT-044", f"{rel}: not listed in export-manifest.json"))
    for rel in sorted(set(listed) - set(present)):
        findings.append(("INT-044", f"{rel}: listed in export-manifest.json but missing"))
    for rel, entry in listed.items():
        if rel in present:
            digest = hashlib.sha256(present[rel].read_bytes()).hexdigest()
            if not re.fullmatch(r"[0-9a-f]{64}", entry.get("sha256", "")) or digest != entry["sha256"]:
                findings.append(("INT-044", f"{rel}: full SHA-256 does not match the manifest"))
    for rel in sorted(present):
        if rel.endswith(".png") and rel != "charts/3d-view.png" and rel[:-4] + ".svg" not in present:
            findings.append(("INT-044", f"{rel}: PNG without an SVG sidecar that carries the statement"))
    docs = (root / "docs/STUDY_EXPORT.md").read_text(encoding="utf-8")
    documented = re.findall(r"`([^`]+)`", "\n".join(l for l in docs.splitlines() if l.startswith("| `")))
    patterns = [p for cell in documented for p in re.split(r",\s*", cell)] + ["export-manifest.json"]
    for rel in sorted(present):
        if not any(fnmatch(rel, p) for p in patterns):
            findings.append(("INT-044", f"{rel}: not documented in docs/STUDY_EXPORT.md"))
    uncertainty: dict[str, set[str]] = {}
    unc = export / "data" / "uncertainty.csv"
    if unc.is_file():
        _, _, header, rows = read_csv(unc)
        if "table" in header and "field" in header:
            t, f = header.index("table"), header.index("field")
            for r in rows:
                uncertainty.setdefault(r[t], set()).add(r[f])
    for csv_path in sorted((export / "data").glob("*.csv")):
        findings += lint_csv(csv_path, statement, uncertainty)
    for svg in sorted((export / "charts").glob("*.svg")):
        findings += lint_chart(svg, statement)
    claims = []
    for path in sorted((export / "data").glob("*.csv")):
        text = "\n".join(line for line in path.read_text(encoding="utf-8").splitlines() if not line.startswith("# "))
        claims += wording_lint.lint_text(f"data/{path.name}", [(text, 1)], wording_lint.load_terms())
    findings += [("VAL-090", f"{f.path}: {f.rule} {f.text!r} in: {f.sentence[:100]}") for f in claims if f.rule in ("LEG-041", "LEG-042", "QA-052")]
    guide = (root / "docs/USER_GUIDE.md").read_text(encoding="utf-8")
    if not TWIN_STATEMENT.search(guide):
        findings.append(("DOC-062", "docs/USER_GUIDE.md does not state the design-model-not-a-twin sentence"))
    return findings


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="lint a study export folder")
    ap.add_argument("export", type=Path)
    ap.add_argument("--strict", action="store_true", help="advisory findings fail too")
    args = ap.parse_args(argv)
    findings = lint_export(args.export)
    failing = 0
    for rule, message in findings:
        advisory = rule in ADVISORY and not args.strict
        failing += not advisory
        print(f"{'advisory ' if advisory else ''}{rule}: {message}")
    print(f"export lint: {len(findings)} finding(s), {failing} failing", file=sys.stderr)
    return 1 if failing else 0


if __name__ == "__main__":
    sys.exit(main())
