#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""R3, mesh-based R2S (openmc.deplete.R2SManager) on RM-M, OpenMC 0.15.3.

    r3_r2s.py --model-dir D --out-dir R [--neutron-particles N --neutron-batches B --photon-particles N --photon-budget S]

Steps run one at a time with their own timings: neutron transport (flux and microscopic cross sections per mesh
element and material), activation (1 full-power year at 525 MW, then decay), and photon transport at cooling times of
1 d, 7 d and 30 d. The chain is ~/nuclear-data/p32-work/chain/depletion/chain.xml.

Workarounds OpenMC 0.15.3 needs (recorded in the result): output_dir is a pathlib.Path (the manager divides it with `/`),
micro_kwargs carries an explicit nuclide list, and the photon model's source_rejection_fraction is 1e-4 (the
material-constrained mesh sources reject most samples).

The prospective full-emission reference activates the complete H5M vertex bounding box on a separate coarse mesh;
the R3 response mesh remains fixed for dose scoring. The starting activation cell size is not a source-resolution
adequacy claim and requires discretization-convergence checks. The dose tally is volume-integrated track length times the ICRP-116 AP photon effective dose coefficient. OpenMC
multiplies fixed-source tallies by the total source strength, so with the photon source in photons/s the tally is
pSv.cm3 per second. The photon Monte Carlo standard error is computed from per-batch aggregate totals; it is
conditional on the generated decay-photon source and does not propagate uncertainty from neutron transport or
activation.
"""
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402
import r3_dose_statistics as DS  # noqa: E402
import r3_shutdown_dose_export as DX  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402

CHAIN = Path.home() / "nuclear-data/p32-work/chain/depletion/chain.xml"


def scoring_mesh(openmc):
    mesh = openmc.RegularMesh(name="r3-scored-region")
    mesh.lower_left = sc.MESH_LOWER_CM
    mesh.upper_right = sc.MESH_UPPER_CM
    mesh.dimension = sc.MESH_DIMENSION
    return mesh


def dose_tally(openmc, mesh, name="photon_dose"):
    energies, coeffs = openmc.data.dose_coefficients("photon", "AP")
    tally = openmc.Tally(name=name)
    tally.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter(["photon"]),
                     openmc.EnergyFunctionFilter(energies, coeffs, interpolation="log-log")]
    tally.scores = ["flux"]
    return tally


def model_nuclides(openmc, model) -> list[str]:
    return sorted({n for m in model.materials for n in m.get_nuclides()})


def build_models(openmc, model_dir: Path, mesh, n_particles, n_batches, p_particles, p_batches, seeds):
    roles = RM.load_roles(model_dir)
    ids = RM.role_ids(roles)
    neutron, dag = RM.build_model(openmc, model_dir / "rm_m.h5m", roles)
    source, _ = RM.tokamak_mesh_source(openmc, ids["plasma"], rate=1.0)
    s = neutron.settings
    s.run_mode = "fixed source"
    s.batches, s.particles, s.seed = n_batches, n_particles, seeds[0]
    s.source = [source]
    s.photon_transport = False
    s.temperature = {"default": RM.DATA_TEMPERATURE_K, "method": "nearest", "tolerance": 1.0}
    s.output = {"summary": False, "tallies": False}
    photon = openmc.Model(geometry=neutron.geometry, materials=neutron.materials)
    ps = openmc.Settings()
    ps.run_mode = "fixed source"
    ps.batches, ps.particles, ps.seed = p_batches, p_particles, seeds[1]
    ps.photon_transport = True
    ps.source_rejection_fraction = 1.0e-4
    ps.temperature = dict(s.temperature)
    ps.output = {"summary": False, "tallies": False}
    photon.settings = ps
    photon.tallies = openmc.Tallies([dose_tally(openmc, mesh)])
    return neutron, photon, ids, dag


def activation_domain(openmc, model_dir: Path, dag, cell_cm: float) -> tuple[object, dict]:
    """Build a coarse activation mesh from verified H5M vertex bounds and its pinned model card."""
    import hashlib

    h5m_path = (model_dir / "rm_m.h5m").resolve()
    card_path = (model_dir / "rm_m_model_card.json").resolve()
    card_bytes = card_path.read_bytes()
    card = json.loads(card_bytes)
    if Path(dag.filename).resolve() != h5m_path:
        raise ValueError("DAGMC universe is not bound to the model-dir H5M whose vertices are being recorded")
    h5m_hash = RM.sha256(h5m_path)
    card_h5m_hash = card.get("outputs", {}).get("h5m_sha256")
    if card.get("schema") != "faris-rm-m-model-card/v1" or card_h5m_hash != h5m_hash:
        raise ValueError("activation bounds require the model card pinned to this exact RM-M H5M")
    bounding_radius = float(card.get("bounding_sphere_radius", {}).get("value", 0.0))
    if not math.isfinite(bounding_radius) or bounding_radius <= 0.0:
        raise ValueError("RM-M model card has no valid authored bounding-sphere radius")
    vertices = DX._h5m_vertex_bounds(h5m_path)
    actual_box = dag.bounding_box
    lower = tuple(map(float, actual_box.lower_left))
    upper = tuple(map(float, actual_box.upper_right))
    if list(lower) != vertices["lower_left_cm"] or list(upper) != vertices["upper_right_cm"]:
        raise ValueError("OpenMC DAGMC bounds differ from the actual H5M node-coordinate bounds")
    if vertices["max_vertex_radius_cm"] > bounding_radius:
        raise ValueError("H5M vertex bounds exceed the model card's authored containment sphere")
    grid = sc.activation_grid_spec(lower, upper, cell_cm)
    mesh = openmc.RegularMesh(name="r3-full-geometry-activation")
    mesh.lower_left = tuple(grid["lower_left_cm"])
    mesh.upper_right = tuple(grid["upper_right_cm"])
    mesh.dimension = tuple(grid["dimension"])
    domain = {
        "schema": "faris-r3-activation-domain/v1",
        "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
        "scope": "full H5M vertex bounding box; material-bearing mesh domains are identified by OpenMC material-volume sampling",
        "photon_source_scope": "decay photons from all material-bearing domains identified in the full activation mesh; coarse-cell spatial approximation requires convergence",
        "emission_transport_scope": "generated decay photons and their descendants are transported; all may score in the shared response mesh",
        "source_rate_normalization": "R2SManager MeshSource strengths in photons/s; OpenMC fixed-source tally applies source-total normalization",
        "geometry": {"h5m_path": str(h5m_path), "h5m_sha256": h5m_hash,
                     "model_card_path": str(card_path), "model_card_sha256": hashlib.sha256(card_bytes).hexdigest(),
                     "model_card_bounding_sphere_radius_cm": bounding_radius,
                     "h5m_vertex_lower_left_cm": list(lower), "h5m_vertex_upper_right_cm": list(upper),
                     "h5m_max_vertex_radius_cm": vertices["max_vertex_radius_cm"]},
        "activation_mesh": {**grid, "mesh_xml": DX._mesh_definition(mesh)},
        "response_mesh": {"lower_left_cm": list(sc.MESH_LOWER_CM), "upper_right_cm": list(sc.MESH_UPPER_CM),
                          "dimension": list(sc.MESH_DIMENSION), "voxel_cm": sc.MESH_VOXEL_CM},
        "discretization_limit": "100 cm is a starting mesh only; no activation-source resolution adequacy is claimed",
    }
    return mesh, domain


def timed(record, name, fn):
    t0 = time.time()
    entry = {"ok": False, "seconds": None, "peak_rss_mb": None, "error": None}
    record["steps"][name] = entry
    try:
        fn()
        entry["ok"] = True
    except Exception:
        import traceback
        entry["error"] = traceback.format_exc(limit=8)
    finally:
        entry["seconds"] = time.time() - t0
        entry["peak_rss_mb"] = max(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                                   resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) / 1024.0
    return entry["ok"]


def inventory_emission(openmc, chain, mgr, time_index):
    """Photon emission of the activated inventory, from the chain's decay data and the nuclide atoms.

    This is a different path from the manager's source (Material.get_decay_photon_energy with clipping): atoms times the
    per-atom photon emission rate, summed over every activation material, and the same sum by nuclide as photon
    power in MeV/s for the nuclide ranking.
    """
    import numpy as np

    results = mgr.results["depletion_results"]
    per_decay = {}
    for nuc in chain.nuclides:
        src = nuc.sources.get("photon") if nuc.sources else None
        if src is None:
            continue
        x, p = np.asarray(src.x, dtype=float), np.asarray(src.p, dtype=float)
        # the chain stores the photon spectrum per atom per second (the decay constant is already in it)
        per_decay[nuc.name] = (float(p.sum()), float((x * p).sum()) / 1.0e6)
    total = 0.0
    power = {}
    for mat in mgr.results["activation_materials"]:
        for name, atoms in results[time_index].get_material(str(mat.id)).get_nuclide_atoms().items():
            if name in per_decay and atoms > 0.0:
                n_ph, mev = per_decay[name]
                total += atoms * n_ph
                power[name] = power.get(name, 0.0) + atoms * mev
    return total, power


def source_strength(sources) -> float:
    total = 0.0
    for ms in sources:
        for src in ms.sources.ravel():
            total += float(src.strength)
    return total


def sampling_settings(args) -> dict:
    """The producer's recorded sampling and irradiation contract, also used by boundary fixtures."""
    return {
        "neutron_particles_per_batch": args.neutron_particles,
        "neutron_batches": args.neutron_batches,
        "photon_particles_per_batch": args.photon_particles,
        "photon_max_batches": args.photon_max_batches,
        "photon_budget_s": args.photon_budget,
        "seeds": [args.seed, args.seed + 1],
        "activation_groups": sc.ACTIVATION_GROUPS,
        "activation_cell_cm": args.activation_cell_cm,
        "mat_vol_samples": args.mat_vol_samples,
        "timesteps": sc.TIMESTEPS,
        "source_rate_n_s": sc.SOURCE_RATES[0],
        "source_rates_n_s": sc.SOURCE_RATES,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--neutron-particles", type=int, default=20000)
    ap.add_argument("--neutron-batches", type=int, default=50)
    ap.add_argument("--photon-particles", type=int, default=100000)
    ap.add_argument("--photon-max-batches", type=int, default=5000)
    ap.add_argument("--photon-budget", type=float, default=3000.0, help="wall seconds per photon-transport run")
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--mat-vol-samples", type=int, default=50000)
    ap.add_argument("--activation-cell-cm", type=float, default=sc.DEFAULT_ACTIVATION_CELL_CM,
                    help="coarse full-H5M activation mesh cell size; starting discretization, not adequacy claim")
    ap.add_argument("--seed", type=int, default=20261201)
    ap.add_argument("--photon-indices", type=int, nargs="+", default=list(sc.PHOTON_TIME_INDICES),
                    help="time indices for the photon step (S0b D5c: 2 = 1 d only)")
    args = ap.parse_args()
    args.model_dir = args.model_dir.resolve()
    args.out_dir = args.out_dir.resolve()
    import openmc
    import openmc.deplete

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)  # sub-models built inside the manager do not inherit materials.cross_sections
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)
    os.chdir(out)
    mesh = scoring_mesh(openmc)
    neutron, photon, ids, dag = build_models(openmc, args.model_dir, mesh, args.neutron_particles, args.neutron_batches,
                                             args.photon_particles, args.photon_max_batches, (args.seed, args.seed + 1))
    activation_mesh, activation_domain_data = activation_domain(openmc, args.model_dir, dag, args.activation_cell_cm)
    nuclides = model_nuclides(openmc, neutron)
    record = {"kind": "r3-r2s", "openmc_version": openmc.__version__, "threads": args.threads, "steps": {},
              "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
              "chain": str(CHAIN), "chain_sha256": RM.sha256(CHAIN), "library": RM.library_identity(),
              "mesh": {"lower_left_cm": sc.MESH_LOWER_CM, "upper_right_cm": sc.MESH_UPPER_CM, "dimension": sc.MESH_DIMENSION},
              "settings": sampling_settings(args),
              "workarounds": ["output_dir passed as pathlib.Path", "micro_kwargs nuclides = model nuclides", "photon source_rejection_fraction = 1e-4",
                            "openmc.config['cross_sections'] set to the audited library (manager sub-models do not inherit it)"],
              "nuclides": nuclides}
    record["runtime_identity"] = DS.runtime_identity(openmc)
    record["neutron_transport_integrity"] = {
        "status": "NOT_EVALUATED",
        "reason": "R2SManager neutron C API lifecycle has not yet been observed",
        "model_xml_sha256": None,
        "lost_particle_assessment": {"status": "NOT_EVALUATED", "lost_particles": None, "histories": None},
    }
    domain_bytes = DX._json_bytes(activation_domain_data)
    record["activation_domain"] = {**DX._write_bytes(out / "activation_domain.json", domain_bytes),
                                    "path": "activation_domain.json"}
    energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
    record["shutdown_dose_basis"] = DX.write_basis_artifacts(
        out, neutron, neutron.settings.source[0], mesh,
        {"timesteps": sc.TIMESTEPS, "source_rates_neutrons_s": sc.SOURCE_RATES,
         "chain_path": str(CHAIN), "chain_sha256": RM.sha256(CHAIN)}, energies, coefficients,
        sc.rm_m_geometry_dependencies(args.model_dir),
        RM.library_identity())
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    mgr = openmc.deplete.R2SManager(neutron, activation_mesh, photon_model=photon)
    jpath = out / "r2s_record.json"

    def save():
        jpath.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")

    def step1():
        neutron_dir = out / "neutron_transport"
        with DS.NeutronManagerRunEvidence(openmc, neutron_dir / "model.xml") as evidence:
            mgr.step1_neutron_transport(neutron_dir, mat_vol_kwargs={"n_samples": args.mat_vol_samples},
                                        micro_kwargs={"nuclides": nuclides, "energies": sc.ACTIVATION_GROUPS,
                                                      "chain_file": str(CHAIN), "run_kwargs": {"output": False}})
        record["neutron_transport_integrity"] = evidence.result()
        if record["neutron_transport_integrity"].get("status") != "PASS":
            raise RuntimeError("R2S neutron transport integrity is " +
                               str(record["neutron_transport_integrity"].get("status")) + ": " +
                               str(record["neutron_transport_integrity"].get("reason", "loss gate failed")))
        record["activation_regions"] = len(mgr.results["fluxes"])

    if not timed(record, "neutron_transport", step1):
        save()
        return 1
    save()

    def step2():
        mgr.step2_activation(sc.TIMESTEPS, sc.SOURCE_RATES, output_dir=out / "activation", operator_kwargs={"chain_file": str(CHAIN)})

    if not timed(record, "activation", step2):
        save()
        return 1
    save()

    conservation = {}
    contributors = {}

    def step3():
        import numpy as np

        record["photon"] = {}
        for idx in args.photon_indices:
            label = sc.COOLING_LABELS[idx]
            t0 = time.time()
            sources = mgr.get_decay_photon_source_mesh(idx)
            method_source = DX.write_r2s_source_artifact(out, idx, sources)
            strength = source_strength(sources)
            total, power = inventory_emission(openmc, chain, mgr, idx)
            conservation[label] = checks_conservation = sc.conservation(strength, total)
            unclipped = 0.0
            for mat in mgr.results["activation_materials"]:
                e = mgr.results["depletion_results"][idx].get_material(str(mat.id)).get_decay_photon_energy(clip_tolerance=0.0)
                unclipped += e.integral() if e is not None else 0.0
            checks_conservation["unclipped_source_check"] = sc.conservation(unclipped, total)
            checks_conservation["note"] = ("source = the manager's own MeshSource strengths (Material.get_decay_photon_energy, default "
                                           "clip_tolerance 1e-6); unclipped_source_check uses clip_tolerance 0")
            contributors[label] = sc.top_contributors(power, 10)
            photon.settings.source = sources
            photon.settings.batches = args.photon_max_batches
            photon.settings.particles = args.photon_particles
            photon.settings.statepoint = {}
            photon.settings.sourcepoint = {"write": False}
            run_dir = out / "photon_transport" / f"time_{idx}"
            run_dir.mkdir(parents=True, exist_ok=True)
            for old in run_dir.glob("statepoint.*.h5"):
                old.unlink()
            photon.export_to_model_xml(str(run_dir))
            tally = photon.tallies[0]
            previous = 0.0
            aggregate_batches = []

            def consume_batch(cumulative, _batch_number):
                nonlocal previous
                total = float(cumulative.sum())
                previous, value = DS.scalar_batch_total(total, previous)
                aggregate_batches.append(value)

            run, final_sums, final_std = DS.run_live_tally(
                openmc, run_dir / "model.xml", tally.id, args.threads, args.photon_max_batches,
                args.photon_budget, consume_batch, particles_per_batch=args.photon_particles)
            if run.get("stop_reason") not in ("batch_limit", "time_budget"):
                raise RuntimeError(f"R2S photon solver ended with {run.get('stop_reason')}; dose summary refused")
            loss_status = run.get("integrity", {}).get("lost_particle_assessment", {}).get("status")
            if loss_status != "PASS":
                raise RuntimeError(f"R2S photon lost-particle assessment is {loss_status}; dose summary refused")
            if run["batches"] < 2:
                raise RuntimeError(f"photon run at {label} completed fewer than two batches; aggregate standard error unavailable")
            mean = (final_sums / run["batches"]).reshape(sc.MESH_DIMENSION[::-1]).copy()
            std = final_std.reshape(sc.MESH_DIMENSION[::-1]).copy()
            summary = DS.batch_summary(aggregate_batches)
            if not np.isclose(summary["mean"], float(mean.sum()), rtol=1.0e-10, atol=1.0e-14):
                raise RuntimeError(f"R2S aggregate batch mean does not reconcile to the tally mean at {label}")
            np.savez(out / f"dose_time_{idx}.npz", mean=mean, std=std)
            batch_path = out / f"shutdown_dose_batch_totals_time_{idx}.npz"
            np.savez_compressed(batch_path, values=np.asarray(aggregate_batches, dtype=float))
            record.setdefault("shutdown_dose", {})[label] = {"time_index": idx, **summary,
                "unit": "pSv.cm3/s summed over the scored region",
                "uncertainty_scope": "conditional Monte Carlo photon batch sampling for this generated decay-photon source; neutron transport and activation-source uncertainty are not propagated",
                "method_specific_source": method_source,
                "batch_values_file": batch_path.name, "batch_values_sha256": RM.sha256(batch_path)}
            record["photon"][label] = {"time_index": idx, "seconds": time.time() - t0, "run": run, "batches": run["batches"],
                                       "histories": run["batches"] * args.photon_particles, "source_strength_photons_per_s": strength,
                                       "unit": "pSv.cm3/s per voxel (track length x dose coefficient x photons/s)",
                                       "sum_pSv_cm3_per_s": float(mean.sum()), "conservation": checks_conservation}
            save()
        record["conservation"] = conservation
        record["top_photon_power_nuclides_MeV_per_s"] = contributors

    ok = timed(record, "photon_transport", step3)
    record["accepted_conservation"] = all(c["pass"] for c in conservation.values()) if conservation else None
    save()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
