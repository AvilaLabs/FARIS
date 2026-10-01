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
import random
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ET

SCHEMA = "faris-openmc-input/v0.1"
ARTIFACT_SCHEMA = "faris-transport-artifact/v0.2"
TEMP_TOLERANCE_K = 0.1
OPENMC_LABEL_TOLERANCE_K = 1.0
# Full transport energy coverage for the authored source and its secondary
# photons; the spectrum-bin sum is checked against integrated neutron flux.
SPECTRUM_EDGES_EV = [0.0, 1.0e3, 1.0e4, 1.0e5, 1.0e6, 2.0e6, 5.0e6, 1.0e7, 1.41e7, 2.0e7, 1.0e9]
MAX_SOLVER_LOG_BYTES = 4 * 1024 * 1024
INTEGRATED_RSE_REVIEW_GOAL = 0.05
LOCAL_RSE_REVIEW_GOAL = 0.10


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def sampling_precision_report(request: dict, tallies: list[dict], volumes: list[dict]) -> dict:
    """Report predeclared exploratory precision goals, never a physics verdict."""
    by_id = {t["response_id"]: t for t in tallies}
    volume_by_domain = {json.dumps(v["domain"], sort_keys=True): v for v in volumes}
    checks = []

    def add(response_id: str, goal: float, quantity: str, include_volume_error: bool) -> None:
        tally = by_id.get(response_id)
        if tally is None:
            return
        score = request["responses"]
        definition = next(r for r in score if r["id"] == response_id)
        mean = float(tally["mean"])
        se = float(tally["standard_error"])
        relative = None if mean == 0.0 else se / abs(mean)
        if include_volume_error and mean > 0.0:
            volume = volume_by_domain[json.dumps(definition["domain"], sort_keys=True)]
            relative = math.hypot(relative or 0.0, float(volume["standard_error"]) / float(volume["value"]))
        checks.append({
            "response_id": response_id,
            "quantity": quantity,
            "target_relative_standard_error": goal,
            "observed_relative_standard_error": relative,
            "met": relative is not None and relative <= goal,
        })

    by_definition = {r["id"]: r for r in request["responses"]}
    for response_id, definition in by_definition.items():
        score = definition["score"]
        domain = definition["domain"]
        if response_id == "total-tritium-production":
            add(response_id, INTEGRATED_RSE_REVIEW_GOAL, "whole-model tritium production", False)
        elif score["kind"] == "heating" and score["particle_scope"] == "total":
            if domain["kind"] == "whole_model":
                add(response_id, INTEGRATED_RSE_REVIEW_GOAL, "whole-model deposited heating", False)
            elif domain["kind"] == "component":
                add(response_id, INTEGRATED_RSE_REVIEW_GOAL, "integrated component deposited heating", False)
                add(response_id, LOCAL_RSE_REVIEW_GOAL, "volume-averaged component deposited heating", True)
        elif score["kind"] == "flux" and (domain["kind"] == "mesh" or domain.get("component_id") == "magnets"):
            add(response_id, LOCAL_RSE_REVIEW_GOAL, "magnet or mesh neutron flux", True)
    return {
        "plan_id": "faris-exploratory-precision-goals/v0.1",
        "purpose": "numerical sampling review only; not a physics or design acceptance test",
        "estimator": "one-standard-error relative to the response mean; zero means have undefined RSE and fail the check",
        "integrated_goal": INTEGRATED_RSE_REVIEW_GOAL,
        "local_goal": LOCAL_RSE_REVIEW_GOAL,
        "checks": checks,
        "all_goals_met": bool(checks) and all(item["met"] for item in checks),
    }


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


def check_data_identity(physics: dict, xml_path: Path, openmc, coupled_heating_required: bool) -> tuple[str, list[str], dict[str, float], float, dict[str, str], dict[str, object]]:
    require(xml_path.is_file(), f"cross_sections XML does not exist: {xml_path}")
    xml_hash = sha256(xml_path)
    selection = physics["nuclear_data"]
    root = ET.parse(xml_path).getroot()
    neutron = {}
    photon = {}
    for library in root.findall("library"):
        if library.get("type") == "neutron":
            for nuclide in library.get("materials", "").split():
                neutron[nuclide] = library.get("path")
        elif library.get("type") == "photon":
            for element in library.get("materials", "").split():
                photon[element] = library.get("path")
    recipes = [m["recipe"] for m in physics["materials"] if m["recipe"]["kind"] == "nuclide_mixture"]
    required = sorted({n["nuclide"] for recipe in recipes for n in recipe["nuclides"]})
    actual_temps = {}
    runtime_labels = set()
    photon_hashes = {}
    photon_physics = {"atomic_relaxation_available": None, "atomic_relaxation_enabled": False, "electron_treatment": None}
    if selection.get("state") == "inventory":
        data_root = xml_path.parent
        declared = {}
        declared_photon = {}
        for item in selection.get("files", []):
            p = (data_root / item["relative_path"]).resolve()
            require(p.is_relative_to(data_root.resolve()), "nuclear-data path escaped selected library root")
            require(p.is_file(), f"selected nuclear-data file missing: {item['relative_path']}")
            require(p.stat().st_size == item["size_bytes"], f"nuclear-data size changed: {item['file_id']}")
            require(item["sha256"] == f"sha256:{sha256(p)}", f"nuclear-data hash changed: {item['file_id']}")
            for nuc in item["nuclides"]:
                if "continuous_energy_neutron_transport" in item["capabilities"]:
                    declared[nuc] = (item, p)
                if "photon_transport" in item["capabilities"]:
                    element = re.match(r"[A-Z][a-z]?", nuc)
                    require(element is not None, f"invalid photon-element isotope name: {nuc}")
                    declared_photon[element.group(0)] = (item, p)
        for nuclide in required:
            require(nuclide in declared, f"selected nuclear-data inventory omits {nuclide}")
            item, file_path = declared[nuclide]
            require(nuclide in neutron, f"cross_sections XML has no neutron entry for {nuclide}")
            require(item["relative_path"] == neutron[nuclide], f"XML/data-inventory path mismatch for {nuclide}")
            if coupled_heating_required:
                require("heating" in item["capabilities"], f"MT=301 heating capability is not declared for {nuclide}")
            data = openmc.data.IncidentNeutron.from_hdf5(str(file_path))
            if coupled_heating_required:
                require(301 in data.reactions, f"MT=301 heating coefficients missing for {nuclide}")
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
            if coupled_heating_required:
                require(301 in data.reactions, f"MT=301 heating coefficients missing for {nuclide}")
            runtime_labels.update(data.temperatures)
    used_elements = sorted({re.match(r"[A-Z][a-z]?", nuclide).group(0) for nuclide in required}) if coupled_heating_required else []
    for element in used_elements:
        require(element in photon, f"cross_sections XML has no photon atomic data entry for {element}")
        if selection.get("state") == "inventory":
            require(element in declared_photon, f"selected nuclear-data inventory omits photon data for {element}")
            item, file_path = declared_photon[element]
            require(item["relative_path"] == photon[element], f"photon XML/data-inventory path mismatch for {element}")
            require(not coupled_heating_required or "atomic_relaxation" in item["capabilities"], f"atomic-relaxation shell map capability is not declared for {element}")
        else:
            file_path = (xml_path.parent / photon[element]).resolve(strict=True)
        data = openmc.data.IncidentPhoton.from_hdf5(str(file_path))
        require(data.name == element and bool(data.reactions), f"photon data unreadable/incomplete for {element}")
        relaxation = data.atomic_relaxation
        # Require binding-energy and electron-count records for every
        # photoelectric shell. Low-Z elements can legitimately have no
        # relaxation transitions; an empty object/map for every shell is
        # the unsafe condition that crashed with the FENDL photon files.
        import h5py
        with h5py.File(file_path, "r") as library:
            shells = set(library[element]["subshells"].keys())
        populated = bool(
            relaxation is not None
            and shells
            and shells <= set(relaxation.binding_energy)
            and shells <= set(relaxation.num_electrons)
        )
        require(populated, f"atomic-relaxation binding/electron shell map incomplete for {element}")
        photon_physics.setdefault("atomic_relaxation_available_by_element", {})[element] = {
            "shell_map_complete": populated,
            "photoelectric_shell_count": len(shells),
            "atomic_relaxation_shell_count": len(relaxation.binding_energy) if relaxation else 0,
            "transition_shell_count": len(relaxation.transitions) if relaxation else 0,
        }
        photon_hashes[element] = sha256(file_path)
        if coupled_heating_required:
            populated_flags = [item["shell_map_complete"] for item in photon_physics["atomic_relaxation_available_by_element"].values()]
            photon_physics["atomic_relaxation_available"] = bool(populated_flags) and all(populated_flags)
            photon_physics["atomic_relaxation_enabled"] = photon_physics["atomic_relaxation_available"]
            photon_physics["electron_treatment"] = "led"
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
    return xml_hash, required, actual_temps, runtime_temp, photon_hashes, photon_physics


def compose(inp: dict, out: Path, openmc, data_runtime_temperature: float, photon_physics: dict):
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
    penetration = manifest.get("penetration")
    port_region = None
    if penetration is not None:
        require(penetration.get("kind") == "outboard_rectangular_prism", "unsupported penetration geometry kind")
        bounds = penetration["bounds_m"]
        minimum = [float(v) for v in bounds["minimum_xyz_m"]]
        maximum = [float(v) for v in bounds["maximum_xyz_m"]]
        require(len(minimum) == len(maximum) == 3 and all(math.isfinite(minimum[i]) and math.isfinite(maximum[i]) and minimum[i] < maximum[i] for i in range(3)), "invalid rectangular penetration bounds")
        require(minimum[0] > R + plasma_minor, "penetration intersects the idealized plasma source volume")
        xlo, ylo_box, zlo = (openmc.XPlane(x0=minimum[0] * scale), openmc.YPlane(y0=minimum[1] * scale), openmc.ZPlane(z0=minimum[2] * scale))
        xhi, yhi_box, zhi = (openmc.XPlane(x0=maximum[0] * scale), openmc.YPlane(y0=maximum[1] * scale), openmc.ZPlane(z0=maximum[2] * scale))
        port_region = +xlo & -xhi & +ylo_box & -yhi_box & +zlo & -zhi
        require(penetration["fill_material_id"] in materials, "penetration fill material is absent")
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
        region = +inner_surface & -surf
        if penetration is not None and rid in penetration["affected_component_ids"]:
            region &= ~port_region
        cell = openmc.Cell(name=rid, fill=materials[component["material_id"]], region=region)
        cells.append(cell)
        component_cells[rid] = cell
        component_filters[rid] = openmc.CellFilter(cell)
    outer_minor = float(variant["components"][-1]["outer_minor_radius_m"])
    require(outer_minor < R, "outer torus minor radius must remain below major radius")
    outer_surface = surfaces[-1]
    outer_surface.boundary_type = "vacuum"
    if penetration is not None:
        port_cell = openmc.Cell(
            name=penetration["id"],
            fill=materials[penetration["fill_material_id"]],
            region=port_region & +plasma_surface & -outer_surface,
        )
        cells.append(port_cell)
    outside = openmc.Cell(name="outside-torus-void", fill=materials["void"], region=+outer_surface)
    cells.append(outside)
    root = openmc.Universe(cells=cells)
    geometry = openmc.Geometry(root)

    penetration_volume_audit = None
    component_volume_cm3 = {}
    component_volume_se_cm3 = {component_id: 0.0 for component_id in component_cells}
    penetration = manifest.get("penetration")
    if penetration is not None:
        bounds = penetration["bounds_m"]
        minimum = [float(v) for v in bounds["minimum_xyz_m"]]
        maximum = [float(v) for v in bounds["maximum_xyz_m"]]
        require(len(minimum) == len(maximum) == 3 and all(minimum[i] < maximum[i] for i in range(3)), "invalid rectangular penetration bounds")
        samples = int(inp.get("penetration_volume_samples", 1_000_000))
        require(100_000 <= samples <= 20_000_000, "penetration volume sampling must be 100k..20M points")
        rng = random.Random(int(inp.get("penetration_volume_seed", 913_731_507)))
        box_volume_m3 = math.prod(maximum[i] - minimum[i] for i in range(3))
        inside = {component_id: 0 for component_id in component_cells}
        for _ in range(samples):
            point = [100.0 * rng.uniform(minimum[i], maximum[i]) for i in range(3)]
            found = geometry.find(point)
            cell = found[-1] if isinstance(found, (list, tuple)) else found
            if cell is not None and cell.name in inside:
                inside[cell.name] += 1
        for component_id in penetration["affected_component_ids"]:
            require(component_id in component_cells, f"penetration lists unknown component: {component_id}")
            p = inside[component_id] / samples
            estimate_m3 = box_volume_m3 * p
            error_m3 = box_volume_m3 * math.sqrt(p * (1.0 - p) / samples)
            full = next(c for c in variant["components"] if c["id"] == component_id)["full_torus_volume_m3"]
            component_volume_cm3[component_id] = (full - estimate_m3) * 1.0e6
            require(component_volume_cm3[component_id] > 0, f"penetration removed all of component {component_id}")
            component_volume_se_cm3[component_id] = error_m3 * 1.0e6
        penetration_volume_audit = {
            "scenario_sha256": request["scenario_sha256"],
            "variant_id": physics["variant_id"],
            "method": "independent_uniform_point_classification_in_port_box_using_OpenMC_Python_Geometry.find",
            "seed": int(inp.get("penetration_volume_seed", 913_731_507)),
            "samples": samples,
            "box_volume_m3": box_volume_m3,
            "intersection_estimates_m3": {key: box_volume_m3 * count / samples for key, count in inside.items()},
            "intersection_standard_errors_m3": {key: box_volume_m3 * math.sqrt((count / samples) * (1.0 - count / samples) / samples) for key, count in inside.items()},
            "cell_counts": inside,
            "fractional_volume_standard_errors_are_binomial": True,
            "independent_of_Rust_midpoint_quadrature": True,
            "not_a_physical_validation": True,
        }
    else:
        for c in variant["components"]:
            component_volume_cm3[c["id"]] = float(c["full_torus_volume_m3"]) * 1.0e6

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
    coupled_heating_required = any(r["score"]["kind"] == "heating" for r in request["responses"])
    settings.photon_transport = coupled_heating_required
    # If photoatomic data lacks relaxation transitions, explicitly omit the
    # fluorescence/Auger cascade rather than allow OpenMC 0.15.3 to index an
    # empty shell map. This approximation is recorded in the worker receipt.
    settings.atomic_relaxation = bool(photon_physics.get("atomic_relaxation_enabled", False))
    settings.electron_treatment = "led"
    settings.source = source
    # HDF kT is checked against the requested target at 0.1 K. OpenMC 0.15.3
    # resolves continuous-energy tables through rounded labels (e.g. 294K),
    # so this separate 1 K window only admits that label rounding at runtime.
    settings.temperature = {"default": data_runtime_temperature, "method": "nearest", "tolerance": OPENMC_LABEL_TOLERANCE_K}

    particle_filters = {
        "neutron": openmc.ParticleFilter("neutron"),
        "photon": openmc.ParticleFilter("photon"),
        "electron": openmc.ParticleFilter("electron"),
        "positron": openmc.ParticleFilter("positron"),
    }
    spectrum_tallies = {}
    spectrum_energy_filter = openmc.EnergyFilter(SPECTRUM_EDGES_EV)
    for component_id in component_cells:
        for particle in (("neutron", "photon") if coupled_heating_required else ("neutron",)):
            spectrum = openmc.Tally(name=f"spectrum-{particle}-{component_id}")
            spectrum.filters = [component_filters[component_id], particle_filters[particle], spectrum_energy_filter]
            spectrum.scores = ["flux"]
            spectrum.estimator = "tracklength"
            spectrum_tallies[(component_id, particle)] = spectrum
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
            tally.filters.append(particle_filters["neutron"])
            tally.scores = ["flux"]
        elif kind == "reaction_rate":
            tally.filters.append(particle_filters["neutron"])
            tally.scores = [score["reaction"]]
        elif kind == "particle_production":
            tally.filters.append(particle_filters["neutron"])
            tally.scores = [score["score"]]
        elif kind == "heating":
            require(score.get("convention") == "heating", "only coupled OpenMC heating is available; heating-local/MT=901 is unsupported")
            scope = score.get("particle_scope")
            if scope == "neutron":
                tally.filters.append(particle_filters["neutron"])
            elif scope == "photon":
                tally.filters.append(particle_filters["photon"])
            elif scope == "electron":
                tally.filters.append(particle_filters["electron"])
            elif scope == "positron":
                tally.filters.append(particle_filters["positron"])
            elif scope != "total":
                raise ValueError(f"unsupported heating particle scope: {scope}")
            tally.scores = ["heating"]
            tally.estimator = "collision"
        else:
            raise ValueError(f"unsupported or unavailable score kind: {kind}")
        if kind != "heating":
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
        mesh_tally.filters = [openmc.MeshFilter(mesh), particle_filters["neutron"]]
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
        volumes_cm3[c["id"]] = component_volume_cm3[c["id"]]
        expected_m3 = 2.0 * math.pi**2 * R * (b*b-a*a)
        require(math.isclose(float(c["full_torus_volume_m3"]), expected_m3, rel_tol=1.0e-12, abs_tol=1.0e-12), f"manifest torus volume mismatch for {c['id']}")
    whole_minor = plasma_minor + gap + sum(float(c["thickness_m"]) for c in variant["components"])
    whole_volume_cm3 = 2.0 * math.pi**2 * R * whole_minor**2 * 1.0e6
    mesh_bin_volume_cm3 = None
    if mesh_meta:
        widths = [(mesh_meta["upper_right_m"][i]-mesh_meta["lower_left_m"][i])/mesh_meta["dimensions"][i] for i in range(3)]
        mesh_bin_volume_cm3 = math.prod(widths)*1.0e6
    return model, response_by_tally, mesh_tally, expected if mesh_tally else {}, volumes_cm3, component_volume_se_cm3, whole_volume_cm3, mesh_bin_volume_cm3, component_cells, spectrum_tallies, plasma, mesh_index_audit, penetration_volume_audit


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
    penetration_volume_audit = None
    try:
        inp = read_input(args.input.expanduser().resolve(strict=True))
        request = inp["request"]
        exe = Path(inp["openmc_executable"]).expanduser().resolve(strict=True)
        xml = Path(inp["cross_sections"]).expanduser().resolve(strict=True)
        require(os.access(exe, os.X_OK), "OpenMC executable is not executable")
        data_digest = inp["nuclear_data_digest"]
        require(data_digest.startswith("sha256:") and len(data_digest) == 71, "nuclear_data_digest must be a sha256-prefixed 64-hex digest")
        import openmc
        require(openmc.__version__ == "0.15.3", f"expected OpenMC 0.15.3, got {openmc.__version__}")
        coupled_heating_required = any(r["score"]["kind"] == "heating" for r in inp["request"]["responses"])
        xml_hash, required_nuclides, actual_data_temps, runtime_data_temperature, photon_data_hashes, photon_physics = check_data_identity(inp["physics"], xml, openmc, coupled_heating_required)
        # Set only the path; all scored quantities remain raw, per source neutron.
        os.environ["OPENMC_CROSS_SECTIONS"] = str(xml)
        model, response_by_tally, mesh_tally, mesh_defs, volumes_cm3, component_volume_se_cm3, whole_cm3, mesh_bin_cm3, component_cells, spectrum_tallies, plasma, mesh_index_audit, penetration_volume_audit = compose(inp, out, openmc, runtime_data_temperature, photon_physics)
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
                expected_estimator = "collision" if response["score"]["kind"] == "heating" else "tracklength"
                require(tally.estimator == expected_estimator, f"response {response['id']} estimator is {tally.estimator!r}; expected {expected_estimator!r}")
                means = tally.mean.ravel()
                errors = tally.std_dev.ravel()
                require(len(means) == 1 and len(errors) == 1, f"response {response['id']} did not produce one scalar")
                mean, se = float(means[0]), float(errors[0])
                signed_heating = response["score"]["kind"] == "heating"
                require(math.isfinite(mean) and (signed_heating or mean >= 0) and math.isfinite(se) and se >= 0, f"response {response['id']} has an invalid mean or standard error")
                domain = response["domain"]
                if domain["kind"] == "component":
                    volume = volumes_cm3[domain["component_id"]]
                elif domain["kind"] == "whole_model":
                    volume = whole_cm3
                else:
                    raise ValueError("unsupported response domain")
                score = response["score"]["kind"]
                unit = "ev_per_source" if score == "heating" else ("cm_per_source" if score == "flux" else ("particles_per_source" if score == "particle_production" else "events_per_source"))
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
            for (component_id, particle), expected_tally in spectrum_tallies.items():
                tally = sp.get_tally(name=expected_tally.name)
                require(tally.estimator == "tracklength", f"spectrum estimator is not explicitly tracklength for {particle}/{component_id}")
                means = tally.mean.ravel()
                errors = tally.std_dev.ravel()
                require(len(means) == len(SPECTRUM_EDGES_EV) - 1, f"spectrum bin count mismatch for {particle}/{component_id}")
                require(all(math.isfinite(float(x)) and float(x) >= 0 for x in means), f"spectrum mean invalid for {particle}/{component_id}")
                require(all(math.isfinite(float(x)) and float(x) >= 0 for x in errors), f"spectrum standard error invalid for {particle}/{component_id}")
                spectra.append({"component_id": component_id, "particle": particle, "estimator": tally.estimator, "unit": "cm_per_source_per_energy_bin", "energy_edges_ev": SPECTRUM_EDGES_EV, "mean_cm_per_source_per_bin": [float(x) for x in means], "standard_error_cm_per_source_per_bin": [float(x) for x in errors], "volume_cm3": volumes_cm3[component_id], "volume_standard_error_cm3": component_volume_se_cm3[component_id]})
                if particle == "neutron":
                    flux_response = next((r for r in request["responses"] if r["id"] == f"{component_id}-flux"), None)
                    require(flux_response is not None, f"missing integrated flux response for {component_id}")
                    flux_raw = next(t for t in raw_tallies if t["response_id"] == flux_response["id"])
                    require(abs(sum(float(x) for x in means) - flux_raw["mean"]) <= 1.0e-8 * max(abs(flux_raw["mean"]), 1.0e-30), f"full-range neutron spectrum does not sum to integrated flux for {component_id}")
            volumes = []
            for response in inp["request"]["responses"]:
                d = response["domain"]
                if d["kind"] == "component":
                    v = volumes_cm3[d["component_id"]]
                    se = component_volume_se_cm3[d["component_id"]]
                elif d["kind"] == "whole_model":
                    v = whole_cm3
                    se = 0.0
                elif d["kind"] == "mesh":
                    v = mesh_bin_cm3
                    se = 0.0
                else:
                    raise ValueError(f"unsupported volume domain: {d['kind']}")
                if not any(item["domain"] == d for item in volumes):
                    volumes.append({"domain": d, "value": v, "standard_error": se, "unit": "cubic_centimetre"})
            artifact = {"schema_version": ARTIFACT_SCHEMA, "request": inp["request"], "solver": {"name": "OpenMC", "version": openmc.__version__, "digest": f"sha256:{sha256(exe)}"}, "nuclear_data": {"name": inp["physics"].get("nuclear_data", {}).get("name", "external cross_sections.xml"), "version": inp["physics"].get("nuclear_data", {}).get("version", "unselected-local-library"), "digest": data_digest}, "histories": int(sp.n_realizations)*int(inp["sampling"]["particles_per_batch"]), "volumes": volumes, "tallies": raw_tallies}
            (out / "transport-artifact.json").write_text(json.dumps(artifact, indent=2, sort_keys=True)+"\n", encoding="utf-8")
            (out / "transport-spectra.json").write_text(json.dumps({"schema_version":"faris-transport-spectra/v0.1","request":request,"scenario_sha256":request["scenario_sha256"],"variant_id":request["variant_id"],"input_sha256":sha256(args.input),"solver":artifact["solver"],"nuclear_data":artifact["nuclear_data"],"histories":artifact["histories"],"spectra":spectra}, indent=2, sort_keys=True)+"\n", encoding="utf-8")
        after_export_xml_hashes = {path.name: sha256(path) for path in sorted(out.glob("*.xml"))}
        require(after_export_xml_hashes == exported_xml_hashes, "OpenMC export XML changed during solver execution")
        precision_report = sampling_precision_report(inp["request"], raw_tallies, volumes)
        record.update({"execution_status":"COMPLETED","scientific_status":"NOT_EVALUATED","histories":artifact["histories"],"solver":artifact["solver"],"nuclear_data":artifact["nuclear_data"],"photon_data_sha256":photon_data_hashes,"photon_physics":photon_physics,"penetration_volume_audit":penetration_volume_audit,"sampling_precision":precision_report,"requested_nuclear_data_temperature_K":sorted({m["recipe"]["nuclear_data_temperature_k"] for m in inp["physics"]["materials"] if m["recipe"]["kind"] == "nuclide_mixture"}),"stored_nuclear_data_temperatures_K":sorted(set(actual_data_temps.values())),"openmc_data_group_temperature_label_K":runtime_data_temperature,"openmc_nearest_label_tolerance_K":OPENMC_LABEL_TOLERANCE_K,"openmc_statepoint_version":list(observed_version),"statepoint":statepoint_identity,"export_xml_sha256":exported_xml_hashes,"solver_output_capture":solver_output,"mesh_index_audit":mesh_index_audit,"transport_artifact":"transport-artifact.json","transport_artifact_sha256":sha256(out / "transport-artifact.json"),"transport_spectra":"transport-spectra.json","transport_spectra_sha256":sha256(out / "transport-spectra.json"),"responses":len(raw_tallies),"normalization":"RAW_PER_SOURCE_NEUTRON; no absolute source normalization in Python","lost_particle_check":"no lost-particle log indication; statepoint present"})
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
        if penetration_volume_audit is not None:
            record["penetration_volume_audit"] = penetration_volume_audit
        (out / "worker-result.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
        raise
    (out / "worker-result.json").write_text(json.dumps(record, indent=2)+"\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
