#!/usr/bin/env python3
"""Cross-check the GEO-023 corpus against DAGMC's own tools (a cross-check only).

For every corpus model that was converted to an h5m, run DAGMC's `check_watertight`
(topological check, no tolerance) and `overlap_check` on a copy, and record what they
say next to what FARIS's check 8 says. FARIS's checks do not use these tools; this
only shows where they agree and where they do not. Standard library only:

    faris_corpus_dagmc.py --work WORKDIR --corpus CORPUS_DIR --bin DAGMC_BIN_DIR --out results.json

`--work` is the directory `faris_corpus.py generate` was given as --work. The FARIS
verdicts come from a JSON file written by the Rust corpus test (--faris verdicts.json)
mapping model id to the sorted list of blocking requirement ids.
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def run(command: list[str], timeout: float = 600.0) -> tuple[int, str]:
    done = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    return done.returncode, done.stdout + done.stderr


def watertight(tool: Path, h5m: Path) -> dict:
    with tempfile.TemporaryDirectory() as scratch:
        copy = Path(scratch) / "in.h5m"
        shutil.copyfile(h5m, copy)
        code, text = run([str(tool), "-o", str(Path(scratch) / "out.h5m"), str(copy)])
    unmatched = re.search(r"(\d+)/(\d+) \(\S+\) unmatched edges", text)
    surfaces = re.search(r"(\d+)/(\d+) \(\S+\) unsealed surfaces", text)
    volumes = re.search(r"(\d+)/(\d+) \(\S+\) unsealed volumes", text)
    if not (unmatched and surfaces and volumes):
        return {"verdict": "error", "exit_code": code, "tail": text.strip().splitlines()[-3:]}
    leaky = int(unmatched.group(1)) + int(surfaces.group(1)) + int(volumes.group(1))
    return {
        "verdict": "leaky" if leaky else "watertight", "exit_code": code,
        "unmatched_edges": int(unmatched.group(1)), "unsealed_surfaces": int(surfaces.group(1)),
        "unsealed_volumes": int(volumes.group(1)),
    }


def overlaps(tool: Path, h5m: Path) -> dict:
    code, text = run([str(tool), "-p", "4", str(h5m)])
    if "No overlaps were found" in text:
        return {"verdict": "none", "exit_code": code}
    if "overlap" in text.lower() and code == 0:
        return {"verdict": "overlaps", "exit_code": code, "tail": text.strip().splitlines()[-4:]}
    return {"verdict": "error", "exit_code": code, "tail": text.strip().splitlines()[-3:]}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--bin", type=Path, required=True)
    parser.add_argument("--faris", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    index = json.loads((args.corpus / "corpus.json").read_text())
    faris = json.loads(args.faris.read_text())
    rows = []
    for m in index["models"]:
        if m["subject"] == "design":
            continue
        candidates = [args.work / m["id"] / "model.h5m", args.work / m["id"] / "convert" / "model.h5m"]
        h5m = next((c for c in candidates if c.is_file()), None)
        row = {"id": m["id"], "class": m["class"], "kind": m["kind"], "faris_blocking": faris.get(m["id"])}
        if h5m is None:
            row["dagmc"] = "no h5m (the defect was applied to the facets, or the conversion failed)"
        else:
            row["check_watertight"] = watertight(args.bin / "check_watertight", h5m)
            row["overlap_check"] = overlaps(args.bin / "overlap_check", h5m)
            dagmc_flags = row["check_watertight"]["verdict"] == "leaky" or row["overlap_check"]["verdict"] == "overlaps"
            faris_flags = bool(row["faris_blocking"])
            row["agree"] = dagmc_flags == faris_flags
        rows.append(row)
    compared = [r for r in rows if "agree" in r]
    summary = {
        "models_with_h5m": len(compared),
        "agree": sum(1 for r in compared if r["agree"]),
        "disagree": [r["id"] for r in compared if not r["agree"]],
    }
    args.out.write_text(json.dumps({"summary": summary, "rows": rows}, indent=1) + "\n")
    print(json.dumps(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
