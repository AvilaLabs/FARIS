"""Tests for the CAD transport risk-test scripts. Standard library only; no OpenMC, CadQuery or numpy.

Run from the repository root:
    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'
"""

import copy
import math
import unittest

import r1_configs as R1
import rm_m_checks as C
import rm_m_spec as S


class ModelCardLabels(unittest.TestCase):
    def test_default_card_is_fully_labelled(self):
        card = S.model_card()
        self.assertEqual(S.check_labels(card), [])
        counts = S.label_counts(card)
        self.assertGreater(counts[S.PUBLISHED], 10)
        self.assertGreater(counts[S.AUTHORED], 10)

    def test_published_dimension_needs_a_source(self):
        card = S.model_card()
        card["plasma"]["major_radius"]["source"] = ""
        problems = S.check_labels(card)
        self.assertEqual(len(problems), 1)
        self.assertIn("plasma.major_radius", problems[0])

    def test_unknown_label_is_rejected(self):
        card = S.model_card()
        card["port"]["width"]["label"] = "estimated"
        self.assertTrue(any("port.width" in p and "neither" in p for p in S.check_labels(card)))

    def test_empty_card_is_a_problem(self):
        self.assertEqual(S.check_labels({"nothing": {}}), ["card has no labelled dimensions"])

    def test_protocol_values_are_published(self):
        plasma = S.model_card()["plasma"]
        for key, value in (("major_radius", 330.0), ("minor_radius", 113.0), ("elongation", 1.84), ("triangularity", 0.375)):
            self.assertEqual(plasma[key]["value"], value)
            self.assertEqual(plasma[key]["label"], S.PUBLISHED)
        self.assertAlmostEqual(S.SOURCE_RATE_N_S, 1.86e20, delta=2e17)

    def test_authored_choices_are_labelled_authored(self):
        card = S.model_card()
        self.assertEqual(card["layers"]["tih2_shield"]["outboard"]["label"], S.AUTHORED)
        self.assertEqual(card["layers"]["tih2_shield"]["inboard"]["label"], S.PUBLISHED)
        self.assertEqual(card["layers"]["blanket_flibe"]["outboard"]["label"], S.PUBLISHED)
        for key in ("toroidal_span", "case_wall"):
            self.assertEqual(card["tf_coils"][key]["label"], S.AUTHORED)
        self.assertEqual(card["tf_coils"]["count"]["label"], S.PUBLISHED)
        self.assertEqual(card["port"]["width"]["label"], S.AUTHORED)

    def test_every_layer_tag_has_a_material_basis(self):
        for layer in S.LAYERS:
            self.assertIn(layer["tag"], S.MATERIAL_BASIS)

    def test_winding_pack_fractions_sum_to_one(self):
        self.assertAlmostEqual(sum(f["value"] for f in S.WP_FRACTIONS.values()), 1.0)


class Geometry(unittest.TestCase):
    def setUp(self):
        self.boundaries = S.layer_boundaries()

    @staticmethod
    def midplane(points, outboard):
        """R of the profile where it crosses Z = 0, inboard or outboard side."""
        side = [p for p in points if (p[0] > 330.0) == outboard]
        return min(side, key=lambda p: abs(p[1]))[0]

    def test_published_inboard_build_thicknesses(self):
        names = [b["name"] for b in self.boundaries]
        radii = [self.midplane(b["outer"], outboard=False) for b in self.boundaries]
        # Radial positions at the inboard midplane (the minimum R of each profile is within 1 cm of it).
        thickness = {n: radii[i - 1] - radii[i] for i, n in enumerate(names) if i}
        self.assertAlmostEqual(radii[0], 330.0 - 113.0, places=6)
        for layer in S.LAYERS:
            self.assertAlmostEqual(thickness[layer["name"]], layer["in"]["value"], delta=0.05, msg=layer["name"])

    def test_outboard_blanket_is_one_metre(self):
        names = [b["name"] for b in self.boundaries]
        outer = {n: self.midplane(b["outer"], outboard=True) for n, b in zip(names, self.boundaries)}
        self.assertAlmostEqual(outer["plasma"], 443.0, places=6)
        self.assertAlmostEqual(outer["blanket_flibe"] - outer["vv_outer_wall"], 100.0, places=3)
        self.assertAlmostEqual(outer["tih2_shield"] - outer["thermal_shield"], 20.0, places=3)

    def test_tf_leg_is_64_cm_radially_at_the_inboard_midplane(self):
        tf = S.tf_profiles()
        # Points nearest the midplane (Z ~ 0) on the inboard side.
        def at_midplane(points):
            return min((p for p in points if p[0] < 330 and abs(p[1]) < 2.0), key=lambda p: abs(p[1]))[0]
        self.assertAlmostEqual(at_midplane(tf["inner"]) - at_midplane(tf["outer"]), 64.0, delta=0.5)

    def test_tf_inboard_leg_position_records_the_six_centimetre_shift(self):
        tf = S.tf_profiles()
        inner = min(p[0] for p in tf["inner"])
        self.assertAlmostEqual(inner, 128.0, delta=1.5)  # not the 134 cm of S15 Fig. 2, see deviation D1
        self.assertTrue(any(d["id"] == "D1" for d in S.DEVIATIONS))

    def test_profiles_are_counter_clockwise_and_nested(self):
        areas = [S.polygon_area(b["outer"]) for b in self.boundaries]
        self.assertTrue(all(a > 0 for a in areas))
        self.assertEqual(areas, sorted(areas))
        self.assertEqual(len(self.boundaries[0]["outer"]), S.POLOIDAL_POINTS)

    def test_plasma_volume_is_near_the_published_value(self):
        volume_m3 = S.revolved_volume_cm3(self.boundaries[0]["outer"]) / 1e6
        self.assertAlmostEqual(volume_m3, 145.7, delta=0.5)  # S15 Table 1 gives 141, K18 137

    def test_revolved_volume_of_a_circle_is_a_torus(self):
        n = 720
        circle = [(300 + 50 * math.cos(2 * math.pi * i / n), 50 * math.sin(2 * math.pi * i / n)) for i in range(n)]
        self.assertAlmostEqual(S.revolved_volume_cm3(circle), 2 * math.pi ** 2 * 300 * 50 ** 2, delta=2e-4 * 2 * math.pi ** 2 * 300 * 2500)

    def test_thickness_grades_between_inboard_and_outboard(self):
        flibe = next(l for l in S.LAYERS if l["name"] == "blanket_flibe")
        self.assertAlmostEqual(S.layer_thickness_cm(flibe, -1.0), 20.0)
        self.assertAlmostEqual(S.layer_thickness_cm(flibe, 1.0), 100.0)
        self.assertAlmostEqual(S.layer_thickness_cm(flibe, 0.0), 60.0)

    def test_eighteen_coils_with_the_port_between_two(self):
        centres = S.coil_centres_deg()
        self.assertEqual(len(centres), 18)
        half = S.TF_TOROIDAL_SPAN_DEG["value"] / 2.0
        for c in centres:
            self.assertTrue(0.0 < c - half and c + half < 360.0)  # no coil straddles the port at 0 degrees
        # The port at 0 degrees lies in the gap between the coil at 350 and the coil at 10.
        gap_half = 10.0 - half
        self.assertGreater(gap_half, 1.5)
        self.assertLessEqual(S.PORT_WIDTH_CM["value"] / 2.0, 580.0 * math.radians(gap_half))


class ProfileFidelity(unittest.TestCase):
    def test_chord_sag_is_within_one_millimetre(self):
        report = S.fidelity_report()
        self.assertEqual(report["points"], S.POLOIDAL_POINTS)
        self.assertLessEqual(report["max_chord_sag_cm"], 0.1)

    def test_sag_falls_with_the_square_of_the_point_count(self):
        coarse = S.chord_sag_cm(lambda c: 0.0, 60)
        fine = S.chord_sag_cm(lambda c: 0.0, 120)
        self.assertAlmostEqual(coarse / fine, 4.0, delta=0.4)

    def test_thickness_departure_is_reported_for_every_layer(self):
        dep = S.thickness_departure_cm()
        self.assertEqual(set(dep), {layer["name"] for layer in S.LAYERS})
        self.assertLess(max(dep.values()), 1.0)

    def test_circle_offset_has_no_departure(self):
        self.assertAlmostEqual(S.point_segment_distance((1.0, 1.0), (0.0, 0.0), (2.0, 0.0)), 1.0)
        self.assertAlmostEqual(S.point_segment_distance((5.0, 0.0), (0.0, 0.0), (2.0, 0.0)), 3.0)

    def test_polygon_deviation_is_recorded(self):
        self.assertTrue(any(d["id"] == "D5" for d in S.DEVIATIONS))


class VolumeComparison(unittest.TestCase):
    def rows(self, errors):
        return [{"name": f"s{i}", "cad_volume_cm3": 1000.0, "faceted_volume_cm3": 1000.0 * (1 + e)} for i, e in enumerate(errors)]

    def test_pass_within_half_percent(self):
        result = C.volume_comparison(self.rows([0.0, -0.004, 0.0049]))
        self.assertTrue(result["pass"])
        self.assertEqual(result["failing"], [])

    def test_one_failing_solid_fails_all(self):
        result = C.volume_comparison(self.rows([0.0, -0.006]))
        self.assertFalse(result["pass"])
        self.assertEqual(result["failing"], ["s1"])
        self.assertEqual(result["worst"]["name"], "s1")

    def test_boundary_is_inclusive(self):
        self.assertTrue(C.volume_comparison(self.rows([0.005]))["pass"])

    def test_empty_input_does_not_pass(self):
        self.assertFalse(C.volume_comparison([])["pass"])

    def test_nonpositive_cad_volume_is_an_error(self):
        with self.assertRaises(ValueError):
            C.volume_comparison([{"name": "x", "cad_volume_cm3": 0.0, "faceted_volume_cm3": 1.0}])


class AcceptanceRules(unittest.TestCase):
    def test_lost_particles_pass_at_the_limit(self):
        self.assertTrue(C.lost_particle_check(1, 1_000_000)["pass"])
        self.assertTrue(C.lost_particle_check(0, 2_000_000)["pass"])

    def test_lost_particles_fail_above_the_limit(self):
        self.assertFalse(C.lost_particle_check(2, 1_000_000)["pass"])

    def test_too_few_histories_cannot_pass(self):
        result = C.lost_particle_check(0, 999_999)
        self.assertFalse(result["enough_histories"])
        self.assertFalse(result["pass"])

    def test_lost_particle_inputs_validated(self):
        with self.assertRaises(ValueError):
            C.lost_particle_check(-1, 10)
        with self.assertRaises(ValueError):
            C.lost_particle_check(0, 0)

    def test_source_sites_must_all_be_inside(self):
        self.assertTrue(C.source_site_check(1000, 1000)["pass"])
        result = C.source_site_check(999, 1000)
        self.assertFalse(result["pass"])
        self.assertAlmostEqual(result["fraction_inside"], 0.999)

    def test_source_site_inputs_validated(self):
        for args in ((1, 0), (5, 4), (-1, 4)):
            with self.assertRaises(ValueError):
                C.source_site_check(*args)


class R6Rule(unittest.TestCase):
    OK = {"ok": True, "workarounds": ["universe id", "filler"]}

    def steps(self, **overrides):
        base = {"mgxs": dict(self.OK), "random_ray": dict(self.OK), "weight_windows": dict(self.OK)}
        base.update(overrides)
        return base

    def test_agreement_within_three_sigma(self):
        self.assertTrue(C.agree_within_sigma(1.0, 0.1, 1.2, 0.1)["agree"])      # z = 1.41
        self.assertFalse(C.agree_within_sigma(1.0, 0.1, 1.5, 0.1)["agree"])     # z = 3.54
        self.assertTrue(C.agree_within_sigma(1.0, 0.1, 1.42, 0.1)["agree"])     # z = 2.97
        self.assertTrue(C.agree_within_sigma(2.0, 0.0, 2.0, 0.0)["agree"])
        self.assertFalse(C.agree_within_sigma(2.0, 0.0, 2.1, 0.0)["agree"])

    def test_choose_new_version_when_clean_and_agreeing(self):
        result = C.r6_choice(self.steps(), self.steps(), (1.0, 0.1), (1.1, 0.1))
        self.assertEqual(result["choice"], "0.16.0")
        self.assertTrue(result["rule1_steps_clean"] and result["rule2_flux_agrees"])

    def test_extra_workaround_keeps_the_old_version(self):
        new = self.steps(random_ray={"ok": True, "workarounds": ["universe id", "filler", "new flag"]})
        result = C.r6_choice(self.steps(), new, (1.0, 0.1), (1.0, 0.1))
        self.assertEqual(result["choice"], "0.15.3")
        self.assertFalse(result["rule1_steps_clean"])
        self.assertTrue(any("new flag" in r for r in result["reasons"]))

    def test_fewer_workarounds_are_fine(self):
        new = self.steps(random_ray={"ok": True, "workarounds": ["filler"]})
        self.assertEqual(C.r6_choice(self.steps(), new, (1.0, 0.1), (1.0, 0.1))["choice"], "0.16.0")

    def test_failed_step_keeps_the_old_version(self):
        new = self.steps(weight_windows={"ok": False, "workarounds": []})
        result = C.r6_choice(self.steps(), new, (1.0, 0.1), (1.0, 0.1))
        self.assertEqual(result["choice"], "0.15.3")
        self.assertTrue(any("failed on 0.16.0" in r for r in result["reasons"]))

    def test_missing_step_keeps_the_old_version(self):
        new = self.steps()
        del new["mgxs"]
        self.assertEqual(C.r6_choice(self.steps(), new, (1.0, 0.1), (1.0, 0.1))["choice"], "0.15.3")

    def test_flux_disagreement_keeps_the_old_version(self):
        result = C.r6_choice(self.steps(), self.steps(), (1.0, 0.05), (1.5, 0.05))
        self.assertEqual(result["choice"], "0.15.3")
        self.assertTrue(result["rule1_steps_clean"])
        self.assertFalse(result["rule2_flux_agrees"])

    def test_inputs_are_not_modified(self):
        a, b = self.steps(), self.steps()
        before = copy.deepcopy((a, b))
        C.r6_choice(a, b, (1.0, 0.1), (1.0, 0.1))
        self.assertEqual((a, b), before)


if __name__ == "__main__":
    unittest.main()


class ResponseUnits(unittest.TestCase):
    def test_per_source_to_rate_changes_the_unit(self):
        q = C.quantity(9.6e-4, C.UNIT_PER_SOURCE, std_error=3e-4)
        r = C.to_rate(q)
        self.assertEqual(r["unit"], C.UNIT_RATE)
        self.assertAlmostEqual(r["value"] / q["value"], 1.8618e20, delta=1e16)
        self.assertEqual(r["factor_n_per_s"], C.SOURCE_RATE_N_S)

    def test_rate_cannot_be_multiplied_again(self):
        r = C.to_rate(C.quantity(1.0, C.UNIT_PER_SOURCE))
        with self.assertRaises(ValueError):
            C.to_rate(r)

    def test_unlabelled_unit_is_refused(self):
        with self.assertRaises(ValueError):
            C.to_rate(C.quantity(1.0, "cm"))
        with self.assertRaises(ValueError):
            C.to_per_source(C.quantity(1.0, C.UNIT_PER_SOURCE))

    def test_round_trip(self):
        q = C.quantity(680.4, C.UNIT_PER_SOURCE)
        self.assertAlmostEqual(C.to_per_source(C.to_rate(q))["value"], 680.4, places=9)

    def test_source_rate_matches_the_spec(self):
        self.assertAlmostEqual(C.SOURCE_RATE_N_S / S.SOURCE_RATE_N_S, 1.0, places=12)


class R1Rules(unittest.TestCase):
    def _rec(self, mean, se, t, b=None):
        return {"response": {"a": {"mean": mean, "std_error": se, "relative_error": se / mean}, "seconds_to_statepoint": t,
                             "histories": 1000, "b": b}}

    def test_seeds_are_disjoint(self):
        self.assertTrue(R1.disjoint_seeds())

    def test_six_declared_configurations(self):
        self.assertEqual(sorted(R1.CONFIGS), ["C1", "C2", "C3", "C4", "C5", "C6"])
        self.assertEqual(R1.CONFIGS["C5"]["upper_lower_ratio"], 10.0)
        self.assertEqual(R1.CONFIGS["C5"]["survival_ratio"], 5.0)
        self.assertEqual([k for k in R1.CONFIGS if R1.objective_includes_b(k)], ["C4", "C6"])

    def test_fast_response_edges_follow_group_edges(self):
        self.assertEqual(R1.fast_response_edges("CASMO-8"), [821000.0, 20000000.0])
        self.assertEqual(R1.fast_response_edges("CASMO-25")[0], 111000.0)
        self.assertEqual(len(R1.fast_response_edges("1")), 2)

    def test_peak_mesh_has_one_phi_bin_per_coil(self):
        g = R1.peak_mesh_grids()
        self.assertEqual(len(g["coil_phi_bins"]), 18)
        self.assertEqual(len(g["phi_grid_deg"]), 36)
        self.assertEqual(g["phi_grid_deg"][:2], [2.0, 18.0])
        self.assertEqual(g["r_grid_cm"][1] - g["r_grid_cm"][0], 5.0)
        self.assertEqual(g["z_grid_cm"][1] - g["z_grid_cm"][0], 10.0)

    def test_fom_gain_and_lower_bound(self):
        self.assertAlmostEqual(R1.figure_of_merit(0.1, 100.0), 1.0)
        out = R1.summarise({"C2": self._rec(1.0, 0.01, 100.0)}, self._rec(1.0, 0.3, 7200.0))
        self.assertTrue(out["analog"]["gain_is_lower_bound"])
        self.assertEqual(out["analog"]["unbiasedness"], "NOT_EVALUATED")
        self.assertAlmostEqual(out["configs"]["C2"]["gain_vs_analog"], (1 / (0.01**2 * 100)) / (1 / (0.3**2 * 7200)), places=6)
        self.assertEqual(out["verdict"]["verdict"], "PASS")

    def test_fail_below_target(self):
        out = R1.summarise({"C1": self._rec(1.0, 0.2, 1800.0)}, self._rec(1.0, 0.3, 7200.0))
        self.assertEqual(out["verdict"]["verdict"], "FAIL")

    def test_analog_counts_when_r_is_small(self):
        out = R1.summarise({"C1": self._rec(1.0, 0.2, 1800.0)}, self._rec(1.0, 0.05, 7200.0))
        self.assertFalse(out["analog"]["gain_is_lower_bound"])
        self.assertEqual(out["analog"]["unbiasedness"], "evaluated")

    def test_z_score(self):
        self.assertAlmostEqual(R1.z_score(2.0, 0.3, 1.0, 0.4), 2.0)
