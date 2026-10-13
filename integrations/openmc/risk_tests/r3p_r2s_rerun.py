#!/usr/bin/env python3
"""R3 follow-ups (docs/notes/CAD_TRANSPORT_RISK_TESTS.md): mesh R2S rerun from R3's stored neutron model file.

    r3p_r2s_rerun.py --r3-r2s DIR --out-dir R [r3_r2s.py options] [--groups NAME] [--voxel-cm 10]

r3_r2s.py unchanged, except that the neutron model is read from DIR/neutron_transport/model.xml (its tallies
dropped) instead of being rebuilt, so the plasma-source package is not needed. --groups sets the activation group
structure and --voxel-cm the activation and scoring voxel size over the same box.
"""
from __future__ import annotations

from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_r2s as R  # noqa: E402
import r3_scenario as sc  # noqa: E402


def take(flag: str, default):
    if flag in sys.argv:
        i = sys.argv.index(flag)
        value = sys.argv[i + 1]
        del sys.argv[i:i + 2]
        return value
    return default


def main() -> int:
    stored = Path(take("--r3-r2s", "")) / "neutron_transport" / "model.xml"
    sc.ACTIVATION_GROUPS = take("--groups", sc.ACTIVATION_GROUPS)
    voxel = float(take("--voxel-cm", sc.MESH_VOXEL_CM))
    if voxel != sc.MESH_VOXEL_CM:
        scale = sc.MESH_VOXEL_CM / voxel
        if abs(scale - round(scale)) > 1e-9:
            raise SystemExit("--voxel-cm must divide 10")
        sc.MESH_DIMENSION = tuple(int(n * round(scale)) for n in sc.MESH_DIMENSION)
        sc.MESH_VOXEL_CM = voxel
    if not stored.is_file():
        raise SystemExit(f"{stored} not found")

    def build_models(openmc, model_dir, mesh, n_particles, n_batches, p_particles, p_batches, seeds):
        neutron = openmc.Model.from_model_xml(str(stored))
        neutron.tallies = openmc.Tallies()
        s = neutron.settings
        s.batches, s.particles, s.seed = n_batches, n_particles, seeds[0]
        photon = openmc.Model(geometry=neutron.geometry, materials=neutron.materials)
        ps = openmc.Settings()
        ps.run_mode = "fixed source"
        ps.batches, ps.particles, ps.seed = p_batches, p_particles, seeds[1]
        ps.photon_transport = True
        ps.source_rejection_fraction = 1.0e-4
        ps.temperature = dict(s.temperature)
        ps.output = {"summary": False, "tallies": False}
        photon.settings = ps
        photon.tallies = openmc.Tallies([R.dose_tally(openmc, mesh)])
        return neutron, photon, {}

    R.build_models = build_models
    return R.main()


if __name__ == "__main__":
    raise SystemExit(main())
