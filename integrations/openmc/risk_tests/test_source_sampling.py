"""Sampling tests for ../source_sites.py: do sampled source positions follow the analytic profile?

The sites come from the real path (`openmc-plasma-source`'s `tokamak_source` sampled through
OpenMC's own `sample_external_source`, as `faris design check` does for check 9). They are
binned in (R, Z) on edges that coincide with the source mesh's voxel edges and compared by
chi-square with the analytic emission of the same profiles, integrated on a fine (a, alpha)
grid without the package's mesh. The tests skip, with the reason, where numpy, OpenMC or the
package is absent; run them with the OpenMC 0.15.3 environment's python and the package on
PYTHONPATH:

    PYTHONPATH=<dir holding openmc_plasma_source> python3 -m unittest test_source_sampling
"""

import importlib
import json
import math
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

try:
    import numpy as np
    import openmc  # noqa: F401
    import openmc.lib  # noqa: F401
    from openmc_plasma_source.fuel_types import get_reactions_from_fuel
    import source_sites as SS

    tokamak = importlib.import_module("openmc_plasma_source.tokamak_source")
    SKIP = None
except Exception as error:  # numpy, OpenMC or openmc_plasma_source missing
    SKIP = f"numpy, OpenMC and openmc_plasma_source are needed for the real sampling path ({error})"

RM_M = Path(__file__).resolve().parents[3] / "scenarios" / "rm-m" / "rm-m.faris-design.json"
SITES = 100_000
SEED = 1
# The strength of the D-T reactions alone, tokamak_source's 100 x 100 mesh over the 500 x 500
# (a, alpha) grid; the Rust integral in crates/faris-engine/src/design/source.rs gives 6.757372e19.
RM_M_DT_RATE_PER_S = 6.757372e19


def chi2_survival(chi2, dof):
    """P(X >= chi2) for X chi-square with `dof` degrees of freedom: the regularised upper
    incomplete gamma Q(dof / 2, chi2 / 2), series below the mean and continued fraction above."""
    a, x = dof / 2.0, chi2 / 2.0
    if x <= 0.0:
        return 1.0
    log_prefix = -x + a * math.log(x) - math.lgamma(a)
    if x < a + 1.0:
        term = total = 1.0 / a
        n = a
        for _ in range(10_000):
            n += 1.0
            term *= x / n
            total += term
            if abs(term) < abs(total) * 1e-16:
                break
        return 1.0 - total * math.exp(log_prefix)
    tiny = 1e-300
    b = x + 1.0 - a
    c = 1.0 / tiny
    d = 1.0 / b
    h = d
    for i in range(1, 10_000):
        an = -i * (i - a)
        b += 2.0
        d = an * d + b
        d = tiny if abs(d) < tiny else d
        c = b + an / c
        c = tiny if abs(c) < tiny else c
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < 1e-16:
            break
    return math.exp(log_prefix) * h


def edges_of(plasma, groups):
    """Coarse (R, Z) bin edges: every (100 / groups)-th edge of the source mesh."""
    a0, shift = plasma["minor_radius_m"], abs(plasma["shafranov_shift_m"])
    r0, kappa = plasma["major_radius_m"], plasma["elongation"]
    step = 100 // groups
    return (np.linspace(r0 - a0 - shift, r0 + a0 + shift, 101)[::step],
            np.linspace(-kappa * a0, kappa * a0, 101)[::step])


def analytic_fractions(plasma, redges, zedges, cells=1500):
    """Fraction of all neutron emission in each (R, Z) bin: the package's profile and reaction
    definitions, integrated over R |J| da dalpha on a midpoint grid with no mesh."""
    a0, esh, r0 = plasma["minor_radius_m"] * 100.0, plasma["shafranov_shift_m"] * 100.0, plasma["major_radius_m"] * 100.0
    kappa, delta, pedestal = plasma["elongation"], plasma["triangularity"], plasma["pedestal_radius_m"] * 100.0
    a = (np.arange(cells) + 0.5) * a0 / cells
    alpha = (np.arange(cells) + 0.5) * 2.0 * np.pi / cells
    a, alpha = (x.ravel() for x in np.meshgrid(a, alpha, indexing="ij"))
    density, temperature = plasma["ion_density_m3"], plasma["ion_temperature_ev"]
    n = tokamak.tokamak_ion_density(
        plasma["mode"], density["centre"], density["peaking_factor"], density["pedestal"], a0, pedestal,
        density["separatrix"], a)
    t_ev = tokamak.tokamak_ion_temperature(
        a, plasma["mode"], pedestal, temperature["pedestal"] / 1e3, temperature["centre"] / 1e3, temperature["beta"],
        temperature["peaking_factor"], temperature["separatrix"] / 1e3, a0)
    theta = alpha + delta * np.sin(alpha)
    radius = r0 + a * np.cos(theta) + esh * (1.0 - (a / a0) ** 2)
    height = kappa * a * np.sin(alpha)
    jacobian = np.abs((np.cos(theta) - 2.0 * esh * a / a0 ** 2) * kappa * a * np.cos(alpha)
                      + a * np.sin(theta) * (1.0 + delta * np.cos(alpha)) * kappa * np.sin(alpha))
    fuel = plasma["fuel"]
    species = {key: n * value for key, value in fuel.items()}
    rate = np.zeros_like(a)
    for reaction in get_reactions_from_fuel(fuel):
        pairs = {"DD": 0.5 * species.get("D", 0) ** 2, "TT": 0.5 * species.get("T", 0) ** 2,
                 "DT": species.get("T", 0) * species.get("D", 0)}[reaction]
        rate += tokamak.tokamak_neutron_source_density(pairs, t_ev, reaction) * (2 if reaction == "TT" else 1)
    weights = rate * radius * jacobian
    hist, _, _ = np.histogram2d(radius * 0.01, height * 0.01, bins=[redges, zedges], weights=weights)
    return hist / hist.sum()


def sample_positions(plasma):
    import contextlib
    import io

    timing = {}
    with contextlib.redirect_stdout(io.StringIO()):
        points, _ = SS.sample({"plasma": plasma, "count": SITES, "seed": SEED}, timing)
    return np.hypot(points[:, 0], points[:, 1]), points[:, 2]


def chi_square(plasma, groups, analytic_plasma=None):
    """(chi2, dof, p) of the sampled sites against the analytic fractions, bins with an expected
    count of at least 5."""
    redges, zedges = edges_of(plasma, groups)
    radius, height = sample_positions(plasma)
    observed, _, _ = np.histogram2d(radius, height, bins=[redges, zedges])
    expected = analytic_fractions(analytic_plasma or plasma, redges, zedges) * observed.sum()
    use = expected >= 5.0
    chi2 = float((((observed[use] - expected[use]) ** 2) / expected[use]).sum())
    dof = int(use.sum()) - 1
    return chi2, dof, chi2_survival(chi2, dof)


def rm_m_plasma():
    return json.loads(RM_M.read_text())["plasma"]


def shifted_l_mode_plasma():
    """A second shape: L mode, negative triangularity, a 0.2 m outward shift, a different
    elongation and strongly peaked profiles."""
    plasma = rm_m_plasma()
    plasma.update(mode="L", triangularity=-0.3, shafranov_shift_m=0.2, elongation=1.5, pedestal_radius_m=1.0)
    plasma["ion_density_m3"].update(centre=1.2e20, peaking_factor=1.2)
    plasma["ion_temperature_ev"].update(centre=15000.0, peaking_factor=2.0)
    return plasma


class Chi2Survival(unittest.TestCase):
    def test_known_values(self):
        # scipy.stats.chi2.sf reference values
        self.assertAlmostEqual(chi2_survival(3.841458820694124, 1), 0.05, places=9)
        self.assertAlmostEqual(chi2_survival(18.307038053275146, 10), 0.05, places=9)
        self.assertAlmostEqual(chi2_survival(124.34211340400407, 100), 0.05, places=8)
        self.assertAlmostEqual(chi2_survival(79.0, 79), 0.4788384304, places=9)


@unittest.skipIf(SKIP, SKIP or "")
class SampledProfile(unittest.TestCase):
    def check_closes(self, plasma, groups):
        chi2, dof, p = chi_square(plasma, groups)
        print(f"\n  chi-square {chi2:.2f} on {dof} dof, p = {p:.4f} ({groups * groups} bins)")
        self.assertGreater(p, 0.001, f"chi-square {chi2:.1f} on {dof} dof")

    # Verifies: SRC-011
    def test_rm_m_sites_follow_the_analytic_profile(self):
        for groups in (5, 10):
            self.check_closes(rm_m_plasma(), groups)

    # Verifies: SRC-011
    def test_a_shifted_negative_triangularity_l_mode_follows_its_profile(self):
        self.check_closes(shifted_l_mode_plasma(), 10)

    # Verifies: SRC-011
    def test_the_test_can_tell_a_wrong_shape_from_the_right_one(self):
        # Sites sampled with triangularity 0.375 against an analytic profile of the same plasma
        # with triangularity 0, shift 0.1 m or a 5 % lower elongation: each must be rejected.
        base = rm_m_plasma()
        for change in ({"triangularity": 0.0}, {"shafranov_shift_m": 0.1}, {"elongation": 1.84 * 0.95}):
            wrong = dict(base, **change)
            # the analytic binning uses the sampled plasma's edges, so keep them
            redges, zedges = edges_of(base, 10)
            radius, height = sample_positions(base)
            observed, _, _ = np.histogram2d(radius, height, bins=[redges, zedges])
            expected = analytic_fractions(wrong, redges, zedges) * observed.sum()
            use = expected >= 5.0
            chi2 = float((((observed[use] - expected[use]) ** 2) / expected[use]).sum())
            self.assertLess(chi2_survival(chi2, int(use.sum()) - 1), 1e-6, f"{change}")

    # Verifies: SRC-010
    def test_the_package_strength_for_dt_matches_the_rust_integral(self):
        import openmc.stats

        spectrum = tokamak.get_neutron_energy_distribution
        reactions = tokamak.get_reactions_from_fuel
        tokamak.get_neutron_energy_distribution = lambda **_: openmc.stats.Discrete([14.1e6], [1.0])
        tokamak.get_reactions_from_fuel = lambda fuel: ["DT"]
        try:
            source = tokamak.tokamak_source(**SS.tokamak_arguments(rm_m_plasma()))
        finally:
            tokamak.get_neutron_energy_distribution = spectrum
            tokamak.get_reactions_from_fuel = reactions
        # The package counts neutrons per second; one per D-T reaction.
        self.assertLess(abs(source.strength / RM_M_DT_RATE_PER_S - 1.0), 1e-3)


if __name__ == "__main__":
    unittest.main()
