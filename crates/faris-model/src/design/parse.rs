//! Check 1: the JSON parses, matches the schema, has no unknown keys and no
//! nulls in required fields.

use super::{
    Averaging, DESIGN_VERSION, Design, Finding, Metric, Provenance, ProvenanceLabel,
    ReplacementDuration, VOID,
};
use serde_json::Value;

const CHECK: u8 = 1;

/// Paths where `null` is a legal value in v0.1. `*` stands for any array index
/// or map key. Every other `null` is a field the user has not filled in.
const NULLABLE: [&str; 6] = [
    "cad.faceting_tolerance_m",
    "solids[*].step_name",
    "solids[*].replacement_group_id",
    "materials[*].temperature_k",
    "replacement_groups[*].limits[*].averaging.voxel_m[*]",
    // `reference_id` is null for authored values; the published rule is checked below.
    "*.reference_id",
];

fn normalise(path: &str) -> String {
    // Replace "[3]" with "[*]".
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            out.push_str("[*]");
            for inner in chars.by_ref() {
                if inner == ']' {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn matches_pattern(pattern: &str, path: &str) -> bool {
    // Segments split on '.', with a bare '*' matching one whole segment (a map
    // key); a leading "*." matches any prefix, as in every provenance block.
    if let Some(rest) = pattern.strip_prefix("*.") {
        return path.ends_with(&format!(".{rest}")) || path == rest;
    }
    let p: Vec<&str> = pattern.split('.').collect();
    let q: Vec<&str> = path.split('.').collect();
    p.len() == q.len() && p.iter().zip(&q).all(|(a, b)| *a == "*" || a == b)
}

fn nullable(path: &str) -> bool {
    let normal = normalise(path);
    NULLABLE
        .iter()
        .any(|pattern| matches_pattern(pattern, &normal))
}

fn walk_nulls(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Null => {
            if !nullable(path) {
                out.push(path.to_string());
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_nulls(item, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk_nulls(item, &child, out);
            }
        }
        _ => {}
    }
}

/// Paths of every `null` that v0.1 does not allow, in document order.
pub fn nullable_paths(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_nulls(value, "", &mut out);
    out
}

/// The result of reading a design file: the typed design when the JSON had the
/// right shape (even if fields are still `null`), and every check-1 finding.
#[derive(Debug)]
pub struct ParseOutcome {
    pub design: Option<Design>,
    pub findings: Vec<Finding>,
}

/// Check 1 on raw bytes.
pub fn parse_design(bytes: &[u8]) -> ParseOutcome {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(error) => {
            return ParseOutcome {
                design: None,
                findings: vec![Finding::new(
                    CHECK,
                    "the design file",
                    format!("it is not valid JSON: {error}"),
                    "Fix the JSON syntax at the line and column shown, then check again.",
                )],
            };
        }
    };
    let mut findings = Vec::new();
    for path in nullable_paths(&value) {
        findings.push(Finding::new(
            CHECK,
            path.clone(),
            "it is null; FARIS never gives a missing value a default",
            format!("Replace the null at {path} with a value, as the format guide describes."),
        ));
    }
    let null_count = findings.len();
    let design: Option<Design> = match serde_json::from_value(value) {
        Ok(design) => Some(design),
        Err(error) => {
            let message = error.to_string();
            // A null in a required non-null field is already listed above.
            if !(null_count > 0 && message.contains("invalid type: null")) {
                findings.push(Finding::new(
                    CHECK,
                    "the design file",
                    format!("it does not match faris-design/v0.1: {message}"),
                    "Fix or remove the key named in the message. Unknown keys are refused so \
                     that a misspelt key cannot be ignored.",
                ));
            }
            None
        }
    };
    if let Some(design) = &design {
        schema_rules(design, &mut findings);
    }
    ParseOutcome { design, findings }
}

fn provenance_rules(item: &str, provenance: &Provenance, findings: &mut Vec<Finding>) {
    if provenance.label == ProvenanceLabel::Published
        && provenance
            .reference_id
            .as_deref()
            .is_none_or(|r| r.trim().is_empty())
    {
        findings.push(Finding::new(
            CHECK,
            format!("{item}.reference_id"),
            "the provenance is labelled published but names no reference",
            "Add a reference_id that matches an entry in references[], or label the value authored.",
        ));
    }
}

fn schema_rules(design: &Design, findings: &mut Vec<Finding>) {
    if design.schema_version != DESIGN_VERSION {
        findings.push(Finding::new(
            CHECK,
            "schema_version",
            format!(
                "it is {:?}; this FARIS reads {DESIGN_VERSION}",
                design.schema_version
            ),
            format!("Set schema_version to {DESIGN_VERSION}, or use a FARIS version that reads this one."),
        ));
    }
    for (value, name) in [(&design.id, "id"), (&design.title, "title")] {
        if value.trim().is_empty() {
            findings.push(Finding::new(
                CHECK,
                name,
                "it is empty",
                format!("Give the design a {name}."),
            ));
        }
    }
    if design.cad.implicit_complement_material_id != VOID {
        findings.push(Finding::new(
            CHECK,
            "cad.implicit_complement_material_id",
            format!(
                "it is {:?}; v0.1 allows only \"void\" outside the solids",
                design.cad.implicit_complement_material_id
            ),
            "Set it to \"void\". FARIS builds the graveyard itself; do not supply one.",
        ));
    }
    for (i, material) in design.materials.iter().enumerate() {
        let item = format!("materials[{i}]");
        if material.catalog_id.is_some() {
            let extras: Vec<&str> = [
                ("label", material.label.is_some()),
                ("density_kg_m3", material.density_kg_m3.is_some()),
                ("composition_basis", material.composition_basis.is_some()),
                ("components", material.components.is_some()),
                ("temperature_k", material.temperature_k.is_some()),
                ("provenance", material.provenance.is_some()),
            ]
            .into_iter()
            .filter_map(|(name, set)| set.then_some(name))
            .collect();
            if !extras.is_empty() {
                findings.push(Finding::new(
                    CHECK,
                    item.clone(),
                    format!(
                        "it names a catalog material and also gives {}; the catalog supplies those",
                        extras.join(", ")
                    ),
                    "Remove those keys to use the catalog material as it is, or remove \
                     catalog_id and write the full recipe.",
                ));
            }
        } else {
            for (name, set) in [
                ("label", material.label.is_some()),
                ("density_kg_m3", material.density_kg_m3.is_some()),
                ("composition_basis", material.composition_basis.is_some()),
                ("components", material.components.is_some()),
                ("provenance", material.provenance.is_some()),
            ] {
                // A null is already listed by path above.
                if !set && !findings.iter().any(|f| f.item == format!("{item}.{name}")) {
                    findings.push(Finding::new(
                        CHECK,
                        format!("{item}.{name}"),
                        "a material without catalog_id needs a full recipe and this key is missing",
                        format!("Add {name}, or use catalog_id to take a catalog material."),
                    ));
                }
            }
        }
        if let Some(provenance) = &material.provenance {
            provenance_rules(&format!("{item}.provenance"), provenance, findings);
        }
        for (j, component) in material.components.iter().flatten().enumerate() {
            let item = format!("{item}.components[{j}]");
            match (&component.element, &component.nuclide) {
                (Some(_), Some(_)) | (None, None) => findings.push(Finding::new(
                    CHECK,
                    item.clone(),
                    "a component names exactly one of element and nuclide",
                    "Keep one of the two keys.",
                )),
                _ => {}
            }
            if component.isotopes.is_some() && component.element.is_none() {
                findings.push(Finding::new(
                    CHECK,
                    format!("{item}.isotopes"),
                    "isotopes split an element, and this component names a nuclide",
                    "Remove isotopes, or name an element.",
                ));
            }
        }
    }
    for (i, group) in design.replacement_groups.iter().enumerate() {
        let item = format!("replacement_groups[{i}]");
        if group.limits.is_empty() {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.limits"),
                "a replacement group needs at least one governing limit",
                "Add a limit, or remove the group and set its solids' replacement_group_id to null.",
            ));
        }
        for (j, limit) in group.limits.iter().enumerate() {
            let item = format!("{item}.limits[{j}]");
            provenance_rules(&format!("{item}.provenance"), &limit.provenance, findings);
            match (limit.metric, limit.energy_threshold_ev) {
                (Metric::FastNeutronFluence, None) => findings.push(Finding::new(
                    CHECK,
                    format!("{item}.energy_threshold_ev"),
                    "a fast_neutron_fluence limit needs the energy above which neutrons count",
                    "Add energy_threshold_ev, for example 1e5 for REBCO.",
                )),
                (Metric::FastNeutronFluence, Some(_)) | (_, None) => {}
                (other, Some(_)) => findings.push(Finding::new(
                    CHECK,
                    format!("{item}.energy_threshold_ev"),
                    format!(
                        "energy_threshold_ev applies only to fast_neutron_fluence, not {}",
                        other.label()
                    ),
                    "Remove energy_threshold_ev.",
                )),
            }
            if let Averaging::PeakMesh { voxel_m } = &limit.averaging
                && voxel_m.len() != 3
            {
                findings.push(Finding::new(
                    CHECK,
                    format!("{item}.averaging.voxel_m"),
                    format!("it has {} entries; peak_mesh needs [dr, dz, dphi]", voxel_m.len()),
                    "Give three entries in metres; null means the full solid width in that direction.",
                ));
            }
        }
        if let ReplacementDuration::Fixed { provenance, .. } = &group.replacement_duration {
            provenance_rules(
                &format!("{item}.replacement_duration.provenance"),
                provenance,
                findings,
            );
        }
    }
    if let Some(provenance) = &design.plasma.provenance {
        provenance_rules("plasma.provenance", provenance, findings);
    }
    for (name, provenance) in &design.plasma.field_provenance {
        provenance_rules(
            &format!("plasma.field_provenance.{name}"),
            provenance,
            findings,
        );
    }
}
