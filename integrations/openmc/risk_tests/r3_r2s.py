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
import shutil
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_scenario as sc  # noqa: E402
import r3_activation_coverage as AC  # noqa: E402
import r3_dose_statistics as DS  # noqa: E402
import r3_shutdown_dose_export as DX  # noqa: E402
import rm_m_checks as checks  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import rm_m_spec as spec  # noqa: E402
import r3_bounded_microxs as BM  # noqa: E402

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


def tree_file_bytes(root: Path) -> int:
    return sum(path.stat().st_size for path in Path(root).rglob("*") if path.is_file())


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


def inventory_emission_chunked(openmc, chain, activation_plan_path: Path,
                               activation_record_path: Path, time_index: int):
    """Read the all-domain inventory/emission sums extracted before chunk retirement."""
    plan = json.loads(Path(activation_plan_path).read_text(encoding="utf-8"))
    record = json.loads(Path(activation_record_path).read_text(encoding="utf-8"))
    if record.get("plan_sha256") != plan.get("plan_sha256") or record.get("status") != "PASS":
        raise ValueError("inventory cross-check requires a completed activation record bound to its plan")
    BM.verify_activation_microxs_receipts(activation_plan_path, activation_record_path)
    summaries = record.get("time_summaries", [])
    if not 0 <= int(time_index) < len(summaries):
        raise ValueError("activation inventory summary lacks the requested cooling state")
    summary = summaries[int(time_index)]
    return (float(summary["inventory_photons_per_s"]),
            {str(name): float(value) for name, value in summary["power_mev_per_s_by_nuclide"].items()},
            float(summary["unclipped_photons_per_s"]))


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
    ap.add_argument("--activation-backend", choices=("bounded", "direct"), default="bounded",
                    help="bounded streams group-mean XS and activates domain chunks; direct retains R2SManager dense arrays")
    ap.add_argument("--microxs-output-budget-gib", type=float, default=8.0,
                    help="explicit retained intermediate budget for bounded collapse")
    ap.add_argument("--disk-reserve-gib", type=float, default=8.0,
                    help="free disk space that must remain after bounded artifacts are written")
    ap.add_argument("--microxs-memory-budget-mib", type=float, default=512.0,
                    help="peak working-set budget for bounded pointwise XS collapse")
    ap.add_argument("--activation-chunk-domains", type=int, default=8,
                    help="number of mesh-material activation domains solved per depletion chunk")
    ap.add_argument("--activation-time-budget", type=float, default=86400.0,
                    help="maximum elapsed seconds checked between bounded depletion chunks")
    ap.add_argument("--capi-domain-index", type=int,
                    help="domain row used by the optional post-run bounded CAPI comparison (default: first positive-flux row)")
    ap.add_argument("--skip-capi-check", action="store_true",
                    help="skip the bounded numerical CAPI sample check (not recommended for a reference run)")
    ap.add_argument("--seed", type=int, default=20261201)
    ap.add_argument("--photon-indices", type=int, nargs="+", default=list(sc.PHOTON_TIME_INDICES),
                    help="time indices for the photon step (S0b D5c: 2 = 1 d only)")
    args = ap.parse_args()
    args.model_dir = args.model_dir.resolve()
    args.out_dir = args.out_dir.resolve()
    import openmc
    import openmc.deplete
    import numpy as np

    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    openmc.config["chain_file"] = str(CHAIN)
    openmc.config["cross_sections"] = str(RM.FARIS_XS)  # sub-models built inside the manager do not inherit materials.cross_sections
    out = args.out_dir
    if out.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 output directory: {out}")
    out.mkdir(parents=True)
    os.chdir(out)
    mesh = scoring_mesh(openmc)
    neutron, photon, ids, dag = build_models(openmc, args.model_dir, mesh, args.neutron_particles, args.neutron_batches,
                                             args.photon_particles, args.photon_max_batches, (args.seed, args.seed + 1))
    nuclides = model_nuclides(openmc, neutron)
    chain = openmc.deplete.Chain.from_xml(str(CHAIN))
    coverage = AC.assess_coverage(CHAIN, RM.FARIS_XS, nuclides, openmc.data.REACTION_MT)
    record = {"kind": "r3-r2s", "openmc_version": openmc.__version__, "threads": args.threads, "steps": {},
              "activation_backend": args.activation_backend,
              "bounded_resources": {"collapse_activation_output_budget_bytes": int(args.microxs_output_budget_gib * BM.GIB),
                                     "disk_reserve_bytes": int(args.disk_reserve_gib * BM.GIB),
                                     "working_memory_budget_bytes": int(args.microxs_memory_budget_mib * 1024 ** 2),
                                     "activation_domain_chunk": args.activation_chunk_domains,
                                     "activation_elapsed_time_budget_s": args.activation_time_budget},
              "source_problem_protocol": sc.FULL_EMISSION_PROTOCOL,
              "chain": str(CHAIN), "chain_sha256": RM.sha256(CHAIN), "library": RM.library_identity(),
              "activation_chain_coverage": coverage,
              "activation_data_status": coverage["status"],
              "activation_microxs_requested_nuclides": coverage["required_microxs_nuclides"],
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
    jpath = out / "r2s_record.json"

    def save():
        jpath.write_text(json.dumps(record, indent=1, sort_keys=True, default=str) + "\n", encoding="utf-8")

    if coverage["status"] != "READY_FOR_R2S_TRANSPORT":
        record["steps"]["activation_data_preflight"] = {
            "ok": False, "seconds": 0.0, "peak_rss_mb": None,
            "error": "reachable activation nuclides or chain-listed reaction channels lack indexed OpenMC data; no R2S neutron transport was started",
        }
        save()
        return 2

    activation_mesh, activation_domain_data = activation_domain(openmc, args.model_dir, dag, args.activation_cell_cm)
    domain_bytes = DX._json_bytes(activation_domain_data)
    record["activation_domain"] = {**DX._write_bytes(out / "activation_domain.json", domain_bytes),
                                    "path": "activation_domain.json"}
    energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
    record["shutdown_dose_basis"] = DX.write_basis_artifacts(
        out, neutron, neutron.settings.source[0], mesh,
        {"timesteps": sc.TIMESTEPS, "source_rates_neutrons_s": sc.SOURCE_RATES,
         "chain_path": str(CHAIN), "chain_sha256": RM.sha256(CHAIN)}, energies, coefficients,
        sc.rm_m_geometry_dependencies(args.model_dir), RM.library_identity())
    save()
    mgr = openmc.deplete.R2SManager(neutron, activation_mesh, photon_model=photon) \
        if args.activation_backend == "direct" else None
    bounded_paths = {}

    def step1():
        neutron_dir = out / "neutron_transport"
        neutron_dir.mkdir()
        if args.activation_backend == "direct":
            estimate_volumes = activation_mesh.material_volumes(
                neutron, n_samples=args.mat_vol_samples, output=False)
            direct_domains = int(np.count_nonzero(np.asarray(estimate_volumes._materials) > -1))
            direct_estimate = BM.collapse_resource_estimate(
                direct_domains, len(coverage["required_microxs_nuclides"]),
                len(chain.reactions), 42, args.activation_chunk_domains)
            direct_output = direct_estimate["direct_one_tensor_bytes"]
            if direct_estimate["direct_peak_bytes_estimate"] > int(args.microxs_memory_budget_mib * 1024 ** 2):
                raise ValueError("direct R2S dense MicroXS/flux path exceeds its explicit memory preflight; select bounded backend in a fresh run")
            if direct_output > int(args.microxs_output_budget_gib * BM.GIB):
                raise ValueError("direct R2S MicroXS output exceeds its explicit output budget")
            if shutil.disk_usage(neutron_dir).free < direct_output + int(args.disk_reserve_gib * BM.GIB):
                raise ValueError("direct R2S MicroXS output plus disk reserve exceeds available space")
            record["direct_resource_preflight"] = {
                "status": "PASS_WITHIN_DECLARED_BUDGETS", "sampled_domain_count": direct_domains,
                "resources": direct_estimate, "output_budget_bytes": int(args.microxs_output_budget_gib * BM.GIB),
                "memory_budget_bytes": int(args.microxs_memory_budget_mib * 1024 ** 2),
                "disk_reserve_bytes": int(args.disk_reserve_gib * BM.GIB)}
            save()
            with DS.NeutronManagerRunEvidence(openmc, neutron_dir / "model.xml") as evidence:
                mgr.step1_neutron_transport(neutron_dir, mat_vol_kwargs={"n_samples": args.mat_vol_samples},
                                            micro_kwargs={"nuclides": coverage["required_microxs_nuclides"],
                                                          "energies": sc.ACTIVATION_GROUPS,
                                                          "chain_file": str(CHAIN), "run_kwargs": {"output": False}})
            record["neutron_transport_integrity"] = evidence.result()
            if record["neutron_transport_integrity"].get("status") != "PASS":
                raise RuntimeError("R2S neutron transport integrity is " +
                                   str(record["neutron_transport_integrity"].get("status")) + ": " +
                                   str(record["neutron_transport_integrity"].get("reason", "loss gate failed")))
            record["activation_regions"] = len(mgr.results["fluxes"])
            microxs_path = neutron_dir / "micros.h5"
            microxs_inspection = AC.inspect_microxs(microxs_path, coverage["required_microxs_nuclides"])
            actual_domain_nuclides = [sorted(set(micro.nuclides)) for micro in mgr.results["micros"]]
            if not actual_domain_nuclides or any(names != coverage["required_microxs_nuclides"]
                                                 for names in actual_domain_nuclides):
                raise RuntimeError("actual in-memory MicroXS nuclides differ from the complete reachable-chain request")
            record["activation_microxs"] = {
                "kind": "OpenMC per-domain MicroXS HDF5",
                "path": microxs_path.relative_to(out).as_posix(), "sha256": RM.sha256(microxs_path),
                **microxs_inspection,
            }
            return

        volume_path = neutron_dir / "mesh_material_volumes.npz"
        mmv = activation_mesh.material_volumes(neutron, n_samples=args.mat_vol_samples, output=False)
        mmv.save(volume_path)
        volume_layout = BM.mesh_material_volumes_preflight(volume_path)
        with np.load(volume_path, allow_pickle=False) as saved:
            material_array = np.asarray(saved["materials"])
            volume_array = np.asarray(saved["volumes"], dtype=np.float64)
            actual_domains = int(np.count_nonzero(material_array > -1))
        if actual_domains < 1:
            raise RuntimeError("OpenMC material-volume sampling found no material-bearing activation domains")
        memory_budget = int(args.microxs_memory_budget_mib * 1024 ** 2)
        output_budget = int(args.microxs_output_budget_gib * BM.GIB)
        disk_reserve = int(args.disk_reserve_gib * BM.GIB)
        estimate = BM.collapse_resource_estimate(actual_domains,
                                                  len(coverage["required_microxs_nuclides"]),
                                                  len(chain.reactions), 42, args.activation_chunk_domains)
        activation_estimate = BM.activation_resource_estimate(
            actual_domains, len(chain.nuclides), len(coverage["required_microxs_nuclides"]),
            len(chain.reactions), len(sc.TIMESTEPS), args.activation_chunk_domains,
            group_count=42, mesh_array_bytes=volume_layout["array_bytes"],
            mesh_element_count=volume_layout["shape"][0],
            material_slot_count=volume_layout["shape"][1])
        _chain_reactions, _reachable, metadata_required, collapse_reactions = BM._chain_spec(
            CHAIN, nuclides, openmc.data.REACTION_MT)
        if metadata_required != coverage["required_microxs_nuclides"]:
            raise ValueError("bounded pointwise-array requirements differ from the full-chain coverage guard")
        collapse_data_memory = BM.collapse_metadata_memory_preflight(
            RM.FARIS_XS, metadata_required, collapse_reactions, openmc.data.REACTION_MT,
            actual_domains, args.activation_chunk_domains)
        if collapse_data_memory["working_bytes_estimate"] > memory_budget:
            raise ValueError(f"bounded pointwise XS metadata working estimate {collapse_data_memory['working_bytes_estimate']} exceeds the declared {memory_budget}-byte cap")
        flux_pair_estimate = 2 * actual_domains * 42 * 8
        chunk_count_estimate = (actual_domains + args.activation_chunk_domains - 1) // args.activation_chunk_domains
        chunk_metadata_estimate = chunk_count_estimate * 8192 + 2 * 1024 ** 2
        other_artifacts_estimate = volume_path.stat().st_size + flux_pair_estimate + chunk_metadata_estimate
        non_activation_output_estimate = (estimate["bounded_output_bytes_upper_bound"] +
                                          other_artifacts_estimate)
        remaining_output_budget = output_budget - non_activation_output_estimate
        if remaining_output_budget < 2:
            raise ValueError("compact collapse and neutron flux artifacts consume the declared output budget")
        activation_source_archive_budget = remaining_output_budget // 2
        photon_model_xml_budget = remaining_output_budget - activation_source_archive_budget
        if estimate["bounded_working_bytes_estimate"] > memory_budget:
            raise ValueError(f"bounded collapse working estimate {estimate['bounded_working_bytes_estimate']} exceeds the declared {memory_budget}-byte cap")
        if activation_estimate["working_bytes_estimate"] > memory_budget:
            raise ValueError(f"bounded activation working estimate {activation_estimate['working_bytes_estimate']} exceeds the declared {memory_budget}-byte cap")
        if shutil.disk_usage(neutron_dir).free < activation_source_archive_budget + disk_reserve:
            raise ValueError("bounded source archive allowance plus declared disk reserve exceeds available space")
        record["bounded_resource_preflight"] = {
            "status": "PASS_WITHIN_DECLARED_BUDGETS",
            "domain_count": actual_domains,
            "output_budget_bytes": output_budget, "disk_reserve_bytes": disk_reserve,
            "memory_budget_bytes": memory_budget,
            "non_activation_output_upper_bound_bytes": non_activation_output_estimate,
            "source_archive_compressed_hard_cap_bytes": activation_source_archive_budget,
            "source_archive_decompressed_hard_cap_bytes": activation_source_archive_budget,
            "photon_model_xml_hard_cap_bytes": photon_model_xml_budget,
            "flux_pair_estimate_bytes": flux_pair_estimate,
            "mesh_material_volumes_bytes": volume_path.stat().st_size,
            "estimated_chunk_count": chunk_count_estimate,
            "chunk_metadata_allowance_bytes": chunk_metadata_estimate,
            "available_disk_bytes": shutil.disk_usage(neutron_dir).free,
            "collapse": estimate, "collapse_pointwise_memory": collapse_data_memory,
            "activation": activation_estimate}
        save()
        elements, slots = np.where(material_array > -1)
        domain_materials = material_array[elements, slots].astype(np.int64, copy=False)
        domain_volumes = volume_array[elements, slots]
        volume_receipt = {"path": volume_path.relative_to(out).as_posix(), "sha256": RM.sha256(volume_path),
                          "size_bytes": volume_path.stat().st_size, "domain_count": actual_domains,
                          "layout": volume_layout,
                          "domain_map_sha256": BM.domain_map_sha256(elements, domain_materials, domain_volumes)}
        tally = openmc.Tally(name="R3 bounded MicroXS flux")
        tally.filters = [openmc.MeshMaterialFilter.from_volumes(activation_mesh, mmv),
                        openmc.EnergyFilter.from_group_structure(sc.ACTIVATION_GROUPS)]
        tally.scores = ["flux"]
        neutron.tallies = openmc.Tallies([tally])
        neutron.export_to_model_xml(str(neutron_dir))
        model_xml = neutron_dir / "model.xml"
        protocol = {
            "schema": "faris-r3-bounded-campaign-protocol/v1", "status": "READY_FOR_NEUTRON_TRANSPORT",
            "activation_mesh": record["activation_domain"], "sampling_settings": record["settings"],
            "coverage": {"status": coverage["status"], "required_nuclide_count": len(coverage["required_microxs_nuclides"]),
                         "required_microxs_nuclides": coverage["required_microxs_nuclides"],
                         "reaction_count": len(chain.reactions)},
            "inputs": {"model_xml": {"path": str(model_xml), "sha256": RM.sha256(model_xml)},
                       "materials_xml": {"path": str(neutron_dir / "materials.xml"), "sha256": RM.sha256(neutron_dir / "materials.xml")},
                       "mesh_material_volumes": volume_receipt,
                       "chain": {"path": str(CHAIN), "sha256": RM.sha256(CHAIN)},
                       "cross_sections_xml": {"path": str(RM.FARIS_XS), "sha256": RM.sha256(RM.FARIS_XS)}},
            "implementation": {str(path.name): RM.sha256(path) for path in
                                (Path(__file__), Path(BM.__file__), Path(AC.__file__), Path(DS.__file__))},
            "resource_preflight": record["bounded_resource_preflight"],
            "fixed_protocol": {"energy_groups": "VITAMIN-J-42", "energy_edges_eV": list(BM.VITAMIN_J_42_EDGES_E_V),
                               "flux_tally": "flux only; no reaction-rate tallies or CE reaction tensors",
                               "domain_order": "row-major MeshMaterialFilter.from_volumes(MeshMaterialVolumes)",
                               "collapse": BM.ALGORITHM, "activation": "OpenMC 0.15.3 independent predictor chunks",
                               "physics_settings": "existing source, irradiation schedule, dose response mesh, and photon source slots are unchanged"}}
        protocol["protocol_sha256"] = BM._canonical_sha256(protocol)
        protocol_path = neutron_dir / "bounded-protocol-plan.json"
        BM._write_json_new(protocol_path, protocol)
        record["bounded_protocol_plan"] = {"path": protocol_path.relative_to(out).as_posix(),
                                            "sha256": RM.sha256(protocol_path),
                                            "protocol_sha256": protocol["protocol_sha256"]}
        save()
        previous = 0.0

        def consume_flux_batch(cumulative, _batch):
            nonlocal previous
            previous, _increment = DS.scalar_batch_total(float(np.sum(cumulative)), previous)

        run, final_sums, final_std = DS.run_live_tally(
            openmc, model_xml, tally.id, args.threads, args.neutron_batches, 86400.0,
            consume_flux_batch, particles_per_batch=args.neutron_particles)
        if run.get("stop_reason") != "batch_limit" or run.get("solver_status") != 1 or \
                run.get("batches") != args.neutron_batches:
            raise RuntimeError(f"bounded neutron flux run did not reach exact planned final batch: {run}")
        if run.get("integrity", {}).get("lost_particle_assessment", {}).get("status") != "PASS":
            raise RuntimeError("bounded neutron flux run has unavailable or unacceptable lost-particle evidence")
        if int(np.asarray(final_sums).size) != actual_domains * 42 or int(np.asarray(final_std).size) != actual_domains * 42:
            raise RuntimeError("bounded neutron flux tally shape differs from material-volume domain x 42 group rows")
        flux_path = neutron_dir / "fluxes.npy"
        flux_mean = np.asarray(final_sums, dtype=np.float64).reshape(actual_domains, 42) / run["batches"]
        np.save(flux_path, flux_mean, allow_pickle=False)
        flux_std_path = neutron_dir / "fluxes_std_dev.npy"
        np.save(flux_std_path, np.asarray(final_std, dtype=np.float64).reshape(actual_domains, 42), allow_pickle=False)
        record["neutron_transport_integrity"] = {"status": "PASS", **run["integrity"],
            "stop_reason": run["stop_reason"], "solver_status": run["solver_status"],
            "batches": run["batches"], "histories": run["histories"], "elapsed_s": run["elapsed_s"],
            "model_xml_sha256": RM.sha256(model_xml), "flux_mean_sha256": RM.sha256(flux_path),
            "flux_std_dev_sha256": RM.sha256(flux_std_path), "mesh_material_volumes": volume_receipt,
            "mean_estimator": "OpenMC cumulative flux tally sums divided by actual batch realizations; one row per exact MeshMaterialFilter.from_volumes domain, groups ascending on frozen VITAMIN-J-42 edges"}
        record["activation_regions"] = actual_domains
        plan_path = neutron_dir / "bounded-collapse-plan.json"
        collapse_dir = neutron_dir / "bounded-collapse"
        collapse_plan = BM.prepare_plan(plan_path, fluxes_path=flux_path, volumes_path=volume_path,
            chain_path=CHAIN, cross_sections_xml=RM.FARIS_XS, materials_xml=neutron_dir / "materials.xml",
            energy_edges_eV=BM.VITAMIN_J_42_EDGES_E_V, output_run_dir=collapse_dir,
            domain_chunk=args.activation_chunk_domains, output_budget_bytes=output_budget,
            disk_reserve_bytes=disk_reserve, memory_limit_bytes=memory_budget)
        collapse_record = BM.run_collapse(plan_path, collapse_dir)
        bounded_paths.update({"volume": volume_path, "flux": flux_path, "flux_std": flux_std_path,
                              "plan": plan_path, "collapse_dir": collapse_dir,
                              "group_mean": Path(collapse_record["output"]["path"]),
                              "collapse_record": collapse_dir / "collapse_record.json"})
        if not args.skip_capi_check:
            capi_domain = args.capi_domain_index
            if capi_domain is None:
                positive = np.flatnonzero(np.sum(flux_mean, axis=1) > 0.0)
                if not len(positive):
                    raise ValueError("no positive-flux activation domain is available for the CAPI sample")
                capi_domain = int(positive[0])
            if capi_domain < 0 or capi_domain >= actual_domains or float(flux_mean[capi_domain].sum()) <= 0.0:
                raise ValueError("selected CAPI sample domain has no positive flux spectrum")
            checks_capi = [{"nuclide": "Fe56", "reaction": "(n,gamma)", "domain_index": capi_domain},
                           {"nuclide": "Fe56", "reaction": "(n,2n)", "domain_index": capi_domain},
                           {"nuclide": "Ni58", "reaction": "(n,gamma)", "domain_index": capi_domain}]
            capi = BM.compare_capi_reference(RM.FARIS_XS, flux_path, BM.VITAMIN_J_42_EDGES_E_V, checks_capi)
            BM._write_json_new(neutron_dir / "bounded-capi-check.json", capi)
            record["bounded_capi_check"] = {"path": "neutron_transport/bounded-capi-check.json",
                                            "sha256": RM.sha256(neutron_dir / "bounded-capi-check.json"), **capi}
        group_mean_rel = bounded_paths["group_mean"].relative_to(out).as_posix()
        record["activation_microxs"] = {"kind": "bounded group-mean XS plus per-chunk reconstructed MicroXS",
            "path": group_mean_rel, "sha256": collapse_record["output"]["sha256"],
            "collapse_plan_path": plan_path.relative_to(out).as_posix(),
            "collapse_plan_sha256": collapse_plan["plan_sha256"],
            "collapse_record_path": (collapse_dir / "collapse_record.json").relative_to(out).as_posix(),
            "collapse_record_sha256": RM.sha256(collapse_dir / "collapse_record.json"),
            "required_nuclides": coverage["required_microxs_nuclides"],
            "domain_count": actual_domains,
            "reaction_count": len(chain.reactions),
            "group_mean_xs_path": group_mean_rel,
            "group_mean_xs_sha256": collapse_record["output"]["sha256"],
            "coverage_revalidation": "full exact chain/material-root/reaction coverage is independently recomputed by the exporter; each activation chunk also records hash of reconstructed domain MicroXS bytes"}

    if not timed(record, "neutron_transport", step1):
        save()
        return 1
    save()

    def step2():
        if args.activation_backend == "direct":
            mgr.step2_activation(sc.TIMESTEPS, sc.SOURCE_RATES, output_dir=out / "activation", operator_kwargs={"chain_file": str(CHAIN)})
            return
        activation_plan_path = out / "activation" / "bounded-activation-plan.json"
        activation_dir = out / "activation" / "bounded-chunks"
        activation_plan = BM.prepare_activation_plan(activation_plan_path,
            collapse_plan_path=bounded_paths["plan"], collapse_record_path=bounded_paths["collapse_record"],
            microxs_path=bounded_paths["group_mean"], fluxes_path=bounded_paths["flux"],
            volumes_path=bounded_paths["volume"], materials_xml=out / "neutron_transport" / "materials.xml",
            chain_path=CHAIN, output_run_dir=activation_dir, timesteps=sc.TIMESTEPS,
            source_rates=sc.SOURCE_RATES, domain_chunk=args.activation_chunk_domains,
            time_budget_s=args.activation_time_budget,
            output_budget_bytes=int(record["bounded_resource_preflight"]["source_archive_compressed_hard_cap_bytes"]),
            disk_reserve_bytes=int(args.disk_reserve_gib * BM.GIB),
            memory_limit_bytes=int(args.microxs_memory_budget_mib * 1024 ** 2))
        activation_record = BM.run_activation_chunks(activation_plan_path)
        bounded_paths.update({"activation_plan": activation_plan_path,
                              "activation_record": activation_dir / "activation_record.json"})
        record["bounded_activation"] = {"plan_path": activation_plan_path.relative_to(out).as_posix(),
            "plan_sha256": activation_plan["plan_sha256"],
            "record_path": (activation_dir / "activation_record.json").relative_to(out).as_posix(),
            "record_sha256": RM.sha256(activation_dir / "activation_record.json"), **activation_record}

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
            source_summary_ref = None
            if args.activation_backend == "direct":
                sources = mgr.get_decay_photon_source_mesh(idx)
                total, power = inventory_emission(openmc, chain, mgr, idx)
                unclipped = 0.0
                for mat in mgr.results["activation_materials"]:
                    e = mgr.results["depletion_results"][idx].get_material(str(mat.id)).get_decay_photon_energy(clip_tolerance=0.0)
                    unclipped += e.integral() if e is not None else 0.0
            else:
                sources, source_summary = BM.build_chunked_decay_photon_sources(
                    bounded_paths["activation_plan"], bounded_paths["activation_record"], activation_mesh, idx,
                    openmc_module=openmc)
                time_dir = out / "photon_transport" / f"time_{idx}"
                time_dir.mkdir(parents=True)
                source_summary_path = time_dir / "chunked_source_summary.json"
                BM._write_json_new(source_summary_path, source_summary)
                source_summary_ref = {"path": source_summary_path.relative_to(out).as_posix(),
                                      "sha256": RM.sha256(source_summary_path)}
                total, power, unclipped = inventory_emission_chunked(
                    openmc, chain, bounded_paths["activation_plan"], bounded_paths["activation_record"], idx)
            method_source = DX.write_r2s_source_artifact(out, idx, sources) \
                if args.activation_backend == "direct" else None
            strength = source_strength(sources)
            conservation[label] = checks_conservation = sc.conservation(strength, total)
            checks_conservation["unclipped_source_check"] = sc.conservation(unclipped, total)
            checks_conservation["note"] = ("source = the manager's own MeshSource strengths (Material.get_decay_photon_energy, default "
                                           "clip_tolerance 1e-6); unclipped_source_check uses clip_tolerance 0")
            contributors[label] = sc.top_contributors(power, 10)
            if source_summary_ref:
                record.setdefault("bounded_photon_source_summaries", {})[label] = source_summary_ref
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
            if args.activation_backend == "bounded":
                model_xml_total = sum(path.stat().st_size for path in
                                      (out / "photon_transport").rglob("model.xml") if path.is_file())
                model_xml_cap = int(record["bounded_resource_preflight"]["photon_model_xml_hard_cap_bytes"])
                if model_xml_total > model_xml_cap:
                    raise ValueError(f"serialized photon model XML files use {model_xml_total} bytes, above their {model_xml_cap}-byte cap")
                output_cap = int(args.microxs_output_budget_gib * BM.GIB)
                if tree_file_bytes(out) > output_cap:
                    raise ValueError("R3 bounded artifacts exceed the declared total output budget before photon transport")
            if args.activation_backend == "bounded":
                method_source = DX.write_r2s_bounded_source_artifact(
                    out, idx, source_summary, source_summary_ref, run_dir / "model.xml")
                if tree_file_bytes(out) > int(args.microxs_output_budget_gib * BM.GIB):
                    raise ValueError("R3 bounded source manifests exceed the declared total output budget before photon transport")
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
            if args.activation_backend == "bounded" and tree_file_bytes(out) > int(args.microxs_output_budget_gib * BM.GIB):
                raise ValueError("R3 bounded output artifacts exceed the declared total output budget after photon scoring")
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
