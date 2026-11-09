"""Writes the FIRST DRAFT of `terrain.png`. It reproduces the first draft only.

    python3 draft_terrain.py [--source DIR] [--out DIR] [--force]

After the owner's session the committed `terrain.png` are this draft plus the hand edits listed in the guesswork log
of `SOURCES.md`, so **running this script again destroys those edits**: it writes into `--out` (default: next to this script) and
refuses to overwrite either file unless `--force` is given. `derive_elevation.py` stays fully reproducible.

Needs `numpy` and `Pillow`, and the `elevation.png`, `map.toml` and `terrain.toml` beside it. The Natural Earth lake files are
fetched into `--source` (default `ireland-source`) when missing and are not committed.

The derivation ports mock-up 2 (`.pipeline/coordination/map-design/mockup/build_data.py`) onto the real grid, where a cell is about
500 m at Ireland's latitude, so the mock-up's scales in cells carry over. Every field of noise is a hash-based value noise with a
fixed seed, so the same inputs give the same layers.

* **Land and sea** come from `elevation.png`: land is 1 m or more.
* **Lakes:** Natural Earth 1:10m lakes (`ne_10m_lakes`, `ne_10m_lakes_europe`), rasterised by **majority cover per cell** (the polygons
  are drawn at `LAKE_SUPERSAMPLE` times the resolution and a cell is a lake where more than half of it is covered), on land only.
  Modern reservoirs are dropped by name (`NOT_MEDIEVAL`). A feature Natural Earth classes as "Reservoir" that is not named there is kept
  and reported: Natural Earth gives Lough Mask, a natural lake, that class.
* **Terrain types:** pasture everywhere on land, then, in this order, later rules overriding earlier ones (`classify`):
  forest where the forest noise is above 0.56 and 15 m < e < 320 m; raised bog where 35 < e < 150, slope < 0.02, the bog noise above
  0.5 and more than 20 cells from water, and blanket bog where 120 < e < 330, west of 8.3 W and the bog noise above 0.42; marsh where
  e < 30, within 8 cells of water and the marsh noise above 0.45; mountain where e > 300 + (the mountain noise - 0.5) * 140. Then two
  passes of the 3 x 3 majority filter. Noise: fractal value noise (4 octaves) with base scales of 40 (mountain), 50 (bog), 30 (forest)
  and 16 (marsh) cells.
* **Usable land:** value noise `0.62 * noise(10 cells) + 0.28 * noise(3.2 cells)`, thresholded for each terrain type separately at the
  quantile that gives it its share of mock-up 2 (`USABLE_SHARE`), measured over that type's own cells so that the shares hold exactly.
"""

from __future__ import annotations

import argparse
import json
import tomllib
from pathlib import Path
from typing import NamedTuple

import numpy as np
from ireland_grid import CELL_GROUND_M, Array, Crop, download
from numpy.typing import NDArray
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent

ELEVATION_OFFSET = 11_000
NATURAL_EARTH = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/{name}.geojson"
)
LAKE_FILES = ("ne_10m_lakes", "ne_10m_lakes_europe")
# Named reservoirs and impoundments that are not medieval lakes. Poulaphouca (Blessington) dates from 1940; Inniscarra and
# Carrigadrohid are 1950s dams on the Lee (Natural Earth does not have them, the names guard against a newer file).
NOT_MEDIEVAL = ("Poulaphouca", "Blessington", "Inniscarra", "Carrigadrohid", "Pollaphuca")
LAKE_SUPERSAMPLE = 4

# The classes of `classify`, in the order of their numbers: 0 is not land.
CLASSES = ("none", "pasture", "forest", "bog", "marsh", "mountain")
# The share of each land type that is usable (mock-up 2).
USABLE_SHARE = {"pasture": 0.32, "forest": 0.28, "bog": 0.20, "marsh": 0.15, "mountain": 0.08}

# A cell is a lake where more than this share of it is covered by a lake polygon.
LAKE_COVER = 0.5

Grid = NDArray[np.bool_]
Classes = NDArray[np.uint8]


class Placement(NamedTuple):
    """The thresholds of the draft rules of `classify` (elevations in metres, distances in cells)."""

    forest_noise_above: float = 0.56
    forest_from: float = 15
    forest_to: float = 320
    raised_bog_from: float = 35
    raised_bog_to: float = 150
    raised_bog_slope_below: float = 0.02
    raised_bog_noise_above: float = 0.5
    raised_bog_distance_over: int = 20
    blanket_bog_from: float = 120
    blanket_bog_to: float = 330
    blanket_bog_west_of: float = -8.3
    blanket_bog_noise_above: float = 0.42
    marsh_below: float = 30
    marsh_distance_under: int = 8
    marsh_noise_above: float = 0.45
    mountain_line: float = 300
    mountain_wobble: float = 140


def value_noise(shape: tuple[int, int], scale: float, seed: int) -> Array:
    """Smooth value noise in 0..1 with a lattice every `scale` cells; the lattice values are a fixed hash of (seed, i, j)."""
    height, width = shape
    grid_width, grid_height = int(width / scale) + 3, int(height / scale) + 3
    i = np.arange(grid_width, dtype=np.uint64)[None, :]
    j = np.arange(grid_height, dtype=np.uint64)[:, None]
    mask = np.uint64(0xFFFFFFFF)
    h = (
        i * np.uint64(73856093) ^ j * np.uint64(19349663) ^ np.uint64(seed) * np.uint64(83492791)
    ) & mask
    for multiplier in (0x5BD1E995, 0x27D4EB2F):
        h = ((h ^ (h >> np.uint64(15))) * np.uint64(multiplier)) & mask
    lattice = (h ^ (h >> np.uint64(13))).astype(np.float64) / 4294967296.0
    u = np.arange(width) / scale
    v = np.arange(height) / scale
    column, row = np.floor(u).astype(int), np.floor(v).astype(int)
    a = (u - column)[None, :]
    b = (v - row)[:, None]
    a, b = a * a * (3 - 2 * a), b * b * (3 - 2 * b)
    c, r = column[None, :], row[:, None]
    return (
        lattice[r, c] * (1 - a) * (1 - b)
        + lattice[r, c + 1] * a * (1 - b)
        + lattice[r + 1, c] * (1 - a) * b
        + lattice[r + 1, c + 1] * a * b
    )


def fractal_noise(shape: tuple[int, int], scale: float, seed: int, octaves: int = 4) -> Array:
    """Value noise summed over octaves of halving scale and amplitude, in 0..1."""
    total, amplitude, weight = np.zeros(shape), 1.0, 0.0
    for octave in range(octaves):
        total += value_noise(shape, scale / 2**octave, seed + octave) * amplitude
        weight += amplitude
        amplitude /= 2
    return total / weight


def distance_to(water: Grid, limit: int = 80) -> NDArray[np.int16]:
    """Cells to the nearest water (4-connected steps), capped at `limit`."""
    distance = np.full(water.shape, limit, dtype=np.int16)
    distance[water] = 0
    front = water.copy()
    for step in range(1, limit):
        grown = front.copy()
        grown[1:] |= front[:-1]
        grown[:-1] |= front[1:]
        grown[:, 1:] |= front[:, :-1]
        grown[:, :-1] |= front[:, 1:]
        distance[grown & ~front] = step
        front = grown
    return distance


def majority(classes: Classes, passes: int, count: int) -> Classes:
    """The 3 x 3 majority class, `passes` times (ties go to the lowest class)."""
    for _ in range(passes):
        padded = np.pad(classes, 1, mode="edge")
        votes = np.zeros((count, *classes.shape), dtype=np.uint8)
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                window = padded[
                    1 + dy : 1 + dy + classes.shape[0], 1 + dx : 1 + dx + classes.shape[1]
                ]
                for kind in range(count):
                    votes[kind] += window == kind
        classes = np.argmax(votes, axis=0).astype(np.uint8)
    return classes


def classify(elevation: Array, land: Grid, longitude: Array) -> Classes:
    """The draft land classes (`CLASSES`), by the rules in the module documentation."""
    shape = elevation.shape
    slope = np.hypot(*np.gradient(elevation, CELL_GROUND_M))
    distance = distance_to(~land)
    forest_noise = fractal_noise(shape, 30, 31)
    bog_noise = fractal_noise(shape, 50, 21)
    marsh_noise = fractal_noise(shape, 16, 41)
    mountain_noise = fractal_noise(shape, 40, 11)
    rule = Placement()
    classes = np.ones(shape, dtype=np.uint8)
    classes[
        (forest_noise > rule.forest_noise_above)
        & (elevation > rule.forest_from)
        & (elevation < rule.forest_to)
    ] = 2
    raised = (
        (elevation > rule.raised_bog_from)
        & (elevation < rule.raised_bog_to)
        & (slope < rule.raised_bog_slope_below)
        & (bog_noise > rule.raised_bog_noise_above)
        & (distance > rule.raised_bog_distance_over)
    )
    blanket = (
        (elevation > rule.blanket_bog_from)
        & (elevation < rule.blanket_bog_to)
        & (longitude < rule.blanket_bog_west_of)
        & (bog_noise > rule.blanket_bog_noise_above)
    )
    classes[raised | blanket] = 3
    classes[
        (elevation < rule.marsh_below)
        & (distance < rule.marsh_distance_under)
        & (marsh_noise > rule.marsh_noise_above)
    ] = 4
    classes[elevation > rule.mountain_line + (mountain_noise - 0.5) * rule.mountain_wobble] = 5
    classes = majority(classes, 2, len(CLASSES))
    classes[~land] = 0
    return classes


def lake_cells(crop: Crop, source: Path) -> tuple[Grid, list[str], list[str]]:
    """The cells that a Natural Earth lake covers by more than half, the lakes dropped as modern and those classed "Reservoir" but kept."""
    scale = LAKE_SUPERSAMPLE
    image = Image.new("L", (crop.width * scale, crop.height * scale), 0)
    drawing = ImageDraw.Draw(image)
    dropped: list[str] = []
    flagged: list[str] = []
    for name in LAKE_FILES:
        path = source / f"{name}.geojson"
        download(NATURAL_EARTH.format(name=name), path)
        for feature in json.loads(path.read_text(encoding="utf-8"))["features"]:
            title = feature["properties"].get("name") or ""
            geometry = feature["geometry"]
            if geometry["type"] not in ("Polygon", "MultiPolygon"):
                continue
            polygons = (
                [geometry["coordinates"]]
                if geometry["type"] == "Polygon"
                else geometry["coordinates"]
            )
            rings = [[crop.cell_of(lat, lon) for lon, lat in polygon[0]] for polygon in polygons]
            if not any(
                0 <= x < crop.width and 0 <= y < crop.height for ring in rings for x, y in ring
            ):
                continue
            if any(word in title for word in NOT_MEDIEVAL):
                dropped.append(title)
                continue
            if feature["properties"].get("featurecla") == "Reservoir":
                flagged.append(title or "(unnamed)")
            for ring in rings:
                drawing.polygon([(x * scale, y * scale) for x, y in ring], fill=1)
    covered = (
        np.asarray(image, dtype=np.float64)
        .reshape(crop.height, scale, crop.width, scale)
        .mean(axis=(1, 3))
    )
    return covered > LAKE_COVER, sorted(set(dropped)), sorted(set(flagged))


def usable_cells(classes: Classes) -> Grid:
    """Usable land: fixed noise under a per-type threshold that gives the type its `USABLE_SHARE`."""
    noise = 0.62 * value_noise(classes.shape, 10, 7) + 0.28 * value_noise(classes.shape, 3.2, 8)
    usable = np.zeros(classes.shape, dtype=bool)
    for index, name in enumerate(CLASSES):
        cells = classes == index
        if name in USABLE_SHARE and cells.any():
            usable[cells] = noise[cells] <= np.quantile(noise[cells], USABLE_SHARE[name])
    return usable


def terrain_keys(path: Path) -> dict[str, tuple[int, int, int]]:
    """The key colour of each terrain name from `terrain.toml`."""
    rules = tomllib.loads(path.read_text(encoding="utf-8"))
    return {entry["name"]: _colour(entry["key"]) for entry in rules["terrain"]}


def _colour(text: str) -> tuple[int, int, int]:
    return int(text[0:2], 16), int(text[2:4], 16), int(text[4:6], 16)


def draft(source: Path, out: Path, *, force: bool) -> None:
    """Writes the initial `terrain.png`; computed fertility is added by `apply_edits.py`."""
    targets = [out / "terrain.png"]
    existing = [target.name for target in targets if target.exists()]
    if existing and not force:
        message = f"{', '.join(existing)} already exist in {out} (the committed copies carry hand edits); pass --force to overwrite"
        raise SystemExit(message)
    crop = Crop.of_map_toml(out / "map.toml")
    with Image.open(out / "elevation.png") as image:
        elevation = np.asarray(image, dtype=np.float64) - ELEVATION_OFFSET
    if elevation.shape != (crop.height, crop.width):
        message = f"elevation.png is {elevation.shape}, map.toml says {(crop.height, crop.width)}"
        raise SystemExit(message)
    longitude, _latitude = crop.centres()
    land = elevation >= 1
    lake, dropped, flagged = lake_cells(crop, source)
    lake &= land
    classes = classify(elevation, land & ~lake, longitude)
    keys = terrain_keys(out / "terrain.toml")
    terrain = np.zeros((*classes.shape, 3), dtype=np.uint8)
    for index, name in enumerate(CLASSES):
        terrain[classes == index] = keys["sea" if name == "none" else name]
    terrain[lake] = keys["lake"]
    Image.fromarray(terrain).save(targets[0])
    (out / "edits.applied").unlink(missing_ok=True)
    report(classes, lake, (dropped, flagged))


def report(classes: Classes, lake: Grid, lakes: tuple[list[str], list[str]]) -> None:
    """Prints the share of each class: the number to compare with mock-up 2."""
    land = int((classes > 0).sum()) - int((lake & (classes > 0)).sum())
    print(f"land cells (not lake) {land}; lake cells {int(lake.sum())}")
    for index, name in enumerate(CLASSES[1:], start=1):
        cells = int(((classes == index) & ~lake).sum())
        print(f"  {name}: {cells} ({100 * cells / max(land, 1):.1f} % of land)")
    print("lakes dropped as modern:", ", ".join(lakes[0]) or "none")
    print('lakes Natural Earth classes "Reservoir" and kept:', ", ".join(lakes[1]) or "none")


def main() -> None:
    """Parses the command line and drafts."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", maxsplit=1)[0])
    parser.add_argument(
        "--source", type=Path, default=Path("ireland-source"), help="download cache directory"
    )
    parser.add_argument("--out", type=Path, default=HERE, help="where the layers are written")
    parser.add_argument("--force", action="store_true", help="overwrite existing files")
    arguments = parser.parse_args()
    draft(arguments.source, arguments.out, force=arguments.force)


if __name__ == "__main__":
    main()
