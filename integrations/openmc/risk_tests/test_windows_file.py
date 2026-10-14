"""Weight windows reach OpenMC as an HDF5 file, not as XML text, and give the same answer as the text did.

On a large machine the windows' bounds are hundreds of megabytes of XML (475,695 voxels x 17 groups, lower and
upper); a model file that size hit the job runner's file limit and OpenMC would parse it again at every run. The run
now sets `settings.weight_windows_file` to a run-local copy of the windows' HDF5 file, and the aiming step caps the
bounds in that file. These tests check, on the 65-voxel slab of test_windows_unbiased.py:

- the model file stays small however many voxels the windows have, and the run-local file carries FARIS's ids;
- a run from the file and a run with the same windows inline give the same tally mean and error to the last digit
  (same seed, same random stream), for the windows as made and for windows capped along a line of sight;
- `cap_model` writes in the file the very bounds it writes in XML for a model with its windows inline.

Nuclear data and the skip rules are those of test_windows_unbiased.py (see openmc_ci.py). Run with the OpenMC 0.15.3
python:

    FARIS_CI_NEUTRON_DATA=<folder> python3 -m unittest test_windows_file
"""

import hashlib
import os
import re
import sys
import tempfile
import unittest
import warnings
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import design_transport as DT  # noqa: E402
import openmc_ci  # noqa: E402
import test_windows_unbiased as U  # noqa: E402

RUN = {"batches": 10, "particles": 2000}
SEED = 303
ENERGY_BOUNDS = U.ENERGY_BOUNDS
# A target in the detector cell, facing the source: (name, centre, half-extent u, half-extent v, normal to the response)
TARGET = ("detector", (62.0, 0.0, 0.0), (0.0, 10.0, 0.0), (0.0, 0.0, 10.0), (1.0, 0.0, 0.0))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_windows(openmc, path: Path, dimension=(65, 1, 1)) -> None:
    """Windows OpenMC itself writes: the lower bound falls with depth, the upper is five times it; the lower
    energy group (below 0.1 MeV) is half the upper's."""
    import numpy as np

    mesh = openmc.RegularMesh(mesh_id=7, name="windows")
    mesh.lower_left = (0.0, -U.HALF_WIDTH_CM, -U.HALF_WIDTH_CM)
    mesh.upper_right = (U.IRON_CM + U.HYDROGEN_CM + U.DETECTOR_CM, U.HALF_WIDTH_CM, U.HALF_WIDTH_CM)
    mesh.dimension = dimension
    x = (np.arange(dimension[0]) + 0.5) * (U.IRON_CM + U.HYDROGEN_CM + U.DETECTOR_CM) / dimension[0]
    depth = np.broadcast_to(np.exp(-x / 10.0)[:, None, None], dimension)
    lower = np.stack([0.5 * depth, depth], axis=-1)
    window = openmc.WeightWindows(mesh, lower, 5.0 * lower, energy_bounds=ENERGY_BOUNDS, particle_type="neutron")
    # OpenMC writes the file from a session on an empty model, which still wants a library with an entry (never read)
    library = path.parent / "unused_cross_sections.xml"
    library.write_text('<?xml version="1.0"?>\n<cross_sections>\n  <library materials="H1" path="H1.h5" '
                       'type="neutron"/>\n</cross_sections>\n')
    saved = os.environ.get("OPENMC_CROSS_SECTIONS")
    os.environ["OPENMC_CROSS_SECTIONS"] = str(library)
    try:
        openmc.WeightWindowsList([window]).export_to_hdf5(path)
    finally:
        if saved is None:
            del os.environ["OPENMC_CROSS_SECTIONS"]
        else:
            os.environ["OPENMC_CROSS_SECTIONS"] = saved


RUN_WINDOWS = "windows.h5"


def labelled(filled: Path, directory: Path) -> Path:
    """A copy of FILLED in DIRECTORY with FARIS's labels, as the window step leaves its own file."""
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / RUN_WINDOWS
    target.write_bytes(filled.read_bytes())
    DT.label_windows(target)
    return target


def inline_windows(openmc, model, filled: Path) -> None:
    """The old way, kept to compare against: the windows written into the model's XML."""
    window = openmc.hdf5_to_wws(str(filled))[0]
    window.id, window.max_split = DT.WINDOW_ID, DT.MAX_SPLIT
    window.mesh.id = DT.WINDOW_MESH_ID
    model.settings.weight_windows = [window]
    model.settings.weight_windows_on = True
    model.settings.weight_window_checkpoints = {"collision": True, "surface": True}


def inline_bounds(xml: str):
    import numpy as np

    block = xml[xml.index('<weight_windows id="1">'):xml.index("</weight_windows>")]
    get = lambda tag: np.array([float(t) for t in re.search(rf"<{tag}>(.*?)</{tag}>", block, re.S).group(1).split()])
    return get("lower_ww_bounds"), get("upper_ww_bounds")


def tally_of(openmc, directory: Path, batches: int = RUN["batches"]):
    return U.read_tally(openmc, directory, batches)


class WindowsFromAFile(unittest.TestCase):
    def setUp(self):
        openmc_ci.require_modules("numpy", "h5py", "openmc")
        warnings.simplefilter("ignore", FutureWarning)
        import openmc

        warnings.simplefilter("ignore", openmc.IDWarning)

    def model(self, library: Path):
        import openmc

        return U.build_model(openmc, library, RUN, SEED)

    # Verifies: NUC-051
    def test_the_model_file_holds_no_bounds_and_the_run_file_carries_the_ids(self):
        import h5py
        import openmc

        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            filled = work / "filled.h5"
            write_windows(openmc, filled, dimension=(50, 50, 40))       # 100,000 voxels x 2 groups
            before = sha256(filled)
            with h5py.File(filled) as stream:
                first = stream["weight_windows"][next(iter(stream["weight_windows"]))]
                self.assertEqual(first["lower_ww_bounds"].shape, (2, 100_000))
            # No nuclear data is needed to write the model
            model = self.model(work / "cross_sections.xml")
            run = work / "solver"
            mesh = DT.apply_windows(openmc, model, labelled(filled, run), run)
            DT.export_model(model, run / "model.xml", mesh)

            xml = (run / "model.xml").read_text()
            self.assertLess((run / "model.xml").stat().st_size, 1_000_000)
            self.assertGreater((run / RUN_WINDOWS).stat().st_size, 1_000_000)
            self.assertNotIn("lower_ww_bounds", xml)
            self.assertNotIn("<weight_windows ", xml)
            self.assertIn("<weight_windows_file>windows.h5</weight_windows_file>", xml)
            self.assertIn("<weight_windows_on>true</weight_windows_on>", xml)
            self.assertIn(f'<mesh id="{DT.WINDOW_MESH_ID}"', xml)
            self.assertEqual(xml.count("<mesh "), 1)
            self.assertIn("<dimension>50 50 40</dimension>", xml)

            with h5py.File(run / RUN_WINDOWS) as stream:
                group = stream[f"weight_windows/weight_windows_{DT.WINDOW_ID}"]
                self.assertEqual(int(group["mesh"][()]), DT.WINDOW_MESH_ID)
                self.assertEqual(int(group["max_split"][()]), DT.MAX_SPLIT)
                self.assertEqual(list(stream["weight_windows"].attrs["ids"]), [DT.WINDOW_ID])
                self.assertEqual(list(stream["meshes"].attrs["ids"]), [DT.WINDOW_MESH_ID])
                self.assertEqual(int(stream[f"meshes/mesh {DT.WINDOW_MESH_ID}"].attrs["id"]), DT.WINDOW_MESH_ID)
            # the bounds are the cache entry's, untouched, and the cache entry itself was not edited
            made = openmc.hdf5_to_wws(str(run / RUN_WINDOWS))[0]
            original = openmc.hdf5_to_wws(str(filled))[0]
            self.assertEqual(made.id, DT.WINDOW_ID)
            self.assertEqual(made.max_split, DT.MAX_SPLIT)
            self.assertEqual(made.mesh.id, DT.WINDOW_MESH_ID)
            self.assertTrue((made.lower_ww_bounds == original.lower_ww_bounds).all())
            self.assertTrue((made.upper_ww_bounds == original.upper_ww_bounds).all())
            self.assertEqual(sha256(filled), before)

    # Verifies: NUC-051
    def test_a_run_from_the_file_equals_a_run_with_the_windows_inline(self):
        import openmc

        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            library = openmc_ci.cross_sections_xml(openmc_ci.neutron_data_dir(), work)
            filled = work / "filled.h5"
            write_windows(openmc, filled)
            results = self.paired_runs(openmc, work, library, filled, "")
            analog = self.model(library)
            U.run_in(work / "analog", analog, "openmc.log", threads=1)
            unwindowed = tally_of(openmc, work / "analog")
        print(f"\n  inline {results['inline'][0]!r} +/- {results['inline'][1]!r}\n  file   {results['file'][0]!r} "
              f"+/- {results['file'][1]!r}\n  no windows {unwindowed[0]!r}")
        self.assertEqual(results["inline"], results["file"])
        # the comparison has teeth: the windows change the random stream, so the unwindowed run differs
        self.assertNotEqual(results["file"], unwindowed)
        self.assertGreater(results["file"][0], 0.0)

    def paired_runs(self, openmc, work: Path, library: Path, filled: Path, tag: str) -> dict:
        """The slab run with the windows of `filled` inline and from the file; one thread, so sums keep their order."""
        inline = self.model(library)
        inline_windows(openmc, inline, filled)
        U.run_in(work / f"inline{tag}", inline, "openmc.log", threads=1)

        return {"inline": tally_of(openmc, work / f"inline{tag}"),
                "file": self.run_from_file(openmc, work / f"file{tag}", library, filled)}

    def run_from_file(self, openmc, directory: Path, library: Path, filled: Path):
        """The slab run with the windows of `filled` read from a file in `directory`."""
        from_file = self.model(library)
        mesh = DT.apply_windows(openmc, from_file, labelled(filled, directory), directory)
        DT.export_model(from_file, directory / "model.xml", mesh)
        DT.run_cli(directory, 1, "openmc.log")
        return tally_of(openmc, directory)

    def fake_visibility(self, W):
        """A reach probability that falls with distance from the target: deterministic, and large enough to lower
        the bounds far from it."""
        import numpy as np

        def visibility_run(base, work, target, groups, mesh_xml, particles, threads=4):
            n_vox = int(re.search(r"<dimension>(\d+)", mesh_xml).group(1))
            x = (np.arange(n_vox) + 0.5)
            prob = 1.0 / (1.0 + (x - 62.0) ** 2 / 100.0)
            volume = float(np.prod([float(t) for t in re.search(r"<width>(.*?)</width>", mesh_xml).group(1).split()]))
            # the inverse of W.reach_probability
            mean = prob * 4.0 / W.target_area(target[2], target[3]) / (len(groups) - 1) * volume
            return np.tile(mean, (len(groups) - 1, 1)), np.full((len(groups) - 1, n_vox), 0.1)

        return visibility_run

    # Verifies: NUC-055
    def test_capped_windows_in_the_file_are_the_bounds_the_xml_path_wrote(self):
        import numpy as np
        import openmc

        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import s0f_los_windows as W

        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            library = openmc_ci.cross_sections_xml(openmc_ci.neutron_data_dir(), work)
            filled = work / "filled.h5"
            write_windows(openmc, filled)

            inline = self.model(library)
            inline_windows(openmc, inline, filled)
            inline.export_to_model_xml(str(work / "inline-base"))
            from_file = self.model(library)
            run = work / "solver"
            mesh = DT.apply_windows(openmc, from_file, labelled(filled, run), run)
            DT.export_model(from_file, run / "model.xml", mesh)

            original = W.read_window_file(run / RUN_WINDOWS)
            real = W.visibility_run
            W.visibility_run = self.fake_visibility(W)
            try:
                aim = work / "aim"
                aim.mkdir()
                counts = W.cap_model(run / "model.xml", aim / "model-aimed.xml", work / "v1", [TARGET])
                inline_counts = W.cap_model(work / "inline-base" / "model.xml", work / "inline-capped.xml",
                                            work / "v2", [TARGET])
            finally:
                W.visibility_run = real

            self.assertEqual(counts["windows"], str(aim / "windows-capped.h5"))
            self.assertIsNone(inline_counts["windows"])
            self.assertEqual({k: counts[k] for k in ("lowered", "positive")},
                             {k: inline_counts[k] for k in ("lowered", "positive")})
            self.assertGreater(counts["lowered"], 10)
            # the aimed model is the run's model; the run puts the capped file under the name it gives
            self.assertEqual((aim / "model-aimed.xml").read_text(), (run / "model.xml").read_text())
            _, lower, upper = W.read_window_file(aim / "windows-capped.h5")
            xml_lower, xml_upper = inline_bounds((work / "inline-capped.xml").read_text())
            self.assertTrue(np.array_equal(lower.ravel(), xml_lower))
            self.assertTrue(np.array_equal(upper.ravel(), xml_upper))
            self.assertTrue((lower <= original[1]).all())
            self.assertTrue((lower < original[1]).any())
            # the uncapped file is untouched and the capped copy keeps FARIS's ids
            self.assertTrue(np.array_equal(W.read_window_file(run / RUN_WINDOWS)[1], original[1]))
            capped = openmc.hdf5_to_wws(str(aim / "windows-capped.h5"))[0]
            self.assertEqual((capped.max_split, capped.mesh.id), (DT.MAX_SPLIT, DT.WINDOW_MESH_ID))

            # and a run on them: inline capped against the file capped, same seed
            inline_dir, file_dir = work / "capped-inline", work / "capped-file"
            inline_dir.mkdir()
            file_dir.mkdir()
            (inline_dir / "model.xml").write_text((work / "inline-capped.xml").read_text())
            (file_dir / "model.xml").write_text((aim / "model-aimed.xml").read_text())
            (file_dir / RUN_WINDOWS).write_bytes((aim / "windows-capped.h5").read_bytes())
            DT.run_cli(inline_dir, 1, "openmc.log")
            DT.run_cli(file_dir, 1, "openmc.log")
            a, b = tally_of(openmc, inline_dir), tally_of(openmc, file_dir)
            uncapped = self.run_from_file(openmc, work / "uncapped", library, filled)
        print(f"\n  capped inline {a[0]!r} +/- {a[1]!r}\n  capped file   {b[0]!r} +/- {b[1]!r}\n  uncapped      {uncapped!r}")
        self.assertEqual(a, b)
        self.assertNotEqual(b, uncapped)

    # Verifies: NUC-055
    def test_cap_model_with_a_real_visibility_run_agrees_between_the_two_paths(self):
        import numpy as np
        import openmc

        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import s0f_los_windows as W

        with tempfile.TemporaryDirectory() as scratch:
            work = Path(scratch)
            library = openmc_ci.cross_sections_xml(openmc_ci.neutron_data_dir(), work)
            filled = work / "filled.h5"
            write_windows(openmc, filled)
            inline = self.model(library)
            inline_windows(openmc, inline, filled)
            inline.export_to_model_xml(str(work / "inline-base"))
            from_file = self.model(library)
            run = work / "solver"
            DT.export_model(from_file, run / "model.xml", DT.apply_windows(openmc, from_file, labelled(filled, run), run))
            (work / "aim").mkdir()
            counts = W.cap_model(run / "model.xml", work / "aim" / "model-aimed.xml", work / "v1", [TARGET], 40000)
            W.cap_model(work / "inline-base" / "model.xml", work / "inline-capped.xml", work / "v2", [TARGET], 40000)
            _, lower, _ = W.read_window_file(work / "aim" / "windows-capped.h5")
            xml_lower, _ = inline_bounds((work / "inline-capped.xml").read_text())
            original = W.read_window_file(run / RUN_WINDOWS)[1]
        # the visibility tallies are summed by several threads, so allow for the order of the sums
        self.assertTrue(np.allclose(lower.ravel(), xml_lower, rtol=1e-6, atol=0.0))
        self.assertTrue((lower <= original).all())
        self.assertEqual(lower.shape, original.shape)
        self.assertIn("lowered", counts)


if __name__ == "__main__":
    unittest.main()
