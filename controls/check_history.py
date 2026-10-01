#!/usr/bin/env python3
"""Independent arithmetic controls for operating-history limiting cases.

Uses only Python's standard library and Decimal; deliberately does not call FARIS.
These controls verify units/identities and analytic decay, not reactor performance.
"""
from decimal import Decimal, getcontext
import math
import argparse
import json
import unittest

getcontext().prec = 45
D = Decimal
YEAR_S = D("365.25") * D(86400)
HALF_LIFE_Y = D("12.32")
LAMBDA = D(str(math.log(2.0))) / (HALF_LIFE_Y * YEAR_S)
MOLAR_MASS_KG_MOL = D("3.0160492779e-3")
AVOGADRO = D("6.02214076e23")
ATOM_KG = MOLAR_MASS_KG_MOL / AVOGADRO


def decay(mass_kg: Decimal, duration_s: Decimal) -> Decimal:
    return mass_kg * D(str(math.exp(-float(LAMBDA * duration_s))))


def source_rates(power_mw: Decimal, q_ev: Decimal, elementary_charge_j_ev: Decimal):
    reactions_s = power_mw * D(1_000_000) / (q_ev * elementary_charge_j_ev)
    return reactions_s, reactions_s  # one neutron per D-T reaction convention


def stock_after(stock: Decimal, burn_kg_s: Decimal, duration_s: Decimal) -> Decimal:
    """Analytic zero-production stock under burn plus radioactive decay."""
    factor = D(str(math.exp(-float(LAMBDA * duration_s))))
    return stock * factor - burn_kg_s * (D(1) - factor) / LAMBDA


def fluence(flux_n_m2_s: Decimal, operating_seconds: Decimal) -> Decimal:
    return flux_n_m2_s * operating_seconds


def close(actual, expected, label, *, rtol=D("1e-10"), atol=D("1e-12")):
    actual, expected = D(str(actual)), D(str(expected))
    if not actual.is_finite() or not expected.is_finite() or abs(actual - expected) > max(atol, rtol * max(abs(actual), abs(expected))):
        raise AssertionError(f"{label} does not close: actual={actual}, expected={expected}")


def verify_energy_ledger(result):
    rates, assumptions = result["driving_rates"], result["assumptions"]
    energy = assumptions["energy"]
    require = (rates["neutron_deposited_heat_w"] is not None
               and energy["alpha_deposition_fraction"] is not None
               and energy["neutron_heat_recovery_fraction"] is not None
               and energy["thermal_to_electric_efficiency"] is not None
               and energy["auxiliary_power_mw_while_operating"] is not None
               and energy["auxiliary_power_mw_while_off"] is not None)
    if not require:
        return {"status": "NOT_EVALUATED_INPUTS_UNAVAILABLE"}

    heat_w = D(str(rates["neutron_deposited_heat_w"]["mean"]))
    recovery = D(str(energy["neutron_heat_recovery_fraction"]))
    alpha_fraction = D(str(energy["alpha_deposition_fraction"]))
    efficiency = D(str(energy["thermal_to_electric_efficiency"]))
    reaction_rate = D(str(rates["fusion_reaction_rate_per_s"]))
    q_ev = D(str(rates["total_reaction_energy_ev"]))
    neutron_ev = D(str(rates["primary_neutron_energy_ev"]))
    joule_per_ev = D("1.602176634e-19")
    full_power_s = D("0")
    last_t = None
    prior_operating = False
    auxiliary_mwh = D("0")
    for snapshot in result["snapshots"]:
        t = D(str(snapshot["time_s"]))
        if last_t is not None:
            aux = D(str(energy["auxiliary_power_mw_while_operating"] if prior_operating
                        else energy["auxiliary_power_mw_while_off"]))
            auxiliary_mwh += aux * (t - last_t) / D("3600")
        fraction = D(str(snapshot["power_fraction"]))
        neutron_mw = heat_w * recovery * fraction / D("1e6")
        alpha_mw = reaction_rate * fraction * (q_ev - neutron_ev) * joule_per_ev / D("1e6") * alpha_fraction
        gross_mw = (neutron_mw + alpha_mw) * efficiency
        aux_mw = D(str(energy["auxiliary_power_mw_while_operating"] if snapshot["operating"]
                        else energy["auxiliary_power_mw_while_off"]))
        for key, expected in (("instantaneous_neutron_recovered_heat_mw", neutron_mw),
                              ("instantaneous_alpha_recovered_heat_mw", alpha_mw),
                              ("instantaneous_gross_electricity_mw", gross_mw),
                              ("instantaneous_auxiliary_electricity_mw", aux_mw),
                              ("instantaneous_net_electricity_mw", gross_mw - aux_mw)):
            close(snapshot[key], expected, key, rtol=D("1e-10"), atol=D("1e-9"))
        last_t = t
        prior_operating = bool(snapshot["operating"])
        full_power_s = D(str(snapshot["cumulative_full_power_seconds"]))

    last = result["snapshots"][-1]
    neutron_mwh = heat_w / D("1e6") * recovery * full_power_s / D("3600")
    alpha_mwh = reaction_rate * (q_ev - neutron_ev) * joule_per_ev / D("1e6") * alpha_fraction * full_power_s / D("3600")
    gross_mwh = (neutron_mwh + alpha_mwh) * efficiency
    for key, expected in (("cumulative_neutron_recovered_heat_mwh", neutron_mwh),
                          ("cumulative_alpha_recovered_heat_mwh", alpha_mwh),
                          ("cumulative_gross_electricity_mwh", gross_mwh),
                          ("cumulative_auxiliary_electricity_mwh", auxiliary_mwh),
                          ("cumulative_net_electricity_mwh", gross_mwh - auxiliary_mwh)):
        close(last[key], expected, key, rtol=D("1e-9"), atol=D("1e-8"))
    return {"status": "PASS", "neutron_heat_input_W": str(heat_w),
            "recovered_neutron_heat_MWh": str(neutron_mwh),
            "recovered_alpha_heat_MWh": str(alpha_mwh),
            "gross_electricity_MWh": str(gross_mwh),
            "auxiliary_electricity_MWh": str(auxiliary_mwh),
            "net_electricity_MWh": str(gross_mwh - auxiliary_mwh),
            "meaning": "conditional ledger arithmetic only; no heat-cycle or engineering qualification"}


def verify_transport_binding(history, run_path):
    run = json.load(open(run_path, encoding="utf-8"))
    if run.get("execution", {}).get("execution_status") != "SUCCEEDED" or not run.get("normalized"):
        raise AssertionError("source run lacks a successful normalized transport result")
    normalized, rates = run["normalized"], history["driving_rates"]
    if rates["transport_artifact_sha256"] != run.get("raw_artifact_sha256"):
        raise AssertionError("history transport artifact identity differs from source run")
    if rates["scenario_sha256"] != run["scenario_sha256"]:
        raise AssertionError("history scenario identity differs from source run")
    if rates["solver_digest"] != normalized["solver"]["digest"] or rates["nuclear_data_digest"] != normalized["nuclear_data"]["digest"]:
        raise AssertionError("history solver or nuclear-data identity differs from source run")
    close(rates["fusion_reaction_rate_per_s"], normalized["source_reaction_rate_per_s"], "reaction-rate binding")
    close(rates["neutron_source_rate_per_s"], normalized["source_neutron_rate_per_s"], "source-rate binding")
    by_id = {item["response_id"]: item for item in normalized["results"]}
    breeder = by_id["blanket-tritium"]
    close(rates["breeder_h3_per_source_neutron"]["mean"],
          D(str(breeder["integrated_mean"])) / D(str(normalized["source_neutron_rate_per_s"])), "H3 tally binding")
    for component, bound in rates["component_average_flux_n_m2_s"].items():
        response = by_id[bound["response_id"]]
        if response["domain"] != {"kind": "component", "component_id": component}:
            raise AssertionError(f"flux response domain differs for {component}")
        close(bound["mean"], response["mean"], f"{component} flux binding")
        close(bound["standard_error"], response["standard_error"], f"{component} flux SE binding")
    heat = by_id["heating-total-whole-model"]
    if heat["domain"] != {"kind": "whole_model"} or heat["integrated_unit"] != "watts":
        raise AssertionError("whole-model heating response has unexpected domain or unit")
    heating_rate = rates["neutron_deposited_heat_w"]
    if heating_rate["response_id"] != "heating-total-whole-model" or heating_rate["unit"] != "W":
        raise AssertionError("history did not select the declared whole-model heating response")
    close(heating_rate["mean"], heat["integrated_mean"], "whole-model heat binding")
    close(heating_rate["standard_error"], heat["integrated_standard_error"], "whole-model heat SE binding")
    scope_ids = ["heating-neutron-whole-model", "heating-photon-whole-model",
                 "heating-electron-whole-model", "heating-positron-whole-model"]
    scope_sum = sum(D(str(by_id[key]["integrated_mean"])) for key in scope_ids)
    total_heat = D(str(heat["integrated_mean"]))
    scope_relative_difference = abs(scope_sum - total_heat) / max(abs(total_heat), D("1e-30"))
    if scope_relative_difference > D("1e-10"):
        raise AssertionError("whole-model heating score does not match sum of declared particle-scope means")
    return {"status": "PASS", "source_run_execution": "SUCCEEDED",
            "raw_artifact_sha256": run["raw_artifact_sha256"],
            "component_flux_bindings_checked": len(rates["component_average_flux_n_m2_s"]),
            "whole_model_heat_estimator": heat["estimator"],
            "whole_model_heat_W": heat["integrated_mean"],
            "whole_model_heat_standard_error_W": heat["integrated_standard_error"],
            "particle_scope_mean_sum_W": str(scope_sum),
            "particle_scope_mean_relative_difference": str(scope_relative_difference),
            "standard_errors_not_summed": True,
            "note": "zero component flux means remain sampled estimates, not proof of zero true flux"}


def verify_engine_history(path, run_path=None):
    result = json.load(open(path, encoding="utf-8"))
    rates = result["driving_rates"]
    atom = D(str(result["tritium_atom_mass_kg"]))
    source = D(str(rates["neutron_source_rate_per_s"]))
    reaction = D(str(rates["fusion_reaction_rate_per_s"]))
    tbr = D(str(rates["breeder_h3_per_source_neutron"]["mean"]))
    max_decimal_residual = D(0)
    max_engine_residual = 0.0
    for snapshot in result["snapshots"]:
        d = {k: D(str(snapshot[k])) for k in (
            "available_tritium_kg", "in_process_tritium_kg", "cumulative_production_kg",
            "cumulative_import_kg", "cumulative_burn_kg", "cumulative_processing_loss_kg",
            "cumulative_decay_kg", "cumulative_full_power_seconds", "mass_balance_residual_kg")}
        balance = (D(str(result["assumptions"]["initial_available_tritium_kg"]))
                   + D(str(result["assumptions"]["initial_in_process_tritium_kg"]))
                   + d["cumulative_production_kg"] + d["cumulative_import_kg"]
                   - d["cumulative_burn_kg"] - d["cumulative_processing_loss_kg"]
                   - d["cumulative_decay_kg"] - d["available_tritium_kg"]
                   - d["in_process_tritium_kg"])
        max_decimal_residual = max(max_decimal_residual, abs(balance))
        max_engine_residual = max(max_engine_residual, abs(float(d["mass_balance_residual_kg"])))
        expected_burn = reaction * atom * d["cumulative_full_power_seconds"]
        expected_production = tbr * source * atom * d["cumulative_full_power_seconds"]
        if abs(expected_burn - d["cumulative_burn_kg"]) > D("1e-10") * max(D(1), abs(expected_burn)):
            raise AssertionError("engine burn differs from independent reaction-rate × tritium-mass arithmetic")
        if abs(expected_production - d["cumulative_production_kg"]) > D("1e-10") * max(D(1), abs(expected_production)):
            raise AssertionError("engine production differs from independent breeder-rate × tritium-mass arithmetic")
    if float(max_decimal_residual) > result["mass_balance_tolerance_kg"]:
        raise AssertionError("Decimal site-balance residual exceeds engine tolerance")
    for snapshot in result["snapshots"]:
        if snapshot["cumulative_net_electricity_mwh"] is not None:
            expected = snapshot["cumulative_gross_electricity_mwh"] - snapshot["cumulative_auxiliary_electricity_mwh"]
            if abs(expected - snapshot["cumulative_net_electricity_mwh"]) > 1e-10 * max(1.0, abs(expected)):
                raise AssertionError("gross minus auxiliary electricity does not close to net")
    energy_audit = verify_energy_ledger(result)
    binding_audit = verify_transport_binding(result, run_path) if run_path else None
    return {
        "source_file": path,
        "outcome": result["outcome"],
        "snapshot_count": len(result["snapshots"]),
        "event_count": len(result["events"]),
        "max_decimal_balance_residual_kg": str(max_decimal_residual),
        "max_engine_balance_residual_kg": max_engine_residual,
        "mass_balance_tolerance_kg": result["mass_balance_tolerance_kg"],
        "independent_source_rate_production_burn_checks": "PASS",
        "energy_audit": energy_audit,
        "transport_binding_audit": binding_audit,
        "scientific_scope": "numerical audit only; no engineering qualification",
    }, result


def compare_refinement(coarse, fine):
    # Frozen rule: aggregate mass/energy changes <=1e-4 relative; final stock/decay <=1e-4 kg;
    # discrete event sequence identical and timings within the coarse one-hour integration step.
    a, b = coarse["snapshots"][-1], fine["snapshots"][-1]
    rel_keys = ("cumulative_production_kg", "cumulative_burn_kg", "cumulative_full_power_seconds", "cumulative_fusion_energy_mwh")
    rel_changes = {key: abs(b[key] - a[key]) / max(abs(a[key]), 1e-30) for key in rel_keys}
    abs_keys = ("available_tritium_kg", "cumulative_decay_kg")
    abs_changes = {key: abs(b[key] - a[key]) for key in abs_keys}
    if max(rel_changes.values()) > 1e-4 or max(abs_changes.values()) > 1e-4:
        raise AssertionError("step-refinement change exceeds frozen aggregate tolerances")
    ea, eb = coarse["events"], fine["events"]
    if [e["kind"] for e in ea] != [e["kind"] for e in eb]:
        raise AssertionError("step refinement changed discrete event sequence")
    max_event_time_delta = max((abs(x["time_s"] - y["time_s"]) for x, y in zip(ea, eb)), default=0.0)
    coarse_step = coarse["assumptions"]["maximum_step_s"]
    if max_event_time_delta > coarse_step + 1e-9:
        raise AssertionError("event timing moved by more than the coarse time resolution")
    return {"relative_changes": rel_changes, "absolute_changes_kg": abs_changes,
            "maximum_event_time_delta_s": max_event_time_delta,
            "coarse_step_s": coarse_step, "acceptance": "PASS",
            "criterion": "relative aggregate <=1e-4; final stock/decay <=1e-4kg; same event-kind sequence; event-time delta <= coarse step"}


class HistoryControlTests(unittest.TestCase):
    def test_half_life_decay_and_conservation(self):
        opening = D("1.25")
        residual = decay(opening, HALF_LIFE_Y * YEAR_S)
        self.assertAlmostEqual(float(residual), 0.625, places=14)
        self.assertAlmostEqual(float(opening - residual), 0.625, places=14)

    def test_dt_source_rate_identity_and_atom_conversion(self):
        e = D("1.602176634e-19")
        rate, neutrons = source_rates(D("525"), D("17.6e6"), e)
        self.assertEqual(rate, neutrons)
        self.assertAlmostEqual(float(rate), 1.8618137864158525e20, delta=2e5)
        self.assertAlmostEqual(float(ATOM_KG), 5.008267e-27, delta=1e-32)

    def test_processing_mass_closure_including_decay_and_loss(self):
        produced = D("0.010")
        decay_loss = D("0.0001")
        recovery = D("0.9999")
        recoverable = (produced - decay_loss) * recovery
        processing_loss = (produced - decay_loss) - recoverable
        self.assertEqual(produced, recoverable + processing_loss + decay_loss)

    def test_energy_units(self):
        rate = D("1e20")
        alpha_ev = D("3.5e6")
        charge = D("1.602176634e-19")
        mw = rate * alpha_ev * charge / D("1e6")
        self.assertEqual(mw, D("56.07618219"))
        mwh = mw * D(3600) / D(3600)
        self.assertEqual(mwh, mw)

    def test_fuel_trip_time_respects_reserve(self):
        opening, reserve, burn = D("1"), D("0.9"), D("1e-8")
        low, high = D(0), D(20_000_000)
        for _ in range(120):
            mid = (low + high) / 2
            if stock_after(opening, burn, mid) > reserve:
                low = mid
            else:
                high = mid
        self.assertGreater(stock_after(opening, burn, low), reserve)
        self.assertLessEqual(stock_after(opening, burn, high), reserve)
        self.assertLess(high - low, D("1e-20"))

    def test_delayed_recovery_is_unavailable_before_release(self):
        release_s, produced, recovery = D(86_400), D("0.25"), D("0.95")
        available = D(0)
        self.assertEqual(available, 0 if release_s > 0 else produced * recovery)
        available += produced * recovery
        self.assertEqual(available, D("0.2375"))

    def test_exposure_stops_during_outage_and_replacement_is_local(self):
        flux, first_operation, outage, second_operation = map(D, ("1e12", "100", "40", "60"))
        total_fluence = fluence(flux, first_operation + second_operation)
        self.assertEqual(total_fluence, D("1.6e14"))
        site_inventory = D("5")
        local_component_reset = D(0)
        self.assertEqual(site_inventory, D("5"))
        self.assertEqual(local_component_reset, 0)

    def test_off_state_auxiliary_load_remains_signed(self):
        gross, off_aux, hours = D(0), D(5), D(12)
        self.assertEqual((gross - off_aux) * hours, D(-60))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine-history", action="append", default=[])
    parser.add_argument("--normalized-run")
    parser.add_argument("--refinement", nargs=2, metavar=("COARSE", "FINE"))
    args, remaining = parser.parse_known_args()
    if args.engine_history or args.refinement:
        if args.normalized_run and len(args.engine_history) != 1:
            parser.error("--normalized-run requires exactly one --engine-history")
        verified = [verify_engine_history(path, args.normalized_run if args.normalized_run else None)
                    for path in args.engine_history]
        report = {"engine_results": [summary for summary, _ in verified]}
        if args.refinement:
            report["refinement"] = compare_refinement(*[json.load(open(path, encoding="utf-8")) for path in args.refinement])
        print(json.dumps(report, indent=2))
    else:
        unittest.main(argv=[__file__, *remaining], verbosity=2)
