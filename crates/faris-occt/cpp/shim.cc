// OpenCASCADE calls behind the cxx bridge. Every entry point turns an OpenCASCADE exception
// into a std::runtime_error, which the bridge returns as a Rust error. A crash or a hang cannot
// be caught here; the worker process around this code is what contains those.
#include "faris-occt/cpp/shim.h"

#include <array>
#include <chrono>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include <BRepBndLib.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepClass_FaceClassifier.hxx>
#include <BRepGProp.hxx>
#include <BRepTools.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepTopAdaptor_FClass2d.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <GProp_GProps.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <Interface_Static.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <STEPControl_Reader.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Version.hxx>
#include <TColStd_SequenceOfAsciiString.hxx>
#include <TCollection_AsciiString.hxx>
#include <TCollection_ExtendedString.hxx>
#include <TDF_Label.hxx>
#include <TDF_LabelSequence.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopAbs_State.hxx>
#include <TopExp_Explorer.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_ColorType.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <gp_Pnt.hxx>
#include <gp_Pnt2d.hxx>

#include "faris-occt/src/ffi.rs.h"

namespace faris_occt {

namespace {

template <typename F>
auto guarded(const char* what, F&& body) -> decltype(body()) {
  try {
    return body();
  } catch (const Standard_Failure& failure) {
    std::string message = std::string(what) + ": OpenCASCADE raised " + failure.DynamicType()->Name();
    if (failure.GetMessageString() != nullptr && failure.GetMessageString()[0] != '\0') {
      message += ": ";
      message += failure.GetMessageString();
    }
    throw std::runtime_error(message);
  }
}

std::string utf8(const TCollection_ExtendedString& text) {
  TCollection_AsciiString ascii(text);
  return std::string(ascii.ToCString(), static_cast<std::size_t>(ascii.Length()));
}

bool label_name(const TDF_Label& label, std::string& out) {
  Handle(TDataStd_Name) attribute;
  if (label.FindAttribute(TDataStd_Name::GetID(), attribute)) {
    out = utf8(attribute->Get());
    return true;
  }
  return false;
}

std::uint8_t state_code(TopAbs_State state) {
  switch (state) {
    case TopAbs_IN:
      return 0;
    case TopAbs_ON:
      return 1;
    case TopAbs_OUT:
      return 2;
    default:
      return 3;
  }
}

}  // namespace

struct Model::Impl {
  Handle(TDocStd_Document) document;
  STEPCAFControl_Reader reader;
  std::vector<TopoDS_Shape> solids;
  std::vector<std::vector<TopoDS_Face>> faces;
  // From the XCAF label named by a Python-compatible rule: first label of solid type that
  // shares the solid's TShape.
  std::vector<bool> has_name;
  std::vector<std::string> name;
  std::vector<std::string> path;
  std::vector<bool> has_colour;
  std::vector<std::array<double, 3>> colour;
};

Model::Model() : impl(new Impl()) {}
Model::~Model() = default;

namespace {

bool find_colour(const Handle(XCAFDoc_ColorTool) & tool, const TDF_Label& label, Quantity_Color& out) {
  for (XCAFDoc_ColorType type : {XCAFDoc_ColorGen, XCAFDoc_ColorSurf, XCAFDoc_ColorCurv}) {
    if (tool->GetColor(label, type, out)) return true;
  }
  return false;
}

bool find_colour(const Handle(XCAFDoc_ColorTool) & tool, const TopoDS_Shape& shape, Quantity_Color& out) {
  for (XCAFDoc_ColorType type : {XCAFDoc_ColorGen, XCAFDoc_ColorSurf, XCAFDoc_ColorCurv}) {
    if (tool->GetColor(shape, type, out)) return true;
  }
  return false;
}

struct Walker {
  Model::Impl& model;
  Handle(XCAFDoc_ShapeTool) shapes;
  Handle(XCAFDoc_ColorTool) colours;
  std::vector<bool> assigned;

  void claim(std::size_t index, const std::string& path, const Quantity_Color* colour) {
    assigned[index] = true;
    model.path[index] = path;
    if (colour != nullptr && !model.has_colour[index]) {
      double r, g, b;
      colour->Values(r, g, b, Quantity_TOC_sRGB);
      model.has_colour[index] = true;
      model.colour[index] = {r, g, b};
    }
  }

  void visit(const TDF_Label& part, const TopLoc_Location& location, const std::string& prefix,
             const Quantity_Color* inherited) {
    std::string own;
    label_name(part, own);
    const std::string path = prefix.empty() ? own : (own.empty() ? prefix : prefix + "/" + own);
    Quantity_Color part_colour;
    const bool part_has = find_colour(colours, part, part_colour);
    const Quantity_Color* here = inherited != nullptr ? inherited : (part_has ? &part_colour : nullptr);
    if (XCAFDoc_ShapeTool::IsAssembly(part)) {
      TDF_LabelSequence components;
      XCAFDoc_ShapeTool::GetComponents(part, components);
      for (Standard_Integer i = 1; i <= components.Length(); ++i) {
        const TDF_Label component = components.Value(i);
        TDF_Label referred;
        if (!XCAFDoc_ShapeTool::GetReferredShape(component, referred)) continue;
        Quantity_Color component_colour;
        const bool component_has = find_colour(colours, component, component_colour);
        const TopLoc_Location next = location * XCAFDoc_ShapeTool::GetLocation(component);
        std::string instance;
        label_name(component, instance);
        // The referred part's own name is added by the recursive call; the instance name
        // is kept only when it differs.
        std::string base = path;
        std::string referred_name;
        label_name(referred, referred_name);
        if (!instance.empty() && instance != referred_name) {
          base = base.empty() ? instance : base + "/" + instance;
        }
        visit(referred, next, base, component_has ? &component_colour : here);
      }
      return;
    }
    const TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(part);
    if (shape.IsNull()) return;
    for (TopExp_Explorer explorer(shape, TopAbs_SOLID); explorer.More(); explorer.Next()) {
      const TopoDS_Shape placed = explorer.Current().Moved(location);
      std::size_t found = model.solids.size();
      for (std::size_t k = 0; k < model.solids.size(); ++k) {
        if (!assigned[k] && model.solids[k].IsSame(placed)) {
          found = k;
          break;
        }
      }
      if (found == model.solids.size()) {
        for (std::size_t k = 0; k < model.solids.size(); ++k) {
          if (!assigned[k] && model.solids[k].IsPartner(placed)) {
            found = k;
            break;
          }
        }
      }
      if (found == model.solids.size()) continue;
      Quantity_Color solid_colour;
      const bool solid_has = find_colour(colours, explorer.Current(), solid_colour);
      claim(found, path, solid_has ? &solid_colour : here);
    }
  }
};

}  // namespace

rust::String occt_version() { return rust::String(OCC_VERSION_COMPLETE); }

std::unique_ptr<Model> read_step(rust::Str path, rust::Str internal_unit) {
  return guarded("read_step", [&]() {
    // Keep OpenCASCADE's own messages off the worker's output.
    Message::DefaultMessenger()->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
    // The reader's constructor registers the STEP parameters, so it comes before SetCVal.
    auto model = std::make_unique<Model>();
    Model::Impl& m = *model->impl;
    const std::string unit(internal_unit.data(), internal_unit.size());
    if (!Interface_Static::SetCVal("xstep.cascade.unit", unit.c_str())) {
      throw std::runtime_error("OpenCASCADE does not accept the unit " + unit);
    }
    Handle(XCAFApp_Application) application = XCAFApp_Application::GetApplication();
    application->NewDocument(TCollection_ExtendedString("MDTV-XCAF"), m.document);
    m.reader.SetNameMode(Standard_True);
    m.reader.SetColorMode(Standard_True);
    m.reader.SetLayerMode(Standard_True);
    const std::string file(path.data(), path.size());
    if (m.reader.ReadFile(file.c_str()) != IFSelect_RetDone) {
      throw std::runtime_error("OpenCASCADE could not read the STEP file");
    }
    if (!m.reader.Transfer(m.document)) {
      throw std::runtime_error("OpenCASCADE could not transfer the STEP file's shapes");
    }
    const TopoDS_Shape whole = m.reader.Reader().OneShape();
    for (TopExp_Explorer explorer(whole, TopAbs_SOLID); explorer.More(); explorer.Next()) {
      m.solids.push_back(explorer.Current());
    }
    const std::size_t n = m.solids.size();
    m.faces.resize(n);
    for (std::size_t i = 0; i < n; ++i) {
      for (TopExp_Explorer explorer(m.solids[i], TopAbs_FACE); explorer.More(); explorer.Next()) {
        m.faces[i].push_back(TopoDS::Face(explorer.Current()));
      }
    }
    m.has_name.assign(n, false);
    m.name.assign(n, std::string());
    m.path.assign(n, std::string());
    m.has_colour.assign(n, false);
    m.colour.assign(n, std::array<double, 3>{0.0, 0.0, 0.0});

    Handle(XCAFDoc_ShapeTool) shape_tool = XCAFDoc_DocumentTool::ShapeTool(m.document->Main());
    // Names: the first label of solid type sharing the solid's TShape, in label order.
    TDF_LabelSequence labels;
    shape_tool->GetShapes(labels);
    for (Standard_Integer i = 1; i <= labels.Length(); ++i) {
      std::string text;
      if (!label_name(labels.Value(i), text)) continue;
      const TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(labels.Value(i));
      if (shape.IsNull()) continue;
      for (std::size_t k = 0; k < n; ++k) {
        if (!m.has_name[k] && shape.ShapeType() == m.solids[k].ShapeType() && m.solids[k].IsPartner(shape)) {
          m.has_name[k] = true;
          m.name[k] = text;
        }
      }
    }
    // Assembly paths and colours: walk the XCAF tree from the free shapes.
    Walker walker{m, shape_tool, XCAFDoc_DocumentTool::ColorTool(m.document->Main()),
                  std::vector<bool>(n, false)};
    TDF_LabelSequence roots;
    shape_tool->GetFreeShapes(roots);
    for (Standard_Integer i = 1; i <= roots.Length(); ++i) {
      walker.visit(roots.Value(i), TopLoc_Location(), std::string(), nullptr);
    }
    return model;
  });
}

rust::Vec<rust::String> file_length_units(const Model& model) {
  return guarded("file_length_units", [&]() {
    TColStd_SequenceOfAsciiString lengths, angles, solid_angles;
    model.impl->reader.ChangeReader().FileUnits(lengths, angles, solid_angles);
    rust::Vec<rust::String> out;
    for (Standard_Integer i = 1; i <= lengths.Length(); ++i) {
      const TCollection_AsciiString& name = lengths.Value(i);
      if (name.Length() == 0) continue;
      out.push_back(rust::String(name.ToCString()));
    }
    return out;
  });
}

std::size_t solid_count(const Model& model) { return model.impl->solids.size(); }

std::size_t face_count(const Model& model, std::size_t solid) {
  if (solid >= model.impl->solids.size()) throw std::out_of_range("solid index");
  return model.impl->faces[solid].size();
}

SolidInfo solid_info(const Model& model, std::size_t solid, double volume_eps) {
  if (solid >= model.impl->solids.size()) throw std::out_of_range("solid index");
  return guarded("solid_info", [&]() {
    const Model::Impl& m = *model.impl;
    const TopoDS_Shape& shape = m.solids[solid];
    SolidInfo info;
    info.name = rust::String(m.name[solid]);
    info.has_name = m.has_name[solid];
    info.assembly_path = rust::String(m.path[solid]);
    info.has_colour = m.has_colour[solid];
    info.colour = m.colour[solid];
    info.faces = static_cast<std::uint32_t>(m.faces[solid].size());
    GProp_GProps props;
    info.volume_error = BRepGProp::VolumeProperties(shape, props, volume_eps, Standard_False, Standard_False);
    info.volume = props.Mass();
    const gp_Pnt centre = props.CentreOfMass();
    info.centroid = {centre.X(), centre.Y(), centre.Z()};
    Bnd_Box box;
    BRepBndLib::AddOptimal(shape, box, Standard_False, Standard_False);
    const gp_Pnt low = box.CornerMin();
    const gp_Pnt high = box.CornerMax();
    info.bbox_min = {low.X(), low.Y(), low.Z()};
    info.bbox_max = {high.X(), high.Y(), high.Z()};
    info.valid = BRepCheck_Analyzer(shape).IsValid();
    return info;
  });
}

MeshedSolid mesh_solid(const Model& model, std::size_t solid, double linear, double angular) {
  if (solid >= model.impl->solids.size()) throw std::out_of_range("solid index");
  return guarded("mesh_solid", [&]() {
    const Model::Impl& m = *model.impl;
    const auto started = std::chrono::steady_clock::now();
    BRepMesh_IncrementalMesh mesher(m.solids[solid], linear, Standard_False, angular, Standard_False);
    if (!mesher.IsDone()) throw std::runtime_error("OpenCASCADE could not mesh the solid");
    MeshedSolid out;
    out.mesh_seconds = std::chrono::duration<double>(std::chrono::steady_clock::now() - started).count();
    const auto& faces = m.faces[solid];
    for (std::size_t f = 0; f < faces.size(); ++f) {
      TopLoc_Location location;
      const Handle(Poly_Triangulation) triangulation = BRep_Tool::Triangulation(faces[f], location);
      if (triangulation.IsNull() || triangulation->NbTriangles() == 0) {
        out.missing_faces.push_back(static_cast<std::uint32_t>(f + 1));
        continue;
      }
      MeshFace face;
      face.face_id = static_cast<std::uint32_t>(f + 1);
      const bool moved = !location.IsIdentity();
      const gp_Trsf transform = location.Transformation();
      face.nodes.reserve(3 * static_cast<std::size_t>(triangulation->NbNodes()));
      for (Standard_Integer i = 1; i <= triangulation->NbNodes(); ++i) {
        gp_Pnt p = triangulation->Node(i);
        if (moved) p.Transform(transform);
        face.nodes.push_back(p.X());
        face.nodes.push_back(p.Y());
        face.nodes.push_back(p.Z());
      }
      const bool reversed = faces[f].Orientation() == TopAbs_REVERSED;
      face.triangles.reserve(3 * static_cast<std::size_t>(triangulation->NbTriangles()));
      for (Standard_Integer i = 1; i <= triangulation->NbTriangles(); ++i) {
        Standard_Integer a, b, c;
        triangulation->Triangle(i).Get(a, b, c);
        face.triangles.push_back(static_cast<std::uint32_t>(a - 1));
        face.triangles.push_back(static_cast<std::uint32_t>((reversed ? c : b) - 1));
        face.triangles.push_back(static_cast<std::uint32_t>((reversed ? b : c) - 1));
      }
      out.faces.push_back(std::move(face));
    }
    return out;
  });
}

namespace {

const TopoDS_Face& face_of(const Model& model, std::size_t solid, std::size_t face) {
  if (solid >= model.impl->solids.size()) throw std::out_of_range("solid index");
  if (face == 0 || face > model.impl->faces[solid].size()) throw std::out_of_range("face id");
  return model.impl->faces[solid][face - 1];
}

}  // namespace

rust::Vec<double> face_uv_bounds(const Model& model, std::size_t solid, std::size_t face) {
  const TopoDS_Face& f = face_of(model, solid, face);
  return guarded("face_uv_bounds", [&]() {
    Standard_Real u0, u1, v0, v1;
    BRepTools::UVBounds(f, u0, u1, v0, v1);
    rust::Vec<double> out;
    for (double x : {u0, u1, v0, v1}) out.push_back(x);
    return out;
  });
}

rust::Vec<std::uint8_t> classify_uv(const Model& model, std::size_t solid, std::size_t face,
                                    double tolerance, rust::Slice<const double> u,
                                    rust::Slice<const double> v) {
  if (u.size() != v.size()) throw std::invalid_argument("u and v differ in length");
  const TopoDS_Face& f = face_of(model, solid, face);
  return guarded("classify_uv", [&]() {
    BRepTopAdaptor_FClass2d classifier(f, tolerance);
    rust::Vec<std::uint8_t> out;
    out.reserve(u.size());
    gp_Pnt2d probe(0.0, 0.0);
    for (std::size_t i = 0; i < u.size(); ++i) {
      probe.SetCoord(u[i], v[i]);
      out.push_back(state_code(classifier.Perform(probe)));
    }
    return out;
  });
}

rust::Vec<std::uint8_t> classify_uv_per_point(const Model& model, std::size_t solid,
                                              std::size_t face, double tolerance,
                                              rust::Slice<const double> u,
                                              rust::Slice<const double> v) {
  if (u.size() != v.size()) throw std::invalid_argument("u and v differ in length");
  const TopoDS_Face& f = face_of(model, solid, face);
  return guarded("classify_uv_per_point", [&]() {
    rust::Vec<std::uint8_t> out;
    out.reserve(u.size());
    for (std::size_t i = 0; i < u.size(); ++i) {
      BRepTopAdaptor_FClass2d classifier(f, tolerance);
      out.push_back(state_code(classifier.Perform(gp_Pnt2d(u[i], v[i]))));
    }
    return out;
  });
}

rust::Vec<std::uint8_t> classify_uv_reference(const Model& model, std::size_t solid,
                                              std::size_t face, double tolerance,
                                              rust::Slice<const double> u,
                                              rust::Slice<const double> v) {
  if (u.size() != v.size()) throw std::invalid_argument("u and v differ in length");
  const TopoDS_Face& f = face_of(model, solid, face);
  return guarded("classify_uv_reference", [&]() {
    rust::Vec<std::uint8_t> out;
    out.reserve(u.size());
    for (std::size_t i = 0; i < u.size(); ++i) {
      BRepClass_FaceClassifier classifier(f, gp_Pnt2d(u[i], v[i]), tolerance);
      out.push_back(state_code(classifier.State()));
    }
    return out;
  });
}

}  // namespace faris_occt
