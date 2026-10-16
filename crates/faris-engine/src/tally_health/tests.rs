use super::*;
use crate::transport::{RawEnergySpectrum, TallyEstimator};
use faris_model::tally_statistics::{
    TallyInterval, batch_mean_covariance, mean_and_standard_error,
};

/// Deterministic normal batches (xorshift64* with Box-Muller).
fn normal_batches(seed: u64, n: usize, mean: f64, sd: f64) -> Vec<f64> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut uniform = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let x = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            let (u, v) = (uniform(), uniform());
            mean + sd
                * (-2.0 * faris_model::math::ln(u)).sqrt()
                * faris_model::math::cos(2.0 * std::f64::consts::PI * v)
        })
        .collect()
}

fn file_with(columns: &[(&str, Vec<f64>)]) -> BatchValuesFile {
    let n = columns[0].1.len();
    let values: serde_json::Map<String, serde_json::Value> = columns
        .iter()
        .map(|(id, v)| (id.to_string(), serde_json::json!(v)))
        .collect();
    let ids: Vec<&str> = columns.iter().map(|c| c.0).collect();
    let doc = serde_json::json!({
        "schema_version": BATCH_VALUES_SCHEMA,
        "method": "batch-means-sample-covariance/v1",
        "n_batches": n,
        "response_ids": ids,
        "unit": "raw tally units per source neutron, per batch",
        "values": values,
    });
    BatchValuesFile::from_bytes(doc.to_string().as_bytes()).unwrap()
}

#[test]
fn the_worker_file_parses_and_bad_files_say_why() {
    let file = file_with(&[("a", vec![1.0; 12]), ("b", vec![2.0; 12])]);
    assert_eq!(file.n_batches, 12);
    assert!(file.batch_end_seconds.is_none());
    let bad = |doc: serde_json::Value| {
        BatchValuesFile::from_bytes(doc.to_string().as_bytes()).unwrap_err()
    };
    let base = || {
        serde_json::json!({
            "schema_version": BATCH_VALUES_SCHEMA, "n_batches": 3,
            "response_ids": ["a"], "values": {"a": [1.0, 2.0, 3.0]},
        })
    };
    let mut doc = base();
    doc["schema_version"] = "other/v9".into();
    assert!(bad(doc).contains("rerun the transport"));
    let mut doc = base();
    doc["values"]["a"] = serde_json::json!([1.0, 2.0]);
    assert!(bad(doc).contains("2 batch values"));
    let mut doc = base();
    doc["response_ids"] = serde_json::json!(["a", "ghost"]);
    assert!(bad(doc).contains("ghost"));
    let mut doc = base();
    doc["batch_end_seconds"] = serde_json::json!([1.0]);
    assert!(bad(doc).contains("batch_end_seconds"));
    let mut doc = base();
    doc["n_batches"] = 0.into();
    assert!(bad(doc).contains("outside"));
    assert!(BatchValuesFile::from_bytes(b"{").is_err());
}

#[test]
fn a_good_tally_gets_its_interval_with_convention_digits_and_exclusions() {
    let values = normal_batches(1, 200, 1.234_567, 0.3);
    let (mean, se) = mean_and_standard_error(&values).unwrap();
    let file = file_with(&[("flux", values)]);
    let reported = BTreeMap::from([("flux".to_string(), se)]);
    let reports = assess_batch_values(&file, &reported, TallyKind::Volume);
    let r = &reports[0];
    assert!(r.allows_verdict, "{:#?}", r.health.checks);
    assert!(matches!(r.interval, TallyInterval::TwoSided { .. }));
    // The mean reported is the mean of the batch values, nothing altered on the way.
    assert_eq!(r.health.mean, mean);
    assert_eq!(r.health.standard_error, se);
    assert!(
        r.value_text.ends_with("(1 σ standard error)"),
        "{}",
        r.value_text
    );
    assert!(
        r.interval_text.contains("95 % confidence interval"),
        "{}",
        r.interval_text
    );
    assert!(
        r.interval_text
            .contains("excludes: nuclear data, geometry, model form")
    );
    assert!(r.reported_error_check.as_ref().unwrap().agrees);
    // The digits stop at the uncertainty: no more decimals than the error supports.
    let shown = r.value_text.split(" (").next().unwrap();
    let decimals = shown
        .split(" ± ")
        .next()
        .unwrap()
        .split('.')
        .nth(1)
        .map_or(0, str::len);
    assert!(decimals <= 3, "{shown}");
}

// Verifies: UNC-005
#[test]
fn a_failing_tally_is_refused_an_interval_and_a_verdict() {
    let mut values = normal_batches(2, 100, 1.0, 0.2);
    values[10] = 40.0;
    let file = file_with(&[("spiky", values)]);
    let reports = assess_batch_values(&file, &BTreeMap::new(), TallyKind::Volume);
    let r = &reports[0];
    assert!(!r.allows_verdict);
    assert_eq!(r.health.status, TallyStatus::Failing);
    match &r.interval {
        TallyInterval::NotEvaluated { why, next_step } => {
            assert!(why.contains("interval refused"), "{why}");
            assert!(next_step.contains("more histories"));
        }
        other => panic!("{other:?}"),
    }
    assert!(
        r.interval_text.starts_with("NOT_EVALUATED"),
        "{}",
        r.interval_text
    );
    let lines = check_lines(r);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("check 5") && l.contains("FAIL")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("check 10") && l.contains("NOT_EVALUATED"))
    );
}

// Verifies: UNC-008
#[test]
fn a_reported_error_that_disagrees_with_the_batches_refuses_the_interval() {
    let values = normal_batches(3, 200, 1.0, 0.3);
    let (_, se) = mean_and_standard_error(&values).unwrap();
    let file = file_with(&[("flux", values)]);
    let honest = BTreeMap::from([("flux".to_string(), se)]);
    assert!(assess_batch_values(&file, &honest, TallyKind::Volume)[0].allows_verdict);
    let understated = BTreeMap::from([("flux".to_string(), 0.4 * se)]);
    let r = &assess_batch_values(&file, &understated, TallyKind::Volume)[0];
    assert!(!r.allows_verdict);
    assert!(!r.reported_error_check.as_ref().unwrap().agrees);
    assert!(r.interval_text.contains("disagree"), "{}", r.interval_text);
}

// Verifies: UNC-013
#[test]
fn the_sum_of_parts_uses_the_recorded_covariance_and_checks_the_total() {
    // Three zones and a global tally that is their sum, all from the same batches.
    let z1 = normal_batches(10, 120, 6.0, 1.0);
    let z2 = normal_batches(11, 120, 3.0, 1.0);
    let z3 = normal_batches(12, 120, 1.0, 1.0);
    let total: Vec<f64> = (0..120).map(|i| z1[i] + z2[i] + z3[i]).collect();
    let columns: [&[f64]; 4] = [&z1, &z2, &z3, &total];
    let c = batch_mean_covariance(&columns).unwrap();
    let cov = ResponseCovariance {
        method: "batch-means-sample-covariance/v1".into(),
        batches: 120,
        response_ids: vec!["z1".into(), "z2".into(), "z3".into(), "all".into()],
        integrated: c,
    };
    let means: BTreeMap<&str, f64> = ["z1", "z2", "z3", "all"]
        .into_iter()
        .zip(columns.iter().map(|col| col.iter().sum::<f64>() / 120.0))
        .collect();
    let lookup = |id: &str| means.get(id).copied();
    let sum = sum_with_covariance(&cov, &lookup, &["z1", "z2", "z3"], Some("all")).unwrap();
    let (_, direct) = mean_and_standard_error(&total).unwrap();
    assert!((sum.standard_error - direct).abs() < 1e-12);
    assert!((sum.value - means["all"]).abs() < 1e-12);
    let check = sum.against_total.unwrap();
    assert!(check.within_3_sigma && check.difference.abs() < 1e-9);
    // z1 holds 60 % of the sum with a relative error near 1 %; z3 is below 10 % and unconstrained.
    assert_eq!(sum.parts.len(), 3);
    assert!(sum.parts[0].share_of_sum > 0.5);
    // z2 holds 30 % of the sum at about 3 % relative error, so not every zone above 5 % is within 2 %.
    assert!(sum.parts[0].relative_error < 0.02 && sum.parts[1].relative_error > 0.02);
    assert!(!sum.large_zones_within_2_percent);
    // A tally that differs from the sum by many sigma fails the sum check.
    let mut off = means.clone();
    off.insert("all", means["all"] + 20.0 * direct);
    let lookup_off = |id: &str| off.get(id).copied();
    let bad = sum_with_covariance(&cov, &lookup_off, &["z1", "z2", "z3"], Some("all")).unwrap();
    assert!(!bad.against_total.unwrap().within_3_sigma);
    // A part missing from the covariance refuses the sum; independence is not assumed.
    let missing = sum_with_covariance(&cov, &lookup, &["z1", "ghost"], None).unwrap_err();
    assert!(
        missing.contains("not in the recorded covariance"),
        "{missing}"
    );
    assert!(sum_with_covariance(&cov, &lookup, &[], None).is_err());
}

// Verifies: NUC-049
#[test]
fn a_spectrum_is_summarised_by_its_converged_bins() {
    let spectrum = RawEnergySpectrum {
        component_id: "blanket".into(),
        particle: "neutron".into(),
        group_structure: None,
        estimator: TallyEstimator::Tracklength,
        unit: "cm_per_source_per_energy_bin".into(),
        energy_edges_ev: (0..=6).map(|i| i as f64).collect(),
        // Relative errors: 2 %, 4 %, 8 %, 20 %, no score, no error recorded.
        mean_cm_per_source_per_bin: vec![1.0, 1.0, 1.0, 1.0, 0.0, 1.0],
        standard_error_cm_per_source_per_bin: vec![0.02, 0.04, 0.08, 0.2, 0.0, 0.0],
        volume_cm3: 1.0,
        volume_standard_error_cm3: 0.0,
    };
    let c = spectrum_convergence(&spectrum).unwrap();
    assert_eq!((c.voxels, c.scored, c.unscored), (6, 4, 2));
    assert_eq!((c.within_10_percent, c.within_5_percent), (3, 2));
    assert_eq!(c.low_sample, 3);
    let line = convergence_line(&c);
    assert!(
        line.contains("3 of 6 bins (50.0 %)") && line.contains("2 unscored"),
        "{line}"
    );
}

#[test]
fn status_and_basis_texts_name_the_outcome() {
    assert!(status_line(TallyStatus::Failing).starts_with("FAIL"));
    assert!(status_line(TallyStatus::NotEvaluated).starts_with("NOT_EVALUATED"));
    assert!(status_line(TallyStatus::Passing).starts_with("PASS"));
    assert!(fom_basis_text(CostBasis::Batches).contains("not recorded"));
    assert!(fom_basis_text(CostBasis::Seconds).contains("second"));
}
