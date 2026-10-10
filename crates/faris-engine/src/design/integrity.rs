//! Check 8: FARIS's own integrity checks on the faceted model, in exact
//! arithmetic (`CAD_INTEGRITY_CHECKS.md`, check 8).
//!
//! Vertices are snapped to a 1 nm integer grid and every orientation and
//! intersection test is exact (see `exact`). The stages, each reporting
//! progress and honouring cancellation:
//!
//! 1. edges per volume: watertight and consistently oriented (GEO-020);
//! 2. signed volume per volume: inverted normals, zero-volume cells, slivers,
//!    and triangles with no area on the grid;
//! 3. triangle pairs that meet where they should not: self-intersection,
//!    faceted overlap, contact without shared vertices, duplicate surfaces
//!    (GEO-021), plus containment of one volume in another;
//! 4. gaps (GEO-022), topologically: the closed components of the surfaces that
//!    face the implicit complement, signed by their enclosed volume;
//! 5. point sampling (GEO-024) and ray consistency (GEO-025);
//! 6. partition (GEO-031) and the void list (GEO-039).
//!
//! A finding is never repaired. Every finding names the solids, a location in
//! metres, why it blocks, and what to change in the CAD model. Sampling needs
//! closed surfaces, so a model that fails GEO-020 skips stages 4 and 5; the
//! record says so.

use super::exact::P3;
use super::intersections::{self, Aggregate, Kind};
use super::mesh::{GRID3_PER_M3, Mesh, to_metres};
use super::model::Model;
use super::par::{Cancelled, Ctl};
use super::rays::{Caster, Hit, MAX_REDRAWS, PointClass, Rng};
use super::topology::{self, Component, EdgeProblems};
use faris_model::design::Finding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

const CHECK: u8 = 8;
/// Findings of one class listed individually; the rest are summarised (and still block).
const LIST_CAP: usize = 100;
/// Tries to find an interior point of a volume or cavity.
const INTERIOR_TRIES: u32 = 20_000;
const SLIVER_FACTOR: f64 = 10.0;
const ZERO_VOLUME_FRACTION: f64 = 1.0e-12;

#[derive(Clone, Debug)]
pub struct Config {
    /// The faceting tolerance the model was converted with, metres.
    pub chord_m: f64,
    pub seed: u64,
    pub samples: usize,
    pub chords: usize,
}

impl Config {
    #[cfg(test)]
    pub fn new(chord_m: f64, seed: u64) -> Self {
        Self {
            chord_m,
            seed,
            samples: 1_000_000,
            chords: 100_000,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct VolumeRecord {
    pub volume: u32,
    pub solid_id: String,
    pub faceted_volume_m3: f64,
    pub area_m2: f64,
    /// Mean thickness 2V/A, metres.
    pub mean_thickness_m: Option<f64>,
    /// A point verified strictly inside the volume, metres.
    pub interior_point_m: Option<[f64; 3]>,
    pub sampled_points: u64,
    pub sampled_volume_m3: f64,
    pub sampled_standard_error_m3: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct CavityRecord {
    pub wall_volumes: Vec<String>,
    pub volume_m3: f64,
    pub bbox_lo_m: [f64; 3],
    pub bbox_hi_m: [f64; 3],
    pub interior_point_m: Option<[f64; 3]>,
    pub sampled_points: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct SamplingRecord {
    pub points: u64,
    pub seed: u64,
    pub outside: u64,
    pub in_two_or_more: u64,
    pub in_cavities: u64,
    pub undecided: u64,
    pub max_redraws: u32,
    pub box_volume_m3: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ChordRecord {
    pub chords: u64,
    pub crossings: u64,
    pub inconsistent: u64,
    pub undecided: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct PartitionRecord {
    pub faceted_sum_m3: f64,
    pub cavity_sum_m3: f64,
    pub sampled_assembly_m3: f64,
    pub standard_error_m3: f64,
    pub sigmas: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct StageTime {
    pub stage: String,
    pub seconds: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct IntegrityRecord {
    /// What snapping to the integer grid does, stated in every report.
    pub snap: String,
    pub vertices: u64,
    pub triangles: u64,
    pub surfaces: u64,
    pub volumes: Vec<VolumeRecord>,
    pub cavities: Vec<CavityRecord>,
    pub sampling: SamplingRecord,
    pub chords: ChordRecord,
    pub partition: Option<PartitionRecord>,
    pub intersecting_pairs: u64,
    /// Stages that did not run, with the reason.
    pub skipped: Vec<String>,
    pub seconds: Vec<StageTime>,
    pub peak_rss_bytes: Option<u64>,
}

pub struct IntegrityResult {
    pub findings: Vec<Finding>,
    pub record: IntegrityRecord,
}

fn centroid_m(mesh: &Mesh, t: u32) -> [f64; 3] {
    let [a, b, c] = mesh.tri_pts(t);
    to_metres([
        (a[0] + b[0] + c[0]) / 3,
        (a[1] + b[1] + c[1]) / 3,
        (a[2] + b[2] + c[2]) / 3,
    ])
}

fn edge_mid_m(mesh: &Mesh, e: (u32, u32)) -> [f64; 3] {
    let (a, b) = (mesh.pts[e.0 as usize], mesh.pts[e.1 as usize]);
    to_metres([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2])
}

fn fmt_m(p: [f64; 3]) -> String {
    format!("({:.6}, {:.6}, {:.6}) m", p[0], p[1], p[2])
}

/// Peak resident memory of this process, from the kernel.
pub fn peak_rss_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn triangle_area_m2(mesh: &Mesh, t: u32) -> f64 {
    let [a, b, c] = mesh.tri_pts(t);
    let u = [
        (b[0] - a[0]) as f64,
        (b[1] - a[1]) as f64,
        (b[2] - a[2]) as f64,
    ];
    let v = [
        (c[0] - a[0]) as f64,
        (c[1] - a[1]) as f64,
        (c[2] - a[2]) as f64,
    ];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() * 1.0e-18
}

struct Labels<'a>(&'a [String]);

impl Labels<'_> {
    fn of(&self, volume: u32) -> String {
        self.0
            .get(volume as usize - 1)
            .cloned()
            .unwrap_or_else(|| format!("volume {volume}"))
    }
    fn set(&self, volumes: &[u32]) -> Vec<String> {
        volumes.iter().map(|&v| self.of(v)).collect()
    }
}

/// Cap a class of findings; the remainder is one blocking summary, so a hundred
/// waivers cannot hide a hundred-and-first finding.
fn capped(mut list: Vec<Finding>, what: &str, requirement: &str) -> Vec<Finding> {
    if list.len() > LIST_CAP {
        let more = list.len() - LIST_CAP;
        list.truncate(LIST_CAP);
        list.push(
            Finding::new(
                CHECK,
                format!("{what}, further cases"),
                format!(
                    "{more} more {what} findings are not listed; the first {LIST_CAP} are above"
                ),
                "Fix the listed cases in the CAD model first, then check again.",
            )
            .requirement(requirement),
        );
    }
    list
}

struct Run<'a> {
    model: &'a Model,
    labels: Labels<'a>,
    config: &'a Config,
    ctl: &'a Ctl<'a>,
}

fn timed<T>(
    record: &mut IntegrityRecord,
    stage: &str,
    f: impl FnOnce(&mut IntegrityRecord) -> Result<T, Cancelled>,
) -> Result<T, Cancelled> {
    let start = Instant::now();
    let out = f(record)?;
    record.seconds.push(StageTime {
        stage: stage.to_string(),
        seconds: start.elapsed().as_secs_f64(),
    });
    Ok(out)
}

/// Run check 8. `labels[v - 1]` is the design-file solid id of volume `v`.
pub fn check_integrity(
    model: &Model,
    labels: &[String],
    config: &Config,
    ctl: &Ctl,
) -> Result<IntegrityResult, Cancelled> {
    let run = Run {
        model,
        labels: Labels(labels),
        config,
        ctl,
    };
    let mesh = &model.mesh;
    let mut record = IntegrityRecord {
        snap: "Vertices are snapped to a 1 nm integer grid (at most 0.5 nm per axis, far below \
               any faceting tolerance); every test is exact on the snapped model."
            .to_string(),
        vertices: mesh.pts.len() as u64,
        triangles: mesh.tris.len() as u64,
        surfaces: mesh.surfaces.len() as u64,
        ..IntegrityRecord::default()
    };
    let mut findings: Vec<Finding> = Vec::new();

    let edges = timed(&mut record, "edges", |_| topology::edge_problems(mesh, ctl))?;
    let edge_findings = run.edge_findings(&edges);
    let watertight = edge_findings.is_empty();
    findings.extend(edge_findings);

    timed(&mut record, "volumes", |record| {
        run.volume_stage(record, &mut findings);
        Ok(())
    })?;

    let pairs = timed(&mut record, "triangle intersections", |_| {
        intersections::intersection_pass(model, ctl)
    })?;
    record.intersecting_pairs = pairs.values().map(|a| a.pairs).sum();
    let reported = run.intersection_findings(&pairs, &mut findings);

    if !watertight {
        record.skipped.push(
            "containment, gaps, point sampling, ray consistency and the partition test were not \
             run: they need closed surfaces, and GEO-020 failed"
                .to_string(),
        );
        findings = capped_all(findings);
        record.peak_rss_bytes = peak_rss_bytes();
        return Ok(IntegrityResult { findings, record });
    }

    // Gap components first: the casters below track them.
    let components = match topology::boundary_components(mesh) {
        Ok(c) => c,
        Err(reason) => {
            record.skipped.push(format!("gap analysis: {reason}"));
            findings.push(
                Finding::new(
                    CHECK,
                    "gap analysis",
                    reason,
                    "Split the model into smaller solids or check the units of the STEP file.",
                )
                .requirement("GEO-022"),
            );
            (Vec::new(), vec![u32::MAX; mesh.surfaces.len()])
        }
    };
    let (components, surface_component) = components;
    let caster = Caster::with_components(mesh, &model.bvh, &surface_component, components.len());

    timed(&mut record, "containment", |_| {
        run.containment(&caster, &reported, &mut findings)
    })?;
    let cavities = timed(&mut record, "gaps", |record| {
        run.gaps(&caster, &components, record, &mut findings)
    })?;
    timed(&mut record, "interior points", |record| {
        run.interior_points(&caster, record)
    })?;
    timed(&mut record, "point sampling", |record| {
        run.sampling(&caster, &components, &cavities, record, &mut findings)
    })?;
    timed(&mut record, "ray consistency", |record| {
        run.chords(&caster, record, &mut findings)
    })?;
    findings = capped_all(findings);
    record.peak_rss_bytes = peak_rss_bytes();
    Ok(IntegrityResult { findings, record })
}

/// Apply the per-class cap to the whole list, keeping order.
fn capped_all(findings: Vec<Finding>) -> Vec<Finding> {
    let mut by_class: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for f in findings {
        let class = f.requirement.clone().unwrap_or_default();
        if !by_class.contains_key(&class) {
            order.push(class.clone());
        }
        by_class.entry(class).or_default().push(f);
    }
    order
        .into_iter()
        .flat_map(|class| {
            let list = by_class.remove(&class).unwrap_or_default();
            capped(list, &class.to_string(), &class)
        })
        .collect()
}

/// Everything one cavity needs to be placed among the others.
struct Cavity {
    component: u32,
    volume_m3: f64,
    interior: Option<P3>,
    /// Number of components that contain this one.
    depth: usize,
}

impl Run<'_> {
    fn mesh(&self) -> &Mesh {
        &self.model.mesh
    }

    fn edge_findings(&self, edges: &[EdgeProblems]) -> Vec<Finding> {
        let mesh = self.mesh();
        let mut out = Vec::new();
        for p in edges {
            let label = self.labels.of(p.volume);
            let cells = vec![label.clone()];
            let item = |what: &str| format!("{label}: {what}");
            let mut add =
                |what: &str, count: u64, first: Option<(u32, u32)>, why: String, next: &str| {
                    if count == 0 {
                        return;
                    }
                    let mut f = Finding::new(CHECK, item(what), why, next)
                        .requirement("GEO-020")
                        .cells(cells.clone());
                    if let Some(e) = first {
                        f = f.at(edge_mid_m(mesh, e));
                    }
                    out.push(f);
                };
            add(
                "open edges",
                p.open,
                p.first_open,
                format!(
                    "{} edges of the surface are used by one triangle only: the surface has a hole, \
                     so a particle can leave the solid without crossing a boundary (first at the marked point)",
                    p.open
                ),
                "Close the hole in the CAD tool (a missing or unsewn face), then export again.",
            );
            add(
                "non-manifold edges",
                p.non_manifold,
                p.first_non_manifold,
                format!(
                    "{} edges are shared by more than two triangles of this solid: the surface \
                     folds back on itself or two surfaces touch along a line",
                    p.non_manifold
                ),
                "Remove the extra faces or separate the touching solids in the CAD tool, then export again.",
            );
            add(
                "inconsistent orientation",
                p.misoriented,
                p.first_misoriented,
                format!(
                    "{} edges are used twice in the same direction: neighbouring triangles \
                     disagree about which side is inside",
                    p.misoriented
                ),
                "Fix the face orientation of the solid in the CAD tool, then export again.",
            );
            add(
                "triangles with a repeated vertex",
                p.repeated_vertex_triangles,
                None,
                format!(
                    "{} triangles use one vertex twice, so they enclose no area",
                    p.repeated_vertex_triangles
                ),
                "Remove the sliver faces in the CAD tool, then export again.",
            );
        }
        out
    }

    fn volume_stage(&self, record: &mut IntegrityRecord, findings: &mut Vec<Finding>) {
        let mesh = self.mesh();
        let per_volume = mesh.volume_triangles();
        let bbox = mesh.bbox_volume_m3();
        let mut slivers = Vec::new();
        for (i, tris) in per_volume.iter().enumerate() {
            let v = i as u32 + 1;
            let label = self.labels.of(v);
            let area: f64 = tris.iter().map(|&t| triangle_area_m2(mesh, t)).sum();
            let mut row = VolumeRecord {
                volume: v,
                solid_id: label.clone(),
                area_m2: area,
                ..VolumeRecord::default()
            };
            let place = tris.first().map(|&t| centroid_m(mesh, t));
            match mesh.six_volume_exact(v, tris) {
                None => findings.push(
                    Finding::new(
                        CHECK,
                        format!("{label}: volume"),
                        "the enclosed volume overflows the exact 128-bit sum",
                        "Check the units of the STEP file; the solid is far larger than any reactor part.",
                    )
                    .requirement("ZERO-VOLUME")
                    .cells(vec![label.clone()]),
                ),
                Some(six) => {
                    let volume = six as f64 / 6.0 / GRID3_PER_M3;
                    row.faceted_volume_m3 = volume;
                    if six < 0 {
                        let mut f = Finding::new(
                            CHECK,
                            format!("{label}: inverted normals"),
                            format!(
                                "the signed volume is {volume:.6e} m3, negative: the triangles point \
                                 into the solid, so the inside is read as outside"
                            ),
                            "Reverse the face orientation of the solid in the CAD tool, then export again.",
                        )
                        .requirement("INVERTED-NORMALS")
                        .cells(vec![label.clone()]);
                        if let Some(p) = place {
                            f = f.at(p);
                        }
                        findings.push(f);
                    } else if volume <= ZERO_VOLUME_FRACTION * bbox {
                        let mut f = Finding::new(
                            CHECK,
                            format!("{label}: zero volume"),
                            format!(
                                "the volume is {volume:.3e} m3, at most 1e-12 of the model's \
                                 bounding box ({bbox:.3e} m3): the solid encloses nothing"
                            ),
                            "Remove the solid or give it thickness in the CAD tool, then export again.",
                        )
                        .requirement("ZERO-VOLUME")
                        .cells(vec![label.clone()]);
                        if let Some(p) = place {
                            f = f.at(p);
                        }
                        findings.push(f);
                    } else if area > 0.0 {
                        let thickness = 2.0 * volume / area;
                        row.mean_thickness_m = Some(thickness);
                        if thickness < SLIVER_FACTOR * self.config.chord_m {
                            slivers.push((label.clone(), thickness, place));
                        }
                    }
                }
            }
            record.volumes.push(row);
        }
        for (label, thickness, place) in slivers {
            let mut f = Finding::new(
                CHECK,
                format!("{label}: sliver"),
                format!(
                    "the mean thickness 2V/A is {:.3e} m, below ten faceting tolerances ({:.3e} m): \
                     the solid is thinner than the mesh resolves, and its volume and boundaries are unreliable",
                    thickness,
                    SLIVER_FACTOR * self.config.chord_m
                ),
                "Thicken the solid in the CAD tool, merge it into its neighbour, or accept it with a waiver.",
            )
            .requirement("SLIVER")
            .cells(vec![label])
            .waivable();
            if let Some(p) = place {
                f = f.at(p);
            }
            findings.push(f);
        }
        // triangles with no area on the grid
        let mut by_volume: BTreeMap<u32, (u64, u32)> = BTreeMap::new();
        for &t in &mesh.degenerate {
            for v in mesh.volumes_of(t) {
                let e = by_volume.entry(v).or_insert((0, t));
                e.0 += 1;
            }
        }
        for (v, (count, first)) in by_volume {
            let label = self.labels.of(v);
            findings.push(
                Finding::new(
                    CHECK,
                    format!("{label}: triangles without area"),
                    format!(
                        "{count} triangles have no area once snapped to the 1 nm grid; they cannot \
                         be tested and are left out of every intersection test"
                    ),
                    "Remove the degenerate faces in the CAD tool, then export again.",
                )
                .requirement("DEGENERATE-TRIANGLE")
                .cells(vec![label])
                .at(centroid_m(mesh, first)),
            );
        }
    }

    fn intersection_findings(
        &self,
        pairs: &BTreeMap<intersections::Key, Aggregate>,
        findings: &mut Vec<Finding>,
    ) -> Vec<(u32, u32)> {
        let mesh = self.mesh();
        let mut reported = Vec::new();
        for (key, agg) in pairs {
            let at = centroid_m(mesh, agg.first.0);
            let triangles = format!(
                "{} triangle pairs meet (first: triangles {} and {})",
                agg.pairs, agg.first.0, agg.first.1
            );
            let finding = match key.kind {
                Kind::SelfIntersection => {
                    let label = self.labels.of(key.a);
                    Finding::new(
                        CHECK,
                        format!("{label}: self-intersection"),
                        format!(
                            "{triangles} where the solid's own boundary intersects itself{}",
                            if agg.coplanar && !agg.proper {
                                " (the triangles overlap in one plane: a fold or duplicated face)"
                            } else {
                                ""
                            }
                        ),
                        "Repair the intersecting or folded faces of the solid in the CAD tool, then export again.",
                    )
                    .requirement("SELF-INTERSECTION")
                    .cells(vec![label])
                }
                Kind::Overlap | Kind::Contact => {
                    reported.push((key.a, key.b));
                    let (la, lb) = (self.labels.of(key.a), self.labels.of(key.b));
                    let (what, why, next) = if key.kind == Kind::Overlap {
                        (
                            "overlap",
                            format!(
                                "{triangles}: the boundaries of the two solids cross, so they \
                                 occupy the same space there and a particle would have two materials"
                            ),
                            "Remove the overlap in the CAD tool (cut one solid with the other), then export again.",
                        )
                    } else {
                        (
                            "contact without shared vertices",
                            format!(
                                "{triangles}: faces of the two solids touch but do not share the \
                                 vertices a conforming mesh needs, so transport can pass between them"
                            ),
                            "Imprint the two solids in the CAD tool so they share a surface, then export again.",
                        )
                    };
                    Finding::new(CHECK, format!("{la} and {lb}: {what}"), why, next)
                        .requirement("GEO-021")
                        .cells(vec![la, lb])
                }
                Kind::DuplicateSurface => {
                    let volumes: Vec<u32> = [key.a, key.b]
                        .iter()
                        .flat_map(|&s| {
                            let info = &mesh.surfaces[s as usize];
                            [info.forward, info.reverse]
                        })
                        .filter(|&v| v != 0)
                        .collect();
                    let cells = self.labels.set(&volumes);
                    Finding::new(
                        CHECK,
                        format!("{}: duplicate surface", cells.join(" and ")),
                        format!(
                            "{triangles}: two surface entities coincide but were not merged, so \
                             the same face exists twice"
                        ),
                        "Faces coincide but are not merged; imprint the solids in the CAD tool and export again.",
                    )
                    .requirement("DUPLICATE-SURFACE")
                    .cells(cells)
                }
            };
            findings.push(finding.at(at).waivable());
        }
        reported
    }

    /// A vertex of one volume strictly inside another: volumes whose surfaces never meet.
    fn containment(
        &self,
        caster: &Caster,
        reported: &[(u32, u32)],
        findings: &mut Vec<Finding>,
    ) -> Result<(), Cancelled> {
        let mesh = self.mesh();
        let per_volume = mesh.volume_triangles();
        let n = per_volume.len();
        let found = self.ctl.run("containment", n, 1, |range| {
            let mut scratch = caster.scratch();
            let mut out: Vec<(u32, Result<PointClass, ()>, P3)> = Vec::new();
            for i in range {
                let Some(&t) = per_volume[i].first() else {
                    continue;
                };
                let vertex = mesh.tri_pts(t)[0];
                let class = caster
                    .classify(
                        vertex,
                        Rng::for_item(self.config.seed ^ 0xC0, i as u64).next_u64(),
                        &mut scratch,
                    )
                    .map_err(|_| ());
                out.push((i as u32 + 1, class, vertex));
            }
            out
        })?;
        let mut seen: Vec<(u32, u32)> = reported.to_vec();
        for (v, class, vertex) in found.into_iter().flatten() {
            let label = self.labels.of(v);
            match class {
                Err(()) => findings.push(
                    Finding::new(
                        CHECK,
                        format!("{label}: containment undecided"),
                        format!(
                            "no clean ray was found in {MAX_REDRAWS} tries from a vertex of the solid, \
                             so FARIS cannot say whether another solid contains it"
                        ),
                        "Simplify the geometry near the marked point or change the faceting tolerance.",
                    )
                    .requirement("GEO-021")
                    .cells(vec![label])
                    .at(to_metres(vertex)),
                ),
                Ok(class) => {
                    for &other in class.volumes.iter().filter(|&&o| o != v) {
                        let pair = (v.min(other), v.max(other));
                        if seen.contains(&pair) {
                            continue;
                        }
                        seen.push(pair);
                        let lo = self.labels.of(other);
                        findings.push(
                            Finding::new(
                                CHECK,
                                format!("{label} and {lo}: containment"),
                                format!(
                                    "a vertex of {label} lies strictly inside {lo}, and their \
                                     surfaces never meet: one solid sits wholly inside the other"
                                ),
                                "Cut the inner solid out of the outer one in the CAD tool (or remove it), then export again.",
                            )
                            .requirement("GEO-021")
                            .cells(vec![label.clone(), lo])
                            .at(to_metres(vertex))
                            .waivable(),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// GEO-022: cavities, from the components of the surfaces that face the complement.
    fn gaps(
        &self,
        caster: &Caster,
        components: &[Component],
        record: &mut IntegrityRecord,
        findings: &mut Vec<Finding>,
    ) -> Result<Vec<Cavity>, Cancelled> {
        let mesh = self.mesh();
        if components.is_empty() {
            return Ok(Vec::new());
        }
        // Which components contain each component's vertex.
        let classes = self.ctl.run("gap nesting", components.len(), 1, |range| {
            let mut scratch = caster.scratch();
            range
                .map(|c| {
                    let seed = Rng::for_item(self.config.seed ^ 0x6A9, c as u64).next_u64();
                    let class = caster.classify(components[c].vertex, seed, &mut scratch);
                    class.map(|k| {
                        k.components
                            .into_iter()
                            .filter(|&d| d as usize != c)
                            .collect::<Vec<u32>>()
                    })
                })
                .collect::<Vec<_>>()
        })?;
        let containing: Vec<Result<Vec<u32>, ()>> = classes
            .into_iter()
            .flatten()
            .map(|r| r.map_err(|_| ()))
            .collect();
        let depth: Vec<usize> = containing
            .iter()
            .map(|c| c.as_ref().map_or(0, Vec::len))
            .collect();
        let parent = |c: usize| -> Option<u32> {
            containing[c]
                .as_ref()
                .ok()?
                .iter()
                .copied()
                .max_by_key(|&d| depth[d as usize])
        };
        let negative: Vec<usize> = (0..components.len())
            .filter(|&c| components[c].six_volume < 0)
            .collect();
        let mut cavities: Vec<Cavity> = Vec::new();
        for &n in &negative {
            let mut six = -components[n].six_volume;
            for (p, comp) in components.iter().enumerate() {
                if comp.six_volume > 0 && parent(p) == Some(n as u32) {
                    six -= comp.six_volume;
                }
            }
            cavities.push(Cavity {
                component: n as u32,
                volume_m3: six as f64 / 6.0 / GRID3_PER_M3,
                interior: None,
                depth: depth[n],
            });
        }
        // An interior point of each cavity: seeded draws, verified exactly.
        let points = self.ctl.run("cavity points", cavities.len(), 1, |range| {
            let mut scratch = caster.scratch();
            range
                .map(|i| {
                    let comp = &components[cavities[i].component as usize];
                    let mut rng = Rng::for_item(self.config.seed ^ 0x6AA, i as u64);
                    for _ in 0..INTERIOR_TRIES {
                        let p = [
                            rng.range_i64(comp.lo[0], comp.hi[0]),
                            rng.range_i64(comp.lo[1], comp.hi[1]),
                            rng.range_i64(comp.lo[2], comp.hi[2]),
                        ];
                        let Ok(class) = caster.classify(p, rng.next_u64(), &mut scratch) else {
                            continue;
                        };
                        if class.volumes.is_empty()
                            && class.on_surface_of.is_empty()
                            && class
                                .components
                                .iter()
                                .copied()
                                .max_by_key(|&d| depth[d as usize])
                                == Some(cavities[i].component)
                        {
                            return Some(p);
                        }
                    }
                    None
                })
                .collect::<Vec<_>>()
        })?;
        for (cavity, point) in cavities.iter_mut().zip(points.into_iter().flatten()) {
            cavity.interior = point;
        }
        for (c, undecided) in containing.iter().enumerate() {
            if undecided.is_err() {
                findings.push(
                    Finding::new(
                        CHECK,
                        "gap nesting undecided",
                        format!(
                            "no clean ray was found in {MAX_REDRAWS} tries from a vertex of boundary \
                             component {c}, so FARIS cannot place it among the others"
                        ),
                        "Simplify the geometry near the marked point.",
                    )
                    .requirement("GEO-022")
                    .at(to_metres(components[c].vertex)),
                );
            }
        }
        let mut gap_findings = Vec::new();
        for cavity in &cavities {
            let comp = &components[cavity.component as usize];
            let walls = self.labels.set(&comp.volumes);
            let (lo, hi) = (to_metres(comp.lo), to_metres(comp.hi));
            let mid = [
                (lo[0] + hi[0]) / 2.0,
                (lo[1] + hi[1]) / 2.0,
                (lo[2] + hi[2]) / 2.0,
            ];
            let location = cavity.interior.map(to_metres).unwrap_or(mid);
            record.cavities.push(CavityRecord {
                wall_volumes: walls.clone(),
                volume_m3: cavity.volume_m3,
                bbox_lo_m: lo,
                bbox_hi_m: hi,
                interior_point_m: cavity.interior.map(to_metres),
                sampled_points: 0,
            });
            gap_findings.push(
                Finding::new(
                    CHECK,
                    format!("cavity bounded by {}", walls.join(", ")),
                    format!(
                        "an enclosed region of {:.6e} m3 (box {} to {}) belongs to no solid; the \
                         implicit complement fills it, so transport there has no material{}",
                        cavity.volume_m3,
                        fmt_m(lo),
                        fmt_m(hi),
                        if cavity.interior.is_none() {
                            "; no interior point was found, the box centre is shown"
                        } else {
                            ""
                        }
                    ),
                    "Model the space as a vacuum solid, or close the gap in the CAD tool, then export again.",
                )
                .requirement("GEO-022")
                .cells(walls)
                .at(location)
                .waivable(),
            );
        }
        findings.extend(gap_findings);
        let _ = mesh;
        Ok(cavities)
    }

    /// One verified interior point per volume.
    fn interior_points(
        &self,
        caster: &Caster,
        record: &mut IntegrityRecord,
    ) -> Result<(), Cancelled> {
        let mesh = self.mesh();
        let per_volume = mesh.volume_triangles();
        let boxes: Vec<(P3, P3)> = per_volume
            .iter()
            .map(|tris| {
                let mut lo = [i64::MAX; 3];
                let mut hi = [i64::MIN; 3];
                for &t in tris {
                    let (l, h) = self.model.bvh.triangle_box(t);
                    for k in 0..3 {
                        lo[k] = lo[k].min(l[k]);
                        hi[k] = hi[k].max(h[k]);
                    }
                }
                (lo, hi)
            })
            .collect();
        let found = self
            .ctl
            .run("interior points", per_volume.len(), 1, |range| {
                let mut scratch = caster.scratch();
                range
                    .map(|i| {
                        let v = i as u32 + 1;
                        let (lo, hi) = boxes[i];
                        if per_volume[i].is_empty() {
                            return None;
                        }
                        let mut rng = Rng::for_item(self.config.seed ^ 0x1717, i as u64);
                        let mut fallback = None;
                        for attempt in 0..INTERIOR_TRIES {
                            let p = [
                                rng.range_i64(lo[0], hi[0]),
                                rng.range_i64(lo[1], hi[1]),
                                rng.range_i64(lo[2], hi[2]),
                            ];
                            let Ok(class) = caster.classify(p, rng.next_u64(), &mut scratch) else {
                                continue;
                            };
                            if class.volumes.contains(&v) {
                                if class.volumes == [v] {
                                    return Some(p);
                                }
                                fallback.get_or_insert(p);
                                if attempt > INTERIOR_TRIES / 10 {
                                    break;
                                }
                            }
                        }
                        fallback
                    })
                    .collect::<Vec<_>>()
            })?;
        for (row, point) in record.volumes.iter_mut().zip(found.into_iter().flatten()) {
            row.interior_point_m = point.map(to_metres);
        }
        Ok(())
    }
}

/// What one chunk of sample points found.
#[derive(Default)]
struct SampleChunk {
    per_volume: Vec<u64>,
    per_cavity: BTreeMap<u32, (u64, P3)>,
    multi: BTreeMap<Vec<u32>, (u64, P3)>,
    assembly: u64,
    outside: u64,
    undecided: u64,
    first_undecided: Option<P3>,
    max_redraws: u32,
}

fn note(map: &mut BTreeMap<Vec<u32>, (u64, P3)>, key: Vec<u32>, p: P3) {
    let e = map.entry(key).or_insert((0, p));
    e.0 += 1;
}

impl Run<'_> {
    /// GEO-024 and GEO-031: uniform points in the box, classified by exact ray parity.
    fn sampling(
        &self,
        caster: &Caster,
        components: &[Component],
        cavities: &[Cavity],
        record: &mut IntegrityRecord,
        findings: &mut Vec<Finding>,
    ) -> Result<(), Cancelled> {
        let mesh = self.mesh();
        let n = self.config.samples;
        let n_volumes = mesh.n_volumes as usize;
        let depth: BTreeMap<u32, usize> = components
            .iter()
            .enumerate()
            .map(|(c, _)| {
                let d = cavities
                    .iter()
                    .find(|k| k.component == c as u32)
                    .map_or(0, |k| k.depth);
                (c as u32, d)
            })
            .collect();
        let chunks = self.ctl.run("point sampling", n, 2048, |range| {
            let mut scratch = caster.scratch();
            let mut chunk = SampleChunk {
                per_volume: vec![0; n_volumes],
                ..SampleChunk::default()
            };
            for i in range {
                let mut rng = Rng::for_item(self.config.seed, i as u64);
                let p = [
                    rng.range_i64(mesh.lo[0], mesh.hi[0]),
                    rng.range_i64(mesh.lo[1], mesh.hi[1]),
                    rng.range_i64(mesh.lo[2], mesh.hi[2]),
                ];
                let Ok(class) = caster.classify(p, rng.next_u64(), &mut scratch) else {
                    chunk.undecided += 1;
                    chunk.first_undecided.get_or_insert(p);
                    continue;
                };
                chunk.max_redraws = chunk.max_redraws.max(class.redraws);
                for &v in &class.volumes {
                    chunk.per_volume[v as usize - 1] += 1;
                }
                if class.volumes.len() >= 2 {
                    note(&mut chunk.multi, class.volumes.clone(), p);
                }
                if class.volumes.is_empty() && !class.components.is_empty() {
                    let deepest = class
                        .components
                        .iter()
                        .copied()
                        .max_by_key(|c| depth.get(c).copied().unwrap_or(0))
                        .unwrap_or(0);
                    let e = chunk.per_cavity.entry(deepest).or_insert((0, p));
                    e.0 += 1;
                    chunk.assembly += 1;
                } else if class.volumes.is_empty() {
                    chunk.outside += 1;
                } else {
                    chunk.assembly += 1;
                }
            }
            chunk
        })?;
        let mut total = SampleChunk {
            per_volume: vec![0; n_volumes],
            ..SampleChunk::default()
        };
        // Merge in chunk order, so "first" is the same on every run.
        for c in chunks {
            for (a, b) in total.per_volume.iter_mut().zip(c.per_volume) {
                *a += b;
            }
            for (k, (count, p)) in c.per_cavity {
                let e = total.per_cavity.entry(k).or_insert((0, p));
                e.0 += count;
            }
            for (k, (count, p)) in c.multi {
                let e = total.multi.entry(k).or_insert((0, p));
                e.0 += count;
            }
            total.assembly += c.assembly;
            total.outside += c.outside;
            total.undecided += c.undecided;
            if total.first_undecided.is_none() {
                total.first_undecided = c.first_undecided;
            }
            total.max_redraws = total.max_redraws.max(c.max_redraws);
        }
        let box_volume = mesh.bbox_volume_m3();
        let nf = n as f64;
        let standard_error = |count: u64| {
            let p = count as f64 / nf;
            box_volume * (p * (1.0 - p) / nf).sqrt()
        };
        for (row, &count) in record.volumes.iter_mut().zip(&total.per_volume) {
            row.sampled_points = count;
            row.sampled_volume_m3 = box_volume * count as f64 / nf;
            row.sampled_standard_error_m3 = standard_error(count);
        }
        record.sampling = SamplingRecord {
            points: n as u64,
            seed: self.config.seed,
            outside: total.outside,
            in_two_or_more: total.multi.values().map(|m| m.0).sum(),
            in_cavities: total.per_cavity.values().map(|m| m.0).sum(),
            undecided: total.undecided,
            max_redraws: total.max_redraws,
            box_volume_m3: box_volume,
        };
        for (set, (count, p)) in &total.multi {
            let cells = self.labels.set(set);
            findings.push(
                Finding::new(
                    CHECK,
                    format!("{}: sampled points in several solids", cells.join(" and ")),
                    format!(
                        "{count} of {n} uniform points lie inside all of these solids at once, so \
                         they overlap (about {:.3e} m3 by sampling)",
                        box_volume * *count as f64 / nf
                    ),
                    "Remove the overlap in the CAD tool (cut one solid with the other), then export again.",
                )
                .requirement("GEO-024")
                .also_covered_by(&["GEO-021", "SELF-INTERSECTION"])
                .cells(cells)
                .at(to_metres(*p))
                .waivable(),
            );
        }
        for (c, (count, p)) in &total.per_cavity {
            let cavity = cavities.iter().position(|k| k.component == *c);
            if let Some(i) = cavity.and_then(|i| record.cavities.get_mut(i)) {
                i.sampled_points = *count;
            }
            let (cells, at) = match cavity {
                Some(i) => (
                    record.cavities[i].wall_volumes.clone(),
                    record.cavities[i].interior_point_m.unwrap_or(to_metres(*p)),
                ),
                None => (
                    self.labels.set(&components[*c as usize].volumes),
                    to_metres(*p),
                ),
            };
            findings.push(
                Finding::new(
                    CHECK,
                    format!("cavity bounded by {}: sampled points", cells.join(", ")),
                    format!(
                        "{count} of {n} uniform points lie in an enclosed region that belongs to no solid"
                    ),
                    "Model the space as a vacuum solid, or close the gap in the CAD tool, then export again.",
                )
                .requirement("GEO-024")
                .also_covered_by(&["GEO-022"])
                .cells(cells)
                .at(at)
                .waivable(),
            );
        }
        if total.undecided > 0 {
            let mut f = Finding::new(
                CHECK,
                "sampled points undecided",
                format!(
                    "{} of {n} points had no clean ray in {MAX_REDRAWS} tries (every direction met an edge or vertex)",
                    total.undecided
                ),
                "Simplify the geometry near the marked point or change the faceting tolerance.",
            )
            .requirement("GEO-024");
            if let Some(p) = total.first_undecided {
                f = f.at(to_metres(p));
            }
            findings.push(f);
        }
        for row in &record.volumes {
            if row.faceted_volume_m3 > 0.0 && row.interior_point_m.is_none() && n > 0 {
                findings.push(
                    Finding::new(
                        CHECK,
                        format!("{}: no interior point", row.solid_id),
                        format!(
                            "no point verified strictly inside the solid was found in {INTERIOR_TRIES} draws"
                        ),
                        "The solid is too thin or too convoluted to sample; thicken it or waive it.",
                    )
                    .requirement("GEO-024")
                    .cells(vec![row.solid_id.clone()])
                    .waivable(),
                );
            }
        }

        // GEO-031: the faceted volumes and the cavities fill the assembly.
        let faceted: f64 = record.volumes.iter().map(|v| v.faceted_volume_m3).sum();
        let cavity_sum: f64 = cavities.iter().map(|c| c.volume_m3).sum::<f64>() + 0.0;
        let sampled = box_volume * total.assembly as f64 / nf;
        let se = standard_error(total.assembly).max(box_volume / nf);
        let sigmas = ((faceted + cavity_sum) - sampled).abs() / se;
        record.partition = Some(PartitionRecord {
            faceted_sum_m3: faceted,
            cavity_sum_m3: cavity_sum,
            sampled_assembly_m3: sampled,
            standard_error_m3: se,
            sigmas,
        });
        if sigmas > 3.0 {
            findings.push(
                Finding::new(
                    CHECK,
                    "partition of the assembly",
                    format!(
                        "the faceted volumes ({faceted:.6e} m3) plus the cavities ({cavity_sum:.6e} m3) \
                         differ from the sampled assembly volume ({sampled:.6e} m3) by {sigmas:.1} \
                         standard errors ({se:.3e} m3); the solids overlap or leave gaps"
                    ),
                    "Resolve the overlaps and gaps reported above in the CAD tool, then export again.",
                )
                .requirement("GEO-031")
                .cells(record.volumes.iter().map(|v| v.solid_id.clone()).collect())
                .waivable(),
            );
        }
        Ok(())
    }

    /// GEO-025: random chords; every crossing must leave the region the chord is in.
    fn chords(
        &self,
        caster: &Caster,
        record: &mut IntegrityRecord,
        findings: &mut Vec<Finding>,
    ) -> Result<(), Cancelled> {
        let mesh = self.mesh();
        let n = self.config.chords;
        let chunks = self.ctl.run("ray consistency", n, 512, |range| {
            let mut scratch = caster.scratch();
            let mut out = ChordChunk::default();
            for i in range {
                let mut rng = Rng::for_item(self.config.seed ^ 0xC40D, i as u64);
                let mut walked = false;
                for _ in 0..MAX_REDRAWS {
                    let p = [
                        rng.range_i64(mesh.lo[0], mesh.hi[0]),
                        rng.range_i64(mesh.lo[1], mesh.hi[1]),
                        rng.range_i64(mesh.lo[2], mesh.hi[2]),
                    ];
                    let d = rng.unit_vector();
                    let s = caster.far_point(p, [-d[0], -d[1], -d[2]]);
                    let e = caster.far_point(p, d);
                    if caster.chord(s, e, &mut scratch).is_err() {
                        continue;
                    }
                    walked = true;
                    out.crossings += scratch.hits.len() as u64;
                    if let Some((key, at)) = self.walk(&scratch.hits, s, e) {
                        out.inconsistent += 1;
                        let entry = out.cases.entry(key).or_insert((0, at));
                        entry.0 += 1;
                    }
                    break;
                }
                if !walked {
                    out.undecided += 1;
                }
            }
            out
        })?;
        let mut total = ChordChunk::default();
        for c in chunks {
            total.crossings += c.crossings;
            total.inconsistent += c.inconsistent;
            total.undecided += c.undecided;
            for (k, (count, at)) in c.cases {
                let e = total.cases.entry(k).or_insert((0, at));
                e.0 += count;
            }
        }
        record.chords = ChordRecord {
            chords: n as u64,
            crossings: total.crossings,
            inconsistent: total.inconsistent,
            undecided: total.undecided,
        };
        for (set, (count, at)) in &total.cases {
            let cells = self.labels.set(set);
            findings.push(
                Finding::new(
                    CHECK,
                    format!("{}: inconsistent crossing", cells.join(" and ")),
                    format!(
                        "{count} of {n} random chords cross a surface that does not lead out of the \
                         region the chord is in; a particle there would be lost (first at the marked point)"
                    ),
                    "Fix the overlap, gap or orientation reported for these solids in the CAD tool, then export again.",
                )
                .requirement("GEO-025")
                .also_covered_by(if set.len() >= 2 {
                    &["GEO-021", "GEO-022"]
                } else {
                    &["SELF-INTERSECTION", "GEO-022"]
                })
                .cells(cells)
                .at(*at)
                .waivable(),
            );
        }
        if total.undecided > 0 {
            findings.push(
                Finding::new(
                    CHECK,
                    "chords undecided",
                    format!(
                        "{} of {n} chords met an edge or vertex in {MAX_REDRAWS} draws in a row",
                        total.undecided
                    ),
                    "Simplify the geometry or change the faceting tolerance.",
                )
                .requirement("GEO-025"),
            );
        }
        Ok(())
    }

    /// Walk a chord's crossings in order. The first inconsistency returns the volumes involved
    /// and the place.
    fn walk(&self, hits: &[Hit], s: P3, e: P3) -> Option<(Vec<u32>, [f64; 3])> {
        let mesh = self.mesh();
        let mut state = 0u32;
        for h in hits {
            let surface = mesh.surface_of(h.tri);
            let (leaving, entering) = if h.towards_normal {
                (surface.forward, surface.reverse)
            } else {
                (surface.reverse, surface.forward)
            };
            if leaving != state {
                let mut key: Vec<u32> = [state, leaving, entering]
                    .into_iter()
                    .filter(|&v| v != 0)
                    .collect();
                key.sort_unstable();
                key.dedup();
                return Some((key, Caster::chord_point_m(s, e, h.t)));
            }
            state = entering;
        }
        if state != 0 {
            return Some((vec![state], to_metres(e)));
        }
        None
    }
}

#[derive(Default)]
struct ChordChunk {
    crossings: u64,
    inconsistent: u64,
    undecided: u64,
    cases: BTreeMap<Vec<u32>, (u64, [f64; 3])>,
}

#[cfg(test)]
mod tests {
    use super::super::mesh::fixtures::*;
    use super::*;
    use crate::jobs::Cancellation;
    use faris_model::design::Facets;

    fn run(f: &Facets, chord_m: f64) -> IntegrityResult {
        let model = Model::new(f).unwrap();
        let labels: Vec<String> = (1..=f.n_volumes).map(|i| format!("s{i}")).collect();
        let cancel = Cancellation::default();
        let progress = |_: &str, _: usize, _: usize| {};
        let mut config = Config::new(chord_m, 7);
        config.samples = 40_000;
        config.chords = 4_000;
        check_integrity(&model, &labels, &config, &Ctl::new(&cancel, &progress, 2)).unwrap()
    }

    fn ids(r: &IntegrityResult) -> Vec<String> {
        r.findings
            .iter()
            .map(|f| f.requirement.clone().unwrap_or_default())
            .collect()
    }

    fn has(r: &IntegrityResult, id: &str) -> bool {
        r.findings
            .iter()
            .any(|f| f.requirement.as_deref() == Some(id))
    }

    const CHORD: f64 = 1.0e-3;

    // Verifies: GEO-020, GEO-021, GEO-022, GEO-024, GEO-025, GEO-031
    #[test]
    fn clean_models_have_no_findings() {
        for f in [
            separate_boxes(&[([0.0; 3], [1.0; 3]), ([2.0; 3], [3.5; 3])]),
            shell_and_core(10.0, 2.0),
        ] {
            let r = run(&f, CHORD);
            assert!(r.findings.is_empty(), "{:#?}", r.findings);
            assert!(r.record.skipped.is_empty());
            assert_eq!(r.record.sampling.undecided, 0);
            assert_eq!(r.record.chords.inconsistent, 0);
            assert!(
                r.record
                    .volumes
                    .iter()
                    .all(|v| v.interior_point_m.is_some())
            );
            let p = r.record.partition.as_ref().unwrap();
            assert!(p.sigmas <= 3.0, "{p:?}");
        }
    }

    // Verifies: GEO-024
    #[test]
    fn sampled_volumes_agree_with_exact_volumes() {
        let r = run(&shell_and_core(10.0, 2.0), CHORD);
        for v in &r.record.volumes {
            let diff = (v.sampled_volume_m3 - v.faceted_volume_m3).abs();
            assert!(diff <= 4.0 * v.sampled_standard_error_m3 + 1e-9, "{v:?}");
        }
    }

    // Verifies: GEO-020
    #[test]
    fn an_open_edge_blocks_and_skips_the_closed_surface_stages() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        f.triangles.remove(3);
        f.surfaces[0].triangle_count -= 1;
        let r = run(&f, CHORD);
        let open = r
            .findings
            .iter()
            .find(|f| f.item.contains("open edges"))
            .unwrap();
        assert_eq!(open.requirement.as_deref(), Some("GEO-020"));
        assert!(!open.waivable && open.location_m.is_some());
        assert_eq!(open.cells, vec!["s1"]);
        assert!(!r.record.skipped.is_empty());
    }

    // Verifies: GEO-020
    #[test]
    fn a_duplicated_triangle_is_a_non_manifold_edge() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        let t = f.triangles[0];
        f.triangles.push(t);
        f.surfaces[0].triangle_count += 1;
        let r = run(&f, CHORD);
        assert!(r.findings.iter().any(|f| f.item.contains("non-manifold")));
    }

    #[test]
    fn inverted_normals_block_and_cannot_be_waived() {
        let mut f = separate_boxes(&[([0.0; 3], [1.0; 3])]);
        for t in &mut f.triangles {
            t.swap(1, 2);
        }
        let r = run(&f, CHORD);
        let finding = r
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("INVERTED-NORMALS"))
            .unwrap();
        assert!(!finding.waivable);
        assert!(r.record.volumes[0].faceted_volume_m3 < 0.0);
    }

    #[test]
    fn a_zero_volume_cell_blocks() {
        let r = run(
            &separate_boxes(&[
                ([0.0; 3], [1.0; 3]),
                ([2.0, 0.0, 0.0], [2.0 + 1e-12, 1.0, 1.0]),
            ]),
            CHORD,
        );
        assert!(has(&r, "ZERO-VOLUME"), "{:?}", ids(&r));
    }

    #[test]
    fn a_sliver_blocks_unless_waived_and_reports_its_thickness() {
        let r = run(
            &separate_boxes(&[([0.0; 3], [1.0; 3]), ([2.0, 0.0, 0.0], [3.0, 1.0, 0.004])]),
            CHORD,
        );
        let sliver = r
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("SLIVER"))
            .unwrap();
        assert!(sliver.waivable && sliver.cells == vec!["s2"]);
        let t = r.record.volumes[1].mean_thickness_m.unwrap();
        assert!((t - 0.004).abs() < 2e-4, "{t}");
        assert!(r.record.volumes[0].mean_thickness_m.unwrap() > 0.3);
    }

    // Verifies: GEO-021
    #[test]
    fn overlapping_solids_are_found_by_triangles_sampling_and_chords() {
        let r = run(
            &separate_boxes(&[([0.0; 3], [2.0; 3]), ([0.7, 0.5, 0.3], [2.9, 2.6, 2.2])]),
            CHORD,
        );
        assert!(r.findings.iter().any(|f| f.item.contains("overlap")));
        assert!(has(&r, "GEO-024"));
        assert!(has(&r, "GEO-025"));
        assert!(has(&r, "GEO-031"));
        let overlap = r
            .findings
            .iter()
            .find(|f| f.item.contains("overlap"))
            .unwrap();
        assert_eq!(overlap.cells, vec!["s1", "s2"]);
        assert!(overlap.waivable);
    }

    // Verifies: GEO-021
    #[test]
    fn a_solid_wholly_inside_another_is_contained() {
        let r = run(
            &separate_boxes(&[([0.0; 3], [10.0; 3]), ([4.0; 3], [5.0; 3])]),
            CHORD,
        );
        let c = r
            .findings
            .iter()
            .find(|f| f.item.contains("containment"))
            .unwrap();
        assert_eq!(c.requirement.as_deref(), Some("GEO-021"));
        assert_eq!(c.cells, vec!["s1", "s2"]);
    }

    // Verifies: GEO-021
    #[test]
    fn a_duplicate_surface_is_named_as_such() {
        let r = run(
            &separate_boxes(&[([0.0; 3], [1.0; 3]), ([1.0, 0.0, 0.0], [2.0, 1.0, 1.0])]),
            CHORD,
        );
        let d = r
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("DUPLICATE-SURFACE"))
            .unwrap();
        assert!(d.next_step.contains("imprint"));
    }

    // Verifies: GEO-021
    #[test]
    fn a_vertex_through_the_opposite_face_is_a_self_intersection() {
        let mut f = separate_boxes(&[([0.0; 3], [2.0; 3])]);
        f.vertices[6] = [-1.0, 0.5, 0.5];
        let r = run(&f, CHORD);
        assert!(has(&r, "SELF-INTERSECTION"), "{:?}", ids(&r));
    }

    // Verifies: GEO-022, GEO-024
    #[test]
    fn a_hollow_inside_a_shell_is_a_cavity_with_a_verified_interior_point() {
        let mut f = shell_and_core(10.0, 2.0);
        f.surfaces[1].forward_volume = 0;
        f.surfaces[1].reverse_volume = 1;
        f.n_volumes = 1;
        let r = run(&f, CHORD);
        let gap = r
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("GEO-022"))
            .unwrap();
        assert!(gap.waivable && gap.cells == vec!["s1"]);
        let p = gap.location_m.unwrap();
        assert!(p.iter().all(|c| c.abs() < 2.0), "{p:?}");
        assert_eq!(r.record.cavities.len(), 1);
        assert!((r.record.cavities[0].volume_m3 - 64.0).abs() < 1e-6);
        // the sampling finding for the same cavity is covered by the same waiver
        let sampled = r
            .findings
            .iter()
            .find(|f| f.requirement.as_deref() == Some("GEO-024"))
            .unwrap();
        assert_eq!(sampled.location_m, gap.location_m);
        assert!(sampled.also_covered_by.contains(&"GEO-022".to_string()));
    }

    // Verifies: GEO-022
    #[test]
    fn a_cavity_is_found_however_small() {
        let mut f = shell_and_core(10.0, 0.01);
        f.surfaces[1].forward_volume = 0;
        f.surfaces[1].reverse_volume = 1;
        f.n_volumes = 1;
        let r = run(&f, CHORD);
        // 40 000 samples cannot hit a 8e-6 m3 cavity; the topology finds it anyway
        assert_eq!(r.record.sampling.in_cavities, 0);
        assert!(has(&r, "GEO-022"));
    }

    // Verifies: GEO-022
    #[test]
    fn a_cluster_nested_in_a_cavity_is_not_counted_as_cavity_volume() {
        // a cavity of 20^3... with a free-floating solid in it: cavity volume excludes the solid
        let mut f = shell_and_core(10.0, 4.0);
        f.surfaces[1].forward_volume = 0;
        f.surfaces[1].reverse_volume = 1;
        let (v, t) = box_triangles([-1.0; 3], [1.0; 3], f.vertices.len() as u32);
        f.vertices.extend(v);
        let first = f.triangles.len() as u64;
        f.triangles.extend(t);
        f.surfaces.push(faris_model::design::Surface {
            first_triangle: first,
            triangle_count: 12,
            forward_volume: 2,
            reverse_volume: 0,
        });
        let r = run(&f, CHORD);
        assert_eq!(r.record.cavities.len(), 1);
        let expected = 8.0 * 8.0 * 8.0 - 2.0 * 2.0 * 2.0;
        assert!(
            (r.record.cavities[0].volume_m3 - expected).abs() < 1e-6,
            "{:?}",
            r.record.cavities
        );
    }

    #[test]
    fn the_same_seed_gives_the_same_report() {
        let f = separate_boxes(&[([0.0; 3], [2.0; 3]), ([0.7, 0.5, 0.3], [2.9, 2.6, 2.2])]);
        let a = run(&f, CHORD);
        let b = run(&f, CHORD);
        assert_eq!(
            serde_json::to_string(&a.findings).unwrap(),
            serde_json::to_string(&b.findings).unwrap()
        );
    }

    #[test]
    fn cancel_stops_the_checks() {
        let f = shell_and_core(10.0, 2.0);
        let model = Model::new(&f).unwrap();
        let cancel = Cancellation::default();
        cancel.cancel();
        let progress = |_: &str, _: usize, _: usize| {};
        let labels = vec!["a".to_string(), "b".to_string()];
        let r = check_integrity(
            &model,
            &labels,
            &Config::new(CHORD, 1),
            &Ctl::new(&cancel, &progress, 2),
        );
        assert!(r.is_err());
    }

    #[test]
    fn more_findings_than_the_cap_leave_a_blocking_summary() {
        let list: Vec<Finding> = (0..LIST_CAP + 5)
            .map(|i| {
                Finding::new(8, format!("f{i}"), "w", "n")
                    .requirement("GEO-021")
                    .waivable()
            })
            .collect();
        let out = capped(list, "GEO-021", "GEO-021");
        assert_eq!(out.len(), LIST_CAP + 1);
        assert!(!out.last().unwrap().waivable);
    }
}
