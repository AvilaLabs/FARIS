# SPDX-License-Identifier: AGPL-3.0-only
"""Small valid manifest and run-record builders shared by the tests."""
from __future__ import annotations

import copy

from .scoring import Identity, seal_run_record

IDENT = Identity("a" * 64, "openmc-0.15.3", "b" * 64)


def manifest(n: int = 6, values=None, norm_status: str = "confirmed", covariance=None) -> dict:
    values = values or [(1.0 + 0.1 * i, 0.05) for i in range(n)]
    return {
        "schema": "faris.validation-case/1.0.0", "case_id": "fixture", "title": "fixture", "evidence_class": "experiment",
        "source": {"url": "https://example.org/benchmarks", "commit": "c" * 40, "retrieved": "2026-10-09"},
        "license": {"spdx": "CC-BY-4.0", "attribution": "Example attribution"},
        "files": [{"role": "input", "path_in_source": "in/a.xml", "sha256": "d" * 64}],
        "files_are_external_not_vendored": True,
        "normalisation": {"status": norm_status, "quantity": "q", "units": "u", "source_normalisation": "per source neutron", "location": "surface",
                          "reaction_or_particle": "neutron", "energy_integration": "bins", "basis": "stated", "blocks_scoring": True},
        "uncertainty": {"components": [{"name": "as provided", "kind": "unspecified"}], "note": "note"},
        "compatibility": {"declared": "2026-10-09", "k": 2.0, "k_basis": "authored",
                          "covariance": covariance or {"kind": "independent", "basis": "authored"}},
        "response_classes": {"flux": {"unit": "u", "description": "d"}, "dose": {"unit": "u", "description": "d"}},
        "detectors": [{"id": f"d{i}", "response_class": "flux", "reference": {"value": v, "u": u}} for i, (v, u) in enumerate(values)],
        "not_covered": ["ports"],
        "qualified_range": {"materials": ["Al"], "geometry_class": "sphere", "spectrum_class": "D-T", "parameters": [], "cooling_time": "not applicable"},
    }


def run_record(manifest_: dict, calc: list[float], u_mc: float = 0.001, identity: Identity = IDENT, case_id: str | None = None) -> dict:
    return seal_run_record({
        "kind": "faris-run", "case_id": case_id or manifest_["case_id"], "run_id": "run-1", "identity": identity.as_dict(),
        "results": [{"detector_id": d["id"], "value": c, "u_mc": u_mc} for d, c in zip(manifest_["detectors"], calc)],
    })


def clone(x):
    return copy.deepcopy(x)
