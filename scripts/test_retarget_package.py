import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
import shutil
import stat
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).parent


def load(name: str):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RETARGET = load("retarget_package")
RELEASE = load("make_release")

FARIS_COMMIT = "a" * 40
CORE_COMMIT = "b" * 40
VERSION = "0.1.1"
MTIME = 1_790_000_000


def sha(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def write(path: Path, data: bytes | str, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data.encode() if isinstance(data, str) else data)
    path.chmod(mode)


def write_index(package: Path, index: dict) -> None:
    index_bytes = (json.dumps(index, indent=2) + "\n").encode()
    write(package / "package-index.json", index_bytes, 0o444)
    write(package / "package-index.sha256", f"{sha(index_bytes)}  package-index.json\n", 0o444)


def linux_package(root: Path) -> Path:
    """A small finished linux/x86_64 v0.5 package with fake programs."""
    package = root / "linux"
    contents = {
        "bin/faris": (b"linux faris", 0o555, "app"),
        "bin/faris-app": (b"linux app", 0o555, "app"),
        "bin/avila-core": (b"linux core", 0o555, "app"),
        "README.md": (b"readme\n", 0o444, "app"),
        "control/scenario.json": (b"{}\n", 0o444, "app"),
        "licenses/faris-LICENSE": (b"licence\n", 0o444, "app"),
        "licenses/faris-THIRD_PARTY_NOTICES.md": (b"linux faris notices\n", 0o444, "app"),
        "licenses/core-RUNTIME_DEPENDENCY_NOTICES.md": (b"linux core notices\n", 0o444, "app"),
        "SOURCE_PROVENANCE.md": (b"linux provenance, debug core\n", 0o444, "app"),
        "verify.sh": (b"#!/bin/sh\n", 0o555, "evidence"),
        "scripts/verify.py": (b"v\n", 0o444, "evidence"),
        "port/cases/reference.tar.gz": (b"evidence archive bytes\n" * 50, 0o444, "evidence"),
    }
    files = []
    for relative, (data, mode, part) in contents.items():
        write(package / relative, data, mode)
        files.append({"path": relative, "bytes": len(data), "sha256": sha(data), "part": part})
    files.sort(key=lambda item: item["path"])
    recorded = {name: {"path": f"bin/{name}", "sha256": sha(contents[f"bin/{name}"][0]),
                       "bytes": len(contents[f"bin/{name}"][0]), "version": f"{name} {VERSION}"}
                for name in ("faris", "faris-app", "avila-core")}
    app = [item for item in files if item["part"] == "app"]
    evidence = [item for item in files if item["part"] == "evidence"]
    index = {
        "schema_version": "faris-recorded-demo-package/v0.5",
        "status": "IDENTITIES_REVALIDATED_CORE_EXECUTIONS_COMPLETED_PHYSICS_NOT_EVALUATED",
        "faris_cli_sha256": recorded["faris"]["sha256"],
        "faris_app_sha256": recorded["faris-app"]["sha256"],
        "core_executable_sha256": recorded["avila-core"]["sha256"],
        "evidence_recorded_with": {"platform": {"os": "linux", "arch": "x86_64"},
                                   "faris_cli_sha256": recorded["faris"]["sha256"],
                                   "core_executable_sha256": recorded["avila-core"]["sha256"]},
        "local_runtime": {
            "schema_version": "faris-local-runtime/v0.1",
            "platform": {"os": "linux", "arch": "x86_64"},
            "executables": recorded,
            "source_provenance": {
                "faris": {"repository": "https://example.test/faris.git", "commit": FARIS_COMMIT},
                "core": {"repository": "https://example.test/core.git", "commit": CORE_COMMIT,
                         "binary_profile": "debug"},
                "rebuild": ["Packaged Core: run cargo build --locked --bin avila-core, then strip --strip-debug."],
            },
            "launcher": {"kind": "native", "executable": "faris-app"},
            "verifier": {"path": "verify.sh", "sha256": sha(contents["verify.sh"][0])},
        },
        "package_file_count": len(files),
        "package_bytes": sum(item["bytes"] for item in files),
        "parts": {"app": {"file_count": len(app), "bytes": sum(item["bytes"] for item in app)},
                  "evidence": {"file_count": len(evidence), "bytes": sum(item["bytes"] for item in evidence),
                               "archive_name": f"FARIS-{VERSION}-evidence.tar.gz"}},
        "files": files,
    }
    write_index(package, index)
    for directory in sorted((p for p in package.rglob("*") if p.is_dir()), key=lambda p: len(p.parts), reverse=True):
        directory.chmod(0o555)
    return package


def desktop_build(root: Path, platform: str, name: str | None = None) -> Path:
    """A build folder as the desktop workflow uploads it."""
    build = root / (name or f"build-{platform}")
    suffix = ".exe" if platform.startswith("windows-") else ""
    executables = {}
    for program in ("faris", "faris-app", "avila-core"):
        data = f"{platform} {program}".encode() * 10
        if platform.startswith("linux-"):
            # Runnable, like the CI build on the laptop: it reports its version.
            data = f"#!/bin/sh\necho {program} {VERSION}\n".encode()
        write(build / "bin" / f"{program}{suffix}", data, 0o644)
        executables[program] = {"path": f"bin/{program}{suffix}", "sha256": sha(data), "bytes": len(data)}
    notices = {}
    for name in ("faris-THIRD_PARTY_NOTICES.md", "core-RUNTIME_DEPENDENCY_NOTICES.md"):
        data = f"{platform} {name}\n".encode()
        write(build / "licenses" / name, data, 0o644)
        notices[name] = {"path": f"licenses/{name}", "sha256": sha(data), "bytes": len(data)}
    record = {"schema_version": "faris-desktop-build/v0.1", "platform": platform, "runner": "test",
              "faris_commit": FARIS_COMMIT, "faris_version": f"faris {VERSION}",
              "core_commit": CORE_COMMIT, "core_version": "avila-core 0.1.0",
              "profile": "release", "executables": executables, "notices": notices}
    if platform.startswith("linux-"):
        record["glibc"] = "2.35"
    write(build / "build.json", json.dumps(record, indent=2) + "\n", 0o644)
    return build


def rewrite_build(build: Path, mutate) -> None:
    record = json.loads((build / "build.json").read_text())
    mutate(record)
    (build / "build.json").write_text(json.dumps(record))


def rewrite_index(package: Path, mutate) -> None:
    for path in [package, *package.rglob("*")]:
        if path.is_dir():
            path.chmod(0o755)
    index = json.loads((package / "package-index.json").read_text())
    mutate(index)
    (package / "package-index.json").chmod(0o644)
    (package / "package-index.sha256").chmod(0o644)
    write_index(package, index)


def unlock(root: Path) -> None:
    for path in [root, *root.rglob("*")]:
        if path.is_dir():
            path.chmod(0o755)


class RetargetTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.linux = linux_package(self.root)

    def tearDown(self):
        unlock(self.root)
        self.temporary.cleanup()

    def retarget(self, platform: str = "windows-x86_64", build: Path | None = None, name: str = "out") -> Path:
        build = build or desktop_build(self.root, platform)
        out = self.root / name
        RETARGET.retarget(self.linux, build, out)
        return out

    def test_index_is_rewritten_for_the_new_platform(self):
        out = self.retarget("windows-x86_64")
        linux = json.loads((self.linux / "package-index.json").read_text())
        index = json.loads((out / "package-index.json").read_text())
        build = json.loads((self.root / "build-windows-x86_64" / "build.json").read_text())
        self.assertEqual(index["local_runtime"]["platform"], {"os": "windows", "arch": "x86_64"})
        self.assertEqual(index["local_runtime"]["executables"], build["executables"])
        self.assertEqual(index["desktop_build"], build)
        self.assertEqual(index["faris_cli_sha256"], build["executables"]["faris"]["sha256"])
        self.assertEqual(index["faris_app_sha256"], build["executables"]["faris-app"]["sha256"])
        self.assertEqual(index["core_executable_sha256"], build["executables"]["avila-core"]["sha256"])
        self.assertEqual(index["evidence_recorded_with"], linux["evidence_recorded_with"])
        self.assertEqual(index["evidence_recorded_with"]["faris_cli_sha256"], linux["faris_cli_sha256"])
        by_path = {item["path"]: item for item in index["files"]}
        for name in ("faris", "faris-app", "avila-core"):
            record = build["executables"][name]
            self.assertEqual(by_path[record["path"]],
                             {"path": record["path"], "bytes": record["bytes"], "sha256": record["sha256"],
                              "part": "app"})
            self.assertNotIn(f"bin/{name}", by_path)
        self.assertEqual(index["package_file_count"], len(index["files"]))
        self.assertEqual(index["package_bytes"], sum(item["bytes"] for item in index["files"]))
        app = [item for item in index["files"] if item["part"] == "app"]
        self.assertEqual(index["parts"]["app"], {"file_count": len(app), "bytes": sum(i["bytes"] for i in app)})
        self.assertEqual(index["parts"]["evidence"], linux["parts"]["evidence"])
        for key in set(linux) - {"faris_cli_sha256", "faris_app_sha256", "core_executable_sha256",
                                 "local_runtime", "files", "package_file_count", "package_bytes", "parts"}:
            self.assertEqual(index[key], linux[key], key)
        sources, linux_sources = (i["local_runtime"]["source_provenance"] for i in (index, linux))
        self.assertEqual(sources["faris"], linux_sources["faris"])
        self.assertEqual(sources["core"], dict(linux_sources["core"], binary_profile="release"))
        for item in index["files"]:
            data = (out / item["path"]).read_bytes()
            self.assertEqual((len(data), sha(data)), (item["bytes"], item["sha256"]))
        index_path = out / "package-index.json"
        self.assertEqual((out / "package-index.sha256").read_text(), f"{sha(index_path.read_bytes())}  package-index.json\n")

    def test_files_other_than_programs_and_index_are_hard_links_and_unchanged(self):
        out = self.retarget("macos-aarch64")
        for relative in ("control/scenario.json", "licenses/faris-LICENSE", "verify.sh", "scripts/verify.py",
                         "port/cases/reference.tar.gz"):
            self.assertTrue(os.path.samefile(self.linux / relative, out / relative), relative)
            self.assertEqual(os.stat(out / relative).st_mode, os.stat(self.linux / relative).st_mode)
        self.assertEqual((out / "README.md").read_bytes(), (self.linux / "README.md").read_bytes())
        self.assertFalse(os.path.samefile(self.linux / "README.md", out / "README.md"))
        files = sorted(p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file())
        self.assertEqual(files, sorted(["bin/faris", "bin/faris-app", "bin/avila-core", "README.md",
                                        "SOURCE_PROVENANCE.md", "licenses/faris-THIRD_PARTY_NOTICES.md",
                                        "licenses/core-RUNTIME_DEPENDENCY_NOTICES.md",
                                        "control/scenario.json", "licenses/faris-LICENSE", "verify.sh",
                                        "scripts/verify.py", "port/cases/reference.tar.gz",
                                        "package-index.json", "package-index.sha256"]))
        for name in ("faris", "faris-app", "avila-core"):
            self.assertEqual(stat.S_IMODE((out / "bin" / name).stat().st_mode), 0o755)
            self.assertFalse(os.path.samefile(self.linux / "bin" / name, out / "bin" / name))
        self.assertEqual((out / "bin/faris").read_bytes(), b"macos-aarch64 faris" * 10)
        self.assertEqual(stat.S_IMODE((self.linux / "bin/faris").stat().st_mode), 0o555)

    def test_notices_are_the_builds_and_indexed(self):
        for platform in ("windows-x86_64", "macos-aarch64"):
            out = self.retarget(platform, name=f"out-{platform}")
            index = json.loads((out / "package-index.json").read_text())
            by_path = {item["path"]: item for item in index["files"]}
            for name in ("faris-THIRD_PARTY_NOTICES.md", "core-RUNTIME_DEPENDENCY_NOTICES.md"):
                relative = f"licenses/{name}"
                data = (out / relative).read_bytes()
                self.assertEqual(data, f"{platform} {name}\n".encode())
                self.assertNotEqual(data, (self.linux / relative).read_bytes())
                self.assertEqual(by_path[relative], {"path": relative, "bytes": len(data),
                                                     "sha256": sha(data), "part": "app"})
                self.assertFalse(os.path.samefile(self.linux / relative, out / relative))

    def test_missing_or_mismatched_notices_are_refused(self):
        cases = {
            "no notices": (lambda r: r.pop("notices"), "notices"),
            "one notice": (lambda r: r["notices"].pop("core-RUNTIME_DEPENDENCY_NOTICES.md"), "notices"),
            "extra notice": (lambda r: r["notices"].update(extra=dict(r["notices"]["faris-THIRD_PARTY_NOTICES.md"])),
                             "notices"),
            "wrong hash": (lambda r: r["notices"]["faris-THIRD_PARTY_NOTICES.md"].update(sha256="sha256:" + "0" * 64),
                           "SHA-256"),
            "wrong size": (lambda r: r["notices"]["core-RUNTIME_DEPENDENCY_NOTICES.md"].update(bytes=3), "SHA-256"),
            "wrong path": (lambda r: r["notices"]["faris-THIRD_PARTY_NOTICES.md"].update(path="x.md"),
                           "must be licenses/"),
        }
        for label, (mutate, message) in cases.items():
            with self.subTest(label):
                build = desktop_build(self.root, "windows-x86_64", f"build-{label.replace(' ', '-')}")
                rewrite_build(build, mutate)
                with self.assertRaisesRegex(SystemExit, message):
                    RETARGET.retarget(self.linux, build, self.root / "never")
                self.assertFalse((self.root / "never").exists())
        build = desktop_build(self.root, "windows-x86_64", "build-changed-bytes")
        (build / "licenses/faris-THIRD_PARTY_NOTICES.md").write_bytes(b"swapped after the build record\n")
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            RETARGET.retarget(self.linux, build, self.root / "never")

    def test_linux_target_swaps_programs_and_keeps_the_laptop_pins_as_recorded_with(self):
        out = self.retarget("linux-x86_64")
        linux = json.loads((self.linux / "package-index.json").read_text())
        index = json.loads((out / "package-index.json").read_text())
        build = json.loads((self.root / "build-linux-x86_64" / "build.json").read_text())
        self.assertEqual(build["glibc"], "2.35")
        self.assertEqual(index["local_runtime"]["platform"], {"os": "linux", "arch": "x86_64"})
        self.assertEqual(index["local_runtime"]["executables"], build["executables"])
        self.assertEqual(index["desktop_build"], build)
        self.assertEqual(index["faris_cli_sha256"], build["executables"]["faris"]["sha256"])
        self.assertEqual(index["core_executable_sha256"], build["executables"]["avila-core"]["sha256"])
        self.assertNotEqual(index["faris_cli_sha256"], linux["faris_cli_sha256"])
        self.assertEqual(index["evidence_recorded_with"], linux["evidence_recorded_with"])
        self.assertEqual(index["evidence_recorded_with"]["faris_cli_sha256"], linux["faris_cli_sha256"])
        for name in ("faris", "faris-app", "avila-core"):
            self.assertEqual(sha((out / "bin" / name).read_bytes()), build["executables"][name]["sha256"])
            self.assertEqual(stat.S_IMODE((out / "bin" / name).stat().st_mode), 0o755)
        self.assertEqual((out / "licenses/faris-THIRD_PARTY_NOTICES.md").read_text(), "linux-x86_64 faris-THIRD_PARTY_NOTICES.md\n")
        text = (out / "SOURCE_PROVENANCE.md").read_text()
        self.assertIn("- Platform: Linux x86_64.", text)
        self.assertIn("release profile", text)
        self.assertIn("The programs need glibc 2.35 or newer", text)
        self.assertIn("`target/release/avila-core`", text)
        self.assertNotIn("debug", text.lower())
        record = next(i for i in index["files"] if i["path"] == "SOURCE_PROVENANCE.md")
        self.assertEqual(record["sha256"], sha(text.encode()))
        for other in ("windows-x86_64", "macos-aarch64"):
            other_text = (self.retarget(other, name=f"out-{other}") / "SOURCE_PROVENANCE.md").read_text()
            self.assertNotIn("glibc", other_text)

    def test_linux_build_needs_a_glibc_version(self):
        for label, mutate in (("missing", lambda r: r.pop("glibc", None)),
                              ("empty", lambda r: r.update(glibc="")),
                              ("not a version", lambda r: r.update(glibc="new")),
                              ("number", lambda r: r.update(glibc=2.35))):
            with self.subTest(label):
                build = desktop_build(self.root, "linux-x86_64", f"build-glibc-{label.replace(' ', '-')}")
                rewrite_build(build, mutate)
                with self.assertRaisesRegex(SystemExit, "glibc"):
                    RETARGET.retarget(self.linux, build, self.root / "never")
                self.assertFalse((self.root / "never").exists())

    def test_linux_build_must_be_x86_64(self):
        build = desktop_build(self.root, "linux-x86_64", "build-linux-arm")
        rewrite_build(build, lambda r: r.update(platform="linux-aarch64"))
        with self.assertRaisesRegex(SystemExit, "x86_64"):
            RETARGET.retarget(self.linux, build, self.root / "never")

    def test_an_already_retargeted_package_is_refused(self):
        for platform in ("linux-x86_64", "windows-x86_64"):
            done = self.retarget(platform, name=f"done-{platform}")
            with self.assertRaisesRegex(SystemExit, "already retargeted"):
                RETARGET.retarget(done, desktop_build(self.root, "linux-x86_64", f"again-{platform}"),
                                  self.root / f"never-{platform}")
            self.assertFalse((self.root / f"never-{platform}").exists())

    def test_provenance_names_the_platform_and_release_profile(self):
        for platform, label, exe in (("windows-x86_64", "Windows x86_64", "avila-core.exe"),
                                     ("macos-aarch64", "macOS aarch64", "avila-core"),
                                     ("macos-x86_64", "macOS x86_64", "avila-core")):
            out = self.retarget(platform, name=f"out-{platform}")
            text = (out / "SOURCE_PROVENANCE.md").read_text()
            self.assertIn(f"- Platform: {label}.", text)
            self.assertIn("release profile", text)
            self.assertNotIn("debug", text.lower())
            self.assertIn(f"`target/release/{exe}`", text)
            self.assertIn("cargo build --release --locked -p faris-cli -p faris-app", text)
            self.assertIn("cargo build --release --locked --bin avila-core", text)
            self.assertIn("https://example.test/faris.git` at `" + FARIS_COMMIT, text)
            self.assertIn("https://example.test/core.git` at `" + CORE_COMMIT, text)
            self.assertIn("`faris 0.1.1`", text)
            self.assertIn("Linux x86_64", text)
            self.assertIn("evidence_recorded_with", text)
            self.assertIn("no bit-for-bit reproducibility claim", text)
            self.assertIn("unsigned", text)
            self.assertNotIn("faris-app 0.1.1", text)
            index = json.loads((out / "package-index.json").read_text())
            record = next(i for i in index["files"] if i["path"] == "SOURCE_PROVENANCE.md")
            self.assertEqual(record, {"path": "SOURCE_PROVENANCE.md", "bytes": len(text.encode()),
                                      "sha256": sha(text.encode()), "part": "app"})
            sources = index["local_runtime"]["source_provenance"]
            self.assertEqual(sources["core"]["binary_profile"], "release")
            self.assertEqual(sources["core"]["commit"], CORE_COMMIT)
            self.assertEqual(sources["core"]["repository"], "https://example.test/core.git")
            self.assertEqual(sources["faris"]["repository"], "https://example.test/faris.git")
            rebuild = "\n".join(sources["rebuild"])
            self.assertIn("cargo build --release --locked -p faris-cli -p faris-app", rebuild)
            self.assertIn(f"target/release/{exe}", rebuild)
            self.assertNotIn("debug", rebuild.lower())

    def test_windows_programs_carry_exe(self):
        out = self.retarget("windows-aarch64")
        self.assertTrue((out / "bin/faris-app.exe").is_file())
        self.assertFalse((out / "bin/faris-app").exists())
        index = json.loads((out / "package-index.json").read_text())
        self.assertEqual(index["local_runtime"]["executables"]["faris-app"]["path"], "bin/faris-app.exe")

    def test_existing_output_is_refused(self):
        build = desktop_build(self.root, "windows-x86_64")
        (self.root / "out").mkdir()
        with self.assertRaisesRegex(SystemExit, "exists"):
            RETARGET.retarget(self.linux, build, self.root / "out")

    def test_wrong_commit_version_hash_platform_or_extension_is_refused(self):
        cases = {
            "faris commit": (lambda r: r.update(faris_commit="c" * 40), "faris_commit"),
            "core commit": (lambda r: r.update(core_commit="c" * 40), "core_commit"),
            "version": (lambda r: r.update(faris_version="faris 0.1.0"), "faris 0.1.0"),
            "schema": (lambda r: r.update(schema_version="faris-desktop-build/v0.2"), "faris-desktop-build"),
            "linux build without glibc": (lambda r: r.update(platform="linux-x86_64"), "glibc"),
            "arch": (lambda r: r.update(platform="windows-riscv"), "platform"),
            "platform shape": (lambda r: r.update(platform="windows"), "platform"),
            "recorded hash": (lambda r: r["executables"]["faris"].update(sha256="sha256:" + "0" * 64),
                              "SHA-256"),
            "recorded size": (lambda r: r["executables"]["faris"].update(bytes=3), "SHA-256"),
            "missing program": (lambda r: r["executables"].pop("avila-core"), "executables"),
            "extension on windows": (lambda r: r["executables"]["faris"].update(path="bin/faris"), "bin/faris.exe"),
        }
        for label, (mutate, message) in cases.items():
            with self.subTest(label):
                build = desktop_build(self.root, "windows-x86_64", f"build-{label.replace(' ', '-')}")
                rewrite_build(build, mutate)
                with self.assertRaisesRegex(SystemExit, message):
                    RETARGET.retarget(self.linux, build, self.root / "never")
                self.assertFalse((self.root / "never").exists())

    def test_extension_on_macos_and_changed_program_bytes_are_refused(self):
        build = desktop_build(self.root, "macos-x86_64", "build-mac-exe")
        rewrite_build(build, lambda r: r["executables"]["faris-app"].update(path="bin/faris-app.exe"))
        with self.assertRaisesRegex(SystemExit, r"bin/faris-app on macos"):
            RETARGET.retarget(self.linux, build, self.root / "never")
        build = desktop_build(self.root, "macos-x86_64", "build-mac-bytes")
        (build / "bin/faris").write_bytes(b"swapped after the build record")
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            RETARGET.retarget(self.linux, build, self.root / "never")
        self.assertFalse((self.root / "never").exists())

    def test_package_must_be_a_matching_linux_v05_package(self):
        build = desktop_build(self.root, "windows-x86_64")

        def variant(mutate):
            target = self.root / "variant"
            if target.exists():
                unlock(target)
                shutil.rmtree(target)
            shutil.copytree(self.linux, target, symlinks=True)
            rewrite_index(target, mutate)
            return target

        for label, mutate, message in (
                ("schema", lambda i: i.update(schema_version="faris-recorded-demo-package/v0.4"), "v0.5"),
                ("platform", lambda i: i["local_runtime"].update(platform={"os": "linux", "arch": "aarch64"}),
                 "linux/x86_64"),
                ("no recorded_with", lambda i: i.pop("evidence_recorded_with"), "evidence_recorded_with"),
                ("faris commit", lambda i: i["local_runtime"]["source_provenance"]["faris"].update(commit="d" * 40),
                 "faris_commit"),
                ("core commit", lambda i: i["local_runtime"]["source_provenance"]["core"].update(commit="d" * 40),
                 "core_commit"),
                ("version", lambda i: i["local_runtime"]["executables"]["faris"].update(version="faris 0.1.0"),
                 "recorded 'faris 0.1.0'")):
            with self.subTest(label):
                package = variant(mutate)
                with self.assertRaisesRegex(SystemExit, message):
                    RETARGET.retarget(package, build, self.root / "never")
        # A checksum that does not match the index is refused.
        package = variant(lambda i: None)
        (package / "package-index.sha256").chmod(0o644)
        (package / "package-index.sha256").write_text("sha256:" + "0" * 64 + "  package-index.json\n")
        with self.assertRaisesRegex(SystemExit, "does not match"):
            RETARGET.retarget(package, build, self.root / "never")
        # A file that differs from its index entry is refused.
        package = variant(lambda i: None)
        (package / "control").chmod(0o755)
        (package / "control/scenario.json").unlink()
        write(package / "control/scenario.json", "{ }\n", 0o444)
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            RETARGET.retarget(package, build, self.root / "never")
        self.assertFalse((self.root / "never").exists())


def make_release_inputs(root: Path) -> None:
    (root / "Cargo.toml").write_text(f'[workspace.package]\nversion = "{VERSION}"\n')
    (root / "CHANGELOG.md").write_text(f"# Changelog\n\n## {VERSION} — 2026-10-06\n\nnotes\n")


def run_release(package: Path, output: Path, *extra: str) -> str:
    saved_argv = sys.argv
    sys.argv = ["make_release.py", "--package", str(package), "--version", VERSION,
                "--output-dir", str(output), *extra]
    try:
        with contextlib.redirect_stdout(io.StringIO()) as captured:
            code = RELEASE.main()
    finally:
        sys.argv = saved_argv
    assert code == 0
    return captured.getvalue()


class RetargetedReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.linux = linux_package(self.root)
        make_release_inputs(self.root)
        self.saved_root = RELEASE.ROOT
        RELEASE.ROOT = self.root
        self.saved_epoch = os.environ.get("SOURCE_DATE_EPOCH")
        os.environ["SOURCE_DATE_EPOCH"] = str(MTIME)

    def tearDown(self):
        RELEASE.ROOT = self.saved_root
        if self.saved_epoch is None:
            os.environ.pop("SOURCE_DATE_EPOCH", None)
        else:
            os.environ["SOURCE_DATE_EPOCH"] = self.saved_epoch
        unlock(self.root)
        self.temporary.cleanup()

    def target(self, platform: str) -> Path:
        out = self.root / f"pkg-{platform}"
        RETARGET.retarget(self.linux, desktop_build(self.root, platform), out)
        return out

    def test_windows_gives_a_deterministic_zip_with_executable_programs(self):
        package = self.target("windows-x86_64")
        run_release(package, self.root / "rel-a")
        os.utime(package / "README.md", (5, 5))  # an on-disk mtime must not matter
        run_release(package, self.root / "rel-b")
        name = f"FARIS-{VERSION}-windows-x86_64.zip"
        self.assertEqual((self.root / "rel-a" / name).read_bytes(), (self.root / "rel-b" / name).read_bytes())
        self.assertFalse((self.root / "rel-a" / f"FARIS-{VERSION}-windows-x86_64.tar.gz").exists())
        with zipfile.ZipFile(self.root / "rel-a" / name) as archive:
            names = archive.namelist()
            self.assertEqual(names, sorted(names))
            self.assertEqual(names[0], f"FARIS-{VERSION}/")
            self.assertIn(f"FARIS-{VERSION}/bin/faris-app.exe", names)
            for info in archive.infolist():
                self.assertEqual(info.date_time, (2026, 9, 21, 14, 13, 20))
                self.assertEqual(info.create_system, 3)
            program = archive.getinfo(f"FARIS-{VERSION}/bin/faris-app.exe")
            self.assertEqual(stat.S_IMODE(program.external_attr >> 16), 0o755)
            self.assertEqual(program.compress_type, zipfile.ZIP_DEFLATED)
            self.assertEqual(archive.read(program), b"windows-x86_64 faris-app" * 10)
            self.assertFalse(any("port/cases" in n or n.endswith("verify.sh") for n in names))

    def test_macos_gives_a_tar_gz_and_the_evidence_archive_equals_the_linux_one(self):
        package = self.target("macos-aarch64")
        out = self.root / "rel"
        run_release(package, out)
        self.assertTrue((out / f"FARIS-{VERSION}-macos-aarch64.tar.gz").is_file())
        self.assertFalse((out / f"FARIS-{VERSION}-macos-aarch64.zip").exists())
        app, evidence, name = RELEASE.split_package(self.linux, VERSION)
        self.assertEqual(name, "linux-x86_64")
        linux_evidence = self.root / "linux-evidence.tar.gz"
        RELEASE.write_archive(self.linux, linux_evidence, f"FARIS-{VERSION}", MTIME, evidence)
        self.assertEqual((out / f"FARIS-{VERSION}-evidence.tar.gz").read_bytes(), linux_evidence.read_bytes())
        with tarfile.open(out / f"FARIS-{VERSION}-macos-aarch64.tar.gz") as tar:
            self.assertEqual(tar.getmember(f"FARIS-{VERSION}/bin/faris-app").mode, 0o755)

    def test_retargeted_linux_gives_both_archives_and_the_same_evidence_archive(self):
        package = self.target("linux-x86_64")
        out = self.root / "rel"
        run_release(package, out)
        app_name = f"FARIS-{VERSION}-linux-x86_64.tar.gz"
        self.assertTrue((out / app_name).is_file())
        _, evidence, name = RELEASE.split_package(self.linux, VERSION)
        self.assertEqual(name, "linux-x86_64")
        original_evidence = self.root / "original-evidence.tar.gz"
        RELEASE.write_archive(self.linux, original_evidence, f"FARIS-{VERSION}", MTIME, evidence)
        self.assertEqual((out / f"FARIS-{VERSION}-evidence.tar.gz").read_bytes(), original_evidence.read_bytes())
        with tarfile.open(out / app_name) as tar:
            names = tar.getnames()
            self.assertEqual(tar.getmember(f"FARIS-{VERSION}/bin/faris-app").mode, 0o755)
            index = json.load(tar.extractfile(f"FARIS-{VERSION}/package-index.json"))
        self.assertIn("desktop_build", index)
        self.assertFalse(any("port/cases" in n or n.endswith("verify.sh") for n in names))
        notes = (out / "RELEASE_NOTES.md").read_text()
        self.assertIn(f"- `{app_name}`", notes)

    def test_retargeted_linux_with_a_changed_program_is_refused(self):
        package = self.target("linux-x86_64")
        unlock(package)
        (package / "bin/faris-app").write_text("#!/bin/sh\necho faris-app 0.0.9\n")
        with self.assertRaisesRegex(SystemExit, "differs from the package index"):
            run_release(package, self.root / "rel")

    def test_app_only_appends_to_the_sums_and_refuses_duplicates(self):
        mac, win = self.target("macos-x86_64"), self.target("windows-x86_64")
        out = self.root / "rel"
        with self.assertRaisesRegex(SystemExit, "SHA256SUMS"):
            run_release(win, self.root / "no-first-run", "--app-only")
        run_release(mac, out)
        before = (out / "SHA256SUMS").read_text()
        evidence_before = (out / f"FARIS-{VERSION}-evidence.tar.gz").read_bytes()
        run_release(win, out, "--app-only")
        after = (out / "SHA256SUMS").read_text()
        self.assertTrue(after.startswith(before))
        archive = out / f"FARIS-{VERSION}-windows-x86_64.zip"
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(after[len(before):], f"{digest}  {archive.name}\n")
        self.assertEqual((out / f"FARIS-{VERSION}-evidence.tar.gz").read_bytes(), evidence_before)
        with self.assertRaisesRegex(SystemExit, "already"):
            run_release(win, out, "--app-only")
        with self.assertRaisesRegex(SystemExit, "already"):
            run_release(mac, out, "--app-only")
        self.assertEqual((out / "SHA256SUMS").read_text(), after)
        notes = (out / "RELEASE_NOTES.md").read_text()
        for name in (f"FARIS-{VERSION}-macos-x86_64.tar.gz", f"FARIS-{VERSION}-windows-x86_64.zip",
                     f"FARIS-{VERSION}-evidence.tar.gz"):
            self.assertIn(f"- `{name}`", notes)
            self.assertIn(f"SHA-256 of `{name}`", notes)
        self.assertIn("bin\\faris-app.exe", notes)

    def test_app_only_archive_has_no_evidence_file(self):
        mac, win = self.target("macos-aarch64"), self.target("windows-x86_64")
        out = self.root / "rel"
        run_release(mac, out)
        run_release(win, out, "--app-only")
        _, evidence, _ = RELEASE.split_package(win, VERSION)
        with zipfile.ZipFile(out / f"FARIS-{VERSION}-windows-x86_64.zip") as archive:
            names = set(archive.namelist())
        self.assertTrue(evidence)
        self.assertFalse(names & {f"FARIS-{VERSION}/{path}" for path in evidence})
        self.assertIn(f"FARIS-{VERSION}/package-index.json", names)

    def test_programs_that_differ_from_the_index_or_wrong_version_are_refused(self):
        package = self.target("windows-x86_64")
        index = json.loads((package / "package-index.json").read_text())
        wrong = copy.deepcopy(index)
        wrong["desktop_build"]["faris_version"] = "faris 0.1.0"
        rewrite_index(package, lambda i: i.update(desktop_build=wrong["desktop_build"]))
        with self.assertRaisesRegex(SystemExit, "faris 0.1.0"):
            run_release(package, self.root / "rel")
        rewrite_index(package, lambda i: i.update(desktop_build=index["desktop_build"]))
        (package / "bin/faris.exe").write_bytes(b"swapped")
        with self.assertRaisesRegex(SystemExit, "differs from the package index"):
            run_release(package, self.root / "rel2")


if __name__ == "__main__":
    unittest.main()
