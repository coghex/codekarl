"""The world grid and the tile arithmetic shared by the Ireland derivation scripts (see `derive_elevation.py`).

The grid is `docs/map-design.md` section 9.3's: spherical Mercator, cells of exactly 839 projected metres, cell (i, j) covering
projected [839 i, 839 (i + 1)) east of the antimeridian and [839 j, 839 (j + 1)) south of the top of the projection. The authority
is `ck-map`'s `project` function; `check_crop` in `crates/map/tests/ireland.rs` loads the committed data through it.
"""

from __future__ import annotations

import math
import time
import tomllib
import urllib.request
from pathlib import Path

import numpy as np
from numpy.typing import NDArray

EARTH_RADIUS_M = 6_378_137.0
HALF_WORLD_M = math.pi * EARTH_RADIUS_M
CELL_M = 839.0
# Ground metres in a cell at Ireland's latitude (53.4 degrees north), for slopes.
CELL_GROUND_M = 500.0

# The crop: a box of latitude and longitude, widened to whole cells. Ireland and its offshore islands lie inside it with a sea
# margin of about 8 km: Inishtearaght (10.66 W), Skellig Michael (10.54 W), Inishtrahull (55.43 N), the Fastnet Rock (51.39 N).
CROP_WEST, CROP_EAST = -10.90, -5.30
CROP_SOUTH, CROP_NORTH = 51.25, 55.55

Array = NDArray[np.float64]


def project(latitude: float, longitude: float) -> tuple[float, float]:
    """Projected metres east of the antimeridian and south of the top of the projection."""
    x_m = EARTH_RADIUS_M * math.radians(longitude) + HALF_WORLD_M
    y_m = HALF_WORLD_M - EARTH_RADIUS_M * math.log(
        math.tan(math.pi / 4 + math.radians(latitude) / 2)
    )
    return x_m, y_m


class Crop:
    """The crop in whole cells."""

    def __init__(self, origin: tuple[int, int], width: int, height: int) -> None:
        self.origin = origin
        self.width = width
        self.height = height

    @classmethod
    def of_box(cls) -> Crop:
        """The crop that holds the latitude and longitude box."""
        west, north = project(CROP_NORTH, CROP_WEST)
        east, south = project(CROP_SOUTH, CROP_EAST)
        origin = (math.floor(west / CELL_M), math.floor(north / CELL_M))
        return cls(
            origin,
            math.floor(east / CELL_M) - origin[0] + 1,
            math.floor(south / CELL_M) - origin[1] + 1,
        )

    @classmethod
    def of_map_toml(cls, path: Path) -> Crop:
        """The crop a `map.toml` names."""
        crop = tomllib.loads(path.read_text(encoding="utf-8"))["crop"]
        return cls((crop["origin"][0], crop["origin"][1]), crop["width"], crop["height"])

    def centres(self) -> tuple[Array, Array]:
        """Longitude and latitude (degrees) of every cell's centre, rows first."""
        columns = (self.origin[0] + np.arange(self.width) + 0.5) * CELL_M
        rows = (self.origin[1] + np.arange(self.height) + 0.5) * CELL_M
        x_m, y_m = np.meshgrid(columns, rows)
        longitude = np.degrees((x_m - HALF_WORLD_M) / EARTH_RADIUS_M)
        latitude = np.degrees(
            2 * np.arctan(np.exp((HALF_WORLD_M - y_m) / EARTH_RADIUS_M)) - math.pi / 2
        )
        return longitude, latitude

    def cell_of(self, latitude: float, longitude: float) -> tuple[float, float]:
        """A point in cell units from the crop's top-left corner (column, row)."""
        x_m, y_m = project(latitude, longitude)
        return x_m / CELL_M - self.origin[0], y_m / CELL_M - self.origin[1]


def download(url: str, target: Path, attempts: int = 6) -> None:
    """Fetches `url` into `target` unless it is already there; retries with a growing pause, and never leaves a partial file."""
    if target.exists():
        return
    target.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={"User-Agent": "codekarl-map-derivation"})
    for attempt in range(attempts):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                body = response.read()
        except OSError:
            if attempt == attempts - 1:
                raise
            time.sleep(2**attempt)
        else:
            partial = target.with_name(target.name + ".part")
            partial.write_bytes(body)
            partial.replace(target)
            return
