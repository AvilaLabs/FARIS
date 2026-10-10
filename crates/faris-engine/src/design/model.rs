//! A faceted model ready for exact queries: the snapped mesh and one
//! bounding-volume hierarchy over its triangles that have area.

use super::bvh::Bvh;
use super::mesh::Mesh;
use faris_model::design::Facets;

#[derive(Debug)]
pub struct Model {
    pub mesh: Mesh,
    pub bvh: Bvh,
}

impl Model {
    pub fn new(facets: &Facets) -> Result<Self, String> {
        let mesh = Mesh::from_facets(facets)?;
        let mut skip = vec![false; mesh.tris.len()];
        for &t in &mesh.degenerate {
            skip[t as usize] = true;
        }
        let active: Vec<u32> = (0..mesh.tris.len() as u32)
            .filter(|&t| !skip[t as usize])
            .collect();
        let bvh = Bvh::build(&mesh, active);
        Ok(Self { mesh, bvh })
    }
}
