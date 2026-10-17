//! One entry point for every classification regime, with the profile and the scheme choice
//! recorded on the result and in a receipt (ACT-033, SAFE-011, SAFE-012).
//!
//! The regime is a choice the caller makes (the command flag). `classify_regime` runs the scheme
//! of that regime, names the regime on the class, stamps the jurisdiction profile (name, version,
//! review date, status on the evaluation date) and builds a receipt that records the choice, the
//! scheme and profile versions and the hash of the inventory file. A profile past its review date
//! blocks the verdict: the class is NOT_EVALUATED and no scheme is run.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    NotEvaluated, SchemeInfo, WasteInventory,
    classify::{WasteResult, classify_nrc},
    clearance::{ClearanceIndex, clearance_index},
    fetter::{FetterResult, classify_fetter},
    profile::{self, ProfileStamp, ProfileStatus},
    uk::{UkClearanceResult, UkWasteResult, classify_uk_waste, uk_clearance_index},
};
use crate::RESEARCH_SCREENING_STATEMENT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    UsNrc,
    Iaea,
    UkWaste,
    UkOutOfScope,
    Fetter,
}

impl Regime {
    pub const ALL: [Regime; 5] = [
        Regime::UsNrc,
        Regime::Iaea,
        Regime::UkWaste,
        Regime::UkOutOfScope,
        Regime::Fetter,
    ];

    /// The scheme identifier the regime is named by.
    pub fn scheme_id(self) -> &'static str {
        match self {
            Regime::UsNrc => "nrc-10cfr61.55",
            Regime::Iaea => "iaea-gsr3-table-i2",
            Regime::UkWaste => "uk-llw-ilw",
            Regime::UkOutOfScope => "uk-epr2016-sch23-table2",
            Regime::Fetter => "fetter-specific-activity",
        }
    }

    pub fn from_scheme_id(id: &str) -> Option<Regime> {
        Regime::ALL.into_iter().find(|r| r.scheme_id() == id)
    }
}

/// The choice of regime and how it was made.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SchemeChoice {
    pub selected: String,
    /// How the regime was selected, for example `command flag --scheme`.
    pub selected_by: String,
    pub available: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum RegimeResult {
    Nrc(Box<WasteResult>),
    Iaea(Box<ClearanceIndex>),
    UkWaste(Box<UkWasteResult>),
    UkOutOfScope(Box<UkClearanceResult>),
    Fetter(Box<FetterResult>),
}

/// The body of a receipt: what was asked and under which versions.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ReceiptBody {
    pub schema: String,
    pub kind: String,
    pub component_id: String,
    /// SHA-256 of the inventory file bytes, when the caller read one.
    pub inventory_sha256: Option<String>,
    pub time_years: f64,
    pub evaluated_on: String,
    pub scheme_choice: SchemeChoice,
    pub scheme_name: String,
    pub scheme_version: String,
    pub scheme_date: String,
    pub profile_id: String,
    pub profile_version: String,
    pub profile_review_by: String,
    pub profile_status: ProfileStatus,
    pub class: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ClassificationReceipt {
    #[serde(flatten)]
    pub body: ReceiptBody,
    /// SHA-256 of the body's JSON text.
    pub receipt_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegimeReport {
    /// The regime named on the class.
    pub regime: String,
    pub regime_name: String,
    pub class: String,
    pub profile: ProfileStamp,
    pub scheme_choice: SchemeChoice,
    /// Set when the profile is past its review date: the class is NOT_EVALUATED.
    pub blocked_by_profile: Option<NotEvaluated>,
    pub result: Option<RegimeResult>,
    pub receipt: ClassificationReceipt,
    pub research_screening: String,
}

fn iaea_class(r: &ClearanceIndex) -> String {
    match r.below_one {
        Some(true) => "CLEARANCE_INDEX_BELOW_1",
        Some(false) => "CLEARANCE_INDEX_AT_OR_ABOVE_1",
        None => "NOT_EVALUATED",
    }
    .to_string()
}

/// Runs the regime on an inventory at a disposal time, on the date `today` (`YYYY-MM-DD`).
pub fn classify_regime(
    regime: Regime,
    inventory: &WasteInventory,
    time_years: f64,
    today: &str,
    selected_by: &str,
    inventory_sha256: Option<String>,
) -> RegimeReport {
    let scheme_id = regime.scheme_id();
    let (scheme, _) =
        profile::scheme_record(scheme_id).expect("every regime has an implemented scheme");
    let p = profile::profile_for_scheme(scheme_id).expect("every regime has a profile");
    let stamp = profile::stamp(p, today);
    let choice = SchemeChoice {
        selected: scheme_id.to_string(),
        selected_by: selected_by.to_string(),
        available: Regime::ALL
            .iter()
            .map(|r| r.scheme_id().to_string())
            .collect(),
    };
    let blocked = profile::check_current(p, today).err();
    let (class, result) = if blocked.is_some() {
        ("NOT_EVALUATED".to_string(), None)
    } else {
        match regime {
            Regime::UsNrc => {
                let r = classify_nrc(inventory, time_years);
                (
                    r.verdict.text().to_string(),
                    Some(RegimeResult::Nrc(Box::new(r))),
                )
            }
            Regime::Iaea => {
                let r = clearance_index(inventory, time_years);
                (iaea_class(&r), Some(RegimeResult::Iaea(Box::new(r))))
            }
            Regime::UkWaste => {
                let r = classify_uk_waste(inventory, time_years);
                (
                    r.verdict.text().to_string(),
                    Some(RegimeResult::UkWaste(Box::new(r))),
                )
            }
            Regime::UkOutOfScope => {
                let r = uk_clearance_index(inventory, time_years);
                (
                    r.class.clone(),
                    Some(RegimeResult::UkOutOfScope(Box::new(r))),
                )
            }
            Regime::Fetter => {
                let r = classify_fetter(inventory, time_years);
                (
                    r.verdict.text().to_string(),
                    Some(RegimeResult::Fetter(Box::new(r))),
                )
            }
        }
    };
    let receipt = receipt(
        inventory,
        inventory_sha256,
        time_years,
        &choice,
        &scheme,
        &stamp,
        &class,
    );
    RegimeReport {
        regime: scheme_id.to_string(),
        regime_name: scheme.name.clone(),
        class,
        profile: stamp,
        scheme_choice: choice,
        blocked_by_profile: blocked,
        result,
        receipt,
        research_screening: RESEARCH_SCREENING_STATEMENT.to_string(),
    }
}

fn receipt(
    inventory: &WasteInventory,
    inventory_sha256: Option<String>,
    time_years: f64,
    choice: &SchemeChoice,
    scheme: &SchemeInfo,
    stamp: &ProfileStamp,
    class: &str,
) -> ClassificationReceipt {
    let body = ReceiptBody {
        schema: "faris-waste-receipt/1".to_string(),
        kind: "waste_classification_receipt".to_string(),
        component_id: inventory.component_id.clone(),
        inventory_sha256,
        time_years,
        evaluated_on: stamp.evaluated_on.clone(),
        scheme_choice: choice.clone(),
        scheme_name: scheme.name.clone(),
        scheme_version: scheme.version.clone(),
        scheme_date: scheme.date.clone(),
        profile_id: stamp.id.clone(),
        profile_version: stamp.version.clone(),
        profile_review_by: stamp.review_by.clone(),
        profile_status: stamp.status,
        class: class.to_string(),
    };
    let text = serde_json::to_vec(&body).expect("a receipt body serialises");
    ClassificationReceipt {
        receipt_sha256: format!("{:x}", Sha256::digest(&text)),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waste::{CURIE_BQ, NuclideActivity, WasteForm, YEAR_S, classify::required_nuclides};

    fn nuclide(name: &str, bq: f64, alpha: Option<bool>) -> NuclideActivity {
        NuclideActivity {
            nuclide: name.to_string(),
            activity_bq: bq,
            half_life_s: 1.0e6 * YEAR_S,
            alpha_emitter: alpha,
            progeny_of: None,
        }
    }

    /// An activated-metal inventory with every nuclide 10 CFR 61.55 needs at zero, then
    /// Nb-94 at 0.05 Ci in 1 m3 and 1e6 g (Table 1: 0.25 of 0.2 Ci/m3).
    fn inventory() -> WasteInventory {
        let mut nuclides: Vec<NuclideActivity> = required_nuclides(WasteForm::ActivatedMetal)
            .into_iter()
            .map(|n| nuclide(&n, 0.0, Some(false)))
            .collect();
        nuclides
            .iter_mut()
            .find(|n| n.nuclide == "Nb-94")
            .unwrap()
            .activity_bq = 0.05 * CURIE_BQ;
        WasteInventory {
            component_id: "regime-test".to_string(),
            waste_form: WasteForm::ActivatedMetal,
            volume_m3: Some(1.0),
            mass_g: Some(1.0e6),
            inventory_time_years: 0.0,
            transuranics_evaluated: true,
            inventory_kind: "authored_test".to_string(),
            nuclides,
        }
    }

    // Verifies: ACT-033, SAFE-011
    #[test]
    fn every_regime_is_named_on_the_class_and_carries_the_profile_name_and_version() {
        let inv = inventory();
        for regime in Regime::ALL {
            let r = classify_regime(regime, &inv, 0.0, "2026-10-16", "test", None);
            assert_eq!(r.regime, regime.scheme_id());
            assert_eq!(Regime::from_scheme_id(&r.regime), Some(regime));
            assert!(!r.regime_name.is_empty());
            assert!(!r.profile.name.is_empty() && !r.profile.version.is_empty());
            assert_eq!(r.profile.status, ProfileStatus::Current);
            assert_eq!(r.scheme_choice.selected, regime.scheme_id());
            assert_eq!(r.scheme_choice.available.len(), 5);
            assert!(r.result.is_some() && r.blocked_by_profile.is_none());
            let json = serde_json::to_value(&r).unwrap();
            assert_eq!(json["regime"], regime.scheme_id());
            assert_eq!(json["profile"]["version"], "1");
            assert_eq!(json["result"]["scheme"]["id"], regime.scheme_id());
            assert_eq!(
                json["receipt"]["scheme_choice"]["selected"],
                regime.scheme_id()
            );
        }
        // The NRC class is the one the earlier command gave.
        let nrc = classify_regime(Regime::UsNrc, &inv, 0.0, "2026-10-16", "test", None);
        assert_eq!(nrc.class, "CLASS_C");
        assert_eq!(nrc.profile.id, "us-nrc");
    }

    // Verifies: SAFE-011
    #[test]
    fn the_regimes_give_different_hand_computed_classes_for_one_inventory() {
        // Nb-94 at 0.05 Ci/m3: 0.25 of 61.55 Table 1 (0.2 Ci/m3): Class C.
        // Fetter: 0.05/0.2 = 0.25, sum at most 1.
        // IAEA: 0.05 Ci = 1.85e9 Bq over 1e6 g = 1850 Bq/g over 0.1 = 18500: above 1.
        // UK LLW: 1850 Bq/g = 1.85 GBq/te beta/gamma with the flag false: LLW (under 12).
        // UK Table 2: 1850 over 0.1: above.
        let inv = inventory();
        let class = |r| classify_regime(r, &inv, 0.0, "2026-10-16", "test", None).class;
        assert_eq!(class(Regime::UsNrc), "CLASS_C");
        assert_eq!(class(Regime::Fetter), "SUM_AT_OR_BELOW_1");
        assert_eq!(class(Regime::Iaea), "CLEARANCE_INDEX_AT_OR_ABOVE_1");
        assert_eq!(class(Regime::UkWaste), "LLW");
        assert_eq!(class(Regime::UkOutOfScope), "ABOVE_TABLE_2");
    }

    // Verifies: SAFE-012
    #[test]
    fn a_profile_past_its_review_date_blocks_the_verdict_for_every_regime() {
        let inv = inventory();
        for regime in Regime::ALL {
            let r = classify_regime(regime, &inv, 0.0, "2030-01-01", "test", None);
            assert_eq!(r.class, "NOT_EVALUATED", "{:?}", regime);
            assert!(r.result.is_none());
            let blocked = r.blocked_by_profile.as_ref().unwrap();
            assert!(blocked.reason.contains("had to be reviewed by"));
            assert_eq!(r.profile.status, ProfileStatus::PastReviewDate);
            assert_eq!(r.receipt.body.class, "NOT_EVALUATED");
        }
        // The day after one profile's review date blocks that profile and not the others.
        let us = classify_regime(Regime::UsNrc, &inv, 0.0, "2027-01-17", "test", None);
        let iaea = classify_regime(Regime::Iaea, &inv, 0.0, "2027-01-17", "test", None);
        assert_eq!(us.class, "NOT_EVALUATED");
        assert_ne!(iaea.class, "NOT_EVALUATED");
        // An unreadable date also blocks.
        let r = classify_regime(Regime::UsNrc, &inv, 0.0, "yesterday", "test", None);
        assert_eq!(r.class, "NOT_EVALUATED");
        assert_eq!(r.profile.status, ProfileStatus::DateUnreadable);
    }

    // Verifies: ACT-033
    #[test]
    fn the_receipt_records_the_scheme_choice_versions_and_a_hash_that_changes_with_the_choice() {
        let inv = inventory();
        let a = classify_regime(
            Regime::UkWaste,
            &inv,
            0.0,
            "2026-10-16",
            "command flag --scheme",
            Some("ab".repeat(32)),
        );
        let b = classify_regime(
            Regime::Fetter,
            &inv,
            0.0,
            "2026-10-16",
            "command flag --scheme",
            Some("ab".repeat(32)),
        );
        let r = &a.receipt;
        assert_eq!(r.body.kind, "waste_classification_receipt");
        assert_eq!(r.body.scheme_choice.selected, "uk-llw-ilw");
        assert_eq!(r.body.scheme_choice.selected_by, "command flag --scheme");
        assert_eq!(
            r.body.inventory_sha256.as_deref(),
            Some("ab".repeat(32).as_str())
        );
        assert_eq!(r.body.profile_id, "uk");
        assert_eq!(r.body.profile_version, "1");
        assert_eq!(r.body.class, a.class);
        assert_eq!(r.receipt_sha256.len(), 64);
        assert_ne!(r.receipt_sha256, b.receipt.receipt_sha256);
        // The hash is the hash of the body text, reproducible.
        let again = classify_regime(
            Regime::UkWaste,
            &inv,
            0.0,
            "2026-10-16",
            "command flag --scheme",
            Some("ab".repeat(32)),
        );
        assert_eq!(again.receipt.receipt_sha256, r.receipt_sha256);
        let text = serde_json::to_vec(&r.body).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&text)), r.receipt_sha256);
    }
}
