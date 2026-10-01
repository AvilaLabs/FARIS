#!/usr/bin/env python3
"""Bounded OpenMC coupled-transport control for thermal 6Li neutron capture.

This is a small, separately identified numerical control. It does not model
FARIS, a blanket, a reactor source, or a design material.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import signal
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

SCHEMA = "faris-openmc-lithium-capture-control/v0.1"
MASS_U_MEV = 931.49410372
NEUTRON_MASS_U_AME2020 = 1.00866491590
ATOMIC_MASS_U = {
    "Li6": 6.01512288742,
    "H3": 3.01604928132,
    "He4": 4.00260325413,
    "Li7": 7.01600343426,
}
Q_MASS_KEV_TOLERANCE_EV = 1000.0
HEATING_RELATIVE_TOLERANCE = 1.0e-3
MAX_HISTORIES = 10_000_000
MAX_THREADS = 16
MAX_TIMEOUT_SECONDS = 600
MAX_FILE_BYTES = 256 * 1024 * 1024
MAX_TREE_BYTES = 512 * 1024 * 1024
MAX_TREE_FILES = 256
MAX_LOG_BYTES = 32 * 1024 * 1024
MAX_ADDRESS_SPACE_BYTES = 4 * 1024 * 1024 * 1024
DEFAULT_SEED = 20261001
DEFAULT_BATCHES = 100
DEFAULT_PARTICLES = 100_000
DEFAULT_THREADS = 8
SOURCE_ENERGY_EV = 0.0253
DATA_TEMPERATURE_K = 293.59430848016336
OPENMC_LABEL_TEMPERATURE_K = 294.0
OPENMC_LABEL_TOLERANCE_K = 1.0
K_BOLTZMANN_EV_PER_K = 8.617333262145e-5
LI6_ATOM_DENSITY_ATOMS_PER_BARN_CM = 0.01
SPHERE_RADIUS_CM = 10.0


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def q_from_atomic_masses(reactant: float, products: tuple[float, ...]) -> float:
    """Return mass defect times c² in eV, using neutral atoms where e⁻ cancel."""
    values = (reactant, *products)
    if not all(math.isfinite(value) and value > 0.0 for value in values):
        raise ValueError("atomic masses must be finite and positive")
    return (reactant - math.fsum(products)) * MASS_U_MEV * 1.0e6


def expected_q_values() -> dict[str, float]:
    li6_plus_n = ATOMIC_MASS_U["Li6"] + NEUTRON_MASS_U_AME2020
    return {
        "Li6_n_t_alpha_mass_defect_eV": q_from_atomic_masses(
            li6_plus_n, (ATOMIC_MASS_U["H3"], ATOMIC_MASS_U["He4"])
        ),
        "Li6_n_gamma_Li7_mass_defect_eV": q_from_atomic_masses(
            li6_plus_n, (ATOMIC_MASS_U["Li7"],)
        ),
    }


def _path_in_root(root: Path, relative: str) -> Path:
    candidate = Path(relative)
    if candidate.is_absolute() or any(part in ("..", ".") for part in candidate.parts):
        raise ValueError("cross_sections.xml contains a non-relative or traversing path")
    resolved = (root / candidate).resolve(strict=True)
    if not resolved.is_file() or not resolved.is_relative_to(root):
        raise ValueError("cross-section file is absent or escapes the data root")
    return resolved


def identify_data(xml_path: Path, audit_path: Path, photon_provenance_path: Path) -> dict[str, Any]:
    xml_path = xml_path.expanduser().resolve(strict=True)
    root = xml_path.parent.resolve(strict=True)
    audit = json.loads(audit_path.read_text(encoding="utf-8"))
    provenance = json.loads(photon_provenance_path.read_text(encoding="utf-8"))
    if audit.get("openmc", {}).get("version") != "0.15.3":
        raise ValueError("the selected nuclear-data audit must identify OpenMC 0.15.3")
    xml_digest = sha256_file(xml_path)
    if provenance.get("cross_sections_xml", {}).get("sha256") != xml_digest:
        raise ValueError("cross_sections.xml does not match the audited combined library")
    li6_metadata = audit.get("neutron_library", {}).get("Li6", {})
    if not li6_metadata.get("library_entry_present"):
        raise ValueError("audit does not declare the Li6 neutron file")
    li6_path = _path_in_root(root, li6_metadata["relative_path"])
    li6_digest = sha256_file(li6_path)
    if li6_digest != li6_metadata.get("sha256"):
        raise ValueError("Li6 neutron file hash differs from its audit")
    mt301 = li6_metadata.get("reactions", {}).get("301", {})
    h3_route = li6_metadata.get("h3_production_route", {})
    if not mt301.get("present") or not h3_route.get("data_present"):
        raise ValueError("Li6 audit lacks declared MT=301 heating or H3-production route")
    if not h3_route.get("reaction_105_n_t_present"):
        raise ValueError("Li6 audit lacks the (n,t) channel")
    photons = {item.get("element"): item for item in provenance.get("photon_data", {}).get("files", [])}
    li_metadata = photons.get("Li")
    if li_metadata is None:
        raise ValueError("photon provenance lacks lithium photoatomic data")
    li_photon_audit = audit.get("photon_atomic_library", {}).get("Li", {})
    if not li_photon_audit.get("atomic_relaxation_populated"):
        raise ValueError("OpenMC library audit does not verify populated lithium relaxation shell maps")
    li_photon_path = _path_in_root(root, li_metadata["relative_path"])
    if sha256_file(li_photon_path) != li_metadata.get("sha256") or li_metadata.get("sha256") != li_photon_audit.get("sha256"):
        raise ValueError("lithium photon file hash differs from its provenance record")
    tree = ET.parse(xml_path).getroot()
    entries = [node for node in tree.findall("library") if node.get("type") in {"neutron", "photon"}]
    found_neutron = any(
        node.get("type") == "neutron"
        and "Li6" in node.get("materials", "").split()
        and _path_in_root(root, node.get("path", "")) == li6_path
        for node in entries
    )
    found_photon = any(
        node.get("type") == "photon"
        and node.get("materials") == "Li"
        and _path_in_root(root, node.get("path", "")) == li_photon_path
        for node in entries
    )
    if not found_neutron or not found_photon:
        raise ValueError("cross_sections.xml does not bind both audited Li6 and Li photon files")
    q_mass = expected_q_values()
    library_q = {
        "Li6_n_t_ENDF_Q_eV": float(h3_route["reaction_105_n_t_q_value_eV"]),
        "Li6_n_gamma_ENDF_Q_eV": None,
    }
    with _h5py().File(li6_path, "r") as data:
        reactions = data["Li6"]["reactions"]
        library_q["Li6_n_gamma_ENDF_Q_eV"] = float(reactions["reaction_102"].attrs["Q_value"])
        stored_kts = data["Li6"]["kTs"]
        matching = [float(group[()]) / K_BOLTZMANN_EV_PER_K for group in stored_kts.values()]
    if not any(abs(value - DATA_TEMPERATURE_K) <= 0.1 for value in matching):
        raise ValueError("Li6 stored kT does not match the control's numeric data temperature within 0.1 K")
    for mass_key, data_key in [
        ("Li6_n_t_alpha_mass_defect_eV", "Li6_n_t_ENDF_Q_eV"),
        ("Li6_n_gamma_Li7_mass_defect_eV", "Li6_n_gamma_ENDF_Q_eV"),
    ]:
        if abs(q_mass[mass_key] - library_q[data_key]) > Q_MASS_KEV_TOLERANCE_EV:
            raise ValueError(f"AME2020 mass Q and evaluated-library Q disagree by >1 keV for {data_key}")
    return {
        "cross_sections_xml": {"path": str(xml_path), "sha256": xml_digest},
        "neutron_file": {"path_relative_to_xml": li6_metadata["relative_path"], "sha256": li6_digest},
        "photon_file": {"path_relative_to_xml": li_metadata["relative_path"], "sha256": li_metadata["sha256"]},
        "neutron_audit_sha256": sha256_file(audit_path),
        "photon_provenance_sha256": sha256_file(photon_provenance_path),
        "temperature": {"numeric_data_temperature_k": DATA_TEMPERATURE_K,
                        "stored_numeric_temperatures_k": matching,
                        "stored_label_for_openmc_k": OPENMC_LABEL_TEMPERATURE_K,
                        "openmc_label_rounding_tolerance_k": OPENMC_LABEL_TOLERANCE_K,
                        "source_energy_ev": SOURCE_ENERGY_EV},
        "reaction_q_values": {"ame2020_atomic_mass_defect": q_mass, "endf_b_vii_1_library": library_q,
                               "maximum_q_crosscheck_difference_ev": Q_MASS_KEV_TOLERANCE_EV},
        "capabilities": {"h3_production_data": True, "neutron_mt301_heating": True,
                         "li_atomic_relaxation": True},
    }


def _h5py():
    try:
        import h5py
    except ImportError as exc:
        raise RuntimeError("h5py is required in the selected OpenMC Python environment") from exc
    return h5py


def build_inputs(out: Path, openmc: Any, seed: int, batches: int, particles: int) -> dict[str, Any]:
    material = openmc.Material(name="idealized enriched Li-6 control medium")
    material.add_nuclide("Li6", 1.0, "ao")
    material.set_density("atom/b-cm", LI6_ATOM_DENSITY_ATOMS_PER_BARN_CM)
    materials = openmc.Materials([material])

    boundary = openmc.Sphere(r=SPHERE_RADIUS_CM, boundary_type="vacuum")
    cell = openmc.Cell(name="idealized pure Li-6 sphere", fill=material, region=-boundary)
    geometry = openmc.Geometry(openmc.Universe(cells=[cell]))

    source = openmc.IndependentSource(
        space=openmc.stats.Point((0.0, 0.0, 0.0)),
        angle=openmc.stats.Isotropic(),
        energy=openmc.stats.Discrete([SOURCE_ENERGY_EV], [1.0]),
        strength=1.0,
        particle="neutron",
    )
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.batches = batches
    settings.particles = particles
    settings.seed = seed
    settings.source = source
    settings.photon_transport = True
    settings.atomic_relaxation = True
    settings.electron_treatment = "led"
    settings.temperature = {"default": DATA_TEMPERATURE_K, "method": "nearest",
                            "tolerance": OPENMC_LABEL_TOLERANCE_K}
    settings.statepoint = {"batches": [batches]}
    settings.max_particle_events = 10_000
    settings.max_lost_particles = 100

    cell_filter = openmc.CellFilter(cell)
    neutron_filter = openmc.ParticleFilter("neutron")
    photon_filter = openmc.ParticleFilter("photon")
    electron_filter = openmc.ParticleFilter("electron")
    positron_filter = openmc.ParticleFilter("positron")
    def tally(name: str, score: str, extra_filters: list[Any], estimator: str) -> Any:
        result = openmc.Tally(name=name)
        result.filters = [cell_filter, *extra_filters]
        result.nuclides = ["Li6"] if score != "heating" or neutron_filter in extra_filters else ["total"]
        result.scores = [score]
        result.estimator = estimator
        return result

    # Keep reaction production tallies neutron-filtered and Li6-specific.
    h3 = tally("Li6 H3 production", "H3-production", [neutron_filter], "tracklength")
    nt = tally("Li6 (n,t) reactions", "(n,t)", [neutron_filter], "tracklength")
    capture = tally("Li6 (n,gamma) reactions", "(n,gamma)", [neutron_filter], "tracklength")
    absorption = tally("Li6 neutron absorption", "absorption", [neutron_filter], "tracklength")
    heat_neutron = tally("neutron MT301 heating", "heating", [neutron_filter], "collision")
    heat_photon = tally("photon deposition heating", "heating", [photon_filter], "collision")
    heat_electron = tally("electron deposition heating", "heating", [electron_filter], "collision")
    heat_positron = tally("positron deposition heating", "heating", [positron_filter], "collision")
    heat_total = openmc.Tally(name="all-particle total heating")
    heat_total.filters = [cell_filter]
    heat_total.nuclides = ["total"]
    heat_total.scores = ["heating"]
    heat_total.estimator = "collision"
    tallies = openmc.Tallies([h3, nt, capture, absorption, heat_neutron, heat_photon,
                              heat_electron, heat_positron, heat_total])

    materials.export_to_xml(path=str(out / "materials.xml"))
    geometry.export_to_xml(path=str(out / "geometry.xml"))
    settings.export_to_xml(path=str(out / "settings.xml"))
    tallies.export_to_xml(path=str(out / "tallies.xml"))
    input_files = {}
    for name in ("materials.xml", "geometry.xml", "settings.xml", "tallies.xml"):
        path = out / name
        input_files[name] = {"sha256": sha256_file(path), "bytes": path.stat().st_size}
    return {
        "cell_id": cell.id,
        "materials_xml_files": input_files,
        "tally_definitions": {
            "H3-production": {"name": h3.name, "score": "H3-production", "particle": "neutron",
                              "nuclide": "Li6", "estimator": h3.estimator,
                              "unit_per_source": "tritium_particles/source_neutron"},
            "n_t": {"name": nt.name, "score": "(n,t)", "particle": "neutron", "nuclide": "Li6",
                    "estimator": nt.estimator, "unit_per_source": "reactions/source_neutron"},
            "n_gamma": {"name": capture.name, "score": "(n,gamma)", "particle": "neutron",
                        "nuclide": "Li6", "estimator": capture.estimator,
                        "unit_per_source": "reactions/source_neutron"},
            "absorption": {"name": absorption.name, "score": "absorption", "particle": "neutron",
                           "nuclide": "Li6", "estimator": absorption.estimator,
                           "unit_per_source": "reactions/source_neutron"},
            "neutron_heating": {"name": heat_neutron.name, "score": "heating", "particle": "neutron",
                                "estimator": heat_neutron.estimator, "unit_per_source": "eV/source_neutron"},
            "photon_heating": {"name": heat_photon.name, "score": "heating", "particle": "photon",
                               "estimator": heat_photon.estimator, "unit_per_source": "eV/source_neutron"},
            "electron_heating": {"name": heat_electron.name, "score": "heating", "particle": "electron",
                                 "estimator": heat_electron.estimator, "unit_per_source": "eV/source_neutron"},
            "positron_heating": {"name": heat_positron.name, "score": "heating", "particle": "positron",
                                 "estimator": heat_positron.estimator, "unit_per_source": "eV/source_neutron"},
            "total_heating": {"name": heat_total.name, "score": "heating", "particle": "all",
                              "estimator": heat_total.estimator, "unit_per_source": "eV/source_neutron"},
        },
        "source": {"particle": "neutron", "energy_ev": SOURCE_ENERGY_EV, "position_cm": [0, 0, 0],
                   "direction": "isotropic", "strength": 1.0, "normalization": "per source neutron"},
        "geometry": {"shape": "vacuum-bounded sphere", "radius_cm": SPHERE_RADIUS_CM},
        "idealized_material": {"nuclide": "Li6", "atom_density_atoms_per_barn_cm": LI6_ATOM_DENSITY_ATOMS_PER_BARN_CM,
                               "density_is_a_control_assumption": True,
                               "physical_material_temperature_k": None},
        "settings": {"photon_transport": True, "atomic_relaxation": True, "electron_treatment": "led",
                     "temperature_k": DATA_TEMPERATURE_K,
                     "openmc_label_rounding_tolerance_k": OPENMC_LABEL_TOLERANCE_K,
                     "statepoint_batch": batches},
    }


def _set_child_limits(timeout: int) -> None:
    import resource

    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))
    resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_FILE_BYTES, MAX_FILE_BYTES))
    resource.setrlimit(resource.RLIMIT_AS, (MAX_ADDRESS_SPACE_BYTES, MAX_ADDRESS_SPACE_BYTES))
    resource.setrlimit(resource.RLIMIT_CPU, (timeout + 10, timeout + 10))


def _bounded_tree(path: Path) -> list[dict[str, Any]]:
    manifest: list[dict[str, Any]] = []
    total = 0
    for item in sorted(path.rglob("*")):
        if item.is_symlink():
            raise RuntimeError("OpenMC run directory contains a symlink")
        if item.is_dir():
            continue
        if not item.is_file():
            raise RuntimeError("OpenMC run directory contains a non-regular file")
        size = item.stat().st_size
        if size > MAX_FILE_BYTES:
            raise RuntimeError("OpenMC output file exceeds the enforced per-file bound")
        total += size
        manifest.append({"path": str(item.relative_to(path)), "sha256": sha256_file(item), "bytes": size})
        if len(manifest) > MAX_TREE_FILES or total > MAX_TREE_BYTES:
            raise RuntimeError("OpenMC output tree exceeds file-count or total-size bound")
    return manifest


def _stat(tally: Any) -> dict[str, Any]:
    if tally.mean.size != 1 or tally.std_dev.size != 1:
        raise RuntimeError(f"tally {tally.name!r} has unexpected shape {tally.mean.shape}")
    mean, standard_error = float(tally.mean.ravel()[0]), float(tally.std_dev.ravel()[0])
    if not math.isfinite(mean) or not math.isfinite(standard_error) or mean < 0.0 or standard_error < 0.0:
        raise RuntimeError(f"tally {tally.name!r} has non-finite or invalid moments")
    return {"mean": mean, "standard_error": standard_error}


def validate_responses(responses: dict[str, dict[str, float]], library_q: dict[str, float]) -> dict[str, Any]:
    h3 = responses["h3_production"]
    nt = responses["n_t_reactions"]
    capture = responses["n_gamma_reactions"]
    absorption = responses["neutron_absorption"]
    total_heat = responses["heating_total"]
    neutron_heat = responses["heating_neutron"]
    photon_heat = responses["heating_photon"]
    electron_heat = responses["heating_electron"]
    positron_heat = responses["heating_positron"]
    h3_delta = abs(h3["mean"] - nt["mean"])
    h3_limit = 3.0 * (h3["standard_error"] + nt["standard_error"]) + 1.0e-10
    absorption_delta = abs(absorption["mean"] - (nt["mean"] + capture["mean"]))
    absorption_limit = 3.0 * (
        absorption["standard_error"] + nt["standard_error"] + capture["standard_error"]
    ) + 1.0e-10

    q_mass = expected_q_values()
    q_nt = q_mass["Li6_n_t_alpha_mass_defect_eV"]
    q_gamma = q_mass["Li6_n_gamma_Li7_mass_defect_eV"]
    q_nt_library = library_q["Li6_n_t_ENDF_Q_eV"]
    q_gamma_library = library_q["Li6_n_gamma_ENDF_Q_eV"]
    q_nt_low = min(q_nt, q_nt_library) - Q_MASS_KEV_TOLERANCE_EV
    q_nt_high = max(q_nt, q_nt_library) + Q_MASS_KEV_TOLERANCE_EV
    q_gamma_high = max(q_gamma, q_gamma_library) + Q_MASS_KEV_TOLERANCE_EV
    # Every Li6 capture makes one triton on (n,t). Radiative-capture photons may
    # escape, so their local contribution is bounded from zero to the full Q.
    # The source neutron's entire kinetic energy is also conservatively allowed
    # to deposit or escape. Mass-vs-library Q differences are explicitly bounded.
    energy_lower = h3["mean"] * q_nt_low - SOURCE_ENERGY_EV
    energy_upper = h3["mean"] * q_nt_high + capture["mean"] * q_gamma_high + SOURCE_ENERGY_EV
    energy_sampling_radius = 3.0 * (
        total_heat["standard_error"]
        + h3["standard_error"] * q_nt_high
        + capture["standard_error"] * q_gamma_high
    )
    energy_model_floor = max(1.0, h3["mean"] * max(q_nt, q_nt_library) * HEATING_RELATIVE_TOLERANCE)
    energy_limit_lower = energy_lower - energy_sampling_radius - energy_model_floor
    energy_limit_upper = energy_upper + energy_sampling_radius + energy_model_floor
    energy_within = energy_limit_lower <= total_heat["mean"] <= energy_limit_upper

    component_sum = math.fsum((neutron_heat["mean"], photon_heat["mean"],
                               electron_heat["mean"], positron_heat["mean"]))
    component_delta = abs(total_heat["mean"] - component_sum)
    component_limit = 3.0 * (
        total_heat["standard_error"]
        + neutron_heat["standard_error"]
        + photon_heat["standard_error"]
        + electron_heat["standard_error"]
        + positron_heat["standard_error"]
    ) + 1.0e-6
    checks = {
        "tritium_particle_production_equals_n_t_reaction_events": {
            "difference_particles_per_source": h3_delta,
            "heuristic_limit_particles_per_source": h3_limit,
            "within_heuristic": h3_delta <= h3_limit,
        },
        "neutron_absorption_equals_n_t_plus_n_gamma": {
            "difference_reactions_per_source": absorption_delta,
            "heuristic_limit_reactions_per_source": absorption_limit,
            "within_heuristic": absorption_delta <= absorption_limit,
        },
        "coupled_heating_lies_within_mass_q_energy_bounds": {
            "heating_mean_ev_per_source": total_heat["mean"],
            "physical_energy_interval_ev_per_source": [energy_lower, energy_upper],
            "sampling_radius_ev_per_source": energy_sampling_radius,
            "predeclared_relative_model_floor": HEATING_RELATIVE_TOLERANCE,
            "q_crosscheck_tolerance_ev_per_reaction": Q_MASS_KEV_TOLERANCE_EV,
            "accepted_interval_ev_per_source": [energy_limit_lower, energy_limit_upper],
            "within_heuristic": energy_within,
            "gamma_capture_deposition": "bounded 0..full AME/evaluated Q because photons may escape",
        },
        "all_particle_heating_equals_neutron_photon_electron_and_positron_components": {
            "difference_ev_per_source": component_delta,
            "heuristic_limit_ev_per_source": component_limit,
            "within_heuristic": component_delta <= component_limit,
        },
    }
    return {
        "checks": checks,
        "status": "PASS" if all(item["within_heuristic"] for item in checks.values()) else "FAIL",
        "limitations": [
            "This control checks response scoring and a mass-defect energy interval for an idealized pure Li-6 sphere only.",
            "The Monte Carlo standard-error screens are heuristic and not rigorous confidence bounds.",
            "Gamma-capture heating is treated as an interval because photon escape energy is not tallied here.",
            "No result qualifies FARIS, a reactor blanket, the data evaluation, or an engineering design.",
        ],
    }


def _positive_int(raw: str, minimum: int, maximum: int, label: str) -> int:
    try:
        value = int(raw)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"{label} must be an integer") from exc
    if value < minimum or value > maximum:
        raise argparse.ArgumentTypeError(f"{label} must lie in [{minimum}, {maximum}]")
    return value


def run_control(args: argparse.Namespace) -> dict[str, Any]:
    import openmc

    out = args.out.expanduser().absolute()
    if out.exists():
        raise ValueError("output directory must not exist")
    if args.batches < 30 or args.batches * args.particles > MAX_HISTORIES:
        raise ValueError("require at least 30 batches and at most 10,000,000 histories")
    if args.threads > MAX_THREADS or args.timeout > MAX_TIMEOUT_SECONDS:
        raise ValueError("thread or timeout request exceeds its declared maximum")
    data = identify_data(args.cross_sections, args.audit, args.photon_provenance)
    executable = args.openmc.expanduser().resolve(strict=True)
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise ValueError("OpenMC executable must be an executable regular file")
    out.mkdir(parents=True, exist_ok=False)
    model = build_inputs(out, openmc, args.seed, args.batches, args.particles)
    input_digest = hashlib.sha256()
    for name, item in sorted(model["materials_xml_files"].items()):
        input_digest.update(name.encode("utf-8") + b"\0" + item["sha256"].encode("ascii") + b"\n")
    for key in ("cross_sections_xml", "neutron_file", "photon_file"):
        input_digest.update(key.encode("ascii") + b"\0" + data[key]["sha256"].encode("ascii") + b"\n")
    data["combined_control_input_sha256"] = input_digest.hexdigest()
    log_stdout = (out / "openmc.stdout.log").open("xb")
    log_stderr = (out / "openmc.stderr.log").open("xb")
    env = {
        "HOME": str(Path.home()),
        "PATH": str(executable.parent),
        "LD_LIBRARY_PATH": str(executable.parent.parent / "lib"),
        "OPENMC_CROSS_SECTIONS": data["cross_sections_xml"]["path"],
        "OMP_NUM_THREADS": str(args.threads),
        "LANG": "C.UTF-8",
        # Single-rank OpenMC with shared-memory OpenMP; avoid MPI transports
        # that attempt socket setup in restricted/air-gapped local runners.
        "OMPI_MCA_pml": "ob1",
        "OMPI_MCA_btl": "self",
    }
    command = [str(executable), "-s", str(args.threads)]
    started = time.monotonic()
    proc = subprocess.Popen(
        command,
        cwd=out,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=log_stdout,
        stderr=log_stderr,
        start_new_session=True,
        preexec_fn=lambda: _set_child_limits(args.timeout),
    )
    status = "FAILED"
    try:
        exit_code = proc.wait(timeout=args.timeout)
        status = "COMPLETED" if exit_code == 0 else "FAILED"
    except subprocess.TimeoutExpired:
        status = "TIMED_OUT"
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
        exit_code = None
    except KeyboardInterrupt:
        status = "CANCELLED"
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
        exit_code = None
    finally:
        log_stdout.close()
        log_stderr.close()
    elapsed = time.monotonic() - started
    logs = [out / "openmc.stdout.log", out / "openmc.stderr.log"]
    if any(path.stat().st_size > MAX_LOG_BYTES for path in logs):
        raise RuntimeError("OpenMC log exceeded 32 MiB; RLIMIT_FSIZE should have stopped it")
    record: dict[str, Any] = {
        "schema_version": SCHEMA,
        "status": "NOT_EVALUATED",
        "solver_status": status,
        "solver_exit_code": exit_code,
        "solver": {"name": "OpenMC", "version": openmc.__version__,
                   "executable": str(executable), "executable_sha256": sha256_file(executable)},
        "script_sha256": sha256_file(Path(__file__).resolve()),
        "data_identity": data,
        "model": model,
        "sampling": {"seed": args.seed, "batches": args.batches,
                     "particles_per_batch": args.particles,
                     "nominal_histories": args.batches * args.particles,
                     "threads": args.threads, "timeout_seconds": args.timeout,
                     "wall_seconds": elapsed,
                     "limits": {"address_space_per_process_bytes": MAX_ADDRESS_SPACE_BYTES,
                                "file_size_bytes": MAX_FILE_BYTES,
                                "output_tree_bytes": MAX_TREE_BYTES,
                                "output_file_count": MAX_TREE_FILES}},
        "responses": {},
        "acceptance": None,
    }
    if status == "COMPLETED" and exit_code == 0:
        statepoint = out / f"statepoint.{args.batches}.h5"
        if not statepoint.is_file():
            raise RuntimeError("OpenMC exited successfully without the expected statepoint")
        with openmc.StatePoint(statepoint) as sp:
            if ".".join(str(int(part)) for part in sp.version) != openmc.__version__:
                raise RuntimeError("OpenMC executable and Python API versions differ")
            if sp.current_batch != args.batches:
                raise RuntimeError("statepoint current batch differs from requested batches")
            definitions = {t.name: t for t in sp.tallies.values()}
            required = [
                "Li6 H3 production", "Li6 (n,t) reactions", "Li6 (n,gamma) reactions",
                "Li6 neutron absorption", "neutron MT301 heating", "photon deposition heating",
                "electron deposition heating", "positron deposition heating",
                "all-particle total heating",
            ]
            if any(name not in definitions for name in required):
                raise RuntimeError("statepoint is missing one or more required scoring tallies")
            stats = {name: _stat(definitions[name]) for name in required}
            expected_tally_specs = {
                "Li6 H3 production": ("tracklength", "(n,Xt)", "neutron", ["Li6"]),
                "Li6 (n,t) reactions": ("tracklength", "(n,t)", "neutron", ["Li6"]),
                "Li6 (n,gamma) reactions": ("tracklength", "(n,gamma)", "neutron", ["Li6"]),
                "Li6 neutron absorption": ("tracklength", "absorption", "neutron", ["Li6"]),
                "neutron MT301 heating": ("collision", "heating", "neutron", ["Li6"]),
                "photon deposition heating": ("collision", "heating", "photon", ["total"]),
                "electron deposition heating": ("collision", "heating", "electron", ["total"]),
                "positron deposition heating": ("collision", "heating", "positron", ["total"]),
                "all-particle total heating": ("collision", "heating", None, ["total"]),
            }
            statepoint_tallies = {}
            for name, tally_result in definitions.items():
                if name not in expected_tally_specs:
                    continue
                expected_estimator, expected_score, expected_particle, expected_nuclides = expected_tally_specs[name]
                observed_scores = list(tally_result.scores)
                observed_particle = None
                observed_cell = None
                for filt in tally_result.filters:
                    if type(filt).__name__ == "CellFilter":
                        observed_cell = [int(value) for value in filt.bins]
                    elif type(filt).__name__ == "ParticleFilter":
                        observed_particle = [str(value) for value in filt.bins]
                if (tally_result.estimator != expected_estimator
                        or observed_scores != [expected_score]
                        or observed_particle != ([expected_particle] if expected_particle else None)
                        or observed_cell != [model["cell_id"]]
                        or list(tally_result.nuclides) != expected_nuclides):
                    raise RuntimeError(f"statepoint tally definition mismatch for {name}")
                statepoint_tallies[name] = {
                    "score": observed_scores,
                    "estimator": tally_result.estimator,
                    "particle_filter": observed_particle,
                    "cell_filter": observed_cell,
                    "nuclides": list(tally_result.nuclides),
                    **stats[name],
                }
            record["statepoint_tallies"] = statepoint_tallies
            record["responses"] = {
                "h3_production": {**stats["Li6 H3 production"], "unit": "tritium_particles/source_neutron",
                                  "score": "H3-production", "estimator": "tracklength"},
                "n_t_reactions": {**stats["Li6 (n,t) reactions"], "unit": "reactions/source_neutron",
                                  "score": "(n,t)", "estimator": "tracklength"},
                "n_gamma_reactions": {**stats["Li6 (n,gamma) reactions"], "unit": "reactions/source_neutron",
                                      "score": "(n,gamma)", "estimator": "tracklength"},
                "neutron_absorption": {**stats["Li6 neutron absorption"], "unit": "reactions/source_neutron",
                                       "score": "absorption", "estimator": "tracklength"},
                "heating_neutron": {**stats["neutron MT301 heating"], "unit": "eV/source_neutron",
                                    "score": "heating", "particle_scope": "neutron", "estimator": "collision"},
                "heating_photon": {**stats["photon deposition heating"], "unit": "eV/source_neutron",
                                   "score": "heating", "particle_scope": "photon", "estimator": "collision"},
                "heating_electron": {**stats["electron deposition heating"], "unit": "eV/source_neutron",
                                     "score": "heating", "particle_scope": "electron", "estimator": "collision"},
                "heating_positron": {**stats["positron deposition heating"], "unit": "eV/source_neutron",
                                     "score": "heating", "particle_scope": "positron", "estimator": "collision"},
                "heating_total": {**stats["all-particle total heating"], "unit": "eV/source_neutron",
                                  "score": "heating", "particle_scope": "all", "estimator": "collision"},
            }
            record["acceptance"] = validate_responses(
                record["responses"], data["reaction_q_values"]["endf_b_vii_1_library"]
            )
        record["statepoint"] = {"path": statepoint.name, "sha256": sha256_file(statepoint),
                                 "bytes": statepoint.stat().st_size,
                                 "openmc_current_batch": args.batches,
                                 "openmc_version": openmc.__version__}
        record["status"] = record["acceptance"]["status"]
    record["output_files"] = _bounded_tree(out)
    record["output_manifest_scope"] = "all regular run files except this control-result.json self-record"
    record_path = out / "control-result.json"
    record_path.write_text(json.dumps(record, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    return record


def main(argv: list[str] | None = None) -> int:
    repo = Path(__file__).resolve().parents[2]
    default_data = repo / "data/raw/combined-fendl32-endfbvii1/cross_sections.xml"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--cross-sections", type=Path, default=default_data)
    parser.add_argument("--audit", type=Path, default=repo / "references/openmc-library-audit.json")
    parser.add_argument("--photon-provenance", type=Path, default=repo / "references/photon-library-provenance.json")
    parser.add_argument("--openmc", type=Path, required=True)
    parser.add_argument("--seed", type=lambda x: _positive_int(x, 1, 2**31 - 1, "seed"), default=DEFAULT_SEED)
    parser.add_argument("--batches", type=lambda x: _positive_int(x, 30, MAX_HISTORIES, "batches"), default=DEFAULT_BATCHES)
    parser.add_argument("--particles", type=lambda x: _positive_int(x, 1, MAX_HISTORIES, "particles"), default=DEFAULT_PARTICLES)
    parser.add_argument("--threads", type=lambda x: _positive_int(x, 1, MAX_THREADS, "threads"), default=DEFAULT_THREADS)
    parser.add_argument("--timeout", type=lambda x: _positive_int(x, 1, MAX_TIMEOUT_SECONDS, "timeout"), default=300)
    args = parser.parse_args(argv)
    try:
        record = run_control(args)
    except Exception as exc:
        print(f"control setup/execution error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(record, indent=2, sort_keys=True, allow_nan=False))
    return 0 if record["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
