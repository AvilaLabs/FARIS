//! Scenario semantics and validation. Display labels never imply material properties.

pub mod physics;
pub mod transport;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

pub const SCENARIO_VERSION: &str = "faris-scenario/v0.1";

#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("scenario file: {0}")]
    Io(#[from] std::io::Error),
    #[error("scenario JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("scenario: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema_version: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub geometry: Geometry,
    pub operating_plan: OperatingPlan,
    pub variants: Vec<Variant>,
    pub references: Vec<Reference>,
    pub assumptions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub major_radius_m: f64,
    pub plasma_minor_radius_m: f64,
    pub plasma_to_first_wall_gap_m: f64,
    pub radial_build_m: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OperatingPlan {
    pub fusion_power_mw: f64,
    pub horizon_years: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub id: String,
    pub label: String,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub id: String,
    pub label: String,
    pub material_id: String,
    pub thickness_m: f64,
    pub color: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub id: String,
    pub title: String,
    pub url: String,
    pub r#use: String,
}

#[derive(Clone, Debug)]
pub struct LoadedScenario {
    pub scenario: Scenario,
    pub source_sha256: String,
}

impl LoadedScenario {
    pub fn load(path: &Path) -> Result<Self, ScenarioError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ScenarioError> {
        let scenario: Scenario = serde_json::from_slice(bytes)?;
        scenario.validate()?;
        Ok(Self {
            scenario,
            source_sha256: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

fn invalid(message: impl Into<String>) -> ScenarioError {
    ScenarioError::Invalid(message.into())
}

fn text(value: &str, name: &str) -> Result<(), ScenarioError> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{name} must be nonempty")));
    }
    Ok(())
}

fn positive(value: f64, name: &str) -> Result<(), ScenarioError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(invalid(format!("{name} must be finite and positive")));
    }
    Ok(())
}

impl Scenario {
    pub fn validate(&self) -> Result<(), ScenarioError> {
        if self.schema_version != SCENARIO_VERSION {
            return Err(invalid(format!(
                "supported schema_version is {SCENARIO_VERSION}"
            )));
        }
        text(&self.id, "id")?;
        text(&self.title, "title")?;
        text(&self.description, "description")?;
        let geometry = &self.geometry;
        for (value, name) in [
            (geometry.major_radius_m, "major_radius_m"),
            (geometry.plasma_minor_radius_m, "plasma_minor_radius_m"),
            (geometry.radial_build_m, "radial_build_m"),
        ] {
            positive(value, name)?;
            if !(1e-6..=10_000.0).contains(&value) {
                return Err(invalid(format!(
                    "{name} is outside the geometry model's 1e-6 to 10000 m domain"
                )));
            }
        }
        if !geometry.plasma_to_first_wall_gap_m.is_finite()
            || !(0.0..=10_000.0).contains(&geometry.plasma_to_first_wall_gap_m)
        {
            return Err(invalid(
                "plasma_to_first_wall_gap_m must be finite and in 0 to 10000 m",
            ));
        }
        if geometry.major_radius_m
            <= geometry.plasma_minor_radius_m
                + geometry.plasma_to_first_wall_gap_m
                + geometry.radial_build_m
        {
            return Err(invalid(
                "radial build must fit a non-self-intersecting torus",
            ));
        }
        positive(self.operating_plan.fusion_power_mw, "fusion_power_mw")?;
        positive(self.operating_plan.horizon_years, "horizon_years")?;
        if self.variants.is_empty() || self.references.is_empty() || self.assumptions.is_empty() {
            return Err(invalid(
                "variants, references, and assumptions must be nonempty",
            ));
        }
        let mut variant_ids = BTreeSet::new();
        let mut identity: Option<Vec<(&str, &str, &str)>> = None;
        for variant in &self.variants {
            text(&variant.id, "variant.id")?;
            text(&variant.label, "variant.label")?;
            if !variant_ids.insert(&variant.id) {
                return Err(invalid(format!("duplicate variant id: {}", variant.id)));
            }
            if variant.layers.is_empty() {
                return Err(invalid("variant layers must be nonempty"));
            }
            let mut layer_ids = BTreeSet::new();
            for layer in &variant.layers {
                for (value, name) in [
                    (&layer.id, "layer.id"),
                    (&layer.label, "layer.label"),
                    (&layer.material_id, "layer.material_id"),
                ] {
                    text(value, name)?;
                }
                if !layer_ids.insert(&layer.id) {
                    return Err(invalid(format!("duplicate layer id: {}", layer.id)));
                }
                positive(layer.thickness_m, "layer.thickness_m")?;
                if layer.color.len() != 7
                    || !layer.color.starts_with('#')
                    || !layer.color.as_bytes()[1..]
                        .iter()
                        .all(u8::is_ascii_hexdigit)
                {
                    return Err(invalid("layer.color must be a six-digit hexadecimal color"));
                }
            }
            let depth: f64 = variant.layers.iter().map(|layer| layer.thickness_m).sum();
            if !depth.is_finite() || (depth - geometry.radial_build_m).abs() > 1e-9 {
                return Err(invalid(format!(
                    "{}: thicknesses must sum to radial_build_m",
                    variant.id
                )));
            }
            let current: Vec<_> = variant
                .layers
                .iter()
                .map(|layer| {
                    (
                        layer.id.as_str(),
                        layer.label.as_str(),
                        layer.material_id.as_str(),
                    )
                })
                .collect();
            if identity
                .as_ref()
                .is_some_and(|expected| expected != &current)
            {
                return Err(invalid(
                    "comparison requires matching component identities and radial order",
                ));
            }
            identity = Some(current);
        }
        for reference in &self.references {
            for (value, name) in [
                (&reference.id, "reference.id"),
                (&reference.title, "reference.title"),
                (&reference.url, "reference.url"),
                (&reference.r#use, "reference.use"),
            ] {
                text(value, name)?;
            }
        }
        for assumption in &self.assumptions {
            text(assumption, "assumption")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASE: &[u8] = include_bytes!("../../../scenarios/arc-inspired/scenario.json");

    #[test]
    fn loads_two_comparable_variants() {
        let loaded = LoadedScenario::from_bytes(CASE).unwrap();
        assert_eq!(loaded.scenario.variants.len(), 2);
        assert_eq!(loaded.source_sha256.len(), 64);
    }

    #[test]
    fn refuses_unknown_fields_and_duplicate_keys() {
        let mut value: serde_json::Value = serde_json::from_slice(CASE).unwrap();
        value["undocumented_units"] = serde_json::json!("cm");
        assert!(LoadedScenario::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
        let text = String::from_utf8(CASE.to_vec()).unwrap().replacen(
            "\"id\": \"arc-inspired-001\"",
            "\"id\": \"first\", \"id\": \"second\"",
            1,
        );
        assert!(LoadedScenario::from_bytes(text.as_bytes()).is_err());
    }

    #[test]
    fn refuses_invalid_dimensions_and_changed_envelope() {
        for value in [f64::NAN, f64::INFINITY, 0.0, -1.0, 2.0] {
            let mut loaded = LoadedScenario::from_bytes(CASE).unwrap();
            loaded.scenario.geometry.major_radius_m = value;
            assert!(loaded.scenario.validate().is_err());
        }
        let mut loaded = LoadedScenario::from_bytes(CASE).unwrap();
        loaded.scenario.variants[1].layers[1].thickness_m += 0.1;
        assert!(loaded.scenario.validate().is_err());
    }

    #[test]
    fn refuses_identity_changes_and_unknown_versions() {
        let mut loaded = LoadedScenario::from_bytes(CASE).unwrap();
        loaded.scenario.variants[1].layers[1].id = "another-blanket".into();
        assert!(loaded.scenario.validate().is_err());
        let mut loaded = LoadedScenario::from_bytes(CASE).unwrap();
        loaded.scenario.schema_version = "future/v9".into();
        assert!(loaded.scenario.validate().is_err());
    }

    #[test]
    fn source_identity_changes_with_authored_inputs() {
        let original = LoadedScenario::from_bytes(CASE).unwrap();
        let mut bytes = CASE.to_vec();
        bytes.push(b'\n');
        let changed = LoadedScenario::from_bytes(&bytes).unwrap();
        assert_ne!(original.source_sha256, changed.source_sha256);
    }
}
