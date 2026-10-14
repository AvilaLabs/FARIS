"""Shared helpers for the tests that need OpenMC, numpy or a small nuclear-data set.

Locally such a test skips, with the reason, when what it needs is absent. The CI job `openmc-tests`
sets FARIS_REQUIRE_OPENMC=1, and then the same absence fails the test, so a broken environment
cannot pass as a skipped one.

The transport tests read their nuclear data from FARIS_CI_NEUTRON_DATA, a folder holding the
neutron HDF5 files of H1 and Fe56 (see `.github/workflows/ci.yml` for where CI gets them and
`DATA_SHA256` for the bytes expected). `cross_sections_xml` writes the small library file that
points at them.
"""

import hashlib
import os
from pathlib import Path
import unittest

REQUIRE_ENV = "FARIS_REQUIRE_OPENMC"
DATA_ENV = "FARIS_CI_NEUTRON_DATA"
# FENDL-3.2 neutron files as shipped by the OpenMC data page (the same bytes the local audited library holds).
DATA_SHA256 = {
    "H1.h5": "2bf2db322a188e5c435675cd732dd36b10b995e8cc6e7d6bd55d6aaf712f9b4d",
    "Fe56.h5": "2c7eedbb5474ffe9cd1576d378d6fa61c5a1d3b8fb1e52bb88c2c255f85b2bf1",
}


def required() -> bool:
    return os.environ.get(REQUIRE_ENV) == "1"


def skip_or_fail(reason: str):
    """Skip the test with `reason`, or fail it when FARIS_REQUIRE_OPENMC=1."""
    if required():
        raise AssertionError(f"{REQUIRE_ENV}=1 but the test cannot run: {reason}")
    raise unittest.SkipTest(reason)


def require_modules(*names: str) -> None:
    """Import each module or skip (fail when required)."""
    import importlib

    for name in names:
        try:
            importlib.import_module(name)
        except Exception as error:
            skip_or_fail(f"{name} is needed ({error})")


def neutron_data_dir() -> Path:
    """The folder with H1.h5 and Fe56.h5, or a skip (fail when required) saying how to provide it.
    The files are checked against DATA_SHA256 so a wrong download cannot give a quiet wrong answer."""
    value = os.environ.get(DATA_ENV)
    if not value:
        skip_or_fail(f"set {DATA_ENV} to a folder holding H1.h5 and Fe56.h5 (FENDL-3.2 neutron data)")
    folder = Path(value)
    for name, expected in DATA_SHA256.items():
        path = folder / name
        if not path.is_file():
            skip_or_fail(f"{path} is missing")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != expected:
            raise AssertionError(f"{path} has sha256 {digest}, expected {expected}: not the FENDL-3.2 file")
    return folder


def cross_sections_xml(folder: Path, into: Path) -> Path:
    """Write INTO/cross_sections.xml listing the H1 and Fe56 files of FOLDER and return its path."""
    lines = ['<?xml version="1.0"?>', "<cross_sections>"]
    for name in DATA_SHA256:
        lines.append(f'  <library materials="{name[:-3]}" path="{(folder / name).resolve()}" type="neutron"/>')
    lines.append("</cross_sections>")
    path = into / "cross_sections.xml"
    path.write_text("\n".join(lines) + "\n")
    return path
