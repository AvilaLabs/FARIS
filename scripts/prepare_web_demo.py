#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Collect the recorded study the browser version opens on load.

Reads an unpacked release package (the folder holding package-index.json),
checks every file it copies against the index, and writes them with a
manifest.json to crates/faris-app/demo/, which `trunk build` copies beside the
page. Only recorded results are copied: the port and no-port transport
bundles, the operating assumptions, the allocation sweep's bundles and the
recorded maintenance result. No program and no nuclear data is included.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "crates/faris-app/demo"
VARIANTS = ("reference", "breeder-emphasis")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def prepare(package: Path, out: Path, ensembles: Path | None = None) -> dict:
    index = json.loads((package / "package-index.json").read_text(encoding="utf-8"))
    listed = {entry["path"]: entry for entry in index["files"]}

    def take(relative: str) -> dict:
        entry = listed.get(relative)
        if entry is None:
            raise ValueError(f"{relative} is not in the package index")
        source = package / relative
        digest = sha256(source)
        if source.stat().st_size != entry["bytes"] or f"sha256:{digest}" != entry["sha256"]:
            raise ValueError(f"{relative} differs from the package index")
        target = out / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        return {"path": relative, "bytes": entry["bytes"], "sha256": f"sha256:{digest}"}

    if out.exists():
        shutil.rmtree(out)
    core = [take("port/scenario.json")]
    core += [take(f"port/bundles/{v}.transport-bundle.json") for v in VARIANTS]
    core.append(take("control/scenario.json"))
    core += [take(f"control/bundles/{v}.transport-bundle.json") for v in VARIANTS]
    core.append(take("operating-assumptions.json"))
    sweep = [take(run["transport_bundle"]) for run in index["sweep"]["runs"]] if index.get("sweep") else []
    maintenance = None
    if index.get("maintenance"):
        maintenance = take(index["maintenance"]["result"])
    ranges = None
    if ensembles is not None:
        target = out / "ensembles.json"
        shutil.copyfile(ensembles, target)
        ranges = {"path": "ensembles.json", "bytes": target.stat().st_size}
    manifest = {
        "schema": "faris-web-demo/v1",
        "package_schema": index["schema_version"],
        "package_status": index["status"],
        "core": [{"path": f["path"], "bytes": f["bytes"]} for f in core],
        "sweep": [{"path": f["path"], "bytes": f["bytes"]} for f in sweep],
        "maintenance": maintenance and {k: maintenance[k] for k in ("path", "sha256", "bytes")},
        "ensembles": ranges,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n", encoding="utf-8")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="unpacked release package folder")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument(
        "--ensembles",
        type=Path,
        help="uncertainty ranges written by `faris-app --package PACKAGE --write-web-ensembles FILE`",
    )
    args = parser.parse_args()
    result = prepare(args.package, args.out, args.ensembles)
    total = sum(f["bytes"] for key in ("core", "sweep") for f in result[key])
    print(f"{len(result['core'])} core + {len(result['sweep'])} sweep files, {total} bytes -> {args.out}")
