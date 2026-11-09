#!/usr/bin/env python3
"""Extract selected PROCESS values with PROCESS's own documented MFILE reader."""
from __future__ import annotations

import importlib.metadata
import json
from pathlib import Path
import sys


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: process_mfile_metrics.py EXPECTED_VERSION MFILE.DAT")
    expected, filename = sys.argv[1], Path(sys.argv[2])
    actual = importlib.metadata.version("process")
    if actual != expected:
        raise RuntimeError(f"PROCESS version mismatch: expected {expected}, got {actual}")
    from process.core.io.mfile.base import MFile

    mfile = MFile(filename=filename)
    mfile.open_mfile()
    mfile.parse_mfile()
    availability, capacity_factor, plant_lifetime = map(
        float, mfile.get_variables("f_t_plant_available", "cpfact", "life_plant")
    )
    print(json.dumps({"version": actual, "plant_availability": availability,
                      "capacity_factor": capacity_factor, "plant_lifetime_years": plant_lifetime}, allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
