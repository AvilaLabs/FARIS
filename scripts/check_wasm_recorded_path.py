#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Compile the production recorded-path predicate for wasm and execute it."""
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/faris-engine/src/transport.rs"


def function_source(source: str, name: str) -> str:
    start = source.index(f"fn {name}(")
    opening = source.index("{", start)
    depth = 0
    for end in range(opening, len(source)):
        if source[end] == "{":
            depth += 1
        elif source[end] == "}":
            depth -= 1
            if depth == 0:
                return source[start : end + 1]
    raise ValueError(f"unterminated {name}")


def main() -> None:
    predicate = function_source(
        SOURCE.read_text(encoding="utf-8"), "recorded_library_path_is_absolute"
    )
    probes = []
    for name, path in (
        ("accepts_posix_absolute", "/tmp/lib/libopenmc.so"),
        ("accepts_root_only", "/"),
        ("rejects_empty", ""),
        ("rejects_relative", "tmp/lib/libopenmc.so"),
        ("rejects_drive_relative", "C:lib/libopenmc.so"),
    ):
        probes.append(
            '#[unsafe(no_mangle)] pub extern "C" fn '
            + name
            + "() -> u32 { recorded_library_path_is_absolute("
            + json.dumps(path)
            + ") as u32 }"
        )
    harness = "\n".join(
        ["#[allow(unused_imports)] use std::path::Path;", predicate, *probes]
    )
    with tempfile.TemporaryDirectory(prefix="faris-wasm-path-") as temporary:
        temporary = Path(temporary)
        source = temporary / "probe.rs"
        wasm = temporary / "probe.wasm"
        source.write_text(harness, encoding="utf-8")
        subprocess.run(
            [
                "rustc",
                "--edition",
                "2024",
                "--target",
                "wasm32-unknown-unknown",
                "--crate-type",
                "cdylib",
                str(source),
                "-o",
                str(wasm),
            ],
            check=True,
        )
        node = '''const fs = await import("node:fs/promises");
const {instance} = await WebAssembly.instantiate(await fs.readFile(process.argv[1]));
const checks = {
  posix_absolute: instance.exports.accepts_posix_absolute(),
  root_only: instance.exports.accepts_root_only(),
  empty_rejected: instance.exports.rejects_empty() === 0,
  relative_rejected: instance.exports.rejects_relative() === 0,
  drive_relative_rejected: instance.exports.rejects_drive_relative() === 0,
};
if (Object.values(checks).some(value => value !== true && value !== 1)) throw Error(JSON.stringify(checks));
console.log(`wasm recorded-path contract: ${JSON.stringify(checks)}`);'''
        subprocess.run(
            ["node", "--input-type=module", "-e", node, str(wasm)], check=True
        )


if __name__ == "__main__":
    main()
