# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Small helpers for stable generated validation artifacts."""
from __future__ import annotations

from pathlib import Path


def write_bytes_if_changed(path: Path, data: bytes) -> bool:
    """Write bytes only when content differs; return whether a write occurred."""
    if path.is_file() and path.read_bytes() == data:
        return False
    path.write_bytes(data)
    return True
