#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded OpenMC mesh-R2S worker for one exact maintenance installation history.

The input is a Rust-authored request; history bytes are opaque to this process.  The request
names the component's unique material IDs, complete irradiation/cooling steps, a full-geometry
OpenMC model, and a one-voxel authored response region.  R2S material-volume bins are filtered
by those IDs before depletion, so no particle-birth filter is used to infer emitter identity.
"""
from __future__ import annotations

import argparse
import copy
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import sys
import time
import traceback
import xml.etree.ElementTree as ET

MAX_REQUEST_BYTES = 4 * 1024 * 1024
MAX_ARTIFACT_BYTES = 256 * 1024 * 1024
MAX_MATERIAL_IDS = 100_000
MAX_STEPS = 5_000_000
MAX_COOLING_POINTS = 512
MAX_BATCHES = 100_000
MAX_PARTICLES = 1_000_000
REQUEST_SCHEMA = "faris-maintenance-r2s-event-request/v0.1"
RECEIPT_SCHEMA = "faris-maintenance-r2s-event-receipt/v0.1"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def checked_file(ref: dict, label: str) -> Path:
    if set(ref) != {"path", "sha256"}:
        raise ValueError(f"{label} artifact must contain exactly path and sha256")
    path = Path(ref["path"]).resolve(strict=True)
    if not path.is_file() or path.stat().st_size > MAX_ARTIFACT_BYTES:
        raise ValueError(f"{label} must be a regular file no larger than {MAX_ARTIFACT_BYTES} bytes")
    digest = sha256(path)
    if digest != ref["sha256"]:
        raise ValueError(f"{label} hash mismatch: expected {ref['sha256']}, observed {digest}")
    return path


def positive_int(value, label, maximum):
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= maximum:
        raise ValueError(f"{label} must be an integer in [1, {maximum}]")
    return value


def export_model_xml(model, model_xml: Path) -> Path:
    """Export a single-file model XML at an explicit path using OpenMC 0.15.3's API."""
    model_xml.parent.mkdir(parents=True, exist_ok=True)
    model.export_to_model_xml(str(model_xml))
    if not model_xml.is_file():
        raise RuntimeError("OpenMC model export did not create the requested model.xml")
    return model_xml


def prepare_checked_cad_model(request: dict, out: Path) -> dict:
    """Build dose geometry from checked numeric design data, never from study plugins."""
    required = {"schema", "h5m", "h5m_sha256", "cross_sections", "solids", "materials",
                "plasma", "solid_history_components", "openmc_helper"}
    if set(request) != required or request["schema"] != "faris-maintenance-dose-cad-prepare/v0.1":
        raise ValueError("invalid checked CAD dose preparation request")
    h5m = Path(request["h5m"]).resolve(strict=True)
    xs = Path(request["cross_sections"]).resolve(strict=True)
    helper = Path(request["openmc_helper"]).resolve(strict=True)
    if not h5m.is_file() or sha256(h5m) != request["h5m_sha256"]:
        raise ValueError("checked CAD H5M is missing or its hash differs from the ImportReport")
    if not xs.is_file() or not helper.is_file():
        raise ValueError("cross sections and trusted repository helper must be regular files")
    solids = request["solids"]
    nonvoid = [s for s in solids if s.get("material_id") is not None]
    mapping = request["solid_history_components"]
    if not isinstance(mapping, dict) or set(mapping) != {s["solid_id"] for s in nonvoid}:
        raise ValueError("solid history mapping must cover every nonvoid checked CAD solid exactly")
    if any(not isinstance(v, str) or not v.strip() for v in mapping.values()):
        raise ValueError("each nonvoid solid needs an authored history component ID")
    trusted_helper = Path(__file__).with_name("design_transport.py").resolve(strict=True)
    if helper != trusted_helper:
        raise ValueError("only the bundled FARIS design_transport.py source helper is accepted")
    import importlib.util
    spec = importlib.util.spec_from_file_location("faris_checked_design_transport", helper)
    if spec is None or spec.loader is None:
        raise ValueError("trusted design transport helper could not be loaded")
    dt = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(dt)
    import openmc
    from openmc_plasma_source import tokamak_source
    recipes = {m["material_id"]: m for m in request["materials"]}
    clones, tagged_solids, tags_by_solid = [], [], {}
    for i, solid in enumerate(sorted(nonvoid, key=lambda x: x["solid_id"]), 1):
        original = recipes.get(solid["material_id"])
        if original is None:
            raise ValueError(f"solid {solid['solid_id']} refers to an unaudited material")
        tag = f"dose{i:06d}"
        clones.append({**original, "material_id": tag})
        tags_by_solid[solid["solid_id"]] = tag
    for solid in solids:
        tagged_solids.append({**solid, "material_id": tags_by_solid.get(solid["solid_id"])})
    derived_request = {"h5m": str(h5m), "solids": tagged_solids, "materials": clones,
                       "cross_sections": str(xs)}
    out.mkdir(parents=True, exist_ok=False)
    geometry = dt.geometry_copy(derived_request, out / "geometry" / "dose.h5m", False)
    model, _, mats = dt.build_model(openmc, derived_request, geometry, False)
    source = tokamak_source(**dt.tokamak_arguments(request["plasma"]))
    total = float(source.strength)
    if not math.isfinite(total) or total <= 0:
        raise ValueError("checked tokamak source has invalid total strength")
    for item in source.sources.ravel():
        item.strength = float(item.strength) / total
    source.strength = 1.0
    model.settings.source = [source]
    model.settings.run_mode = "fixed source"
    model.settings.photon_transport = True
    model.settings.particles, model.settings.batches = 1000, 2
    model.settings.output = {"summary": False, "tallies": False}
    model.settings.statepoint = {"batches": []}
    model.settings.sourcepoint = {"write": False}
    model_dir = out / "model"
    model_xml = export_model_xml(model, model_dir / "model.xml")
    by_component = {}
    for solid in sorted(nonvoid, key=lambda x: x["solid_id"]):
        by_component.setdefault(mapping[solid["solid_id"]], []).append(mats[tags_by_solid[solid["solid_id"]]].id)
    component_map = out / "component-material-map.json"
    component_map.write_text(json.dumps({"schema": "faris-cad-component-material-map/v0.1",
        "geometry_sha256": sha256(geometry), "materials": by_component}, sort_keys=True, separators=(",", ":")))
    return {"schema": "faris-maintenance-dose-cad-preparation-receipt/v0.1",
            "source_basis": "checked Design plasma tokamak source normalized to unit neutron strength",
            "original_h5m_sha256": sha256(h5m), "derived_h5m_sha256": sha256(geometry),
            "model_xml_sha256": sha256(model_xml), "component_material_map_sha256": sha256(component_map),
            "model_xml": str(model_xml), "geometry_h5m": str(geometry),
            "component_material_map": str(component_map), "material_clones": len(clones),
            "solid_history_components": mapping}


def dose_tally(openmc, mesh, name):
    energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
    tally = openmc.Tally(name=name)
    tally.filters = [openmc.MeshFilter(mesh), openmc.ParticleFilter(["photon"]),
                     openmc.EnergyFunctionFilter(energies, coefficients, interpolation="log-log")]
    tally.scores = ["flux"]
    return tally


def chain_cross_section_coverage(chain, cross_sections_xml: Path, material_nuclides) -> dict:
    """Check neutron data for chain nuclides reachable from the model's actual materials.

    The chain can contain nuclides and reaction paths unrelated to the supplied model. They are
    excluded from this coverage check, while decay, transmutation, and fission-yield descendants
    of the starting material nuclides are included. Library presence does not establish reaction
    channel completeness or evaluation quality.
    """
    root = ET.parse(cross_sections_xml).getroot()
    available = set()
    for node in root.findall("library"):
        if node.attrib.get("type") != "neutron":
            continue
        names = node.attrib.get("materials", node.attrib.get("name", "")).split()
        available.update(name.strip(",") for name in names if name.strip(","))
    nuclides = {n.name: n for n in chain.nuclides}
    roots = set(material_nuclides)
    missing_roots = sorted(roots - set(nuclides))
    if missing_roots:
        raise RuntimeError("starting model material nuclides are absent from the depletion chain: " +
                           ", ".join(missing_roots[:40]))
    fission_products = chain.get_default_fission_yields()
    reachable = set(roots)
    pending = list(roots)
    while pending:
        name = pending.pop()
        nuc = nuclides[name]
        products = {path.target for path in nuc.reactions + nuc.decay_modes if path.target is not None}
        products.update(fission_products.get(name, {}).keys())
        for product in products - reachable:
            if product not in nuclides:
                raise RuntimeError(f"reachable chain product {product} from {name} has no chain nuclide entry")
            reachable.add(product)
            pending.append(product)
    reachable_reaction_nuclides = {name for name in reachable if getattr(nuclides[name], "reactions", None)}
    required_material_nuclides = roots
    required = sorted(required_material_nuclides | reachable_reaction_nuclides)
    missing = sorted(set(required) - available)
    if missing:
        raise RuntimeError("starting model material nuclides or reachable activation-chain nuclides with neutron "
                           "reactions are absent from the cross-section index; transport or MicroXS rates would "
                           "be unavailable: " + ", ".join(missing[:40]))
    chain_reaction_nuclides = sum(bool(getattr(n, "reactions", None)) for n in chain.nuclides)
    return {"material_root_nuclides": sorted(roots), "reachable_chain_nuclides": len(reachable),
            "required_material_nuclides": sorted(required_material_nuclides),
            "reachable_reaction_nuclides": len(reachable_reaction_nuclides), "unreachable_reaction_nuclides_excluded":
                chain_reaction_nuclides - len(required), "neutron_library_nuclides": len(available),
            "reachability_basis": "all listed decay targets, transmutation targets, and default fission-yield products from model material nuclides",
            "coverage": "all reachable chain nuclides with listed neutron reactions appear in cross_sections.xml",
            "important_limit": "Presence in the index does not establish channel completeness, evaluation quality, or chain adequacy; those remain nuclear-data/model uncertainty."}


def validate_data_only_sources(model_xml: Path) -> None:
    """Reject studies that ask OpenMC to import executable source plugins."""
    root = ET.parse(model_xml).getroot()
    for element in root.iter():
        attrs = {key.lower(): value for key, value in element.attrib.items()}
        if any(key in attrs for key in ("library", "module", "callable", "python_module")):
            raise ValueError("compiled or dynamically imported source plugins are not accepted")
        if element.tag.lower() == "source":
            kind = attrs.get("type", "independent").lower()
            if kind != "independent":
                raise ValueError(f"source type {kind!r} is not a checked data-defined source")
            for key, value in attrs.items():
                if key in {"filename", "file", "path"} and Path(value).suffix.lower() in {".py", ".so", ".dll", ".dylib", ".pyd"}:
                    raise ValueError("source XML references executable code")


def validate_model_geometry_binding(model_xml: Path, geometry_path: Path) -> None:
    """Require one modern or legacy DAGMC XML reference to the hash-checked H5M."""
    root = ET.parse(model_xml).getroot()
    nodes = root.findall(".//dagmc_universe") + root.findall(".//dagmc")
    if len(nodes) != 1:
        raise ValueError("model XML must reference exactly the hash-verified DAGMC geometry artifact")
    node = nodes[0]
    if node.tag == "dagmc_universe":
        reference = node.get("filename")
    else:
        filenames = node.findall("filename")
        reference = filenames[0].text if len(filenames) == 1 else None
    if not isinstance(reference, str) or not reference.strip():
        raise ValueError("model XML must reference exactly the hash-verified DAGMC geometry artifact")
    target = Path(reference)
    if not target.is_absolute():
        target = Path(model_xml).parent / target
    try:
        if target.resolve(strict=True) != Path(geometry_path).resolve(strict=True):
            raise ValueError("model XML must reference exactly the hash-verified DAGMC geometry artifact")
    except OSError as exc:
        raise ValueError("model XML must reference exactly the hash-verified DAGMC geometry artifact") from exc


def _sha256_file(path: Path) -> str:
    return sha256(path)


def _library_identity(openmc) -> dict:
    path = Path(openmc.lib.core._dll._name).resolve()
    if not path.is_file():
        raise RuntimeError("loaded OpenMC C API library path is not a regular file")
    return {"path": str(path), "sha256": _sha256_file(path)}


class MicroXsRunEvidence:
    """Bind R2S neutron C-API loss counts to its exact generated MicroXS model."""

    def __init__(self, openmc, model_path: Path):
        self.openmc, self.expected_model_path = openmc, model_path.resolve()
        self.original_init = self.original_run = None
        self.active = None
        self.events = []
        self.error = None

    def __enter__(self):
        import importlib
        self.lib = getattr(self.openmc, "lib", None) or importlib.import_module("openmc.lib")
        self.original_init, self.original_run = self.lib.init, self.lib.run

        def observed_init(*args, **kwargs):
            model_path = (Path.cwd() / "model.xml").resolve()
            try:
                is_microxs = any(t.get("name") == "MicroXS flux" for t in ET.parse(model_path).getroot().iter("tally"))
            except (ET.ParseError, OSError):
                is_microxs = False
            supplied = args[0] if args else kwargs.get("args")
            if is_microxs and supplied is not None:
                self.error = "MicroXS initialization supplied explicit alternate arguments"
                is_microxs = False
            if is_microxs and set(kwargs) - {"intracomm", "output", "args"}:
                self.error = "MicroXS initialization supplied unrecognized options"
                is_microxs = False
            model_hash = _sha256_file(model_path) if is_microxs else None
            library = _library_identity(self.openmc) if is_microxs else None
            result = self.original_init(*args, **kwargs)
            if is_microxs:
                if model_hash != _sha256_file(model_path) or library != _library_identity(self.openmc):
                    self.error = "MicroXS model or C API library changed during initialization"
                else:
                    if self.active is not None:
                        self.error = "overlapping MicroXS C API initialization observed"
                    self.active = {"path": model_path, "sha256": model_hash, "library": library}
            return result

        def observed_run(*args, **kwargs):
            active = self.active
            if active is None:
                return self.original_run(*args, **kwargs)
            if _sha256_file(active["path"]) != active["sha256"]:
                self.error = "MicroXS model changed after C API initialization"
            result = self.original_run(*args, **kwargs)
            if _sha256_file(active["path"]) != active["sha256"]:
                self.error = "MicroXS model changed during C API run"
            histories = int(self.lib.settings.batches) * int(self.lib.settings.particles)
            try:
                lost = int(ctypes.c_int.in_dll(getattr(self.lib, "_dll", None), "n_lost_particles").value)
            except (AttributeError, OSError, TypeError, ValueError):
                lost = None
            self.events.append({"model_path": str(active["path"]), "model_sha256": active["sha256"],
                                "library": active["library"], "histories": histories, "lost_particles": lost})
            self.active = None
            return result

        self.lib.init, self.lib.run = observed_init, observed_run
        return self

    def __exit__(self, *_):
        self.lib.init, self.lib.run = self.original_init, self.original_run

    def result(self) -> dict:
        if self.error:
            raise RuntimeError(self.error)
        if self.active is not None or len(self.events) != 1:
            raise RuntimeError(f"expected exactly one R2S neutron MicroXS C API run, observed {len(self.events)}")
        event = self.events[0]
        if not self.expected_model_path.is_file() or _sha256_file(self.expected_model_path) != event["model_sha256"]:
            raise RuntimeError("persisted R2S MicroXS model bytes differ from the exact observed C API init input")
        if _library_identity(self.openmc) != event["library"]:
            raise RuntimeError("OpenMC C API library changed during the R2S MicroXS response")
        lost, histories = event["lost_particles"], event["histories"]
        if lost is None:
            raise RuntimeError("OpenMC C API lost-particle counter is unavailable for R2S neutron response")
        if histories < 1 or lost < 0 or lost > 1e-6 * histories:
            raise RuntimeError(f"R2S neutron lost-particle limit exceeded ({lost}/{histories})")
        return {"status": "PASS", "model_xml": {"path": str(self.expected_model_path),
                                                    "sha256": event["model_sha256"]},
                "observed_c_api_init_model_path": event["model_path"],
                "c_api_library": event["library"], "histories": histories, "lost_particles": lost,
                "lost_particle_limit": 1e-6, "scope": "R2S neutron MicroXS response"}


def run_live_scalar(openmc, model_xml: Path, tally_id: int, particles: int, batches_max: int,
                    threads: int, budget_s: float) -> dict:
    """Run one integrated location response with one aggregate value per independent batch."""
    import numpy as np
    lib = openmc.lib
    model_xml = model_xml.resolve()
    model_digest = _sha256_file(model_xml)
    library = _library_identity(openmc)
    previous_cwd = Path.cwd()
    started = time.monotonic()
    histories = batches = 0
    values = []
    last_status = None
    lost_particles = None
    initialized = simulation_started = False
    try:
        os.chdir(model_xml.parent)
        lib.init(["--threads", str(threads), str(model_xml)])
        initialized = True
        lib.simulation_init()
        simulation_started = True
        tally = lib.tallies[tally_id]
        previous = None
        while batches < batches_max:
            if batches >= 2 and time.monotonic() - started >= budget_s:
                break
            last_status = int(lib.next_batch())
            batches = int(tally.num_realizations)
            cumulative = np.asarray(tally.results[:, 0, 1], dtype=float).reshape(-1)
            if cumulative.size != 1 or not np.isfinite(cumulative).all():
                raise RuntimeError("task-location integrated photon tally must be one finite scalar bin")
            current = float(cumulative[0])
            values.append(current if previous is None else current - previous)
            previous = current
            histories = batches * particles
            try:
                lost_particles = int(ctypes.c_int.in_dll(getattr(lib, "_dll", None), "n_lost_particles").value)
            except (AttributeError, OSError, TypeError, ValueError):
                raise RuntimeError("OpenMC C API lost-particle counter is unavailable")
            if lost_particles < 0 or lost_particles > 1e-6 * histories:
                raise RuntimeError(f"photon lost-particle fraction exceeds 1e-6 ({lost_particles}/{histories})")
            if last_status == 1 and batches >= batches_max:
                break
            if last_status not in (0, 1):
                raise RuntimeError(f"photon transport stopped with solver status {last_status}")
    finally:
        try:
            if simulation_started:
                lib.simulation_finalize()
        finally:
            if initialized:
                lib.finalize()
            os.chdir(previous_cwd)
    if batches < 2 or len(values) != batches:
        raise RuntimeError("photon transport needs at least two complete aggregate batches")
    if _sha256_file(model_xml) != model_digest or _library_identity(openmc) != library:
        raise RuntimeError("photon model XML or loaded C API library changed during transport")
    mean = math.fsum(values) / len(values)
    variance = math.fsum((x - mean) ** 2 for x in values) / (len(values) - 1)
    return {"mean": mean, "standard_error": math.sqrt(variance / len(values)), "batches": batches,
            "histories": histories, "lost_particles": lost_particles, "lost_particle_limit": 1e-6,
            "model_xml_sha256": model_digest, "c_api_library": library,
            "elapsed_s": time.monotonic() - started, "uncertainty_scope": "OpenMC fixed-source photon batch sampling conditional on the R2S source."}


def validate_request(request: dict) -> tuple[Path, Path, Path, Path]:
    expected = {"schema", "request_id", "history_sha256", "history_artifact", "event", "model",
                "cross_sections", "chain", "component_material_map", "geometry", "activation_mesh", "response_mesh", "sampling"}
    if set(request) != expected or request["schema"] != REQUEST_SCHEMA:
        raise ValueError(f"request must match {REQUEST_SCHEMA} with no unknown fields")
    for key in ("request_id", "history_sha256"):
        if not isinstance(request[key], str) or not request[key].strip():
            raise ValueError(f"{key} must be nonempty")
    history_path = checked_file(request["history_artifact"], "exact Rust history serialization")
    raw_history = history_path.read_bytes()
    if hashlib.sha256(raw_history).hexdigest() != request["history_sha256"]:
        raise ValueError("history artifact bytes do not match the Rust-authored history SHA-256")
    json.loads(raw_history)  # check it's readable, but never serialize or canonicalize it

    event = request["event"]
    event_fields = {"design", "class", "component", "replacement_k", "shutdown_s",
                    "location_id", "location_sha256", "cooling_s", "source_groups"}
    if not isinstance(event, dict) or set(event) != event_fields:
        raise ValueError("event must contain the complete exact replacement/history binding")
    if (not isinstance(event["shutdown_s"], (int, float)) or not math.isfinite(event["shutdown_s"])
            or event["shutdown_s"] < 0 or isinstance(event["replacement_k"], bool)
            or not isinstance(event["replacement_k"], int) or event["replacement_k"] < 1):
        raise ValueError("replacement event needs a finite shutdown time and positive replacement index")
    if not isinstance(event["design"], str) or not event["design"] or not isinstance(event["component"], str) or not event["component"]:
        raise ValueError("event design and component must be nonempty")
    if not isinstance(event["source_groups"], list) or not event["source_groups"]:
        raise ValueError("event source_groups must include every currently installed irradiated component")
    if not isinstance(event["cooling_s"], list) or not 1 <= len(event["cooling_s"]) <= MAX_COOLING_POINTS:
        raise ValueError("cooling_s must contain 1 to 512 values")
    cooling = event["cooling_s"]
    if any(not isinstance(x, (int, float)) or not math.isfinite(x) or x <= 0 for x in cooling):
        raise ValueError("cooling times must be finite and positive")
    if any(b <= a for a, b in zip(cooling, cooling[1:])):
        raise ValueError("cooling times must be strictly increasing")

    map_path = checked_file(request["component_material_map"], "component-to-material map")
    component_map = json.loads(map_path.read_bytes())
    if (not isinstance(component_map, dict) or component_map.get("schema") != "faris-cad-component-material-map/v0.1"
            or set(component_map) != {"schema", "geometry_sha256", "materials"}):
        raise ValueError("component-to-material map has an unsupported schema")
    seen_ids = set()
    for component, ids in component_map["materials"].items():
        if not isinstance(component, str) or not component or not isinstance(ids, list) or not ids:
            raise ValueError("component map needs a nonempty list of material IDs per solid/component")
        for material_id in ids:
            material_id = positive_int(material_id, "component material id", 2**31 - 1)
            if material_id in seen_ids:
                raise ValueError("component material IDs must be unique across CAD solids")
            seen_ids.add(material_id)
    if component_map["geometry_sha256"] != request["geometry"]["sha256"]:
        raise ValueError("component map is not bound to the exact model geometry artifact")
    seen_components = set()
    for group in event["source_groups"]:
        if not isinstance(group, dict) or set(group) != {"component", "history_sha256", "installation_s", "schedule"}:
            raise ValueError("each source group must bind component, history, installation and schedule")
        if group["history_sha256"] != request["history_sha256"]:
            raise ValueError("source group history SHA differs from the exact event history SHA")
        if group["component"] not in component_map["materials"]:
            raise ValueError(f"no CAD material-ID map for source component {group['component']}")
        if group["component"] in seen_components:
            raise ValueError("event source_groups must contain each active component exactly once")
        seen_components.add(group["component"])
        if (not isinstance(group["installation_s"], (int, float)) or not math.isfinite(group["installation_s"])
                or group["installation_s"] < 0 or group["installation_s"] > event["shutdown_s"]):
            raise ValueError(f"source group {group['component']} has an invalid installation time")
        schedule = group["schedule"]
        if not isinstance(schedule, list) or not 1 <= len(schedule) <= MAX_STEPS:
            raise ValueError("each source schedule must contain 1 to 5000000 intervals")
        total_duration = 0.0
        for index, step in enumerate(schedule):
            if not isinstance(step, dict) or set(step) != {"duration_s", "source_rate_n_s"}:
                raise ValueError(f"source schedule step {index} must contain duration_s and source_rate_n_s")
            duration, rate = step["duration_s"], step["source_rate_n_s"]
            if any(not isinstance(x, (int, float)) or not math.isfinite(x) for x in (duration, rate)):
                raise ValueError(f"source schedule step {index} contains a non-finite value")
            if duration <= 0 or rate < 0:
                raise ValueError(f"source schedule step {index} requires positive duration and nonnegative source rate")
            total_duration += duration
        expected_duration = event["shutdown_s"] - group["installation_s"] + cooling[-1]
        if not math.isclose(total_duration, expected_duration, rel_tol=1e-9, abs_tol=1e-3):
            raise ValueError(f"source schedule for {group['component']} does not span installation through the final cooling point")

    if not isinstance(request["model"], dict) or set(request["model"]) != {"model_xml", "sha256"}:
        raise ValueError("model must contain its model_xml path and hash")
    model = checked_file({"path": request["model"]["model_xml"], "sha256": request["model"]["sha256"]}, "model XML")
    geometry = checked_file(request["geometry"], "DAGMC geometry")
    validate_model_geometry_binding(model, geometry)
    validate_data_only_sources(model)
    xs = checked_file(request["cross_sections"], "cross sections XML")
    chain = checked_file(request["chain"], "depletion chain")
    activation = request["activation_mesh"]
    if not isinstance(activation, dict) or set(activation) != {"lower_left_cm", "upper_right_cm", "dimension"}:
        raise ValueError("activation_mesh must declare lower_left_cm, upper_right_cm and dimension")
    if len(activation["dimension"]) != 3 or any(positive_int(x, "activation mesh dimension", 100_000) <= 0 for x in activation["dimension"]):
        raise ValueError("activation mesh dimension must have three positive bounded entries")
    if math.prod(activation["dimension"]) > 5_000_000:
        raise ValueError("activation mesh exceeds the 5000000-voxel worker bound")
    for bound in (activation["lower_left_cm"], activation["upper_right_cm"]):
        if len(bound) != 3 or any(not isinstance(x, (int, float)) or not math.isfinite(x) for x in bound):
            raise ValueError("activation mesh bounds must be finite 3-vectors in cm")
    if any(a >= b for a, b in zip(activation["lower_left_cm"], activation["upper_right_cm"])):
        raise ValueError("activation mesh upper bounds must exceed lower bounds")

    response = request["response_mesh"]
    if not isinstance(response, dict) or set(response) != {"artifact", "lower_left_cm", "upper_right_cm", "dimension"}:
        raise ValueError("response_mesh must contain an artifact hash and explicit bounds/dimension")
    location_file = checked_file(response["artifact"], "task-location definition")
    if response["dimension"] != [1, 1, 1]:
        raise ValueError("this scalar response worker requires a single explicitly authored task-location volume")
    for bound in (response["lower_left_cm"], response["upper_right_cm"]):
        if len(bound) != 3 or any(not isinstance(x, (int, float)) or not math.isfinite(x) for x in bound):
            raise ValueError("response mesh bounds must be finite 3-vectors in cm")
    if any(a >= b for a, b in zip(response["lower_left_cm"], response["upper_right_cm"])):
        raise ValueError("response mesh upper bounds must exceed lower bounds")
    if event["location_sha256"] != response["artifact"]["sha256"]:
        raise ValueError("event task-location hash does not match response artifact hash")

    sampling = request["sampling"]
    if not isinstance(sampling, dict) or set(sampling) != {"neutron_particles", "neutron_batches", "photon_particles", "photon_batches", "seed", "threads", "mat_vol_samples", "activation_groups", "run_budget_s"}:
        raise ValueError("sampling fields do not match bounded worker contract")
    for key, maximum in (("neutron_particles", MAX_PARTICLES), ("neutron_batches", MAX_BATCHES),
                         ("photon_particles", MAX_PARTICLES), ("photon_batches", MAX_BATCHES),
                         ("threads", 64), ("mat_vol_samples", 10_000_000)):
        positive_int(sampling[key], key, maximum)
    positive_int(sampling["seed"], "seed", 2**31 - 1)
    budget = sampling["run_budget_s"]
    if not isinstance(budget, (int, float)) or not math.isfinite(budget) or not 1 <= budget <= 86400:
        raise ValueError("run_budget_s must be in [1, 86400]")
    if sampling["activation_groups"] not in {"VITAMIN-J-42", "CCFE-709"}:
        raise ValueError("activation_groups must name a supported OpenMC energy structure")
    location = json.loads(location_file.read_bytes())
    if (not isinstance(location, dict) or location.get("id") != event["location_id"]
            or location.get("lower_left_cm") != response["lower_left_cm"]
            or location.get("upper_right_cm") != response["upper_right_cm"]
            or location.get("dimension") != [1, 1, 1]):
        raise ValueError("task-location artifact content does not match event and response-mesh values")
    return model, xs, chain, map_path


def run_common_history_d1s(openmc, request: dict, model_path: Path, out: Path, response_mesh,
                           dose_conversion: float) -> dict:
    """Run stock D1S only when every currently installed source group shares one schedule."""
    import numpy as np
    from openmc.deplete import d1s

    groups = request["event"]["source_groups"]
    first = groups[0]
    same_schedule = all(group["schedule"] == first["schedule"] for group in groups[1:])
    if not same_schedule:
        return {"status": "NOT_EVALUATED",
                "reason": "stock OpenMC D1S time factors are nuclide-global and cannot represent distinct component installation/irradiation schedules; use the event-specific R2S estimate."}

    model = openmc.Model.from_model_xml(str(model_path))
    # The input model has a generated data-defined neutron source distribution. This must have
    # unit strength: the physical neutrons/s are carried by D1S factors and R2S source-rate steps.
    sources = model.settings.source
    if sources is None:
        raise RuntimeError("common-history D1S model has no data-defined neutron source")
    if not isinstance(sources, (list, tuple)):
        sources = [sources]
    source_strength = math.fsum(float(source.strength) for source in sources)
    if not math.isclose(source_strength, 1.0, rel_tol=1e-12, abs_tol=1e-15):
        raise RuntimeError(f"D1S neutron source kernel strength must be 1; found {source_strength}")
    model.settings.run_mode = "fixed source"
    model.settings.photon_transport = True
    model.settings.use_decay_photons = True
    sampling = request["sampling"]
    model.settings.particles = sampling["neutron_particles"]
    model.settings.batches = sampling["neutron_batches"]
    model.settings.seed = sampling["seed"] % (2**31 - 101) + 100
    model.settings.statepoint = {}
    model.settings.sourcepoint = {"write": False}
    model.settings.output = {"summary": False, "tallies": False}
    model.tallies = openmc.Tallies([dose_tally(openmc, response_mesh, "d1s_task_location")])
    chain_path = checked_file(request["chain"], "depletion chain")
    nuclides = d1s.prepare_tallies(model, chain_file=str(chain_path))
    if not nuclides:
        return {"status": "NOT_EVALUATED", "reason": "no parent radionuclides were found in the verified model and depletion chain"}
    tcf = d1s.time_correction_factors(
        nuclides,
        [(step["duration_s"], "s") for step in first["schedule"]],
        [step["source_rate_n_s"] for step in first["schedule"]],
    )
    cooling_s = request["event"]["cooling_s"]
    schedule_indices = []
    elapsed = 0.0
    for index, step in enumerate(first["schedule"]):
        elapsed += step["duration_s"]
        for cooling in cooling_s:
            if math.isclose(elapsed, request["event"]["shutdown_s"] - first["installation_s"] + cooling,
                            rel_tol=1e-9, abs_tol=1e-3):
                schedule_indices.append((cooling_s.index(cooling), index + 1))
    if len(schedule_indices) != len(cooling_s):
        raise RuntimeError("common schedule misses a requested D1S cooling boundary")

    run_dir = out / "d1s_common_history"
    run_dir.mkdir(parents=True, exist_ok=True)
    model_xml = run_dir / "model.xml"
    export_model_xml(model, model_xml)
    model_hash_before = sha256(model_xml)
    dll_path = Path(openmc.lib.core._dll._name).resolve()
    dll_hash_before = sha256(dll_path)
    library = openmc.lib
    tally = model.tallies[0]
    parent_filter = next(f for f in tally.filters if isinstance(f, openmc.ParentNuclideFilter))
    parent_names = [str(n) for n in parent_filter.bins]
    if parent_names != list(nuclides):
        raise RuntimeError("D1S parent-nuclide filter order changed during tally preparation")
    factors = [[float(tcf[name][time_index]) for name in parent_names] for _, time_index in schedule_indices]
    values = [[] for _ in cooling_s]
    previous = np.zeros(len(parent_names), dtype=float)
    batches = 0
    last_status = None
    lost_particles = None
    lost_status_available = False
    started = time.monotonic()
    budget_s = sampling["run_budget_s"]
    initialized = False
    try:
        os.chdir(run_dir)
        library.init(["--threads", str(sampling["threads"]), str(model_xml)])
        initialized = True
        library.simulation_init()
        live_tally = library.tallies[tally.id]
        dll = getattr(library, "_dll", None)
        while batches < sampling["neutron_batches"]:
            if batches >= 2 and time.monotonic() - started >= budget_s:
                break
            last_status = int(library.next_batch())
            batches = int(live_tally.num_realizations)
            cumulative = np.asarray(live_tally.results[:, 0, 1], dtype=float).reshape(-1)
            if cumulative.size != len(parent_names) or not np.isfinite(cumulative).all():
                raise RuntimeError("D1S live tally shape or values are invalid")
            delta = cumulative - previous
            for (point_index, _), weight in zip(schedule_indices, factors):
                values[point_index].append(float(np.dot(delta, np.asarray(weight, dtype=float))))
            previous = cumulative.copy()
            try:
                lost_particles = int(ctypes.c_int.in_dll(dll, "n_lost_particles").value)
                lost_status_available = lost_particles >= 0
            except (AttributeError, OSError, TypeError, ValueError):
                lost_status_available = False
            histories = batches * sampling["neutron_particles"]
            if not lost_status_available:
                raise RuntimeError("OpenMC C API did not expose its lost-particle counter")
            if lost_particles > 1e-6 * histories:
                raise RuntimeError(f"D1S lost-particle fraction exceeds 1e-6 ({lost_particles}/{histories})")
            if last_status == 1 and batches >= sampling["neutron_batches"]:
                break
            if last_status not in (0, 1):
                raise RuntimeError(f"D1S solver stopped with status {last_status}")
    finally:
        try:
            if initialized:
                library.simulation_finalize()
        finally:
            if initialized:
                library.finalize()
    if batches < 2:
        raise RuntimeError("D1S needs at least two complete aggregate batches")
    if sha256(model_xml) != model_hash_before or sha256(dll_path) != dll_hash_before:
        raise RuntimeError("D1S model or loaded OpenMC C library changed during run")
    results = []
    for index, (cooling, samples) in enumerate(zip(cooling_s, values)):
        mean = math.fsum(samples) / len(samples)
        variance = math.fsum((sample - mean) ** 2 for sample in samples) / (len(samples) - 1)
        standard_error = math.sqrt(variance / len(samples))
        results.append({"cooling_s": cooling, "mean_sv_h": mean * dose_conversion,
                        "standard_error_sv_h": standard_error * dose_conversion,
                        "batch_values": len(samples)})
    return {"status": "PASS", "method": "OpenMC D1S", "points": results,
            "parent_nuclides": parent_names, "batch_covariance": "Each independent batch was summed across all parent nuclides before estimating the standard error.",
            "lost_particles": lost_particles, "histories": batches * sampling["neutron_particles"],
            "lost_particle_limit": 1e-6, "elapsed_s": time.monotonic() - started,
            "model_xml_sha256": model_hash_before, "openmc_c_library_path": str(dll_path),
            "openmc_c_library_sha256": dll_hash_before,
            "uncertainty_scope": "Monte Carlo batch sampling only; excludes neutron-response, nuclear-data, geometry, composition, and activation-source uncertainty."}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--request", type=Path)
    parser.add_argument("--prepare-request", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    if (args.request is None) == (args.prepare_request is None):
        parser.error("supply exactly one of --request or --prepare-request")
    request_path = args.prepare_request or args.request
    request_bytes = request_path.read_bytes()
    if len(request_bytes) > MAX_REQUEST_BYTES:
        raise SystemExit(f"request exceeds {MAX_REQUEST_BYTES} bytes")
    request = json.loads(request_bytes)
    out = args.out.resolve()
    if args.prepare_request is not None or request.get("schema") == "faris-maintenance-dose-cad-prepare/v0.1":
        try:
            receipt = prepare_checked_cad_model(request, out)
            receipt["request_sha256"] = hashlib.sha256(request_bytes).hexdigest()
            (out / "dose-cad-preparation-receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True))
            return 0
        except Exception as error:
            print(f"CAD dose preparation failed: {type(error).__name__}: {error}", file=sys.stderr)
            return 2
    if request.get("schema") in {
        "faris-maintenance-709-neutron-request/v0.1",
        "faris-maintenance-709-photon-request/v0.1",
    }:
        try:
            from maintenance_709_response import run_neutron_stage, run_photon_stage
            request_sha = hashlib.sha256(request_bytes).hexdigest()
            if request["schema"] == "faris-maintenance-709-neutron-request/v0.1":
                run_neutron_stage(request, out, request_sha256=request_sha)
            else:
                run_photon_stage(request, out, request_sha256=request_sha)
            return 0
        except Exception as error:
            print(f"709 shutdown-dose stage failed: {type(error).__name__}: {error}", file=sys.stderr)
            return 2
    out.mkdir(parents=True, exist_ok=True)
    record = {"schema": RECEIPT_SCHEMA, "request_id": request.get("request_id"), "status": "NOT_EVALUATED",
              "request_sha256": hashlib.sha256(request_bytes).hexdigest(), "steps": {},
              "uncertainty_scope": "R2S photon-transport batch sampling conditional on the activation source; excludes neutron-response, nuclear-data, geometry, composition, and activation-source uncertainty."}
    record_path = out / "r2s_event_receipt.json"
    try:
        model_path, xs_path, chain_path, map_path = validate_request(request)
        import numpy as np
        import openmc
        import openmc.deplete
        from openmc.deplete import R2SManager
        from openmc.deplete import Chain

        if openmc.__version__ not in {"0.15.3", "0.16.0"}:
            raise RuntimeError(f"OpenMC {openmc.__version__} is outside the validated worker range")
        os.environ["OMP_NUM_THREADS"] = str(request["sampling"]["threads"])
        openmc.config["cross_sections"] = str(xs_path)
        openmc.config["chain_file"] = str(chain_path)
        model = openmc.Model.from_model_xml(str(model_path))
        s = request["sampling"]
        model.settings.run_mode = "fixed source"
        model.settings.photon_transport = False
        model.settings.particles = s["neutron_particles"]
        model.settings.batches = s["neutron_batches"]
        model.settings.seed = s["seed"]
        model.settings.output = {"summary": False, "tallies": False}
        model.settings.statepoint = {"batches": [s["neutron_batches"]]}
        chain = Chain.from_xml(str(chain_path))
        initial_nuclides = {name for material in model.materials for name in material.get_nuclides()}
        chain_coverage = chain_cross_section_coverage(chain, xs_path, initial_nuclides)

        activation = request["activation_mesh"]
        mesh = openmc.RegularMesh(name="faris-component-activation")
        mesh.lower_left = activation["lower_left_cm"]
        mesh.upper_right = activation["upper_right_cm"]
        mesh.dimension = activation["dimension"]
        response = request["response_mesh"]
        response_mesh = openmc.RegularMesh(name="faris-task-location")
        response_mesh.lower_left = response["lower_left_cm"]
        response_mesh.upper_right = response["upper_right_cm"]
        response_mesh.dimension = (1, 1, 1)
        energies, coefficients = openmc.data.dose_coefficients("photon", "AP")
        tally = openmc.Tally(name="task_location_effective_dose")
        tally.filters = [openmc.MeshFilter(response_mesh), openmc.ParticleFilter(["photon"]),
                         openmc.EnergyFunctionFilter(energies, coefficients, interpolation="log-log")]
        tally.scores = ["flux"]
        model.tallies = openmc.Tallies([tally])
        mgr = R2SManager(model, mesh)
        component_map = json.loads(map_path.read_bytes())
        record["runtime"] = {"openmc_python_version": openmc.__version__,
                             "openmc_python_module": str(Path(openmc.__file__).resolve()),
                             "openmc_c_library": getattr(openmc.lib.core._dll, "_name", None),
                             "openmc_c_library_sha256": sha256(Path(openmc.lib.core._dll._name)),
                             "cross_sections_sha256": sha256(xs_path), "chain_sha256": sha256(chain_path),
                             "model_xml_sha256": sha256(model_path),
                             "geometry_sha256": sha256(checked_file(request["geometry"], "DAGMC geometry")),
                             "component_material_map_sha256": sha256(map_path)}
        record["method"] = {"name": "OpenMC mesh-based R2S", "component_domain": "material IDs explicitly scoped to one CAD component; no particle-birth filter",
                             "response": "ICRP-116 AP effective-dose coefficient, one authored task-location volume, Sv/h",
                             "activation_mesh": activation,
                             "response_mesh": {k: v for k, v in response.items() if k != "artifact"},
                             "activation_spatial_approximation": "OpenMC mesh material-volume regions; random material-volume sampling is used and component identities require unique material IDs per CAD solid.",
                             "resolution_convergence": "NOT_EVALUATED: this result uses only the declared activation mesh; compare finer meshes and material-volume sampling before treating spatial-source resolution as adequate.",
                             "material_volume_sampling_count": s["mat_vol_samples"],
                             "activation_chain_coverage": chain_coverage,
                             "neutron_response_reuse_assumption": "One full-geometry neutron response and microscopic cross-section calculation is reused across component histories. This assumes irradiated geometry, material recipes, density, temperature, and boundary conditions remain fixed over those histories; any change requires a separately prepared model."}
        run_kwargs = {"output": False}
        neutron_model_path = out / "neutron_transport" / "model.xml"
        with MicroXsRunEvidence(openmc, neutron_model_path) as neutron_evidence:
            mgr.step1_neutron_transport(out / "neutron_transport",
                mat_vol_kwargs={"n_samples": s["mat_vol_samples"]},
                micro_kwargs={"energies": s["activation_groups"], "chain_file": str(chain_path),
                              "run_kwargs": run_kwargs})
        neutron_run = neutron_evidence.result()
        record["steps"]["neutron_response"] = {**neutron_run, "material_volume_pairs": len(mgr.results["fluxes"])}
        full_mmv = mgr.results["mesh_material_volumes"]
        full_fluxes, full_micros = mgr.results["fluxes"], mgr.results["micros"]
        event = request["event"]
        cooling_s = event["cooling_s"]
        source_meshes_by_cooling = [[] for _ in cooling_s]
        conservation = []
        activation_regions = 0
        component_activation = []
        present_ids = {int(x) for x in np.unique(full_mmv._materials) if x > 0}
        for group_index, group in enumerate(event["source_groups"]):
            target_ids = set(component_map["materials"][group["component"]])
            missing_ids = sorted(target_ids - present_ids)
            if missing_ids:
                raise RuntimeError(f"component {group['component']} CAD material IDs are absent from the mesh response: {missing_ids}")
            keep_pairs = [(e, slot) for e in range(full_mmv.num_elements)
                           for slot, material_id in enumerate(full_mmv._materials[e])
                           if material_id > 0 and int(material_id) in target_ids]
            if not keep_pairs:
                raise RuntimeError(f"component {group['component']} has zero activation mesh volume")
            filtered_materials = np.full_like(full_mmv._materials, -2)
            filtered_volumes = np.zeros_like(full_mmv._volumes)
            for element, slot in keep_pairs:
                filtered_materials[element, slot] = full_mmv._materials[element, slot]
                filtered_volumes[element, slot] = full_mmv._volumes[element, slot]
            # Pair arrays preserve OpenMC's element-major/slot-major filter ordering.
            full_pairs = [(e, j) for e in range(full_mmv.num_elements)
                          for j, material_id in enumerate(full_mmv._materials[e]) if material_id > -1]
            selected_indices = [i for i, (e, j) in enumerate(full_pairs)
                                if int(full_mmv._materials[e, j]) in target_ids]
            if len(selected_indices) != len(keep_pairs):
                raise RuntimeError("mesh material-volume response order did not reconcile")
            mgr.results["mesh_material_volumes"] = openmc.MeshMaterialVolumes(filtered_materials, filtered_volumes)
            mgr.results["fluxes"] = [full_fluxes[i] for i in selected_indices]
            mgr.results["micros"] = [full_micros[i] for i in selected_indices]
            component_volume_cm3 = math.fsum(float(filtered_volumes[e, j]) for e, j in keep_pairs)
            schedule = group["schedule"]
            timesteps = [(step["duration_s"], "s") for step in schedule]
            source_rates = [step["source_rate_n_s"] for step in schedule]
            mgr.step2_activation(timesteps, source_rates, output_dir=out / "activation" / f"group_{group_index}",
                                 operator_kwargs={"chain_file": str(chain_path)})
            activation_regions += len(mgr.results["activation_materials"])
            component_activation.append({"component": group["component"],
                "material_ids": sorted(target_ids), "activation_regions": len(mgr.results["activation_materials"]),
                "mesh_material_volume_cm3": component_volume_cm3,
                "history_sha256": group["history_sha256"], "installation_s": group["installation_s"],
                "schedule_intervals": len(schedule)})
            elapsed = 0.0
            group_indices = []
            for step_index, step in enumerate(schedule):
                elapsed += step["duration_s"]
                for cooling in cooling_s:
                    if math.isclose(elapsed, event["shutdown_s"] - group["installation_s"] + cooling,
                                    rel_tol=1e-9, abs_tol=1e-3):
                        group_indices.append((cooling_s.index(cooling), step_index + 1))
            if len(group_indices) != len(cooling_s):
                raise RuntimeError(f"schedule for {group['component']} misses cooling boundaries")
            for cooling_slot, inventory_index in group_indices:
                group_sources = mgr.get_decay_photon_source_mesh(inventory_index)
                strength = sum(float(source.strength) for mesh_source in group_sources for source in mesh_source.sources.ravel())
                inventory_photons = 0.0
                inv = mgr.results["depletion_results"][inventory_index]
                for material in mgr.results["activation_materials"]:
                    atoms = inv.get_material(str(material.id)).get_nuclide_atoms()
                    for nuclide in chain.nuclides:
                        photon = nuclide.sources.get("photon") if nuclide.sources else None
                        if photon is not None and atoms.get(nuclide.name, 0.0) > 0:
                            inventory_photons += float(atoms[nuclide.name]) * float(np.asarray(photon.p).sum())
                relative = abs(strength - inventory_photons) / inventory_photons if inventory_photons else (0.0 if strength == 0 else math.inf)
                if not math.isfinite(relative) or relative > 1e-6:
                    raise RuntimeError(f"decay source conservation failed for {group['component']} at {cooling_s[cooling_slot]} s: {relative:.3g}")
                source_meshes_by_cooling[cooling_slot].extend(group_sources)
                conservation.append({"component": group["component"], "cooling_s": cooling_s[cooling_slot],
                                     "source_photons_s": strength, "inventory_photons_s": inventory_photons,
                                     "relative_difference": relative, "limit": 1e-6, "status": "PASS"})
        record["steps"]["activation"] = {"status": "PASS", "unique_activation_regions": activation_regions,
                                           "history_sha256": request["history_sha256"],
                                           "history_classes": len(event["source_groups"]),
                                           "component_regions": component_activation}

        photon = openmc.Model(geometry=model.geometry, materials=model.materials,
                              settings=openmc.Settings(), tallies=model.tallies)
        photon.settings.run_mode = "fixed source"
        photon.settings.photon_transport = True
        photon.settings.particles = s["photon_particles"]
        photon.settings.batches = s["photon_batches"]
        photon.settings.seed = s["seed"] + 1
        photon.settings.sourcepoint = {"write": False}
        photon.settings.statepoint = {"batches": [s["photon_batches"]]}
        photon.settings.output = {"summary": False, "tallies": False}
        photon.settings.source = source_meshes_by_cooling[0]
        mesh_volume_cm3 = math.prod(b - a for a, b in zip(response["lower_left_cm"], response["upper_right_cm"]))
        if not math.isfinite(mesh_volume_cm3) or mesh_volume_cm3 <= 0:
            raise RuntimeError("task-location volume is not finite and positive")
        # Coefficient is pSv/cm2; mesh-filtered flux integrates track length, then divide by
        # the authored response volume and convert pSv/s to Sv/h.
        conversion = 3600.0 * 1e-12 / mesh_volume_cm3
        points = []
        record["steps"]["photon_response"] = []
        for point_index, cooling in enumerate(cooling_s):
            if point_index:
                photon.settings.source = source_meshes_by_cooling[point_index]
            photon.settings.seed = s["seed"] + 1 + point_index
            photon_dir = out / "photon_transport" / f"cooling_{point_index}"
            photon_dir.mkdir(parents=True, exist_ok=True)
            photon_model_xml = photon_dir / "model.xml"
            export_model_xml(photon, photon_model_xml)
            photon_result = run_live_scalar(openmc, photon_model_xml, tally.id,
                s["photon_particles"], s["photon_batches"], s["threads"], s["run_budget_s"])
            raw_mean = photon_result["mean"]
            raw_se = photon_result["standard_error"]
            # OpenMC applies the MeshSource photons/s strengths to the fixed-source tally.
            source_strength = sum(float(source.strength) for mesh_source in source_meshes_by_cooling[point_index]
                                  for source in mesh_source.sources.ravel())
            points.append({"cooling_s": cooling, "mean_sv_h": raw_mean * conversion,
                           "standard_error_sv_h": raw_se * conversion,
                           "source_strength_photons_s": source_strength,
                           "sampling_batches": s["photon_batches"], "sampling_particles_per_batch": s["photon_particles"],
                           "normalization": "OpenMC fixed-source tally with MeshSource strengths in photons/s; no additional strength multiplier"})
            record["steps"]["photon_response"].append({**photon_result, "cooling_s": cooling,
                "lost_particle_assessment": {"status": "PASS", "lost_particles": photon_result["lost_particles"],
                    "histories": photon_result["histories"], "fraction_limit": photon_result["lost_particle_limit"]}})
        if any(not math.isfinite(p["mean_sv_h"]) or p["mean_sv_h"] < 0 or not math.isfinite(p["standard_error_sv_h"])
               for p in points):
            raise RuntimeError("dose response contains a non-finite or negative value")
        try:
            d1s_result = run_common_history_d1s(openmc, request, model_path, out, response_mesh, conversion)
        except Exception as exc:
            d1s_result = {"status": "NOT_EVALUATED", "reason": f"common-history D1S run did not produce an accepted estimate: {exc}"}
        record["d1s"] = d1s_result
        record["source_conservation"] = conservation
        record["event"] = {k: v for k, v in event.items() if k != "source_groups"}
        record["points"] = points
        record["status"] = "PASS"
        record["created_unix_s"] = time.time()
    except Exception as exc:
        record["error"] = str(exc)
        record["traceback"] = traceback.format_exc(limit=8)
        record["status"] = "NOT_EVALUATED"
    record_path.write_text(json.dumps(record, indent=2, sort_keys=True, allow_nan=False) + "\n", encoding="utf-8")
    return 0 if record["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
