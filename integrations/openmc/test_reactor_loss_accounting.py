# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""The standard reactor worker binds loss accounting to exact C API receipts."""

import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("reactor_transport", HERE / "reactor_transport.py")
WORKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(WORKER)


class ReactorLossAccounting(unittest.TestCase):
    def test_stream_capture_is_json_serializable_after_final_partial_line_flush(self):
        class Sink:
            def __init__(self):
                self.buffer = io.BytesIO()

        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            sink_out = Sink()
            sink_err = Sink()
            command = [
                sys.executable,
                "-c",
                "import os; os.write(1, b'Particle 1 was lost'); os.write(2, b'trailing fragment')",
            ]
            with patch.object(WORKER.sys, "stdout", sink_out), \
                    patch.object(WORKER.sys, "stderr", sink_err):
                capture = WORKER.run_solver_streaming(command, root, os.environ.copy(), 1024)

            encoded = json.dumps(capture)
            self.assertTrue(encoded)
            stdout = capture["streams"]["stdout"]
            stderr = capture["streams"]["stderr"]
            self.assertNotIn("pending", stdout)
            self.assertNotIn("pending", stderr)
            self.assertEqual(stdout["loss_warning_lines"], 1)
            self.assertEqual((root / stdout["file"]).read_bytes(), b"Particle 1 was lost")
            self.assertEqual((root / stderr["file"]).read_bytes(), b"trailing fragment")

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

    def test_capi_restart_receipt_reads_counters_after_lifecycle_restores_restart(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            library = root / "libopenmc.so"
            executable = root / "openmc"
            receipt = root / "loss-pass.json"
            library.write_bytes(b"selected library")
            executable.write_bytes(b"selected executable")
            calls = []

            class Api:
                def __init__(self, callback):
                    self.callback = callback

                def __call__(self, *args):
                    return self.callback(*args)

            class FakeDll:
                n_lost_particles = 0
                current_batch = 0
                openmc_err_msg = 0

            fake_dll = FakeDll()
            def init(*args):
                calls.append("init")
                return 0

            def simulation_init():
                calls.append("simulation_init")
                # Restart restoration occurs during this lifecycle call.
                fake_dll.current_batch = 1
                fake_dll.n_lost_particles = 100
                return 0

            def next_batch(status_ptr):
                calls.append("next_batch")
                fake_dll.current_batch += 1
                fake_dll.n_lost_particles += 1000
                status_ptr._obj.value = 1 if fake_dll.current_batch == 3 else 0
                return 0

            def simulation_finalize():
                calls.append("simulation_finalize")
                return 0

            def finalize():
                calls.append("finalize")
                return 0

            fake_dll.openmc_init = Api(init)
            fake_dll.openmc_simulation_init = Api(simulation_init)
            fake_dll.openmc_next_batch = Api(next_batch)
            fake_dll.openmc_simulation_finalize = Api(simulation_finalize)
            fake_dll.openmc_finalize = Api(finalize)
            with patch.object(WORKER.ctypes, "CDLL", return_value=fake_dll), \
                    patch.object(WORKER, "_capi_int", side_effect=lambda dll, symbol: getattr(dll, symbol)):
                result = WORKER.openmc_capi_runner([
                    "--capi-runner", "--library", str(library), "--library-sha256", WORKER.sha256(library),
                    "--executable", str(executable), "--receipt", str(receipt), "-r", "statepoint.1.h5",
                ])
            self.assertEqual(result, 0)
            self.assertEqual(calls, ["init", "simulation_init", "next_batch", "next_batch",
                                     "simulation_finalize", "finalize"])
            written = json.loads(receipt.read_text())
            self.assertEqual((written["start_batch"], written["end_batch"],
                              written["start_lost_particle_events"],
                              written["end_lost_particle_events"],
                              written["lost_particle_events"]), (1, 3, 100, 2100, 2000))
            self.assertEqual(written["schema_version"], "faris-openmc-loss-pass/v0.2")
            self.assertEqual(written["method"], "openmc-c-api-n_lost_particles-delta/v1")
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
                    "schema_version": "faris-openmc-loss-pass/v0.2",
                    "method": "openmc-c-api-n_lost_particles-delta/v1",
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
                    "schema_version": "faris-openmc-loss-pass/v0.2",
                    "method": "openmc-c-api-n_lost_particles-delta/v1",
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
