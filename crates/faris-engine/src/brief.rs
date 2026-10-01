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
        component_replacement_spans, difference_resolved_2sigma,
        first_crossing_relative_uncertainty,
    },
    history::{HistoryResult, JULIAN_YEAR_SECONDS},
    reactor::ReactorRun,
};
use faris_model::history::{ComponentClass, OperatingHistoryAssumptions};

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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrangementSummary {
    pub recorded: bool,
    pub breeding: Option<(f64, f64)>,
    pub breeding_is_total: bool,
    pub magnet_flux: Option<(f64, f64)>,
    pub swaps: Option<usize>,
    pub first_swap_y: Option<f64>,
    pub first_swap_relative_sampling: Option<f64>,
    pub net_twh: Option<f64>,
    pub final_tritium_kg: Option<f64>,
    pub horizon_years: f64,
}

/// Combine one arrangement's recorded transport summary and calculated history.
/// `transport` is None when no record is loaded for the arrangement.
pub fn summarize_arrangement(
    transport: Option<&TransportSummary>,
    history: Option<&HistoryResult>,
) -> ArrangementSummary {
    let Some(summary) = transport else {
        return ArrangementSummary::default();
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
            cell.first_swap_y = spans.first().map(|(s, _)| s / JULIAN_YEAR_SECONDS);
            cell.first_swap_relative_sampling = h
                .driving_rates
                .component_average_flux_n_m2_s
                .get("magnets")
                .and_then(|r| first_crossing_relative_uncertainty(r.mean, r.standard_error));
        }
        if let Some(last) = h.snapshots.last() {
            cell.net_twh = last.cumulative_net_electricity_mwh.map(|v| v / 1e6);
            cell.final_tritium_kg = Some(last.available_tritium_kg);
        }
    }
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
        format!("changes breeding by {breeding:+.1} % (within sampling noise)")
    };
    let mut tail = Vec::new();
    let mut worse_first = false;
    if let Some(d) = c.first_swap_y
        && d.abs() >= 0.05
    {
        worse_first = d < 0.0;
        tail.push(format!(
            "{} the first magnet swap {:.1} years {}",
            if d < 0.0 { "brings" } else { "pushes" },
            d.abs(),
            if d < 0.0 { "earlier" } else { "later" }
        ));
    }
    if let Some(d) = c.swaps {
        tail.push(match d {
            0 => format!("leaves the swap count over {horizon_years:.0} years unchanged"),
            d if d > 0 => format!(
                "adds {} over {horizon_years:.0} years",
                plural(d.unsigned_abs(), "swap")
            ),
            d => format!(
                "removes {} over {horizon_years:.0} years",
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

/// One sentence for the finite outboard port in the reference allocation
/// (`c` is port minus no-port).
pub fn port_takeaway(c: &Contrast, horizon_years: f64) -> Option<String> {
    let swaps = c.swaps?;
    let net = c.net_twh?;
    let swap_clause = match swaps {
        0 => format!("leaves the magnet swap count over {horizon_years:.0} years unchanged"),
        d if d > 0 => format!(
            "adds {} over {horizon_years:.0} years",
            plural(d.unsigned_abs(), "magnet swap")
        ),
        d => format!(
            "removes {} over {horizon_years:.0} years",
            plural(d.unsigned_abs(), "magnet swap")
        ),
    };
    Some(format!(
        "Adding the finite outboard port {swap_clause} and {} {:.2} TWh of lifetime net electricity (reference allocation).",
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
/// A limit moved away from the selected preset (`preset_limit`) is always
/// called authored.
pub fn magnet_limit(history: &HistoryResult, preset_limit: Option<f64>) -> Option<(f64, bool)> {
    let limit = history
        .assumptions
        .service_limits
        .iter()
        .find(|l| l.component_id == "magnets")?;
    let moved = preset_limit.is_some_and(|p| limits_differ(p, limit.limit));
    let literature = limit
        .provenance
        .to_ascii_lowercase()
        .starts_with("literature");
    Some((limit.limit, literature && !moved))
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
        push(
            &format!("{} service limit ({class})", limit.component_id),
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
        "Flags compare differences with 2·√(SE₁²+SE₂²) of independent runs with distinct seeds; covariance is not modelled. The first-swap uncertainty is the relative flux error carried onto the crossing time (first order, sampling only), a lower bound.".into(),
        "Propagate joint Monte Carlo uncertainty through the histories, with replicate runs or paired seeds, and add volume and nuclear-data uncertainty.",
    );
    add(
        StatusKind::Conditional,
        "Magnet swaps and first swap",
        "The authored fluence limit is applied to the component-average neutron flux of the magnet envelope times operating time. A volume average understates the local peak behind the port; total flux overstates the fast flux. Not a qualified lifetime.".into(),
        "A local peak-flux tally in the winding pack, fast-flux (E>0.1 MeV) scoring, and REBCO irradiation data at operating temperature.",
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
            "Shifting 10 cm from shield to blanket changes breeding by +0.4 % (within sampling noise) and leaves the swap count over 30 years unchanged."
        );
        assert!(allocation_takeaway(&Contrast::default(), 10.0, 30.0).is_none());
        let port = Contrast {
            swaps: Some(4),
            net_twh: Some(-1.31),
            ..Contrast::default()
        };
        assert_eq!(
            port_takeaway(&port, 30.0).unwrap(),
            "Adding the finite outboard port adds four magnet swaps over 30 years and costs 1.31 TWh of lifetime net electricity (reference allocation)."
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
}
