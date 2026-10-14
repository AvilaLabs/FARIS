"""Seeded unit-slip test of the independent normalization control (NUC-003, VAL-015).

A synthetic run directory is written with every normalized value computed here, in
50-digit decimals, from the raw per-source tallies. The control must pass it, and it
must reject each of more than twenty deliberate unit and normalization slips (per cm^3
read as per m^3, a missing source rate, a lost 0.01 for cm to m, and so on). It
exercises the arithmetic of the control on supplied values; it says nothing about a
real transport run.
"""
import copy
from decimal import Decimal, getcontext
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

getcontext().prec = 50


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


CHECK = load_module("check_transport_arithmetic", Path(__file__).with_name("check_transport_arithmetic.py"))
PI = CHECK.PI
E = CHECK.ELEMENTARY_CHARGE
POWER_MW = Decimal("500")
ENERGY_EV = Decimal("17600000")
RATE = POWER_MW * Decimal(1000000) / (ENERGY_EV * E)
MAJOR = Decimal("6.0")
BLANKET = (Decimal("1.0"), Decimal("1.5"))
MAGNETS = (Decimal("1.6"), Decimal("1.8"))
MESH = {"lower_left_m": [5.0, -0.5, -0.5], "upper_right_m": [7.0, 0.5, 0.5], "dimensions": [4, 2, 2]}
FACTORS = {"cm_per_source": Decimal("0.01"), "particles_per_source": Decimal(1),
           "events_per_source": Decimal(1), "ev_per_source": E}
BLANKET_DOMAIN = {"kind": "component", "component_id": "blanket"}
MAGNETS_DOMAIN = {"kind": "component", "component_id": "magnets"}
WHOLE_DOMAIN = {"kind": "whole_model"}
MESH_DOMAIN = {"kind": "mesh", "mesh_id": "m", "bin": 0}
# response id, domain, raw unit, raw mean, raw standard error
TALLIES = [
    ("magnets-flux", MAGNETS_DOMAIN, "cm_per_source", "3.5e-3", "1.4e-5"),
    ("total-tritium-production", WHOLE_DOMAIN, "particles_per_source", "1.1", "0.011"),
    ("heating-total-blanket", BLANKET_DOMAIN, "ev_per_source", "2.0e7", "4.0e4"),
    ("blanket-capture", BLANKET_DOMAIN, "events_per_source", "0.8", "0.004"),
    ("mesh-flux-0", MESH_DOMAIN, "cm_per_source", "2.5e-4", "5.0e-6"),
]


def shell_volume(inner, outer):
    return 2 * PI * PI * MAJOR * (outer**2 - inner**2)


def volumes_m3():
    bin_volume = Decimal(1)
    for axis in range(3):
        bin_volume *= (Decimal(str(MESH["upper_right_m"][axis])) - Decimal(str(MESH["lower_left_m"][axis]))) \
            / Decimal(MESH["dimensions"][axis])
    return [
        (BLANKET_DOMAIN, shell_volume(*BLANKET)),
        (MAGNETS_DOMAIN, shell_volume(*MAGNETS)),
        (WHOLE_DOMAIN, 2 * PI * PI * MAJOR * MAGNETS[1] ** 2),
        (MESH_DOMAIN, bin_volume),
    ]


def build():
    """The three documents of a correct run: input, raw artifact and normalized record."""
    volumes = dict((json.dumps(d, sort_keys=True), v) for d, v in volumes_m3())
    inp = {
        "request": {"fusion_power_mw": float(POWER_MW), "variant_id": "v",
                    "source": {"energy_per_reaction_ev": float(ENERGY_EV), "neutrons_per_reaction": 1.0}},
        "manifest": {"major_radius_m": float(MAJOR), "variants": [{"id": "v", "components": [
            {"id": "blanket", "inner_minor_radius_m": float(BLANKET[0]), "outer_minor_radius_m": float(BLANKET[1])},
            {"id": "magnets", "inner_minor_radius_m": float(MAGNETS[0]), "outer_minor_radius_m": float(MAGNETS[1])},
        ]}]},
        "mesh": copy.deepcopy(MESH),
        "sampling": {"seed": 7, "threads": 1},
    }
    raw = {
        "histories": 1000,
        "volumes": [{"domain": d, "value": float(v), "standard_error": 0.0, "unit": "cubic_metre"}
                    for d, v in volumes_m3()],
        "tallies": [{"response_id": i, "estimator": "tracklength", "unit": u, "mean": float(m), "standard_error": float(s)}
                    for i, _, u, m, s in [(a, b, c, Decimal(d), Decimal(e)) for a, b, c, d, e in TALLIES]],
    }
    results = []
    for response_id, domain, unit, mean, se in TALLIES:
        scale = RATE * FACTORS[unit]
        volume = volumes[json.dumps(domain, sort_keys=True)]
        results.append({
            "response_id": response_id, "domain": domain,
            "mean": float(Decimal(mean) * scale / volume), "standard_error": float(Decimal(se) * scale / volume),
            "integrated_mean": float(Decimal(mean) * scale), "integrated_standard_error": float(Decimal(se) * scale),
        })
    record = {"normalized": {"results": results}, "execution": {"elapsed_seconds": 1.0}}
    return inp, raw, record


def result_of(record, response_id):
    return next(r for r in record["normalized"]["results"] if r["response_id"] == response_id)


def scale(field, factor):
    """A slip that multiplies one field of one result by a wrong factor."""
    def slip(inp, raw, record):
        result = result_of(record, "magnets-flux")
        result[field] = result[field] * factor
    return slip


def slip_heating(field, factor):
    def slip(inp, raw, record):
        result = result_of(record, "heating-total-blanket")
        result[field] = result[field] * factor
    return slip


def slip_raw_volume(index, factor):
    def slip(inp, raw, record):
        raw["volumes"][index]["value"] *= factor
    return slip


def relabel_volume_unit(inp, raw, record):
    # Value is in m^3 but labelled cm^3: the control converts it and finds the wrong shell volume.
    for volume in raw["volumes"]:
        volume["unit"] = "cubic_centimetre"


def mislabel_raw_unit(inp, raw, record):
    raw["tallies"][0]["unit"] = "events_per_source"


def raw_flux_in_metres(inp, raw, record):
    raw["tallies"][0]["mean"] *= 100.0


def power_in_watts(inp, raw, record):
    inp["request"]["fusion_power_mw"] *= 1.0e6


def neutron_energy_for_reaction_energy(inp, raw, record):
    inp["request"]["source"]["energy_per_reaction_ev"] = 14.1e6


def ignore_neutrons_per_reaction(inp, raw, record):
    inp["request"]["source"]["neutrons_per_reaction"] = 2.0


def mesh_bin_count_slip(inp, raw, record):
    inp["mesh"]["dimensions"] = [8, 2, 2]


SLIPS = {
    "mean per m3 read as per cm3 (x1e6)": scale("mean", 1.0e6),
    "mean per cm3 read as per m3 (x1e-6)": scale("mean", 1.0e-6),
    "integrated flux in cm not m (x100)": scale("integrated_mean", 100.0),
    "integrated flux lost its 0.01 (x0.01)": scale("integrated_mean", 0.01),
    "mean without the 0.01 (x100)": scale("mean", 100.0),
    "mean not divided by volume again (x volume ~ 5.8)": scale("mean", 5.8),
    "standard error per cm3 while mean per m3": scale("standard_error", 1.0e-6),
    "standard error not normalized (/rate)": scale("standard_error", 1.0e-18),
    "integrated standard error in the wrong unit": scale("integrated_standard_error", 100.0),
    "integrated standard error dropped": scale("integrated_standard_error", 0.0),
    "heating mean in eV (no charge factor)": slip_heating("mean", float(1 / E)),
    "heating mean with the charge factor twice": slip_heating("mean", float(E)),
    "heating integrated in eV per second": slip_heating("integrated_mean", float(1 / E)),
    "heating standard error without the charge factor": slip_heating("standard_error", float(1 / E)),
    "per-source value left unscaled by the source rate": scale("mean", float(1 / RATE)),
    "blanket volume x1e6 in the artifact": slip_raw_volume(0, 1.0e6),
    "magnet volume /1e6 in the artifact": slip_raw_volume(1, 1.0e-6),
    "whole-model volume doubled": slip_raw_volume(2, 2.0),
    "all volumes labelled cm3 but given in m3": relabel_volume_unit,
    "raw flux unit label changed to events": mislabel_raw_unit,
    "raw flux already in metres (x100)": raw_flux_in_metres,
    "fusion power in watts, not megawatts": power_in_watts,
    "14.1 MeV used as the energy per reaction": neutron_energy_for_reaction_energy,
    "neutrons per reaction ignored": ignore_neutrons_per_reaction,
    "mesh bin count wrong": mesh_bin_count_slip,
}


class TransportArithmeticTests(unittest.TestCase):
    def write(self, directory, documents):
        inp, raw, record = documents
        directory = Path(directory)
        (directory / "solver").mkdir(parents=True, exist_ok=True)
        (directory / "input.json").write_text(json.dumps(inp))
        (directory / "solver" / "transport-artifact.json").write_text(json.dumps(raw))
        (directory / "run.json").write_text(json.dumps(record))
        return directory

    # Verifies: NUC-003
    def test_a_correct_run_reproduces_the_conversion_table_to_1e_12(self):
        with tempfile.TemporaryDirectory() as temp:
            report = CHECK.check(self.write(temp, build()))
        self.assertEqual(report["checked_tallies"], len(TALLIES))
        self.assertLess(Decimal(report["max_relative_normalization_difference"]), Decimal("1e-12"))
        self.assertLess(Decimal(report["max_relative_volume_difference"]), Decimal("1e-12"))
        rate = Decimal(report["independent_source_neutron_rate_per_s"])
        self.assertLess(abs(rate - RATE) / RATE, Decimal("1e-12"))

    # Verifies: NUC-003, VAL-015
    def test_each_seeded_unit_slip_is_detected(self):
        self.assertGreaterEqual(len(SLIPS), 20)
        for name, slip in SLIPS.items():
            documents = build()
            slip(*documents)
            with tempfile.TemporaryDirectory() as temp:
                directory = self.write(temp, documents)
                with self.assertRaises(ValueError, msg=f"slip not detected: {name}"):
                    CHECK.check(directory)

    def test_the_slips_are_distinct_changes_to_the_run(self):
        reference = build()
        seen = set()
        for name, slip in SLIPS.items():
            documents = copy.deepcopy(reference)
            slip(*documents)
            self.assertNotEqual(documents, reference, name)
            seen.add(json.dumps(documents, sort_keys=True, default=str))
        self.assertEqual(len(seen), len(SLIPS))


if __name__ == "__main__":
    unittest.main()
