#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Extract a runtime and check its hashes, CLI version and C API without transport."""
from __future__ import annotations
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
from build_heating_runtime import (
    BASE_COMMIT as EXPECTED_COMMIT, PATCH_SHA256, VENDOR_LICENSES, python_inventory,
)

ROOT_NAME = "faris-openmc-runtime"
FILES_SCHEMA = "faris.runtime-files/1.0.0"
MANIFEST_SCHEMA = "faris.openmc-compensated-runtime/v1"


def sha(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def canonical_sha(value) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(payload).hexdigest()


def inside(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=True))
        return True
    except ValueError:
        return False


def extract_safe(archive: Path, destination: Path) -> Path:
    if destination.exists():
        raise ValueError(f"destination already exists: {destination}")
    destination.mkdir(parents=True)
    root = destination / ROOT_NAME
    root.mkdir()
    with tarfile.open(archive, "r:*") as tf:
        members = tf.getmembers()
        symlinks = {PurePosixPath(m.name) for m in members if m.issym()}
        seen = set()
        for m in members:
            p = PurePosixPath(m.name)
            if p.is_absolute() or ".." in p.parts or not p.parts or p.parts[0] != ROOT_NAME:
                raise ValueError(f"unsafe archive member path: {m.name}")
            if m.name in seen:
                raise ValueError(f"duplicate archive member: {m.name}")
            if any(parent in symlinks for parent in p.parents):
                raise ValueError(f"archive member is nested under a symlink: {m.name}")
            seen.add(m.name)
            target = destination.joinpath(*p.parts)
            if m.isdev() or m.isfifo() or m.islnk():
                raise ValueError(f"unsupported archive member type: {m.name}")
            if m.issym():
                link = PurePosixPath(m.linkname)
                if link.is_absolute():
                    raise ValueError(f"absolute symlink: {m.name} -> {m.linkname}")
                resolved = (target.parent / m.linkname).resolve(strict=False)
                if not inside(resolved, root):
                    raise ValueError(f"escaping symlink: {m.name} -> {m.linkname}")
            elif not (m.isfile() or m.isdir()):
                raise ValueError(f"unsupported archive member: {m.name}")
        # Directories and regular files first; create symlinks only after parents exist.
        for m in members:
            if m.isdir():
                (destination / m.name).mkdir(parents=True, exist_ok=True)
            elif m.isfile():
                out = destination / m.name
                out.parent.mkdir(parents=True, exist_ok=True)
                src = tf.extractfile(m)
                if src is None:
                    raise ValueError(f"cannot read archive member: {m.name}")
                with src, out.open("xb") as f:
                    while block := src.read(1024 * 1024):
                        f.write(block)
                out.chmod(m.mode & 0o777)
        for m in members:
            if m.issym():
                out = destination / m.name
                out.parent.mkdir(parents=True, exist_ok=True)
                out.symlink_to(m.linkname)
    if not root.is_dir():
        raise ValueError(f"archive is missing {ROOT_NAME}/")
    return root


def verify_files(root: Path) -> dict:
    manifest_path = root / "runtime-files.sha256.json"
    data = json.loads(manifest_path.read_text())
    if data.get("schema") != FILES_SCHEMA:
        raise ValueError(f"unsupported files manifest schema: {data.get('schema')}")
    rows = data.get("files")
    if not isinstance(rows, list) or data.get("sha256") != canonical_sha(rows):
        raise ValueError("runtime-files manifest canonical digest mismatch")
    expected = set()
    for row in rows:
        rel = PurePosixPath(row["path"])
        if rel.is_absolute() or ".." in rel.parts:
            raise ValueError(f"unsafe manifest path: {rel}")
        if rel.as_posix() in expected:
            raise ValueError(f"duplicate manifest path: {rel}")
        expected.add(rel.as_posix())
        p = root.joinpath(*rel.parts)
        if row["kind"] == "file":
            if not p.is_file() or p.is_symlink() or sha(p) != row.get("sha256"):
                raise ValueError(f"file hash/type mismatch: {rel}")
        elif row["kind"] == "symlink":
            if not p.is_symlink() or os.readlink(p) != row.get("target"):
                raise ValueError(f"symlink mismatch: {rel}")
            if not inside(p.resolve(strict=True), root):
                raise ValueError(f"symlink escapes package: {rel}")
        else:
            raise ValueError(f"unsupported file kind for {rel}: {row['kind']}")
    for name in ("LICENSE-OpenMC-MIT", "faris-openmc-0.15.3-compensated-heating.patch"):
        if name not in expected:
            raise ValueError(f"runtime file manifest does not include {name}")
    actual = {p.relative_to(root).as_posix() for p in root.rglob("*")
              if (p.is_file() or p.is_symlink())
              and p.relative_to(root).as_posix() != "runtime-files.sha256.json"}
    if actual != expected:
        raise ValueError(f"package file inventory differs from manifest: missing={sorted(expected-actual)}, "
                         f"unlisted={sorted(actual-expected)}")
    return data


def run_clean(argv: list[str], cwd: Path | None = None) -> str:
    env = {"PATH": "/usr/bin:/bin", "HOME": "/tmp", "PYTHONDONTWRITEBYTECODE": "1",
           "OMP_NUM_THREADS": "1", "OPENBLAS_NUM_THREADS": "1"}
    return subprocess.run(argv, env=env, cwd=cwd, check=True, capture_output=True,
                          text=True, timeout=30).stdout


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("archive", type=Path)
    ap.add_argument("destination", type=Path, help="new extraction parent path; must not exist")
    ap.add_argument("--reactor-transport", type=Path, required=True,
                    help="path to integrations/openmc/reactor_transport.py")
    ap.add_argument("--output", type=Path)
    a = ap.parse_args()
    root = extract_safe(a.archive.resolve(strict=True), a.destination.resolve())
    files = verify_files(root)
    manifest = json.loads((root / "runtime-manifest.json").read_text())
    if manifest.get("schema") != MANIFEST_SCHEMA:
        raise ValueError("unsupported runtime manifest schema")
    if manifest.get("source_commit") != EXPECTED_COMMIT:
        raise ValueError("source commit differs from requested pinned upstream")
    patch = root / "faris-openmc-0.15.3-compensated-heating.patch"
    if sha(patch) != manifest.get("patch_sha256") or manifest.get("patch_sha256") != PATCH_SHA256:
        raise ValueError("patch hash mismatch")
    licenses = manifest.get("license_files", {})
    required_licenses = {"LICENSE-OpenMC-MIT", "LICENSE-FARIS-AGPL-3.0"}
    required_licenses.update(f"share/licenses/{name}/{filename}"
                             for name, filename in VENDOR_LICENSES.items())
    if set(licenses) != required_licenses:
        raise ValueError("runtime license manifest is incomplete")
    for name, digest in licenses.items():
        if sha(root / name) != digest:
            raise ValueError(f"runtime license hash mismatch: {name}")
    for dep, digest in manifest.get("external_runtime_libraries", {}).items():
        p = Path(dep)
        if not p.is_file() or sha(p) != digest:
            raise ValueError(f"external runtime dependency changed or missing: {dep}")
    interpreter = manifest.get("python_base", {})
    base_python = Path(interpreter.get("path", ""))
    if not base_python.is_file() or sha(base_python) != interpreter.get("sha256"):
        raise ValueError("external base Python interpreter changed or is missing")

    exe = root / "bin/openmc"
    prefix = root
    py = prefix / "bin/python"
    lib = root / "lib/libopenmc.so"
    cli_version = run_clean([str(exe), "--version"])
    if "OpenMC version 0.15.3" not in cli_version:
        raise ValueError("relocated CLI reports an unexpected version")
    ldd_cli = run_clean(["/usr/bin/ldd", "-r", str(exe)])
    ldd_lib = run_clean(["/usr/bin/ldd", "-r", str(lib)])
    if "not found" in ldd_cli or "undefined symbol" in ldd_cli or "not found" in ldd_lib or "undefined symbol" in ldd_lib:
        raise ValueError("unresolved dependency in relocated runtime")
    dynamic = run_clean(["/usr/bin/readelf", "-d", str(exe)])
    if "Library rpath: [$ORIGIN/../lib]" not in dynamic or "Library runpath:" in dynamic:
        raise ValueError("CLI does not have the required relative legacy RPATH")

    # Load the existing Python resolver directly, then use its exact selected-library check.
    spec = importlib.util.spec_from_file_location("faris_reactor_transport", a.reactor_transport.resolve(strict=True))
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    selected, selected_hash = module.selected_openmc_library(exe, py, {})
    expected = lib.resolve(strict=True)
    if selected.resolve(strict=True) != expected:
        raise ValueError("reactor_transport selected a different library")
    probe = """import hashlib,json,pathlib,sys,openmc,openmc.lib
p=pathlib.Path(sys.argv[1]).resolve(strict=True)
l=pathlib.Path(openmc.lib._dll._name).resolve(strict=True)
assert pathlib.Path(sys.executable).resolve().parent.parent==p
assert pathlib.Path(openmc.__file__).resolve().is_relative_to(p)
assert l==(p/'lib/libopenmc.so').resolve(strict=True)
print(json.dumps({'python':str(pathlib.Path(sys.executable).resolve()),'openmc':str(pathlib.Path(openmc.__file__).resolve()),'library':str(l),'library_sha256':hashlib.sha256(l.read_bytes()).hexdigest(),'version':openmc.__version__}))"""
    py_result = json.loads(run_clean([str(py), "-c", probe, str(prefix)]))
    if py_result["version"] != "0.15.3" or py_result["library_sha256"] != selected_hash:
        raise ValueError("imported Python API and selected CLI C API library differ")
    inventory_env = {"PATH": "/usr/bin:/bin", "HOME": "/tmp", "PYTHONDONTWRITEBYTECODE": "1"}
    dependencies = python_inventory(py, inventory_env)
    if dependencies != manifest.get("python_dependencies"):
        raise ValueError("external Python dependency versions or metadata changed")
    report = {"status": "PASS", "archive_sha256": sha(a.archive), "root": str(root),
              "files_manifest_sha256": files["sha256"], "source_commit": manifest["source_commit"],
              "patch_sha256": manifest["patch_sha256"], "cli": str(exe), "python": str(py),
              "library": str(selected), "library_sha256": selected_hash, "python_probe": py_result,
              "cli_version": cli_version.strip(),
              "python_dependencies": dependencies,
              "external_runtime_libraries": manifest.get("external_runtime_libraries", {}),
              "transport_executed": False}
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if a.output:
        a.output.write_text(encoded)
    print(encoded, end="")

if __name__ == "__main__":
    main()
