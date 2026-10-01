//! Shared geometry, checked transport normalization and bounded external jobs.

pub mod case_archive;
pub mod comparison;
pub mod core_evidence;
pub mod geometry;
pub mod history;
pub mod jobs;
pub mod mesh;
pub mod reactor;
pub mod study;
pub mod transport;

use faris_model::{LoadedScenario, Penetration, Reference, ScenarioError};
use serde::Serialize;
use std::{collections::BTreeMap, f64::consts::PI};

pub const MANIFEST_VERSION: &str = "faris-demo/v0.1";

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvaluationStatus {
    Pass,
    Fail,
    Inconclusive,
    NotEvaluated,
}

#[derive(Clone, Debug, Serialize)]
pub struct Evaluation {
    pub status: EvaluationStatus,
    pub reason: String,
    pub results: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DemoManifest {
    pub schema_version: String,
    pub scenario_id: String,
    pub title: String,
    pub description: String,
    pub source_sha256: String,
    pub geometry_model: String,
    pub coordinate_system: String,
    pub major_radius_m: f64,
    pub plasma_minor_radius_m: f64,
    pub radial_build_m: f64,
    pub fusion_power_mw: f64,
    pub horizon_years: f64,
    pub variants: Vec<VariantGeometry>,
    pub penetration: Option<Penetration>,
    pub geometry_volume_status: GeometryVolumeStatus,
    pub references: Vec<Reference>,
    pub assumptions: Vec<String>,
    pub evaluations: BTreeMap<String, Evaluation>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GeometryVolumeStatus {
    ExactAnalyticFullTorus,
    PenetrationEstimateNotIndependentlyValidated,
}

#[derive(Clone, Debug, Serialize)]
pub struct VariantGeometry {
    pub id: String,
    pub label: String,
    pub components: Vec<ComponentGeometry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComponentGeometry {
    pub id: String,
    pub label: String,
    pub material_id: String,
    pub color: String,
    pub thickness_m: f64,
    pub inner_minor_radius_m: f64,
    pub outer_minor_radius_m: f64,
    pub full_torus_volume_m3: f64,
    /// Physical region volume. Absent for a penetrated region until an
    /// independent volume calculation validates its post-cut volume.
    pub effective_volume_m3: Option<f64>,
    pub penetration_intersection_estimate: Option<geometry::NumericalVolumeEstimate>,
}

pub fn build_manifest(loaded: &LoadedScenario) -> Result<DemoManifest, ScenarioError> {
    loaded.validate_identity()?;
    let scenario = &loaded.scenario;
    scenario.validate()?;
    let geometry = &scenario.geometry;
    let variants = scenario
        .variants
        .iter()
        .map(|variant| {
            let mut inner = geometry.plasma_minor_radius_m + geometry.plasma_to_first_wall_gap_m;
            let components = variant
                .layers
                .iter()
                .map(|layer| {
                    let outer = inner + layer.thickness_m;
                    let full_torus_volume_m3 = 2.0
                        * PI.powi(2)
                        * geometry.major_radius_m
                        * (outer.powi(2) - inner.powi(2));
                    let penetration_intersection_estimate =
                        scenario.penetration.as_ref().map(|p| {
                            let Penetration::OutboardRectangularPrism { bounds_m, .. } = p;
                            crate::geometry::estimate_torus_shell_box_intersection(
                                geometry.major_radius_m,
                                inner,
                                outer,
                                &bounds_m.minimum_xyz_m,
                                &bounds_m.maximum_xyz_m,
                            )
                        });
                    let component = ComponentGeometry {
                        id: layer.id.clone(),
                        label: layer.label.clone(),
                        material_id: layer.material_id.clone(),
                        color: layer.color.clone(),
                        thickness_m: layer.thickness_m,
                        inner_minor_radius_m: inner,
                        outer_minor_radius_m: outer,
                        full_torus_volume_m3,
                        effective_volume_m3: scenario
                            .penetration
                            .is_none()
                            .then_some(full_torus_volume_m3),
                        penetration_intersection_estimate,
                    };
                    inner = outer;
                    component
                })
                .collect();
            VariantGeometry {
                id: variant.id.clone(),
                label: variant.label.clone(),
                components,
            }
        })
        .collect();
    let evaluations = [
        (
            "transport",
            "No transport adapter or evaluated nuclear-data library is configured.",
        ),
        (
            "activation",
            "No irradiation spectrum or inventory calculation has been supplied.",
        ),
        (
            "lifetime",
            "Service limits, fuel dynamics, and maintenance rules are not implemented.",
        ),
        (
            "avila_core",
            "No Core compilation or scientific assessment is bound to this geometry export.",
        ),
    ]
    .into_iter()
    .map(|(name, reason)| {
        (
            name.into(),
            Evaluation {
                status: EvaluationStatus::NotEvaluated,
                reason: reason.into(),
                results: None,
            },
        )
    })
    .collect();
    Ok(DemoManifest {
        schema_version: MANIFEST_VERSION.into(),
        scenario_id: scenario.id.clone(),
        title: scenario.title.clone(),
        description: scenario.description.clone(),
        source_sha256: loaded.source_sha256.clone(),
        geometry_model: "circular-concentric-tori/v0.1".into(),
        coordinate_system: "right-handed; Y is vertical; lengths in metres".into(),
        major_radius_m: geometry.major_radius_m,
        plasma_minor_radius_m: geometry.plasma_minor_radius_m,
        radial_build_m: geometry.radial_build_m,
        fusion_power_mw: scenario.operating_plan.fusion_power_mw,
        horizon_years: scenario.operating_plan.horizon_years,
        variants,
        penetration: scenario.penetration.clone(),
        geometry_volume_status: if scenario.penetration.is_some() {
            GeometryVolumeStatus::PenetrationEstimateNotIndependentlyValidated
        } else {
            GeometryVolumeStatus::ExactAnalyticFullTorus
        },
        references: scenario.references.clone(),
        assumptions: scenario.assumptions.clone(),
        evaluations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> DemoManifest {
        build_manifest(
            &LoadedScenario::from_bytes(include_bytes!(
                "../../../scenarios/arc-inspired/scenario.json"
            ))
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn port_manifest_never_reuses_unperforated_volume_as_effective() {
        let loaded = LoadedScenario::from_bytes(include_bytes!(
            "../../../scenarios/arc-inspired/cold-reference-port.scenario.json"
        ))
        .unwrap();
        let result = build_manifest(&loaded).unwrap();
        assert_eq!(
            result.geometry_volume_status,
            GeometryVolumeStatus::PenetrationEstimateNotIndependentlyValidated
        );
        assert!(result.penetration.is_some());
        for variant in &result.variants {
            assert!(variant.components.iter().all(|component| {
                component.effective_volume_m3.is_none()
                    && component
                        .penetration_intersection_estimate
                        .as_ref()
                        .is_some_and(|estimate| {
                            estimate.volume_m3 > 0.0 && !estimate.independently_validated
                        })
            }));
        }
    }

    #[test]
    fn volume_matches_area_times_centroid_path_length() {
        let result = manifest();
        for variant in &result.variants {
            for component in &variant.components {
                let area = PI
                    * (component.outer_minor_radius_m.powi(2)
                        - component.inner_minor_radius_m.powi(2));
                let expected = area * (2.0 * PI * result.major_radius_m);
                assert!((component.full_torus_volume_m3 - expected).abs() < 1e-10);
            }
            assert!((variant.components.last().unwrap().outer_minor_radius_m - 2.28).abs() < 1e-12);
        }
    }

    #[test]
    fn deterministic_records_never_invent_physics_results() {
        let first = manifest();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&manifest()).unwrap()
        );
        assert!(
            first
                .evaluations
                .values()
                .all(
                    |evaluation| evaluation.status == EvaluationStatus::NotEvaluated
                        && evaluation.results.is_none()
                )
        );
    }
}
