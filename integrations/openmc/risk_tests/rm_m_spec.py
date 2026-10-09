"""Dimensions, labels and poloidal profile geometry of the reference model RM-M.

Pure Python (standard library only): this module defines what RM-M is, labels
every dimension as published or authored, and computes the closed poloidal
profiles that build_rm_m.py turns into CAD. It imports neither CadQuery nor
OpenMC, so its unit tests run anywhere.

The protocol is docs/notes/CAD_TRANSPORT_RISK_TESTS.md ("Reference model
RM-M"); published sources are labelled with the keys of
docs/notes/ARC_GEOMETRY_SOURCES_0.2.md. All lengths here are centimetres.
"""
from __future__ import annotations

import math

PUBLISHED = "published"
AUTHORED = "authored"
LABELS = (PUBLISHED, AUTHORED)

S15 = "S15"
K18 = "K18"
SRC = "ARC_GEOMETRY_SOURCES_0.2.md"

# Plasma (published; S15 Table 1 and K18 Sec 3.2).
MAJOR_RADIUS_CM = 330.0
MINOR_RADIUS_CM = 113.0
ELONGATION = 1.84
TRIANGULARITY = 0.375
FUSION_POWER_W = 525.0e6
DT_Q_EV = 17.6e6
EV_J = 1.602176634e-19
SOURCE_RATE_N_S = FUSION_POWER_W / (DT_Q_EV * EV_J)  # 1.86e20 n/s (SRC-001)

N_TF_COILS = 18
POLOIDAL_POINTS = 240


def dim(value, unit, label, source, note=""):
    """One labelled dimension. Label is 'published' (with a source) or 'authored'."""
    return {"value": value, "unit": unit, "label": label, "source": source, "note": note}


# Layers outward from the plasma boundary. Thickness is measured along the
# local normal; inboard (poloidal angle 180 degrees) and outboard (0 degrees)
# values are given, and the thickness is graded between them by
# t(theta) = t_in + (t_out - t_in) (1 + cos theta) / 2, theta being the polar
# angle of the plasma-boundary point about the magnetic axis. A layer whose two
# values are equal has a constant thickness.
LAYERS = [
    {"name": "scrape_off_layer", "tag": "void", "role": "vacuum scrape-off layer",
     "in": dim(3.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 220-223 cm)"),
     "out": dim(3.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "first_wall_tungsten", "tag": "tungsten", "role": "W armour, vacuum vessel",
     "in": dim(1.0, "cm", PUBLISHED, f"{S15} Sec 5.4.3 (1 cm W)", "Seg20 gives 0.1 cm; S15 value used"),
     "out": dim(1.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "vv_inner_wall", "tag": "structure", "role": "vacuum vessel inner wall (Fe surrogate for Inconel 718)",
     "in": dim(1.0, "cm", PUBLISHED, f"{S15} Sec 5.4.3"),
     "out": dim(1.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "vv_flibe_channel", "tag": "flibe", "role": "vacuum vessel FLiBe channel",
     "in": dim(2.0, "cm", PUBLISHED, f"{S15} Sec 5.4.3"),
     "out": dim(2.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "vv_beryllium", "tag": "beryllium", "role": "non-structural Be multiplier",
     "in": dim(1.0, "cm", PUBLISHED, f"{S15} Sec 5.4.3"),
     "out": dim(1.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "vv_outer_wall", "tag": "structure", "role": "vacuum vessel outer wall (Fe surrogate for Inconel 718)",
     "in": dim(3.0, "cm", PUBLISHED, f"{S15} Sec 5.4.3"),
     "out": dim(3.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "blanket_flibe", "tag": "flibe", "role": "bulk FLiBe blanket",
     "in": dim(20.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 192-212 cm)"),
     "out": dim(100.0, "cm", PUBLISHED, "Seg20 Sec 2 and K18 Sec 1: outboard FLiBe 'about 1 m'")},
    {"name": "tank_wall", "tag": "structure", "role": "blanket tank wall (Fe surrogate)",
     "in": dim(3.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 189-192 cm)"),
     "out": dim(3.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "thermal_shield", "tag": "thermal_insulation", "role": "thermal shield (aluminium silicate in ARC)",
     "in": dim(3.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 186-189 cm)"),
     "out": dim(3.0, "cm", AUTHORED, "same as the published inboard value")},
    {"name": "tih2_shield", "tag": "tih2", "role": "TiH2 neutron shield",
     "in": dim(51.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 135-186 cm)"),
     "out": dim(20.0, "cm", AUTHORED, "published outboard value is a gap (ARC_GEOMETRY_SOURCES_0.2.md, Outboard)")},
]

GAP_TO_TF = dim(1.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 134-135 cm vacuum gaps)")
TF_RADIAL_CM = dim(64.0, "cm", PUBLISHED, f"{S15} Fig. 2 (R 70-134 cm)",
                   "applied as a normal thickness around the whole coil (outside the inboard leg: authored)")

# Authored TF and port values.
TF_TOROIDAL_SPAN_DEG = dim(16.0, "deg", AUTHORED, "toroidal width not published (GAP); 16 of 20 degrees per coil")
TF_CASE_WALL_CM = dim(2.0, "cm", AUTHORED, "case/winding-pack split not published (GAP); S15 gives 15 layers of 4 cm jackets = 60 cm of 64 cm; sets the homogenised pack fraction only")
PORT_WIDTH_CM = dim(30.0, "cm", AUTHORED, "FARIS 0.2 port: 0.30 m x 0.30 m duct (docs/guide/design.md)")
PORT_HEIGHT_CM = dim(30.0, "cm", AUTHORED, "FARIS 0.2 port: 0.30 m x 0.30 m duct (docs/guide/design.md)")
PORT_ANGLE_DEG = dim(0.0, "deg", AUTHORED, "outboard midplane, centred between two TF coils")
BOUNDING_SPHERE_RADIUS_CM = dim(1000.0, "cm", AUTHORED, "vacuum boundary outside the TF coils (largest extent 645 cm); the random-ray box corner is at 962 cm")

WP_FRACTIONS = {  # S15 Sec 4.2.1
    "copper": dim(0.459, "volume fraction", PUBLISHED, f"{S15} Sec 4.2.1"),
    "steel": dim(0.461, "volume fraction", PUBLISHED, f"{S15} Sec 4.2.1"),
    "rebco": dim(0.080, "volume fraction", PUBLISHED, f"{S15} Sec 4.2.1"),
}

# Materials: tag -> description. Compositions and densities come from
# references/demo-input-spec.json (the FARIS material baseline); tags with no
# entry there are authored here and say so.
MATERIAL_BASIS = {
    "void": {"basis": "vacuum (plasma, scrape-off layer, port duct)", "label": PUBLISHED},
    "tungsten": {"basis": "demo-input-spec tungsten-natural, 19300 kg/m3", "label": AUTHORED},
    "structure": {"basis": "demo-input-spec pure-iron-surrogate, 7874 kg/m3; a surrogate, not Inconel 718", "label": AUTHORED},
    "flibe": {"basis": "demo-input-spec flibe-cold-solid, Li2BeF4 90 atom% Li-6, 2137.175 kg/m3 (cold solid state)", "label": AUTHORED},
    "beryllium": {"basis": "Be-9, 1850 kg/m3 (docs/DEMO_INPUT_SPEC.md candidate recipe)", "label": AUTHORED},
    "tih2": {"basis": "demo-input-spec ti-hydride-shield-surrogate, TiH2, 3750 kg/m3", "label": AUTHORED},
    "thermal_insulation": {"basis": "Fe at 1.0 g/cm3: aluminium and silicon are not in the audited library", "label": AUTHORED},
    "tf_coil": {"basis": "case and winding pack homogenised by volume: pack fraction (64 - 2 x case wall)/64 = 0.9375, case steel the rest; "
                         "pack = WP volume fractions 45.9% Cu, 46.1% steel, 8.0% REBCO (published); REBCO fraction represented as Cu "
                              "because Y, Ba and O are not in the audited library (authored substitution); Cu and Fe densities from the baseline",
                     "label": AUTHORED},
    "filler": {"basis": "random-ray only: H-1 at 0.001 g/cm3 in place of void", "label": AUTHORED},
}

# Documented departures from the sources (not hidden): see model card.
DEVIATIONS = [
    {"id": "D1", "what": "inboard absolute radii",
     "detail": "a = 1.13 m (published, S15 Table 1) puts the inboard separatrix at R = 217 cm, while S15 Fig. 2 places the "
               "plasma/scrape-off boundary at 223 cm (implied a = 1.07 m). All published layer thicknesses and the 64 cm TF leg are kept; "
               "the inboard build therefore sits 6 cm closer to the machine axis than Fig. 2: TF inboard leg R = 64-128 cm, not 70-134 cm.",
     "label": AUTHORED},
    {"id": "D2", "what": "plasma shape convention",
     "detail": "R = R0 + a cos(t + delta sin t), Z = kappa a sin t, the convention of openmc-plasma-source, so the CAD plasma "
               "boundary and the source use one definition of kappa and delta.",
     "label": AUTHORED},
    {"id": "D3", "what": "TF coil poloidal shape",
     "detail": "D-shaped by construction: a constant 64 cm normal offset of the shield-plus-gap boundary, revolved through a "
               "16 degree toroidal wedge (not a Princeton constant-tension D, not planar).",
     "label": AUTHORED},
    {"id": "D4", "what": "TF coil case and winding pack",
     "detail": "each coil is one homogenised solid (pack fraction 0.9375 from the authored 2 cm case wall, rest case steel). "
               "A separate pack solid made cad_to_dagmc 0.14.2 report the full model as overlapping; the case wall dimension "
               "only sets the homogenised fraction.",
     "label": AUTHORED},
    {"id": "D5", "what": "poloidal profiles are closed polygons",
     "detail": "every profile (plasma, layer boundaries, TF rings) joins POLOIDAL_POINTS sampled points with straight segments. "
               "OCCT booleans and volume integration failed on periodic-spline surfaces of revolution: an imprint of the layers "
               "plus one coil gave an inverted scrape-off-layer region and a filled-torus tungsten region, and default and adaptive "
               "volumes disagreed by 1 % to 15x. With polygons the imprint gives one region per solid and the volumes agree. "
               "The geometric cost is the chord sag and thickness departure in profile_fidelity.",
     "label": AUTHORED},
]


def layer_thickness_cm(layer: dict, cos_theta: float) -> float:
    """Normal thickness of a layer at the given cosine of the poloidal polar angle."""
    t_in = layer["in"]["value"]
    t_out = layer["out"]["value"]
    return t_in + (t_out - t_in) * (1.0 + cos_theta) / 2.0


def plasma_point(t: float) -> tuple[float, float]:
    """Plasma-boundary point (R, Z) in cm at poloidal parameter t."""
    return (MAJOR_RADIUS_CM + MINOR_RADIUS_CM * math.cos(t + TRIANGULARITY * math.sin(t)),
            ELONGATION * MINOR_RADIUS_CM * math.sin(t))


def plasma_tangent(t: float) -> tuple[float, float]:
    d_theta = 1.0 + TRIANGULARITY * math.cos(t)
    return (-MINOR_RADIUS_CM * math.sin(t + TRIANGULARITY * math.sin(t)) * d_theta,
            ELONGATION * MINOR_RADIUS_CM * math.cos(t))


def offset_point(t: float, distance_fn) -> tuple[float, float]:
    """Point of the analytic offset curve at plasma parameter t: the plasma point moved along its outward normal."""
    r, z = plasma_point(t)
    tr, tz = plasma_tangent(t)
    norm = math.hypot(tr, tz)
    nr, nz = tz / norm, -tr / norm
    dr, dz = r - MAJOR_RADIUS_CM, z
    d = distance_fn(dr / math.hypot(dr, dz))
    return (r + nr * d, z + nz * d)


def offset_profile(distance_fn, n: int = POLOIDAL_POINTS) -> list[tuple[float, float]]:
    """Closed profile: plasma boundary offset along the outward normal by distance_fn(cos_theta).

    Returns n points, counter-clockwise in (R, Z), first point at the outboard
    midplane. The last point is not repeated. CAD joins them with straight
    segments (deviation D5).
    """
    return [offset_point(2.0 * math.pi * i / n, distance_fn) for i in range(n)]


def point_segment_distance(p, a, b) -> float:
    ax, ay, bx, by, px, py = a[0], a[1], b[0], b[1], p[0], p[1]
    dx, dy = bx - ax, by - ay
    length2 = dx * dx + dy * dy
    u = 0.0 if length2 == 0.0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / length2))
    return math.hypot(px - (ax + u * dx), py - (ay + u * dy))


def chord_sag_cm(distance_fn, n: int = POLOIDAL_POINTS) -> float:
    """Largest distance from the analytic offset curve (at segment mid-parameters) to its polygon chord."""
    pts = offset_profile(distance_fn, n)
    worst = 0.0
    for i in range(n):
        mid = offset_point(2.0 * math.pi * (i + 0.5) / n, distance_fn)
        worst = max(worst, point_segment_distance(mid, pts[i], pts[(i + 1) % n]))
    return worst


def thickness_departure_cm(n: int = POLOIDAL_POINTS) -> dict:
    """Largest |polygon normal thickness - intended thickness| over segment midpoints, per layer.

    At each midpoint of a layer's outer polygon the thickness is the distance to
    the inner polygon; the intended value is layer_thickness_cm at the plasma
    point's polar angle. Includes the effect of the poloidal grading.
    """
    names = [layer["name"] for layer in LAYERS]
    inner = offset_profile(lambda c: 0.0, n)
    result = {}
    for i, layer in enumerate(LAYERS):
        outer = offset_profile(cumulative_distance_fn(names[: i + 1]), n)
        worst = 0.0
        for j in range(n):
            t = 2.0 * math.pi * (j + 0.5) / n
            r, z = plasma_point(t)
            dr = r - MAJOR_RADIUS_CM
            cos_theta = dr / math.hypot(dr, z)
            q = offset_point(t, cumulative_distance_fn(names[: i + 1]))
            measured = min(point_segment_distance(q, inner[k], inner[(k + 1) % n]) for k in range(n))
            worst = max(worst, abs(measured - layer_thickness_cm(layer, cos_theta)))
        result[layer["name"]] = worst
        inner = outer
    return result


def fidelity_report(n: int = POLOIDAL_POINTS) -> dict:
    names = [layer["name"] for layer in LAYERS]
    sags = {"plasma": chord_sag_cm(lambda c: 0.0, n)}
    for i, layer in enumerate(LAYERS):
        sags[layer["name"]] = chord_sag_cm(cumulative_distance_fn(names[: i + 1]), n)
    sags["tf_inner"] = chord_sag_cm(cumulative_distance_fn(names, GAP_TO_TF["value"]), n)
    sags["tf_outer"] = chord_sag_cm(lambda c: cumulative_distance_fn(names, GAP_TO_TF["value"])(c) + TF_RADIAL_CM["value"], n)
    dep = thickness_departure_cm(n)
    return {"points": n, "max_chord_sag_cm": max(sags.values()), "chord_sag_cm": sags,
            "max_thickness_departure_cm": max(dep.values()), "thickness_departure_cm": dep}


def cumulative_distance_fn(layer_names: list[str], extra_cm: float = 0.0):
    """Distance function summing the named layers' thickness plus a constant."""
    chosen = [layer for layer in LAYERS if layer["name"] in layer_names]
    return lambda cos_theta: extra_cm + sum(layer_thickness_cm(layer, cos_theta) for layer in chosen)


def layer_boundaries() -> list[dict]:
    """Ordered named profiles: the plasma boundary and the outer boundary of every layer."""
    names = [layer["name"] for layer in LAYERS]
    out = [{"name": "plasma", "outer": offset_profile(lambda c: 0.0)}]
    for i, layer in enumerate(LAYERS):
        out.append({"name": layer["name"], "outer": offset_profile(cumulative_distance_fn(names[: i + 1]))})
    return out


def tf_profiles() -> dict:
    """Poloidal D-ring profiles for the TF coil case and winding pack."""
    names = [layer["name"] for layer in LAYERS]
    base = cumulative_distance_fn(names, GAP_TO_TF["value"])
    radial = TF_RADIAL_CM["value"]
    return {
        "inner": offset_profile(base),
        "outer": offset_profile(lambda c: base(c) + radial),
    }


def polygon_area(points: list[tuple[float, float]]) -> float:
    """Signed area of a closed polygon (counter-clockwise positive)."""
    total = 0.0
    for (x0, y0), (x1, y1) in zip(points, points[1:] + points[:1]):
        total += x0 * y1 - x1 * y0
    return total / 2.0


def revolved_volume_cm3(points: list[tuple[float, float]]) -> float:
    """Pappus volume of the closed polygon revolved through 360 degrees about the Z axis."""
    cx = cy = 0.0
    a = polygon_area(points)
    for (x0, y0), (x1, y1) in zip(points, points[1:] + points[:1]):
        cross = x0 * y1 - x1 * y0
        cx += (x0 + x1) * cross
    cx /= 6.0 * a
    return 2.0 * math.pi * cx * abs(a)


def coil_centres_deg(n: int = N_TF_COILS) -> list[float]:
    """Toroidal centre angles: the port sits at 0 degrees, between coils."""
    pitch = 360.0 / n
    return [pitch / 2.0 + k * pitch for k in range(n)]


def model_card(tolerance_cm: float | None = None, extra: dict | None = None) -> dict:
    """The model card: every dimension with its label, source and note."""
    layers = {}
    for layer in LAYERS:
        layers[layer["name"]] = {"tag": layer["tag"], "role": layer["role"], "inboard": layer["in"], "outboard": layer["out"]}
    card = {
        "schema": "faris-rm-m-model-card/v1",
        "protocol": "docs/notes/CAD_TRANSPORT_RISK_TESTS.md",
        "units": "cm in this card; CAD built in mm (x10), converted to cm by cad_to_dagmc scale_factor 0.1",
        "plasma": {
            "major_radius": dim(MAJOR_RADIUS_CM, "cm", PUBLISHED, f"{S15} Table 1"),
            "minor_radius": dim(MINOR_RADIUS_CM, "cm", PUBLISHED, f"{S15} Table 1", "S15 Fig. 2 implies 107 cm; see D1"),
            "elongation": dim(ELONGATION, "1", PUBLISHED, f"{S15} Table 1"),
            "triangularity": dim(TRIANGULARITY, "1", PUBLISHED, f"{K18} Sec 3.2 (quoting S15)"),
            "fusion_power": dim(FUSION_POWER_W, "W", PUBLISHED, f"{S15} Table 1"),
            "source_rate": dim(SOURCE_RATE_N_S, "n/s", PUBLISHED, "Seg20 Sec 2; 525 MW / 17.6 MeV (SRC-001)"),
        },
        "layers": layers,
        "gap_to_tf": GAP_TO_TF,
        "tf_coils": {
            "count": dim(N_TF_COILS, "1", PUBLISHED, f"{S15} Sec 4.2"),
            "radial_thickness": TF_RADIAL_CM,
            "toroidal_span": TF_TOROIDAL_SPAN_DEG,
            "case_wall": TF_CASE_WALL_CM,
            "winding_pack_fractions": WP_FRACTIONS,
            "coil_centres_deg": dim(coil_centres_deg(), "deg", AUTHORED, "evenly spaced, port between two coils"),
        },
        "port": {
            "width": PORT_WIDTH_CM, "height": PORT_HEIGHT_CM, "angle": PORT_ANGLE_DEG,
            "extent": dim("from the scrape-off layer outer surface through vessel, tank, thermal shield and TiH2 shield", "", AUTHORED, "no plug"),
        },
        "bounding_sphere_radius": BOUNDING_SPHERE_RADIUS_CM,
        "materials": MATERIAL_BASIS,
        "deviations": DEVIATIONS,
    }
    if tolerance_cm is not None:
        card["conversion"] = {"tolerance_cm": tolerance_cm}
    if extra:
        card.update(extra)
    return card


def iter_dimensions(card: dict):
    """Yield (path, dimension dict) for every labelled dimension in a card."""
    def walk(node, path):
        if isinstance(node, dict):
            if "label" in node and "value" in node and "source" in node:
                yield path, node
            else:
                for key, value in node.items():
                    yield from walk(value, path + [key])
    yield from walk(card, [])


def check_labels(card: dict) -> list[str]:
    """Return problems: a dimension without a valid label, or published without a source."""
    problems = []
    count = 0
    for path, d in iter_dimensions(card):
        count += 1
        name = ".".join(path)
        if d["label"] not in LABELS:
            problems.append(f"{name}: label {d['label']!r} is neither published nor authored")
        if d["label"] == PUBLISHED and not d["source"]:
            problems.append(f"{name}: published without a source")
    if count == 0:
        problems.append("card has no labelled dimensions")
    return problems


def label_counts(card: dict) -> dict:
    counts = {PUBLISHED: 0, AUTHORED: 0}
    for _, d in iter_dimensions(card):
        if d["label"] in counts:
            counts[d["label"]] += 1
    return counts
