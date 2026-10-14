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

const NO_CONTINGENCY: &str = "the paper states no percentage contingency; its handling times already include equipment-failure recovery (about 20 % of the total, p. 5) and transporter delays (Table 1)";

/// Months Crofts and Harman add around the handling time: "a 1 month cooling period before
/// remote handing operations begin and a 1 month period to condition and pump-down before
/// start-up" (arXiv:1412.4008, Sec. 5.2, p. 5).
pub const CROFTS_COOLING_MONTHS: f64 = 1.0;
pub const CROFTS_CONDITIONING_MONTHS: f64 = 1.0;

/// What every Crofts and Harman campaign replaces: all blankets and divertor cassettes through
/// all sixteen upper and sixteen divertor ports, five blanket segments and three cassettes per
/// sector (Sec. 2 and 4). Replacing only the blankets takes as long, because the divertor
/// must come out first (p. 4).
const CROFTS_SCOPE: &str = "all blanket segments and divertor cassettes of EU-DEMO (2012), through all 16 upper and 16 divertor ports";

/// One campaign from Table 1 of Crofts and Harman (p. 5) with the paper's cooling and
/// conditioning months (Sec. 5.2) added, so the duration is the whole outage. The handling
/// months include the paper's recovery and transporter delays and its shift and operator
/// productivity factors.
fn crofts_campaign(
    id: &'static str,
    title: &'static str,
    systems: u32,
    conditions: &'static str,
) -> MaintenancePreset {
    let handling = CROFTS_TABLE1_MONTHS
        .iter()
        .find(|(s, _)| *s == systems)
        .map(|(_, m)| *m)
        .expect("a Table 1 row");
    MaintenancePreset {
        id,
        title,
        label: LITERATURE_LABEL,
        duration_months: CROFTS_COOLING_MONTHS + handling + CROFTS_CONDITIONING_MONTHS,
        scope: CROFTS_SCOPE,
        parallel_systems: Condition::stated(systems),
        ports: Condition::stated(32),
        // 16 sectors x (5 blanket segments + 3 divertor cassettes).
        modules: Condition::stated(128),
        contingency_fraction: Condition::missing(NO_CONTINGENCY),
        conditions,
        citation: CROFTS_CITATION,
        source_link: Condition::stated(CROFTS_LINK),
    }
}

/// The three presets OPS-032 names, read from the paper's full text (arXiv:1412.4008, read
/// 2026-10-14): Table 1 handling months at 4, 8 and 1 remote-handling systems, plus one
/// month of cooling and one of conditioning (Sec. 5.2).
pub fn maintenance_presets() -> Vec<MaintenancePreset> {
    vec![
        crofts_campaign(
            "demo-blanket-divertor-4",
            "EU-DEMO blankets and divertor, 4 remote-handling systems",
            4,
            "5.9 months of handling with 4 systems in parallel, each working 4 sectors (Table 1, p. 5; the paper's \"about 6 months\", p. 4), plus 1 month of cooling and 1 month of conditioning and pump-down (Sec. 5.2)",
        ),
        crofts_campaign(
            "demo-blanket-divertor-8",
            "EU-DEMO blankets and divertor, 8 systems (shortest in the paper)",
            8,
            "3.2 months of handling with 8 systems in parallel, each working 2 sectors (Table 1, p. 5), plus 1 month of cooling and 1 month of conditioning and pump-down (Sec. 5.2)",
        ),
        crofts_campaign(
            "demo-blanket-divertor-1",
            "EU-DEMO blankets and divertor, 1 system (longest in the paper)",
            1,
            "22 months of handling with one system working all 16 sectors in turn (Table 1, p. 5; about 1,000 hours per sector, p. 4), plus 1 month of cooling and 1 month of conditioning and pump-down (Sec. 5.2)",
        ),
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
    fn presets_carry_the_paper_values_label_link_and_conditions() {
        let p = maintenance_presets();
        let by = |id: &str| p.iter().find(|x| x.id == id).unwrap();
        // Table 1 handling months plus 1 month cooling and 1 month conditioning (Sec. 5.2).
        assert!((by("demo-blanket-divertor-4").duration_months - 7.9).abs() < 1e-9);
        assert!((by("demo-blanket-divertor-8").duration_months - 5.2).abs() < 1e-9);
        assert!((by("demo-blanket-divertor-1").duration_months - 24.0).abs() < 1e-9);
        for (id, systems) in [
            ("demo-blanket-divertor-4", 4),
            ("demo-blanket-divertor-8", 8),
            ("demo-blanket-divertor-1", 1),
        ] {
            let x = by(id);
            assert_eq!(x.parallel_systems.value, Some(systems));
            assert_eq!(x.ports.value, Some(32));
            assert_eq!(x.modules.value, Some(128));
            // The paper states no percentage contingency, and the preset says so.
            assert!(x.contingency_fraction.value.is_none());
            assert!(
                x.contingency_fraction
                    .missing_reason
                    .unwrap()
                    .contains("p. 5")
            );
        }
        for x in &p {
            assert_eq!(x.label, "literature");
            assert_eq!(x.source_link.value, Some(CROFTS_LINK));
            assert_eq!(x.input_label().text(), "literature");
            assert!(x.conditions.contains("Table 1") && x.conditions.contains("Sec. 5.2"));
            let json = serde_json::to_string(x).unwrap();
            assert!(json.contains("\"label\":\"literature\""));
        }
        let four = by("demo-blanket-divertor-4");
        assert!((four.duration_s() - 7.9 * 30.4375 * 86_400.0).abs() < 1e-6);
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
    fn model_with_the_papers_cooling_and_conditioning_gives_the_presets() {
        // The fitted handling model plus the paper's own 1 + 1 months reproduces each preset's
        // whole outage within 10 %.
        for preset in maintenance_presets() {
            let systems = preset.parallel_systems.value.unwrap();
            let m = RemoteHandling {
                cooling_months: CROFTS_COOLING_MONTHS,
                conditioning_months: CROFTS_CONDITIONING_MONTHS,
                ..RemoteHandling::crofts_table1_fit()
            }
            .with_systems(systems)
            .with_ports(systems);
            let got = m.outage_months().unwrap();
            let want = preset.duration_months;
            assert!(
                (got - want).abs() / want <= 0.10,
                "{}: {got} vs {want}",
                preset.id
            );
        }
    }
}
