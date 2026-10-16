use super::*;
use faris_model::math;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// The edges of ACTINV's fispact-709 structure, read from the activation library's bounds.
const EDGES: &str = include_str!("../../tests/fixtures/activation/fispact709_edges.json");
// Recorded ACTINV 1.4.0 results for `fixture_request` and its zero-flux twin; the bulky
// ledger entries and the photon line list FARIS does not read were removed, nothing else.
const RECORDED: &str = include_str!("../../tests/fixtures/activation/co59_result.json");
const RECORDED_ZERO: &str = include_str!("../../tests/fixtures/activation/co59_zero_result.json");

fn edges() -> Vec<f64> {
    serde_json::from_str(EDGES).unwrap()
}

fn flat(total: f64) -> Vec<f64> {
    vec![total / GROUPS as f64; GROUPS]
}

fn zone(
    id: &str,
    mass_g: f64,
    volume_m3: f64,
    composition: &[(&str, f64)],
    flux: Vec<f64>,
) -> ZoneInput {
    ZoneInput {
        id: id.into(),
        mass_g,
        volume_m3,
        basis: CompositionBasis::WtPercent,
        composition: composition
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect(),
        spectrum: GroupFlux {
            energy_edges_ev: edges(),
            flux_per_cm2_s: flux,
        },
    }
}

fn fixture_request(flux_total: f64) -> ActivationRequest {
    ActivationRequest {
        zones: vec![zone(
            "co-a",
            1000.0,
            0.002,
            &[("Co", 100.0)],
            flat(flux_total),
        )],
        steps: vec![
            IrradiationStep {
                duration_s: 43_200.0,
                flux_scale: 1.0,
            },
            IrradiationStep {
                duration_s: 43_200.0,
                flux_scale: 0.5,
            },
        ],
        schedule_input_sha256: None,
        cooling_s: vec![1.0, 3600.0, 86_400.0, 31_557_600.0],
        photon: PhotonOptions::default(),
        mode: SolverMode::Auto,
    }
}

/// A data folder of small stand-ins, so hashes can be pinned without the ACTINV data.
struct FakeData {
    _dir: tempfile::TempDir,
    data: ActivationData,
}

fn fake_data() -> FakeData {
    let dir = tempfile::tempdir().unwrap();
    let mut data = ActivationData::catalog(dir.path());
    for (i, p) in [&data.library, &data.decay_primary].into_iter().enumerate() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("stand-in {i}")).unwrap();
    }
    let fb = data.decay_fallback.as_ref().unwrap().0.clone();
    std::fs::write(&fb, "stand-in 2").unwrap();
    data.library_id = "stand-in-library".into();
    FakeData { _dir: dir, data }
}

/// The recorded result with its certificate pointing at the stand-in decay files.
fn recorded(text: &str, data: &ActivationData) -> Value {
    let mut v: Value = serde_json::from_str(text).unwrap();
    v["certificate"]["inputs"]["decay_primary"]["sha256"] =
        json!(hash_file(&data.decay_primary).unwrap());
    v["certificate"]["inputs"]["decay_fallback"]["sha256"] =
        json!(hash_file(&data.decay_fallback.as_ref().unwrap().0).unwrap());
    v
}

#[cfg(unix)]
/// A stand-in `actinv`: counts its calls and copies a recorded result to the output path.
fn fake_actinv(dir: &Path, result: &Value) -> PathBuf {
    let canned = dir.join("canned.json");
    std::fs::write(&canned, serde_json::to_vec(result).unwrap()).unwrap();
    let script = dir.join("actinv");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho x >> '{}/calls'\ncp '{}' \"$3\"\n",
            dir.display(),
            canned.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
fn calls(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("calls")).map_or(0, |t| t.lines().count())
}

#[cfg(unix)]
struct Rig {
    _fake: FakeData,
    scratch: tempfile::TempDir,
    config: ActivationConfig,
}

#[cfg(unix)]
fn rig(result_text: &str) -> Rig {
    let fake = fake_data();
    let scratch = tempfile::tempdir().unwrap();
    let result = recorded(result_text, &fake.data);
    let actinv = fake_actinv(scratch.path(), &result);
    let mut config = ActivationConfig::new(actinv, fake.data.clone(), scratch.path().join("work"));
    config.timeout = Duration::from_secs(60);
    Rig {
        _fake: fake,
        scratch,
        config,
    }
}

// -------------------------------------------------------------- the hand-off --

// Verifies: ACT-002
#[test]
fn group_flux_and_structure_reach_the_problem_unchanged() {
    // Five different spectra: flat, one spike, a falling power law, tiny values, huge values.
    let n = GROUPS;
    let spectra: Vec<Vec<f64>> = vec![
        flat(1.0e14),
        (0..n)
            .map(|g| if g == 400 { 3.0e13 } else { 0.0 })
            .collect(),
        (0..n)
            .map(|g| 1.0e12 * math::exp(-(g as f64) / 90.0))
            .collect(),
        (0..n).map(|g| 1.0e-30 * (1.0 + g as f64)).collect(),
        (0..n).map(|g| 1.0e22 / (1.0 + (g % 7) as f64)).collect(),
    ];
    // An arbitrary per-group cross section, to compare reaction-rate integrals.
    let sigma: Vec<f64> = (0..n)
        .map(|g| 1.0e-24 * (1.0 + math::sin(g as f64 * 0.37).abs()))
        .collect();
    for (i, flux) in spectra.into_iter().enumerate() {
        let request = ActivationRequest {
            zones: vec![zone("z", 10.0, 1.0, &[("Fe", 100.0)], flux.clone())],
            ..fixture_request(1.0)
        };
        let data = fake_data();
        let problem = build_problem(&request.zones[0], &request, &data.data, "ab");
        let sent: Vec<f64> =
            serde_json::from_value(problem["spectrum"]["flux_per_group"].clone()).unwrap();
        let sent_edges: Vec<f64> =
            serde_json::from_value(problem["spectrum"]["boundaries_eV"].clone()).unwrap();
        assert_eq!(
            sent, flux,
            "spectrum {i}: every group flux is passed bit for bit"
        );
        assert_eq!(
            sent_edges,
            edges(),
            "spectrum {i}: the group structure is stored with the flux"
        );
        let want: f64 = flux.iter().sum();
        let got: f64 = sent.iter().sum();
        assert!(
            relative_difference(want, got) <= 1.0e-9,
            "spectrum {i}: flux integral"
        );
        let rate = |f: &[f64]| f.iter().zip(&sigma).map(|(a, b)| a * b).sum::<f64>();
        assert!(
            relative_difference(rate(&flux), rate(&sent)) <= 1.0e-6,
            "spectrum {i}: reaction rate"
        );
    }
}

// Verifies: ACT-002
#[test]
fn normalized_spectrum_converts_per_m2_to_per_cm2_and_nothing_else() {
    let m2: Vec<f64> = (0..GROUPS).map(|g| 1.0e16 * (1.0 + g as f64)).collect();
    let spectrum = NormalizedEnergySpectrum {
        component_id: "blanket".into(),
        particle: "neutron".into(),
        estimator: crate::transport::TallyEstimator::Tracklength,
        energy_edges_ev: edges(),
        mean_per_square_metre_second: m2.clone(),
        standard_error_per_square_metre_second: vec![0.0; GROUPS],
        normalization_volume_m3: 1.0,
        volume_standard_error_m3: 0.0,
    };
    let flux = GroupFlux::from_normalized(&spectrum).unwrap();
    assert_eq!(flux.energy_edges_ev, edges());
    for (a, b) in m2.iter().zip(&flux.flux_per_cm2_s) {
        assert!(relative_difference(a * 1.0e-4, *b) <= 4.0e-16);
    }
    assert!(relative_difference(m2.iter().sum::<f64>() * 1.0e-4, flux.total()) <= 1.0e-12);
    let mut photon = spectrum;
    photon.particle = "photon".into();
    assert!(
        GroupFlux::from_normalized(&photon)
            .unwrap_err()
            .contains("neutron")
    );
}

#[test]
fn problem_carries_the_schedule_and_cooling_grid_exactly() {
    let request = fixture_request(1.0e8);
    let data = fake_data();
    let problem = build_problem(&request.zones[0], &request, &data.data, "ab");
    let schedule = problem["schedule"].as_array().unwrap();
    assert_eq!(schedule.len(), 2 + 4);
    assert_eq!(schedule[0]["dt"], "43200 s");
    assert_eq!(schedule[0]["flux"], 1.0);
    assert_eq!(schedule[1]["flux"], 0.5);
    // Cooling steps are zero-flux and their durations add up to each cooling time.
    let mut elapsed = 0.0;
    for (j, t) in request.cooling_s.iter().enumerate() {
        let step = &schedule[2 + j];
        assert_eq!(step["flux"], 0.0);
        let dt: f64 = step["dt"]
            .as_str()
            .unwrap()
            .trim_end_matches(" s")
            .parse()
            .unwrap();
        elapsed += dt;
        assert!(
            relative_difference(elapsed, *t) <= 1.0e-12,
            "cooling time {t}"
        );
    }
    assert_eq!(problem["material"]["mass_g"], 1000.0);
    assert_eq!(problem["material"]["composition"]["Co"], 100.0);
    assert_eq!(problem["library"]["sha256"], "ab");
}

// Verifies: ACT-019
#[test]
fn cooling_grid_covers_one_second_to_1e9_and_refuses_outside() {
    let mut request = fixture_request(1.0e8);
    request.cooling_s = decade_cooling_grid();
    assert_eq!(request.cooling_s.first(), Some(&1.0));
    assert_eq!(request.cooling_s.last(), Some(&1.0e9));
    request.validate().unwrap();
    for bad in [
        vec![0.5],
        vec![2.0e9],
        vec![10.0, 10.0],
        vec![100.0, 10.0],
        vec![],
    ] {
        request.cooling_s = bad;
        assert!(request.validate().is_err());
    }
    let parsed = parse_cooling_list("1s,1min,1h,1d,1w,30d,1y,1e9s").unwrap();
    assert_eq!(parsed[..3], [1.0, 60.0, 3600.0]);
    assert_eq!(parsed[6], YEAR_S);
    assert_eq!(parsed[7], 1.0e9);
    assert!(parse_cooling_list("5").unwrap_err().contains("unit"));
    assert!(parse_cooling_list("5 fortnights").is_err());
}

#[test]
fn request_checks_refuse_with_a_reason() {
    let base = fixture_request(1.0e8);
    let mut r = base.clone();
    r.zones[0].mass_g = 0.0;
    assert!(r.validate().unwrap_err().contains("mass_g"));
    let mut r = base.clone();
    r.zones[0].volume_m3 = -1.0;
    assert!(r.validate().unwrap_err().contains("volume_m3"));
    let mut r = base.clone();
    r.zones[0].spectrum.flux_per_cm2_s.pop();
    assert!(r.validate().unwrap_err().contains("709"));
    let mut r = base.clone();
    r.zones[0].spectrum.flux_per_cm2_s[3] = -1.0;
    assert!(r.validate().unwrap_err().contains("negative"));
    let mut r = base.clone();
    r.zones.push(r.zones[0].clone());
    assert!(r.validate().unwrap_err().contains("repeated"));
    let mut r = base.clone();
    r.steps = vec![
        IrradiationStep {
            duration_s: 1.0,
            flux_scale: 1.0
        };
        MAX_SCHEDULE_STEPS + 1
    ];
    assert!(r.validate().unwrap_err().contains("condense"));
    let mut r = base;
    r.steps.clear();
    assert!(r.validate().is_err());
}

// ---------------------------------------------------- recorded ACTINV results --

// Verifies: ACT-001, ACT-020, ACT-021, ACT-022, ACT-023, ACT-045, ACT-047, ACT-049
#[test]
#[cfg(unix)]
fn recorded_run_gives_typed_inventory_heat_activity_photons_and_versions() {
    let rig = rig(RECORDED);
    let request = fixture_request(1.0e8);
    let (result, stats) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    assert_eq!(
        stats,
        RunStats {
            actinv_runs: 1,
            cache_hits: 0
        }
    );
    assert_eq!(result.schema, RESULT_SCHEMA);
    assert_eq!(
        result.research_screening,
        faris_model::RESEARCH_SCREENING_STATEMENT
    );
    assert!(result.label.starts_with("research screening"));
    // Versions: solver from the certificate, data hashes from the files pinned.
    assert_eq!(result.data.solver, "actinv-core 1.4.0");
    assert_eq!(
        result.data.library.sha256,
        hash_file(&rig.config.data.library).unwrap()
    );
    assert_eq!(
        result.data.decay_primary.sha256,
        hash_file(&rig.config.data.decay_primary).unwrap()
    );
    assert!(result.data.decay_fallback.is_some());
    assert_eq!(result.cooling_s, request.cooling_s);

    let zone = &result.zones[0];
    assert_eq!(zone.points.len(), 4);
    let raw = recorded(RECORDED, &rig.config.data);
    for (j, p) in zone.points.iter().enumerate() {
        let step = &raw["steps"][2 + j];
        let heat = step["heat_W_per_g"]["total"].as_f64().unwrap();
        // Units: ACTINV is W per g; the zone is 1000 g in 0.002 m3.
        assert!(relative_difference(p.decay_heat.total_w_per_m3, heat * 1000.0 / 0.002) <= 1e-14);
        assert!(relative_difference(p.decay_heat.total_kw, heat * 1000.0 / 1000.0) <= 1e-14);
        // The radiation-type parts sum to the total (ACT-021).
        let d = &p.decay_heat;
        let parts = d.alpha_w_per_m3 + d.beta_w_per_m3 + d.gamma_w_per_m3;
        assert!(relative_difference(d.total_w_per_m3, parts) <= 1.0e-9);
        assert!(d.parts_closure_relative <= 1.0e-9);
        // Activity per nuclide in Bq of the whole zone sums to the point total.
        let sum: f64 = p.nuclides.iter().map(|n| n.activity_bq).sum();
        assert!(relative_difference(sum, p.activity_bq) <= 1.0e-12);
        assert!(relative_difference(p.activity_bq / 0.002, p.activity_bq_per_m3) <= 1.0e-14);
        // Per-nuclide activity is ACTINV's per-gram value times the mass.
        for n in p.nuclides.iter().filter(|n| n.activity_bq > 0.0) {
            let per_g = step["activity_Bq_per_g"][&n.nuclide].as_f64().unwrap();
            assert_eq!(n.activity_bq, per_g * 1000.0);
        }
        // Photon source: groups integrate to the grouped emission, the line total is kept.
        let ps = &p.photon_source;
        let grouped: f64 = ps.groups.iter().map(|g| g.photons_per_s).sum();
        assert!(relative_difference(grouped, ps.grouped_photons_per_s) <= 1.0e-12);
        assert!(relative_difference(ps.total_photons_per_s, ps.grouped_photons_per_s) <= 1.0e-6);
        assert_eq!(p.kind, Kind::Conditional);
    }
    assert!(zone.points[1].activity_bq > 0.0, "cobalt is activated");
    assert!(zone.points[1].nuclides.iter().any(|n| n.nuclide == "Co60"));
    assert!(zone.points[1].top_by_activity.iter().all(|n| n != "Co59"));
    // Totals over one zone equal the zone; kind labels are on every block.
    assert_eq!(
        result.totals[2].decay_heat_kw,
        zone.points[2].decay_heat.total_kw
    );
    assert_eq!(zone.input.kind, Kind::Authored);
    assert_eq!(result.schedule.kind, Kind::Authored);
    // No contact dose without a response: said, with the next step.
    assert!(
        zone.points[0]
            .photon_source
            .contact_dose_proxy_gy_h
            .is_none()
    );
    let dose = zone
        .not_evaluated
        .iter()
        .find(|n| n.quantity.starts_with("contact dose"))
        .unwrap();
    assert!(dose.next_step.contains("photon-response"));
}

// Verifies: ACT-020
#[test]
#[cfg(unix)]
fn zone_totals_equal_the_sum_over_zones() {
    let rig = rig(RECORDED);
    let mut request = fixture_request(1.0e8);
    request.zones = vec![
        zone("co-a", 1000.0, 0.002, &[("Co", 100.0)], flat(1.0e8)),
        zone("co-b", 250.0, 0.0005, &[("Co", 100.0)], flat(1.0e8)),
        zone("co-c", 4000.0, 0.01, &[("Co", 100.0)], flat(1.0e8)),
    ];
    let (result, _) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    for (j, t) in result.totals.iter().enumerate() {
        let kw: f64 = result
            .zones
            .iter()
            .map(|z| z.points[j].decay_heat.total_kw)
            .sum();
        let bq: f64 = result.zones.iter().map(|z| z.points[j].activity_bq).sum();
        assert!(relative_difference(t.decay_heat_kw, kw) <= 1.0e-9);
        assert!(relative_difference(t.activity_bq, bq) <= 1.0e-9);
    }
    // Same material per gram: heat scales with mass, W/m3 with density.
    let (a, b) = (&result.zones[0].points[1], &result.zones[1].points[1]);
    assert!(relative_difference(a.decay_heat.total_kw / 4.0, b.decay_heat.total_kw) <= 1e-12);
    assert!(relative_difference(a.decay_heat.total_w_per_m3, b.decay_heat.total_w_per_m3) <= 1e-12);
}

// Verifies: ACT-042
#[test]
#[cfg(unix)]
fn identical_requests_give_bit_identical_results() {
    let rig = rig(RECORDED);
    let request = fixture_request(1.0e8);
    let first = serde_json::to_vec(
        &run_activation(&request, &rig.config, &Cancellation::default())
            .unwrap()
            .0,
    )
    .unwrap();
    for _ in 0..9 {
        let again = serde_json::to_vec(
            &run_activation(&request, &rig.config, &Cancellation::default())
                .unwrap()
                .0,
        )
        .unwrap();
        assert_eq!(first, again);
    }
    assert_eq!(calls(rig.scratch.path()), 10);
}

// Verifies: ACT-042, ACT-045
#[test]
#[cfg(unix)]
fn any_changed_input_or_data_file_changes_the_receipt() {
    let rig = rig(RECORDED);
    let base = fixture_request(1.0e8);
    let hash = |request: &ActivationRequest, config: &ActivationConfig| {
        run_activation(request, config, &Cancellation::default())
            .unwrap()
            .0
            .input_sha256
    };
    let reference = hash(&base, &rig.config);
    let mut changed = base.clone();
    changed.zones[0].spectrum.flux_per_cm2_s[10] *= 1.0000001;
    assert_ne!(hash(&changed, &rig.config), reference, "spectrum");
    // Changed fluxes are a different problem, but the stand-in still returns the old total:
    // that mismatch is caught by the hand-off check, which is also what we want.
    let mut changed = base.clone();
    changed.steps[1].flux_scale = 0.25;
    assert!(run_activation(&changed, &rig.config, &Cancellation::default()).is_err());
    let mut changed = base.clone();
    changed.zones[0].mass_g = 1001.0;
    assert_ne!(hash(&changed, &rig.config), reference, "mass");
    let mut changed = base.clone();
    changed.zones[0].composition.insert("Fe".into(), 1.0);
    assert_ne!(hash(&changed, &rig.config), reference, "composition");
    let mut changed = base.clone();
    changed.cooling_s[3] += 1.0;
    let sha = |r: &ActivationRequest| {
        hex_sha256(
            &serde_json::to_vec(&build_problem(&r.zones[0], r, &rig.config.data, "ab")).unwrap(),
        )
    };
    assert_ne!(sha(&changed), sha(&base), "cooling grid");
    let mut changed = base.clone();
    changed.mode = SolverMode::Coupled;
    assert_ne!(sha(&changed), sha(&base), "solver mode");
    let mut changed = base.clone();
    changed.photon.group_boundaries_ev = Some(log_photon_groups(120).unwrap());
    assert_ne!(hash(&changed, &rig.config), reference, "photon groups");
    // A changed library file changes the hash in the receipt and the cache key.
    std::fs::write(&rig.config.data.library, "another library").unwrap();
    assert_ne!(hash(&base, &rig.config), reference, "library file");
}

#[test]
#[cfg(unix)]
fn an_identical_request_is_served_from_the_cache_and_a_changed_data_file_is_not() {
    let mut rig = rig(RECORDED);
    rig.config.cache_dir = Some(rig.scratch.path().join("cache"));
    let request = fixture_request(1.0e8);
    let (first, s1) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    let (second, s2) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    assert_eq!((s1.actinv_runs, s1.cache_hits), (1, 0));
    assert_eq!((s2.actinv_runs, s2.cache_hits), (0, 1));
    assert_eq!(first, second);
    assert_eq!(calls(rig.scratch.path()), 1);
    // A corrupt cache entry is ignored, not trusted.
    for entry in std::fs::read_dir(rig.scratch.path().join("cache")).unwrap() {
        std::fs::write(entry.unwrap().path(), "{").unwrap();
    }
    let (third, s3) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    assert_eq!(s3.actinv_runs, 1);
    assert_eq!(first, third);
    // New decay data: the key changes, ACTINV runs again.
    std::fs::write(&rig.config.data.decay_primary, "other decay").unwrap();
    let raw = recorded(RECORDED, &rig.config.data);
    std::fs::write(
        rig.scratch.path().join("canned.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let (_, s4) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    assert_eq!(s4.actinv_runs, 1);
}

// Verifies: ACT-006
#[test]
#[cfg(unix)]
fn nuclides_lacking_data_are_listed_and_make_the_totals_a_lower_bound() {
    let rig = rig(RECORDED);
    let request = fixture_request(1.0e8);
    let (result, _) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    let zone = &result.zones[0];
    let raw = recorded(RECORDED, &rig.config.data);
    let missing = raw["ledger"]["products_no_evaluated_decay_data"]
        .as_object()
        .unwrap();
    let gap = zone
        .gaps
        .iter()
        .find(|g| g.category.contains("products_no_evaluated_decay_data"))
        .expect("the products without decay data are a gap");
    assert_eq!(gap.count, missing.len());
    assert!(gap.lower_bound);
    assert!(gap.amount_atoms_per_g.unwrap() > 0.0);
    assert!(gap.effect.contains("lower bound") && !gap.next_step.is_empty());
    assert!(gap.items[0].starts_with("Z="));
    assert_eq!(zone.completeness, Completeness::LowerBound);
    assert_eq!(result.totals[0].completeness, Completeness::LowerBound);
    assert_eq!(result.totals[0].kind, Kind::Conditional);

    // Remove the gaps from the recorded ledger: the zone is complete and labelled calculated.
    let mut clean = raw.clone();
    for key in [
        "products_no_evaluated_decay_data",
        "targets_absent_from_decay_library",
        "isomer_state_absent_from_decay_library_used_ground",
        "fission_no_yields_to_leakage",
        "library_target_limitations",
    ] {
        clean["ledger"][key] = json!({});
    }
    clean["ledger"]["decay_daughters_missing"] = json!(0);
    clean["ledger"]["spontaneous_fission_branches_to_leakage"] = json!(0);
    for s in clean["steps"].as_array_mut().unwrap() {
        s["leakage_atoms_per_g"] = json!(0.0);
    }
    let data = &rig.config.data;
    let zone2 = zone_from_actinv_result(&request.zones[0], &request, data, "h", &clean).unwrap();
    assert_eq!(zone2.completeness, Completeness::Complete);
    assert!(zone2.gaps.is_empty());
    assert!(zone2.points.iter().all(|p| p.kind == Kind::Calculated));

    // A new missing product is named and amounts are kept, never dropped to zero.
    clean["ledger"]["products_no_evaluated_decay_data"] = json!({"270601_0": 1.5e-3});
    let zone3 = zone_from_actinv_result(&request.zones[0], &request, data, "h", &clean).unwrap();
    assert_eq!(zone3.completeness, Completeness::LowerBound);
    assert_eq!(
        zone3.gaps[0].items,
        vec!["Z=270 A=601 isomer 0".to_string()]
    );
    assert_eq!(zone3.gaps[0].amount_atoms_per_g, Some(1.5e-3));
}

// Verifies: ACT-041
#[test]
#[cfg(unix)]
fn zero_and_empty_spectra_give_zero_activity_without_error() {
    let rig = rig(RECORDED_ZERO);
    for spectrum in [
        GroupFlux {
            energy_edges_ev: edges(),
            flux_per_cm2_s: vec![0.0; GROUPS],
        },
        GroupFlux {
            energy_edges_ev: vec![],
            flux_per_cm2_s: vec![],
        },
    ] {
        let empty = spectrum.is_empty();
        let mut request = fixture_request(0.0);
        request.zones[0].spectrum = spectrum;
        let (result, _) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
        let zone = &result.zones[0];
        assert!(
            zone.input.messages.iter().any(|m| m.contains("zero")),
            "a message names the cause"
        );
        assert_eq!(zone.input.spectrum_total_flux_cm2_s, 0.0);
        assert_eq!(zone.input.fluence_n_cm2, 0.0);
        for p in &zone.points {
            assert_eq!(p.decay_heat.total_w_per_m3, 0.0);
            assert_eq!(p.activity_bq, 0.0);
            assert!(p.nuclides.iter().all(|n| n.activity_bq == 0.0));
            assert_eq!(p.photon_source.total_photons_per_s, 0.0);
            assert!(p.decay_heat.parts_closure_relative == 0.0);
            assert!(
                serde_json::to_string(p).is_ok(),
                "no NaN or infinity to serialise"
            );
        }
        if empty {
            let data = fake_data();
            let problem = build_problem(&request.zones[0], &request, &data.data, "ab");
            assert_eq!(problem["spectrum"]["structure"], "fispact-709");
            assert!(
                problem["spectrum"]["flux_per_group"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|v| v == 0.0)
            );
        }
    }
}

// Verifies: ACT-048
#[test]
#[cfg(unix)]
fn a_changed_hand_off_is_caught_not_absorbed() {
    let rig = rig(RECORDED);
    let request = fixture_request(1.0e8);
    let data = &rig.config.data;
    let good = recorded(RECORDED, data);
    let check = |v: &Value| zone_from_actinv_result(&request.zones[0], &request, data, "h", v);
    check(&good).unwrap();

    // Stale spectrum: ACTINV computed with a different total flux.
    let mut v = good.clone();
    v["ledger"]["assembly"]["flux_total"] = json!(1.1e8);
    assert!(check(&v).unwrap_err().contains("total flux"));
    // Wrong history scaling: the fluence is not the schedule's.
    let mut v = good.clone();
    let f = v["steps"][1]["fluence_n_cm2"].as_f64().unwrap();
    v["steps"][1]["fluence_n_cm2"] = json!(f * 1.001);
    assert!(check(&v).unwrap_err().contains("fluence"));
    // Dropped nuclide: an activity with no inventory row.
    let mut v = good.clone();
    let inv = v["steps"][3]["inventory"].as_array_mut().unwrap();
    let at = inv.iter().position(|n| n["nuclide"] == "Co60").unwrap();
    inv.remove(at);
    assert!(check(&v).unwrap_err().contains("no inventory row"));
    // A cooling step that does not end at the requested time.
    let mut v = good.clone();
    v["steps"][4]["t_s"] = json!(v["steps"][4]["t_s"].as_f64().unwrap() * 1.01);
    assert!(check(&v).unwrap_err().contains("cooling grid"));
    // A missing step.
    let mut v = good.clone();
    v["steps"].as_array_mut().unwrap().pop();
    assert!(check(&v).unwrap_err().contains("steps"));
    // Decay data other than the pinned file.
    let mut v = good.clone();
    v["certificate"]["inputs"]["decay_primary"]["sha256"] = json!("00");
    assert!(check(&v).unwrap_err().contains("decay file"));
    // A missing heat field is an error, not a zero.
    let mut v = good;
    v["steps"][2]["heat_W_per_g"]["total"] = json!(null);
    assert!(check(&v).unwrap_err().contains("heat_W_per_g.total"));
}

// Verifies: ACT-029, NUC-035
#[test]
#[cfg(unix)]
fn photon_source_file_conserves_emission_states_units_and_is_hashed() {
    let rig = rig(RECORDED);
    let mut request = fixture_request(1.0e8);
    let (result, _) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    let file = photon_source_file(&result, "co-a", 3600.0).unwrap();
    assert_eq!(file.unit, "photons/s/cm3");
    assert_eq!(
        file.research_screening,
        faris_model::RESEARCH_SCREENING_STATEMENT
    );
    assert_eq!(file.volume_cm3, 2000.0);
    assert_eq!(
        file.energy_edges_ev.len(),
        file.photons_per_s_per_cm3.len() + 1
    );
    let point = &result.zones[0].points[1];
    let integral: f64 = file
        .photons_per_s_per_cm3
        .iter()
        .map(|r| r * file.volume_cm3)
        .sum();
    assert!(relative_difference(integral, point.photon_source.grouped_photons_per_s) <= 1.0e-12);
    assert!(relative_difference(integral, point.photon_source.total_photons_per_s) <= 1.0e-6);
    assert!(
        photon_source_file(&result, "co-a", 12.0)
            .unwrap_err()
            .contains("cooling time")
    );
    assert!(photon_source_file(&result, "nope", 1.0).is_err());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("src.json");
    let sha = write_photon_source_file(&path, &file).unwrap();
    assert_eq!(sha, hash_file(&path).unwrap());
    assert!(
        write_photon_source_file(&path, &file).is_err(),
        "never replaces a file"
    );
    let back: PhotonSourceFile = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(back, file);
    assert_eq!(back.activation_input_sha256, result.input_sha256);
    request.zones[0].mass_g = 1.0;
}

// Verifies: ACT-009, ACT-010
#[test]
fn an_irradiation_schedule_from_the_history_feeds_the_request_with_its_hash() {
    use crate::history::run_operating_history_cancellable;
    use crate::maintenance::irradiation::{CondensationRule, irradiation_schedule};
    let history = run_operating_history_cancellable(
        &crate::fixtures::assumptions(),
        &crate::fixtures::rates_without_covariance(0.0, 'a'),
        &Cancellation::default(),
    )
    .unwrap();
    let mut schedule =
        irradiation_schedule(&history, None, &CondensationRule::LOSSLESS, &[]).unwrap();
    let zones = vec![zone("co-a", 1000.0, 0.002, &[("Co", 100.0)], flat(1.0e8))];
    let request = ActivationRequest::from_schedule(
        zones.clone(),
        &schedule,
        vec![1.0, 3600.0],
        PhotonOptions::default(),
    )
    .unwrap();
    assert_eq!(request.steps, schedule.steps);
    assert_eq!(
        request.schedule_input_sha256.as_deref(),
        Some(schedule.input_sha256.as_str())
    );
    request.validate().unwrap();
    // The problem's schedule is the history's steps, in order.
    let data = fake_data();
    let problem = build_problem(&request.zones[0], &request, &data.data, "ab");
    for (s, p) in schedule
        .steps
        .iter()
        .zip(problem["schedule"].as_array().unwrap())
    {
        assert_eq!(p["dt"].as_str().unwrap(), format!("{} s", s.duration_s));
        assert_eq!(p["flux"].as_f64().unwrap(), s.flux_scale);
    }
    // A schedule edited after it was hashed is refused.
    schedule.steps[0].flux_scale *= 2.0;
    assert!(
        ActivationRequest::from_schedule(zones, &schedule, vec![1.0], PhotonOptions::default())
            .unwrap_err()
            .contains("hash")
    );
}

#[test]
#[cfg(unix)]
fn limiting_nuclides_keeps_totals_and_counts_the_omitted() {
    let rig = rig(RECORDED);
    let request = fixture_request(1.0e8);
    let (mut result, _) = run_activation(&request, &rig.config, &Cancellation::default()).unwrap();
    let before = result.zones[0].points[1].clone();
    assert!(before.nuclides.len() > 5);
    limit_nuclides(&mut result, 5);
    let after = &result.zones[0].points[1];
    assert_eq!(after.nuclides.len(), 5);
    assert_eq!(after.nuclides_omitted, before.nuclides.len() - 5);
    assert_eq!(after.activity_bq, before.activity_bq);
    assert_eq!(after.nuclides, before.nuclides[..5]);
}

#[test]
fn photon_group_helper_makes_a_valid_fine_structure() {
    let edges = log_photon_groups(120).unwrap();
    assert_eq!(edges.len(), 121);
    assert_eq!(edges[0], 0.0);
    assert!(edges.windows(2).all(|w| w[1] > w[0]));
    assert!(relative_difference(*edges.last().unwrap(), 2.0e7) < 1e-12);
    assert!(log_photon_groups(1).is_err());
}

// ----------------------------------------------- the real ACTINV, if installed --

struct Live {
    config: ActivationConfig,
    _work: tempfile::TempDir,
}

/// The installed ACTINV and its data, or None with the reason printed (CI has neither).
fn live() -> Option<Live> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let actinv = std::env::var_os("FARIS_ACTINV")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/bin/actinv"));
    let data_dir = std::env::var_os("FARIS_ACTINV_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Documents/actinv/actinv-data"));
    let data = ActivationData::catalog(&data_dir);
    if !actinv.is_file() || !data.library.is_file() || !data.decay_primary.is_file() {
        eprintln!(
            "skipped: ACTINV is not installed here (looked for {} and {}); set FARIS_ACTINV and FARIS_ACTINV_DATA",
            actinv.display(),
            data.library.display()
        );
        return None;
    }
    let work = tempfile::tempdir().unwrap();
    let mut config = ActivationConfig::new(actinv, data, work.path().join("work"));
    config.timeout = Duration::from_secs(120);
    Some(Live {
        config,
        _work: work,
    })
}

fn live_run(live: &Live, request: &ActivationRequest) -> ActivationResult {
    run_activation(request, &live.config, &Cancellation::default())
        .unwrap()
        .0
}

/// Writes the raw ACTINV results the recorded fixtures are made from.
/// `FARIS_RECORD_ACTINV_FIXTURES=DIR cargo test record_actinv_fixtures`.
#[test]
fn record_actinv_fixtures() {
    let Some(dir) = std::env::var_os("FARIS_RECORD_ACTINV_FIXTURES").map(PathBuf::from) else {
        return;
    };
    let Some(live) = live() else { return };
    for (name, flux) in [("co59_result", 1.0e8), ("co59_zero_result", 0.0)] {
        let request = fixture_request(flux);
        let problem = build_problem(
            &request.zones[0],
            &request,
            &live.config.data,
            &hash_file(&live.config.data.library).unwrap(),
        );
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{name}.problem.json"));
        std::fs::write(&p, serde_json::to_vec(&problem).unwrap()).unwrap();
        let out = dir.join(format!("{name}.json"));
        run_actinv(
            &live.config,
            &live.config.actinv,
            &p,
            &out,
            &dir,
            &Cancellation::default(),
        )
        .unwrap();
    }
}

// Verifies: ACT-001, ACT-042
#[test]
fn live_actinv_end_to_end_is_reproducible_and_matches_the_recording() {
    let Some(live) = live() else { return };
    let request = fixture_request(1.0e8);
    let a = live_run(&live, &request);
    let b = live_run(&live, &request);
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    // The fixtures were recorded with ACTINV 1.4.0. Later versions must reproduce their decay
    // heat for this start from stable material (ACTINV 1.5.0 changed ledger counts only here).
    assert!(
        a.data.solver.starts_with("actinv-core 1."),
        "unexpected solver {}",
        a.data.solver
    );
    let recorded = recorded(RECORDED, &live.config.data);
    for (j, p) in a.zones[0].points.iter().enumerate() {
        let want = recorded["steps"][2 + j]["heat_W_per_g"]["total"]
            .as_f64()
            .unwrap()
            * 1000.0
            / 0.002;
        assert!(
            relative_difference(p.decay_heat.total_w_per_m3, want) <= 1.0e-9,
            "point {j}"
        );
    }
    let co60 = a.zones[0].points[3]
        .nuclides
        .iter()
        .find(|n| n.nuclide == "Co60")
        .unwrap();
    assert!(co60.activity_bq > 0.0 && co60.atoms.unwrap() > 0.0);
}

// Verifies: ACT-041
#[test]
fn live_actinv_zero_spectrum_is_exactly_zero() {
    let Some(live) = live() else { return };
    let mut request = fixture_request(0.0);
    request.zones[0].spectrum.flux_per_cm2_s = vec![0.0; GROUPS];
    let result = live_run(&live, &request);
    for p in &result.zones[0].points {
        assert_eq!(p.decay_heat.total_w_per_m3, 0.0);
        assert_eq!(p.activity_bq, 0.0);
    }
    assert_eq!(result.zones[0].completeness, Completeness::Complete);
}

// Verifies: ACT-002
#[test]
fn live_actinv_refuses_a_group_structure_that_is_not_its_own() {
    let Some(live) = live() else { return };
    let mut request = fixture_request(1.0e8);
    request.zones[0].spectrum.energy_edges_ev[100] *= 1.001;
    let error = run_activation(&request, &live.config, &Cancellation::default()).unwrap_err();
    assert!(error.contains("actinv run failed"), "{error}");
}

// Verifies: ACT-018
#[test]
fn live_actinv_decay_chain_matches_the_analytic_solution_through_the_adapter() {
    let Some(live) = live() else { return };
    // Decay only: zero flux, starting from Mo-99 (parent), Tc-99m (daughter) or both.
    let cooling = vec![
        3600.0,
        6.0 * 3600.0,
        86_400.0,
        3.0 * 86_400.0,
        10.0 * 86_400.0,
    ];
    let run = |composition: &[(&str, f64)]| {
        let mut request = fixture_request(0.0);
        request.zones[0].basis = CompositionBasis::AtomsPerG;
        request.zones[0].composition = composition
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        request.zones[0].mass_g = 1.0;
        request.zones[0].volume_m3 = 1.0e-6;
        request.zones[0].spectrum.flux_per_cm2_s = vec![0.0; GROUPS];
        request.steps = vec![IrradiationStep {
            duration_s: 1.0,
            flux_scale: 0.0,
        }];
        request.cooling_s = cooling.clone();
        request.mode = SolverMode::Coupled;
        let result = live_run(&live, &request);
        let atoms = |nuclide: &str| -> Vec<f64> {
            result.zones[0]
                .points
                .iter()
                .map(|p| {
                    p.nuclides
                        .iter()
                        .find(|n| n.nuclide == nuclide)
                        .and_then(|n| n.atoms)
                        .unwrap_or(0.0)
                })
                .collect()
        };
        (atoms("Mo99"), atoms("Tc99m1"))
    };
    let n0 = 1.0e18;
    let (parent, _) = run(&[("Mo99", n0)]);
    let (_, pure_daughter) = run(&[("Tc99m", n0)]);
    // Decay constants measured from the pure single-nuclide runs, each checked as a pure
    // exponential at every cooling time (1 s of zero-flux irradiation precedes the grid).
    let t = |j: usize| cooling[j] + 1.0;
    let lambda1 = -math::ln(parent[4] / n0) / t(4);
    // Tc-99m is down to a few atoms by 10 d, at ACTINV's numerical floor: measure it at 6 h.
    let lambda2 = -math::ln(pure_daughter[1] / n0) / t(1);
    for j in 0..cooling.len() {
        assert!(
            relative_difference(parent[j], n0 * math::exp(-lambda1 * t(j))) <= 1.0e-9,
            "Mo-99 exponential at {j}"
        );
        assert!(
            pure_daughter[j] < 1.0e-6 * n0
                || relative_difference(pure_daughter[j], n0 * math::exp(-lambda2 * t(j))) <= 1.0e-9,
            "Tc-99m exponential at {j}"
        );
    }
    // The chain: Tc-99m grows in from Mo-99 with a branching fraction b, the same at every time.
    let (_, daughter) = run(&[("Mo99", n0)]);
    let growth = |j: usize| {
        lambda1 * n0 / (lambda2 - lambda1)
            * (math::exp(-lambda1 * t(j)) - math::exp(-lambda2 * t(j)))
    };
    let b = daughter[1] / growth(1);
    assert!((0.5..1.0).contains(&b), "branching fraction {b}");
    for (j, value) in daughter.iter().enumerate() {
        assert!(
            relative_difference(*value, b * growth(j)) <= 1.0e-8,
            "Bateman at {j}"
        );
    }
}

// Verifies: ACT-001
#[test]
fn a_start_from_named_nuclides_is_sent_coupled_and_explicit_trace_is_refused() {
    let data = fake_data();
    let mut request = fixture_request(0.0);
    request.zones[0].basis = CompositionBasis::AtomsPerG;
    request.zones[0].composition = [("Co60".to_string(), 1.0e18)].into();
    let problem = build_problem(&request.zones[0], &request, &data.data, "ab");
    assert_eq!(problem["options"]["mode"], "coupled");
    assert!(
        effective_mode(&request.zones[0], SolverMode::Auto)
            .1
            .unwrap()
            .contains("coupled")
    );
    request.mode = SolverMode::Trace;
    assert!(request.validate().unwrap_err().contains("coupled"));
    // A natural-element start keeps auto.
    let natural = fixture_request(1.0e8);
    let problem = build_problem(&natural.zones[0], &natural, &data.data, "ab");
    assert_eq!(problem["options"]["mode"], "auto");
    assert!(
        effective_mode(&natural.zones[0], SolverMode::Auto)
            .1
            .is_none()
    );
}

// Verifies: ACT-001
#[test]
fn live_cobalt_60_start_at_zero_flux_halves_in_one_half_life() {
    let Some(live) = live() else { return };
    let n0 = 1.0e18;
    let run = |cooling: f64| {
        let mut request = fixture_request(0.0);
        request.zones[0].basis = CompositionBasis::AtomsPerG;
        request.zones[0].composition = [("Co60".to_string(), n0)].into();
        request.zones[0].mass_g = 1.0;
        request.zones[0].volume_m3 = 1.0e-6;
        request.steps = vec![IrradiationStep {
            duration_s: 1.0,
            flux_scale: 0.0,
        }];
        request.cooling_s = vec![cooling];
        live_run(&live, &request).zones.remove(0)
    };
    let first = run(1.0);
    assert_eq!(first.input.solver_mode_sent, SolverMode::Coupled);
    assert!(first.input.mode_substitution.is_some());
    let p = &first.points[0];
    let co60 = p.nuclides.iter().find(|n| n.nuclide == "Co60").unwrap();
    let lambda = co60.activity_bq / co60.atoms.unwrap();
    // Total time since the start is one half-life: the 1 s step plus the cooling time.
    let half = run(std::f64::consts::LN_2 / lambda - 1.0);
    let a = half.points[0]
        .nuclides
        .iter()
        .find(|n| n.nuclide == "Co60")
        .unwrap()
        .activity_bq;
    assert!(
        relative_difference(a, 0.5 * lambda * n0) <= 1.0e-6,
        "activity {a:e}"
    );
}
