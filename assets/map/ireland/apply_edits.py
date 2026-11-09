"""Apply the terrain edits and derive Ireland's climate, glacier and fertility layers.

Run `python3 apply_edits.py [--out DIR] [--force]`; `--self-test` runs the
stdlib-only equilibrium-line vectors. Terrain edits run in TOML order. Climate
and fertility are then computed from the edited terrain and elevation using the
normative algorithm settings in `edits.toml`. The input digest is written to
`edits.applied`; an existing marker requires `--force`.
"""

from __future__ import annotations

import argparse
import hashlib
import heapq
import math
import sys
import tomllib
from collections import deque
from pathlib import Path
from typing import Any

ELA_TOLERANCE_C = 0.01


def _run_ela_self_test() -> None:
    """Run the Ohmura ELA vectors without importing third-party modules."""
    coefficients = (966.0, 230.0, 5.87)
    for precipitation, expected in ((300.0, -3.15), (1000.0, 0.15), (3000.0, 7.43)):
        a, b, c = coefficients[2], coefficients[1], coefficients[0] - precipitation
        value = (-b + math.sqrt(b * b - 4.0 * a * c)) / (2.0 * a)
        if abs(value - expected) > ELA_TOLERANCE_C:
            raise SystemExit(f"T_ela({precipitation}) = {value:.4f}, expected {expected:.2f}")
    print("T_ela self-test: 3 vectors passed")


if "--self-test" in sys.argv:
    _run_ela_self_test()
    raise SystemExit(0)

import numpy as np  # noqa: E402 -- --self-test must run before optional image dependencies load
from draft_terrain import ELEVATION_OFFSET, distance_to, terrain_keys  # noqa: E402
from ireland_grid import CELL_GROUND_M, Array, Crop  # noqa: E402
from numpy.typing import NDArray  # noqa: E402
from PIL import Image, ImageDraw  # noqa: E402

HERE = Path(__file__).resolve().parent

NAMES = ("sea", "lake", "pasture", "forest", "bog", "marsh", "mountain")
SEA, LAKE, PASTURE, FOREST, BOG, MARSH, MOUNTAIN = range(len(NAMES))
LAND_TYPES = (PASTURE, FOREST, BOG, MARSH, MOUNTAIN)
REGIONS = ("polygon", "box", "circle")
FILTERS = (
    "terrain",
    "elevation_min",
    "elevation_max",
    "slope_min",
    "slope_max",
    "near_water_max",
    "far_from_water_min",
    "relative_elevation_max",
    "relative_radius",
)
DEFAULT_RELATIVE_RADIUS = 10
GROW_FIELDS = (
    "from",
    "seed_where",
    "share",
    "max_distance",
    "order",
    "seed_points",
    "seed_radius",
    "seed_over",
)
TIDY_FIELDS = ("min_cells", "back", "keep_seed_points")
ORDERS = ("wet", "near", "dry_slope")
FIELDS = {
    "id",
    "area",
    "reason",
    "confidence",
    "paint",
    "region",
    "where",
    "grow",
    "tidy",
    "depth",
}
REQUIRED = ("id", "area", "reason", "paint")
SETTINGS = ("climate", "fertility")
POLYGON_SUPERSAMPLE = 4
POLYGON_COVER = 0.5
KM_PER_DEGREE = 111.0
LAND_ELEVATION = 2
SEA_DEPTH = 2
NEIGHBOURS8 = ((-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1))
NEIGHBOURS4 = ((-1, 0), (1, 0), (0, -1), (0, 1))

Grid = NDArray[np.bool_]
Cell = tuple[int, int]


class State:
    """The terrain and elevation layers edited before climate derivation."""

    def __init__(self, out: Path) -> None:
        self.directory = out
        self.crop = Crop.of_map_toml(out / "map.toml")
        self.keys = terrain_keys(out / "terrain.toml")
        self.display_usable_pm = int(
            tomllib.loads((out / "terrain.toml").read_text(encoding="utf-8")).get(
                "display_usable_pm", 500
            )
        )
        with Image.open(out / "elevation.png") as image:
            self.elevation: Array = np.asarray(image, dtype=np.float64) - ELEVATION_OFFSET
        with Image.open(out / "terrain.png") as image:
            rgb = np.asarray(image.convert("RGB"))
        self.terrain = np.full(self.elevation.shape, -1, dtype=np.int8)
        for index, name in enumerate(NAMES):
            self.terrain[(rgb == self.keys[name]).all(axis=2)] = index
        if (self.terrain < 0).any():
            message = "terrain.png has a pixel that is not a key of terrain.toml"
            raise SystemExit(message)
        self.longitude, self.latitude = self.crop.centres()
        self.slope: Array = np.hypot(*np.gradient(self.elevation, CELL_GROUND_M))
        self.seed_cells_by_paint: dict[int, list[Cell]] = {}

    def write(self, out: Path) -> None:
        """Writes the two edited base images."""
        rgb = np.zeros((*self.terrain.shape, 3), dtype=np.uint8)
        for index, name in enumerate(NAMES):
            rgb[self.terrain == index] = self.keys[name]
        stored = np.clip(self.elevation + ELEVATION_OFFSET, 0, 65535).astype(np.uint16)
        Image.fromarray(stored).save(out / "elevation.png")
        Image.fromarray(rgb).save(out / "terrain.png")

    def land(self) -> Grid:
        """Land that is not a lake."""
        return (self.elevation >= 1) & (self.terrain != LAKE)

    def water(self) -> Grid:
        """The cells that are sea or lake."""
        return (self.elevation < 1) | (self.terrain == LAKE)

    def refresh_slope(self) -> None:
        """Computes the slope again (after an edit changed the elevation)."""
        self.slope = np.hypot(*np.gradient(self.elevation, CELL_GROUND_M))


def river_cells(state: State) -> Grid:
    """Rasterize configured river paths for edit growth seeds."""
    document = tomllib.loads((state.directory / "rivers.toml").read_text(encoding="utf-8"))
    image = Image.new("L", (state.crop.width, state.crop.height), 0)
    draw = ImageDraw.Draw(image)
    for river in document.get("river", []):
        points = [
            tuple(
                round(value) for value in state.crop.cell_of(point["latitude"], point["longitude"])
            )
            for point in river["points"]
        ]
        if len(points) > 1:
            draw.line(points, fill=1, width=1)
        elif points:
            draw.point(points[0], fill=1)
    return np.asarray(image, dtype=np.uint8) > 0


def region_mask(state: State, region: dict[str, Any], identifier: str) -> Grid:
    """The cells a region covers: a polygon by majority cover, a box or a circle by the cell centre."""
    kinds = [kind for kind in REGIONS if kind in region]
    if len(kinds) != 1 or len(region) != 1:
        message = f"edit {identifier}: region needs exactly one of {', '.join(REGIONS)}"
        raise SystemExit(message)
    kind = kinds[0]
    shape = state.elevation.shape
    if kind == "box":
        south, north, west, east = region["box"]
        return (
            (state.latitude >= south)
            & (state.latitude <= north)
            & (state.longitude >= west)
            & (state.longitude <= east)
        )
    if kind == "circle":
        latitude, longitude, radius = region["circle"]
        d_x = (state.longitude - longitude) * math.cos(math.radians(latitude)) * KM_PER_DEGREE
        d_y = (state.latitude - latitude) * KM_PER_DEGREE
        return np.hypot(d_x, d_y) <= radius
    scale = POLYGON_SUPERSAMPLE
    image = Image.new("L", (shape[1] * scale, shape[0] * scale), 0)
    ring = [state.crop.cell_of(lat, lon) for lat, lon in region["polygon"]]
    ImageDraw.Draw(image).polygon([(x * scale, y * scale) for x, y in ring], fill=1)
    covered = (
        np.asarray(image, dtype=np.float64)
        .reshape(shape[0], scale, shape[1], scale)
        .mean(axis=(1, 3))
    )
    return covered > POLYGON_COVER


def where_mask(state: State, where: dict[str, Any], identifier: str) -> Grid:
    """The cells that satisfy an edit's `where` filters."""
    unknown = set(where) - set(FILTERS)
    if unknown:
        message = f"edit {identifier}: unknown filter {sorted(unknown)}"
        raise SystemExit(message)
    mask = np.ones(state.elevation.shape, dtype=bool)
    if "terrain" in where:
        mask &= np.isin(state.terrain, [NAMES.index(name) for name in where["terrain"]])
    mask &= state.elevation >= where.get("elevation_min", -math.inf)
    mask &= state.elevation <= where.get("elevation_max", math.inf)
    mask &= (state.slope >= where.get("slope_min", 0.0)) & (
        state.slope <= where.get("slope_max", math.inf)
    )
    if "relative_elevation_max" in where:
        radius = int(where.get("relative_radius", DEFAULT_RELATIVE_RADIUS))
        mask &= (
            state.elevation - box_blur(state.elevation, radius) <= where["relative_elevation_max"]
        )
    if "near_water_max" in where or "far_from_water_min" in where:
        distance = distance_to(state.water())
        mask &= (distance <= where.get("near_water_max", math.inf)) & (
            distance >= where.get("far_from_water_min", 0)
        )
    return mask


def growth_key(state: State, order: str, row: int, column: int, ring: int) -> tuple[float, ...]:
    """The place of a cell in the growth order: the smaller, the earlier; the last number is the row-major index."""
    index = row * state.elevation.shape[1] + column
    if order == "wet":
        return (float(state.elevation[row, column]), float(state.slope[row, column]), index)
    if order == "near":
        return (ring, index)
    return (-float(state.slope[row, column]), -float(state.elevation[row, column]), index)


def paint_seed_areas(state: State, inside: Grid, paint: int, grow: dict[str, Any]) -> None:
    """The named sites always count: cells within `seed_radius` of each seed point, in the region, whose terrain is in `seed_over`, take the paint."""
    radius = int(grow.get("seed_radius", 0))
    over = [NAMES.index(name) for name in grow.get("seed_over", [])]
    if radius < 0 or (radius > 0) != bool(over):
        message = "grow: seed_radius and seed_over go together (a radius of at least 1 and a list of terrains)"
        raise SystemExit(message)
    height, width = state.elevation.shape
    for latitude, longitude in grow.get("seed_points", []):
        column, row = state.crop.cell_of(latitude, longitude)
        row, column = int(row), int(column)
        state.seed_cells_by_paint.setdefault(paint, []).append((row, column))
        for r in range(max(row - radius, 0), min(row + radius + 1, height)):
            for c in range(max(column - radius, 0), min(column + radius + 1, width)):
                if inside[r, c] and int(state.terrain[r, c]) in over:
                    state.terrain[r, c] = paint


def seed_cells(
    state: State, inside: Grid, eligible: Grid, paint: int, grow: dict[str, Any]
) -> Grid:
    """The seeds of a growth: the region's cells of the `from` terrains, and each seed point's cell and its neighbours."""
    names = [name for name in grow.get("from", []) if name != "rivers"]
    seeds = inside & np.isin(state.terrain, [NAMES.index(name) for name in names])
    if "rivers" in grow.get("from", []):
        seeds |= inside & river_cells(state)
    if "seed_where" in grow:
        seeds |= inside & where_mask(state, grow["seed_where"], "seed_where")
    height, width = state.elevation.shape
    for latitude, longitude in grow.get("seed_points", []):
        column, row = state.crop.cell_of(latitude, longitude)
        for d_row, d_column in ((0, 0), *NEIGHBOURS8):
            r, c = int(row) + d_row, int(column) + d_column
            if (
                0 <= r < height
                and 0 <= c < width
                and inside[r, c]
                and (eligible[r, c] or state.terrain[r, c] == paint)
            ):
                seeds[r, c] = True
    return seeds


def grow_cells(state: State, edit: dict[str, Any], inside: Grid, where: Grid) -> int:
    """Grows the painted terrain from the seeds over the cells that pass `where`, in the order's rule, to the share; returns the cells taken."""
    grow = edit["grow"]
    sources = ("from", "seed_where", "seed_points")
    if (
        set(grow) - set(GROW_FIELDS)
        or not any(key in grow for key in sources)
        or grow.get("order", "wet") not in ORDERS
    ):
        message = f"edit {edit['id']}: grow takes {GROW_FIELDS} (one of {sources} is required, order one of {ORDERS}); got {sorted(grow)}"
        raise SystemExit(message)
    paint = NAMES.index(edit["paint"])
    paint_seed_areas(state, inside, paint, grow)
    eligible = inside & where & (state.terrain != paint)
    seeds = seed_cells(state, inside, eligible, paint, grow)
    height, width = state.elevation.shape
    land = inside & np.isin(state.terrain, LAND_TYPES)
    target = int(float(grow["share"]) * int(land.sum())) if "share" in grow else None
    have = int((inside & (state.terrain == paint)).sum())
    limit = grow.get("max_distance")
    order = grow.get("order", "wet")
    ring = np.full((height, width), -1, dtype=np.int64)
    origin = np.zeros((height, width, 2), dtype=np.int64)
    heap: list[tuple[tuple[float, ...], int, int]] = []

    def push_ring(at: Cell, next_ring: int, seed: Cell) -> None:
        """Queues the unvisited eligible neighbours of a cell within `max_distance` of its seed, each marked with its ring."""
        for d_row, d_column in NEIGHBOURS8:
            row, column = at[0] + d_row, at[1] + d_column
            if not (
                0 <= row < height
                and 0 <= column < width
                and eligible[row, column]
                and ring[row, column] < 0
            ):
                continue
            if limit is not None and math.hypot(row - seed[0], column - seed[1]) > float(limit):
                continue
            ring[row, column] = next_ring
            origin[row, column] = seed
            heapq.heappush(heap, (growth_key(state, order, row, column, next_ring), row, column))

    taken = 0
    for row, column in zip(*np.nonzero(seeds), strict=True):
        ring[row, column] = 0
        if state.terrain[row, column] != paint and eligible[row, column]:
            state.terrain[row, column] = paint
            have += 1
            taken += 1
    for row, column in zip(*np.nonzero(seeds), strict=True):
        push_ring((int(row), int(column)), 1, (int(row), int(column)))
    while heap and (target is None or have < target):
        _, row, column = heapq.heappop(heap)
        if state.terrain[row, column] == paint:
            continue
        state.terrain[row, column] = paint
        have += 1
        taken += 1
        push_ring(
            (row, column),
            int(ring[row, column]) + 1,
            (int(origin[row, column, 0]), int(origin[row, column, 1])),
        )
    return taken


def components(mask: Grid, neighbours: tuple[Cell, ...] = NEIGHBOURS4) -> list[list[Cell]]:
    """The connected patches of `mask` (four-neighbour by default), in row-major order of their first cell."""
    height, width = mask.shape
    seen = np.zeros_like(mask)
    patches: list[list[Cell]] = []
    for row, column in zip(*np.nonzero(mask), strict=True):
        if seen[row, column]:
            continue
        patch: list[Cell] = []
        queue = deque([(int(row), int(column))])
        seen[row, column] = True
        while queue:
            r, c = queue.popleft()
            patch.append((r, c))
            for d_row, d_column in neighbours:
                n_row, n_column = r + d_row, c + d_column
                if (
                    0 <= n_row < height
                    and 0 <= n_column < width
                    and mask[n_row, n_column]
                    and not seen[n_row, n_column]
                ):
                    seen[n_row, n_column] = True
                    queue.append((n_row, n_column))
        patches.append(patch)
    return patches


def fill_enclosed(state: State, paint: int, back: int, minimum: int) -> int:
    """Patches of `back` smaller than `minimum` whose whole border is `paint` (real enclosed holes) become `paint`; returns the cells."""
    height, width = state.terrain.shape
    changed = 0
    for patch in components(state.terrain == back):
        if len(patch) >= minimum:
            continue
        members = set(patch)
        enclosed = True
        for row, column in patch:
            for d_row, d_column in NEIGHBOURS4:
                n_row, n_column = (
                    min(max(row + d_row, 0), height - 1),
                    min(max(column + d_column, 0), width - 1),
                )
                if (n_row, n_column) not in members and state.terrain[n_row, n_column] != paint:
                    enclosed = False
        if enclosed:
            for row, column in patch:
                state.terrain[row, column] = paint
            changed += len(patch)
    return changed


def tidy_terrain(state: State, edit: dict[str, Any]) -> int:
    """Patches of the painted terrain smaller than `min_cells` go back; small patches of `back` that it entirely surrounds are filled."""
    tidy = edit["tidy"]
    if set(tidy) - set(TIDY_FIELDS) or "min_cells" not in tidy or "back" not in tidy:
        message = f"edit {edit['id']}: tidy takes {TIDY_FIELDS} (min_cells and back are required); got {sorted(tidy)}"
        raise SystemExit(message)
    paint = NAMES.index(edit["paint"])
    back = NAMES.index(tidy["back"])
    minimum = int(tidy["min_cells"])
    kept: set[Cell] = set()
    if tidy.get("keep_seed_points", False):
        for row, column in state.seed_cells_by_paint.get(paint, []):
            kept.update(
                (row + d_row, column + d_column) for d_row in (-1, 0, 1) for d_column in (-1, 0, 1)
            )
    changed = 0
    for patch in components(state.terrain == paint):
        if len(patch) < minimum and kept.isdisjoint(patch):
            for row, column in patch:
                state.terrain[row, column] = back
            changed += len(patch)
    return changed + fill_enclosed(state, paint, back, minimum)


def apply_edit(state: State, edit: dict[str, Any]) -> int:
    """Applies one edit; returns how many cells it changed."""
    identifier = edit["id"]
    if "tidy" in edit:
        return tidy_terrain(state, edit)
    paint = edit["paint"]
    inside = region_mask(state, edit["region"], identifier)
    where = where_mask(state, edit.get("where", {}), identifier)
    if "grow" in edit:
        before = state.terrain.copy()
        grow_cells(state, edit, inside, where)
        cells = state.terrain != before
    else:
        cells = inside & where
        state.terrain[cells] = NAMES.index(paint)
    if paint == "sea":
        state.elevation[cells] = -float(edit.get("depth", SEA_DEPTH))
        state.refresh_slope()
    elif paint != "lake":
        state.elevation[cells & (state.elevation < 1)] = LAND_ELEVATION
    return int(cells.sum())


def box_blur(field: Array, radius: int) -> Array:
    """The mean of the square of `2 radius + 1` cells round each cell; the edges repeat."""
    padded = np.pad(field.astype(np.float64), radius, mode="edge")
    total = np.pad(padded.cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    side = 2 * radius + 1
    return (
        total[side:, side:] - total[:-side, side:] - total[side:, :-side] + total[:-side, :-side]
    ) / (side * side)


def interpolate(latitude: Array, knots: list[float], values: list[float]) -> Array:
    """Piecewise-linear lookup used by the signed latitude climate tables."""
    return np.interp(
        latitude, np.asarray(knots, dtype=np.float64), np.asarray(values, dtype=np.float64)
    )


def priority_flood(elevation: Array, water: Grid, epsilon: float) -> Array:
    """Fill inland depressions from every water cell and crop edge, preserving the normative D8 ordering."""
    height, width = elevation.shape
    surface = elevation.copy()
    surface[water] = -1e9
    filled = np.full((height, width), np.inf, dtype=np.float64)
    seen = np.zeros((height, width), dtype=bool)
    queue: list[tuple[float, int, int]] = []
    for row in range(height):
        for column in range(width):
            if water[row, column] or row in (0, height - 1) or column in (0, width - 1):
                filled[row, column] = surface[row, column]
                seen[row, column] = True
                heapq.heappush(queue, (filled[row, column], row, column))
    while queue:
        level, row, column = heapq.heappop(queue)
        for d_row, d_column in NEIGHBOURS8:
            next_row, next_column = row + d_row, column + d_column
            if (
                0 <= next_row < height
                and 0 <= next_column < width
                and not seen[next_row, next_column]
            ):
                seen[next_row, next_column] = True
                filled[next_row, next_column] = max(surface[next_row, next_column], level + epsilon)
                heapq.heappush(queue, (filled[next_row, next_column], next_row, next_column))
    return filled


def flow_accumulation(state: State, settings: dict[str, Any]) -> tuple[Array, Array]:
    """Return D8 upstream area in km² and the prototype's ground slope in percent."""
    elevation = state.elevation
    land = state.land()
    ground_m = float(settings["cell_metres"]) * np.cos(np.radians(state.latitude))
    ground_m = np.maximum(ground_m, 1.0)
    slope_y, slope_x = np.gradient(elevation.astype(np.float64))
    slope = np.hypot(slope_x, slope_y) / ground_m * 100.0
    filled = priority_flood(elevation, ~land, float(settings["flow_epsilon_m"]))
    area = np.where(land, (ground_m / 1000.0) ** 2, 0.0)
    order = np.argsort(-filled, axis=None)
    accumulation = area.ravel().copy()
    flat = filled.ravel()
    height, width = elevation.shape
    for raw_index in order:
        index = int(raw_index)
        row, column = divmod(index, width)
        if not land[row, column]:
            continue
        best: int | None = None
        drop = 0.0
        for d_row, d_column in NEIGHBOURS8:
            next_row, next_column = row + d_row, column + d_column
            if 0 <= next_row < height and 0 <= next_column < width:
                next_index = next_row * width + next_column
                candidate = (flat[index] - flat[next_index]) / math.hypot(d_row, d_column)
                if candidate > drop:
                    drop = candidate
                    best = next_index
        if best is not None:
            accumulation[best] += accumulation[index]
    return accumulation.reshape(elevation.shape), slope


def climate_fields(  # noqa: PLR0915 -- keep the prototype's climate, moisture, seasonal and snowline steps together
    state: State, settings: dict[str, Any]
) -> tuple[Array, Array, Array, Array, Array]:
    """Compute mean temperature, seasonal half-range, GDD, smoothed rain and glacier mask."""
    height, width = state.elevation.shape
    lat = np.abs(state.latitude[:, 0])
    climate_rows = settings
    knots = climate_rows["latitude_deg"]
    zonal = interpolate(lat, knots, climate_rows["zonal_mean_c"])[:, None]
    ocean_range = interpolate(lat, knots, climate_rows["ocean_half_range_c"])[:, None]
    continent_range = interpolate(lat, knots, climate_rows["continental_half_range_c"])[:, None]
    zonal_rain = interpolate(lat, knots, climate_rows["zonal_rainfall_mm"])[:, None]
    ground_km = (float(climate_rows["cell_metres"]) * np.cos(np.radians(lat)) / 1000.0)[:, None]
    ground_km = np.maximum(ground_km, 1e-12)
    sea = state.terrain == SEA
    lake = state.terrain == LAKE
    land = state.land()
    maritime_distance = np.zeros((height, width), dtype=np.float64)
    rain = np.zeros((height, width), dtype=np.float64)
    absolute_lat = np.abs(state.latitude[:, 0])
    for row in range(height):
        westerly = (
            float(climate_rows["westerly_min_deg"])
            <= absolute_lat[row]
            < float(climate_rows["westerly_max_deg"])
        )
        columns = range(width) if westerly else range(width - 1, -1, -1)
        distance = 0.0
        moisture = 1.0
        previous = 0.0
        for column in columns:
            if sea[row, column]:
                distance = 0.0
            else:
                distance += float(ground_km[row, 0])
            maritime_distance[row, column] = distance
            if sea[row, column] or lake[row, column]:
                moisture = min(
                    1.0,
                    moisture
                    + float(climate_rows["moisture_recovery_per_km"]) * float(ground_km[row, 0]),
                )
                previous = (
                    0.0 if sea[row, column] else max(0.0, float(state.elevation[row, column]))
                )
                rain[row, column] = zonal_rain[row, 0] * moisture
                continue
            elevation = max(0.0, float(state.elevation[row, column]))
            uplift = max(0.0, (elevation - previous) / (float(ground_km[row, 0]) * 1000.0)) * 1000.0
            rain_factor = moisture * (
                float(climate_rows["rainfall_base_factor"])
                + min(
                    float(climate_rows["uplift_cap_factor"]),
                    float(climate_rows["uplift_factor_per_m_km"]) * uplift,
                )
                + float(climate_rows["height_rainfall_factor_per_km"]) * elevation / 1000.0
            )
            rain[row, column] = zonal_rain[row, 0] * rain_factor
            moisture = max(
                float(climate_rows["moisture_floor"]),
                moisture
                - float(climate_rows["moisture_depletion"])
                * rain_factor
                * float(ground_km[row, 0]),
            )
            previous = elevation
    maritime = np.exp(-maritime_distance / float(climate_rows["maritime_length_km"]))
    mean = (
        zonal
        + float(climate_rows["ocean_effect_c"]) * maritime
        - float(climate_rows["lapse_c_per_km"]) * np.maximum(state.elevation, 0.0) / 1000.0
    )
    half = ocean_range + (continent_range - ocean_range) * (1.0 - maritime)
    months = np.arange(12, dtype=np.float64)
    phase = np.cos(2.0 * math.pi * (months - float(climate_rows["warmest_month_phase"])) / 12.0)
    south = state.latitude < 0
    phase_south = np.cos(
        2.0 * math.pi * ((months + 6.0) % 12.0 - float(climate_rows["warmest_month_phase"])) / 12.0
    )
    gdd = np.zeros_like(mean)
    for month in range(12):
        seasonal = np.where(south, phase_south[month], phase[month])
        gdd += np.maximum(0.0, mean + half * seasonal - float(climate_rows["gdd_base_c"])) * float(
            climate_rows["days_per_month"]
        )
    radius = int(climate_rows["rainfall_smooth_width"]) // 2
    rain = box_blur(rain, radius)
    ela_a = float(climate_rows["glacier_ohmura_a"])
    ela_b = float(climate_rows["glacier_ohmura_b"])
    ela_c = float(climate_rows["glacier_ohmura_c"])
    summer_factor = (math.cos(math.pi / 4.0) + 2.0 * math.cos(math.pi / 12.0)) / 3.0
    discriminant = ela_b * ela_b - 4.0 * ela_c * (ela_a - rain)
    if np.any(discriminant < 0):
        row, column = np.argwhere(discriminant < 0)[0]
        raise SystemExit(
            f"climate_glacier cell ({row}, {column}) has no real T_ela for rainfall {rain[row, column]}"
        )
    ela = (-ela_b + np.sqrt(discriminant)) / (2.0 * ela_c)
    glacier = land & ((mean + summer_factor * half) <= ela)
    return mean, half, gdd, rain, glacier


def fertility_field(
    state: State, settings: dict[str, Any], gdd: Array, rainfall: Array, glacier: Grid
) -> Array:
    """Calculate the prototype's valley, slope and climate fertility score in per-thousand units."""
    land = state.land()
    accumulation, slope = flow_accumulation(state, settings)
    minimum = float(settings["channel_min_area_km2"])
    maximum = float(settings["channel_full_area_km2"])
    channel = (
        np.clip(
            np.log10(np.maximum(accumulation, 1e-9) / minimum) / math.log10(maximum / minimum),
            0.0,
            1.0,
        )
        * 1000.0
    )
    channel[~land] = 0.0
    valley = channel.copy()
    radius = int(settings["valley_spread_cells"])
    for d_row in range(-radius, radius + 1):
        for d_column in range(-radius, radius + 1):
            if d_row == 0 and d_column == 0:
                continue
            source = np.roll(np.roll(channel, d_row, axis=0), d_column, axis=1)
            source_elevation = np.roll(np.roll(state.elevation, d_row, axis=0), d_column, axis=1)
            allowed = (state.elevation - source_elevation) <= float(settings["valley_max_above_m"])
            candidate = source * (
                1.0 - float(settings["valley_decay_per_ring"]) * max(abs(d_row), abs(d_column))
            )
            valley = np.maximum(valley, np.where(allowed, candidate, 0.0))
    valley[~land] = 0.0
    slope_factor = np.clip(
        (float(settings["slope_zero_pct"]) - slope)
        / (float(settings["slope_zero_pct"]) - float(settings["slope_full_pct"])),
        0.0,
        1.0,
    )
    warmth = np.clip(
        (gdd - float(settings["warmth_min_gdd"]))
        / (float(settings["warmth_full_gdd"]) - float(settings["warmth_min_gdd"])),
        0.0,
        1.0,
    )
    dryness = np.clip(
        (rainfall - float(settings["dryness_min_mm"]))
        / (float(settings["dryness_full_mm"]) - float(settings["dryness_min_mm"])),
        0.0,
        1.0,
    )
    wetness = np.clip(
        1.0
        - float(settings["wetness_falloff"])
        * (rainfall - float(settings["wetness_full_mm"]))
        / (float(settings["wetness_zero_mm"]) - float(settings["wetness_full_mm"])),
        float(settings["wetness_floor"]),
        1.0,
    )
    climate = warmth * dryness * wetness
    fertility = np.zeros_like(gdd)
    terrain_values = settings["terrain_values"]
    for name in NAMES[PASTURE:]:
        base, valley_bonus = terrain_values[name]
        mask = state.terrain == NAMES.index(name)
        fertility[mask] = float(base) + float(valley_bonus) * valley[mask] / 1000.0
    fertility = np.floor(np.clip(fertility * slope_factor * climate, 0.0, 1000.0))
    fertility[~land | glacier] = 0.0
    return fertility


def check_bounds(layer: str, values: Array, lower: float, upper: float) -> None:
    """Fail with the first offending layer value and row/column before encoding."""
    invalid = ~np.isfinite(values) | (values < lower) | (values > upper)
    if invalid.any():
        row, column = np.argwhere(invalid)[0]
        raise SystemExit(
            f"{layer} cell ({row}, {column}) value {values[row, column]!r} outside {lower}..={upper}"
        )


def compute_layers(state: State, settings: dict[str, Any]) -> None:
    """Compute and write fertility, glacier and climate layers from the edited terrain."""
    if set(settings) != set(SETTINGS):
        raise SystemExit(f"[settings]: expected exactly {SETTINGS}, found {sorted(settings)}")
    climate_settings = settings["climate"]
    fertility_settings = settings["fertility"]
    land = state.land()
    edge = np.zeros_like(land)
    edge[0, :] = edge[-1, :] = True
    edge[:, 0] = edge[:, -1] = True
    offending = land & edge
    if offending.any():
        row, column = np.argwhere(offending)[0]
        raise SystemExit(f"terrain crop edge must be water; first offending cell ({row}, {column})")
    mean, _half, gdd, rainfall, glacier = climate_fields(state, climate_settings)
    fertility = fertility_field(state, fertility_settings, gdd, rainfall, glacier)
    check_bounds("climate_temperature", mean, -60.0, 50.0)
    check_bounds("climate_gdd", gdd, 0.0, 12_000.0)
    check_bounds("climate_rainfall", rainfall, 0.0, 15_000.0)
    check_bounds("fertility", fertility, 0.0, 1000.0)
    temperature_stored = (
        np.where(mean * 10 >= 0, np.floor(mean * 10 + 0.5), np.ceil(mean * 10 - 0.5)) + 1000
    )
    gdd_stored = np.floor(gdd + 0.5)
    rainfall_stored = np.floor(rainfall + 0.5)
    fertility_stored = np.minimum(250, np.floor((fertility + 2.0) / 4.0))
    for name, values, lower, upper in (
        ("climate_temperature", temperature_stored, 400, 1500),
        ("climate_gdd", gdd_stored, 0, 12000),
        ("climate_rainfall", rainfall_stored, 0, 15000),
        ("fertility", fertility_stored, 0, 250),
    ):
        check_bounds(name, values, lower, upper)
    output = {
        "fertility.png": fertility_stored.astype(np.uint8),
        "climate_glacier.png": glacier.astype(np.uint8) * 255,
        "climate_temperature.png": temperature_stored.astype(np.uint16),
        "climate_gdd.png": gdd_stored.astype(np.uint16),
        "climate_rainfall.png": rainfall_stored.astype(np.uint16),
    }
    for filename, values in output.items():
        Image.fromarray(values).save(state.directory / filename)
    state.fertility = fertility
    state.glacier = glacier


def apply_settings(state: State, settings: dict[str, Any]) -> None:
    """Run the configured climate and fertility derivation once, after all terrain edits."""
    compute_layers(state, settings)


def check_edits(edits: list[dict[str, Any]]) -> None:
    """Reject an edit table that is malformed, naming the edit."""
    seen: set[str] = set()
    for position, edit in enumerate(edits, start=1):
        identifier = edit.get("id", f"#{position}")
        unknown = sorted(set(edit) - FIELDS)
        missing = [field for field in REQUIRED if field not in edit]
        if "tidy" not in edit and "region" not in edit:
            missing.append("region")
        if unknown or missing or identifier in seen or edit.get("paint") not in NAMES:
            message = (
                f"edit {identifier}: unknown fields {unknown}, missing {missing}, "
                f"duplicate id or paint not one of {NAMES}"
            )
            raise SystemExit(message)
        seen.add(identifier)


def shares(state: State) -> list[str]:
    """The share of land (not lake) by terrain and nonzero computed fertility."""
    land = state.land()
    total = max(int(land.sum()), 1)
    lines = [f"land cells {total}"]
    for index in range(PASTURE, len(NAMES)):
        count = int(((state.terrain == index) & land).sum())
        lines.append(f"  {NAMES[index]}: {count} ({100 * count / total:.1f} %)")
    fertility = getattr(state, "fertility", None)
    if fertility is not None:
        fertile = int(((fertility > 0) & land).sum())
        threshold = int(((fertility >= state.display_usable_pm) & land).sum())
        lines.append(f"  fertility > 0: {fertile} ({100 * fertile / total:.1f} % of land)")
        lines.append(
            f"  fertility >= {state.display_usable_pm}: {threshold} ({100 * threshold / total:.1f} % of land)"
        )
    return lines


def patches(state: State) -> list[str]:
    """How large the bog and forest patches are: the number, the smallest, the median and the largest."""
    lines = []
    for index in (BOG, FOREST):
        sizes = sorted(len(patch) for patch in components(state.terrain == index))
        if sizes:
            lines.append(
                f"  {NAMES[index]} patches: {len(sizes)}, smallest {sizes[0]}, median {sizes[len(sizes) // 2]}, largest {sizes[-1]}"
            )
    return lines


def apply(out: Path, *, force: bool) -> None:
    """Applies `edits.toml` to the layers in `out`."""
    source = out / "edits.toml"
    marker = out / "edits.applied"
    if marker.exists() and not force:
        message = "edits.applied exists: the edits were already applied to these files; pass --force to apply them again"
        raise SystemExit(message)
    document = tomllib.loads(source.read_text(encoding="utf-8"))
    edits: list[dict[str, Any]] = document.get("edit", [])
    check_edits(edits)
    state = State(out)
    report = ["draft:", *shares(state)]
    for edit in edits:
        report.append(f"{edit['id']}: {apply_edit(state, edit)} cells -> {edit['paint']}")
    apply_settings(state, document.get("settings", {}))
    state.write(out)
    marker.write_text(
        f"{hashlib.sha256(source.read_bytes()).hexdigest()}  edits.toml\n", encoding="utf-8"
    )
    print(
        "\n".join([*report, f"{len(edits)} edits applied; after:", *shares(state), *patches(state)])
    )


def main() -> None:
    """Parses the command line and applies."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", maxsplit=1)[0])
    parser.add_argument(
        "--out", type=Path, default=HERE, help="the directory that holds the layers"
    )
    parser.add_argument(
        "--force", action="store_true", help="apply again over files that already have the edits"
    )
    parser.add_argument("--self-test", action="store_true", help="run stdlib-only T_ela vectors")
    arguments = parser.parse_args()
    if arguments.self_test:
        _run_ela_self_test()
        return
    apply(arguments.out, force=arguments.force)


if __name__ == "__main__":
    main()
