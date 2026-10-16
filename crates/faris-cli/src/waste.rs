//! `faris waste`: classify an activated component's inventory under 10 CFR 61.55, give its IAEA
//! clearance index and time, and its recycling index. Every command prints one JSON document.
//! Research screening, not a regulatory determination.

use crate::transport::read_bounded;
use clap::{Subcommand, ValueEnum};
use faris_model::waste::{
    self, WasteInventory,
    classify::classify_nrc,
    clearance::{
        DoseRateContribution, RecyclingCriterion, SEARCH_END_YEARS, SEARCH_START_YEARS,
        clearance_index, clearance_time, recycling_index,
    },
    tables,
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
    /// Classify an inventory at a disposal time. Refuses (NOT_EVALUATED) when a needed nuclide,
    /// volume, mass or flag is missing; it never defaults to Class A.
    Classify {
        /// faris waste inventory JSON: component, waste form, averaging volume and mass, nuclides.
        #[arg(long)]
        inventory: PathBuf,
        /// Years after the inventory reference at which the waste is disposed of.
        #[arg(long)]
        time_years: f64,
        #[arg(long, value_enum, default_value = "nrc-10cfr61.55")]
        scheme: Scheme,
    },
    /// Find the time the IAEA clearance index falls below 1, searched between two times.
    ClearanceTime {
        #[arg(long)]
        inventory: PathBuf,
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
        },
        WasteCommand::Classify {
            inventory,
            time_years,
            scheme,
        } => {
            let inventory: WasteInventory = read_json(&inventory)?;
            match scheme {
                Scheme::Nrc => print(&classify_nrc(&inventory, time_years)),
                Scheme::Iaea => print(&clearance_index(&inventory, time_years)),
            }
        }
        WasteCommand::ClearanceTime {
            inventory,
            from_years,
            to_years,
        } => {
            let inventory: WasteInventory = read_json(&inventory)?;
            print(&clearance_time(&inventory, from_years, to_years))
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
        let r = classify_nrc(&inventory, 10.0);
        let json = serde_json::to_value(&r).unwrap();
        // Nb-94 at 0.05 Ci/m3 in activated metal is 0.25 of its Table 1 value: Class C.
        assert_eq!(json["verdict"], "CLASS_C");
        assert_eq!(json["scheme"]["id"], "nrc-10cfr61.55");
        assert!(read_json::<WasteInventory>(&dir.path().join("missing.json")).is_err());
    }
}
