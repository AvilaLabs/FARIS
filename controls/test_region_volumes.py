import copy
from decimal import Decimal
import importlib.util
import math
from pathlib import Path
import random
import unittest

ROOT = Path(__file__).resolve().parents[1]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


CHECK = load_module("check_transport_arithmetic", Path(__file__).with_name("check_transport_arithmetic.py"))
WORKER = load_module("reactor_transport", ROOT / "integrations/openmc/reactor_transport.py")

R0, A, B, W = 3.3, 2.12, 2.28, 0.1745
INBOARD = {"kind": "inboard_half"}
OUTBOARD = {"kind": "outboard_half"}
OUTBOARD_EX = {"kind": "outboard_half", "excluding_sector_half_width_rad": W}
SECTOR = {"kind": "port_sector", "half_width_rad": W}
PARTITION = [INBOARD, OUTBOARD_EX, SECTOR]


def quadrature(outboard, dphi, n=600):
    """Midpoint rule for int (R0 + r cos t) r dr dt over a half annulus, times dphi."""
    lo, hi = (-math.pi / 2, math.pi / 2) if outboard else (math.pi / 2, 3 * math.pi / 2)
    dr, dt = (B - A) / n, (hi - lo) / n
    total = 0.0
    for i in range(n):
        r = A + (i + 0.5) * dr
        for j in range(n):
            total += (R0 + r * math.cos(lo + (j + 0.5) * dt)) * r * dr * dt
    return total * dphi


class RegionVolumeTests(unittest.TestCase):
    def test_worker_and_control_formulas_agree_to_double_precision(self):
        for region in [INBOARD, OUTBOARD, OUTBOARD_EX, SECTOR]:
            worker = WORKER.torus_region_volume_m3(R0, A, B, region)
            control = CHECK.region_volume_m3(Decimal(str(R0)), Decimal(str(A)), Decimal(str(B)), region)
            self.assertAlmostEqual(worker / float(control), 1.0, places=13)

    def test_regions_sum_to_the_full_torus(self):
        full = 2 * math.pi**2 * R0 * (B * B - A * A)
        halves = WORKER.torus_region_volume_m3(R0, A, B, INBOARD) + WORKER.torus_region_volume_m3(R0, A, B, OUTBOARD)
        self.assertAlmostEqual(halves / full, 1.0, places=14)
        three = sum(WORKER.torus_region_volume_m3(R0, A, B, r) for r in PARTITION)
        self.assertAlmostEqual(three / full, 1.0, places=14)
        self.assertGreater(WORKER.torus_region_volume_m3(R0, A, B, OUTBOARD), WORKER.torus_region_volume_m3(R0, A, B, INBOARD))

    def test_formulas_match_numerical_quadrature(self):
        self.assertAlmostEqual(WORKER.torus_region_volume_m3(R0, A, B, INBOARD) / quadrature(False, 2 * math.pi), 1.0, places=5)
        self.assertAlmostEqual(WORKER.torus_region_volume_m3(R0, A, B, OUTBOARD) / quadrature(True, 2 * math.pi), 1.0, places=5)
        self.assertAlmostEqual(WORKER.torus_region_volume_m3(R0, A, B, SECTOR) / quadrature(True, 2 * W), 1.0, places=5)

    def test_membership_samples_reproduce_analytic_fractions(self):
        # Independent of the formulas: uniform points of the torus shell are
        # classified by position; the fractions match the analytic volumes.
        rng = random.Random(20261005)
        n = 120_000
        counts = [0, 0, 0]
        hits = 0
        full = 2 * math.pi**2 * R0 * (B * B - A * A)
        while hits < n:
            # Rejection from the torus bounding box in (R, y).
            rho = rng.uniform(R0 - B, R0 + B)
            y = rng.uniform(-B, B)
            if not (A**2 <= (rho - R0) ** 2 + y * y <= B**2):
                continue
            # Weight by R for the toroidal volume element: accept with prob R/(R0+B).
            if rng.random() * (R0 + B) > rho:
                continue
            phi = rng.uniform(-math.pi, math.pi)
            x, z = 100 * rho * math.cos(phi), 100 * rho * math.sin(phi)
            hits += 1
            for index, region in enumerate(PARTITION):
                inside = WORKER.region_contains(region, 100 * R0, x, z)
                counts[index] += inside
            self.assertEqual(sum(WORKER.region_contains(r, 100 * R0, x, z) for r in PARTITION), 1)
        for count, region in zip(counts, PARTITION):
            expected = WORKER.torus_region_volume_m3(R0, A, B, region) / full
            sigma = math.sqrt(expected * (1 - expected) / n)
            self.assertLess(abs(count / n - expected), 5 * sigma, region)

    def test_sector_is_symmetric_and_excludes_the_far_side(self):
        for sign in (1, -1):
            self.assertTrue(WORKER.region_contains(SECTOR, 330, 550, sign * 50))
            self.assertFalse(WORKER.region_contains(SECTOR, 330, 550, sign * 150))
        self.assertFalse(WORKER.region_contains(SECTOR, 330, -550, 0))
        self.assertFalse(WORKER.region_contains(SECTOR, 330, 200, 0))
        self.assertTrue(WORKER.region_contains(INBOARD, 330, 200, 0))
        self.assertTrue(WORKER.region_contains(OUTBOARD, 330, -550, 0))
        self.assertFalse(WORKER.region_contains(OUTBOARD_EX, 330, 550, 50))


def synthetic_run(perforated=False, whole_fast=None):
    """Smallest input/artifact/results triple the region check reads."""
    full = {r["kind"] + str(r.get("excluding_sector_half_width_rad")): CHECK.region_volume_m3(
        Decimal(str(R0)), Decimal(str(A)), Decimal(str(B)), r) for r in PARTITION}
    values = {
        "inboard": float(full["inboard_halfNone"]),
        "outboard": float(full["outboard_half" + str(W)]),
        "port": float(full["port_sector" + "None"]),
    }
    domain = lambda region: {"kind": "component_region", "component_id": "magnets", "region": region}
    domains = {"inboard": domain(INBOARD), "outboard": domain(OUTBOARD_EX), "port": domain(SECTOR)}
    component = {"kind": "component", "component_id": "magnets"}
    removed = 0.01 if perforated else 0.0
    values["port"] -= removed
    raw_means = {"inboard": 1.0, "outboard": 2.0, "port": 4.0}
    whole = sum(raw_means.values()) if whole_fast is None else whole_fast
    volumes = [{"domain": component, "value": sum(values.values()) * 1e6, "standard_error": 0.0, "unit": "cubic_centimetre"}]
    volumes += [{"domain": d, "value": values[k] * 1e6, "standard_error": 0.0, "unit": "cubic_centimetre"} for k, d in domains.items()]
    tallies = [{"response_id": "magnets-fast-flux", "mean": whole}]
    tallies += [{"response_id": f"magnets-{k}-fast-flux", "mean": raw_means[k]} for k in domains]
    tallies += [{"response_id": "magnets-flux", "mean": 100.0}]
    fast = {"kind": "flux_above", "energy_min_ev": 1e5}
    results = {"magnets-fast-flux": {"response_id": "magnets-fast-flux", "domain": component, "score": fast}}
    results.update({f"magnets-{k}-fast-flux": {"response_id": f"magnets-{k}-fast-flux", "domain": d, "score": fast} for k, d in domains.items()})
    results["magnets-flux"] = {"response_id": "magnets-flux", "domain": component, "score": {"kind": "flux"}}
    penetration = {"bounds_m": {"minimum_xyz_m": [4.34, -0.15, -0.15], "maximum_xyz_m": [5.59, 0.15, 0.15]},
                   "affected_component_ids": ["magnets"]} if perforated else None
    inp = {"manifest": {"penetration": penetration}}
    variant = {"components": [{"id": "magnets", "inner_minor_radius_m": str(A), "outer_minor_radius_m": str(B)}]}
    return inp, {"volumes": volumes, "tallies": tallies}, variant, results


def run_check(inp, raw, variant, results):
    # Mirror the control's own loading: floats as Decimal (on copies).
    raw = copy.deepcopy(raw)
    raw = {"volumes": [{**v, "value": Decimal(str(v["value"])), "standard_error": Decimal(str(v["standard_error"]))} for v in raw["volumes"]],
           "tallies": [{**t, "mean": Decimal(str(t["mean"]))} for t in raw["tallies"]]}
    for item in raw["volumes"]:
        region = item["domain"].get("region")
        if region:
            for key in ("half_width_rad", "excluding_sector_half_width_rad"):
                if key in region:
                    region[key] = Decimal(str(region[key]))
    volumes = {CHECK.domain_key(v["domain"]): v["value"] * Decimal("0.000001") for v in raw["volumes"]}
    return CHECK.check_regions(inp, raw, variant, Decimal(str(R0)), volumes, results)


class RegionControlTests(unittest.TestCase):
    def test_unperforated_regions_partition_and_add(self):
        report = run_check(*synthetic_run())
        self.assertLess(Decimal(report["max_partition_volume_difference"]), Decimal("1e-12"))
        self.assertLess(Decimal(report["max_fast_flux_additivity_difference"]), Decimal("1e-12"))
        self.assertEqual(len(report["regions"]), 3)

    def test_perforated_port_sector_reports_removed_volume(self):
        inp, raw, variant, results = synthetic_run(perforated=True)
        report = run_check(inp, raw, variant, results)
        removed = [Decimal(v["removed_or_difference"]) for v in report["regions"].values()]
        self.assertEqual(sorted(removed)[-1].quantize(Decimal("1e-9")), Decimal("0.01"))
        # The partition no longer matches an unperforated component volume,
        # but the three region volumes sum to the perforated component volume.
        self.assertIsNotNone(report["max_partition_volume_difference"])

    def test_wrong_region_volume_or_flux_sum_is_rejected(self):
        inp, raw, variant, results = synthetic_run()
        bad = copy.deepcopy(raw)
        bad["volumes"][1]["value"] *= 1.001
        with self.assertRaises(ValueError):
            run_check(inp, bad, variant, results)
        with self.assertRaises(ValueError):
            run_check(*synthetic_run(whole_fast=8.0))
        # A fast flux above the energy-integrated flux is impossible.
        inp, raw, variant, results = synthetic_run()
        for tally in raw["tallies"]:
            if tally["response_id"] == "magnets-flux":
                tally["mean"] = 1.0
        with self.assertRaises(ValueError):
            run_check(inp, raw, variant, results)


if __name__ == "__main__":
    unittest.main()
