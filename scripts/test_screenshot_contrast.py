# SPDX-License-Identifier: AGPL-3.0-only
import tempfile
import unittest
from pathlib import Path

from PIL import Image, ImageDraw

import screenshot_contrast as sc


def shot(text_colour, background=(37, 39, 44), size=(120, 60)):
    image = Image.new("RGB", size, background)
    draw = ImageDraw.Draw(image)
    draw.rectangle([20, 20, 23, 35], fill=text_colour)  # a stem
    draw.rectangle([40, 20, 49, 22], fill=text_colour)  # a bar
    return image


class ScreenshotContrastTest(unittest.TestCase):
    def test_ratio_matches_wcag_reference_values(self):
        self.assertAlmostEqual(sc.ratio((0, 0, 0), (255, 255, 255)), 21.0, places=6)
        self.assertAlmostEqual(sc.ratio((119, 119, 119), (255, 255, 255)), 4.48, places=2)

    def test_shapes_on_a_background_report_their_core_contrast(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "a.png"
            shot((190, 190, 190)).save(path)
            report = sc.sample(path)
        background = report["backgrounds"][0]
        self.assertEqual(background["colour"], [37, 39, 44])
        self.assertEqual(background["shapes"], 2)
        self.assertAlmostEqual(background["lowest"], round(sc.ratio((190, 190, 190), (37, 39, 44)), 2))
        self.assertEqual(background["below_3"], 0)

    def test_low_contrast_shapes_are_listed_and_fail_the_run(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "b.png"
            shot((60, 60, 60)).save(path)
            self.assertEqual(sc.main([str(path)]), 1)
            report = sc.sample(path)
        self.assertEqual(report["backgrounds"][0]["below_3"], 2)

    def test_specks_and_regions_that_are_not_enclosed_are_skipped(self):
        image = Image.new("RGB", (60, 60), (37, 39, 44))
        draw = ImageDraw.Draw(image)
        draw.point((10, 10), fill=(60, 60, 60))  # one-pixel anti-aliasing speck
        draw.rectangle([0, 30, 59, 59], fill=(50, 50, 50))  # another panel, touching the edge
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "c.png"
            image.save(path)
            report = sc.sample(path)
        self.assertTrue(all(b["shapes"] == 0 for b in report["backgrounds"]))

    def test_a_near_background_stripe_is_fill_not_a_failing_shape(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "d.png"
            shot((38, 40, 44)).save(path)
            self.assertEqual(sc.main([str(path)]), 0)
            background = sc.sample(path)["backgrounds"][0]
        self.assertEqual(background["shapes"], 0)
        self.assertEqual(background["fill_pieces"], 2)


    def test_a_decoration_background_is_reported_but_does_not_fail(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / "e.png"
            shot((60, 60, 60)).save(path)
            self.assertEqual(sc.main([str(path), "--skip-background", "37,39,44"]), 0)
            self.assertEqual(sc.main([str(path)]), 1)


if __name__ == "__main__":
    unittest.main()
