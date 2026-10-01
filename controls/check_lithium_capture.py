#!/usr/bin/env python3
"""Independent acceptance and file-identity checker for the Li-6 control."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from decimal import Decimal, getcontext
from pathlib import Path
from typing import Any

getcontext().prec = 40
SCHEMA = "faris-openmc-lithium-capture-check/v0.1"
U_C2_EV = Decimal("931494103.72")
AME_MASS_U = {
    "Li6": Decimal("6.01512288742"),
    "n": Decimal("1.00866491590"),
    "H3": Decimal("3.01604928132"),
    "He4": Decimal("4.00260325413"),
    "Li7": Decimal("7.01600343426"),
}
REACTION_Q_TOLERANCE_EV = Decimal("1000")
RELATIVE_HEATING_FLOOR = Decimal("0.001")
SOURCE_ENERGY_EV = Decimal("0.0253")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def qvalue(*reactants: str, products: tuple[str, ...]) -> Decimal:
    initial = sum((AME_MASS_U[item] for item in reactants), Decimal(0))
    final = sum((AME_MASS_U[item] for item in products), Decimal(0))
    return (initial - final) * U_C2_EV


def finite_response(record: dict[str, Any], name: str, unit: str) -> tuple[Decimal, Decimal]:
    response = record.get("responses", {}).get(name)
    if not isinstance(response, dict) or response.get("unit") != unit:
        raise ValueError(f"missing {name} response or explicit unit {unit!r}")
    mean = Decimal(str(response.get("mean")))
    se = Decimal(str(response.get("standard_error")))
    if not mean.is_finite() or not se.is_finite() or mean < 0 or se < 0:
        raise ValueError(f"{name} response must have a finite nonnegative mean and standard error")
    return mean, se


def check_record(record_path: Path) -> dict[str, Any]:
    record_path = record_path.resolve(strict=True)
    if record_path.name != "control-result.json":
        raise ValueError("record path must name control-result.json")
    run_root = record_path.parent
    record = json.loads(record_path.read_text(encoding="utf-8"))
    if record.get("schema_version") != "faris-openmc-lithium-capture-control/v0.1":
        raise ValueError("unsupported Li-6 control record schema")
    if record.get("solver_status") != "COMPLETED" or record.get("solver_exit_code") != 0:
        raise ValueError("OpenMC did not report a completed successful execution")
    if record.get("solver", {}).get("version") != "0.15.3":
        raise ValueError("control requires the identified OpenMC 0.15.3 solver")
    if record.get("model", {}).get("source") != {
        "particle": "neutron", "energy_ev": 0.0253, "position_cm": [0, 0, 0],
        "direction": "isotropic", "strength": 1.0, "normalization": "per source neutron",
    }:
        raise ValueError("recorded source is not the predeclared central 0.0253 eV neutron source")
    material = record.get("model", {}).get("idealized_material", {})
    geometry = record.get("model", {}).get("geometry", {})
    settings = record.get("model", {}).get("settings", {})
    if (material.get("nuclide") != "Li6"
            or material.get("atom_density_atoms_per_barn_cm") != 0.01
            or material.get("density_is_a_control_assumption") is not True
            or material.get("physical_material_temperature_k", "missing") is not None
            or geometry != {"shape": "vacuum-bounded sphere", "radius_cm": 10.0}
            or settings.get("photon_transport") is not True
            or settings.get("atomic_relaxation") is not True
            or settings.get("electron_treatment") != "led"):
        raise ValueError("recorded model differs from the predeclared coupled Li-6 control")
    data_identity = record.get("data_identity", {})
    xml_entry = data_identity.get("cross_sections_xml", {})
    for key in ("cross_sections_xml", "neutron_file", "photon_file"):
        item = data_identity.get(key)
        if not isinstance(item, dict) or not item.get("sha256"):
            raise ValueError(f"missing exact data identity for {key}")
        data_path = (Path(item["path"]) if key == "cross_sections_xml"
                     else Path(xml_entry["path"]).parent / item["path_relative_to_xml"])
        if sha256(data_path.resolve(strict=True)) != item["sha256"]:
            raise ValueError(f"data identity changed for {key}")
    solver = record.get("solver", {})
    solver_executable = Path(solver.get("executable", "")).resolve(strict=True)
    if not solver_executable.is_file() or sha256(solver_executable) != solver.get("executable_sha256"):
        raise ValueError("identified OpenMC executable is missing or its bytes changed")
    statepoint = record.get("statepoint", {})
    statepoint_path = run_root / statepoint.get("path", "")
    if (not statepoint_path.is_file()
            or sha256(statepoint_path) != statepoint.get("sha256")
            or statepoint_path.stat().st_size != statepoint.get("bytes")
            or statepoint.get("openmc_version") != "0.15.3"
            or statepoint.get("openmc_current_batch") != record["sampling"]["batches"]):
        raise ValueError("statepoint file or run parameters do not match the record")
    if record["sampling"]["batches"] * record["sampling"]["particles_per_batch"] != record["sampling"]["nominal_histories"]:
        raise ValueError("nominal history count is inconsistent")

    # Rehash the materialized execution files, independent of the generator's
    # own output manifest. control-result.json is not self-included.
    for item in record.get("output_files", []):
        path = run_root / item["path"]
        if not path.is_file() or path.is_symlink() or path.stat().st_size != item["bytes"] or sha256(path) != item["sha256"]:
            raise ValueError(f"run output differs from its declared digest: {item.get('path')}")
    expected_outputs = {item["path"] for item in record.get("output_files", [])}
    if record.get("output_manifest_scope") != "all regular run files except this control-result.json self-record":
        raise ValueError("output manifest scope is missing or unsupported")
    actual_outputs = {str(path.relative_to(run_root)) for path in run_root.rglob("*") if path.is_file() and path.name != "control-result.json"}
    if actual_outputs != expected_outputs:
        raise ValueError("run directory contains undeclared output or omits a declared file")

    # Independently reopen the raw statepoint and compare each moment and score
    # identity to the emitted record. This checks that the numbers came from the
    # identified solver artifact, rather than just rechecking JSON arithmetic.
    try:
        import openmc
    except ImportError as exc:
        raise RuntimeError("run checker requires the selected OpenMC Python API") from exc
    if openmc.__version__ != "0.15.3":
        raise ValueError("run checker requires OpenMC Python API 0.15.3")
    expected_specs = {
        "Li6 H3 production": ("(n,Xt)", "tracklength", ["neutron"], ["Li6"], "h3_production"),
        "Li6 (n,t) reactions": ("(n,t)", "tracklength", ["neutron"], ["Li6"], "n_t_reactions"),
        "Li6 (n,gamma) reactions": ("(n,gamma)", "tracklength", ["neutron"], ["Li6"], "n_gamma_reactions"),
        "Li6 neutron absorption": ("absorption", "tracklength", ["neutron"], ["Li6"], "neutron_absorption"),
        "neutron MT301 heating": ("heating", "collision", ["neutron"], ["Li6"], "heating_neutron"),
        "photon deposition heating": ("heating", "collision", ["photon"], ["total"], "heating_photon"),
        "electron deposition heating": ("heating", "collision", ["electron"], ["total"], "heating_electron"),
        "positron deposition heating": ("heating", "collision", ["positron"], ["total"], "heating_positron"),
        "all-particle total heating": ("heating", "collision", None, ["total"], "heating_total"),
    }
    with openmc.StatePoint(statepoint_path) as sp:
        if ".".join(map(str, sp.version)) != "0.15.3" or sp.current_batch != record["sampling"]["batches"]:
            raise ValueError("statepoint solver version or batch count differs from the record")
        by_name = {tally.name: tally for tally in sp.tallies.values()}
        if set(by_name) != set(expected_specs):
            raise ValueError("statepoint does not contain exactly the seven expected tallies")
        stated = record.get("statepoint_tallies", {})
        if set(stated) != set(expected_specs):
            raise ValueError("run record omits statepoint tally identities")
        for name, (score, estimator, particle_bins, nuclides, response_key) in expected_specs.items():
            tally = by_name[name]
            observed_particles = None
            observed_cell = None
            for filt in tally.filters:
                if type(filt).__name__ == "CellFilter":
                    observed_cell = [int(value) for value in filt.bins]
                elif type(filt).__name__ == "ParticleFilter":
                    observed_particles = [str(value) for value in filt.bins]
            expected_particles = particle_bins
            if (tally.scores != [score] or tally.estimator != estimator
                    or observed_particles != expected_particles
                    or observed_cell != [record["model"]["cell_id"]]
                    or list(tally.nuclides) != nuclides):
                raise ValueError(f"statepoint score/filter/estimator identity mismatch: {name}")
            mean = float(tally.mean.ravel()[0])
            standard_error = float(tally.std_dev.ravel()[0])
            response = record["responses"][response_key]
            identity = stated[name]
            if (response["mean"] != mean or response["standard_error"] != standard_error
                    or identity.get("mean") != mean or identity.get("standard_error") != standard_error
                    or identity.get("score") != [score] or identity.get("estimator") != estimator
                    or identity.get("particle_filter") != expected_particles
                    or identity.get("cell_filter") != [record["model"]["cell_id"]]
                    or identity.get("nuclides") != nuclides):
                raise ValueError(f"reported moment differs from statepoint tally: {name}")

    h3, h3_se = finite_response(record, "h3_production", "tritium_particles/source_neutron")
    nt, nt_se = finite_response(record, "n_t_reactions", "reactions/source_neutron")
    ng, ng_se = finite_response(record, "n_gamma_reactions", "reactions/source_neutron")
    absorption, absorption_se = finite_response(record, "neutron_absorption", "reactions/source_neutron")
    heat, heat_se = finite_response(record, "heating_total", "eV/source_neutron")
    heat_n, heat_n_se = finite_response(record, "heating_neutron", "eV/source_neutron")
    heat_g, heat_g_se = finite_response(record, "heating_photon", "eV/source_neutron")
    heat_e, heat_e_se = finite_response(record, "heating_electron", "eV/source_neutron")
    heat_p, heat_p_se = finite_response(record, "heating_positron", "eV/source_neutron")

    q_nt_ame = qvalue("Li6", "n", products=("H3", "He4"))
    q_gamma_ame = qvalue("Li6", "n", products=("Li7",))
    endf = record["data_identity"]["reaction_q_values"]["endf_b_vii_1_library"]
    q_nt_endf = Decimal(str(endf["Li6_n_t_ENDF_Q_eV"]))
    q_gamma_endf = Decimal(str(endf["Li6_n_gamma_ENDF_Q_eV"]))
    q_crosscheck_ok = (
        abs(q_nt_ame - q_nt_endf) <= REACTION_Q_TOLERANCE_EV
        and abs(q_gamma_ame - q_gamma_endf) <= REACTION_Q_TOLERANCE_EV
    )

    h3_delta = abs(h3 - nt)
    h3_tol = Decimal(3) * (h3_se + nt_se) + Decimal("1e-10")
    absorption_delta = abs(absorption - nt - ng)
    absorption_tol = Decimal(3) * (absorption_se + nt_se + ng_se) + Decimal("1e-10")

    q_nt_low = min(q_nt_ame, q_nt_endf) - REACTION_Q_TOLERANCE_EV
    q_nt_high = max(q_nt_ame, q_nt_endf) + REACTION_Q_TOLERANCE_EV
    q_gamma_high = max(q_gamma_ame, q_gamma_endf) + REACTION_Q_TOLERANCE_EV
    physical_low = h3 * q_nt_low - SOURCE_ENERGY_EV
    physical_high = h3 * q_nt_high + ng * q_gamma_high + SOURCE_ENERGY_EV
    sampling_radius = Decimal(3) * (heat_se + h3_se * q_nt_high + ng_se * q_gamma_high)
    model_floor = max(Decimal(1), h3 * max(q_nt_ame, q_nt_endf) * RELATIVE_HEATING_FLOOR)
    accepted_low = physical_low - sampling_radius - model_floor
    accepted_high = physical_high + sampling_radius + model_floor
    heat_delta = abs(heat - heat_n - heat_g - heat_e - heat_p)
    heat_component_tol = Decimal(3) * (heat_se + heat_n_se + heat_g_se + heat_e_se + heat_p_se) + Decimal("1e-6")
    checks = {
        "ame2020_and_endf_q_crosscheck": q_crosscheck_ok,
        "h3_production_matches_n_t_events": h3_delta <= h3_tol,
        "absorption_is_n_t_plus_n_gamma": absorption_delta <= absorption_tol,
        "coupled_heating_in_conservative_energy_interval": accepted_low <= heat <= accepted_high,
        "all_particle_heat_is_neutron_photon_electron_plus_positron_heat": heat_delta <= heat_component_tol,
    }
    status = "PASS" if all(checks.values()) else "FAIL"
    return {
        "schema_version": SCHEMA,
        "status": status,
        "scope": "isolated idealized 6Li thermal-neutron numerical control; not reactor qualification",
        "record_sha256": sha256(record_path),
        "input_sha256": record.get("data_identity", {}).get("combined_control_input_sha256"),
        "independent_ame2020_q_eV": {"n_t": str(q_nt_ame), "n_gamma": str(q_gamma_ame)},
        "mass_q_acceptance_tolerance_ev": str(REACTION_Q_TOLERANCE_EV),
        "heating_interval_eV_per_source": {
            "physical_lower": float(physical_low), "physical_upper": float(physical_high),
            "sampling_expansion": float(sampling_radius), "relative_model_floor": float(model_floor),
            "accepted_lower": float(accepted_low), "accepted_upper": float(accepted_high),
            "observed_heating_mean": float(heat),
        },
        "checks": checks,
        "limitations": [
            "All score means are source-normalized OpenMC responses; no fusion source rate or volume division is applied.",
            "OpenMC standard errors and the three-standard-error screening are heuristic; no rigorous confidence level is asserted.",
            "Li6(n,gamma) photon local deposition is bounded from zero to the full reaction Q; photon leakage energy is not tallied.",
            "The control material atom density is an explicit numerical assumption and is not a physical FARIS material recipe.",
            "A PASS verifies only these response identities and conservative bounds for this exact local library/model.",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path, help="path to control-result.json")
    args = parser.parse_args()
    try:
        result = check_record(args.record)
    except Exception as exc:
        print(json.dumps({"schema_version": SCHEMA, "status": "NOT_EVALUATED",
                          "failure": f"{type(exc).__name__}: {exc}"}, indent=2))
        return 2
    print(json.dumps(result, indent=2, sort_keys=True, allow_nan=False))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
