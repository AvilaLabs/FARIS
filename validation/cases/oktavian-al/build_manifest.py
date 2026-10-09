#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Write manifest.json for the OKTAVIAN aluminium sphere from a local, read-only
checkout of IAEA-NDS/open-benchmarks (CC-BY-4.0).

    python3 build_manifest.py --upstream DIR [--ofb DIR] [--out manifest.json]

--upstream is a git checkout of https://github.com/IAEA-NDS/open-benchmarks; the
script reads the commit from it and refuses a dirty tree. --ofb is an optional
checkout of eepeterson/openmc_fusion_benchmarks (MIT); with h5py available it adds
a literature-context summary (never scored, requirement VAL-036).
The upstream input files are hashed, not copied.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from validation.manifest import SCHEMA, check_manifest  # noqa: E402

UPSTREAM_URL = "https://github.com/IAEA-NDS/open-benchmarks"
OFB_URL = "https://github.com/eepeterson/openmc_fusion_benchmarks"
BASE = "jade_open_benchmarks"
INPUTS = f"{BASE}/inputs/Oktavian/Oktavian_Al"
EXP = f"{BASE}/exp_results/Oktavian"
# 4 pi r^2 with r = 19.5 cm: the constant that turns the packaged per-cm2 surface tallies into the CSV values (see normalisation.basis).
AREA_CM2 = 4.0 * math.pi * 19.5**2
NEUTRON_FIRST_LOWER_MEV = 0.097122  # lower edge of the first neutron bin as packaged by openmc_fusion_benchmarks


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout.strip()


def read_table(path: Path) -> list[tuple[float, float, float]]:
    return [(float(r["Energy"]), float(r["Value"]), float(r["Error"])) for r in csv.DictReader(path.open(encoding="utf-8"))]


def ofb_context(ofb: Path | None) -> list[dict]:
    """Literature context from the MIT-licensed results database; empty when it cannot be read."""
    if ofb is None:
        return []
    try:
        import h5py  # noqa: PLC0415
    except ImportError:
        return []
    db = ofb / "src/openmc_fusion_benchmarks/results_database/oktavian_al"
    exp, calc = db / "experiment.h5", db / "openmc-0-14-0_fendl32_csg.h5"
    with h5py.File(exp) as e, h5py.File(calc) as c:
        a, b = e["neutron_leakage/neutron_leakage"][0], c["neutron_leakage/neutron_leakage"][0]
    ce = sorted(float(y[3] / x[2]) for x, y in zip(a, b) if x[2] > 0 and x[0] >= 1.0e6)
    median = ce[len(ce) // 2]
    return [{
        "kind": "literature", "label": "eepeterson/openmc_fusion_benchmarks, OpenMC 0.14.0 with FENDL-3.2, CSG geometry",
        "source": {"url": OFB_URL, "commit": git(ofb, "rev-parse", "HEAD"), "license": "MIT",
                   "files": {p.name: sha256(p) for p in (exp, calc)}},
        "summary": f"ratio of the packaged calculation to the packaged experiment values above 1 MeV (n = {len(ce)} bins, as stored, not independently checked): median {median:.3f}, minimum {ce[0]:.3f}, maximum {ce[-1]:.3f}. "
                   "Context for FARIS's own C/E; computed by that project's code, library build and tally definition, not a FARIS run.",
        "scored": False,
    }]


def build(upstream: Path, ofb: Path | None) -> dict:
    if git(upstream, "status", "--porcelain"):
        raise SystemExit("upstream checkout has local changes; use a clean checkout")
    commit = git(upstream, "rev-parse", "HEAD")
    files = []
    roles = [
        ("openmc geometry (as published)", f"{INPUTS}/openmc/geometry.xml"),
        ("openmc materials (as published; see not_covered and the runner for the material defect)", f"{INPUTS}/openmc/materials.xml"),
        ("mcnp input (source spectrum, tallies, full material compositions)", f"{INPUTS}/mcnp/Oktavian_Al.i"),
        ("serpent input", f"{INPUTS}/serpent/Oktavian_Al.i"),
        ("benchmark metadata", f"{BASE}/inputs/Oktavian/benchmark_metadata.json"),
        ("experimental neutron leakage spectrum", f"{EXP}/Oktavian_Al Neutron flux.csv"),
        ("experimental photon leakage spectrum", f"{EXP}/Oktavian_Al Gamma flux.csv"),
        ("experimental neutron spectrum, coarse bins (derived sums, not scored)", f"{EXP}/Oktavian_Al Coarse neutron flux.csv"),
        ("experimental photon spectrum, coarse bins (derived sums, not scored)", f"{EXP}/Oktavian_Al Coarse gamma flux.csv"),
        ("licence text", "LICENSE"),
    ]
    for role, rel in roles:
        path = upstream / rel
        files.append({"role": role, "path_in_source": rel, "sha256": sha256(path), "bytes": path.stat().st_size})

    neutron = read_table(upstream / EXP / "Oktavian_Al Neutron flux.csv")
    photon = read_table(upstream / EXP / "Oktavian_Al Gamma flux.csv")
    detectors = []
    for i, (hi, value, err) in enumerate(neutron):
        lo = NEUTRON_FIRST_LOWER_MEV if i == 0 else neutron[i - 1][0]
        d = {"id": f"n-{hi:g}", "response_class": "neutron_leakage_spectrum",
             "bin": {"lo_mev": lo, "hi_mev": hi, "edge_convention": "CSV energy is the upper edge of the e21 tally bin (identical edge list in the MCNP input)"},
             "controlling_variable": {"name": "energy", "value": hi, "unit": "MeV"},
             "reference": {"value": value, "u": err}}
        if i == 0:
            d["blocked"] = {"why": "the first bin (97.1 to 101.1 keV) straddles the 100 keV lower limit of the time-of-flight measurement",
                            "next_step": "score only bins entirely above 100 keV, or confirm the measurement range in the original publication"}
        detectors.append(d)
    for i, (energy, value, err) in enumerate(photon):
        lo = energy
        hi = photon[i + 1][0] if i + 1 < len(photon) else round(energy + 0.5, 1)
        detectors.append({
            "id": f"g-{energy:g}", "response_class": "photon_leakage_spectrum",
            "bin": {"lo_mev": lo, "hi_mev": hi, "edge_convention": "read as the lower edge by openmc_fusion_benchmarks; the MCNP e41 card would make it the upper edge (unresolved)"},
            "controlling_variable": {"name": "energy", "value": energy, "unit": "MeV"},
            "reference": {"value": value, "u": err},
            "blocked": {"why": "the photon bin edge convention is unresolved (lower edge in one packaging, upper edge by the MCNP e41 card), and the upstream MCNP photon tally "
                               "f41:p names cell 6, which has zero importance",
                        "next_step": "resolve the convention against the original OKTAVIAN gamma-spectrum publication or the CoNDERC record, then lift this block in the manifest"},
        })

    manifest = {
        "schema": SCHEMA,
        "case_id": "oktavian-al",
        "title": "OKTAVIAN aluminium sphere, leakage neutron and photon spectra (D-T point source)",
        "evidence_class": "experiment",
        "source": {"url": UPSTREAM_URL, "commit": commit, "retrieved": "2026-10-09",
                   "original_experiment": "Osaka University OKTAVIAN fusion neutronics benchmark (via the IAEA-NDS repository, partly adopted from CoNDERC); the original publication was not read in this pass",
                   "fetch": f"git clone {UPSTREAM_URL} && git -C open-benchmarks checkout {commit}"},
        "license": {"spdx": "CC-BY-4.0",
                    "attribution": "Experimental spectra and input files from IAEA-NDS/open-benchmarks (IAEA Nuclear Data Section), CC BY 4.0, "
                                   f"https://creativecommons.org/licenses/by/4.0/, commit {commit}; partly adopted from the IAEA CoNDERC compilation. "
                                   "The tables in this manifest were copied unchanged from the experimental CSV files named under files; no other change was made.",
                    "terms": "Upstream states that all contents, including input files, reference output and experimental data, are CC-BY-4.0."},
        "files": files,
        "files_are_external_not_vendored": True,
        "normalisation": {
            "status": "inferred", "blocks_scoring": True,
            "quantity": "leakage spectrum from the sphere surface at r = 19.95 cm",
            "units": "neutrons per source neutron per unit lethargy; photons per source neutron per MeV",
            "source_normalisation": "per source neutron (D-T source, MCNP sdef histogram si1/sp1 in the MCNP input)",
            "location": "spherical surface 6 (r = 19.95 cm) of the MCNP model; the experiment is a time-of-flight detector at about 11 m",
            "reaction_or_particle": "neutrons above 100 keV; photons 0.5 to 11 MeV",
            "energy_integration": "neutron: log-spaced bins of constant lethargy width (about 0.04); photon: 0.1 MeV bins to 5 MeV, then 0.5 MeV bins",
            "area_cm2": AREA_CM2,
            "basis": "Inferred from numbers, not documented upstream. The packaged openmc_fusion_benchmarks experiment.h5 holds per-cm2 surface values; the CSV values equal those times "
                     "4*pi*19.5^2 = 4778.36 cm2 divided by the lethargy width (neutrons, constant ratio 1.1946e5 = 4778.36/0.04, scatter 0.05 %) or the bin width in MeV "
                     "(photons, ratio constant to 1e-11 within each width). The area uses r = 19.5 cm although the tally surface is at 19.95 cm.",
            "why": "the unit is inferred from a numeric identity with a second packaging, the upstream tally is a flux (MCNP F2) not a current, and the area radius differs from the tally surface",
            "next_step": "confirm against the CoNDERC record or the original publication, then set status to confirmed"},
        "uncertainty": {
            "components": [{"name": "Error column as provided", "kind": "unspecified",
                            "meaning": "absolute 1 sigma assumed, same units as Value; upstream does not state whether it is statistical only or includes systematic terms or a normalisation uncertainty"}],
            "note": "Relative values run from about 0.2 % near the 15 MeV peak to above 80 % in the last bins; a statistical-only column would make the test stricter than the experiment justifies."},
        "compatibility": {
            "declared": "2026-10-09",
            "k": 2.0, "k_basis": "authored: coverage factor 2 on the combined standard uncertainty, assuming the Error column is a 1 sigma value (unconfirmed)",
            "covariance": {"kind": "independent",
                           "basis": "authored: the calculation shares no random input or measurement with the experiment, so Cov(C, E) = 0; shared source or normalisation terms are inside u_E as published or not stated"}},
        "response_classes": {
            "neutron_leakage_spectrum": {"unit": "n per source neutron per unit lethargy", "description": "neutron leakage, 134 bins from 97 keV to 20.66 MeV"},
            "photon_leakage_spectrum": {"unit": "photons per source neutron per MeV", "description": "photon leakage, 57 bins from 0.5 MeV"}},
        "detectors": detectors,
        "not_covered": [
            "fusion plant geometry: ports, streaming paths, plasma source shape, divertor and breeder blanket",
            "tritium production and activation reaction rates (leakage spectra only)",
            "nuclear heating and dose",
            "materials other than aluminium and the stainless-steel shells at 14 MeV; deep penetration beyond 20 cm",
            "time dependence, burnup and cooling-time behaviour",
            "magnet and superconductor response"],
        "qualified_range": {
            "materials": ["aluminium (density 1.223 g/cm3 in the model)", "stainless steel shells (7.824 g/cm3)"],
            "geometry_class": "spherical shell assembly, 19.95 cm outer radius, one beam-tube cylinder",
            "spectrum_class": "D-T point source with the MCNP histogram energy distribution, leakage observed above 100 keV",
            "parameters": [{"name": "outer_radius", "min": 19.95, "max": 19.95, "unit": "cm"}, {"name": "source_energy", "min": 0.1, "max": 20.25, "unit": "MeV"}],
            "cooling_time": "not applicable (prompt transport only)"},
        "known_upstream_defects": [
            "The published OpenMC materials.xml gives the steel shells as pure Cr50 (weight fraction 1.0, 7.824 g/cm3) and aluminium as pure Al27, "
            "while the MCNP input gives 13-nuclide stainless steel and aluminium with Si, Fe and Cu impurities. The OpenMC file also has no settings, source or tallies. "
            "The FARIS runner builds materials from the MCNP input and keeps the OpenMC geometry; it does not use the published OpenMC materials.",
            "The MCNP photon tally f41:p names cell 6, which has zero importance."],
        "literature_context": ofb_context(ofb),
    }
    return check_manifest(manifest)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--upstream", type=Path, required=True)
    ap.add_argument("--ofb", type=Path)
    ap.add_argument("--out", type=Path, default=HERE / "manifest.json")
    args = ap.parse_args()
    manifest = build(args.upstream.expanduser(), args.ofb.expanduser() if args.ofb else None)
    args.out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {args.out}: {len(manifest['detectors'])} detectors, commit {manifest['source']['commit'][:12]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
