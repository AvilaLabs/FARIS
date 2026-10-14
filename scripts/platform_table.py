#!/usr/bin/env python3
"""Checks the support table in docs/guide/platforms.md against the CI workflows (PLAT-001).

Every row must either name its evidence as `workflow.yml:job`, where the job exists in
.github/workflows/workflow.yml and the row's runner label (if it has one) appears in that
file, or say `NONE: reason` with a reason. Exit 1 on any problem; the rows without evidence
are listed so that they stay visible.
"""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TABLE = ROOT / "docs/guide/platforms.md"
WORKFLOWS = ROOT / ".github/workflows"
START, END = "<!-- platform-table:start -->", "<!-- platform-table:end -->"
JOB_REF = re.compile(r"^`([\w.-]+\.yml):([\w-]+)`$")


def parse_rows(text):
    """Returns (platform, tier, runner, evidence) per table row; raises ValueError if the table is absent."""
    if START not in text or END not in text:
        raise ValueError(f"the table markers {START} and {END} are missing")
    body = text.split(START, 1)[1].split(END, 1)[0]
    rows = []
    for line in body.strip().splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 5:
            raise ValueError(f"row does not have 5 columns: {line}")
        if cells[0] in ("Platform",) or set(cells[0]) <= set("- "):
            continue
        rows.append(tuple(cells[:4]))
    if not rows:
        raise ValueError("the table has no rows")
    return rows


def job_ids(workflow_text):
    """Job ids of a workflow file: the keys one level under `jobs:`."""
    ids, in_jobs = [], False
    for line in workflow_text.splitlines():
        if re.match(r"^jobs:\s*$", line):
            in_jobs = True
        elif in_jobs and re.match(r"^\S", line):
            in_jobs = False
        elif in_jobs:
            m = re.match(r"^  ([\w-]+):\s*(#.*)?$", line)
            if m:
                ids.append(m.group(1))
    return ids


def check(table_text, workflows_dir):
    """Returns (problems, rows without evidence)."""
    problems, gaps = [], []
    for platform, tier, runner, evidence in parse_rows(table_text):
        if tier not in ("1", "2", "3"):
            problems.append(f"{platform}: tier '{tier}' is not 1, 2 or 3")
        if evidence.startswith("NONE"):
            reason = evidence[len("NONE"):].lstrip(": ").strip()
            if not reason:
                problems.append(f"{platform}: NONE needs a reason")
            gaps.append(platform)
            continue
        # The evidence cell may carry a note after the reference; the reference is the first backticked item.
        m = re.match(r"^(`[^`]+`)", evidence)
        ref = JOB_REF.match(m.group(1)) if m else None
        if not ref:
            problems.append(f"{platform}: evidence '{evidence}' is not `workflow.yml:job` or NONE: reason")
            continue
        wf = workflows_dir / ref.group(1)
        if not wf.is_file():
            problems.append(f"{platform}: workflow {ref.group(1)} does not exist")
            continue
        text = wf.read_text()
        if ref.group(2) not in job_ids(text):
            problems.append(f"{platform}: job {ref.group(2)} is not in {ref.group(1)}")
        label = runner.strip("`")
        if label and label != "none" and label not in text:
            problems.append(f"{platform}: runner or image '{label}' does not appear in {ref.group(1)}")
    return problems, gaps


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail on any row without a real job")
    ap.add_argument("--table", type=Path, default=TABLE)
    ap.add_argument("--workflows", type=Path, default=WORKFLOWS)
    args = ap.parse_args()
    try:
        problems, gaps = check(args.table.read_text(), args.workflows)
    except ValueError as err:
        problems, gaps = [str(err)], []
    for g in gaps:
        print(f"no automated evidence: {g}")
    for p in problems:
        print(f"PROBLEM: {p}")
    if problems:
        return 1
    print(f"platform table consistent with the workflows ({len(gaps)} rows without a hosted runner)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
