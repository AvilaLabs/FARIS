#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""R3, direct one-step (D1S, openmc.deplete.d1s) on RM-M, OpenMC 0.15.3.

    r3_d1s.py --model-dir D --out-dir R [--particles N] [--budget S]

One coupled neutron and photon run with delayed (decay) photons sampled at the reaction site
(settings.use_decay_photons, chain from OPENMC_CHAIN_FILE). The dose tally carries a ParentNuclideFilter; the time
correction factors for 1 full-power year at 525 MW and cooling times of 1 d, 7 d and 30 d are applied afterwards.

The scored mesh is the R3 response region. The prospective full-emission protocol scores all photon descendants
entering this region, without a MeshBornFilter; R2S uses a separately discretized activation mesh spanning the
complete H5M geometry. The neutron source strength is 1, so the tally is per source neutron and the time
correction factors carry the source rate: the integrated tally is pSv.cm3/s per voxel, as for R2S.

The run uses the OpenMC C API batch loop. Only the cumulative tally array and compact aggregate batch totals stay
in memory; per-batch statepoints are not written. Aggregate uncertainty is calculated from parent- and
space-weighted batch totals, retaining their shared-history covariance. Per-voxel standard errors are omitted because
parent-nuclide covariance is not separately estimated for each voxel.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import resource
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import r3_dose_statistics as DS  # noqa: E402
import r3_shutdown_dose_export as DX  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--particles", type=int, default=5000, help="source neutrons per batch")
    ap.add_argument("--max-batches", type=int, default=100000)
    ap.add_argument("--budget", type=float, default=7000.0, help="wall seconds for the transport run")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--seed", type=int, default=20261301)
    args = ap.parse_args()
    args.model_dir = args.model_dir.resolve()
    args.out_dir = args.out_dir.resolve()
    import numpy as np
    import openmc
    import openmc.deplete
    from openmc.deplete import d1s

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["OPENMC_CHAIN_FILE"] = str(CHAIN)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)

    from r3_r2s import dose_tally, scoring_mesh  # after the environment is set

    mesh = scoring_mesh(openmc)
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    model, _ = RM.build_model(openmc, args.model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = model.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = args.max_batches, args.particles, args.seed
    s.source = [source]
    s.photon_transport = True
    s.use_decay_photons = True
    s.statepoint = {}
    s.sourcepoint = {"write": False}
    s.max_lost_particles = 10000
    s.rel_max_lost_particles = 0.5
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    tally = dose_tally(openmc, mesh)
    model.tallies = openmc.Tallies([tally])
    nuclides = d1s.prepare_tallies(model, chain_file=str(CHAIN))
    run_dir = out / "run"
    run_dir.mkdir(parents=True, exist_ok=True)
    for old in run_dir.glob("statepoint.*.h5"):
        old.unlink()
    model.export_to_model_xml(str(run_dir))
    record = {"kind": "r3-d1s", "openmc_version": openmc.__version__, "threads": args.threads,
              "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
              "chain": str(CHAIN), "chain_sha256": RM.sha256(CHAIN),
              "library": RM.library_identity(), "seed": args.seed, "particles_per_batch": args.particles, "budget_s": args.budget,
              "mesh": {"lower_left_cm": sc.MESH_LOWER_CM, "upper_right_cm": sc.MESH_UPPER_CM, "dimension": sc.MESH_DIMENSION},
              "radionuclides": nuclides, "n_radionuclides": len(nuclides),
              "settings": {"photon_transport": True, "use_decay_photons": True, "born_filter": None,
                           "dose_scope": "all transported photon births and descendants; score only in the response mesh",
                           "timesteps": sc.TIMESTEPS, "source_rates": sc.SOURCE_RATES, "max_batches": args.max_batches},
              "workarounds": ["openmc.config['cross_sections'] set to the audited library", "OPENMC_CHAIN_FILE exported for the openmc executable"]}
    energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
    record["shutdown_dose_basis"] = DX.write_basis_artifacts(
        out, model, source, mesh,
        {"timesteps": sc.TIMESTEPS, "source_rates_neutrons_s": sc.SOURCE_RATES,
         "chain_path": str(CHAIN), "chain_sha256": RM.sha256(CHAIN)}, energies, coefficients,
        sc.rm_m_geometry_dependencies(args.model_dir),
        RM.library_identity())
    tcf = d1s.time_correction_factors(nuclides, sc.TIMESTEPS, sc.SOURCE_RATES)
    parent_filter = next(f for f in tally.filters if isinstance(f, openmc.ParentNuclideFilter))
    parent_names = [str(x) for x in parent_filter.bins]
    if parent_names != [str(x) for x in nuclides]:
        raise RuntimeError("D1S ParentNuclideFilter order differs from the recorded radionuclide order")
    n_vox = int(np.prod(sc.MESH_DIMENSION))
    previous = np.zeros(n_vox * len(parent_names), dtype=float)
    aggregate_batches = {idx: [] for idx in sc.PHOTON_TIME_INDICES}

    def consume_batch(cumulative, _batch_number):
        nonlocal previous
        prior = previous
        for idx in sc.PHOTON_TIME_INDICES:
            factors = [tcf[n][idx] for n in parent_names]
            _current, aggregate = DS.d1s_batch_total(cumulative, prior, n_vox, len(parent_names), factors)
            aggregate_batches[idx].append(aggregate)
        previous = np.asarray(cumulative, dtype=float).copy()

    run, final_sums, _final_std = DS.run_live_tally(openmc, run_dir / "model.xml", tally.id, args.threads,
                                                    args.max_batches, args.budget, consume_batch,
                                                    particles_per_batch=args.particles)
    record["run"] = run
    record["batches"] = run["batches"]
    record["histories"] = record["batches"] * args.particles
    if run.get("stop_reason") not in ("batch_limit", "time_budget"):
        record["error"] = f"solver ended with {run.get('stop_reason')}; shutdown-dose result refused"
        (out / "d1s_record.json").write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
        return 1
    loss_status = run.get("integrity", {}).get("lost_particle_assessment", {}).get("status")
    if loss_status != "PASS":
        record["error"] = f"lost-particle assessment is {loss_status}; shutdown-dose summary refused"
        (out / "d1s_record.json").write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
        return 1
    if record["batches"] < 2:
        record["error"] = "fewer than two complete batches; aggregate Monte Carlo standard error unavailable"
        (out / "d1s_record.json").write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
        return 1
    record["shutdown_dose"] = {}
    per_nuclide_tot = {}
    record["parent_filter_order"] = [str(f.__class__.__name__) for f in tally.filters]
    cumulative = final_sums.reshape(n_vox, len(parent_names))
    mean_all = cumulative / record["batches"]
    dose_batch_arrays = {}
    for idx in sc.PHOTON_TIME_INDICES:
        label = sc.COOLING_LABELS[idx]
        factors = np.array([tcf[n][idx] for n in parent_names], dtype=float)
        mean = (mean_all * factors[None, :]).sum(axis=1).reshape(sc.MESH_DIMENSION[::-1]).copy()
        batch_values = np.asarray(aggregate_batches[idx], dtype=float)
        summary = DS.batch_summary(batch_values)
        if not np.isclose(summary["mean"], float(mean.sum()), rtol=1.0e-10, atol=1.0e-14):
            raise RuntimeError(f"D1S aggregate batch mean does not reconcile to the tally mean at {label}")
        np.savez(out / f"dose_time_{idx}.npz", mean=mean)
        dose_batch_arrays[f"time_{idx}"] = batch_values
        by_nuc = (mean_all * factors[None, :]).sum(axis=0)
        per_nuclide_tot[label] = {n: float(v) for n, v in zip(parent_names, by_nuc) if v > 0.0}
        record["shutdown_dose"][label] = {"time_index": idx, **summary,
            "unit": "pSv.cm3/s summed over the scored region",
            "uncertainty_scope": "Monte Carlo transport batch sampling only; excludes nuclear-data and model uncertainty",
            "method_specific_source": DX.write_d1s_source_artifact(
                out, idx, parent_names, {n: tcf[n][idx] for n in parent_names}, record["chain_sha256"])}
    batch_path = out / "shutdown_dose_batch_totals.npz"
    np.savez_compressed(batch_path, **dose_batch_arrays)
    batch_hash = RM.sha256(batch_path)
    for summary in record["shutdown_dose"].values():
        summary["batch_values_file"] = batch_path.name
        summary["batch_values_sha256"] = batch_hash
    record["photon"] = {label: {"time_index": idx, "sum_pSv_cm3_per_s": record["shutdown_dose"][label]["mean"],
        "unit": "pSv.cm3/s per voxel (track length x dose coefficient x time-corrected source rate)"}
        for idx, label in sc.COOLING_LABELS.items()}
    record["dose_by_parent_nuclide_pSv_cm_per_s"] = {k: sc.top_contributors(v, 10) for k, v in per_nuclide_tot.items()}
    record["peak_rss_mb"] = max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0
    (out / "d1s_record.json").write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
