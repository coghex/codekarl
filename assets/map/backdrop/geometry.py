"""World backdrop geometry and Natural Earth land sampling."""

import json
import math
from collections.abc import Iterator
from pathlib import Path

import numpy as np
import numpy.typing as npt

RADIUS = 6_378_137.0
CELL = 839.0
HALF = math.pi * RADIUS
WORLD = 2 * HALF
WORLD_CELLS = math.ceil(WORLD / CELL)
MIN_BLOCK, MAX_BLOCK = 16, 8192
LAND_THRESHOLD = 32
MAX_LAT = math.degrees(math.atan(math.sinh(math.pi)))


def project(lat: float, lon: float) -> tuple[float, float]:
    return (
        RADIUS * math.radians((lon + 180) % 360 - 180) + HALF,
        HALF - RADIUS * math.asinh(math.tan(math.radians(lat))),
    )


def unproject(x: float, y: float) -> tuple[float, float]:
    return (
        math.degrees(math.atan(math.sinh((HALF - y) / RADIUS))),
        (math.degrees((x % WORLD - HALF) / RADIUS)),
    )


class Grid:
    def __init__(self, block_cells: int, north: float, south: float) -> None:
        if (
            not isinstance(block_cells, int)
            or block_cells % 16
            or not MIN_BLOCK <= block_cells <= MAX_BLOCK
        ):
            raise ValueError(
                f"block_cells {block_cells}: expected a multiple of 16 from 16 to 8192"
            )
        if not -MAX_LAT < south < north < MAX_LAT:
            raise ValueError(f"bounds {north}, {south}: outside projection or reversed")
        self.block_cells = block_cells
        self.columns = math.ceil(WORLD_CELLS / block_cells)
        self.first_row = math.floor(project(north, 0)[1] / CELL / block_cells)
        last = math.floor(project(south, 0)[1] / CELL / block_cells)
        self.rows = last - self.first_row + 1

    def block_of(self, lat: float, lon: float) -> tuple[int, int]:
        x, y = project(lat, lon)
        return (math.floor(x / CELL) // self.block_cells, math.floor(y / CELL) // self.block_cells)

    def centre(self, column: int, row: int) -> tuple[float, float]:
        return unproject(
            (column + 0.5) * self.block_cells * CELL, (row + 0.5) * self.block_cells * CELL
        )

    def samples(self, column: int, row: int) -> Iterator[tuple[float, float]]:
        for b in range(8):
            for a in range(8):
                yield unproject(
                    (column + (a + 0.5) / 8) * self.block_cells * CELL,
                    (row + (b + 0.5) / 8) * self.block_cells * CELL,
                )


def in_ring(lon: float, lat: float, ring: list[list[float]]) -> bool:
    inside = False
    for a, b in zip(ring, ring[1:] + ring[:1], strict=False):
        if (a[1] > lat) != (b[1] > lat):
            cross = a[0] + (lat - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
            if lon < cross:
                inside = not inside
    return inside


def on_land(lon: float, lat: float, polygons: list[list[list[list[float]]]]) -> bool:
    return any(
        in_ring(lon, lat, polygon[0]) and not any(in_ring(lon, lat, hole) for hole in polygon[1:])
        for polygon in polygons
    )


def is_land(count: int) -> bool:
    return count >= LAND_THRESHOLD


def load_polygons(path: Path) -> list[list[list[list[float]]]]:
    with path.open(encoding="utf-8") as source:
        features = json.load(source)["features"]
    polygons = []
    for feature in features:
        geom = feature["geometry"]
        if geom["type"] == "Polygon":
            polygons.append(geom["coordinates"])
        elif geom["type"] == "MultiPolygon":
            polygons.extend(geom["coordinates"])
        else:
            raise ValueError(f"unsupported geometry {geom['type']}")
    return polygons


def land_counts(grid: Grid, polygons: list[list[list[list[float]]]]) -> npt.NDArray[np.int64]:
    """Rasterize rings at the sample latitudes, unioning polygons and cutting holes."""
    width = grid.columns * 8
    height = grid.rows * 8
    lons = np.array(
        [unproject((i + 0.5) * grid.block_cells * CELL / 8, HALF)[1] for i in range(width)]
    )
    lats = np.array(
        [
            unproject(HALF, (grid.first_row * 8 + j + 0.5) * grid.block_cells * CELL / 8)[0]
            for j in range(height)
        ]
    )
    inside = np.zeros((height, width), dtype=bool)
    for polygon in polygons:
        outer = np.asarray(polygon[0], dtype=np.float64)
        rows = np.flatnonzero((lats >= outer[:, 1].min()) & (lats <= outer[:, 1].max()))
        for j in rows:
            mask = np.zeros(width, dtype=bool)
            for ring in polygon:
                points = np.asarray(ring, dtype=np.float64)
                x1, y1 = points[:, 0], points[:, 1]
                x2, y2 = np.roll(x1, -1), np.roll(y1, -1)
                spans = (y1 > lats[j]) != (y2 > lats[j])
                crossings = x1[spans] + (lats[j] - y1[spans]) * (x2[spans] - x1[spans]) / (
                    y2[spans] - y1[spans]
                )
                crossings.sort()
                ring_mask = (
                    len(crossings) - np.searchsorted(crossings, lons, side="right")
                ) % 2 == 1
                if ring is polygon[0]:
                    mask = ring_mask
                else:
                    mask &= ~ring_mask
            inside[j] |= mask
    return inside.reshape(grid.rows, 8, grid.columns, 8).sum(axis=(1, 3))
