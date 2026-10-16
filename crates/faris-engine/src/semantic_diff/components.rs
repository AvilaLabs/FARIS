//! Components of a model: the cells of the radial build, what changed in them between two
//! versions (GEO-067), and the geometry summary table (GEO-069).

use faris_model::canonical::values_equal;
use faris_model::physics::{MaterialRecipe, PhysicsCase};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::f64::consts::PI;

/// One cell of a radial build: a torus shell between two minor radii.
#[derive(Clone, Debug, PartialEq)]
struct Cell {
    variant: String,
    id: String,
    label: String,
    material_id: String,
    thickness_m: f64,
    inner_m: f64,
    outer_m: f64,
    volume_m3: f64,
    index: usize,
}

/// Volume of the full torus shell between minor radii `a < b` about major radius `r0`.
fn shell_volume_m3(r0: f64, a: f64, b: f64) -> f64 {
    2.0 * PI * PI * r0 * (b * b - a * a)
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The cells of a scenario (`variants[].layers[]`) or of a transport input
/// (`manifest.variants[].components[]`). Empty for any other document.
fn cells_of(document: &Value) -> Vec<Cell> {
    let (root, geometry, list_key) = if let Some(manifest) = document.get("manifest") {
        (manifest, manifest, "components")
    } else {
        (
            document,
            document.get("geometry").unwrap_or(document),
            "layers",
        )
    };
    let Some(r0) = number(geometry, "major_radius_m") else {
        return Vec::new();
    };
    let start = number(geometry, "plasma_minor_radius_m").unwrap_or(0.0)
        + number(geometry, "plasma_to_first_wall_gap_m").unwrap_or(0.0);
    let mut cells = Vec::new();
    for variant in root
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let variant_id = text(variant, "id");
        let mut inner = start;
        for (index, layer) in variant
            .get(list_key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let thickness = number(layer, "thickness_m").unwrap_or(0.0);
            let (a, b) = match (
                number(layer, "inner_minor_radius_m"),
                number(layer, "outer_minor_radius_m"),
            ) {
                (Some(a), Some(b)) => (a, b),
                _ => (inner, inner + thickness),
            };
            let volume =
                number(layer, "full_torus_volume_m3").unwrap_or_else(|| shell_volume_m3(r0, a, b));
            cells.push(Cell {
                variant: variant_id.clone(),
                id: text(layer, "id"),
                label: text(layer, "label"),
                material_id: text(layer, "material_id"),
                thickness_m: thickness,
                inner_m: a,
                outer_m: b,
                volume_m3: volume,
                index,
            });
            inner = b;
        }
    }
    cells
}

/// What changed for one component between two versions.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComponentChange {
    pub document: String,
    pub variant: String,
    pub component: String,
    /// Any of `added`, `removed`, `moved`, `material`, `thickness`, `volume`.
    pub changes: Vec<&'static str>,
    pub material_old: Option<String>,
    pub material_new: Option<String>,
    pub thickness_old_m: Option<f64>,
    pub thickness_new_m: Option<f64>,
    pub position_old: Option<usize>,
    pub position_new: Option<usize>,
    pub volume_old_m3: Option<f64>,
    pub volume_new_m3: Option<f64>,
    /// New minus old; absent for an added or removed component.
    pub volume_delta_m3: Option<f64>,
}

pub(super) fn component_changes(document: &str, a: &Value, b: &Value) -> Vec<ComponentChange> {
    let key = |c: &Cell| (c.variant.clone(), c.id.clone());
    let old: BTreeMap<_, _> = cells_of(a).into_iter().map(|c| (key(&c), c)).collect();
    let new: BTreeMap<_, _> = cells_of(b).into_iter().map(|c| (key(&c), c)).collect();
    // The order of the cells both sides have, per variant, to find the ones that moved.
    let order = |cells: &BTreeMap<(String, String), Cell>,
                 other: &BTreeMap<(String, String), Cell>| {
        let mut list: Vec<&Cell> = cells
            .values()
            .filter(|c| other.contains_key(&key(c)))
            .collect();
        list.sort_by(|p, q| (p.variant.as_str(), p.index).cmp(&(q.variant.as_str(), q.index)));
        list.into_iter().map(&key).collect::<Vec<_>>()
    };
    let (order_old, order_new) = (order(&old, &new), order(&new, &old));
    let mut stayed = std::collections::BTreeSet::new();
    if order_old == order_new {
        stayed.extend(order_old.iter().cloned());
    } else {
        // Per variant, the longest common run of ids.
        let variants: std::collections::BTreeSet<_> =
            order_old.iter().map(|k| k.0.clone()).collect();
        for variant in variants {
            let ids = |o: &[(String, String)]| -> Vec<String> {
                o.iter()
                    .filter(|k| k.0 == variant)
                    .map(|k| k.1.clone())
                    .collect()
            };
            let (x, y) = (ids(&order_old), ids(&order_new));
            let (xr, yr): (Vec<&str>, Vec<&str>) = (
                x.iter().map(String::as_str).collect(),
                y.iter().map(String::as_str).collect(),
            );
            for id in super::longest_common_subsequence(&xr, &yr) {
                stayed.insert((variant.clone(), id.to_string()));
            }
        }
    }
    let mut out = Vec::new();
    let mut keys: Vec<_> = old.keys().chain(new.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let (o, n) = (old.get(&k), new.get(&k));
        let mut changes = Vec::new();
        let mut entry = ComponentChange {
            document: document.to_string(),
            variant: k.0.clone(),
            component: k.1.clone(),
            changes: Vec::new(),
            material_old: o.map(|c| c.material_id.clone()),
            material_new: n.map(|c| c.material_id.clone()),
            thickness_old_m: o.map(|c| c.thickness_m),
            thickness_new_m: n.map(|c| c.thickness_m),
            position_old: o.map(|c| c.index),
            position_new: n.map(|c| c.index),
            volume_old_m3: o.map(|c| c.volume_m3),
            volume_new_m3: n.map(|c| c.volume_m3),
            volume_delta_m3: None,
        };
        match (o, n) {
            (None, Some(_)) => changes.push("added"),
            (Some(_), None) => changes.push("removed"),
            (Some(p), Some(q)) => {
                if !stayed.contains(&k) {
                    changes.push("moved");
                }
                if p.material_id != q.material_id {
                    changes.push("material");
                }
                if !values_equal(&Value::from(p.thickness_m), &Value::from(q.thickness_m)) {
                    changes.push("thickness");
                }
                if !values_equal(&Value::from(p.volume_m3), &Value::from(q.volume_m3)) {
                    changes.push("volume");
                    entry.volume_delta_m3 = Some(q.volume_m3 - p.volume_m3);
                }
            }
            (None, None) => {}
        }
        if !changes.is_empty() {
            entry.changes = changes;
            out.push(entry);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The geometry summary table (GEO-069)

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometryRow {
    pub name: String,
    pub id: String,
    pub variant: String,
    pub material_id: String,
    pub volume_m3: f64,
    pub density_kg_m3: Option<f64>,
    /// `None` when there is no density: no physics case was given, or the material is a void.
    pub mass_kg: Option<f64>,
    pub representation: &'static str,
    pub kind_label: &'static str,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GeometrySummary {
    pub scenario_id: String,
    pub variant: String,
    pub rows: Vec<GeometryRow>,
    pub total_volume_m3: f64,
    /// The sum of the cell masses; `None` when any cell with volume has no mass stated and is
    /// not a declared void, so the total would not be one.
    pub total_mass_kg: Option<f64>,
    pub notes: Vec<String>,
}

/// The cells of one variant of a scenario with volume and, with a physics case, density and mass.
/// Volumes are those of the full torus shell; a penetration is not subtracted, and the table says
/// so. The kind label of a volume and a mass is `calculated`.
pub fn geometry_summary(
    scenario: &Value,
    variant: &str,
    physics: Option<&PhysicsCase>,
) -> Result<GeometrySummary, String> {
    let cells: Vec<Cell> = cells_of(scenario)
        .into_iter()
        .filter(|c| c.variant == variant)
        .collect();
    if cells.is_empty() {
        let known: Vec<String> = cells_of(scenario)
            .into_iter()
            .map(|c| c.variant)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        return Err(format!(
            "the scenario has no variant {variant:?}. Next step: use one of {}",
            known.join(", ")
        ));
    }
    let penetration = scenario.get("penetration").is_some_and(|p| !p.is_null());
    let mut rows = Vec::new();
    let mut total_volume = 0.0;
    let mut total_mass = Some(0.0);
    for cell in &cells {
        let mut note = String::new();
        let recipe = physics.and_then(|p| {
            let material = p
                .component_assignments
                .iter()
                .find(|a| a.component_id == cell.id)
                .map(|a| a.material_id.as_str())?;
            p.materials
                .iter()
                .find(|m| m.id == material)
                .map(|m| &m.recipe)
        });
        let (density, mass) = match recipe {
            Some(MaterialRecipe::NuclideMixture { density_kg_m3, .. }) => {
                (Some(*density_kg_m3), Some(density_kg_m3 * cell.volume_m3))
            }
            Some(MaterialRecipe::Void { .. }) => {
                note = "declared void: no mass".into();
                (None, Some(0.0))
            }
            None => {
                note = if physics.is_some() {
                    "no material assigned to this component in the physics case: mass not evaluated"
                } else {
                    "no physics case given: mass not evaluated"
                }
                .into();
                (None, None)
            }
        };
        if penetration {
            if !note.is_empty() {
                note.push_str("; ");
            }
            note.push_str("full torus shell, the penetration is not subtracted");
        }
        total_volume += cell.volume_m3;
        total_mass = match (total_mass, mass) {
            (Some(t), Some(m)) => Some(t + m),
            _ => None,
        };
        rows.push(GeometryRow {
            name: if cell.label.is_empty() {
                cell.id.clone()
            } else {
                cell.label.clone()
            },
            id: cell.id.clone(),
            variant: cell.variant.clone(),
            material_id: cell.material_id.clone(),
            volume_m3: cell.volume_m3,
            density_kg_m3: density,
            mass_kg: mass,
            representation: "analytic torus shell",
            kind_label: "calculated",
            note,
        });
    }
    let mut notes = Vec::new();
    if total_mass.is_none() {
        notes.push("The total mass is NOT_EVALUATED: at least one cell has no mass.".into());
    }
    Ok(GeometrySummary {
        scenario_id: text(scenario, "id"),
        variant: variant.to_string(),
        rows,
        total_volume_m3: total_volume,
        total_mass_kg: total_mass,
        notes,
    })
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// The table as CSV, `.` as the decimal mark and full precision. The last row is the total.
pub fn geometry_summary_csv(summary: &GeometrySummary) -> String {
    let mut out = String::from(
        "cell_name,cell_id,variant,material_id,volume_m3,density_kg_m3,mass_kg,representation,kind_label,note\n",
    );
    let num = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:?}"));
    for row in &summary.rows {
        out.push_str(&format!(
            "{},{},{},{},{:?},{},{},{},{},{}\n",
            csv_field(&row.name),
            csv_field(&row.id),
            csv_field(&row.variant),
            csv_field(&row.material_id),
            row.volume_m3,
            num(row.density_kg_m3),
            num(row.mass_kg),
            row.representation,
            row.kind_label,
            csv_field(&row.note),
        ));
    }
    out.push_str(&format!(
        "TOTAL,,{},,{:?},,{},,calculated,{}\n",
        csv_field(&summary.variant),
        summary.total_volume_m3,
        num(summary.total_mass_kg),
        csv_field(&summary.notes.join(" ")),
    ));
    out
}

/// The rows of every cell of a document, for tests and for tools that want the radial build.
pub fn cell_rows(document: &Value) -> Vec<(String, String, f64, f64)> {
    cells_of(document)
        .into_iter()
        .map(|c| (c.variant, c.id, c.volume_m3, c.outer_m))
        .collect()
}
