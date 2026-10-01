#!/usr/bin/env python3
"""Execute one bounded-input FARIS/OpenMC fixed-source transport job.

The Rust model owns inputs and normalization. This adapter builds OpenMC CSG,
executes the pinned solver, and copies raw per-source means and standard errors
into the strict transport artifact without source-rate scaling.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ET

SCHEMA = "faris-openmc-input/v0.1"
ARTIFACT_SCHEMA = "faris-transport-artifact/v0.1"
TEMP_TOLERANCE_K = 0.1
OPENMC_LABEL_TOLERANCE_K = 1.0
SPECTRUM_EDGES_EV = [1.0e-5, 1.0e3, 1.0e4, 1.0e5, 1.0e6, 2.0e6, 5.0e6, 1.0e7, 1.41e7, 2.0e7]
MAX_SOLVER_LOG_BYTES = 4 * 1024 * 1024


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def read_input(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    require(data.get("schema_version") == SCHEMA, f"input schema must be {SCHEMA}")
    for key in ("manifest", "physics", "request", "sampling", "cross_sections", "openmc_executable"):
        require(key in data, f"missing top-level field: {key}")
    return data


def _terminate_solver(process: subprocess.Popen[bytes], grace_seconds: float = 5.0) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_solver_streaming(command: list[str], cwd: Path, env: dict[str, str], log_limit: int) -> dict:
    """Tee raw solver bytes to the Rust worker pipes and bounded log files.

    Rust owns cancellation, timeout, process-group cleanup, and its independent
    output cap. This layer retains at most ``log_limit`` bytes per stream and
    terminates OpenMC if that local retained-log cap is exceeded.
    """
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    exceeded = threading.Event()
    tee_failed = threading.Event()
    stream_state: dict[str, dict] = {
        "stdout": {"bytes_seen": 0, "bytes_saved": 0, "truncated": False, "error": None},
        "stderr": {"bytes_seen": 0, "bytes_saved": 0, "truncated": False, "error": None},
    }

    def pump(name: str, source, parent_stream, log_path: Path) -> None:
        state = stream_state[name]
        try:
            with log_path.open("wb") as log:
                while True:
                    # BufferedReader.read(n) may wait for n bytes or EOF;
                    # read1 forwards whatever is currently available so Rust
                    # sees output promptly and can enforce cancellation/limits.
                    chunk = source.read1(64 * 1024)
                    if not chunk:
                        break
                    state["bytes_seen"] += len(chunk)
                    remaining = max(0, log_limit - state["bytes_saved"])
                    saved = chunk[:remaining]
                    if saved:
                        log.write(saved)
                        state["bytes_saved"] += len(saved)
                    if len(saved) != len(chunk):
                        state["truncated"] = True
                        exceeded.set()
                    try:
                        parent_stream.write(chunk)
                        parent_stream.flush()
                    except (BrokenPipeError, OSError) as error:
                        state["error"] = f"parent log pipe: {error}"
                        tee_failed.set()
                log.flush()
        except Exception as error:
            state["error"] = f"log capture: {type(error).__name__}: {error}"
            tee_failed.set()

    require(process.stdout is not None and process.stderr is not None, "failed to create solver output pipes")
    stdout_thread = threading.Thread(
        target=pump, args=("stdout", process.stdout, sys.stdout.buffer, cwd / "openmc.stdout.log"), daemon=True
    )
    stderr_thread = threading.Thread(
        target=pump, args=("stderr", process.stderr, sys.stderr.buffer, cwd / "openmc.stderr.log"), daemon=True
    )
    stdout_thread.start()
    stderr_thread.start()
    stop_reason = None
    while process.poll() is None:
        if exceeded.is_set():
            stop_reason = f"solver log exceeded {log_limit} bytes on at least one stream"
            _terminate_solver(process)
            break
        if tee_failed.is_set():
            stop_reason = "failed to forward or retain solver output"
            _terminate_solver(process)
            break
        time.sleep(0.02)
    return_code = process.wait()
    stdout_thread.join()
    stderr_thread.join()
    if stop_reason is None and exceeded.is_set():
        stop_reason = f"solver log exceeded {log_limit} bytes on at least one stream"
    if stop_reason is None and tee_failed.is_set():
        stop_reason = "failed to forward or retain solver output"
    for name in ("stdout", "stderr"):
        path = cwd / f"openmc.{name}.log"
        stream_state[name]["file"] = path.name
        stream_state[name]["sha256"] = sha256(path)
    return {
        "return_code": return_code,
        "stop_reason": stop_reason,
        "log_limit_bytes_per_stream": log_limit,
        "streams": stream_state,
        "capture_policy": "bounded local prefix; complete raw bytes tee to parent Rust job output pipes",
    }


def check_data_identity(physics: dict, xml_path: Path, openmc) -> tuple[str, list[str], dict[str, float], float]:
    require(xml_path.is_file(), f"cross_sections XML does not exist: {xml_path}")
    xml_hash = sha256(xml_path)
    selection = physics["nuclear_data"]
    root = ET.parse(xml_path).getroot()
    neutron = {}
    for library in root.findall("library"):
        if library.get("type") == "neutron":
            for nuclide in library.get("materials", "").split():
                neutron[nuclide] = library.get("path")
    recipes = [m["recipe"] for m in physics["materials"] if m["recipe"]["kind"] == "nuclide_mixture"]
    required = sorted({n["nuclide"] for recipe in recipes for n in recipe["nuclides"]})
    actual_temps = {}
    runtime_labels = set()
    if selection.get("state") == "inventory":
        data_root = xml_path.parent
        declared = {}
        for item in selection.get("files", []):
            p = (data_root / item["relative_path"]).resolve()
            require(p.is_relative_to(data_root.resolve()), "nuclear-data path escaped selected library root")
            require(p.is_file(), f"selected nuclear-data file missing: {item['relative_path']}")
            require(p.stat().st_size == item["size_bytes"], f"nuclear-data size changed: {item['file_id']}")
            require(item["sha256"] == f"sha256:{sha256(p)}", f"nuclear-data hash changed: {item['file_id']}")
            for nuc in item["nuclides"]:
                declared[nuc] = (item, p)
        for nuclide in required:
            require(nuclide in declared, f"selected nuclear-data inventory omits {nuclide}")
            item, file_path = declared[nuclide]
            require(nuclide in neutron, f"cross_sections XML has no neutron entry for {nuclide}")
            require(item["relative_path"] == neutron[nuclide], f"XML/data-inventory path mismatch for {nuclide}")
            data = openmc.data.IncidentNeutron.from_hdf5(str(file_path))
            stored_temperatures = [float(kT) / openmc.data.K_BOLTZMANN for kT in data.kTs]
            require(len(stored_temperatures) == len(item["temperatures_k"]), f"audited temperature count changed for {nuclide}")
            require(all(any(abs(a-b) <= 1.0e-8 for b in item["temperatures_k"]) for a in stored_temperatures), f"audited stored temperatures changed for {nuclide}")
            actual_temps[nuclide] = min(stored_temperatures)
            runtime_labels.update(data.temperatures)
    else:
        for nuclide in required:
            require(nuclide in neutron, f"cross_sections XML has no neutron entry for {nuclide}")
            file_path = (xml_path.parent / neutron[nuclide]).resolve(strict=True)
            data = openmc.data.IncidentNeutron.from_hdf5(str(file_path))
            actual_temps[nuclide] = min(float(kT) / openmc.data.K_BOLTZMANN for kT in data.kTs)
            runtime_labels.update(data.temperatures)
    require(len(runtime_labels) == 1, "this adapter requires a common labeled neutron-data temperature across its recipes")
    label = next(iter(runtime_labels))
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)K", label)
    require(match is not None, f"unsupported neutron data temperature label: {label}")
    runtime_temp = float(match.group(1))
    for nuclide in required:
        require(nuclide in actual_temps, f"no stored temperature found for {nuclide}")
    for recipe in recipes:
        target = recipe["nuclear_data_temperature_k"]
        for nuclide in (n["nuclide"] for n in recipe["nuclides"]):
            require(abs(actual_temps[nuclide] - target) <= TEMP_TOLERANCE_K, f"{nuclide} stored numeric temperature is outside {TEMP_TOLERANCE_K} K of requested {target}")
    require(abs(runtime_temp - min(actual_temps.values())) < 1.0, "rounded OpenMC data label is inconsistent with exact stored temperatures")
    return xml_hash, required, actual_temps, runtime_temp


def compose(inp: dict, out: Path, openmc, data_runtime_temperature: float):
    manifest = inp["manifest"]
    physics = inp["physics"]
    request = inp["request"]
    sampling = inp["sampling"]
    variant = next((v for v in manifest["variants"] if v["id"] == physics["variant_id"]), None)
    require(variant is not None, "physics variant missing from manifest")
    require(request["scenario_id"] == manifest["scenario_id"] and request["scenario_sha256"] == manifest["source_sha256"], "request/manifest identity mismatch")
    require(request["variant_id"] == physics["variant_id"], "request/physics variant mismatch")
    require(request["fusion_power_mw"] == manifest["fusion_power_mw"], "request fusion power differs from manifest")
    require(physics["scenario_sha256"] == manifest["source_sha256"], "physics/manifest hash mismatch")
    require(physics["source"]["spatial_distribution"] == "uniform_circular_plasma_torus", "unsupported spatial source")
    require(physics["source"]["angular_distribution"] == "isotropic" and physics["source"]["energy_distribution"] == "monoenergetic", "unsupported source recipe")
    require(all(physics["source"][k] == request["source"][k] for k in ("energy_per_reaction_ev", "neutron_energy_ev", "neutrons_per_reaction")), "source/request mismatch")
    require(sampling["threads"] >= 1 and sampling["batches"] >= 1 and sampling["particles_per_batch"] >= 1, "sampling values must be positive integers")

    material_defs = {m["id"]: m["recipe"] for m in physics["materials"]}
    assignments = {a["component_id"]: a["material_id"] for a in physics["component_assignments"]}
    require(len(assignments) == len(variant["components"]), "every variant component must have one physics assignment")
    materials = {}
    for material_id, recipe in material_defs.items():
        if recipe["kind"] == "void":
            # OpenMC represents a geometric void with a null-filled cell, not
            # a zero-density Material (which is rejected by the solver).
            materials[material_id] = None
            continue
        else:
            mat = openmc.Material(name=material_id)
            for nuclide in recipe["nuclides"]:
                mat.add_nuclide(nuclide["nuclide"], nuclide["atom_fraction"], percent_type="ao")
            mat.set_density("kg/m3", recipe["density_kg_m3"])
            # OpenMC CE groups are addressed by their rounded label (e.g.
            # 294K); the exact HDF5 kT value is separately checked above.
            mat.temperature = data_runtime_temperature
        materials[material_id] = mat

    R = float(manifest["major_radius_m"])
    plasma_minor = float(manifest["plasma_minor_radius_m"])
    require(bool(variant["components"]), "variant has no components")
    gap = float(variant["components"][0]["inner_minor_radius_m"]) - plasma_minor
    require(gap >= 0.0, "first component begins inside the plasma minor radius")
    require(math.isfinite(R) and R > 0 and plasma_minor > 0, "invalid torus dimensions")
    scale = 100.0
    plasma_surface = openmc.YTorus(a=R * scale, b=plasma_minor * scale, c=plasma_minor * scale)
    require("void" in materials, "explicit void material required for plasma and clearance")
    plasma = openmc.Cell(name="plasma-source-domain", fill=materials["void"], region=-plasma_surface)
    surfaces = []
    cells = [plasma]
    component_cells = {}
    component_filters = {}
    inner = plasma_minor + gap
    first_inner = inner
    for component in variant["components"]:
        rid = component["id"]
        require(rid in assignments and assignments[rid] == component["material_id"], f"material assignment mismatch for {rid}")
        require(component["material_id"] in materials, f"material missing for component {rid}")
        inner = float(component["inner_minor_radius_m"])
        outer = float(component["outer_minor_radius_m"])
        require(math.isclose(inner, first_inner if not surfaces else float(surfaces[-1].c) / scale, abs_tol=1e-8), f"non-contiguous radial geometry before {rid}")
        surf = openmc.YTorus(a=R * scale, b=outer * scale, c=outer * scale)
        surfaces.append(surf)
        inner_surface = plasma_surface if len(surfaces) == 1 else surfaces[-2]
        cell = openmc.Cell(name=rid, fill=materials[component["material_id"]], region=+inner_surface & -surf)
        cells.append(cell)
        component_cells[rid] = cell
        component_filters[rid] = openmc.CellFilter(cell)
    outer_minor = float(variant["components"][-1]["outer_minor_radius_m"])
    require(outer_minor < R, "outer torus minor radius must remain below major radius")
    outer_surface = surfaces[-1]
    outer_surface.boundary_type = "vacuum"
    outside = openmc.Cell(name="outside-torus-void", fill=materials["void"], region=+outer_surface)
    cells.append(outside)
    root = openmc.Universe(cells=cells)
    geometry = openmc.Geometry(root)

    low = -(R + outer_minor) * scale
    high = (R + outer_minor) * scale
    ylow, yhigh = -outer_minor * scale, outer_minor * scale
    source = openmc.IndependentSource(
        space=openmc.stats.Box((low, ylow, low), (high, yhigh, high)),
        angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([request["source"]["neutron_energy_ev"]], [1.0]),
        particle="neutron",
        constraints={"domains": [plasma], "rejection_strategy": "resample"},
    )
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.batches = int(sampling["batches"])
    settings.particles = int(sampling["particles_per_batch"])
    settings.seed = int(sampling["seed"])
    settings.photon_transport = False
    settings.source = source
    # HDF kT is checked against the requested target at 0.1 K. OpenMC 0.15.3
    # resolves continuous-energy tables through rounded labels (e.g. 294K),
    # so this separate 1 K window only admits that label rounding at runtime.
    settings.temperature = {"default": data_runtime_temperature, "method": "nearest", "tolerance": OPENMC_LABEL_TOLERANCE_K}

    spectrum_tallies = {}
    for component_id, cell in component_cells.items():
        spectrum = openmc.Tally(name=f"spectrum-{component_id}")
        spectrum.filters = [component_filters[component_id], openmc.EnergyFilter(SPECTRUM_EDGES_EV)]
        spectrum.scores = ["flux"]
        spectrum.estimator = "tracklength"
        spectrum_tallies[component_id] = spectrum
    tallies = openmc.Tallies(list(spectrum_tallies.values()))
    response_by_tally = {}
    for index, response in enumerate(request["responses"], start=1):
        domain = response["domain"]
        score = response["score"]
        tally = openmc.Tally(name=f"response-{response['id']}")
        if domain["kind"] == "component":
            comp = domain["component_id"]
            require(comp in component_cells, f"response references unknown component {comp}")
            tally.filters = [component_filters[comp]]
        elif domain["kind"] == "whole_model":
            pass
        elif domain["kind"] == "mesh":
            # The full mesh tally is constructed once below; per-bin definitions map to its bins.
            continue
        else:
            raise ValueError(f"unsupported response domain {domain['kind']}")
        kind = score["kind"]
        if kind == "flux":
            tally.scores = ["flux"]
        elif kind == "reaction_rate":
            tally.scores = [score["reaction"]]
        elif kind == "particle_production":
            tally.scores = [score["score"]]
        else:
            raise ValueError(f"unsupported or unavailable score kind: {kind}")
        tally.estimator = "tracklength"
        tallies.append(tally)
        response_by_tally[tally.name] = response

    mesh_meta = inp.get("mesh")
    mesh_response_defs = [r for r in request["responses"] if r["domain"]["kind"] == "mesh"]
    mesh_tally = None
    mesh_index_audit = None
    if mesh_response_defs:
        require(mesh_meta is not None, "mesh-domain responses require mesh input metadata")
        dims = tuple(int(v) for v in mesh_meta["dimensions"])
        require(len(dims) == 3 and math.prod(dims) <= 4096, "mesh dimensions exceed adapter bound")
        mesh = openmc.RegularMesh()
        mesh.dimension = dims
        mesh.lower_left = [100.0 * v for v in mesh_meta["lower_left_m"]]
        mesh.upper_right = [100.0 * v for v in mesh_meta["upper_right_m"]]
        api_indices = list(mesh.indices)
        expected_indices = [
            (x, y, z)
            for z in range(1, dims[2] + 1)
            for y in range(1, dims[1] + 1)
            for x in range(1, dims[0] + 1)
        ]
        require(api_indices == expected_indices, "OpenMC RegularMesh.indices order disagrees with requested x-fast bin order")
        require(api_indices[0] == (1, 1, 1), "OpenMC mesh bin zero is not index (1,1,1)")
        require(api_indices[1] == (2, 1, 1), "OpenMC mesh bin 1 does not increment x first")
        require(api_indices[dims[0]] == (1, 2, 1), "OpenMC mesh x-row rollover does not increment y")
        require(api_indices[dims[0] * dims[1]] == (1, 1, 2), "OpenMC mesh plane rollover does not increment z")
        mesh_index_audit = {
            "api": "openmc.RegularMesh.indices",
            "ordering": "x-fastest, then y, then z; 1-based tuple indices correspond to zero-based flat bin i + nx*(j + ny*k)",
            "first_index_tuples": [list(v) for v in api_indices[: min(4, len(api_indices))]],
            "tested_rollovers": {
                "bin_0": list(api_indices[0]),
                "bin_1": list(api_indices[1]),
                "bin_nx": list(api_indices[dims[0]]),
                "bin_nx_times_ny": list(api_indices[dims[0] * dims[1]]),
            },
            "assertion": "PASS",
        }
        mesh_tally = openmc.Tally(name="spatial-neutron-flux")
        mesh_tally.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter("neutron")]
        mesh_tally.scores = ["flux"]
        mesh_tally.estimator = "tracklength"
        tallies.append(mesh_tally)
        expected = {int(r["domain"]["bin"]): r for r in mesh_response_defs}
        require(set(expected) == set(range(math.prod(dims))), "mesh response bins must exactly cover the declared mesh")
        require(all(r["domain"]["mesh_id"] == mesh_meta["id"] for r in mesh_response_defs), "mesh ID mismatch")

    model = openmc.Model(geometry=geometry, materials=openmc.Materials([m for m in materials.values() if m is not None]), settings=settings, tallies=tallies)
    model.export_to_xml(directory=str(out))
    # Exact analytic full-torus volume for component shells and the whole modeled outer torus.
    volumes_cm3 = {}
    for c in variant["components"]:
        a, b = c["inner_minor_radius_m"], c["outer_minor_radius_m"]
        volumes_cm3[c["id"]] = 2.0 * math.pi**2 * R * (b*b-a*a) * 1.0e6
        expected_m3 = 2.0 * math.pi**2 * R * (b*b-a*a)
        require(math.isclose(float(c["full_torus_volume_m3"]), expected_m3, rel_tol=1.0e-12, abs_tol=1.0e-12), f"manifest torus volume mismatch for {c['id']}")
    whole_minor = plasma_minor + gap + sum(float(c["thickness_m"]) for c in variant["components"])
    whole_volume_cm3 = 2.0 * math.pi**2 * R * whole_minor**2 * 1.0e6
    mesh_bin_volume_cm3 = None
    if mesh_meta:
        widths = [(mesh_meta["upper_right_m"][i]-mesh_meta["lower_left_m"][i])/mesh_meta["dimensions"][i] for i in range(3)]
        mesh_bin_volume_cm3 = math.prod(widths)*1.0e6
    return model, response_by_tally, mesh_tally, expected if mesh_tally else {}, volumes_cm3, whole_volume_cm3, mesh_bin_volume_cm3, component_cells, spectrum_tallies, plasma, mesh_index_audit


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    out = args.output_dir.expanduser().resolve()
    require(not out.exists(), "output directory must be fresh; refusing overwrite")
    out.mkdir(parents=True)
    record = {"execution_status": "NOT_STARTED", "scientific_status": "NOT_EVALUATED"}
    solver_output = None
    statepoint_identity = None
    exported_xml_hashes = {}
    mesh_index_audit = None
    try:
        inp = read_input(args.input.expanduser().resolve(strict=True))
        exe = Path(inp["openmc_executable"]).expanduser().resolve(strict=True)
        xml = Path(inp["cross_sections"]).expanduser().resolve(strict=True)
        require(os.access(exe, os.X_OK), "OpenMC executable is not executable")
        data_digest = inp["nuclear_data_digest"]
        require(data_digest.startswith("sha256:") and len(data_digest) == 71, "nuclear_data_digest must be a sha256-prefixed 64-hex digest")
        import openmc
        require(openmc.__version__ == "0.15.3", f"expected OpenMC 0.15.3, got {openmc.__version__}")
        xml_hash, required_nuclides, actual_data_temps, runtime_data_temperature = check_data_identity(inp["physics"], xml, openmc)
        # Set only the path; all scored quantities remain raw, per source neutron.
        os.environ["OPENMC_CROSS_SECTIONS"] = str(xml)
        model, response_by_tally, mesh_tally, mesh_defs, volumes_cm3, whole_cm3, mesh_bin_cm3, component_cells, spectrum_tallies, plasma, mesh_index_audit = compose(inp, out, openmc, runtime_data_temperature)
        exported_xml = sorted(out.glob("*.xml"))
        require(exported_xml, "OpenMC model export produced no XML inputs")
        exported_xml_hashes = {path.name: sha256(path) for path in exported_xml}
        command = [str(exe)]
        run_env = os.environ.copy()
        run_env["OMP_NUM_THREADS"] = str(int(inp["sampling"]["threads"]))
        run_env.setdefault("OPENBLAS_NUM_THREADS", "1")
        solver_output = run_solver_streaming(command, out, run_env, MAX_SOLVER_LOG_BYTES)
        require(solver_output["stop_reason"] is None, solver_output["stop_reason"] or "solver output forwarding failed")
        require(solver_output["return_code"] == 0, f"OpenMC exited {solver_output['return_code']}; inspect bounded output logs")
        log = (out / "openmc.stdout.log").read_bytes() + b"\n" + (out / "openmc.stderr.log").read_bytes()
        require(not re.search(rb"(?i)particle\s+\d+\s+was lost|lost particles?", log), "OpenMC reported lost particles")
        statepoints = sorted(out.glob("statepoint.*.h5"))
        require(statepoints, "OpenMC produced no statepoint")
        require(len(statepoints) == 1, "expected exactly one final statepoint")
        statepoint_identity = {"filename": statepoints[-1].name, "size_bytes": statepoints[-1].stat().st_size, "sha256": sha256(statepoints[-1])}
        with openmc.StatePoint(str(statepoints[-1])) as sp:
            expected_version = tuple(int(part) for part in openmc.__version__.split(".")[:3])
            observed_version = tuple(int(part) for part in sp.version)
            require(observed_version == expected_version == (0, 15, 3), f"statepoint version {observed_version} does not match requested OpenMC {expected_version}")
            require(sp.run_mode == "fixed source", f"statepoint run mode is {sp.run_mode!r}, not fixed source")
            require(int(sp.seed) == int(inp["sampling"]["seed"]), "statepoint seed differs from request")
            require(int(sp.n_batches) == int(inp["sampling"]["batches"]), "statepoint n_batches differs from request")
            require(int(sp.current_batch) == int(inp["sampling"]["batches"]), "statepoint current_batch is incomplete")
            require(int(sp.n_realizations) == int(inp["sampling"]["batches"]), "statepoint realization count differs from requested batches")
            require(int(sp.n_particles) == int(inp["sampling"]["particles_per_batch"]), "statepoint particles per batch differs from request")
            raw_tallies = []
            for name, response in response_by_tally.items():
                tally = sp.get_tally(name=name)
                require(tally.estimator == "tracklength", f"response {response['id']} estimator is not explicitly tracklength")
                means = tally.mean.ravel()
                errors = tally.std_dev.ravel()
                require(len(means) == 1 and len(errors) == 1, f"response {response['id']} did not produce one scalar")
                mean, se = float(means[0]), float(errors[0])
                require(math.isfinite(mean) and mean >= 0 and math.isfinite(se) and se >= 0, f"response {response['id']} is non-finite/negative")
                domain = response["domain"]
                if domain["kind"] == "component":
                    volume = volumes_cm3[domain["component_id"]]
                elif domain["kind"] == "whole_model":
                    volume = whole_cm3
                else:
                    raise ValueError("unsupported response domain")
                score = response["score"]["kind"]
                unit = "cm_per_source" if score == "flux" else ("particles_per_source" if score == "particle_production" else "events_per_source")
                raw_tallies.append({"response_id": response["id"], "estimator": tally.estimator, "unit": unit, "mean": mean, "standard_error": se})
            if mesh_tally is not None:
                tally = sp.get_tally(name=mesh_tally.name)
                require(tally.estimator == "tracklength", "spatial flux estimator is not explicitly tracklength")
                means = tally.mean.ravel(order="F")
                errors = tally.std_dev.ravel(order="F")
                require(len(means) == len(mesh_defs) and len(errors) == len(mesh_defs), "mesh tally bin count mismatch")
                for bin_id, response in sorted(mesh_defs.items()):
                    mean, se = float(means[bin_id]), float(errors[bin_id])
                    require(math.isfinite(mean) and mean >= 0 and math.isfinite(se) and se >= 0, f"mesh bin {bin_id} non-finite/negative")
                    raw_tallies.append({"response_id": response["id"], "estimator": tally.estimator, "unit": "cm_per_source", "mean": mean, "standard_error": se})
            spectra = []
            for component_id, expected_tally in spectrum_tallies.items():
                tally = sp.get_tally(name=expected_tally.name)
                require(tally.estimator == "tracklength", f"spectrum estimator is not explicitly tracklength for {component_id}")
                means = tally.mean.ravel()
                errors = tally.std_dev.ravel()
                require(len(means) == len(SPECTRUM_EDGES_EV) - 1, f"spectrum bin count mismatch for {component_id}")
                spectra.append({"component_id": component_id, "score": "flux", "unit": "cm_per_source_per_energy_bin", "estimator": tally.estimator, "energy_edges_eV": SPECTRUM_EDGES_EV, "mean": [float(x) for x in means], "standard_error": [float(x) for x in errors], "volume_cm3": volumes_cm3[component_id]})
            volumes = []
            for response in inp["request"]["responses"]:
                d = response["domain"]
                if d["kind"] == "component":
                    v = volumes_cm3[d["component_id"]]
                elif d["kind"] == "whole_model":
                    v = whole_cm3
                elif d["kind"] == "mesh":
                    v = mesh_bin_cm3
                else:
                    raise ValueError(f"unsupported volume domain: {d['kind']}")
                if not any(item["domain"] == d for item in volumes):
                    volumes.append({"domain": d, "value": v, "unit": "cubic_centimetre"})
            artifact = {"schema_version": ARTIFACT_SCHEMA, "request": inp["request"], "solver": {"name": "OpenMC", "version": openmc.__version__, "digest": f"sha256:{sha256(exe)}"}, "nuclear_data": {"name": inp["physics"].get("nuclear_data", {}).get("name", "external cross_sections.xml"), "version": inp["physics"].get("nuclear_data", {}).get("version", "unselected-local-library"), "digest": data_digest}, "histories": int(sp.n_realizations)*int(inp["sampling"]["particles_per_batch"]), "volumes": volumes, "tallies": raw_tallies}
            (out / "transport-artifact.json").write_text(json.dumps(artifact, indent=2, sort_keys=True)+"\n", encoding="utf-8")
            (out / "transport-spectra.json").write_text(json.dumps({"schema":"faris-openmc-spectra/v0.1","scientific_status":"NOT_EVALUATED","note":"Auxiliary OpenMC cell-filtered volume-integrated flux spectra, per source neutron; no absolute source normalization.","components":spectra}, indent=2)+"\n", encoding="utf-8")
        after_export_xml_hashes = {path.name: sha256(path) for path in sorted(out.glob("*.xml"))}
        require(after_export_xml_hashes == exported_xml_hashes, "OpenMC export XML changed during solver execution")
        record.update({"execution_status":"COMPLETED","scientific_status":"NOT_EVALUATED","histories":artifact["histories"],"solver":artifact["solver"],"nuclear_data":artifact["nuclear_data"],"requested_nuclear_data_temperature_K":sorted({m["recipe"]["nuclear_data_temperature_k"] for m in inp["physics"]["materials"] if m["recipe"]["kind"] == "nuclide_mixture"}),"stored_nuclear_data_temperatures_K":sorted(set(actual_data_temps.values())),"openmc_data_group_temperature_label_K":runtime_data_temperature,"openmc_nearest_label_tolerance_K":OPENMC_LABEL_TOLERANCE_K,"openmc_statepoint_version":list(observed_version),"statepoint":statepoint_identity,"export_xml_sha256":exported_xml_hashes,"solver_output_capture":solver_output,"mesh_index_audit":mesh_index_audit,"transport_artifact":"transport-artifact.json","responses":len(raw_tallies),"normalization":"RAW_PER_SOURCE_NEUTRON; no absolute source normalization in Python","lost_particle_check":"no lost-particle log indication; statepoint present"})
    except Exception as error:
        record.update({"execution_status":"FAILED","scientific_status":"NOT_EVALUATED","error":f"{type(error).__name__}: {error}"})
        if solver_output is not None:
            record["solver_output_capture"] = solver_output
        if statepoint_identity is not None:
            record["statepoint"] = statepoint_identity
        if exported_xml_hashes:
            record["export_xml_sha256"] = exported_xml_hashes
        if mesh_index_audit is not None:
            record["mesh_index_audit"] = mesh_index_audit
        (out / "worker-result.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
        raise
    (out / "worker-result.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
