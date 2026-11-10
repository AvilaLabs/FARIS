#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""RM-M model acceptance: the three checks that must pass before any risk test.

Run with an OpenMC interpreter (0.15.3, ~/.venvs/w003env) under the memory cap:

    PYTHONPATH=<dir with openmc_plasma_source and NeSST> accept_rm_m.py --model-dir DIR --out acceptance.json

Checks (docs/notes/CAD_TRANSPORT_RISK_TESTS.md, Model acceptance):
  1. per-solid faceted volume within 0.5 % of the CAD volume (GEO-030);
  2. lost particles <= 1e-6 per history over 1e6 histories (GEO-025);
  3. 100 % of sampled source sites inside the plasma volume (SRC-018).
The CAD volumes come from the build (rm_m_roles.json); the faceted volumes are
read from the h5m file itself.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h5m_volumes  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402

THERMAL_SPLIT_EV = (0.0, 1.0e3, 5.0e7)
THERMAL_TALLY_MAX_BINS = 8192
THERMAL_TALLY_BYTES_PER_BIN_ESTIMATE = 24


def _canonical_sha256(value: dict) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def thermal_diagnostic_plan(openmc, model) -> tuple[list, dict]:
    """Build bounded flux/capture tallies split at the R2S 1-keV lower edge."""
    materials = sorted(list(model.materials), key=lambda m: (str(m.name), int(m.id)))
    material_rows = []
    captures = []
    for material in materials:
        nuclides = sorted(set(str(n) for n in material.get_nuclides()))
        material_rows.append({"name": str(material.name), "id": int(material.id), "nuclides": nuclides})
        if nuclides:
            tally = openmc.Tally(name=f"thermal_capture_{int(material.id)}")
            tally.filters = [openmc.MaterialFilter([material]), openmc.EnergyFilter(THERMAL_SPLIT_EV)]
            tally.nuclides = nuclides
            tally.scores = ["102"]  # ENDF MT 102, (n,gamma)
            captures.append(tally)
    if len({row["id"] for row in material_rows}) != len(material_rows):
        raise ValueError("thermal diagnostic requires unique OpenMC material IDs")
    if len({row["name"] for row in material_rows}) != len(material_rows):
        raise ValueError("thermal diagnostic requires unique material names for its exported report")
    flux = openmc.Tally(name="thermal_split_material_flux")
    flux.filters = [openmc.MaterialFilter(materials), openmc.EnergyFilter(THERMAL_SPLIT_EV)]
    flux.nuclides = ["total"]
    flux.scores = ["flux"]
    bins = len(materials) * 2 + sum(len(row["nuclides"]) * 2 for row in material_rows)
    estimate = bins * THERMAL_TALLY_BYTES_PER_BIN_ESTIMATE
    if not materials or bins > THERMAL_TALLY_MAX_BINS or estimate > 256 * 1024:
        raise ValueError(f"thermal diagnostic tally estimate exceeds its bound: {bins} bins, {estimate} bytes")
    plan = {"schema": "faris-rm-m-thermal-activation-diagnostic/v1",
            "purpose": "descriptive CE flux and MT102 capture split at 1 keV; not full activation or dose convergence",
            "energy_edges_eV": list(THERMAL_SPLIT_EV), "energy_bins": ["0-1 keV", "1 keV-50 MeV"],
            "reaction_mt": 102, "reaction": "(n,gamma)", "materials": material_rows,
            "tally_bin_count_estimate": bins, "tally_bytes_estimate": estimate,
            "tally_bytes_per_bin_estimate": THERMAL_TALLY_BYTES_PER_BIN_ESTIMATE}
    plan["configuration_sha256"] = _canonical_sha256(plan)
    return [flux, *captures], plan


def _batch_fraction_summary(thermal_batches: list[float], fast_batches: list[float]) -> dict:
    """Ratio of paired batch sums with delta-method SE retaining shared-history covariance."""
    if len(thermal_batches) != len(fast_batches) or len(thermal_batches) < 2:
        return {"thermal_fraction": None, "standard_error": None, "status": "UNAVAILABLE",
                "uncertainty": "unavailable: fewer than two aligned batches or mismatched batch data"}
    low = [float(x) for x in thermal_batches]
    high = [float(x) for x in fast_batches]
    den = math.fsum(low) + math.fsum(high)
    if not math.isfinite(den) or den <= 0.0 or any(not math.isfinite(x) for x in low + high):
        return {"thermal_fraction": None, "standard_error": None, "status": "UNAVAILABLE",
                "uncertainty": "unavailable: non-finite or non-positive total"}
    fraction = math.fsum(low) / den
    residuals = [a - fraction * (a + b) for a, b in zip(low, high)]
    mean_total = den / len(low)
    residual_mean = math.fsum(residuals) / len(residuals)
    variance = math.fsum((x - residual_mean) ** 2 for x in residuals) / (len(residuals) - 1)
    se = math.sqrt(variance / len(residuals)) / mean_total
    return {"thermal_fraction": fraction, "standard_error": se, "status": "AVAILABLE",
            "uncertainty": "batch-sampling delta-method SE from aligned batch totals; shared-history covariance retained"}


def read_thermal_diagnostic(openmc, run_dir: Path, batches: int, particles_per_batch: int,
                            plan: dict) -> dict:
    """Read aligned cumulative tally sums and report per-material flux and capture fractions."""
    import numpy as np

    descriptors = plan["materials"]
    prior_capture = {row["id"]: [0.0] * (2 * len(row["nuclides"])) for row in descriptors}
    capture_batch_values = {row["id"]: [[] for _ in row["nuclides"]] for row in descriptors}
    flux_batch_values = {row["name"]: [[], []] for row in descriptors}
    prior_flux = [0.0] * (2 * len(descriptors))
    statepoints = _statepoint_paths(run_dir, batches)
    for batch in range(1, batches + 1):
        path = statepoints[batch]
        with openmc.StatePoint(str(path)) as sp:
            flux_tally = sp.get_tally(name="thermal_split_material_flux")
            flux_data = np.asarray(flux_tally.get_reshaped_data(value="sum"), dtype=float)
            if flux_data.shape != (len(descriptors), 2, 1, 1):
                raise ValueError(f"thermal flux tally shape {flux_data.shape} != {(len(descriptors), 2, 1, 1)}")
            flux_sum = flux_data[:, :, 0, 0].ravel().tolist()
            current_flux = [x - y for x, y in zip(flux_sum, prior_flux)]
            if any(not math.isfinite(value) or value < 0.0 for value in current_flux):
                raise ValueError(f"invalid thermal flux batch increments at batch {batch}")
            for i, row in enumerate(descriptors):
                flux_batch_values[row["name"]][0].append(current_flux[2 * i])
                flux_batch_values[row["name"]][1].append(current_flux[2 * i + 1])
            prior_flux = flux_sum
            for row in descriptors:
                nuclides = row["nuclides"]
                if not nuclides:
                    continue
                tally = sp.get_tally(name=f"thermal_capture_{row['id']}")
                capture_data = np.asarray(tally.get_reshaped_data(value="sum"), dtype=float)
                if capture_data.shape != (1, 2, len(nuclides), 1):
                    raise ValueError(f"thermal MT102 tally shape for {row['name']} is {capture_data.shape}; expected {(1, 2, len(nuclides), 1)}")
                current = capture_data[0, :, :, 0].ravel().tolist()
                increments = [x - y for x, y in zip(current, prior_capture[row["id"]])]
                if any(not math.isfinite(value) or value < 0.0 for value in increments):
                    raise ValueError(f"invalid MT102 batch increments for {row['name']} at batch {batch}")
                for index, _nuc in enumerate(nuclides):
                    capture_batch_values[row["id"]][index].append(increments[index])
                    capture_batch_values[row["id"]][index].append(increments[len(nuclides) + index])
                prior_capture[row["id"]] = current
    result_materials = []
    for row in descriptors:
        name = row["name"]
        flux_low, flux_fast = flux_batch_values[name]
        flux_summary = _batch_fraction_summary(flux_low, flux_fast)
        capture_rows = []
        for idx, nuc in enumerate(row["nuclides"]):
            values = capture_batch_values[row["id"]][idx]
            thermal, fast = values[0::2], values[1::2]
            capture_rows.append({"nuclide": nuc, **_batch_fraction_summary(thermal, fast)})
        result_materials.append({"material": name, "material_id": row["id"],
                                 "flux_thermal_fraction": flux_summary["thermal_fraction"],
                                 "flux_standard_error": flux_summary["standard_error"],
                                 "captures_mt102": capture_rows})
    return {"plan": plan, "materials": result_materials, "batches": batches,
            "particles_per_batch": particles_per_batch, "histories": batches * particles_per_batch,
            "status": "DESCRIPTIVE_ONLY",
            "qualification_verdict": "NOT_PROVIDED: capture/flux diagnostic only; not full activation, shutdown dose, or activation-grid convergence"}


def _statepoint_paths(run_dir: Path, batches: int) -> dict[int, Path]:
    """Discover batch files by numeric suffix and require exactly one complete file per batch."""
    found = {}
    for path in Path(run_dir).glob("statepoint.*.h5"):
        match = re.fullmatch(r"statepoint\.(\d+)\.h5", path.name)
        if match:
            batch = int(match.group(1))
            if batch in found:
                raise ValueError(f"duplicate statepoint file for batch {batch}")
            found[batch] = path
    expected = set(range(1, batches + 1))
    if set(found) != expected:
        raise ValueError(f"statepoint batches {sorted(found)} do not match expected {sorted(expected)}")
    return found


def prepare_thermal_protocol(model, model_dir: Path, run_dir: Path, h5m_name: str, source_unscaled: float,
                             histories: int, batches: int, threads: int, plan: dict) -> dict:
    """Export and validate all bound inputs before the costly transport starts."""
    model_xml = Path(run_dir) / "model.xml"
    materials_xml = Path(run_dir) / "thermal-materials.xml"
    model.export_to_model_xml(str(model_xml))
    model.materials.export_to_xml(path=materials_xml)
    tally_rows = [{"id": int(tally.id), "name": str(tally.name)} for tally in model.tallies]
    tally_ids = [row["id"] for row in tally_rows]
    if len(tally_ids) != len(set(tally_ids)):
        raise ValueError("acceptance and thermal diagnostic tallies have duplicate OpenMC tally IDs")
    required = (model_xml, materials_xml, Path(model_dir) / h5m_name,
                Path(model_dir) / "rm_m_roles.json", RM.FARIS_XS)
    if any(not path.is_file() for path in required):
        raise ValueError("thermal diagnostic protocol inputs are incomplete before neutron transport")
    actual_histories = batches * (histories // batches)
    protocol = {"schema": "faris-rm-m-thermal-activation-diagnostic-run/v1",
                "h5m_sha256": RM.sha256(Path(model_dir) / h5m_name),
                "roles_sha256": RM.sha256(Path(model_dir) / "rm_m_roles.json"),
                "model_xml_sha256": RM.sha256(model_xml),
                "materials_xml_sha256": RM.sha256(materials_xml),
                "cross_sections_xml_sha256": RM.sha256(RM.FARIS_XS),
                "tally_configuration": {"plan": plan, "tallies": tally_rows},
                "source": {"kind": "RM-M tokamak mesh source constrained to plasma cell", "seed": model.settings.seed,
                           "strength_n_s": 1.0, "unscaled_source_rate_n_s": source_unscaled,
                           "profile": RM.PLASMA_PROFILE},
                "sampling": {"requested_histories": histories, "histories": actual_histories, "batches": batches,
                             "particles_per_batch": histories // batches, "threads": threads},
                "physics_settings": {"run_mode": "fixed source", "photon_transport": False,
                                     "temperature": RM.DATA_TEMPERATURE_K, "neutron_cutoff_eV": None}}
    protocol["protocol_sha256"] = _canonical_sha256(protocol)
    protocol_path = Path(run_dir) / "thermal-diagnostic-protocol.json"
    protocol_path.write_text(json.dumps(protocol, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    serialized = json.loads(protocol_path.read_text(encoding="utf-8"))
    digest = serialized.pop("protocol_sha256", None)
    if digest != _canonical_sha256(serialized) or len(RM.sha256(protocol_path)) != 64:
        raise ValueError("thermal diagnostic protocol failed its pre-run hash validation")
    return protocol


def read_thermal_tf_flux(openmc, run_dir: Path, batches: int, tally_name: str = "tf_fast_flux") -> dict:
    """Read TF response and retain all statepoints for diagnostic review and replay."""
    paths = _statepoint_paths(run_dir, batches)
    cumulative, per_cell_final = [], None
    for batch in range(1, batches + 1):
        path = paths[batch]
        with openmc.StatePoint(str(path)) as sp:
            tally = sp.get_tally(name=tally_name)
            cumulative.append(float(tally.sum.sum()))
            if batch == batches:
                per_cell_final = (tally.mean.ravel().tolist(), tally.std_dev.ravel().tolist())
    stats = RM.batch_statistics(RM.cumulative_to_batches(cumulative))
    stats["per_cell_mean"], stats["per_cell_std_dev"] = per_cell_final
    return stats


def check_volumes(model_dir: Path, h5m_name: str) -> dict:
    roles = RM.load_roles(model_dir)
    facets = h5m_volumes.facet_volumes(str(model_dir / h5m_name))
    rows = []
    for r in roles:
        rows.append({"name": r["name"], "cad_volume_cm3": r["cad_volume_cm3"],
                     "faceted_volume_cm3": facets[r["volume_id"]]["faceted_volume"], "volume_id": r["volume_id"],
                     "triangles": facets[r["volume_id"]]["triangles"]})
    result = checks.volume_comparison(rows)
    result["h5m"] = h5m_name
    result["h5m_sha256"] = RM.sha256(model_dir / h5m_name)
    result["total_triangles"] = sum(r["triangles"] for r in rows)
    return result


def sample_sites(openmc, model_dir: Path, run_dir: Path, constrained: bool, n: int, seed: int, threads: int, h5m_name: str) -> dict:
    """Sample source sites with the real OpenMC source machinery and locate each in the DAGMC model."""
    import numpy as np
    from matplotlib.path import Path as MplPath

    run_dir.mkdir(parents=True, exist_ok=True)
    roles = RM.load_roles(model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, model_dir / h5m_name, roles)
    source, unscaled = RM.tokamak_mesh_source(openmc, ids["plasma"] if constrained else None)
    model.settings.run_mode = "fixed source"
    model.settings.batches = 1
    model.settings.particles = 1000
    model.settings.source = [source]
    model.settings.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    model.export_to_model_xml(str(run_dir))
    cwd = os.getcwd()
    os.chdir(run_dir)
    import openmc.lib

    cell_of = {}
    outside_any = 0
    try:
        openmc.lib.init(["--threads", str(threads)])
        t0 = time.time()
        sites = openmc.lib.sample_external_source(n, prn_seed=seed)
        sample_seconds = time.time() - t0
        points = np.array([s.r for s in sites])
        t0 = time.time()
        for r in points:
            try:
                cell, _ = openmc.lib.find_cell(tuple(float(v) for v in r))
                cell_of[cell.id] = cell_of.get(cell.id, 0) + 1
            except Exception:
                outside_any += 1
        locate_seconds = time.time() - t0
    finally:
        openmc.lib.finalize()
        os.chdir(cwd)
    inside = cell_of.get(ids["plasma"], 0)
    # Independent check against the analytic plasma boundary polygon (no DAGMC involved).
    boundary = MplPath(np.array([spec.plasma_point(2.0 * math.pi * i / 2000) for i in range(2000)]))
    rz = np.column_stack([np.hypot(points[:, 0], points[:, 1]), points[:, 2]])
    analytic_inside = int(boundary.contains_points(rz).sum())
    result = checks.source_site_check(inside, n)
    result.update({
        "constrained_to_plasma_cell": constrained, "seed": seed,
        "sites_by_volume_id": {str(k): v for k, v in sorted(cell_of.items())},
        "sites_not_in_any_cell": outside_any,
        "analytic_polygon_inside": analytic_inside,
        "analytic_fraction_inside": analytic_inside / n,
        "unscaled_source_rate_n_s": unscaled,
        "sample_seconds": sample_seconds, "locate_seconds": locate_seconds,
    })
    return result


def run_analog_with_thermal_diagnostic(openmc, model_dir: Path, run_dir: Path, histories: int, batches: int,
                                      seed: int, threads: int, h5m_name: str) -> dict:
    """Acceptance neutron run with optional diagnostic tallies on the same histories."""
    if histories <= 0 or batches <= 0 or histories % batches or threads <= 0:
        raise ValueError("thermal diagnostic requires positive threads and positive histories divisible by batches")
    model_dir, run_dir = Path(model_dir), Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    roles = RM.load_roles(model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, model_dir / h5m_name, roles)
    source, unscaled = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    settings = model.settings
    settings.run_mode = "fixed source"
    settings.batches = batches
    settings.particles = histories // batches
    settings.seed = seed
    settings.source = [source]
    settings.photon_transport = False
    settings.max_lost_particles = 10000
    settings.rel_max_lost_particles = 0.5
    settings.statepoint = {"batches": list(range(1, batches + 1))}
    settings.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    settings.output = {"summary": False, "tallies": False}
    tf_tally = RM.tf_fast_flux_tallies(openmc, ids["tf"])
    diagnostic_tallies, plan = thermal_diagnostic_plan(openmc, model)
    model.tallies = openmc.Tallies([tf_tally, *diagnostic_tallies])
    protocol = prepare_thermal_protocol(model, model_dir, run_dir, h5m_name, unscaled,
                                        histories, batches, threads, plan)
    protocol_file_sha256 = RM.sha256(run_dir / "thermal-diagnostic-protocol.json")
    run = RM.run_openmc_cli(run_dir, threads)
    result = {"run_dir": str(run_dir), "openmc_version": openmc.__version__, "histories": histories,
              "batches": batches, "particles_per_batch": histories // batches, "seed": seed,
              "threads": threads, "weight_windows": None, "source_unscaled_rate_n_s": unscaled, "run": run}
    if run["returncode"] == 0:
        result["lost"] = RM.count_lost_particles(run_dir)
        thermal = read_thermal_diagnostic(openmc, run_dir, batches, histories // batches, plan)
        thermal.update({"protocol": protocol,
                        "protocol_file_sha256": protocol_file_sha256,
                        "uncertainty_scope": "batch-sampling uncertainty from the same acceptance histories; no nuclear-data or model uncertainty"})
        result["thermal_activation_diagnostic"] = thermal
        result["tf_fast_flux"] = read_thermal_tf_flux(openmc, run_dir, batches)
        result["tf_fast_flux"]["unit"] = checks.UNIT_PER_SOURCE
    result["source_strength"] = 1.0
    return result


def check_lost_particles(openmc, model_dir: Path, run_dir: Path, histories: int, batches: int, threads: int, seed: int, h5m_name: str, reuse: bool = False,
                         thermal_diagnostic: bool = False) -> dict:
    if reuse:
        if thermal_diagnostic:
            raise ValueError("thermal diagnostic requires a fresh acceptance transport with its diagnostic tallies")
        # evaluate a finished run's output without repeating it (used when only the post-processing was fixed)
        result = {"run_dir": str(run_dir), "openmc_version": openmc.__version__, "histories": histories, "batches": batches,
                  "seed": seed, "threads": threads, "reused_existing_run": True, "run": {"returncode": 0},
                  "lost": RM.count_lost_particles(run_dir)}
        result["tf_fast_flux"] = RM.read_tf_flux(openmc, run_dir, batches)
    else:
        if run_dir.exists():
            for p in run_dir.glob("*"):
                p.unlink()
        if thermal_diagnostic:
            result = run_analog_with_thermal_diagnostic(openmc, model_dir, run_dir, histories, batches,
                                                        seed, threads, h5m_name)
        else:
            result = RM.run_analog(openmc, model_dir, run_dir, histories, batches, seed, threads, h5m_name=h5m_name)
    if result["run"]["returncode"] != 0:
        return {"pass": False, "error": f"openmc exited {result['run']['returncode']}", "run": result}
    lost = result["lost"]["restart_files"]
    rule = checks.lost_particle_check(lost, histories)
    rule["lost_log_lines"] = result["lost"]["log_lines_mentioning_lost"]
    rule["run"] = result
    return rule


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--h5m", default="rm_m.h5m")
    ap.add_argument("--out", required=True, type=Path, help="acceptance result JSON")
    ap.add_argument("--run-dir", required=True, type=Path, help="scratch directory for the OpenMC runs")
    ap.add_argument("--histories", type=int, default=1_000_000)
    ap.add_argument("--batches", type=int, default=100)
    ap.add_argument("--sites", type=int, default=200_000)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--seed", type=int, default=20261009)
    ap.add_argument("--reuse-lost-run", action="store_true", help="evaluate the existing lost-particle run directory instead of running it again")
    ap.add_argument("--skip-lost-particle-run", action="store_true")
    ap.add_argument("--thermal-activation-diagnostic", action="store_true", default=False,
                    help="add a bounded 0-1 keV / >=1 keV material flux and MT102 tally to the same lost-particle acceptance histories")
    args = ap.parse_args()
    if args.thermal_activation_diagnostic and (args.skip_lost_particle_run or args.reuse_lost_run):
        ap.error("--thermal-activation-diagnostic requires a fresh lost-particle acceptance transport")
    if args.histories <= 0 or args.batches <= 0 or args.histories % args.batches or args.sites <= 0 or args.threads <= 0:
        ap.error("histories must be positive and divisible by batches; sites and threads must be positive")
    import openmc

    t0 = time.time()
    out = {"schema": "faris-rm-m-acceptance/v1", "openmc_version": openmc.__version__,
           "library": RM.library_identity(), "model_dir": str(args.model_dir)}
    out["volumes"] = check_volumes(args.model_dir, args.h5m)
    print("volumes:", out["volumes"]["pass"], out["volumes"]["worst"], flush=True)
    out["source_sites_unconstrained"] = sample_sites(openmc, args.model_dir, args.run_dir / "sites_free", False, args.sites, args.seed, args.threads, args.h5m)
    print("sites (free):", out["source_sites_unconstrained"]["fraction_inside"], flush=True)
    out["source_sites"] = sample_sites(openmc, args.model_dir, args.run_dir / "sites", True, args.sites, args.seed, args.threads, args.h5m)
    print("sites (plasma-cell constraint):", out["source_sites"]["fraction_inside"], flush=True)
    if not args.skip_lost_particle_run:
        out["lost_particles"] = check_lost_particles(openmc, args.model_dir, args.run_dir / "lost", args.histories, args.batches, args.threads, args.seed, args.h5m, args.reuse_lost_run,
                                                     args.thermal_activation_diagnostic)
        run_record = out["lost_particles"].get("run", {})
        if args.thermal_activation_diagnostic and "thermal_activation_diagnostic" in run_record:
            out["thermal_activation_diagnostic"] = run_record["thermal_activation_diagnostic"]
        print("lost:", out["lost_particles"].get("lost"), "of", out["lost_particles"].get("histories"), flush=True)
    required = ["volumes", "source_sites"] + ([] if args.skip_lost_particle_run else ["lost_particles"])
    out["accepted"] = all(out[k]["pass"] for k in required)
    out["wall_seconds"] = time.time() - t0
    out["peak_rss_mb_self"] = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
    out["peak_rss_mb_children"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(out, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print("accepted:", out["accepted"])
    return 0 if out["accepted"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
