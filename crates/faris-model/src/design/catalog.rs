//! FARIS's material baseline, `references/demo-input-spec.json`. A design file
//! can name one of these by `catalog_id` instead of writing a recipe.

use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

const BASELINE: &str = include_str!("../../../../references/demo-input-spec.json");

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogMaterial {
    pub id: String,
    pub density_kg_m3: f64,
    /// Nuclide atom fractions of all atoms, from OpenMC's natural-element expansion.
    pub nuclide_atom_fractions: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Baseline {
    materials: Vec<Value>,
}

/// Catalog materials with a recipe. `vacuum-gap` has none and is not a catalog
/// material: empty space is the built-in `void`.
pub fn catalog() -> &'static [CatalogMaterial] {
    static CATALOG: OnceLock<Vec<CatalogMaterial>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let baseline: Baseline =
            serde_json::from_str(BASELINE).expect("the embedded material baseline is valid JSON");
        baseline
            .materials
            .iter()
            .filter_map(|material| {
                let recipe = material.get("recipe")?;
                let fractions = recipe
                    .get("nuclide_atom_fractions")
                    .or_else(|| recipe.get("nuclide_atom_fractions_across_all_atoms"))?
                    .as_object()?;
                Some(CatalogMaterial {
                    id: material.get("id")?.as_str()?.to_string(),
                    density_kg_m3: material.get("density_kg_m3")?.as_f64()?,
                    nuclide_atom_fractions: fractions
                        .iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_f64()?)))
                        .collect(),
                })
            })
            .collect()
    })
}

pub fn catalog_material(id: &str) -> Option<&'static CatalogMaterial> {
    catalog().iter().find(|m| m.id == id)
}
