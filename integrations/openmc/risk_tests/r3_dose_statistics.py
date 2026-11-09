# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Small, pure helpers for batch-covariant R3 shutdown-dose tallies."""
from __future__ import annotations

import math
import os
from pathlib import Path
import hashlib
import ctypes
import importlib
import shutil
import xml.etree.ElementTree as ET
import time


def d1s_batch_total(cumulative, previous, voxel_count: int, parent_count: int, factors):
    """Return the weighted spatial total for one D1S batch from cumulative C API sums.

    The returned batch value first differences OpenMC's cumulative parent/voxel bins, then
    applies the parent-specific time factors and sums all bins. This retains covariance across
    both spatial and parent-nuclide bins because the standard error is computed over these
    already-aggregated independent batch values.
    """
    import numpy as np

    current = np.asarray(cumulative, dtype=float).reshape(-1)
    before = np.asarray(previous, dtype=float).reshape(-1)
    weights = np.asarray(factors, dtype=float).reshape(-1)
    if voxel_count < 1 or parent_count < 1 or current.size != voxel_count * parent_count:
        raise ValueError("D1S cumulative tally shape does not match voxel and parent counts")
    if before.shape != current.shape or weights.size != parent_count:
        raise ValueError("D1S previous sums or time factors have the wrong shape")
    if not np.isfinite(current).all() or not np.isfinite(before).all() or not np.isfinite(weights).all():
        raise ValueError("D1S tally sums and time factors must be finite")
    delta = (current - before).reshape(voxel_count, parent_count)
    value = float((delta * weights[None, :]).sum())
    if not math.isfinite(value):
        raise ValueError("D1S corrected batch total is not finite")
    return current.copy(), value


def scalar_batch_total(cumulative, previous) -> tuple[float, float]:
    """Return one scalar tally's cumulative sum and per-batch increment."""
    current = float(cumulative)
    before = float(previous)
    value = current - before
    if not math.isfinite(current) or not math.isfinite(value):
        raise ValueError("R2S cumulative tally sum must be finite")
    return current, value


def batch_summary(values) -> dict:
    """Summarize independent, already-aggregated batch totals using sample SE of the mean."""
    samples = [float(value) for value in values]
    if len(samples) < 2:
        raise ValueError("at least two complete batches are required for a standard error")
    if not all(math.isfinite(value) for value in samples):
        raise ValueError("batch totals must all be finite")
    mean = math.fsum(samples) / len(samples)
    variance = math.fsum((value - mean) ** 2 for value in samples) / (len(samples) - 1)
    standard_error = math.sqrt(variance / len(samples))
    if not math.isfinite(mean) or not math.isfinite(standard_error):
        raise ValueError("batch mean or standard error is not finite")
    return {"mean": mean, "standard_error": standard_error, "batches": len(samples),
            "uncertainty_scope": "Monte Carlo batch sampling only"}


LOST_FRACTION_LIMIT = 1.0e-6


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _lost_count(dll) -> int | None:
    try:
        value = int(ctypes.c_int.in_dll(dll, "n_lost_particles").value)
        return value if value >= 0 else None
    except (AttributeError, OSError, TypeError, ValueError):
        return None


def _library_identity(openmc) -> dict:
    lib = getattr(openmc, "lib", None)
    if lib is None:
        lib = importlib.import_module("openmc.lib")
    dll = getattr(lib, "_dll", None)
    raw_path = getattr(dll, "_name", None)
    if not isinstance(raw_path, str) or not raw_path:
        return {"path": None, "sha256": None}
    path = Path(raw_path).resolve()
    if not path.is_file():
        return {"path": str(path), "sha256": None}
    return {"path": str(path), "sha256": _sha256_file(path)}


def runtime_identity(openmc, cli_name: str = "openmc") -> dict:
    """Record the already-loaded C API library and sibling CLI executable identities."""
    cli_path = shutil.which(cli_name)
    cli = None
    if cli_path is not None:
        path = Path(cli_path).resolve()
        cli = {"path": str(path), "sha256": _sha256_file(path) if path.is_file() else None}
    return {"openmc_version": getattr(openmc, "__version__", None),
            "c_api_library": _library_identity(openmc), "cli_executable": cli}


def _is_microxs_model(path: Path, marker: str = "MicroXS flux") -> bool:
    try:
        root = ET.parse(path).getroot()
    except (ET.ParseError, OSError):
        return False
    return any(tally.get("name") == marker for tally in root.iter("tally"))


class NeutronManagerRunEvidence:
    """Observe only the OpenMC C API run initialized from R2S's MicroXS model XML."""

    def __init__(self, openmc, expected_model_path: Path):
        self.openmc = openmc
        self.expected_model_path = Path(expected_model_path).resolve()
        self.lib = None
        self.original_init = None
        self.original_run = None
        self.active = None
        self.events = []
        self.error = None

    def __enter__(self):
        self.lib = getattr(self.openmc, "lib", None)
        if self.lib is None:
            self.lib = importlib.import_module("openmc.lib")
        self.original_init = self.lib.init
        self.original_run = self.lib.run

        def observed_init(*args, **kwargs):
            model_path = (Path.cwd() / "model.xml").resolve()
            candidate = _is_microxs_model(model_path)
            supplied_args = args[0] if args else kwargs.get("args")
            if candidate and supplied_args is not None:
                self.error = "MicroXS init supplied explicit args; the observed CWD model cannot be assumed to be its input"
                candidate = False
            if candidate and set(kwargs) - {"intracomm", "output", "args"}:
                self.error = "MicroXS init supplied unrecognized options; the observed CWD model cannot be bound safely"
                candidate = False
            pre_init_hash = _sha256_file(model_path) if candidate else None
            pre_init_library = _library_identity(self.openmc) if candidate else None
            if self.active is not None:
                self.error = "a second C API initialization occurred before the observed run completed"
            result = self.original_init(*args, **kwargs)
            if candidate:
                post_init_hash = _sha256_file(model_path)
                post_init_library = _library_identity(self.openmc)
                if post_init_hash != pre_init_hash:
                    self.error = "MicroXS model XML changed during C API initialization"
                elif post_init_library != pre_init_library:
                    self.error = "C API library identity changed during MicroXS initialization"
                else:
                    self.active = {"model_path": str(model_path), "sha256": pre_init_hash,
                                   "c_api_library": pre_init_library}
            return result

        def observed_run(*args, **kwargs):
            active = self.active
            if active is None:
                return self.original_run(*args, **kwargs)
            model_path = Path(active["model_path"])
            pre_run_hash = _sha256_file(model_path)
            if pre_run_hash != active["sha256"]:
                self.error = "MicroXS model XML changed between C API initialization and lib.run"
                self.active = None
                return self.original_run(*args, **kwargs)
            result = self.original_run(*args, **kwargs)
            histories = int(self.lib.settings.batches) * int(self.lib.settings.particles)
            lost = _lost_count(getattr(self.lib, "_dll", None))
            self.events.append({"model_xml_path": str(model_path), "model_xml_sha256": pre_run_hash,
                                "histories": histories, "lost_particles": lost,
                                "c_api_library": active["c_api_library"]})
            self.active = None
            return result

        self.lib.init = observed_init
        self.lib.run = observed_run
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.lib.init = self.original_init
        self.lib.run = self.original_run
        return False

    def result(self) -> dict:
        if self.error:
            return {"status": "NOT_EVALUATED", "reason": self.error,
                    "lost_particle_assessment": {"status": "NOT_EVALUATED", "lost_particles": None,
                                                  "histories": None}}
        if self.active is not None:
            reason = "MicroXS C API initialization was observed without its matching lib.run callback"
            return {"status": "NOT_EVALUATED", "reason": reason,
                    "lost_particle_assessment": {"status": "NOT_EVALUATED", "lost_particles": None,
                                                  "histories": None}}
        if len(self.events) != 1:
            return {"status": "NOT_EVALUATED", "reason": f"expected exactly one MicroXS C API run, observed {len(self.events)}",
                    "lost_particle_assessment": {"status": "NOT_EVALUATED", "lost_particles": None,
                                                  "histories": None}}
        event = self.events[0]
        output_path = self.expected_model_path
        if not output_path.is_file() or _sha256_file(output_path) != event["model_xml_sha256"]:
            return {"status": "NOT_EVALUATED", "reason": "persisted R2S neutron model XML differs from the exact pre-init C API input",
                    "solver_model": {"path": str(output_path), "sha256": event["model_xml_sha256"]},
                    "lost_particle_assessment": {"status": "NOT_EVALUATED", "lost_particles": None,
                                                  "histories": event["histories"]}}
        lost = event["lost_particles"]
        status = ("NOT_EVALUATED" if lost is None else
                  "REJECTED" if event["histories"] < 1 or lost > LOST_FRACTION_LIMIT * event["histories"] else "PASS")
        return {"status": status, "solver_model": {"path": str(output_path), "sha256": event["model_xml_sha256"]},
                "c_api_library": event["c_api_library"], "histories": event["histories"],
                "lost_particle_assessment": {"status": status, "lost_particles": lost,
                                              "histories": event["histories"],
                                              "fraction_limit": LOST_FRACTION_LIMIT,
                                              "scope": "R2S neutron MicroXS transport run"}}


def run_live_tally(openmc, model_xml, tally_id: int, threads: int, max_batches: int, budget_s: float,
                   on_batch, particles_per_batch: int):
    """Run a pre-exported fixed-source model through OpenMC's C API and consume cumulative sums per batch.

    `on_batch` receives a copy of the cumulative tally sums and completed batch count. This avoids retaining
    per-batch statepoint files; only caller-selected compact aggregates need to be kept in memory.
    """
    if threads < 1 or max_batches < 1 or max_batches > 250_000 or particles_per_batch < 1 or \
            not math.isfinite(budget_s) or budget_s <= 0.0:
        raise ValueError("threads and budget_s must be positive; max_batches must be within 1..250000")
    model_xml = os.path.abspath(model_xml)
    lib = openmc.lib
    model_identity = {"path": model_xml, "sha256": _sha256_file(Path(model_xml))}
    library_identity = _library_identity(openmc)
    previous_cwd = os.getcwd()
    started = time.monotonic()
    batches = 0
    stop_reason = "batch_limit"
    initialized = False
    final_sums = None
    final_std_dev = None
    final_lost_count = None
    last_solver_status = None
    try:
        os.chdir(os.path.dirname(model_xml))
        lib.init(["--threads", str(threads), str(model_xml)])
        initialized = True
        lib.simulation_init()
        tally = lib.tallies[tally_id]
        while batches < max_batches:
            if batches > 0 and time.monotonic() - started >= budget_s:
                stop_reason = "time_budget"
                break
            status = lib.next_batch()
            last_solver_status = int(status)
            batches = int(tally.num_realizations)
            if batches < 1:
                raise RuntimeError("OpenMC completed a batch without tally realizations")
            sums = tally.results[:, 0, 1].copy()
            on_batch(sums, batches)
            final_lost_count = _lost_count(getattr(lib, "_dll", None))
            histories = batches * particles_per_batch
            if final_lost_count is not None and final_lost_count > LOST_FRACTION_LIMIT * histories:
                stop_reason = "lost_particle_limit"
                break
            if status == 1:
                stop_reason = "batch_limit" if batches >= max_batches else "early_maximum_batches"
                break
            if status == 2:
                stop_reason = "tally_trigger"
                break
            if status != 0:
                stop_reason = f"unexpected_solver_status_{status}"
                break
        elapsed = time.monotonic() - started
        final_sums = tally.results[:, 0, 1].copy()
        final_std_dev = tally.std_dev[:, 0].copy()
        histories = batches * particles_per_batch
        loss_status = ("NOT_EVALUATED" if final_lost_count is None else
                       "REJECTED" if histories <= 0 or final_lost_count > LOST_FRACTION_LIMIT * histories else "PASS")
        integrity = {
            "solver_model": model_identity,
            "c_api_library": library_identity,
            "lost_particle_assessment": {"status": loss_status, "lost_particles": final_lost_count,
                                          "histories": histories, "fraction_limit": LOST_FRACTION_LIMIT,
                                          "scope": "all particle species transported in this C API run"},
        }
        return {"batches": batches, "elapsed_s": elapsed, "stop_reason": stop_reason,
                "histories": histories, "solver_status": last_solver_status,
                "integrity": integrity}, final_sums, final_std_dev
    finally:
        try:
            if initialized:
                lib.simulation_finalize()
        finally:
            if initialized:
                lib.finalize()
            os.chdir(previous_cwd)
