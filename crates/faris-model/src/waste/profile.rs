//! Versioned jurisdiction profiles (SAFE-012).
//!
//! A profile (`data/waste/jurisdiction_profiles.json`, authored) names a jurisdiction, its
//! version, the date it was authored, the date it must be reviewed by, and the classification
//! schemes it holds. Every limit of those schemes takes its citation, source version and check
//! date from the scheme's own source record, so a limit cannot be listed without them. A profile
//! past its review date blocks verdicts: `check_current` refuses, and every classification made
//! through `regime::classify_regime` turns into NOT_EVALUATED with that reason.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::{NotEvaluated, SchemeInfo, fetter, tables, uk};

const PROFILES_JSON: &str = include_str!("../../data/waste/jurisdiction_profiles.json");

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FrameworkCitation {
    pub title: String,
    pub url: String,
    pub checked: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub version: String,
    pub authored: String,
    pub review_by: String,
    pub framework: String,
    pub framework_status: String,
    pub framework_citations: Vec<FrameworkCitation>,
    pub schemes: Vec<String>,
    pub review_note: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Profiles {
    pub schema: String,
    pub profiles: Vec<Profile>,
}

pub fn profiles() -> &'static Profiles {
    static PROFILES: OnceLock<Profiles> = OnceLock::new();
    PROFILES.get_or_init(|| {
        serde_json::from_str(PROFILES_JSON)
            .expect("the embedded jurisdiction profile file is valid")
    })
}

/// The profile that holds a scheme.
pub fn profile_for_scheme(scheme_id: &str) -> Option<&'static Profile> {
    profiles()
        .profiles
        .iter()
        .find(|p| p.schemes.iter().any(|s| s == scheme_id))
}

/// One limit with the citation, version and check date it carries (SAFE-012).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ProfileLimit {
    pub profile_id: String,
    pub profile_version: String,
    pub scheme_id: String,
    /// Row or criterion the value belongs to, as the source labels it.
    pub key: String,
    pub value: f64,
    pub unit: String,
    pub citation: String,
    pub source_version: String,
    /// Date the source was retrieved and read.
    pub checked: String,
}

/// A scheme's description and the date its source was retrieved, for every implemented scheme.
pub fn scheme_record(scheme_id: &str) -> Option<(SchemeInfo, String)> {
    match scheme_id {
        "nrc-10cfr61.55" => {
            let t = tables::nrc();
            Some((t.scheme_info(), t.source.retrieved.clone()))
        }
        "iaea-gsr3-table-i2" => {
            let t = tables::iaea();
            Some((t.scheme_info(), t.source.retrieved.clone()))
        }
        "uk-llw-ilw" => {
            let t = uk::uk_waste();
            Some((t.scheme_info(), t.retrieved().to_string()))
        }
        "uk-epr2016-sch23-table2" => {
            let t = uk::uk_clearance();
            Some((t.scheme_info(), t.source.retrieved.clone()))
        }
        "fetter-specific-activity" => {
            let t = fetter::fetter();
            Some((t.scheme_info(), t.source.retrieved.clone()))
        }
        _ => None,
    }
}

/// Every limit of a scheme as `(key, value, unit)`.
fn scheme_values(scheme_id: &str) -> Vec<(String, f64, String)> {
    let mut v: Vec<(String, f64, String)> = Vec::new();
    match scheme_id {
        "nrc-10cfr61.55" => {
            let t = tables::nrc();
            for r in &t.table1 {
                v.push((format!("Table 1: {}", r.label), r.limit, r.unit.clone()));
            }
            for r in &t.table2 {
                for column in 1..=3u8 {
                    if let Some(limit) = r.column(column) {
                        v.push((
                            format!("Table 2 column {column}: {}", r.label),
                            limit,
                            r.unit.clone(),
                        ));
                    }
                }
            }
        }
        "iaea-gsr3-table-i2" => {
            for l in &tables::iaea().levels {
                v.push((l.nuclide.clone(), l.bq_per_g, "Bq/g".to_string()));
            }
        }
        "uk-llw-ilw" => {
            let t = uk::uk_waste();
            let push = |v: &mut Vec<(String, f64, String)>, k: &str, x: f64, u: &str| {
                v.push((k.to_string(), x, u.to_string()));
            };
            push(&mut v, "LLW alpha", t.llw.alpha_gbq_per_te, "GBq/te");
            push(
                &mut v,
                "LLW beta/gamma",
                t.llw.beta_gamma_gbq_per_te,
                "GBq/te",
            );
            push(
                &mut v,
                "high-volume VLLW total activity",
                t.vllw_high_volume.total_mbq_per_te,
                "MBq/te",
            );
            push(
                &mut v,
                "high-volume VLLW tritium",
                t.vllw_high_volume.tritium_mbq_per_te,
                "MBq/te",
            );
            push(
                &mut v,
                "low-volume VLLW per 0.1 m3",
                t.vllw_low_volume.per_0_1_m3_kbq,
                "kBq",
            );
            push(
                &mut v,
                "low-volume VLLW per item",
                t.vllw_low_volume.per_item_kbq,
                "kBq",
            );
            push(
                &mut v,
                "low-volume VLLW C-14 and H-3 per 0.1 m3",
                t.vllw_low_volume.per_0_1_m3_c14_h3_kbq,
                "kBq",
            );
            push(
                &mut v,
                "low-volume VLLW C-14 and H-3 per item",
                t.vllw_low_volume.per_item_c14_h3_kbq,
                "kBq",
            );
        }
        "uk-epr2016-sch23-table2" => {
            for l in &uk::uk_clearance().levels {
                v.push((l.label.clone(), l.bq_per_g, "Bq/g".to_string()));
            }
        }
        "fetter-specific-activity" => {
            for l in &fetter::fetter().limits {
                if let Some(low) = l.sal_low_ci_per_m3 {
                    v.push((format!("{} lower SAL", l.nuclide), low, "Ci/m3".to_string()));
                }
                if let Some(high) = l.sal_high_ci_per_m3 {
                    v.push((
                        format!("{} upper SAL", l.nuclide),
                        high,
                        "Ci/m3".to_string(),
                    ));
                }
            }
        }
        _ => {}
    }
    v
}

/// Every limit of a profile, each with its citation, source version and check date.
pub fn limits(profile: &Profile) -> Vec<ProfileLimit> {
    let mut out = Vec::new();
    for scheme_id in &profile.schemes {
        let Some((info, checked)) = scheme_record(scheme_id) else {
            continue;
        };
        for (key, value, unit) in scheme_values(scheme_id) {
            out.push(ProfileLimit {
                profile_id: profile.id.clone(),
                profile_version: profile.version.clone(),
                scheme_id: scheme_id.clone(),
                key,
                value,
                unit,
                citation: info.citation.clone(),
                source_version: info.version.clone(),
                checked: checked.clone(),
            });
        }
    }
    out
}

/// `(year, month, day)` of an ISO date `YYYY-MM-DD`.
pub fn parse_date(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let (y, m, d): (u32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum ProfileStatus {
    #[serde(rename = "CURRENT")]
    Current,
    #[serde(rename = "PAST_REVIEW_DATE")]
    PastReviewDate,
    #[serde(rename = "DATE_UNREADABLE")]
    DateUnreadable,
}

impl ProfileStatus {
    pub fn text(self) -> &'static str {
        match self {
            ProfileStatus::Current => "CURRENT",
            ProfileStatus::PastReviewDate => "PAST_REVIEW_DATE",
            ProfileStatus::DateUnreadable => "DATE_UNREADABLE",
        }
    }
}

/// The status of a profile on a date. A profile is current through its review date.
pub fn status_on(profile: &Profile, today: &str) -> ProfileStatus {
    match (parse_date(today), parse_date(&profile.review_by)) {
        (Some(now), Some(review)) => {
            if now <= review {
                ProfileStatus::Current
            } else {
                ProfileStatus::PastReviewDate
            }
        }
        _ => ProfileStatus::DateUnreadable,
    }
}

/// Refuses when the profile is past its review date (or a date cannot be read): a stale profile
/// blocks verdicts.
pub fn check_current(profile: &Profile, today: &str) -> Result<(), NotEvaluated> {
    match status_on(profile, today) {
        ProfileStatus::Current => Ok(()),
        ProfileStatus::PastReviewDate => Err(NotEvaluated::new(
            format!(
                "the {} profile (version {}) was authored on {} and had to be reviewed by {}; the date is now {today}, so its limits may be out of date and no verdict is given",
                profile.name, profile.version, profile.authored, profile.review_by
            ),
            vec![format!("profile {} review", profile.id)],
            "Re-check the profile's sources against the current regulation, then update its version, authored and review_by dates in data/waste/jurisdiction_profiles.json.",
        )),
        ProfileStatus::DateUnreadable => Err(NotEvaluated::new(
            format!(
                "a date could not be read as YYYY-MM-DD (today `{today}`, review_by `{}`), so the profile cannot be shown to be current",
                profile.review_by
            ),
            vec!["date".to_string()],
            "Give the evaluation date as YYYY-MM-DD.",
        )),
    }
}

/// What a result carries about the profile it was made under.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ProfileStamp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub authored: String,
    pub review_by: String,
    pub evaluated_on: String,
    pub status: ProfileStatus,
    pub framework_status: String,
}

pub fn stamp(profile: &Profile, today: &str) -> ProfileStamp {
    ProfileStamp {
        id: profile.id.clone(),
        name: profile.name.clone(),
        version: profile.version.clone(),
        authored: profile.authored.clone(),
        review_by: profile.review_by.clone(),
        evaluated_on: today.to_string(),
        status: status_on(profile, today),
        framework_status: profile.framework_status.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> &'static Profile {
        profiles().profiles.iter().find(|p| p.id == id).unwrap()
    }

    // Verifies: SAFE-012
    #[test]
    fn profiles_exist_for_the_nrc_the_uk_and_the_iaea_and_hold_every_scheme_once() {
        let p = profiles();
        assert_eq!(p.schema, "faris-jurisdiction-profiles/1");
        for id in ["us-nrc", "uk", "iaea"] {
            assert!(p.profiles.iter().any(|x| x.id == id), "{id}");
        }
        let mut held: Vec<&str> = p
            .profiles
            .iter()
            .flat_map(|x| x.schemes.iter().map(String::as_str))
            .collect();
        held.sort_unstable();
        let before = held.len();
        held.dedup();
        assert_eq!(held.len(), before, "a scheme is held by two profiles");
        for id in &held {
            assert!(scheme_record(id).is_some(), "{id} has no record");
        }
        assert_eq!(held.len(), 5);
        for x in &p.profiles {
            assert!(parse_date(&x.authored).is_some() && parse_date(&x.review_by).is_some());
            assert!(
                parse_date(&x.authored) < parse_date(&x.review_by),
                "{}",
                x.id
            );
            assert!(!x.version.is_empty() && !x.framework_status.is_empty());
        }
        // The US profile says plainly that the fusion-machine rule is only proposed.
        assert!(profile("us-nrc").framework_status.contains("proposed rule"));
        assert!(profile("us-nrc").framework_status.contains("no final rule"));
    }

    // Verifies: SAFE-012
    #[test]
    fn every_limit_carries_a_citation_a_source_version_and_a_check_date() {
        let mut total = 0;
        for profile in &profiles().profiles {
            let all = limits(profile);
            assert!(!all.is_empty(), "{}", profile.id);
            for l in &all {
                assert_eq!(l.profile_id, profile.id);
                assert_eq!(l.profile_version, profile.version);
                assert!(l.citation.contains("http"), "{} {}", l.scheme_id, l.key);
                assert!(!l.source_version.is_empty(), "{} {}", l.scheme_id, l.key);
                assert!(
                    parse_date(&l.checked).is_some(),
                    "{} {}",
                    l.scheme_id,
                    l.key
                );
                assert!(
                    l.value > 0.0 && l.value.is_finite(),
                    "{} {}",
                    l.scheme_id,
                    l.key
                );
                assert!(!l.unit.is_empty() && !l.key.is_empty());
            }
            total += all.len();
        }
        // 9 + 18 (NRC) + 257 (IAEA) + 8 + 284 (UK) + the Fetter lower and upper values.
        assert!(total > 600, "{total}");
        let nb = limits(profile("us-nrc"))
            .into_iter()
            .find(|l| l.key == "Table 1: Nb-94 in activated metal")
            .unwrap();
        assert_eq!(nb.value, 0.2);
        assert_eq!(nb.unit, "Ci/m3");
        assert_eq!(nb.checked, "2026-10-15");
        let llw = limits(profile("uk"))
            .into_iter()
            .find(|l| l.key == "LLW beta/gamma")
            .unwrap();
        assert_eq!((llw.value, llw.unit.as_str()), (12.0, "GBq/te"));
        assert_eq!(llw.checked, "2026-10-16");
    }

    // Verifies: SAFE-012
    #[test]
    fn a_profile_is_current_through_its_review_date_and_blocks_after_it() {
        let p = profile("us-nrc");
        assert_eq!(p.review_by, "2027-01-16");
        assert_eq!(status_on(p, "2026-10-16"), ProfileStatus::Current);
        assert_eq!(status_on(p, "2027-01-16"), ProfileStatus::Current);
        assert!(check_current(p, "2027-01-16").is_ok());
        assert_eq!(status_on(p, "2027-01-17"), ProfileStatus::PastReviewDate);
        let e = check_current(p, "2027-01-17").unwrap_err();
        assert!(e.reason.contains("had to be reviewed by 2027-01-16"));
        assert!(e.next_step.contains("jurisdiction_profiles.json"));
        // A date that cannot be read is not taken as current.
        for bad in ["", "2027-1-17", "17/01/2027", "2027-13-01", "2027-01-17x"] {
            assert_eq!(status_on(p, bad), ProfileStatus::DateUnreadable, "{bad}");
            assert!(check_current(p, bad).is_err());
        }
        // A year-rollover and month compare as dates, not as text lengths.
        assert_eq!(status_on(p, "2026-12-31"), ProfileStatus::Current);
        assert_eq!(status_on(p, "2028-01-01"), ProfileStatus::PastReviewDate);
    }

    // Verifies: SAFE-012
    #[test]
    fn a_stamp_names_profile_version_dates_and_status() {
        let s = stamp(profile("uk"), "2027-05-01");
        assert_eq!((s.id.as_str(), s.version.as_str()), ("uk", "1"));
        assert_eq!(s.status, ProfileStatus::PastReviewDate);
        assert_eq!(s.review_by, "2027-04-16");
        assert_eq!(s.evaluated_on, "2027-05-01");
        assert!(profile_for_scheme("uk-llw-ilw").is_some());
        assert!(profile_for_scheme("no-such-scheme").is_none());
        assert_eq!(
            profile_for_scheme("fetter-specific-activity").unwrap().id,
            "fetter-literature"
        );
    }
}
