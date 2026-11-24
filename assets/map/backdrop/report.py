"""Compare a built backdrop with a region map and its scenario locations."""

import argparse
import math
import tomllib
from pathlib import Path

import numpy as np
from geometry import CELL, MAX_BLOCK, MIN_BLOCK, WORLD_CELLS, project, unproject
from PIL import Image

PRESSURE_MIN, PRESSURE_MAX = 30000, 47000
AIR_MIN, AIR_MAX = 300, 1500
SEA_MIN, SEA_MAX = 970, 1400
LAND, HALF = 255, 0.5


def comparable_blocks(  # noqa: PLR0913  # crop and backdrop geometry are independent
    x: int, y: int, width: int, height: int, size: int, first: int, rows: int, columns: int
) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
    squares = []
    for row in range((y + size - 1) // size, (y + height) // size):
        for column in range((x + size - 1) // size, (x + width) // size):
            if (
                column < columns
                and (column + 1) * size <= WORLD_CELLS
                and (row + 1) * size <= WORLD_CELLS
            ):
                squares.append((column, row))
    return squares, [(c, r) for c, r in squares if first <= r < first + rows]


BACKDROP_FORMAT = 2


def load_backdrop(path: Path) -> tuple[dict, np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    manifest = tomllib.loads((path / "backdrop.toml").read_text())
    size, first, rows, columns = (
        manifest[k] for k in ("block_cells", "first_row", "rows", "columns")
    )
    if (
        manifest.get("format") != BACKDROP_FORMAT
        or manifest.get("projection") != "spherical_mercator"
        or manifest.get("cell_metres") != CELL
    ):
        raise ValueError("backdrop.toml: invalid format, projection or cell size")
    if not isinstance(size, int) or size % MIN_BLOCK or not MIN_BLOCK <= size <= MAX_BLOCK:
        raise ValueError("backdrop.toml: invalid block_cells")
    if first < 0 or rows < 1 or columns != math.ceil(WORLD_CELLS / size) or first + rows > columns:
        raise ValueError("backdrop.toml: invalid dimensions")
    arrays = []
    for name in ("land", "pressure", "air_temperature", "sea_temperature"):
        filename = manifest["layers"][name]
        relative = Path(filename)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"backdrop.toml: invalid path {filename}")
        with Image.open(path / relative) as image:
            expected_height = rows if name == "land" else rows * 12
            if image.size != (columns, expected_height):
                raise ValueError(f"{filename}: invalid dimensions")
            expected_mode = "L" if name == "land" else "I;16"
            if image.mode != expected_mode:
                raise ValueError(f"{filename}: invalid grayscale depth")
            arrays.append(np.asarray(image).copy())
    land, pressure, air, sea = arrays
    if not np.isin(land, [0, 255]).all():
        raise ValueError(f"{manifest['layers']['land']}: invalid sample")
    if np.any((pressure < PRESSURE_MIN) | (pressure > PRESSURE_MAX)):
        raise ValueError(f"{manifest['layers']['pressure']}: invalid sample")
    if np.any((air < AIR_MIN) | (air > AIR_MAX)):
        raise ValueError(f"{manifest['layers']['air_temperature']}: invalid sample")
    mask = np.tile(land == LAND, (12, 1))
    if np.any(sea[mask] != 0) or np.any((sea[~mask] < SEA_MIN) | (sea[~mask] > SEA_MAX)):
        raise ValueError(f"{manifest['layers']['sea_temperature']}: invalid sample")
    return manifest, land, pressure, air, sea


def rounded_mean(values: list[int]) -> int:
    value = sum(values) / len(values)
    return math.floor(value + 0.5) if value >= 0 else -math.floor(-value + 0.5)


def read_sea_keys(path: Path) -> list[bytes]:
    rules = tomllib.loads(path.read_text())
    keys = [bytes.fromhex(t["key"]) for t in rules["terrain"] if t["kind"] == "sea"]
    if not keys:
        raise ValueError(f"{path.name}: no sea terrain")
    return keys


def land_share(block: np.ndarray, sea_keys: list[bytes]) -> float:
    sea_mask = np.zeros(block.shape[:2], dtype=bool)
    for key in sea_keys:
        sea_mask |= np.all(block == np.frombuffer(key, dtype=np.uint8), axis=2)
    return float(np.mean(~sea_mask))


def generate(backdrop_dir: Path, region_dir: Path, locations_path: Path, points_path: Path) -> str:  # noqa: PLR0915  # the report writes its required sections in order
    manifest, land, pressure, air, sea = load_backdrop(backdrop_dir)
    size, first, rows, columns = (
        manifest[k] for k in ("block_cells", "first_row", "rows", "columns")
    )
    region = tomllib.loads((region_dir / "map.toml").read_text())
    crop = region["crop"]
    x, y = crop["origin"]
    width, height = crop["width"], crop["height"]
    layers = region["layers"]
    with Image.open(region_dir / layers["climate_temperature"]) as image:
        means = np.asarray(image, dtype=np.int32)
    with Image.open(region_dir / layers["climate_half_range"]) as image:
        ranges = np.asarray(image, dtype=np.int32)
    with Image.open(region_dir / layers["terrain"]) as image:
        terrain = np.asarray(image.convert("RGB"))
    if (
        means.shape != (height, width)
        or ranges.shape != (height, width)
        or terrain.shape[:2] != (height, width)
    ):
        raise ValueError("map.toml: crop and layer dimensions differ")
    sea_keys = read_sea_keys(region_dir / layers["terrain_rules"])
    locations = tomllib.loads(locations_path.read_text())["location"]
    points = tomllib.loads(points_path.read_text())["point"]
    north = unproject(0, first * size * CELL)[0]
    south = unproject(0, (first + rows) * size * CELL)[0]
    lines = [
        "# Backdrop comparison",
        f"Block cells: {size}; backdrop rows: {first} to {first + rows - 1}; outer latitudes: {north:.4f}, {south:.4f}.",
        f"Region reaches rows {y // size} to {(y + height - 1) // size}.",
        "Temperatures and differences are tenths of a degree Celsius; pressure is in pascals.",
    ]
    places = []
    for place in locations:
        px, py = project(place["lat"], place["lon"])
        i, j = math.floor(px / CELL), math.floor(py / CELL)
        if not (x <= i < x + width and y <= j < y + height):
            raise ValueError(f"{place['id']}: outside region crop")
        c, r = i // size, j // size
        mean = int(means[j - y, i - x]) - 1000
        half = int(ranges[j - y, i - x])
        local = (mean, mean - half, mean + half)
        if not first <= r < first + rows:
            lines.append(f"- {place['id']}: map {local}; outside the backdrop")
            continue
        months = [int(air[m * rows + r - first, c]) - 1000 for m in range(12)]
        normal = (rounded_mean(months), min(months), max(months))
        differences = tuple(a - b for a, b in zip(normal, local, strict=False))
        places.append(differences)
        lines.append(f"- {place['id']}: map {local}; backdrop {normal}; differences {differences}")
    lines.insert(4, f"Places inside backdrop: {len(places)} of {len(locations)}.")
    if places:
        lines.append(
            f"Places summary ({len(places)} of {len(locations)}): mean, least, greatest differences (year, coldest, warmest): "
            + str([(sum(v) / len(v), min(v), max(v)) for v in zip(*places, strict=False)])
        )
    else:
        lines.append("no place lies inside the backdrop")
    squares, compared = comparable_blocks(x, y, width, height, size, first, rows, columns)
    mismatches = []
    for c, r in compared:
        block = terrain[r * size - y : (r + 1) * size - y, c * size - x : (c + 1) * size - x]
        share = land_share(block, sea_keys)
        backdrop_land = land[r - first, c] == LAND
        if (share >= HALF) != backdrop_land:
            mismatches.append((c, r))
        lines.append(
            f"- block ({c}, {r}): map land share {share:.6f}; backdrop {'land' if backdrop_land else 'sea'}"
        )
    if compared:
        lines.append(
            f"Blocks: {len(squares)} complete squares, {len(compared)} compared, {len(compared) - len(mismatches)} agree; differing: {mismatches}."
        )
    else:
        lines.append(
            f"Blocks: {len(squares)} complete squares, {len(compared)} held; none compared."
        )
    for point in points:
        px, py = project(point["lat"], point["lon"])
        c, r = math.floor(px / CELL) // size, math.floor(py / CELL) // size
        if not first <= r < first + rows:
            lines.append(f"- {point['name']}: block ({c}, {r}); outside the backdrop")
            continue
        p = [int(pressure[m * rows + r - first, c]) + 60000 for m in range(12)]
        a = [int(air[m * rows + r - first, c]) - 1000 for m in range(12)]
        s = (
            "land"
            if land[r - first, c]
            else str([int(sea[m * rows + r - first, c]) - 1000 for m in range(12)])
        )
        lines.append(f"- {point['name']}: block ({c}, {r}); pressure {p}; air {a}; sea {s}")
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("backdrop", type=Path)
    parser.add_argument("region", type=Path)
    parser.add_argument("locations", type=Path)
    parser.add_argument("points", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    text = generate(args.backdrop, args.region, args.locations, args.points)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "report.md").write_text(text)


if __name__ == "__main__":
    main()
