#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Runs the code samples of the handbook and writes the gate evidence (requirement QA-016).

    python3 scripts/doc_samples.py --faris target/debug/faris --evidence doc-tests.json [--cargo CMD]

Scans the fenced blocks of README.md, docs/*.md and docs/guide/*.md, plus the `faris ...` commands in
backticks in the prose, and the code blocks in rustdoc comments. Every one gets a disposition, and a block
with none is a failure, so a new sample cannot slip in unchecked:

- EXECUTED   a `faris` command run with the debug binary on fixtures in a temporary folder; it must exit 0.
             The commands of a block share one folder, in order.
- CHECKED    a command that needs OpenMC, ACTINV, a CAD Python, Avila Core or a long run, or a path the reader
             fills in. It is not run. Its subcommand path and every `--flag` are checked against the binary's
             own help, a script it names must exist and mention each flag, and a repository path must exist.
- JSON       a JSON example of a file format, parsed by the real parser in the Rust test
             crates/faris-engine/tests/doc_json_samples.rs, which this script runs with --cargo.
- PYTHON     a Python snippet, parsed with ast.
- FRAGMENT   a diagram, a formula or a fragment that is not a whole file; its field names are checked against
             the source they describe.
- LISTING    a generated licence listing (scripts/collect_dependency_notices.py has its own test).
- SKIPPED    the program is not FARIS and runs outside it (the desktop, a download, tar, the build). The reasons
             are in SKIP_PROGRAMS below; the list is deliberately short.

A sample may be marked as expected to fail only in EXPECTED_FAIL, with a written reason. It is empty.

Fixtures are synthetic: transport bundles with placeholder members and the recorded transport rates of the
engine's determinism test. They make the commands run; they say nothing about physics.

The evidence file is JSON {"commit": SHA, "checks": [{"name", "verdict", ...}], "summary": {...}} as
scripts/release_gate.py reads it for --doc-tests. A check is PASS only when its disposition holds; any other
outcome is FAIL and exits 1.
"""
from __future__ import annotations

import argparse
import ast
import json
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter
from concurrent.futures import Future, ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent

SCANNED = ["README.md", "docs/*.md", "docs/guide/*.md"]
LISTING_FILES = {"docs/THIRD_PARTY_NOTICES.md", "docs/CORE_RUNTIME_DEPENDENCY_NOTICES.md"}
TIMEOUT_S = 120

# Programs that are not FARIS, and why a sample that starts with one is not run here.
SKIP_PROGRAMS = {
    "app": "the desktop app needs a display and is not built in the command-line job (the desktop workflow builds it)",
    "build": "cargo build, format, lint and test commands: CI runs these as the steps before this one",
    "package": "unpacks or verifies a downloaded release archive; the release-smoke workflow runs that on a real release",
    "download": "downloads data from an outside site; no network use in CI",
    "assign": "sets the path of the reader's own OpenMC; nothing to run",
}

# (file, first words of the command) -> reason. Empty: no sample is expected to fail.
EXPECTED_FAIL: dict[tuple[str, str], str] = {}

# Commands that take half a minute in a debug build: identical invocations are run once.
SLOW = {("history", "ensemble")}

# faris leaves that cannot run here, and why.
NEEDS = {
    ("reactor", "run"): "needs the reader's OpenMC, a Python and nuclear data, and a long run",
    ("reactor", "inspect"): "needs a saved transport run (run.json), which only a transport run writes",
    ("control", "absorber"): "needs OpenMC",
    ("evidence", "prepare"): "needs Avila Core and a transport run",
    ("evidence", "run"): "needs Avila Core",
    ("evidence", "inspect"): "needs an evidence store from the recorded package",
    ("study", "compile"): "needs an Avila Core executable",
    ("design", "init"): "needs a Python with cadquery and a STEP model",
    ("design", "check"): "needs a Python with cadquery and a STEP model",
    ("design", "transport"): "needs OpenMC with DAGMC and a CAD model",
    ("maintenance", "run"): "needs ACTINV and its data",
    ("maintenance", "report"): "needs a result file that `maintenance run` writes",
    ("history", "from-run"): "needs a saved transport run (run.json)",
    ("transport", "pack"): "needs a saved transport run (run.json)",
    ("transport", "normalize"): "needs a raw solver artifact",
    ("study-file", "export"): "needs bundles that hold real transport results; the fixture bundles carry placeholder members",
    ("transport", "validate-request"): "needs a transport request file written for the reader's scenario",
}

# Placeholder names the handbook uses for files, mapped to the fixture that stands in. Used when a
# command is run.
FIXTURE_NAMES = {"A.json": "assumptions", "R.json": "rates", "rates.json": "rates"}
OUTPUT_FLAGS = {"--output", "--output-dir", "--out", "-o", "--provenance-out", "--rates-output", "--interface-check-output", "--report"}
REPO_TOPS = ("scenarios", "references", "scripts", "integrations", "controls", "validation", "docs", "tools")

# Illustrative blocks: key -> (kind, source files that must contain each name). The key is the
# file and the first line of the block.
FRAGMENTS = {
    ("docs/ARCHITECTURE.md", "scenario JSON"): ("diagram", [], []),
    ("docs/COLD_REFERENCE.md", "python reactor_transport.py --input input.json --output-dir solver"): (
        "command of a generated script, written into each run folder by `faris reactor run`", [], []),
    ("docs/OPERATING_HISTORY.md", "available + in_process"): (
        "formula (the ledger identity in words; its terms are prose names, not field names)", [], []),
    ("docs/STUDY_EXPORT.md", "faris_report::export_study(&ReportInput { study_file: Option<StudyFileStamp>,"): (
        "signature outline", ["export_study", "ReportInput", "study_file", "view_image", "StudyFileStamp"], ["crates/faris-report/src"]),
    ("docs/STUDY_FILE.md", '"layers": {'): (
        "JSON outline with `|` between alternatives", ["evidence_store", "mode", "trees", "arrangement", "allocation",
                                                        "kind", "tree", "files", "path", "sha256", "bytes"], ["crates/faris-study/src"]),
}

# JSON blocks of a file format: file -> the Rust test names (in doc_json_samples.rs) that parse them.
JSON_TESTS = {
    "docs/guide/design-file.md": ["design_file_waivers_example_parses_and_names_waivable_checks"],
    "docs/guide/maintenance.md": ["maintenance_examples_parse_and_validate"],
}
RUST_JSON_TEST = ("faris-engine", "doc_json_samples")
DOCTESTED = {"faris-model", "faris-engine", "faris-study", "faris-report", "faris-occt-client"}


# --- extraction -----------------------------------------------------------------------------------

FENCE = re.compile(r"^(```+)([^\n]*)\n(.*?)^\1[ \t]*$", re.M | re.S)
INLINE = re.compile(r"`(faris [^`\n]+)`")


class Block:
    def __init__(self, path: str, line: int, lang: str, text: str):
        self.path, self.line, self.lang, self.text = path, line, lang.strip(), text

    @property
    def first(self) -> str:
        return next((l.strip() for l in self.text.splitlines() if l.strip()), "")

    @property
    def name(self) -> str:
        return f"{self.path}:{self.line}"


def scanned_files(root: Path) -> list[str]:
    found: list[str] = []
    for pattern in SCANNED:
        found += sorted(str(p.relative_to(root)) for p in root.glob(pattern))
    return found


def extract(root: Path, relative: str) -> tuple[list[Block], list[tuple[int, str]]]:
    """The fenced blocks of a file, and the `faris ...` spans in its prose (outside fences)."""
    text = (root / relative).read_text(encoding="utf-8")
    blocks = [Block(relative, text.count("\n", 0, m.start()) + 1, m.group(2), m.group(3)) for m in FENCE.finditer(text)]
    prose = FENCE.sub(lambda m: "\n" * m.group(0).count("\n"), text)
    spans = [(prose.count("\n", 0, m.start()) + 1, m.group(1)) for m in INLINE.finditer(prose)]
    return blocks, spans


def rustdoc_blocks(root: Path) -> list[Block]:
    """Code blocks in /// and //! comments of the crates (rustdoc examples)."""
    found: list[Block] = []
    for path in sorted((root / "crates").glob("*/src/**/*.rs")):
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        i = 0
        while i < len(lines):
            m = re.match(r"\s*//[/!]\s?(```+)(.*)$", lines[i])
            if m:
                body, j = [], i + 1
                while j < len(lines) and not re.match(r"\s*//[/!]\s?```", lines[j]):
                    body.append(re.sub(r"^\s*//[/!] ?", "", lines[j]))
                    j += 1
                found.append(Block(str(path.relative_to(root)), i + 1, m.group(2), "\n".join(body) + "\n"))
                i = j
            i += 1
    return found


def commands(block: Block) -> list[str]:
    """The commands of a shell block: continuations joined, comments and optional [parts] removed."""
    out: list[str] = []
    pending = ""
    for raw in block.text.splitlines():
        line = raw.rstrip()
        if not pending and (not line.strip() or line.lstrip().startswith("#")):
            continue
        if line.endswith("\\"):
            pending += (line[:-1].strip() + " ") if pending else (line[:-1].rstrip() + " ")
            continue
        full = (pending + (line.strip() if pending else line)).strip()
        pending = ""
        full = re.sub(r"\s+#\s.*$", "", full)  # trailing comment (a `#` after space)
        full = re.sub(r"\s*\[[^\]]*\]", "", full)  # optional parts: [--pack-evidence], [--workers 2]
        if full:
            out.append(full)
    if pending.strip():
        out.append(pending.strip())
    return out


def tokens(command: str) -> list[str]:
    try:
        return shlex.split(command)
    except ValueError:
        return command.split()


# --- the binary -----------------------------------------------------------------------------------

class Binary:
    """The faris binary and its help, cached per subcommand path."""

    def __init__(self, path: Path):
        self.path = path
        self.lock = threading.Lock()
        self.memo: dict = {}
        self._help: dict[tuple[str, ...], str | None] = {}

    def help(self, words: tuple[str, ...]) -> str | None:
        if words not in self._help:
            run = subprocess.run([str(self.path), *words, "--help"], capture_output=True, text=True, timeout=TIMEOUT_S)
            self._help[words] = run.stdout if run.returncode == 0 else None
        return self._help[words]

    def children(self, words: tuple[str, ...]) -> list[str]:
        text = self.help(words) or ""
        m = re.search(r"^Commands:\n((?:  .*\n?)+)", text, re.M)
        return [l.split()[0] for l in m.group(1).splitlines() if l.strip()] if m else []

    def leaf(self, args: list[str]) -> tuple[tuple[str, ...], list[str]]:
        """The subcommand path at the front of `args` and the rest."""
        words: tuple[str, ...] = ()
        rest = list(args)
        while rest and rest[0] in self.children(words):
            words += (rest.pop(0),)
        return words, rest


def check_flags(binary: Binary, words: tuple[str, ...], rest: list[str]) -> list[str]:
    """Problems with the subcommand path and the flags of a faris command."""
    text = binary.help(words)
    if text is None:
        return [f"`faris {' '.join(words)}` is not a command of this binary"]
    problems = []
    if not words and rest and not rest[0].startswith("-"):
        problems.append(f"`{rest[0]}` is not a command of this binary")
    for token in rest:
        flag = token.split("=")[0]
        if re.fullmatch(r"--?[A-Za-z][\w-]*", flag) and not re.search(rf"(?<![\w-]){re.escape(flag)}(?![\w-])", text):
            problems.append(f"{flag} is not a flag of `faris {' '.join(words)}`")
    return problems


def check_repo_paths(root: Path, args: list[str]) -> list[str]:
    problems = []
    previous = ""
    for token in args:
        value = token.split("=", 1)[1] if token.startswith("--") and "=" in token else token
        if previous not in OUTPUT_FLAGS and value.split("/")[0] in REPO_TOPS and "/" in value and not re.search(r"[<*>]", value):
            if not (root / value).exists():
                problems.append(f"{value} does not exist in the repository")
        previous = token
    return problems


def mentions(source: str, flag: str) -> bool:
    """The flag is spelled in the script, or its bare name is quoted (parser.add_argument(f"--{name}") in a loop)."""
    name = flag[2:]
    return flag in source or f'"{name}"' in source or f"'{name}'" in source


def check_script(root: Path, args: list[str]) -> list[str]:
    """A python command: the script exists in the repository and mentions each flag."""
    if not args:
        return []
    if args[0] == "-m":
        module = args[1] if len(args) > 1 else ""
        return [] if (root / module).is_dir() else [f"module {module} is not a folder of the repository"]
    script = args[0]
    if not (root / script).is_file():
        return [f"{script} does not exist in the repository"]
    source = (root / script).read_text(encoding="utf-8", errors="replace")
    problems = [f"{t.split('=')[0]} is not mentioned in {script}" for t in args[1:]
                if re.fullmatch(r"--[A-Za-z][\w-]*(=.*)?", t) and not mentions(source, t.split("=")[0])]
    return problems + check_repo_paths(root, args[1:])


def program_kind(first: str, args: list[str]) -> str:
    """`faris`, `python`, or the key of a SKIP_PROGRAMS entry."""
    name = Path(first).name
    if first.startswith(("OPENMC_PY=", "OPENMC_EXE=")):
        return "assign"
    if name == "faris":
        return "faris"
    if name in ("faris-app",):
        return "app"
    if name == "cargo":
        if args[:1] == ["run"]:
            if "faris-app" in args:
                return "app"
            return "faris"
        return "build"
    if first.startswith("scripts/") and name.endswith(".sh"):
        return "shell-script"
    if name.startswith("python") or first in ('"$OPENMC_PY"', "$OPENMC_PY", "/path/to/openmc-python"):
        return "python"
    if name in ("sha256sum", "tar", "xattr", "md5sum"):
        return "package"
    if name in ("curl", "printf"):
        return "download"
    if name == "mkdir":
        return "mkdir"
    return "unknown"


# --- fixtures -------------------------------------------------------------------------------------

def bundle(tag: str) -> str:
    members = ["run.json", "input.json", "scenario.json", "audit.json", "reactor_transport.py", "solver/transport-artifact.json"]
    files = {m: ('{"shared":true}\n' if m == "scenario.json" else json.dumps({m: tag}) + "\n") for m in members}
    return json.dumps({"schema_version": "faris-recorded-transport-bundle/v0.1", "files": files, "notice": "documentation-sample fixture"})


def make_workspace(root: Path, work: Path, faris: Path, with_study: bool) -> None:
    """The folder a block runs in: repository inputs by symlink, synthetic bundles, rates, a .faris file."""
    for name in ("scenarios", "references"):
        (work / name).symlink_to(root / name)
    (work / "runs").mkdir()
    (work / "parent").mkdir()
    shutil.copy(root / "crates/faris-engine/tests/fixtures/determinism_rates.json", work / "rates.json")
    (work / "assumptions.json").write_text((root / "scenarios/arc-inspired/demountable-magnet-assumptions.json").read_text())
    (work / "operating-assumptions.json").write_text('{"fixture": true}\n')
    for relative, tag in (("port/bundles/reference", "a"), ("control/bundles/reference", "b"), ("sweep/blanket-045cm", "c")):
        path = work / f"{relative}.transport-bundle.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(bundle(tag))
    if with_study:
        run = subprocess.run([str(faris), "study-file", "create", "--bundle", "port/bundles/reference.transport-bundle.json",
                              "--control-bundle", "control/bundles/reference.transport-bundle.json",
                              "--sweep-bundle", "sweep/blanket-045cm.transport-bundle.json",
                              "--assumptions", "operating-assumptions.json", "-o", "demo.faris"],
                             cwd=work, capture_output=True, text=True, timeout=TIMEOUT_S)
        if run.returncode != 0:
            raise RuntimeError(f"could not make the fixture study file: {run.stderr.strip()}")


SAMPLES_RUN = "4"

# Options dropped when a command is run, with the reason (the file they name is a large recorded asset).
DROPPED = {"--evidence": "the saved evidence archive is a recorded asset that is not in the repository",
           "--pack-evidence": "it only applies with --evidence"}


def prepare(tokens_: list[str], work: Path) -> list[str]:
    """The arguments to run: handbook placeholder names mapped to fixtures, /tmp and /path/to/parent into the folder."""
    out: list[str] = []
    skip = False
    for i, token in enumerate(tokens_):
        if skip:
            skip = False
            continue
        if i and tokens_[i - 1] == "--samples":
            token = SAMPLES_RUN  # the debug binary takes minutes over the documented 200 draws
        if token in DROPPED:
            skip = token == "--evidence"
            continue
        token = token.replace("/tmp/", f"{work}/tmp/").replace("/path/to/parent", str(work / "parent"))
        if token in ("A.json",):
            token = "assumptions.json"
        elif token == "R.json":
            token = "rates.json"
        elif token == "E.json":
            token = "E-ensemble.json"
        out.append(token)
    (work / "tmp").mkdir(exist_ok=True)
    return out


# --- the checks -----------------------------------------------------------------------------------

class Results:
    def __init__(self):
        self.checks: list[dict] = []

    def add(self, name: str, kind: str, ok: bool, detail: str = ""):
        entry = {"name": name, "kind": kind, "verdict": "PASS" if ok else "FAIL"}
        if detail:
            entry["detail"] = detail
        self.checks.append(entry)

    def failures(self):
        return [c for c in self.checks if c["verdict"] != "PASS"]


def run_command(binary: Binary, root: Path, work: Path, where: str, command: str, results: Results, file: str):
    toks = tokens(command)
    if toks[0] == "eval" and len(toks) > 1 and toks[1].startswith("$("):  # eval "$(command ...)": the command inside
        toks = tokens(" ".join(toks[1:]).removeprefix("$(").removesuffix(")"))
    first = toks[0]
    kind = program_kind(first, toks[1:])
    label = f"{where} `{' '.join(toks[:4])}{' ...' if len(toks) > 4 else ''}`"
    if kind == "unknown":
        results.add(label, "unknown", False, "no rule for this program: add it to the runner with a reason")
        return
    if kind == "mkdir":  # runs in the block's folder; /tmp/ is mapped into it
        for target in [t for t in prepare(toks[1:], work) if not t.startswith("-")]:
            Path(target if Path(target).is_absolute() else work / target).mkdir(parents=True, exist_ok=True)
        results.add(label, "executed", True, "folders made in the block's temporary folder")
        return
    if kind in SKIP_PROGRAMS:
        results.add(label, "skipped", True, SKIP_PROGRAMS[kind])
        return
    if kind == "shell-script":
        problems = check_script(root, toks)
        results.add(label, "checked", not problems, "; ".join(problems) or "the script exists and each flag is spelled in it; not run (it builds the release binary twice)")
        return
    if kind == "python" and toks[1:3] == ["-m", "unittest"]:
        results.add(label, "skipped", True, SKIP_PROGRAMS["build"])
        return
    if kind == "python":
        problems = check_script(root, toks[1:])
        how = "the script exists and each flag is spelled in it; not run (the reader's own interpreter)"
        if not problems and Path(first).name.startswith("python") and toks[1:2] and not toks[1].startswith("-"):
            # A plain interpreter: ask the script for its own help and check the flags there.
            with tempfile.TemporaryDirectory(prefix="doc-samples-help-") as tmp:
                run = subprocess.run([sys.executable, str(root / toks[1]), "--help"], cwd=tmp, capture_output=True, text=True, timeout=60)
            if run.returncode == 0:
                problems = [f"{t} is not in the help of {toks[1]}" for t in toks[2:]
                            if re.fullmatch(r"--[A-Za-z][\w-]*", t) and not re.search(rf"(?<![\w-]){re.escape(t)}(?![\w-])", run.stdout)]
                how = "ran --help; every flag is in it"
            else:
                how = f"flags are spelled in the script; --help did not run ({run.stderr.strip().splitlines()[-1][:80] if run.stderr.strip() else 'exit ' + str(run.returncode)})"
        elif not problems and toks[1:2] == ["-m"]:
            how = "the module folder exists; it is run at release time"
        results.add(label, "checked", not problems, "; ".join(problems) or how)
        return
    # a faris command, or `cargo run -- COMMAND` which runs the same binary
    args = toks[toks.index("--") + 1:] if first.endswith("cargo") and "--" in toks else toks[1:]
    words, rest = binary.leaf(args)
    problems = check_flags(binary, words, rest) + check_repo_paths(root, rest)
    placeholder = any(re.search(r"<[^>]*>|/path/to/", t.replace("/path/to/parent", "")) for t in rest)
    reason = NEEDS.get(words[:2]) if len(words) >= 2 else None
    key = (file, " ".join(toks[:3]))
    if key in EXPECTED_FAIL:
        results.add(label, "expected-fail", True, EXPECTED_FAIL[key])
        return
    if problems or reason or placeholder:
        why = "; ".join(problems) if problems else (reason or "a path the reader fills in (a placeholder)")
        results.add(label, "checked", not problems, why if problems else f"flags checked against the binary; not run: {why}")
        return
    argv = prepare(list(words) + rest, work)
    key = None
    if words in SLOW:
        # The same invocation (apart from where it writes) is run once; the others wait for its verdict.
        key = tuple(a for i, a in enumerate(argv) if a not in OUTPUT_FLAGS and (i == 0 or argv[i - 1] not in OUTPUT_FLAGS))
        with binary.lock:
            earlier = binary.memo.get(key)
            if earlier is None:
                binary.memo[key] = earlier = Future()
                key = (key, earlier)
            else:
                key = None
        if key is None:
            outcome = earlier.result()
            results.add(label, "executed", outcome[0], f"same arguments as {outcome[1]}, which was run once: " + outcome[2])
            return
    started = time.monotonic()
    try:
        run = subprocess.run([str(binary.path), *argv], cwd=work, capture_output=True, text=True, timeout=TIMEOUT_S)
        ok = run.returncode == 0
        detail = f"exit {run.returncode}, {time.monotonic() - started:.1f} s" + ("" if ok else f": {(run.stderr or run.stdout).strip()[:300]}")
    except subprocess.TimeoutExpired:
        ok, detail = False, f"did not finish in {TIMEOUT_S} s"
    if key is not None:
        key[1].set_result((ok, label, detail))
    results.add(label, "executed", ok, detail)


def run_shell_block(binary: Binary, root: Path, block: Block, results: Results):
    cmds = commands(block)
    if not cmds:
        results.add(block.name, "unknown", False, "no command found in a shell block")
        return
    needs_study = not any("study-file create" in c for c in cmds)
    with tempfile.TemporaryDirectory(prefix="doc-samples-") as tmp:
        work = Path(tmp)
        make_workspace(root, work, binary.path, needs_study)
        for index, command in enumerate(cmds, 1):
            run_command(binary, root, work, f"{block.name}#{index}", command, results, block.path)


def check_inline(binary: Binary, root: Path, path: str, line: int, span: str, results: Results):
    toks = tokens(span)[1:]
    if not toks or not re.match(r"[a-z]", toks[0]):
        return  # `faris --version`, `faris COMMAND --help`: nothing to resolve
    words, rest = binary.leaf(toks)
    name = f"{path}:{line} `{span[:60]}`"
    if not words:
        results.add(name, "inline", False, f"`{toks[0]}` is not a faris command")
        return
    problems = check_flags(binary, words, rest)
    results.add(name, "inline", not problems, "; ".join(problems) or "command and flags exist")


def check_fragment(root: Path, block: Block, results: Results):
    spec = FRAGMENTS.get((block.path, block.first))
    if spec is None:
        results.add(block.name, "unknown", False, f"no disposition for this {block.lang or 'plain'} block; add it to FRAGMENTS (with its field names) or make it a runnable sample")
        return
    what, names, sources = spec
    text = ""
    for source in sources:
        target = root / source
        files = [target] if target.is_file() else sorted(target.rglob("*.rs"))
        text += "\n".join(f.read_text(encoding="utf-8", errors="replace") for f in files)
    missing = [n for n in names if not re.search(rf"\b{re.escape(n)}\b", text)]
    results.add(block.name, "fragment", not missing,
                f"{what}; " + (f"not found in {', '.join(sources)}: {', '.join(missing)}" if missing else f"{len(names)} names found in the source" if names else "not executable"))


def run_rust(cargo: list[str], root: Path, results: Results, tests_needed: list[str], doc: bool):
    """cargo test for the JSON samples (and the library doc tests); the evidence is the test names."""
    package, test = RUST_JSON_TEST
    run = subprocess.run([*cargo, "test", "-p", package, "--test", test, "--locked"], cwd=root, capture_output=True, text=True, timeout=1500)
    out = run.stdout + run.stderr
    seen = dict(re.findall(r"^test (\S+) \.\.\. (\w+)$", out, re.M))
    for name in tests_needed:
        results.add(f"{test}::{name}", "json-test", seen.get(name) == "ok", seen.get(name) or f"the test did not run (exit {run.returncode}): {out[-300:].strip()}")
    if doc:
        libs = [x for p in sorted(DOCTESTED) for x in ("-p", p)]
        run = subprocess.run([*cargo, "test", "--doc", *libs, "--locked"], cwd=root, capture_output=True, text=True, timeout=1500)
        out = run.stdout + run.stderr
        passed = sum(int(n) for n in re.findall(r"(\d+) passed", out))
        results.add("cargo test --doc (library crates)", "rustdoc", run.returncode == 0,
                    f"{passed} rustdoc examples passed" + ("" if run.returncode == 0 else f"; exit {run.returncode}: {out[-300:].strip()}"))


def git_head(root: Path) -> str | None:
    run = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"], capture_output=True, text=True)
    return run.stdout.strip() or None if run.returncode == 0 else None


def run_all(root: Path, faris: Path, cargo: list[str] | None, doc_tests: bool = True) -> Results:
    results = Results()
    binary = Binary(faris)
    json_needed: list[str] = []
    shell: list[Block] = []
    for relative in scanned_files(root):
        blocks, spans = extract(root, relative)
        for line, span in spans:
            check_inline(binary, root, relative, line, span, results)
        for block in blocks:
            lang = block.lang
            if relative in LISTING_FILES:
                results.add(block.name, "listing", lang == "text", "generated licence listing, checked by scripts/test_collect_dependency_notices.py" if lang == "text" else "unexpected language in a listing file")
            elif lang == "json" and (block.path, block.first) not in FRAGMENTS:
                tests = JSON_TESTS.get(block.path)
                if tests is None:
                    results.add(block.name, "unknown", False, "a JSON example with no parser check: add its test to doc_json_samples.rs and JSON_TESTS")
                else:
                    json_needed += [t for t in tests if t not in json_needed]
                    results.add(block.name, "json", True, "parsed by " + ", ".join(tests))
            elif lang == "python":
                try:
                    ast.parse(block.text)
                    results.add(block.name, "python", True, "parses")
                except SyntaxError as e:
                    results.add(block.name, "python", False, f"syntax error: {e}")
            elif lang in ("", "bash", "sh") and not (relative, block.first) in FRAGMENTS:
                shell.append(block)
            else:
                check_fragment(root, block, results)
    with ThreadPoolExecutor(max_workers=2) as pool:
        for future in [pool.submit(run_shell_block, binary, root, b, results) for b in shell]:
            future.result()
    for block in rustdoc_blocks(root):
        crate = block.path.split("/")[1]
        tested = block.lang != "text" and crate in DOCTESTED
        ok = block.lang == "text" or tested
        results.add(block.name, "rustdoc-block", ok, "a text block, not compiled" if block.lang == "text" else
                    "compiled and run by cargo test --doc" if tested else f"a code block in {crate}, which cargo test --doc does not cover")
    if cargo is not None:
        run_rust(cargo, root, results, json_needed, doc_tests)
    else:
        results.add("doc_json_samples (Rust test)", "json-test", False, "not run: pass --cargo so the JSON examples are parsed by the real parsers")
    for key, why in EXPECTED_FAIL.items():
        if not why.strip():
            results.add(f"expected failure {key}", "expected-fail", False, "an expected failure needs a written reason")
    return results


def summary(results: Results) -> dict:
    kinds = Counter(c["kind"] for c in results.checks)
    return {"checks": len(results.checks), "failed": len(results.failures()), "by_kind": dict(sorted(kinds.items()))}


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--faris", type=Path, default=ROOT / "target" / "debug" / "faris", help="the debug faris binary")
    ap.add_argument("--evidence", type=Path, help="write the gate evidence (release_gate.py --doc-tests) here")
    ap.add_argument("--cargo", help="the cargo command, for the Rust JSON test and rustdoc tests (default: do not run them, which fails)")
    ap.add_argument("--root", type=Path, default=ROOT)
    args = ap.parse_args(argv)
    if not args.faris.is_file():
        print(f"doc_samples: {args.faris} does not exist; build it with `cargo build -p faris-cli`", file=sys.stderr)
        return 2
    results = run_all(args.root, args.faris.resolve(), shlex.split(args.cargo) if args.cargo else None)
    for c in results.checks:
        if c["verdict"] != "PASS":
            print(f"FAIL  {c['name']}: {c.get('detail', '')}")
    s = summary(results)
    print(f"doc samples: {s['checks'] - s['failed']} of {s['checks']} checks pass; " + ", ".join(f"{k} {v}" for k, v in s["by_kind"].items()))
    if args.evidence:
        commit = git_head(args.root)
        args.evidence.write_text(json.dumps({"commit": commit, "checks": results.checks, "summary": s}, indent=1) + "\n", encoding="utf-8")
    return 1 if results.failures() else 0


if __name__ == "__main__":
    sys.exit(main())
