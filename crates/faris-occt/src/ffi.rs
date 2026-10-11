//! The cxx bridge to the C++ shim. Plain data structs cross it; OpenCASCADE objects stay
//! behind the opaque `Model`. Nothing outside this crate's worker and tests calls it.

// The bridge macro expands to unsafe blocks; this is the only module allowed to hold them.
#![allow(unsafe_code)]

#[cxx::bridge(namespace = "faris_occt")]
pub mod bridge {
    /// One solid of the STEP file, in shape units of the reader (millimetres for a file with a
    /// declared unit, see `read_step`).
    struct SolidInfo {
        /// Raw XCAF name; empty when the label carries none.
        name: String,
        has_name: bool,
        /// Names from the root of the assembly to the part, joined by `/`; empty when the
        /// solid was not found in the XCAF tree.
        assembly_path: String,
        has_colour: bool,
        colour: [f64; 3],
        faces: u32,
        volume: f64,
        volume_error: f64,
        centroid: [f64; 3],
        bbox_min: [f64; 3],
        bbox_max: [f64; 3],
        valid: bool,
    }

    /// One face's triangulation, nodes already placed in the model frame and triangles
    /// oriented outward (reversed faces have their winding flipped).
    struct MeshFace {
        /// 1-based, explorer order within the solid.
        face_id: u32,
        /// x y z per node, shape units.
        nodes: Vec<f64>,
        /// Three node indices per triangle, 0-based within this face.
        triangles: Vec<u32>,
    }

    struct MeshedSolid {
        faces: Vec<MeshFace>,
        /// Faces with no triangulation after meshing (1-based ids).
        missing_faces: Vec<u32>,
        mesh_seconds: f64,
    }

    unsafe extern "C++" {
        include!("faris-occt/cpp/shim.h");

        type Model;

        fn occt_version() -> String;
        fn read_step(path: &str, internal_unit: &str) -> Result<UniquePtr<Model>>;
        fn file_length_units(model: &Model) -> Vec<String>;
        fn solid_count(model: &Model) -> usize;
        fn face_count(model: &Model, solid: usize) -> usize;
        fn solid_info(model: &Model, solid: usize, volume_eps: f64) -> Result<SolidInfo>;
        fn mesh_solid(
            model: &Model,
            solid: usize,
            linear: f64,
            angular: f64,
        ) -> Result<MeshedSolid>;
        fn face_uv_bounds(model: &Model, solid: usize, face: usize) -> Result<Vec<f64>>;
        fn classify_uv(
            model: &Model,
            solid: usize,
            face: usize,
            tolerance: f64,
            u: &[f64],
            v: &[f64],
        ) -> Result<Vec<u8>>;
        fn classify_uv_per_point(
            model: &Model,
            solid: usize,
            face: usize,
            tolerance: f64,
            u: &[f64],
            v: &[f64],
        ) -> Result<Vec<u8>>;
        fn classify_uv_reference(
            model: &Model,
            solid: usize,
            face: usize,
            tolerance: f64,
            u: &[f64],
            v: &[f64],
        ) -> Result<Vec<u8>>;
    }
}
