"""Writes the three placeholder background images of a button, next to this script.

    python3 make_buttons.py            write the images
    python3 make_buttons.py --check    fail if a committed image differs from what this script writes, or is missing or extra

Each image is one opaque pixel, stretched over the button: `normal.png` is the panel colour `#2a2119`, `hover.png` is `#3b2e23`
and `pressed.png` is `#1d1712`. **The colours are placeholders** until the owner describes the look (milestone 8) and slice 7 (the
theme) lands. The PNG writer is the pixel font's, so the output is deterministic in the same way.
"""

import importlib.util
import sys
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent

Pixel = tuple[int, int, int, int]
COLOURS: dict[str, Pixel] = {
    "normal.png": (0x2A, 0x21, 0x19, 255),
    "hover.png": (0x3B, 0x2E, 0x23, 255),
    "pressed.png": (0x1D, 0x17, 0x12, 255),
}


def font_png() -> Callable[[list[list[Pixel]]], bytes]:
    """The pixel font's PNG writer, loaded from `assets/ui/font/make_font.py`."""
    path = HERE.parent / "font" / "make_font.py"
    spec = importlib.util.spec_from_file_location("make_font", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    writer: Callable[[list[list[Pixel]]], bytes] = module.png
    return writer


def images() -> dict[Path, bytes]:
    """Every image a button has, by path."""
    png = font_png()
    return {HERE / name: png([[pixel]]) for name, pixel in COLOURS.items()}


def main(arguments: list[str]) -> int:
    wanted = images()
    if arguments == ["--check"]:
        problems = [
            f"{path} differs from what {Path(__file__).name} writes, or is missing"
            for path, data in sorted(wanted.items())
            if not path.exists() or path.read_bytes() != data
        ]
        problems += [
            f"{path} is not an image of {Path(__file__).name}"
            for path in sorted(HERE.glob("*.png"))
            if path not in wanted
        ]
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1 if problems else 0
    if arguments:
        print(f"usage: {Path(__file__).name} [--check]", file=sys.stderr)
        return 2
    for path, data in wanted.items():
        path.write_bytes(data)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
