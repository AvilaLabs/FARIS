# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

import h5py
import numpy as np

import r3_review_composite as RC


def _write(path: Path, data: bytes) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return hashlib.sha256(data).hexdigest()


def _json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


class ReviewCompositeTests(unittest.TestCase):
    def _fixture_build_inputs(self, base: Path):
        candidate_dir = base / "candidate"
        candidate_dir.mkdir()
        candidate_file = candidate_dir / "Fe56.h5"
        _write(candidate_file, b"candidate bytes")
        candidate_index = candidate_dir / "cross_sections.xml"
        candidate_index.write_text(
            '<cross_sections><library materials="Fe56" path="Fe56.h5" type="neutron" /></cross_sections>\n',
            encoding="utf-8")
        index_hash = hashlib.sha256(candidate_index.read_bytes()).hexdigest()
        receipt_path = candidate_dir / "download-receipt.json"
        _json(receipt_path, {"sha256": RC.TENDL_ARCHIVE_SHA256, "zenodo_record": 21534376,
                             "known_missing": ["H1", "H2", "Ne24", "S42"],
                             "license": "test fixture", "provider_creator_claim": [{"name": "TENDL"}]})

        fendl_dir = base / "fendl"
        fendl_dir.mkdir()
        hpaths = {}
        hhash = {}
        for isotope in ("H1", "H2"):
            p = fendl_dir / f"{isotope}.h5"
            hhash[isotope] = _write(p, (isotope + " data").encode())
            hpaths[isotope] = p
        fendl_index = fendl_dir / "cross_sections.xml"
        fendl_index.write_text("<cross_sections>" + "".join(
            f'<library materials="{name}" path="{name}.h5" type="neutron" />' for name in ("H1", "H2")) +
            "</cross_sections>\n", encoding="utf-8")

        pilots = {}
        pilot_paths = {}
        for isotope in RC.PILOT_NUCLIDES:
            d = base / f"pilot-{isotope}"
            d.mkdir()
            source, hdf, njoy = d / "source.endf", d / f"{isotope}.h5", d / "njoy"
            source_hash = _write(source, b"ENDF source")
            hdf.parent.mkdir(parents=True, exist_ok=True)
            with h5py.File(hdf, "w") as handle:
                group = handle.create_group(f"{isotope}/reactions")
                for mt in (2, 102):
                    reaction = group.create_group(f"reaction_{mt:03d}")
                    reaction.attrs["mt"] = mt
            hdf_hash = hashlib.sha256(hdf.read_bytes()).hexdigest()
            njoy_hash = _write(njoy, b"NJOY executable")
            test_log = d / "Testing/Temporary/LastTest.log"
            _write(test_log, ("Test Passed.\n" * 13 + "End testing: test fixture\n").encode())
            receipt = {"status": "CONVERTED_UNQUALIFIED", "nuclide": isotope,
                       "source_file": str(source), "source_sha256": source_hash, "source_bytes": source.stat().st_size,
                       "njoy": str(njoy), "njoy_sha256": njoy_hash, "openmc_version": "0.15.3",
                       "temperatures_K": ["294K"], "max_energy_eV": 200e6,
                       "hdf5": str(hdf), "hdf5_sha256": hdf_hash, "mts": [1, 2, 101, 102]}
            _json(d / "receipt.json", receipt)
            _json(d / "audit.json", {"status": "STRUCTURAL_AND_DESCRIPTIVE_AUDIT_PASS",
                                      "source_sha256": source_hash, "hdf5_sha256": hdf_hash,
                                      "xs_ranges": [{"mt": 2}, {"mt": 102}],
                                      "energy_max_ev": {"294K": 200e6},
                                      "qualification": "not a physical validation; structural fixture only"})
            pilots[isotope] = d
            pilot_paths[isotope] = hdf
        constants = {
            "TENDL_INDEX_SHA256": index_hash,
            "TENDL_ARCHIVE_SHA256": RC.TENDL_ARCHIVE_SHA256,
            "FENDL_H_SHA256": hhash,
        }
        return candidate_index, receipt_path, fendl_index, pilots, hpaths, pilot_paths, constants

    def test_build_writes_new_sorted_four_override_index_without_mutating_inputs(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            candidate, receipt, fendl, pilots, hpaths, pilot_paths, constants = self._fixture_build_inputs(base)
            before = {p: p.read_bytes() for p in [candidate, receipt, fendl, *hpaths.values(),
                                                   *(d / f for d in pilots.values() for f in ("receipt.json", "audit.json"))]}
            output = base / "review"
            with mock.patch.multiple(RC, **constants):
                plan = RC.build_review_index(candidate_index=candidate, candidate_receipt=receipt,
                                             fendl_index=fendl, pilot_roots=pilots, output_dir=output)
                self.assertEqual(plan["index_review"]["status"], "REVIEW_COMPOSITE_UNQUALIFIED")
                self.assertFalse(plan["index_review"]["transport_eligible"])
                root, entries = RC._entry_map(output / "cross_sections.review.xml")
                self.assertEqual(set(entries), {"Fe56", "H1", "H2", "Ne24", "S42"})
                self.assertEqual(entries["Fe56"][1], candidate.parent / "Fe56.h5")
                self.assertEqual(entries["H1"][1], hpaths["H1"])
                self.assertEqual(entries["H2"][1], hpaths["H2"])
                self.assertEqual(entries["Ne24"][1], pilot_paths["Ne24"])
                self.assertEqual(entries["S42"][1], pilot_paths["S42"])
                self.assertEqual([name for name, _ in sorted(entries.items())],
                                 sorted(entries))
            for path, data in before.items():
                self.assertEqual(path.read_bytes(), data)
            with mock.patch.multiple(RC, **constants), self.assertRaisesRegex(ValueError, "already exists"):
                RC.build_review_index(candidate_index=candidate, candidate_receipt=receipt,
                                      fendl_index=fendl, pilot_roots=pilots, output_dir=output)
            second = base / "review-copy"
            with mock.patch.multiple(RC, **constants):
                RC.build_review_index(candidate_index=candidate, candidate_receipt=receipt,
                                      fendl_index=fendl, pilot_roots=pilots, output_dir=second)
            self.assertEqual((output / "cross_sections.review.xml").read_bytes(),
                             (second / "cross_sections.review.xml").read_bytes())

    @staticmethod
    def _write_xs(path: Path, name: str, *, temperature="294K", max_energy=20e6,
                  reaction_mts=(102,)):
        path.parent.mkdir(parents=True, exist_ok=True)
        grid = np.array([1e-5, 1e6, max_energy], dtype=np.float64)
        with h5py.File(path, "w") as handle:
            handle.attrs["filetype"] = np.bytes_(b"data_neutron")
            handle.attrs["version"] = np.array([3, 0], dtype=np.int32)
            group = handle.create_group(name)
            energy = group.create_group("energy")
            energy.create_dataset(temperature, data=grid)
            reactions = group.create_group("reactions")
            for mt in reaction_mts:
                reaction = reactions.create_group(f"reaction_{mt:03d}")
                reaction.attrs["mt"] = mt
                temp = reaction.create_group(temperature)
                temp.create_dataset("xs", data=np.array([0.0, 1.0, 2.0]))

    def _write_audit_inputs(self, base: Path, *, s_energy=20e6, missing_u237=False):
        chain = base / "chain.xml"
        chain.write_text(
            '<depletion_chain><nuclide name="U235"><reaction type="(n,gamma)" target="U236" />'
            '</nuclide><nuclide name="U236"><reaction type="(n,gamma)" target="U237" />'
            '</nuclide><nuclide name="U237"><reaction type="(n,gamma)" target="U238" />'
            '</nuclide><nuclide name="U238" /></depletion_chain>', encoding="utf-8")
        materials = base / "materials.xml"
        materials.write_text('<materials><material><nuclide name="U235" /></material></materials>',
                             encoding="utf-8")
        datadir = base / "xs"
        names = ["U235", "U236", "U237"]
        index = datadir / "cross_sections.xml"
        rows = []
        for name in names:
            path = datadir / f"{name}.h5"
            self._write_xs(path, name, max_energy=s_energy if name == "U236" else 20e6,
                           reaction_mts=() if missing_u237 and name == "U237" else (102,))
            rows.append(f'<library materials="{name}" path="{path.name}" type="neutron" />')
        index.write_text("<cross_sections>" + "".join(rows) + "</cross_sections>", encoding="utf-8")
        return chain, materials, index

    def test_audit_checks_all_reachable_reactions_energy_and_temperature(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            chain, materials, index = self._write_audit_inputs(base)
            report = RC.audit_review_index(review_index=index, chain_xml=chain, materials_xml=materials,
                                           reaction_mt={"(n,gamma)": 102}, output_path=base / "audit.json")
            self.assertEqual(report["status"], "STRUCTURAL_CE_PRESENCE_PASS_UNQUALIFIED")
            self.assertFalse(report["transport_eligible"])
            self.assertEqual(report["reviewed_file_count"], 3)
            self.assertEqual(report["required_reaction_channel_count"], 3)
            self.assertEqual(len(report["required_nuclides"]), 3)

    def test_audit_refuses_incomplete_channel_and_high_energy_coverage(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            chain, materials, index = self._write_audit_inputs(base, s_energy=19e6, missing_u237=True)
            report = RC.audit_review_index(review_index=index, chain_xml=chain, materials_xml=materials,
                                           reaction_mt={"(n,gamma)": 102}, output_path=base / "audit.json")
            self.assertEqual(report["status"], "INCOMPLETE")
            self.assertFalse(report["transport_eligible"])
            reasons = " ".join(item["reason"] for item in report["malformed_or_insufficient_files"])
            self.assertIn("ends below", reasons)
            self.assertTrue(report["missing_reaction_channels"])


if __name__ == "__main__":
    unittest.main()
