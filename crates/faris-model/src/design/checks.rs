//! Import checks 2 to 5. Pure functions: the STEP file is represented by the
//! hash FARIS computed and the CAD helper's inspection result. Each returns
//! every finding of its stage; the caller stops before the next stage when a
//! stage has findings.

use super::{
    Averaging, CENTROID_TOLERANCE, Design, FRACTION_SUM_TOLERANCE, Finding, Metric, Notice,
    ReplacementDuration, Role, StepInspection, VOID, VOLUME_TOLERANCE, is_reserved, valid_id,
};
use crate::history::OperatingHistoryAssumptions;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::catalog::{catalog, catalog_material};

/// What the engine learned about the linked operating-history file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioFacts {
    pub sha256: String,
    /// Set when the file is not a valid `faris-operating-history/v0.1` document.
    pub problem: Option<String>,
    pub service_limits: usize,
}

/// Hash and read the operating-history file's bytes.
pub fn scenario_facts(bytes: &[u8]) -> ScenarioFacts {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    match serde_json::from_slice::<OperatingHistoryAssumptions>(bytes) {
        Ok(history) => ScenarioFacts {
            sha256,
            problem: history.validate().err(),
            service_limits: history.service_limits.len(),
        },
        Err(error) => ScenarioFacts {
            sha256,
            problem: Some(format!(
                "it does not parse as faris-operating-history/v0.1: {error}"
            )),
            service_limits: 0,
        },
    }
}

fn short(hash: &str) -> String {
    hash.chars().take(16).collect()
}

/// Check 2, the parts that need no CAD kernel: STEP hash, extent, machine axis
/// and the linked operating-history file. `scenario` is `Err(text)` when the
/// file could not be read.
pub fn check_2_static(
    design: &Design,
    actual_step_sha256: &str,
    scenario: Result<&ScenarioFacts, &str>,
) -> Vec<Finding> {
    const CHECK: u8 = 2;
    let mut findings = Vec::new();
    if !design
        .cad
        .step_sha256
        .eq_ignore_ascii_case(actual_step_sha256)
    {
        findings.push(Finding::new(
            CHECK,
            "cad.step_sha256",
            format!(
                "the STEP file {} hashes to {}..., the design file records {}...; the STEP file \
                 is not the one this design was written for (GEO-019)",
                design.cad.step_file,
                short(actual_step_sha256),
                short(&design.cad.step_sha256)
            ),
            "If you changed the STEP file on purpose, run `faris design init` on it again and \
             carry over your materials and roles. Otherwise restore the original STEP file.",
        ));
    }
    match design.cad.extent.kind.as_str() {
        "full" => {}
        "sector" => findings.push(Finding::new(
            CHECK,
            "cad.extent",
            "this is a sector model. DAGMC in OpenMC 0.15.3 has no periodic boundary, and FARIS \
             does not yet replicate sectors",
            "Export the full model by patterning the sector in the CAD tool, run `faris design \
             init` on it, and set extent to {\"kind\": \"full\"}.",
        )),
        other => findings.push(Finding::new(
            CHECK,
            "cad.extent",
            format!("the extent kind {other:?} is not known; v0.1 accepts only \"full\""),
            "Set extent to {\"kind\": \"full\"} for a full 360-degree model.",
        )),
    }
    if design.cad.machine_axis != "z" {
        findings.push(Finding::new(
            CHECK,
            "cad.machine_axis",
            format!(
                "the machine axis is {:?}. The plasma source is built about +z through the \
                 origin, and v0.1 supports no other axis",
                design.cad.machine_axis
            ),
            "Place the model with its machine axis on +z through the origin in the CAD tool, \
             export it again, and set machine_axis to \"z\".",
        ));
    }
    let link = &design.operating_scenario;
    let (Some(path), Some(expected)) = (&link.path, &link.sha256) else {
        return findings; // nulls are check 1's
    };
    match scenario {
        Err(error) => findings.push(Finding::new(
            CHECK,
            "operating_scenario.path",
            format!("the operating-history file {path} could not be read: {error}"),
            "Put the file at that path (relative to the design file) or correct the path.",
        )),
        Ok(facts) => {
            if !expected.eq_ignore_ascii_case(&facts.sha256) {
                findings.push(Finding::new(
                    CHECK,
                    "operating_scenario.sha256",
                    format!(
                        "{path} hashes to {}..., the design file records {}...",
                        short(&facts.sha256),
                        short(expected)
                    ),
                    "If you edited the operating-history file on purpose, copy its new sha256 \
                     into the design file. Otherwise restore the original.",
                ));
            }
            if let Some(problem) = &facts.problem {
                findings.push(Finding::new(
                    CHECK,
                    "operating_scenario.path",
                    format!("{path} is not a usable operating-history file: {problem}"),
                    "Fix the file, or link a faris-operating-history/v0.1 document.",
                ));
            }
            if facts.service_limits > 0 {
                findings.push(Finding::new(
                    CHECK,
                    "operating_scenario.path",
                    format!(
                        "{path} has {} service_limits. A CAD design takes its limits from \
                         replacement_groups, and two sets of limits could disagree without notice",
                        facts.service_limits
                    ),
                    "Remove service_limits from the operating-history file (set it to []) and \
                     put the limits on the design's replacement groups.",
                ));
            }
        }
    }
    findings
}

/// Check 2, the parts that need the helper: it read the file, the unit agrees.
pub fn check_2_unit(
    design: &Design,
    actual_step_sha256: &str,
    inspection: &StepInspection,
) -> (Vec<Finding>, Vec<Notice>) {
    const CHECK: u8 = 2;
    let mut findings = Vec::new();
    let mut notices = Vec::new();
    for problem in &inspection.problems {
        findings.push(Finding::new(
            CHECK,
            "cad.step_file",
            problem.clone(),
            "Check that the file is a valid STEP file (AP203, AP214 or AP242) and export it again.",
        ));
    }
    if !inspection
        .step
        .sha256
        .eq_ignore_ascii_case(actual_step_sha256)
    {
        findings.push(Finding::new(
            CHECK,
            "cad.step_file",
            "the file changed while the CAD helper was reading it",
            "Do not edit the STEP file during a check; run the check again.",
        ));
    }
    for problem in &inspection.unit.problems {
        findings.push(Finding::new(
            CHECK,
            "cad.length_unit",
            format!("the STEP file's length unit cannot be trusted: {problem}"),
            "Export the STEP file again from the CAD tool with one length unit (mm, cm or m).",
        ));
    }
    if inspection.problems.is_empty() && inspection.unit.problems.is_empty() {
        match inspection.unit.declared.as_deref() {
            Some(declared) if declared == design.cad.length_unit.label() => {}
            Some(declared) => {
                let accepted = matches!(declared, "m" | "cm" | "mm");
                findings.push(Finding::new(
                    CHECK,
                    "cad.length_unit",
                    if accepted {
                        format!(
                            "the STEP file declares {declared}, the design file says {}; a wrong \
                             unit would scale every volume by 10^3 or more without warning (GEO-011)",
                            design.cad.length_unit.label()
                        )
                    } else {
                        format!(
                            "the STEP file declares {declared}, which v0.1 does not accept \
                             (it accepts m, cm and mm)"
                        )
                    },
                    if accepted {
                        format!("Set length_unit to \"{declared}\", the unit the file declares.")
                    } else {
                        "Export the STEP file again in mm, cm or m.".to_string()
                    },
                ));
            }
            None => notices.push(Notice::new(
                CHECK,
                "cad.length_unit",
                format!(
                    "the STEP file declares no length unit; its numbers were read in {}, the \
                     unit the design file gives",
                    design.cad.length_unit.label()
                ),
            )),
        }
    }
    (findings, notices)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Check 3: solid count and fingerprints.
pub fn check_3(design: &Design, inspection: &StepInspection) -> (Vec<Finding>, Vec<Notice>) {
    const CHECK: u8 = 3;
    let mut findings = Vec::new();
    let mut notices = Vec::new();
    let found = inspection.solids.len();
    let listed = design.solids.len();
    if found != listed {
        findings.push(Finding::new(
            CHECK,
            "solids",
            format!("the STEP file has {found} solids and the design file lists {listed}; no solid may be dropped or invented (GEO-010)"),
            "Run `faris design init` on the STEP file to see every solid, and make solids[] match it.",
        ));
    }
    let Some(diagonal) = inspection.model_bbox_diagonal_m else {
        findings.push(Finding::new(
            CHECK,
            "solids",
            "the CAD helper gave no model bounding box, so centroids cannot be compared",
            "Check that the STEP file holds at least one solid.",
        ));
        return (findings, notices);
    };
    let centroid_tolerance = CENTROID_TOLERANCE * diagonal;
    let mut seen_index: BTreeMap<usize, &str> = BTreeMap::new();
    for (i, solid) in design.solids.iter().enumerate() {
        let item = format!("solids[{i}] ({})", solid.id);
        if let Some(first) = seen_index.insert(solid.step_index, &solid.id) {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.step_index"),
                format!(
                    "step_index {} is also used by solid {first}",
                    solid.step_index
                ),
                "Give each solid its own step_index, as `faris design init` wrote them.",
            ));
            continue;
        }
        let Some(measured) = inspection.solids.get(solid.step_index) else {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.step_index"),
                format!(
                    "step_index {} does not exist; the STEP file has {found} solids (0 to {})",
                    solid.step_index,
                    found.saturating_sub(1)
                ),
                "Correct step_index, or remove the entry if its solid is gone.",
            ));
            continue;
        };
        let (Some(volume), Some(centroid)) = (measured.cad_volume_m3, measured.centroid_m) else {
            findings.push(Finding::new(
                CHECK,
                item,
                "the CAD helper gave no volume for this solid",
                "Check the STEP file's length unit and run the check again.",
            ));
            continue;
        };
        let fingerprint = &solid.fingerprint;
        let volume_error =
            (fingerprint.cad_volume_m3 - volume).abs() / volume.abs().max(f64::MIN_POSITIVE);
        if !(volume_error <= VOLUME_TOLERANCE) {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.fingerprint.cad_volume_m3"),
                format!(
                    "STEP solid {} has volume {:.9e} m3, the design file records {:.9e} m3 \
                     (relative difference {:.3e}, tolerance {:.0e}); the STEP file was edited \
                     after the design file was written, or the entry points at the wrong solid",
                    solid.step_index,
                    volume,
                    fingerprint.cad_volume_m3,
                    volume_error,
                    VOLUME_TOLERANCE
                ),
                "Run `faris design init` on the current STEP file and compare, then correct \
                 step_index or the fingerprint.",
            ));
        }
        let centroid_error = distance(fingerprint.centroid_m, centroid);
        if !(centroid_error <= centroid_tolerance) {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.fingerprint.centroid_m"),
                format!(
                    "STEP solid {} has its centroid {centroid_error:.3e} m from the design \
                     file's (tolerance {centroid_tolerance:.3e} m, which is {CENTROID_TOLERANCE:.0e} of the \
                     {diagonal:.3} m model diagonal); the solid moved or the entry points at the wrong one",
                    solid.step_index
                ),
                "Run `faris design init` on the current STEP file and compare, then correct \
                 step_index or the fingerprint.",
            ));
        }
        if solid.step_name != measured.step_name {
            notices.push(Notice::new(
                CHECK,
                item.clone(),
                format!(
                    "the STEP name is {:?} but the design file records {:?}; names are compared \
                     but never used to bind a solid",
                    measured.step_name, solid.step_name
                ),
            ));
        }
    }
    for index in 0..found {
        if !seen_index.contains_key(&index) {
            let solid = &inspection.solids[index];
            findings.push(Finding::new(
                CHECK,
                format!("STEP solid {index}"),
                format!(
                    "the STEP file's solid {index}{} has no entry in the design file; a dropped \
                     solid would be missing from the transport model",
                    solid
                        .step_name
                        .as_ref()
                        .map(|n| format!(" ({n:?})"))
                        .unwrap_or_default()
                ),
                "Add an entry for it to solids[], as `faris design init` would write it.",
            ));
        }
    }
    // Two STEP solids that cannot be told apart by fingerprint stop the import.
    for a in 0..found {
        for b in (a + 1)..found {
            let (sa, sb) = (&inspection.solids[a], &inspection.solids[b]);
            let (Some(va), Some(vb), Some(ca), Some(cb)) = (
                sa.cad_volume_m3,
                sb.cad_volume_m3,
                sa.centroid_m,
                sb.centroid_m,
            ) else {
                continue;
            };
            if (va - vb).abs() <= VOLUME_TOLERANCE * va.abs().max(vb.abs())
                && distance(ca, cb) <= centroid_tolerance
            {
                let label = |i: usize| {
                    design
                        .solids
                        .iter()
                        .find(|s| s.step_index == i)
                        .map_or_else(
                            || format!("STEP solid {i}"),
                            |s| format!("{} (STEP solid {i})", s.id),
                        )
                };
                findings.push(Finding::new(
                    CHECK,
                    format!("{} and {}", label(a), label(b)),
                    "their volumes and centroids are the same within tolerance, so FARIS cannot \
                     tell them apart after conversion; coincident solids are an overlap (GEO-021)",
                    "Remove the duplicate solid in the CAD tool and export again.",
                ));
            }
        }
    }
    for solid in &inspection.solids {
        if !solid.occt_valid {
            notices.push(Notice::new(
                CHECK,
                format!("STEP solid {}", solid.step_index),
                "OpenCASCADE's shape check (BRepCheck) reports this solid as invalid; \
                 conversion may fail or give a wrong volume",
            ));
        }
    }
    (findings, notices)
}

/// Check 4: ids are unique and valid; every reference resolves.
pub fn check_4(design: &Design) -> Vec<Finding> {
    const CHECK: u8 = 4;
    let mut findings = Vec::new();
    let id_rules = |kind: &str, i: usize, id: &str, findings: &mut Vec<Finding>| {
        let item = format!("{kind}[{i}].id");
        if is_reserved(id) {
            findings.push(Finding::new(
                CHECK,
                item,
                format!(
                    "{id:?} is reserved: DAGMC treats vacuum and graveyard (in any case) as \
                     special, and void is the built-in empty material"
                ),
                "Choose another id, for example by adding a word to it.",
            ));
        } else if !valid_id(id) {
            findings.push(Finding::new(
                CHECK,
                item,
                format!(
                    "{id:?} does not match ^[a-z0-9][a-z0-9-]{{0,27}}$; ids are short so the \
                     28-character DAGMC tag is never cut"
                ),
                "Use 1 to 28 characters: lower-case letters, digits and hyphens, starting with a \
                 letter or digit.",
            ));
        }
    };
    let duplicates = |kind: &str, ids: Vec<&str>, findings: &mut Vec<Finding>| {
        let mut first: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, id) in ids.into_iter().enumerate() {
            if let Some(j) = first.insert(id, i) {
                findings.push(Finding::new(
                    CHECK,
                    format!("{kind}[{i}].id"),
                    format!("the id {id:?} is already used by {kind}[{j}]"),
                    "Give each entry its own id.",
                ));
                first.insert(id, j);
            }
        }
    };
    for (i, solid) in design.solids.iter().enumerate() {
        id_rules("solids", i, &solid.id, &mut findings);
    }
    for (i, material) in design.materials.iter().enumerate() {
        id_rules("materials", i, &material.id, &mut findings);
    }
    duplicates(
        "solids",
        design.solids.iter().map(|s| s.id.as_str()).collect(),
        &mut findings,
    );
    duplicates(
        "materials",
        design.materials.iter().map(|m| m.id.as_str()).collect(),
        &mut findings,
    );
    duplicates(
        "replacement_groups",
        design
            .replacement_groups
            .iter()
            .map(|g| g.id.as_str())
            .collect(),
        &mut findings,
    );
    duplicates(
        "references",
        design.references.iter().map(|r| r.id.as_str()).collect(),
        &mut findings,
    );

    let reference_ok = |id: &str| design.references.iter().any(|r| r.id == id);
    let check_reference =
        |item: String, reference_id: &Option<String>, findings: &mut Vec<Finding>| {
            if let Some(id) = reference_id
                && !reference_ok(id)
            {
                findings.push(Finding::new(
                    CHECK,
                    item,
                    format!("the reference {id:?} is not in references[]"),
                    "Add the reference (id, title, url, use) to references[], or correct the id.",
                ));
            }
        };
    for (i, material) in design.materials.iter().enumerate() {
        if let Some(catalog_id) = &material.catalog_id
            && catalog_material(catalog_id).is_none()
        {
            findings.push(Finding::new(
                CHECK,
                format!("materials[{i}].catalog_id"),
                format!(
                    "{catalog_id:?} is not a catalog material; the catalog holds {}",
                    {
                        let ids: Vec<&str> = catalog().iter().map(|m| m.id.as_str()).collect();
                        ids.join(", ")
                    }
                ),
                "Use one of those ids, or write the full recipe.",
            ));
        }
        if let Some(provenance) = &material.provenance {
            check_reference(
                format!("materials[{i}].provenance.reference_id"),
                &provenance.reference_id,
                &mut findings,
            );
        }
    }
    for (i, solid) in design.solids.iter().enumerate() {
        if let Some(material_id) = &solid.material_id
            && material_id != VOID
            && !design.materials.iter().any(|m| &m.id == material_id)
        {
            findings.push(Finding::new(
                CHECK,
                format!("solids[{i}] ({}).material_id", solid.id),
                format!("the material {material_id:?} is not in materials[]"),
                "Add the material to materials[], or use an id that is there (or \"void\").",
            ));
        }
        if let Some(group_id) = &solid.replacement_group_id
            && !design.replacement_groups.iter().any(|g| &g.id == group_id)
        {
            findings.push(Finding::new(
                CHECK,
                format!("solids[{i}] ({}).replacement_group_id", solid.id),
                format!("the replacement group {group_id:?} is not in replacement_groups[]"),
                "Add the group, or correct the id, or set it to null for a permanent solid.",
            ));
        }
    }
    for (i, group) in design.replacement_groups.iter().enumerate() {
        if !design
            .solids
            .iter()
            .any(|s| s.replacement_group_id.as_deref() == Some(group.id.as_str()))
        {
            findings.push(Finding::new(
                CHECK,
                format!("replacement_groups[{i}] ({})", group.id),
                "no solid belongs to this group, so its limits could never be reached",
                "Set replacement_group_id on the solids it replaces, or remove the group.",
            ));
        }
        for (j, limit) in group.limits.iter().enumerate() {
            check_reference(
                format!("replacement_groups[{i}].limits[{j}].provenance.reference_id"),
                &limit.provenance.reference_id,
                &mut findings,
            );
        }
        if let ReplacementDuration::Fixed { provenance, .. } = &group.replacement_duration {
            check_reference(
                format!("replacement_groups[{i}].replacement_duration.provenance.reference_id"),
                &provenance.reference_id,
                &mut findings,
            );
        }
    }
    if let Some(chamber) = &design.plasma.chamber_solid_id
        && design.solid(chamber).is_none()
    {
        findings.push(Finding::new(
            CHECK,
            "plasma.chamber_solid_id",
            format!("the solid {chamber:?} is not in solids[]"),
            "Set chamber_solid_id to the id of the plasma_chamber solid.",
        ));
    }
    if let Some(provenance) = &design.plasma.provenance {
        check_reference(
            "plasma.provenance.reference_id".into(),
            &provenance.reference_id,
            &mut findings,
        );
    }
    for (name, provenance) in &design.plasma.field_provenance {
        check_reference(
            format!("plasma.field_provenance.{name}.reference_id"),
            &provenance.reference_id,
            &mut findings,
        );
    }
    findings
}

fn element_symbol(name: &str) -> bool {
    let b = name.as_bytes();
    matches!(b.len(), 1 | 2)
        && b[0].is_ascii_uppercase()
        && b[1..].iter().all(u8::is_ascii_lowercase)
}

/// The element symbol of a nuclide name such as `Fe56`, `Li6` or `Ag110_m1`.
fn nuclide_symbol(name: &str) -> Option<&str> {
    let end = name.find(|c: char| c.is_ascii_digit())?;
    let (symbol, rest) = name.split_at(end);
    let (digits, metastable) = rest.split_at(rest.find('_').unwrap_or(rest.len()));
    let metastable_ok = metastable.is_empty()
        || metastable
            .strip_prefix("_m")
            .is_some_and(|m| !m.is_empty() && m.bytes().all(|c| c.is_ascii_digit()));
    (element_symbol(symbol)
        && (1..=3).contains(&digits.len())
        && digits.bytes().all(|c| c.is_ascii_digit())
        && metastable_ok)
        .then_some(symbol)
}

/// A void solid; every one is listed in the import report (GEO-039).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VoidSolid {
    pub id: String,
    pub step_index: usize,
    pub role: Role,
}

pub fn void_solids(design: &Design) -> Vec<VoidSolid> {
    design
        .solids
        .iter()
        .filter(|s| s.material_id.as_deref() == Some(VOID))
        .filter_map(|s| {
            Some(VoidSolid {
                id: s.id.clone(),
                step_index: s.step_index,
                role: s.role?,
            })
        })
        .collect()
}

/// Check 5, without the nuclear-data audit (that needs OpenMC and is run by the
/// engine): role and material rules, plus notices for limits not yet evaluated.
pub fn check_5(design: &Design) -> (Vec<Finding>, Vec<Notice>) {
    const CHECK: u8 = 5;
    let mut findings = Vec::new();
    let mut notices = Vec::new();
    let chambers: Vec<&str> = design
        .solids
        .iter()
        .filter(|s| s.role == Some(Role::PlasmaChamber))
        .map(|s| s.id.as_str())
        .collect();
    if chambers.len() != 1 {
        findings.push(Finding::new(
            CHECK,
            "solids[].role",
            if chambers.is_empty() {
                "no solid has the role plasma_chamber; the source sites are tested against it (SRC-018)".to_string()
            } else {
                format!("{} solids have the role plasma_chamber ({}); exactly one is allowed", chambers.len(), chambers.join(", "))
            },
            "Give exactly one solid the role plasma_chamber, the solid that holds the plasma.",
        ));
    }
    for (i, solid) in design.solids.iter().enumerate() {
        let (Some(role), Some(material)) = (solid.role, solid.material_id.as_deref()) else {
            continue;
        };
        let item = format!("solids[{i}] ({})", solid.id);
        if role == Role::PlasmaChamber && material != VOID {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.material_id"),
                format!("the plasma chamber has the material {material:?}; it must be \"void\""),
                "Set material_id to \"void\" for the plasma_chamber solid.",
            ));
        }
        if role == Role::Vacuum && material != VOID {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.material_id"),
                format!("a vacuum solid has the material {material:?}; it must be \"void\""),
                "Set material_id to \"void\", or give the solid another role.",
            ));
        }
        if material == VOID && !role.allows_void() {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.material_id"),
                format!(
                    "the role {} has the material void. Only plasma_chamber, vacuum and \
                     port_plug may be empty, so an empty blanket cannot pass unseen",
                    role.label()
                ),
                "Give the solid a material, or change its role to vacuum or port_plug if it is empty by design.",
            ));
        }
    }
    if let (Some(chamber), [only]) = (&design.plasma.chamber_solid_id, chambers.as_slice())
        && chamber != only
        && design.solid(chamber).is_some()
    {
        findings.push(Finding::new(
            CHECK,
            "plasma.chamber_solid_id",
            format!("it names {chamber:?}, but the plasma_chamber solid is {only:?}"),
            format!("Set chamber_solid_id to {only:?}, or change the roles."),
        ));
    }
    for (i, material) in design.materials.iter().enumerate() {
        if material.catalog_id.is_some() {
            continue;
        }
        let item = format!("materials[{i}] ({})", material.id);
        if let Some(density) = material.density_kg_m3
            && !(density.is_finite() && density > 0.0)
        {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.density_kg_m3"),
                format!("the density is {density}; it must be positive"),
                "Give the density in kg/m3. Empty space is the \"void\" material, not a zero density.",
            ));
        }
        if let Some(temperature) = material.temperature_k
            && !(temperature.is_finite() && temperature > 0.0)
        {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.temperature_k"),
                format!("the temperature is {temperature} K; it must be positive"),
                "Give a temperature in kelvin, or null to use the library's nearest one.",
            ));
        }
        let components = material.components.as_deref().unwrap_or_default();
        if material.components.is_some() && components.is_empty() {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.components"),
                "the material has no components",
                "List the elements or nuclides and their fractions.",
            ));
        }
        let mut sum = 0.0;
        for (j, component) in components.iter().enumerate() {
            let item = format!("{item}.components[{j}]");
            sum += component.fraction;
            if !(component.fraction.is_finite() && component.fraction > 0.0) {
                findings.push(Finding::new(
                    CHECK,
                    format!("{item}.fraction"),
                    format!(
                        "the fraction is {}; it must be positive",
                        component.fraction
                    ),
                    "Give a fraction above 0, or remove the component.",
                ));
            }
            match (&component.element, &component.nuclide) {
                (Some(element), None) if !element_symbol(element) => findings.push(Finding::new(
                    CHECK,
                    format!("{item}.element"),
                    format!("{element:?} is not an element symbol such as Li or Fe"),
                    "Use the element's symbol with a capital first letter.",
                )),
                (None, Some(nuclide)) if nuclide_symbol(nuclide).is_none() => {
                    findings.push(Finding::new(
                        CHECK,
                        format!("{item}.nuclide"),
                        format!("{nuclide:?} is not a nuclide name such as Fe56 or Li6"),
                        "Use the symbol and mass number, for example Fe56.",
                    ))
                }
                _ => {}
            }
            if let (Some(element), Some(isotopes)) = (&component.element, &component.isotopes) {
                let isotope_sum: f64 = isotopes.values().sum();
                if isotopes.is_empty() || (isotope_sum - 1.0).abs() > FRACTION_SUM_TOLERANCE {
                    findings.push(Finding::new(
                        CHECK,
                        format!("{item}.isotopes"),
                        format!("the isotope fractions sum to {isotope_sum}; they must sum to 1 within {FRACTION_SUM_TOLERANCE:e}"),
                        "Correct the isotope atom fractions of this element so they sum to 1.",
                    ));
                }
                for (name, fraction) in isotopes {
                    if nuclide_symbol(name) != Some(element.as_str()) {
                        findings.push(Finding::new(
                            CHECK,
                            format!("{item}.isotopes.{name}"),
                            format!("{name:?} is not a nuclide of {element}"),
                            "Use nuclide names of this element, for example Li6 and Li7 for Li.",
                        ));
                    }
                    if !(fraction.is_finite() && *fraction > 0.0) {
                        findings.push(Finding::new(
                            CHECK,
                            format!("{item}.isotopes.{name}"),
                            format!("the fraction is {fraction}; it must be positive"),
                            "Give a fraction above 0, or remove the isotope.",
                        ));
                    }
                }
            }
        }
        if !components.is_empty() && (sum - 1.0).abs() > FRACTION_SUM_TOLERANCE {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.components"),
                format!("the fractions sum to {sum}; they must sum to 1 within {FRACTION_SUM_TOLERANCE:e}"),
                "Correct the fractions so they sum to 1.",
            ));
        }
    }
    for (i, group) in design.replacement_groups.iter().enumerate() {
        for (j, limit) in group.limits.iter().enumerate() {
            let item = format!("replacement_groups[{i}] ({}).limits[{j}]", group.id);
            if !(limit.limit.is_finite() && limit.limit > 0.0) {
                findings.push(Finding::new(
                    CHECK,
                    format!("{item}.limit"),
                    format!("the limit is {}; it must be positive", limit.limit),
                    "Give the limit in the metric's SI unit: n/m2, dpa, appm or W/m3.",
                ));
            }
            if let Averaging::PeakMesh { voxel_m } = &limit.averaging {
                for (k, v) in voxel_m.iter().enumerate() {
                    if let Some(v) = v
                        && !(v.is_finite() && *v > 0.0)
                    {
                        findings.push(Finding::new(
                            CHECK,
                            format!("{item}.averaging.voxel_m[{k}]"),
                            format!("the voxel size is {v} m; it must be positive or null"),
                            "Give a size in metres, or null for the full solid width.",
                        ));
                    }
                }
            }
            if matches!(
                limit.metric,
                Metric::DisplacementsPerAtom | Metric::HeliumAppm
            ) {
                notices.push(Notice::new(
                    CHECK,
                    format!("{item}.metric"),
                    format!(
                        "NOT_EVALUATED: FARIS has no calculation for {} yet. The limit is \
                         recorded and shown as not evaluated, never as \"never reached\"",
                        limit.metric.label()
                    ),
                ));
            }
        }
        if group.replacement_duration == ReplacementDuration::Computed {
            notices.push(Notice::new(
                CHECK,
                format!(
                    "replacement_groups[{i}] ({}).replacement_duration",
                    group.id
                ),
                "NOT_EVALUATED: a computed replacement duration needs the dose-rate rule of \
                 stage S3, which does not exist yet; use a fixed duration until then",
            ));
        }
    }
    (findings, notices)
}
