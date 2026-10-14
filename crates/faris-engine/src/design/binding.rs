//! Check 7: each DAGMC volume is matched to exactly one design-file solid by
//! faceted volume and centroid, and its tag must name that solid.
//!
//! Tags are not trusted. Imprinting can reorder volumes, so a tag can end up on
//! the wrong solid without any error; the fingerprint is the binding and the tag
//! is checked against it (`DESIGN_FILE_FORMAT.md`, decision 2).
//!
//! The normalised distance of a volume from a solid is the larger of
//! `|V_faceted / V_cad - 1| / 0.5 %` and `|c_faceted - c_cad| / (1e-3 x model
//! bounding-box diagonal)`. A match is within tolerance when it is at most 1.
//! The second-best candidate must be at least 10 times farther, or the import
//! stops as ambiguous and names both. GEO-030 (the faceted volume within 0.5 %
//! of the CAD volume) is the volume half of the same test, and is named when it
//! is the half that fails.

use super::mesh::{GRID3_PER_M3, Mesh, to_metres};
use faris_model::design::{Design, Facets, FacetsManifest, Finding};
use faris_model::math::powi;
use serde::{Deserialize, Serialize};

const CHECK: u8 = 7;
pub const VOLUME_TOLERANCE: f64 = 0.005;
/// The per-solid volume limit when the converter runs at the default tolerance (GEO-012: the
/// default is chosen so no cell is off by more than 0.1 %). A tolerance the user sets is judged
/// at `VOLUME_TOLERANCE`, the limit of GEO-030 for faceted cells.
pub const DEFAULT_VOLUME_LIMIT: f64 = 0.001;
pub const CENTROID_FRACTION: f64 = 1.0e-3;
pub const SECOND_BEST_FACTOR: f64 = 10.0;

/// One solid's faceted volume against its CAD volume, with the verdict against the limit.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VolumeVerdict {
    pub solid_id: String,
    pub relative_difference: f64,
    /// `WITHIN` or `EXCEEDED`.
    pub verdict: String,
}

/// The per-solid volume gate of GEO-012 and GEO-030, and the tolerance it was run at.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VolumeGate {
    pub chord_m: f64,
    /// `default` (the design file gave none) or `design file`.
    pub tolerance_source: String,
    pub limit: f64,
    pub largest_relative_difference: f64,
    pub solids: Vec<VolumeVerdict>,
}

/// What the facets say about one volume.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VolumeStats {
    pub index: u32,
    pub global_id: i64,
    pub material_tag: Option<String>,
    pub faceted_volume_m3: f64,
    pub centroid_m: [f64; 3],
    pub triangles: u64,
    pub vertices: u64,
}

/// Exact signed volume (summed in `i128`) and the centroid of every volume.
pub fn volume_stats(mesh: &Mesh, facets: &Facets, manifest: &FacetsManifest) -> Vec<VolumeStats> {
    let per_vertices = facets.vertices_per_volume();
    let tris = mesh.volume_triangles();
    (0..mesh.n_volumes)
        .map(|i| {
            let v = i + 1;
            let list = &tris[i as usize];
            let mut six: i128 = 0;
            let mut moment = [0.0f64; 3];
            for &t in list {
                let [a, b, c] = mesh.oriented_for(t, v);
                let det = super::mesh::six_volume_term(mesh.lo, a, b, c);
                six = six.saturating_add(det);
                for k in 0..3 {
                    let centre = ((a[k] - mesh.lo[k]) + (b[k] - mesh.lo[k]) + (c[k] - mesh.lo[k]))
                        as f64
                        / 4.0;
                    moment[k] += det as f64 * centre;
                }
            }
            let volume_grid = six as f64 / 6.0;
            let centroid = if six != 0 {
                let total = six as f64;
                to_metres([
                    mesh.lo[0] + (moment[0] / total).round() as i64,
                    mesh.lo[1] + (moment[1] / total).round() as i64,
                    mesh.lo[2] + (moment[2] / total).round() as i64,
                ])
            } else {
                to_metres(mesh.lo)
            };
            let entry = &manifest.volumes[i as usize];
            VolumeStats {
                index: v,
                global_id: entry.global_id,
                material_tag: entry.material_tag.clone(),
                faceted_volume_m3: volume_grid / GRID3_PER_M3,
                centroid_m: centroid,
                triangles: list.len() as u64,
                vertices: per_vertices[i as usize],
            }
        })
        .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BindingRow {
    pub volume: u32,
    pub global_id: i64,
    pub material_tag: Option<String>,
    pub solid_id: Option<String>,
    pub faceted_volume_m3: f64,
    pub cad_volume_m3: Option<f64>,
    pub volume_relative_difference: Option<f64>,
    pub centroid_distance_m: Option<f64>,
    /// 1 is exactly at the tolerance (0.5 % in volume or 1e-3 of the diagonal in centroid).
    pub normalised_distance: Option<f64>,
    pub second_best_solid_id: Option<String>,
    pub second_best_normalised_distance: Option<f64>,
}

/// Judge every matched solid's volume error against the limit that applies to the tolerance in
/// use: 0.1 % at the default tolerance, 0.5 % at one the user set. A solid over the limit
/// blocks and cannot be waived.
pub fn volume_gate(
    rows: &[BindingRow],
    chord_m: f64,
    user_set: bool,
) -> (VolumeGate, Vec<Finding>) {
    let limit = if user_set {
        VOLUME_TOLERANCE
    } else {
        DEFAULT_VOLUME_LIMIT
    };
    let mut findings = Vec::new();
    let mut solids = Vec::new();
    for row in rows {
        let (Some(id), Some(difference)) = (&row.solid_id, row.volume_relative_difference) else {
            continue;
        };
        let exceeded = difference.abs() > limit;
        if exceeded {
            let (source, advice) = if user_set {
                (
                    format!("the design file's faceting tolerance of {chord_m:.3e} m"),
                    "Set cad.faceting_tolerance_m smaller",
                )
            } else {
                (
                    format!("the default faceting tolerance of {chord_m:.3e} m"),
                    "Set cad.faceting_tolerance_m smaller than the default",
                )
            };
            findings.push(
                Finding::new(
                    CHECK,
                    format!("solid {id:?}"),
                    format!(
                        "its faceted volume differs from the CAD volume by {:+.4} %, beyond the {:.1} % limit at {source} (GEO-012, GEO-030)",
                        100.0 * difference,
                        100.0 * limit
                    ),
                    format!(
                        "{advice} so the facets follow its surfaces more closely, then check again; \
                         the report lists every solid's volume error."
                    ),
                )
                .requirement("GEO-030")
                .cells(vec![id.clone()]),
            );
        }
        solids.push(VolumeVerdict {
            solid_id: id.clone(),
            relative_difference: difference,
            verdict: if exceeded { "EXCEEDED" } else { "WITHIN" }.into(),
        });
    }
    let largest = solids
        .iter()
        .map(|v| v.relative_difference.abs())
        .fold(0.0, f64::max);
    (
        VolumeGate {
            chord_m,
            tolerance_source: if user_set { "design file" } else { "default" }.into(),
            limit,
            largest_relative_difference: largest,
            solids,
        },
        findings,
    )
}

#[derive(Clone, Debug)]
pub struct BindingOutcome {
    pub findings: Vec<Finding>,
    pub rows: Vec<BindingRow>,
    /// For every volume (index = volume - 1), the index in `design.solids`. `None` unless
    /// every volume and every solid is matched exactly once.
    pub volume_solid: Option<Vec<usize>>,
    pub model_diagonal_m: f64,
}

struct Candidate {
    solid: usize,
    centroid_term: f64,
    distance: f64,
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    (powi(a[0] - b[0], 2) + powi(a[1] - b[1], 2) + powi(a[2] - b[2], 2)).sqrt()
}

pub fn bind(design: &Design, stats: &[VolumeStats], model_diagonal_m: f64) -> BindingOutcome {
    let mut findings = Vec::new();
    let mut rows = Vec::new();
    if stats.len() != design.solids.len() {
        findings.push(Finding::new(
            CHECK,
            "the model's volumes",
            format!(
                "the converted model has {} volumes and the design file lists {} solids; every \
                 solid must be one volume and every volume one solid",
                stats.len(),
                design.solids.len()
            ),
            "Check the STEP file for solids the converter merged, dropped or split, and for \
             overlapping solids; fix them in the CAD tool and export again.",
        ));
    }
    let mut best_for_volume: Vec<Option<usize>> = vec![None; stats.len()];
    let mut claimed: Vec<Vec<u32>> = vec![Vec::new(); design.solids.len()];
    for (i, volume) in stats.iter().enumerate() {
        let mut candidates: Vec<Candidate> = design
            .solids
            .iter()
            .enumerate()
            .map(|(s, solid)| {
                let cad = solid.fingerprint.cad_volume_m3;
                let volume_term = if cad != 0.0 {
                    (volume.faceted_volume_m3 / cad - 1.0).abs() / VOLUME_TOLERANCE
                } else {
                    f64::INFINITY
                };
                let centroid_term = dist3(volume.centroid_m, solid.fingerprint.centroid_m)
                    / (CENTROID_FRACTION * model_diagonal_m);
                Candidate {
                    solid: s,
                    centroid_term,
                    distance: volume_term.max(centroid_term),
                }
            })
            .collect();
        candidates.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        let mut row = BindingRow {
            volume: volume.index,
            global_id: volume.global_id,
            material_tag: volume.material_tag.clone(),
            solid_id: None,
            faceted_volume_m3: volume.faceted_volume_m3,
            cad_volume_m3: None,
            volume_relative_difference: None,
            centroid_distance_m: None,
            normalised_distance: None,
            second_best_solid_id: None,
            second_best_normalised_distance: None,
        };
        let describe = format!(
            "volume {} (tag {})",
            volume.index,
            volume.material_tag.as_deref().unwrap_or("none")
        );
        let Some(best) = candidates.first() else {
            rows.push(row);
            continue;
        };
        let solid = &design.solids[best.solid];
        row.cad_volume_m3 = Some(solid.fingerprint.cad_volume_m3);
        row.volume_relative_difference =
            Some(volume.faceted_volume_m3 / solid.fingerprint.cad_volume_m3 - 1.0);
        row.centroid_distance_m = Some(dist3(volume.centroid_m, solid.fingerprint.centroid_m));
        row.normalised_distance = Some(best.distance);
        if let Some(second) = candidates.get(1) {
            row.second_best_solid_id = Some(design.solids[second.solid].id.clone());
            row.second_best_normalised_distance = Some(second.distance);
        }
        if best.distance > 1.0 {
            // Name the half of the test that failed.
            if best.centroid_term <= 1.0 {
                findings.push(
                    Finding::new(
                        CHECK,
                        describe.clone(),
                        format!(
                            "its centroid matches solid {:?} but its faceted volume is {:.4} m3 against the CAD volume {:.4} m3 ({:+.3} %), beyond the 0.5 % limit (GEO-030)",
                            solid.id,
                            volume.faceted_volume_m3,
                            solid.fingerprint.cad_volume_m3,
                            100.0 * row.volume_relative_difference.unwrap_or(0.0)
                        ),
                        "Set cad.faceting_tolerance_m smaller so the facets follow the surfaces \
                         more closely, or look for a defect in that solid in the CAD tool.",
                    )
                    .requirement("GEO-030")
                    .cells(vec![solid.id.clone()]),
                );
            } else {
                findings.push(Finding::new(
                    CHECK,
                    describe.clone(),
                    format!(
                        "it matches no solid: the closest is {:?}, off by {:+.3} % in volume and {:.4} m in centroid (limits 0.5 % and {:.4} m)",
                        solid.id,
                        100.0 * row.volume_relative_difference.unwrap_or(0.0),
                        row.centroid_distance_m.unwrap_or(0.0),
                        CENTROID_FRACTION * model_diagonal_m
                    ),
                    "A solid was changed, merged or split by the conversion. Compare the design \
                     file's fingerprints with the model; check the STEP file for overlapping solids.",
                ));
            }
            rows.push(row);
            continue;
        }
        if let Some(second) = candidates.get(1)
            // A floor on the best distance, so two exact copies (both at 0) are ambiguous.
            && second.distance < SECOND_BEST_FACTOR * best.distance.max(1.0e-9)
        {
            findings.push(
                Finding::new(
                    CHECK,
                    describe.clone(),
                    format!(
                        "it is ambiguous between solids {:?} (distance {:.4}) and {:?} (distance {:.4}); the second must be at least {SECOND_BEST_FACTOR} times farther than the first",
                        solid.id,
                        best.distance,
                        design.solids[second.solid].id,
                        second.distance
                    ),
                    "Two solids have the same volume and place within tolerance, so no tag can be \
                     trusted. They are overlapping or duplicated: remove one in the CAD tool.",
                )
                .cells(vec![solid.id.clone(), design.solids[second.solid].id.clone()]),
            );
            rows.push(row);
            continue;
        }
        row.solid_id = Some(solid.id.clone());
        best_for_volume[i] = Some(best.solid);
        claimed[best.solid].push(volume.index);
        // The tag must name the matched solid.
        let expected = format!("mat:{}", solid.id);
        if volume.material_tag.as_deref() != Some(expected.as_str()) {
            findings.push(
                Finding::new(
                    CHECK,
                    describe,
                    format!(
                        "tag on the wrong solid: the geometry is solid {:?} but the tag is {:?} (expected {expected:?}). Imprinting reordered the volumes",
                        solid.id,
                        volume.material_tag.as_deref().unwrap_or("none")
                    ),
                    "FARIS never re-tags a volume. Check the conversion log and the STEP file; if \
                     it repeats, report the model.",
                )
                .cells(vec![solid.id.clone()]),
            );
        }
        rows.push(row);
    }
    for (s, volumes) in claimed.iter().enumerate() {
        match volumes.len() {
            0 => findings.push(
                Finding::new(
                    CHECK,
                    format!("solid {:?}", design.solids[s].id),
                    "no volume of the converted model matches it, so it is missing from the transport model",
                    "Look for a solid the converter dropped or merged into another; fix it in the CAD tool.",
                )
                .cells(vec![design.solids[s].id.clone()]),
            ),
            1 => {}
            _ => findings.push(
                Finding::new(
                    CHECK,
                    format!("solid {:?}", design.solids[s].id),
                    format!("volumes {volumes:?} all match it; each solid must match exactly one volume"),
                    "The solid was split by the converter or duplicated in the STEP file; fix it in the CAD tool.",
                )
                .cells(vec![design.solids[s].id.clone()]),
            ),
        }
    }
    let complete = findings.is_empty()
        && best_for_volume.iter().all(Option::is_some)
        && stats.len() == design.solids.len();
    BindingOutcome {
        findings,
        rows,
        volume_solid: complete.then(|| best_for_volume.into_iter().flatten().collect()),
        model_diagonal_m,
    }
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::super::testutil::{design_for, manifest_of};
    use super::*;

    fn setup() -> (Mesh, Facets, FacetsManifest) {
        let f = separate_boxes(&[
            ([0.0; 3], [1.0, 1.0, 1.0]),
            ([5.0, 0.0, 0.0], [7.0, 1.0, 1.0]),
            ([0.0, 5.0, 0.0], [3.0, 7.0, 1.0]),
        ]);
        let m = manifest_of(&f, &["mat:a", "mat:b", "mat:c"]);
        (Mesh::from_facets(&f).unwrap(), f, m)
    }

    // Verifies: GEO-030
    #[test]
    fn exact_volumes_and_centroids_of_boxes() {
        let (mesh, f, m) = setup();
        let stats = volume_stats(&mesh, &f, &m);
        assert!((stats[0].faceted_volume_m3 - 1.0).abs() < 1e-12);
        assert!((stats[1].faceted_volume_m3 - 2.0).abs() < 1e-12);
        assert!((stats[2].faceted_volume_m3 - 6.0).abs() < 1e-12);
        assert!((stats[1].centroid_m[0] - 6.0).abs() < 1e-9);
        assert!((stats[2].centroid_m[1] - 6.0).abs() < 1e-9);
        assert_eq!(stats[0].triangles, 12);
        assert_eq!(stats[0].vertices, 8);
    }

    // Verifies: GEO-012
    #[test]
    fn every_volume_binds_to_its_own_solid() {
        let (mesh, f, m) = setup();
        let stats = volume_stats(&mesh, &f, &m);
        let design = design_for(&["a", "b", "c"], &stats);
        let out = bind(&design, &stats, mesh.diagonal() / 1e9);
        assert!(out.findings.is_empty(), "{:#?}", out.findings);
        assert_eq!(out.volume_solid, Some(vec![0, 1, 2]));
        assert_eq!(out.rows[1].solid_id.as_deref(), Some("b"));
        assert!(
            out.rows
                .iter()
                .all(|r| r.normalised_distance.unwrap() < 1e-6)
        );
    }

    #[test]
    fn a_tag_on_the_wrong_solid_stops_the_import() {
        let (mesh, f, _) = setup();
        let m = manifest_of(&f, &["mat:b", "mat:a", "mat:c"]);
        let stats = volume_stats(&mesh, &f, &m);
        let design = design_for(&["a", "b", "c"], &stats);
        let out = bind(&design, &stats, mesh.diagonal() / 1e9);
        assert!(out.volume_solid.is_none());
        assert_eq!(out.findings.len(), 2);
        assert!(
            out.findings
                .iter()
                .all(|f| f.why.contains("tag on the wrong solid"))
        );
        assert!(out.findings[0].next_step.contains("never re-tags"));
    }

    #[test]
    fn two_solids_that_cannot_be_told_apart_are_ambiguous_and_named() {
        let f = separate_boxes(&[([0.0; 3], [1.0; 3]), ([0.0; 3], [1.0; 3])]);
        let m = manifest_of(&f, &["mat:a", "mat:b"]);
        let mesh = Mesh::from_facets(&f).unwrap();
        let stats = volume_stats(&mesh, &f, &m);
        let design = design_for(&["a", "b"], &stats);
        let out = bind(&design, &stats, mesh.diagonal() / 1e9);
        assert!(out.volume_solid.is_none());
        let f0 = &out.findings[0];
        assert!(
            f0.why.contains("ambiguous") && f0.why.contains("\"a\"") && f0.why.contains("\"b\""),
            "{f0:?}"
        );
        assert_eq!(f0.cells, vec!["a".to_string(), "b".to_string()]);
    }

    // Verifies: GEO-030
    #[test]
    fn a_faceted_volume_off_by_more_than_half_a_percent_is_geo_030() {
        let (mesh, f, m) = setup();
        let stats = volume_stats(&mesh, &f, &m);
        let mut design = design_for(&["a", "b", "c"], &stats);
        design.solids[1].fingerprint.cad_volume_m3 *= 1.006;
        let out = bind(&design, &stats, mesh.diagonal() / 1e9);
        let f = out
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("GEO-030"))
            .unwrap();
        assert!(
            f.why.contains("GEO-030") && f.why.contains("0.5 %"),
            "{f:?}"
        );
        assert!(!f.waivable, "a volume mismatch cannot be waived");
        // exactly at 0.4 % is fine
        design.solids[1].fingerprint.cad_volume_m3 = stats[1].faceted_volume_m3 / 1.004;
        assert!(
            bind(&design, &stats, mesh.diagonal() / 1e9)
                .findings
                .is_empty()
        );
    }

    // Verifies: GEO-012, GEO-030
    #[test]
    fn the_default_tolerance_holds_every_solid_to_a_tenth_of_a_percent() {
        let (mesh, f, m) = setup();
        let stats = volume_stats(&mesh, &f, &m);
        let mut design = design_for(&["a", "b", "c"], &stats);
        // 0.2 % off: inside the 0.5 % that matches a volume to a solid, outside 0.1 %
        design.solids[1].fingerprint.cad_volume_m3 = stats[1].faceted_volume_m3 / 1.002;
        let out = bind(&design, &stats, mesh.diagonal() / 1e9);
        assert!(out.findings.is_empty(), "{:#?}", out.findings);
        let (gate, findings) = volume_gate(&out.rows, 1.0e-3, false);
        assert_eq!(gate.tolerance_source, "default");
        assert_eq!(gate.limit, DEFAULT_VOLUME_LIMIT);
        let verdicts: Vec<&str> = gate.solids.iter().map(|v| v.verdict.as_str()).collect();
        assert_eq!(verdicts, ["WITHIN", "EXCEEDED", "WITHIN"]);
        assert!((gate.largest_relative_difference - 0.002).abs() < 1e-6);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.requirement.as_deref(), Some("GEO-030"));
        assert!(
            f.why.contains("0.1 %") && f.why.contains("default"),
            "{f:?}"
        );
        assert!(f.next_step.contains("cad.faceting_tolerance_m"));
        assert!(!f.waivable && f.blocks());
        // a tolerance the user set is judged at 0.5 %
        let (gate, findings) = volume_gate(&out.rows, 1.0e-3, true);
        assert_eq!(
            (gate.limit, gate.tolerance_source.as_str()),
            (0.005, "design file")
        );
        assert!(findings.is_empty());
        // exactly on the limit is within it
        design.solids[1].fingerprint.cad_volume_m3 = stats[1].faceted_volume_m3 / 1.0009;
        let out = bind(&design, &stats, mesh.diagonal() / 1e9);
        assert!(volume_gate(&out.rows, 1.0e-3, false).1.is_empty());
    }

    #[test]
    fn a_missing_or_extra_volume_is_reported_with_the_solid() {
        let (mesh, f, m) = setup();
        let stats = volume_stats(&mesh, &f, &m);
        let design = design_for(&["a", "b", "c"], &stats);
        let out = bind(&design, &stats[..2], mesh.diagonal() / 1e9);
        assert!(out.findings.iter().any(|f| f.why.contains("has 2 volumes")));
        assert!(
            out.findings
                .iter()
                .any(|f| f.item.contains("\"c\"") && f.why.contains("missing"))
        );
        assert!(out.volume_solid.is_none());
        // a displaced centroid matches nothing
        let mut moved = design_for(&["a", "b", "c"], &stats);
        moved.solids[0].fingerprint.centroid_m[0] += 0.5;
        let out = bind(&moved, &stats, mesh.diagonal() / 1e9);
        assert!(
            out.findings
                .iter()
                .any(|f| f.why.contains("matches no solid"))
        );
    }
}
