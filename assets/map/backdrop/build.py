"""Build backdrop fields from the reduced normals."""

import argparse
import tomllib
from pathlib import Path

import numpy as np
from geometry import LAND_THRESHOLD, Grid, land_counts, load_polygons
from PIL import Image

U16_MAX = 65535


def half_away(value: np.ndarray) -> np.ndarray:
    return np.copysign(np.floor(np.abs(value) + 0.5), value).astype(np.int64)


def fill(field: np.ndarray, month: int) -> np.ndarray:
    field = np.array(field, dtype=np.float64, copy=True)
    if not np.isfinite(field).any():
        raise ValueError(f"sea_mean.png: month {month} has no sea value")
    while not np.isfinite(field).all():
        sums = np.zeros(field.shape, dtype=np.float64)
        counts = np.zeros(field.shape, dtype=np.uint8)
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                if dy == dx == 0:
                    continue
                shifted = np.roll(field, dx, axis=1)
                if dy < 0:
                    target, source = slice(0, -1), slice(1, None)
                elif dy > 0:
                    target, source = slice(1, None), slice(0, -1)
                else:
                    target = source = slice(None)
                values = shifted[source]
                present = np.isfinite(values)
                sums[target] += np.where(present, values, 0)
                counts[target] += present
        take = ~np.isfinite(field) & (counts > 0)
        field[take] = sums[take] / counts[take]
    return field


def bilinear(  # noqa: PLR0913  # four corners and two interpolation fractions
    sw: np.ndarray, se: np.ndarray, nw: np.ndarray, ne: np.ndarray, tx: np.ndarray, ty: np.ndarray
) -> np.ndarray:  # four corners and two interpolation fractions
    return (1 - ty) * ((1 - tx) * sw + tx * se) + ty * ((1 - tx) * nw + tx * ne)


def interpolate(  # noqa: PLR0913  # grid axes and sample coordinates are independent
    field: np.ndarray,
    lat: np.ndarray,
    lon: np.ndarray,
    north: float,
    spacing: float,
    lat_step: float,
) -> np.ndarray:  # grid axes and sample coordinates are independent
    x = (np.asarray(lon) % 360) / spacing
    y = (north - np.asarray(lat)) / lat_step
    west = np.floor(x).astype(int)
    upper = np.floor(y).astype(int)
    if np.any(upper < 0) or np.any(upper + 1 >= field.shape[0]):
        raise ValueError("latitude is outside the measurement grid")
    tx = x - west
    ty = y - upper
    west %= field.shape[1]
    east = (west + 1) % field.shape[1]
    return bilinear(
        field[upper + 1, west],
        field[upper + 1, east],
        field[upper, west],
        field[upper, east],
        tx,
        1 - ty,
    )


def read_grid(path: Path, height: int, width: int) -> np.ndarray:
    with Image.open(path) as image:
        if image.size != (width, height * 12):
            raise ValueError(f"{path.name}: unexpected image size")
        return np.asarray(image, dtype=np.uint16).reshape(12, height, width)


def write_png(path: Path, pixels: np.ndarray) -> None:
    Image.fromarray(pixels).save(path, format="PNG", optimize=True)


def build(settings_path: Path, sources: Path, output: Path) -> None:
    settings = tomllib.loads(settings_path.read_text())
    grid = Grid(settings["block_cells"], settings["north"], settings["south"])
    sea = read_grid(sources / "sea_mean.png", 89, 180)
    air = read_grid(sources / "air_mean.png", 181, 360)
    pressure = read_grid(sources / "pressure_median.png", 181, 360)
    # All months must be fillable; fail before writing any output.
    filled = [
        fill(np.where(sea[m] == 0, np.nan, sea[m].astype(float) - 10000), m + 1) for m in range(12)
    ]
    land = land_counts(grid, load_polygons(sources / "ne_50m_land.geojson")) >= LAND_THRESHOLD
    lat = np.array(
        [grid.centre(0, r)[0] for r in range(grid.first_row, grid.first_row + grid.rows)]
    )[:, None]
    lon = np.array([grid.centre(c, grid.first_row)[1] for c in range(grid.columns)])[None, :]
    fields: dict[str, np.ndarray] = {}
    for name, values, north, spacing in (
        ("pressure", pressure, 90, 1),
        ("air_temperature", air, 90, 1),
        ("sea_temperature", filled, 88, 2),
    ):
        months = []
        for m in range(12):
            samples = np.asarray(values[m], dtype=np.float64)
            result = interpolate(samples, lat, lon, north, spacing, spacing)
            if name == "air_temperature":
                result = half_away((result - 10000) / 10) + 1000
            elif name == "sea_temperature":
                result = np.where(land, 0, half_away(result / 10) + 1000)
            else:
                result = half_away(result)
            if np.any(result < 0) or np.any(result > U16_MAX):
                raise ValueError(f"{name}.png: value outside 16-bit storage")
            months.append(result.astype(np.uint16))
        fields[name] = np.asarray(months).reshape(12 * grid.rows, grid.columns)
    output.mkdir(parents=True, exist_ok=True)
    write_png(output / "land.png", (land * 255).astype(np.uint8))
    for name, image in fields.items():
        write_png(output / f"{name}.png", image)
    manifest = f"""format = 1
projection = "spherical_mercator"
cell_metres = 839.0
block_cells = {grid.block_cells}
first_row = {grid.first_row}
rows = {grid.rows}
columns = {grid.columns}

[layers]
land = "land.png"
pressure = "pressure.png"
air_temperature = "air_temperature.png"
sea_temperature = "sea_temperature.png"
"""
    (output / "backdrop.toml").write_text(manifest)


def main() -> None:
    parser = argparse.ArgumentParser(description="Build a backdrop from the reduced source grids")
    parser.add_argument("output", type=Path)
    parser.add_argument("--settings", type=Path, default=Path(__file__).parent / "settings.toml")
    parser.add_argument("--sources", type=Path, default=Path(__file__).parent / "sources")
    args = parser.parse_args()
    build(args.settings, args.sources, args.output)


if __name__ == "__main__":
    main()
