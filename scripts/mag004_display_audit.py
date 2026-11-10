#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Fail if a registered MAG-004 margin display loses its renderer or fixture."""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "docs/requirements/mag004-margin-displays.json"


def has_test(source: str, name: str) -> bool:
    pattern = r"#\[test\](?:\s|#\[[^]]+\])*fn\s+" + re.escape(name) + r"\s*\("
    return re.search(pattern, source) is not None


def main() -> int:
    registry = json.loads(REGISTRY.read_text())
    surfaces = registry.get("surfaces", [])
    errors: list[str] = []
    ids = [row.get("id") for row in surfaces]
    if not surfaces:
        errors.append("registry has no surfaces")
    if len(ids) != len(set(ids)):
        errors.append("registry has duplicate surface IDs")

    for row in surfaces:
        surface = row.get("id", "<missing id>")
        source_path = ROOT / row.get("source", "")
        if not source_path.is_file():
            errors.append(f"{surface}: missing production source {row.get('source')}")
            continue
        source = source_path.read_text()
        anchor = row.get("anchor", "")
        if not anchor or anchor not in source:
            errors.append(f"{surface}: production callback anchor missing: {anchor!r}")
        symbols = row.get("renderer_symbol", [])
        if not symbols:
            errors.append(f"{surface}: no production renderer callback is registered")
        renderer_path = ROOT / row.get("renderer_source", row.get("source", ""))
        renderer_source = renderer_path.read_text() if renderer_path.is_file() else ""
        for symbol in symbols:
            if symbol not in renderer_source:
                errors.append(f"{surface}: renderer callback missing: {symbol} in {row.get('renderer_source', row.get('source'))}")
        fixture_path = ROOT / row.get("fixture_source", row.get("source", ""))
        if not fixture_path.is_file():
            errors.append(f"{surface}: missing fixture source {row.get('fixture_source')}")
            continue
        fixture_source = fixture_path.read_text()
        fixture_names = [name.strip() for name in row.get("rendered_or_structural_fixture", "").split(";")]
        if not fixture_names or any(not name for name in fixture_names):
            errors.append(f"{surface}: no named fixture is registered")
        for name in fixture_names:
            if not has_test(fixture_source, name):
                errors.append(f"{surface}: fixture/test missing: {name}")
        if not row.get("quantity"):
            errors.append(f"{surface}: affected quantity is missing")
        if not row.get("interval_and_edge_expectation"):
            errors.append(f"{surface}: interval/unfavourable-edge expectation is missing")

    if errors:
        for error in errors:
            print(f"MAG-004 AUDIT FAIL: {error}", file=sys.stderr)
        print(f"MAG-004 registry check failed ({len(errors)} issue(s), {len(surfaces)} surfaces)", file=sys.stderr)
        return 1
    print(f"MAG-004 registry check: {len(surfaces)}/{len(surfaces)} display callbacks, quantity contracts and fixture functions are present")
    print("Named Rust fixtures must still be executed; this check is inventory freshness, not physical validation.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
