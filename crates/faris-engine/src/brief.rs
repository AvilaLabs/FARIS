//! Shared study-brief arithmetic: per-arrangement numbers, the contrasts between
//! arrangements, the plain-language takeaway, assumption rows with provenance and
//! the caveats a reader needs. The desktop comparison view and the exported
//! summary both read these, so the two cannot disagree.
//!
//! Everything here is a presentation of recorded transport results and
//! calculated histories. Transport statements carry Monte Carlo sampling error
//! only; history statements are conditional on the authored assumptions.

use crate::{
    comparison::{
        MARGIN_BAND_SIGMAS, component_replacement_spans, crossing_time_band,
        difference_resolved_2sigma, first_crossing_relative_uncertainty, replacement_count_band,
    },
    history::{HistoryResult, JULIAN_YEAR_SECONDS},
    history_ensemble::{EnsembleStatus, HistoryEnsemble},
    output_uncertainty::{COUNT_BAND_BASIS, OutputUncertainty},
    reactor::ReactorRun,
};
use faris_model::history::{ComponentClass, OperatingHistoryAssumptions};
use faris_model::validation::{Output, Quantity, ValidationMarks, calculated, not_calculated};
use serde::Serialize;
use std::collections::BTreeMap;

/// What kind of statement a value or result is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// Calculated by the FARIS engine or a recorded solver run.
    Calculated,
    /// A numerical control or check that passed within its declared scope.
    Checked,
    /// An authored scenario assumption (tunable, not measured).
    Authored,
    /// A value taken from cited literature.
    Literature,
    /// Valid only under stated conditions (e.g. a cold-data surrogate).
    Conditional,
    /// Precision or coverage goal not yet met; result usable with care.
    Partial,
    /// Not evaluated; no claim either way.
    NotEvaluated,
    /// A failed check or an error.
    Failed,
}

impl StatusKind {
    pub fn rgb(self) -> [u8; 3] {
        match self {
            StatusKind::Calculated => [96, 165, 250],
            StatusKind::Checked => [74, 196, 140],
            StatusKind::Authored => [196, 160, 250],
            StatusKind::Literature => [110, 200, 210],
            StatusKind::Conditional => [170, 176, 190],
            StatusKind::Partial => [232, 178, 92],
            StatusKind::NotEvaluated => [140, 146, 160],
            StatusKind::Failed => [240, 110, 110],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StatusKind::Calculated => "calculated",
            StatusKind::Checked => "checked",
            StatusKind::Authored => "authored",
            StatusKind::Literature => "literature",
            StatusKind::Conditional => "conditional",
            StatusKind::Partial => "partial",
            StatusKind::NotEvaluated => "not evaluated",
            StatusKind::Failed => "failed",
        }
    }
}

/// Variant ids of the two allocations, in `Arrangement` order.
pub const VARIANTS: [&str; 2] = ["reference", "breeder-emphasis"];

/// One of the four recorded arrangements: with or without the outboard port,
/// reference or breeder-heavy blanket/shield allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrangement {
    pub port: bool,
    pub breeder: bool,
}

impl Arrangement {
    /// Presentation order: port and no-port of the reference allocation, then of
    /// the breeder-heavy allocation.
    pub const ORDER: [Arrangement; 4] = [
        Arrangement {
            port: true,
            breeder: false,
        },
        Arrangement {
            port: false,
            breeder: false,
        },
        Arrangement {
            port: true,
            breeder: true,
        },
        Arrangement {
            port: false,
            breeder: true,
        },
    ];

    /// Series colour: port is warm and no-port cool; the breeder-heavy
    /// allocation is the lighter shade of each family.
    pub fn rgb(self) -> [u8; 3] {
        match (self.port, self.breeder) {
            (true, false) => [232, 104, 52],
            (true, true) => [250, 186, 104],
            (false, false) => [66, 133, 235],
            (false, true) => [138, 205, 250],
        }
    }

    pub fn label(self) -> &'static str {
        match (self.port, self.breeder) {
            (true, false) => "Port · reference",
            (true, true) => "Port · breeder-heavy",
            (false, false) => "No port · reference",
            (false, true) => "No port · breeder-heavy",
        }
    }

    /// Stable identifier for data files.
    pub fn id(self) -> &'static str {
        match (self.port, self.breeder) {
            (true, false) => "port-reference",
            (true, true) => "port-breeder-heavy",
            (false, false) => "no-port-reference",
            (false, true) => "no-port-breeder-heavy",
        }
    }
}

/// Blanket thickness of the two recorded allocations, metres (the shield takes
/// the remainder of the same 0.9 m).
pub const REFERENCE_BLANKET_M: f64 = 0.45;
pub const BREEDER_BLANKET_M: f64 = 0.55;
pub const BLANKET_PLUS_SHIELD_M: f64 = 0.9;

/// Every number of the per-arrangement summaries and the contrasts between them, by field name:
/// a calculated number names its quantity, any other says why it is not one. The magnet swap
/// count and the first swap follow from the fluence against the authored limit, so they carry the
/// fluence state. These types are not stored, so there is no field to hold the marks: exports
/// read them with `validation()`.
pub const BRIEF_OUTPUTS: &[Output] = &[
    calculated("breeder_h3_per_source", Quantity::Tbr),
    calculated("total_h3_per_source", Quantity::Tbr),
    calculated("breeding", Quantity::Tbr),
    calculated("breeding_pct", Quantity::Tbr),
    calculated("magnet_flux", Quantity::Flux),
    calculated("flux_pct", Quantity::Flux),
    calculated("nuclear_heat_w", Quantity::Heating),
    calculated("swaps", Quantity::Fluence),
    calculated("first_swap_y", Quantity::Fluence),
    calculated("first_swap_relative_sampling", Quantity::Fluence),
    calculated("net_twh", Quantity::NetElectricPower),
    calculated("final_tritium_kg", Quantity::TritiumInventory),
    calculated("magnet_replacements", Quantity::Fluence),
    calculated("first_magnet_replacement_years", Quantity::Fluence),
    calculated("magnet_permanent_limit_years", Quantity::Fluence),
    calculated("net_electricity_twh", Quantity::NetElectricPower),
    calculated("final_available_tritium_kg", Quantity::TritiumInventory),
    not_calculated(
        "magnets_replaceable",
        "whether the authored limit class allows swaps",
    ),
    calculated("swaps_two_sigma", Quantity::Fluence),
    not_calculated(
        "uncertainty",
        "the uncertainty state of the other fields, not a result",
    ),
    not_calculated(
        "swaps_band",
        "whether two swap counts' bands separate, a verdict on numbers already tagged",
    ),
    not_calculated(
        "first_swap_band",
        "whether two first-swap times' bands separate, a verdict on numbers already tagged",
    ),
    not_calculated("horizon_years", "authored operating-history horizon"),
    not_calculated("recorded", "whether a transport record is loaded"),
    not_calculated(
        "breeding_is_total",
        "which H3 response the breeding value is",
    ),
];

/// The validation state of each calculated output of the summaries and contrasts (VAL-054).
pub fn validation() -> ValidationMarks {
    ValidationMarks::for_catalogue(BRIEF_OUTPUTS)
}

/// Per-arrangement transport numbers as (mean, standard error).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransportSummary {
    /// Breeder-only H3 births per source neutron.
    pub breeder_h3_per_source: Option<(f64, f64)>,
    /// Whole-model H3 production per source neutron.
    pub total_h3_per_source: Option<(f64, f64)>,
    /// Magnet-envelope mean neutron flux, n/m²/s.
    pub magnet_flux: Option<(f64, f64)>,
    /// Whole-model total nuclear heating, W.
    pub nuclear_heat_w: Option<(f64, f64)>,
}

/// Recorded, already-normalized transport quantities for one arrangement.
/// Values are mean and one Monte Carlo standard error; nothing is computed
/// beyond the per-source-neutron normalization.
pub fn transport_summary(run: &ReactorRun) -> TransportSummary {
    let Some(normalized) = run.normalized.as_ref() else {
        return TransportSummary::default();
    };
    let rate = normalized.source_neutron_rate_per_s;
    let response = |id: &str| normalized.results.iter().find(|r| r.response_id == id);
    let per_source = |id: &str| {
        let tally = response(id)?;
        Some((
            tally.integrated_mean / rate,
            tally.integrated_standard_error / rate,
        ))
    };
    TransportSummary {
        breeder_h3_per_source: per_source("blanket-tritium"),
        total_h3_per_source: per_source("total-tritium-production"),
        magnet_flux: response("magnets-flux").map(|t| (t.mean, t.standard_error)),
        nuclear_heat_w: response("heating-total-whole-model")
            .map(|t| (t.integrated_mean, t.integrated_standard_error)),
    }
}

/// Numbers for one arrangement. None means the source was not recorded or not
/// calculated; nothing is filled in.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ArrangementSummary {
    pub recorded: bool,
    pub breeding: Option<(f64, f64)>,
    pub breeding_is_total: bool,
    pub magnet_flux: Option<(f64, f64)>,
    pub swaps: Option<usize>,
    /// Fewest and most swaps at two standard errors of the driving flux, to first order; `None`
    /// without a swap count or a flux error.
    pub swaps_two_sigma: Option<(usize, usize)>,
    pub first_swap_y: Option<f64>,
    pub first_swap_relative_sampling: Option<f64>,
    pub net_twh: Option<f64>,
    pub final_tritium_kg: Option<f64>,
    pub horizon_years: f64,
    /// The uncertainty state of every number above, by field name: an interval with its basis,
    /// or why there is none and what to do next.
    pub uncertainty: BTreeMap<String, OutputUncertainty>,
}

impl ArrangementSummary {
    /// The first swap as an earliest and a latest year at two standard errors of the driving
    /// flux. The earliest edge is the unfavourable one; the latest is `None` when the band
    /// reaches zero flux. `None` without a first swap or a sampling error.
    pub fn first_swap_two_sigma_y(&self) -> Option<(f64, Option<f64>)> {
        crossing_time_band(self.first_swap_y?, self.first_swap_relative_sampling?)
    }

    /// Fill the uncertainty state of each number from what the summary holds. A transport
    /// value carries its 2-sigma sampling band; swap count and first swap carry the first-order
    /// flux band when there is a flux error; the plant outputs say why a single history has no
    /// interval. Nothing is invented: a value that is absent says so.
    fn fill_uncertainty(&mut self) {
        let no_flux_error = || {
            OutputUncertainty::none(
                "the flux that drives the magnet limit has no standard error in the transport record",
                "record the transport run with batch statistics so the flux carries a standard error",
            )
        };
        let tally = |value: Option<(f64, f64)>, what: &str| match value {
            Some((m, se)) => OutputUncertainty::interval(
                m - MARGIN_BAND_SIGMAS * se,
                Some(m + MARGIN_BAND_SIGMAS * se),
                "2 sigma Monte Carlo sampling error of the tally; no nuclear data or model uncertainty",
            ),
            None => OutputUncertainty::absent(what),
        };
        let swaps = match (self.swaps, self.swaps_two_sigma) {
            (None, _) => OutputUncertainty::absent("the swap count"),
            (Some(_), Some((fewest, most))) => {
                OutputUncertainty::interval(fewest as f64, Some(most as f64), COUNT_BAND_BASIS)
            }
            (Some(_), None) => no_flux_error(),
        };
        let first = match (self.first_swap_y, self.first_swap_two_sigma_y()) {
            (None, _) => OutputUncertainty::absent("the first swap"),
            (Some(_), Some((early, late))) => OutputUncertainty::interval(
                early,
                late,
                "2 sigma of the driving flux carried onto the crossing time (time scales as 1/flux); first order, sampling only, calendar outages not re-timed",
            ),
            (Some(_), None) => no_flux_error(),
        };
        let plant = |value: Option<f64>, what: &str| match value {
            Some(_) => OutputUncertainty::deterministic(),
            None => OutputUncertainty::absent(what),
        };
        self.uncertainty = [
            ("breeding", tally(self.breeding, "the breeding ratio")),
            ("magnet_flux", tally(self.magnet_flux, "the magnet flux")),
            ("swaps", swaps),
            (
                "swaps_two_sigma",
                OutputUncertainty::none(
                    "it is itself the 2 sigma range of the swap count",
                    "nothing to do; it is the interval of the swap count",
                ),
            ),
            ("first_swap_y", first),
            (
                "first_swap_relative_sampling",
                OutputUncertainty::none(
                    "it is itself the relative sampling error of the driving flux",
                    "nothing to do; it is the input of the first-swap band",
                ),
            ),
            ("net_twh", plant(self.net_twh, "net electricity")),
            (
                "final_tritium_kg",
                plant(self.final_tritium_kg, "the final tritium"),
            ),
            (
                "horizon_years",
                OutputUncertainty::none(
                    "it is the authored operating horizon, not a result",
                    "nothing to do; change the horizon in the assumptions",
                ),
            ),
        ]
        .into_iter()
        .map(|(name, state)| (name.to_owned(), state))
        .collect();
    }

    /// Replace the single-history state of the plant outputs by the ensemble range when the
    /// ensemble was evaluated: net electricity, final tritium and the swap count take the P5 to
    /// P95 range of the samples. A quantity the ensemble does not summarise keeps its state.
    pub fn attach_ensemble(&mut self, ensemble: &HistoryEnsemble) {
        if ensemble.status != EnsembleStatus::Evaluated {
            return;
        }
        let Some(summary) = &ensemble.summary else {
            return;
        };
        let basis = format!(
            "P5 to P95 of {} history ensemble samples; transport Monte Carlo sampling uncertainty only (not nuclear data, model or assumption uncertainty)",
            ensemble.samples_accepted
        );
        // The swap count is a discrete output: its P5 and P95 are the smallest counts whose
        // cumulative share of the samples reaches 5 % and 95 %.
        if let Some(counts) = summary
            .discrete
            .iter()
            .find(|d| d.name == "replacements:magnets")
            && self.uncertainty.contains_key("swaps")
        {
            let mut shares: Vec<(u32, f64)> = counts
                .categories
                .iter()
                .filter_map(|(count, share)| Some((count.parse().ok()?, share.fraction)))
                .collect();
            shares.sort_by_key(|(count, _)| *count);
            let (mut cumulative, mut low, mut high) = (0.0, None, None);
            for (count, share) in shares {
                cumulative += share;
                if low.is_none() && cumulative >= 0.05 - 1e-12 {
                    low = Some(count);
                }
                if high.is_none() && cumulative >= 0.95 - 1e-12 {
                    high = Some(count);
                }
            }
            if let (Some(low), Some(high)) = (low, high) {
                self.uncertainty.insert(
                    "swaps".to_owned(),
                    OutputUncertainty::interval(
                        f64::from(low),
                        Some(f64::from(high)),
                        basis.clone(),
                    ),
                );
            }
        }
        for (field, output, scale) in [
            ("net_twh", "cumulative_net_electricity_mwh", 1.0e6),
            ("final_tritium_kg", "final_available_tritium_kg", 1.0),
        ] {
            let Some(row) = summary.continuous.iter().find(|c| c.name == output) else {
                continue;
            };
            if self.uncertainty.contains_key(field) && row.n > 0 {
                self.uncertainty.insert(
                    field.to_owned(),
                    OutputUncertainty::interval(
                        row.p5.value / scale,
                        Some(row.p95.value / scale),
                        basis.clone(),
                    ),
                );
            }
        }
    }
}

/// Combine one arrangement's recorded transport summary and calculated history.
/// `transport` is None when no record is loaded for the arrangement.
pub fn summarize_arrangement(
    transport: Option<&TransportSummary>,
    history: Option<&HistoryResult>,
) -> ArrangementSummary {
    let Some(summary) = transport else {
        let mut empty = ArrangementSummary::default();
        empty.fill_uncertainty();
        return empty;
    };
    let mut cell = ArrangementSummary {
        recorded: true,
        breeding: summary
            .total_h3_per_source
            .or(summary.breeder_h3_per_source),
        breeding_is_total: summary.total_h3_per_source.is_some(),
        magnet_flux: summary.magnet_flux,
        ..ArrangementSummary::default()
    };
    if let Some(h) = history {
        let horizon_s = h.assumptions.horizon_s;
        cell.horizon_years = horizon_s / JULIAN_YEAR_SECONDS;
        if h.assumptions
            .service_limits
            .iter()
            .any(|l| l.component_id == "magnets")
        {
            let spans = component_replacement_spans(&h.events, "magnets", horizon_s);
            cell.swaps = Some(spans.len());
            cell.swaps_two_sigma = replacement_count_band(h, "magnets");
            cell.first_swap_y = spans.first().map(|(s, _)| s / JULIAN_YEAR_SECONDS);
            cell.first_swap_relative_sampling = first_crossing_flux(h)
                .and_then(|r| first_crossing_relative_uncertainty(r.mean, r.standard_error));
        }
        if let Some(last) = h.snapshots.last() {
            cell.net_twh = last.cumulative_net_electricity_mwh.map(|v| v / 1e6);
            cell.final_tritium_kg = Some(last.available_tritium_kg);
        }
    }
    cell.fill_uncertainty();
    cell
}

/// A change `to − from` for each compared quantity. Percentages are relative to
/// `from`; the flag says whether a transport difference exceeds 2σ.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contrast {
    pub breeding_pct: Option<(f64, bool)>,
    pub flux_pct: Option<(f64, bool)>,
    pub swaps: Option<i64>,
    pub first_swap_y: Option<f64>,
    pub net_twh: Option<f64>,
    /// Whether the two 2-sigma bands of the swap counts separate; `None` when not assessed.
    pub swaps_band: Option<BandVerdict>,
    /// The same for the first-swap years.
    pub first_swap_band: Option<BandVerdict>,
}

/// Whether the 2-sigma bands of two compared values separate. A difference counts as resolved
/// only when the unfavourable edge of one lies beyond the favourable edge of the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandVerdict {
    Separated,
    Overlapping,
    /// At least one side has no flux error, so there is no band to compare.
    NoBand,
}

impl BandVerdict {
    /// The clause a sentence about the difference carries; empty when the bands separate.
    pub fn note(self) -> &'static str {
        match self {
            BandVerdict::Separated => "",
            BandVerdict::Overlapping => " (within 2σ bands, not resolved)",
            BandVerdict::NoBand => " (no flux error, no band)",
        }
    }
}

fn bands_separate(a: (f64, Option<f64>), b: (f64, Option<f64>)) -> BandVerdict {
    let before = |x: (f64, Option<f64>), y: (f64, Option<f64>)| x.1.is_some_and(|late| late < y.0);
    if before(a, b) || before(b, a) {
        BandVerdict::Separated
    } else {
        BandVerdict::Overlapping
    }
}

fn percent_change(from: f64, to: f64) -> Option<f64> {
    (from != 0.0 && from.is_finite() && to.is_finite()).then(|| (to - from) / from * 100.0)
}

fn sampled_change(from: Option<(f64, f64)>, to: Option<(f64, f64)>) -> Option<(f64, bool)> {
    let ((a, sa), (b, sb)) = (from?, to?);
    Some((
        percent_change(a, b)?,
        difference_resolved_2sigma(a, sa, b, sb),
    ))
}

pub fn contrast(from: &ArrangementSummary, to: &ArrangementSummary) -> Contrast {
    Contrast {
        breeding_pct: sampled_change(from.breeding, to.breeding),
        flux_pct: sampled_change(from.magnet_flux, to.magnet_flux),
        swaps: from.swaps.zip(to.swaps).map(|(a, b)| b as i64 - a as i64),
        first_swap_y: from.first_swap_y.zip(to.first_swap_y).map(|(a, b)| b - a),
        net_twh: from.net_twh.zip(to.net_twh).map(|(a, b)| b - a),
        swaps_band: from.swaps.zip(to.swaps).map(|_| {
            match from.swaps_two_sigma.zip(to.swaps_two_sigma) {
                Some((a, b)) => bands_separate(
                    (a.0 as f64, Some(a.1 as f64)),
                    (b.0 as f64, Some(b.1 as f64)),
                ),
                None => BandVerdict::NoBand,
            }
        }),
        first_swap_band: from.first_swap_y.zip(to.first_swap_y).map(|_| {
            match from
                .first_swap_two_sigma_y()
                .zip(to.first_swap_two_sigma_y())
            {
                Some((a, b)) => bands_separate(a, b),
                None => BandVerdict::NoBand,
            }
        }),
    }
}

fn count_word(n: u64) -> String {
    match n {
        1 => "one".into(),
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        5 => "five".into(),
        other => other.to_string(),
    }
}

fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("{} {noun}", count_word(n))
    } else {
        format!("{} {noun}s", count_word(n))
    }
}

/// One plain-language sentence for moving blanket thickness at the expense of
/// shield (breeder-heavy versus reference), built from the contrast numbers.
pub fn allocation_takeaway(c: &Contrast, shift_cm: f64, horizon_years: f64) -> Option<String> {
    let (breeding, resolved) = c.breeding_pct?;
    let breeding_clause = if resolved {
        let verb = if breeding >= 0.0 { "raises" } else { "lowers" };
        format!("{verb} breeding by {:.1} %", breeding.abs())
    } else {
        format!("changes breeding by {breeding:+.1} % (within 2σ sampling noise)")
    };
    let mut tail = Vec::new();
    let mut worse_first = false;
    if let Some(d) = c.first_swap_y
        && d.abs() >= 0.05
    {
        worse_first = d < 0.0;
        tail.push(format!(
            "{} the first magnet swap {:.1} years {}{}",
            if d < 0.0 { "brings" } else { "pushes" },
            d.abs(),
            if d < 0.0 { "earlier" } else { "later" },
            c.first_swap_band.map_or("", BandVerdict::note)
        ));
    }
    if let Some(d) = c.swaps {
        let band = c.swaps_band.map_or("", BandVerdict::note);
        tail.push(match d {
            0 => format!("leaves the swap count over {horizon_years:.0} years unchanged{band}"),
            d if d > 0 => format!(
                "adds {} over {horizon_years:.0} years{band}",
                plural(d.unsigned_abs(), "swap")
            ),
            d => format!(
                "removes {} over {horizon_years:.0} years{band}",
                plural(d.unsigned_abs(), "swap")
            ),
        });
    }
    let joiner = if worse_first && breeding > 0.0 && resolved {
        "but"
    } else {
        "and"
    };
    let mut sentence =
        format!("Shifting {shift_cm:.0} cm from shield to blanket {breeding_clause}");
    if let Some((first, rest)) = tail.split_first() {
        sentence.push_str(&format!(" {joiner} {first}"));
        if let Some(last) = rest.first() {
            sentence.push_str(&format!(" and {last}"));
        }
    }
    sentence.push('.');
    Some(sentence)
}

/// One sentence for the open outboard port in the reference allocation
/// (`c` is port minus no-port).
pub fn port_takeaway(c: &Contrast, horizon_years: f64) -> Option<String> {
    let swaps = c.swaps?;
    let net = c.net_twh?;
    let band = c.swaps_band.map_or("", BandVerdict::note);
    let swap_clause = match swaps {
        0 => format!("leaves the magnet swap count over {horizon_years:.0} years unchanged{band}"),
        d if d > 0 => format!(
            "adds {} over {horizon_years:.0} years{band}",
            plural(d.unsigned_abs(), "magnet swap")
        ),
        d => format!(
            "removes {} over {horizon_years:.0} years{band}",
            plural(d.unsigned_abs(), "magnet swap")
        ),
    };
    Some(format!(
        "Adding the outboard port, an open duct with no shield plug, {swap_clause} and {} {:.2} TWh of lifetime net electricity (reference allocation).",
        if net < 0.0 { "costs" } else { "adds" },
        net.abs()
    ))
}

/// The four arrangements compared: `cells[port 0 / no-port 1][reference 0 /
/// breeder-heavy 1]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StudyComparison {
    /// Breeder-heavy minus reference, with the port.
    pub allocation_with_port: Contrast,
    /// Breeder-heavy minus reference, without the port.
    pub allocation_no_port: Contrast,
    /// Port minus no-port, reference allocation.
    pub port_reference: Contrast,
    /// Port minus no-port, breeder-heavy allocation.
    pub port_breeder: Contrast,
    pub horizon_years: f64,
    /// Generated sentences; empty when the inputs do not support any.
    pub takeaways: Vec<String>,
}

pub fn compare_study(cells: &[[ArrangementSummary; 2]; 2]) -> StudyComparison {
    let allocation_with_port = contrast(&cells[0][0], &cells[0][1]);
    let allocation_no_port = contrast(&cells[1][0], &cells[1][1]);
    let port_reference = contrast(&cells[1][0], &cells[0][0]);
    let port_breeder = contrast(&cells[1][1], &cells[0][1]);
    let horizon = cells[0][0].horizon_years.max(cells[0][1].horizon_years);
    let horizon_years = if horizon > 0.0 { horizon } else { 30.0 };
    let shift_cm = ((BREEDER_BLANKET_M - REFERENCE_BLANKET_M) * 100.0).round();
    let takeaways = [
        allocation_takeaway(&allocation_with_port, shift_cm, horizon_years),
        port_takeaway(&port_reference, horizon_years),
    ]
    .into_iter()
    .flatten()
    .collect();
    StudyComparison {
        allocation_with_port,
        allocation_no_port,
        port_reference,
        port_breeder,
        horizon_years,
        takeaways,
    }
}

/// Keep at most `max` points, always including the last, and every point that
/// is a local extreme of its stride bucket so sawtooth resets stay sharp.
pub fn decimate(points: Vec<[f64; 2]>, max: usize) -> Vec<[f64; 2]> {
    if points.len() <= max {
        return points;
    }
    let buckets = (max / 2).max(1);
    let stride = points.len().div_ceil(buckets);
    let mut out = Vec::with_capacity(max + 2);
    for chunk in points.chunks(stride) {
        let mut lo = 0;
        let mut hi = 0;
        for (i, p) in chunk.iter().enumerate() {
            if p[1] < chunk[lo][1] {
                lo = i;
            }
            if p[1] > chunk[hi][1] {
                hi = i;
            }
        }
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        out.push(chunk[a]);
        if b != a {
            out.push(chunk[b]);
        }
    }
    if let (Some(last), Some(tail)) = (points.last(), out.last())
        && last != tail
    {
        out.push(*last);
    }
    out
}

/// The magnet service limit of a history and whether it is a literature value.
/// The magnet may carry one limit per named region; the value is the lowest of
/// them, and it is called literature only when every one is. A limit moved
/// away from the selected preset (`preset_limit`) is always called authored.
pub fn magnet_limit(history: &HistoryResult, preset_limit: Option<f64>) -> Option<(f64, bool)> {
    let limits: Vec<_> = history
        .assumptions
        .service_limits
        .iter()
        .filter(|l| l.component_id == "magnets")
        .collect();
    let lowest = limits.iter().map(|l| l.limit).reduce(f64::min)?;
    let moved = preset_limit.is_some_and(|p| limits.iter().any(|l| limits_differ(p, l.limit)));
    let literature = limits
        .iter()
        .all(|l| l.provenance.to_ascii_lowercase().starts_with("literature"));
    Some((lowest, literature && !moved))
}

/// The flux that brings the magnet to its first service limit: for each limit
/// the flux of the response it names (the region average of a fast-flux limit,
/// else the component average), whichever crosses earliest.
fn first_crossing_flux(history: &HistoryResult) -> Option<&crate::history::ScalarRate> {
    let rates = &history.driving_rates;
    history
        .assumptions
        .service_limits
        .iter()
        .filter(|l| l.component_id == "magnets")
        .filter_map(|l| {
            let flux = rates
                .region_flux_n_m2_s
                .get(&l.response_id)
                .map(|r| &r.rate)
                .or_else(|| rates.component_average_flux_n_m2_s.get("magnets"))?;
            (flux.mean > 0.0 && flux.mean.is_finite()).then_some((l.limit / flux.mean, flux))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, flux)| flux)
}

/// Relative inequality used to detect a value edited away from its preset.
pub fn limits_differ(a: f64, b: f64) -> bool {
    (a - b).abs() > 1e-9 * a.abs().max(b.abs()).max(1e-300)
}

/// One authored, literature or calculated input of the study with its source.
#[derive(Clone, Debug, PartialEq)]
pub struct AssumptionRow {
    pub name: String,
    /// Human-readable value.
    pub value: String,
    pub value_number: Option<f64>,
    pub unit: String,
    pub kind: StatusKind,
    pub provenance: String,
}

const DAY_S: f64 = 86_400.0;

/// Every operating assumption behind the histories, with its kind and the
/// provenance the assumption file records. `fusion_power_mw` and `horizon` come
/// from the scenario; `preset_limit` is the selected preset's magnet limit.
pub fn assumption_rows(
    history: &HistoryResult,
    fusion_power_mw: f64,
    preset_limit: Option<f64>,
) -> Vec<AssumptionRow> {
    let a: &OperatingHistoryAssumptions = &history.assumptions;
    let mut rows = Vec::new();
    let mut push =
        |name: &str, value: f64, text: String, unit: &str, kind: StatusKind, provenance: &str| {
            rows.push(AssumptionRow {
                name: name.into(),
                value: text,
                value_number: Some(value),
                unit: unit.into(),
                kind,
                provenance: provenance.into(),
            });
        };
    let scenario = "Scenario operating plan; authored scaffold value, not an ARC design estimate.";
    push(
        "Fusion power",
        fusion_power_mw,
        format!("{fusion_power_mw}"),
        "MW",
        StatusKind::Authored,
        scenario,
    );
    push(
        "Operating horizon",
        a.horizon_s / JULIAN_YEAR_SECONDS,
        format!("{:.1}", a.horizon_s / JULIAN_YEAR_SECONDS),
        "years",
        StatusKind::Authored,
        scenario,
    );
    let general = a.provenance.as_str();
    push(
        "Initial usable tritium",
        a.initial_available_tritium_kg,
        format!("{}", a.initial_available_tritium_kg),
        "kg",
        StatusKind::Authored,
        general,
    );
    push(
        "Startup reserve",
        a.startup_reserve_kg,
        format!("{}", a.startup_reserve_kg),
        "kg",
        StatusKind::Authored,
        general,
    );
    push(
        "Restart inventory",
        a.restart_inventory_kg,
        format!("{}", a.restart_inventory_kg),
        "kg",
        StatusKind::Authored,
        general,
    );
    push(
        "Tritium recovery fraction",
        a.recovery_fraction,
        format!("{}", a.recovery_fraction),
        "fraction",
        StatusKind::Authored,
        general,
    );
    push(
        "Processing delay",
        a.processing_delay_s / DAY_S,
        format!("{}", a.processing_delay_s / DAY_S),
        "days",
        StatusKind::Authored,
        general,
    );
    let outage_days: f64 = a
        .planned_outages
        .iter()
        .map(|o| (o.end_s - o.start_s) / DAY_S)
        .sum();
    push(
        "Planned outages",
        outage_days,
        format!("{outage_days:.0}"),
        &format!("days ({} outages)", a.planned_outages.len()),
        StatusKind::Authored,
        a.planned_outages
            .first()
            .map_or("No planned outage declared.", |o| o.reason.as_str()),
    );
    for limit in &a.service_limits {
        let literature = limit
            .provenance
            .to_ascii_lowercase()
            .starts_with("literature")
            && (limit.component_id != "magnets"
                || preset_limit.is_none_or(|p| !limits_differ(p, limit.limit)));
        let class = match limit.class {
            ComponentClass::Permanent => "permanent",
            ComponentClass::Replaceable => "replaceable",
        };
        let several = a
            .service_limits
            .iter()
            .filter(|l| l.component_id == limit.component_id)
            .count()
            > 1;
        let region = if several {
            format!(", {}", limit.response_id)
        } else {
            String::new()
        };
        push(
            &format!("{} service limit ({class}{region})", limit.component_id),
            limit.limit,
            format!("{:e}", limit.limit),
            "n/m² fluence",
            if literature {
                StatusKind::Literature
            } else {
                StatusKind::Authored
            },
            &limit.provenance,
        );
        if let Some(duration) = limit.replacement_duration_s {
            push(
                &format!("{} replacement duration", limit.component_id),
                duration / DAY_S,
                format!("{}", duration / DAY_S),
                "days",
                StatusKind::Authored,
                "Authored outage length for swapping the component; not an engineering estimate.",
            );
        }
    }
    let e = &a.energy;
    let mut energy = |name: &str, value: Option<f64>, unit: &str| {
        if let Some(v) = value {
            push(
                name,
                v,
                format!("{v}"),
                unit,
                StatusKind::Authored,
                &e.provenance,
            );
        }
    };
    energy(
        "Alpha deposition fraction",
        e.alpha_deposition_fraction,
        "fraction",
    );
    energy(
        "Transport heat recovery fraction",
        e.transport_heat_recovery_fraction,
        "fraction",
    );
    energy(
        "Thermal-to-electric efficiency",
        e.thermal_to_electric_efficiency,
        "fraction",
    );
    energy(
        "Auxiliary power while operating",
        e.auxiliary_power_mw_while_operating,
        "MW",
    );
    energy(
        "Auxiliary power while off",
        e.auxiliary_power_mw_while_off,
        "MW",
    );
    let half_life_years =
        std::f64::consts::LN_2 / history.tritium_decay_constant_per_s / JULIAN_YEAR_SECONDS;
    push(
        "Tritium half-life",
        half_life_years,
        format!("{half_life_years:.2}"),
        "years",
        StatusKind::Literature,
        "Decay constant carried by the history model; the demo assumptions cite NUBASE2020 Table I (12.32 years).",
    );
    rows
}

/// A statement the reader should not over-read, with why it applies and what
/// would settle it.
#[derive(Clone, Debug, PartialEq)]
pub struct Caveat {
    pub kind: StatusKind,
    pub item: String,
    pub why: String,
    pub settle: String,
}

/// What a caveat list needs to know about the study.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaveatContext<'a> {
    pub arrangements: &'a [(Arrangement, ArrangementSummary)],
    pub sweep_points: usize,
    pub sweep_histories_ready: bool,
    pub view_image_included: bool,
    pub port_volume_unvalidated: bool,
}

/// Caveats in reading order. Every not-evaluated, partial or conditional item
/// says why and what would settle it.
pub fn caveats(context: &CaveatContext) -> Vec<Caveat> {
    let mut list = Vec::new();
    let mut add = |kind: StatusKind, item: &str, why: String, settle: &str| {
        list.push(Caveat {
            kind,
            item: item.into(),
            why,
            settle: settle.into(),
        });
    };
    let missing: Vec<&str> = context
        .arrangements
        .iter()
        .filter(|(_, s)| !s.recorded)
        .map(|(a, _)| a.label())
        .collect();
    if !missing.is_empty() {
        add(
            StatusKind::NotEvaluated,
            "Arrangements without a transport record",
            format!(
                "No recorded transport result is loaded for {}, so its cells and every difference involving it are blank rather than estimated.",
                missing.join(", ")
            ),
            "Load or run the matching transport record (bundle or run.json) for each missing arrangement.",
        );
    }
    let no_limit = context
        .arrangements
        .iter()
        .any(|(_, s)| s.recorded && s.horizon_years > 0.0 && s.swaps.is_none());
    if no_limit {
        add(
            StatusKind::NotEvaluated,
            "Magnet swaps where no limit is declared",
            "The selected assumptions declare no magnet service limit, so no swap count or first-swap year is inferred for those arrangements.".into(),
            "Select assumptions that declare a magnet service limit, for example the literature REBCO value.",
        );
    }
    let no_energy = context
        .arrangements
        .iter()
        .any(|(_, s)| s.recorded && s.horizon_years > 0.0 && s.net_twh.is_none());
    if no_energy {
        add(
            StatusKind::NotEvaluated,
            "Net electricity where no energy ledger exists",
            "The history has no energy result: the transport record lacks the whole-model heating response or the assumptions omit the energy conversion.".into(),
            "Record the whole-model total-particle heating response and declare the energy assumptions.",
        );
    }
    let flux_rse = context
        .arrangements
        .iter()
        .filter_map(|(_, s)| {
            s.magnet_flux
                .filter(|(m, _)| *m > 0.0)
                .map(|(m, se)| se / m)
        })
        .fold(None, |acc: Option<(f64, f64)>, r| match acc {
            Some((lo, hi)) => Some((lo.min(r), hi.max(r))),
            None => Some((r, r)),
        });
    match flux_rse {
        Some((lo, hi)) if hi > 0.05 => add(
            StatusKind::Partial,
            "Magnet-region flux precision",
            format!(
                "The magnet-region flux has a relative sampling error of {}, above the 5 % goal, and it drives every swap year.",
                if format!("{:.0}", lo * 100.0) == format!("{:.0}", hi * 100.0) {
                    format!("{:.0} % in every arrangement", hi * 100.0)
                } else {
                    format!(
                        "{:.0} % to {:.0} % across the arrangements",
                        lo * 100.0,
                        hi * 100.0
                    )
                }
            ),
            "More source histories in the magnet envelope, or variance reduction, until the relative error is under the predeclared 5 % goal.",
        ),
        Some(_) => {}
        None => add(
            StatusKind::NotEvaluated,
            "Magnet-region flux precision",
            "No arrangement has a recorded magnet-region flux, so its precision cannot be stated."
                .into(),
            "Record the magnet-flux response for each arrangement.",
        ),
    }
    add(
        StatusKind::Partial,
        "Two-sigma flags and first-swap uncertainty",
        "Flags compare differences with 2·√(SE₁²+SE₂²) of independent runs with distinct seeds; covariance is not modelled. The first-swap and swap-count bands are the flux error on the crossing time (first order, sampling only), a lower bound.".into(),
        "Propagate joint Monte Carlo uncertainty through the histories, with replicate runs or paired seeds, and add volume and nuclear-data uncertainty.",
    );
    add(
        StatusKind::Conditional,
        "Magnet swaps and first swap",
        "The authored limit applies to the fast fluence (E > 0.1 MeV) averaged over each named magnet region (inboard, outboard, port sector), each region flux times operating time; the first region to reach it triggers the swap. The local peak within a region is not resolved, so a hot spot can reach the limit sooner. Not a qualified lifetime.".into(),
        "A local peak-flux tally in the winding pack with variance reduction, and REBCO irradiation data at operating temperature.",
    );
    add(
        StatusKind::Conditional,
        "Lifetime net electricity",
        "Signed net of gross output and auxiliary load under authored conversion assumptions, fed by the whole-model transport heating. Not a plant estimate; source-to-heating energy closure is not evaluated.".into(),
        "A plant thermal-cycle model with measured or designed efficiency and auxiliary loads, and closed energy accounting between source and heating.",
    );
    add(
        StatusKind::Conditional,
        "Tritium breeding and magnet flux",
        "Results come from cold-data (293.6 K) OpenMC fixed-source runs with an authored 14.1 MeV uniform source and surrogate materials; bound thermal scattering is not modelled and nuclear-data provenance is not authenticated. Qualification is NOT_EVALUATED.".into(),
        "An independent benchmark against measured or higher-fidelity transport, authenticated nuclear data and hot-temperature or thermal-scattering sensitivity.",
    );
    if context.port_volume_unvalidated {
        add(
            StatusKind::Partial,
            "Port geometry volumes",
            "Volumes of the port intersection with each layer are estimates, not independently validated.".into(),
            "An independent geometry or Monte Carlo volume check of the port intersection.",
        );
    }
    add(
        StatusKind::NotEvaluated,
        "Activation, shutdown dose and safety",
        "No irradiation spectrum or inventory calculation supports these results, and no claim about waste, dose or safety is made.".into(),
        "An activation and decay-heat calculation on the recorded spectra with an evaluated activation library.",
    );
    if context.sweep_points == 0 {
        add(
            StatusKind::NotEvaluated,
            "Allocation sweep",
            "No allocation-sweep transport records are loaded, so the sweep charts and findings are absent.".into(),
            "Provide the sweep bundles (one recorded run per allocation).",
        );
    } else {
        add(
            StatusKind::Partial,
            "Allocation sweep",
            format!(
                "Each of the {} allocations is an independent run with its own seed; transport statements carry sampling error only and history statements follow from authored limits.",
                context.sweep_points
            ),
            "Replicate each allocation with more histories and propagate the error into the histories.",
        );
        if !context.sweep_histories_ready {
            add(
                StatusKind::NotEvaluated,
                "Sweep histories",
                "The sweep operating histories were not available when the export ran, so sweep swaps and net electricity are missing.".into(),
                "Wait until the sweep histories finish calculating, then export again.",
            );
        }
    }
    if !context.view_image_included {
        add(
            StatusKind::NotEvaluated,
            "3D view image",
            "The 3D viewport could not be captured, so the summary has no model picture.".into(),
            "Export from the desktop app with the 3D view visible.",
        );
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(
        breeding: f64,
        flux: f64,
        swaps: usize,
        first: Option<f64>,
        net: f64,
    ) -> ArrangementSummary {
        ArrangementSummary {
            recorded: true,
            breeding: Some((breeding, 0.0014)),
            magnet_flux: Some((flux, flux * 0.14)),
            swaps: Some(swaps),
            first_swap_y: first,
            net_twh: Some(net),
            horizon_years: 30.0,
            ..ArrangementSummary::default()
        }
    }

    // Verifies: VAL-054
    #[test]
    fn every_field_of_the_summaries_is_classed() {
        // Naming every field, without `..`, stops compiling when one is added; the fix is to
        // class the new field in BRIEF_OUTPUTS below.
        let TransportSummary {
            breeder_h3_per_source: _,
            total_h3_per_source: _,
            magnet_flux: _,
            nuclear_heat_w: _,
        } = TransportSummary::default();
        let ArrangementSummary {
            recorded: _,
            breeding: _,
            breeding_is_total: _,
            magnet_flux: _,
            swaps: _,
            first_swap_y: _,
            first_swap_relative_sampling: _,
            net_twh: _,
            final_tritium_kg: _,
            horizon_years: _,
            swaps_two_sigma: _,
            uncertainty: _,
        } = ArrangementSummary::default();
        let Contrast {
            breeding_pct: _,
            flux_pct: _,
            swaps: _,
            first_swap_y: _,
            net_twh: _,
            swaps_band: _,
            first_swap_band: _,
        } = Contrast::default();
        let crate::sweep::HistorySummary {
            magnets_replaceable: _,
            magnet_replacements: _,
            first_magnet_replacement_years: _,
            magnet_permanent_limit_years: _,
            net_electricity_twh: _,
            final_available_tritium_kg: _,
            uncertainty: _,
        } = crate::sweep::HistorySummary {
            magnets_replaceable: true,
            magnet_replacements: 0,
            first_magnet_replacement_years: None,
            magnet_permanent_limit_years: None,
            net_electricity_twh: None,
            final_available_tritium_kg: 0.0,
            uncertainty: Default::default(),
        };
        let fields = [
            "magnets_replaceable",
            "magnet_replacements",
            "first_magnet_replacement_years",
            "magnet_permanent_limit_years",
            "net_electricity_twh",
            "final_available_tritium_kg",
            "breeder_h3_per_source",
            "total_h3_per_source",
            "magnet_flux",
            "nuclear_heat_w",
            "recorded",
            "breeding",
            "breeding_is_total",
            "swaps",
            "first_swap_y",
            "first_swap_relative_sampling",
            "net_twh",
            "final_tritium_kg",
            "horizon_years",
            "breeding_pct",
            "flux_pct",
            "swaps_two_sigma",
            "uncertainty",
            "swaps_band",
            "first_swap_band",
        ];
        for field in fields {
            assert!(
                BRIEF_OUTPUTS.iter().any(|o| o.name == field),
                "{field} is not in BRIEF_OUTPUTS"
            );
        }
        let marks = validation();
        for q in [
            Quantity::Tbr,
            Quantity::Flux,
            Quantity::Heating,
            Quantity::Fluence,
            Quantity::TritiumInventory,
            Quantity::NetElectricPower,
        ] {
            assert!(marks.quantities().contains(&q), "{q:?}");
        }
    }

    // Verifies: DSN-050, UNC-084
    #[test]
    fn contrast_is_second_minus_first() {
        let a = cell(1.295, 1.6e14, 4, Some(6.8), 34.13);
        let b = cell(1.312, 2.1e14, 5, Some(5.2), 33.72);
        let c = contrast(&a, &b);
        assert_eq!(c.swaps, Some(1));
        assert!((c.first_swap_y.unwrap() + 1.6).abs() < 1e-9);
        assert!((c.net_twh.unwrap() + 0.41).abs() < 1e-9);
        let (breeding, resolved) = c.breeding_pct.unwrap();
        assert!((breeding - 1.3127).abs() < 1e-3);
        assert!(resolved);
        // A 31 % flux change on 14 % relative errors is within sampling noise.
        assert!(!c.flux_pct.unwrap().1);
    }

    // Verifies: MAG-004
    #[test]
    fn the_first_swap_is_shown_with_a_two_sigma_band() {
        let cell = ArrangementSummary {
            first_swap_y: Some(10.0),
            first_swap_relative_sampling: Some(0.1),
            ..ArrangementSummary::default()
        };
        let (early, late) = cell.first_swap_two_sigma_y().unwrap();
        assert!((early - 10.0 / 1.2).abs() < 1e-12 && (late.unwrap() - 10.0 / 0.8).abs() < 1e-12);
        let bare = ArrangementSummary {
            first_swap_relative_sampling: None,
            ..cell
        };
        assert_eq!(bare.first_swap_two_sigma_y(), None);
    }

    #[test]
    fn missing_swaps_leave_the_first_swap_delta_unset() {
        let none = cell(1.3, 1e14, 0, None, 35.4);
        let some = cell(1.3, 1e14, 4, Some(6.8), 34.1);
        let c = contrast(&none, &some);
        assert_eq!(c.swaps, Some(4));
        assert_eq!(c.first_swap_y, None);
        assert_eq!(contrast(&ArrangementSummary::default(), &some).swaps, None);
    }

    #[test]
    fn takeaway_follows_the_numbers() {
        let c = Contrast {
            breeding_pct: Some((1.3, true)),
            swaps: Some(1),
            first_swap_y: Some(-1.6),
            ..Contrast::default()
        };
        assert_eq!(
            allocation_takeaway(&c, 10.0, 30.0).unwrap(),
            "Shifting 10 cm from shield to blanket raises breeding by 1.3 % but brings the first magnet swap 1.6 years earlier and adds one swap over 30 years."
        );
        let flat = Contrast {
            breeding_pct: Some((0.4, false)),
            swaps: Some(0),
            ..Contrast::default()
        };
        assert_eq!(
            allocation_takeaway(&flat, 10.0, 30.0).unwrap(),
            "Shifting 10 cm from shield to blanket changes breeding by +0.4 % (within 2σ sampling noise) and leaves the swap count over 30 years unchanged."
        );
        assert!(allocation_takeaway(&Contrast::default(), 10.0, 30.0).is_none());
        let port = Contrast {
            swaps: Some(4),
            net_twh: Some(-1.31),
            ..Contrast::default()
        };
        assert_eq!(
            port_takeaway(&port, 30.0).unwrap(),
            "Adding the outboard port, an open duct with no shield plug, adds four magnet swaps over 30 years and costs 1.31 TWh of lifetime net electricity (reference allocation)."
        );
    }

    #[test]
    fn study_comparison_pairs_the_right_cells() {
        let cells = [
            [
                cell(1.30, 2.0e14, 5, Some(5.0), 33.0),
                cell(1.32, 2.4e14, 6, Some(4.0), 32.5),
            ],
            [
                cell(1.30, 1.0e14, 1, Some(20.0), 35.0),
                cell(1.32, 1.1e14, 1, Some(18.0), 34.8),
            ],
        ];
        let study = compare_study(&cells);
        assert_eq!(study.allocation_with_port.swaps, Some(1));
        assert_eq!(study.port_reference.swaps, Some(4));
        assert_eq!(study.port_breeder.swaps, Some(5));
        assert_eq!(study.allocation_no_port.swaps, Some(0));
        assert_eq!(study.takeaways.len(), 2);
        assert_eq!(study.horizon_years, 30.0);
    }

    #[test]
    fn decimation_bounds_points_and_keeps_extremes_and_end() {
        let points: Vec<[f64; 2]> = (0..11_000)
            .map(|i| {
                let t = f64::from(i);
                [t, (t % 1000.0)]
            })
            .collect();
        let out = decimate(points.clone(), 1500);
        assert!(out.len() <= 1500 + 2);
        assert_eq!(out.last(), points.last());
        assert!(out.iter().any(|p| p[1] == 999.0));
        assert!(out.iter().any(|p| p[1] == 0.0));
        assert_eq!(decimate(points[..100].to_vec(), 1500).len(), 100);
    }

    #[test]
    fn preset_edits_are_detected_with_tolerance() {
        assert!(!limits_differ(3e22, 3e22 * (1.0 + 1e-12)));
        assert!(limits_differ(3e22, 3.1e22));
    }

    // Verifies: PRV-012
    #[test]
    fn every_caveat_says_why_and_what_would_settle_it() {
        let arrangements: Vec<_> = Arrangement::ORDER
            .iter()
            .map(|a| (*a, ArrangementSummary::default()))
            .collect();
        let list = caveats(&CaveatContext {
            arrangements: &arrangements,
            ..CaveatContext::default()
        });
        assert!(list.len() >= 8);
        for caveat in &list {
            assert!(!caveat.why.trim().is_empty(), "{}", caveat.item);
            assert!(!caveat.settle.trim().is_empty(), "{}", caveat.item);
        }
        assert!(
            list.iter()
                .any(|c| c.item.contains("without a transport record"))
        );
    }

    fn regional_history(fluxes: [(f64, f64); 3], limits: [f64; 3]) -> HistoryResult {
        use crate::history::{RegionFluxRate, ScalarRate, run_operating_history};
        use faris_model::history::ServiceLimit;
        let mut rates = crate::fixtures::rates_without_covariance(0.05, 'a');
        let mut assumptions = crate::fixtures::assumptions();
        assumptions.service_limits.clear();
        for ((id, (mean, se)), limit) in
            ["magnets-inboard", "magnets-outboard", "magnets-port-sector"]
                .iter()
                .zip(fluxes)
                .zip(limits)
        {
            let response = format!("{id}-fast-flux");
            rates.region_flux_n_m2_s.insert(
                response.clone(),
                RegionFluxRate {
                    component_id: "magnets".into(),
                    region: None,
                    energy_min_ev: 1.0e5,
                    rate: ScalarRate {
                        mean,
                        standard_error: Some(se),
                        unit: "neutrons/m\u{b2}/s".into(),
                        response_id: response.clone(),
                    },
                },
            );
            assumptions.service_limits.push(ServiceLimit {
                component_id: "magnets".into(),
                class: ComponentClass::Replaceable,
                response_id: response,
                metric: crate::history::FAST_FLUX_REGION_METRIC.into(),
                unit: "neutrons/m\u{b2}".into(),
                limit,
                replacement_duration_s: Some(1.0e6),
                replacement_durations_s: None,
                provenance: "literature test".into(),
            });
        }
        run_operating_history(&assumptions, &rates).unwrap()
    }

    #[test]
    fn the_first_crossing_uses_the_region_that_reaches_its_limit_first() {
        // The port sector has the highest flux against equal limits, so its
        // 20 % error is the one carried onto the first swap.
        let h = regional_history(
            [(1.0e10, 1.0e8), (2.0e10, 4.0e8), (5.0e10, 1.0e10)],
            [3.0e11; 3],
        );
        let flux = first_crossing_flux(&h).unwrap();
        assert_eq!(flux.response_id, "magnets-port-sector-fast-flux");
        // Raising the port limit makes the outboard region the earliest.
        let h = regional_history(
            [(1.0e10, 1.0e8), (2.0e10, 4.0e8), (5.0e10, 1.0e10)],
            [3.0e11, 3.0e11, 3.0e12],
        );
        assert_eq!(
            first_crossing_flux(&h).unwrap().response_id,
            "magnets-outboard-fast-flux"
        );
    }

    #[test]
    fn the_magnet_limit_is_the_lowest_and_literature_only_when_all_are_unmoved() {
        let h = regional_history([(1.0e10, 1.0e8); 3], [3.0e11, 2.5e11, 4.0e11]);
        assert_eq!(magnet_limit(&h, None), Some((2.5e11, true)));
        // A preset limit that any of them differs from calls it authored.
        assert_eq!(magnet_limit(&h, Some(3.0e11)), Some((2.5e11, false)));
        let same = regional_history([(1.0e10, 1.0e8); 3], [3.0e11; 3]);
        assert_eq!(magnet_limit(&same, Some(3.0e11)), Some((3.0e11, true)));
    }

    fn fixture_summary() -> ArrangementSummary {
        let rates = crate::fixtures::rates_without_covariance(0.05, 'a');
        let h =
            crate::history::run_operating_history(&crate::fixtures::assumptions(), &rates).unwrap();
        let transport = TransportSummary {
            breeder_h3_per_source: Some((1.1, 0.002)),
            total_h3_per_source: Some((1.3, 0.003)),
            magnet_flux: Some((1.0e14, 5.0e12)),
            nuclear_heat_w: Some((9.0e8, 1.0e7)),
        };
        summarize_arrangement(Some(&transport), Some(&h))
    }

    // Verifies: UNC-080
    #[test]
    fn every_number_of_the_arrangement_summary_has_an_interval_or_a_reason() {
        let s = fixture_summary();
        // The fixture fills every field, so the audit sees them all.
        assert!(s.swaps.is_some() && s.first_swap_y.is_some() && s.swaps_two_sigma.is_some());
        assert!(s.net_twh.is_some() && s.final_tritium_kg.is_some() && s.breeding.is_some());
        crate::output_uncertainty::tests::assert_every_number_has_a_state(
            &serde_json::to_value(&s).unwrap(),
            &s.uncertainty,
            &["uncertainty"],
        );
        // Plant outputs of a single history say why they have no interval.
        for field in ["net_twh", "final_tritium_kg"] {
            assert!(!s.uncertainty[field].has_interval(), "{field}");
        }
        // Counts, times and transport tallies carry an interval.
        for field in ["swaps", "first_swap_y", "breeding", "magnet_flux"] {
            assert!(s.uncertainty[field].has_interval(), "{field}");
        }
        // An unrecorded arrangement still says why each number is absent.
        let empty = summarize_arrangement(None, None);
        assert!(empty.uncertainty.values().all(|u| !u.has_interval()));
        assert!(empty.uncertainty.keys().eq(s.uncertainty.keys()));
    }

    // Verifies: UNC-080
    #[test]
    fn a_flux_without_an_error_estimate_says_so_instead_of_showing_a_bare_count() {
        let mut rates = crate::fixtures::rates_without_covariance(0.05, 'a');
        for rate in rates.component_average_flux_n_m2_s.values_mut() {
            rate.standard_error = None;
        }
        let h =
            crate::history::run_operating_history(&crate::fixtures::assumptions(), &rates).unwrap();
        let s = summarize_arrangement(Some(&TransportSummary::default()), Some(&h));
        assert!(s.swaps.is_some() && s.swaps_two_sigma.is_none());
        for field in ["swaps", "first_swap_y"] {
            let text = s.uncertainty[field].text(1);
            assert!(
                text.contains("no standard error") && text.contains("next step"),
                "{text}"
            );
        }
    }

    // Verifies: UNC-080
    #[test]
    fn an_evaluated_ensemble_replaces_the_reason_with_its_range() {
        let mut s = fixture_summary();
        assert!(!s.uncertainty["net_twh"].has_interval());
        let ensemble = crate::fixtures::ensemble(20, 3, 'a');
        s.attach_ensemble(&ensemble);
        for field in ["net_twh", "final_tritium_kg", "swaps"] {
            let OutputUncertainty::Interval {
                lower,
                upper,
                basis,
            } = &s.uncertainty[field]
            else {
                panic!("{field} kept its reason");
            };
            assert!(lower <= &upper.unwrap(), "{field}");
            assert!(basis.contains("ensemble") && basis.contains("sampling uncertainty only"));
        }
        // A not-evaluated ensemble changes nothing.
        let mut other = fixture_summary();
        let mut failed = ensemble;
        failed.status = EnsembleStatus::NotEvaluated {
            why: "x".into(),
            next_step: "y".into(),
        };
        other.attach_ensemble(&failed);
        assert_eq!(other, fixture_summary());
    }

    // Verifies: MAG-004
    #[test]
    fn the_swap_count_carries_a_two_sigma_range_and_a_difference_needs_separated_bands() {
        let s = fixture_summary();
        let (fewest, most) = s.swaps_two_sigma.unwrap();
        let swaps = s.swaps.unwrap();
        assert!(fewest <= swaps && swaps <= most, "{fewest} {swaps} {most}");
        // Overlapping ranges are not called a difference; separated ranges are.
        let near = ArrangementSummary {
            swaps: Some(5),
            swaps_two_sigma: Some((4, 6)),
            ..ArrangementSummary::default()
        };
        let overlapping = ArrangementSummary {
            swaps: Some(6),
            swaps_two_sigma: Some((5, 7)),
            ..ArrangementSummary::default()
        };
        let far = ArrangementSummary {
            swaps: Some(9),
            swaps_two_sigma: Some((8, 10)),
            ..ArrangementSummary::default()
        };
        assert_eq!(
            contrast(&near, &overlapping).swaps_band,
            Some(BandVerdict::Overlapping)
        );
        assert_eq!(
            contrast(&near, &far).swaps_band,
            Some(BandVerdict::Separated)
        );
        let bare = ArrangementSummary {
            swaps: Some(9),
            ..ArrangementSummary::default()
        };
        assert_eq!(contrast(&near, &bare).swaps_band, Some(BandVerdict::NoBand));
        // The sentence says so, and says nothing extra when the bands separate.
        let c = |band| Contrast {
            breeding_pct: Some((1.3, true)),
            swaps: Some(1),
            swaps_band: band,
            ..Contrast::default()
        };
        assert!(
            allocation_takeaway(&c(Some(BandVerdict::Overlapping)), 10.0, 30.0)
                .unwrap()
                .contains("within 2σ bands")
        );
        assert!(
            allocation_takeaway(&c(Some(BandVerdict::NoBand)), 10.0, 30.0)
                .unwrap()
                .contains("no flux error, no band")
        );
        assert!(
            !allocation_takeaway(&c(Some(BandVerdict::Separated)), 10.0, 30.0)
                .unwrap()
                .contains("band")
        );
    }

    // Verifies: MAG-004
    #[test]
    fn first_swap_bands_that_overlap_are_not_called_earlier() {
        let early = ArrangementSummary {
            first_swap_y: Some(5.0),
            first_swap_relative_sampling: Some(0.1),
            ..ArrangementSummary::default()
        };
        let later = ArrangementSummary {
            first_swap_y: Some(5.5),
            first_swap_relative_sampling: Some(0.1),
            ..ArrangementSummary::default()
        };
        let far = ArrangementSummary {
            first_swap_y: Some(9.0),
            first_swap_relative_sampling: Some(0.1),
            ..ArrangementSummary::default()
        };
        assert_eq!(
            contrast(&early, &later).first_swap_band,
            Some(BandVerdict::Overlapping)
        );
        assert_eq!(
            contrast(&early, &far).first_swap_band,
            Some(BandVerdict::Separated)
        );
    }
}
