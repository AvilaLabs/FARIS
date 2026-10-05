#!/usr/bin/env python3
"""Assemble recorded FARIS frames into a looping GIF with one global palette.

    make_readme_gif.py FRAMES_DIR OUTPUT.gif [--width 960] [--fps 12] [--max-bytes 8000000]

FRAMES_DIR comes from `faris-app --record-frames`. If it holds frames.json
(capture times), output frames are picked by real time so slow recording
frames do not speed the animation up; otherwise frames are skipped evenly
from the recorded rate. If the GIF exceeds --max-bytes the width steps down
(960, 800, 720, 640) and the build is retried.
"""
import argparse
import json
import sys
from pathlib import Path

from PIL import Image

WIDTHS = [960, 800, 720, 640]
PALETTE_SAMPLES = 24


def list_frames(directory):
    frames = sorted(Path(directory).glob("frame-*.png"))
    if not frames:
        raise SystemExit(f"no frame-*.png files in {directory}")
    return frames


def select_frames(frames, directory, fps, source_fps=15.0):
    """Indices of frames to keep so the output plays at `fps` in real time."""
    manifest = Path(directory) / "frames.json"
    times = None
    if manifest.is_file():
        entries = json.loads(manifest.read_text())["frames"]
        if len(entries) == len(frames):
            times = [e["t"] for e in entries]
    if times is None:
        step = source_fps / fps
        count = max(1, int(len(frames) / step))
        return [min(len(frames) - 1, round(i * step)) for i in range(count)]
    picked, tick = [], 0
    while tick / fps <= times[-1]:
        target = tick / fps
        picked.append(min(range(len(times)), key=lambda i: abs(times[i] - target)))
        tick += 1
    return picked


def resized(path, width):
    image = Image.open(path).convert("RGB")
    height = round(image.height * width / image.width)
    return image.resize((width, height), Image.LANCZOS)


def build(frames, picked, output, width, fps):
    images = [resized(frames[i], width) for i in picked]
    # One adaptive palette from an even sample of frames, so colours are stable.
    stride = max(1, len(images) // PALETTE_SAMPLES)
    sample = images[::stride][:PALETTE_SAMPLES]
    sheet = Image.new("RGB", (width, sum(s.height for s in sample)))
    y = 0
    for s in sample:
        sheet.paste(s, (0, y))
        y += s.height
    palette = sheet.quantize(colors=255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    frames_p = [im.quantize(palette=palette, dither=Image.Dither.NONE) for im in images]
    frames_p[0].save(
        output,
        save_all=True,
        append_images=frames_p[1:],
        duration=round(1000 / fps),
        loop=0,
        optimize=True,
        disposal=1,
    )
    return len(frames_p), len(frames_p) / fps


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("frames_dir")
    parser.add_argument("output")
    parser.add_argument("--width", type=int, default=960)
    parser.add_argument("--fps", type=float, default=12.0)
    parser.add_argument("--max-bytes", type=int, default=8_000_000)
    args = parser.parse_args(argv)

    frames = list_frames(args.frames_dir)
    picked = select_frames(frames, args.frames_dir, args.fps)
    widths = [args.width] + [w for w in WIDTHS if w < args.width]
    for width in widths:
        count, seconds = build(frames, picked, args.output, width, args.fps)
        size = Path(args.output).stat().st_size
        print(f"width {width}: {size} bytes, {count} frames, {seconds:.1f} s")
        if size <= args.max_bytes:
            return 0
    Path(args.output).unlink(missing_ok=True)
    print(
        f"error: GIF still over {args.max_bytes} bytes at width {widths[-1]}; "
        "shorten the plan, lower --fps, or raise --max-bytes",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
