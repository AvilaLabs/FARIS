# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""The standard reactor worker binds loss accounting to exact C API receipts."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("reactor_transport", HERE / "reactor_transport.py")
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)


class ReactorLossAccounting(unittest.TestCase):
    def test_library_receipt_uses_only_verified_cli_rpath_library(self):
        with tempfile.TemporaryDirectory() as scratch:
            prefix = Path(scratch)
            (prefix / "bin").mkdir()
            (prefix / "lib").mkdir()
            executable = prefix / "bin/openmc"
            python = prefix / "bin/python"
            library = prefix / "lib/libopenmc.so"
            executable.write_bytes(b"cli")
            python.write_bytes(b"python")
            library.write_bytes(b"linked library")
            dynamic = " 0x1 (NEEDED) Shared library: [libopenmc.so]\n 0xf (RPATH) Library rpath: [$ORIGIN/../lib]\n"
            with patch.object(WORKER.shutil, "which", return_value="/usr/bin/readelf"), \
                    patch.object(WORKER.subprocess, "run", return_value=type("Result", (), {"stdout": dynamic})()):
                selected, digest = WORKER.selected_openmc_library(executable, python, {})
            self.assertEqual(selected, library.resolve())
            self.assertEqual(digest, WORKER.sha256(library))
            bad = dynamic.replace("RPATH", "RUNPATH").replace("rpath:", "runpath:")
            with patch.object(WORKER.shutil, "which", return_value="/usr/bin/readelf"), \
                    patch.object(WORKER.subprocess, "run", return_value=type("Result", (), {"stdout": bad})()):
                with self.assertRaisesRegex(ValueError, "RUNPATH"):
                    WORKER.selected_openmc_library(executable, python, {})

    def test_capi_child_runs_selected_library_and_writes_real_counter_receipt(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            library = root / "libopenmc.so"
            executable = root / "openmc"
            receipt = root / "loss-pass.json"
            library.write_bytes(b"selected library")
            executable.write_bytes(b"selected executable")
            calls = []

            class Api:
                def __init__(self, name, result=0):
                    self.name = name
                    self.result = result

                def __call__(self, *args):
                    calls.append((self.name, *args))
                    return self.result

            class FakeDll:
                n_lost_particles = 0
                current_batch = 0
                openmc_err_msg = 0
                openmc_init = Api("init")
                openmc_run = Api("run")
                openmc_finalize = Api("finalize")

            fake_dll = FakeDll()
            counters = iter([0, 30, 4])
            with patch.object(WORKER.ctypes, "CDLL", return_value=fake_dll), \
                    patch.object(WORKER, "_capi_int", side_effect=lambda dll, symbol: next(counters)):
                result = WORKER.openmc_capi_runner([
                    "--capi-runner", "--library", str(library), "--library-sha256", WORKER.sha256(library),
                    "--executable", str(executable), "--receipt", str(receipt),
                ])
            self.assertEqual(result, 0)
            self.assertEqual([call[0] for call in calls], ["init", "run", "finalize"])
            written = json.loads(receipt.read_text())
            self.assertEqual((written["start_batch"], written["end_batch"], written["lost_particle_events"]), (0, 30, 4))
            self.assertEqual(written["library_sha256"], WORKER.sha256(library))

    def test_repetitive_loss_warnings_are_bounded_without_becoming_the_count(self):
        state = {"loss_warning_lines": 0, "loss_warning_bytes_suppressed": 0}
        warnings = b"".join(f"Particle {index} was lost\n".encode() for index in range(20))
        retained = WORKER._filtered_solver_lines(state, warnings)
        retained += WORKER._filtered_solver_lines(state, b"", final=True)
        self.assertEqual(state["loss_warning_lines"], 20)
        self.assertEqual(retained.count(b"was lost"), WORKER.LOST_WARNING_LINES_TO_RETAIN)
        self.assertGreater(state["loss_warning_bytes_suppressed"], 0)

    def test_standard_run_uses_receipt_count_even_when_logs_have_no_loss_text(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            out = root / "solver"
            out.mkdir()
            library = root / "libopenmc.so"
            library.write_bytes(b"selected test library")
            executable = root / "openmc"
            executable.write_bytes(b"selected test executable")

            def fake_solver(command, cwd, env, log_limit, append=False):
                receipt = Path(command[command.index("--receipt") + 1])
                receipt.write_text(json.dumps({
                    "schema_version": "faris-openmc-loss-pass/v0.1",
                    "method": "openmc-c-api-n_lost_particles/v1",
                    "library_path": str(library),
                    "library_sha256": WORKER.sha256(library),
                    "start_batch": 0,
                    "end_batch": 30,
                    "lost_particle_events": 2,
                }))
                # No lost-particle text in the logs. The count must come from the C API receipt.
                return {"return_code": 0, "stop_reason": None, "streams": {}}

            with patch.object(WORKER, "run_solver_streaming", side_effect=fake_solver), \
                    patch.object(WORKER, "read_batch_sums", return_value=({}, 30)):
                _, _, batches, restarts, passes = WORKER.sample_until_settled(
                    None, out, [str(executable)], {}, None, 30, [], {},
                    selected_executable=executable, capi_library=library, particles_per_batch=1000)
            self.assertEqual((batches, restarts), (30, []))
            self.assertEqual(passes, [{
                "pass": 1,
                "start_batch": 0,
                "end_batch": 30,
                "histories": 30_000,
                "lost_particle_events": 2,
            }])

    def test_missing_receipt_is_not_interpreted_as_zero(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            out = root / "solver"
            out.mkdir()
            library = root / "libopenmc.so"
            library.write_bytes(b"selected test library")
            executable = root / "openmc"
            executable.write_bytes(b"selected test executable")
            with patch.object(WORKER, "run_solver_streaming", return_value={"return_code": 0, "stop_reason": None, "streams": {}}), \
                    patch.object(WORKER, "read_batch_sums", return_value=({}, 30)):
                with self.assertRaisesRegex(ValueError, "receipt"):
                    WORKER.sample_until_settled(
                        None, out, [str(executable)], {}, None, 30, [], {},
                        selected_executable=executable, capi_library=library, particles_per_batch=1000)

    def test_early_stop_restart_keeps_each_process_count_separate(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            out = root / "solver"
            out.mkdir()
            library = root / "libopenmc.so"
            library.write_bytes(b"selected test library")
            executable = root / "openmc"
            executable.write_bytes(b"selected test executable")
            settings = out / "settings.xml"
            settings.write_text("<settings><batches>50</batches></settings>")
            calls = 0

            def fake_solver(command, cwd, env, log_limit, append=False):
                nonlocal calls
                calls += 1
                start, end, losses = (0, 30, 1) if calls == 1 else (30, 38, 0)
                receipt = Path(command[command.index("--receipt") + 1])
                receipt.write_text(json.dumps({
                    "schema_version": "faris-openmc-loss-pass/v0.1",
                    "method": "openmc-c-api-n_lost_particles/v1",
                    "library_path": str(library),
                    "library_sha256": WORKER.sha256(library),
                    "start_batch": start,
                    "end_batch": end,
                    "lost_particle_events": losses,
                }))
                if calls == 1:
                    (out / "statepoint.30.h5").write_bytes(b"statepoint")
                return {"return_code": 0, "stop_reason": None, "streams": {}}

            unsettled = {"met": True, "settled": False, "variance_of_variance": 0.5}
            settled = {"met": True, "settled": True, "variance_of_variance": 0.05}
            with patch.object(WORKER, "run_solver_streaming", side_effect=fake_solver), \
                    patch.object(WORKER, "read_batch_sums", side_effect=[({}, 30), ({}, 38)]), \
                    patch.object(WORKER, "guard_verdict", side_effect=[unsettled, settled]):
                _, _, batches, restarts, passes = WORKER.sample_until_settled(
                    None, out, [str(executable)], {}, {"min_batches": 30, "response_ids": ["x"]}, 50,
                    [], {"settings.xml": WORKER.sha256(settings)}, selected_executable=executable,
                    capi_library=library, particles_per_batch=1000)
            self.assertEqual(batches, 38)
            self.assertEqual(len(restarts), 1)
            self.assertEqual(
                [(p["histories"], p["lost_particle_events"]) for p in passes],
                [(30_000, 1), (8_000, 0)],
            )


if __name__ == "__main__":
    unittest.main()
