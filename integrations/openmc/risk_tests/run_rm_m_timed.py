#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Run build_rm_m with durable stage timing and third-party implementation hashes.

This driver only wraps existing call boundaries. It does not replace mesh
algorithms or suppress the mesher's mesh-quality audit. Use a fresh output
directory and a supervisor with the recorded 90 minute / 6 GiB limits.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import functools
import hashlib
import importlib.metadata
import importlib.util
import inspect
import json
import os
from pathlib import Path
import resource
import sys
import time
import traceback


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


class Trace:
    def __init__(self, path: Path):
        self.path = path
        self.stream = path.open("x", encoding="utf-8", buffering=1)

    def event(self, stage: str, event: str, **details) -> None:
        record = {
            "timestamp_utc": datetime.now(timezone.utc).isoformat(),
            "monotonic_seconds": time.monotonic(),
            "stage": stage,
            "event": event,
            **details,
        }
        self.stream.write(json.dumps(record, sort_keys=True, default=str) + "\n")
        self.stream.flush()
        os.fsync(self.stream.fileno())

    def close(self):
        self.stream.close()


def timed_wrapper(trace: Trace, stage: str, function):
    """Wrap a callable while preserving its result, exception and metadata."""
    @functools.wraps(function)
    def wrapped(*args, **kwargs):
        started = time.monotonic()
        trace.event(stage, "start", function=f"{function.__module__}.{function.__qualname__}",
                    peak_rss_mb=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0)
        try:
            result = function(*args, **kwargs)
        except BaseException as error:
            trace.event(stage, "end", outcome="exception", seconds=time.monotonic() - started,
                        exception_type=type(error).__name__, exception=str(error),
                        peak_rss_mb=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0)
            raise
        trace.event(stage, "end", outcome="return", seconds=time.monotonic() - started,
                    result_type=type(result).__name__,
                    peak_rss_mb=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0)
        return result
    wrapped.__rm_m_trace_wrapper__ = True
    return wrapped


class _TimedH5File:
    """Transparent context proxy that marks the actual HDF5 write interval."""
    def __init__(self, handle, trace: Trace, path: Path):
        self._handle = handle
        self._trace = trace
        self._path = path
        self._opened = time.monotonic()

    def __enter__(self):
        self._handle.__enter__()
        self._trace.event("h5m_hdf5_write", "start", path=str(self._path))
        return self

    def __exit__(self, exc_type, exc, tb):
        try:
            return self._handle.__exit__(exc_type, exc, tb)
        finally:
            self._trace.event(
                "h5m_hdf5_write", "end", path=str(self._path),
                outcome="exception" if exc_type else "return",
                seconds=time.monotonic() - self._opened,
                exception_type=exc_type.__name__ if exc_type else None,
                exception=str(exc) if exc else None,
            )

    def __getattr__(self, name):
        return getattr(self._handle, name)


def _install_conversion_timing(trace: Trace, producer_path: Path):
    """Temporarily wrap audit and H5M writer boundaries; restore installed code afterward."""
    import h5py
    import cad_to_dagmc
    import cad_to_dagmc.core as core
    import cad_to_dagmc_mesher
    import cad_to_dagmc_mesher.cad._assembly as assembly

    audit_original = assembly._report_mesh_quality
    writer_original = core._vertices_to_h5m_h5py
    h5_file_original = h5py.File
    producer = producer_path.resolve()

    def h5_file(*args, **kwargs):
        target = args[0] if args else kwargs.get("name")
        mode = args[1] if len(args) > 1 else kwargs.get("mode", "r")
        handle = h5_file_original(*args, **kwargs)
        if target is not None and Path(target).resolve() == producer and mode == "w":
            trace.event("h5m_writer_preparation", "end", boundary="producer_h5m_open")
            return _TimedH5File(handle, trace, producer)
        return handle

    assembly._report_mesh_quality = timed_wrapper(trace, "mesh_quality_audit", audit_original)
    core._vertices_to_h5m_h5py = timed_wrapper(trace, "h5m_writer_preparation_and_write", writer_original)
    h5py.File = h5_file

    paths = {
        "cad_to_dagmc_core": Path(inspect.getsourcefile(cad_to_dagmc.CadToDagmc.export_dagmc_h5m_file)).resolve(),
        "cad_to_dagmc_mesher_assembly": Path(inspect.getsourcefile(audit_original)).resolve(),
        "cad_to_dagmc_mesher_extension": Path(cad_to_dagmc_mesher.__file__).resolve().parent / "cad_to_dagmc_mesher.abi3.so",
        "h5py_file_layer": Path(inspect.getsourcefile(h5_file_original)).resolve(),
    }
    package_files = {}
    for label, path in paths.items():
        package_files[label] = {"path": str(path), "sha256": _sha256(path)}
    wrapper_ids = {
        "mesh_quality_audit": {
            "target": f"{audit_original.__module__}.{audit_original.__qualname__}",
            "wrapper": f"{assembly._report_mesh_quality.__module__}.{assembly._report_mesh_quality.__qualname__}",
            "wrapper_kind": "run_rm_m_timed.timed_wrapper (transparent return/exception tracing)",
            "installed": bool(getattr(assembly._report_mesh_quality, "__rm_m_trace_wrapper__", False)),
            "source_file_sha256": package_files["cad_to_dagmc_mesher_assembly"]["sha256"],
        },
        "h5m_writer": {
            "target": f"{writer_original.__module__}.{writer_original.__qualname__}",
            "wrapper": f"{core._vertices_to_h5m_h5py.__module__}.{core._vertices_to_h5m_h5py.__qualname__}",
            "wrapper_kind": "run_rm_m_timed.timed_wrapper (transparent return/exception tracing)",
            "installed": bool(getattr(core._vertices_to_h5m_h5py, "__rm_m_trace_wrapper__", False)),
            "source_file_sha256": package_files["cad_to_dagmc_core"]["sha256"],
        },
        "h5py_file_open": {
            "target": "h5py.File",
            "wrapper": f"{h5_file.__module__}.{h5_file.__qualname__}",
            "wrapper_kind": "target-filtered File proxy that times producer H5M context",
            "installed": h5py.File is h5_file,
            "source_file_sha256": package_files["h5py_file_layer"]["sha256"],
        },
    }
    trace.event("conversion_instrumentation", "installed", package_files=package_files,
                wrapper_identities=wrapper_ids,
                versions={name: importlib.metadata.version(name)
                          for name in ("cad-to-dagmc", "cad-to-dagmc-mesher", "h5py")})

    def restore():
        assembly._report_mesh_quality = audit_original
        core._vertices_to_h5m_h5py = writer_original
        h5py.File = h5_file_original

    return restore, package_files, wrapper_ids


def _load_builder(path: Path):
    builder_dir = path.resolve().parent
    sys.path.insert(0, str(builder_dir))
    spec = importlib.util.spec_from_file_location("faris_rm_m_v3_builder", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load builder: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0], add_help=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--stem", default="rm_m")
    args, builder_args = parser.parse_known_args(argv)
    if "--out-dir" in builder_args or "--stem" in builder_args:
        parser.error("pass --out-dir and --stem to the driver, before builder options")
    out = args.out_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    trace_path = out / f"{args.stem}_build_trace.jsonl"
    metadata_path = out / f"{args.stem}_build_instrumentation.json"
    trace = Trace(trace_path)
    builder_path = Path(__file__).with_name("build_rm_m.py").resolve()
    builder = _load_builder(builder_path)
    metadata = {
        "schema": "faris-rm-m-build-instrumentation/v1",
        "status": "running",
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "wall_time_limit_seconds": 5400,
        "memory_max_bytes": 6 * 1024**3,
        "memory_swap_max_bytes": 0,
        "address_space_limit_bytes": 16 * 1024**3,
        "mesher_threads": 1,
        "builder": {"path": str(builder_path), "sha256": _sha256(builder_path)},
        "driver": {"path": str(Path(__file__).resolve()), "sha256": _sha256(Path(__file__))},
        "trace_file": trace_path.name,
        "command": [str(Path(sys.executable).resolve()), str(Path(__file__).resolve()),
                    "--out-dir", str(out), "--stem", args.stem, *builder_args],
    }
    metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    restore_conversion = None
    try:
        for name in ("build_solids", "overlap_checks", "export_step", "reimport_step",
                     "match_roles", "volume_checks", "write_card_files"):
            setattr(builder, name, timed_wrapper(trace, name, getattr(builder, name)))
        builder.spec.model_card = timed_wrapper(trace, "build_model_card", builder.spec.model_card)

        convert_original = builder.convert

        @functools.wraps(convert_original)
        def timed_convert(*call_args, **call_kwargs):
            # The destination argument is the producer file (the corrected H5M is created afterward).
            producer = Path(call_args[3] if len(call_args) > 3 else call_kwargs["h5m"])
            nonlocal restore_conversion
            restore_conversion, files, identities = _install_conversion_timing(trace, producer)
            metadata["third_party_files"] = files
            metadata["wrapper_identities"] = identities
            metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            try:
                return timed_wrapper(trace, "convert", convert_original)(*call_args, **call_kwargs)
            finally:
                restore_conversion()
                restore_conversion = None

        builder.convert = timed_convert
        old_argv = sys.argv
        sys.argv = [str(builder_path), "--out-dir", str(out), "--stem", args.stem, *builder_args]
        try:
            status = builder.main()
        finally:
            sys.argv = old_argv
        metadata["status"] = "success" if status == 0 else "failure"
        metadata["return_code"] = status
        return status
    except BaseException as error:
        metadata["status"] = "exception"
        metadata["exception_type"] = type(error).__name__
        metadata["exception"] = str(error)
        metadata["traceback"] = traceback.format_exc()
        raise
    finally:
        if restore_conversion is not None:
            restore_conversion()
        metadata["ended_utc"] = datetime.now(timezone.utc).isoformat()
        metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        trace.close()


if __name__ == "__main__":
    raise SystemExit(main())
