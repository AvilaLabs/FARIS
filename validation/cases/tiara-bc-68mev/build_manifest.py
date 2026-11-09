#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Build the TIARA 68 MeV BC501A Fe/Concrete manifests from a pinned JADE checkout."""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
UPSTREAM_URL = "https://github.com/IAEA-NDS/open-benchmarks"
PINNED_COMMIT = "1d9855b00e05c5d5e25a11026ac56940dc75b516"
BASE = "jade_open_benchmarks"
CASES = {
    "tiara-bc-fe-68-40": ("Tiara-BC_fe-68-40-00", "iron", 40.0, "7.87 g/cm3 natural iron", 7.87),
    "tiara-bc-cc-68-100": ("Tiara-BC_cc-68-100-00", "concrete", 100.0, "TIARA concrete", 2.31),
}
LICENSE_TEXT = "CC-BY-4.0"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout.strip()


def parse_table(raw: bytes) -> list[dict]:
    import io

    reader = csv.DictReader(io.StringIO(raw.decode("utf-8")))
    if reader.fieldnames != ["Energy", "Value", "Error", "abs err", "Offset"]:
        raise ValueError("unexpected TIARA BC table columns")
    rows = []
    for row in reader:
        energy = float(row["Energy"])
        if row["Offset"].strip() != "on-axis":
            continue
        value = float(row["Value"]) if row["Value"].strip() else None
        rel_u = float(row["Error"]) if row["Error"].strip() else None
        abs_u = float(row["abs err"]) if row["abs err"].strip() else None
        rows.append({"energy_mev": energy, "value": value, "relative_u": rel_u, "absolute_u": abs_u})
    if len(rows) != 52 or [r["energy_mev"] for r in rows][-1] != 70.0:
        raise ValueError("expected the complete 52-row, on-axis TIARA BC spectrum")
    if len({r["energy_mev"] for r in rows}) != len(rows):
        raise ValueError("TIARA spectrum has duplicate energy coordinates")
    return rows


def build_case(upstream: Path, case_id: str, data: tuple) -> dict:
    if git(upstream, "status", "--porcelain"):
        raise ValueError("upstream checkout has local changes; use a clean pinned checkout")
    commit = git(upstream, "rev-parse", "HEAD")
    if commit != PINNED_COMMIT:
        raise ValueError(f"expected IAEA/JADE commit {PINNED_COMMIT}, got {commit}")

    name, material_label, shield_cm, material_spec, density = data
    deck_rel = f"{BASE}/inputs/Tiara-BC/{name}/mcnp/{name}.i"
    table_rel = f"{BASE}/exp_results/Tiara-BC/{name} Neutron yield.csv"
    deck_path, table_path = upstream / deck_rel, upstream / table_rel
    if not deck_path.is_file() or not table_path.is_file():
        raise ValueError(f"pinned TIARA sources missing for {case_id}")
    table = parse_table(table_path.read_bytes())
    deck = deck_path.read_text(encoding="utf-8")
    if "F14:n 109" not in deck or "FM14 1.31126E+11" not in deck or "E14" not in deck:
        raise ValueError("selected MCNP input no longer has the documented on-axis BC detector and normalization")

    files = []
    for role, path, rel in (("pinned MCNP model input", deck_path, deck_rel),
                            ("measured TIARA BC501A on-axis neutron yield table", table_path, table_rel),
                            ("upstream CC-BY-4.0 licence", upstream / "LICENSE", "LICENSE")):
        files.append({"role": role, "path_in_source": rel, "sha256": sha256(path), "bytes": path.stat().st_size})

    detectors = []
    for row in table:
        energy = row["energy_mev"]
        det = {"id": f"n-{energy:g}", "response_class": "neutron_spectrum_in_detector",
               "bin": {"upper_mev": energy, "edge_convention": "the CSV Energy value matches an upper energy boundary in the MCNP E14 card; the corresponding tally bin ends at this energy"},
               "controlling_variable": {"name": "energy-bin upper boundary", "value": energy, "unit": "MeV"}}
        if row["value"] is None or (row["value"] == 0.0 and row["absolute_u"] == 0.0):
            reason = "the upstream table contains no value and uncertainty for this energy row" if row["value"] is None else "the upstream table uses 0 value and 0 uncertainty as a placeholder, not a quantified measurement"
            det.update({"reference_missing_why": reason,
                        "reference_missing_next_step": "retain the row as NOT_EVALUATED unless the primary experimental report supplies a value and uncertainty"})
        else:
            det["reference"] = {"value": row["value"], "u": row["absolute_u"]}
            det["reported_relative_error"] = row["relative_u"]
        if energy == 6.0:
            det["blocked"] = {"why": "the first response interval begins at zero energy, so its lethargy width is undefined", "next_step": "leave this bin NOT_EVALUATED; do not extrapolate a lower edge"}
        detectors.append(det)

    return {
        "schema": "faris.validation-case/1.0.0", "case_id": case_id,
        "title": f"TIARA {material_label} shield, 68 MeV p-Li source, BC501A on-axis spectrum",
        "evidence_class": "experiment",
        "source": {"url": UPSTREAM_URL, "commit": commit, "retrieved": "2026-11-09",
                   "reference": "IAEA-NDS INDC(NDS)-0785, Kos and Kodeli, May 2019, DOI 10.61092/iaea.ny5x-0pq9; OECD-NEA SINBAD TIARA 43/68 MeV benchmark, Tables 1 and 4, https://www.oecd-nea.org/science/wprs/shielding/sinbad/tiara/tia-abs.htm",
                   "fetch": f"git clone {UPSTREAM_URL} open-benchmarks && git -C open-benchmarks checkout {commit}"},
        "license": {"spdx": LICENSE_TEXT,
                    "attribution": f"Experimental spectrum and MCNP input from IAEA-NDS/open-benchmarks, commit {commit}; upstream states CC-BY-4.0. The uncertainty column `abs err` is retained without alteration; the Error column is retained as reported relative error. JADE and IAEA report attribution applies."},
        "files": files, "files_are_external_not_vendored": True,
        "normalisation": {
            "status": "confirmed", "blocks_scoring": False,
            "quantity": "BC501A detector-volume neutron flux per proton beam charge, per unit lethargy",
            "units": "neutrons cm-2 microC-1 per unit lethargy",
            "source_normalisation": "source spectrum shape from the 68 MeV TIARA/SINBAD source spectrum (JADE SI1 H/SP1 cards); the MCNP tally multiplier FM14=1.31126E+11 is preserved exactly and corresponds to PtC 2.61 x peak flux 4.00E+9 n sr-1 microC-1 x 4*pi using the printed rounded factor 3.14",
            "location": "F4 volume flux tally in the 12.7 cm diameter by 12.7 cm long BC501A scintillator, MCNP cell 109, on axis just behind the shield",
            "reaction_or_particle": "neutron fluence spectrum (not detector pulse-height response); BC501A modeled as liquid scintillator in the input",
            "energy_integration": "MCNP E14 upper boundaries; each tally integral is divided by ln(E_hi/E_lo) to obtain per-unit-lethargy response; the 0-to-6 MeV first interval is blocked because its lower edge is zero",
            "basis": "The pinned MCNP input defines F14:n 109, FM14 and E14. Kos and Kodeli (IAEA INDC(NDS)-0785, section 2.2.2-2.2.3) state that the source energy spectrum is taken from SINBAD Table 4 with source bin boundaries interpreted as upper boundaries, the source is at the origin, the BC501A volume is 12.7 cm x 12.7 cm and tally F4 is normalized with PtC*PF*4*pi. The OECD-NEA SINBAD TIARA 43/68 MeV page says transmitted spectra are normalized per proton beam charge using the source peak flux and source spectrum. The exact FM card is used; the source card's SB sampling bias is not folded into the physical source distribution.",
            "normalisation_inputs": {"ptc": 2.61, "peak_flux_n_sr-1_microC-1": 4.0e9, "solid_angle_factor_sr": 12.56, "mcnp_fm14": 1.31126e11, "derived_rounded": 2.61 * 4.0e9 * 12.56, "relative_difference": abs(1.31126e11 - (2.61 * 4.0e9 * 12.56)) / 1.31126e11},
        },
        "uncertainty": {"components": [{"name": "JADE experimental Error and abs err columns", "kind": "reported", "meaning": "The CSV's Error column is the reported relative error and abs err is the absolute uncertainty column. The source materials do not identify the confidence level or covariance structure; no confidence interpretation is added."}],
                        "note": "Retains each published abs err as u. The measurement confidence meaning and inter-bin covariance are unknown; compatibility therefore uses covariance unknown and remains INCONCLUSIVE until that uncertainty structure is resolved."},
        "compatibility": {"declared": "2026-11-09", "k": 2.0,
                          "k_basis": "Authored coverage factor 2 for comparison only; the experimental uncertainty confidence meaning is unspecified, so this rule cannot establish PASS/FAIL.",
                          "covariance": {"kind": "unknown", "basis": "The source reports bin uncertainties but no covariance; source normalization and unfolding can correlate energy bins."}},
        "response_classes": {"neutron_spectrum_in_detector": {"unit": "neutrons cm-2 microC-1 per unit lethargy", "description": "measured on-axis BC501A volume neutron spectrum immediately behind the named TIARA iron or concrete shield; not a general transport flux validation"}},
        "detectors": detectors,
        "not_covered": ["6 MeV first bin is blocked because its lower energy bound is zero", "rows without reported values and the 70 MeV zero/zero placeholder remain NOT_EVALUATED", "source and measurement uncertainty covariance is unknown", "BC501A unfolding response and detector model systematic uncertainty are outside this transport-only comparison", "no FARIS OpenMC run record exists yet"],
        "qualified_range": {"materials": [material_spec, "iron collimator, iron/polyethylene shutter, concrete building, aluminium support and BC501A scintillator as defined in the pinned input"],
                            "geometry_class": "full TIARA beamline, collimator, shield and BC501A detector CSG from JADE MCNP input translated to OpenMC; on-axis detector",
                            "spectrum_class": "68 MeV proton on 99.9% Li-7 target; measured quasi-monoenergetic source spectrum prescribed as MCNP SI1 H/SP1 histogram; physical angular distribution from SP2; MCNP SB2 bias omitted",
                            "parameters": [{"name": "shield thickness", "min": shield_cm, "max": shield_cm, "unit": "cm"}, {"name": "source peak flux", "min": 4.0e9, "max": 4.0e9, "unit": "neutrons sr-1 microC-1"}],
                            "cooling_time": "not applicable (prompt neutron transport)"},
        "literature_context": [],
        "translation": {"status": "implemented; API-tested, no transport run", "geometry": "directly translated from the pinned MCNP cell/surface cards; see run_case.py strict supported-card set", "source": "SI1 H/SP1 normalized histogram; SP2 checked against isotropic cosine probability; SB2 is omitted as a sampling bias", "scoring": "Cell 109 flux energy tally with FM14 scale and lethargy normalization", "nuclear_data_preflight": "all required nuclide tables must cover the 72.5 MeV maximum source energy; incomplete upper-energy evaluations block scoring and no extrapolation is applied"}
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", type=Path, required=True, help="clean checkout of IAEA-NDS/open-benchmarks at the pinned commit")
    parser.add_argument("--out-dir", type=Path, default=Path(__file__).resolve().parent.parent, help="directory where case folders are written")
    args = parser.parse_args()
    sys.path.insert(0, str(ROOT))
    from validation.manifest import check_manifest

    for case_id, data in CASES.items():
        manifest = check_manifest(build_case(args.upstream, case_id, data))
        out = args.out_dir / case_id / "manifest.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
