//! Test helpers shared by the integrity tests.
#![cfg(test)]

use super::binding::VolumeStats;
use faris_model::design::{
    Design, Facets, FacetsManifest, ManifestCounts, ManifestSource, ManifestSurface,
    ManifestVolume, sha256_hex,
};

pub fn manifest_of(f: &Facets, tags: &[&str]) -> FacetsManifest {
    let binary = f.to_bytes();
    FacetsManifest {
        schema: "faris-facets/v1".into(),
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
        binary_sha256: sha256_hex(&binary),
        helper_version: "test".into(),
        implicit_complement_tag: None,
        volumes: f
            .triangles_per_volume()
            .into_iter()
            .enumerate()
            .map(|(i, triangles)| ManifestVolume {
                index: i as u32 + 1,
                global_id: i as i64 + 1,
                material_tag: tags.get(i).map(|t| (*t).to_string()),
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

pub fn design_for(ids: &[&str], stats: &[VolumeStats]) -> Design {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../faris-model/tests/fixtures/design/pass.faris-design.json"
    ))
    .unwrap();
    let template = value["solids"][1].clone();
    let solids: Vec<serde_json::Value> = ids
            .iter()
            .zip(stats)
            .enumerate()
            .map(|(i, (id, s))| {
                let mut v = template.clone();
                v["id"] = (*id).into();
                v["step_index"] = i.into();
                v["fingerprint"] = serde_json::json!({"cad_volume_m3": s.faceted_volume_m3, "centroid_m": s.centroid_m});
                v
            })
            .collect();
    value["solids"] = solids.into();
    serde_json::from_value(value).unwrap()
}

/// A worker that answers a ping the way `faris-occt-worker` does, and fails any other request.
#[cfg(unix)]
pub fn stub_worker(
    dir: &std::path::Path,
    name: &str,
    tag: &str,
    other: &str,
) -> faris_occt_client::client::Worker {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let body = format!(
        "#!/bin/sh\ncase \"$(cat \"$1\")\" in\n*'\"op\":\"ping\"'*)\nprintf '{{\"schema\":\"faris-occt-response/v1\",\"status\":\"ok\",\"result\":{{\"occt\":\"7.9.3\",\"occt_tag\":\"{tag}\",\"helper\":\"faris-occt-1\"}}}}' > \"$2\";;\n*) {other};;\nesac\n"
    );
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    faris_occt_client::client::Worker::new(path)
}
