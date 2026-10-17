#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Contrast of what was actually drawn, sampled from screenshots (A11Y-010, A11Y-011 method).

    python3 scripts/screenshot_contrast.py shot.png [more.png ...] [--json out.json]

The palette test in crates/faris-app/src/theme.rs checks the colours the app asks for. This
checks the pixels it drew. For every background colour that covers a large area, it finds the
small shapes drawn on it (glyphs, icons, markers, thin borders): connected groups of other
pixels, enclosed by that background, no larger than MAX_SHAPE pixels. Each shape's core is its
pixel of highest contrast against the background (edges are anti-aliased, the core is the drawn
colour), and the WCAG 2.2 ratio of core to background is the shape's contrast. Shapes of fewer
than MIN_SHAPE pixels are anti-aliasing specks and are skipped.

Text needs 4.5 to 1 and graphical objects 3 to 1. A shape is not known to be text or graphics,
so both counts are reported; read the listed shapes below 4.5 to see which they are. Exit status
1 when any shape is below 3 to 1.
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path

from PIL import Image

MIN_SHAPE = 4
MAX_SHAPE = 3000
BACKGROUND_SHARE = 0.01
ENCLOSED_SHARE = 0.8


def luminance(rgb: tuple[int, int, int]) -> float:
    def channel(c: int) -> float:
        s = c / 255.0
        return s / 12.92 if s <= 0.04045 else ((s + 0.055) / 1.055) ** 2.4

    r, g, b = (channel(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def ratio(a: tuple[int, int, int], b: tuple[int, int, int]) -> float:
    la, lb = luminance(a), luminance(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def shapes_on(pixels: list[tuple[int, int, int]], width: int, height: int, background: tuple[int, int, int]) -> list[dict]:
    """The small enclosed shapes drawn on `background`, each with its core colour and ratio."""
    seen = bytearray(width * height)
    out = []
    for start in range(width * height):
        if seen[start] or pixels[start] == background:
            continue
        stack, members, too_big = [start], [], False
        seen[start] = 1
        while stack:
            i = stack.pop()
            if not too_big:
                members.append(i)
                too_big = len(members) > MAX_SHAPE
            x, y = i % width, i // width
            for j, ok in ((i - 1, x > 0), (i + 1, x < width - 1), (i - width, y > 0), (i + width, y < height - 1)):
                if ok and not seen[j] and pixels[j] != background:
                    seen[j] = 1
                    stack.append(j)
        if too_big or len(members) < MIN_SHAPE:
            continue
        xs = [i % width for i in members]
        ys = [i // width for i in members]
        x0, x1, y0, y1 = min(xs) - 1, max(xs) + 1, min(ys) - 1, max(ys) + 1
        if x0 < 0 or y0 < 0 or x1 >= width or y1 >= height:
            continue
        ring = [(x, y0) for x in range(x0, x1 + 1)] + [(x, y1) for x in range(x0, x1 + 1)]
        ring += [(x0, y) for y in range(y0 + 1, y1)] + [(x1, y) for y in range(y0 + 1, y1)]
        if sum(pixels[y * width + x] == background for x, y in ring) < ENCLOSED_SHARE * len(ring):
            continue
        core = max((pixels[i] for i in members), key=lambda c: ratio(c, background))
        out.append({"bbox": [x0 + 1, y0 + 1, x1 - 1, y1 - 1], "pixels": len(members), "core": list(core), "ratio": round(ratio(core, background), 2)})
    return out


def sample(path: Path) -> dict:
    image = Image.open(path).convert("RGB")
    width, height = image.size
    pixels = list(image.getdata())
    counts = Counter(pixels)
    backgrounds = [c for c, n in counts.most_common() if n >= BACKGROUND_SHARE * width * height]
    report = {"file": str(path), "size": [width, height], "backgrounds": []}
    for background in backgrounds:
        shapes = shapes_on(pixels, width, height, background)
        low = sorted((s for s in shapes if s["ratio"] < 4.5), key=lambda s: s["ratio"])
        report["backgrounds"].append({
            "colour": list(background),
            "share": round(counts[background] / (width * height), 3),
            "shapes": len(shapes),
            "lowest": min((s["ratio"] for s in shapes), default=None),
            "below_4_5": len(low),
            "below_3": sum(s["ratio"] < 3 for s in shapes),
            "low_shapes": low[:20],
        })
    return report


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="sample drawn contrast from screenshots")
    ap.add_argument("images", nargs="+", type=Path)
    ap.add_argument("--json", type=Path, help="write the full report here")
    args = ap.parse_args(argv)
    reports = [sample(p) for p in args.images]
    failing = 0
    for r in reports:
        print(r["file"])
        for b in r["backgrounds"]:
            failing += b["below_3"]
            print(f"  background {tuple(b['colour'])} ({b['share']:.0%}): {b['shapes']} shapes, lowest {b['lowest']}, "
                  f"{b['below_4_5']} below 4.5, {b['below_3']} below 3")
            for s in b["low_shapes"][:5]:
                print(f"    {s['ratio']} core {tuple(s['core'])} at {s['bbox']} ({s['pixels']} px)")
    if args.json:
        args.json.write_text(json.dumps(reports, indent=2) + "\n", encoding="utf-8")
    return 1 if failing else 0


if __name__ == "__main__":
    sys.exit(main())
