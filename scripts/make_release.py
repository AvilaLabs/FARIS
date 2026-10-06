#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Turn a verified recorded-demo package into the release downloads.

Checks, before writing anything:
- the workspace version in Cargo.toml equals --version;
- CHANGELOG.md has a dated section for that version (not "unreleased");
- the package's bundled `faris` and `faris-app` report that version;
- the package index is v0.5, names FARIS-<version>-evidence.tar.gz, and covers every
  file in the package (scripts/verify_binary_manifest.py from the package also runs).

A package made by scripts/retarget_package.py has CI-built programs: its version comes
from `desktop_build.faris_version` in the index, and the programs' hashes and sizes are
checked against the index. Windows and macOS programs cannot run here, so nothing is
executed for them; the CI-built Linux programs also report their version, which must match.

Then writes, into a new --output-dir:
- FARIS-<version>-<os>-<arch>.tar.gz (FARIS-<version>-windows-<arch>.zip on Windows): the
  app part plus package-index.json and package-index.sha256, under the top folder
  FARIS-<version>/;
- FARIS-<version>-evidence.tar.gz: the evidence part under the same top folder, so
  unpacking both into one place rebuilds the package;
- SHA256SUMS for both archives;
- RELEASE_NOTES.md: the changelog section and the downloads, for the GitHub release body.

The archives are deterministic: entries sorted, owner and group zeroed, mtimes fixed
to SOURCE_DATE_EPOCH or the commit time of HEAD, gzip header mtime 0, modes from the
package. The evidence archive of a retargeted package is byte-identical to the Linux
one. With --app-only, --output-dir must already hold the SHA256SUMS of a first run:
only the app archive is written, its line is appended to SHA256SUMS (a duplicate is
refused) and RELEASE_NOTES.md is rewritten for every platform archive present. A
release is therefore the Linux run, then one --app-only run per other platform.
Nothing is uploaded or tagged; that stays a separate, confirmed step.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import subprocess
import sys
import stat
import tarfile
import time
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent
SECTION = re.compile(r"^## (\d+\.\d+\.\d+) — (.+)$")


def fail(message: str) -> None:
    raise SystemExit(f"make_release: {message}")


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^\[workspace\.package\][^\[]*?^version = "([^"]+)"', text, re.M | re.S)
    if not match:
        fail("no [workspace.package] version in Cargo.toml")
    return match.group(1)


def changelog_section(version: str) -> tuple[str, str]:
    """The section body and its date for one version."""
    lines = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8").splitlines()
    start = date = None
    for index, line in enumerate(lines):
        match = SECTION.match(line)
        if match and match.group(1) == version:
            start, date = index, match.group(2).strip()
            break
    if start is None:
        fail(f"CHANGELOG.md has no section for {version}")
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", date):
        fail(f"CHANGELOG.md section {version} is dated {date!r}; set the release date (YYYY-MM-DD)")
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    return "\n".join(lines[start + 1:end]).strip() + "\n", date


def reported_version(binary: Path) -> str:
    out = subprocess.run([str(binary), "--version"], capture_output=True, text=True, check=True, timeout=30)
    return out.stdout.strip().split()[-1]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def release_mtime() -> int:
    """SOURCE_DATE_EPOCH when set, else the commit time of HEAD."""
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    if epoch is not None:
        if not epoch.isdigit():
            fail("SOURCE_DATE_EPOCH must be whole seconds")
        return int(epoch)
    out = subprocess.run(["git", "-C", str(ROOT), "log", "-1", "--format=%ct", "HEAD"],
                         capture_output=True, text=True, check=False)
    if out.returncode != 0 or not out.stdout.strip().isdigit():
        fail("cannot read the release commit time; set SOURCE_DATE_EPOCH")
    return int(out.stdout.strip())


def split_package(package: Path, version: str) -> tuple[list[str], list[str], str]:
    """The package-relative files of the app archive and of the evidence archive, and the
    `<os>-<arch>` name. Refuses a package whose files differ from its index."""
    index = json.loads((package / "package-index.json").read_text(encoding="utf-8"))
    if index.get("schema_version") != "faris-recorded-demo-package/v0.5":
        fail("package index is not faris-recorded-demo-package/v0.5")
    evidence_name = f"FARIS-{version}-evidence.tar.gz"
    if index.get("parts", {}).get("evidence", {}).get("archive_name") != evidence_name:
        fail(f"package index does not name {evidence_name}; rebuild the package with --version {version}")
    platform = index.get("local_runtime", {}).get("platform", {})
    if not all(isinstance(platform.get(key), str) and re.fullmatch(r"[a-z0-9_]+", platform[key])
               for key in ("os", "arch")):
        fail("package index has no valid platform")
    app = ["package-index.json", "package-index.sha256"]
    evidence = []
    for item in index["files"]:
        {"app": app, "evidence": evidence}[item["part"]].append(item["path"])
    indexed = set(app) | set(evidence)
    present = {path.relative_to(package).as_posix() for path in package.rglob("*") if not path.is_dir()}
    if present != indexed:
        fail("package files differ from its index: " + ", ".join(sorted(present ^ indexed)[:5]))
    if not evidence or len(app) < 3:
        fail("package has an empty app or evidence part")
    return sorted(app), sorted(evidence), f"{platform['os']}-{platform['arch']}"


def write_archive(package: Path, archive: Path, top: str, mtime: int, files: list[str]) -> int:
    """Write `files` (package-relative) and the directories above them under `top/`."""
    directories = {PurePosixPath(".")}
    for relative in files:
        directories.update(PurePosixPath(relative).parents)
    entries = sorted([d.as_posix() for d in directories] + list(files))

    def normalise(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = mtime
        return info

    # gzip header mtime is 0, so the archive bytes depend only on content.
    with open(archive, "xb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", compresslevel=9, mtime=0) as gz, \
            tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for relative in entries:
            path = package if relative == "." else package / relative
            if path.is_symlink():
                fail(f"package contains a symbolic link: {path}")
            arcname = top if relative == "." else f"{top}/{relative}"
            tar.add(path, arcname=arcname, recursive=False, filter=normalise)
    return len(entries)


def check_retargeted_binaries(package: Path, index: dict, version: str, platform_name: str) -> None:
    """The non-Linux counterpart of running the programs: the version the desktop build
    recorded, and the programs' hashes and sizes against the index. Nothing is executed."""
    build = index.get("desktop_build")
    if not isinstance(build, dict) or build.get("schema_version") != "faris-desktop-build/v0.1":
        fail("package index has no desktop_build record")
    if build.get("platform") != platform_name:
        fail(f"desktop_build platform {build.get('platform')!r} differs from the package platform {platform_name}")
    if str(build.get("faris_version", "")).split()[-1:] != [version]:
        fail(f"desktop build reports {build.get('faris_version')!r}, not {version}; rebuild the package")
    executables = index["local_runtime"].get("executables")
    if not isinstance(executables, dict) or executables != build.get("executables"):
        fail("package executables differ from its desktop_build record")
    pins = {"faris": index.get("faris_cli_sha256"), "faris-app": index.get("faris_app_sha256"),
            "avila-core": index.get("core_executable_sha256")}
    suffix = ".exe" if platform_name.startswith("windows-") else ""
    for name, record in executables.items():
        if record.get("path") != f"bin/{name}{suffix}":
            fail(f"unexpected program path for {name}: {record.get('path')!r}")
        path = package / record["path"]
        if (path.is_symlink() or not path.is_file() or "sha256:" + sha256(path) != record.get("sha256")
                or record["sha256"] != pins.get(name) or path.stat().st_size != record.get("bytes")):
            fail(f"program {record['path']} differs from the package index")


def write_zip_archive(package: Path, archive: Path, top: str, mtime: int, files: list[str]) -> int:
    """The zip counterpart of write_archive: sorted entries, one fixed timestamp, deflate,
    Unix modes in the external attributes (so bin/ programs keep 0755)."""
    stamp = time.gmtime(mtime)[:6]
    if stamp[0] < 1980:
        fail("zip archives cannot carry a timestamp before 1980; set SOURCE_DATE_EPOCH")
    directories = {PurePosixPath(".")}
    for relative in files:
        directories.update(PurePosixPath(relative).parents)
    entries = sorted([d.as_posix() for d in directories] + list(files))
    with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for relative in entries:
            path = package if relative == "." else package / relative
            if path.is_symlink():
                fail(f"package contains a symbolic link: {path}")
            name = top if relative == "." else f"{top}/{relative}"
            mode = stat.S_IMODE(path.stat().st_mode)
            if path.is_dir():
                info = zipfile.ZipInfo(name + "/", stamp)
                info.create_system = 3
                info.external_attr = ((stat.S_IFDIR | mode) << 16) | 0x10
                zf.writestr(info, b"", zipfile.ZIP_STORED)
            else:
                info = zipfile.ZipInfo(name, stamp)
                info.create_system = 3
                info.external_attr = (stat.S_IFREG | mode) << 16
                zf.writestr(info, path.read_bytes(), zipfile.ZIP_DEFLATED, 9)
    return len(entries)


def read_sums(path: Path) -> list[tuple[str, str]]:
    sums = []
    for line in path.read_text(encoding="utf-8").splitlines():
        digest, _, name = line.partition("  ")
        if not re.fullmatch(r"[0-9a-f]{64}", digest) or not name:
            fail(f"malformed line in {path.name}: {line!r}")
        sums.append((name, digest))
    return sums


PLATFORM_NAMES = {
    "linux-x86_64": "Linux x86_64",
    "windows-x86_64": "Windows x86_64",
    "macos-aarch64": "macOS on Apple silicon",
    "macos-x86_64": "macOS on an Intel Mac",
}


def downloads_section(version: str, app_names: list[str], evidence_name: str | None) -> str:
    lines = ["\n## Downloads\n\n"]
    for app_name in app_names:
        platform_name = re.fullmatch(rf"FARIS-{re.escape(version)}-(.+?)(\.tar\.gz|\.zip)", app_name).group(1)
        program = (rf"FARIS-{version}\bin\faris-app.exe" if platform_name.startswith("windows-")
                   else f"FARIS-{version}/bin/faris-app")
        lines.append(f"- `{app_name}`: the app for {PLATFORM_NAMES.get(platform_name, platform_name)}. Unpack it "
                     f"and run `{program}`; it opens the whole study.\n")
    if evidence_name:
        lines.append(
            f"- `{evidence_name}`: the Core receipts the app's Evidence step shows, and the files `verify.sh` checks "
            "(the same bytes for every platform). Optional: unpack it into the same place as the app archive, "
            f"so both fill `FARIS-{version}/`. The app needs temporary space for the receipts while it runs; "
            "the package README gives the amount.\n")
    lines.append("- `SHA256SUMS`: check the downloads before unpacking. Linux: `sha256sum -c SHA256SUMS "
                 "--ignore-missing`. macOS: `shasum -a 256 -c SHA256SUMS --ignore-missing`. Windows: in PowerShell, "
                 f"`Get-FileHash FARIS-{version}-windows-x86_64.zip` must print the SHA-256 on that file's line.\n")
    return "".join(lines)


def write_release_notes(output_dir: Path, version: str, notes: str) -> None:
    """RELEASE_NOTES.md for every platform archive present in the output directory."""
    sums = read_sums(output_dir / "SHA256SUMS")
    evidence_name = f"FARIS-{version}-evidence.tar.gz"
    app_names = sorted(name for name, _ in sums if name != evidence_name and (output_dir / name).is_file()
                       and re.fullmatch(rf"FARIS-{re.escape(version)}-.+(\.tar\.gz|\.zip)", name))
    has_evidence = evidence_name in dict(sums) and (output_dir / evidence_name).is_file()
    (output_dir / "RELEASE_NOTES.md").write_text(
        notes + downloads_section(version, app_names, evidence_name if has_evidence else None)
        + "".join(f"\nSHA-256 of `{name}`: `{digest}`\n" for name, digest in sums), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--package", type=Path, required=True, help="verified recorded-demo package directory")
    parser.add_argument("--version", required=True)
    parser.add_argument("--output-dir", type=Path, required=True,
                        help="new directory for the release files (existing, with SHA256SUMS, for --app-only)")
    parser.add_argument("--app-only", action="store_true",
                        help="write only the app archive and add its line to the existing SHA256SUMS")
    args = parser.parse_args()

    package = args.package.resolve()
    if workspace_version() != args.version:
        fail(f"Cargo.toml workspace version is {workspace_version()}, not {args.version}")
    notes, _date = changelog_section(args.version)
    app_files, evidence_files, platform_name = split_package(package, args.version)
    index = json.loads((package / "package-index.json").read_text(encoding="utf-8"))
    if "desktop_build" in index:
        check_retargeted_binaries(package, index, args.version, platform_name)
        if platform_name.split("-")[0] == "linux":
            # The CI-built Linux programs run here too; the version they report must match.
            for name in ("faris", "faris-app"):
                found = reported_version(package / "bin" / name)
                if found != args.version:
                    fail(f"package bin/{name} reports {found}, not {args.version}; rebuild the package")
    else:
        if platform_name.split("-")[0] != "linux":
            fail("package index has no desktop_build record")
        for name in ("faris", "faris-app"):
            found = reported_version(package / "bin" / name)
            if found != args.version:
                fail(f"package bin/{name} reports {found}, not {args.version}; rebuild the package")
        subprocess.run([sys.executable, str(package / "scripts" / "verify_binary_manifest.py"), str(package)],
                       check=True, timeout=600)

    top = f"FARIS-{args.version}"
    app_name = (f"{top}-{platform_name}.zip" if platform_name.startswith("windows-")
                else f"{top}-{platform_name}.tar.gz")
    sums_path = args.output_dir / "SHA256SUMS"
    if args.app_only:
        if not sums_path.is_file():
            fail(f"{sums_path} does not exist; run the Linux release first, then --app-only")
        if app_name in dict(read_sums(sums_path)) or (args.output_dir / app_name).exists():
            fail(f"{app_name} is already in {args.output_dir}")
        jobs = [(app_name, app_files)]
    else:
        if args.output_dir.exists():
            fail(f"{args.output_dir} exists; choose a new directory")
        args.output_dir.mkdir(parents=True)
        jobs = [(app_name, app_files), (f"{top}-evidence.tar.gz", evidence_files)]
    mtime = release_mtime()
    sums = []
    for name, files in jobs:
        archive = args.output_dir / name
        count = (write_zip_archive if name.endswith(".zip") else write_archive)(package, archive, top, mtime, files)
        digest = sha256(archive)
        sums.append((name, digest))
        print(f"{name}: {count} entries, {archive.stat().st_size / 1e6:.1f} MB, sha256 {digest}")
    with sums_path.open("a" if args.app_only else "w", encoding="utf-8") as stream:
        stream.write("".join(f"{digest}  {name}\n" for name, digest in sums))
    write_release_notes(args.output_dir, args.version, notes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
