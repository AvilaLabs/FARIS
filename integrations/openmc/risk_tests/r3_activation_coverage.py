# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Reachable R3 depletion-chain cross-section coverage checks.

This is a preflight, not a nuclear-data qualification. It verifies that every
reaction-bearing nuclide reachable from the actual activation materials has a
neutron library entry and every reaction named by the chain has a corresponding
OpenMC HDF5 MT group. Evaluation quality, covariance, branching quality, decay
data adequacy, and activation-model accuracy remain separate qualifications.
"""
from __future__ import annotations

import hashlib
from pathlib import Path
import xml.etree.ElementTree as ET

SCHEMA = "faris-r3-activation-chain-coverage/v1"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def _chain_graph(chain_xml: Path) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    root = ET.parse(chain_xml).getroot()
    nuclides: dict[str, set[str]] = {}
    adjacency: dict[str, set[str]] = {}
    for entry in root.findall("nuclide"):
        name = entry.get("name")
        if not name or name in nuclides:
            raise ValueError("depletion chain has a missing or duplicate nuclide name")
        reactions = set()
        targets = set()
        for reaction in entry.findall("reaction"):
            reaction_type = reaction.get("type")
            if not reaction_type:
                raise ValueError(f"chain reaction for {name} has no type")
            reactions.add(reaction_type)
            target = reaction.get("target")
            if target:
                targets.add(target)
        for decay in entry.findall("decay"):
            target = decay.get("target")
            if target:
                targets.add(target)
        nuclides[name] = reactions
        adjacency[name] = targets
    if not nuclides:
        raise ValueError("depletion chain has no nuclides")
    for parent, targets in adjacency.items():
        unknown = targets - set(nuclides)
        if unknown:
            raise ValueError(f"chain products from {parent} are absent from chain: {sorted(unknown)[:10]}")
    return nuclides, adjacency


def _cross_section_index(xml_path: Path) -> dict[str, Path]:
    root = ET.parse(xml_path).getroot()
    directory_text = (root.findtext("directory") or "").strip()
    directory = Path(directory_text) if directory_text else xml_path.parent
    if not directory.is_absolute():
        directory = xml_path.parent / directory
    index: dict[str, Path] = {}
    for entry in root.findall("library"):
        if entry.get("type") != "neutron":
            continue
        rel = entry.get("path")
        if not rel:
            raise ValueError("neutron library index entry has no path")
        path = Path(rel)
        if not path.is_absolute():
            path = directory / path
        path = path.resolve()
        names = (entry.get("materials") or entry.get("name") or "").split()
        for name in names:
            previous = index.get(name)
            if previous is not None and previous != path:
                raise ValueError(f"cross_sections.xml ambiguously indexes {name}")
            index[name] = path
    return index


def _mt_for(reaction_type: str, reaction_mt: dict[str, int]) -> int | None:
    value = reaction_mt.get(reaction_type)
    return int(value) if value is not None else None


def assess_coverage(chain_xml: Path, cross_sections_xml: Path, material_roots,
                    reaction_mt: dict[str, int] | None = None) -> dict:
    """Return complete reachable-nuclide and chain-reaction-channel presence.

    ``reaction_mt`` is OpenMC's ``openmc.data.REACTION_MT`` mapping in the
    pinned producer. Tests may pass the same small mapping for synthetic XML.
    No solver is started. Only HDF5 group metadata is read for indexed nuclides.
    """
    import h5py

    chain_xml, cross_sections_xml = Path(chain_xml).resolve(), Path(cross_sections_xml).resolve()
    chain_digest_before = sha256_file(chain_xml)
    cross_sections_digest_before = sha256_file(cross_sections_xml)
    if reaction_mt is None:
        from openmc.data import REACTION_MT
        reaction_mt = REACTION_MT
    roots = sorted({str(name) for name in material_roots})
    if not roots:
        raise ValueError("R3 activation coverage requires actual material root nuclides")
    nuclides, adjacency = _chain_graph(chain_xml)
    missing_roots = sorted(set(roots) - set(nuclides))
    if missing_roots:
        raise ValueError("actual activation material nuclides are absent from the bound chain: " +
                         ", ".join(missing_roots[:30]))

    reachable = set(roots)
    pending = list(roots)
    while pending:
        parent = pending.pop()
        for target in adjacency[parent] - reachable:
            reachable.add(target)
            pending.append(target)
    required = sorted(set(roots) | {name for name in reachable if nuclides[name]})
    index = _cross_section_index(cross_sections_xml)
    missing_nuclides = sorted(set(required) - set(index))
    indexed_data_files = []
    for name in required:
        data_path = index.get(name)
        if data_path is not None and data_path.is_file():
            indexed_data_files.append({"nuclide": name, "path": str(data_path),
                                       "sha256": sha256_file(data_path)})

    missing_channels = []
    checked_channels = []
    unmapped_channels = []
    for name in sorted(reachable):
        for reaction_type in sorted(nuclides[name]):
            mt = _mt_for(reaction_type, reaction_mt)
            if mt is None:
                unmapped_channels.append({"nuclide": name, "reaction": reaction_type})
                continue
            data_path = index.get(name)
            if data_path is None or not data_path.is_file():
                continue
            tag = f"reaction_{mt:03d}"
            try:
                with h5py.File(data_path, "r") as handle:
                    group = handle.get(f"{name}/reactions/{tag}")
                    if group is None:
                        missing_channels.append({"nuclide": name, "reaction": reaction_type, "mt": mt,
                                                 "reason": "OpenMC HDF5 reaction group missing"})
                        continue
                    stored_mt = group.attrs.get("mt")
                    if stored_mt is not None and int(stored_mt) != mt:
                        missing_channels.append({"nuclide": name, "reaction": reaction_type, "mt": mt,
                                                 "reason": f"HDF5 group declares MT {int(stored_mt)}"})
                        continue
                    checked_channels.append({"nuclide": name, "reaction": reaction_type, "mt": mt})
            except (OSError, KeyError, ValueError) as exc:
                missing_channels.append({"nuclide": name, "reaction": reaction_type, "mt": mt,
                                         "reason": f"cannot inspect indexed HDF5 file: {type(exc).__name__}"})

    absent_files = sorted(name for name in required if name in index and not index[name].is_file())
    mutated_data_files = [item["nuclide"] for item in indexed_data_files
                          if sha256_file(Path(item["path"])) != item["sha256"]]
    if sha256_file(chain_xml) != chain_digest_before or sha256_file(cross_sections_xml) != cross_sections_digest_before:
        raise ValueError("depletion chain or cross_sections.xml changed during activation coverage inspection")
    ready = not (missing_roots or missing_nuclides or absent_files or missing_channels or unmapped_channels or mutated_data_files)
    return {
        "schema": SCHEMA,
        "status": "READY_FOR_R2S_TRANSPORT" if ready else "NOT_EVALUATED",
        "chain_xml_path": str(chain_xml), "chain_xml_sha256": chain_digest_before,
        "cross_sections_xml_path": str(cross_sections_xml),
        "cross_sections_xml_sha256": cross_sections_digest_before,
        "material_root_nuclides": roots,
        "reachable_chain_nuclide_count": len(reachable),
        "reachable_chain_reaction_nuclide_count": sum(bool(nuclides[name]) for name in reachable),
        "required_microxs_nuclides": required,
        "indexed_data_files": indexed_data_files,
        "missing_reachable_nuclides": missing_nuclides,
        "missing_library_files": absent_files,
        "changed_data_files_during_inspection": mutated_data_files,
        "checked_reaction_channel_count": len(checked_channels),
        "checked_reaction_channels": checked_channels,
        "missing_reaction_channels": missing_channels,
        "unmapped_chain_reactions": unmapped_channels,
        "qualification_limit": (
            "READY only means every chain-reachable reaction nuclide is indexed and each chain-listed MT group exists. "
            "It does not qualify evaluation quality, cross-section channel completeness beyond listed chain reactions, "
            "decay data, branching adequacy, covariance, spectrum collapse, composition, or activation-model accuracy. "
            "HDF5 input hashes bind the indexed files used by the preflight; they do not certify their provenance "
            "or scientific quality."
        ),
    }


def actual_material_nuclides(materials_xml: Path) -> list[str]:
    """Read nuclide/isotope names from the exact serialized OpenMC materials XML."""
    root = ET.parse(materials_xml).getroot()
    names = set()
    for element in root.iter():
        if element.tag in {"nuclide", "isotope"}:
            name = element.get("name")
            if name:
                names.add(name)
    if not names:
        raise ValueError("serialized R2S materials contain no nuclide/isotope entries")
    return sorted(names)


def inspect_microxs(path: Path, required_nuclides) -> dict:
    """Inspect actual OpenMC MicroXS HDF5 nuclide datasets, across every domain."""
    import numpy as np
    import h5py

    path = Path(path)
    if not path.is_file():
        raise ValueError(f"R2S MicroXS artifact is missing: {path}")
    required = set(required_nuclides)
    domain_count = 0
    common_nuclides = None
    with h5py.File(path, "r") as handle:
        for domain_name, group in handle.items():
            if not isinstance(group, h5py.Group) or not {"nuclides", "reactions", "data"}.issubset(group):
                raise ValueError("R2S MicroXS HDF5 contains a malformed domain")
            observed = [value.decode("utf-8") if isinstance(value, bytes) else str(value)
                        for value in group["nuclides"][()]]
            reactions = [value.decode("utf-8") if isinstance(value, bytes) else str(value)
                         for value in group["reactions"][()]]
            shape = group["data"].shape
            if len(set(observed)) != len(observed) or shape[:2] != (len(observed), len(reactions)) or \
                    len(shape) != 3 or not np.issubdtype(group["data"].dtype, np.number):
                raise ValueError(f"R2S MicroXS dimensions or values are invalid for {domain_name}")
            missing = sorted(required - set(observed))
            if missing:
                raise ValueError(f"R2S MicroXS domain {domain_name} lacks required nuclides: " +
                                 ", ".join(missing[:30]))
            if set(observed) != required:
                raise ValueError(f"R2S MicroXS domain {domain_name} includes nuclides outside the declared request")
            if common_nuclides is None:
                common_nuclides = observed
            elif observed != common_nuclides:
                raise ValueError(f"R2S MicroXS domain {domain_name} has a different nuclide ordering")
            domain_count += 1
    if not domain_count:
        raise ValueError("R2S MicroXS HDF5 has no domains")
    return {"domain_count": domain_count, "observed_nuclides": common_nuclides,
            "required_nuclides": sorted(required)}
