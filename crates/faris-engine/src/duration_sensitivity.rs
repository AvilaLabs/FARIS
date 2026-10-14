//! Sensitivity of lifetime net electricity and availability to each replacement duration
//! (OPS-031), and the link from remote-handling constraints (OPS-033) to the history's
//! replacement durations.
//!
//! The tornado reruns the operating history once per bound of each duration, changing that
//! one component's duration and nothing else. Each bar carries the label of its duration
//! (authored or literature, with the preset and link), so a literature value is never shown
//! as a FARIS result.

use crate::history::{EventKind, HistoryResult, TransportDrivingRates, run_operating_history};
use crate::maintenance::history_end_s;
use faris_model::history::{ComponentClass, OperatingHistoryAssumptions};
use faris_model::remote_handling::{InputLabel, RemoteHandling};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One replacement duration to vary: the component's low and high bound, in seconds.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DurationRange {
    pub component_id: String,
    pub low_s: f64,
    pub high_s: f64,
    pub label: InputLabel,
}

/// The two outputs the tornado reports, for one history.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TornadoOutputs {
    /// Absent when the history has no net-electricity result (its energy inputs are missing).
    pub net_electricity_mwh: Option<f64>,
    /// 1 - replacement downtime / history end, over every replaceable component.
    pub availability: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct TornadoBar {
    pub component_id: String,
    /// The duration in the base case, in seconds.
    pub base_s: f64,
    pub low_s: f64,
    pub high_s: f64,
    pub label: InputLabel,
    pub at_low: TornadoOutputs,
    pub at_high: TornadoOutputs,
    /// `at_high - at_low` in net electricity; absent when either result is.
    pub net_electricity_swing_mwh: Option<f64>,
    pub availability_swing: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DurationTornado {
    pub base: TornadoOutputs,
    /// Largest swing in net electricity first; bars without a net result sort by availability.
    pub bars: Vec<TornadoBar>,
}

/// Replacement downtime summed over every replaceable component, to the history end.
fn replacement_downtime_s(history: &HistoryResult) -> f64 {
    let end = history_end_s(history);
    let mut open: BTreeMap<&str, f64> = BTreeMap::new();
    let mut total = 0.0;
    for e in &history.events {
        let Some(id) = e.component_id.as_deref() else {
            continue;
        };
        match e.kind {
            EventKind::ReplacementStarted => {
                open.insert(id, e.time_s);
            }
            EventKind::ReplacementCompleted => {
                if let Some(start) = open.remove(id) {
                    total += e.time_s - start;
                }
            }
            _ => {}
        }
    }
    total + open.values().map(|start| end - start).sum::<f64>()
}

fn outputs(history: &HistoryResult) -> TornadoOutputs {
    let end = history_end_s(history);
    TornadoOutputs {
        net_electricity_mwh: history
            .snapshots
            .last()
            .and_then(|s| s.cumulative_net_electricity_mwh),
        availability: if end > 0.0 {
            1.0 - replacement_downtime_s(history) / end
        } else {
            1.0
        },
    }
}

/// The base duration of a replaceable component (the duration its limits share).
pub fn base_replacement_duration_s(
    assumptions: &OperatingHistoryAssumptions,
    component_id: &str,
) -> Result<f64, String> {
    assumptions
        .service_limits
        .iter()
        .find(|l| l.component_id == component_id && l.class == ComponentClass::Replaceable)
        .and_then(|l| l.replacement_duration_s)
        .ok_or_else(|| {
            format!(
                "component {component_id} is not a replaceable component with a replacement duration; name one from the history's service limits"
            )
        })
}

/// Set every replacement of one component to `duration_s`. A per-event list is dropped, so
/// the single value applies to all of that component's replacements.
pub fn with_replacement_duration_s(
    assumptions: &OperatingHistoryAssumptions,
    component_id: &str,
    duration_s: f64,
) -> Result<OperatingHistoryAssumptions, String> {
    base_replacement_duration_s(assumptions, component_id)?;
    let mut out = assumptions.clone();
    for limit in out
        .service_limits
        .iter_mut()
        .filter(|l| l.component_id == component_id)
    {
        limit.replacement_duration_s = Some(duration_s);
        limit.replacement_durations_s = None;
    }
    out.validate()?;
    Ok(out)
}

/// Set a component's replacement duration from remote-handling constraints. The outage
/// length is the model's (see [`RemoteHandling`] for its assumptions).
pub fn with_remote_handling(
    assumptions: &OperatingHistoryAssumptions,
    component_id: &str,
    handling: &RemoteHandling,
) -> Result<OperatingHistoryAssumptions, String> {
    with_replacement_duration_s(assumptions, component_id, handling.outage_s()?)
}

/// Run the base history and, for each range, the histories at its low and high bound.
/// Bars are ordered by the size of their net-electricity swing, largest first.
pub fn duration_tornado(
    assumptions: &OperatingHistoryAssumptions,
    rates: &TransportDrivingRates,
    ranges: &[DurationRange],
) -> Result<DurationTornado, String> {
    let base = outputs(&run_operating_history(assumptions, rates)?);
    let mut bars = Vec::with_capacity(ranges.len());
    for range in ranges {
        if !(range.low_s.is_finite() && range.high_s.is_finite() && range.low_s > 0.0) {
            return Err(format!(
                "duration range for {} needs finite positive bounds in seconds",
                range.component_id
            ));
        }
        if range.low_s > range.high_s {
            return Err(format!(
                "duration range for {} has low above high; swap them",
                range.component_id
            ));
        }
        let base_s = base_replacement_duration_s(assumptions, &range.component_id)?;
        let run = |d: f64| -> Result<TornadoOutputs, String> {
            let a = with_replacement_duration_s(assumptions, &range.component_id, d)?;
            Ok(outputs(&run_operating_history(&a, rates)?))
        };
        let (at_low, at_high) = (run(range.low_s)?, run(range.high_s)?);
        bars.push(TornadoBar {
            component_id: range.component_id.clone(),
            base_s,
            low_s: range.low_s,
            high_s: range.high_s,
            label: range.label.clone(),
            net_electricity_swing_mwh: at_low
                .net_electricity_mwh
                .zip(at_high.net_electricity_mwh)
                .map(|(l, h)| h - l),
            availability_swing: at_high.availability - at_low.availability,
            at_low,
            at_high,
        });
    }
    bars.sort_by(|a, b| {
        let key = |x: &TornadoBar| {
            x.net_electricity_swing_mwh
                .map_or(x.availability_swing.abs(), f64::abs)
        };
        key(b).total_cmp(&key(a))
    });
    Ok(DurationTornado { base, bars })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{assumptions, rates_without_covariance};
    use faris_model::history::ServiceLimit;
    use faris_model::remote_handling::maintenance_preset;

    fn two_replaceable() -> OperatingHistoryAssumptions {
        // The fixture's magnets are replaceable; make the blanket replaceable too, at a
        // lower limit than the magnets so it is swapped less often.
        let mut a = assumptions();
        a.service_limits[1] = ServiceLimit {
            class: ComponentClass::Replaceable,
            limit: 5.0e11,
            replacement_duration_s: Some(2.0),
            ..a.service_limits[1].clone()
        };
        a
    }

    fn range(id: &str, low: f64, high: f64) -> DurationRange {
        DurationRange {
            component_id: id.into(),
            low_s: low,
            high_s: high,
            label: InputLabel::Authored,
        }
    }

    // Verifies: OPS-031
    #[test]
    fn tornado_matches_direct_runs_and_longer_outages_cost_electricity_and_availability() {
        let a = two_replaceable();
        let rates = rates_without_covariance(0.0, 'a');
        let t = duration_tornado(
            &a,
            &rates,
            &[range("magnets", 2.0, 10.0), range("blanket", 1.0, 4.0)],
        )
        .unwrap();
        assert_eq!(t.bars.len(), 2);
        // Base equals a plain run.
        let direct = outputs(&run_operating_history(&a, &rates).unwrap());
        assert_eq!(t.base, direct);
        for bar in &t.bars {
            // Each bar's ends equal a direct run with only that duration changed.
            for (d, got) in [(bar.low_s, &bar.at_low), (bar.high_s, &bar.at_high)] {
                let b = with_replacement_duration_s(&a, &bar.component_id, d).unwrap();
                assert_eq!(*got, outputs(&run_operating_history(&b, &rates).unwrap()));
            }
            assert!(bar.at_high.availability < bar.at_low.availability);
            assert!(
                bar.at_high.net_electricity_mwh.unwrap() < bar.at_low.net_electricity_mwh.unwrap()
            );
            assert!(bar.net_electricity_swing_mwh.unwrap() < 0.0);
        }
        // The component replaced more often with the wider range comes first.
        let swings: Vec<f64> = t
            .bars
            .iter()
            .map(|b| b.net_electricity_swing_mwh.unwrap().abs())
            .collect();
        assert!(swings[0] >= swings[1]);
        assert_eq!(t.bars[0].component_id, "magnets");
        assert_eq!(t.bars[0].base_s, 5.0);
        assert_eq!(t.bars[1].base_s, 2.0);
    }

    // Verifies: OPS-031
    #[test]
    fn literature_label_travels_with_the_bar_and_bad_ranges_are_refused() {
        let a = two_replaceable();
        let rates = rates_without_covariance(0.0, 'a');
        let preset = maintenance_preset("demo-blanket-divertor-4").unwrap();
        let mut r = range("magnets", 2.0, 10.0);
        r.label = preset.input_label();
        let t = duration_tornado(&a, &rates, &[r]).unwrap();
        assert_eq!(t.bars[0].label.text(), "literature");
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("https://arxiv.org/pdf/1412.4008"));
        assert!(json.contains("demo-blanket-divertor-4"));
        let swapped = duration_tornado(&a, &rates, &[range("magnets", 10.0, 2.0)]);
        assert!(swapped.unwrap_err().contains("low above high"));
        let unknown = duration_tornado(&a, &rates, &[range("nope", 1.0, 2.0)]);
        assert!(unknown.unwrap_err().contains("not a replaceable"));
        let permanent = two_replaceable();
        assert!(base_replacement_duration_s(&permanent, "blanket").is_ok());
        assert!(base_replacement_duration_s(&assumptions(), "blanket").is_err());
    }

    // Verifies: OPS-031
    #[test]
    fn per_event_list_is_replaced_by_the_single_value() {
        let mut a = assumptions();
        a.service_limits[0].replacement_durations_s = Some(vec![9.0, 9.0]);
        let b = with_replacement_duration_s(&a, "magnets", 3.0).unwrap();
        assert_eq!(b.service_limits[0].replacement_duration_s, Some(3.0));
        assert!(b.service_limits[0].replacement_durations_s.is_none());
    }

    // Verifies: OPS-033
    #[test]
    fn more_parallel_systems_shorten_the_history_outage_until_ports_cap_them() {
        let a = assumptions();
        let rates = rates_without_covariance(0.0, 'a');
        // Scale the Crofts-fitted model (months) to the 100 s fixture by treating one month as 1 s.
        let scale = 1.0 / faris_model::remote_handling::months_to_seconds(1.0);
        let outage = |systems: u32, ports: u32| {
            let mut h = RemoteHandling::crofts_table1_fit()
                .with_systems(systems)
                .with_ports(ports);
            h.fixed_months *= scale;
            h.work_months *= scale;
            let b = with_remote_handling(&a, "magnets", &h).unwrap();
            let run = run_operating_history(&b, &rates).unwrap();
            (h.outage_s().unwrap(), outputs(&run))
        };
        let (d1, o1) = outage(1, 8);
        let (d4, o4) = outage(4, 8);
        let (d8_capped, o8_capped) = outage(8, 4);
        assert!(d1 > d4);
        assert!(o1.availability < o4.availability);
        assert!(o1.net_electricity_mwh.unwrap() < o4.net_electricity_mwh.unwrap());
        assert_eq!(d8_capped, d4);
        assert_eq!(o8_capped, o4);
    }
}
