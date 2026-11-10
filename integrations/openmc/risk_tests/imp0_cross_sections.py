#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Bounded, non-physics preflight for explicit OpenMC cross-section indexes."""

from __future__ import annotations

import hashlib
from pathlib import Path
import stat
import xml.etree.ElementTree as ET

MAX_INDEX_BYTES = 4 * 1024 * 1024
MAX_LIBRARY_ENTRIES = 2048
MAX_MATERIAL_NAMES = 100_000
MAX_PATH_LENGTH = 4096
ALLOWED_TYPES = {"neutron", "photon", "thermal"}


def validate_cross_sections_index(path: Path) -> dict[str, object]:
    """Check a bounded XML catalogue and that every referenced file resolves.

    This only verifies index structure and referenced file existence. It does not
    authenticate nuclear data or qualify the catalogue's physical suitability.
    """
    selected = path.expanduser().resolve(strict=True)
    if selected.name != "cross_sections.xml":
        raise ValueError("cross-sections index must be named cross_sections.xml")
    if not stat.S_ISREG(selected.stat().st_mode):
        raise ValueError("cross-sections index must be a regular file")
    size = selected.stat().st_size
    if size <= 0 or size > MAX_INDEX_BYTES:
        raise ValueError(f"cross-sections XML size {size} is outside the allowed bound")
    data = selected.read_bytes()
    try:
        root = ET.fromstring(data)
    except ET.ParseError as error:
        raise ValueError(f"invalid cross-sections XML: {error}") from error
    if root.tag != "cross_sections":
        raise ValueError("cross-sections XML has an unexpected root element")

    directory_text = root.findtext("directory")
    if directory_text and directory_text.strip():
        library_directory = Path(directory_text.strip()).expanduser()
        if not library_directory.is_absolute():
            library_directory = selected.parent / library_directory
        try:
            library_directory = library_directory.resolve(strict=True)
        except FileNotFoundError as error:
            raise ValueError("cross-sections directory entry does not exist") from error
        if not library_directory.is_dir():
            raise ValueError("cross-sections directory entry does not resolve to a directory")
    else:
        library_directory = selected.parent

    libraries = root.findall("library")
    if not libraries or len(libraries) > MAX_LIBRARY_ENTRIES:
        raise ValueError("cross-sections XML has no libraries or exceeds the entry bound")
    catalogues: dict[str, set[str]] = {kind: set() for kind in ALLOWED_TYPES}
    resolved_paths: set[str] = set()
    material_count = 0
    for item in libraries:
        kind = item.get("type", "")
        if kind not in ALLOWED_TYPES:
            raise ValueError(f"unsupported cross-section library type: {kind!r}")
        names = item.get("materials", "").split()
        if not names:
            raise ValueError(f"{kind} library entry has no material catalogue")
        material_count += len(names)
        if material_count > MAX_MATERIAL_NAMES:
            raise ValueError("cross-sections XML exceeds the material-catalogue bound")
        catalogues[kind].update(names)

        raw_path = item.get("path", "")
        if not raw_path or len(raw_path) > MAX_PATH_LENGTH:
            raise ValueError("cross-section library entry has a missing or oversized path")
        library_path = Path(raw_path).expanduser()
        if not library_path.is_absolute():
            library_path = library_directory / library_path
        try:
            library_path = library_path.resolve(strict=True)
        except FileNotFoundError as error:
            raise ValueError(f"cross-section library file does not exist: {raw_path}") from error
        if not stat.S_ISREG(library_path.stat().st_mode):
            raise ValueError(f"cross-section library path is not a regular file: {raw_path}")
        resolved_paths.add(str(library_path))

    if not catalogues["neutron"]:
        raise ValueError("cross-sections XML has no neutron catalogue")
    if not catalogues["photon"]:
        raise ValueError("cross-sections XML has no photon catalogue")

    return {
        "path": str(selected),
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": size,
        "library_entry_count": len(libraries),
        "catalogue_counts_by_type": {
            kind: len(names) for kind, names in sorted(catalogues.items()) if names
        },
        "resolved_library_paths": sorted(resolved_paths),
    }
