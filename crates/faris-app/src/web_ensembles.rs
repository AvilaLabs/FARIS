//! `--write-web-ensembles`: calculate the history uncertainty ensembles of the
//! loaded study on this machine and write them for the browser version, which
//! cannot run them itself. Development tool; the file is read back by key.

use crate::{
    Session, browser, build_session,
    native::{Arguments, session_inputs_from},
};
use faris_engine::{
    history::{TransportDrivingRates, run_operating_history_cancellable},
    history_ensemble::{EnsembleSettings, run_history_ensemble},
    history_uncertainty::{EnsembleKey, SAMPLE_CHOICES},
    jobs::Cancellation,
};
use std::path::Path;

pub(crate) fn write(
    args: &Arguments,
    runs_directory: &Path,
    out: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let Session {
        manifest,
        control,
        transport,
        history,
    } = build_session(session_inputs_from(args, runs_directory))?;
    let assumptions = history
        .assumptions
        .clone()
        .ok_or("the study has no operating assumptions to calculate ensembles for")?;
    let samples = SAMPLE_CHOICES[0];
    let mut arrangements = vec![(&manifest, &transport)];
    if let Some((control_manifest, control_panel)) = &control {
        arrangements.push((control_manifest, control_panel));
    }
    let mut ensembles = Vec::new();
    for (manifest, panel) in arrangements {
        for record in panel.records().values() {
            let Some(normalized) = &record.normalized else {
                continue;
            };
            let raw = record
                .raw_artifact_sha256
                .as_deref()
                .ok_or("a transport record has no raw identity")?;
            let rates =
                TransportDrivingRates::from_normalized(normalized, manifest.fusion_power_mw, raw)?;
            let result =
                run_operating_history_cancellable(&assumptions, &rates, &Cancellation::default())?;
            let key = EnsembleKey::new(&result.driving_rates, &result.assumptions, samples)?;
            let started = std::time::Instant::now();
            let ensemble = run_history_ensemble(
                &result.driving_rates,
                &result.assumptions,
                &EnsembleSettings {
                    samples,
                    seed: Some(key.seed),
                    threads: Some(2),
                },
                &Cancellation::default(),
                &|_, _| {},
            )?;
            eprintln!(
                "{} · {}: {samples} samples in {:.1} s",
                record.scenario_sha256,
                record.variant_id,
                started.elapsed().as_secs_f64()
            );
            ensembles.push(browser::PrecomputedEnsemble {
                scenario_sha256: record.scenario_sha256.clone(),
                variant: record.variant_id.clone(),
                key,
                ensemble,
            });
        }
    }
    let file = browser::PrecomputedEnsembles {
        schema: browser::ENSEMBLES_SCHEMA.into(),
        ensembles,
    };
    std::fs::write(out, serde_json::to_vec(&file)?)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}
