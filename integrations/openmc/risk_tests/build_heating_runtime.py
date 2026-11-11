#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Build and package a pinned, compensated-heating OpenMC 0.15.3 runtime."""
from __future__ import annotations

import argparse
import gzip
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
from build_mcnp_imp0_runtime import (
    manifest_digest as digest_json,
    sha256_file,
    source_manifest as manifest,
    verify_generated_version_header,
)

BASE_COMMIT = "27e38e894697bb32a1dac7848d2618818b6b8daf"
PATCH_SHA256 = "cb1537776fefd4bf733dd33cc8b9b265a4e8a5d07af82362126de783fdc55cec"
PATCH_REL = Path("integrations/openmc/patches/faris-openmc-0.15.3-compensated-heating.patch")
LICENSE_REL = Path("integrations/openmc/patches/LICENSE-OpenMC-MIT")
VENDOR_COMMITS = {
    "vendor/Catch2": "5a40b2275caa05cf809bf04df848764a9d7df2e2",
    "vendor/fmt": "0c9fce2ffefecfdce794e1859584e25877b7b592",
    "vendor/pugixml": "ee86beb30e4973f5feffe3ce63bfa4fbadf72f38",
    "vendor/xtensor": "3634f2ded19e0cf38208c8b86cea9e1d7c8e397d",
    "vendor/xtl": "a7c1c5444dfc57f76620391af4c94785ff82c8d6",
}
VENDOR_LICENSES = {
    "Catch2": "LICENSE.txt",
    "fmt": "LICENSE",
    "pugixml": "LICENSE.md",
    "xtensor": "LICENSE",
    "xtl": "LICENSE",
}
PYTHON_DISTRIBUTIONS = (
    "numpy", "h5py", "scipy", "ipython", "matplotlib", "pandas", "lxml",
    "uncertainties", "setuptools", "endf",
)
PYTHON_INVENTORY = """
import hashlib, importlib.metadata, json, sys
result = {}
for name in json.loads(sys.argv[1]):
    try:
        distribution = importlib.metadata.distribution(name)
    except importlib.metadata.PackageNotFoundError:
        result[name] = {"version": None, "metadata_sha256": None}
        continue
    metadata = distribution.read_text("METADATA") or distribution.read_text("PKG-INFO") or ""
    result[name] = {"version": distribution.version,
                    "metadata_sha256": hashlib.sha256(metadata.encode()).hexdigest()}
print(json.dumps(result, sort_keys=True))
"""


def run(argv: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None,
        log: Path | None = None) -> str:
    if log:
        log.parent.mkdir(parents=True, exist_ok=True)
        with log.open("wb") as output:
            subprocess.run(argv, cwd=cwd, env=env, check=True, stdout=output, stderr=subprocess.STDOUT)
        return str(log)
    result = subprocess.run(argv, cwd=cwd, env=env, check=True, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return result.stdout.strip()


def parse_gitlinks(repo: Path) -> dict[str, str]:
    output = run(["git", "-C", str(repo), "ls-tree", "-r", BASE_COMMIT])
    links = {}
    for line in output.splitlines():
        left, path = line.split("\t", 1)
        mode, kind, oid = left.split()
        if mode == "160000" and kind == "commit":
            links[path] = oid
    return links


def verify_vendor(repo: Path, vendor_root: Path) -> dict[str, str]:
    links = parse_gitlinks(repo)
    if links != VENDOR_COMMITS:
        raise RuntimeError(f"pinned source gitlinks differ from reviewed vendor set: {links}")
    receipts = {}
    for relative, expected in links.items():
        checkout = (vendor_root / relative).resolve(strict=True)
        head = run(["git", "-C", str(checkout), "rev-parse", "HEAD"])
        if head != expected:
            raise RuntimeError(f"{relative} vendor checkout is {head}, expected {expected}")
        receipts[relative] = head
    return receipts


def checked_tool(path: Path, label: str) -> Path:
    resolved = path.expanduser().resolve(strict=True)
    if not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise RuntimeError(f"--{label} must be an executable file")
    return resolved


def archive_source(repo: Path, commit: str, destination: Path) -> None:
    with subprocess.Popen(["git", "-C", str(repo), "archive", "--format=tar", commit],
                          stdout=subprocess.PIPE) as process:
        with tarfile.open(fileobj=process.stdout, mode="r|*") as archive:
            archive.extractall(destination, filter="data")
        if process.wait() != 0:
            raise RuntimeError(f"git archive failed for {repo}")


def write_package(prefix: Path, package: Path) -> None:
    def stable_metadata(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = 0
        return info

    temporary = package.with_suffix(package.suffix + ".tmp")
    with temporary.open("xb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                archive.add(prefix, arcname="faris-openmc-runtime", filter=stable_metadata)
    temporary.replace(package)


def stage_licenses(source: Path, prefix: Path) -> dict[str, str]:
    repository = Path(__file__).resolve().parents[3]
    shutil.copy2(repository / "LICENSE", prefix / "LICENSE-FARIS-AGPL-3.0")
    licenses = {name: sha256_file(prefix / name)
                for name in ("LICENSE-OpenMC-MIT", "LICENSE-FARIS-AGPL-3.0")}
    for vendor, filename in VENDOR_LICENSES.items():
        relative = Path("share/licenses") / vendor / filename
        destination = prefix / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / "vendor" / vendor / filename, destination)
        licenses[relative.as_posix()] = sha256_file(destination)
    return licenses


def python_inventory(python: Path, env: dict[str, str]) -> dict:
    return json.loads(run([str(python), "-c", PYTHON_INVENTORY,
                           json.dumps(PYTHON_DISTRIBUTIONS)], env=env))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-repo", type=Path, required=True, help="pinned OpenMC git checkout")
    parser.add_argument("--vendor-root", type=Path, required=True, help="directory containing vendor/<name> git checkouts")
    parser.add_argument("--dependency-prefix", type=Path, required=True, help="base prefix providing Python and native dependencies")
    parser.add_argument("--cmake", type=Path, required=True)
    parser.add_argument("--ninja", type=Path, required=True)
    parser.add_argument("--cxx", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=2)
    args = parser.parse_args()

    if sys.platform != "linux":
        raise RuntimeError("this local runtime builder currently supports Linux ldd")
    if args.jobs < 1:
        raise RuntimeError("--jobs must be positive")
    repo = args.source_repo.expanduser().resolve(strict=True)
    vendor_root = args.vendor_root.expanduser().resolve(strict=True)
    base = args.dependency_prefix.expanduser().resolve(strict=True)
    output = args.output_dir.expanduser().resolve(strict=False)
    if output.exists():
        raise RuntimeError(f"refusing to reuse output directory: {output}")
    source_head = run(["git", "-C", str(repo), "rev-parse", "HEAD"])
    if source_head != BASE_COMMIT:
        raise RuntimeError(f"source checkout HEAD {source_head} != required {BASE_COMMIT}")
    vendor_heads = verify_vendor(repo, vendor_root)
    patch = Path(__file__).resolve().parents[3] / PATCH_REL
    license_file = patch.parent / LICENSE_REL.name
    if sha256_file(patch) != PATCH_SHA256:
        raise RuntimeError("checked-in compensated-heating patch hash changed")
    cmake = checked_tool(args.cmake, "cmake")
    ninja = checked_tool(args.ninja, "ninja")
    cxx = checked_tool(args.cxx, "cxx")
    python = (base / "bin/python").resolve(strict=True)
    if not python.is_file() or not os.access(python, os.X_OK):
        raise RuntimeError("dependency prefix must provide executable bin/python")
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    pyver = run([str(python), "-c", "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')"])
    for needed in ("LICENSE", "CMakeLists.txt", "include/openmc/version.h.in"):
        if not (repo / needed).exists():
            raise RuntimeError(f"source checkout is missing {needed}")

    output.parent.mkdir(parents=True, exist_ok=True)
    output.mkdir()
    source = output / "source"
    source.mkdir()
    archive_source(repo, BASE_COMMIT, source)
    for relative in vendor_heads:
        target = source / relative
        shutil.rmtree(target, ignore_errors=True)
        target.mkdir(parents=True)
        archive_source(vendor_root / relative, vendor_heads[relative], target)
    source_manifest_sha256 = digest_json(manifest(source))
    if sha256_file(source / "LICENSE") != sha256_file(repo / "LICENSE"):
        raise RuntimeError("archived source license differs from pinned checkout")

    archival_file = output / ".git_archival.resolved.txt"
    archival_file.write_text(
        "# Generated by FARIS from a verified OpenMC source archive; do not copy into source.\n"
        f"commit: {BASE_COMMIT}\n"
        "describe-name: v0.15.3-faris-compensated-heating\n", encoding="utf-8")
    patch_log = output / "patch-application.log"
    run(["patch", "--batch", "--forward", "-p1", "-i", str(patch)], cwd=source, log=patch_log)
    patched_source_manifest_sha256 = digest_json(manifest(source))
    build = output / "build"
    prefix = output / "runtime"
    cmake_env = os.environ.copy()
    for key in ("CMAKE_PREFIX_PATH", "CMAKE_TOOLCHAIN_FILE", "CMAKE_GENERATOR", "CMAKE_ARGS", "CFLAGS", "CXXFLAGS", "LDFLAGS", "LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT"):
        cmake_env.pop(key, None)
    dep_lib = base / "lib"
    hook = output / "runtime-layout.cmake"
    hook.write_text(
        "cmake_language(DEFER CALL set_target_properties openmc PROPERTIES "
        "INSTALL_RPATH \"$ORIGIN/../lib\" INSTALL_RPATH_USE_LINK_PATH FALSE "
        "BUILD_WITH_INSTALL_RPATH TRUE)\n"
        "cmake_language(DEFER CALL set_target_properties libopenmc PROPERTIES "
        f"INSTALL_RPATH \"$ORIGIN;{dep_lib}\" INSTALL_RPATH_USE_LINK_PATH FALSE "
        "BUILD_WITH_INSTALL_RPATH TRUE)\n"
        "cmake_language(DEFER CALL target_link_options openmc PRIVATE \"LINKER:--disable-new-dtags\")\n"
        "cmake_language(DEFER CALL target_link_options libopenmc PRIVATE \"LINKER:--disable-new-dtags\")\n",
        encoding="utf-8")
    hook_sha256 = sha256_file(hook)
    configure = [str(cmake), "-S", str(source), "-B", str(build), "-G", "Ninja",
                 f"-DCMAKE_MAKE_PROGRAM={ninja}", f"-DCMAKE_CXX_COMPILER={cxx}",
                 "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_INSTALL_PREFIX={prefix}",
                 f"-DCMAKE_PREFIX_PATH={base}", f"-DHDF5_ROOT={base}",
                 f"-DARCHIVAL_FILE={archival_file}",
                 "-DOPENMC_USE_MPI=OFF", "-DOPENMC_USE_DAGMC=OFF", "-DOPENMC_USE_OPENMP=ON",
                 f"-DCMAKE_PROJECT_openmc_INCLUDE={hook}"]
    run(configure, env=cmake_env, log=output / "cmake-configure.log")
    version_header = build / "include/openmc/version.h"
    version_text = version_header.read_text(encoding="utf-8")
    version_values = verify_generated_version_header(version_text, BASE_COMMIT)
    run([str(cmake), "--build", str(build), "--target", "openmc", "--parallel", str(args.jobs)],
        env=cmake_env, log=output / "cmake-build.log")
    run([str(cmake), "--install", str(build)], env=cmake_env, log=output / "cmake-install.log")

    executable = prefix / "bin/openmc"
    library = prefix / "lib/libopenmc.so"
    if not executable.is_file() or not library.is_file():
        raise RuntimeError("CMake install did not produce the expected CLI and shared library")
    venv_dir = prefix
    run([str(python), "-m", "venv", "--copies", "--without-pip", str(venv_dir)])
    vpy = venv_dir / "bin/python"
    site = Path(run([str(vpy), "-c", "import site; print(site.getsitepackages()[0])"]))
    (site / "faris-runtime-dependencies.pth").write_text(
        str(base / "lib" / f"python{pyver}" / "site-packages") + "\n", encoding="utf-8")
    shutil.copytree(source / "openmc", site / "openmc", symlinks=True,
                    ignore=shutil.ignore_patterns("*.so", "*.pyd", "__pycache__"))
    metadata = site / "openmc-0.15.3.dist-info"
    metadata.mkdir()
    (metadata / "METADATA").write_text("Metadata-Version: 2.1\nName: openmc\nVersion: 0.15.3\n\n", encoding="utf-8")
    (metadata / "INSTALLER").write_text("faris-local-builder\n", encoding="utf-8")
    if run([str(vpy), "-c", "import openmc; print(openmc.__version__)"]) != "0.15.3":
        raise RuntimeError("bundled Python OpenMC version check failed")
    runtime_executable = executable
    api_library = site / "openmc/lib/libopenmc.so"
    api_library.symlink_to(os.path.relpath(library, api_library.parent))
    for activation in (prefix / "bin").glob("activate*"):
        activation.unlink()
    shutil.copy2(license_file, prefix / "LICENSE-OpenMC-MIT")
    shutil.copy2(patch, prefix / "faris-openmc-0.15.3-compensated-heating.patch")
    license_files = stage_licenses(source, prefix)

    # The installed executable must use only its relocatable sibling library path.
    cli_dynamic = run(["readelf", "-d", str(runtime_executable)])
    cli_rpath = re.findall(r"\(RPATH\).*?\[(.*?)\]", cli_dynamic)
    if cli_rpath != ["$ORIGIN/../lib"] or "RUNPATH" in cli_dynamic:
        raise RuntimeError(f"installed CLI DT_RPATH is not exactly $ORIGIN/../lib: {cli_dynamic}")
    lib_dynamic = run(["readelf", "-d", str(library)])
    lib_rpath = re.findall(r"\(RPATH\).*?\[(.*?)\]", lib_dynamic)
    expected_lib_rpath = f"$ORIGIN:{dep_lib}"
    if lib_rpath != [expected_lib_rpath] or "RUNPATH" in lib_dynamic:
        raise RuntimeError(f"installed library DT_RPATH differs from {expected_lib_rpath}: {lib_dynamic}")

    runtime_env = os.environ.copy()
    for key in ("LD_PRELOAD", "LD_AUDIT"):
        runtime_env.pop(key, None)
    runtime_env["LD_LIBRARY_PATH"] = os.pathsep.join((str(prefix / "lib"), str(dep_lib)))
    clean_runtime_env = {key: value for key, value in runtime_env.items()
                         if key not in {"LD_LIBRARY_PATH", "PYTHONPATH", "LD_PRELOAD", "LD_AUDIT"}}
    python_probe = run([str(vpy), "-c",
        "import pathlib,sys,openmc,openmc.lib; p=pathlib.Path(sys.argv[1]).resolve(); "
        "assert pathlib.Path(sys.executable).resolve().parent.parent==p; "
        "assert pathlib.Path(openmc.__file__).resolve().is_relative_to(p); "
        "assert pathlib.Path(openmc.lib._dll._name).resolve()==(p/'lib/libopenmc.so').resolve(); "
        "print(openmc.__version__)", str(prefix)], env=clean_runtime_env)
    if python_probe != "0.15.3":
        raise RuntimeError("bundled Python C API probe returned the wrong version")
    ldd_text = run(["ldd", "-r", str(runtime_executable)], env=clean_runtime_env) + "\n" + run(["ldd", "-r", str(library)], env=clean_runtime_env)
    if "not found" in ldd_text or "undefined symbol" in ldd_text:
        raise RuntimeError(f"installed runtime has unresolved dependencies: {ldd_text}")
    external = {}
    for line in ldd_text.splitlines():
        match = re.search(r"=>\s+(/\S+)\s+\(", line)
        if match:
            path = Path(match.group(1)).resolve()
            if path.is_file() and not path.is_relative_to(prefix):
                external[str(path)] = sha256_file(path)
    runtime_receipt = {
        "schema": "faris.openmc-compensated-runtime/v1",
        "openmc_version": "0.15.3", "source_commit": BASE_COMMIT,
        "source_manifest_sha256": source_manifest_sha256,
        "patched_source_manifest_sha256": patched_source_manifest_sha256,
        "patch_sha256": PATCH_SHA256, "dependency_prefix": str(base),
        "cmake_hook_sha256": hook_sha256,
        "configure_argv": configure,
        "tools": {str(tool): {"sha256": sha256_file(tool), "version": run([str(tool), "--version"]).splitlines()[0]}
                  for tool in (cmake, ninja, cxx)},
        "python_base": {"path": str(python), "sha256": sha256_file(python)},
        "python_dependencies": python_inventory(vpy, clean_runtime_env),
        "python_version": pyver, "vendor_gitlinks": vendor_heads,
        "external_runtime_libraries": external,
        "license_files": license_files,
        "runtime_scope": "local package; requires the selected dependency prefix Python and native libraries listed above",
    }
    (prefix / "runtime-manifest.json").write_text(json.dumps(runtime_receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    files = manifest(prefix)
    file_list = {"schema": "faris.runtime-files/1.0.0", "files": files, "sha256": digest_json(files)}
    (prefix / "runtime-files.sha256.json").write_text(json.dumps(file_list, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    package = output / "faris-openmc-0.15.3-compensated-heating-linux.tar.gz"
    write_package(prefix, package)
    receipt = {
        "schema": "faris.openmc-compensated-build/v1", "status": "BUILT",
        "source_commit": BASE_COMMIT, "source_manifest_sha256": source_manifest_sha256,
        "patched_source_manifest_sha256": patched_source_manifest_sha256, "vendor_gitlinks": vendor_heads,
        "patch_sha256": PATCH_SHA256, "license_sha256": sha256_file(license_file),
        "license_files": license_files,
        "openmc_cli_sha256": sha256_file(executable),
        "openmc_library_sha256": sha256_file(library),
        "package_sha256": sha256_file(package), "runtime_manifest_sha256": sha256_file(prefix / "runtime-manifest.json"),
        "cmake_hook_sha256": hook_sha256,
        "runtime_files_sha256": file_list["sha256"], "external_runtime_libraries": external,
        "package": package.name,
    }
    (output / "build-receipt.json").write_text(json.dumps(receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(receipt, sort_keys=True, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
