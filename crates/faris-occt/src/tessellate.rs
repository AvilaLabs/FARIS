//! `tessellate` in Rust: every solid meshed on its own with `BRepMesh_IncrementalMesh`, nodes
//! welded by position, closedness checked per solid, and the result written as
//! `faris-facets/v1` with the unimprinted flag. The rules are those of `tessellate` in
//! `integrations/cad/faris_cad.py`.

use crate::inspect::{HELPER_VERSION, INTERNAL_UNIT, sha256_file, versions};
use crate::native::StepModel;
use faris_model::design::{
    FACETS_SCHEMA, FLAG_UNIMPRINTED, Facets, FacetsManifest, ManifestCounts, ManifestSource,
    ManifestSurface, ManifestVolume, Surface, sha256_hex,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path, time::Instant};

pub const TESSELLATION_SCHEMA: &str = "faris-tessellation/v1";
/// Nodes closer than this (shape units) are one vertex.
pub const WELD: f64 = 1.0e-6;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tag {
    pub step_index: usize,
    pub tag: String,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub chord_cm: f64,
    pub angular: f64,
    /// Metres per shape unit (0.001 for a file with a declared unit).
    pub shape_unit_m: f64,
    /// One material tag per solid; default `solid-NNN`.
    pub tags: Option<Vec<Tag>>,
}

/// One solid's mesh, welded, in metres, with its faces' triangle runs.
#[derive(Clone, Debug)]
pub struct SolidMesh {
    pub vertices: Vec<[f64; 3]>,
    /// (face id, triangles) with indices into `vertices`.
    pub faces: Vec<(u32, Vec<[u32; 3]>)>,
    pub missing_faces: Vec<u32>,
    pub collapsed_triangles: usize,
    pub mesh_seconds: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolidResult {
    pub step_index: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub faces: usize,
    pub closed: bool,
    pub unpaired_edges: usize,
    pub repeated_edges: usize,
    pub collapsed_triangles: usize,
    pub faceted_volume_m3: f64,
    pub mesh_seconds: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub versions: crate::inspect::Versions,
    pub method: String,
    pub tolerances: BTreeMap<String, f64>,
    pub timing_seconds: BTreeMap<String, f64>,
    pub peak_rss_mb: f64,
    pub facets: ReportFacets,
    pub counts: ManifestCounts,
    pub solids: Vec<SolidResult>,
    pub problems: Vec<String>,
    pub collapsed_triangles: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportFacets {
    pub bin: String,
    pub json: String,
    pub bin_sha256: String,
}

/// Peak resident set of this process in MiB (`VmHWM`), 0 where unavailable.
pub fn peak_rss_mb() -> f64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                let rest = l.strip_prefix("VmHWM:")?;
                rest.split_whitespace().next()?.parse::<f64>().ok()
            })
        })
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

/// Meshes one solid and welds its nodes by position (rounded to `WELD`, ties to even, keys
/// ordered lexicographically, first node of a key kept), dropping triangles that collapse.
pub fn mesh_welded(
    model: &StepModel,
    index: usize,
    chord: f64,
    angular: f64,
) -> Result<SolidMesh, String> {
    let started = Instant::now();
    let meshed = model.mesh_solid(index, chord, angular)?;
    let mut nodes: Vec<[f64; 3]> = Vec::new();
    let mut faces: Vec<(u32, Vec<[usize; 3]>)> = Vec::new();
    for face in &meshed.faces {
        let offset = nodes.len();
        nodes.extend(face.nodes.as_chunks::<3>().0.iter().copied());
        faces.push((
            face.face_id,
            face.triangles
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| {
                    [
                        t[0] as usize + offset,
                        t[1] as usize + offset,
                        t[2] as usize + offset,
                    ]
                })
                .collect(),
        ));
    }
    if faces.is_empty() {
        return Err("the solid has no meshed face".into());
    }
    let key = |p: &[f64; 3]| -> [i64; 3] {
        std::array::from_fn(|i| (p[i] / WELD).round_ties_even() as i64)
    };
    let keys: Vec<[i64; 3]> = nodes.iter().map(key).collect();
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    // A stable sort keeps the first occurrence of a key first.
    order.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
    let mut inverse = vec![0usize; nodes.len()];
    let mut vertices: Vec<[f64; 3]> = Vec::new();
    let mut previous: Option<[i64; 3]> = None;
    for &i in &order {
        if previous != Some(keys[i]) {
            vertices.push(nodes[i]);
            previous = Some(keys[i]);
        }
        inverse[i] = vertices.len() - 1;
    }
    let mut collapsed = 0usize;
    let mut welded: Vec<(u32, Vec<[u32; 3]>)> = Vec::new();
    for (number, triangles) in faces {
        let mut kept = Vec::with_capacity(triangles.len());
        for t in triangles {
            let (a, b, c) = (inverse[t[0]], inverse[t[1]], inverse[t[2]]);
            if a == b || b == c || a == c {
                collapsed += 1;
            } else {
                kept.push([a as u32, b as u32, c as u32]);
            }
        }
        welded.push((number, kept));
    }
    Ok(SolidMesh {
        vertices,
        faces: welded,
        missing_faces: meshed.missing_faces,
        collapsed_triangles: collapsed,
        mesh_seconds: started.elapsed().as_secs_f64(),
    })
}

/// (edges used once in their direction without a partner, edges used more than once in one
/// direction): a mesh is closed and consistently oriented when both are zero.
pub fn open_edge_report(triangles: &[[u32; 3]]) -> (usize, usize) {
    let width = triangles
        .iter()
        .flatten()
        .copied()
        .max()
        .map_or(1, |m| u64::from(m) + 1);
    let mut forward: Vec<u64> = Vec::with_capacity(triangles.len() * 3);
    let mut backward: Vec<u64> = Vec::with_capacity(triangles.len() * 3);
    for t in triangles {
        for k in 0..3 {
            let (a, b) = (u64::from(t[k]), u64::from(t[(k + 1) % 3]));
            forward.push(a * width + b);
            backward.push(b * width + a);
        }
    }
    let mut sorted = forward;
    sorted.sort_unstable();
    let mut repeated = 0usize;
    let mut run = 1usize;
    for i in 1..=sorted.len() {
        if i < sorted.len() && sorted[i] == sorted[i - 1] {
            run += 1;
        } else {
            if i > 0 && run > 1 {
                repeated += 1;
            }
            run = 1;
        }
    }
    let unpaired = backward
        .iter()
        .filter(|code| sorted.binary_search(code).is_err())
        .count();
    (unpaired, repeated)
}

/// Signed volume of a closed outward-oriented mesh (vertices in any unit).
pub fn signed_volume(vertices: &[[f64; 3]], faces: &[(u32, Vec<[u32; 3]>)]) -> f64 {
    let mut sum = 0.0;
    for (_, triangles) in faces {
        for t in triangles {
            let (p, q, r) = (
                vertices[t[0] as usize],
                vertices[t[1] as usize],
                vertices[t[2] as usize],
            );
            sum += p[0] * (q[1] * r[2] - q[2] * r[1]) - p[1] * (q[0] * r[2] - q[2] * r[0])
                + p[2] * (q[0] * r[1] - q[1] * r[0]);
        }
    }
    sum / 6.0
}

/// Meshes every solid of `step` and writes `<out_base>.facets.bin` and `.facets.json`.
/// Returns the report (also the caller's to store).
pub fn tessellate_step(step: &Path, out_base: &Path, options: &Options) -> Result<Report, String> {
    let started = Instant::now();
    let mut timing: BTreeMap<String, f64> = BTreeMap::new();
    let t = Instant::now();
    let model = StepModel::read(&step.display().to_string(), INTERNAL_UNIT)?;
    timing.insert("read_step".into(), t.elapsed().as_secs_f64());
    let count = model.solid_count();
    let mut tags = options.tags.clone().unwrap_or_else(|| {
        (0..count)
            .map(|i| Tag {
                step_index: i,
                tag: format!("solid-{:03}", i + 1),
            })
            .collect()
    });
    tags.sort_by_key(|e| e.step_index);
    if tags.len() != count || tags.iter().enumerate().any(|(i, e)| e.step_index != i) {
        return Err(format!(
            "the STEP file holds {count} solids; the tags must cover indices 0..{count} once each"
        ));
    }
    let scale_to_cm = options.shape_unit_m * 100.0;
    let chord = options.chord_cm / scale_to_cm; // shape units
    let t = Instant::now();
    let mut problems = Vec::new();
    let mut results = Vec::new();
    let mut collapsed_by_solid = BTreeMap::new();
    let mut vertices: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut surfaces: Vec<Surface> = Vec::new();
    let mut surface_ids: Vec<i64> = Vec::new();
    let mut per_volume = vec![0u64; count];
    for entry in &tags {
        let label = format!("solid {} ({})", entry.step_index, entry.tag);
        let mesh = match mesh_welded(&model, entry.step_index, chord, options.angular) {
            Ok(mesh) => mesh,
            Err(error) => {
                problems.push(format!("{label}: {error}"));
                continue;
            }
        };
        if let Some(first) = mesh.missing_faces.first() {
            problems.push(format!(
                "{label}: {} face(s) have no tessellation (first: face {first})",
                mesh.missing_faces.len()
            ));
        }
        let metres: Vec<[f64; 3]> = mesh
            .vertices
            .iter()
            .map(|p| {
                [
                    p[0] * options.shape_unit_m,
                    p[1] * options.shape_unit_m,
                    p[2] * options.shape_unit_m,
                ]
            })
            .collect();
        let all: Vec<[u32; 3]> = mesh
            .faces
            .iter()
            .flat_map(|(_, t)| t.iter().copied())
            .collect();
        let (unpaired, repeated) = if all.is_empty() {
            (0, 0)
        } else {
            open_edge_report(&all)
        };
        if unpaired > 0 || repeated > 0 {
            problems.push(format!(
                "{label}: the tessellation is not closed: {unpaired} edge(s) have no partner triangle on the other side and \
                 {repeated} edge(s) are used more than once in one direction"
            ));
        }
        if mesh.collapsed_triangles > 0 {
            collapsed_by_solid.insert(label.clone(), mesh.collapsed_triangles);
        }
        let volume = signed_volume(&metres, &mesh.faces);
        if volume <= 0.0 {
            problems.push(format!(
                "{label} has a faceted volume of {volume:e} (not positive), so its triangles are not oriented outward"
            ));
        }
        let offset = vertices.len() as u32;
        let volume_number = entry.step_index as u32 + 1;
        let mut face_count = 0usize;
        for (number, face_triangles) in &mesh.faces {
            if face_triangles.is_empty() {
                continue;
            }
            surfaces.push(Surface {
                first_triangle: triangles.len() as u64,
                triangle_count: face_triangles.len() as u64,
                forward_volume: volume_number,
                reverse_volume: 0,
            });
            surface_ids.push(i64::from(*number));
            per_volume[entry.step_index] += face_triangles.len() as u64;
            triangles.extend(
                face_triangles
                    .iter()
                    .map(|t| [t[0] + offset, t[1] + offset, t[2] + offset]),
            );
            face_count += 1;
        }
        results.push(SolidResult {
            step_index: entry.step_index,
            vertices: metres.len(),
            triangles: all.len(),
            faces: face_count,
            closed: unpaired == 0 && repeated == 0,
            unpaired_edges: unpaired,
            repeated_edges: repeated,
            collapsed_triangles: mesh.collapsed_triangles,
            faceted_volume_m3: volume,
            mesh_seconds: mesh.mesh_seconds,
        });
        vertices.extend(metres);
    }
    timing.insert("mesh".into(), t.elapsed().as_secs_f64());
    if surfaces.is_empty() {
        return Err("no solid could be meshed".into());
    }
    let facets = Facets {
        flags: FLAG_UNIMPRINTED,
        n_volumes: count as u32,
        vertices,
        triangles,
        surfaces,
    };
    let binary = facets.to_bytes();
    let counts = ManifestCounts {
        vertices: facets.vertices.len() as u64,
        triangles: facets.triangles.len() as u64,
        surfaces: facets.surfaces.len() as u64,
        volumes: count as u64,
    };
    let manifest = FacetsManifest {
        schema: FACETS_SCHEMA.into(),
        unit: "m".into(),
        unimprinted: true,
        counts: counts.clone(),
        source: ManifestSource {
            kind: "step".into(),
            sha256: sha256_file(step)?,
        },
        binary_sha256: sha256_hex(&binary),
        helper_version: HELPER_VERSION.into(),
        implicit_complement_tag: None,
        volumes: tags
            .iter()
            .map(|e| ManifestVolume {
                index: e.step_index as u32 + 1,
                global_id: e.step_index as i64 + 1,
                material_tag: Some(format!("mat:{}", e.tag)),
                triangles: per_volume[e.step_index],
            })
            .collect(),
        surfaces: surface_ids
            .iter()
            .enumerate()
            .map(|(i, g)| ManifestSurface {
                index: i as u32,
                global_id: *g,
            })
            .collect(),
    };
    let name = out_base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bin_path = out_base.with_file_name(format!("{name}.facets.bin"));
    let json_path = out_base.with_file_name(format!("{name}.facets.json"));
    fs::write(&bin_path, &binary).map_err(|e| format!("{}: {e}", bin_path.display()))?;
    let mut json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    json.push('\n');
    fs::write(&json_path, json).map_err(|e| format!("{}: {e}", json_path.display()))?;
    timing.insert("total".into(), started.elapsed().as_secs_f64());
    Ok(Report {
        schema: TESSELLATION_SCHEMA.into(),
        versions: versions(),
        method: "BRepMesh_IncrementalMesh on each solid alone, nodes welded by position, closedness checked per solid"
            .into(),
        tolerances: BTreeMap::from([
            ("chord_cm".to_string(), options.chord_cm),
            ("angular_rad".to_string(), options.angular),
            ("scale_factor_to_cm".to_string(), scale_to_cm),
        ]),
        timing_seconds: timing,
        peak_rss_mb: peak_rss_mb(),
        facets: ReportFacets {
            bin: bin_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            json: json_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            bin_sha256: manifest.binary_sha256.clone(),
        },
        counts,
        solids: results,
        problems,
        collapsed_triangles: collapsed_by_solid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_edges_of_a_closed_and_an_open_mesh() {
        // A tetrahedron, outward oriented.
        let tetra = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        assert_eq!(open_edge_report(&tetra), (0, 0));
        assert_eq!(open_edge_report(&tetra[..3]).0, 3);
        let doubled = [tetra[0], tetra[0], tetra[1], tetra[2], tetra[3]];
        assert!(open_edge_report(&doubled).1 > 0);
    }
}
