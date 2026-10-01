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
from recorded_archives import extract_indexed_trees
from verify_recorded_demo import verify_index


def main() -> int:
    if len(sys.argv) < 2:
        raise ValueError("usage: launch_recorded_demo.py PACKAGE_ROOT [APP_ARGUMENTS...]")
    root = Path(sys.argv[1]).resolve(strict=True)
    app_arguments = sys.argv[2:]
    faris, core, app = (root / "bin/faris", root / "bin/avila-core", root / "bin/faris-app")
    index, _ = verify_index(root, faris, core)
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
