# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Synthetic chain/index fixtures for R3 activation-data preflight."""
from __future__ import annotations

import tempfile
from pathlib import Path
import sys
import unittest
import os

try:
    import h5py
except ImportError as exc:
    if os.environ.get("FARIS_REQUIRE_OPENMC") == "1":
        raise
    H5PY_MISSING = str(exc)
    h5py = None
else:
    H5PY_MISSING = None

sys.path.insert(0, str(Path(__file__).resolve().parent))
from r3_activation_coverage import assess_coverage


@unittest.skipIf(H5PY_MISSING is not None, f"activation coverage fixtures require h5py: {H5PY_MISSING}")
class R3ActivationCoverageTest(unittest.TestCase):
    def _inputs(self, root: Path, *, include_u236=True, include_channel=True):
        chain = root / "chain.xml"
        chain.write_text(
            '<depletion_chain><nuclide name="U235"><reaction type="(n,gamma)" target="U236"/></nuclide>'
            '<nuclide name="U236"><reaction type="(n,gamma)" target="U237"/></nuclide>'
            '<nuclide name="U237"><decay type="beta-" target="Np237"/></nuclide>'
            '<nuclide name="Np237"/></depletion_chain>\n', encoding="utf-8")
        xs = root / "cross_sections.xml"
        materials = "U235 U236" if include_u236 else "U235"
        xs.write_text(f'<cross_sections><library type="neutron" materials="{materials}" path="neutron.h5"/></cross_sections>\n',
                      encoding="utf-8")
        with h5py.File(root / "neutron.h5", "w") as handle:
            for name in materials.split():
                reactions = handle.create_group(f"{name}/reactions")
                if include_channel or name != "U236":
                    reactions.create_group("reaction_102").attrs["mt"] = 102
        return chain, xs

    def test_complete_reachable_chain_and_channels_are_ready_but_unqualified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            chain, xs = self._inputs(root)
            report = assess_coverage(chain, xs, ["U235"], {"(n,gamma)": 102})
            self.assertEqual(report["status"], "READY_FOR_R2S_TRANSPORT")
            self.assertEqual(report["reachable_chain_nuclide_count"], 4)
            self.assertEqual(report["required_microxs_nuclides"], ["U235", "U236"])
            self.assertEqual([item["nuclide"] for item in report["indexed_data_files"]], ["U235", "U236"])
            self.assertEqual(report["changed_data_files_during_inspection"], [])
            self.assertEqual(report["checked_reaction_channel_count"], 2)
            self.assertIn("does not qualify evaluation quality", report["qualification_limit"])

    def test_missing_reachable_xs_nuclide_or_required_mt_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            chain, xs = self._inputs(root, include_u236=False)
            report = assess_coverage(chain, xs, ["U235"], {"(n,gamma)": 102})
            self.assertEqual(report["status"], "NOT_EVALUATED")
            self.assertEqual(report["missing_reachable_nuclides"], ["U236"])

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            chain, xs = self._inputs(root, include_channel=False)
            report = assess_coverage(chain, xs, ["U235"], {"(n,gamma)": 102})
            self.assertEqual(report["status"], "NOT_EVALUATED")
            self.assertEqual(report["missing_reaction_channels"], [
                {"nuclide": "U236", "reaction": "(n,gamma)", "mt": 102,
                 "reason": "OpenMC HDF5 reaction group missing"}])


if __name__ == "__main__":
    unittest.main()
