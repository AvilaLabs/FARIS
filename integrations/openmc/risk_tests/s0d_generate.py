#!/usr/bin/env python3
"""S0d window generation on OpenMC 0.16.0, reusing S0b G6's multigroup library.

    s0d_generate.py --mode targeted --mgxs G6/mgxs.h5 --model-dir D --run-dir R --out gen.json --seed N
    s0d_generate.py --mode global   ...   (T3: as T1 without targets)
    s0d_generate.py --mode cadis    ...   (T2: user adjoint source over the TF material, no forward weighting)
    s0d_generate.py --mode targeted ... --smoke   (T0: two batches, one inactive)
    s0d_generate.py ... --mesh cylindrical        (S0e: build-aligned cylindrical mesh; see s0e_generate.py)

Protocol: docs/notes/CAD_TRANSPORT_S0D.md. Everything but the adjoint source is G6's: the same mgxs.h5 (not
regenerated, so the multigroup data is identical and 0.16.0's stochastic-slab defect is not reached), group structure,
rays, batches, mesh and random-ray settings. Materials are converted to their macroscopic entries by hand, under their
own names, because 0.16.0's convert_to_multigroup renames them (name_id) and would not match G6's library.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import os
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r1_configs as cfg  # noqa: E402
import r1_worker as R1  # noqa: E402
import r6_worker as W  # noqa: E402
import rm_m_openmc as RM  # noqa: E402
import s0b_common as s0b_c  # noqa: E402
import s0d_common as C  # noqa: E402

MODES = ("targeted", "global", "cadis")


def to_multigroup(openmc, model, mgxs_path: Path) -> list[str]:
    """Point every material at its macroscopic entry in an existing library; return the names used."""
    model.materials.cross_sections = str(mgxs_path)
    names = []
    for material in model.materials:
        material.set_density("macro", 1.0)
        material._nuclides = []
        material._sab = []
        material.add_macroscopic(material.name)
        names.append(material.name)
    model.settings.energy_mode = "multi-group"
    return names


def generate(args) -> int:
    import openmc

    structure = "CASMO-8+0.1MeV"
    edges = cfg.STRUCTURES[structure]
    resp_edges = cfg.fast_response_edges(structure)
    params = dict(W.DEFAULTS)
    if args.smoke:
        params.update(batches=2, inactive=1)
    run_dir = args.run_dir
    run_dir.mkdir(parents=True, exist_ok=True)
    os.chdir(run_dir)
    os.environ["PATH"] = str(Path(sys.executable).parent) + os.pathsep + os.environ.get("PATH", "")
    os.environ["OMP_NUM_THREADS"] = str(args.threads)
    mgxs_path = run_dir / "mgxs.h5"
    shutil.copyfile(args.mgxs, mgxs_path)
    record = {"kind": "s0d-generate", "mode": args.mode, "smoke": args.smoke, "openmc_version": openmc.__version__,
              "parameters": params, "group_edges_ev": edges, "adjoint_response_energy_edges_ev": resp_edges,
              "generation_seed": args.seed, "threads": args.threads, "steps": {}, "library": RM.library_identity(),
              "mgxs": {"source": str(args.mgxs), "sha256": RM.sha256(mgxs_path)}}
    roles = RM.load_roles(args.model_dir)
    ids = RM.role_ids(roles)
    h5m = args.model_dir / "rm_m_rr.h5m"
    record["h5m"] = {"file": str(h5m), "sha256": RM.sha256(h5m)}
    state = {}

    def build():
        model, dag = RM.build_model(openmc, h5m, roles, rr=True)
        state["model"], state["dag"] = model, dag
        model.settings.source = [openmc.IndependentSource(
            energy=openmc.stats.Discrete([W.SOURCE_ENERGY_EV], [1.0]), particle="neutron",
            constraints={"domains": [openmc.Cell(cell_id=ids["plasma"])]}, strength=1.0)]
        model.settings.run_mode = "fixed source"
        # The production tally's definition: one bin over every coil, 0.1-20 MeV.
        target = R1.response_a_material_tally(openmc, model, name="response_a")
        state["target"] = target
        model.tallies = openmc.Tallies([target])
        record["macroscopic_names"] = to_multigroup(openmc, model, mgxs_path)

    W.step(record, "build_model", build, workarounds=W.WORKAROUNDS_0153[1:2] + W.WORKAROUNDS_0153[3:4])
    if not record["steps"]["build_model"]["ok"]:
        return W.finish(record, args.out)
    model, dag, target = state["model"], state["dag"], state["target"]

    lower, upper = W.RAY_BOX_CM
    if getattr(args, "mesh", "cartesian") == "cylindrical":
        # S0e H6: one cylindrical mesh for the source regions and the windows (docs/notes/CAD_TRANSPORT_S0E.md)
        import s0e_common as E

        ww_mesh, record["ww_mesh"] = E.build_cylindrical_mesh(openmc)
    else:
        ww_mesh = openmc.RegularMesh(name="ww-mesh")
        cell = params["ww_cell_cm"]
        ww_mesh.lower_left, ww_mesh.upper_right = lower, upper
        ww_mesh.dimension = tuple(int(round((u - l) / cell)) for l, u in zip(lower, upper))
        record["ww_mesh"] = {"lower_left": lower, "upper_right": upper, "dimension": list(ww_mesh.dimension), "cell_cm": cell}

    def configure_and_run():
        s = model.settings
        s.photon_transport = False
        s.batches, s.inactive, s.particles = params["batches"], params["inactive"], params["rays"]
        s.seed = args.seed
        s.statepoint = {}
        random_ray = {
            "distance_inactive": params["distance_inactive_cm"], "distance_active": params["distance_active_cm"],
            "ray_source": openmc.IndependentSource(space=openmc.stats.Box(lower, upper)),
            "volume_estimator": params["volume_estimator"], "source_shape": params["source_shape"], "adjoint": False,
            "sample_method": params["sample_method"], "diagonal_stabilization_rho": params["diagonal_stabilization_rho"],
            "source_region_meshes": [(ww_mesh, [openmc.Universe(universe_id=dag.id)])],
        }
        generator = dict(mesh=ww_mesh, energy_bounds=list(edges), particle_type="neutron", method="fw_cadis",
                         max_realizations=params["batches"] - params["inactive"])
        if args.mode == "targeted":
            generator["targets"] = openmc.Tallies([target])
        elif args.mode == "cadis":
            tf = R1.tf_material(model)
            random_ray["adjoint_source"] = [C.cadis_adjoint_source(openmc, tf, resp_edges)]
            record["adjoint_source"] = C.cadis_description(resp_edges)
        s.random_ray = random_ray
        s.weight_window_generators = [openmc.WeightWindowGenerator(**generator)]
        model.export_to_model_xml(str(run_dir))
        run = RM.run_openmc_cli(run_dir, args.threads, "openmc_rr.out")
        record["random_ray_run"] = run
        if run["returncode"] != 0:
            raise RuntimeError(f"openmc random-ray run exited {run['returncode']}; see openmc_rr.out")
        files = sorted(run_dir.glob("weight_windows*.h5"))
        if not files:
            raise RuntimeError("random-ray run wrote no weight_windows.h5")
        record["weight_windows_file"] = str(files[0])
        record["weight_windows_sha256"] = RM.sha256(files[0])

    W.step(record, "random_ray", configure_and_run, workarounds=[W.WORKAROUNDS_0153[0], W.WORKAROUNDS_0153[2]])
    log = run_dir / "openmc_rr.out"
    if log.is_file():
        text = log.read_text(encoding="utf-8", errors="replace")
        record["random_ray_log_warnings"] = sorted({ln.strip() for ln in text.splitlines() if "WARNING" in ln.upper()})[:20]
        record["random_ray_log_stats"] = s0b_c.parse_random_ray_log(text)
    if record["steps"]["random_ray"]["ok"]:
        R1.summarise_windows(openmc, record)
        record["window_ranges"] = C.window_ranges(openmc, record["weight_windows_file"], resp_edges)
    return W.finish(record, args.out)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--mode", required=True, choices=MODES)
    ap.add_argument("--mgxs", required=True, type=Path, help="S0b G6's mgxs.h5")
    ap.add_argument("--model-dir", required=True, type=Path)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--seed", required=True, type=int)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--smoke", action="store_true", help="T0: two batches, one inactive")
    ap.add_argument("--mesh", choices=("cartesian", "cylindrical"), default="cartesian",
                    help="window and source-region mesh: S0d's 25 cm Cartesian cells, or S0e's build-aligned cylindrical mesh")
    return generate(ap.parse_args(argv))


if __name__ == "__main__":
    sys.exit(main())
