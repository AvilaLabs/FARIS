# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded, offline R3 MicroXS collapse from OpenMC mesh-flux results.

This producer reads only the pointwise energy grid and cross-section arrays in
each OpenMC neutron HDF5 file. It does not load CE angular distributions or
start OpenMC. Its energy-bin integration reproduces the pinned OpenMC 0.15.3
``MicroXS.from_multigroup_flux`` / ``Nuclide::collapse_rate`` calculation for
the declared group flux: flux is uniform within each group, pointwise XS is
linear between tabulated points, and values outside the table/threshold are
not extrapolated. The output is a one-group MicroXS tensor, streamed by
nuclide and domain chunk.

An audited full-chain coverage report is a hard prerequisite. The caller must
provide the actual neutron-response ``fluxes.npy`` and matching
``mesh_material_volumes.npz`` in MeshMaterialFilter bin order. The files and
all required nuclear-data HDF5 files are hash-pinned in a fresh plan before
the collapse can be run.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import xml.etree.ElementTree as ET

import h5py
import numpy as np

import r3_activation_coverage as AC

SCHEMA = "faris-r3-bounded-microxs/v1"
PLAN_SCHEMA = "faris-r3-bounded-microxs-plan/v1"
ALGORITHM = "openmc-0153-flux-collapse-pointwise-linear-v1"
DEFAULT_REACTION_ORDER = ("(n,gamma)", "(n,2n)", "(n,p)", "(n,3n)", "(n,4n)", "(n,a)", "fission")
DEFAULT_REACTION_MT = {
    "(n,gamma)": 102,
    "(n,2n)": 16,
    "(n,p)": 103,
    "(n,3n)": 17,
    "(n,4n)": 37,
    "(n,a)": 107,
    "fission": 18,
}
VITAMIN_J_42_EDGES_E_V = (
    1000.0, 10000.0, 20000.0, 30000.0, 45000.0, 60000.0, 70000.0, 75000.0,
    100000.0, 150000.0, 200000.0, 300000.0, 400000.0, 450000.0, 510000.0,
    512000.0, 600000.0, 700000.0, 800000.0, 1000000.0, 1330000.0, 1340000.0,
    1500000.0, 1660000.0, 2000000.0, 2500000.0, 3000000.0, 3500000.0,
    4000000.0, 4500000.0, 5000000.0, 5500000.0, 6000000.0, 6500000.0,
    7000000.0, 7500000.0, 8000000.0, 10000000.0, 12000000.0, 14000000.0,
    20000000.0, 30000000.0, 50000000.0,
)
VITAMIN_J_42_EDGE_SHA256 = "1a3ebfef2e84b797cbeab9199fd590a17942d969c92f0f2d551080d31f69fbd6"
GIB = 1024 ** 3
DEFAULT_MAX_OUTPUT_BYTES = 8 * GIB
DEFAULT_DISK_RESERVE_BYTES = 8 * GIB
DEFAULT_MEMORY_LIMIT_BYTES = 512 * 1024 * 1024


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _canonical_sha256(value: object) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    return hashlib.sha256(payload).hexdigest()


def _chain_spec(chain_path: Path, roots: list[str], reaction_mt: dict[str, int]):
    root = ET.parse(chain_path).getroot()
    chain_reactions: dict[str, set[str]] = {}
    adjacency: dict[str, set[str]] = {}
    for entry in root.findall("nuclide"):
        name = entry.get("name")
        if not name or name in chain_reactions:
            raise ValueError("depletion chain has a missing or duplicate nuclide name")
        reaction_names = set()
        targets = set()
        for reaction in entry.findall("reaction"):
            reaction_type = reaction.get("type")
            if not reaction_type:
                raise ValueError(f"chain reaction for {name} has no type")
            if reaction_type not in reaction_mt:
                raise ValueError(f"no pinned OpenMC MT mapping for chain reaction {reaction_type!r}")
            reaction_names.add(reaction_type)
            if reaction.get("target"):
                targets.add(reaction.get("target"))
        for decay in entry.findall("decay"):
            if decay.get("target"):
                targets.add(decay.get("target"))
        chain_reactions[name] = reaction_names
        adjacency[name] = targets
    if not chain_reactions:
        raise ValueError("depletion chain has no nuclides")
    missing = set(roots) - set(chain_reactions)
    if missing:
        raise ValueError("actual material roots absent from chain: " + ", ".join(sorted(missing)[:30]))
    known = set(chain_reactions)
    for name, targets in adjacency.items():
        absent = targets - known
        if absent:
            raise ValueError(f"chain products of {name} are absent from chain: {sorted(absent)[:10]}")
    reachable = set(roots)
    pending = list(roots)
    while pending:
        parent = pending.pop()
        for target in adjacency[parent] - reachable:
            reachable.add(target)
            pending.append(target)
    required = sorted(set(roots) | {name for name in reachable if chain_reactions[name]})
    reaction_types = set().union(*chain_reactions.values())
    reactions = [name for name in DEFAULT_REACTION_ORDER if name in reaction_types]
    if reaction_types - set(reactions):
        raise ValueError("chain contains reactions outside the pinned R3 MicroXS reaction order")
    return chain_reactions, reachable, required, reactions


def _nearest_temperature(handle: h5py.File, nuclide: str, target_k: float, tolerance_k: float) -> str:
    group = handle.get(f"{nuclide}/kTs")
    if group is None or not group.keys():
        raise ValueError(f"{nuclide}: no OpenMC HDF5 temperature table")
    choices = []
    boltzmann_ev_per_k = 8.617333262145e-5
    for name, dataset in group.items():
        value = float(dataset[()])
        if not math.isfinite(value) or value < 0.0:
            raise ValueError(f"{nuclide}: invalid kT value in {name}")
        actual_k = value / boltzmann_ev_per_k
        choices.append((abs(actual_k - target_k), name, actual_k))
    delta, name, actual_k = min(choices)
    if delta >= tolerance_k:
        raise ValueError(f"{nuclide}: nearest data temperature {actual_k:.8g} K is outside {tolerance_k:g} K tolerance")
    return name


def _validate_nuclide_hdf(path: Path, nuclide: str, reaction_names: list[str],
                          reaction_mt: dict[str, int], target_k: float, tolerance_k: float):
    """Read only HDF metadata; return chosen temperature and storage bounds."""
    with h5py.File(path, "r") as handle:
        temp_name = _nearest_temperature(handle, nuclide, target_k, tolerance_k)
        energy_path = f"{nuclide}/energy/{temp_name}"
        energy_ds = handle.get(energy_path)
        if energy_ds is None or len(energy_ds.shape) != 1 or energy_ds.shape[0] < 2:
            raise ValueError(f"{nuclide}: selected temperature has no one-dimensional energy grid")
        point_count = int(energy_ds.shape[0])
        source_bytes = int(energy_ds.size * energy_ds.dtype.itemsize)
        max_chunk = int(np.prod(energy_ds.chunks) * energy_ds.dtype.itemsize) if energy_ds.chunks else source_bytes
        present_reactions = []
        for reaction in reaction_names:
            mt = int(reaction_mt[reaction])
            reaction_group = handle.get(f"{nuclide}/reactions/reaction_{mt:03d}")
            if reaction_group is None:
                continue
            stored_mt = reaction_group.attrs.get("mt")
            if stored_mt is not None and int(stored_mt) != mt:
                raise ValueError(f"{nuclide}: reaction_{mt:03d} declares MT {int(stored_mt)}")
            xs_path = f"{nuclide}/reactions/reaction_{mt:03d}/{temp_name}/xs"
            xs_ds = handle.get(xs_path)
            if xs_ds is None or len(xs_ds.shape) != 1:
                raise ValueError(f"{nuclide}: MT {mt} group has no XS array at selected temperature {temp_name}")
            threshold = xs_ds.attrs.get("threshold_idx")
            if threshold is None:
                raise ValueError(f"{nuclide}: required MT {mt} has no threshold_idx metadata")
            threshold = int(threshold)
            if threshold < 0 or threshold >= point_count or xs_ds.shape[0] != point_count - threshold:
                raise ValueError(f"{nuclide}: required MT {mt} XS dimensions disagree with its energy grid/threshold")
            source_bytes += int(xs_ds.size * xs_ds.dtype.itemsize)
            if xs_ds.chunks:
                max_chunk = max(max_chunk, int(np.prod(xs_ds.chunks) * xs_ds.dtype.itemsize))
            present_reactions.append(reaction)
        return {"temperature_group": temp_name, "energy_points": point_count,
                "required_mt_count": len(reaction_names), "data_reactions": present_reactions,
                "source_array_bytes": source_bytes,
                "largest_hdf_chunk_bytes": max_chunk}


def _integral_at(grid: np.ndarray, xs: np.ndarray, query: np.ndarray) -> np.ndarray:
    """Integrate a threshold-trimmed, linearly interpolated XS from grid[0] to query."""
    if xs.size != grid.size:
        raise ValueError("threshold-trimmed XS and energy grid lengths differ")
    if xs.size == 1:
        return np.zeros_like(query, dtype=np.float64)
    dx = np.diff(grid)
    area = 0.5 * (xs[:-1] + xs[1:]) * dx
    cumulative = np.empty(grid.size, dtype=np.float64)
    cumulative[0] = 0.0
    np.cumsum(area, out=cumulative[1:])
    result = np.zeros_like(query, dtype=np.float64)
    active = query > grid[0]
    above = query >= grid[-1]
    result[above] = cumulative[-1]
    inside = active & ~above
    if np.any(inside):
        q = query[inside]
        index = np.searchsorted(grid, q, side="right") - 1
        index = np.clip(index, 0, grid.size - 2)
        offset = q - grid[index]
        slope = (xs[index + 1] - xs[index]) / (grid[index + 1] - grid[index])
        result[inside] = cumulative[index] + xs[index] * offset + 0.5 * slope * offset * offset
    return result


def group_mean_xs(energy_grid: np.ndarray, xs_values: np.ndarray, threshold_idx: int,
                  energy_edges: np.ndarray) -> np.ndarray:
    """Return group-bin mean XS matching OpenMC 0.15.3's collapse_rate integral.

    Values below the HDF threshold and outside the tabulated energy grid
    contribute zero. Equal adjacent energy points are retained as zero-width
    discontinuities, matching OpenMC's C++ collapse loop, which skips their
    zero-width segments.
    """
    grid = np.asarray(energy_grid, dtype=np.float64)
    values = np.asarray(xs_values, dtype=np.float64)
    edges = np.asarray(energy_edges, dtype=np.float64)
    if grid.ndim != 1 or values.ndim != 1 or edges.ndim != 1:
        raise ValueError("energy grid, XS values, and group edges must be one-dimensional")
    if not np.all(np.isfinite(grid)) or not np.all(np.isfinite(values)) or not np.all(np.isfinite(edges)):
        raise ValueError("energy grid, XS values, and group edges must be finite")
    if np.any(np.diff(grid) < 0.0):
        raise ValueError("OpenMC energy grid must be nondecreasing for the pinned collapse algorithm")
    if np.any(values < 0.0) or np.any(np.diff(edges) <= 0.0):
        raise ValueError("XS values must be nonnegative and energy group edges strictly ascending")
    if threshold_idx < 0 or threshold_idx >= grid.size or values.size != grid.size - threshold_idx:
        raise ValueError("XS length and threshold_idx do not match the energy grid")
    threshold_grid = grid[threshold_idx:]
    lo = np.maximum(edges[:-1], threshold_grid[0])
    hi = np.minimum(edges[1:], threshold_grid[-1])
    valid = hi > lo
    means = np.zeros(edges.size - 1, dtype=np.float64)
    if np.any(valid):
        cdf = _integral_at(threshold_grid, values, hi[valid]) - \
              _integral_at(threshold_grid, values, lo[valid])
        means[valid] = cdf / np.diff(edges)[valid]
    if not np.all(np.isfinite(means)) or np.any(means < 0.0):
        raise ValueError("collapsed group means are invalid")
    return means


def collapse_flux(means: np.ndarray, flux: np.ndarray) -> np.ndarray:
    """Flux-average precomputed group means; zero flux follows the OpenMC API."""
    values = np.asarray(means, dtype=np.float64)
    spectrum = np.asarray(flux, dtype=np.float64)
    if values.ndim != 2 or spectrum.ndim != 1 or values.shape[1] != spectrum.size:
        raise ValueError("group-mean XS and domain flux dimensions disagree")
    if not np.all(np.isfinite(values)) or np.any(values < 0.0):
        raise ValueError("group-mean XS must be finite and nonnegative")
    if not np.all(np.isfinite(spectrum)) or np.any(spectrum < 0.0):
        raise ValueError("domain flux must be finite and nonnegative")
    total = float(np.sum(spectrum))
    if total == 0.0:
        return np.zeros(values.shape[0], dtype=np.float64)
    return (values @ spectrum) / total


def _domain_table(volumes_path: Path, domain_count_expected: int):
    with np.load(volumes_path, allow_pickle=False) as data:
        if set(data.files) != {"materials", "volumes"}:
            raise ValueError("mesh material volumes NPZ has an unexpected array catalogue")
        materials = np.asarray(data["materials"])
        volumes = np.asarray(data["volumes"], dtype=np.float64)
    if materials.ndim != 2 or volumes.shape != materials.shape:
        raise ValueError("mesh material/volume arrays have inconsistent dimensions")
    mask = materials > -1
    elements, slots = np.where(mask)
    material_ids = materials[elements, slots].astype(np.int64, copy=False)
    domain_volumes = volumes[elements, slots]
    if len(elements) != domain_count_expected:
        raise ValueError(f"mesh material volume domain count {len(elements)} differs from flux rows {domain_count_expected}")
    if np.any(material_ids < 0) or not np.all(np.isfinite(domain_volumes)) or np.any(domain_volumes <= 0.0):
        raise ValueError("mesh material domains must have nonnegative material IDs and positive finite volumes")
    return elements.astype(np.int64), material_ids, domain_volumes


def _input_file(path: Path, label: str) -> dict:
    path = Path(path).resolve(strict=True)
    if not path.is_file():
        raise ValueError(f"{label} is not a regular file: {path}")
    return {"path": str(path), "sha256": sha256_file(path), "size_bytes": path.stat().st_size}


def prepare_plan(plan_path: Path, *, fluxes_path: Path, volumes_path: Path, chain_path: Path,
                 cross_sections_xml: Path, materials_xml: Path, energy_edges_eV,
                 output_run_dir: Path,
                 temperature_k: float = 293.6, temperature_tolerance_k: float = 1.0,
                 domain_chunk: int = 64, output_budget_bytes: int = DEFAULT_MAX_OUTPUT_BYTES,
                 disk_reserve_bytes: int = DEFAULT_DISK_RESERVE_BYTES,
                 memory_limit_bytes: int = DEFAULT_MEMORY_LIMIT_BYTES,
                 reaction_mt: dict[str, int] | None = None) -> dict:
    """Create a frozen preflight plan at a new path; no solver is run."""
    plan_path = Path(plan_path).resolve()
    if plan_path.exists():
        raise FileExistsError(f"refusing to overwrite existing R3 collapse plan: {plan_path}")
    output_run_dir = Path(output_run_dir).resolve()
    if output_run_dir.exists():
        raise FileExistsError(f"refusing to overwrite existing bounded MicroXS run: {output_run_dir}")
    output_run_dir.parent.mkdir(parents=True, exist_ok=True)
    plan_path.parent.mkdir(parents=True, exist_ok=True)
    reaction_mt = dict(DEFAULT_REACTION_MT if reaction_mt is None else reaction_mt)
    paths = {"fluxes": Path(fluxes_path).resolve(strict=True),
             "volumes": Path(volumes_path).resolve(strict=True),
             "chain": Path(chain_path).resolve(strict=True),
             "cross_sections": Path(cross_sections_xml).resolve(strict=True),
             "materials": Path(materials_xml).resolve(strict=True)}
    if domain_chunk < 1 or output_budget_bytes < 1 or disk_reserve_bytes < 0 or memory_limit_bytes < 1:
        raise ValueError("resource bounds and domain chunk must be positive")
    if not math.isfinite(temperature_k) or temperature_k <= 0.0 or \
            not math.isfinite(temperature_tolerance_k) or temperature_tolerance_k <= 0.0:
        raise ValueError("temperature and temperature tolerance must be finite and positive")
    edges = np.asarray(energy_edges_eV, dtype=np.float64)
    if edges.ndim != 1 or edges.size < 2 or not np.all(np.isfinite(edges)) or np.any(np.diff(edges) <= 0.0):
        raise ValueError("R3 energy group edges must be finite and strictly ascending")
    fluxes = np.load(paths["fluxes"], mmap_mode="r", allow_pickle=False)
    if fluxes.ndim != 2 or fluxes.shape[1] != edges.size - 1 or not np.issubdtype(fluxes.dtype, np.number):
        raise ValueError("fluxes.npy must have exactly one column per declared energy group")
    elements, material_ids, domain_volumes = _domain_table(paths["volumes"], int(fluxes.shape[0]))
    if not len(elements):
        raise ValueError("mesh material volume artifact contains no activation domains")
    zero_flux_count = 0
    for start in range(0, int(fluxes.shape[0]), domain_chunk):
        spectra = np.asarray(fluxes[start:start + domain_chunk], dtype=np.float64)
        if not np.all(np.isfinite(spectra)) or np.any(spectra < 0.0):
            raise ValueError(f"neutron flux artifact has invalid values in domains {start}:{start + domain_chunk}")
        zero_flux_count += int(np.count_nonzero(np.sum(spectra, axis=1) == 0.0))
    roots = AC.actual_material_nuclides(paths["materials"])
    coverage = AC.assess_coverage(paths["chain"], paths["cross_sections"], roots, reaction_mt)
    if coverage["status"] != "READY_FOR_R2S_TRANSPORT":
        raise ValueError("bounded collapse requires READY full-chain coverage; assessment is NOT_EVALUATED")
    chain_reactions, reachable, required, reactions = _chain_spec(paths["chain"], roots, reaction_mt)
    if required != coverage["required_microxs_nuclides"]:
        raise ValueError("chain requirement list differs from the independent activation coverage guard")
    if int(fluxes.shape[1]) != 42:
        raise ValueError("this predeclared R3 bounded collapse currently requires the VITAMIN-J-42 42-group basis")
    edges_hash = hashlib.sha256(np.asarray(edges, dtype="<f8").tobytes()).hexdigest()
    if edges_hash != VITAMIN_J_42_EDGE_SHA256:
        raise ValueError("bounded R3 collapse requires the pinned OpenMC VITAMIN-J-42 group edges")
    hdf_by_nuclide = AC._cross_section_index(paths["cross_sections"])
    hdf_hashes = {item["nuclide"]: item["sha256"] for item in coverage["indexed_data_files"]}
    specs = {}
    max_source_bytes = 0
    max_hdf_chunk_bytes = 0
    for nuclide in required:
        channel_names = reactions
        specs[nuclide] = _validate_nuclide_hdf(hdf_by_nuclide[nuclide], nuclide, channel_names,
                                                reaction_mt, temperature_k, temperature_tolerance_k)
        max_source_bytes = max(max_source_bytes, specs[nuclide]["source_array_bytes"])
        max_hdf_chunk_bytes = max(max_hdf_chunk_bytes, specs[nuclide]["largest_hdf_chunk_bytes"])
    nuclide_count = len(required)
    reaction_count = len(reactions)
    domain_count = int(fluxes.shape[0])
    domain_map_hash = _canonical_sha256([
        [int(element), int(material), float(volume)]
        for element, material, volume in zip(elements, material_ids, domain_volumes)
    ])
    output_raw_bytes = domain_count * nuclide_count * reaction_count * np.dtype(np.float64).itemsize
    if output_raw_bytes > output_budget_bytes:
        raise ValueError(f"one-group MicroXS archive needs {output_raw_bytes} bytes, above {output_budget_bytes}-byte budget")
    chunk_domains = min(domain_chunk, domain_count)
    working_estimate = 4 * max_source_bytes + max_hdf_chunk_bytes + \
        chunk_domains * (edges.size + reaction_count * 3) * np.dtype(np.float64).itemsize + 128 * 1024 * 1024
    if working_estimate > memory_limit_bytes:
        raise ValueError(f"bounded collapse working estimate {working_estimate} exceeds memory limit {memory_limit_bytes}")
    output_sidecar_bytes = domain_count * (2 * 8 + 8) + nuclide_count * reaction_count + 16 * 1024 * 1024
    output_upper_bytes = output_raw_bytes + output_sidecar_bytes
    free_bytes = shutil.disk_usage(output_run_dir.parent).free
    required_free = output_upper_bytes + disk_reserve_bytes
    if free_bytes < required_free:
        raise ValueError(f"collapse output requires at most {output_upper_bytes} bytes plus {disk_reserve_bytes} reserve; only {free_bytes} bytes free")
    inputs = {name: _input_file(path, name) for name, path in paths.items()}
    hdf_inputs = [{"nuclide": n, "path": str(hdf_by_nuclide[n]), "sha256": hdf_hashes[n]}
                  for n in required]
    guard_impl = Path(AC.__file__).resolve()
    plan = {
        "schema": PLAN_SCHEMA,
        "status": "READY_FOR_BOUNDED_COLLAPSE",
        "algorithm": ALGORITHM,
        "implementation": {"path": str(Path(__file__).resolve()), "sha256": sha256_file(Path(__file__)),
                           "coverage_guard_path": str(guard_impl), "coverage_guard_sha256": sha256_file(guard_impl)},
        "inputs": inputs,
        "output_run_dir": str(output_run_dir),
        "required_nuclide_hdf5": hdf_inputs,
        "coverage": coverage,
        "material_root_nuclides": roots,
        "reachable_chain_nuclide_count": len(reachable),
        "required_microxs_nuclides": required,
        "reactions": reactions,
        "reaction_mt": {name: reaction_mt[name] for name in reactions},
        "energy_edges_eV": edges.tolist(),
        "energy_edges_sha256": edges_hash,
        "energy_group_structure": "VITAMIN-J-42",
        "temperature": {"requested_K": temperature_k, "method": "nearest", "tolerance_K": temperature_tolerance_k,
                        "per_nuclide_hdf": specs},
        "mesh_material_domain_order": "row-major np.where(mesh_material_volumes.materials > -1), identical to OpenMC 0.15.3 MeshMaterialFilter.from_volumes",
        "domains": {"count": domain_count, "mesh_elements": int(np.max(elements)) + 1 if len(elements) else 0,
                    "material_ids": sorted(map(int, np.unique(material_ids))),
                    "volume_sum_cm3": float(math.fsum(map(float, domain_volumes))),
                    "element_material_volume_map_sha256": domain_map_hash},
        "resources": {"max_domain_chunk": chunk_domains, "estimated_working_bytes": working_estimate,
                      "max_source_arrays_bytes": max_source_bytes, "largest_hdf_chunk_bytes": max_hdf_chunk_bytes,
                      "output_raw_bytes_upper_bound": output_raw_bytes,
                      "output_file_bytes_upper_bound": output_upper_bytes,
                      "output_budget_bytes": output_budget_bytes, "disk_reserve_bytes": disk_reserve_bytes,
                      "free_bytes_at_plan": free_bytes, "memory_limit_bytes": memory_limit_bytes,
                      "estimate_scope": "one nuclide's required energy/XS arrays, one HDF chunk, flux/XS domain chunk, and 128 MiB fixed Python/HDF overhead; excludes any external OpenMC process"},
        "flux_zero_domain_count": zero_flux_count,
    }
    plan["plan_sha256"] = _canonical_sha256(plan)
    with plan_path.open("x", encoding="utf-8") as stream:
        json.dump(plan, stream, indent=2, sort_keys=True, allow_nan=False)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    return plan


def _check_plan(plan: dict) -> tuple[dict[str, Path], np.ndarray, list[str], list[str], list[dict]]:
    if plan.get("schema") != PLAN_SCHEMA or plan.get("status") != "READY_FOR_BOUNDED_COLLAPSE":
        raise ValueError("unsupported or unready bounded MicroXS plan")
    declared = dict(plan)
    declared_sha = declared.pop("plan_sha256", None)
    if declared_sha != _canonical_sha256(declared):
        raise ValueError("bounded MicroXS plan checksum mismatch")
    if plan.get("algorithm") != ALGORITHM:
        raise ValueError("bounded MicroXS plan uses an unsupported collapse algorithm")
    current_implementation = sha256_file(Path(__file__))
    current_guard = sha256_file(Path(AC.__file__).resolve())
    if plan["implementation"]["sha256"] != current_implementation or plan["implementation"]["coverage_guard_sha256"] != current_guard:
        raise ValueError("bounded MicroXS implementation or activation coverage guard changed after planning")
    paths = {name: Path(item["path"]).resolve(strict=True) for name, item in plan["inputs"].items()}
    for name, path in paths.items():
        if sha256_file(path) != plan["inputs"][name]["sha256"]:
            raise ValueError(f"bounded MicroXS input changed after planning: {name}")
    for item in plan["required_nuclide_hdf5"]:
        if sha256_file(Path(item["path"])) != item["sha256"]:
            raise ValueError(f"required nuclear-data file changed after planning: {item['nuclide']}")
    edge_values = np.asarray(plan["energy_edges_eV"], dtype=np.float64)
    if hashlib.sha256(edge_values.astype("<f8", copy=False).tobytes()).hexdigest() != plan["energy_edges_sha256"]:
        raise ValueError("bounded MicroXS energy group edges changed after planning")
    return paths, edge_values, list(plan["required_microxs_nuclides"]), list(plan["reactions"]), list(plan["required_nuclide_hdf5"])


def run_collapse(plan_path: Path, run_dir: Path) -> dict:
    """Stream one-group MicroXS collapse into a fresh bounded HDF5 artifact.

    This is an offline numerical/data transformation. It does not initialize
    OpenMC, create an OpenMC model, or execute a solver.
    """
    plan_path = Path(plan_path).resolve(strict=True)
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    paths, edges, nuclides, reactions, hdf_inputs = _check_plan(plan)
    run_dir = Path(run_dir).resolve()
    if run_dir != Path(plan["output_run_dir"]).resolve():
        raise ValueError("bounded MicroXS execution destination differs from its frozen plan")
    if run_dir.exists():
        raise FileExistsError(f"refusing to overwrite existing bounded MicroXS run: {run_dir}")
    run_dir.parent.mkdir(parents=True, exist_ok=True)
    run_dir.mkdir()
    flux = np.load(paths["fluxes"], mmap_mode="r", allow_pickle=False)
    elements, material_ids, volumes = _domain_table(paths["volumes"], int(flux.shape[0]))
    domain_map_hash = _canonical_sha256([
        [int(element), int(material), float(volume)]
        for element, material, volume in zip(elements, material_ids, volumes)
    ])
    if domain_map_hash != plan["domains"]["element_material_volume_map_sha256"]:
        raise ValueError("mesh/material/volume domain map changed after planning")
    chain_reactions, _, _, _ = _chain_spec(paths["chain"], plan["material_root_nuclides"], plan["reaction_mt"])
    xml_index = AC._cross_section_index(paths["cross_sections"])
    mt_by_name = plan["reaction_mt"]
    specs = plan["temperature"]["per_nuclide_hdf"]
    n_domains, n_nuclides, n_reactions = flux.shape[0], len(nuclides), len(reactions)
    chunk = int(plan["resources"]["max_domain_chunk"])
    zero_flux_count = 0
    output_path = run_dir / "microxs_one_group.h5"
    with h5py.File(output_path, "x") as out:
        out.attrs["schema"] = SCHEMA
        out.attrs["algorithm"] = ALGORITHM
        out.attrs["plan_sha256"] = plan["plan_sha256"]
        out.attrs["energy_edges_sha256"] = plan["energy_edges_sha256"]
        out.create_dataset("nuclides", data=np.asarray(nuclides, dtype="S"))
        out.create_dataset("reactions", data=np.asarray(reactions, dtype="S"))
        out.create_dataset("domain_element", data=elements)
        out.create_dataset("domain_material_id", data=material_ids)
        out.create_dataset("domain_volume_cm3", data=volumes)
        out.create_dataset("reaction_required_mask", data=np.asarray(
            [[reaction in chain_reactions[name] for reaction in reactions] for name in nuclides], dtype=np.uint8))
        out.create_dataset("reaction_data_present_mask", data=np.asarray(
            [[reaction in specs[name]["data_reactions"] for reaction in reactions] for name in nuclides], dtype=np.uint8))
        chunk_shape = (1, min(chunk, n_domains), n_reactions)
        data = out.create_dataset("microxs_barns", shape=(n_nuclides, n_domains, n_reactions),
                                  dtype=np.float64, chunks=chunk_shape, compression="lzf",
                                  shuffle=True, fillvalue=0.0)
        for ni, name in enumerate(nuclides):
            channel_names = list(specs[name]["data_reactions"])
            group_means = {}
            if channel_names:
                data_path = xml_index[name]
                temp_name = specs[name]["temperature_group"]
                with h5py.File(data_path, "r") as hdf:
                    energy = np.asarray(hdf[f"{name}/energy/{temp_name}"][()], dtype=np.float64)
                    for reaction in channel_names:
                        mt = int(mt_by_name[reaction])
                        xs_dataset = hdf[f"{name}/reactions/reaction_{mt:03d}/{temp_name}/xs"]
                        threshold = int(xs_dataset.attrs["threshold_idx"])
                        xs_values = np.asarray(xs_dataset[()], dtype=np.float64)
                        if not np.all(np.isfinite(xs_values)) or np.any(xs_values < 0.0):
                            raise ValueError(f"{name} MT {mt} has invalid pointwise XS values")
                        group_means[reaction] = group_mean_xs(energy, xs_values, threshold, edges)
            for start in range(0, n_domains, chunk):
                stop = min(start + chunk, n_domains)
                spectra = np.asarray(flux[start:stop], dtype=np.float64)
                if not np.all(np.isfinite(spectra)) or np.any(spectra < 0.0):
                    raise ValueError(f"neutron flux artifact has invalid values in domains {start}:{stop}")
                totals = np.sum(spectra, axis=1)
                zero_flux_count += int(np.count_nonzero(totals == 0.0)) if ni == 0 else 0
                block = np.zeros((stop - start, n_reactions), dtype=np.float64)
                nonzero = totals > 0.0
                for reaction, means in group_means.items():
                    ri = reactions.index(reaction)
                    if np.any(nonzero):
                        block[nonzero, ri] = (spectra[nonzero] @ means) / totals[nonzero]
                data[ni, start:stop, :] = block
    for item in hdf_inputs:
        if sha256_file(Path(item["path"])) != item["sha256"]:
            raise ValueError(f"nuclear-data file changed during bounded collapse: {item['nuclide']}")
    if output_path.stat().st_size > plan["resources"]["output_budget_bytes"]:
        raise ValueError("bounded MicroXS HDF5 exceeds the predeclared output budget")
    record = {
        "schema": SCHEMA,
        "status": "PASS",
        "algorithm": ALGORITHM,
        "plan_path": str(plan_path),
        "plan_sha256": plan["plan_sha256"],
        "output": {"path": str(output_path), "sha256": sha256_file(output_path),
                   "size_bytes": output_path.stat().st_size},
        "domains": n_domains,
        "required_nuclides": n_nuclides,
        "reactions": reactions,
        "zero_flux_domain_count": zero_flux_count,
        "zero_flux_behavior": "matches OpenMC MicroXS.from_multigroup_flux: returns zero one-group MicroXS for a domain with zero total declared-group flux; nuclear-data coverage is checked independently and no required nuclide/channel is omitted",
        "source_files": plan["inputs"],
        "required_nuclide_hdf5": hdf_inputs,
        "activation_coverage": plan["coverage"],
        "uncertainty_scope": "No new transport or depletion uncertainty is estimated; this file is a deterministic collapse of the recorded neutron flux mean and pointwise nuclear data.",
    }
    record_path = run_dir / "collapse_record.json"
    with record_path.open("x", encoding="utf-8") as stream:
        json.dump(record, stream, indent=2, sort_keys=True, allow_nan=False)
        stream.write("\n")
    record["record_sha256"] = sha256_file(record_path)
    return record


def read_microxs_chunk(path: Path, start: int, stop: int):
    """Read a bounded domain chunk into OpenMC MicroXS objects and flux arrays."""
    try:
        import openmc.deplete
    except ImportError as exc:
        raise RuntimeError("OpenMC Python is required only to create activation objects from a completed collapse") from exc
    path = Path(path)
    with h5py.File(path, "r") as handle:
        if handle.attrs.get("schema") != SCHEMA:
            raise ValueError("one-group MicroXS artifact has an unsupported schema")
        names = [v.decode() if isinstance(v, bytes) else str(v) for v in handle["nuclides"][()]]
        reactions = [v.decode() if isinstance(v, bytes) else str(v) for v in handle["reactions"][()]]
        total = handle["microxs_barns"].shape[1]
        if start < 0 or stop <= start or stop > total:
            raise ValueError("requested MicroXS domain chunk is outside the archive")
        array = np.asarray(handle["microxs_barns"][:, start:stop, :], dtype=np.float64)
    micros = [openmc.deplete.MicroXS(array[:, i, :, None], names, reactions) for i in range(stop - start)]
    return micros


def compare_capi_reference(cross_sections_xml: Path, fluxes_path: Path, energy_edges_eV,
                           checks: list[dict], *, temperature_k: float = 293.6,
                           temperature_tolerance_k: float = 1.0, max_nuclides: int = 3,
                           relative_tolerance: float = 2.0e-12) -> dict:
    """Compare the offline collapse to pinned OpenMC CAPI for <=3 actual nuclides.

    This initializes OpenMC's C API only to load one requested nuclide at a
    time and call ``Nuclide.collapse_rate``. It does not create a transport
    tally or run a solver. The input HDF coverage and the energy/flux rows must
    come from the same prepared neutron problem. A successful sample check
    validates this numerical implementation; it does not qualify the library
    or activate the full case.
    """
    if not 1 <= len(checks) <= max_nuclides or max_nuclides > 3:
        raise ValueError("CAPI reference is bounded to one through three nuclides")
    xs_path = Path(cross_sections_xml).resolve(strict=True)
    flux_path = Path(fluxes_path).resolve(strict=True)
    edges = np.asarray(energy_edges_eV, dtype=np.float64)
    flux_rows = np.load(flux_path, mmap_mode="r", allow_pickle=False)
    if edges.ndim != 1 or np.any(np.diff(edges) <= 0.0) or flux_rows.ndim != 2 or flux_rows.shape[1] != edges.size - 1:
        raise ValueError("CAPI reference flux and energy groups have inconsistent dimensions")
    if hashlib.sha256(edges.astype("<f8", copy=False).tobytes()).hexdigest() != VITAMIN_J_42_EDGE_SHA256:
        raise ValueError("CAPI reference requires the pinned VITAMIN-J-42 group boundaries")
    seen = set()
    prepared = []
    index = AC._cross_section_index(xs_path)
    for check in checks:
        if set(check) != {"nuclide", "reaction", "domain_index"}:
            raise ValueError("each CAPI reference check must name exactly nuclide, reaction, and domain_index")
        name, reaction, domain = str(check["nuclide"]), str(check["reaction"]), int(check["domain_index"])
        if reaction not in DEFAULT_REACTION_MT or name not in index or not index[name].is_file():
            raise ValueError(f"CAPI reference check lacks a mapped nuclide or reaction: {name}/{reaction}")
        if (name, reaction, domain) in seen or domain < 0 or domain >= flux_rows.shape[0]:
            raise ValueError("CAPI reference check is duplicate or names an invalid domain row")
        seen.add((name, reaction, domain))
        spectrum = np.asarray(flux_rows[domain], dtype=np.float64)
        if not np.all(np.isfinite(spectrum)) or np.any(spectrum < 0.0) or float(np.sum(spectrum)) <= 0.0:
            raise ValueError(f"CAPI reference domain {domain} has no valid positive spectrum")
        norm = spectrum / np.sum(spectrum)
        mt = DEFAULT_REACTION_MT[reaction]
        with h5py.File(index[name], "r") as hdf:
            temp_name = _nearest_temperature(hdf, name, temperature_k, temperature_tolerance_k)
            energy = np.asarray(hdf[f"{name}/energy/{temp_name}"][()], dtype=np.float64)
            xs_dataset = hdf.get(f"{name}/reactions/reaction_{mt:03d}/{temp_name}/xs")
            if xs_dataset is None:
                raise ValueError(f"CAPI reference required MT {mt} absent for {name} at {temp_name}")
            threshold = int(xs_dataset.attrs["threshold_idx"])
            pointwise = np.asarray(xs_dataset[()], dtype=np.float64)
        means = group_mean_xs(energy, pointwise, threshold, edges)
        offline = float(collapse_flux(means[None, :], spectrum)[0])
        prepared.append({"nuclide": name, "reaction": reaction, "mt": mt,
                         "domain_index": domain, "normalized_flux": norm,
                         "offline_microxs_b": offline})
    try:
        import openmc
    except ImportError as exc:
        raise RuntimeError("pinned OpenMC Python is required for the sample CAPI reference") from exc
    if str(openmc.__version__) != "0.15.3":
        raise ValueError(f"CAPI comparison requires OpenMC 0.15.3, got {openmc.__version__}")
    library_name = getattr(getattr(openmc.lib, "_dll", None), "_name", None)
    if not library_name or not Path(library_name).is_file():
        raise ValueError("OpenMC 0.15.3 C API shared library path is unavailable")
    library_path = Path(library_name).resolve()
    library_hash = sha256_file(library_path)
    previous_cross_sections = openmc.config.get("cross_sections")
    results = []
    api_error = None
    openmc.config["cross_sections"] = str(xs_path)
    try:
        with openmc.lib.TemporarySession():
            for item in prepared:
                try:
                    lib_nuclide = openmc.lib.load_nuclide(item["nuclide"])
                    api_value = float(lib_nuclide.collapse_rate(item["mt"], temperature_k,
                                                                 edges, item["normalized_flux"]))
                    diff = abs(api_value - item["offline_microxs_b"])
                    scale = max(abs(api_value), abs(item["offline_microxs_b"]), np.finfo(float).tiny)
                    relative = diff / scale
                    check = {key: value for key, value in item.items() if key != "normalized_flux"} | {
                        "capi_microxs_b": api_value, "absolute_difference_b": diff,
                        "relative_difference": relative, "tolerance": relative_tolerance,
                    }
                    results.append(check)
                    if not math.isfinite(api_value) or relative > relative_tolerance:
                        api_error = (f"offline/CAPI MicroXS mismatch for {item['nuclide']} MT {item['mt']}: "
                                     f"rel={relative:g}")
                        break
                except Exception as exc:  # keep TemporarySession's __exit__ on its normal cleanup path
                    api_error = f"OpenMC CAPI reference failed for {item['nuclide']} MT {item['mt']}: {type(exc).__name__}"
                    break
    finally:
        if previous_cross_sections is None:
            openmc.config.pop("cross_sections", None)
        else:
            openmc.config["cross_sections"] = previous_cross_sections
    if sha256_file(library_path) != library_hash:
        raise ValueError("OpenMC C API shared library changed during sample comparison")
    if api_error is not None:
        raise ValueError(api_error)
    return {
        "schema": "faris-r3-bounded-microxs-capi-check/v1",
        "status": "PASS_NUMERICAL_SAMPLE_CHECK",
        "openmc_version": str(openmc.__version__),
        "openmc_c_api_library": {"path": str(library_path), "sha256": library_hash},
        "cross_sections_xml": {"path": str(xs_path), "sha256": sha256_file(xs_path)},
        "fluxes": {"path": str(flux_path), "sha256": sha256_file(flux_path)},
        "energy_edges_eV_sha256": hashlib.sha256(edges.astype("<f8", copy=False).tobytes()).hexdigest(),
        "checks": results,
        "reference_scope": "CAPI load_nuclide plus collapse_rate only; no transport solver was started",
        "qualification_limit": "A bounded sample agreement validates the collapse implementation only; full-chain data coverage, evaluation provenance, decay data and model adequacy remain separate gates.",
    }


def domain_source_slot_map(materials: np.ndarray) -> list[dict[str, int]]:
    """Map each material domain to the exact material slot used by R2S MeshSource.

    OpenMC's ``MeshMaterialVolumes.by_element`` omits ``-2`` padding but keeps
    ``-1`` void slots. The index in that filtered row is the slot index used by
    ``R2SManager.get_decay_photon_source_mesh``.
    """
    values = np.asarray(materials)
    if values.ndim != 2 or not np.issubdtype(values.dtype, np.integer):
        raise ValueError("mesh material IDs must be a two-dimensional integer array")
    result = []
    for element in range(values.shape[0]):
        slot = 0
        for raw in values[element]:
            material_id = int(raw)
            if material_id == -2:
                continue
            if material_id > -1:
                result.append({"domain_index": len(result), "element": element,
                               "material_id": material_id, "source_slot": slot})
            slot += 1
    return result


def activation_resource_estimate(domain_count: int, chain_nuclide_count: int, microxs_nuclide_count: int,
                                reaction_count: int, timestep_count: int, domain_chunk: int,
                                file_overhead_bytes: int = 1024 * 1024) -> dict:
    """Conservative bounded storage/working-set estimate for chunked depletion."""
    values = (domain_count, chain_nuclide_count, microxs_nuclide_count, reaction_count,
              timestep_count, domain_chunk, file_overhead_bytes)
    if any(int(value) < 1 for value in values):
        raise ValueError("activation resource-estimate inputs must be positive")
    # OpenMC depletion results store the initial state and each integrated
    # interval. A factor of two covers HDF metadata, uncertainty arrays, and
    # serialization overhead; one bounded chunk is live at a time.
    states = timestep_count + 1
    result_raw = domain_count * chain_nuclide_count * states * 8
    result_upper = 2 * result_raw + math.ceil(domain_count / domain_chunk) * file_overhead_bytes
    one_group_microxs = domain_count * microxs_nuclide_count * reaction_count * 8
    working = 128 * 1024 * 1024 + domain_chunk * (
        microxs_nuclide_count * reaction_count * 8 + chain_nuclide_count * states * 8 * 4
    )
    return {"depletion_result_raw_bytes_lower_bound": result_raw,
            "depletion_results_bytes_upper_bound": result_upper,
            "one_group_microxs_bytes": one_group_microxs,
            "working_bytes_estimate": working,
            "estimate_scope": "one activation domain chunk plus 128 MiB Python/OpenMC overhead; depletion file upper bound is twice dense chain state storage plus 1 MiB per chunk"}


def _write_json_new(path: Path, value: dict) -> None:
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True, allow_nan=False)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def prepare_activation_plan(plan_path: Path, *, collapse_plan_path: Path,
                             collapse_record_path: Path, microxs_path: Path,
                             fluxes_path: Path, volumes_path: Path,
                             materials_xml: Path, chain_path: Path,
                             output_run_dir: Path, timesteps, source_rates,
                             domain_chunk: int = 16, time_budget_s: float = 24 * 3600.0,
                             output_budget_bytes: int = DEFAULT_MAX_OUTPUT_BYTES,
                             disk_reserve_bytes: int = DEFAULT_DISK_RESERVE_BYTES,
                             memory_limit_bytes: int = DEFAULT_MEMORY_LIMIT_BYTES) -> dict:
    """Freeze chunked activation inputs and resource bounds without running depletion."""
    plan_path = Path(plan_path).resolve()
    if plan_path.exists():
        raise FileExistsError(f"refusing to overwrite activation plan: {plan_path}")
    output_run_dir = Path(output_run_dir).resolve()
    if output_run_dir.exists():
        raise FileExistsError(f"refusing to overwrite activation run: {output_run_dir}")
    if domain_chunk < 1 or not math.isfinite(time_budget_s) or time_budget_s <= 0.0:
        raise ValueError("activation chunk and time budget must be positive")
    plan_path.parent.mkdir(parents=True, exist_ok=True)
    output_run_dir.parent.mkdir(parents=True, exist_ok=True)
    collapse_plan_path = Path(collapse_plan_path).resolve(strict=True)
    collapse_plan = json.loads(collapse_plan_path.read_text(encoding="utf-8"))
    _check_plan(collapse_plan)
    collapse_record_path = Path(collapse_record_path).resolve(strict=True)
    collapse_record = json.loads(collapse_record_path.read_text(encoding="utf-8"))
    if collapse_record.get("schema") != SCHEMA or collapse_record.get("status") != "PASS":
        raise ValueError("activation requires a completed bounded-collapse record")
    microxs_path = Path(microxs_path).resolve(strict=True)
    if collapse_record.get("output", {}).get("sha256") != sha256_file(microxs_path):
        raise ValueError("activation MicroXS HDF5 differs from its collapse record")
    if collapse_record.get("plan_sha256") != collapse_plan.get("plan_sha256"):
        raise ValueError("collapse record does not bind the selected collapse plan")
    paths = {"collapse_plan": collapse_plan_path, "collapse_record": collapse_record_path,
             "microxs": microxs_path, "fluxes": Path(fluxes_path).resolve(strict=True),
             "volumes": Path(volumes_path).resolve(strict=True),
             "materials": Path(materials_xml).resolve(strict=True),
             "chain": Path(chain_path).resolve(strict=True)}
    expected = {"fluxes": collapse_plan["inputs"]["fluxes"]["sha256"],
                "volumes": collapse_plan["inputs"]["volumes"]["sha256"],
                "materials": collapse_plan["inputs"]["materials"]["sha256"],
                "chain": collapse_plan["inputs"]["chain"]["sha256"]}
    for name, digest in expected.items():
        if sha256_file(paths[name]) != digest:
            raise ValueError(f"activation input differs from frozen collapse plan: {name}")
    if not isinstance(timesteps, list) or not timesteps or len(source_rates) != len(timesteps):
        raise ValueError("activation timesteps and source-rate schedule must be nonempty and equal length")
    for step in timesteps:
        if not isinstance(step, (list, tuple)) or len(step) != 2 or \
                not math.isfinite(float(step[0])) or float(step[0]) <= 0.0 or \
                step[1] not in {"s", "min", "h", "d", "a"}:
            raise ValueError("each activation timestep must be a positive (value, unit) pair")
    if not all(math.isfinite(float(rate)) and float(rate) >= 0.0 for rate in source_rates):
        raise ValueError("activation source rates must be finite and nonnegative")
    with h5py.File(microxs_path, "r") as micro:
        if micro.attrs.get("schema") != SCHEMA or micro.attrs.get("plan_sha256") != collapse_plan["plan_sha256"]:
            raise ValueError("activation MicroXS archive schema/plan identity mismatch")
        if int(micro["microxs_barns"].shape[1]) != collapse_plan["domains"]["count"]:
            raise ValueError("activation MicroXS domain count differs from collapse plan")
        micro_nuclides = len(micro["nuclides"])
        reactions = len(micro["reactions"])
    chain_nuclides = len(ET.parse(paths["chain"]).getroot().findall("nuclide"))
    domains = int(collapse_plan["domains"]["count"])
    chunk = min(int(domain_chunk), domains)
    estimate = activation_resource_estimate(domains, chain_nuclides, micro_nuclides,
                                            reactions, len(timesteps), chunk)
    if estimate["depletion_results_bytes_upper_bound"] > output_budget_bytes:
        raise ValueError("chunked depletion artifacts exceed the explicit output budget")
    if estimate["working_bytes_estimate"] > memory_limit_bytes:
        raise ValueError("chunked depletion working-set estimate exceeds memory budget")
    free_bytes = shutil.disk_usage(output_run_dir.parent).free
    if free_bytes < estimate["depletion_results_bytes_upper_bound"] + disk_reserve_bytes:
        raise ValueError("activation artifacts plus the required free-space reserve exceed available disk")
    inputs = {name: _input_file(path, name) for name, path in paths.items()}
    result = {
        "schema": "faris-r3-bounded-activation-plan/v1",
        "status": "READY_FOR_CHUNKED_ACTIVATION",
        "algorithm": "openmc-independent-operator-predictor-chunked-v1",
        "output_run_dir": str(output_run_dir),
        "inputs": inputs,
        "collapse_plan_sha256": collapse_plan["plan_sha256"],
        "collapse_record_sha256": inputs["collapse_record"]["sha256"],
        "domain_count": domains,
        "microxs_nuclide_count": micro_nuclides,
        "reaction_count": reactions,
        "chain_nuclide_count": chain_nuclides,
        "domain_chunk": chunk,
        "timesteps": timesteps,
        "source_rates": [float(x) for x in source_rates],
        "time_budget_s": float(time_budget_s),
        "resources": {**estimate, "output_budget_bytes": output_budget_bytes,
                       "disk_reserve_bytes": disk_reserve_bytes, "free_bytes_at_plan": free_bytes,
                       "memory_limit_bytes": memory_limit_bytes},
        "implementation": {"path": str(Path(__file__).resolve()), "sha256": sha256_file(Path(__file__))},
        "limitations": ["Activation runs in bounded independent domain chunks; output is a collection of content-hashed OpenMC depletion result artifacts.",
                        "Photon source assembly must include every domain material and preserve the recorded mesh-material slot mapping.",
                        "The elapsed-time limit is checked between depletion chunks; an in-progress chunk is not interrupted.",
                        "This plan does not assert nuclear-data evaluation quality or VAL043 validation."],
    }
    result["plan_sha256"] = _canonical_sha256(result)
    _write_json_new(plan_path, result)
    return result


def run_activation_chunks(plan_path: Path) -> dict:
    """Run OpenMC's independent depletion operator on bounded domain chunks.

    This stage is solverless (it does not launch neutron or photon transport),
    but it is still a potentially long depletion calculation. It creates a new
    output directory and never resumes/overwrites an existing run.
    """
    plan_path = Path(plan_path).resolve(strict=True)
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    declared = dict(plan)
    plan_sha = declared.pop("plan_sha256", None)
    if plan.get("schema") != "faris-r3-bounded-activation-plan/v1" or \
            plan.get("status") != "READY_FOR_CHUNKED_ACTIVATION" or plan_sha != _canonical_sha256(declared):
        raise ValueError("activation plan schema, status or checksum is invalid")
    if plan["implementation"]["sha256"] != sha256_file(Path(__file__)):
        raise ValueError("activation implementation changed after planning")
    input_paths = {name: Path(item["path"]).resolve(strict=True) for name, item in plan["inputs"].items()}
    for name, path in input_paths.items():
        if sha256_file(path) != plan["inputs"][name]["sha256"]:
            raise ValueError(f"activation input changed after planning: {name}")
    out_dir = Path(plan["output_run_dir"]).resolve()
    if out_dir.exists():
        raise FileExistsError(f"refusing to overwrite activation artifacts: {out_dir}")
    try:
        import openmc
        import openmc.deplete
    except ImportError as exc:
        raise RuntimeError("the pinned OpenMC Python environment is required for chunked depletion") from exc
    if str(openmc.__version__) != "0.15.3":
        raise ValueError(f"chunked activation protocol requires OpenMC 0.15.3, got {openmc.__version__}")
    mats = openmc.Materials.from_xml(input_paths["materials"])
    materials_by_id = {int(mat.id): mat for mat in mats}
    with np.load(input_paths["volumes"], allow_pickle=False) as file_data:
        material_array = np.asarray(file_data["materials"])
        volume_array = np.asarray(file_data["volumes"], dtype=np.float64)
    slot_map = domain_source_slot_map(material_array)
    if len(slot_map) != plan["domain_count"]:
        raise ValueError("activation domain source slots differ from frozen domain count")
    with h5py.File(input_paths["microxs"], "r") as collapsed:
        stored_elements = np.asarray(collapsed["domain_element"][()], dtype=np.int64)
        stored_materials = np.asarray(collapsed["domain_material_id"][()], dtype=np.int64)
        stored_volumes = np.asarray(collapsed["domain_volume_cm3"][()], dtype=np.float64)
    if not np.array_equal(stored_elements, [x["element"] for x in slot_map]) or \
            not np.array_equal(stored_materials, [x["material_id"] for x in slot_map]):
        raise ValueError("activation domain element/material order differs from the collapsed MicroXS archive")
    # The OpenMC volume file is the authoritative activation-domain volume.
    volume_rows = np.asarray(volume_array[material_array > -1], dtype=np.float64)
    if not np.allclose(stored_volumes, volume_rows, rtol=0.0, atol=0.0):
        raise ValueError("activation domain volumes differ from the collapsed MicroXS archive")
    fluxes = np.load(input_paths["fluxes"], mmap_mode="r", allow_pickle=False)
    with h5py.File(input_paths["microxs"], "r") as collapsed:
        nuclides = [v.decode() if isinstance(v, bytes) else str(v) for v in collapsed["nuclides"][()]]
        reactions = [v.decode() if isinstance(v, bytes) else str(v) for v in collapsed["reactions"][()]]
    out_dir.parent.mkdir(parents=True, exist_ok=True)
    out_dir.mkdir()
    chunks = []
    started = __import__("time").monotonic()
    domain_chunk = int(plan["domain_chunk"])
    for chunk_index, start in enumerate(range(0, plan["domain_count"], domain_chunk)):
        if __import__("time").monotonic() - started >= float(plan["time_budget_s"]):
            raise TimeoutError("chunked activation reached the predeclared elapsed-time limit; output is incomplete and cannot be resumed")
        stop = min(start + domain_chunk, plan["domain_count"])
        chunk_dir = out_dir / f"chunk_{chunk_index:05d}"
        chunk_dir.mkdir()
        mats_chunk = []
        domain_rows = slot_map[start:stop]
        for index, row in enumerate(domain_rows, start=start):
            source_material = materials_by_id.get(row["material_id"])
            if source_material is None:
                raise ValueError(f"activation material ID {row['material_id']} is absent from materials.xml")
            mat = source_material.clone()
            mat.name = f"R3 activation domain {index}"
            mat.depletable = True
            mat.volume = stored_volumes[index]
            mats_chunk.append(mat)
        chunk_materials = openmc.Materials(mats_chunk)
        material_xml = chunk_dir / "materials.xml"
        chunk_materials.export_to_xml(material_xml)
        with h5py.File(input_paths["microxs"], "r") as collapsed:
            raw = np.asarray(collapsed["microxs_barns"][:, start:stop, :], dtype=np.float64)
        micro_list = [openmc.deplete.MicroXS(raw[:, local, :, None], nuclides, reactions)
                      for local in range(stop - start)]
        flux_list = [np.asarray(fluxes[i], dtype=np.float64) for i in range(start, stop)]
        operator = openmc.deplete.IndependentOperator(
            chunk_materials, flux_list, micro_list, chain_file=str(input_paths["chain"]),
            normalization_mode="source-rate")
        integrator = openmc.deplete.PredictorIntegrator(
            operator, plan["timesteps"], source_rates=plan["source_rates"])
        result_path = chunk_dir / "depletion_results.h5"
        integrator.integrate(final_step=False, path=result_path)
        receipt = {
            "schema": "faris-r3-bounded-activation-chunk/v1",
            "status": "PASS",
            "chunk_index": chunk_index,
            "domain_start": start,
            "domain_stop": stop,
            "domain_indices": list(range(start, stop)),
            "domain_element_material_slot": domain_rows,
            "activation_material_ids": [int(mat.id) for mat in chunk_materials],
            "materials_xml": {"path": material_xml.name, "sha256": sha256_file(material_xml),
                              "size_bytes": material_xml.stat().st_size},
            "depletion_results": {"path": result_path.name, "sha256": sha256_file(result_path),
                                  "size_bytes": result_path.stat().st_size},
            "plan_sha256": plan["plan_sha256"],
            "microxs_sha256": plan["inputs"]["microxs"]["sha256"],
            "chain_sha256": plan["inputs"]["chain"]["sha256"],
        }
        receipt_path = chunk_dir / "chunk_receipt.json"
        _write_json_new(receipt_path, receipt)
        chunks.append({"receipt_path": str(receipt_path.relative_to(out_dir)),
                       "receipt_sha256": sha256_file(receipt_path),
                       "depletion_results_sha256": receipt["depletion_results"]["sha256"],
                       "domain_start": start, "domain_stop": stop})
    record = {
        "schema": "faris-r3-bounded-activation/v1",
        "status": "PASS",
        "plan_sha256": plan["plan_sha256"],
        "activation_plan_path": str(plan_path),
        "output_dir": str(out_dir),
        "openmc_version": str(openmc.__version__),
        "openmc_python_path": str(Path(openmc.__file__).resolve()),
        "domain_count": plan["domain_count"],
        "domain_chunk": domain_chunk,
        "chunk_count": len(chunks),
        "timesteps": plan["timesteps"],
        "source_rates": plan["source_rates"],
        "chunks": chunks,
        "photon_source_mapping": "every material-bearing domain is recorded in row-major MeshMaterialFilter order with its source-slot index; source assembly must include all chunks and preserve these entries",
        "uncertainty_scope": "No neutron or photon transport uncertainty is computed here; this is the deterministic depletion stage conditional on the recorded flux and collapsed MicroXS.",
    }
    for name, path in input_paths.items():
        if sha256_file(path) != plan["inputs"][name]["sha256"]:
            raise ValueError(f"activation input changed during chunked depletion: {name}")
    _write_json_new(out_dir / "activation_record.json", record)
    return record


def build_chunked_decay_photon_sources(activation_plan_path: Path, activation_record_path: Path,
                                      activation_mesh, time_index: int, *,
                                      openmc_module=None, results_loader=None) -> tuple[list, dict]:
    """Reassemble all recorded activation chunks into R2S MeshSource objects.

    Every material-bearing voxel domain is visited exactly once. The output
    source-list slot for a voxel follows OpenMC's MeshMaterialVolumes
    ``by_element`` ordering (padding ``-2`` omitted, void ``-1`` retained).
    The caller should export the returned source list in its photon model XML
    and record that XML hash beside this summary.
    """
    activation_plan_path = Path(activation_plan_path).resolve(strict=True)
    activation_record_path = Path(activation_record_path).resolve(strict=True)
    plan = json.loads(activation_plan_path.read_text(encoding="utf-8"))
    record = json.loads(activation_record_path.read_text(encoding="utf-8"))
    if plan.get("schema") != "faris-r3-bounded-activation-plan/v1" or \
            record.get("schema") != "faris-r3-bounded-activation/v1" or \
            record.get("status") != "PASS" or record.get("plan_sha256") != plan.get("plan_sha256"):
        raise ValueError("photon source assembly requires a completed activation record bound to its plan")
    collapse_record_ref = plan["inputs"]["collapse_record"]
    if sha256_file(Path(collapse_record_ref["path"])) != collapse_record_ref["sha256"]:
        raise ValueError("activation collapse record changed after the activation plan")
    if not 0 <= int(time_index) <= len(plan["timesteps"]):
        raise ValueError("photon source cooling index is outside depletion-results states")
    for name, ref in plan["inputs"].items():
        if sha256_file(Path(ref["path"])) != ref["sha256"]:
            raise ValueError(f"activation source input changed after planning: {name}")
    try:
        if openmc_module is None:
            import openmc as openmc_module
        if results_loader is None:
            import openmc.deplete
            results_loader = openmc.deplete.Results
    except ImportError as exc:
        raise RuntimeError("OpenMC Python is required to assemble recorded decay-photon sources") from exc
    if str(openmc_module.__version__) != "0.15.3":
        raise ValueError("chunked photon source assembly requires OpenMC 0.15.3")
    materials = openmc_module.Materials.from_xml(Path(plan["inputs"]["materials"]["path"]))
    materials_by_id = {int(mat.id): mat for mat in materials}
    with np.load(Path(plan["inputs"]["volumes"]["path"]), allow_pickle=False) as values:
        material_array = np.asarray(values["materials"])
    slot_map = domain_source_slot_map(material_array)
    domain_count = int(plan["domain_count"])
    if len(slot_map) != domain_count or record.get("domain_count") != domain_count:
        raise ValueError("activation source records do not cover the planned mesh-material domains")
    n_elements = material_array.shape[0]
    n_slots = max((int(np.count_nonzero(row != -2)) for row in material_array), default=0)
    if n_slots < 1:
        raise ValueError("mesh-material volume artifact contains no source slots")
    null_source = openmc_module.IndependentSource(particle="photon", strength=0.0)
    source_lists = [[null_source for _ in range(n_elements)] for _ in range(n_slots)]
    expected_domain = 0
    emitting_domains = 0
    total_photons_per_second = 0.0
    used_result_hashes = []
    out_dir = Path(record["output_dir"]).resolve(strict=True)
    for chunk_ref in record["chunks"]:
        receipt_path = (out_dir / chunk_ref["receipt_path"]).resolve(strict=True)
        if sha256_file(receipt_path) != chunk_ref["receipt_sha256"]:
            raise ValueError("activation chunk receipt changed before photon source assembly")
        chunk = json.loads(receipt_path.read_text(encoding="utf-8"))
        if chunk.get("schema") != "faris-r3-bounded-activation-chunk/v1" or \
                chunk.get("plan_sha256") != plan["plan_sha256"] or chunk.get("status") != "PASS":
            raise ValueError("malformed or unbound activation chunk receipt")
        result_path = (receipt_path.parent / chunk["depletion_results"]["path"]).resolve(strict=True)
        if sha256_file(result_path) != chunk["depletion_results"]["sha256"]:
            raise ValueError("activation depletion result changed before photon source assembly")
        if chunk["domain_start"] != expected_domain:
            raise ValueError("activation chunk domain ranges are not contiguous and ordered")
        result = results_loader(result_path)
        if int(time_index) >= len(result):
            raise ValueError("cooling index is absent from a chunk depletion result")
        step = result[int(time_index)]
        domain_indices = chunk["domain_indices"]
        activation_ids = chunk["activation_material_ids"]
        if len(domain_indices) != len(activation_ids) or domain_indices != list(
                range(chunk["domain_start"], chunk["domain_stop"])):
            raise ValueError("activation material ID mapping is incomplete or misordered")
        if chunk.get("domain_element_material_slot") != slot_map[chunk["domain_start"]:chunk["domain_stop"]]:
            raise ValueError("activation chunk domain/material/source-slot mapping changed")
        for domain_index, activation_id in zip(domain_indices, activation_ids):
            row = slot_map[domain_index]
            original = materials_by_id.get(row["material_id"])
            if original is None:
                raise ValueError(f"source material {row['material_id']} is absent from bound materials.xml")
            activated = step.get_material(str(activation_id))
            energy = activated.get_decay_photon_energy()
            if energy is not None:
                strength = float(energy.integral())
                if not math.isfinite(strength) or strength < 0.0:
                    raise ValueError("activation source has invalid photons/s strength")
                source_lists[row["source_slot"]][row["element"]] = openmc_module.IndependentSource(
                    energy=energy, particle="photon", strength=strength,
                    constraints={"domains": [original]})
                emitting_domains += 1
                total_photons_per_second += strength
        expected_domain = int(chunk["domain_stop"])
        used_result_hashes.append({"path": str(result_path), "sha256": chunk["depletion_results"]["sha256"]})
        del result
    if expected_domain != domain_count:
        raise ValueError("photon source assembly did not visit every activation domain")
    if emitting_domains == 0:
        raise ValueError("no activation domain produced a decay-photon source")
    sources = [openmc_module.MeshSource(activation_mesh, slot) for slot in source_lists]
    summary = {
        "schema": "faris-r3-bounded-photon-source-summary/v1",
        "status": "PASS_SOURCE_ASSEMBLED",
        "activation_plan_sha256": plan["plan_sha256"],
        "activation_record_sha256": sha256_file(activation_record_path),
        "time_index": int(time_index),
        "domain_count": domain_count,
        "source_slot_count": n_slots,
        "emitting_domain_count": emitting_domains,
        "zero_or_no_photon_domain_count": domain_count - emitting_domains,
        "total_photons_per_second": total_photons_per_second,
        "depletion_result_files": used_result_hashes,
        "source_domain_order": "row-major MeshMaterialVolumes.materials > -1; source slots retain every non-padding -2 slot in each voxel row",
        "source_normalization": "each source strength is Material.get_decay_photon_energy().integral() in photons/s; OpenMC MeshSource sampling preserves per-bin strengths",
        "source_model_xml_sha256": None,
        "note": "Record the photon model XML hash after assigning these sources; source assembly contains all activation domains and does not validate the scientific completeness of the decay data.",
    }
    return sources, summary

