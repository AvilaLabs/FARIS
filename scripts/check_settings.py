#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Settings registry checks (requirements CFG-009, CFG-076).

    python3 scripts/check_settings.py [--list]

CFG-076  docs/settings-keys.json is the history of the settings keys: every key the registry (faris_model::settings::
         REGISTRY) holds is under "live", every "live" key is still in the registry or has moved to "retired" with a
         replacement (or null, with a reason) and the version that retired it, and a retired key must not appear in the
         registry again. A key is never reused for another meaning: removing one means retiring it.
CFG-009  every environment variable the non-test Rust code names (the FARIS_* string literals, and every literal passed
         to env::var, env::var_os, option_env! or env!) is either the variable of a registry setting, a standard
         operating-system or Cargo variable, or listed in ENV_EXCEPTIONS with what it does. A new unlisted variable
         fails the check. --list prints every variable read and where. The exceptions are the variables that are not
         yet in the registry; they are named, not hidden.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import source_lint  # noqa: E402

REGISTRY_FILE = Path("crates/faris-model/src/settings.rs")
HISTORY_FILE = Path("docs/settings-keys.json")
# Variables set by the operating system, the shell or Cargo, not settings.
STANDARD_ENV = {"PATH", "HOME", "SystemRoot", "OPENMC_CROSS_SECTIONS", "OUT_DIR", "NUM_JOBS"}
# variable -> (kind, what it does). Not in the registry.
ENV_EXCEPTIONS = {
    "FARIS_CONFIG_DIR": ("runtime", "folder of the settings file itself, so it cannot live in the settings file"),
    "FARIS_SCRATCH_DIR": ("runtime", "folder for scratch runs of external tools"),
    "FARIS_CLI_EXECUTABLE": ("runtime", "the faris program the desktop runs for studies"),
    "FARIS_KILL_TRIALS": ("test-harness", "trial count of the crash-during-save campaign (atomic_file/campaign.rs)"),
    "FARIS_KILL_GO": ("test-harness", "start marker passed to the crash-during-save child process"),
    "FARIS_SOURCE_DIR": ("build", "source folder baked in at build time by .cargo/config.toml"),
    "FARIS_OCCT_PREFIX": ("build", "where build.rs installs the static OCCT"),
    "FARIS_OCCT_TARBALL": ("build", "local OCCT tarball for build.rs"),
    "FARIS_CMAKE": ("build", "cmake program for build.rs"),
    "FARIS_OCCT_SHA256": ("build", "OCCT digest compiled into the program by build.rs"),
    "FARIS_OCCT_TAG": ("build", "OCCT tag compiled into the program by build.rs"),
    "FARIS_OCCT_WINDOWS_HELPER": ("test-harness", "marks the isolated Windows cancellation-test helper child process"),
    "FARIS_OCCT_WINDOWS_PID_FILE": ("test-harness", "PID file used by the Windows cancellation-test helper child process"),
}
KEY_LINE = re.compile(r'^\s{8}"([a-z][a-z_]*\.[a-z][a-z_]*)",\s*$', re.M)
ENV_LITERAL = re.compile(r'"(FARIS_[A-Z0-9_]+)"')
ENV_READ = re.compile(r'(?:\benv::var_os|\benv::var|\boption_env!|\benv!)\(\s*"([^"]+)"')
ENV_FIELD = re.compile(r'Some\("(FARIS_[A-Z0-9_]+)"\)')


def is_standard(name: str) -> bool:
    return name in STANDARD_ENV or name.startswith("CARGO_")


def registry_text(root: Path) -> str:
    text = (root / REGISTRY_FILE).read_text(encoding="utf-8")
    start = text.index("pub static REGISTRY")
    return text[start:text.index("\n];", start)]


def registry_keys(root: Path = ROOT) -> list[str]:
    return KEY_LINE.findall(registry_text(root))


def registry_envs(root: Path = ROOT) -> set[str]:
    return set(ENV_FIELD.findall(registry_text(root)))


def check_history(keys: list[str], history: dict) -> list[str]:
    """Problems with the key history against the registry keys."""
    live = history.get("live")
    retired = history.get("retired")
    if not isinstance(live, dict) or not isinstance(retired, list):
        return ["the key history needs a 'live' object and a 'retired' list"]
    problems = []
    retired_keys = set()
    for entry in retired:
        key = entry.get("key") if isinstance(entry, dict) else None
        if not isinstance(key, str) or not isinstance(entry.get("retired_in"), str) or "replacement" not in entry:
            problems.append(f"retired entry {entry!r} needs 'key', 'replacement' (a key or null) and 'retired_in'")
            continue
        if entry["replacement"] is None and not str(entry.get("reason", "")).strip():
            problems.append(f"retired key {key} has no replacement and no reason")
        if entry["replacement"] is not None and not isinstance(entry["replacement"], str):
            problems.append(f"retired key {key} has a replacement that is not a key")
        if key in retired_keys:
            problems.append(f"retired key {key} is listed twice")
        retired_keys.add(key)
    for key in sorted(set(keys) & retired_keys):
        problems.append(f"retired key {key} is in the registry again; a key is never reused, pick a new name")
    for key in sorted(set(keys) - set(live)):
        problems.append(f"registry key {key} is not in the key history; add it under 'live' with the version that introduced it")
    for key in sorted(set(live) - set(keys) - retired_keys):
        problems.append(f"key {key} was released but is gone from the registry and not retired")
    return problems


def env_reads(root: Path = ROOT) -> dict[str, set[str]]:
    """Variable name -> files, over non-test Rust code and the build scripts."""
    found: dict[str, set[str]] = {}
    files = source_lint.rust_files(root) + sorted(root.glob("crates/*/build.rs"))
    for path in files:
        code = source_lint.strip_rust(path.read_text(encoding="utf-8"), keep_strings=True)
        for pattern in (ENV_LITERAL, ENV_READ):
            for m in pattern.finditer(code):
                found.setdefault(m.group(1), set()).add(str(path.relative_to(root)))
    return found


def check_env(reads: dict[str, set[str]], registered: set[str]) -> list[str]:
    problems = []
    for name, files in sorted(reads.items()):
        if name in registered or is_standard(name) or name in ENV_EXCEPTIONS:
            continue
        problems.append(f"environment variable {name} is read in {', '.join(sorted(files))} but is neither a registry "
                        "setting's variable nor listed in ENV_EXCEPTIONS")
    for name in sorted(ENV_EXCEPTIONS):
        if name not in reads:
            problems.append(f"ENV_EXCEPTIONS lists {name}, which no code reads; remove it")
        if name in registered:
            problems.append(f"ENV_EXCEPTIONS lists {name}, which is in the registry; remove it")
    return problems


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="settings registry checks")
    ap.add_argument("--list", action="store_true", help="print every environment variable the code reads")
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    root = args.root
    keys, registered = registry_keys(root), registry_envs(root)
    reads = env_reads(root)
    if args.list:
        for name, files in sorted(reads.items()):
            kind = "registry" if name in registered else "standard" if is_standard(name) else ENV_EXCEPTIONS.get(name, ("UNLISTED", ""))[0]
            print(f"{name}\t{kind}\t{', '.join(sorted(files))}")
    problems = check_history(keys, json.loads((root / HISTORY_FILE).read_text(encoding="utf-8"))) + check_env(reads, registered)
    for p in problems:
        print(p)
    print(f"settings check: {len(keys)} registry keys, {len(reads)} environment variables, {len(problems)} problem(s)", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
