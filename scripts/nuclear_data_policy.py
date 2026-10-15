# SPDX-License-Identifier: AGPL-3.0-only
"""The nuclear data files allowed in Git and the rule they must meet (QA-090, NUC-069).

A data file is allowed when it is listed in nuclear_data_in_git.json, is at most `max_file_bytes`,
matches its recorded SHA-256 and names its provider, source, redistribution terms, the evidence for
those terms and the credit owed. An index file (an OpenMC `cross_sections.xml`) needs only its entry.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

REGISTRY = Path(__file__).resolve().parent / "nuclear_data_in_git.json"
DATA_FIELDS = ("sha256", "content", "provider", "source", "terms", "terms_evidence", "attribution")


def load(registry: Path = REGISTRY) -> dict:
    return json.loads(registry.read_text(encoding="utf-8"))


def allowed(registry: Path = REGISTRY) -> dict[str, str]:
    """{path: one-line reason} for every listed file."""
    return {entry["path"]: entry["content"] for entry in load(registry)["files"]}


def problems(root: Path, registry: Path = REGISTRY) -> list[str]:
    """One sentence per listed file that breaks the rule."""
    document = load(registry)
    limit = document["max_file_bytes"]
    found = []
    for entry in document["files"]:
        path = root / entry["path"]
        if not path.is_file():
            found.append(f"{entry['path']} is listed in {registry.name} but missing; remove its entry")
            continue
        if entry.get("kind") != "data":
            continue
        missing = [field for field in DATA_FIELDS if not str(entry.get(field, "")).strip()]
        if missing:
            found.append(f"{entry['path']} does not name its {', '.join(missing)}; record them in {registry.name}")
        size = path.stat().st_size
        if size > limit:
            found.append(f"{entry['path']} is {size} bytes, over the {limit} byte limit for nuclear data in Git; "
                         f"fetch it by hash instead")
        if entry.get("sha256") and hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            found.append(f"{entry['path']} differs from its recorded SHA-256; record the new file's provenance")
    return found
