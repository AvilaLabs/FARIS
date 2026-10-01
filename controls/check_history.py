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
from copy import deepcopy

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
    heating_rate = rates.get("transport_deposited_heat_w", rates.get("neutron_deposited_heat_w"))
    recovery = energy.get("transport_heat_recovery_fraction", energy.get("neutron_heat_recovery_fraction"))
    require = (heating_rate is not None
               and energy["alpha_deposition_fraction"] is not None
               and recovery is not None
               and energy["thermal_to_electric_efficiency"] is not None
               and energy["auxiliary_power_mw_while_operating"] is not None
               and energy["auxiliary_power_mw_while_off"] is not None)
    if not require:
        return {"status": "NOT_EVALUATED_INPUTS_UNAVAILABLE"}

    heat_w = D(str(heating_rate["mean"]))
    recovery = D(str(recovery))
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
        for key, legacy, expected in (("instantaneous_transport_recovered_heat_mw", "instantaneous_neutron_recovered_heat_mw", neutron_mw),
                                      ("instantaneous_alpha_recovered_heat_mw", None, alpha_mw),
                                      ("instantaneous_gross_electricity_mw", None, gross_mw),
                                      ("instantaneous_auxiliary_electricity_mw", None, aux_mw),
                                      ("instantaneous_net_electricity_mw", None, gross_mw - aux_mw)):
            close(snapshot.get(key, snapshot.get(legacy) if legacy else None), expected, key,
                  rtol=D("1e-10"), atol=D("1e-9"))
        last_t = t
        prior_operating = bool(snapshot["operating"])
        full_power_s = D(str(snapshot["cumulative_full_power_seconds"]))

    last = result["snapshots"][-1]
    neutron_mwh = heat_w / D("1e6") * recovery * full_power_s / D("3600")
    alpha_mwh = reaction_rate * (q_ev - neutron_ev) * joule_per_ev / D("1e6") * alpha_fraction * full_power_s / D("3600")
    gross_mwh = (neutron_mwh + alpha_mwh) * efficiency
    for key, legacy, expected in (("cumulative_transport_recovered_heat_mwh", "cumulative_neutron_recovered_heat_mwh", neutron_mwh),
                                  ("cumulative_alpha_recovered_heat_mwh", None, alpha_mwh),
                                  ("cumulative_gross_electricity_mwh", None, gross_mwh),
                                  ("cumulative_auxiliary_electricity_mwh", None, auxiliary_mwh),
                                  ("cumulative_net_electricity_mwh", None, gross_mwh - auxiliary_mwh)):
        close(last.get(key, last.get(legacy) if legacy else None), expected, key,
              rtol=D("1e-9"), atol=D("1e-8"))
    return {"status": "PASS", "transport_heating_input_W": str(heat_w),
            "recovered_transport_heat_MWh": str(neutron_mwh),
            "recovered_alpha_heat_MWh": str(alpha_mwh),
            "gross_electricity_MWh": str(gross_mwh),
            "auxiliary_electricity_MWh": str(auxiliary_mwh),
            "net_electricity_MWh": str(gross_mwh - auxiliary_mwh),
            "meaning": "conditional ledger arithmetic only; no heat-cycle or engineering qualification"}


def verify_continuous_processing_interval(result, phase="on-delayed-on"):
    """Check one smooth CLI interval against an independent Decimal DDE solution."""
    if result.get("processing_model") != "continuous-delayed-release-v2":
        return {"status": "NOT_EVALUATED_METHOD_IDENTITY_UNAVAILABLE"}
    assumptions, rates = result["assumptions"], result["driving_rates"]
    if assumptions["initial_in_process_tritium_kg"] > 0:
        return {"status": "NOT_EVALUATED_INITIAL_POINT_COHORT_PRESENT"}
    snapshots = result["snapshots"]
    events = result["events"]
    delay = D(str(assumptions["processing_delay_s"]))
    recovery = D(str(assumptions["recovery_fraction"]))
    lam = D(str(result["tritium_decay_constant_per_s"]))
    atom = D(str(result["tritium_atom_mass_kg"]))
    production_full = (D(str(rates["breeder_h3_per_source_neutron"]["mean"]))
                       * D(str(rates["neutron_source_rate_per_s"])) * atom)
    burn_full = D(str(rates["fusion_reaction_rate_per_s"])) * atom

    def exp_decay(dt):
        return (-lam * dt).exp()

    def integral(dt):
        return dt if lam == 0 else (D(1) - exp_decay(dt)) / lam

    def power_at(t):
        if t < 0:
            return D(0)
        prior = [s for s in snapshots if D(str(s["time_s"])) <= t]
        return D(str(prior[-1]["power_fraction"])) if prior else D(0)

    def has_event_between(left, right):
        # Left-boundary state is already represented by snapshot a; an event at
        # the right boundary changes the state used by snapshot b.
        return any(left < D(str(e["time_s"])) <= right for e in events)

    selected = None
    for a, b in zip(snapshots, snapshots[1:]):
        t0, t1 = D(str(a["time_s"])), D(str(b["time_s"]))
        dt = t1 - t0
        if dt <= 0 or has_event_between(t0, t1):
            continue
        p0, p1 = D(str(a["power_fraction"])), D(str(b["power_fraction"]))
        if p0 != p1:
            continue
        delayed0, delayed1 = power_at(t0 - delay), power_at(t1 - delay)
        if delayed0 != delayed1 or has_event_between(t0 - delay, t1 - delay):
            continue
        current_on, delayed_on = p0 > 0, delayed0 > 0
        phase_matches = {
            "on-delayed-on": current_on and delayed_on,
            "off-delayed-on": not current_on and delayed_on,
            "on-delayed-off": current_on and not delayed_on,
            "off-delayed-off": not current_on and not delayed_on,
        }.get(phase)
        if phase_matches is None:
            raise ValueError(f"unknown delayed-processing phase: {phase}")
        if t0 > delay and phase_matches:
            selected = (a, b, t0, dt, p0, delayed0)
            break
    if selected is None:
        return {"status": "NOT_EVALUATED_NO_SMOOTH_POST_DELAY_INTERVAL"}

    a, b, t0, dt, power, delayed_power = selected
    produced_rate = production_full * power
    release_rate = production_full * delayed_power * exp_decay(delay)
    burn_rate = burn_full * power
    i0, a0 = D(str(a["in_process_tritium_kg"])), D(str(a["available_tritium_kg"]))
    expected_i1 = i0 * exp_decay(dt) + (produced_rate - release_rate) * integral(dt)
    expected_a1 = a0 * exp_decay(dt) + (recovery * release_rate - burn_rate) * integral(dt)
    expected_process_decay = lam * i0 * integral(dt) + (produced_rate - release_rate) * (dt - integral(dt))
    expected_available_decay = lam * a0 * integral(dt) + (recovery * release_rate - burn_rate) * (dt - integral(dt))
    observed = {
        "in_process_tritium_kg": D(str(b["in_process_tritium_kg"])),
        "available_tritium_kg": D(str(b["available_tritium_kg"])),
        "production_kg": D(str(b["cumulative_production_kg"])) - D(str(a["cumulative_production_kg"])),
        "burn_kg": D(str(b["cumulative_burn_kg"])) - D(str(a["cumulative_burn_kg"])),
        "processing_loss_kg": D(str(b["cumulative_processing_loss_kg"])) - D(str(a["cumulative_processing_loss_kg"])),
        "decay_kg": D(str(b["cumulative_decay_kg"])) - D(str(a["cumulative_decay_kg"])),
    }
    expected = {
        "in_process_tritium_kg": expected_i1,
        "available_tritium_kg": expected_a1,
        "production_kg": produced_rate * dt,
        "burn_kg": burn_rate * dt,
        "processing_loss_kg": (D(1) - recovery) * release_rate * dt,
        "decay_kg": expected_process_decay + expected_available_decay,
    }
    for key in expected:
        close(observed[key], expected[key], f"independent continuous-processing {key}",
              rtol=D("2e-8"), atol=D("2e-13"))
    return {"status": "PASS", "method": "independent Decimal constant-rate delayed-release solution",
            "phase": phase,
            "interval_start_s": str(t0), "interval_duration_s": str(dt),
            "processing_delay_s": str(delay), "current_power_fraction": str(power),
            "delayed_source_power_fraction": str(delayed_power),
            "observed": {k: str(v) for k, v in observed.items()},
            "expected": {k: str(v) for k, v in expected.items()},
            "scope": "software/mathematical control only; no reactor qualification"}


def verify_processing_delay_boundaries(result):
    """Integrate process inventory at aligned source-change-plus-delay boundaries."""
    if result.get("processing_model") != "continuous-delayed-release-v2":
        return {"status": "NOT_EVALUATED_METHOD_IDENTITY_UNAVAILABLE"}
    assumptions, rates = result["assumptions"], result["driving_rates"]
    if assumptions["initial_in_process_tritium_kg"] > 0:
        return {"status": "NOT_EVALUATED_INITIAL_POINT_COHORT_PRESENT"}
    delay = D(str(assumptions["processing_delay_s"]))
    if delay <= 0:
        return {"status": "NOT_APPLICABLE_ZERO_DELAY"}
    lam, atom = D(str(result["tritium_decay_constant_per_s"])), D(str(result["tritium_atom_mass_kg"]))
    production_full = D(str(rates["breeder_h3_per_source_neutron"]["mean"])) * D(str(rates["neutron_source_rate_per_s"])) * atom
    snapshots = result["snapshots"]
    boundaries = sorted({
        D(str(e["time_s"])) + delay for e in result["events"]
        if e["kind"] in ("operation_started", "operation_stopped")
        and D(str(e["time_s"])) + delay <= D(str(snapshots[-1]["time_s"]))
    })
    checked = []
    for boundary in boundaries:
        snap = next((s for s in snapshots if D(str(s["time_s"])) == boundary), None)
        if snap is None:
            continue
        window_start, expected = boundary - delay, D(0)
        for a, b in zip(snapshots, snapshots[1:]):
            ta, tb = D(str(a["time_s"])), D(str(b["time_s"]))
            lo, hi = max(ta, window_start), min(tb, boundary)
            if hi <= lo:
                continue
            overlap, age = hi - lo, boundary - hi
            factor = overlap if lam == 0 else (D(1) - (-lam * overlap).exp()) / lam
            expected += production_full * D(str(a["power_fraction"])) * (-lam * age).exp() * factor
        observed = D(str(snap["in_process_tritium_kg"]))
        close(observed, expected, f"delayed-release boundary process inventory at t={boundary}",
              rtol=D("2e-8"), atol=D("1e-10"))
        checked.append({"time_s": str(boundary), "observed_in_process_kg": str(observed),
                        "expected_in_process_kg": str(expected)})
    return {"status": "PASS" if checked else "NOT_EVALUATED_NO_ALIGNED_DELAY_BOUNDARY",
            "checked_boundaries": checked,
            "scope": "software/math boundary control; no reactor qualification"}


def verify_restart_crossing(result):
    """Solve the first synthetic off-state restart from its delayed source analytically."""
    if result.get("processing_model") != "continuous-delayed-release-v2":
        return {"status": "NOT_EVALUATED_METHOD_IDENTITY_UNAVAILABLE"}
    assumptions, rates = result["assumptions"], result["driving_rates"]
    unavailable = next((e for e in result["events"] if e["kind"] == "fuel_unavailable"), None)
    available = next((e for e in result["events"] if e["kind"] == "fuel_available"), None)
    if unavailable is None or available is None:
        return {"status": "NOT_EVALUATED_NO_OFF_RESTART_CYCLE"}
    t_off, t_restart = D(str(unavailable["time_s"])), D(str(available["time_s"]))
    delay = D(str(assumptions["processing_delay_s"]))
    if not (D(0) < t_off < delay < t_restart < t_off + delay):
        return {"status": "NOT_EVALUATED_FIRST_RESTART_NOT_SINGLE_DELAYED_PULSE"}
    lam, atom = D(str(result["tritium_decay_constant_per_s"])), D(str(result["tritium_atom_mass_kg"]))
    recovery = D(str(assumptions["recovery_fraction"]))
    production = D(str(rates["breeder_h3_per_source_neutron"]["mean"])) * D(str(rates["neutron_source_rate_per_s"])) * atom
    burn = D(str(rates["fusion_reaction_rate_per_s"])) * atom
    opening, restart_floor = D(str(assumptions["initial_available_tritium_kg"])), D(str(assumptions["restart_inventory_kg"]))
    threshold = restart_floor + D("2e-13") * max(restart_floor, D(1))
    exp_decay = lambda dt: (-lam * dt).exp()
    flow = lambda dt: (D(1) - exp_decay(dt)) / lam if lam else dt
    stock_off = opening * exp_decay(t_off) - burn * flow(t_off)
    off_snapshot = next((s for s in result["snapshots"] if D(str(s["time_s"])) == t_off), None)
    if off_snapshot is None:
        raise AssertionError("fuel-stop event lacks exact state snapshot")
    close(off_snapshot["available_tritium_kg"], stock_off, "analytic fuel-stop stock",
          rtol=D("2e-8"), atol=D("2e-13"))
    stock_at_release = stock_off * exp_decay(delay - t_off)
    release_rate = production * exp_decay(delay)
    lo, hi = D(0), t_restart - delay
    for _ in range(160):
        mid = (lo + hi) / 2
        stock = stock_at_release * exp_decay(mid) + recovery * release_rate * flow(mid)
        if stock < threshold:
            lo = mid
        else:
            hi = mid
    predicted = delay + hi
    close(t_restart, predicted, "analytic delayed-recovery restart time", rtol=D(0), atol=D("2e-6"))
    return {"status": "PASS", "fuel_off_time_s": str(t_off), "observed_restart_time_s": str(t_restart),
            "predicted_restart_time_s": str(predicted), "restart_threshold_kg": str(threshold),
            "scope": "synthetic software/math control only; not a physical result"}


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
    heating_rate = rates.get("transport_deposited_heat_w", rates.get("neutron_deposited_heat_w"))
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
            gross = snapshot["cumulative_gross_electricity_mwh"]
            auxiliary = snapshot["cumulative_auxiliary_electricity_mwh"]
            expected = gross - auxiliary
            # Net power can be a small difference between large cumulative terms;
            # scale roundoff by those operands instead of the cancellation result.
            if abs(expected - snapshot["cumulative_net_electricity_mwh"]) > 1e-10 * max(1.0, abs(gross), abs(auxiliary)):
                raise AssertionError("gross minus auxiliary electricity does not close to net")
    energy_audit = verify_energy_ledger(result)
    processing_audits = {
        phase: verify_continuous_processing_interval(result, phase)
        for phase in ("on-delayed-on", "off-delayed-on", "on-delayed-off", "off-delayed-off")
    }
    processing_boundaries = verify_processing_delay_boundaries(result)
    restart_audit = verify_restart_crossing(result)
    binding_audit = verify_transport_binding(result, run_path) if run_path else None
    return {
        "source_file": path,
        "processing_model": result.get("processing_model"),
        "outcome": result["outcome"],
        "snapshot_count": len(result["snapshots"]),
        "event_count": len(result["events"]),
        "max_decimal_balance_residual_kg": str(max_decimal_residual),
        "max_engine_balance_residual_kg": max_engine_residual,
        "mass_balance_tolerance_kg": result["mass_balance_tolerance_kg"],
        "independent_source_rate_production_burn_checks": "PASS",
        "energy_audit": energy_audit,
        "continuous_processing_phase_audits": processing_audits,
        "processing_delay_boundary_audit": processing_boundaries,
        "restart_crossing_audit": restart_audit,
        "transport_binding_audit": binding_audit,
        "scientific_scope": "numerical audit only; no engineering qualification",
    }, result


def compare_refinement(coarse, fine):
    # Criterion extension recorded before corrected-geometry 1M primary outputs.
    # Preserve prior v1 control reports separately; they did not assess all displayed outputs.
    if (coarse.get("processing_model") != "continuous-delayed-release-v2"
            or fine.get("processing_model") != coarse.get("processing_model")):
        raise AssertionError("refinement requires identical current processing-model identities")
    coarse_assumptions = dict(coarse["assumptions"])
    fine_assumptions = dict(fine["assumptions"])
    coarse_step = coarse_assumptions.pop("maximum_step_s", None)
    fine_step = fine_assumptions.pop("maximum_step_s", None)
    if coarse_assumptions != fine_assumptions or coarse.get("driving_rates") != fine.get("driving_rates"):
        raise AssertionError("step refinement must use identical assumptions and driving rates except maximum_step_s")
    if not isinstance(coarse_step, (int, float)) or not isinstance(fine_step, (int, float)) or not (0 < fine_step < coarse_step):
        raise AssertionError("refinement requires a positive, strictly finer integration step")
    a, b = coarse["snapshots"][-1], fine["snapshots"][-1]
    rel_keys = ("cumulative_production_kg", "cumulative_burn_kg", "cumulative_full_power_seconds", "cumulative_fusion_energy_mwh")
    rel_changes = {key: abs(b[key] - a[key]) / max(abs(a[key]), 1e-30) for key in rel_keys}
    abs_keys = (
        "available_tritium_kg",
        "in_process_tritium_kg",
        "cumulative_processing_loss_kg",
        "cumulative_decay_kg",
    )
    abs_changes = {key: abs(b[key] - a[key]) for key in abs_keys}
    if any(not math.isfinite(value) for value in (*rel_changes.values(), *abs_changes.values())):
        raise AssertionError("step refinement contains a nonfinite mass or energy state")
    if max(rel_changes.values()) > 1e-4 or max(abs_changes.values()) > 1e-4:
        raise AssertionError("step-refinement change exceeds frozen aggregate tolerances")
    ea, eb = coarse["events"], fine["events"]
    event_signature = lambda events: [(e["kind"], e.get("component_id")) for e in events]
    if event_signature(ea) != event_signature(eb):
        raise AssertionError("step refinement changed discrete event kind/component sequence")
    if coarse.get("outcome") != fine.get("outcome"):
        raise AssertionError("step refinement changed final history outcome")
    if a["component_replacements"] != b["component_replacements"]:
        raise AssertionError("step refinement changed final component replacement counts")
    fluence_a, fluence_b = a["component_fluence_n_m2"], b["component_fluence_n_m2"]
    if fluence_a.keys() != fluence_b.keys():
        raise AssertionError("step refinement changed final component-fluence identities")
    fluence_relative_changes = {
        key: abs(fluence_b[key] - fluence_a[key]) / max(abs(fluence_a[key]), abs(fluence_b[key]), 1.0)
        for key in fluence_a
    }
    if any(not math.isfinite(value) for value in fluence_relative_changes.values()) or max(fluence_relative_changes.values(), default=0.0) > 1e-4:
        raise AssertionError("step-refinement component-fluence change exceeds 1e-4 relative")

    energy_keys = (
        "cumulative_transport_recovered_heat_mwh",
        "cumulative_alpha_recovered_heat_mwh",
        "cumulative_gross_electricity_mwh",
        "cumulative_auxiliary_electricity_mwh",
    )
    energy_relative_changes = {}
    unavailable_energy = []
    for key in energy_keys:
        x, y = a.get(key), b.get(key)
        if x is None and y is None:
            unavailable_energy.append(key)
            continue
        if x is None or y is None:
            raise AssertionError(f"step refinement changed availability of displayed energy output {key}")
        change = abs(y - x) / max(abs(x), abs(y), 1e-30)
        energy_relative_changes[key] = change
        if not math.isfinite(change) or change > 1e-4:
            raise AssertionError(f"step-refinement displayed energy change exceeds 1e-4 relative: {key}")
    net_a, net_b = a.get("cumulative_net_electricity_mwh"), b.get("cumulative_net_electricity_mwh")
    if net_a is None and net_b is None:
        unavailable_energy.append("cumulative_net_electricity_mwh")
        net_scaled_change = None
    elif net_a is None or net_b is None:
        raise AssertionError("step refinement changed availability of signed net electricity")
    else:
        if any(a.get(key) is None or b.get(key) is None for key in energy_keys[-2:]):
            raise AssertionError("net electricity is present without both gross and auxiliary totals")
        energy_scale_mwh = max(
            abs(a.get("cumulative_gross_electricity_mwh") or 0.0),
            abs(b.get("cumulative_gross_electricity_mwh") or 0.0),
            abs(a.get("cumulative_auxiliary_electricity_mwh") or 0.0),
            abs(b.get("cumulative_auxiliary_electricity_mwh") or 0.0),
            1.0,
        )
        net_scaled_change = abs(net_b - net_a) / energy_scale_mwh
        if not math.isfinite(net_scaled_change) or net_scaled_change > 1e-4:
            raise AssertionError("step-refinement signed net-energy change exceeds 1e-4 on gross/auxiliary scale")
    event_time_deltas = [abs(x["time_s"] - y["time_s"]) for x, y in zip(ea, eb)]
    if any(not math.isfinite(value) for value in event_time_deltas):
        raise AssertionError("step refinement contains a nonfinite event time")
    max_event_time_delta = max(event_time_deltas, default=0.0)
    if max_event_time_delta > coarse_step + 1e-9:
        raise AssertionError("event timing moved by more than the coarse time resolution")
    return {"criteria_version": "extended-displayed-output-v2",
            "criterion_extension_date": "2026-10-01",
            "criterion_extension_notice": "Added before corrected-geometry 1M primary outputs. Prior v1 reports evaluated only the earlier aggregate/event gate and are retained separately; rerun this v2 gate for final evidence.",
            "relative_changes": rel_changes, "absolute_changes_kg": abs_changes,
            "component_fluence_relative_changes": fluence_relative_changes,
            "displayed_energy_relative_changes": energy_relative_changes,
            "signed_net_energy_scaled_change": net_scaled_change,
            "energy_outputs_unavailable": unavailable_energy,
            "maximum_event_time_delta_s": max_event_time_delta,
            "coarse_step_s": coarse_step, "acceptance": "PASS",
            "criterion": "relative aggregate and recovered heat/gross/aux <=1e-4; final available/in-process/processing-loss/decay <=1e-4kg; component fluence relative <=1e-4; signed net difference divided by max(gross,aux,1MWh) <=1e-4; same event kind/component and replacement counts; event-time delta <= coarse step"}


class HistoryControlTests(unittest.TestCase):
    def test_decimal_oracle_checks_continuous_delayed_cli_interval_shape(self):
        lam = D(str(math.log(2.0) / float(HALF_LIFE_Y * YEAR_S)))
        delay, dt = D(100), D(10)
        production_rate, burn_rate = D("2e-9"), D("1e-9")
        atom = ATOM_KG
        source = D("1e20")
        tbr = production_rate / (source * atom)
        recovery = D("0.9")
        exp = lambda t: (-lam * t).exp()
        flow = lambda t: (D(1) - exp(t)) / lam
        release_rate = production_rate * exp(delay)
        i0 = production_rate * flow(delay)
        a0 = D("0.5")
        i1 = i0 * exp(dt) + (production_rate - release_rate) * flow(dt)
        a1 = a0 * exp(dt) + (recovery * release_rate - burn_rate) * flow(dt)
        process_decay = lam * i0 * flow(dt) + (production_rate - release_rate) * (dt - flow(dt))
        available_decay = lam * a0 * flow(dt) + (recovery * release_rate - burn_rate) * (dt - flow(dt))
        blank = {
            "available_tritium_kg": a0,
            "in_process_tritium_kg": i0,
            "cumulative_production_kg": D("0.1"),
            "cumulative_burn_kg": D("0.05"),
            "cumulative_processing_loss_kg": D("0.01"),
            "cumulative_decay_kg": D("0.02"),
            "time_s": D(200),
            "power_fraction": D(1),
        }
        final = {
            "available_tritium_kg": a1,
            "in_process_tritium_kg": i1,
            "cumulative_production_kg": D("0.1") + production_rate * dt,
            "cumulative_burn_kg": D("0.05") + burn_rate * dt,
            "cumulative_processing_loss_kg": D("0.01") + (D(1) - recovery) * release_rate * dt,
            "cumulative_decay_kg": D("0.02") + process_decay + available_decay,
            "time_s": D(210),
            "power_fraction": D(1),
        }
        result = {
            "processing_model": "continuous-delayed-release-v2",
            "assumptions": {"initial_in_process_tritium_kg": 0,
                            "processing_delay_s": float(delay),
                            "recovery_fraction": float(recovery)},
            "driving_rates": {
                "breeder_h3_per_source_neutron": {"mean": str(tbr)},
                "neutron_source_rate_per_s": str(source),
                "fusion_reaction_rate_per_s": str(burn_rate / atom),
            },
            "tritium_decay_constant_per_s": str(lam),
            "tritium_atom_mass_kg": str(atom),
            "events": [],
            "snapshots": [
                {**blank, "time_s": 0.0},
                {**blank, "time_s": 100.0},
                blank,
                final,
            ],
        }
        audit = verify_continuous_processing_interval(result)
        self.assertEqual(audit["status"], "PASS")

    def refinement_fixture(self):
        snapshot = {
            "cumulative_production_kg": 1.0,
            "cumulative_burn_kg": 1.0,
            "cumulative_full_power_seconds": 100.0,
            "cumulative_fusion_energy_mwh": 10.0,
            "available_tritium_kg": 1.0,
            "in_process_tritium_kg": 0.0,
            "cumulative_processing_loss_kg": 0.0,
            "cumulative_decay_kg": 0.0,
            "component_replacements": {"blanket": 1, "magnets": 0},
            "component_fluence_n_m2": {"blanket": 1.0e20, "magnets": 0.0},
            "cumulative_transport_recovered_heat_mwh": 1.0e6,
            "cumulative_alpha_recovered_heat_mwh": 1.0e5,
            "cumulative_gross_electricity_mwh": 8.0e5,
            "cumulative_auxiliary_electricity_mwh": 799_999.9999,
            "cumulative_net_electricity_mwh": 0.0001,
        }
        coarse = {
            "processing_model": "continuous-delayed-release-v2",
            "assumptions": {"maximum_step_s": 600.0, "horizon_s": 100_000.0},
            "driving_rates": {"source_rate": 1.0},
            "outcome": "horizon_completed",
            "events": [{"kind": "blanket_replaced", "component_id": "blanket", "time_s": 50.0}],
            "snapshots": [snapshot],
        }
        fine = deepcopy(coarse)
        fine["assumptions"]["maximum_step_s"] = 500.0
        fine["snapshots"][0]["cumulative_net_electricity_mwh"] = -0.0001
        return coarse, fine

    def test_refinement_accepts_near_zero_net_when_large_terms_agree(self):
        coarse, fine = self.refinement_fixture()
        result = compare_refinement(coarse, fine)
        self.assertEqual(result["acceptance"], "PASS")
        self.assertLess(result["signed_net_energy_scaled_change"], 1e-4)

    def test_refinement_rejects_component_event_reassignment_and_energy_drift(self):
        coarse, fine = self.refinement_fixture()
        fine["events"][0]["component_id"] = "magnets"
        with self.assertRaisesRegex(AssertionError, "event kind/component"):
            compare_refinement(coarse, fine)

        coarse, fine = self.refinement_fixture()
        fine["snapshots"][0]["cumulative_transport_recovered_heat_mwh"] *= 1.01
        with self.assertRaisesRegex(AssertionError, "displayed energy"):
            compare_refinement(coarse, fine)

    def test_refinement_rejects_component_fluence_and_driver_changes(self):
        coarse, fine = self.refinement_fixture()
        fine["processing_model"] = "continuous-delayed-release-v1"
        with self.assertRaisesRegex(AssertionError, "processing-model identities"):
            compare_refinement(coarse, fine)

        coarse, fine = self.refinement_fixture()
        fine["snapshots"][0]["component_fluence_n_m2"]["blanket"] *= 1.001
        with self.assertRaisesRegex(AssertionError, "component-fluence"):
            compare_refinement(coarse, fine)

        coarse, fine = self.refinement_fixture()
        fine["driving_rates"]["source_rate"] *= 1.001
        with self.assertRaisesRegex(AssertionError, "identical assumptions and driving rates"):
            compare_refinement(coarse, fine)

        coarse, fine = self.refinement_fixture()
        coarse["snapshots"][0]["cumulative_fusion_energy_mwh"] = float("nan")
        with self.assertRaisesRegex(AssertionError, "nonfinite mass or energy"):
            compare_refinement(coarse, fine)

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
