//! `faris-facets/v1`: the faceted geometry the CAD helper exports for FARIS's own checks.
//!
//! One binary file and one JSON manifest. The helper reads the DAGMC `h5m` (or
//! meshes each STEP solid on its own) and writes both; FARIS parses them here,
//! rejecting any malformed file with a reason, and never trusts the helper's
//! counts: every number in the manifest is checked against the binary.
//!
//! # Binary layout (little-endian, no padding)
//!
//! | offset | size | field |
//! | --- | --- | --- |
//! | 0 | 8 | magic `FARISFC1` |
//! | 8 | 4 | `u32` version, 1 |
//! | 12 | 4 | `u32` flags: bit 0 set means unimprinted per-solid meshes (no surface is shared by two volumes) |
//! | 16 | 8 | `u64` n_vertices |
//! | 24 | 8 | `u64` n_triangles |
//! | 32 | 8 | `u64` n_surfaces |
//! | 40 | 8 | `u64` n_volumes |
//! | 48 | 24 n_vertices | vertices, `f64` x y z, in metres |
//! | | 12 n_triangles | triangles, `u32` a b c vertex indices, grouped by surface |
//! | | 24 n_surfaces | per surface: `u64` first_triangle, `u64` triangle_count, `u32` forward_volume, `u32` reverse_volume |
//!
//! Volumes are numbered from 1 in file order; 0 means the implicit complement.
//! A triangle's normal (right-hand rule on a, b, c) points out of the surface's
//! forward volume, towards its reverse volume. This is the DAGMC sense
//! convention: the signed volume of a volume is the sum over its surfaces of
//! `+det(a, b, c) / 6` where it is the forward volume and `-det(a, b, c) / 6`
//! where it is the reverse volume. Vertices are deduplicated by the mesh's own
//! vertex identity (the MOAB handle), never by position, so two vertices at the
//! same place stay two vertices. Surfaces tile the triangle array in order.
//!
//! # Manifest (`faris-facets/v1`)
//!
//! Counts, the unit (`m`), the source's kind and sha256, the binary's sha256,
//! the helper version, and per volume its index, DAGMC global id, material tag
//! and triangle count, and per surface its global id.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FACETS_SCHEMA: &str = "faris-facets/v1";
pub const FACETS_MAGIC: &[u8; 8] = b"FARISFC1";
pub const FACETS_VERSION: u32 = 1;
/// Header flag: unimprinted per-solid meshes.
pub const FLAG_UNIMPRINTED: u32 = 1;
pub const HEADER_BYTES: usize = 48;
/// A bound on every count, so a corrupt header cannot ask for a huge allocation.
pub const MAX_COUNT: u64 = u32::MAX as u64;

/// A surface: a run of triangles and the two volumes on its sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    pub first_triangle: u64,
    pub triangle_count: u64,
    pub forward_volume: u32,
    pub reverse_volume: u32,
}

/// The parsed binary file.
#[derive(Clone, Debug, PartialEq)]
pub struct Facets {
    pub flags: u32,
    pub n_volumes: u32,
    /// Metres.
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub surfaces: Vec<Surface>,
}

impl Facets {
    pub fn unimprinted(&self) -> bool {
        self.flags & FLAG_UNIMPRINTED != 0
    }

    /// Triangle count per volume (a triangle on a surface between two volumes counts for both).
    pub fn triangles_per_volume(&self) -> Vec<u64> {
        let mut counts = vec![0u64; self.n_volumes as usize];
        for s in &self.surfaces {
            for v in [s.forward_volume, s.reverse_volume] {
                if v != 0 {
                    counts[v as usize - 1] += s.triangle_count;
                }
            }
        }
        counts
    }

    /// Distinct vertices used by each volume's surfaces.
    pub fn vertices_per_volume(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.n_volumes as usize);
        let mut seen = vec![0u32; self.vertices.len()];
        for volume in 1..=self.n_volumes {
            let mut count = 0u64;
            for s in &self.surfaces {
                if s.forward_volume != volume && s.reverse_volume != volume {
                    continue;
                }
                let first = s.first_triangle as usize;
                for t in &self.triangles[first..first + s.triangle_count as usize] {
                    for &v in t {
                        if seen[v as usize] != volume {
                            seen[v as usize] = volume;
                            count += 1;
                        }
                    }
                }
            }
            out.push(count);
        }
        out
    }

    /// Serialise in the binary layout (used by tests and the corpus builders).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            HEADER_BYTES
                + 24 * self.vertices.len()
                + 12 * self.triangles.len()
                + 24 * self.surfaces.len(),
        );
        out.extend_from_slice(FACETS_MAGIC);
        out.extend_from_slice(&FACETS_VERSION.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&(self.vertices.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.triangles.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.surfaces.len() as u64).to_le_bytes());
        out.extend_from_slice(&u64::from(self.n_volumes).to_le_bytes());
        for v in &self.vertices {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        for t in &self.triangles {
            for i in t {
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
        for s in &self.surfaces {
            out.extend_from_slice(&s.first_triangle.to_le_bytes());
            out.extend_from_slice(&s.triangle_count.to_le_bytes());
            out.extend_from_slice(&s.forward_volume.to_le_bytes());
            out.extend_from_slice(&s.reverse_volume.to_le_bytes());
        }
        out
    }
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

fn le_f64(bytes: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// Parse the binary file. Every inconsistency is an error with its reason.
pub fn parse_facets(bytes: &[u8]) -> Result<Facets, String> {
    if bytes.len() < HEADER_BYTES {
        return Err(format!(
            "the file is {} bytes, shorter than the {HEADER_BYTES}-byte header",
            bytes.len()
        ));
    }
    if &bytes[..8] != FACETS_MAGIC {
        return Err("the magic is not FARISFC1; this is not a faris-facets file".into());
    }
    let version = le_u32(bytes, 8);
    if version != FACETS_VERSION {
        return Err(format!(
            "the file is version {version}; this FARIS reads version {FACETS_VERSION}"
        ));
    }
    let flags = le_u32(bytes, 12);
    if flags & !FLAG_UNIMPRINTED != 0 {
        return Err(format!(
            "flags {flags:#x} set bits this FARIS does not know"
        ));
    }
    let n_vertices = le_u64(bytes, 16);
    let n_triangles = le_u64(bytes, 24);
    let n_surfaces = le_u64(bytes, 32);
    let n_volumes = le_u64(bytes, 40);
    for (name, n) in [
        ("vertices", n_vertices),
        ("triangles", n_triangles),
        ("surfaces", n_surfaces),
        ("volumes", n_volumes),
    ] {
        if n > MAX_COUNT {
            return Err(format!(
                "the header claims {n} {name}, over the {MAX_COUNT} bound"
            ));
        }
    }
    let expected = HEADER_BYTES as u64 + 24 * n_vertices + 12 * n_triangles + 24 * n_surfaces;
    if expected != bytes.len() as u64 {
        return Err(format!(
            "the header's counts need {expected} bytes and the file has {}",
            bytes.len()
        ));
    }
    if n_volumes == 0 || n_surfaces == 0 || n_triangles == 0 {
        return Err("the file holds no volumes, surfaces or triangles".into());
    }
    let mut at = HEADER_BYTES;
    let mut vertices = Vec::with_capacity(n_vertices as usize);
    for i in 0..n_vertices as usize {
        let v = [
            le_f64(bytes, at),
            le_f64(bytes, at + 8),
            le_f64(bytes, at + 16),
        ];
        if v.iter().any(|c| !c.is_finite()) {
            return Err(format!("vertex {i} has a coordinate that is not finite"));
        }
        vertices.push(v);
        at += 24;
    }
    let mut triangles = Vec::with_capacity(n_triangles as usize);
    for i in 0..n_triangles as usize {
        let t = [
            le_u32(bytes, at),
            le_u32(bytes, at + 4),
            le_u32(bytes, at + 8),
        ];
        if t.iter().any(|&v| u64::from(v) >= n_vertices) {
            return Err(format!(
                "triangle {i} names vertex {} but there are only {n_vertices} vertices",
                t.iter().max().expect("three entries")
            ));
        }
        triangles.push(t);
        at += 12;
    }
    let mut surfaces = Vec::with_capacity(n_surfaces as usize);
    let mut next = 0u64;
    let mut used = vec![false; n_volumes as usize];
    for i in 0..n_surfaces as usize {
        let s = Surface {
            first_triangle: le_u64(bytes, at),
            triangle_count: le_u64(bytes, at + 8),
            forward_volume: le_u32(bytes, at + 16),
            reverse_volume: le_u32(bytes, at + 20),
        };
        at += 24;
        if s.first_triangle != next || s.triangle_count == 0 {
            return Err(format!(
                "surface {i} starts at triangle {} with {} triangles; surfaces must tile the \
                 triangles in order and none may be empty (expected start {next})",
                s.first_triangle, s.triangle_count
            ));
        }
        next += s.triangle_count;
        for v in [s.forward_volume, s.reverse_volume] {
            if u64::from(v) > n_volumes {
                return Err(format!(
                    "surface {i} names volume {v} but there are only {n_volumes} volumes"
                ));
            }
            if v != 0 {
                used[v as usize - 1] = true;
            }
        }
        if s.forward_volume == s.reverse_volume {
            return Err(format!(
                "surface {i} has volume {} on both sides (0 means the implicit complement)",
                s.forward_volume
            ));
        }
        surfaces.push(s);
    }
    if next != n_triangles {
        return Err(format!(
            "the surfaces cover {next} triangles but the file has {n_triangles}"
        ));
    }
    if let Some(i) = used.iter().position(|u| !u) {
        return Err(format!("volume {} has no surface", i + 1));
    }
    Ok(Facets {
        flags,
        n_volumes: n_volumes as u32,
        vertices,
        triangles,
        surfaces,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManifestCounts {
    pub vertices: u64,
    pub triangles: u64,
    pub surfaces: u64,
    pub volumes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    /// `h5m` for the imprinted DAGMC model, `step` for per-solid meshes.
    pub kind: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManifestVolume {
    pub index: u32,
    /// DAGMC `GLOBAL_ID`; for per-solid meshes the STEP read index plus 1.
    pub global_id: i64,
    /// The `NAME` of the group holding the volume, for example `mat:rm-m-plasma`.
    pub material_tag: Option<String>,
    pub triangles: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManifestSurface {
    pub index: u32,
    pub global_id: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FacetsManifest {
    pub schema: String,
    pub unit: String,
    pub unimprinted: bool,
    pub counts: ManifestCounts,
    pub source: ManifestSource,
    pub binary_sha256: String,
    pub helper_version: String,
    pub volumes: Vec<ManifestVolume>,
    pub surfaces: Vec<ManifestSurface>,
}

/// Binary and manifest, checked against each other.
#[derive(Clone, Debug)]
pub struct LoadedFacets {
    pub facets: Facets,
    pub manifest: FacetsManifest,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Parse and cross-check the pair. The manifest's hash, counts, unit, flags and
/// per-volume triangle counts must all agree with the binary.
pub fn load_facets(binary: &[u8], manifest: &[u8]) -> Result<LoadedFacets, String> {
    let manifest: FacetsManifest = serde_json::from_slice(manifest)
        .map_err(|e| format!("the facets manifest could not be read: {e}"))?;
    if manifest.schema != FACETS_SCHEMA {
        return Err(format!(
            "the manifest has schema {:?}; this FARIS reads {FACETS_SCHEMA}",
            manifest.schema
        ));
    }
    if manifest.unit != "m" {
        return Err(format!(
            "the manifest's unit is {:?}; the binary holds metres and the manifest must say \"m\"",
            manifest.unit
        ));
    }
    let hash = sha256_hex(binary);
    if !hash.eq_ignore_ascii_case(&manifest.binary_sha256) {
        return Err(format!(
            "the binary's sha256 is {hash} but the manifest records {}",
            manifest.binary_sha256
        ));
    }
    let facets = parse_facets(binary)?;
    let counts = &manifest.counts;
    if counts.vertices != facets.vertices.len() as u64
        || counts.triangles != facets.triangles.len() as u64
        || counts.surfaces != facets.surfaces.len() as u64
        || counts.volumes != u64::from(facets.n_volumes)
    {
        return Err(format!(
            "the manifest counts ({} vertices, {} triangles, {} surfaces, {} volumes) differ from \
             the binary ({}, {}, {}, {})",
            counts.vertices,
            counts.triangles,
            counts.surfaces,
            counts.volumes,
            facets.vertices.len(),
            facets.triangles.len(),
            facets.surfaces.len(),
            facets.n_volumes
        ));
    }
    if manifest.unimprinted != facets.unimprinted() {
        return Err("the manifest and the binary disagree about the unimprinted flag".into());
    }
    if manifest.volumes.len() != facets.n_volumes as usize
        || manifest.surfaces.len() != facets.surfaces.len()
    {
        return Err("the manifest does not list every volume and surface".into());
    }
    let per_volume = facets.triangles_per_volume();
    for (i, volume) in manifest.volumes.iter().enumerate() {
        if volume.index as usize != i + 1 {
            return Err(format!(
                "manifest volume {i} has index {}; volumes are listed in order from 1",
                volume.index
            ));
        }
        if volume.triangles != per_volume[i] {
            return Err(format!(
                "manifest volume {} claims {} triangles and the binary gives {}",
                volume.index, volume.triangles, per_volume[i]
            ));
        }
    }
    for (i, surface) in manifest.surfaces.iter().enumerate() {
        if surface.index as usize != i {
            return Err(format!("manifest surface {i} has index {}", surface.index));
        }
    }
    Ok(LoadedFacets { facets, manifest })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetra() -> Facets {
        // A tetrahedron around the origin with outward normals, volume 1 forward.
        Facets {
            flags: 0,
            n_volumes: 1,
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            surfaces: vec![Surface {
                first_triangle: 0,
                triangle_count: 4,
                forward_volume: 1,
                reverse_volume: 0,
            }],
        }
    }

    fn manifest_for(f: &Facets, binary: &[u8]) -> FacetsManifest {
        FacetsManifest {
            schema: FACETS_SCHEMA.into(),
            unit: "m".into(),
            unimprinted: f.unimprinted(),
            counts: ManifestCounts {
                vertices: f.vertices.len() as u64,
                triangles: f.triangles.len() as u64,
                surfaces: f.surfaces.len() as u64,
                volumes: u64::from(f.n_volumes),
            },
            source: ManifestSource {
                kind: "h5m".into(),
                sha256: "0".repeat(64),
            },
            binary_sha256: sha256_hex(binary),
            helper_version: "test".into(),
            volumes: (1..=f.n_volumes)
                .zip(f.triangles_per_volume())
                .map(|(index, triangles)| ManifestVolume {
                    index,
                    global_id: i64::from(index),
                    material_tag: Some("mat:a".into()),
                    triangles,
                })
                .collect(),
            surfaces: (0..f.surfaces.len() as u32)
                .map(|index| ManifestSurface {
                    index,
                    global_id: i64::from(index) + 1,
                })
                .collect(),
        }
    }

    // Verifies: GEO-012
    #[test]
    fn round_trips_through_the_binary_layout() {
        let f = tetra();
        let bytes = f.to_bytes();
        assert_eq!(bytes.len(), 48 + 24 * 4 + 12 * 4 + 24);
        assert_eq!(parse_facets(&bytes).unwrap(), f);
        assert_eq!(f.triangles_per_volume(), vec![4]);
        assert_eq!(f.vertices_per_volume(), vec![4]);
    }

    #[test]
    fn manifest_must_agree_with_the_binary() {
        let f = tetra();
        let bytes = f.to_bytes();
        let m = manifest_for(&f, &bytes);
        let text = serde_json::to_vec(&m).unwrap();
        assert!(load_facets(&bytes, &text).is_ok());
        let mut wrong = m.clone();
        wrong.counts.triangles = 5;
        let err = load_facets(&bytes, &serde_json::to_vec(&wrong).unwrap()).unwrap_err();
        assert!(err.contains("differ"), "{err}");
        let mut wrong = m.clone();
        wrong.binary_sha256 = "1".repeat(64);
        assert!(
            load_facets(&bytes, &serde_json::to_vec(&wrong).unwrap())
                .unwrap_err()
                .contains("sha256")
        );
        let mut wrong = m.clone();
        wrong.volumes[0].triangles = 3;
        assert!(
            load_facets(&bytes, &serde_json::to_vec(&wrong).unwrap())
                .unwrap_err()
                .contains("claims 3")
        );
        let mut wrong = m;
        wrong.unit = "cm".into();
        assert!(
            load_facets(&bytes, &serde_json::to_vec(&wrong).unwrap())
                .unwrap_err()
                .contains("unit")
        );
        assert!(load_facets(&bytes, b"{\"schema\":1}").is_err());
    }

    #[test]
    fn malformed_files_are_rejected_with_a_reason() {
        let good = tetra().to_bytes();
        let reject = |bytes: &[u8], needle: &str| {
            let err = parse_facets(bytes).unwrap_err();
            assert!(err.contains(needle), "{err:?} lacks {needle:?}");
        };
        reject(&good[..10], "shorter");
        let mut b = good.clone();
        b[0] = b'X';
        reject(&b, "magic");
        let mut b = good.clone();
        b[8] = 2;
        reject(&b, "version 2");
        let mut b = good.clone();
        b[12] = 2;
        reject(&b, "flags");
        let mut b = good.clone();
        b[16..24].copy_from_slice(&5u64.to_le_bytes());
        reject(&b, "need");
        let mut b = good.clone();
        b[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        reject(&b, "bound");
        let mut b = good.clone();
        b.push(0);
        reject(&b, "need");
        let mut b = good.clone();
        b[48..56].copy_from_slice(&f64::NAN.to_le_bytes());
        reject(&b, "not finite");
        let mut b = good.clone();
        let tri0 = 48 + 24 * 4;
        b[tri0..tri0 + 4].copy_from_slice(&9u32.to_le_bytes());
        reject(&b, "names vertex 9");
        let surf = good.len() - 24;
        let mut b = good.clone();
        b[surf + 16..surf + 20].copy_from_slice(&3u32.to_le_bytes());
        reject(&b, "volume 3");
        let mut b = good.clone();
        b[surf + 20..surf + 24].copy_from_slice(&1u32.to_le_bytes());
        reject(&b, "both sides");
        let mut b = good.clone();
        b[surf..surf + 8].copy_from_slice(&1u64.to_le_bytes());
        reject(&b, "tile");
        let mut b = good.clone();
        b[surf + 8..surf + 16].copy_from_slice(&3u64.to_le_bytes());
        reject(&b, "cover");
        // Volumes that no surface touches.
        let mut f = tetra();
        f.n_volumes = 2;
        reject(&f.to_bytes(), "volume 2 has no surface");
    }
}
