#!/usr/bin/env python3
"""Build the reference model RM-M: CadQuery solids, STEP, DAGMC h5m, model card and role map.

Run with the CAD environment (cadquery 2.8, cad_to_dagmc 0.14.2, gmsh, h5py):

    build_rm_m.py --out-dir DIR [--tolerance-cm 0.05] [--angular-tolerance 0.1] [--random-ray]

The model is built directly in CadQuery, in millimetres (STEP convention), and
converted to centimetres (OpenMC) by cad_to_dagmc with scale_factor 0.1.
Paramak 0.10.0 was assessed and not used: its tokamak builder takes one
constant thickness per layer, its D-shaped TF coil has a straight inboard leg
and it has no port builder, so the graded inboard/outboard build, the
conformal 18-coil wedges with a case/winding-pack split and the port would all
have been CadQuery code anyway. The poloidal geometry is in rm_m_spec.py.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rm_m_spec as spec  # noqa: E402

MM = 10.0  # cm -> mm


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def peak_mb() -> float:
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0


def mass_properties(solid, eps: float = 1.0e-7):
    """Adaptive-precision volume (mm3) and centroid (mm) of a solid.

    cq.Shape.Volume() uses a fixed-order integration that is off by up to 1 %
    on these spline surfaces of revolution (the plasma: 144.5 against
    145.69 m3 by Pappus on the profile), so the reference CAD volume comes from
    OCCT's adaptive BRepGProp integration (relative tolerance eps).
    """
    from OCP.BRepGProp import BRepGProp
    from OCP.GProp import GProp_GProps

    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(solid.wrapped, props, eps, False, False)
    c = props.CentreOfMass()
    return props.Mass(), (c.X(), c.Y(), c.Z())


def parts_properties(parts):
    """Volume (mm3) and centroid (mm) of a solid given as signed parts: sum of sign * part.

    OCCT's volume integration is unreliable on a spline surface of revolution
    whose seam a boolean cut crosses (it read a 1 cm layer 15 times too thin
    at the port). A solid cut by the port or by a winding pack is therefore
    measured as the uncut solid minus the removed piece, each integrated on a
    shape without that problem. The result is the CAD reference volume.
    """
    volume = 0.0
    moment = [0.0, 0.0, 0.0]
    for sign, shape in parts:
        v, c = mass_properties(shape)
        volume += sign * v
        for i in range(3):
            moment[i] += sign * v * c[i]
    return volume, tuple(m / volume for m in moment)


def build_solids(cq) -> list[dict]:
    """All RM-M solids, in mm. Each entry: name, solid, tag, role."""
    axis0, axis1 = cq.Vector(0, 0, 0), cq.Vector(0, 0, 1)

    def wire(points):
        return cq.Wire.makePolygon([cq.Vector(r * MM, 0.0, z * MM) for r, z in points], close=True)

    def ring(outer, inner, angle=360.0, start=0.0):
        # A full revolution is seamed where its profile sits. The profile of a 360 degree ring is placed at
        # phi = 90 degrees, away from the port at 0: with the seam under the port, the port cut survives the
        # STEP round trip only as edges whose pcurves cannot be rebuilt, and the imprint then fails.
        turn = 90.0 if angle == 360.0 else start
        profile = wire(outer).rotate(axis0, axis1, turn)
        holes = [wire(inner).rotate(axis0, axis1, turn)] if inner is not None else []
        return cq.Solid.revolve(profile, holes, angle, axis0, axis1)

    def rotate(solid, degrees):
        return solid.rotate(axis0, axis1, degrees)

    boundaries = spec.layer_boundaries()
    solids = []
    plasma = ring(boundaries[0]["outer"], None)
    solids.append({"name": "plasma", "solid": plasma, "parts": [(1, plasma)], "tag": "void", "role": "plasma (neutron source volume)"})
    layer_solids = {}
    for previous, current, layer in zip(boundaries, boundaries[1:], spec.LAYERS):
        layer_solids[layer["name"]] = ring(current["outer"], previous["outer"])

    # Port: a 30 x 30 cm duct through vessel, tank, thermal shield and TiH2 shield.
    sol_filled = ring(boundaries[1]["outer"], None)
    tih2_filled = ring(boundaries[-1]["outer"], None)
    half_w = spec.PORT_WIDTH_CM["value"] * MM / 2.0
    half_h = spec.PORT_HEIGHT_CM["value"] * MM / 2.0
    x0, x1 = 400.0 * MM, 700.0 * MM  # the box only has to span the layers it cuts
    box = cq.Solid.makeBox(x1 - x0, 2 * half_w, 2 * half_h, cq.Vector(x0, -half_w, -half_h))
    duct = tih2_filled.intersect(box).cut(sol_filled)
    for layer in spec.LAYERS:
        solid = layer_solids[layer["name"]]
        parts = [(1, solid)]
        if layer["name"] != "scrape_off_layer":
            parts.append((-1, solid.intersect(box)))
            solid = solid.cut(box)
        solids.append({"name": layer["name"], "solid": solid, "parts": parts, "tag": layer["tag"], "role": layer["role"]})
    solids.append({"name": "port_duct", "solid": duct, "parts": [(1, duct)], "tag": "void", "role": "equatorial port duct (vacuum, no plug)"})

    # TF coils: 18 wedges of a D-shaped ring, each a steel case around a winding pack.
    tf = spec.tf_profiles()
    span = spec.TF_TOROIDAL_SPAN_DEG["value"]
    # Each coil is one solid of homogenised case plus winding pack ("tf_coil" material). With the pack as a
    # separate solid, cad_to_dagmc 0.14.2 reported the full 66-solid model as overlapping (garbage shared
    # volumes after the imprint) although a single coil with its pack imprinted cleanly.
    for k, centre in enumerate(spec.coil_centres_deg()):
        # revolved in place from its own start angle (rotating one solid 18 times gave the mesher false overlaps)
        placed = ring(tf["outer"], tf["inner"], span, start=centre - span / 2.0)
        solids.append({"name": f"tf_coil_{k:02d}", "solid": placed, "parts": [(1, placed)], "tag": "tf_coil",
                       "role": "TF coil, case and winding pack homogenised"})
    return solids


def solid_summary(entry) -> dict:
    solid = entry["solid"]
    volume, c = parts_properties(entry["parts"])
    return {
        "name": entry["name"], "tag": entry["tag"], "role": entry["role"],
        "cad_volume_cm3": volume / MM ** 3,
        "centroid_cm": [c[0] / MM, c[1] / MM, c[2] / MM],
        "valid": bool(solid.isValid()),
    }


def overlap_checks(cq, solids: list[dict]) -> list[dict]:
    """Common volume of neighbouring solids (adjacent layers, adjacent coils, coil to shield, duct to layers)."""
    by_name = {s["name"]: s["solid"] for s in solids}
    pairs = []
    names = [s["name"] for s in solids]
    layers = [n for n in names if n in ("plasma",) or n in {x["name"] for x in spec.LAYERS}]
    pairs += list(zip(layers, layers[1:]))
    for name in ("first_wall_tungsten", "vv_inner_wall", "vv_flibe_channel", "vv_beryllium", "vv_outer_wall",
                 "blanket_flibe", "tank_wall", "thermal_shield", "tih2_shield"):
        pairs.append(("port_duct", name))
    n = spec.N_TF_COILS
    for k in range(n):
        pairs.append((f"tf_coil_{k:02d}", f"tf_coil_{(k + 1) % n:02d}"))
    pairs += [("tf_coil_00", "tih2_shield"), ("tf_coil_00", "port_duct"), ("tf_coil_17", "port_duct")]
    out = []
    for a, b in pairs:
        shared = by_name[a].intersect(by_name[b])
        common = (mass_properties(shared)[0] if shared.Solids() else 0.0) / MM ** 3
        out.append({"a": a, "b": b, "common_volume_cm3": common})
    return out


def export_step(cq, solids: list[dict], path: Path) -> None:
    assembly = cq.Assembly(name="rm_m")
    for entry in solids:
        assembly.add(entry["solid"], name=entry["name"])
    assembly.export(str(path), "STEP")


def reimport_step(cq, path: Path, solids: list[dict]) -> list[dict]:
    """Re-read the STEP file and pair each original solid with its re-imported twin.

    OCCT bounding boxes of the re-imported spline solids are loose by about
    2.5 mm, so a twin is the unused solid with the smallest bounding-box
    difference, which must be under 5 mm. Solids whose volume was measured as
    a single uncut part are also compared by volume (the round trip must keep
    it to 1e-6); the largest such difference is returned with the pairs.
    """
    loaded = cq.importers.importStep(str(path)).solids().vals()
    if len(loaded) != len(solids):
        raise RuntimeError(f"STEP re-import has {len(loaded)} solids, expected {len(solids)}")

    def box(shape):
        b = shape.BoundingBox()
        return (b.xmin, b.xmax, b.ymin, b.ymax, b.zmin, b.zmax)

    loaded_boxes = [box(candidate) for candidate in loaded]
    paired, used, worst_volume = [], set(), 0.0
    for entry in solids:
        want = box(entry["solid"])
        diffs = sorted((max(abs(a - b) for a, b in zip(want, got)), i) for i, got in enumerate(loaded_boxes) if i not in used)
        if not diffs or diffs[0][0] > 5.0:
            raise RuntimeError(f"no STEP counterpart for {entry['name']}")
        best = diffs[0][1]
        used.add(best)
        if len(entry["parts"]) == 1:
            original = mass_properties(entry["solid"])[0]
            worst_volume = max(worst_volume, abs(mass_properties(loaded[best])[0] / original - 1.0))
        paired.append({**entry, "solid": loaded[best]})
    reimport_step.worst_uncut_volume_difference = worst_volume
    return paired


def convert(cq, cad_to_dagmc, solids, h5m: Path, tolerance_cm: float, angular: float, complement_tag: str, rename_void: str | None, imprint: int = 1, backend: str = "cad-to-dagmc-mesher"):
    model = cad_to_dagmc.CadToDagmc()
    for entry in solids:
        tag = rename_void if (rename_void and entry["tag"] == "void") else entry["tag"]
        model.add_cadquery_object(cq.Workplane("XY").add(entry["solid"]), material_tags=[tag])
    t0 = time.time()
    model.export_dagmc_h5m_file(filename=str(h5m), implicit_complement_material_tag=complement_tag, scale_factor=0.1,
                                imprint=imprint, meshing_backend=backend, tolerance=tolerance_cm, angular_tolerance=angular)
    return time.time() - t0


def match_roles(solids: list[dict], facets: dict) -> list[dict]:
    """Pair each CAD solid with the h5m volume of the same volume and centroid; return the role map."""
    summaries = [solid_summary(s) for s in solids]
    used, roles = set(), []
    for item in summaries:
        best, best_score = None, None
        for vid, f in facets.items():
            if vid in used:
                continue
            dist = math.dist(item["centroid_cm"], f["centroid"])
            rel = abs(f["faceted_volume"] / item["cad_volume_cm3"] - 1.0)
            score = dist + 100.0 * rel
            if best_score is None or score < best_score:
                best, best_score = vid, score
        if best is None or best_score > 5.0:
            raise RuntimeError(f"no h5m volume matches {item['name']} (best score {best_score})")
        used.add(best)
        roles.append({**item, "volume_id": best, "faceted_volume_cm3": facets[best]["faceted_volume"],
                      "triangles": facets[best]["triangles"], "h5m_material_tag": item["tag"]})
    return roles


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out-dir", required=True, type=Path)
    ap.add_argument("--tolerance-cm", type=float, default=0.05, help="surface meshing deflection in cm")
    ap.add_argument("--angular-tolerance", type=float, default=0.1, help="surface angular tolerance in radians")
    ap.add_argument("--random-ray", action="store_true", help="also write a second h5m with the void tag replaced by 'filler'")
    ap.add_argument("--imprint-threads", type=int, default=1, help="threads for the imprint step (fewer lowers peak RAM)")
    ap.add_argument("--convert-from", choices=("step", "native"), default="step",
                    help="convert the re-imported STEP solids (the protocol path) or the in-memory CadQuery solids")
    ap.add_argument("--backend", choices=("cadquery", "cad-to-dagmc-mesher"), default="cad-to-dagmc-mesher",
                    help="cad_to_dagmc meshing backend")
    ap.add_argument("--stem", default="rm_m")
    ap.add_argument("--skip-overlap-checks", action="store_true")
    args = ap.parse_args()
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)

    import cadquery as cq
    import cad_to_dagmc
    from importlib.metadata import version

    import h5m_volumes

    log = {"stages": {}}
    t0 = time.time()
    solids = build_solids(cq)
    log["stages"]["build"] = {"seconds": time.time() - t0, "peak_rss_mb": peak_mb()}
    summaries = [solid_summary(s) for s in solids]
    invalid = [s["name"] for s in summaries if not s["valid"] or s["cad_volume_cm3"] <= 0]
    if invalid:
        raise RuntimeError(f"invalid CAD solids: {invalid}")

    t0 = time.time()
    overlaps = [] if args.skip_overlap_checks else overlap_checks(cq, solids)
    log["stages"]["overlap_checks"] = {"seconds": time.time() - t0, "peak_rss_mb": peak_mb()}

    step = out / f"{args.stem}.step"
    t0 = time.time()
    export_step(cq, solids, step)
    reread = reimport_step(cq, step, solids)
    log["stages"]["step"] = {"seconds": time.time() - t0, "peak_rss_mb": peak_mb()}

    h5m = out / f"{args.stem}.h5m"
    source = reread if args.convert_from == "step" else solids
    seconds = convert(cq, cad_to_dagmc, source, h5m, args.tolerance_cm, args.angular_tolerance, "void", None, args.imprint_threads, args.backend)
    log["stages"]["convert"] = {"seconds": seconds, "peak_rss_mb": peak_mb()}
    facets = h5m_volumes.facet_volumes(str(h5m))
    roles = match_roles(solids, facets)
    outputs = {"step": step.name, "step_sha256": sha256(step), "h5m": h5m.name, "h5m_sha256": sha256(h5m),
               "h5m_bytes": h5m.stat().st_size}

    if args.random_ray:
        rr = out / f"{args.stem}_rr.h5m"
        seconds = convert(cq, cad_to_dagmc, source, rr, args.tolerance_cm, args.angular_tolerance, "filler", "filler", args.imprint_threads, args.backend)
        log["stages"]["convert_random_ray"] = {"seconds": seconds, "peak_rss_mb": peak_mb()}
        rr_facets = h5m_volumes.facet_volumes(str(rr))
        same = all(abs(rr_facets[v]["faceted_volume"] / facets[v]["faceted_volume"] - 1.0) < 1e-9 for v in facets)
        outputs.update({"h5m_random_ray": rr.name, "h5m_random_ray_sha256": sha256(rr), "random_ray_geometry_identical": same})

    checks_v = volume_checks(solids)
    if not checks_v["uncut_pass"]:
        raise RuntimeError("adaptive GProp volume of an uncut solid disagrees with Pappus: " + json.dumps(checks_v["uncut"]))
    total_cad = sum(s["cad_volume_cm3"] for s in summaries)
    filled = ring_filled_volume(cq, solids)
    card = spec.model_card(args.tolerance_cm, {
        "builder": "CadQuery directly (Paramak 0.10.0 assessed, not used; see build_rm_m.py docstring)",
        "tools": {"cadquery": version("cadquery"), "cad_to_dagmc": version("cad_to_dagmc"), "python": sys.version.split()[0]},
        "conversion_settings": {"tolerance_cm": args.tolerance_cm, "angular_tolerance_rad": args.angular_tolerance,
                                "scale_factor": 0.1, "implicit_complement_tag": "void", "imprint_threads": args.imprint_threads,
                                "converted_from": args.convert_from, "meshing_backend": args.backend},
        "outputs": outputs,
        "solids": roles,
        "tf_total_cad_volume_cm3": sum(s["cad_volume_cm3"] for s in summaries if s["name"].startswith("tf_coil")),
        "volume_conservation": {"sum_of_solids_up_to_tih2_cm3": sum(s["cad_volume_cm3"] for s in summaries if not s["name"].startswith("tf_coil")),
                                "filled_tih2_envelope_cm3": filled},
        "overlap_checks": overlaps,
        "cad_volume_checks": checks_v,
        "profile_fidelity": spec.fidelity_report(),
        "plasma_volume_m3": next(s["cad_volume_cm3"] for s in summaries if s["name"] == "plasma") / 1e6,
        "total_cad_volume_cm3": total_cad,
        "label_counts": spec.label_counts(spec.model_card()),
        "build_log": log,
    })
    problems = spec.check_labels(card)
    if problems:
        raise RuntimeError("model card labelling problems: " + "; ".join(problems))
    (out / f"{args.stem}_model_card.json").write_text(json.dumps(card, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    (out / f"{args.stem}_roles.json").write_text(json.dumps({"solids": roles}, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"solids": len(solids), "h5m": str(h5m), "peak_rss_mb": peak_mb(), "stages": log["stages"]}, indent=1))
    return 0


def ring_filled_volume(cq, solids) -> float:
    """Volume of the solid envelope inside the TiH2 shield (Pappus on the polygon), for the conservation check."""
    return spec.revolved_volume_cm3(spec.layer_boundaries()[-1]["outer"])


def pappus_expected_cm3() -> dict:
    """Exact volumes of the uncut revolved polygons, by Pappus on the profile polygons."""
    b = spec.layer_boundaries()
    expected = {"plasma": spec.revolved_volume_cm3(b[0]["outer"])}
    for previous, current, layer in zip(b, b[1:], spec.LAYERS):
        expected[layer["name"]] = spec.revolved_volume_cm3(current["outer"]) - spec.revolved_volume_cm3(previous["outer"])
    tf = spec.tf_profiles()
    wedge = spec.TF_TOROIDAL_SPAN_DEG["value"] / 360.0
    expected["tf_coil"] = wedge * (spec.revolved_volume_cm3(tf["outer"]) - spec.revolved_volume_cm3(tf["inner"]))
    return expected


def volume_checks(solids: list[dict], tolerance: float = 1.0e-6) -> dict:
    """Adaptive BRepGProp volume of each uncut solid against Pappus (must agree to 1e-6), and for every
    port-cut solid the direct integral against uncut-minus-removed."""
    expected = pappus_expected_cm3()
    uncut, cut = [], []
    for entry in solids:
        name = entry["name"]
        key = "tf_coil" if name.startswith("tf_coil") else name
        parts_volume = parts_properties(entry["parts"])[0] / MM ** 3
        direct = mass_properties(entry["solid"])[0] / MM ** 3
        if len(entry["parts"]) == 1 and key in expected:
            uncut.append({"name": name, "pappus_cm3": expected[key], "gprop_cm3": direct, "relative_difference": direct / expected[key] - 1.0})
        elif len(entry["parts"]) > 1:
            cut.append({"name": name, "parts_cm3": parts_volume, "direct_cm3": direct, "relative_difference": direct / parts_volume - 1.0})
    return {"tolerance": tolerance, "uncut": uncut, "cut": cut,
            "uncut_pass": all(abs(u["relative_difference"]) <= tolerance for u in uncut),
            "cut_direct_matches_parts": all(abs(c["relative_difference"]) <= tolerance for c in cut)}


if __name__ == "__main__":
    raise SystemExit(main())
