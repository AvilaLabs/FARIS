# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure ITER_1D response mapping and coverage tests; no OpenMC import or solver."""
from __future__ import annotations

import importlib.util
import hashlib
import tempfile
import unittest
from pathlib import Path

WORKER = Path(__file__).resolve().parents[1] / "iter_1d_reference.py"
SPEC = importlib.util.spec_from_file_location("iter_1d_reference", WORKER)
ITER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(ITER)


class Iter1dResponseMapping(unittest.TestCase):
    def test_preflight_requires_registered_upstream_bytes_and_rejects_modified_translation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            benchmark_root = root / "ITER_1D"
            input_dir = benchmark_root / "ITER_1D" / "openmc"
            input_dir.mkdir(parents=True)
            content = {
                "geometry.xml": b"<geometry />",
                "materials.xml": b"<materials />",
                "iter_1d_source.cpp": b"source",
                "CMakeLists.txt": b"build",
                "benchmark_metadata.json": b'{"name":"ITER_1D","version":{"openmc":"1.1"}}',
            }
            for name, data in content.items():
                destination = benchmark_root / name if name == "benchmark_metadata.json" else input_dir / name
                destination.write_bytes(data)
            identity = {
                name: {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
                for name, data in content.items()
            }
            registration = {"input_identity": {
                "geometry_xml": identity["geometry.xml"],
                "materials_xml": identity["materials.xml"],
                "iter_1d_source_cpp": identity["iter_1d_source.cpp"],
                "cmakelists_txt": identity["CMakeLists.txt"],
                "benchmark_metadata": {"sha256": identity["benchmark_metadata.json"]["sha256"]},
            }}
            commit = "1d9855b00e05c5d5e25a11026ac56940dc75b516"
            manifest = {"case_id": "iter-1d", "source": {"commit": commit}, "files": [
                {"path_in_source": name, "sha256": row["sha256"]} for name, row in identity.items()
            ]}
            checked = ITER.verify_registered_inputs(input_dir, benchmark_root, registration, manifest)
            self.assertEqual(set(checked), set(content))
            self.assertEqual(ITER.resolve_inputs(input_dir), input_dir.resolve())
            # Direct --benchmark-dir <.../openmc> intake resolves the same
            # directory and keeps the upstream metadata adjacent to it.
            (input_dir.parent / "benchmark_metadata.json").write_bytes(content["benchmark_metadata.json"])
            direct = ITER.resolve_inputs(input_dir)
            self.assertEqual(direct, input_dir.resolve())
            direct_checked = ITER.verify_registered_inputs(direct, direct, registration, manifest)
            self.assertEqual(direct_checked["benchmark_metadata.json"]["sha256"], identity["benchmark_metadata.json"]["sha256"])
            original = (input_dir / "materials.xml").read_bytes()
            (input_dir / "materials.xml").write_bytes(bytes([original[0] ^ 1]) + original[1:])
            self.assertEqual(ITER.resolve_inputs(input_dir), input_dir.resolve())
            with self.assertRaisesRegex(ValueError, "differs from registration: materials.xml"):
                ITER.verify_registered_inputs(input_dir, benchmark_root, registration, manifest)

            copied_inputs = root / "copied_inputs"
            copied_inputs.mkdir()
            for name in content:
                source = benchmark_root / name if name == "benchmark_metadata.json" else input_dir / name
                if name == "materials.xml":
                    source.write_bytes(original)
                (copied_inputs / name).write_bytes(source.read_bytes())
            ITER.verify_registered_inputs(copied_inputs, copied_inputs, registration, manifest)
            copied_source = copied_inputs / "iter_1d_source.cpp"
            source_bytes = copied_source.read_bytes()
            copied_source.write_bytes(bytes([source_bytes[0] ^ 1]) + source_bytes[1:])
            with self.assertRaisesRegex(ValueError, "SHA-256 differs from registration: iter_1d_source.cpp"):
                ITER.verify_registered_inputs(copied_inputs, copied_inputs, registration, manifest)

    def test_input_preflight_requires_all_97_material_cells_and_analytic_volumes(self):
        cells = list(range(1, 98))
        volumes = {cell: float(cell) for cell in cells}
        ITER.validate_material_cell_coverage(cells, volumes)
        with self.assertRaisesRegex(ValueError, "exactly 97"):
            ITER.validate_material_cell_coverage(cells[:-1], {cell: volumes[cell] for cell in cells[:-1]})
        with self.assertRaisesRegex(ValueError, "exactly cover"):
            ITER.validate_material_cell_coverage(cells, {cell: volumes[cell] for cell in cells[:-1]})

    def test_maps_flux_and_absorption_by_exact_material_cell_and_volume(self):
        cells = [51, 52, 53]
        volumes = {51: 10.0, 52: 20.0, 53: 40.0}
        flux = ITER.map_material_cell_tally(cells, cells, volumes, [10.0, 20.0, 40.0], [1.0, 2.0, 4.0])
        absorption = ITER.map_material_cell_tally(cells, cells, volumes, [2.0, 4.0, 8.0], [0.2, 0.4, 0.8])
        self.assertEqual([row["cell_id"] for row in flux], cells)
        self.assertEqual([row["volume_average_per_source"] for row in flux], [1.0, 1.0, 1.0])
        self.assertEqual([row["volume_average_per_source"] for row in absorption], [0.2, 0.2, 0.2])
        self.assertEqual(flux[2]["volume_average_std_dev_per_source"], 0.1)

    def test_coverage_requires_both_97_cell_response_classes(self):
        cells = list(range(1, 98))
        bins = [{"cell_id": cell} for cell in cells]
        results = {
            "ITER_1D_material_cell_neutron_flux_tracklength": {
                "score": "flux", "estimator": "tracklength", "filter_cell_ids": cells, "bins": bins,
            },
            "ITER_1D_material_cell_absorption_analog": {
                "score": "absorption", "estimator": "analog", "filter_cell_ids": cells, "bins": bins,
            },
        }
        self.assertEqual(ITER.validate_response_coverage(results, cells), 194)
        with self.assertRaisesRegex(ValueError, "absorption"):
            ITER.validate_response_coverage({key: value for key, value in results.items() if "absorption" not in key}, cells)

    def test_tally_mapping_refuses_missing_or_reordered_cells_and_invalid_uncertainty(self):
        ids = [1, 2]
        volumes = {1: 10.0, 2: 20.0}
        with self.assertRaisesRegex(ValueError, "exact material-cell order"):
            ITER.map_material_cell_tally([2, 1], ids, volumes, [1.0, 2.0], [0.1, 0.2])
        with self.assertRaisesRegex(ValueError, "standard deviation"):
            ITER.map_material_cell_tally(ids, ids, volumes, [1.0, 2.0], [0.1, float("nan")])


if __name__ == "__main__":
    unittest.main()
