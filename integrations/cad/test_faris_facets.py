"""Tests for the faris-facets/v1 writer and the h5m reader.

The pack/unpack tests use the standard library. Reading an h5m needs h5py, and the
comparison with MOAB needs its `mbsize` tool; each test skips when its tool is absent
(FARIS_H5PY_PYTHON and FARIS_MBSIZE name them). Run from the repository root:

    python3 -m unittest discover -s integrations/cad -p 'test_*.py'
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import faris_facets  # noqa: E402

PYTHON = Path(os.environ.get("FARIS_H5PY_PYTHON", str(Path.home() / ".cache/avila-night/feasibility-cad/cadvenv/bin/python")))
MBSIZE = Path(os.environ.get("FARIS_MBSIZE", str(Path.home() / "micromamba/envs/openmc0153dd/bin/mbsize")))
H5M = HERE / "fixtures" / "three-boxes.h5m"

READ = """
import json, sys
sys.path.insert(0, sys.argv[1])
import faris_facets
v, t, s, vols, ids, comp = faris_facets.read_h5m(sys.argv[2], 0.01)
print(json.dumps({"vertices": len(v), "triangles": len(t), "surfaces": [list(x) for x in s],
                  "volumes": vols, "bbox": [float(v.min()), float(v.max())]}))
"""


class Pack(unittest.TestCase):
    def test_round_trip(self):
        vertices = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]]
        triangles = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]]
        binary = faris_facets.pack_facets(vertices, triangles, [(0, 4, 1, 0)], 1)
        back = faris_facets.unpack_facets(binary)
        self.assertEqual(back["triangles"], triangles)
        self.assertEqual(back["surfaces"], [[0, 4, 1, 0]])
        self.assertEqual(back["n_volumes"], 1)

    def test_truncated_and_foreign_files_are_rejected(self):
        binary = faris_facets.pack_facets([[0, 0, 0]] * 3, [[0, 1, 2]], [(0, 1, 1, 0)], 1)
        with self.assertRaises(ValueError):
            faris_facets.unpack_facets(binary[:-1])
        with self.assertRaises(ValueError):
            faris_facets.unpack_facets(b"NOTFACETS" + binary[9:])


@unittest.skipUnless(PYTHON.exists() and H5M.exists(), "needs an interpreter with h5py")
class H5m(unittest.TestCase):
    def read(self):
        done = subprocess.run([str(PYTHON), "-c", READ, str(HERE), str(H5M)], capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        return json.loads(done.stdout)

    def test_the_layout_is_read_without_moab(self):
        got = self.read()
        self.assertEqual((got["vertices"], got["triangles"], len(got["surfaces"]), len(got["volumes"])), (24, 36, 18, 3))
        self.assertEqual([v["material_tag"] for v in got["volumes"]], ["mat:core", "mat:shell1", "mat:shell2"])
        # the h5m is in centimetres; the reader gives metres: a 10 cm box is 0.1 m
        self.assertAlmostEqual(got["bbox"][1] - got["bbox"][0], 0.3, places=9)

    @unittest.skipUnless(MBSIZE.exists(), "needs MOAB's mbsize")
    def test_counts_match_mbsize(self):
        done = subprocess.run([str(MBSIZE), str(H5M)], capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stderr)
        tri = int(re.search(r"^\s*Tri\s+(\d+)", done.stdout, re.M).group(1))
        vertex = int(re.search(r"^\s*Vertex\s+(\d+)", done.stdout, re.M).group(1))
        got = self.read()
        self.assertEqual((got["triangles"], got["vertices"]), (tri, vertex))


if __name__ == "__main__":
    unittest.main()
