//! `faris materials`: the cited materials and limits database. Every command prints JSON, except
//! `export --format csv-limits` and `csv-properties`, which print CSV. Research screening only.

use crate::{transport::write_new_bytes, waste::read_json};
use clap::{Subcommand, ValueEnum};
use faris_model::{
    materials_db::{
        MaterialsDb, export,
        impurity::{self, ElementYield, SpecSelection},
        limits::{self, CaseConditions, Criterion, LimitOverride},
    },
    waste::{WasteInventory, classify::Verdict},
};
use std::path::PathBuf;

#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    Json,
    CsvLimits,
    CsvProperties,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Target {
    #[value(name = "CLASS_A")]
    A,
    #[value(name = "CLASS_B")]
    B,
    #[value(name = "CLASS_C")]
    C,
}

#[derive(Subcommand)]
pub enum MaterialsCommand {
    /// Database id, version, date and content hash; the materials and counts.
    List,
    /// One material: composition, impurity specifications, properties, limits.
    Show { material: String },
    /// The limit table (every row with convention, window, spectrum, conditions, status and citation).
    Limits {
        #[arg(long)]
        material: Option<String>,
    },
    /// The impurity watch-list with the 10 CFR 61.55 thresholds each element endangers.
    Watchlist,
    /// Validate the embedded database, or another database file (schema, citations, labels).
    Check {
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Compare one value with one stored limit under the case's conditions.
    Compare {
        #[arg(long)]
        limit: String,
        #[arg(long)]
        value: f64,
        /// JSON: dpa_convention, irradiation_temperature_c, spectrum, form (each optional).
        #[arg(long)]
        conditions: Option<PathBuf>,
        /// JSON authored override: limit_id, value, convention, author, time, reason.
        #[arg(long = "override")]
        override_file: Option<PathBuf>,
    },
    /// Years to each limit at a constant rate, and which is reached first. A dpa-only check is
    /// refused for a material with a helium or hydrogen limit.
    TimeToLimit {
        #[arg(long)]
        material: String,
        /// JSON list of criteria: limit_id, accumulated, rate_per_year, optional override.
        #[arg(long)]
        criteria: PathBuf,
        #[arg(long)]
        conditions: Option<PathBuf>,
    },
    /// A waste class for a component of a stored material, only with an impurity specification.
    Classify {
        #[arg(long)]
        material: String,
        #[arg(long)]
        inventory: PathBuf,
        /// The specification id the inventory was computed with.
        #[arg(long)]
        inventory_spec: String,
        /// JSON: spec_id and assumed limits (element, max_wt_ppm, author, reason). Omit to see the refusal.
        #[arg(long)]
        spec: Option<PathBuf>,
        #[arg(long)]
        time_years: f64,
    },
    /// The largest content of one element that keeps the inventory within a target class.
    MaxImpurity {
        #[arg(long)]
        inventory: PathBuf,
        /// JSON element yield: element and per_ppm nuclide activities (Bq per wt ppm).
        #[arg(long)]
        element_yield: PathBuf,
        /// The element's wt ppm that the inventory was computed with.
        #[arg(long, default_value_t = 0.0)]
        current_ppm: f64,
        #[arg(long, value_enum)]
        target: Target,
        #[arg(long)]
        time_years: f64,
    },
    /// Export the database or its tables with citations.
    Export {
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
        /// Write to this new file instead of standard output.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn print(value: &impl serde::Serialize) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn run(command: MaterialsCommand) -> Result<(), Box<dyn std::error::Error>> {
    let db = MaterialsDb::embedded();
    match command {
        MaterialsCommand::List => print(&serde_json::json!({
            "database": db.binding,
            "scope": db.data.database.scope,
            "materials": db.data.materials.iter().map(|m| serde_json::json!({
                "id": m.id, "name": m.name, "class": m.class,
                "composition": m.composition.status,
                "impurity_specs": m.impurity_specs.iter().map(|s| &s.id).collect::<Vec<_>>(),
                "properties": m.properties.len(),
                "limits": db.data.limits.iter().filter(|l| l.material == m.id).count(),
            })).collect::<Vec<_>>(),
            "sources": db.data.sources.iter().map(|s| serde_json::json!({
                "id": s.id, "citation": s.citation, "url": s.url,
                "licence": s.licence, "redistribution": s.redistribution,
            })).collect::<Vec<_>>(),
            "research_screening": faris_model::RESEARCH_SCREENING_STATEMENT,
        })),
        MaterialsCommand::Show { material } => {
            let m = db.material(&material).ok_or_else(|| {
                format!("no material '{material}'; `faris materials list` shows the ids")
            })?;
            print(&serde_json::json!({
                "database": db.binding,
                "material": m,
                "mass_fractions": m.mass_fractions().ok(),
                "limits": db.data.limits.iter().filter(|l| l.material == m.id).collect::<Vec<_>>(),
            }))
        }
        MaterialsCommand::Limits { material } => print(&serde_json::json!({
            "database": db.binding,
            "limits": db.data.limits.iter().filter(|l| material.as_ref().is_none_or(|m| &l.material == m)).collect::<Vec<_>>(),
            "research_screening": faris_model::RESEARCH_SCREENING_STATEMENT,
        })),
        MaterialsCommand::Watchlist => print(&serde_json::json!({
            "database": db.binding,
            "watchlist": impurity::watchlist(db),
            "research_screening": faris_model::RESEARCH_SCREENING_STATEMENT,
        })),
        MaterialsCommand::Check { file } => {
            let (text, name) = match &file {
                Some(path) => (
                    String::from_utf8(crate::transport::read_bounded(path)?)?,
                    path.display().to_string(),
                ),
                None => (
                    include_str!("../../faris-model/data/materials/materials_db.json").to_string(),
                    "embedded".to_string(),
                ),
            };
            match MaterialsDb::from_json_str(&text) {
                Ok(checked) => print(&serde_json::json!({
                    "database": name, "valid": true, "binding": checked.binding,
                    "limits": checked.data.limits.len(), "materials": checked.data.materials.len(),
                })),
                Err(e) => Err(format!("{name}: {e}").into()),
            }
        }
        MaterialsCommand::Compare {
            limit,
            value,
            conditions,
            override_file,
        } => {
            let case: CaseConditions = conditions
                .map(|p| read_json(&p))
                .transpose()?
                .unwrap_or_default();
            let o: Option<LimitOverride> = override_file.map(|p| read_json(&p)).transpose()?;
            print(&limits::compare(db, &limit, value, &case, o.as_ref()).map_err(|e| e.reason)?)
        }
        MaterialsCommand::TimeToLimit {
            material,
            criteria,
            conditions,
        } => {
            let criteria: Vec<Criterion> = read_json(&criteria)?;
            let case: CaseConditions = conditions
                .map(|p| read_json(&p))
                .transpose()?
                .unwrap_or_default();
            print(&limits::time_to_limit(db, &material, &criteria, &case))
        }
        MaterialsCommand::Classify {
            material,
            inventory,
            inventory_spec,
            spec,
            time_years,
        } => {
            let inventory: WasteInventory = read_json(&inventory)?;
            let selection: Option<SpecSelection> = spec.map(|p| read_json(&p)).transpose()?;
            print(&impurity::classify_material(
                db,
                &material,
                selection.as_ref(),
                &inventory_spec,
                &inventory,
                time_years,
            ))
        }
        MaterialsCommand::MaxImpurity {
            inventory,
            element_yield,
            current_ppm,
            target,
            time_years,
        } => {
            let inventory: WasteInventory = read_json(&inventory)?;
            let element_yield: ElementYield = read_json(&element_yield)?;
            let target = match target {
                Target::A => Verdict::ClassA,
                Target::B => Verdict::ClassB,
                Target::C => Verdict::ClassC,
            };
            print(&impurity::max_impurity_for_class(
                db,
                &inventory,
                &element_yield,
                current_ppm,
                target,
                time_years,
            ))
        }
        MaterialsCommand::Export { format, output } => {
            let text = match format {
                Format::Json => export::to_json(db),
                Format::CsvLimits => export::limits_csv(db),
                Format::CsvProperties => export::properties_csv(db),
            };
            match output {
                Some(path) => {
                    write_new_bytes(&path, text.as_bytes())?;
                    println!("Wrote {}", path.display());
                }
                None => print!("{text}"),
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies: MAT-009, MAT-056
    #[test]
    fn the_embedded_database_checks_and_exports_through_the_cli_functions() {
        run(MaterialsCommand::Check { file: None }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("limits.csv");
        run(MaterialsCommand::Export {
            format: Format::CsvLimits,
            output: Some(out.clone()),
        })
        .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("id,material,component,quantity"));
        // An existing file is refused, never overwritten.
        assert!(
            run(MaterialsCommand::Export {
                format: Format::CsvLimits,
                output: Some(out)
            })
            .is_err()
        );
    }

    // Verifies: MAT-009
    #[test]
    fn a_damaged_database_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        let text = include_str!("../../faris-model/data/materials/materials_db.json").replacen(
            "\"max_wt_ppm\": 10,",
            "\"max_wt_ppm\": 11,",
            1,
        );
        std::fs::write(&path, text).unwrap();
        let err = run(MaterialsCommand::Check { file: Some(path) })
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not the value as written in the quote"),
            "{err}"
        );
    }
}
