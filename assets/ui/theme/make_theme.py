"""Writes the images of the cut-corner frames and the time-bar controls (milestone 8, slices 7b/7c/owner follow-up).

    python3 make_theme.py            write the images
    python3 make_theme.py --check    fail if a committed image differs from what this script writes, or is missing or extra

`white.png` is one opaque white pixel: the frames' bands and edges are stretched copies of it, tinted by the theme. `speed_box.png`
is the one-pixel rounded outline of a 6 by 11 speed indicator. The `icon_*.png` files are white 11-row glyphs: four for the time bar, two for the map modes and two (`filter`, `sort`) for the list bar; `sort_down.png` and `sort_up.png` are its 5 by 5 direction marks. For each cut
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
ICON_HEIGHT = 11
ICONS = {
    "rewind": (
        9,
        ("...#...#", "..##..##", ".###.###", "########", ".###.###", "..##..##", "...#...#"),
    ),
    "fast_forward": (
        9,
        ("#...#...", "##..##..", "###.###.", "########", "###.###.", "##..##..", "#...#..."),
    ),
    "pause": (7, ("##.##.",) * 7),
    "play": (7, ("#.....", "##....", "###...", "####..", "###...", "##....", "#.....")),
    # The map modes' buttons (milestone 9, slice 8): a barrel of stores, and three bars that rise (the yield).
    "stockpile": (
        9,
        ("..####..", ".######.", "########", "########", "########", ".######.", "..####.."),
    ),
    "yield": (
        9,
        ("......##", "......##", "...##.##", "...##.##", "##.##.##", "##.##.##", "##.##.##"),
    ),
    # The elevation mode's button (slice 10b): a peak.
    "elevation": (
        9,
        ("...##...", "..####..", "..####..", ".######.", ".######.", "########", "########"),
    ),
    # The owner-approved ledger tab icons are the exact 9x11 mock-up rows.
    "location": (
        9,
        (
            "....#....",
            "...###...",
            "..#####..",
            "..##.##..",
            "..#####..",
            "..#####..",
            "..##.##..",
            "..#####..",
            "..#####..",
            "..##.##..",
            ".#######.",
        ),
    ),
    "pops": (
        9,
        (
            "...###...",
            "...###...",
            "...###...",
            "....#....",
            ".#######.",
            "#.#####.#",
            "#.#####.#",
            "..#####..",
            "..##.##..",
            "..##.##..",
            "..##.##..",
        ),
    ),
    # CK-259's list-look selector: two stacked cards, and three text rows.
    "cards": (
        9,
        (
            ".........",
            ".........",
            ".#######.",
            ".#.....#.",
            ".#######.",
            ".........",
            ".#######.",
            ".#.....#.",
            ".#######.",
            ".........",
            ".........",
        ),
    ),
    "rows": (
        9,
        (
            ".........",
            ".........",
            "##.######",
            ".........",
            ".........",
            "##.######",
            ".........",
            ".........",
            "##.######",
            ".........",
            ".........",
        ),
    ),
    "farm": (
        9,
        (
            ".........",
            "....#....",
            "...###...",
            "..#####..",
            ".#######.",
            "#########",
            ".#.....#.",
            ".#.###.#.",
            ".#.#.#.#.",
            ".#.#.#.#.",
            ".#######.",
        ),
    ),
    "holdings": (
        9,
        (
            "....#....",
            "...###...",
            "....#....",
            "...###...",
            "..#####..",
            ".#######.",
            ".#.....#.",
            ".#..#..#.",
            ".#.###.#.",
            ".#.###.#.",
            ".#######.",
        ),
    ),
    "storehouse": (
        9,
        (
            ".........",
            ".........",
            ".#######.",
            "#########",
            "#.......#",
            "#.#####.#",
            "#.......#",
            "#.#####.#",
            "#.......#",
            "#########",
            ".........",
        ),
    ),
    # The lists' two buttons (CK-295; owner-approved, docs/ui-art-provenance.md): a solid funnel, and an arrow down beside four bars.
    "filter": (
        9,
        (
            ".........",
            "#########",
            ".#######.",
            "..#####..",
            "...###...",
            "...###...",
            "...###...",
            "...###...",
            "...##....",
            ".........",
            ".........",
        ),
    ),
    "sort": (
        9,
        (
            ".........",
            ".#..#####",
            ".#.......",
            ".#..####.",
            ".#.......",
            ".#..###..",
            ".#.......",
            "###.##...",
            ".#.......",
            ".........",
            ".........",
        ),
    ),
}

# The sort picker's direction marks: 5 by 5, white on clear, an arrow with a one-pixel stem and a five-wide head.
MARKS = {
    "sort_down": ("..#..", "..#..", "#.#.#", ".###.", "..#.."),
    "sort_up": ("..#..", ".###.", "#.#.#", "..#..", "..#.."),
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


def icon(width: int, shape: tuple[str, ...]) -> list[list[Pixel]]:
    """An 11-row glyph; ledger glyphs are full shapes, the other icons have seven padded rows."""
    rows = [[CLEAR for _ in range(width)] for _ in range(ICON_HEIGHT)]
    full_shape = len(shape) == ICON_HEIGHT
    for y, pattern in enumerate(shape):
        for x, pixel in enumerate(pattern):
            if pixel == "#":
                rows[y if full_shape else y + 2][x if full_shape else x + 1] = WHITE
    return rows


def mark(shape: tuple[str, ...]) -> list[list[Pixel]]:
    """A 5 by 5 direction mark."""
    return [[WHITE if pixel == "#" else CLEAR for pixel in row] for row in shape]


def speed_box() -> list[list[Pixel]]:
    """A 6 by 11 one-pixel outline with transparent corner pixels."""
    return [
        [
            WHITE
            if (y in (0, 10) and x in (1, 2, 3, 4)) or (x in (0, 5) and y in range(1, 10))
            else CLEAR
            for x in range(6)
        ]
        for y in range(11)
    ]


def images() -> dict[Path, bytes]:
    """Every image the theme has, by path."""
    png = font_png()
    found = {HERE / "white.png": png([[WHITE]])}
    found[HERE / "speed_box.png"] = png(speed_box())
    for name, (width, shape) in ICONS.items():
        found[HERE / f"icon_{name}.png"] = png(icon(width, shape))
    for name, shape in MARKS.items():
        found[HERE / f"{name}.png"] = png(mark(shape))
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
