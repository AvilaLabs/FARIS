"""The adapter's embedded 709-group edges must equal the activation library's boundaries."""
import importlib.util
import os
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BUILD = load("build_activation_inputs", ROOT / "build_activation_inputs.py")
ADAPTER = load("reactor_transport", ROOT.parent / "integrations" / "openmc" / "reactor_transport.py")


class ActivationSpectrumEdges(unittest.TestCase):
    def test_embedded_edges_are_a_valid_709_group_structure(self):
        edges = ADAPTER.ACTIVATION_709_EDGES_EV
        self.assertEqual(len(edges), BUILD.GROUPS + 1)
        self.assertTrue(all(a < b for a, b in zip(edges, edges[1:])))
        self.assertGreater(edges[0], 0.0)

    def test_embedded_edges_equal_the_library_bounds_when_the_library_is_present(self):
        data_dir = Path(os.environ.get("ACTINV_DATA_DIR", BUILD.DEFAULT_DATA_DIR))
        npz = data_dir / BUILD.CATALOG_VERSION / "activation" / f"{BUILD.LIBRARY_ID}.npz"
        if not npz.is_file():
            self.skipTest(f"activation library not available at {npz}")
        self.assertEqual(ADAPTER.ACTIVATION_709_EDGES_EV, BUILD.read_library_bounds(npz))


if __name__ == "__main__":
    unittest.main()
