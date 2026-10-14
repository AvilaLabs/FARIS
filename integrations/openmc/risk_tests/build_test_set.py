#!/usr/bin/env python3
"""Build the CAD transport test set: four deliberately different reactor models, STEP plus design file.

Run with the CAD environment (cadquery 2.8, numpy):

    build_test_set.py --model {st-compact,no-port,bent-duct,large} --out DIR [--design] [--verify]

The models reuse the RM-M profile geometry of rm_m_spec.py (graded layers around a D-shaped plasma, a D-shaped
TF ring cut into wedges) and the helpers of build_rm_m.py. The spec module keeps its dimensions in module
globals, so each model is applied by temporarily setting those globals (`configured`). Lengths are centimetres
in the parameters and millimetres in the STEP file, as for RM-M.

    st-compact  spherical tokamak, R 1.7 m, a 1.0 m, kappa 2.5, 12 coils, inboard first wall + 20 cm TiH2 only
    no-port     RM-M dimensions, no port duct
    bent-duct   RM-M dimensions, dog-leg port duct with no line of sight from plasma to coils
    large       RM-M scaled by 1.8 in every linear dimension

Nothing here runs transport. `--design` writes the design file with `faris design init` plus the RM-M roles;
`--verify` runs the cheap CAD checks (validity, overlaps, plasma containment, line of sight through the duct).
"""
from __future__ import annotations

import argparse
import contextlib
import copy
import hashlib
import json
import math
from pathlib import Path
import random
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rm_m_spec as spec  # noqa: E402
import build_rm_m  # noqa: E402

MM = build_rm_m.MM
REPO = Path(__file__).resolve().parents[3]
RM_M_DESIGN = REPO / "scenarios" / "rm-m" / "rm-m.faris-design.json"
RM_M_HISTORY = REPO / "scenarios" / "rm-m" / "rm-m.operating-history.json"
FARIS = "/home/connoravila/.cache/avila-night/faris-wt/target-main/release/faris"
CAD_PYTHON = "/home/connoravila/.cache/avila-night/feasibility-cad/cadvenv/bin/python"
MODELS = ("st-compact", "no-port", "bent-duct", "large")

# st-compact: layers that are absent on the inboard side, and where they end (cosine of the poloidal polar angle).
PINCH_START = -0.3
PINCH_RAMP_THIN = 0.1
PINCH_RAMP_THICK = 0.6
PINCHED = ("vv_inner_wall", "vv_flibe_channel", "vv_beryllium", "vv_outer_wall", "blanket_flibe", "tank_wall",
           "thermal_shield")


def scaled_layers(factor: float) -> list[dict]:
    """The RM-M layer stack with every thickness multiplied by factor."""
    layers = copy.deepcopy(spec.LAYERS)
    for layer in layers:
        for side in ("in", "out"):
            layer[side]["value"] = layer[side]["value"] * factor
    return layers


def pinch_function(full_cm: float, ramp: float):
    """Thickness as a function of cos(polar angle): zero inboard of PINCH_START, rising to full_cm over `ramp`."""
    def fn(cos_theta: float) -> float:
        return full_cm * min(1.0, max(0.0, (cos_theta - PINCH_START) / ramp))
    return fn


def st_layers() -> list[dict]:
    """RM-M's outboard stack all round except that the vessel, blanket, tank and thermal shield stop inboard."""
    layers = copy.deepcopy(spec.LAYERS)
    for layer in layers:
        layer["in"]["value"] = 20.0 if layer["name"] == "tih2_shield" else layer["in"]["value"]
        if layer["name"] in PINCHED:
            full = layer["out"]["value"]
            layer["fn"] = pinch_function(full, PINCH_RAMP_THICK if full > 10.0 else PINCH_RAMP_THIN)
            layer["in"]["value"] = 0.0
            layer["in"]["note"] = "absent inboard: the layer ends where cos(polar angle) = %.1f" % PINCH_START
    return layers


def coil_span_deg(n_coils: int) -> float:
    """Toroidal width of a coil: 80 % of the pitch, as RM-M (16 of 20 degrees)."""
    return 0.8 * 360.0 / n_coils


def model_params(name: str) -> dict:
    """All dimensions of one model, in cm. Port keys: kind None, "straight" or "dogleg"."""
    if name not in MODELS:
        raise ValueError(f"unknown model {name!r}; expected one of {MODELS}")
    rm = {
        "major": spec.MAJOR_RADIUS_CM, "minor": spec.MINOR_RADIUS_CM, "elongation": spec.ELONGATION,
        "triangularity": spec.TRIANGULARITY, "n_coils": spec.N_TF_COILS, "span": spec.TF_TOROIDAL_SPAN_DEG["value"],
        "tf_radial": spec.TF_RADIAL_CM["value"], "gap": spec.GAP_TO_TF["value"],
        "port_w": spec.PORT_WIDTH_CM["value"], "port_h": spec.PORT_HEIGHT_CM["value"],
    }
    if name == "st-compact":
        p = dict(rm, major=170.0, minor=100.0, elongation=2.5, n_coils=12, span=coil_span_deg(12), tf_radial=20.0,
                 layers=st_layers(), port="straight", fusion_power_mw=200.0, title="Compact spherical tokamak test model")
    elif name == "no-port":
        p = dict(rm, layers=scaled_layers(1.0), port=None, fusion_power_mw=525.0, title="RM-M without a port duct (deep shielding test model)")
    elif name == "bent-duct":
        p = dict(rm, layers=scaled_layers(1.0), port="dogleg", fusion_power_mw=525.0, title="RM-M with a dog-leg port duct (test model)")
    else:
        f = 1.8
        p = dict(rm, major=rm["major"] * f, minor=rm["minor"] * f, tf_radial=rm["tf_radial"] * f, gap=rm["gap"] * f,
                 port_w=rm["port_w"] * f, port_h=rm["port_h"] * f, layers=scaled_layers(f), port="straight",
                 fusion_power_mw=2000.0, title="RM-M scaled by 1.8 (large test model)")
    p["name"] = name
    p["outboard_total"] = sum(layer["out"]["value"] for layer in p["layers"])
    p["sol_out"] = p["layers"][0]["out"]["value"]
    return p


ENERGY_PER_REACTION_J = 17.6e6 * 1.602176634e-19  # D-T, as crates/faris-engine/src/design/source.rs
CLOSURE_TOLERANCE = 1.0e-3


def reactivity_dt(t_ev: float) -> float:
    """D-T reactivity in m3/s (Bosch-Hale fit as NeSST.reac_DT, truncated exponents included)."""
    kev = t_ev / 1.0e3
    if not kev > 0.0:
        return 0.0
    numerator = ((-0.10675e-3 * kev + 4.6064e-3) * kev + 15.136e-3) * kev
    denominator = ((0.01366e-3 * kev + 13.5e-3) * kev + 75.189e-3) * kev + 1.0
    xi = 6.6610 * kev ** -0.333333333
    eta = 1.0 - numerator / denominator
    return 643.41e-22 * eta ** -0.833333333 * xi * xi * math.exp(-3.0 * eta ** 0.333333333 * xi)


def implied_fusion_power_mw(plasma: dict, cells: int = 300) -> float:
    """The D-T fusion power the plasma block's profiles imply: the integral of crates/faris-engine/src/design/source.rs
    (H mode, equal D and T), on a midpoint grid of cells x cells. 300 cells are within 1e-5 of the Rust 1000 x 1000."""
    major, minor, kappa = plasma["major_radius_m"], plasma["minor_radius_m"], plasma["elongation"]
    delta, shift, pedestal = plasma["triangularity"], plasma["shafranov_shift_m"], plasma["pedestal_radius_m"]
    n, t = plasma["ion_density_m3"], plasma["ion_temperature_ev"]
    fuel = plasma["fuel"]
    da, dalpha = minor / cells, 2.0 * math.pi / cells
    angles = [(j + 0.5) * dalpha for j in range(cells)]
    total = 0.0
    for i in range(cells):
        a = (i + 0.5) * da
        if a < pedestal:
            density = (n["centre"] - n["pedestal"]) * (1.0 - (a / pedestal) ** 2) ** n["peaking_factor"] + n["pedestal"]
            temperature = t["pedestal"] + (t["centre"] - t["pedestal"]) * (
                1.0 - (a / pedestal) ** t["beta"]) ** t["peaking_factor"]
        else:
            density = (n["pedestal"] - n["separatrix"]) * (minor - a) / (minor - pedestal) + n["separatrix"]
            temperature = t["separatrix"] + (t["pedestal"] - t["separatrix"]) * (minor - a) / (minor - pedestal)
        rate = density ** 2 * fuel["D"] * fuel["T"] * reactivity_dt(temperature)
        ring = 0.0
        for alpha in angles:
            theta = alpha + delta * math.sin(alpha)
            radius = major + a * math.cos(theta) + shift * (1.0 - (a / minor) ** 2)
            d_r_da = math.cos(theta) - 2.0 * shift * a / (minor * minor)
            d_r_dalpha = -a * math.sin(theta) * (1.0 + delta * math.cos(alpha))
            ring += radius * abs(d_r_da * kappa * a * math.cos(alpha) - d_r_dalpha * kappa * math.sin(alpha))
        total += rate * ring
    return total * da * dalpha * 2.0 * math.pi * ENERGY_PER_REACTION_J / 1.0e6


def close_density(plasma: dict) -> float:
    """Multiply the centre, pedestal and separatrix ion densities of plasma by one factor so the profiles imply its
    declared fusion power (the implied power goes as the density squared; the shape is unchanged). Returns the factor."""
    factor = math.sqrt(plasma["fusion_power_mw"] / implied_fusion_power_mw(plasma))
    for key in ("centre", "pedestal", "separatrix"):
        plasma["ion_density_m3"][key] *= factor
    if abs(implied_fusion_power_mw(plasma) / plasma["fusion_power_mw"] - 1.0) > CLOSURE_TOLERANCE / 10:
        raise ValueError("the scaled profiles do not close at the declared fusion power")
    return factor


def port_boxes(p: dict) -> list[tuple[float, float, float, float, float, float]]:
    """Cutter boxes (x0, x1, y0, y1, z0, z1) in cm; their union is the duct cutter. Empty without a port."""
    if p["port"] is None:
        return []
    w, h = p["port_w"], p["port_h"]
    r_out = p["major"] + p["minor"]
    x0 = p["major"] + 0.6 * p["minor"]  # inside the plasma; the box only has to span the layers it cuts
    x1 = r_out + 2.0 * p["outboard_total"]
    if p["port"] == "straight":
        return [(x0, x1, -w / 2, w / 2, -h / 2, h / 2)]
    # dog-leg: radial, then a toroidal step of 1.5 widths over a short lateral leg, then radial again
    xm = r_out + (p["sol_out"] + p["outboard_total"]) / 2.0
    lat = w / 2.0
    s = 1.5 * w
    return [(x0, xm + lat / 2, -w / 2, w / 2, -h / 2, h / 2),
            (xm - lat / 2, xm + lat / 2, -w / 2, s + w / 2, -h / 2, h / 2),
            (xm - lat / 2, x1, s - w / 2, s + w / 2, -h / 2, h / 2)]


@contextlib.contextmanager
def configured(p: dict):
    """Set the rm_m_spec module globals to one model's dimensions, restoring them afterwards."""
    names = ["MAJOR_RADIUS_CM", "MINOR_RADIUS_CM", "ELONGATION", "TRIANGULARITY", "LAYERS", "GAP_TO_TF", "TF_RADIAL_CM",
             "TF_TOROIDAL_SPAN_DEG", "N_TF_COILS", "layer_thickness_cm"]
    saved = {n: getattr(spec, n) for n in names}
    original = spec.layer_thickness_cm

    def thickness(layer, cos_theta):
        return layer["fn"](cos_theta) if "fn" in layer else original(layer, cos_theta)

    spec.MAJOR_RADIUS_CM, spec.MINOR_RADIUS_CM = p["major"], p["minor"]
    spec.ELONGATION, spec.TRIANGULARITY = p["elongation"], p["triangularity"]
    spec.LAYERS = p["layers"]
    spec.GAP_TO_TF = dict(saved["GAP_TO_TF"], value=p["gap"])
    spec.TF_RADIAL_CM = dict(saved["TF_RADIAL_CM"], value=p["tf_radial"])
    spec.TF_TOROIDAL_SPAN_DEG = dict(saved["TF_TOROIDAL_SPAN_DEG"], value=p["span"])
    spec.N_TF_COILS = p["n_coils"]
    spec.layer_thickness_cm = thickness
    try:
        yield
    finally:
        for n, v in saved.items():
            setattr(spec, n, v)


def layer_regions(inner: list, outer: list, eps: float = 1.0e-9) -> list[dict]:
    """Profile(s) of one layer between two boundary polygons sampled at the same parameters.

    Where the layer has thickness the polygons differ; where it has none (a layer that ends) they coincide
    exactly. A full layer is an annulus (outer, hole inner). A layer that ends is one crescent per run of
    thick samples, closed at the shared tip vertices.
    """
    n = len(outer)
    thin = [abs(outer[i][0] - inner[i][0]) + abs(outer[i][1] - inner[i][1]) < eps for i in range(n)]
    if not any(thin):
        return [{"kind": "annulus", "outer": outer, "hole": inner}]
    if all(thin):
        raise ValueError("layer has zero thickness everywhere")
    start = next(i for i in range(n) if thin[i])  # a tip: runs are read from here
    runs, run = [], []
    for k in range(1, n + 1):
        i = (start + k) % n
        if thin[i]:
            if run:
                runs.append(run)
            run = []
        else:
            run.append(i)
    regions = []
    for run in runs:
        before, after = (run[0] - 1) % n, (run[-1] + 1) % n
        chain = [outer[before]] + [outer[i] for i in run] + [outer[after]] + [inner[i] for i in reversed(run)]
        regions.append({"kind": "crescent", "outer": chain, "hole": None})
    return regions


def build_solids(cq, p: dict) -> list[dict]:
    """All solids of one model, in mm, in STEP order: plasma, layers, port duct, TF coils."""
    axis0, axis1 = cq.Vector(0, 0, 0), cq.Vector(0, 0, 1)

    def wire(points):
        return cq.Wire.makePolygon([cq.Vector(r * MM, 0.0, z * MM) for r, z in points], close=True)

    def ring(outer, inner, angle=360.0, start=0.0):
        # profile of a full revolution at phi = 90 degrees, away from the port at 0 (see build_rm_m.build_solids)
        turn = 90.0 if angle == 360.0 else start
        profile = wire(outer).rotate(axis0, axis1, turn)
        holes = [wire(inner).rotate(axis0, axis1, turn)] if inner is not None else []
        return cq.Solid.revolve(profile, holes, angle, axis0, axis1)

    with configured(p):
        boundaries = spec.layer_boundaries()
        layers = spec.LAYERS
        tf = spec.tf_profiles()
        centres = spec.coil_centres_deg(p["n_coils"])
    solids = []
    plasma = ring(boundaries[0]["outer"], None)
    solids.append({"name": "plasma", "solid": plasma, "tag": "void", "role": "plasma (neutron source volume)"})
    layer_solids = {}
    for previous, current, layer in zip(boundaries, boundaries[1:], layers):
        regions = layer_regions(previous["outer"], current["outer"])
        if len(regions) != 1:
            raise RuntimeError(f"layer {layer['name']} splits into {len(regions)} solids")
        layer_solids[layer["name"]] = ring(regions[0]["outer"], regions[0]["hole"])

    boxes = port_boxes(p)
    cutter = None
    if boxes:
        made = [cq.Solid.makeBox((x1 - x0) * MM, (y1 - y0) * MM, (z1 - z0) * MM, cq.Vector(x0 * MM, y0 * MM, z0 * MM))
                for x0, x1, y0, y1, z0, z1 in boxes]
        cutter = made[0]
        for extra in made[1:]:
            cutter = cutter.fuse(extra)
        cutter = cutter.clean()
        sol_filled = ring(boundaries[1]["outer"], None)
        envelope = ring(boundaries[-1]["outer"], None)
        duct = envelope.intersect(cutter).cut(sol_filled)
    for layer in layers:
        solid = layer_solids[layer["name"]]
        if cutter is not None and layer["name"] != "scrape_off_layer":
            solid = solid.cut(cutter)
        solids.append({"name": layer["name"], "solid": solid, "tag": layer["tag"], "role": layer["role"]})
    if cutter is not None:
        solids.append({"name": "port_duct", "solid": duct, "tag": "void",
                       "role": "dog-leg port duct (vacuum, no plug)" if p["port"] == "dogleg" else "equatorial port duct (vacuum, no plug)"})
    for k, centre in enumerate(centres):
        placed = ring(tf["outer"], tf["inner"], p["span"], start=centre - p["span"] / 2.0)
        solids.append({"name": f"tf_coil_{k:02d}", "solid": placed, "tag": "tf_coil",
                       "role": "TF coil, case and winding pack homogenised"})
    return solids


def export_step(cq, solids: list[dict], path: Path) -> None:
    build_rm_m.export_step(cq, solids, path)


# ---------------------------------------------------------------- design file

def model_plasma(p: dict, template: dict) -> dict:
    """The plasma block of one model: RM-M's profiles on the model's own dimensions and fusion power, with the ion
    density scaled so the profiles close at that power."""
    name = p["name"]
    plasma = copy.deepcopy(template)
    plasma.update({"chamber_solid_id": "plasma", "major_radius_m": p["major"] / 100.0, "minor_radius_m": p["minor"] / 100.0,
                   "elongation": p["elongation"], "triangularity": p["triangularity"],
                   "pedestal_radius_m": round(0.9 * p["minor"] / 100.0, 12), "fusion_power_mw": p["fusion_power_mw"]})
    note = f"Authored for the {name} test-set model, not from a reference."
    plasma["provenance"] = {"label": "authored", "note": note + " Major radius, minor radius, elongation and fusion power."}
    plasma["field_provenance"]["triangularity"] = {"label": "authored", "note": note + " Triangularity 0.375 as RM-M."}
    plasma["field_provenance"]["pedestal_radius_m"]["note"] = "0.9 of the minor radius, as in RM-M."
    factor = close_density(plasma)
    plasma["field_provenance"]["ion_density_m3"] = {
        "label": "authored",
        "note": f"Profile shape is the RM-M S0 model card's; the level is RM-M's scaled by {factor:.6f} so the profiles "
                f"close at this model's {p['fusion_power_mw']:g} MW fusion power. A normalisation, not a published density."}
    return plasma


def write_design(p: dict, step: Path, out_dir: Path, faris: str, cad_python: str) -> Path:
    """`faris design init` on the STEP, then the RM-M materials, roles, group, limits and a model-specific plasma."""
    name = p["name"]
    draft = out_dir / f"{name}.draft.faris-design.json"
    if draft.exists():
        draft.unlink()
    subprocess.run([faris, "design", "init", str(step), "--out", str(draft), "--cad-python", cad_python], check=True)
    d = json.loads(draft.read_text())
    template = json.loads(RM_M_DESIGN.read_text())
    by_step_name = {s["step_name"]: s for s in template["solids"]}

    d["id"] = name
    d["title"] = p["title"]
    d["description"] = (f"Test-set model {name} of the FARIS CAD transport tests, built in CadQuery by "
                        "integrations/openmc/risk_tests/build_test_set.py from the RM-M profile geometry. "
                        "It is an authored test model, not a design.")
    d["cad"]["step_file"] = step.name
    d["cad"]["implicit_complement_material_id"] = template["cad"]["implicit_complement_material_id"]
    d["materials"] = copy.deepcopy(template["materials"])
    for s in d["solids"]:
        key = "tf_coil_00" if s["step_name"].startswith("tf_coil_") else s["step_name"]
        ref = by_step_name[key]
        for field in ("material_id", "role", "replacement_group_id", "tally"):
            s[field] = ref[field]
        if s["step_name"] == "port_duct" and p["port"] == "dogleg":
            s["role"] = ref["role"]
    group = copy.deepcopy(template["replacement_groups"])
    group[0]["label"] = f"All {p['n_coils']} TF coils"
    d["replacement_groups"] = group
    d["plasma"] = model_plasma(p, template["plasma"])
    history = out_dir / f"{name}.operating-history.json"
    history.write_bytes(RM_M_HISTORY.read_bytes())
    d["operating_scenario"] = {"path": history.name, "sha256": build_rm_m.sha256(history)}
    d["references"] = [r for r in template["references"] if r["id"] == "sorbom-2015"]
    d["assumptions"] = [
        f"Test-set model {name}: authored geometry that reuses the RM-M profile code; it is not a design and has no published source.",
        "Materials, roles and the TF replacement limit are those of RM-M (scenarios/rm-m); the limit is a REBCO screening value, not a qualified allowable.",
        "Each TF coil is one homogenised solid of winding pack and case, as in RM-M.",
        "The solids named plasma, scrape_off_layer and port_duct are not tallied; they are empty.",
        {"st-compact": "Spherical tokamak: the vessel, blanket, tank and thermal shield end inboard; the inboard build is the first wall and a 20 cm TiH2 shield; the TF coil is 20 cm thick radially.",
         "no-port": "RM-M dimensions without the port duct.",
         "bent-duct": "RM-M dimensions; the port duct is radial for the first half of the stack, steps toroidally by 1.5 duct widths over a lateral leg half a width long, then continues radially: no straight line of sight from plasma to coils.",
         "large": "RM-M with every linear dimension multiplied by 1.8."}[name],
    ]
    path = out_dir / f"{name}.faris-design.json"
    path.write_text(json.dumps(d, indent=2) + "\n")
    draft.unlink()
    return path


# ---------------------------------------------------------------- verification

def polar_table(points, axis_r):
    import numpy as np
    pts = np.array(points)
    ang = np.arctan2(pts[:, 1], pts[:, 0] - axis_r)
    rad = np.hypot(pts[:, 1], pts[:, 0] - axis_r)
    order = np.argsort(ang)
    return ang[order], rad[order]


def make_classifier(p: dict):
    """Vectorised classification of points (cm): 0 void inside the shield envelope, 1 plasma, 2 shield, 3 coil, 4 void outside."""
    import numpy as np
    with configured(p):
        boundaries = spec.layer_boundaries()
        tf = spec.tf_profiles()
    r0 = p["major"]
    plasma_t, sol_t, env_t = (polar_table(boundaries[i]["outer"], r0) for i in (0, 1, -1))
    tf_in_t, tf_out_t = polar_table(tf["inner"], r0), polar_table(tf["outer"], r0)
    boxes = port_boxes(p)
    pitch, half = 360.0 / p["n_coils"], p["span"] / 2.0

    def inside(table, rr, z):
        ang = np.arctan2(z, rr - r0)
        return np.hypot(rr - r0, z) <= np.interp(ang, table[0], table[1], period=2 * math.pi)

    def classify(x, y, z):
        rr = np.hypot(x, y)
        in_plasma, in_sol, in_env = inside(plasma_t, rr, z), inside(sol_t, rr, z), inside(env_t, rr, z)
        in_duct = np.zeros(x.shape, dtype=bool)
        for x0, x1, y0, y1, z0, z1 in boxes:
            in_duct |= (x >= x0) & (x <= x1) & (y >= y0) & (y <= y1) & (z >= z0) & (z <= z1)
        phi = np.degrees(np.arctan2(y, x)) % 360.0
        d = (phi % pitch) - pitch / 2  # coil centres sit at pitch / 2 + k pitch
        in_coil = inside(tf_out_t, rr, z) & ~inside(tf_in_t, rr, z) & (np.abs(d) <= half)
        out = np.full(x.shape, 4, dtype=np.int8)
        out[in_env & ~in_plasma] = 2
        out[in_env & (in_sol | in_duct)] = 0
        out[in_plasma] = 1
        out[~in_env & in_coil] = 3
        return out

    return classify


def line_of_sight(p: dict, lines: int = 20000, seed: int = 1, chunk: int = 400) -> dict:
    """Candidate straight lines through the port, and how many run from the plasma to a coil (or out of the shield) through void.

    Every line with a line of sight crosses the first and the last opening of the duct, so lines are drawn
    uniformly between a point in the entry cross-section and a point in the exit cross-section (plus the corner
    combinations), extended 6 lengths back and 4 forward at 1 cm steps. A line has sight when a plasma sample is
    followed, with no shield or coil sample between, by a coil sample (or by void outside the shield).
    """
    import numpy as np
    boxes = port_boxes(p)
    if not boxes:
        return {"lines": 0}
    classify = make_classifier(p)
    first, last = boxes[0], boxes[-1]
    x_in = p["major"] + p["minor"] + p["sol_out"]
    x_out = p["major"] + p["minor"] + p["outboard_total"]
    rng = random.Random(seed)

    def point(box, x):
        return (x, rng.uniform(box[2], box[3]), rng.uniform(box[4], box[5]))

    pairs = [(point(first, x_in), point(last, x_out)) for _ in range(lines)]
    corners = lambda box, x: [(x, y, z) for y in (box[2], box[3]) for z in (box[4], box[5])]
    pairs += [(a, b) for a in corners(first, x_in) for b in corners(last, x_out)]
    length = math.dist(pairs[0][0], pairs[0][1])
    steps = np.arange(-6.0 * length, 4.0 * length, 1.0)
    coil_hits = exit_hits = 0
    for start in range(0, len(pairs), chunk):
        a = np.array([q[0] for q in pairs[start:start + chunk]])
        b = np.array([q[1] for q in pairs[start:start + chunk]])
        direction = (b - a) / np.linalg.norm(b - a, axis=1)[:, None]
        pos = a[:, None, :] + direction[:, None, :] * steps[None, :, None]
        kind = classify(pos[..., 0], pos[..., 1], pos[..., 2])
        idx = np.broadcast_to(np.arange(kind.shape[1]), kind.shape)
        block = (kind == 2) | (kind == 3)
        last_block = np.maximum.accumulate(np.where(block, idx, -1), axis=1)
        last_block = np.concatenate([np.full((kind.shape[0], 1), -1), last_block[:, :-1]], axis=1)
        last_plasma = np.maximum.accumulate(np.where(kind == 1, idx, -1), axis=1)
        sees = last_plasma > last_block
        coil_hits += int(np.any(sees & (kind == 3), axis=1).sum())
        exit_hits += int(np.any(sees & (kind == 4), axis=1).sum())
    return {"lines": len(pairs), "plasma_to_coil": coil_hits, "plasma_to_outside": exit_hits}


def straight_variant(p: dict) -> dict:
    """The same machine with a straight 30 cm duct, to prove the line-of-sight check finds sight where it exists."""
    return dict(p, port="straight")


def verify(cq, p: dict, solids: list[dict]) -> dict:
    report = {"model": p["name"], "solid_count": len(solids)}
    summaries = []
    for s in solids:
        v, c = build_rm_m.mass_properties(s["solid"])
        summaries.append({"name": s["name"], "valid": bool(s["solid"].isValid()), "volume_m3": v / MM ** 3 / 1e6,
                          "solids_in_shape": len(s["solid"].Solids())})
    report["solids"] = summaries
    report["all_valid"] = all(x["valid"] and x["solids_in_shape"] == 1 and x["volume_m3"] > 0 for x in summaries)

    # pairwise common volume of every pair whose bounding boxes overlap
    worst, pairs = 0.0, 0
    for i, a in enumerate(solids):
        ba = a["solid"].BoundingBox()
        for b in solids[i + 1:]:
            bb = b["solid"].BoundingBox()
            if ba.xmax < bb.xmin or bb.xmax < ba.xmin or ba.ymax < bb.ymin or bb.ymax < ba.ymin or ba.zmax < bb.zmin or bb.zmax < ba.zmin:
                continue
            shared = a["solid"].intersect(b["solid"])
            common = (build_rm_m.mass_properties(shared)[0] / MM ** 3) if shared.Solids() else 0.0
            pairs += 1
            if common > worst:
                worst, worst_pair = common, (a["name"], b["name"])
    report["overlap"] = {"pairs_tested": pairs, "max_common_volume_cm3": worst, "worst_pair": worst_pair if worst else None}

    # plasma containment
    by = {s["name"]: s["solid"] for s in solids}
    pb, sb = by["plasma"].BoundingBox(), by["scrape_off_layer"].BoundingBox()
    report["plasma_bbox_inside_sol_bbox"] = bool(pb.xmin >= sb.xmin and pb.xmax <= sb.xmax and pb.ymin >= sb.ymin and pb.ymax <= sb.ymax
                                                 and pb.zmin >= sb.zmin and pb.zmax <= sb.zmax)
    with configured(p):
        fidelity = spec.chord_sag_cm(lambda c: 0.0)
        source = [spec.plasma_point(2 * math.pi * i / 3600) for i in range(3600)]
    classify = make_classifier(p)
    import numpy as np
    pts = np.array(source)
    shrink = 1.0 - 1e-3  # source points a hair inside the separatrix, as the sampled source lies inside it
    rr = p["major"] + (pts[:, 0] - p["major"]) * shrink
    kind = classify(rr, np.zeros_like(rr), pts[:, 1] * shrink)
    report["plasma_chord_sag_cm"] = fidelity
    report["source_boundary_points_outside_chamber"] = int((kind != 1).sum())

    if p["port"]:
        report["line_of_sight"] = line_of_sight(p)
        if p["port"] == "dogleg":
            report["line_of_sight_straight_control"] = line_of_sight(straight_variant(p))
        # analytic classifier against the CAD solids at random points around the duct
        rng = random.Random(7)
        box0, box1 = port_boxes(p)[0], port_boxes(p)[-1]
        names = [s["name"] for s in solids if s["name"] not in ("plasma",)]
        mismatches, tested = [], 0
        x_lo, x_hi = p["major"] + p["minor"] - 10.0, p["major"] + p["minor"] + p["outboard_total"] + 10.0
        for _ in range(400):
            x, y, z = rng.uniform(x_lo, x_hi), rng.uniform(-2 * p["port_w"], 3 * p["port_w"]), rng.uniform(-p["port_h"], p["port_h"])
            got = int(classify(np.array([x]), np.array([y]), np.array([z]))[0])
            vec = cq.Vector(x * MM, y * MM, z * MM)
            in_solid = [n for n in names if by[n].isInside(vec, 1e-3)]
            in_duct = "port_duct" in in_solid or "scrape_off_layer" in in_solid
            in_shield = any(n in in_solid for n in PINCHED + ("first_wall_tungsten", "tih2_shield", "tank_wall", "thermal_shield", "vv_inner_wall"))
            in_coil = any(n.startswith("tf_coil") for n in in_solid)
            want = 3 if in_coil else 2 if in_shield else 0 if in_duct else None
            plasma_in = by["plasma"].isInside(vec, 1e-3)
            if plasma_in:
                want = 1
            if want is None:
                want = 4  # in no solid: outside the shield envelope
            tested += 1
            if got != want:
                mismatches.append({"point_cm": [x, y, z], "analytic": got, "cad": want})
        report["classifier_vs_cad"] = {"points": tested, "mismatches": len(mismatches), "examples": mismatches[:5]}
    return report


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--model", required=True, choices=MODELS)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--design", action="store_true", help="also write the design file and operating history (needs the faris CLI)")
    ap.add_argument("--verify", action="store_true", help="run the cheap CAD checks and write <model>.verify.json")
    ap.add_argument("--faris", default=FARIS)
    ap.add_argument("--cad-python", default=CAD_PYTHON)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    import cadquery as cq

    p = model_params(args.model)
    t0 = time.time()
    solids = build_solids(cq, p)
    print(f"built {len(solids)} solids in {time.time() - t0:.1f} s", file=sys.stderr)
    step = args.out / f"{args.model}.step"
    export_step(cq, solids, step)
    print(f"wrote {step} ({step.stat().st_size} bytes), sha256 {build_rm_m.sha256(step)}", file=sys.stderr)
    if args.verify:
        t0 = time.time()
        report = verify(cq, p, solids)
        report["verify_seconds"] = time.time() - t0
        (args.out / f"{args.model}.verify.json").write_text(json.dumps(report, indent=1) + "\n")
        print(json.dumps({k: v for k, v in report.items() if k != "solids"}, indent=1))
    if args.design:
        print(write_design(p, step, args.out, args.faris, args.cad_python))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
