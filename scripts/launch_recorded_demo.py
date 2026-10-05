#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Materialize bounded Core evidence archives for the lifetime of the GUI."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
sys.dont_write_bytecode = True
from recorded_archives import extract_indexed_trees
from verify_recorded_demo import verify_index, sweep_bundle_paths


def split_app_override(arguments: list[str]) -> tuple[str | None, list[str]]:
    """Remove a development `--app PATH` from the arguments passed to the GUI."""
    override = None
    remaining: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--app" or argument.startswith("--app="):
            if override is not None:
                raise ValueError("--app must be supplied only once")
            if argument == "--app":
                if index + 1 >= len(arguments) or arguments[index + 1].startswith("--"):
                    raise ValueError("--app requires a path value")
                override = arguments[index + 1]
                index += 2
            else:
                override = argument.split("=", 1)[1]
                index += 1
            if not override:
                raise ValueError("--app requires a path value")
            continue
        remaining.append(argument)
        index += 1
    return override, remaining


def main() -> int:
    if len(sys.argv) < 2:
        raise ValueError("usage: launch_recorded_demo.py PACKAGE_ROOT [APP_ARGUMENTS...]")
    root = Path(sys.argv[1]).resolve(strict=True)
    app_override, app_arguments = split_app_override(sys.argv[2:])
    runs_override = None
    index = 0
    while index < len(app_arguments):
        argument = app_arguments[index]
        if argument == "--runs-directory":
            if runs_override is not None or index + 1 >= len(app_arguments):
                raise ValueError("--runs-directory must be supplied once with a path")
            runs_override = app_arguments[index + 1]
            if runs_override.startswith("--"):
                raise ValueError("--runs-directory requires a path value")
            index += 2
            continue
        if argument.startswith("--runs-directory="):
            if runs_override is not None:
                raise ValueError("--runs-directory must be supplied only once")
            runs_override = argument.split("=", 1)[1]
            if not runs_override:
                raise ValueError("--runs-directory requires a path value")
        index += 1
    if runs_override is None:
        state_home = os.environ.get("XDG_STATE_HOME")
        state_root = Path(state_home) if state_home else Path.home() / ".local" / "state"
        if not state_root.is_absolute():
            raise ValueError("XDG_STATE_HOME must be an absolute path")
        runs_directory = state_root / "faris" / "recorded-demo-runs" / root.name
        app_arguments.extend(("--runs-directory", str(runs_directory)))
    else:
        runs_directory = Path(runs_override)
        if not runs_directory.is_absolute():
            runs_directory = Path.cwd() / runs_directory
    runs_directory = runs_directory.resolve(strict=False)
    if runs_directory == root or root in runs_directory.parents:
        raise ValueError("--runs-directory must resolve outside the read-only distribution")
    faris, core, app = (root / "bin/faris", root / "bin/avila-core", root / "bin/faris-app")
    index, _ = verify_index(root, faris, core)
    sweep_bundles = sweep_bundle_paths(root, index)
    if not sweep_bundles:
        print("FARIS demo launcher: this package contains no allocation sweep.",
              file=sys.stderr, flush=True)
    if app_override is not None:
        app = Path(app_override).resolve(strict=True)
        if not app.is_file() or not os.access(app, os.X_OK):
            raise ValueError("--app must name an executable file")
        print("FARIS demo launcher: development app binary: not covered by the package index "
              f"({app})", file=sys.stderr, flush=True)
    expanded_bytes = index.get("expanded_case_workspace_bytes")
    directory_count = index.get("expanded_case_workspace_directory_count")
    if (not isinstance(expanded_bytes, int) or expanded_bytes < 0
            or not isinstance(directory_count, int) or directory_count < 0):
        raise ValueError("package has no bounded expanded payload/directory totals")
    temporary_parent = Path(tempfile.gettempdir())
    block_bytes = max(4096, os.statvfs(temporary_parent).f_frsize)
    required = expanded_bytes + (directory_count + 16) * block_bytes + 64 * 1024 * 1024
    free = shutil.disk_usage(temporary_parent).free
    if free < required:
        raise OSError(f"need {required} bytes free to materialize saved Core cases; have {free}")
    with tempfile.TemporaryDirectory(prefix="faris-recorded-demo-", dir=temporary_parent) as temp_name:
        extracted_root = Path(temp_name)
        materialized = extracted_root / "materialized"
        descriptors = []
        for pair_id, variant in (("port", "reference"), ("port", "breeder-emphasis"),
                                 ("control", "reference"), ("control", "breeder-emphasis")):
            descriptor = extracted_root / f"saved-study-{pair_id}-{variant}.json"
            case_rel = Path("materialized") / pair_id / "cases" / variant
            workspace_rel = Path("materialized") / pair_id / "core-workspaces" / variant
            descriptor.write_text(json.dumps({
                "case_directory": case_rel.as_posix(),
                "execution_report": (case_rel / "execution-report.json").as_posix(),
                "execution_workspace": workspace_rel.as_posix(),
            }, indent=2) + "\n", encoding="utf-8")
            descriptors.append(descriptor)
        marker = extracted_root / "materialization.json"
        started = time.monotonic()
        cancelled = threading.Event()
        extraction_errors: list[BaseException] = []

        def publish_marker(status: str, error: str | None = None) -> None:
            value = {"schema_version": "faris-recorded-materialization/v0.1", "status": status}
            if error:
                detail = error.encode("utf-8", "replace")[:1024]
                while True:
                    try:
                        value["error"] = detail.decode("utf-8")
                        break
                    except UnicodeDecodeError:
                        detail = detail[:-1]
            payload = (json.dumps(value, sort_keys=True) + "\n").encode("utf-8")
            if len(payload) > 4096:
                raise ValueError("materialization marker exceeds 4096 bytes")
            staging = extracted_root / ".materialization.json.tmp"
            descriptor_fd = os.open(staging, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor_fd, "wb") as stream:
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(staging, marker)

        def materialize() -> None:
            try:
                extract_indexed_trees(root, index, materialized, cancelled)
                elapsed = time.monotonic() - started
                print(f"FARIS demo: materialized {expanded_bytes} bytes of Core evidence in {elapsed:.3f} s; "
                      "the private temporary copy remains until the app exits.", file=sys.stderr, flush=True)
                publish_marker("COMPLETE")
            except BaseException as error:
                extraction_errors.append(error)
                try:
                    publish_marker("FAILED", str(error))
                except Exception as marker_error:
                    extraction_errors.append(marker_error)

        extraction_thread = threading.Thread(target=materialize, name="faris-materialization", daemon=False)
        extraction_thread.start()
        command = [str(app), "--core", str(core),
                   "--bundle", str(root / "port/bundles/reference.transport-bundle.json"),
                   "--bundle", str(root / "port/bundles/breeder-emphasis.transport-bundle.json"),
                   "--control-scenario", str(root / "control/scenario.json"),
                   "--control-bundle", str(root / "control/bundles/reference.transport-bundle.json"),
                   "--control-bundle", str(root / "control/bundles/breeder-emphasis.transport-bundle.json"),
                   "--assumptions", str(root / "operating-assumptions.json")]
        for bundle in sweep_bundles:
            command.extend(("--sweep-bundle", str(bundle)))
        for descriptor in descriptors:
            command.extend(("--saved-study", str(descriptor)))
        command.extend(("--saved-study-ready-marker", str(marker)))
        command.extend(app_arguments)
        try:
            child = subprocess.Popen(command)
        except Exception:
            cancelled.set()
            extraction_thread.join()
            raise
        def forward(signum, _frame):
            cancelled.set()
            if child.poll() is None:
                child.send_signal(signum)
        previous = {}
        for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            previous[signum] = signal.signal(signum, forward)
        try:
            while child.poll() is None:
                if not extraction_thread.is_alive():
                    break
                time.sleep(0.02)
            result = child.wait()
            app_closed_during_materialization = extraction_thread.is_alive()
            if result != 0 or app_closed_during_materialization:
                cancelled.set()
            extraction_thread.join()
            non_cancel_errors = [error for error in extraction_errors
                                 if not (app_closed_during_materialization
                                         and isinstance(error, InterruptedError))]
            if non_cancel_errors:
                detail = str(non_cancel_errors[0])
                print(f"FARIS demo: saved Core evidence materialization failed: {detail}",
                      file=sys.stderr, flush=True)
                if result == 0:
                    raise RuntimeError(f"saved Core evidence materialization failed: {detail}")
            return result
        finally:
            cancelled.set()
            extraction_thread.join()
            for signum, handler in previous.items():
                signal.signal(signum, handler)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        print(f"FARIS demo launcher: {error}", file=sys.stderr)
        raise SystemExit(2)
