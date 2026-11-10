# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Avila Labs
"""Focused checks for the CE library binding around OpenMC's MGXS conversion."""

import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
import xml.etree.ElementTree as ET
from unittest.mock import patch

import window_generator as WG


class _Model:
    def __init__(self, cross_sections, should_fail=False):
        self.expected_cross_sections = str(Path(cross_sections).resolve())
        self.materials = types.SimpleNamespace(cross_sections=str(cross_sections))
        self.settings = types.SimpleNamespace()
        self.should_fail = should_fail

    def convert_to_multigroup(self, **kwargs):
        assert os.environ["OPENMC_CROSS_SECTIONS"] == self.expected_cross_sections
        if self.should_fail:
            raise ValueError("conversion failed")
        Path(kwargs["mgxs_path"]).write_bytes(b"stub library")


class LibraryBindingTest(unittest.TestCase):
    def test_conversion_binds_model_index_and_restores_environment(self):
        mgxs = types.ModuleType("openmc.mgxs")
        mgxs.EnergyGroups = lambda edges: edges
        openmc_pkg = types.ModuleType("openmc")
        openmc_pkg.__path__ = []
        openmc_pkg.mgxs = mgxs
        openmc_pkg.__version__ = "0.16-test"
        openmc = types.SimpleNamespace(mgxs=mgxs, __version__="0.16-test")
        with tempfile.TemporaryDirectory() as tmp, patch.dict(sys.modules,
                                                              {"openmc": openmc_pkg, "openmc.mgxs": mgxs}):
            root = Path(tmp)
            (root / "library" / "ce-index.xml").parent.mkdir()
            (root / "library" / "ce-index.xml").write_text("<cross_sections/>\n")
            original_cwd = Path.cwd()
            for fails in (False, True):
                with self.subTest(fails=fails):
                    old = "ambient-index.xml"
                    with patch.dict(os.environ, {"OPENMC_CROSS_SECTIONS": old}):
                        try:
                            os.chdir(root)
                            model = _Model("library/ce-index.xml", should_fail=fails)
                            if fails:
                                with self.assertRaisesRegex(ValueError, "conversion failed"):
                                    WG.condense_multigroup(openmc, model, [0.0, 1.0], None,
                                                           root / "failure", 1,
                                                           {"method": "stochastic_slab", "correction": None,
                                                            "nparticles": 2, "batches": 2})
                            else:
                                WG.condense_multigroup(openmc, model, [0.0, 1.0], None,
                                                       root / "success", 1,
                                                       {"method": "stochastic_slab", "correction": None,
                                                        "nparticles": 2, "batches": 2})
                            self.assertEqual(os.environ["OPENMC_CROSS_SECTIONS"], old)
                        finally:
                            os.chdir(original_cwd)

    def test_missing_model_index_fails_even_with_ambient_index(self):
        mgxs = types.ModuleType("openmc.mgxs")
        mgxs.EnergyGroups = lambda edges: edges
        openmc_pkg = types.ModuleType("openmc")
        openmc_pkg.__path__ = []
        openmc_pkg.mgxs = mgxs
        openmc_pkg.__version__ = "0.16-test"
        openmc = types.SimpleNamespace(mgxs=mgxs, __version__="0.16-test")
        with tempfile.TemporaryDirectory() as tmp, patch.dict(sys.modules,
                                                             {"openmc": openmc_pkg, "openmc.mgxs": mgxs}), \
                patch.dict(os.environ, {"OPENMC_CROSS_SECTIONS": "unrelated.xml"}):
            library = Path(tmp) / "library.xml"
            library.write_text("<cross_sections/>\n")
            model = _Model(library)
            model.materials.cross_sections = None
            with self.assertRaisesRegex(RuntimeError, "model must specify materials.cross_sections"):
                WG.condense_multigroup(openmc, model, [0.0, 1.0], None, Path(tmp), 1,
                                       {"method": "stochastic_slab", "correction": None,
                                        "nparticles": 2, "batches": 2})

    def test_random_ray_exports_fixed_source_model_before_launch(self):
        class Material:
            name = "fuel"

            def set_density(self, *args):
                pass

            def add_macroscopic(self, *args):
                pass

        class Materials(list):
            cross_sections = None

        class Model:
            def __init__(self):
                self.materials = Materials([Material()])
                self.settings = types.SimpleNamespace(run_mode="eigenvalue")

            def export_to_model_xml(self, directory):
                root = ET.Element("model")
                ET.SubElement(root, "settings")
                ET.SubElement(root.find("settings"), "run_mode").text = self.settings.run_mode
                ET.ElementTree(root).write(Path(directory) / "model.xml")

        class OpenMC:
            __version__ = "0.16-test"
            RegularMesh = lambda self, **kwargs: types.SimpleNamespace(**kwargs)
            IndependentSource = lambda self, **kwargs: kwargs
            Universe = lambda self, **kwargs: kwargs
            WeightWindowGenerator = lambda self, **kwargs: kwargs
            stats = types.SimpleNamespace(Discrete=lambda *args, **kwargs: args,
                                          Box=lambda *args, **kwargs: args)

        fake_common = types.ModuleType("s0f_common")
        class FakeH5File:
            def __init__(self, path, mode):
                self.mode = mode

            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

            def values(self):
                return []

            def keys(self):
                return ["fuel"]

        fake_h5py = types.ModuleType("h5py")
        fake_h5py.File = FakeH5File
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            library = root / "mgxs.h5"
            library.write_bytes(b"stub mgxs library")
            model = Model()
            captured = {}

            def launch(directory, threads, log_name):
                xml = ET.parse(Path(directory) / "model.xml")
                captured["run_mode"] = xml.findtext("./settings/run_mode")
                raise RuntimeError("stop at mocked launch")

            with patch.dict(sys.modules, {"s0f_common": fake_common, "h5py": fake_h5py}), \
                    patch.object(WG, "run_cli", launch):
                with self.assertRaisesRegex(RuntimeError, "stop at mocked launch"):
                    WG.solve_windows(OpenMC(), model, types.SimpleNamespace(id=1), object(), [], 1.0,
                                     ([-1.0] * 3, [1.0] * 3, [1, 1, 1]), library, root / "run", 1)
            self.assertEqual(captured["run_mode"], "fixed source")


if __name__ == "__main__":
    unittest.main()
