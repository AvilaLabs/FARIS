#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Build the seed corpus for the fuzz targets (QA-033) from the repository's own fixtures.

    python3 scripts/fuzz_seed.py [--out fuzz/corpus]

One folder per target under --out, one file per seed. Seeds are real design, scenario,
operating-history, physics-case, history-result, maintenance, ACTINV, facets, inspection and
PNG files from the repository, plus a few small hand-built study, archive, store and case files.
The corpus is generated, not committed (fuzz/corpus is ignored); libFuzzer adds what it finds.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import struct
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAX_SEED_BYTES = 200_000
MAX_PER_TARGET = 300

# schema_version value (prefix) -> targets, each "name" or "name:kind", kind being the selector byte.
SCHEMAS = {
    "faris-scenario/": ["scenario", "transport_artifact"],
    "faris-design/": ["design_file"],
    "faris-operating-history/": ["history_assumptions"],
    "faris-maintenance-assumptions/": ["history_assumptions"],
    "faris-physics-case/": ["json_records:4", "file_readers:2"],
    "faris-history-result/": ["json_records:0"],
    "faris-maintenance-result/": ["json_records:2", "history_assumptions"],
    "faris-reactor-run/": ["json_records:3", "file_readers:3"],
    "faris-recorded-transport-bundle/": ["json_records:7", "file_readers:4", "file_readers:5"],
    "faris-design-import-report/": ["json_records:10"],
    "faris.openmc-library-audit/": ["nuclear_data_audit"],
    "faris-saved-study-descriptor/": ["file_readers:6", "file_readers:7"],
}


def candidates() -> list[Path]:
    out = []
    for path in sorted(ROOT.rglob("*.json")):
        rel = path.relative_to(ROOT).parts
        if rel[0] in ("target", "runs", "node_modules", "fuzz") or ".git" in rel:
            continue
        if path.stat().st_size <= MAX_SEED_BYTES:
            out.append(path)
    return out


class Corpus:
    def __init__(self, out: Path):
        self.out = out
        self.counts: dict[str, int] = {}

    def add(self, target: str, data: bytes) -> None:
        if self.counts.get(target, 0) >= MAX_PER_TARGET:
            return
        folder = self.out / target
        folder.mkdir(parents=True, exist_ok=True)
        name = hashlib.sha256(data).hexdigest()[:20]
        if not (folder / name).exists():
            (folder / name).write_bytes(data)
            self.counts[target] = self.counts.get(target, 0) + 1

    def add_selected(self, spec: str, data: bytes) -> None:
        target, _, kind = spec.partition(":")
        self.add(target, (bytes([int(kind)]) if kind else b"") + data)


def schema_of(data: bytes) -> str:
    try:
        value = json.loads(data)
    except ValueError:
        return ""
    if isinstance(value, dict):
        for key in ("schema_version", "schema"):
            if isinstance(value.get(key), str):
                return value[key]
    return ""


def markdown_json_blocks(path: str) -> list[bytes]:
    """Read fenced JSON examples as bytes so their documentation remains the source of truth."""
    text = (ROOT / path).read_text(encoding="utf-8")
    blocks: list[bytes] = []
    current: list[str] | None = None
    for line in text.splitlines():
        if current is None and line == "```json":
            current = []
        elif current is not None and line == "```":
            blocks.append(("\n".join(current) + "\n").encode())
            current = None
        elif current is not None:
            current.append(line)
    return blocks


def study_files() -> list[bytes]:
    manifest = {"format": "faris-study/1", "mimetype": "application/vnd.avila-labs.faris-study"}
    out = []
    for with_manifest in (False, True):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as z:
            z.writestr(zipfile.ZipInfo("mimetype"), "application/vnd.avila-labs.faris-study")
            if with_manifest:
                z.writestr("manifest.json", json.dumps(manifest))
        out.append(buffer.getvalue())
    return out


def archives() -> list[bytes]:
    out = []
    for members in ([], [("a/b.txt", b"hello")], [("dir/", None), ("dir/x.json", b"{}")]):
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as t:
            for name, body in members:
                info = tarfile.TarInfo(name)
                if body is None:
                    info.type = tarfile.DIRTYPE
                    t.addfile(info)
                else:
                    info.size = len(body)
                    t.addfile(info, io.BytesIO(body))
        import gzip

        out.append(gzip.compress(raw.getvalue(), mtime=0))
        out.append(raw.getvalue())
    return out


def store_indexes() -> list[bytes]:
    digest = hashlib.sha256(b"x").hexdigest()
    index = {
        "schema_version": "avila.core/evidence-store/v0.1",
        "codec": "xz",
        "trees": [{"name": "case", "files": [{"path": "package.json", "sha256": digest, "bytes": 1}]}],
    }
    empty = dict(index, trees=[])
    return [json.dumps(i, indent=2).encode() + b"\n" for i in (index, empty)]


def heating_worker_seed() -> bytes:
    """A structurally valid coupled-heating report reaches nested private Rust score parsers."""
    def score(mean: float, se: float, closure: bool = False) -> dict:
        value = {"mean_ev_per_source": mean, "standard_error_ev_per_source": se}
        if closure:
            value.update(within_3se=True, roundoff_tolerance_ev_per_source=1.0e-12)
        return value

    components = {"neutron": score(2.0, 0.1), "photon": score(1.0, 0.1),
                  "electron": score(0.5, 0.1), "positron": score(0.5, 0.1), "total": score(4.0, 0.2)}
    closure = score(0.0, 0.01, closure=True)
    voxel = {"r_cm": [100.0, 105.0], "z_cm": [-5.0, 0.0], "volume_cm3": 1000.0,
             "components": components, "closure_residual": closure, "is_peak": True}
    peak = {"mean": 4.0, "standard_error": 0.2, "volume_cm3": 1000.0,
            "r_cm": [100.0, 105.0], "z_cm": [-5.0, 0.0], "pieces_ranked": 2,
            "volume_estimator": {"method": "OpenMC ray sampled mesh volume", "rays": 1000,
                                 "sampling_relative_error": None, "sampling_uncertainty_reported": False},
            "closure_residual": closure, "components": components}
    solid = {"solid_id": "tf-winding-01", "neutron": score(3.0, 0.2), "photon": score(1.0, 0.1),
             "electron": score(0.5, 0.1), "positron": score(0.5, 0.1), "total": score(5.0, 0.3),
             "closure_residual": closure, "peak": peak, "voxel_scores": [voxel]}
    report = {"schema": "faris-design-heating/v1", "status": "CALCULATED", "openmc": "0.16.0",
              "h5m_sha256": "a" * 64, "cross_sections_sha256": "b" * 64, "histories": 30000,
              "c_api_terminal_status": 1, "lost_particles": 0, "stop_reason": "history_limit",
              "normalization": "per_source_neutron", "unit": "eV/source_neutron",
              "data_identity": {"neutron_mt301": {"H1": {"mt301": True}}}, "solids": [solid]}
    return (json.dumps(report, separators=(",", ":")) + "\n").encode()


def heating_preflight_seed() -> bytes:
    report = {"schema": "faris-design-heating-preflight/v1", "status": "READY",
              "h5m_sha256": "a" * 64, "cross_sections_sha256": "b" * 64,
              "data_identity": {"neutron_mt301": {"H1": {"mt301": True}},
                                "photon": {"H": {"shell_count": 1}},
                                "atomic_relaxation": "enabled", "electron_treatment": "led"}}
    return (json.dumps(report, separators=(",", ":")) + "\n").encode()


def blocks(files: list[tuple[str, bytes]]) -> bytes:
    return b"\n=====\n".join(name.encode() + b"\n" + body for name, body in files)


def build(out: Path) -> dict[str, int]:
    corpus = Corpus(out)
    for path in candidates():
        data = path.read_bytes()
        name = path.name
        schema = schema_of(data)
        for prefix, targets in SCHEMAS.items():
            if schema.startswith(prefix):
                for spec in targets:
                    corpus.add_selected(spec, data)
        if name.endswith(".inspect.json"):
            corpus.add("step_inspection", data)
        if name.startswith("actinv_result") or name == "actinv_scan_expected.json":
            corpus.add("actinv_result", data)
        if name.endswith(".design.json") or name.endswith(".faris-design.json"):
            corpus.add("design_file", data)
        if "assumptions" in name or "operating-history" in name:
            corpus.add("history_assumptions", data)
        if name.endswith(".facets.json"):
            binary = path.with_name(name[: -len(".json")] + ".bin")
            if binary.is_file() and binary.stat().st_size <= 40_000:
                corpus.add("facets", struct.pack("<H", len(data)) + data + binary.read_bytes())
        # A few files of every other kind feed the generic record readers.
        if schema and len(data) < 20_000:
            corpus.add_selected(f"json_records:{len(data) % 13}", data)
    for png in ROOT.glob("crates/*/assets/*.png"):
        if png.stat().st_size <= MAX_SEED_BYTES:
            corpus.add("png_header", png.read_bytes())
    for data in study_files():
        corpus.add("study_file", data)
    for data in archives():
        corpus.add("tar_gz", data)
    for data in store_indexes():
        corpus.add("evidence_store", data)
    for data in markdown_json_blocks("docs/guide/shutdown-dose.md"):
        corpus.add("shutdown_dose_records", data)
    case = [
        ("execution-report.json", b"{}"),
        ("package.json", b'{"schema_version": "avila.core/package/v0.2-draft"}'),
        ("claims.json", b"[]"),
    ]
    corpus.add("saved_case", blocks(case))
    for sample in ("/proc/meminfo", "/proc/self/stat", "/proc/self/status"):
        try:
            corpus.add("proc_and_config_text", Path(sample).read_bytes())
        except OSError:
            pass
    for text in (b"auto", b"native", b"python", b"coarse", b"outboard-local", b"MemAvailable: 1 kB\n"):
        corpus.add("proc_and_config_text", text)
    corpus.add("helper_outputs", b'{"progress": {"histories": 1, "relative_error": 0.1, "elapsed_seconds": 1.0}}')
    corpus.add("helper_outputs", b'{"filled_sha256": "00"}')
    corpus.add("helper_outputs", heating_worker_seed())
    corpus.add("helper_outputs", heating_preflight_seed())
    return corpus.counts


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", type=Path, default=ROOT / "fuzz" / "corpus")
    args = ap.parse_args()
    counts = build(args.out)
    for target, n in sorted(counts.items()):
        print(f"{target}: {n} seeds")
    print(f"{sum(counts.values())} seeds in {args.out}")


if __name__ == "__main__":
    main()
