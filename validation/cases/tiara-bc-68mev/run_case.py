#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Translate the pinned TIARA BC MCNP deck into an OpenMC fixed-source model.

The translator deliberately supports only the audited cell/surface subset found
in the two pinned on-axis 68 MeV cases. It raises on unknown syntax rather than
dropping a surface or card. `build` exports XML and performs no transport;
`run` requires an explicit CPU acknowledgement.
"""
from __future__ import annotations

import argparse
import contextlib
import csv
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT))
from validation.manifest import load_manifest, manifest_sha256, verify_files  # noqa: E402
from validation.scoring import Identity, RUN_KIND, seal_run_record  # noqa: E402

UPSTREAM_URL = "https://github.com/IAEA-NDS/open-benchmarks"
UPSTREAM_COMMIT = "1d9855b00e05c5d5e25a11026ac56940dc75b516"
DEFAULT_UPSTREAM = Path.home() / ".cache/avila-night/validation-data/open-benchmarks"
DEFAULT_CROSS_SECTIONS = Path.home() / "nuclear-data/p32-work/cross_sections.xml"
MAX_SOURCE_ENERGY_EV = 72.5e6
DEFAULT_PARTICLES = 10_000_000
DEFAULT_BATCHES = 100
DEFAULT_SEED = 1
AGREEMENT_NOTES = "The transport adapter has not been compared with MCNP. A run record is raw OpenMC output, not independent validation of the translation."


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _numbers(items: list[str]) -> list[float]:
    return [float(x.replace("D", "E").replace("d", "e")) for x in items]


def parse_cards(deck: str) -> dict[str, str]:
    """Collect the source and tally data cards, retaining every continuation value."""
    cards: dict[str, str] = {}
    current: str | None = None
    for raw in deck.splitlines():
        line = raw.split("$", 1)[0]
        if line.lstrip().lower().startswith("c") or not line.strip():
            continue
        match = re.match(r"^\s*(sdef|si\d+|sp\d+|sb\d+|f\d+:n|fm\d+|e\d+)\b(.*)$", line, re.I)
        if match:
            current = match.group(1).lower()
            cards[current] = match.group(2).strip()
        elif current is not None and raw[:1].isspace():
            cards[current] += " " + line.strip()
        else:
            current = None
    return cards


def parse_deck(deck: str) -> dict:
    cards = parse_cards(deck)
    require("si1" in cards and "sp1" in cards and "si2" in cards and "sp2" in cards and "sb2" in cards,
            "TIARA source requires SI1/SP1 and SI2/SP2/SB2 cards")
    e1 = cards["si1"].split()
    require(e1[0].upper() == "H", "source energy SI1 must be an MCNP histogram (`H`)")
    e_edges_mev = _numbers(e1[1:])
    weights = _numbers(cards["sp1"].split())
    require(len(e_edges_mev) == len(weights) and len(e_edges_mev) > 2,
            "SI1 H boundaries and SP1 interval weights must have equal lengths")
    require(e_edges_mev[0] == 0.0 and all(b > a for a, b in zip(e_edges_mev, e_edges_mev[1:])),
            "source histogram edges must increase from the authored 0 MeV lower boundary")
    require(weights[0] == 0.0 and weights[1] == 0.0 and all(w >= 0.0 for w in weights),
            "unexpected MCNP SI1 H/SP1 leading zero bins or negative source probability")
    intervals = weights[1:]
    total = math.fsum(intervals)
    require(math.isfinite(total) and total > 0.0, "source energy distribution has no positive mass")

    angular_tokens = cards["si2"].split()
    require(angular_tokens[0] == "-1", "angular SI2 must use MCNP histogram interpolation marker -1")
    mu_edges = _numbers(angular_tokens)
    angular_weights = _numbers(cards["sp2"].split())
    bias = _numbers(cards["sb2"].split())
    require(len(mu_edges) == len(angular_weights) == len(bias) == 3,
            "audited TIARA angular distribution must have two cosine bins")
    require(mu_edges == [-1.0, mu_edges[1], 1.0] and -1.0 < mu_edges[1] < 1.0,
            "TIARA cosine edges do not match the audited physical range")
    expected_sp = [(mu_edges[i + 1] - mu_edges[i]) / 2.0 for i in range(2)]
    require(all(math.isclose(actual, expected, rel_tol=2e-6, abs_tol=6e-9)
                for actual, expected in zip(angular_weights[1:], expected_sp)),
            "SP2 is no longer the isotropic cosine probability; do not guess an angular source")
    require(bias == [0.0, 0.0, 1.0], "unexpected source bias card; reassess its sampling-only semantics")

    tally_cell = re.search(r"(?im)^\s*f14:n\s+(\d+)\s*$", deck)
    fm = re.search(r"(?im)^\s*fm14\s+([+\-0-9.eEdD]+)", deck)
    e14 = re.search(r"(?ims)^\s*e14\s+(.+?)(?=\n\s*(?:print|c|$))", deck)
    require(tally_cell and fm and e14, "expected F14:n, FM14, and E14 cards")
    energy_edges = _numbers(re.findall(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eEdD][-+]?\d+)?", e14.group(1)))
    require(int(tally_cell.group(1)) == 109, "only the pinned on-axis cell-109 BC detector is supported")
    require(len(energy_edges) == 52 and energy_edges[0] == 6.0 and energy_edges[-1] == 70.0,
            "unexpected TIARA detector E14 energy edges")
    require(all(b > a for a, b in zip(energy_edges, energy_edges[1:])), "E14 edges must strictly increase")
    require(math.isclose(float(fm.group(1).replace("D", "E")), 1.31126e11, rel_tol=1e-12),
            "unexpected FM14 source-charge normalisation")
    return {"energy_edges_mev": e_edges_mev, "energy_weights": intervals, "energy_weight_sum": total,
            "mu_edges": mu_edges, "mu_weights": angular_weights[1:], "sb_weights": bias,
            "tally_cell": int(tally_cell.group(1)), "fm14": float(fm.group(1).replace("D", "E")),
            "tally_edges_mev": energy_edges}


@dataclass
class CellCard:
    id: int
    material: int
    density_g_cm3: float | None
    expression: str


def _strip_mc_comments(line: str) -> str:
    return line.split("$", 1)[0]


def _read_cards(section: str, begin: str, end: str) -> list[str]:
    part = section.split(begin, 1)[1] if begin else section
    part = part.split(end, 1)[0]
    cards: list[str] = []
    for raw in part.splitlines():
        clean = _strip_mc_comments(raw)
        if not clean.strip() or clean.lstrip().lower().startswith("c"):
            continue
        if raw[:1].isspace() and cards:
            cards[-1] += " " + clean.strip()
        else:
            cards.append(clean.strip())
    return cards


def _parse_cells(deck: str) -> tuple[list[CellCard], dict[int, dict]]:
    cards = _read_cards(deck, "", "SURFACES")
    cells: list[CellCard] = []
    mat_composition: dict[int, dict] = {}
    for card in cards:
        first = re.match(r"^(\d+)\s+(\d+)\s+(.*)$", card)
        if not first:
            continue
        cell_id, material = int(first.group(1)), int(first.group(2))
        rest = first.group(3)
        if material == 0:
            density, region = None, rest.split("imp:", 1)[0].strip()
        else:
            den = re.match(r"^([+\-]?(?:\d+\.?\d*|\.\d+)(?:[Ee][+\-]?\d+)?)\s+(.*)$", rest)
            if not den:
                continue
            density, region = abs(float(den.group(1))), den.group(2).split("imp:", 1)[0].strip()
        if not region:
            continue
        # This section contains cell cards only; non-card lines (surface declarations) are handled separately.
        if not re.match(r"^[+\-()#0-9.:\s]+$", region):
            continue
        cells.append(CellCard(cell_id, material, density, region))

    material_section = deck.split("MATERIALS", 1)[1].split("SOURCE", 1)[0]
    current: int | None = None
    for raw in material_section.splitlines():
        if raw.lstrip().lower().startswith("c"):
            continue
        line = _strip_mc_comments(raw).strip()
        if not line:
            continue
        match = re.match(r"^m(\d+)\s+(.*)$", line, re.I)
        if match:
            current = int(match.group(1))
            mat_composition[current] = {"basis": "ao", "nuclides": []}
            tail = match.group(2)
        elif current is not None:
            tail = line
        else:
            continue
        tokens = tail.split()
        if len(tokens) % 2:
            raise ValueError(f"malformed nuclide/fraction pairs in material {current}")
        for zaid, frac in zip(tokens[::2], tokens[1::2]):
            if not zaid.split(".")[0].isdigit():
                current = None
                break
            digits = zaid.split(".")[0]
            z, a = int(digits[:-3]), int(digits[-3:])
            mat_composition[current]["nuclides"].append((z, a, float(frac)))
    require({m for m in mat_composition} == {1, 2, 3, 4, 5, 6}, "TIARA material cards changed")
    require({c.id for c in cells} == {100, 101, 102, 103, 104, 105, 108, 109, 110, 600, 700, 601},
            "TIARA cell card coverage changed")
    return cells, mat_composition


def _unit(v):
    norm = math.sqrt(sum(x * x for x in v))
    require(norm > 0.0, "zero-length surface vector")
    return tuple(x / norm for x in v)


def _dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def _cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


class McnpGeometry:
    """Strict translation of this TIARA deck family: P/PX/PY/PZ, BOX and axis RCC."""

    def __init__(self, openmc, deck: str):
        self.openmc = openmc
        self.surface_halfspace: dict[tuple[int, int | None], object] = {}
        self.macro_inside: dict[int, object] = {}
        self.outer_faces: list[object] = []
        section = deck.split("SURFACES", 1)[1].split("MATERIALS", 1)[0]
        cards = self._surface_cards(section)
        for card in cards:
            self._translate_surface(card)
        require(600 in self.macro_inside and len(self.outer_faces) == 6, "outer boundary surface 600 was not translated as a six-face box")

    @staticmethod
    def _surface_cards(section: str) -> list[str]:
        cards = []
        for raw in section.splitlines():
            line = _strip_mc_comments(raw)
            if not line.strip() or line.lstrip().lower().startswith("c"):
                continue
            if raw[:1].isspace() and cards:
                cards[-1] += " " + line.strip()
            else:
                cards.append(line.strip())
        return cards

    def _plane(self, sid: int, facet: int | None, point, normal):
        o = self.openmc
        a, b, c = normal
        d = _dot(normal, point)
        surf = o.Plane(a=a, b=b, c=c, d=d, surface_id=sid * 100 + (facet or 0))
        self.surface_halfspace[(sid, facet)] = surf
        return surf

    def _translate_surface(self, card: str) -> None:
        o = self.openmc
        tokens = card.split()
        require(len(tokens) >= 3 and tokens[0].isdigit(), f"unrecognized TIARA surface card: {card}")
        sid, kind = int(tokens[0]), tokens[1].lower()
        v = [float(x) for x in tokens[2:]]
        if kind in ("px", "py", "pz"):
            axis = {"px": (1.0, 0.0, 0.0), "py": (0.0, 1.0, 0.0), "pz": (0.0, 0.0, 1.0)}[kind]
            self._plane(sid, None, tuple(axis[i] * v[0] for i in range(3)), axis)
            return
        if kind == "p":
            require(len(v) == 4, f"plane card {sid} needs a,b,c,d")
            self._plane(sid, None, tuple(v[:3]) if math.isclose(_dot(v[:3], v[:3]), 1.0) else tuple(x * v[3] / _dot(v[:3], v[:3]) for x in v[:3]), tuple(v[:3]))
            return
        if kind == "box":
            require(len(v) == 12, f"box macrobody {sid} needs 12 coordinates")
            origin = tuple(v[:3]); aa, bb, cc = tuple(v[3:6]), tuple(v[6:9]), tuple(v[9:12])
            pairs = ((bb, cc, aa), (cc, aa, bb), (aa, bb, cc))
            inside = None
            for axis_no, (u, w, edge) in enumerate(pairs):
                normal = _cross(u, w)
                if _dot(normal, edge) < 0:
                    normal = tuple(-x for x in normal)
                low = self._plane(sid, 2 * axis_no + 1, origin, normal)
                high_point = tuple(origin[i] + edge[i] for i in range(3))
                high = self._plane(sid, 2 * axis_no + 2, high_point, normal)
                inward = +low & -high
                inside = inward if inside is None else inside & inward
                if sid == 600:
                    self.outer_faces.extend([low, high])
            self.macro_inside[sid] = inside
            return
        if kind == "rcc":
            require(len(v) == 7, f"RCC macrobody {sid} needs base, axis vector and radius")
            base, axis, radius = tuple(v[:3]), tuple(v[3:6]), v[6]
            unit = _unit(axis)
            require(sum(abs(abs(x) - 1.0) < 1e-9 for x in unit) == 1 and all(abs(x) < 1e-9 or abs(abs(x)-1.0)<1e-9 for x in unit),
                    f"RCC macrobody {sid} is not aligned to an OpenMC coordinate axis")
            far = tuple(base[i] + axis[i] for i in range(3))
            if unit[0]:
                cylinder = o.XCylinder(y0=base[1], z0=base[2], r=radius, surface_id=sid * 100)
            elif unit[1]:
                cylinder = o.YCylinder(x0=base[0], z0=base[2], r=radius, surface_id=sid * 100)
            else:
                cylinder = o.ZCylinder(x0=base[0], y0=base[1], r=radius, surface_id=sid * 100)
            cap1 = self._plane(sid, 1, base, unit)
            cap2 = self._plane(sid, 2, far, unit)
            self.surface_halfspace[(sid, 3)] = cylinder
            self.macro_inside[sid] = +cap1 & -cap2 & -cylinder
            return
        raise ValueError(f"unsupported TIARA surface type {kind!r} on surface {sid}")

    def region(self, expression: str, cells_by_id: dict[int, CellCard], cache: dict[int, object] | None = None):
        cache = cache if cache is not None else {}
        tokens = re.findall(r"#\d+|[()]|:|[+-]?\d+(?:\.\d+)?", expression)
        if "".join(tokens).replace("+", "") == "":
            raise ValueError("empty cell region")
        pos = 0

        def union():
            nonlocal pos
            out = intersection()
            while pos < len(tokens) and tokens[pos] == ":":
                pos += 1
                out = out | intersection()
            return out

        def intersection():
            nonlocal pos
            out = atom()
            while pos < len(tokens) and tokens[pos] not in (":", ")"):
                out = out & atom()
            return out

        def atom():
            nonlocal pos
            require(pos < len(tokens), f"unexpected end of region {expression!r}")
            token = tokens[pos]; pos += 1
            if token == "(":
                out = union()
                require(pos < len(tokens) and tokens[pos] == ")", f"unbalanced parentheses in region {expression!r}")
                pos += 1
                return out
            if token.startswith("#"):
                cellid = int(token[1:])
                require(cellid in cells_by_id, f"cell complement references missing cell {cellid}")
                if cellid not in cache:
                    cache[cellid] = self.region(cells_by_id[cellid].expression, cells_by_id, cache)
                return ~cache[cellid]
            match = re.fullmatch(r"([+\-]?)(\d+)(?:\.(\d+))?", token)
            require(match is not None, f"unsupported region token {token!r}")
            sign, sid, facet = match.group(1), int(match.group(2)), int(match.group(3)) if match.group(3) else None
            if facet is None and sid in self.macro_inside:
                region = self.macro_inside[sid]
                return region if sign == "-" else ~region
            surf = self.surface_halfspace.get((sid, facet))
            require(surf is not None, f"surface {sid}{'.'+str(facet) if facet else ''} was not translated")
            return -surf if sign == "-" else +surf

        result = union()
        require(pos == len(tokens), f"unconsumed region tokens in {expression!r}: {tokens[pos:]}")
        return result


def build_model(openmc, deck: str):
    # This adapter owns IDs in the model it constructs; reset OpenMC's global
    # auto-ID registries so sequential case builds remain deterministic.
    openmc.reset_auto_ids()
    parsed = parse_deck(deck)
    cell_cards, compositions = _parse_cells(deck)
    cells_by_id = {c.id: c for c in cell_cards}
    geometry_translator = McnpGeometry(openmc, deck)
    regions = {}
    # Resolve complements with memoization; these decks are acyclic by construction.
    for card in cell_cards:
        regions[card.id] = geometry_translator.region(card.expression, cells_by_id, regions)
    materials = {}
    material_objects = []
    for card in cell_cards:
        if not card.material:
            continue
        material_key = (card.material, card.density_g_cm3)
        if material_key not in materials:
            source = compositions[card.material]
            mat_id = card.material * 1000 + int(round(card.density_g_cm3 * 100.0))
            mat = openmc.Material(material_id=mat_id, name=f"MCNP m{card.material} at {card.density_g_cm3:g} g/cm3")
            for z, a, fraction in source["nuclides"]:
                symbol = openmc.data.ATOMIC_SYMBOL.get(z)
                require(symbol is not None, f"unknown atomic number {z} in m{card.material}")
                if a == 0:
                    # MCNP ZAID 6000 is natural elemental carbon, not a C0 isotope.
                    mat.add_element(symbol, fraction, "ao")
                else:
                    mat.add_nuclide(f"{symbol}{a}", fraction, "ao")
            # MCNP material cards with positive fractions are normalized atomic fractions.
            mat.set_density("g/cm3", card.density_g_cm3)
            materials[material_key] = mat
            material_objects.append(mat)
    material_cells = []
    for card in cell_cards:
        fill = materials.get((card.material, card.density_g_cm3)) if card.material else None
        material_cells.append(openmc.Cell(cell_id=card.id, name=f"MCNP cell {card.id}", fill=fill, region=regions[card.id]))
    for face in geometry_translator.outer_faces:
        face.boundary_type = "vacuum"
    geometry = openmc.Geometry(openmc.Universe(cells=material_cells))

    source_edges = parsed["energy_edges_mev"]
    source_weights = parsed["energy_weights"]
    total = parsed["energy_weight_sum"]
    x_ev = [e * 1e6 for e in source_edges]
    density = [w / total / (x_ev[i + 1] - x_ev[i]) for i, w in enumerate(source_weights)] + [0.0]
    energy = openmc.stats.Tabular(x_ev, density, interpolation="histogram")
    spatial = openmc.stats.Point((0.0, 0.0, 0.0))
    angle = openmc.stats.PolarAzimuthal(mu=openmc.stats.Uniform(-1.0, 1.0),
                                        phi=openmc.stats.Uniform(0.0, 2.0 * math.pi),
                                        reference_uvw=(1.0, 0.0, 0.0),
                                        reference_vwu=(0.0, 1.0, 0.0))
    source = openmc.IndependentSource(space=spatial, angle=angle, energy=energy,
                                      strength=parsed["fm14"], particle="neutron",
                                      constraints={"domains": [next(c for c in material_cells if c.id == 600)]})
    settings = openmc.Settings()
    settings.run_mode = "fixed source"
    settings.source = source
    settings.particles = DEFAULT_PARTICLES
    settings.batches = DEFAULT_BATCHES
    settings.seed = DEFAULT_SEED

    e_mev = parsed["tally_edges_mev"]
    tally_edges_ev = [0.0] + [e * 1e6 for e in e_mev]
    energy_filter = openmc.EnergyFilter(tally_edges_ev)
    tally = openmc.Tally(name="tiara_bc501a_flux")
    tally.filters = [openmc.CellFilter([next(c for c in material_cells if c.id == 109)]), energy_filter]
    tally.scores = ["flux"]
    model = openmc.Model(geometry=geometry, materials=openmc.Materials(material_objects),
                         settings=settings, tallies=openmc.Tallies([tally]))
    # Point-location checks catch the common sign/side errors in a hand-translated CSG.
    source_found = geometry.find((0.0, 0.0, 0.0))[-1]
    require(source_found.id == 600, f"source point belongs to MCNP cell {source_found.id}, expected 600")
    detector_x = 507.35 if "concrete" in deck.splitlines()[0].lower() else 447.35
    detector_found = geometry.find((detector_x, 0.0, 0.0))[-1]
    require(detector_found.id == 109, f"on-axis BC detector sample point belongs to MCNP cell {detector_found.id}, expected 109")
    return model, parsed, {"source_cell": source_found.id, "detector_cell": detector_found.id}


def validate_reference_mapping(manifest: dict, parsed: dict) -> None:
    detectors = manifest["detectors"]
    require(len(detectors) == len(parsed["tally_edges_mev"]), "manifest detector rows do not preserve every MCNP E14 response boundary")
    require([d["controlling_variable"]["value"] for d in detectors] == parsed["tally_edges_mev"],
            "manifest detector energies differ from the pinned E14 card")
    require(sum(1 for d in detectors if "reference" in d) >= 45,
            "measured spectrum data rows unexpectedly disappeared")


def score_from_tally(tally, parsed: dict) -> list[dict]:
    """Map one tally's bin-integrated flux to the per-unit-lethargy response."""
    means = [float(x) for x in tally.mean.ravel()]
    stds = [float(x) for x in tally.std_dev.ravel()]
    edges = parsed["tally_edges_mev"]
    require(len(means) == len(stds) == len(edges), "OpenMC flux tally has an unexpected bin shape")
    results = []
    for i, upper in enumerate(edges):
        lower = 0.0 if i == 0 else edges[i - 1]
        # MCNP E14 first bin has a zero lower edge; no finite lethargy conversion exists.
        if lower <= 0.0:
            value = error = None
        else:
            width = math.log(upper / lower)
            value, error = means[i] / width, stds[i] / width
        results.append({"detector_id": f"n-{upper:g}", "response_class": "neutron_spectrum_in_detector",
                        "value": value, "u_mc": error, "unit": "neutrons cm-2 microC-1 per unit lethargy",
                        "energy_bin": {"lo_mev": lower, "hi_mev": upper},
                        "note": "OpenMC cell flux multiplied by the exact FM14 source-charge scale and divided by logarithmic lethargy width"})
    return results


def aperture_bias_plan(deck: str, beta: float) -> dict:
    """Describe an unbiased isotropic/rectangle angular mixture for AIM-patched OpenMC."""
    require(math.isfinite(beta) and 0.0 < beta < 1.0,
            "aperture mixture beta must be between zero and one so isotropic support remains positive")
    surface = re.search(r"(?im)^\s*237\s+rcc\s+(.+?)\s*(?:\$.*)?$", deck)
    require(surface is not None, "audited BC501A detector RCC surface 237 is missing")
    values = _numbers(surface.group(1).split())
    require(len(values) == 7, "BC501A detector RCC surface 237 changed")
    x, y, z, ax, ay, az, radius = values
    require(ax > 0 and ay == 0 and az == 0 and math.isclose(ax, 12.7, abs_tol=1e-10)
            and y == 0.0 and z == 0.0 and math.isclose(radius, 6.35, abs_tol=1e-10),
            "BC501A is no longer the audited x-axis 12.7 cm by 12.7 cm cylinder")
    # The patch aims into a rectangle. Choosing a square with half-side r/sqrt(2)
    # keeps every aimed point inside the projected circular detector face.
    half_side = radius / math.sqrt(2.0)
    rectangle = (x, y, z, 0.0, half_side, 0.0, 0.0, 0.0, half_side)
    spec = " ".join(format(v, ".17g") for v in (beta, *rectangle))
    return {
        "method": "FARIS OpenMC 0.15.3 aperture-mixture angular source bias",
        "beta": beta,
        "physical_source": "unchanged isotropic SP2 law; q=(1-beta)*p_iso + beta*p_rectangle",
        "weight_correction": "w *= p_iso/q = 1/((1-beta)+4*pi*beta*p_rectangle(direction))",
        "sampling_support": f"beta={beta:g} leaves the positive (1-beta) isotropic component in every physical direction",
        "target": {"geometry": "square inscribed in the projected circular BC501A front face",
                   "center_cm": [x, y, z], "u_cm": [0.0, half_side, 0.0],
                   "v_cm": [0.0, 0.0, half_side], "one_sided": True},
        "environment": {"OPENMC_APERTURE_BIAS": spec, "OPENMC_APERTURE_ONE_SIDED": "1"},
        "variance_reduction_claim": "not established; compare against the analog control using tally uncertainty and figure of merit",
    }


def verify_aim_executable(executable: Path, library: Path) -> dict:
    """Fail closed unless the CLI executable is linked to the selected AIM-patched library."""
    executable, library = executable.resolve(strict=True), library.resolve(strict=True)
    markers = (b"OPENMC_APERTURE_BIAS", b"OPENMC_APERTURE_ONE_SIDED")
    lib_bytes = library.read_bytes()
    require(all(marker in lib_bytes for marker in markers),
            f"selected library lacks FARIS aperture AIM support: {library}")
    readelf = shutil.which("readelf")
    require(readelf is not None, "readelf is required to verify the AIM-patched OpenMC executable/library pair")
    dynamic = subprocess.run([readelf, "-d", str(executable)], check=True, capture_output=True,
                             text=True, timeout=10).stdout
    has_search_path = "Library rpath:" in dynamic or "Library runpath:" in dynamic
    require("Shared library: [libopenmc.so]" in dynamic and has_search_path,
            "AIM mode requires an OpenMC CLI executable with a verifiable libopenmc.so dependency and RPATH/RUNPATH")
    clean_env = {k: v for k, v in os.environ.items() if k not in ("LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT")}
    linked_output = subprocess.run(["ldd", str(executable)], check=True, capture_output=True,
                                   text=True, timeout=10, env=clean_env).stdout
    linked_match = re.search(r"(?m)^\s*libopenmc\.so\s+=>\s+(\S+)", linked_output)
    require(linked_match is not None, "ldd could not resolve the AIM executable's libopenmc.so")
    linked = Path(linked_match.group(1)).resolve(strict=True)
    require(linked == library, f"OpenMC executable resolves {linked}, not requested AIM library {library}")
    return {"executable": str(executable), "library": str(library),
            "library_sha256": sha256(library), "linkage": "verified OpenMC RPATH/RUNPATH with sanitized ldd",
            "aim_patch_sha256": sha256(ROOT / "integrations/openmc/patches/faris-openmc-0.15.3-aim.patch")}


@contextlib.contextmanager
def patched_aim_environment(plan: dict):
    """Temporarily expose only the audited source-aperture settings to the solver."""
    names = set(plan["environment"]) | {"LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT"}
    previous = {name: os.environ.get(name) for name in names}
    try:
        for name in ("LD_LIBRARY_PATH", "LD_PRELOAD", "LD_AUDIT"):
            os.environ.pop(name, None)
        os.environ.update(plan["environment"])
        yield
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def check_xs_coverage(openmc, xml_path: Path, nuclides: list[str], max_energy_ev: float = MAX_SOURCE_ENERGY_EV) -> dict:
    """Verify every neutron table supports the full source energy; never allow implicit extrapolation."""
    root = openmc.data.DataLibrary.from_xml(xml_path)
    entries = {}
    for entry in root.libraries:
        if entry["type"] != "neutron":
            continue
        names = entry["materials"]
        names = names.split() if isinstance(names, str) else names
        for name in names:
            entries[name] = entry
    missing, short = [], []
    maxima = {}
    for name in sorted(set(nuclides)):
        entry = entries.get(name)
        if entry is None:
            missing.append(name)
            continue
        try:
            data_path = Path(entry["path"])
            if not data_path.is_absolute():
                data_path = xml_path.parent / data_path
            import h5py

            with h5py.File(data_path, "r") as h5:
                energy_group = h5[name]["energy"]
                maximum = max(float(grid[-1]) for grid in energy_group.values())
        except (ImportError, OSError, KeyError, ValueError) as exc:
            short.append({"nuclide": name, "reason": f"cannot read neutron table: {exc}"})
            continue
        maxima[name] = maximum
        if maximum < max_energy_ev:
            short.append({"nuclide": name, "max_energy_ev": maximum, "required_energy_ev": max_energy_ev})
    return {"missing_nuclides": missing, "short_energy_tables": short, "max_energy_ev_by_nuclide": maxima,
            "required_max_energy_ev": max_energy_ev, "complete": not missing and not short}


def _case_id(args) -> str:
    return args.case


def _model_path(upstream: Path, case_id: str) -> Path:
    folder = {"tiara-bc-fe-68-40": "Tiara-BC_fe-68-40-00", "tiara-bc-cc-68-100": "Tiara-BC_cc-68-100-00"}[case_id]
    return upstream / "jade_open_benchmarks" / "inputs" / "Tiara-BC" / folder / "mcnp" / f"{folder}.i"


def _cli() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("plan", "build", "run"))
    parser.add_argument("--case", choices=("tiara-bc-fe-68-40", "tiara-bc-cc-68-100"), default="tiara-bc-fe-68-40")
    parser.add_argument("--upstream", type=Path, default=DEFAULT_UPSTREAM)
    parser.add_argument("--cross-sections", type=Path, default=DEFAULT_CROSS_SECTIONS)
    parser.add_argument("--work-dir", type=Path, default=Path("tiara-work"))
    parser.add_argument("--particles", type=int, default=DEFAULT_PARTICLES)
    parser.add_argument("--batches", type=int, default=DEFAULT_BATCHES)
    parser.add_argument("--statepoint-every", type=int, default=5,
                        help="write restartable statepoints at this interval and at the final batch")
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--aim-aperture-beta", type=float,
                        help="enable the optional, exact-weight AIM aperture mixture (must be 0 < beta < 1)")
    parser.add_argument("--aim-openmc-exec", type=Path,
                        help="OpenMC CLI executable linked to the AIM library; required with --aim-aperture-beta")
    parser.add_argument("--aim-library", type=Path,
                        help="AIM-patched libopenmc.so linked by --aim-openmc-exec; required with --aim-aperture-beta")
    parser.add_argument("--i-have-the-cpu", action="store_true")
    args = parser.parse_args()
    manifest = load_manifest(HERE.parent / args.case / "manifest.json")
    if args.upstream.is_dir():
        commit = subprocess.run(["git", "-C", str(args.upstream), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
        if commit != UPSTREAM_COMMIT:
            raise SystemExit(f"upstream commit is {commit}, expected pinned {UPSTREAM_COMMIT}")
        problems = verify_files(manifest, args.upstream)
        if problems:
            raise SystemExit("upstream hash check failed: " + "; ".join(problems))
    else:
        raise SystemExit(f"upstream checkout not found: {args.upstream}")
    deck_path = _model_path(args.upstream, args.case)
    deck = deck_path.read_text(encoding="utf-8")
    parsed = parse_deck(deck)
    validate_reference_mapping(manifest, parsed)
    print(json.dumps({"case_id": args.case, "upstream_commit": UPSTREAM_COMMIT, "input_sha256": sha256(deck_path),
                      "manifest_sha256": manifest_sha256(manifest), "n_source_edges": len(parsed["energy_edges_mev"]),
                      "source_weight_sum_before_normalize": parsed["energy_weight_sum"], "tally_cell": parsed["tally_cell"],
                      "n_detector_bins": len(parsed["tally_edges_mev"]), "fm14": parsed["fm14"],
                      "source_bias_omitted": parsed["sb_weights"], "action": args.action}, indent=2))
    if args.action == "plan":
        return 0
    try:
        import openmc
    except ImportError as exc:
        raise SystemExit("OpenMC Python API is required; use the pinned OpenMC 0.15.3 environment") from exc
    model, parsed, locations = build_model(openmc, deck)
    if args.action == "build":
        args.work_dir.mkdir(parents=True, exist_ok=False)
        model.settings.particles = args.particles
        model.settings.batches = args.batches
        model.settings.seed = args.seed
        model.export_to_xml(directory=str(args.work_dir))
        print(json.dumps({"xml_dir": str(args.work_dir.resolve()), "point_checks": locations,
                          "transport_run": False, "scientific_verdict": "NOT_EVALUATED"}, indent=2))
        return 0
    if not args.i_have_the_cpu:
        raise SystemExit("run can consume substantial CPU; repeat with --i-have-the-cpu")
    if not args.cross_sections.is_file():
        raise SystemExit(f"cross_sections.xml does not exist: {args.cross_sections}")
    aim_plan = None
    aim_identity = None
    aim_executable = None
    if args.aim_aperture_beta is not None:
        if args.aim_library is None or args.aim_openmc_exec is None:
            raise SystemExit("AIM mode requires both --aim-openmc-exec and --aim-library; it is never inferred")
        aim_plan = aperture_bias_plan(deck, args.aim_aperture_beta)
        aim_identity = verify_aim_executable(args.aim_openmc_exec, args.aim_library)
        aim_executable = Path(aim_identity["executable"])
        if os.environ.get("LD_PRELOAD") or os.environ.get("LD_AUDIT"):
            raise SystemExit("AIM mode refuses LD_PRELOAD or LD_AUDIT because they can replace the verified library")
    elif args.aim_library is not None or args.aim_openmc_exec is not None:
        raise SystemExit("AIM executable/library arguments require explicit --aim-aperture-beta")
    model.settings.particles, model.settings.batches, model.settings.seed = args.particles, args.batches, args.seed
    if args.statepoint_every < 1:
        raise SystemExit("--statepoint-every must be a positive batch count")
    model.settings.statepoint = {"batches": sorted(set(range(args.statepoint_every, args.batches + 1, args.statepoint_every)) | {args.batches})}
    xs = check_xs_coverage(openmc, args.cross_sections, [n for m in model.materials for n in m.get_nuclides()])
    if not xs["complete"]:
        print(json.dumps({"status": "NOT_EVALUATED", "nuclear_data_preflight": xs}, indent=2))
        raise SystemExit("neutron data does not cover the full 72.5 MeV source; no extrapolation is permitted")
    args.work_dir.mkdir(parents=True, exist_ok=False)
    model.settings.cross_sections = str(args.cross_sections)
    model.export_to_xml(directory=str(args.work_dir))
    if aim_plan is None:
        statepoint = Path(model.run(cwd=str(args.work_dir)))
        variance_reduction = {"method": "analog source directions", "enabled": False,
                              "variance_reduction_claim": "none"}
    else:
        with patched_aim_environment(aim_plan):
            statepoint = Path(model.run(cwd=str(args.work_dir), openmc_exec=str(aim_executable)))
        variance_reduction = {**aim_plan, "enabled": True, "solver_library": aim_identity}
    with openmc.StatePoint(statepoint) as sp:
        tally = sp.get_tally(name="tiara_bc501a_flux")
        results = score_from_tally(tally, parsed)
    binary = Path(openmc.__file__).resolve()
    adapter_hash = hashlib.sha256((HERE / "run_case.py").read_bytes() + (HERE / "build_manifest.py").read_bytes()).hexdigest()
    identity = Identity(sha256(args.cross_sections), f"OpenMC {openmc.__version__}", adapter_hash)
    record = seal_run_record({"schema": "faris.validation-run/1.0.0", "kind": RUN_KIND,
                              "case_id": args.case, "run_id": statepoint.stem,
                              "identity": identity.as_dict(),
                              "results": results,
                              "context": {"manifest_sha256": manifest_sha256(manifest), "source_sha256": sha256(deck_path),
                                          "cross_sections_sha256": sha256(args.cross_sections),
                                          "openmc_python_sha256": sha256(binary),
                                          "translation": "direct MCNP-to-OpenMC subset for the pinned TIARA BC cards; no MCNP comparison run exists",
                                          "point_checks": locations, "nuclear_data_preflight": xs, "statement": AGREEMENT_NOTES,
                                          "variance_reduction": variance_reduction,
                                          "histories": args.particles * args.batches, "seed": args.seed}})
    record_path = args.work_dir / f"{args.case}.run.json"
    record_path.write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(record_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(_cli())
