#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Check that a local recorded-demo bundle still contains its pinned binaries."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_recorded_demo import rust_platform


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return "sha256:" + digest.hexdigest()


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: verify_binary_manifest.py PACKAGE_ROOT", file=sys.stderr)
        return 2
    root = Path(sys.argv[1]).resolve(strict=True)
    index = json.loads((root / "package-index.json").read_text(encoding="utf-8"))
    runtime = index.get("local_runtime")
    if not isinstance(runtime, dict) or runtime.get("schema_version") != "faris-local-runtime/v0.1":
        print("package has no supported local runtime manifest", file=sys.stderr)
        return 1
    if runtime.get("platform") != rust_platform():
        print("bundled executables target a different operating system or architecture", file=sys.stderr)
        return 1
    executables = runtime.get("executables")
    if not isinstance(executables, dict) or set(executables) != {"faris", "faris-app", "avila-core"}:
        print("package runtime executable list is malformed", file=sys.stderr)
        return 1
    top_level = {"faris": index.get("faris_cli_sha256"),
                 "faris-app": index.get("faris_app_sha256"),
                 "avila-core": index.get("core_executable_sha256")}
    for name, record in executables.items():
        relative = record.get("path")
        if not isinstance(relative, str) or Path(relative).is_absolute() or ".." in Path(relative).parts:
            print(f"unsafe runtime path for {name}", file=sys.stderr)
            return 1
        executable = (root / relative).resolve(strict=True)
        if root not in executable.parents or not executable.is_file():
            print(f"runtime executable escapes the package: {name}", file=sys.stderr)
            return 1
        observed = sha256(executable)
        if (observed != record.get("sha256") or observed != top_level[name]
                or executable.stat().st_size != record.get("bytes")
                or not executable.stat().st_mode & 0o111):
            print(f"runtime executable identity or permissions changed: {name}", file=sys.stderr)
            return 1
        version = subprocess.run(
            [str(executable), "--version"], capture_output=True, text=True,
            check=False, timeout=10)
        # A CI-built package records the programs' versions once, in desktop_build.
        expected = record.get("version") if "version" in record or "desktop_build" not in index else None
        if version.returncode != 0 or (expected is not None and version.stdout.strip() != expected):
            print(f"runtime executable version differs from its recorded version: {name}", file=sys.stderr)
            return 1
    print("Pinned local executables revalidated; hashes establish identity, not authenticity.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
