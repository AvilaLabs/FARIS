#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes crates/faris-engine/tests/fixtures/actinv_result_synthetic.json (a small synthetic result in the shape of an
ACTINV `run` result, with extra nested members, unicode, awkward numbers, and steps with and without flux and
photon_source) and actinv_scan_expected.json, the output of `scan_result` in scripts/maintenance_coupling_test.py for
it. The Rust reader in faris_engine::maintenance::actinv must reproduce the kept values exactly.

Nothing here is a real ACTINV result. Run from the repository root:
python3 scripts/make_actinv_points_fixture.py
"""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "crates" / "faris-engine" / "tests" / "fixtures"


def load_reference():
    spec = importlib.util.spec_from_file_location("maintenance_coupling_test", HERE / "maintenance_coupling_test.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def steps() -> list:
    nuclides = {"Fe-55": {"atoms": 1.5e22, "activity_Bq": 4987.0}, "Mn-54": {"atoms": 2.5e-3, "activity_Bq": [1, 2, 3]}}
    out = [
        # a plain step
        {"t_s": 1.0, "heat_W_per_g": {"total": 0.125, "by_nuclide": nuclides}, "flux": 2.5e13,
         "label": "irradiation µ-Ω \U0001f600", "nuclides": nuclides},
        # integer flux and heat
        {"t_s": 3600, "heat_W_per_g": {"total": 5, "gamma": 3}, "flux": 7},
        # flux missing, photon_source with a dose
        {"t_s": 7200.5, "heat_W_per_g": {"total": 4987.0},
         "photon_source": {"contact_gamma_air_dose_proxy_Gy_h": 0.0625, "groups": [1, 2, 3]}},
        # photon_source present without the dose key
        {"t_s": 1.0e4, "heat_W_per_g": {"total": 1e-300}, "flux": 0.0, "photon_source": {"other": 1}},
        # photon_source null and empty
        {"t_s": 2.0e4, "heat_W_per_g": {"total": 2.2250738585072014e-308}, "flux": 0, "photon_source": None},
        {"t_s": 3.0e4, "heat_W_per_g": {"total": 5e-324}, "flux": 0.0, "photon_source": {}},
        # extremes and awkward decimals
        {"t_s": 4.0e4, "heat_W_per_g": {"total": 1.7976931348623157e308}, "flux": -0.0,
         "photon_source": {"contact_gamma_air_dose_proxy_Gy_h": 123456789.12345678}},
        {"t_s": 5.0e4, "heat_W_per_g": {"total": 0.1}, "flux": 1e-5},
        {"t_s": 6.0e4, "heat_W_per_g": {"total": 0.30000000000000004}, "flux": 0.0,
         "photon_source": {"contact_gamma_air_dose_proxy_Gy_h": 0.0}},
        # cooling part: zero flux
        {"t_s": 31536000.0, "heat_W_per_g": {"total": 3.14159e-7}, "flux": 0.0,
         "text": "a \"steps\": [ 1, 2 ] } in a string, and a tab\t and \\ backslash"},
    ]
    return out


def result() -> dict:
    return {
        "version": "synthetic-1",
        "ms": 1234,
        "meta": {"title": "unicode éè 中文 \U0001f680", "steps": "not the steps", "deep": [[[{"a": [None, True, False]}]]]},
        "steps": steps(),
        "pruned_states": 17,
        "total_states": 4987,
        "trailer": {"steps": [{"t_s": 1}], "ms": -1},
    }


def main() -> None:
    ref = load_reference()
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / "actinv_result_synthetic.json"
    path.write_text(json.dumps(result(), indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    got = ref.scan_result(path)
    small = ref.scan_result(path, chunk=7)  # values cut by the buffer's end must come out the same
    if got["points"] != small["points"] or got["kept"] != small["kept"]:
        raise SystemExit("scan_result depends on the chunk size")
    (OUT / "actinv_scan_expected.json").write_text(json.dumps(got, indent=1) + "\n", encoding="utf-8")
    print(f"{len(got['points'])} steps, {got['bytes']} bytes, sha256 {got['sha256'][:12]}")


if __name__ == "__main__":
    main()
