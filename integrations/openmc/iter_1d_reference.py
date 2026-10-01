#!/usr/bin/env python3
"""Run the local IAEA-NDS ITER_1D OpenMC case with auditable cell tallies.

This is a code-to-code reference execution helper, not the authoritative FARIS
transport implementation. It never loads the benchmark's bundled libsource.so;
the hashed C++ source law is translated using the selected OpenMC Python API.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import traceback
import xml.etree.ElementTree as ET

SCHEMA = "faris.iter-1d-openmc-reference/1.0.0"
MAX_HISTORIES = 10_000_000
MAX_SECONDS = 600


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def atomic_json(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def resolve_inputs(root: Path) -> Path:
    candidates = [root, root / "ITER_1D" / "openmc", root / "openmc"]
    for candidate in candidates:
        if all((candidate / name).is_file() for name in ("geometry.xml", "materials.xml", "iter_1d_source.cpp")):
            return candidate.resolve()
    raise ValueError("benchmark input directory must contain geometry.xml, materials.xml, and iter_1d_source.cpp")


def cylindrical_cell_volumes(root: ET.Element) -> tuple[dict[int, float], dict[str, object]]:
    """Derive exact volumes for this benchmark's concentric annular CSG cells."""
    surfaces = {int(s.get("id", "")): s for s in root.findall("surface")}
    vacuum_radii = [
        float(s.get("coeffs", "").split()[-1]) for s in surfaces.values()
        if s.get("type") == "z-cylinder" and s.get("boundary") == "vacuum"
    ]
    reflective_z = [
        float(s.get("coeffs", "").split()[-1]) for s in surfaces.values()
        if s.get("type") == "z-plane" and s.get("boundary") == "reflective"
    ]
    if len(vacuum_radii) != 1 or len(reflective_z) != 2:
        raise ValueError("expected one vacuum outer cylinder and two reflective axial planes")
    outer_radius = vacuum_radii[0]
    z_min, z_max = min(reflective_z), max(reflective_z)
    if outer_radius <= 0 or z_max <= z_min:
        raise ValueError("invalid outer CSG dimensions")

    volumes: dict[int, float] = {}
    bounds: dict[str, dict[str, float]] = {}
    annuli: list[tuple[float, float, int, float, float]] = []
    for cell in root.findall("cell"):
        cell_id = int(cell.get("id", ""))
        tokens = cell.get("region", "").split()
        if not tokens or any(not (t.lstrip("+-").isdigit()) for t in tokens):
            raise ValueError(f"cell {cell_id} is outside supported simple-halfspace CSG subset")
        r_inner, r_outer = 0.0, math.inf
        cell_z_min, cell_z_max = -math.inf, math.inf
        for token in tokens:
            sign = -1 if token.startswith("-") else 1
            surface = surfaces.get(abs(int(token)))
            if surface is None:
                raise ValueError(f"cell {cell_id} references missing surface {token}")
            coeffs = [float(v) for v in surface.get("coeffs", "").split()]
            if surface.get("type") == "z-cylinder":
                if len(coeffs) != 3 or coeffs[0] != 0.0 or coeffs[1] != 0.0:
                    raise ValueError(f"cell {cell_id} has a nonconcentric/noncircular cylinder")
                radius = coeffs[2]
                if sign > 0:
                    r_inner = max(r_inner, radius)
                else:
                    r_outer = min(r_outer, radius)
            elif surface.get("type") == "z-plane":
                if len(coeffs) != 1:
                    raise ValueError(f"cell {cell_id} has invalid z-plane coefficients")
                z = coeffs[0]
                if sign > 0:
                    cell_z_min = max(cell_z_min, z)
                else:
                    cell_z_max = min(cell_z_max, z)
            else:
                raise ValueError(f"cell {cell_id} uses unsupported surface type {surface.get('type')}")
        if not (math.isfinite(r_inner) and math.isfinite(r_outer) and math.isfinite(cell_z_min)
                and math.isfinite(cell_z_max) and r_outer > r_inner and cell_z_max > cell_z_min):
            raise ValueError(f"cell {cell_id} does not define a finite positive annular volume")
        if not (math.isclose(cell_z_min, z_min, abs_tol=1e-12)
                and math.isclose(cell_z_max, z_max, abs_tol=1e-12)):
            raise ValueError(f"cell {cell_id} does not span the full reflective axial interval")
        volume = math.pi * (r_outer**2 - r_inner**2) * (cell_z_max - cell_z_min)
        volumes[cell_id] = volume
        bounds[str(cell_id)] = {"r_inner_cm": r_inner, "r_outer_cm": r_outer,
                                "z_min_cm": cell_z_min, "z_max_cm": cell_z_max}
        annuli.append((r_inner, r_outer, cell_id, cell_z_min, cell_z_max))

    annuli.sort()
    if not math.isclose(annuli[0][0], 0.0, abs_tol=1e-12):
        raise ValueError("radial cell partition does not start at the axis")
    for left, right in zip(annuli, annuli[1:]):
        if not math.isclose(left[1], right[0], rel_tol=0.0, abs_tol=1e-10):
            raise ValueError(f"radial cells {left[2]} and {right[2]} have a gap or overlap")
    if not math.isclose(annuli[-1][1], outer_radius, rel_tol=0.0, abs_tol=1e-10):
        raise ValueError("radial cell partition does not terminate at the outer vacuum cylinder")
    calculated_total = sum(volumes.values())
    expected_total = math.pi * outer_radius**2 * (z_max - z_min)
    if not math.isclose(calculated_total, expected_total, rel_tol=1e-12, abs_tol=1e-6):
        raise ValueError("analytic annular cell volumes do not close to the full cylinder")
    return volumes, {
        "method": "Exact analytic CSG intersection of concentric z-cylinder annuli and shared z planes",
        "units": "cm^3",
        "outer_radius_cm": outer_radius,
        "z_min_cm": z_min,
        "z_max_cm": z_max,
        "cell_volume_cm3": {str(cell): value for cell, value in sorted(volumes.items())},
        "cell_bounds_cm": bounds,
        "partition_check": "PASS: radial intervals are contiguous, non-overlapping, span axis to vacuum boundary, and sum to pi*R^2*height",
        "partition_sum_cm3": calculated_total,
        "cylinder_volume_cm3": expected_total,
    }


def terminate_group(proc: subprocess.Popen[bytes], grace: float = 5.0) -> None:
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=grace)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", choices=["run"])
    parser.add_argument("--benchmark-dir", type=Path, required=True)
    parser.add_argument("--cross-sections", type=Path, required=True)
    parser.add_argument("--openmc-executable", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=1)
    parser.add_argument("--histories", type=int, default=1_000_000)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--time-limit-sec", type=int, default=MAX_SECONDS)
    args = parser.parse_args()

    output = args.output_dir.expanduser().absolute()
    result_path = output / "reference-result.json"
    owned_output = False
    record: dict[str, object] = {
        "schema": SCHEMA,
        "benchmark": "IAEA-NDS ITER_1D",
        "benchmark_version": "OpenMC input 1.1",
        "execution_status": "NOT_STARTED",
        "scientific_status": "NOT_EVALUATED",
        "comparison_status": "NOT_EVALUATED",
        "comparison_reason": "No provenance-qualified numerical ITER_1D response reference is configured by this control.",
        "seed": args.seed,
        "histories_requested": args.histories,
        "threads": args.threads,
        "time_limit_sec": args.time_limit_sec,
    }
    try:
        if output.exists():
            raise FileExistsError(f"refusing to overwrite existing output directory: {output}")
        output.mkdir(parents=True, exist_ok=False)
        owned_output = True
        if args.histories < 100 or args.histories > MAX_HISTORIES or args.histories % 100 != 0:
            raise ValueError(f"histories must be a multiple of 100 in [100, {MAX_HISTORIES}]")
        if args.threads < 1 or args.threads > 64:
            raise ValueError("threads must be in [1, 64]")
        if args.seed < 1 or args.seed > 2**31 - 1:
            raise ValueError("seed must be a positive 31-bit integer")
        if args.time_limit_sec < 1 or args.time_limit_sec > MAX_SECONDS:
            raise ValueError(f"time limit must be in [1, {MAX_SECONDS}] seconds")
        input_dir = resolve_inputs(args.benchmark_dir.expanduser())
        data_xml = args.cross_sections.expanduser().resolve(strict=True)
        executable = args.openmc_executable.expanduser().resolve(strict=True)
        if not os.access(executable, os.X_OK):
            raise ValueError("OpenMC executable is not executable")
        record["openmc_executable_identity"] = {"path": str(executable), "sha256": sha256(executable)}
        input_copy = output / "inputs"
        input_copy.mkdir()
        copied_names = ["geometry.xml", "materials.xml", "iter_1d_source.cpp"]
        if (input_dir / "CMakeLists.txt").is_file():
            copied_names.append("CMakeLists.txt")
        for name in copied_names:
            shutil.copy2(input_dir / name, input_copy / name)
        record["input_directory"] = str(input_dir)
        record["openmc_executable"] = str(executable)
        record["cross_sections_xml"] = str(data_xml)
        record["input_identities"] = {
            name: {"path": str(input_dir / name), "sha256": sha256(input_dir / name)}
            for name in copied_names
        }
        record["runner_identity"] = {"path": str(Path(__file__).resolve()), "sha256": sha256(Path(__file__).resolve())}
        benchmark_metadata = args.benchmark_dir.expanduser().resolve() / "benchmark_metadata.json"
        if benchmark_metadata.is_file():
            record["benchmark_metadata_identity"] = {"path": str(benchmark_metadata), "sha256": sha256(benchmark_metadata)}
        record["cross_sections_identity"] = {"path": str(data_xml), "sha256": sha256(data_xml)}
        record["bundled_source_library"] = {
            "present": (input_dir / "libsource.so").is_file(),
            "loaded": False,
            "policy": "benchmark library artifact is never loaded; hashed C++ source law is translated through OpenMC's documented Python source API",
        }
        record["execution_status"] = "PREPARING"
        atomic_json(result_path, record)

        # Inspect XML and preserve source cell identities instead of assuming IDs.
        geometry_root = ET.parse(input_copy / "geometry.xml").getroot()
        cell_volumes, volume_audit = cylindrical_cell_volumes(geometry_root)
        material_cell_ids = []
        for cell in geometry_root.findall("cell"):
            material = cell.get("material")
            if material and material.lower() != "void":
                material_cell_ids.append(int(cell.get("id", "")))
        if not material_cell_ids:
            raise ValueError("geometry contains no explicitly material-filled cells")
        record["geometry_audit"] = {
            "cell_count": len(geometry_root.findall("cell")),
            "material_cell_ids": material_cell_ids,
            "source_domain_cell_id": 51,
            "source_cell_present": any(c.get("id") == "51" for c in geometry_root.findall("cell")),
            "source_cell_material": next((c.get("material") for c in geometry_root.findall("cell") if c.get("id") == "51"), None),
            "source_cell_region": next((c.get("region") for c in geometry_root.findall("cell") if c.get("id") == "51"), None),
            "cell_volumes": volume_audit,
        }
        if not record["geometry_audit"]["source_cell_present"]:
            raise ValueError("source implementation targets cell 51, but geometry does not define it")
        if record["geometry_audit"]["source_cell_material"] != "void":
            raise ValueError("source cell 51 is not void as assumed by the source law")
        source_text = (input_copy / "iter_1d_source.cpp").read_text(encoding="utf-8")
        required_source_law_fragments = (
            "uniform_distribution(-1115.2, 1115.2, seed)",
            "uniform_distribution(-1000.2, 1000.2, seed)",
            "int domain_id = 51;",
            "uniform_distribution(0.0, 2*M_PI, seed)",
            "std::acos(openmc::uniform_distribution(-1.0, 1.0, seed))",
            "14.055843863040961e6",
            "0.23863574091114037e6",
            "particle.wgt = 1.0;",
        )
        missing_fragments = [fragment for fragment in required_source_law_fragments if fragment not in source_text]
        if missing_fragments:
            raise ValueError(f"C++ source no longer matches the translated source law: {missing_fragments}")
        record["source_law_check"] = {
            "status": "PASS",
            "checked_against_hashed_cpp_input": True,
            "checks": ["Cartesian bounds", "cell 51 domain", "uniform azimuth and cosine", "Gaussian energy constants", "unit accepted-site weight"],
            "limitation": "This textual contract check does not establish RNG stream equivalence or finite rejection-cap identity.",
        }
        source_bounds = volume_audit["cell_bounds_cm"]["51"]
        source_box_volume = (2.0 * 1115.2) * (2.0 * 1115.2) * (2.0 * 1000.2)
        source_acceptance = cell_volumes[51] / source_box_volume
        record["source_law_check"]["cell_51_bounds_cm"] = source_bounds
        record["source_law_check"]["box_volume_cm3"] = source_box_volume
        record["source_law_check"]["box_proposal_acceptance_probability"] = source_acceptance
        record["source_law_check"]["log10_probability_all_100000_proposals_rejected"] = 100000 * math.log10(1.0 - source_acceptance)

        env_python = Path(sys.executable).resolve()
        record["python"] = str(env_python)
        try:
            import openmc  # type: ignore
        except Exception as exc:
            raise RuntimeError(f"selected Python cannot import OpenMC: {exc}") from exc
        version = str(openmc.__version__)
        record["openmc_python_version"] = version
        record["openmc_python_module"] = str(Path(openmc.__file__).resolve())
        record["python_executable_identity"] = {"path": str(env_python), "sha256": sha256(env_python)}
        if version != "0.15.3":
            raise RuntimeError(f"this audited control requires OpenMC Python 0.15.3, found {version}")

        env = os.environ.copy()
        env["OPENMC_CROSS_SECTIONS"] = str(data_xml)
        cli_version = subprocess.run([str(executable), "--version"], cwd=output, env=env,
                                     capture_output=True, text=True, timeout=10, check=False)
        record["openmc_cli_version_check"] = {
            "returncode": cli_version.returncode,
            "stdout": cli_version.stdout.strip(),
            "stderr": cli_version.stderr.strip(),
        }
        if cli_version.returncode != 0:
            raise RuntimeError(f"OpenMC executable --version exited {cli_version.returncode}")
        record["compiled_source_policy"] = {
            "status": "NOT_USED",
            "bundled_library_loaded": False,
            "compiled_plugin_built_in_this_execution": False,
            "reason": "OpenMC IndependentSource implements the benchmark distribution law; routine runs do not need a C++ plug-in.",
            "separate_smoke_test": {
                "status": "BUILD_SUCCEEDED_BUT_RUNTIME_ABORTED",
                "plugin_sha256": "156959480d2c03b4e378affeaa976d21b070cd3e849c5c8d09af8ad15c3fc9c5",
                "compiler": "system GCC 15.2.0; cached OpenMC build GCC 14.3.0",
                "observation": "OpenMC aborted on first history with stack smashing in custom CompiledSource::sample; root cause undiagnosed; artifact was never used by successful run.",
            },
        }

        # Load the benchmark's exact geometry/materials, then add only settings and tallies.
        materials = openmc.Materials.from_xml(str(input_copy / "materials.xml"))
        geometry = openmc.Geometry.from_xml(str(input_copy / "geometry.xml"), materials=materials)
        source_cell = geometry.get_all_cells().get(51)
        if source_cell is None:
            raise ValueError("OpenMC parsed geometry but could not resolve source cell 51")
        materials.cross_sections = str(data_xml)
        data_library = openmc.data.DataLibrary.from_xml(str(data_xml))
        material_nuclides = sorted({nuc[0] for material in materials for nuc in material.nuclides})
        neutron_paths: dict[str, str] = {}
        for entry in data_library.libraries:
            if entry.get("type") == "neutron":
                for nuclide in entry.get("materials", []):
                    if nuclide in material_nuclides:
                        if nuclide in neutron_paths and neutron_paths[nuclide] != entry["path"]:
                            raise ValueError(f"multiple neutron cross-section files match material nuclide {nuclide}")
                        neutron_paths[nuclide] = entry["path"]
        missing_nuclides = sorted(set(material_nuclides) - set(neutron_paths))
        if missing_nuclides:
            raise ValueError(f"cross_sections.xml has no neutron data entries for material nuclides: {missing_nuclides}")
        record["neutron_data_file_identities"] = [
            {"nuclide": name, "path": neutron_paths[name], "sha256": sha256(Path(neutron_paths[name]).resolve(strict=True))}
            for name in material_nuclides
        ]
        record["photon_transport"] = False
        settings = openmc.Settings()
        settings.run_mode = "fixed source"
        settings.particles = args.histories // 100
        settings.batches = 100
        settings.seed = args.seed
        settings.statepoint = {"batches": [100]}
        settings.temperature = {"default": 293.6, "method": "nearest", "tolerance": 10.0}
        settings.source = openmc.IndependentSource(
            space=openmc.stats.Box((-1115.2, -1115.2, -1000.2), (1115.2, 1115.2, 1000.2)),
            angle=openmc.stats.Isotropic(),
            energy=openmc.stats.Normal(14.055843863040961e6, 0.23863574091114037e6),
            strength=1.0,
            particle="neutron",
            constraints={"domains": [source_cell], "rejection_strategy": "resample"},
        )
        cell_filter = openmc.CellFilter(material_cell_ids)
        flux_tally = openmc.Tally(name="ITER_1D_material_cell_neutron_flux_tracklength")
        flux_tally.filters = [cell_filter]
        flux_tally.scores = ["flux"]
        flux_tally.estimator = "tracklength"
        absorption_tally = openmc.Tally(name="ITER_1D_material_cell_absorption_analog")
        absorption_tally.filters = [cell_filter]
        absorption_tally.scores = ["absorption"]
        absorption_tally.estimator = "analog"
        model = openmc.Model(geometry=geometry, materials=materials, settings=settings,
                             tallies=openmc.Tallies([flux_tally, absorption_tally]))
        model.export_to_xml(directory=str(output))
        record["run_settings"] = {
            "run_mode": "fixed source", "histories_requested": args.histories,
            "batches": 100, "particles_per_batch": settings.particles,
            "effective_histories": settings.particles * settings.batches,
            "seed": args.seed, "threads": args.threads,
            "temperature": {"default_kelvin": 293.6, "method": "nearest", "tolerance_kelvin": 10.0,
                            "basis": "explicit OpenMC room-temperature default; the selected local data have 294 K files only"},
            "source": {
                "implementation": "OpenMC IndependentSource translation of hashed IAEA C++ source distribution",
                "distribution": "uniform Cartesian box, rejection/resampling into cell 51, isotropic direction, normal energy (eV)",
                "box_cm": {"lower_left": [-1115.2, -1115.2, -1000.2], "upper_right": [1115.2, 1115.2, 1000.2]},
                "domain_cell_id": 51,
                "energy_mean_eV": 14.055843863040961e6,
                "energy_sd_eV": 0.23863574091114037e6,
                "strength": 1.0,
                "difference_from_CXX": "The C++ code stops after 100000 failed proposals with a zero-weight site; IndependentSource resamples until acceptance. Geometry-derived proposal acceptance and the log10 probability of 100000 consecutive rejects are retained in source_law_check.",
                "compiled_source_plugin": "Not built or loaded for this run; see compiled_source_policy for the isolated failed smoke-test observation.",
            },
            "tallies": [
                {"name": flux_tally.name, "score": "flux", "estimator": "tracklength",
                 "filters": ["CellFilter(material_cell_ids)"], "units": "particle-cm/source neutron"},
                {"name": absorption_tally.name, "score": "absorption", "estimator": "analog",
                 "filters": ["CellFilter(material_cell_ids)"], "units": "reactions/source neutron"},
            ],
        }
        for name in ("geometry.xml", "materials.xml", "settings.xml", "tallies.xml"):
            p = output / name
            if p.is_file():
                record.setdefault("generated_input_identities", {})[name] = {"sha256": sha256(p)}
        statepoint = output / "statepoint.100.h5"
        record["expected_statepoint_path"] = str(statepoint)
        atomic_json(result_path, record)

        log_out = output / "openmc.stdout.txt"
        log_err = output / "openmc.stderr.txt"
        start = time.monotonic()
        with log_out.open("wb") as stdout, log_err.open("wb") as stderr:
            proc = subprocess.Popen([str(executable), "--threads", str(args.threads)], cwd=output, env=env,
                                    stdout=stdout, stderr=stderr, start_new_session=True)
            try:
                returncode = proc.wait(timeout=args.time_limit_sec)
            except subprocess.TimeoutExpired:
                terminate_group(proc)
                record["execution_status"] = "TIMED_OUT"
                record["error"] = f"OpenMC exceeded {args.time_limit_sec} seconds; process group terminated"
                raise RuntimeError(record["error"])
        record["runtime_seconds"] = time.monotonic() - start
        record["openmc_returncode"] = returncode
        record["statepoint_path"] = str(statepoint) if statepoint.is_file() else None
        record["statepoint_sha256"] = sha256(statepoint) if statepoint.is_file() else None
        record["solver_log_identities"] = {
            "stdout": {"path": str(log_out), "sha256": sha256(log_out)},
            "stderr": {"path": str(log_err), "sha256": sha256(log_err)},
        }
        if returncode != 0 or not statepoint.is_file():
            record["execution_status"] = "FAILED"
            record["error"] = f"OpenMC returned {returncode}; expected statepoint.100.h5 was not produced"
            raise RuntimeError(record["error"])

        with openmc.StatePoint(str(statepoint)) as sp:
            statepoint_version = tuple(int(x) for x in sp.version)
            if statepoint_version != tuple(int(x) for x in version.split(".")):
                raise RuntimeError(f"statepoint OpenMC version {statepoint_version} differs from Python API {version}")
            if sp.run_mode != "fixed source" or sp.seed != args.seed or sp.n_realizations != 100:
                raise RuntimeError("statepoint mode/seed/realization count does not match requested run")
            results = {}
            for expected_name, expected_score, expected_estimator in (
                (flux_tally.name, "flux", "tracklength"),
                (absorption_tally.name, "absorption", "analog"),
            ):
                tally = sp.get_tally(name=expected_name)
                if tally.estimator != expected_estimator or tally.scores != [expected_score]:
                    raise RuntimeError(f"statepoint tally identity mismatch for {expected_name}")
                tally_cell_ids = [int(value) for value in tally.filters[0].bins]
                if tally_cell_ids != material_cell_ids:
                    raise RuntimeError(f"statepoint cell filter identity mismatch for {expected_name}")
                means = tally.mean.ravel().tolist()
                stds = tally.std_dev.ravel().tolist()
                if len(means) != len(material_cell_ids) or len(stds) != len(material_cell_ids):
                    raise RuntimeError(f"unexpected tally bin count for {expected_name}")
                bins = []
                for cell_id, mean, std in zip(material_cell_ids, means, stds):
                    if not (math.isfinite(mean) and mean >= 0 and math.isfinite(std) and std >= 0):
                        raise RuntimeError(f"invalid tally result in {expected_name}, cell {cell_id}")
                    volume = cell_volumes[cell_id]
                    bins.append({"cell_id": cell_id, "volume_cm3": volume,
                                 "mean_per_source": mean, "std_dev_per_source": std,
                                 "volume_average_per_source": mean / volume,
                                 "volume_average_std_dev_per_source": std / volume})
                results[expected_name] = {"score": expected_score, "estimator": expected_estimator,
                                          "filter_cell_ids": tally_cell_ids, "bins": bins}
            global_tallies = []
            for item in sp.global_tallies:
                global_tallies.append({"name": item["name"].decode().rstrip("\x00"),
                                       "mean": float(item["mean"]), "std_dev": float(item["std_dev"])})
            record["statepoint"] = {"version": list(statepoint_version), "run_mode": sp.run_mode,
                                    "seed": int(sp.seed), "realizations": int(sp.n_realizations),
                                    "global_tallies": global_tallies, "results": results}
        record["execution_status"] = "COMPLETE"
        record["scientific_status"] = "NOT_EVALUATED"
        record["comparison_status"] = "NOT_EVALUATED"
        combined_logs = log_out.read_text(errors="replace") + log_err.read_text(errors="replace")
        lost_counts = re.findall(r"(?i)(\d+)\s+particles?\s+were lost", combined_logs)
        record["lost_particle_summary"] = {
            "status": "REPORTED" if lost_counts else "NOT_REPORTED",
            "count": int(lost_counts[-1]) if lost_counts else None,
            "meaning": "A missing explicit lost-particle count is unknown, not zero; raw stdout/stderr and their hashes are preserved.",
        }
        atomic_json(result_path, record)
        print(json.dumps({"execution_status": record["execution_status"],
                          "scientific_status": record["scientific_status"],
                          "result": str(result_path)}, sort_keys=True))
        return 0
    except Exception as exc:
        record["execution_status"] = record.get("execution_status") if record.get("execution_status") == "TIMED_OUT" else "FAILED"
        record["scientific_status"] = "NOT_EVALUATED"
        record["comparison_status"] = "NOT_EVALUATED"
        record["error"] = f"{type(exc).__name__}: {exc}"
        record["traceback"] = traceback.format_exc()
        if owned_output and output.exists() and output.is_dir():
            try:
                atomic_json(result_path, record)
            except Exception:
                pass
        print(json.dumps({"execution_status": record["execution_status"],
                          "scientific_status": "NOT_EVALUATED", "error": record["error"],
                          "result": str(result_path)}, sort_keys=True), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
