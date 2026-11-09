# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Pure, bounded tests for offline OpenMC-0.15.3 MicroXS collapse."""
from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

import h5py
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r3_bounded_microxs as BC  # noqa: E402


class CollapseMathTests(unittest.TestCase):
    def test_pinned_vitamin_j_42_edges_have_the_openmc_0153_hash(self):
        edges = np.asarray(BC.VITAMIN_J_42_EDGES_E_V, dtype="<f8")
        self.assertEqual(hashlib.sha256(edges.tobytes()).hexdigest(), BC.VITAMIN_J_42_EDGE_SHA256)

    def test_constant_and_linear_bins_match_hand_integrals(self):
        edges = np.array([0.0, 1.0, 2.0, 4.0])
        grid = np.array([0.0, 1.0, 2.0, 4.0])
        constant = BC.group_mean_xs(grid, np.array([3.0, 3.0, 3.0, 3.0]), 0, edges)
        np.testing.assert_allclose(constant, [3.0, 3.0, 3.0], rtol=0.0, atol=1e-14)
        linear = BC.group_mean_xs(grid, np.array([1.0, 3.0, 5.0, 9.0]), 0, edges)
        np.testing.assert_allclose(linear, [2.0, 4.0, 7.0], rtol=0.0, atol=1e-14)

    def test_threshold_and_grid_clipping_are_zero_outside_evaluated_range(self):
        edges = np.array([0.0, 1.0, 2.0, 3.0, 4.0, 5.0])
        grid = np.array([0.5, 1.5, 2.5, 3.5])
        # Threshold starts at 1.5 eV, where XS is 2 b; linear through 4 b at 2.5
        means = BC.group_mean_xs(grid, np.array([2.0, 4.0, 6.0]), 1, edges)
        np.testing.assert_allclose(means, [0.0, 1.25, 4.0, 2.75, 0.0], rtol=0.0, atol=1e-14)

    def test_flux_collapse_uses_actual_domain_flux_and_matches_zero_flux_api(self):
        group_means = np.array([[1.0, 3.0, 5.0], [2.0, 2.0, 8.0]])
        np.testing.assert_allclose(BC.collapse_flux(group_means, np.array([1.0, 2.0, 1.0])),
                                   [3.0, 3.5], rtol=0.0, atol=1e-14)
        np.testing.assert_array_equal(BC.collapse_flux(group_means, np.zeros(3)), np.zeros(2))

    def test_invalid_energy_or_xs_grid_is_refused(self):
        with self.assertRaisesRegex(ValueError, "nondecreasing"):
            BC.group_mean_xs(np.array([0.0, 2.0, 1.0]), np.array([1.0, 1.0, 1.0]), 0,
                             np.array([0.0, 1.0]))
        with self.assertRaisesRegex(ValueError, "nonnegative"):
            BC.group_mean_xs(np.array([0.0, 1.0]), np.array([1.0, -1.0]), 0,
                             np.array([0.0, 1.0]))

    def test_duplicate_grid_points_follow_openmc_zero_width_segment_behavior(self):
        means = BC.group_mean_xs(np.array([0.0, 1.0, 1.0, 2.0]),
                                 np.array([1.0, 2.0, 4.0, 6.0]), 0,
                                 np.array([0.0, 1.0, 2.0]))
        np.testing.assert_allclose(means, [1.5, 5.0], rtol=0.0, atol=1e-14)

    def test_group_means_agree_with_segment_by_segment_openmc_collapse_reference(self):
        grid = np.array([0.2, 0.7, 1.0, 1.6, 2.8, 3.2, 5.0])
        xs = np.array([0.1, 0.4, 0.2, 2.0, 1.0, 3.0])
        edges = np.array([0.0, 0.5, 1.3, 2.0, 4.0, 6.0])
        threshold = 1
        result = BC.group_mean_xs(grid, xs, threshold, edges)
        tabulated = grid[threshold:]
        expected = []
        for low, high in zip(edges[:-1], edges[1:]):
            lo = max(float(low), float(tabulated[0]))
            hi = min(float(high), float(tabulated[-1]))
            integral = 0.0
            if hi > lo:
                for i in range(len(tabulated) - 1):
                    left = max(lo, float(tabulated[i]))
                    right = min(hi, float(tabulated[i + 1]))
                    if right <= left:
                        continue
                    slope = (xs[i + 1] - xs[i]) / (tabulated[i + 1] - tabulated[i])
                    xs_left = xs[i] + slope * (left - tabulated[i])
                    xs_right = xs[i] + slope * (right - tabulated[i])
                    integral += 0.5 * (xs_left + xs_right) * (right - left)
            expected.append(integral / (high - low))
        np.testing.assert_allclose(result, expected, rtol=2e-14, atol=2e-14)


def _write_hdf(path: Path, name: str, *, with_gamma: bool = True):
    edges = np.asarray(BC.VITAMIN_J_42_EDGES_E_V, dtype=np.float64)
    grid = np.array([edges[0], edges[19], edges[20], edges[21], edges[-1]], dtype=np.float64)
    with h5py.File(path, "w") as hdf:
        nuc = hdf.create_group(name)
        kts = nuc.create_group("kTs")
        kts.create_dataset("294K", data=0.0253)
        energy = nuc.create_group("energy")
        energy.create_dataset("294K", data=grid)
        if with_gamma:
            xs = nuc.create_group("reactions/reaction_102/294K").create_dataset(
                "xs", data=np.array([1.0, 3.0, 5.0, 7.0]))
            xs.attrs["threshold_idx"] = 1
        # A large-like unneeded table stands in for angular/distribution data;
        # the collapse must never inspect it.
        nuc.create_dataset("reactions/reaction_002/product_0/distribution_0/angle/mu",
                           data=np.ones((3, 64), dtype=np.float64))


def _fixture(directory: Path, *, with_gamma: bool = True):
    a, b = directory / "A.h5", directory / "B.h5"
    _write_hdf(a, "A", with_gamma=with_gamma)
    _write_hdf(b, "B", with_gamma=False)
    xsec = directory / "cross_sections.xml"
    xsec.write_text("""<cross_sections>
  <library materials="A" path="A.h5" type="neutron"/>
  <library materials="B" path="B.h5" type="neutron"/>
</cross_sections>
""", encoding="utf-8")
    chain = directory / "chain.xml"
    chain.write_text("""<depletion_chain>
  <nuclide name="A"><reaction type="(n,gamma)" target="B"/></nuclide>
  <nuclide name="B"/>
</depletion_chain>
""", encoding="utf-8")
    materials = directory / "materials.xml"
    materials.write_text("""<materials><material id="1">
  <nuclide name="A" ao="1"/><nuclide name="B" ao="1"/>
</material></materials>
""", encoding="utf-8")
    # OpenMC MeshMaterialFilter.from_volumes order is row-major positions
    # where materials > -1; this fixture has three material-bearing bins.
    volumes = directory / "mesh_material_volumes.npz"
    np.savez_compressed(volumes, materials=np.array([[1, 2], [2, -2]], dtype=np.int64),
                        volumes=np.array([[1.0, 2.0], [3.0, 0.0]], dtype=np.float64))
    fluxes = directory / "fluxes.npy"
    spectra = np.zeros((3, 42), dtype=np.float64)
    spectra[0, :] = 1.0
    spectra[1, 20] = 1.0
    np.save(fluxes, spectra, allow_pickle=False)
    return {"xsec": xsec, "chain": chain, "materials": materials,
            "volumes": volumes, "fluxes": fluxes}


class BoundedWorkflowTests(unittest.TestCase):
    def test_plan_and_offline_collapse_stream_with_full_domain_flux_and_material_map(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root)
            edges = np.asarray(BC.VITAMIN_J_42_EDGES_E_V, dtype=np.float64)
            plan_path = root / "plan.json"
            run_dir = root / "run"
            plan = BC.prepare_plan(plan_path, fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                                   chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                                   materials_xml=paths["materials"], energy_edges_eV=edges,
                                   output_run_dir=run_dir, domain_chunk=2, output_budget_bytes=32 * 1024 * 1024,
                                   disk_reserve_bytes=0, memory_limit_bytes=256 * 1024 * 1024)
            self.assertEqual(plan["status"], "READY_FOR_BOUNDED_COLLAPSE")
            self.assertEqual(plan["domains"]["count"], 3)
            self.assertEqual(plan["required_microxs_nuclides"], ["A", "B"])
            self.assertEqual(plan["flux_zero_domain_count"], 1)
            record = BC.run_collapse(plan_path, run_dir)
            self.assertEqual(record["status"], "PASS")
            with h5py.File(run_dir / "microxs_one_group.h5", "r") as output:
                self.assertEqual(output["microxs_barns"].shape, (2, 3, 1))
                self.assertEqual(output["domain_element"][()].tolist(), [0, 0, 1])
                self.assertEqual(output["domain_material_id"][()].tolist(), [1, 2, 2])
                self.assertEqual(output["reaction_required_mask"][()].tolist(), [[1], [0]])
                self.assertEqual(output["reaction_data_present_mask"][()].tolist(), [[1], [0]])
                grid = np.array([edges[0], edges[19], edges[20], edges[21], edges[-1]], dtype=np.float64)
                group_means = BC.group_mean_xs(grid, np.array([1.0, 3.0, 5.0, 7.0]), 1, edges)
                mean = BC.collapse_flux(group_means[None, :], np.ones(42))[0]
                self.assertAlmostEqual(float(output["microxs_barns"][0, 0, 0]), mean, places=12)
                self.assertEqual(float(output["microxs_barns"][0, 2, 0]), 0.0)
                self.assertEqual(float(output["microxs_barns"][1, 0, 0]), 0.0)

    def test_missing_chain_channel_is_not_zero_filled(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root, with_gamma=False)
            with self.assertRaisesRegex(ValueError, "full-chain coverage"):
                BC.prepare_plan(root / "plan.json", fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                                chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                                materials_xml=paths["materials"], energy_edges_eV=BC.VITAMIN_J_42_EDGES_E_V,
                                output_run_dir=root / "run", disk_reserve_bytes=0)
            self.assertFalse((root / "plan.json").exists())

    def test_input_mutation_after_plan_is_rejected_before_run_directory_creation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root)
            plan_path, run_dir = root / "plan.json", root / "run"
            BC.prepare_plan(plan_path, fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                            chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                            materials_xml=paths["materials"], energy_edges_eV=BC.VITAMIN_J_42_EDGES_E_V,
                            output_run_dir=run_dir, output_budget_bytes=32 * 1024 * 1024,
                            disk_reserve_bytes=0, memory_limit_bytes=256 * 1024 * 1024)
            with paths["fluxes"].open("ab") as stream:
                stream.write(b"mutated")
            with self.assertRaisesRegex(ValueError, "input changed after planning"):
                BC.run_collapse(plan_path, run_dir)
            self.assertFalse(run_dir.exists())

    def test_activation_plan_binds_collapse_and_bounds_results_by_chunk(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root)
            plan_path, collapse_dir = root / "plan.json", root / "collapse"
            BC.prepare_plan(plan_path, fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                            chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                            materials_xml=paths["materials"], energy_edges_eV=BC.VITAMIN_J_42_EDGES_E_V,
                            output_run_dir=collapse_dir, output_budget_bytes=32 * 1024 * 1024,
                            disk_reserve_bytes=0, memory_limit_bytes=256 * 1024 * 1024)
            BC.run_collapse(plan_path, collapse_dir)
            activation_plan = BC.prepare_activation_plan(
                root / "activation-plan.json", collapse_plan_path=plan_path,
                collapse_record_path=collapse_dir / "collapse_record.json",
                microxs_path=collapse_dir / "microxs_one_group.h5", fluxes_path=paths["fluxes"],
                volumes_path=paths["volumes"], materials_xml=paths["materials"],
                chain_path=paths["chain"], output_run_dir=root / "activation-run",
                timesteps=[[1.0, "a"], [1.0, "d"]], source_rates=[5.0, 0.0],
                domain_chunk=2, output_budget_bytes=32 * 1024 * 1024,
                disk_reserve_bytes=0, memory_limit_bytes=256 * 1024 * 1024)
            self.assertEqual(activation_plan["status"], "READY_FOR_CHUNKED_ACTIVATION")
            self.assertEqual(activation_plan["domain_count"], 3)
            self.assertEqual(activation_plan["source_rates"], [5.0, 0.0])
            self.assertLess(activation_plan["resources"]["working_bytes_estimate"], 256 * 1024 * 1024)
            self.assertEqual(activation_plan["inputs"]["microxs"]["sha256"],
                             BC.sha256_file(collapse_dir / "microxs_one_group.h5"))

    def test_plan_and_run_refuse_existing_artifacts(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root)
            plan_path, run_dir = root / "plan.json", root / "run"
            args = dict(fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                        chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                        materials_xml=paths["materials"], energy_edges_eV=BC.VITAMIN_J_42_EDGES_E_V,
                        output_run_dir=run_dir, output_budget_bytes=32 * 1024 * 1024,
                        disk_reserve_bytes=0, memory_limit_bytes=256 * 1024 * 1024)
            BC.prepare_plan(plan_path, **args)
            with self.assertRaises(FileExistsError):
                BC.prepare_plan(plan_path, **args)
            run_dir.mkdir()
            with self.assertRaises(FileExistsError):
                BC.run_collapse(plan_path, run_dir)

    def test_plan_checks_flux_domain_and_group_shapes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root)
            np.save(paths["fluxes"], np.ones((2, 42)), allow_pickle=False)
            with self.assertRaisesRegex(ValueError, "domain count.*differs from flux rows"):
                BC.prepare_plan(root / "plan.json", fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                                chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                                materials_xml=paths["materials"], energy_edges_eV=BC.VITAMIN_J_42_EDGES_E_V,
                                output_run_dir=root / "run", disk_reserve_bytes=0)

    def test_plan_refuses_a_non_vitamin_42_group_map(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            paths = _fixture(root)
            with self.assertRaisesRegex(ValueError, "pinned OpenMC VITAMIN-J-42"):
                BC.prepare_plan(root / "plan.json", fluxes_path=paths["fluxes"], volumes_path=paths["volumes"],
                                chain_path=paths["chain"], cross_sections_xml=paths["xsec"],
                                materials_xml=paths["materials"], energy_edges_eV=np.linspace(0, 4.2, 43),
                                output_run_dir=root / "run", disk_reserve_bytes=0)

    def test_activation_source_slots_keep_void_position_and_skip_padding(self):
        materials = np.array([[1, -1, 2, -2], [3, -2, -2, -2]], dtype=np.int64)
        slots = BC.domain_source_slot_map(materials)
        self.assertEqual([(x["domain_index"], x["element"], x["material_id"], x["source_slot"])
                          for x in slots], [(0, 0, 1, 0), (1, 0, 2, 2), (2, 1, 3, 0)])

    def test_activation_resource_estimate_scales_with_domain_chunk(self):
        small = BC.activation_resource_estimate(6400, 3820, 2190, 7, 4, 8)
        large_chunk = BC.activation_resource_estimate(6400, 3820, 2190, 7, 4, 32)
        self.assertEqual(small["depletion_result_raw_bytes_lower_bound"],
                         large_chunk["depletion_result_raw_bytes_lower_bound"])
        self.assertLess(small["working_bytes_estimate"], large_chunk["working_bytes_estimate"])
        self.assertLess(small["depletion_results_bytes_upper_bound"], 8 * BC.GIB)


if __name__ == "__main__":
    unittest.main()
