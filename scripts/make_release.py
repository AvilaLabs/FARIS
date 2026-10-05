#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Turn a verified recorded-demo package into the release download.

Checks, before writing anything:
- the workspace version in Cargo.toml equals --version;
- CHANGELOG.md has a dated section for that version (not "unreleased");
- the package's bundled `faris` and `faris-app` report that version;
- the package index verifies (scripts/verify_binary_manifest.py from the package).

Then writes, into a new --output-dir:
- FARIS-<version>-linux-x86_64.tar.gz: the package under a top folder of the same
  name, entries sorted, owner/group zeroed, mtimes fixed to the changelog date,
  so the same package gives the same bytes;
- SHA256SUMS for that archive;
- RELEASE_NOTES.md: the changelog section, for the GitHub release body.

Nothing is uploaded or tagged; that stays a separate, confirmed step.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import re
import subprocess
import sys
import tarfile
from datetime import datetime, timezone
from pathlib import Path

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


def write_archive(package: Path, archive: Path, top: str, mtime: int) -> int:
    paths = sorted(p for p in package.rglob("*"))
    count = 0

    def normalise(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = mtime
        return info

    # gzip header mtime fixed too, so the archive bytes depend only on content.
    with open(archive, "xb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", compresslevel=9, mtime=mtime) as gz, \
            tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
        tar.add(package, arcname=top, recursive=False, filter=normalise)
        for path in paths:
            if path.is_symlink():
                fail(f"package contains a symbolic link: {path}")
            tar.add(path, arcname=f"{top}/{path.relative_to(package).as_posix()}", recursive=False,
                    filter=normalise)
            count += 1
    return count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--package", type=Path, required=True, help="verified recorded-demo package directory")
    parser.add_argument("--version", required=True)
    parser.add_argument("--output-dir", type=Path, required=True, help="new directory for the release files")
    args = parser.parse_args()

    package = args.package.resolve()
    if workspace_version() != args.version:
        fail(f"Cargo.toml workspace version is {workspace_version()}, not {args.version}")
    notes, date = changelog_section(args.version)
    for name in ("faris", "faris-app"):
        found = reported_version(package / "bin" / name)
        if found != args.version:
            fail(f"package bin/{name} reports {found}, not {args.version}; rebuild the package")
    subprocess.run([sys.executable, str(package / "scripts" / "verify_binary_manifest.py"), str(package)],
                   check=True, timeout=600)

    if args.output_dir.exists():
        fail(f"{args.output_dir} exists; choose a new directory")
    args.output_dir.mkdir(parents=True)
    top = f"FARIS-{args.version}-linux-x86_64"
    archive = args.output_dir / f"{top}.tar.gz"
    mtime = int(datetime.strptime(date, "%Y-%m-%d").replace(tzinfo=timezone.utc).timestamp())
    count = write_archive(package, archive, top, mtime)
    digest = sha256(archive)
    (args.output_dir / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    (args.output_dir / "RELEASE_NOTES.md").write_text(
        notes + f"\nSHA-256 of `{archive.name}`: `{digest}`\n", encoding="utf-8")
    print(f"{archive.name}: {count} entries, {archive.stat().st_size / 1e6:.1f} MB, sha256 {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
