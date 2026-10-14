//! Literature maintenance-duration presets (OPS-032) and the remote-handling constraint model
//! (OPS-033).
//!
//! A preset is a labelled literature input, never a FARIS result: every preset carries the
//! label `literature`, its source link and the conditions it was derived under, and a
//! condition the repo does not state is left empty with the reason beside it.

use serde::{Deserialize, Serialize};

/// A month as the maintenance validation note counts it (365.25 / 12 days).
pub const MONTH_DAYS: f64 = 30.4375;
const DAY_S: f64 = 86_400.0;

/// The label every preset carries wherever its value travels.
pub const LITERATURE_LABEL: &str = "literature";

/// Crofts and Harman, "Maintenance duration estimate for a DEMO fusion power plant, based on
/// the EFDA WP12 pre-conceptual studies", Fusion Eng. Des. 89 (2014).
pub const CROFTS_CITATION: &str =
    "O. Crofts, J. Harman, Fusion Eng. Des. 89 (2014), arXiv:1412.4008";
pub const CROFTS_LINK: &str = "https://arxiv.org/pdf/1412.4008";

pub fn months_to_seconds(months: f64) -> f64 {
    months * MONTH_DAYS * DAY_S
}

/// What the input is, so the label survives into any table or chart built from it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputLabel {
    /// Typed by the user or a scenario author.
    Authored,
    /// Taken from a published source; `link` is empty only with a stated reason.
    Literature {
        preset_id: String,
        link: Option<String>,
    },
}

impl InputLabel {
    pub fn text(&self) -> &'static str {
        match self {
            InputLabel::Authored => "authored",
            InputLabel::Literature { .. } => LITERATURE_LABEL,
        }
    }
}

/// A condition a preset was derived under; `None` carries the reason it is not stated.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Condition<T> {
    pub value: Option<T>,
    /// Why the value is empty. Present exactly when `value` is `None`.
    pub missing_reason: Option<&'static str>,
}

impl<T> Condition<T> {
    const fn stated(value: T) -> Self {
        Self {
            value: Some(value),
            missing_reason: None,
        }
    }
    const fn missing(reason: &'static str) -> Self {
        Self {
            value: None,
            missing_reason: Some(reason),
        }
    }
}

/// One literature maintenance duration with the conditions it was derived under.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MaintenancePreset {
    pub id: &'static str,
    pub title: &'static str,
    /// Always `literature`.
    pub label: &'static str,
    pub duration_months: f64,
    pub scope: &'static str,
    pub parallel_systems: Condition<u32>,
    pub ports: Condition<u32>,
    pub modules: Condition<u32>,
    pub contingency_fraction: Condition<f64>,
    pub conditions: &'static str,
    pub citation: &'static str,
    pub source_link: Condition<&'static str>,
}

impl MaintenancePreset {
    pub fn duration_s(&self) -> f64 {
        months_to_seconds(self.duration_months)
    }

    /// The label to attach to a replacement duration taken from this preset.
    pub fn input_label(&self) -> InputLabel {
        InputLabel::Literature {
            preset_id: self.id.into(),
            link: self.source_link.value.map(str::to_owned),
        }
    }
}

const NOT_IN_REPO_RH: &str = "the requirement row does not state it and the paper has not been read for this case; confirm in arXiv:1412.4008 before use";

/// The three presets OPS-032 names, as the requirement row states them (values via search of
/// Crofts et al.; the row marks them unverified against the paper's full text).
pub fn maintenance_presets() -> Vec<MaintenancePreset> {
    let link = Condition::stated(CROFTS_LINK);
    vec![
        MaintenancePreset {
            id: "demo-blanket-full",
            title: "EU-DEMO full blanket replacement",
            label: LITERATURE_LABEL,
            duration_months: 10.0,
            scope: "all blanket modules",
            parallel_systems: Condition::stated(4),
            ports: Condition::stated(4),
            modules: Condition::stated(421),
            contingency_fraction: Condition::stated(0.20),
            conditions: "421 modules through 4 ports with 4 remote-handling systems in parallel, including 20 % contingency",
            citation: CROFTS_CITATION,
            source_link: link.clone(),
        },
        MaintenancePreset {
            id: "demo-blanket-best-case",
            title: "EU-DEMO blanket replacement, best case",
            label: LITERATURE_LABEL,
            duration_months: 6.0,
            scope: "all blanket modules",
            parallel_systems: Condition::missing(NOT_IN_REPO_RH),
            ports: Condition::missing(NOT_IN_REPO_RH),
            modules: Condition::missing(NOT_IN_REPO_RH),
            contingency_fraction: Condition::missing(NOT_IN_REPO_RH),
            conditions: "best case found in the paper's virtual-reality simulation of the handling sequence",
            citation: CROFTS_CITATION,
            source_link: link.clone(),
        },
        MaintenancePreset {
            id: "demo-internals",
            title: "EU-DEMO internal components (divertor then blanket)",
            label: LITERATURE_LABEL,
            duration_months: 20.0,
            scope: "divertor, then blanket, replaced one after the other",
            parallel_systems: Condition::missing(NOT_IN_REPO_RH),
            ports: Condition::missing(NOT_IN_REPO_RH),
            modules: Condition::missing(NOT_IN_REPO_RH),
            contingency_fraction: Condition::missing(NOT_IN_REPO_RH),
            conditions: "divertor and blanket handled sequentially, about 20 months in total",
            citation: CROFTS_CITATION,
            source_link: link,
        },
    ]
}

pub fn maintenance_preset(id: &str) -> Option<MaintenancePreset> {
    maintenance_presets().into_iter().find(|p| p.id == id)
}

/// Crofts and Harman Table 1: months to replace all EU-DEMO blankets and divertor cassettes
/// against the number of remote-handling systems working in parallel (read in full on
/// 2026-10-06, `docs/notes/MAINTENANCE_COUPLING_VALIDATION.md`). Literature.
pub const CROFTS_TABLE1_MONTHS: [(u32, f64); 5] =
    [(1, 22.0), (2, 11.0), (4, 5.9), (6, 4.4), (8, 3.2)];

/// Remote-handling constraints on one replacement campaign.
///
/// Assumptions, all authored here rather than read from a source:
/// - Handling work is shared evenly over the systems that can work at the same time, so the
///   handling time is `fixed_months + work_months / effective_systems`.
/// - A system needs one port and one crew to work, so `effective_systems` is the smallest of
///   the systems, the ports and the crews. `crews: None` means a crew for every system.
/// - Cooling before handling, conditioning and pump-down after it, and contingency are
///   separate inputs; contingency multiplies the whole outage.
/// - Systems never wait for each other and never fail (no reliability or queueing; see OPS-043).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoteHandling {
    pub parallel_systems: u32,
    pub ports: u32,
    pub crews: Option<u32>,
    /// Handling time that does not shorten with more systems, in months.
    pub fixed_months: f64,
    /// Handling work that is shared over the systems, in system-months.
    pub work_months: f64,
    pub cooling_months: f64,
    pub conditioning_months: f64,
    pub contingency_fraction: f64,
}

impl RemoteHandling {
    pub fn validate(&self) -> Result<(), String> {
        if self.parallel_systems == 0 || self.ports == 0 || self.crews == Some(0) {
            return Err(
                "remote handling needs at least one system, one port and one crew; raise the zero count"
                    .into(),
            );
        }
        for (v, name) in [
            (self.fixed_months, "fixed_months"),
            (self.work_months, "work_months"),
            (self.cooling_months, "cooling_months"),
            (self.conditioning_months, "conditioning_months"),
            (self.contingency_fraction, "contingency_fraction"),
        ] {
            if !v.is_finite() || v < 0.0 {
                return Err(format!("{name} must be zero or more"));
            }
        }
        Ok(())
    }

    /// Systems that can work at once, limited by ports and crews.
    pub fn effective_systems(&self) -> u32 {
        self.parallel_systems
            .min(self.ports)
            .min(self.crews.unwrap_or(u32::MAX))
    }

    /// Handling time alone, in months.
    pub fn handling_months(&self) -> f64 {
        self.fixed_months + self.work_months / f64::from(self.effective_systems())
    }

    /// Whole outage, in months.
    pub fn outage_months(&self) -> Result<f64, String> {
        self.validate()?;
        Ok(
            (self.cooling_months + self.handling_months() + self.conditioning_months)
                * (1.0 + self.contingency_fraction),
        )
    }

    pub fn outage_s(&self) -> Result<f64, String> {
        self.outage_months().map(months_to_seconds)
    }

    pub fn with_ports(&self, ports: u32) -> Self {
        Self {
            ports,
            ..self.clone()
        }
    }

    /// The same campaign with another count of systems; the outage-length-against-systems curve.
    pub fn with_systems(&self, parallel_systems: u32) -> Self {
        Self {
            parallel_systems,
            ..self.clone()
        }
    }

    /// Handling model fitted to Crofts Table 1 by least squares of months on 1 / systems, with
    /// no cooling, conditioning or contingency (the table gives the handling time itself).
    /// Ports equal systems, so the ports never bind. This is a calibration to five literature
    /// points, not a derivation from the paper's handling sequence.
    pub fn crofts_table1_fit() -> Self {
        let n = CROFTS_TABLE1_MONTHS.len() as f64;
        let xs: Vec<f64> = CROFTS_TABLE1_MONTHS
            .iter()
            .map(|(s, _)| 1.0 / f64::from(*s))
            .collect();
        let ys: Vec<f64> = CROFTS_TABLE1_MONTHS.iter().map(|(_, m)| *m).collect();
        let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
        let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
        let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
        let work = sxy / sxx;
        Self {
            parallel_systems: 4,
            ports: 4,
            crews: None,
            fixed_months: my - work * mx,
            work_months: work,
            cooling_months: 0.0,
            conditioning_months: 0.0,
            contingency_fraction: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: OPS-032
    #[test]
    fn presets_carry_the_row_values_label_link_and_conditions() {
        let p = maintenance_presets();
        let by = |id: &str| p.iter().find(|x| x.id == id).unwrap();
        assert_eq!(by("demo-blanket-full").duration_months, 10.0);
        assert_eq!(by("demo-blanket-best-case").duration_months, 6.0);
        assert_eq!(by("demo-internals").duration_months, 20.0);
        let full = by("demo-blanket-full");
        assert_eq!(full.parallel_systems.value, Some(4));
        assert_eq!(full.ports.value, Some(4));
        assert_eq!(full.modules.value, Some(421));
        assert_eq!(full.contingency_fraction.value, Some(0.20));
        for x in &p {
            assert_eq!(x.label, "literature");
            assert_eq!(x.source_link.value, Some(CROFTS_LINK));
            assert_eq!(x.input_label().text(), "literature");
            // A condition is empty only with its reason beside it.
            for (empty, reason) in [
                (
                    x.parallel_systems.value.is_none(),
                    x.parallel_systems.missing_reason,
                ),
                (x.ports.value.is_none(), x.ports.missing_reason),
                (x.modules.value.is_none(), x.modules.missing_reason),
                (
                    x.contingency_fraction.value.is_none(),
                    x.contingency_fraction.missing_reason,
                ),
            ] {
                assert_eq!(empty, reason.is_some(), "{}", x.id);
            }
            let json = serde_json::to_string(x).unwrap();
            assert!(json.contains("\"label\":\"literature\""));
        }
        // The two presets whose systems and ports the row does not state say so.
        assert!(
            by("demo-blanket-best-case")
                .parallel_systems
                .value
                .is_none()
        );
        assert!(by("demo-internals").ports.missing_reason.is_some());
        assert!((full.duration_s() - 10.0 * 30.4375 * 86_400.0).abs() < 1e-6);
        assert!(maintenance_preset("nope").is_none());
    }

    // Verifies: OPS-033
    #[test]
    fn model_reproduces_crofts_table1_within_ten_percent() {
        let fit = RemoteHandling::crofts_table1_fit();
        for (systems, published) in CROFTS_TABLE1_MONTHS {
            let got = fit
                .with_systems(systems)
                .with_ports(systems)
                .outage_months()
                .unwrap();
            assert!(
                (got - published).abs() / published <= 0.10,
                "{systems} systems: model {got:.2} vs published {published}"
            );
        }
    }

    // Verifies: OPS-033
    #[test]
    fn outage_shortens_with_systems_and_ports_and_crews_cap_it() {
        let fit = RemoteHandling::crofts_table1_fit();
        let months = |s: u32, ports: u32, crews: Option<u32>| {
            RemoteHandling {
                parallel_systems: s,
                ports,
                crews,
                ..fit.clone()
            }
            .outage_months()
            .unwrap()
        };
        assert!(months(1, 8, None) > months(2, 8, None));
        assert!(months(2, 8, None) > months(8, 8, None));
        // Extra systems behind too few ports or crews change nothing.
        assert_eq!(months(8, 4, None), months(4, 4, None));
        assert_eq!(months(8, 8, Some(2)), months(2, 8, None));
        // Two systems share the work exactly: handling is fixed + work / 2.
        let two = fit.with_systems(2).with_ports(2);
        assert!((two.handling_months() - (fit.fixed_months + fit.work_months / 2.0)).abs() < 1e-12);
        let zero = RemoteHandling { ports: 0, ..fit };
        assert!(zero.outage_months().unwrap_err().contains("at least one"));
    }

    // Verifies: OPS-033
    #[test]
    fn row_blanket_inputs_give_about_ten_months_if_table1_excludes_cooling() {
        // The paper's 10 months (4 systems, 4 ports, 20 % contingency) is matched only if
        // Table 1 leaves out the 1 month cooling and 1 month conditioning; whether it does is
        // not confirmed, so this checks consistency, not reproduction.
        let m = RemoteHandling {
            cooling_months: 1.0,
            conditioning_months: 1.0,
            contingency_fraction: 0.20,
            ..RemoteHandling::crofts_table1_fit()
        };
        let got = m.outage_months().unwrap();
        assert!((got - 10.0).abs() / 10.0 <= 0.10, "{got}");
    }
}
