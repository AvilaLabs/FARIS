//! Safe wrappers over the bridge. This is OpenCASCADE running inside the calling process; the
//! worker binary is the only production caller. FARIS itself goes through [`crate::client`].

use crate::ffi::bridge;
use cxx::UniquePtr;

pub use bridge::{MeshFace, MeshedSolid, SolidInfo};

/// The OpenCASCADE the crate was built against, for example `7.9.3`.
pub fn occt_version() -> String {
    bridge::occt_version()
}

/// A STEP file read into an XCAF document. Solids are numbered from 0 in the reader's order.
pub struct StepModel {
    inner: UniquePtr<bridge::Model>,
}

impl StepModel {
    /// Reads `path`. A file with a declared length unit is converted to `internal_unit`
    /// (an OpenCASCADE unit name such as `MM`); a file without one is read unscaled.
    pub fn read(path: &str, internal_unit: &str) -> Result<Self, String> {
        let inner = bridge::read_step(path, internal_unit).map_err(|e| e.what().to_string())?;
        Ok(Self { inner })
    }

    pub fn solid_count(&self) -> usize {
        bridge::solid_count(&self.inner)
    }

    pub fn face_count(&self, solid: usize) -> Result<usize, String> {
        self.check_solid(solid)?;
        Ok(bridge::face_count(&self.inner, solid))
    }

    /// OpenCASCADE's names for the length units the file declares.
    pub fn file_length_units(&self) -> Vec<String> {
        bridge::file_length_units(&self.inner)
    }

    pub fn solid_info(&self, solid: usize, volume_eps: f64) -> Result<SolidInfo, String> {
        self.check_solid(solid)?;
        bridge::solid_info(&self.inner, solid, volume_eps).map_err(|e| e.what().to_string())
    }

    /// Meshes one solid (the mesh stays on the shape) and returns its faces' triangulations.
    pub fn mesh_solid(
        &self,
        solid: usize,
        linear: f64,
        angular: f64,
    ) -> Result<MeshedSolid, String> {
        self.check_solid(solid)?;
        bridge::mesh_solid(&self.inner, solid, linear, angular).map_err(|e| e.what().to_string())
    }

    /// `[u_min, u_max, v_min, v_max]` of a face (1-based id).
    pub fn face_uv_bounds(&self, solid: usize, face: usize) -> Result<[f64; 4], String> {
        self.check_solid(solid)?;
        let v =
            bridge::face_uv_bounds(&self.inner, solid, face).map_err(|e| e.what().to_string())?;
        <[f64; 4]>::try_from(v.as_slice())
            .map_err(|_| "face_uv_bounds returned the wrong length".to_string())
    }

    /// States of (u, v) points on a face (1-based id) from one classifier built once:
    /// one native call for the whole batch.
    pub fn classify(
        &self,
        solid: usize,
        face: usize,
        tolerance: f64,
        u: &[f64],
        v: &[f64],
    ) -> Result<Vec<FaceState>, String> {
        self.check_solid(solid)?;
        if u.len() != v.len() {
            return Err("u and v differ in length".into());
        }
        let codes = bridge::classify_uv(&self.inner, solid, face, tolerance, u, v)
            .map_err(|e| e.what().to_string())?;
        Ok(codes.into_iter().map(FaceState::from_code).collect())
    }

    /// The same states asked one point at a time, a new classifier for each point.
    pub fn classify_per_point(
        &self,
        solid: usize,
        face: usize,
        tolerance: f64,
        u: &[f64],
        v: &[f64],
    ) -> Result<Vec<FaceState>, String> {
        self.check_solid(solid)?;
        if u.len() != v.len() {
            return Err("u and v differ in length".into());
        }
        let codes = bridge::classify_uv_per_point(&self.inner, solid, face, tolerance, u, v)
            .map_err(|e| e.what().to_string())?;
        Ok(codes.into_iter().map(FaceState::from_code).collect())
    }

    /// A different OpenCASCADE algorithm (`BRepClass_FaceClassifier`), for cross-checks.
    pub fn classify_reference(
        &self,
        solid: usize,
        face: usize,
        tolerance: f64,
        u: &[f64],
        v: &[f64],
    ) -> Result<Vec<FaceState>, String> {
        self.check_solid(solid)?;
        if u.len() != v.len() {
            return Err("u and v differ in length".into());
        }
        let codes = bridge::classify_uv_reference(&self.inner, solid, face, tolerance, u, v)
            .map_err(|e| e.what().to_string())?;
        Ok(codes.into_iter().map(FaceState::from_code).collect())
    }

    fn check_solid(&self, solid: usize) -> Result<(), String> {
        if solid < self.solid_count() {
            Ok(())
        } else {
            Err(format!(
                "solid {solid} is out of range (the file has {})",
                self.solid_count()
            ))
        }
    }
}

/// Where a (u, v) point lies relative to a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FaceState {
    In = 0,
    On = 1,
    Out = 2,
    Unknown = 3,
}

impl FaceState {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::In,
            1 => Self::On,
            2 => Self::Out,
            _ => Self::Unknown,
        }
    }
}
