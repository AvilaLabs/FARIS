// Thin C++ layer between Rust and OpenCASCADE. Plain data goes across the cxx bridge; the
// OpenCASCADE objects stay behind the opaque Model.
#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>

#include "rust/cxx.h"

namespace faris_occt {

// Shared structs, defined by the bridge in src/ffi.rs.
struct SolidInfo;
struct MeshedSolid;

// A STEP file read into an XCAF document, with its solids in the reader's order.
struct Model {
  struct Impl;
  Model();
  ~Model();
  std::unique_ptr<Impl> impl;
};

rust::String occt_version();

// Reads `path` with STEPCAFControl_Reader. Shape numbers are converted into `internal_unit`
// (an OpenCASCADE unit name such as "MM") when the file declares a unit.
std::unique_ptr<Model> read_step(rust::Str path, rust::Str internal_unit);

// Length-unit names the reader found in the file (empty names left out).
rust::Vec<rust::String> file_length_units(const Model& model);

std::size_t solid_count(const Model& model);
std::size_t face_count(const Model& model, std::size_t solid);

// Name, assembly path, colour, volume, centroid, box and validity of one solid.
SolidInfo solid_info(const Model& model, std::size_t solid, double volume_eps);

// Meshes one solid with BRepMesh_IncrementalMesh and returns its faces' triangulations.
MeshedSolid mesh_solid(const Model& model, std::size_t solid, double linear, double angular);

// UVBounds of one face (1-based id): u_min, u_max, v_min, v_max.
rust::Vec<double> face_uv_bounds(const Model& model, std::size_t solid, std::size_t face);

// Classifies (u, v) points on one face (1-based id, explorer order within the solid) with
// BRepTopAdaptor_FClass2d: 0 in, 1 on, 2 out, 3 unknown.
rust::Vec<std::uint8_t> classify_uv(const Model& model, std::size_t solid, std::size_t face,
                                    double tolerance, rust::Slice<const double> u,
                                    rust::Slice<const double> v);

// The same question asked one point at a time with a new classifier for each point.
rust::Vec<std::uint8_t> classify_uv_per_point(const Model& model, std::size_t solid,
                                              std::size_t face, double tolerance,
                                              rust::Slice<const double> u,
                                              rust::Slice<const double> v);

// A different OpenCASCADE algorithm (BRepClass_FaceClassifier), for cross-checks.
rust::Vec<std::uint8_t> classify_uv_reference(const Model& model, std::size_t solid,
                                              std::size_t face, double tolerance,
                                              rust::Slice<const double> u,
                                              rust::Slice<const double> v);

}  // namespace faris_occt
