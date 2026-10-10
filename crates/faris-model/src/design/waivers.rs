//! Waivers (GEO-026): an author's recorded acceptance of one integrity finding.
//!
//! A waiver names the check, the solids or the point it covers, a reason, an
//! author and a date. A waived finding stays in the report and in every receipt;
//! it just no longer stops the import. Watertightness, orientation, inverted
//! normals, zero-volume cells and degenerate triangles cannot be waived,
//! because transport is not defined without them. A waiver must match exactly:
//! the solid ids as a set, and a point to a nanometre. Nothing matches loosely.

use super::{Design, Finding};
use serde::{Deserialize, Serialize};

/// Waiver ids for findings a person may accept.
pub const WAIVABLE_IDS: [&str; 9] = [
    "GEO-021",
    "GEO-022",
    "GEO-024",
    "GEO-025",
    "GEO-031",
    "GEO-032",
    "SLIVER",
    "SELF-INTERSECTION",
    "DUPLICATE-SURFACE",
];

/// Ids that name a class which cannot be waived.
pub const NON_WAIVABLE_IDS: [&str; 4] = [
    "GEO-020",
    "INVERTED-NORMALS",
    "ZERO-VOLUME",
    "DEGENERATE-TRIANGLE",
];

pub fn waiver_id_is_known(id: &str) -> bool {
    WAIVABLE_IDS.contains(&id) || NON_WAIVABLE_IDS.contains(&id)
}

/// Point equality for a waiver, metres. The report prints locations to more
/// digits than this, so copying a reported point always matches.
const POINT_TOLERANCE_M: f64 = 1e-9;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub check_id: String,
    /// The solids the waiver covers, as a set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub solid_ids: Vec<String>,
    /// A point in the model, for a gap (the cavity's interior point as reported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_m: Option<[f64; 3]>,
    pub reason: String,
    pub author: String,
    /// `YYYY-MM-DD`.
    pub date: String,
}

/// What a finding records about the waiver that covers it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WaiverRecord {
    /// Position in the design file's `waivers` array.
    pub index: usize,
    pub check_id: String,
    pub author: String,
    pub date: String,
    pub reason: String,
}

fn valid_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    let [y, m, d] = parts[..] else {
        return false;
    };
    let numeric = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    if !(numeric(y, 4) && numeric(m, 2) && numeric(d, 2)) {
        return false;
    }
    let (m, d): (u32, u32) = (m.parse().unwrap_or(0), d.parse().unwrap_or(0));
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// Check 1 rules for `waivers[]`.
pub fn check_waivers(design: &Design) -> Vec<Finding> {
    const CHECK: u8 = 1;
    let mut findings = Vec::new();
    for (i, waiver) in design.waivers.iter().enumerate() {
        let item = format!("waivers[{i}]");
        if !waiver_id_is_known(&waiver.check_id) {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.check_id"),
                format!("{:?} is not a check id", waiver.check_id),
                format!("Use one of {}.", WAIVABLE_IDS.join(", ")),
            ));
        } else if NON_WAIVABLE_IDS.contains(&waiver.check_id.as_str()) {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.check_id"),
                format!(
                    "{} cannot be waived: transport is not defined for a model that fails it",
                    waiver.check_id
                ),
                "Fix the geometry in the CAD tool and export the STEP file again.",
            ));
        }
        for (name, value) in [("reason", &waiver.reason), ("author", &waiver.author)] {
            if value.trim().is_empty() {
                findings.push(Finding::new(
                    CHECK,
                    format!("{item}.{name}"),
                    "it is empty; a waiver records who accepted the finding and why",
                    format!("Write the {name}."),
                ));
            }
        }
        if !valid_date(&waiver.date) {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.date"),
                format!("{:?} is not a date written YYYY-MM-DD", waiver.date),
                "Write the date the finding was accepted, for example 2026-10-10.",
            ));
        }
        if waiver.solid_ids.is_empty() && waiver.point_m.is_none() {
            findings.push(Finding::new(
                CHECK,
                item.clone(),
                "it names neither solid_ids nor point_m, so it covers nothing",
                "Give the solid ids of the finding, or for a gap the interior point FARIS reported.",
            ));
        }
        if let Some(p) = waiver.point_m
            && p.iter().any(|c| !c.is_finite())
        {
            findings.push(Finding::new(
                CHECK,
                format!("{item}.point_m"),
                "a coordinate is not a finite number",
                "Copy the point from the check report.",
            ));
        }
    }
    findings
}

/// Check 4 rule: every solid a waiver names exists.
pub fn waiver_references(design: &Design) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (i, waiver) in design.waivers.iter().enumerate() {
        for (j, id) in waiver.solid_ids.iter().enumerate() {
            if design.solid(id).is_none() {
                findings.push(Finding::new(
                    4,
                    format!("waivers[{i}].solid_ids[{j}]"),
                    format!("there is no solid with id {id:?}"),
                    "Name a solid of the design file, or remove the waiver.",
                ));
            }
        }
    }
    findings
}

fn covers(waiver: &Waiver, finding: &Finding) -> bool {
    if !finding.waivable {
        return false;
    }
    let named = finding.requirement.as_deref() == Some(waiver.check_id.as_str())
        || finding.also_covered_by.contains(&waiver.check_id);
    if !named {
        return false;
    }
    if !waiver.solid_ids.is_empty() {
        let mut ids = waiver.solid_ids.clone();
        ids.sort();
        ids.dedup();
        if ids != finding.cells {
            return false;
        }
    }
    if let Some(p) = waiver.point_m {
        match finding.location_m {
            Some(l)
                if l.iter()
                    .zip(p)
                    .all(|(a, b)| (a - b).abs() <= POINT_TOLERANCE_M) => {}
            _ => return false,
        }
    }
    true
}

/// Mark every finding a waiver covers. Returns, per waiver, whether it matched
/// anything, so the report can say which waivers covered nothing.
pub fn apply_waivers(findings: &mut [Finding], waivers: &[Waiver]) -> Vec<bool> {
    let mut used = vec![false; waivers.len()];
    for finding in findings.iter_mut() {
        if finding.waived.is_some() {
            continue;
        }
        if let Some((index, waiver)) = waivers
            .iter()
            .enumerate()
            .find(|(_, waiver)| covers(waiver, finding))
        {
            used[index] = true;
            finding.waived = Some(WaiverRecord {
                index,
                check_id: waiver.check_id.clone(),
                author: waiver.author.clone(),
                date: waiver.date.clone(),
                reason: waiver.reason.clone(),
            });
        }
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiver(check_id: &str, ids: &[&str]) -> Waiver {
        Waiver {
            check_id: check_id.into(),
            solid_ids: ids.iter().map(|s| (*s).to_string()).collect(),
            point_m: None,
            reason: "thin sacrificial layer".into(),
            author: "A. Designer".into(),
            date: "2026-10-10".into(),
        }
    }

    fn overlap(ids: &[&str]) -> Finding {
        Finding::new(8, "pair", "overlap", "fix")
            .requirement("GEO-021")
            .cells(ids.iter().map(|s| (*s).to_string()).collect())
            .waivable()
    }

    // Verifies: GEO-026
    #[test]
    fn a_waiver_covers_exactly_the_finding_it_names() {
        let mut findings = vec![overlap(&["a", "b"]), overlap(&["a", "c"])];
        let used = apply_waivers(&mut findings, &[waiver("GEO-021", &["b", "a"])]);
        assert_eq!(used, vec![true]);
        assert!(findings[0].waived.is_some());
        assert!(!findings[0].blocks());
        assert!(findings[1].blocks());
        assert_eq!(findings[0].waived.as_ref().unwrap().author, "A. Designer");
    }

    #[test]
    fn a_waiver_for_another_check_or_a_subset_covers_nothing() {
        let mut findings = vec![overlap(&["a", "b"])];
        let used = apply_waivers(
            &mut findings,
            &[waiver("GEO-022", &["a", "b"]), waiver("GEO-021", &["a"])],
        );
        assert_eq!(used, vec![false, false]);
        assert!(findings[0].blocks());
    }

    #[test]
    fn a_finding_that_is_not_waivable_stays_blocking() {
        let mut f = vec![
            Finding::new(8, "edge", "open edge", "fix")
                .requirement("GEO-020")
                .cells(vec!["a".into()]),
        ];
        assert_eq!(
            apply_waivers(&mut f, &[waiver("GEO-020", &["a"])]),
            vec![false]
        );
        assert!(f[0].blocks());
    }

    #[test]
    fn a_secondary_waiver_id_covers_a_sampling_finding() {
        let mut f = vec![
            Finding::new(8, "samples", "in both", "fix")
                .requirement("GEO-024")
                .also_covered_by(&["GEO-021"])
                .cells(vec!["a".into(), "b".into()])
                .waivable(),
        ];
        assert_eq!(
            apply_waivers(&mut f, &[waiver("GEO-021", &["a", "b"])]),
            vec![true]
        );
    }

    #[test]
    fn a_point_must_match_to_a_nanometre() {
        let mut f = vec![
            Finding::new(8, "cavity", "gap", "fix")
                .requirement("GEO-022")
                .at([1.0, 2.0, 3.0])
                .waivable(),
        ];
        let mut w = waiver("GEO-022", &[]);
        w.solid_ids.clear();
        w.point_m = Some([1.0, 2.0, 3.000_000_1]);
        assert_eq!(apply_waivers(&mut f, std::slice::from_ref(&w)), vec![false]);
        w.point_m = Some([1.0, 2.0, 3.0 + 5e-10]);
        assert_eq!(apply_waivers(&mut f, &[w]), vec![true]);
    }

    #[test]
    fn dates_are_checked() {
        assert!(valid_date("2026-10-10"));
        for bad in [
            "2026-1-1",
            "2026-13-01",
            "20261010",
            "2026-10-32",
            "yyyy-mm-dd",
            "",
        ] {
            assert!(!valid_date(bad), "{bad}");
        }
    }
}
