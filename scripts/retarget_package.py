#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Build a Windows or macOS package from a finished, verified Linux v0.5 package.

The recorded study data are produced on Linux. The Windows and macOS programs are
built by the desktop workflow, which uploads `bin/` and `build.json`
(faris-desktop-build/v0.1) per platform, with the platform's two licence notices
under `notices`. This script copies the Linux package, swaps the three programs, the two
notices files and SOURCE_PROVENANCE.md for the platform's, and rewrites the index; it
executes nothing.

Refuses unless:
- the Linux package's index is v0.5 for linux/x86_64, its package-index.sha256 matches,
  and every indexed file has the indexed size and SHA-256;
- build.json is a valid faris-desktop-build/v0.1 for windows or macos on x86_64 or
  aarch64, and each program in it has the recorded SHA-256 and size, at bin/<name>
  (bin/<name>.exe on Windows), and `notices` names licenses/faris-THIRD_PARTY_NOTICES.md
  and licenses/core-RUNTIME_DEPENDENCY_NOTICES.md, each with the recorded SHA-256 and size;
- the build's FARIS and Core commits equal the commits the package records, and the
  build's `faris --version` equals the version the Linux package recorded for `faris`;
- the output directory does not exist.

The output is the Linux package with every file except bin/*, the two notices files,
SOURCE_PROVENANCE.md, package-index.json, package-index.sha256 and README.md hard-linked
(copied where linking fails), the build's programs in bin/ (mode 0755), the build's notices
and a SOURCE_PROVENANCE.md written for the platform (release-profile desktop build), and an
index that names the new platform and programs, records the release profile and rebuild
route in `local_runtime.source_provenance`, keeps the Linux programs that produced the
recorded evidence under `evidence_recorded_with`, and carries the build record under
`desktop_build`.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
from pathlib import Path

INDEX = "package-index.json"
CHECKSUM = "package-index.sha256"
SCHEMA = "faris-recorded-demo-package/v0.5"
BUILD_SCHEMA = "faris-desktop-build/v0.1"
PROGRAMS = ("faris", "faris-app", "avila-core")
TARGET_OS = {"windows", "macos"}
TARGET_ARCH = {"x86_64", "aarch64"}
PROVENANCE = "SOURCE_PROVENANCE.md"
NOTICES = ("faris-THIRD_PARTY_NOTICES.md", "core-RUNTIME_DEPENDENCY_NOTICES.md")
NOTICE_PATHS = {f"licenses/{name}" for name in NOTICES}
PLATFORM_NAMES = {"windows": "Windows", "macos": "macOS"}
REWRITTEN = {INDEX, CHECKSUM, "README.md", PROVENANCE, *NOTICE_PATHS}
HEX64 = re.compile(r"sha256:[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{40}")


def fail(message: str) -> None:
    raise SystemExit(f"retarget_package: {message}")


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            hasher.update(block)
    return "sha256:" + hasher.hexdigest()


def load_linux_index(package: Path) -> dict:
    """The package's index, after the checks that make it a finished Linux v0.5 package."""
    if not package.is_dir() or package.is_symlink():
        fail(f"{package} is not a package directory")
    index_path, checksum_path = package / INDEX, package / CHECKSUM
    if not index_path.is_file() or not checksum_path.is_file():
        fail("package has no package-index.json and package-index.sha256")
    if checksum_path.read_bytes() != f"{digest(index_path)}  {INDEX}\n".encode():
        fail("package-index.sha256 does not match package-index.json")
    index = json.loads(index_path.read_text(encoding="utf-8"))
    if index.get("schema_version") != SCHEMA:
        fail(f"package index is not {SCHEMA}")
    runtime = index.get("local_runtime")
    if not isinstance(runtime, dict) or runtime.get("platform") != {"os": "linux", "arch": "x86_64"}:
        fail("package is not a linux/x86_64 package")
    executables = runtime.get("executables")
    if not isinstance(executables, dict) or set(executables) != set(PROGRAMS):
        fail("package executable manifest is malformed")
    if not isinstance(index.get("evidence_recorded_with"), dict):
        fail("package index has no evidence_recorded_with; rebuild it with the current packager")
    files = index.get("files")
    if not isinstance(files, list) or not files:
        fail("package index has no file inventory")
    indexed = set()
    for item in files:
        relative = item.get("path") if isinstance(item, dict) else None
        if (not isinstance(relative, str) or relative in indexed or relative in {INDEX, CHECKSUM}
                or relative.startswith("/") or any(part in {"", ".", ".."} for part in relative.split("/"))):
            fail(f"package index has an unsafe or repeated path: {relative!r}")
        indexed.add(relative)
        path = package / relative
        if path.is_symlink() or not path.is_file():
            fail(f"indexed file is missing or not a regular file: {relative}")
        if path.stat().st_size != item.get("bytes") or digest(path) != item.get("sha256"):
            fail(f"indexed file failed its size or SHA-256 check: {relative}")
    present = set()
    for path in package.rglob("*"):
        if path.is_symlink():
            fail(f"package contains a symbolic link: {path}")
        if path.is_file():
            present.add(path.relative_to(package).as_posix())
    if present != indexed | {INDEX, CHECKSUM}:
        fail("package files differ from its index: " + ", ".join(sorted(present ^ (indexed | {INDEX, CHECKSUM}))[:5]))
    return index


def load_build(build_dir: Path) -> tuple[dict, str, str]:
    """The validated build record and its (os, arch)."""
    if not build_dir.is_dir() or build_dir.is_symlink():
        fail(f"{build_dir} is not a build directory")
    record_path = build_dir / "build.json"
    if not record_path.is_file():
        fail("build directory has no build.json")
    build = json.loads(record_path.read_text(encoding="utf-8"))
    if not isinstance(build, dict) or build.get("schema_version") != BUILD_SCHEMA:
        fail(f"build.json is not {BUILD_SCHEMA}")
    match = re.fullmatch(r"([a-z0-9_]+)-([a-z0-9_]+)", str(build.get("platform")))
    if not match or match.group(1) not in TARGET_OS or match.group(2) not in TARGET_ARCH:
        fail(f"build platform {build.get('platform')!r} is not <os>-<arch> with os in "
             f"{sorted(TARGET_OS)} and arch in {sorted(TARGET_ARCH)}")
    target_os, target_arch = match.groups()
    for key in ("faris_commit", "core_commit"):
        if not isinstance(build.get(key), str) or not COMMIT.fullmatch(build[key]):
            fail(f"build.json {key} is not a full commit")
    for key in ("faris_version", "core_version"):
        if not isinstance(build.get(key), str) or not build[key]:
            fail(f"build.json has no {key}")
    executables = build.get("executables")
    if not isinstance(executables, dict) or set(executables) != set(PROGRAMS):
        fail(f"build.json executables must be exactly {', '.join(PROGRAMS)}")
    suffix = ".exe" if target_os == "windows" else ""
    for name, record in executables.items():
        if (not isinstance(record, dict) or not isinstance(record.get("sha256"), str)
                or not HEX64.fullmatch(record["sha256"]) or not isinstance(record.get("bytes"), int)
                or isinstance(record["bytes"], bool)):
            fail(f"build.json record for {name} is malformed")
        if record.get("path") != f"bin/{name}{suffix}":
            fail(f"build.json path for {name} must be bin/{name}{suffix} on {target_os}")
        path = build_dir / record["path"]
        if path.is_symlink() or not path.is_file():
            fail(f"build program is missing or not a regular file: {record['path']}")
        if path.stat().st_size != record["bytes"] or digest(path) != record["sha256"]:
            fail(f"build program failed its recorded size or SHA-256: {record['path']}")
    notices = build.get("notices")
    if not isinstance(notices, dict) or set(notices) != set(NOTICES):
        fail(f"build.json notices must be exactly {', '.join(NOTICES)}; "
             "rebuild with the current desktop workflow")
    for name, record in notices.items():
        if (not isinstance(record, dict) or not isinstance(record.get("sha256"), str)
                or not HEX64.fullmatch(record["sha256"]) or not isinstance(record.get("bytes"), int)
                or isinstance(record["bytes"], bool)):
            fail(f"build.json notices record for {name} is malformed")
        if record.get("path") != f"licenses/{name}":
            fail(f"build.json notices path for {name} must be licenses/{name}")
        path = build_dir / record["path"]
        if path.is_symlink() or not path.is_file():
            fail(f"build notices file is missing or not a regular file: {record['path']}")
        if path.stat().st_size != record["bytes"] or digest(path) != record["sha256"]:
            fail(f"build notices file failed its recorded size or SHA-256: {record['path']}")
    return build, target_os, target_arch


def check_provenance(index: dict, build: dict) -> None:
    """The build must be of the same sources, and the same FARIS version, as the package."""
    runtime = index["local_runtime"]
    sources = runtime.get("source_provenance") or {}
    for role, key in (("faris", "faris_commit"), ("core", "core_commit")):
        recorded = (sources.get(role) or {}).get("commit")
        if build[key] != recorded:
            fail(f"build {key} {build[key]} differs from the package's {role} commit {recorded}")
    recorded_version = runtime["executables"]["faris"].get("version")
    if build["faris_version"] != recorded_version:
        fail(f"build reports {build['faris_version']!r}, the Linux package recorded {recorded_version!r} for faris")


def link_or_copy(source: Path, target: Path) -> None:
    try:
        os.link(source, target)
    except OSError:
        shutil.copy2(source, target)


def write_with_mode(path: Path, data: bytes, mode: int) -> None:
    path.write_bytes(data)
    path.chmod(mode)


def copy_note(target_os: str) -> str:
    return "target/release/avila-core.exe" if target_os == "windows" else "target/release/avila-core"


def provenance_text(index: dict, build: dict, target_os: str, target_arch: str) -> str:
    """SOURCE_PROVENANCE.md for a desktop build: the Linux file's structure, this platform's facts."""
    sources = index["local_runtime"]["source_provenance"]
    faris_url, core_url = sources["faris"].get("repository"), sources["core"].get("repository")
    if not faris_url or not core_url:
        fail("package index records no repository URL for FARIS or Core")
    platform = f"{PLATFORM_NAMES[target_os]} {target_arch}"
    return (
        "# Local runtime provenance\n\n"
        f"- Platform: {platform}.\n"
        f"- FARIS repository: `{faris_url}` at `{build['faris_commit']}`.\n"
        f"- Avila Core repository: `{core_url}` at `{build['core_commit']}`.\n"
        f"- Packaged FARIS CLI reports: `{build['faris_version']}`.\n"
        f"- Core executable reports: `{build['core_version']}`.\n"
        f"- The programs were built by the desktop workflow on the build's runner "
        f"(`{build.get('runner')}`), release profile.\n"
        "- Rebuild FARIS with `cargo build --release --locked -p faris-cli -p faris-app`.\n"
        "- Rebuild Core at the recorded commit with `cargo build --release --locked --bin avila-core`, "
        f"then copy `{copy_note(target_os)}` to the distribution.\n"
        "- The recorded study evidence was produced on Linux x86_64 by the programs named under "
        "`evidence_recorded_with` in `package-index.json`; it is the same evidence in every platform's download.\n"
        f"- The licence notices in `licenses/` are collected from this platform's Cargo dependency graph "
        f"({platform}).\n"
        "- These commands document the build route; no bit-for-bit reproducibility claim is made.\n"
        "- Binary SHA-256 values in the package index identify bytes only; they are unsigned.\n")


def rebuild_lines(target_os: str) -> list[str]:
    return [
        "FARIS: check out the recorded commit, then run cargo build --release --locked -p faris-cli -p faris-app.",
        "Core: check out the recorded commit, run cargo build --release --locked --bin avila-core, "
        f"then copy {copy_note(target_os)} to the distribution.",
        "These instructions identify the source and toolchain command; they do not claim bit-for-bit reproducibility.",
    ]


def retargeted_index(index: dict, build: dict, target_os: str, target_arch: str, provenance: bytes) -> dict:
    linux = index["local_runtime"]
    executables = {name: dict(build["executables"][name]) for name in PROGRAMS}
    runtime = dict(linux)
    runtime["platform"] = {"os": target_os, "arch": target_arch}
    runtime["executables"] = executables
    sources = dict(linux["source_provenance"])
    sources["core"] = dict(sources["core"], binary_profile="release")
    sources["rebuild"] = rebuild_lines(target_os)
    runtime["source_provenance"] = sources
    def app_record(record: dict) -> dict:
        return {"path": record["path"], "bytes": record["bytes"], "sha256": record["sha256"], "part": "app"}

    # Keyed by the Linux package's path: the programs gain .exe on Windows.
    records = {f"bin/{name}": app_record(record) for name, record in executables.items()}
    records.update({record["path"]: app_record(record) for record in build["notices"].values()})
    records[PROVENANCE] = {"path": PROVENANCE, "bytes": len(provenance),
                           "sha256": "sha256:" + hashlib.sha256(provenance).hexdigest(), "part": "app"}
    indexed = {item["path"] for item in index["files"]}
    missing = (NOTICE_PATHS | {PROVENANCE}) - indexed
    if missing:
        fail("package lacks files to replace: " + ", ".join(sorted(missing)))
    inventory = []
    for item in index["files"]:
        replacement = records.get(item["path"])
        if item["path"].startswith("bin/") and replacement is None:
            fail(f"package has a program the build lacks: {item['path']}")
        inventory.append(replacement or dict(item))
    app = [item for item in inventory if item["part"] == "app"]
    out = {}
    for key, value in index.items():
        if key == "faris_cli_sha256":
            value = executables["faris"]["sha256"]
        elif key == "faris_app_sha256":
            value = executables["faris-app"]["sha256"]
        elif key == "core_executable_sha256":
            value = executables["avila-core"]["sha256"]
        elif key == "evidence_recorded_with":
            value = {"platform": {"os": "linux", "arch": "x86_64"},
                     "faris_cli_sha256": index["faris_cli_sha256"],
                     "core_executable_sha256": index["core_executable_sha256"]}
        elif key == "local_runtime":
            value = runtime
        elif key == "files":
            value = inventory
        elif key == "package_file_count":
            value = len(inventory)
        elif key == "package_bytes":
            value = sum(item["bytes"] for item in inventory)
        elif key == "parts":
            value = dict(value)
            value["app"] = {"file_count": len(app), "bytes": sum(item["bytes"] for item in app)}
        out[key] = value
        if key == "local_runtime":
            out["desktop_build"] = build
    return out


def retarget(package: Path, build_dir: Path, output: Path) -> dict:
    package, build_dir = package.resolve(), build_dir.resolve()
    index = load_linux_index(package)
    build, target_os, target_arch = load_build(build_dir)
    check_provenance(index, build)
    if os.path.lexists(output):
        fail(f"{output} exists; choose a new directory")
    provenance = provenance_text(index, build, target_os, target_arch).encode("utf-8")
    new_index = retargeted_index(index, build, target_os, target_arch, provenance)

    directories = sorted((p for p in package.rglob("*") if p.is_dir()), key=lambda p: len(p.parts))
    try:
        output.mkdir(parents=True)
        for directory in directories:
            (output / directory.relative_to(package)).mkdir(mode=0o755)
        for item in index["files"]:
            relative = item["path"]
            if relative.startswith("bin/") or relative in REWRITTEN:
                continue
            link_or_copy(package / relative, output / relative)
        readme = package / "README.md"
        if readme.is_file():
            shutil.copyfile(readme, output / "README.md")
            (output / "README.md").chmod(stat.S_IMODE(readme.stat().st_mode))
        for relative, data in [(PROVENANCE, provenance),
                               *((record["path"], (build_dir / record["path"]).read_bytes())
                                 for record in build["notices"].values())]:
            write_with_mode(output / relative, data, stat.S_IMODE((package / relative).stat().st_mode))
        (output / "bin").mkdir(exist_ok=True)
        for record in new_index["local_runtime"]["executables"].values():
            shutil.copyfile(build_dir / record["path"], output / record["path"])
            (output / record["path"]).chmod(0o755)
        index_bytes = (json.dumps(new_index, indent=2) + "\n").encode("utf-8")
        mode = stat.S_IMODE((package / INDEX).stat().st_mode)
        write_with_mode(output / INDEX, index_bytes, mode)
        write_with_mode(output / CHECKSUM,
                        f"sha256:{hashlib.sha256(index_bytes).hexdigest()}  {INDEX}\n".encode("ascii"),
                        stat.S_IMODE((package / CHECKSUM).stat().st_mode))
        for directory in sorted(directories, key=lambda p: len(p.parts), reverse=True):
            (output / directory.relative_to(package)).chmod(stat.S_IMODE(directory.stat().st_mode))
    except BaseException:
        for path in [output, *output.rglob("*")]:
            if path.is_dir() and not path.is_symlink():
                path.chmod(0o755)
        shutil.rmtree(output, ignore_errors=True)
        raise
    return new_index


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--package", type=Path, required=True, help="finished, verified Linux v0.5 package")
    parser.add_argument("--build", type=Path, required=True, help="desktop build folder (bin/ and build.json)")
    parser.add_argument("--output", type=Path, required=True, help="new package directory")
    args = parser.parse_args()
    index = retarget(args.package, args.build, args.output)
    platform = index["local_runtime"]["platform"]
    print(f"{args.output}: {platform['os']}-{platform['arch']} package, "
          f"{index['package_file_count']} files, {index['package_bytes']} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
