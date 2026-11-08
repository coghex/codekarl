"""Writes the images of the cut-corner frames (milestone 8, slice 7b), next to this script.

    python3 make_theme.py            write the images
    python3 make_theme.py --check    fail if a committed image differs from what this script writes, or is missing or extra

`white.png` is one opaque white pixel: the frames' bands and edges are stretched copies of it, tinted by the theme. For each cut
`c` in 3, 6 and 7 there are four tiles of `(c + 1) x (c + 1)` pixels, opaque white or transparent:

- `cut<c>_tl_line.png` and `cut<c>_tl_fill.png`, the top-left corner: pixel `(i, j)` is line where `i + j == c`, fill where
  `i + j > c`, and transparent otherwise;
- `cut<c>_br_line.png` and `cut<c>_br_fill.png`, the bottom-right corner: line where `i + j == c`, fill where `i + j < c`, and
  transparent otherwise.

The PNG writer is the pixel font's (`assets/ui/font/make_font.py`), so the output is deterministic in the same way.
"""

import importlib.util
import sys
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent

Pixel = tuple[int, int, int, int]
WHITE: Pixel = (255, 255, 255, 255)
CLEAR: Pixel = (0, 0, 0, 0)
CUTS = (3, 6, 7)


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


def tile(cut: int, corner: str, part: str) -> list[list[Pixel]]:
    """The rows of one tile: `corner` is "tl" or "br", `part` is "line" or "fill"."""
    rows = []
    for j in range(cut + 1):
        row = []
        for i in range(cut + 1):
            line = i + j == cut
            fill = i + j > cut if corner == "tl" else i + j < cut
            row.append(WHITE if (line if part == "line" else fill) else CLEAR)
        rows.append(row)
    return rows


def images() -> dict[Path, bytes]:
    """Every image the theme has, by path."""
    png = font_png()
    found = {HERE / "white.png": png([[WHITE]])}
    for cut in CUTS:
        for corner in ("tl", "br"):
            for part in ("fill", "line"):
                found[HERE / f"cut{cut}_{corner}_{part}.png"] = png(tile(cut, corner, part))
    return found


def main(arguments: list[str]) -> int:
    wanted = images()
    if arguments == ["--check"]:
        problems = [
            f"{path} differs from what {Path(__file__).name} writes, or is missing"
            for path, data in sorted(wanted.items())
            if not path.exists() or path.read_bytes() != data
        ]
        problems += [
            f"{path} is not an image of the theme"
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
