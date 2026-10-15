#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Release notes generated from the release record (requirements QA-074, PLAT-034, LEG-052).

    python3 scripts/release_notes.py --version 0.3.0 --release-record release-record.json \\
        --gate gate-output.txt [--root DIR] [--out notes.md]

Writes the notes for one version from: that version's section of CHANGELOG.md, the release record written by
`python3 -m validation report --release-record` (the V&V report hash), the archived output of
scripts/release_gate.py, and the defect log. The sections are, in order:

1. Changes that can alter a published number (PLAT-034). Taken from the "### Numerical changes" heading of the
   version's changelog section. When the section has none, the notes say that the changelog records none and that
   no golden-study diff was run (PRV-027 is not built), because absence from a changelog is not a measurement.
2. Security fixes: the "### Security" heading of the changelog section, with the advisory IDs.
3. Verification and validation status: the V&V report hash and the gate's conditions with PASS or FAIL.
4. Gate result: the gate's last line.
5. Known issues: counts from the defect log and a pointer to docs/KNOWN_ISSUES.md.
6. Migration guides named in the changelog section.
7. Export-control review status (LEG-052), always present.
8. Software bill of materials files.

The script exits 1, writing nothing, when the version has no changelog section, the release record or the gate
output is missing or unreadable, or the release record has no V&V report hash.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXPORT_LINE = (
    "Export-control review status of the code, the bundled data and the generated files: NOT REVIEWED "
    "(draft statement in docs/EXPORT_CONTROL.md awaits owner and counsel review)."
)
NO_NUMERIC = (
    "The changelog records no change that alters a published number in this release. That is a statement from "
    "the changelog, not a measurement: the golden-study comparison between versions (PRV-027) is not automated, "
    "so a change that nobody wrote down would not be found here."
)


def changelog_section(text: str, version: str) -> str:
    """The body of the `## VERSION` section of the changelog, or "Unreleased" for the version in progress."""
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(rf"^##\s+{re.escape(version)}(\s|$)", line)), None)
    if start is None:
        raise ValueError(f"CHANGELOG.md has no section for {version}")
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    return "\n".join(lines[start + 1:end])


def subsection(body: str, title: str) -> str | None:
    lines = body.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(rf"^###\s+{re.escape(title)}\s*$", line)), None)
    if start is None:
        return None
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("### ")), len(lines))
    text = "\n".join(lines[start + 1:end]).strip()
    return text or None


def gate_lines(gate_text: str) -> tuple[list[str], str]:
    lines = [l for l in gate_text.splitlines() if l.strip()]
    conditions = [l for l in lines if re.match(r"^(PASS|FAIL)\s", l)]
    summary = next((l for l in reversed(lines) if l.startswith("release gate:")), None)
    if not conditions or summary is None:
        raise ValueError("the gate output has no condition lines or no 'release gate:' summary line")
    return conditions, summary


def defect_counts(root: Path) -> dict[str, int]:
    path = root / "docs" / "defects" / "faris-defects.json"
    counts = {"open": 0, "fixed": 0, "wont-fix": 0}
    for d in json.loads(path.read_text(encoding="utf-8"))["defects"]:
        counts[d["status"]] = counts.get(d["status"], 0) + 1
    return counts


def render(version: str, changelog: str, record: dict, gate_text: str, counts: dict[str, int]) -> str:
    body = changelog_section(changelog, version)
    report_hash = record.get("vv_report_sha256")
    if not isinstance(report_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", report_hash):
        raise ValueError("the release record has no vv_report_sha256 (run python3 -m validation report --release-record)")
    conditions, summary = gate_lines(gate_text)
    numeric = subsection(body, "Numerical changes")
    security = subsection(body, "Security")
    migrations = sorted(set(re.findall(r"\(((?:\.\./)*docs/migrations/[^)\s]+|migrations/[^)\s]+)\)", body)))
    out = [f"# FARIS {version}", "", "Research screening only. FARIS results are not a licensing, safety or design basis.", "",
           "## Changes that can alter a published number", "", numeric or NO_NUMERIC, "",
           "## Security fixes", "", security or "No security fix in this release.", "",
           "## Verification and validation status", "",
           f"V&V report SHA-256: `{report_hash}`", "",
           "Release gate conditions:", "", *[f"- {c}" for c in conditions], "",
           "## Gate result", "", summary, "",
           "## Known issues", "",
           f"{counts.get('open', 0)} open and {counts.get('wont-fix', 0)} unfixed defects in the project's defect log; "
           f"{counts.get('fixed', 0)} fixed. The full list is `docs/KNOWN_ISSUES.md`.", "",
           "## Migration guides", "",
           *( [f"- `{m}`" for m in migrations] or ["None for this release."] ), "",
           "## Export-control review", "", EXPORT_LINE, "",
           "## Software bill of materials", "",
           "`faris.spdx.json` (SPDX 2.3) and `faris.cdx.json` (CycloneDX 1.5) are attached to the release.", ""]
    return "\n".join(out)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--version", required=True)
    ap.add_argument("--release-record", required=True, type=Path)
    ap.add_argument("--gate", required=True, type=Path, help="archived output of scripts/release_gate.py")
    ap.add_argument("--root", type=Path, default=ROOT)
    ap.add_argument("--out", type=Path)
    args = ap.parse_args(argv)
    try:
        text = render(args.version, (args.root / "CHANGELOG.md").read_text(encoding="utf-8"),
                      json.loads(args.release_record.read_text(encoding="utf-8")),
                      args.gate.read_text(encoding="utf-8"), defect_counts(args.root))
    except (OSError, ValueError, KeyError) as e:
        print(f"release notes: {e}", file=sys.stderr)
        return 1
    if args.out:
        args.out.write_text(text, encoding="utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
