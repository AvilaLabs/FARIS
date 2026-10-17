//! `faris waste`: classify an activated component's inventory under a regime you choose (10 CFR 61.55,
//! IAEA clearance, UK waste thresholds, UK out-of-scope values, Fetter limits), give its IAEA
//! clearance index and time, and its recycling index. Every command prints one JSON document.
//! Research screening, not a regulatory determination.

use crate::transport::{read_bounded, sha256, write_new_bytes};
use clap::{Subcommand, ValueEnum};
use faris_engine::maintenance::{MaintenanceResult, ReplacementSummary};
use faris_engine::waste_lifetime::{LifetimePlan, lifetime_from_replacements};
use faris_model::waste::{
    self, WasteInventory,
    clearance::{
        DoseRateContribution, RecyclingCriterion, SEARCH_END_YEARS, SEARCH_START_YEARS,
        recycling_index,
    },
    fetter,
    regime::{Regime, classify_regime, regime_time},
    tables, uk,
};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, ValueEnum)]
pub enum Scheme {
    /// US NRC 10 CFR 61.55 Tables 1 and 2: Class A, B, C or greater than Class C.
    #[value(name = "nrc-10cfr61.55")]
    Nrc,
    /// IAEA GSR Part 3 Table I.2: clearance index and the time it falls below 1.
    #[value(name = "iaea-gsr3-table-i2")]
    Iaea,
    /// UK LLW definition (4 GBq/te alpha, 12 GBq/te beta/gamma) and high-volume VLLW (4 MBq/te): VLLW, LLW or above LLW.
    #[value(name = "uk-llw-ilw")]
    UkLlwIlw,
    /// UK (England and Wales) Environmental Permitting Regulations 2016 Schedule 23 Table 2 out-of-scope concentrations.
    #[value(name = "uk-epr2016-sch23-table2")]
    UkOutOfScope,
    /// Fetter, Cheng and Mann (1990) Class C specific-activity limits for activated metal: sum of fractions.
    #[value(name = "fetter-specific-activity")]
    Fetter,
}

impl Scheme {
    fn regime(self) -> Regime {
        match self {
            Scheme::Nrc => Regime::UsNrc,
            Scheme::Iaea => Regime::Iaea,
            Scheme::UkLlwIlw => Regime::UkWaste,
            Scheme::UkOutOfScope => Regime::UkOutOfScope,
            Scheme::Fetter => Regime::Fetter,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum HistoryKind {
    /// The history run with the authored replacement durations.
    Fixed,
    /// The history run with the converged, computed durations.
    Computed,
}

#[derive(Subcommand)]
pub enum WasteCommand {
    /// List the classification schemes with name, version and date, and which are implemented.
    Schemes,
    /// Print the encoded limit tables with their source, retrieval date and licence.
    Tables {
        #[arg(long, value_enum, default_value = "nrc-10cfr61.55")]
        scheme: Scheme,
    },
    /// Classify an inventory at a disposal time under the regime you choose with --scheme. The
    /// regime and its jurisdiction profile (name, version, review date) are named on the result.
    /// Refuses (NOT_EVALUATED) when a needed nuclide, volume, mass or flag is missing, or when the
    /// profile is past its review date; it never defaults to Class A.
    Classify {
        /// faris waste inventory JSON: component, waste form, averaging volume and mass, nuclides.
        #[arg(long)]
        inventory: PathBuf,
        /// Years after the inventory reference at which the waste is disposed of.
        #[arg(long)]
        time_years: f64,
        #[arg(long, value_enum, default_value = "nrc-10cfr61.55")]
        scheme: Scheme,
        /// Write the receipt (scheme choice, scheme and profile versions, inventory hash, class) to
        /// this new file. The same receipt is in the printed result.
        #[arg(long)]
        receipt: Option<PathBuf>,
        /// Evaluation date YYYY-MM-DD for the profile review check. Default: today (UTC).
        #[arg(long)]
        date: Option<String>,
    },
    /// Waste mass and volume by class and component over plant life: each replacement batch and
    /// each end-of-life component counted once. A batch with no inventory is NOT_EVALUATED.
    Lifetime {
        /// Lifetime plan JSON: per component the pieces, mass and volume of one installation and
        /// the inventories its replaced and end-of-life installations are classified from.
        #[arg(long)]
        plan: PathBuf,
        /// A `maintenance run` result file; the replacements of `--design` are used.
        #[arg(long, requires = "design", conflicts_with = "replacements")]
        maintenance_result: Option<PathBuf>,
        #[arg(long)]
        design: Option<String>,
        #[arg(long, value_enum, default_value = "computed")]
        history: HistoryKind,
        /// Or a JSON list of replacements (class, component, k, start_s, end_s).
        #[arg(long)]
        replacements: Option<PathBuf>,
    },
    /// Find the time a scheme's sum of fractions falls below 1 (IAEA) or to at most 1 (the UK
    /// schedule, Fetter and UK VLLW sums), searched between two times. Decay only.
    ClearanceTime {
        #[arg(long)]
        inventory: PathBuf,
        /// The scheme whose sum is searched; 10 CFR 61.55 has no single index and is refused.
        #[arg(long, value_enum, default_value = "iaea-gsr3-table-i2")]
        scheme: Scheme,
        #[arg(long, default_value_t = SEARCH_START_YEARS)]
        from_years: f64,
        #[arg(long, default_value_t = SEARCH_END_YEARS)]
        to_years: f64,
    },
    /// Recycling index: a dose rate over a dose-rate criterion you write. FARIS supplies no criterion.
    Recycling {
        /// JSON with name, dose_rate_limit_sv_per_h and source.
        #[arg(long)]
        criterion: PathBuf,
        /// JSON list of per-nuclide dose rates with half-life and sigma.
        #[arg(long)]
        contributions: PathBuf,
        /// Years after the reference at which the contributions are given.
        #[arg(long, default_value_t = 0.0)]
        contributions_at_years: f64,
        #[arg(long)]
        time_years: f64,
    },
}

/// Today's date in UTC as `YYYY-MM-DD`. The model takes the date as an argument and never reads the clock.
fn today_utc() -> String {
    let unix_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = faris_model::display::civil_date(unix_s);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, Box<dyn std::error::Error>> {
    let bytes = read_bounded(path)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()).into())
}

fn print(value: &impl serde::Serialize) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn run(command: WasteCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        WasteCommand::Schemes => print(&serde_json::json!({
            "schemes": waste::schemes(),
            "research_screening": faris_model::RESEARCH_SCREENING_STATEMENT,
        })),
        WasteCommand::Tables { scheme } => match scheme {
            Scheme::Nrc => {
                let t = tables::nrc();
                print(&serde_json::json!({
                    "scheme": t.scheme_info(),
                    "source": {
                        "title": t.source.title, "url": t.source.url, "retrieved": t.source.retrieved,
                        "snapshot_sha256": t.source.snapshot_sha256, "licence": t.source.licence,
                    },
                    "table1": t.table1.iter().map(|r| serde_json::json!({
                        "row": r.label, "unit": r.unit, "limit": r.limit,
                        "applies_to": if r.scope == tables::Scope::ActivatedMetal { "activated metal" } else { "all other forms (and activated metal where no activated-metal row exists)" },
                    })).collect::<Vec<_>>(),
                    "table2": t.table2.iter().map(|r| serde_json::json!({
                        "row": r.label, "unit": r.unit, "col1": r.col1, "col2": r.col2, "col3": r.col3,
                        "applies_to": if r.scope == tables::Scope::ActivatedMetal { "activated metal" } else { "all other forms (and activated metal where no activated-metal row exists)" },
                    })).collect::<Vec<_>>(),
                    "null_means": "no limit is established for that column (10 CFR 61.55 Table 2 footnote 1)",
                }))
            }
            Scheme::Iaea => {
                let t = tables::iaea();
                print(&serde_json::json!({
                    "scheme": t.scheme_info(),
                    "unit": "Bq/g",
                    "levels": t.levels.iter().map(|l| serde_json::json!({
                        "nuclide": l.nuclide, "bq_per_g": l.bq_per_g,
                        "parent_with_progeny": l.parent_with_progeny, "printed_page": l.printed_page,
                    })).collect::<Vec<_>>(),
                }))
            }
            Scheme::UkLlwIlw => {
                let t = uk::uk_waste();
                print(&serde_json::json!({
                    "scheme": t.scheme_info(),
                    "sources": t.sources.iter().map(|s| serde_json::json!({
                        "tag": s.tag, "title": s.title, "kind": s.kind, "url": s.url,
                        "retrieved": s.retrieved, "original_sha256": s.original_sha256,
                    })).collect::<Vec<_>>(),
                    "extracts_file": t.snapshot_file,
                    "extracts_sha256": t.snapshot_sha256,
                    "licence": t.licence,
                    "llw": {"alpha_gbq_per_te": t.llw.alpha_gbq_per_te, "beta_gamma_gbq_per_te": t.llw.beta_gamma_gbq_per_te, "quote": t.llw.quote},
                    "vllw_high_volume": {"total_mbq_per_te": t.vllw_high_volume.total_mbq_per_te, "tritium_mbq_per_te": t.vllw_high_volume.tritium_mbq_per_te, "quote": t.vllw_high_volume.quote},
                    "vllw_low_volume": {
                        "per_0_1_m3_kbq": t.vllw_low_volume.per_0_1_m3_kbq, "per_item_kbq": t.vllw_low_volume.per_item_kbq,
                        "per_0_1_m3_c14_h3_kbq": t.vllw_low_volume.per_0_1_m3_c14_h3_kbq, "per_item_c14_h3_kbq": t.vllw_low_volume.per_item_c14_h3_kbq,
                        "evaluated_by_faris": t.vllw_low_volume.evaluated_by_faris, "reason": t.vllw_low_volume.reason,
                    },
                    "ilw": {"definition": t.ilw.quote, "quantitative": false},
                    "hlw": {"definition": t.hlw.quote, "quantitative": false, "reason": t.hlw.reason},
                }))
            }
            Scheme::UkOutOfScope => {
                let t = uk::uk_clearance();
                print(&serde_json::json!({
                    "scheme": t.scheme_info(),
                    "unit": "Bq/g",
                    "source": {
                        "title": t.source.title, "kind": t.source.kind, "url": t.source.url, "data_url": t.source.data_url,
                        "retrieved": t.source.retrieved, "original_sha256": t.source.original_sha256,
                        "snapshot_sha256": t.source.snapshot_sha256, "licence": t.source.licence,
                    },
                    "rule_quote": t.rule_quote,
                    "levels": t.levels.iter().map(|l| serde_json::json!({
                        "row": l.label, "nuclide": l.nuclide, "suffix": l.suffix, "bq_per_g": l.bq_per_g,
                    })).collect::<Vec<_>>(),
                    "table3_daughters": t.table3.iter().map(|r| serde_json::json!({"parent": r.parent, "daughters": r.daughters})).collect::<Vec<_>>(),
                }))
            }
            Scheme::Fetter => {
                let t = fetter::fetter();
                print(&serde_json::json!({
                    "scheme": t.scheme_info(),
                    "unit": t.unit,
                    "source": {
                        "title": t.source.title, "kind": t.source.kind, "url": t.source.url,
                        "retrieved": t.source.retrieved, "original_sha256": t.source.original_sha256,
                        "snapshot_sha256": t.source.snapshot_sha256, "licence": t.source.licence,
                    },
                    "tmsa_means": "theoretical maximum specific activity of the pure radionuclide: no limit can be reached",
                    "limits": t.limits.iter().map(|l| serde_json::json!({
                        "nuclide": l.nuclide, "half_life": l.half_life_text, "sal": l.sal_text,
                        "lower_ci_per_m3": l.sal_low_ci_per_m3, "upper_ci_per_m3": l.sal_high_ci_per_m3,
                        "upper_is_tmsa": l.sal_high_is_tmsa, "no_limit_tmsa": l.no_limit_tmsa, "quote": l.quote,
                    })).collect::<Vec<_>>(),
                }))
            }
        },
        WasteCommand::Classify {
            inventory,
            time_years,
            scheme,
            receipt,
            date,
        } => {
            let bytes = read_bounded(&inventory)?;
            let parsed: WasteInventory = serde_json::from_slice(&bytes)
                .map_err(|e| format!("{}: {e}", inventory.display()))?;
            let today = date.unwrap_or_else(today_utc);
            let report = classify_regime(
                scheme.regime(),
                &parsed,
                time_years,
                &today,
                "command flag --scheme",
                Some(sha256(&bytes)),
            );
            if let Some(path) = receipt {
                let mut text = serde_json::to_string_pretty(&report.receipt)?;
                text.push('\n');
                write_new_bytes(&path, text.as_bytes())?;
            }
            print(&report)
        }
        WasteCommand::Lifetime {
            plan,
            maintenance_result,
            design,
            history,
            replacements,
        } => {
            let plan: LifetimePlan = read_json(&plan)?;
            let list: Vec<ReplacementSummary> = match (maintenance_result, replacements) {
                (Some(path), _) => {
                    let result: MaintenanceResult = read_json(&path)?;
                    let name = design.unwrap_or_default();
                    let d = result.designs.get(&name).ok_or_else(|| {
                        format!(
                            "design `{name}` is not in the result; it has: {}",
                            result
                                .designs
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?;
                    match history {
                        HistoryKind::Fixed => d.fixed.replacements.clone(),
                        HistoryKind::Computed => match &d.computed.summary {
                            Some(s) => s.replacements.clone(),
                            None => {
                                return print(&serde_json::json!({
                                    "status": "NOT_EVALUATED",
                                    "not_evaluated": d.computed.not_evaluated,
                                    "next_step": "Use --history fixed, or repair the computed history.",
                                }));
                            }
                        },
                    }
                }
                (None, Some(path)) => read_json(&path)?,
                (None, None) => {
                    return Err("give --maintenance-result with --design, or --replacements".into());
                }
            };
            match lifetime_from_replacements(&plan, &list) {
                Ok(r) => print(&r),
                Err(ne) => print(&serde_json::json!({
                    "status": "NOT_EVALUATED",
                    "not_evaluated": ne,
                })),
            }
        }
        WasteCommand::ClearanceTime {
            inventory,
            scheme,
            from_years,
            to_years,
        } => {
            let inventory: WasteInventory = read_json(&inventory)?;
            print(&regime_time(
                scheme.regime(),
                &inventory,
                from_years,
                to_years,
            ))
        }
        WasteCommand::Recycling {
            criterion,
            contributions,
            contributions_at_years,
            time_years,
        } => {
            let criterion: RecyclingCriterion = read_json(&criterion)?;
            let contributions: Vec<DoseRateContribution> = read_json(&contributions)?;
            print(&recycling_index(
                &criterion,
                &contributions,
                contributions_at_years,
                time_years,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faris_model::waste::{CURIE_BQ, NuclideActivity, WasteForm, YEAR_S};

    pub(crate) fn inventory_file(dir: &Path) -> PathBuf {
        let mut nuclides: Vec<NuclideActivity> =
            faris_model::waste::classify::required_nuclides(WasteForm::ActivatedMetal)
                .into_iter()
                .map(|n| NuclideActivity {
                    nuclide: n,
                    activity_bq: 0.0,
                    half_life_s: 30.0 * YEAR_S,
                    alpha_emitter: None,
                    progeny_of: None,
                })
                .collect();
        nuclides
            .iter_mut()
            .find(|n| n.nuclide == "Nb-94")
            .unwrap()
            .activity_bq = 0.05 * CURIE_BQ;
        let inventory = WasteInventory {
            component_id: "cli-test".to_string(),
            waste_form: WasteForm::ActivatedMetal,
            volume_m3: Some(1.0),
            mass_g: Some(1.0e6),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides,
        };
        let path = dir.join("inventory.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&inventory).unwrap()).unwrap();
        path
    }

    // Verifies: ACT-030, ACT-033
    #[test]
    fn classify_reads_an_inventory_file_and_the_result_names_its_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let path = inventory_file(dir.path());
        let inventory: WasteInventory = read_json(&path).unwrap();
        let r = classify_regime(Regime::UsNrc, &inventory, 10.0, "2026-10-16", "test", None);
        let json = serde_json::to_value(&r).unwrap();
        // Nb-94 at 0.05 Ci/m3 in activated metal is 0.25 of its Table 1 value: Class C.
        assert_eq!(json["class"], "CLASS_C");
        assert_eq!(json["result"]["verdict"], "CLASS_C");
        assert_eq!(json["result"]["scheme"]["id"], "nrc-10cfr61.55");
        assert_eq!(json["regime"], "nrc-10cfr61.55");
        assert_eq!(json["profile"]["id"], "us-nrc");
        assert!(read_json::<WasteInventory>(&dir.path().join("missing.json")).is_err());
    }

    // Verifies: ACT-033, SAFE-011, SAFE-012
    #[test]
    fn every_scheme_flag_maps_to_its_own_regime_and_a_stale_date_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = inventory_file(dir.path());
        let inventory: WasteInventory = read_json(&path).unwrap();
        let mut ids = Vec::new();
        for scheme in [
            Scheme::Nrc,
            Scheme::Iaea,
            Scheme::UkLlwIlw,
            Scheme::UkOutOfScope,
            Scheme::Fetter,
        ] {
            let r = classify_regime(
                scheme.regime(),
                &inventory,
                10.0,
                "2026-10-16",
                "test",
                None,
            );
            assert_eq!(r.regime, scheme.regime().scheme_id());
            assert!(r.result.is_some());
            ids.push(r.regime);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5);
        let stale = classify_regime(Regime::Fetter, &inventory, 10.0, "2099-01-01", "test", None);
        assert_eq!(stale.class, "NOT_EVALUATED");
        assert!(stale.blocked_by_profile.is_some());
    }

    // Verifies: ACT-038
    #[test]
    fn lifetime_reads_a_plan_and_replacements_and_counts_every_batch_once() {
        let dir = tempfile::tempdir().unwrap();
        let plan = serde_json::json!({"components": [
            {"component_id": "magnets", "pieces": 18, "mass_kg_each": 5000.5, "volume_m3_each": 0.7},
            {"component_id": "blanket", "mass_kg_each": 123456.789, "volume_m3_each": 21.3}
        ]});
        let replacements = serde_json::json!([
            {"class": "magnet", "component": "magnets", "k": 1, "start_s": 1.0e8, "end_s": 1.1e8},
            {"class": "magnet", "component": "magnets", "k": 2, "start_s": 3.0e8, "end_s": null}
        ]);
        let (p, r) = (dir.path().join("plan.json"), dir.path().join("rep.json"));
        std::fs::write(&p, plan.to_string()).unwrap();
        std::fs::write(&r, replacements.to_string()).unwrap();
        let plan: LifetimePlan = read_json(&p).unwrap();
        let list: Vec<ReplacementSummary> = read_json(&r).unwrap();
        let out = lifetime_from_replacements(&plan, &list).unwrap();
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["plant"]["batches"], 4);
        assert_eq!(json["by_component"]["magnets"]["pieces"], 54);
        assert_eq!(json["by_class"]["NOT_EVALUATED"]["batches"], 4);
        assert_eq!(json["sum_check"]["passed"], true);
    }
}
