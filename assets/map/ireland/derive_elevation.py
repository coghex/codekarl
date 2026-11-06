"""Derives `elevation.png` and `map.toml` of Ireland's map from AWS Terrain Tiles. Fully reproducible.

    python3 derive_elevation.py [--source DIR] [--out DIR] [--force]

Needs `numpy` and `Pillow`. The terrarium tiles are fetched into `--source` (default `ireland-source` in the current directory) when
missing and are **not committed**. Writes `map.toml` and `elevation.png` into `--out` (default: next to this script) and refuses to
overwrite either unless `--force` is given. Every parameter below is stated in `SOURCES.md`.

* **The crop** (`ireland_grid.CROP_*`): latitude 51.25 to 55.55 N, longitude 10.90 W to 5.30 W, in whole cells of the world grid
  (839 projected metres; `docs/map-design.md` section 9.3).
* **The samples:** AWS Terrain Tiles, terrarium encoding, zoom `TILE_ZOOM` (a pixel is 152.9 projected metres, so a cell holds about
  30 sample centres). A sample belongs to the cell its **centre** falls in.
* **Land and sea per cell:** a cell is land where **more than half of its samples are above 0 m**.
* **Elevation per cell:** for a land cell the mean of its land samples, for a sea cell the mean of its sea samples (so coastal land is
  not averaged down towards 0 m, nor coastal sea up, which would flatten the dithering at the outline and lose the shallow-sea tone),
  **rounded to whole metres**. A land cell is at least 1 m and a sea cell at most 0 m, so the land mask is recoverable from
  `elevation.png` alone (elevation 1 m or more). Depths below sea level are kept: the shallow-sea tone reads them.
* **Foreign land:** land that is not Ireland's (the Mull of Kintyre and the Scottish islands to the north-east, Pembrokeshire) is
  painted sea: its cells are given the depth `FOREIGN_SEA_DEPTH`. The rule is two boxes of latitude and longitude, `FOREIGN_BOXES`.
* The 16-bit value stored is `elevation + 11000`.
"""

from __future__ import annotations

import argparse
import math
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from ireland_grid import (
    CELL_M,
    HALF_WORLD_M,
    Array,
    Crop,
    download,
)
from PIL import Image

HERE = Path(__file__).resolve().parent

TILE_ZOOM = 10
TILE_SIZE = 256
TILE_URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png"
TILE_PIXEL_M = 2 * HALF_WORLD_M / (TILE_SIZE * 2**TILE_ZOOM)

# Elevation is stored as `metres + 11000` in 16 bits.
ELEVATION_OFFSET = 11_000
# Foreign land painted as sea is given this depth, in metres: open sea, past the shallow-sea threshold.
FOREIGN_SEA_DEPTH = 80
# Boxes of (south, north, west, east) degrees. Kintyre, Arran and the islands to the north-east; Pembrokeshire.
FOREIGN_BOXES = ((55.25, 90.0, -5.95, 180.0), (-90.0, 52.2, -5.4, 180.0))


class Cells:
    """Per-cell sums over the crop of the samples whose centres fall in each cell."""

    def __init__(self, crop: Crop) -> None:
        self.crop = crop
        size = crop.width * crop.height
        self.count = np.zeros(size)
        self.land_count = np.zeros(size)
        self.land_sum = np.zeros(size)
        self.sea_sum = np.zeros(size)

    def add_tile(self, tile_x: int, tile_y: int, metres: Array) -> None:
        """Adds one decoded tile of samples."""
        crop = self.crop
        pixel = np.arange(TILE_SIZE)
        cell_x = (
            np.floor((tile_x * TILE_SIZE + pixel + 0.5) * TILE_PIXEL_M / CELL_M).astype(int)
            - crop.origin[0]
        )
        cell_y = (
            np.floor((tile_y * TILE_SIZE + pixel + 0.5) * TILE_PIXEL_M / CELL_M).astype(int)
            - crop.origin[1]
        )
        inside_x = (cell_x >= 0) & (cell_x < crop.width)
        inside_y = (cell_y >= 0) & (cell_y < crop.height)
        if not inside_x.any() or not inside_y.any():
            return
        flat = (cell_y[inside_y][:, None] * crop.width + cell_x[inside_x][None, :]).ravel()
        samples = metres[np.ix_(inside_y, inside_x)].ravel()
        size = crop.width * crop.height
        land = samples > 0
        self.count += np.bincount(flat, minlength=size)
        self.land_count += np.bincount(flat[land], minlength=size)
        self.land_sum += np.bincount(flat[land], weights=samples[land], minlength=size)
        self.sea_sum += np.bincount(flat[~land], weights=samples[~land], minlength=size)

    def elevation(self) -> tuple[Array, Array]:
        """The rounded elevation of every cell (rows first) and the land mask as 0 or 1."""
        if (self.count == 0).any():
            message = "a cell has no sample centre: the cell is narrower than a pixel"
            raise ValueError(message)
        land = self.land_count * 2 > self.count
        sea_count = self.count - self.land_count
        land_mean = self.land_sum / np.maximum(self.land_count, 1)
        sea_mean = self.sea_sum / np.maximum(sea_count, 1)
        mean = np.where(land, land_mean, sea_mean)
        rounded = np.where(land, np.maximum(np.rint(mean), 1), np.minimum(np.rint(mean), 0))
        shape = (self.crop.height, self.crop.width)
        return rounded.reshape(shape), land.reshape(shape)


def read_tile(path: Path) -> Array:
    """A terrarium tile decoded to metres."""
    with Image.open(path) as image:
        rgb = np.asarray(image.convert("RGB"), dtype=np.float64)
    return rgb[..., 0] * 256 + rgb[..., 1] + rgb[..., 2] / 256 - 32768


def tile_range(crop: Crop) -> tuple[range, range]:
    """The tile columns and rows that hold the crop's sample centres."""
    first = [math.floor(crop.origin[axis] * CELL_M / TILE_PIXEL_M) // TILE_SIZE for axis in (0, 1)]
    last = [
        math.floor((crop.origin[axis] + (crop.width, crop.height)[axis]) * CELL_M / TILE_PIXEL_M)
        // TILE_SIZE
        for axis in (0, 1)
    ]
    return range(first[0], last[0] + 1), range(first[1], last[1] + 1)


def fetch_tiles(crop: Crop, source: Path) -> list[tuple[int, int, Path]]:
    """Downloads the missing tiles of the crop (a few at a time) and lists them all."""
    columns, rows = tile_range(crop)
    tiles = [(x, y, source / "tiles" / f"{TILE_ZOOM}_{x}_{y}.png") for x in columns for y in rows]

    def fetch(tile: tuple[int, int, Path]) -> None:
        download(TILE_URL.format(z=TILE_ZOOM, x=tile[0], y=tile[1]), tile[2])

    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(fetch, tiles))
    return tiles


def paint_foreign_land_as_sea(crop: Crop, elevation: Array) -> int:
    """Gives the cells of the foreign boxes the depth `FOREIGN_SEA_DEPTH` where they are above it. Returns how many were land."""
    longitude, latitude = crop.centres()
    inside = np.zeros(elevation.shape, dtype=bool)
    for south, north, west, east in FOREIGN_BOXES:
        inside |= (latitude > south) & (latitude < north) & (longitude > west) & (longitude < east)
    was_land = int(((elevation >= 1) & inside).sum())
    elevation[inside & (elevation > -FOREIGN_SEA_DEPTH)] = -FOREIGN_SEA_DEPTH
    return was_land


def map_toml(crop: Crop) -> str:
    """The text of `map.toml`."""
    return (
        'projection = "spherical_mercator"\n'
        f"cell_metres = {CELL_M}\n\n"
        "[crop]\n"
        f"origin = [{crop.origin[0]}, {crop.origin[1]}]\n"
        f"width = {crop.width}\n"
        f"height = {crop.height}\n\n"
        "[layers]\n"
        'terrain = "terrain.png"\n'
        'usable = "usable.png"\n'
        'elevation = "elevation.png"\n'
        'terrain_rules = "terrain.toml"\n'
        'rivers = "rivers.toml"\n'
    )


def derive(source: Path, out: Path, *, force: bool) -> None:
    """Writes `map.toml` and `elevation.png`."""
    targets = [out / "map.toml", out / "elevation.png"]
    existing = [target.name for target in targets if target.exists()]
    if existing and not force:
        message = f"{', '.join(existing)} already exist in {out}; pass --force to overwrite"
        raise SystemExit(message)
    crop = Crop.of_box()
    cells = Cells(crop)
    for tile_x, tile_y, path in fetch_tiles(crop, source):
        cells.add_tile(tile_x, tile_y, read_tile(path))
    elevation, land = cells.elevation()
    foreign = paint_foreign_land_as_sea(crop, elevation)
    stored = np.clip(elevation + ELEVATION_OFFSET, 0, 65535).astype(np.uint16)
    Image.fromarray(stored).save(targets[1])
    targets[0].write_text(map_toml(crop), encoding="utf-8")
    (out / "edits.applied").unlink(missing_ok=True)
    kept = int(((elevation >= 1) & land).sum())
    print(f"crop origin {crop.origin} size {crop.width} x {crop.height}; tiles zoom {TILE_ZOOM}")
    print(f"land cells {kept}, foreign land painted as sea {foreign}")
    print(f"highest {int(elevation.max())} m, deepest {int(elevation.min())} m")


def main() -> None:
    """Parses the command line and derives."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", maxsplit=1)[0])
    parser.add_argument(
        "--source", type=Path, default=Path("ireland-source"), help="download cache directory"
    )
    parser.add_argument("--out", type=Path, default=HERE, help="where the files are written")
    parser.add_argument("--force", action="store_true", help="overwrite existing files")
    arguments = parser.parse_args()
    derive(arguments.source, arguments.out, force=arguments.force)


if __name__ == "__main__":
    main()
