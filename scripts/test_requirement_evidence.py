# SPDX-License-Identifier: AGPL-3.0-only
"""Checks on the repository's own configuration that requirements name as their acceptance criteria:
the unsafe-code lint, dependency pinning, the CI steps, the data and update rules, and the font licences."""
import re
import tempfile
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CI = ROOT / ".github/workflows/ci.yml"
SOURCE_SUFFIXES = {".rs", ".py", ".toml", ".md", ".yml", ".json", ".sh"}


def read(relative: str) -> str:
    return (ROOT / relative).read_text(encoding="utf-8")


def cargo(relative: str) -> dict:
    return tomllib.loads(read(relative))


def own_crates() -> dict[str, dict]:
    """{crate directory name: parsed Cargo.toml} for every member of the workspace."""
    members = cargo("Cargo.toml")["workspace"]["members"]
    return {Path(member).name: cargo(f"{member}/Cargo.toml") for member in members}


def ci_commands() -> list[str]:
    """Every `run:` command of ci.yml with continuation lines joined; block scalars are split per line."""
    commands = []
    lines = CI.read_text(encoding="utf-8").splitlines()
    index = 0
    while index < len(lines):
        match = re.match(r"^(\s*)(?:-\s+)?run:\s*(.*)$", lines[index])
        index += 1
        if not match:
            continue
        indent, rest = len(match.group(1)), match.group(2)
        if rest not in ("|", ">", "|-", ">-"):
            commands.append(rest)
            continue
        while index < len(lines) and (not lines[index].strip() or len(lines[index]) - len(lines[index].lstrip()) > indent):
            if lines[index].strip():
                commands.append(lines[index].strip())
            index += 1
    return commands


def rust_code_without_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return "\n".join(line.split("//")[0] for line in text.splitlines())


class UnsafeCodeTests(unittest.TestCase):
    def test_the_workspace_forbids_unsafe_code_and_every_crate_inherits_or_documents_its_exception(self):
        # Verifies: SEC-005, QA-035
        lints = cargo("Cargo.toml")["workspace"]["lints"]["rust"]
        self.assertEqual(lints["unsafe_code"], "forbid")
        crates = own_crates()
        inheriting = [n for n, c in crates.items() if c.get("lints", {}).get("workspace") is True]
        others = {n: c for n, c in crates.items() if n not in inheriting}
        # Only the crate that holds the OpenCASCADE bridge steps outside the workspace lint, and it denies it.
        self.assertEqual(sorted(others), ["faris-occt"])
        self.assertEqual(others["faris-occt"]["lints"]["rust"]["unsafe_code"], "deny")
        # The forbid reaches 7 of the 8 own crates (87.5 %); SEC-005 asks for 90 %, so the row is Partial.
        self.assertEqual((len(inheriting), len(crates)), (7, 8))
        # The one crate that is native code runs as a worker process; no other crate links it.
        self.assertIn("bin", others["faris-occt"])
        for name, crate in crates.items():
            if name != "faris-occt":
                self.assertNotIn("faris-occt", crate.get("dependencies", {}), name)
                self.assertNotIn("faris-occt", crate.get("build-dependencies", {}), name)

    def test_the_only_unsafe_in_the_own_code_is_the_commented_bridge_module(self):
        # Verifies: SEC-025
        keyword = re.compile(r"\bunsafe\b")
        total_lines, unsafe_lines, found = 0, 0, {}
        for path in sorted((ROOT / "crates").rglob("*.rs")):
            if "target" in path.parts:
                continue
            code = rust_code_without_comments(path.read_text(encoding="utf-8"))
            # A word inside a string literal ("an unsafe path") is a message, not the keyword.
            code = re.sub(r'"(?:[^"\\\n]|\\.)*"', '""', code)
            lines = code.splitlines()
            total_lines += len(lines)
            hits = [n for n, line in enumerate(lines, 1) if keyword.search(line)]
            if hits:
                found[path.relative_to(ROOT).as_posix()] = hits
                unsafe_lines += len(hits)
        self.assertEqual(list(found), ["crates/faris-occt/src/ffi.rs"])
        self.assertLessEqual(unsafe_lines / total_lines, 0.01)
        # Every unsafe site is explained by a comment before the module's first item.
        bridge = read("crates/faris-occt/src/ffi.rs")
        self.assertIn("this is the only module allowed to hold them", bridge.split("#[cxx::bridge")[0])
        self.assertIn("#![allow(unsafe_code)]", bridge)
        # Outside the bridge, the lint level forbids the keyword, so a new one cannot compile.
        self.assertEqual(cargo("Cargo.toml")["workspace"]["lints"]["rust"]["unsafe_code"], "forbid")

    def test_native_code_is_built_only_by_the_worker_crate_and_the_c_readers_have_fuzz_targets(self):
        # Verifies: SEC-065
        built = sorted(p.parent.name for p in (ROOT / "crates").glob("*/build.rs"))
        self.assertEqual(built, ["faris-occt"])
        self.assertTrue((ROOT / "crates/faris-occt/src/bin/faris-occt-worker.rs").is_file())
        client = cargo("crates/faris-occt-client/Cargo.toml")
        self.assertEqual(sorted(client["dependencies"]), ["serde", "serde_json", "tempfile"])
        self.assertIn("Command::new", read("crates/faris-occt-client/src/client.rs"))
        # The in-process xz and zstd decoders (C libraries behind xz2 and zip) are reached by fuzz targets.
        for target in ("study_file", "evidence_store"):
            self.assertTrue((ROOT / f"fuzz/fuzz_targets/{target}.rs").is_file(), target)


class DependencyPinningTests(unittest.TestCase):
    def test_every_workspace_dependency_is_exact_pinned_and_cargo_lock_is_committed(self):
        # Verifies: SEC-032
        dependencies = cargo("Cargo.toml")["workspace"]["dependencies"]
        external = {n: d for n, d in dependencies.items() if not (isinstance(d, dict) and "path" in d)}
        self.assertGreater(len(external), 15)
        for name, spec in external.items():
            version = spec if isinstance(spec, str) else spec.get("version", "")
            self.assertRegex(version, r"^=\d+\.\d+\.\d+$", name)
        # Member crates take every external dependency from the workspace table or pin it exactly themselves.
        loose = []
        for name, crate in own_crates().items():
            tables = [crate.get(kind, {}) for kind in ("dependencies", "dev-dependencies", "build-dependencies")]
            for target in crate.get("target", {}).values():
                tables += [target.get(kind, {}) for kind in ("dependencies", "dev-dependencies", "build-dependencies")]
            for table in tables:
                for dependency, spec in table.items():
                    if isinstance(spec, dict) and (spec.get("workspace") or "path" in spec):
                        continue
                    version = spec if isinstance(spec, str) else spec.get("version", "")
                    if not re.fullmatch(r"=\d+\.\d+\.\d+", version):
                        loose.append(f"{name}: {dependency} {version}")
        self.assertEqual(loose, [])
        self.assertTrue((ROOT / "Cargo.lock").is_file())
        self.assertIn("name = \"serde\"", read("Cargo.lock"))
        # Every cargo command that resolves or builds uses the lock file.
        resolving = [c for c in ci_commands() if re.match(r"cargo (clippy|test|doc|build|metadata|fetch)\b", c)]
        self.assertGreaterEqual(len(resolving), 5)
        for command in resolving:
            self.assertIn("--locked", command)


class CiWorkflowTests(unittest.TestCase):
    def test_ci_runs_format_clippy_and_tests_on_every_push_and_pull_request(self):
        # Verifies: QA-010
        text = CI.read_text(encoding="utf-8")
        self.assertRegex(text, r"(?m)^on:\s*\n\s+push:\s*\n\s+pull_request:")
        self.assertNotRegex(text, r"(?m)^\s+(branches|paths)(-ignore)?:")
        commands = ci_commands()
        self.assertIn("cargo fmt --all --check", commands)
        # faris-occt builds OpenCASCADE from source and has its own gate; every other crate is checked here.
        self.assertTrue(any(
            c.startswith("cargo clippy --workspace") and "--all-targets" in c and "--locked" in c
            and c.endswith("-- -D warnings") for c in commands))
        self.assertTrue(any(c.startswith("cargo test --workspace") and "--locked" in c for c in commands))
        for command in commands:
            if command.startswith(("cargo clippy", "cargo test --workspace")):
                self.assertNotRegex(command, r"--exclude (?!faris-occt\b)", command)

    def test_ci_runs_the_analytic_controls_without_openmc(self):
        # Verifies: QA-011
        commands = ci_commands()
        self.assertIn("python3 -m unittest discover -s controls -p 'test_*.py'", commands)
        # The check job installs no OpenMC; the job that does is separate.
        check_job = CI.read_text(encoding="utf-8").split("\n  openmc-tests:")[0]
        self.assertNotRegex(check_job, r"micromamba|conda|pip install[^\n]*openmc")
        tests = sorted((ROOT / "controls").glob("test_*.py"))
        self.assertGreater(len(tests), 3)
        for path in tests:
            self.assertNotRegex(path.read_text(encoding="utf-8"), r"(?m)^(import|from) openmc\b", path.name)


class DataAndUpdateRuleTests(unittest.TestCase):
    def test_no_update_check_exists_in_the_source_or_the_lock_file(self):
        # Verifies: PLAT-030
        lock = read("Cargo.lock")
        for crate in ("self_update", "update-informer", "self-replace", "cargo-update", "sparkle"):
            self.assertNotIn(f'name = "{crate}"', lock)
        # The only HTTP client in the graph is the account client's, which signs in; it is not an update check.
        packages = re.findall(r'name = "([^"]+)"', lock)
        self.assertEqual([p for p in packages if p in ("reqwest", "hyper", "curl", "isahc", "surf", "attohttpc", "minreq")], [])
        pattern = re.compile(r"update[ _-]?check|check[ _-]?for[ _-]?updates?|auto[ _-]?update|self[ _-]?update|releases/latest"
                             r"|api\.github\.com|new[ _-]version[ _-]available", re.I)
        offenders = []
        for path in sorted((ROOT / "crates").rglob("*")):
            if path.suffix in {".rs", ".toml"} and "target" not in path.parts:
                for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                    if pattern.search(line):
                        offenders.append(f"{path.relative_to(ROOT)}:{number}")
        self.assertEqual(offenders, [])

    def test_data_of_unclear_terms_is_ignored_by_git_documented_for_user_download_and_refused_by_the_packager(self):
        # Verifies: PLAT-053, LEG-011
        ignored = read(".gitignore").splitlines()
        self.assertIn("data/raw/*", ignored)
        acquisition = read("docs/PHOTON_LIBRARY_ACQUISITION.md")
        self.assertRegex(acquisition, r"provide no explicit license statement")
        self.assertRegex(acquisition, r"redistribution rights\s+remain unresolved")
        self.assertRegex(acquisition, r"not part of the source repository")
        self.assertIn("## Rebuild the local overlay", acquisition)
        self.assertIn("https://www.nndc.bnl.gov/endf-b7.1/zips/ENDF-B-VII.1-photoat.zip", acquisition)
        # The release packager and the package verifier both refuse data files.
        import package_recorded_demo as packager
        import verify_recorded_demo as verifier
        for suffix in (".h5", ".hdf5", ".endf", ".zip"):
            self.assertIn(suffix, packager.FORBIDDEN_SUFFIXES)
            self.assertIn(suffix, verifier.FORBIDDEN_SUFFIXES)
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            (root / "README.md").write_text("package", encoding="utf-8")
            self.assertEqual([r["path"] for r in packager.scan_package(root)], ["README.md"])
            (root / "photon.h5").write_bytes(b"\x89HDF")
            with self.assertRaisesRegex(RuntimeError, "external scientific data"):
                packager.scan_package(root)

    def test_the_combined_work_licence_and_the_account_client_licence_are_recorded(self):
        # Verifies: LEG-007
        self.assertEqual(cargo("Cargo.toml")["workspace"]["package"]["license"], "AGPL-3.0-only")
        self.assertIn("GNU AFFERO GENERAL PUBLIC LICENSE", read("LICENSE"))
        self.assertIn("Version 3", read("LICENSE"))
        self.assertIn("`AGPL-3.0-only`", read("README.md"))
        for name, crate in own_crates().items():
            if name != "faris-occt":
                self.assertIs(crate["package"]["license"]["workspace"], True, name)
        self.assertIn("avila-account", read("crates/faris-app/Cargo.toml"))
        notices = read("docs/THIRD_PARTY_NOTICES.md")
        self.assertIn("The combined work, FARIS together with the permissive crates it links (including avila-account), "
                      "is distributed under AGPL-3.0-only", notices)
        self.assertRegex(notices, r"\| avila-account \d+\.\d+\.\d+ \| MIT OR Apache-2\.0 \|")
        # The permissive licences of the dependencies are on the policy's allow list (LEG-003 checks the graph).
        policy = read("scripts/licence_policy.json")
        for licence in ("MIT", "Apache-2.0"):
            self.assertIn(f'"{licence}"', policy)

    def test_both_font_licences_ship_with_the_program_and_are_in_the_notices(self):
        # Verifies: LEG-015
        fonts = ROOT / "crates/faris-report/fonts"
        ubuntu, hack = (fonts / "UFL.txt").read_text(encoding="utf-8"), (fonts / "Hack-LICENSE.md").read_text(encoding="utf-8")
        self.assertIn("UBUNTU FONT LICENCE Version 1.0", ubuntu)
        self.assertIn("Hack project", hack)
        self.assertIn("MIT License", hack)
        self.assertIn("Bitstream Vera License", hack)
        squash = lambda text: re.sub(r"\s+", " ", text).strip()
        self.assertEqual(squash(read("licenses/ubuntu-font-licence-1.0.txt")), squash(ubuntu))
        self.assertEqual(squash(read("licenses/hack-font-licence.txt")), squash(hack))
        # The fonts the program embeds are these files, and the notices carry the same two texts.
        source = read("crates/faris-report/src/fonts.rs")
        self.assertIn('include_bytes!("../fonts/Ubuntu-Light.ttf")', source)
        self.assertIn('include_bytes!("../fonts/Hack-Regular.ttf")', source)
        self.assertTrue((fonts / "Ubuntu-Light.ttf").stat().st_size > 10_000)
        self.assertTrue((fonts / "Hack-Regular.ttf").stat().st_size > 10_000)
        notices = read("docs/THIRD_PARTY_NOTICES.md")
        self.assertIn("UBUNTU FONT LICENCE Version 1.0", notices)
        self.assertIn("The work in the Hack project is Copyright 2018 Source Foundry Authors", notices)
        # The release packages carry the notices and the licences folder.
        self.assertIn("THIRD_PARTY_NOTICES.md", read("scripts/package_recorded_demo.py"))
        self.assertIn("faris-THIRD_PARTY_NOTICES.md", read(".github/workflows/desktop.yml"))
        # PDF embedding of Ubuntu Light is covered by the licence's own terms: embedding in documents is not
        # restricted by it (the licence limits renaming and modified fonts, not the use of the font in a document).
        self.assertIn("embedding", read("docs/STUDY_EXPORT.md"))


class DisplayAndScratchTests(unittest.TestCase):
    def test_every_place_that_shows_the_accumulated_fluence_names_it_a_proxy(self):
        # Verifies: MAT-019
        shown = {
            "crates/faris-app/src/lib.rs": ("neutron fluence (a transport proxy, not dpa or life) follows the timeline",
                                            "accumulated neutron fluence (proxy, not dpa or life)"),
            "crates/faris-app/src/transport_panel.rs": ("accumulated fluence (a transport proxy, not dpa or life)",),
            "crates/faris-app/src/history_panel.rs": ("(flux × time, a transport proxy, not dpa)",),
            "crates/faris-report/src/charts.rs": ("FLUENCE_PROXY_NOTE",),
        }
        for path, phrases in shown.items():
            text = read(path)
            for phrase in phrases:
                self.assertIn(phrase, text, path)
        note = re.search(r'FLUENCE_PROXY_NOTE: &str =\s*"([^"]+)"', read("crates/faris-model/src/lib.rs")).group(1)
        self.assertIn("proxy", note)
        self.assertIn("not dpa", note)

    # Verifies: PLAT-029
    def test_only_small_probe_folders_use_the_system_temporary_folder(self):
        # A scratch folder made with tempfile::tempdir() or Builder::tempdir() lands in the system temporary
        # folder. The large ones (an unpacked study, a CAD conversion) use faris_engine::scratch instead; the
        # remaining uses hold a request file, a probe or a staging file beside its target.
        small = {
            "crates/faris-app/src/tool_finder.rs": 1,
            "crates/faris-cli/src/adapters.rs": 1,
            "crates/faris-occt-client/src/client.rs": 1,
            "crates/faris-engine/src/scratch.rs": 1,
            "crates/faris-engine/src/vfs.rs": 1,
        }
        found = {}
        for path in sorted((ROOT / "crates").rglob("*.rs")):
            relative = path.relative_to(ROOT).as_posix()
            if "/tests/" in relative or path.name.endswith("tests.rs") or "tests" in path.parts:
                continue
            text = path.read_text(encoding="utf-8").split("#[cfg(test)]")[0].split("#[cfg(all(test")[0]
            text = "\n".join(line for line in text.splitlines() if not line.strip().startswith("//"))
            count = len(re.findall(r"tempfile::tempdir\(\)|\.tempdir\(\)|tempfile::Builder::new\(\)\.[^;]*?\.tempdir\(\)", text))
            if count:
                found[relative] = count
        self.assertEqual(found, small)
        for path in ("crates/faris-study/src/read.rs", "crates/faris-cli/src/study_export.rs",
                     "crates/faris-engine/src/design/geometry.rs"):
            self.assertIn("scratch::tempdir", read(path), path)


if __name__ == "__main__":
    unittest.main()
