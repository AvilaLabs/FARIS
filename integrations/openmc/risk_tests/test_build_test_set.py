"""Fast tests of the parameter helpers of build_test_set.py (standard library; the classifier test needs numpy)."""
import copy
import json
import math
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_test_set as bts  # noqa: E402
import rm_m_spec as spec  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import openmc_ci  # noqa: E402

try:
    import numpy  # noqa: F401
    HAVE_NUMPY = True
except ImportError:
    HAVE_NUMPY = False


class Parameters(unittest.TestCase):
    def test_models(self):
        for name in bts.MODELS:
            p = bts.model_params(name)
            self.assertEqual(p["name"], name)
            self.assertGreater(p["outboard_total"], 100.0)
        with self.assertRaises(ValueError):
            bts.model_params("nope")

    def test_st_compact(self):
        p = bts.model_params("st-compact")
        self.assertEqual((p["major"], p["minor"], p["elongation"], p["n_coils"]), (170.0, 100.0, 2.5, 12))
        self.assertAlmostEqual(p["major"] / p["minor"], 1.7)
        self.assertEqual(p["fusion_power_mw"], 200.0)
        with bts.configured(p):
            by_name = {layer["name"]: layer for layer in spec.LAYERS}
            inboard = {n: spec.layer_thickness_cm(layer, -1.0) for n, layer in by_name.items()}
            outboard = {n: spec.layer_thickness_cm(layer, 1.0) for n, layer in by_name.items()}
        self.assertEqual(inboard["tih2_shield"], 20.0)
        self.assertEqual(inboard["first_wall_tungsten"], 1.0)
        for n in bts.PINCHED:
            self.assertEqual(inboard[n], 0.0)
            self.assertGreater(outboard[n], 0.0)
        self.assertEqual(outboard["blanket_flibe"], 100.0)

    def test_configured_restores_spec(self):
        before = (spec.MAJOR_RADIUS_CM, spec.N_TF_COILS, spec.LAYERS, spec.layer_thickness_cm)
        with bts.configured(bts.model_params("large")):
            self.assertAlmostEqual(spec.MAJOR_RADIUS_CM, 594.0)
        self.assertEqual(before, (spec.MAJOR_RADIUS_CM, spec.N_TF_COILS, spec.LAYERS, spec.layer_thickness_cm))

    def test_large_is_scaled(self):
        rm, large = bts.model_params("bent-duct"), bts.model_params("large")
        for key in ("major", "minor", "tf_radial", "gap", "port_w", "port_h", "outboard_total"):
            self.assertAlmostEqual(large[key], 1.8 * rm[key])
        self.assertEqual(large["elongation"], rm["elongation"])
        self.assertEqual(large["n_coils"], 18)
        for a, b in zip(port_x(bts.straight_variant(rm)), port_x(large)):
            self.assertAlmostEqual(b, 1.8 * a, places=6)

    def test_ports(self):
        self.assertEqual(bts.port_boxes(bts.model_params("no-port")), [])
        self.assertEqual(len(bts.port_boxes(bts.model_params("bent-duct"))), 3)
        self.assertEqual(len(bts.port_boxes(bts.model_params("large"))), 1)

    def test_dogleg_offset(self):
        p = bts.model_params("bent-duct")
        a, b, c = bts.port_boxes(p)
        w = p["port_w"]
        self.assertAlmostEqual((c[2] + c[3]) / 2 - (a[2] + a[3]) / 2, 1.5 * w)
        # the first leg ends and the last begins in the middle of the stack, and their y ranges do not meet
        xm = p["major"] + p["minor"] + (p["sol_out"] + p["outboard_total"]) / 2
        self.assertTrue(c[0] < xm < a[1])
        self.assertLess(a[3], c[2])

    def test_coil_span(self):
        self.assertAlmostEqual(bts.coil_span_deg(18), 16.0)
        self.assertAlmostEqual(bts.coil_span_deg(12), 24.0)


class Closure(unittest.TestCase):
    """The ion density of each generated design is scaled so its profiles close at its own fusion power."""

    @classmethod
    def setUpClass(cls):
        cls.template = json.loads(bts.RM_M_DESIGN.read_text())["plasma"]

    # Verifies: SRC-010
    def test_the_rm_m_template_closes_at_525_mw(self):
        self.assertEqual(self.template["fusion_power_mw"], 525.0)
        self.assertLess(abs(bts.implied_fusion_power_mw(self.template) / 525.0 - 1.0), 1.0e-4)

    # Verifies: SRC-010
    def test_the_model_card_densities_imply_the_rust_value(self):
        # 190.5465 MW is the Rust integral of crates/faris-engine/src/design/source.rs for the model card's densities.
        plasma = copy.deepcopy(self.template)
        plasma["ion_density_m3"].update(centre=1.5e20, pedestal=1.1e20, separatrix=4.0e19)
        self.assertLess(abs(bts.implied_fusion_power_mw(plasma) / 190.5465 - 1.0), 1.0e-4)

    # Verifies: SRC-010
    def test_every_model_closes_at_its_own_fusion_power(self):
        for name in bts.MODELS:
            p = bts.model_params(name)
            plasma = bts.model_plasma(p, self.template)
            implied = bts.implied_fusion_power_mw(plasma)
            self.assertLess(abs(implied / p["fusion_power_mw"] - 1.0), bts.CLOSURE_TOLERANCE / 10, name)
            self.assertEqual(plasma["fusion_power_mw"], p["fusion_power_mw"])
            self.assertIn("scaled by", plasma["field_provenance"]["ion_density_m3"]["note"])

    # Verifies: SRC-010
    def test_scaling_keeps_the_shape_and_everything_else(self):
        p = bts.model_params("st-compact")
        plasma = bts.model_plasma(p, self.template)
        old, new = self.template["ion_density_m3"], plasma["ion_density_m3"]
        ratios = [new[k] / old[k] for k in ("centre", "pedestal", "separatrix")]
        self.assertAlmostEqual(ratios[0], ratios[1], places=9)
        self.assertAlmostEqual(ratios[0], ratios[2], places=9)
        self.assertEqual(new["peaking_factor"], old["peaking_factor"])
        self.assertEqual(plasma["ion_temperature_ev"], self.template["ion_temperature_ev"])
        # a spherical tokamak at 200 MW in a smaller volume needs a different level than RM-M
        self.assertNotAlmostEqual(ratios[0], 1.0, places=2)
        self.assertEqual(self.template["ion_density_m3"], old)


def port_x(p):
    return [v for box in bts.port_boxes(p) for v in (box[0], box[1])]


class Regions(unittest.TestCase):
    def test_annulus(self):
        outer = [(2.0, 0.0), (0.0, 2.0), (-2.0, 0.0), (0.0, -2.0)]
        inner = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)]
        regions = bts.layer_regions(inner, outer)
        self.assertEqual([r["kind"] for r in regions], ["annulus"])

    def test_crescent_shares_tips(self):
        inner = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0), (0.5, -0.5)]
        outer = [(2.0, 0.0), (0.0, 2.0), (-1.0, 0.0), (0.0, -1.0), (0.5, -0.5)]
        (region,) = bts.layer_regions(inner, outer)
        self.assertEqual(region["kind"], "crescent")
        self.assertIsNone(region["hole"])
        pts = region["outer"]
        self.assertEqual(len(pts), len(set(pts)) + 0)  # no repeated vertex, tips are used once
        self.assertIn((-1.0, 0.0), pts)
        self.assertIn((1.0, 0.0), pts)

    def test_zero_everywhere(self):
        pts = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]
        with self.assertRaises(ValueError):
            bts.layer_regions(pts, list(pts))

    def test_st_layers_pinch_exactly(self):
        p = bts.model_params("st-compact")
        with bts.configured(p):
            boundaries = spec.layer_boundaries()
        pinched = 0
        for before, after, layer in zip(boundaries, boundaries[1:], p["layers"]):
            regions = bts.layer_regions(before["outer"], after["outer"])
            self.assertEqual(len(regions), 1)
            if layer["name"] in bts.PINCHED:
                self.assertEqual(regions[0]["kind"], "crescent")
                pinched += 1
            else:
                self.assertEqual(regions[0]["kind"], "annulus")
        self.assertEqual(pinched, len(bts.PINCHED))


class Classifier(unittest.TestCase):
    def setUp(self):
        if not HAVE_NUMPY:
            openmc_ci.skip_or_fail("numpy not available")

    def test_straight_duct_has_sight_and_dogleg_does_not(self):
        straight = bts.model_params("large")
        dog = bts.model_params("bent-duct")
        self.assertGreater(bts.line_of_sight(straight, lines=300)["plasma_to_outside"], 0)
        self.assertGreater(bts.line_of_sight(bts.straight_variant(dog), lines=300)["plasma_to_outside"], 0)
        result = bts.line_of_sight(dog, lines=300)
        self.assertEqual(result["plasma_to_outside"], 0)
        self.assertEqual(result["plasma_to_coil"], 0)

    def test_regions(self):
        import numpy as np
        p = bts.model_params("no-port")
        classify = bts.make_classifier(p)
        r = p["major"]
        got = classify(np.array([r, r + p["minor"] + 1.0, r + p["minor"] + 60.0, 0.0]), np.zeros(4), np.zeros(4))
        self.assertEqual(list(got), [1, 0, 2, 4])


if __name__ == "__main__":
    unittest.main()
