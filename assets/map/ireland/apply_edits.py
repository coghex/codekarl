"""Applies `edits.toml`, the owner-delegated corrections toward medieval Ireland, to the first draft. Deterministic: no noise, no chance.

    python3 apply_edits.py [--out DIR] [--force]

Reads `terrain.png`, `usable.png`, `elevation.png`, `map.toml`, `terrain.toml` and `rivers.toml` from `--out` (default: next to this
script), applies every `[[edit]]` of `edits.toml` there in file order (each sees what the earlier ones left) and then its `[settings]`,
and writes the three images back. It records the SHA-256 of the `edits.toml` it applied in `edits.applied` and **refuses to run again
on the same files** unless `--force` is given. Running `derive_elevation.py` or `draft_terrain.py` with `--force` regenerates the draft
and removes `edits.applied`, so the whole chain is: derive, draft, apply. Needs `numpy` and `Pillow`.

The target period is "medieval, before the 17th-century clearances and the 18th-20th-century drainage and reclamation". The owner's
rule for how bog, forest and usable land are placed (2026-11-05): "noise shouldnt be used to place these, we need to expand existing
features in an intelligent way". So they **grow** from features that exist, by rules that stop on terrain (slope, elevation, distance
from the seed), not on chance. Nothing here draws a random or noise value.

An edit is one change with the reason for it; `SOURCES.md`'s guesswork log cites it by `id`. There are three kinds:

```toml
[[edit]]                             # 1. paint: every cell of the region that passes `where`
id = "wexford-north-slob-sea"
area = "North Slob, Wexford Harbour"
reason = "embanked and drained in the 1840s-50s"
confidence = "high"                  # free text
paint = "sea"                        # sea | lake | pasture | forest | bog | marsh | mountain
region = { polygon = [[lat, lon], ...] }   # or { box = [south, north, west, east] } or { circle = [lat, lon, radius_km] }
where = { terrain = ["pasture"], elevation_min = 0, elevation_max = 4, slope_min = 0.0, slope_max = 0.02,
          near_water_max = 8, far_from_water_min = 20 }
depth = 2                            # paint = sea only: the depth in metres the cells get (default 2)
usable = "false"                     # optional: keep (default), true or false, for the changed cells

[[edit]]                             # 2. grow: spread from existing features
id = "bog-ulster-lowlands"
...
paint = "bog"
region = { ... }
where = { ... }                      # the STOP RULE: the only cells the growth may take
grow = { from = ["bog"], seed_where = { elevation_min = 200, slope_max = 0.08 }, share = 0.25, max_distance = 30, order = "wet",
         seed_points = [[lat, lon]], seed_radius = 2, seed_over = ["pasture", "forest", "marsh"] }

[[edit]]                             # 3. tidy: remove fragments and fill real holes
id = "tidy-bog"
area = "..."
reason = "..."
paint = "bog"
tidy = { min_cells = 12, back = "pasture", keep_seed_points = true }

[settings]                           # optional, once, after every edit
usable = "suitability"
usable_shares = { pasture = 0.32, forest = 0.28, bog = 0.20, marsh = 0.15, mountain = 0.08 }
usable_relative_radius = 15          # elevation is ranked against the mean within this many cells
usable_water = ["sea", "lake", "rivers"]
usable_water_cap = 12                # cells: farther from water than this ranks the same
usable_smooth = 4                    # cells over which the suitability is box-blurred
usable_local_radius = 30             # optional: the box mean of the suitability within this many cells is subtracted before thresholding
usable_min_patch = 20                # usable and unusable patches smaller than this many cells merge into their surroundings
```

* **paint** changes every cell of the region that passes `where`: the terrain it has now (`terrain`), its elevation in metres, its slope
  (rise over run) and its distance in cells to the nearest water (sea or lake), at most or at least a number. Painting sea sets the
  elevation to `-depth` (the land mask is elevation 1 m or more); painting a land type on a cell that is sea raises it to 2 m; a lake
  keeps its elevation. Sea is always unusable.
* **grow** takes cells that pass `where`, never any other. The seeds are the cells of the region whose terrain now is one of `from` (a
  terrain name, or `"rivers"` for the cells the rivers of `rivers.toml` pass through), the cells of the region that pass `seed_where`
  (a `where` table: a rule of the ground, not a list of places), and the cell under each of `seed_points` with its eight neighbours
  (known locations). **Named sites always count:** with `seed_radius = R` and `seed_over = [terrains]`, the cells within R cells (a square of
  `2R + 1` cells) of each seed point, inside the region, whose terrain is one of `seed_over` take the painted terrain whatever `where`
  says, and they are seeds; the growth then continues under `where`. Growth
  spreads from the seeds to eight-neighbour cells, a ring at a time counted from the seed (`max_distance` is the straight-line distance in cells from the seed the growth came from, so spreads are rounded and follow the ground), in an order that
  is a rule of the ground: `wet` takes the lowest, then flattest, ground first (a basin fills from its floor up), `near` takes the nearest
  cells first (a compact block round the seed), `dry_slope` takes the steepest, then highest, ground first (valley sides and hill
  flanks). Ties go to the lowest row-major index. It stops when `share` of the region's land (not lake or sea) is of the painted
  terrain, cells that already were counting, or when nothing eligible is left.
* **tidy** takes no region: over the whole map, patches (four-neighbour) of `paint` smaller than `min_cells` go back to `back`, and
  patches of `back` smaller than `min_cells` whose whole border is `paint` (real enclosed holes) become `paint`. With
  `keep_seed_points = true` a patch that contains, or touches within one cell, a seed point of an earlier growth with the same `paint`
  is never removed (a coastal named site's patch can be under `min_cells`).
* **usable = "suitability"**: for each terrain type its usable cells are the ones that rank best for being flat, low against their
  surroundings and near water (the sea, lakes and the rivers of `rivers.toml`): the ranks over the whole map of the elevation minus the
  mean within `usable_relative_radius` cells, of the slope and of the distance to water (capped), summed and box-blurred over
  `usable_smooth` cells (and, with `usable_local_radius`, minus its own box mean within that many cells, so each district ranks its land against its surroundings), lowest first, at the type's share (mock-up 2's). The patches follow the valleys and the lowlands.
"""

from __future__ import annotations

import argparse
import hashlib
import heapq
import math
import tomllib
from collections import deque
from pathlib import Path
from typing import Any

import numpy as np
from draft_terrain import ELEVATION_OFFSET, distance_to, terrain_keys
from ireland_grid import CELL_GROUND_M, Array, Crop
from numpy.typing import NDArray
from PIL import Image, ImageDraw

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
    "usable",
    "depth",
}
REQUIRED = ("id", "area", "reason", "paint")
SETTINGS = (
    "usable",
    "usable_shares",
    "usable_relative_radius",
    "usable_water",
    "usable_water_cap",
    "usable_smooth",
    "usable_local_radius",
    "usable_min_patch",
)
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
    """The three layers being edited, in memory."""

    def __init__(self, out: Path) -> None:
        self.directory = out
        self.crop = Crop.of_map_toml(out / "map.toml")
        self.keys = terrain_keys(out / "terrain.toml")
        with Image.open(out / "elevation.png") as image:
            self.elevation: Array = np.asarray(image, dtype=np.float64) - ELEVATION_OFFSET
        with Image.open(out / "terrain.png") as image:
            rgb = np.asarray(image.convert("RGB"))
        with Image.open(out / "usable.png") as image:
            usable_rgb = np.asarray(image.convert("RGB"))
        self.terrain = np.full(self.elevation.shape, -1, dtype=np.int8)
        for index, name in enumerate(NAMES):
            self.terrain[(rgb == self.keys[name]).all(axis=2)] = index
        if (self.terrain < 0).any():
            message = "terrain.png has a pixel that is not a key of terrain.toml"
            raise SystemExit(message)
        self.usable: Grid = (usable_rgb == self.keys["usable"]).all(axis=2)
        self.longitude, self.latitude = self.crop.centres()
        self.slope: Array = np.hypot(*np.gradient(self.elevation, CELL_GROUND_M))
        self.seed_cells_by_paint: dict[int, list[Cell]] = {}

    def write(self, out: Path) -> None:
        """Writes the three images."""
        rgb = np.zeros((*self.terrain.shape, 3), dtype=np.uint8)
        for index, name in enumerate(NAMES):
            rgb[self.terrain == index] = self.keys[name]
        mask = np.zeros((*self.terrain.shape, 3), dtype=np.uint8)
        mask[:] = self.keys["unusable"]
        mask[self.usable] = self.keys["usable"]
        stored = np.clip(self.elevation + ELEVATION_OFFSET, 0, 65535).astype(np.uint16)
        Image.fromarray(stored).save(out / "elevation.png")
        Image.fromarray(rgb).save(out / "terrain.png")
        Image.fromarray(mask).save(out / "usable.png")

    def land(self) -> Grid:
        """Land that is not a lake."""
        return (self.elevation >= 1) & (self.terrain != LAKE)

    def water(self) -> Grid:
        """The cells that are sea or lake."""
        return (self.elevation < 1) | (self.terrain == LAKE)

    def refresh_slope(self) -> None:
        """Computes the slope again (after an edit changed the elevation)."""
        self.slope = np.hypot(*np.gradient(self.elevation, CELL_GROUND_M))


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


def usable_flag(value: object, identifier: str) -> bool | None:
    """`True`, `False` or None (keep), from an entry's `usable` (the strings `"true"` and `"false"` are booleans)."""
    if value in ("keep", None):
        return None
    if value in ("true", "false", True, False):
        return value in ("true", True)
    message = f'edit {identifier}: usable is keep, true or false (use [settings] usable = "suitability" for the rest)'
    raise SystemExit(message)


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
    flag = usable_flag(edit.get("usable"), identifier)
    if paint == "sea" or flag is False:
        state.usable[cells] = False
    elif flag:
        state.usable[cells] = True
    return int(cells.sum())


def box_blur(field: Array, radius: int) -> Array:
    """The mean of the square of `2 radius + 1` cells round each cell; the edges repeat."""
    padded = np.pad(field.astype(np.float64), radius, mode="edge")
    total = np.pad(padded.cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    side = 2 * radius + 1
    return (
        total[side:, side:] - total[:-side, side:] - total[side:, :-side] + total[:-side, :-side]
    ) / (side * side)


def rank_fraction(field: Array) -> Array:
    """The rank of each cell among all cells, from 0 (lowest) to 1; equal values rank by position."""
    order = np.argsort(field, axis=None, kind="stable")
    ranks = np.empty(field.size)
    ranks[order] = np.arange(field.size)
    return (ranks / field.size).reshape(field.shape)


def river_cells(state: State) -> Grid:
    """The cells that any river of `rivers.toml` passes through."""
    rivers = tomllib.loads((state.directory / "rivers.toml").read_text(encoding="utf-8"))["river"]
    height, width = state.elevation.shape
    image = Image.new("L", (width, height), 0)
    drawing = ImageDraw.Draw(image)
    for river in rivers:
        line = [
            state.crop.cell_of(point["latitude"], point["longitude"]) for point in river["points"]
        ]
        drawing.line(line, fill=1, width=1)
    return np.asarray(image) > 0


def suitability_usable(state: State, settings: dict[str, Any]) -> Grid:
    """Usable land by suitability: per terrain type, the cells that rank best for flat, low and near water, at the type's share."""
    water = np.zeros_like(state.usable)
    kinds = settings.get("usable_water", ["sea", "lake", "rivers"])
    if "sea" in kinds:
        water |= state.elevation < 1
    if "lake" in kinds:
        water |= state.terrain == LAKE
    if "rivers" in kinds:
        water |= river_cells(state)
    cap = int(settings.get("usable_water_cap", 12))
    near = np.minimum(distance_to(water, limit=cap), cap)
    relative = state.elevation - box_blur(
        state.elevation, int(settings.get("usable_relative_radius", 15))
    )
    suitability = box_blur(
        rank_fraction(relative) + rank_fraction(state.slope) + rank_fraction(near),
        int(settings.get("usable_smooth", 4)),
    )
    if "usable_local_radius" in settings:
        suitability = suitability - box_blur(suitability, int(settings["usable_local_radius"]))
    usable = np.zeros_like(state.usable)
    shares = settings["usable_shares"]
    for name, share in shares.items():
        cells = state.terrain == NAMES.index(name)
        if cells.any():
            usable |= cells & (suitability <= np.quantile(suitability[cells], float(share)))
    return usable


def merge_small_usable(usable: Grid, land: Grid, minimum: int) -> Grid:
    """`usable` with the usable and the unusable patches smaller than `minimum` cells (on land) flipped into their surroundings."""
    if minimum <= 1:
        return usable
    merged = usable.copy()
    for wanted in (True, False):
        for patch in components((merged == wanted) & land):
            if len(patch) < minimum:
                rows, columns = zip(*patch, strict=True)
                merged[list(rows), list(columns)] = not wanted
    return merged


def apply_settings(state: State, settings: dict[str, Any]) -> None:
    """The `[settings]` table, after every edit."""
    unknown = set(settings) - set(SETTINGS)
    if unknown:
        message = f"[settings]: unknown {sorted(unknown)}"
        raise SystemExit(message)
    if settings.get("usable") == "suitability":
        state.usable = suitability_usable(state, settings)
    if "usable_min_patch" in settings:
        state.usable = merge_small_usable(
            state.usable, state.land(), int(settings["usable_min_patch"])
        )
    state.usable &= state.land()


def check_edits(edits: list[dict[str, Any]]) -> None:
    """Rejects an edit table that is malformed, naming the edit."""
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
    """The share of land (not lake) of each terrain type, and of usable land."""
    land = state.land()
    total = max(int(land.sum()), 1)
    lines = [f"land cells {total}"]
    for index in range(PASTURE, len(NAMES)):
        count = int(((state.terrain == index) & land).sum())
        lines.append(f"  {NAMES[index]}: {count} ({100 * count / total:.1f} %)")
    usable = int((state.usable & land).sum())
    lines.append(f"  usable: {usable} ({100 * usable / total:.1f} % of land)")
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
    arguments = parser.parse_args()
    apply(arguments.out, force=arguments.force)


if __name__ == "__main__":
    main()
