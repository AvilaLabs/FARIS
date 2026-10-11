"""Tests for the S0e rules, mesh and breakdown analysis (s0e_common, s0e_analyse, s0e_run helpers).

Standard library only; no OpenMC or numpy. Run from the repository root:
    python3 -m unittest discover -s integrations/openmc/risk_tests -p 'test_*.py'
"""

import math
import unittest

import r1_configs as R1
import s0c_common as S0C
import s0d_common as S0D
import s0e_analyse as A
import s0e_check_windows as CW
import s0e_common as S
import s0e_run as RUN


def row(gain, sigma, unbiased=True):
    return {"gain": gain, "sigma": sigma, "unbiased": unbiased}


class Seeds(unittest.TestCase):
    def test_seeds_match_the_protocol(self):
        self.assertEqual(S.SEEDS["gen"], {"smoke": 90130000, "E2": 90130002})
        self.assertEqual(S.SEEDS["prod"], {"E0": 90140000, "E1": 90140001, "E2": 90140002, "E3": 90140003})

    def test_seeds_are_disjoint_from_earlier_stages(self):
        s0e = set(S.SEEDS["gen"].values()) | set(S.SEEDS["prod"].values())
        s0b = set(R1.S0B_GENERATION_SEEDS.values()) | set(R1.S0B_PRODUCTION_SEEDS.values()) | {R1.S0B_MGXS_SEED}
        s0c = {v for v in S0C.SEEDS.values() if isinstance(v, int)}
        for v in S0C.SEEDS.values():
            if isinstance(v, dict):
                s0c |= set(v.values())
        s0d = set(S0D.SEEDS["gen"].values()) | set(S0D.SEEDS["prod"].values())
        for earlier in (s0b, s0c, s0d):
            self.assertFalse(s0e & earlier)
        self.assertEqual(len(s0e), len(S.SEEDS["gen"]) + len(S.SEEDS["prod"]))


class Mesh(unittest.TestCase):
    def setUp(self):
        self.e = S.cylindrical_edges()

    def test_shape(self):
        # The protocol text says "52 radial bins" but its edge list has 52 edges, hence 51 bins (see the report).
        self.assertEqual(len(self.e["r_cm"]), 52)
        self.assertEqual(S.mesh_shape(self.e), (51, 32, 72))

    def test_named_edges_are_present(self):
        for edge in (129, 180, 217, 443, 581, 645, 183, 186, 206, 213, 214, 446, 447, 454, 554, 557, 560, 580, 670, 700):
            self.assertTrue(any(abs(x - edge) < 1e-9 for x in self.e["r_cm"]), edge)

    def test_equal_cells(self):
        r = self.e["r_cm"]
        i = r.index(129.0)
        steps = [r[i + k + 1] - r[i + k] for k in range(10)]
        self.assertTrue(all(abs(s - 5.1) < 1e-9 for s in steps))
        self.assertAlmostEqual(r[r.index(217.0) + 1] - 217.0, 226.0 / 9.0)
        self.assertAlmostEqual(r[r.index(581.0) + 1] - 581.0, 16.0)

    def test_increasing_and_ranges(self):
        for axis, v in self.e.items():
            self.assertTrue(all(b > a for a, b in zip(v, v[1:])), axis)
        self.assertEqual((self.e["r_cm"][0], self.e["r_cm"][-1]), (0.0, 700.0))
        self.assertEqual((self.e["z_cm"][0], self.e["z_cm"][-1]), (-400.0, 400.0))
        self.assertAlmostEqual(self.e["phi_rad"][-1], 2.0 * math.pi)
        self.assertAlmostEqual(self.e["phi_rad"][1], math.radians(5.0))

    def test_hash_is_stable_and_sensitive(self):
        h = S.edges_sha256(self.e)
        self.assertEqual(len(h), 64)
        self.assertEqual(h, S.edges_sha256(S.cylindrical_edges()))
        changed = {k: list(v) for k, v in self.e.items()}
        changed["r_cm"][3] += 0.01
        self.assertNotEqual(h, S.edges_sha256(changed))

    def test_window_file_comparison(self):
        self.assertTrue(CW.compare(self.e, S.cylindrical_edges())["all_equal"])
        bad = {k: list(v) for k, v in self.e.items()}
        bad["z_cm"][5] += 1.0
        out = CW.compare(bad, self.e)
        self.assertFalse(out["all_equal"])
        self.assertFalse(out["equal"]["z_cm"])
        self.assertFalse(CW.compare({**self.e, "r_cm": self.e["r_cm"][:-1]}, self.e)["equal"]["r_cm"])

    def test_map_edges(self):
        m = S.map_edges()
        self.assertEqual((len(m["r_cm"]) - 1, len(m["z_cm"]) - 1, len(m["phi_rad"]) - 1), (35, 40, 1))
        self.assertEqual((m["r_cm"][-1], m["z_cm"][0], m["z_cm"][-1]), (700.0, -400.0, 400.0))


class H5(unittest.TestCase):
    e0 = row(20.0, 1.5)

    def test_confirmed(self):
        self.assertEqual(S.h5_rule(self.e0, row(30.0, 2.0))["outcome"], "H5 confirmed")

    def test_rejected_when_within_3_sigma(self):
        self.assertEqual(S.h5_rule(self.e0, row(22.0, 1.5))["outcome"], "H5 rejected")

    def test_rejected_when_beyond_3_sigma_but_below_factor(self):
        # 3 sigma needs a difference over 3 * hypot(0.3, 0.3) = 1.27; 21.5 passes it but 21.5 / 20 < 1.2
        self.assertEqual(S.h5_rule(row(20.0, 0.3), row(21.5, 0.3))["outcome"], "H5 rejected")

    def test_rejected_when_factor_met_but_within_3_sigma(self):
        self.assertEqual(S.h5_rule(row(20.0, 5.0), row(25.0, 5.0))["outcome"], "H5 rejected")

    def test_rejected_below_e0(self):
        self.assertEqual(S.h5_rule(self.e0, row(10.0, 1.0))["outcome"], "H5 rejected")

    def test_biased(self):
        out = S.h5_rule(self.e0, row(60.0, 4.0, unbiased=False))
        self.assertEqual(out["outcome"], "biased")
        self.assertIn("fact 3", out["note"])

    def test_not_evaluated(self):
        self.assertEqual(S.h5_rule(None, row(30.0, 2.0))["outcome"], "NOT_EVALUATED")
        self.assertEqual(S.h5_rule(self.e0, None)["outcome"], "NOT_EVALUATED")
        self.assertEqual(S.h5_rule(self.e0, row(None, None))["outcome"], "NOT_EVALUATED")
        self.assertEqual(S.h5_rule(self.e0, row(30.0, 2.0, unbiased=None))["outcome"], "NOT_EVALUATED")


class H6(unittest.TestCase):
    e0 = row(20.0, 1.5)

    def test_confirmed(self):
        self.assertEqual(S.h6_rule(self.e0, row(35.0, 2.5))["outcome"], "H6 confirmed")

    def test_rejected(self):
        self.assertEqual(S.h6_rule(self.e0, row(21.0, 1.5))["outcome"], "H6 rejected")
        self.assertEqual(S.h6_rule(self.e0, row(19.0, 1.5))["outcome"], "H6 rejected")

    def test_biased_gain_does_not_confirm(self):
        out = S.h6_rule(self.e0, row(80.0, 5.0, unbiased=False))
        self.assertEqual(out["outcome"], "H6 rejected")

    def test_not_evaluated_without_a_gain(self):
        self.assertEqual(S.h6_rule(self.e0, row(None, None, None))["outcome"], "NOT_EVALUATED")
        self.assertEqual(S.h6_rule(self.e0, None)["outcome"], "NOT_EVALUATED")


class E3(unittest.TestCase):
    def test_only_when_both_confirmed(self):
        c5, c6 = {"outcome": "H5 confirmed"}, {"outcome": "H6 confirmed"}
        self.assertTrue(S.e3_required(c5, c6))
        self.assertFalse(S.e3_required(c5, {"outcome": "H6 rejected"}))
        self.assertFalse(S.e3_required({"outcome": "biased"}, c6))
        self.assertFalse(S.e3_required({"outcome": "NOT_EVALUATED"}, c6))
        self.assertFalse(S.e3_required(None, None))


class GStar(unittest.TestCase):
    def test_largest_unbiased(self):
        rows = {"E0": row(20.0, 1.5), "E1": row(90.0, 6.0, unbiased=False), "E2": row(45.0, 3.0), "E3": row(None, None, None)}
        out = S.g_star(rows)
        self.assertEqual((out["best"], out["g_star"]), ("E2", 45.0))
        self.assertTrue(out["nuc050"].startswith("10-100"))

    def test_bands(self):
        self.assertTrue(S.g_star({"E3": row(120.0, 8.0)})["nuc050"].startswith(">= 100"))
        self.assertTrue(S.g_star({"E0": row(8.0, 0.5)})["nuc050"].startswith("< 10"))
        out = S.g_star({"E0": row(5.0, 0.5, unbiased=False)})
        self.assertIsNone(out["best"])
        self.assertEqual(out["nuc050"], "NOT_EVALUATED")

    def test_repeatability(self):
        z = S.repeatability_z(row(20.0, 1.5), 19.65, 1.60)
        self.assertAlmostEqual(z, 0.35 / math.hypot(1.5, 1.6))
        self.assertIsNone(S.repeatability_z(None, 19.65, 1.6))
        self.assertIsNone(S.repeatability_z(row(20.0, 1.5), None, None))


class QuietMachine(unittest.TestCase):
    def test_load_parsing(self):
        self.assertEqual(S.load1("1.52 1.10 0.90 2/500 1234"), 1.52)
        self.assertIsNone(S.load1(""))

    def test_quiet_rule(self):
        self.assertTrue(S.quiet_now(1.99, True))
        self.assertFalse(S.quiet_now(2.0, True))
        self.assertFalse(S.quiet_now(0.5, False))
        self.assertFalse(S.quiet_now(None, True))

    def test_contended_note(self):
        self.assertTrue(S.contended("6.5 5.0 4.0 1/1 1"))
        self.assertFalse(S.contended("6.0 5.0 4.0 1/1 1"))
        self.assertFalse(S.contended(""))

    def test_constants(self):
        self.assertEqual((S.CAP_S, S.PRODUCTION_S, S.MAX_SPLIT, S.CUTOFF_EV), (4500.0, 600.0, 1000, 1.0e5))
        self.assertEqual(str(S.BUILD_LOCK), "/home/connoravila/.cache/avila-night/faris-wt/cargo.lock")


class Ledger(unittest.TestCase):
    def test_fits(self):
        self.assertTrue(S.fits(3000.0, 1200.0))
        self.assertFalse(S.fits(3400.0, 1200.0))

    def test_budget_counts_job_time_not_waiting(self):
        b = RUN.Budget(4500.0)
        b.add("E0", "produce", {"wall_s": 700.0, "wait": {"waited_s": 9000.0}})
        self.assertEqual(b.used(), 700.0)
        self.assertEqual(b.remaining(), 3800.0)
        self.assertEqual(b.timeout(5000.0), 3800.0)
        self.assertEqual(b.timeout(900.0), 900.0)

    def test_item_that_does_not_fit_is_not_evaluated_with_its_cost(self):
        b = RUN.Budget(4500.0)
        b.add("E0", "produce", {"wall_s": 3800.0})
        row_ = RUN.fit_or_skip("E2", "E2", b)
        self.assertIsNone(row_["gain"])
        self.assertTrue(row_["note"].startswith("NOT_EVALUATED"))
        self.assertIn("3800", row_["note"])
        self.assertIsNone(RUN.fit_or_skip("E0", "E0", RUN.Budget(4500.0)))

    def test_smoke_pass_criteria(self):
        ok_check = {"passed": True}
        ok_rec = {"response": {}, "lost": {"restart_files": 0}}
        self.assertTrue(RUN.smoke_passed({"returncode": 0}, ok_check, {"returncode": 0}, ok_rec))
        self.assertFalse(RUN.smoke_passed({"returncode": 0}, {"passed": False}, {"returncode": 0}, ok_rec))
        self.assertFalse(RUN.smoke_passed({"returncode": 1}, ok_check, {"returncode": 0}, ok_rec))
        self.assertFalse(RUN.smoke_passed({"returncode": 0}, ok_check, {"returncode": 0},
                                          {"response": {}, "lost": {"restart_files": 2}}))
        self.assertFalse(RUN.smoke_passed({"returncode": 0}, ok_check, {"returncode": 0}, {"lost": {"restart_files": 0}}))

    def test_production_argv(self):
        from pathlib import Path
        argv = RUN.production_argv("E1", Path("/x"), Path("/w.h5"), True)
        self.assertIn("--breakdown", argv)
        self.assertEqual(argv[argv.index("--cutoff-ev") + 1], "100000")
        self.assertEqual(argv[argv.index("--seed") + 1], "90140001")
        self.assertNotIn("--cutoff-ev", RUN.production_argv("E0", Path("/x"), Path("/w.h5"), False))

    def test_summary_applies_rules(self):
        b = RUN.Budget(4500.0)
        rows = {"E0": row(20.0, 1.5), "E1": row(30.0, 2.0), "E2": row(35.0, 2.5)}
        out = RUN.summarise(rows, b, {})
        self.assertEqual(out["rule"]["H5"]["outcome"], "H5 confirmed")
        self.assertEqual(out["rule"]["H6"]["outcome"], "H6 confirmed")
        self.assertTrue(out["rule"]["E3_required"])
        self.assertEqual(out["rule"]["best"], "E2")


def synthetic_breakdown(boost_port=2.0):
    """18 coils (port-adjacent ones scaled) and a 35 x 40 map whose total matches the coils' total."""
    centres = [10.0 + 20.0 * k for k in range(18)]
    coils = []
    for k in range(18):
        mean = 1.0e-6 * (boost_port if centres[k] in (10.0, 350.0) else 1.0)
        coils.append({"name": f"tf_coil_{k:02d}", "volume_id": 13 + k, "mean": mean, "std_error": 0.1 * mean})
    e = S.map_edges()
    nr, nz = len(e["r_cm"]) - 1, len(e["z_cm"]) - 1
    mean, sd = [0.0] * (nr * nz), [0.0] * (nr * nz)
    # inboard bin (R 100-120 -> index 5, Z 0-20 -> 20), outboard (R 600-620 -> 30, Z -20-0 -> 19), top (R 300-320, Z 200-220)
    for (ir, iz, m, s) in ((5, 20, 4.0, 0.4), (30, 19, 2.0, 0.2), (15, 30, 1.0, 0.9), (15, 9, 3.0, 0.1)):
        mean[iz * nr + ir], sd[iz * nr + ir] = m, s
    return centres, coils, {"r_edges_cm": e["r_cm"], "z_edges_cm": e["z_cm"], "mean": mean, "std_error": sd}


class Analysis(unittest.TestCase):
    def test_coil_table_and_port_comparison(self):
        centres, coils, _ = synthetic_breakdown(2.0)
        t = A.coil_table(coils, centres)
        self.assertEqual(t["port_adjacent"], ["tf_coil_00", "tf_coil_17"])
        self.assertAlmostEqual(math.fsum(r["share"] for r in t["coils"]), 1.0)
        self.assertAlmostEqual(t["ratio_port_adjacent_to_other"], 2.0)
        self.assertAlmostEqual(t["coils"][0]["relative_error"], 0.1)
        self.assertAlmostEqual(math.fsum(r["variance_share"] for r in t["coils"]), 1.0)

    def test_regions_and_variance(self):
        _, _, rz = synthetic_breakdown()
        t = A.map_table(rz)
        reg = t["regions"]
        self.assertAlmostEqual(reg["inboard"]["share"], 4.0 / 10.0)
        self.assertAlmostEqual(reg["outboard"]["share"], 2.0 / 10.0)
        self.assertAlmostEqual(reg["top"]["share"], 1.0 / 10.0)
        self.assertAlmostEqual(reg["bottom"]["share"], 3.0 / 10.0)
        self.assertAlmostEqual(sum(v["share"] for v in reg.values()), 1.0)
        self.assertAlmostEqual(sum(v["variance_share"] for v in reg.values()), 1.0)
        # variances: 0.16, 0.04, 0.81, 0.01 -> total 1.02; the top bin (sigma 0.9) leads
        first = t["largest_variance_bins"][0]
        self.assertEqual(first["region"], "top")
        self.assertAlmostEqual(first["variance_share"], 0.81 / 1.02)

    def test_region_edges(self):
        self.assertEqual(A.region_of(120, 140, -20, 0), "inboard")   # straddles 129: inboard whole
        self.assertEqual(A.region_of(140, 160, 0, 20), "top")
        self.assertEqual(A.region_of(140, 160, -20, 0), "bottom")
        self.assertEqual(A.region_of(580, 600, 0, 20), "outboard")
        self.assertEqual(A.region_of(560, 580, 0, 20), "top")

    def test_full_analysis_and_markdown(self):
        centres, coils, rz = synthetic_breakdown()
        roles = [{"name": f"tf_coil_{k:02d}", "centroid_cm": [100.0 * math.cos(math.radians(c)), 100.0 * math.sin(math.radians(c)), 0.0]}
                 for k, c in enumerate(centres)]
        e0 = {"label": "S0e-E0", "response": {"a": {"mean": math.fsum(c["mean"] for c in coils)}}, "breakdown": {"per_coil": coils, "rz_map": rz}}
        out = A.analyse(e0, roles, centres)
        self.assertTrue(out["angles"]["mapping_confirmed"])
        self.assertAlmostEqual(out["coil_sum_vs_response_a"], 1.0)
        text = A.markdown(out)
        self.assertIn("tf_coil_17", text)
        self.assertIn("covariance", text)
        with self.assertRaises(ValueError):
            A.analyse({"label": "x"}, roles, centres)

    def test_wrong_angle_mapping_is_flagged(self):
        centres, _, _ = synthetic_breakdown()
        roles = [{"name": f"tf_coil_{k:02d}", "centroid_cm": [100.0, 0.0, 0.0]} for k in range(18)]
        self.assertFalse(A.coil_angles(roles, centres)["mapping_confirmed"])

    def test_map_size_mismatch(self):
        with self.assertRaises(ValueError):
            A.map_table({"r_edges_cm": [0, 1, 2], "z_edges_cm": [0, 1], "mean": [1.0], "std_error": [1.0]})


if __name__ == "__main__":
    unittest.main()
