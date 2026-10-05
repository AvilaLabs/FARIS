import json
import tempfile
import unittest
from pathlib import Path

from PIL import Image

import make_readme_gif as gif


def synthetic(directory, count=10, size=(200, 100), timed=False):
    for i in range(count):
        image = Image.new("RGB", size, (20 + i * 20, 40, 90))
        image.paste((250, 250, 250), (i * 10, 10, i * 10 + 20, 40))
        image.save(directory / f"frame-{i:05d}.png")
    if timed:
        entries = [{"file": f"frame-{i:05d}.png", "t": i * 0.1} for i in range(count)]
        (directory / "frames.json").write_text(json.dumps({"frames": entries}))


class MakeGif(unittest.TestCase):
    def test_assembles_looping_gif_of_requested_width(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            synthetic(d)
            out = d / "out.gif"
            self.assertEqual(gif.main([str(d), str(out), "--width", "100", "--fps", "5"]), 0)
            image = Image.open(out)
            self.assertEqual(image.width, 100)
            self.assertEqual(image.info.get("loop"), 0)
            self.assertGreater(image.n_frames, 1)

    def test_time_based_selection_resamples_by_capture_time(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            synthetic(d, count=10, timed=True)
            picked = gif.select_frames(gif.list_frames(d), d, fps=5)
            self.assertEqual(picked, [0, 2, 4, 6, 8])

    def test_rate_based_skipping_without_manifest(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            synthetic(d, count=30)
            picked = gif.select_frames(gif.list_frames(d), d, fps=5, source_fps=15.0)
            self.assertEqual(len(picked), 10)

    def test_oversize_fails_clearly_and_leaves_no_file(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            synthetic(d)
            out = d / "out.gif"
            self.assertEqual(gif.main([str(d), str(out), "--width", "800", "--max-bytes", "10"]), 1)
            self.assertFalse(out.exists())

    def test_empty_directory_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(SystemExit):
                gif.main([d, str(Path(d) / "o.gif")])


if __name__ == "__main__":
    unittest.main()
