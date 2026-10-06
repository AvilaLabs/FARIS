#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Turn a verified recorded-demo package into the release downloads.

Checks, before writing anything:
- the workspace version in Cargo.toml equals --version;
- CHANGELOG.md has a dated section for that version (not "unreleased");
- the package's bundled `faris` and `faris-app` report that version;
- the package index is v0.5, names FARIS-<version>-evidence.tar.gz, and covers every
  file in the package (scripts/verify_binary_manifest.py from the package also runs).

Then writes, into a new --output-dir:
- FARIS-<version>-<os>-<arch>.tar.gz: the app part plus package-index.json and
  package-index.sha256, under the top folder FARIS-<version>/;
- FARIS-<version>-evidence.tar.gz: the evidence part under the same top folder, so
  unpacking both into one place rebuilds the package;
- SHA256SUMS for both archives;
- RELEASE_NOTES.md: the changelog section and the downloads, for the GitHub release body.

Both archives are deterministic: entries sorted, owner and group zeroed, mtimes fixed
to SOURCE_DATE_EPOCH or the commit time of HEAD, gzip header mtime 0, modes from the
package. Nothing is uploaded or tagged; that stays a separate, confirmed step.
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
import tarfile
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


def downloads_section(version: str, app_name: str, evidence_name: str) -> str:
    return (
        "\n## Downloads\n\n"
        f"- `{app_name}`: the app. Unpack it and run `FARIS-{version}/bin/faris-app`; it opens the whole study.\n"
        f"- `{evidence_name}`: the Core receipts the app's Evidence step shows, and the files `verify.sh` checks "
        "(the same bytes for every platform). Optional: unpack it into the same place as the app archive, "
        f"so both fill `FARIS-{version}/`. The app needs temporary space for the receipts while it runs; "
        "the package README gives the amount.\n"
        "- `SHA256SUMS`: check both downloads with `sha256sum -c SHA256SUMS` before unpacking.\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--package", type=Path, required=True, help="verified recorded-demo package directory")
    parser.add_argument("--version", required=True)
    parser.add_argument("--output-dir", type=Path, required=True, help="new directory for the release files")
    args = parser.parse_args()

    package = args.package.resolve()
    if workspace_version() != args.version:
        fail(f"Cargo.toml workspace version is {workspace_version()}, not {args.version}")
    notes, _date = changelog_section(args.version)
    for name in ("faris", "faris-app"):
        found = reported_version(package / "bin" / name)
        if found != args.version:
            fail(f"package bin/{name} reports {found}, not {args.version}; rebuild the package")
    subprocess.run([sys.executable, str(package / "scripts" / "verify_binary_manifest.py"), str(package)],
                   check=True, timeout=600)
    app_files, evidence_files, platform_name = split_package(package, args.version)

    if args.output_dir.exists():
        fail(f"{args.output_dir} exists; choose a new directory")
    args.output_dir.mkdir(parents=True)
    top = f"FARIS-{args.version}"
    mtime = release_mtime()
    sums = []
    for name, files in ((f"{top}-{platform_name}.tar.gz", app_files),
                        (f"{top}-evidence.tar.gz", evidence_files)):
        archive = args.output_dir / name
        count = write_archive(package, archive, top, mtime, files)
        digest = sha256(archive)
        sums.append((name, digest))
        print(f"{name}: {count} entries, {archive.stat().st_size / 1e6:.1f} MB, sha256 {digest}")
    (args.output_dir / "SHA256SUMS").write_text(
        "".join(f"{digest}  {name}\n" for name, digest in sums), encoding="utf-8")
    (args.output_dir / "RELEASE_NOTES.md").write_text(
        notes + downloads_section(args.version, sums[0][0], sums[1][0])
        + "".join(f"\nSHA-256 of `{name}`: `{digest}`\n" for name, digest in sums), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
