"""Import tooling for Ireland's rivers, loughs and tidal outlines (CK-348, slice 3 of the rivers design).

Three commands, run by hand from a checkout like the sibling scripts (`SOURCES.md`, "Water import"):

    python3 assets/map/ireland/import_waters.py fetch   --source DIR [--force]
    python3 assets/map/ireland/import_waters.py propose --source DIR --out DIR [--map DIR] [--force]
    python3 assets/map/ireland/import_waters.py build   --source DIR --out DIR [--map DIR] [--waters FILE]
                                                        [--locations FILE] [--force]

`fetch` downloads the public surveys into the cache and writes a manifest. `propose` writes a first `waters.toml`, the hand-kept
list of rivers and loughs the owner cuts and adds to. `build` reads `waters.toml`, the cache and the map directory and writes
`rivers.toml` (the new river network format), `water.png` (the cells that become sea and lough, applied by `apply_edits.py`'s
`mask` region) and `waters-report.md`. The same inputs give the same bytes. Nothing here changes Ireland's committed map data.

The rules are plain functions over plain values (lists, dicts, tuples) so that `tests/test_map_waters.py` runs them in the
project's own Python. Only reading and writing the map's images needs Pillow, and `fetch` borrows `ireland_grid.download`
(which needs numpy); both are imported where they are used.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import heapq
import json
import math
import re
import struct
import sys
import tomllib
import urllib.parse
import zipfile
import zlib
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass, field, replace
from datetime import date
from itertools import pairwise
from pathlib import Path
from typing import TYPE_CHECKING, Any
from xml.etree import ElementTree

if TYPE_CHECKING:
    from PIL import Image

HERE = Path(__file__).resolve().parent

# ---- the grid and the terrain classes ------------------------------------------------------------------------------------

EARTH_RADIUS_M = 6_378_137.0
HALF_WORLD_M = math.pi * EARTH_RADIUS_M
CELL_M = 839.0
GROUND_M_PER_DEGREE = 111_319.49
NAMES = ("sea", "lake", "pasture", "forest", "bog", "marsh", "mountain")
SEA, LAKE, PASTURE, FOREST, BOG, MARSH, MOUNTAIN = range(len(NAMES))
BLACK = (0, 0, 0)
UNCHANGED = -1
SUPERSAMPLE = 4
MOUTH_REACH_CELLS = 30
FIRST_STEP_CELLS = 3
SNAP_UNITS = 1
OPEN_SEA_REACH = 2
MIN_ORDER = 4
POINT_DECIMALS = 5
MIN_WIDTH_M = 0.5
MAX_TIDAL_WIDTH_M = 800.0
MAX_WIDTH_M = 10_000.0
HEADWATER_FLOOR_KM2 = 1.0
TIER1_KM2 = 1200.0
TIER2_KM2 = 300.0
LOUGH_MIN_KM2 = 1.0
FIXED_UNITS_PER_CELL = 32_768
MAX_ID_LENGTH = 48
PROPOSE_LENGTH_M = 15_000.0
PROPOSE_ORDER = 5
SNAP_LIMIT_M = 400.0
RIVER_ID = re.compile(r"[a-z0-9-]{1,40}")  # a river id leaves room for the stretch suffix
STRETCH_ID = re.compile(r"[a-z0-9-]{1,48}")  # the format's own bound for a stretch id
LatLon = tuple[float, float]
Cell = tuple[int, int]  # (column, row)
RGB = tuple[int, int, int]


class WatersError(ValueError):
    """A refusal that names the entry or file at fault."""


def project(latitude: float, longitude: float) -> tuple[float, float]:
    """Projected metres east of the antimeridian and south of the top of the projection (`ireland_grid.project`)."""
    x_m = EARTH_RADIUS_M * math.radians(longitude) + HALF_WORLD_M
    y_m = HALF_WORLD_M - EARTH_RADIUS_M * math.log(
        math.tan(math.pi / 4 + math.radians(latitude) / 2)
    )
    return x_m, y_m


def unproject(x_m: float, y_m: float) -> LatLon:
    """The latitude and longitude of a projected point."""
    latitude = math.degrees(
        2 * math.atan(math.exp((HALF_WORLD_M - y_m) / EARTH_RADIUS_M)) - math.pi / 2
    )
    return latitude, math.degrees((x_m - HALF_WORLD_M) / EARTH_RADIUS_M)


def fixed_point(point: LatLon) -> tuple[int, int]:
    """A point in the map's fixed-point units (32,768 a cell), exactly as the loader's `ck_map::project` makes it: the longitude
    is wrapped as it does, the projected metres are scaled by 32,768 / 839 and floored (a cell boundary belongs to the east or
    south cell). Format rules 2 and 3 compare these integers."""
    latitude, longitude = point
    longitude = (longitude + 180.0) % 360.0 - 180.0
    x_m, y_m = project(latitude, longitude)
    scale = FIXED_UNITS_PER_CELL / CELL_M
    return math.floor(x_m * scale), math.floor(y_m * scale)


@dataclass(frozen=True)
class Crop:
    """The crop in whole cells of the world grid; cells are (column, row) from its top-left corner."""

    origin: tuple[int, int]
    width: int
    height: int

    def cell_of(self, latitude: float, longitude: float) -> tuple[float, float]:
        """A point in cell units from the crop's corner: (column, row), fractions included."""
        x_m, y_m = project(latitude, longitude)
        return x_m / CELL_M - self.origin[0], y_m / CELL_M - self.origin[1]

    def latlon_at(self, column: float, row: float) -> LatLon:
        """The point at a position in cell units (the inverse of `cell_of`)."""
        return unproject((column + self.origin[0]) * CELL_M, (row + self.origin[1]) * CELL_M)

    def cell_containing(self, point: LatLon) -> Cell | None:
        """The cell that holds a point, or None outside the crop."""
        x, y = fixed_point(point)
        cell = (
            x // FIXED_UNITS_PER_CELL - self.origin[0],
            y // FIXED_UNITS_PER_CELL - self.origin[1],
        )
        return cell if self.holds(cell) else None

    def holds(self, cell: Cell) -> bool:
        """Whether a cell lies in the crop."""
        return 0 <= cell[0] < self.width and 0 <= cell[1] < self.height

    def row_latitude(self, row: int) -> float:
        """The latitude of a row's centre line."""
        return self.latlon_at(0, row + 0.5)[0]


def ground_m(a: LatLon, b: LatLon) -> float:
    """The ground distance between two nearby points, in metres (equirectangular)."""
    mean = math.radians((a[0] + b[0]) / 2)
    return math.hypot((b[1] - a[1]) * math.cos(mean), b[0] - a[0]) * GROUND_M_PER_DEGREE


def path_length_m(points: Sequence[LatLon]) -> float:
    """The ground length of a line."""
    return sum(ground_m(a, b) for a, b in pairwise(points))


def round_point(point: LatLon) -> LatLon:
    """A point at the written precision, so that a join is the same numbers on both sides."""
    return (round(point[0], POINT_DECIMALS), round(point[1], POINT_DECIMALS))


def clean_line(points: Iterable[LatLon]) -> list[LatLon]:
    """A line at the written precision without consecutive repeats."""
    line: list[LatLon] = []
    for point in points:
        rounded = round_point(point)
        if not line or line[-1] != rounded:
            line.append(rounded)
    return line


# ---- the terrain as the water will make it -------------------------------------------------------------------------------


@dataclass(frozen=True)
class SeaRun:
    """The run a stretch is given to open sea (CK-354 rule 4): the points added after its last survey point, and its length in cells."""

    first_step: bool  # the stretch's last point is not in a sea cell: the first point added is reached by a step that may cross land
    points: tuple[LatLon, ...]
    cells: int


class Terrain:
    """Terrain classes by row and column, with the questions the network asks of the sea and the loughs."""

    def __init__(self, crop: Crop, rows: Sequence[Sequence[int]]) -> None:
        self.crop = crop
        self.rows = rows
        self._patches: dict[Cell, int] | None = None
        self._ways: dict[Cell, list[Cell] | None] = {}

    def is_sea(self, cell: Cell) -> bool:
        """Whether a cell is in the crop and sea."""
        return self.crop.holds(cell) and self.rows[cell[1]][cell[0]] == SEA

    def is_lake(self, cell: Cell) -> bool:
        """Whether a cell is in the crop and lake."""
        return self.crop.holds(cell) and self.rows[cell[1]][cell[0]] == LAKE

    def sea_distance(self, point: LatLon, limit: int = MOUTH_REACH_CELLS) -> int | None:
        """How many cells from the point's cell to the nearest sea cell (larger of the row and column distances), within `limit`."""
        cell = self.crop.cell_containing(point)
        if cell is None:
            return None
        for radius in range(limit + 1):
            for d_r in range(-radius, radius + 1):
                step = 1 if abs(d_r) == radius else 2 * radius
                for d_c in range(-radius, radius + 1, step or 1):
                    if self.is_sea((cell[0] + d_c, cell[1] + d_r)):
                        return radius
        return None

    def open_cell(self, cell: Cell) -> bool:
        """Format rule 3 for a cell: it is sea and so is every cell of the 5 x 5 box round it that lies in the crop."""
        if not self.crop.holds(cell):
            return False
        for d_r in range(-OPEN_SEA_REACH, OPEN_SEA_REACH + 1):
            for d_c in range(-OPEN_SEA_REACH, OPEN_SEA_REACH + 1):
                other = (cell[0] + d_c, cell[1] + d_r)
                if self.crop.holds(other) and not self.is_sea(other):
                    return False
        return self.is_sea(cell)

    def open_sea(self, point: LatLon) -> bool:
        """Format rule 3: the point's cell is sea and so is every cell of the 5 x 5 box round it that lies in the crop."""
        cell = self.crop.cell_containing(point)
        return cell is not None and self.open_cell(cell)

    def nearest_sea_cell(self, cell: Cell, reach: int = FIRST_STEP_CELLS) -> Cell | None:
        """The nearest sea cell within `reach` cells (the larger of the row and column distances; ties by row, then column)."""
        for radius in range(reach + 1):
            found = [
                (cell[1] + d_r, cell[0] + d_c)
                for d_r in range(-radius, radius + 1)
                for d_c in range(-radius, radius + 1)
                if max(abs(d_r), abs(d_c)) == radius and self.is_sea((cell[0] + d_c, cell[1] + d_r))
            ]
            if found:
                row, column = min(found)
                return (column, row)
        return None

    def _way_through_sea(self, start: Cell) -> list[Cell] | None:
        """The shortest way from a sea cell to the nearest open-sea cell through four-connected sea cells, as the cells walked
        from `start` to the open one; among equal ways, the first by the fixed order of `neighbours4` (cached by start)."""
        if start in self._ways:
            return self._ways[start]
        parents: dict[Cell, Cell | None] = {start: None}
        queue = collections.deque([start])
        way: list[Cell] | None = None
        while queue and way is None:
            cell = queue.popleft()
            if self.open_cell(cell):
                way = []
                walk: Cell | None = cell
                while walk is not None:
                    way.append(walk)
                    walk = parents[walk]
                way.reverse()
                break
            for other in neighbours4(cell):
                if other not in parents and self.is_sea(other):
                    parents[other] = cell
                    queue.append(other)
        self._ways[start] = way
        return way

    def sea_run(self, point: LatLon) -> SeaRun | None:
        """CK-354 rule 4: what a stretch that ends at `point`, short of open sea and with nothing onward, is given. A first step to the
        centre of the nearest sea cell, when the point's own cell is not sea (at most `FIRST_STEP_CELLS` cells away); then the
        shortest way through four-connected sea cells to the nearest open-sea cell, written as the centres of the cells where it
        turns and of its last cell. None where there is no such way."""
        cell = self.crop.cell_containing(point)
        if cell is None:
            return None
        first: Cell | None = None
        start = cell
        if not self.is_sea(cell):
            first = self.nearest_sea_cell(cell)
            if first is None:
                return None
            start = first
        way = self._way_through_sea(start)
        if way is None:
            return None
        turns = [
            way[i]
            for i in range(1, len(way) - 1)
            if (way[i][0] - way[i - 1][0], way[i][1] - way[i - 1][1])
            != (way[i + 1][0] - way[i][0], way[i + 1][1] - way[i][1])
        ]
        written = (
            ([start, *turns, way[-1]] if len(way) > 1 else [start])
            if first is not None
            else [*turns, way[-1]]
        )
        centres = [round_point(self.crop.latlon_at(c + 0.5, r + 0.5)) for c, r in written]
        steps = (
            (max(abs(first[0] - cell[0]), abs(first[1] - cell[1])) if first else 0) + len(way) - 1
        )
        return SeaRun(first is not None, tuple(clean_line([point, *centres])[1:]), steps)

    def patch_near(self, point: LatLon) -> int | None:
        """The label of a lough (four-connected patch of lake cells) in or beside the point's cell, if any."""
        if self._patches is None:
            self._patches = {}
            label = 0
            for row in range(self.crop.height):
                for column in range(self.crop.width):
                    if self.rows[row][column] == LAKE and (column, row) not in self._patches:
                        label += 1
                        for cell in flood({(column, row)}, self.is_lake):
                            self._patches[cell] = label
        cell = self.crop.cell_containing(point)
        if cell is None:
            return None
        labels = [
            self._patches[other]
            for other in (
                (cell[0] + d_c, cell[1] + d_r) for d_r in (-1, 0, 1) for d_c in (-1, 0, 1)
            )
            if other in self._patches
        ]
        return min(labels) if labels else None


def neighbours4(cell: Cell) -> list[Cell]:
    """The four cells that share an edge with a cell."""
    return [
        (cell[0] - 1, cell[1]),
        (cell[0] + 1, cell[1]),
        (cell[0], cell[1] - 1),
        (cell[0], cell[1] + 1),
    ]


def flood(seeds: Iterable[Cell], allowed: Callable[[Cell], bool]) -> set[Cell]:
    """The cells reached from the seeds through four-neighbours that `allowed` accepts."""
    reached = {cell for cell in seeds if allowed(cell)}
    queue = collections.deque(sorted(reached))
    while queue:
        for other in neighbours4(queue.popleft()):
            if other not in reached and allowed(other):
                reached.add(other)
                queue.append(other)
    return reached


# ---- polygons as cells ---------------------------------------------------------------------------------------------------

Ring = list[LatLon]
Polygon = list[Ring]


def cover_cells(polygons: Sequence[Polygon], crop: Crop) -> set[Cell]:
    """The cells of the crop more than half inside the polygons (each ring list is a shell and its holes, even-odd).

    A cell is sampled at 4 x 4 points; more than half is more than eight of them, so exactly half is not covered.
    """
    counts: collections.Counter[Cell] = collections.Counter()
    for rings in polygons:
        edges: dict[int, list[tuple[float, float, float, float]]] = collections.defaultdict(list)
        for ring in rings:
            scaled = [
                (c * SUPERSAMPLE, r * SUPERSAMPLE) for c, r in (crop.cell_of(*p) for p in ring)
            ]
            for (x_0, y_0), (x_1, y_1) in zip(scaled, [*scaled[1:], scaled[0]], strict=True):
                if y_0 == y_1:
                    continue
                first = max(math.ceil(min(y_0, y_1) - 0.5), 0)
                last = min(math.ceil(max(y_0, y_1) - 0.5) - 1, crop.height * SUPERSAMPLE - 1)
                for sub_row in range(first, last + 1):
                    edges[sub_row].append((x_0, y_0, x_1, y_1))
        for sub_row, crossing in edges.items():
            height = sub_row + 0.5
            xs = sorted(
                x_0 + (height - y_0) * (x_1 - x_0) / (y_1 - y_0) for x_0, y_0, x_1, y_1 in crossing
            )
            for x_a, x_b in zip(xs[0::2], xs[1::2], strict=False):
                sample = max(math.ceil(x_a - 0.5), 0)
                end = min(math.ceil(x_b - 0.5) - 1, crop.width * SUPERSAMPLE - 1)
                while sample <= end:
                    column = sample // SUPERSAMPLE
                    stop = min(end, (column + 1) * SUPERSAMPLE - 1)
                    counts[(column, sub_row // SUPERSAMPLE)] += stop - sample + 1
                    sample = stop + 1
    return {cell for cell, count in counts.items() if count * 2 > SUPERSAMPLE * SUPERSAMPLE}


# ---- the hand-kept list --------------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Settings:
    """`[settings]`: the width rule, the simplification and the tier steps."""

    a: float = 2.3
    b: float = 0.4
    simplify_m: float = 8.0
    tier_steps: tuple[int, int, int] = (0, 14, 24)


@dataclass(frozen=True)
class RiverDef:
    """`[[river]]`: a river and the survey lines that make it up."""

    id: str
    name: str
    tier: int
    epa_names: tuple[str, ...] = ()
    rwb_codes: tuple[str, ...] = ()
    segments: tuple[
        str, ...
    ] = ()  # survey segment codes, for a line that has no name and no water-body code (CK-354 rule 9)


@dataclass(frozen=True)
class LoughDef:
    """`[[lough]]`: a lough taken from the survey, added or redrawn."""

    name: str
    source: str
    mode: str
    keep_existing: bool = False
    reason: str = ""


@dataclass(frozen=True)
class KeepDef:
    """`[[keep]]`: the cell that holds a point keeps its present terrain."""

    id: str
    reason: str
    point: LatLon


@dataclass(frozen=True)
class SeaDef:
    """`[[sea]]`: cells the owner adds to the tidal cells (CK-354 rule 10): a coastal outline of the survey, or the cells that hold
    points (how a mouth narrower than a cell is opened)."""

    name: str
    reason: str
    source: str | None = None
    points: tuple[LatLon, ...] = ()


@dataclass(frozen=True)
class ReservoirDef:
    """`[[reservoir]]`: a modern reservoir whose outline is not water."""

    name: str
    source: str
    reason: str


@dataclass(frozen=True)
class Correction:
    """`[[correction]]`: a width, segments to leave out, or a hand-drawn course for a river."""

    id: str
    reason: str
    confidence: str
    river: str
    stretch: str | None = None
    width_m: tuple[float, float] | None = None
    width_factor: float | None = None
    omit: tuple[str, ...] = ()
    course: tuple[LatLon, ...] = ()
    replaces: tuple[str, ...] = ()
    kind: str = "river"

    def describe(self) -> str:
        """What the correction does, for the report."""
        if self.width_m is not None:
            what = f"width {self.width_m[0]:.1f} to {self.width_m[1]:.1f} m"
        elif self.width_factor is not None:
            what = f"width times {self.width_factor:g}"
        elif self.course:
            what = f"hand-drawn course of {len(self.course)} points for {len(self.replaces)} survey segments"
        else:
            what = f"leaves out {len(self.omit)} survey segments"
        return f"{self.id}: {what} ({self.confidence}): {self.reason}"


@dataclass(frozen=True)
class Waters:
    """The parsed `waters.toml`."""

    settings: Settings = field(default_factory=Settings)
    rivers: tuple[RiverDef, ...] = ()
    loughs: tuple[LoughDef, ...] = ()
    keeps: tuple[KeepDef, ...] = ()
    reservoirs: tuple[ReservoirDef, ...] = ()
    corrections: tuple[Correction, ...] = ()
    seas: tuple[SeaDef, ...] = ()


def _entry(kind: str, table: dict[str, Any], position: int, name_key: str) -> str:
    """How an entry is named in a refusal."""
    return f"{kind} #{position} ({table.get(name_key, '?')})"


def _keys(
    table: dict[str, Any], label: str, allowed: set[str], required: Sequence[str] = ()
) -> None:
    unknown = sorted(set(table) - allowed)
    if unknown:
        message = f"waters.toml: {label} has unknown keys {unknown}"
        raise WatersError(message)
    missing = [key for key in required if key not in table]
    if missing:
        message = f"waters.toml: {label} is missing {missing}"
        raise WatersError(message)


def _text(table: dict[str, Any], key: str, label: str, *, empty: bool = False) -> str:
    value = table.get(key, "")
    if not isinstance(value, str) or (not empty and not value.strip()):
        message = f"waters.toml: {label}: {key} must be a non-empty string"
        raise WatersError(message)
    return value


def _number(
    value: object, label: str, key: str, *, low: float = 0.0, high: float = MAX_WIDTH_M
) -> float:
    if isinstance(value, bool) or not isinstance(value, int | float) or not low < value <= high:
        message = f"waters.toml: {label}: {key} must be a number above {low:g} and at most {high:g}"
        raise WatersError(message)
    return float(value)


def _point(value: object, label: str, key: str) -> LatLon:
    if (
        not isinstance(value, list)
        or len(value) != 2  # noqa: PLR2004
        or not all(isinstance(v, int | float) and not isinstance(v, bool) for v in value)
    ):
        message = f"waters.toml: {label}: {key} must be a [latitude, longitude] pair"
        raise WatersError(message)
    return (float(value[0]), float(value[1]))


def _parse_settings(table: dict[str, Any]) -> Settings:
    _keys(table, "[settings]", {"a", "b", "simplify_m", "tier_steps"})
    base = Settings()
    steps = table.get("tier_steps", list(base.tier_steps))
    valid = (
        isinstance(steps, list)
        and len(steps) == 3  # noqa: PLR2004
        and all(isinstance(s, int) and not isinstance(s, bool) and 0 <= s <= 75 for s in steps)  # noqa: PLR2004
        and steps == sorted(steps)
    )
    if not valid:
        message = "waters.toml: [settings] tier_steps must be three whole numbers from 0 to 75, none smaller than the one before"
        raise WatersError(message)
    return Settings(
        a=_number(table.get("a", base.a), "[settings]", "a"),
        b=_number(table.get("b", base.b), "[settings]", "b", high=10.0),
        simplify_m=_number(
            table.get("simplify_m", base.simplify_m), "[settings]", "simplify_m", low=-1.0
        ),
        tier_steps=(steps[0], steps[1], steps[2]),
    )


def _code_list(source: dict[str, Any], key: str, label: str) -> tuple[str, ...]:
    """A river's `source` list: an array of non-empty strings (a string, a table or a number is refused, naming the river and the key)."""
    value = source.get(key, [])
    if not isinstance(value, list) or not all(isinstance(v, str) and v for v in value):
        message = f"waters.toml: {label}: source {key} must be a list of non-empty strings"
        raise WatersError(message)
    return tuple(value)


def _parse_river(table: dict[str, Any], position: int) -> RiverDef:
    label = _entry("[[river]]", table, position, "id")
    _keys(table, label, {"id", "name", "tier", "source"}, ("id", "name", "tier", "source"))
    identifier = _text(table, "id", label)
    if not RIVER_ID.fullmatch(identifier) or identifier == "sea":
        message = f"waters.toml: {label}: id must be 1 to 40 of a-z, 0-9 and -, and not sea"
        raise WatersError(message)
    name = _text(table, "name", label, empty=True)
    if not name.strip():
        message = (
            f"waters.toml: {label}: the name is empty (a river needs a name before it can be built)"
        )
        raise WatersError(message)
    if table["tier"] not in (1, 2, 3) or isinstance(table["tier"], bool):
        message = f"waters.toml: {label}: tier must be 1, 2 or 3"
        raise WatersError(message)
    source = table["source"]
    if not isinstance(source, dict):
        message = (
            f"waters.toml: {label}: source must be a table of epa_names, rwb_codes and/or segments"
        )
        raise WatersError(message)
    _keys(source, f"{label} source", {"epa_names", "rwb_codes", "segments"})
    names, codes, segments = (
        _code_list(source, key, label) for key in ("epa_names", "rwb_codes", "segments")
    )
    if not (names or codes or segments):
        message = f"waters.toml: {label}: source needs a list of epa_names, rwb_codes or segments of non-empty strings"
        raise WatersError(message)
    return RiverDef(identifier, name, table["tier"], names, codes, segments)


def _parse_sea(table: dict[str, Any], position: int) -> SeaDef:
    label = _entry("[[sea]]", table, position, "name")
    _keys(table, label, {"name", "reason", "source", "points"}, ("name", "reason"))
    if ("source" in table) == ("points" in table):
        message = f"waters.toml: {label}: needs exactly one of source or points"
        raise WatersError(message)
    name, reason = _text(table, "name", label), _text(table, "reason", label)
    if "source" in table:
        return SeaDef(name, reason, source=_text(table, "source", label))
    points = table["points"]
    if not isinstance(points, list) or not points:
        message = f"waters.toml: {label}: points must be a list of [latitude, longitude] pairs"
        raise WatersError(message)
    return SeaDef(name, reason, points=tuple(_point(p, label, "points") for p in points))


def _parse_correction(table: dict[str, Any], position: int) -> Correction:
    label = _entry("[[correction]]", table, position, "id")
    allowed = {
        "id",
        "reason",
        "confidence",
        "river",
        "stretch",
        "width_m",
        "width_factor",
        "omit",
        "course",
        "replaces",
        "kind",
    }
    _keys(table, label, allowed, ("id", "reason", "confidence", "river"))
    actions = [key for key in ("width_m", "width_factor", "omit", "course") if key in table]
    if len(actions) != 1:
        message = (
            f"waters.toml: {label}: needs exactly one of width_m, width_factor, omit or course"
        )
        raise WatersError(message)
    fields: dict[str, Any] = {
        "id": _text(table, "id", label),
        "reason": _text(table, "reason", label),
        "confidence": _text(table, "confidence", label),
        "river": _text(table, "river", label),
    }
    if "stretch" in table:
        fields["stretch"] = _text(table, "stretch", label)
        if actions[0] not in ("width_m", "width_factor"):
            message = f"waters.toml: {label}: stretch goes only with a width"
            raise WatersError(message)
    action = actions[0]
    if action == "width_m":
        value = table["width_m"]
        pair = value if isinstance(value, list) and len(value) == 2 else [value, value]  # noqa: PLR2004
        fields["width_m"] = (_number(pair[0], label, "width_m"), _number(pair[1], label, "width_m"))
    elif action == "width_factor":
        fields["width_factor"] = _number(table["width_factor"], label, "width_factor", high=100.0)
    elif action == "omit":
        codes = table["omit"]
        if (
            not isinstance(codes, list)
            or not codes
            or not all(isinstance(c, str) and c for c in codes)
        ):
            message = f"waters.toml: {label}: omit must be a list of survey segment codes"
            raise WatersError(message)
        fields["omit"] = tuple(codes)
    else:
        course = table["course"]
        replaces = table.get("replaces")
        if not isinstance(course, list) or len(course) < 2:  # noqa: PLR2004
            message = f"waters.toml: {label}: course must be a list of at least two [latitude, longitude] points"
            raise WatersError(message)
        if (
            not isinstance(replaces, list)
            or not replaces
            or not all(isinstance(c, str) and c for c in replaces)
        ):
            message = f"waters.toml: {label}: a course needs replaces, the survey segment codes it stands in for"
            raise WatersError(message)
        fields["course"] = tuple(_point(p, label, "course") for p in course)
        fields["replaces"] = tuple(replaces)
        if table.get("kind", "river") not in ("river", "lough", "tidal"):
            message = f"waters.toml: {label}: kind must be river, lough or tidal"
            raise WatersError(message)
        fields["kind"] = table.get("kind", "river")
    return Correction(**fields)


def parse_waters(text: str) -> Waters:
    """Parse `waters.toml`; an unknown key, an empty name or a dangling reference is refused, naming the entry."""
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        message = f"waters.toml: not valid TOML: {error}"
        raise WatersError(message) from error
    _keys(
        document,
        "the file",
        {"settings", "river", "lough", "keep", "reservoir", "correction", "sea"},
    )
    rivers = tuple(_parse_river(t, i) for i, t in enumerate(document.get("river", []), 1))
    loughs = []
    for position, table in enumerate(document.get("lough", []), 1):
        label = _entry("[[lough]]", table, position, "name")
        _keys(
            table,
            label,
            {"name", "source", "mode", "keep_existing", "reason"},
            ("name", "source", "mode"),
        )
        if table["mode"] not in ("added", "redrawn"):
            message = f'waters.toml: {label}: mode must be "added" or "redrawn"'
            raise WatersError(message)
        keep = table.get("keep_existing", False)
        if not isinstance(keep, bool) or (
            keep and (table["mode"] != "redrawn" or not str(table.get("reason", "")).strip())
        ):
            message = f"waters.toml: {label}: keep_existing is true or false, goes with a redrawn lough, and needs a reason"
            raise WatersError(message)
        loughs.append(
            LoughDef(
                _text(table, "name", label),
                _text(table, "source", label),
                table["mode"],
                keep,
                table.get("reason", ""),
            )
        )
    keeps = []
    for position, table in enumerate(document.get("keep", []), 1):
        label = _entry("[[keep]]", table, position, "id")
        _keys(table, label, {"id", "reason", "point"}, ("id", "reason", "point"))
        keeps.append(
            KeepDef(
                _text(table, "id", label),
                _text(table, "reason", label),
                _point(table["point"], label, "point"),
            )
        )
    reservoirs = []
    for position, table in enumerate(document.get("reservoir", []), 1):
        label = _entry("[[reservoir]]", table, position, "name")
        _keys(table, label, {"name", "source", "reason"}, ("name", "source", "reason"))
        reservoirs.append(
            ReservoirDef(
                _text(table, "name", label),
                _text(table, "source", label),
                _text(table, "reason", label),
            )
        )
    corrections = tuple(
        _parse_correction(t, i) for i, t in enumerate(document.get("correction", []), 1)
    )
    seas = tuple(_parse_sea(t, i) for i, t in enumerate(document.get("sea", []), 1))
    waters = Waters(
        _parse_settings(document.get("settings", {})),
        rivers,
        tuple(loughs),
        tuple(keeps),
        tuple(reservoirs),
        corrections,
        seas,
    )
    _check_references(waters)
    return waters


def _check_references(waters: Waters) -> None:
    """Ids are unique, names are not repeated, a survey name belongs to one river and a correction names a river."""
    ids = [r.id for r in waters.rivers]
    for label, values in (
        ("river id", ids),
        ("river name", [r.name.casefold() for r in waters.rivers]),
        ("correction id", [c.id for c in waters.corrections]),
        ("keep id", [k.id for k in waters.keeps]),
        ("sea name", [x.name.casefold() for x in waters.seas]),
    ):
        repeated = sorted(v for v, n in collections.Counter(values).items() if n > 1)
        if repeated:
            message = f"waters.toml: {label} {repeated[0]!r} is repeated"
            raise WatersError(message)
    owners: dict[str, str] = {}
    for river in waters.rivers:
        for source in (*river.epa_names, *river.rwb_codes):
            if owners.setdefault(source, river.id) != river.id:
                message = (
                    f"waters.toml: source {source!r} is in rivers {owners[source]} and {river.id}"
                )
                raise WatersError(message)
    segment_owners: dict[str, str] = {}
    for river in waters.rivers:
        for code in river.segments:
            if segment_owners.setdefault(code, river.id) != river.id:
                message = f"waters.toml: segment {code!r} is in the segments of rivers {segment_owners[code]} and {river.id}"
                raise WatersError(message)
    for correction in waters.corrections:
        if correction.river not in ids:
            message = f"waters.toml: correction {correction.id}: river {correction.river!r} is not in the list"
            raise WatersError(message)


def toml_text(value: str) -> str:
    """A TOML basic string."""
    return json.dumps(value, ensure_ascii=False)


# ---- the surveys ---------------------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Seg:
    """A survey river segment, oriented as the survey gives it."""

    code: str
    source: str
    order: int
    points: tuple[LatLon, ...]
    name: str | None = None
    water_body: str | None = None
    continua: str | None = None
    lake_code: str | None = None
    river_hint: str | None = None
    course: str | None = None
    course_kind: str = "river"
    twin: str | None = (
        None  # a line in both surveys (rule 7): the Irish survey's segment code, `code` being DAERA's
    )
    twin_order: int = 0  # ... and the stream order DAERA gives it, `order` being the Irish survey's

    @property
    def length_m(self) -> float:
        """The ground length of the segment."""
        return path_length_m(self.points)

    @property
    def codes(self) -> tuple[str, ...]:
        """Every segment code the line answers to: one, or two for a line that is in both surveys."""
        return (self.code, self.twin) if self.twin else (self.code,)

    @property
    def irish(self) -> bool:
        """Whether the Irish survey has the line (alone, or as one line with DAERA's)."""
        return self.source in ("epa", "both")

    @property
    def epa_order(self) -> int:
        """The stream order the Irish survey gives the line, 0 where it does not have it."""
        return self.order if self.source in ("epa", "both") else 0

    @property
    def ni_order(self) -> int:
        """The stream order DAERA gives the line, 0 where it does not have it."""
        return (
            self.order if self.source == "ni" else self.twin_order if self.source == "both" else 0
        )

    def label(self) -> str:
        """The line's code, with its second code where it has one, for a message."""
        return " / ".join(self.codes)


@dataclass(frozen=True)
class Outline:
    """A surveyed lake or tidal outline: a code, a name, its area and its polygons (each a shell and holes)."""

    code: str
    name: str
    area_km2: float
    polygons: tuple[Polygon, ...]
    source: str


def lines_of(geometry: dict[str, Any] | None) -> list[list[LatLon]]:
    """The lines of a GeoJSON geometry as (latitude, longitude)."""
    if not geometry:
        return []
    lines = {
        "LineString": [geometry.get("coordinates")],
        "MultiLineString": geometry.get("coordinates"),
    }.get(geometry.get("type", ""), [])
    return [[(c[1], c[0]) for c in line] for line in lines or [] if line]


def polygons_of(geometry: dict[str, Any] | None) -> list[Polygon]:
    """The polygons of a GeoJSON geometry; each is its rings in (latitude, longitude)."""
    if not geometry:
        return []
    kind = geometry.get("type")
    shapes = (
        [geometry["coordinates"]]
        if kind == "Polygon"
        else geometry["coordinates"]
        if kind == "MultiPolygon"
        else []
    )
    return [[[(c[1], c[0]) for c in ring] for ring in shape] for shape in shapes]


def epa_segments(collection: dict[str, Any]) -> list[Seg]:
    """The Irish EPA river network routes (`Segment_Code`, `EPA_Name`, `ORDER_`, `Continua`, `Lake_Waterbody_Code`)."""
    segments = []
    for feature in collection["features"]:
        props = feature["properties"]
        lines = lines_of(feature.get("geometry"))
        for number, line in enumerate(lines, 1):
            code = props["Segment_Code"] + (f"#{number}" if len(lines) > 1 else "")
            segments.append(
                Seg(
                    code,
                    "epa",
                    int(props["ORDER_"]),
                    tuple(line),
                    props.get("EPA_Name") or None,
                    None,
                    props.get("Continua") or None,
                    props.get("Lake_Waterbody_Code") or None,
                )
            )
    return segments


NI_MARKS = {
    "L": "L",
    "T": "T",
}  # DAERA's SEGMENT_TYPE `Y` is a plain river line; `L` is a lough line and `T` a tidal one


def ni_segments(collection: dict[str, Any]) -> list[Seg]:
    """DAERA's river segments (`RWB_CD`, `STRAHLER`, `SEGMENT_TYPE`); the survey gives no names. A line with an empty water-body code
    has none (the tidal Foyle, the Quoile's last reach)."""
    segments = []
    for feature in collection["features"]:
        props = feature["properties"]
        identifier = props.get("OBJECTID", feature.get("id"))
        for number, line in enumerate(lines_of(feature.get("geometry")), 1):
            suffix = f"#{number}" if number > 1 else ""
            segments.append(
                Seg(
                    f"{props['RWB_CD']}.{identifier}{suffix}",
                    "ni",
                    int(props["STRAHLER"]),
                    tuple(line),
                    None,
                    props["RWB_CD"] or None,
                    NI_MARKS.get(str(props.get("SEGMENT_TYPE") or "")),
                )
            )
    return segments


def outlines(
    collection: dict[str, Any],
    source: str,
    code: str,
    name: str,
    area: Callable[[dict[str, Any]], float],
) -> list[Outline]:
    """Lake or tidal outlines from a feature collection, in a fixed order."""
    result = []
    for feature in collection["features"]:
        props = feature["properties"]
        polygons = polygons_of(feature.get("geometry"))
        if polygons:
            result.append(
                Outline(
                    str(props.get(code) or "?"),
                    str(props.get(name) or "?"),
                    area(props),
                    tuple(polygons),
                    source,
                )
            )
    return sorted(result, key=lambda o: (o.name, o.code))


# ---- stretches -----------------------------------------------------------------------------------------------------------


@dataclass
class Edge:
    """A survey segment with its river, its kind and its points running downstream."""

    seg: Seg
    river: str | None
    kind: str
    points: list[LatLon]
    synthetic: str | None = None

    @property
    def start(self) -> LatLon:
        """The upstream node."""
        return self.points[0]

    @property
    def end(self) -> LatLon:
        """The downstream node."""
        return self.points[-1]

    @property
    def length_m(self) -> float:
        """The ground length."""
        return path_length_m(self.points)


@dataclass
class Stretch:
    """A stretch of the network (format rule 2): one river, one kind, running downstream."""

    id: str = ""
    river: str = ""
    kind: str = "river"
    points: list[LatLon] = field(default_factory=list)
    ends: str = "sea"
    length_m: float = 0.0
    order: int = 0
    codes: tuple[str, ...] = ()
    start: LatLon = (0.0, 0.0)
    finish: LatLon = (0.0, 0.0)
    distance: float = 0.0
    note: str = ""
    course: str | None = None
    edges: list[Edge] = field(default_factory=list)
    survey_end: LatLon | None = (
        None  # the last survey point, where the run to open sea (rule 4) starts
    )
    run: tuple[LatLon, ...] = ()  # the points the tool added after it


@dataclass(frozen=True)
class Contest:
    """A line that two rivers' entries both reach (CK-354 rules 7 and 9): `river` has it, `other` lists it another way."""

    seg: Seg
    river: str
    other: str
    how: str  # how `river` has it: "segments", "name" or "water body"


@dataclass
class Network:
    """The built network: stretches in their written order, what the tool added, and the rivers it could not build."""

    stretches: list[Stretch] = field(default_factory=list)
    rivers: list[str] = field(default_factory=list)
    straight_lough: list[str] = field(default_factory=list)
    sea_runs: dict[str, float] = field(
        default_factory=dict
    )  # stretch id -> length in metres of the run the tool added
    contests: list[Contest] = field(default_factory=list)
    lough_as_river: dict[str, str] = field(default_factory=dict)
    failed: dict[str, str] = field(default_factory=dict)
    blockers: dict[str, list[tuple[str, str]]] = field(default_factory=dict)


def seg_kind(seg: Seg, reservoir_lakes: frozenset[str]) -> str:
    """ "lough" or "tidal" where the survey marks the line so, otherwise "river"; a line through a reservoir is a river."""
    if seg.course is not None:
        return seg.course_kind
    if seg.continua == "L":
        return "river" if seg.lake_code in reservoir_lakes else "lough"
    return "tidal" if seg.continua in ("T", "C") else "river"


def thin(
    points: Sequence[LatLon], tolerance_m: float, pinned: Iterable[LatLon] = ()
) -> list[LatLon]:
    """Douglas-Peucker thinning to a ground distance, keeping the two ends and any pinned point."""
    if len(points) < 3 or tolerance_m <= 0:  # noqa: PLR2004
        return list(points)
    mean = math.radians(sum(p[0] for p in points) / len(points))
    flat = [
        (p[1] * math.cos(mean) * GROUND_M_PER_DEGREE, p[0] * GROUND_M_PER_DEGREE) for p in points
    ]
    pins = set(pinned)
    must = [i for i, p in enumerate(points) if i in (0, len(points) - 1) or p in pins]
    keep = set(must)
    for low, high in pairwise(must):
        stack = [(low, high)]
        while stack:
            first, last = stack.pop()
            worst, split = 0.0, -1
            for index in range(first + 1, last):
                distance = _segment_distance(flat[index], flat[first], flat[last])
                if distance > worst:
                    worst, split = distance, index
            if worst > tolerance_m:
                keep.add(split)
                stack += [(first, split), (split, last)]
    return [p for i, p in enumerate(points) if i in keep]


def _segment_distance(
    point: tuple[float, float], a: tuple[float, float], b: tuple[float, float]
) -> float:
    d_x, d_y = b[0] - a[0], b[1] - a[1]
    square = d_x * d_x + d_y * d_y
    share = (
        0.0
        if square == 0
        else max(0.0, min(1.0, ((point[0] - a[0]) * d_x + (point[1] - a[1]) * d_y) / square))
    )
    return math.hypot(point[0] - a[0] - share * d_x, point[1] - a[1] - share * d_y)


class _Components:
    """Union-find over nodes."""

    def __init__(self) -> None:
        self.parent: dict[LatLon, LatLon] = {}

    def find(self, node: LatLon) -> LatLon:
        """The representative of a node's component (the smallest node it has met)."""
        self.parent.setdefault(node, node)
        while self.parent[node] != node:
            self.parent[node] = self.parent[self.parent[node]]
            node = self.parent[node]
        return node

    def union(self, a: LatLon, b: LatLon) -> None:
        """Joins two components under the smaller representative."""
        root_a, root_b = self.find(a), self.find(b)
        if root_a != root_b:
            self.parent[max(root_a, root_b)] = min(root_a, root_b)


def _snap_ends(segs: list[Seg]) -> list[Seg]:
    """Ends that differ by one unit of the written precision (about a metre: a rounding edge) become the same node."""
    units: dict[tuple[int, int], LatLon] = {}
    for seg in segs:
        for node in (seg.points[0], seg.points[-1]):
            units[(round(node[0] * 10**POINT_DECIMALS), round(node[1] * 10**POINT_DECIMALS))] = node
    components = _Components()
    for (lat, lon), node in units.items():
        for d_lat in range(-SNAP_UNITS, SNAP_UNITS + 1):
            for d_lon in range(-SNAP_UNITS, SNAP_UNITS + 1):
                other = units.get((lat + d_lat, lon + d_lon))
                if other is not None:
                    components.union(node, other)
    snapped = []
    for seg in segs:
        first, last = components.find(seg.points[0]), components.find(seg.points[-1])
        line = clean_line([first, *seg.points[1:-1], last])
        if len(line) >= 2:  # noqa: PLR2004
            snapped.append(replace(seg, points=tuple(line)))
    return snapped


PAIR_TOLERANCE_M = 2.0
PAIR_TOLERANCE_SHARE = 0.01


def pair_surveys(segs: Sequence[Seg]) -> list[Seg]:
    """Rule 7 (CK-354): a segment of the Irish survey and one of DAERA's with the same two end points and the same length, to
    within 2 m or 1 % of the Irish line's length (the larger), are one line. Pairing is one to one: where more than one line on
    either side fits, the pair with the closest lengths is taken first, then by code, and a line is paired at most once. The one
    line keeps DAERA's points, water-body code and segment code, and the Irish survey's segment code, name, mark and lake code
    where it has them (the Irish mark, then DAERA's)."""
    by_ends: dict[tuple[LatLon, LatLon], list[Seg]] = collections.defaultdict(list)
    for seg in segs:
        low, high = sorted((seg.points[0], seg.points[-1]))
        by_ends[(low, high)].append(seg)
    pairs: list[tuple[float, str, str]] = []
    for group in by_ends.values():
        irish = [s for s in group if s.source == "epa"]
        daera = [s for s in group if s.source == "ni"]
        for epa in irish:
            epa_length = epa.length_m
            for ni in daera:
                gap = abs(epa_length - ni.length_m)
                if gap <= max(PAIR_TOLERANCE_M, PAIR_TOLERANCE_SHARE * epa_length):
                    pairs.append((gap, epa.code, ni.code))
    by_code = {s.code: s for s in segs}
    taken: set[str] = set()
    merged: dict[str, Seg] = {}
    for _gap, epa_code, ni_code in sorted(pairs):
        if epa_code in taken or ni_code in taken:
            continue
        taken.update((epa_code, ni_code))
        epa, ni = by_code[epa_code], by_code[ni_code]
        merged[ni_code] = Seg(
            ni.code,
            "both",
            epa.order,
            ni.points,
            epa.name,
            ni.water_body,
            epa.continua or ni.continua,
            epa.lake_code,
            twin=epa.code,
            twin_order=ni.order,
        )
    return [
        merged.get(seg.code, seg) for seg in segs if not (seg.source == "epa" and seg.code in taken)
    ]


def _prepare(segments: Sequence[Seg], corrections: Sequence[Correction]) -> list[Seg]:
    """Segments at the written precision, the lines both surveys have made one, without the ones a correction leaves out, plus the
    hand-drawn courses."""
    return _corrected(_cleaned(segments), corrections)


def _cleaned(segments: Sequence[Seg]) -> list[Seg]:
    """Segments at the written precision, with their ends snapped and the lines that are in both surveys paired (rule 7)."""
    cleaned = []
    for seg in segments:
        line = clean_line(seg.points)
        if len(line) >= 2:  # noqa: PLR2004
            cleaned.append(replace(seg, points=tuple(line)))
    return pair_surveys(_snap_ends(cleaned))


def _corrected(cleaned: Sequence[Seg], corrections: Sequence[Correction]) -> list[Seg]:
    """The segments without the ones a correction leaves out, plus the hand-drawn courses; a segment is named by either of its
    codes or by its water body."""

    def matches(seg: Seg, codes: Iterable[str]) -> bool:
        return any(c in codes for c in seg.codes) or (
            seg.water_body is not None and seg.water_body in codes
        )

    for correction in corrections:
        codes = set(correction.omit) | set(correction.replaces)
        if codes and not any(matches(seg, codes) for seg in cleaned):
            message = (
                f"correction {correction.id}: the survey has none of its segments {sorted(codes)}"
            )
            raise WatersError(message)
    result = list(cleaned)
    for correction in corrections:
        codes = set(correction.omit) | set(correction.replaces)
        gone = [seg for seg in result if matches(seg, codes)]
        result = [seg for seg in result if not matches(seg, codes)]
        if correction.course:
            result.append(_course_seg(correction, gone, result))
    return result


def _course_seg(correction: Correction, replaced: Sequence[Seg], rest: Sequence[Seg]) -> Seg:
    """A hand-drawn course, its ends snapped to the nodes where the segments it stands in for met the rest."""
    kept_nodes = {n for seg in rest for n in (seg.points[0], seg.points[-1])}
    boundary = sorted({n for seg in replaced for n in (seg.points[0], seg.points[-1])} & kept_nodes)
    points = clean_line(correction.course)
    if len(boundary) == 1:
        # rule 6: the segments it stands in for met the rest of the river at one point only, so the course starts there and its far
        # end is free: it is the river's last reach
        node = boundary[0]
        if ground_m(node, points[-1]) < ground_m(node, points[0]):
            points.reverse()
        if ground_m(node, points[0]) > SNAP_LIMIT_M:
            message = f"correction {correction.id}: the course does not meet the river where the survey segments it replaces did"
            raise WatersError(message)
        points = clean_line([node, *points[1:]])
    elif boundary:
        first = min(boundary, key=lambda n: (ground_m(n, points[0]), n))
        others = [n for n in boundary if n != first] or boundary
        last = min(others, key=lambda n: (ground_m(n, points[-1]), n))
        if (
            first == last
            or max(ground_m(first, points[0]), ground_m(last, points[-1])) > SNAP_LIMIT_M
        ):
            message = f"correction {correction.id}: the course does not meet the river where the survey segments it replaces did"
            raise WatersError(message)
        points = [first, *points[1:-1], last]
    order = max((seg.order for seg in replaced), default=MIN_ORDER)
    return Seg(
        f"course:{correction.id}",
        "course",
        order,
        tuple(points),
        river_hint=correction.river,
        course=correction.id,
        course_kind=correction.kind,
    )


def _piece_mouth(ends: Sequence[LatLon], terrain: Terrain) -> LatLon | None:
    """Rule 2: the one mouth of a piece that reaches no open sea: of its loose ends, the one with the shortest way to open sea by rule 4
    (ties to the first by position); where none has one, the one nearest a sea cell within 30 cells (a river's survey line can stop
    short of the sea, at the head of its estuary). None where the piece has no sea in reach."""
    ways = [(run.cells, n) for n in ends if (run := terrain.sea_run(n)) is not None]
    if ways:
        return min(ways)[1]
    reach = [(d, n) for n in ends if (d := terrain.sea_distance(n)) is not None]
    return min(reach)[1] if reach else None


def _join_loughs(raws: Sequence[Seg], terrain: Terrain) -> tuple[list[Seg], set[LatLon]]:
    """Joins each piece of network with no mouth to another one through the lough they both touch (the survey has no
    line through some loughs), and returns the segments with the mouths: the line ends in open sea (rule 1) and one loose end of
    each piece that reaches none (rule 2)."""
    segs = list(raws)
    degree: collections.Counter[LatLon] = collections.Counter(
        n for s in segs for n in (s.points[0], s.points[-1])
    )
    components = _Components()
    for seg in segs:
        components.union(seg.points[0], seg.points[-1])
    # rule 1 (CK-354): the end of a survey line that lies in an open-sea cell is at the sea; nothing else is a mouth for being tidal
    # or near the coast
    mouths = {n for n in degree if terrain.open_sea(n)}
    by_patch: dict[int, list[LatLon]] = collections.defaultdict(list)
    for node, count in sorted(degree.items()):
        if count == 1 and node not in mouths and (patch := terrain.patch_near(node)) is not None:
            by_patch[patch].append(node)
    changed = True
    while changed:
        changed = False
        with_mouth = {components.find(n) for n in mouths}
        for root in sorted(
            {components.find(n) for ends in by_patch.values() for n in ends} - with_mouth
        ):
            for _patch, ends in sorted(by_patch.items()):
                own = [n for n in ends if components.find(n) == root and degree[n] == 1]
                others = [n for n in ends if components.find(n) != root and degree[n] == 1]
                if own and others:
                    node = own[0]
                    other = min(others, key=lambda n: (ground_m(node, n), n))
                    segs.append(
                        Seg(
                            f"lough-join:{node[0]:.5f},{node[1]:.5f}",
                            "join",
                            MIN_ORDER,
                            (node, other),
                            course_kind="lough",
                            course="join",
                        )
                    )
                    degree[node] += 1
                    degree[other] += 1
                    components.union(node, other)
                    changed = True
                    break
            if changed:
                break
    ends_by_root: dict[LatLon, list[LatLon]] = collections.defaultdict(list)
    for node, count in sorted(degree.items()):
        if count == 1:
            ends_by_root[components.find(node)].append(node)
    for root in sorted(set(ends_by_root) - {components.find(n) for n in mouths}):
        if (mouth := _piece_mouth(ends_by_root[root], terrain)) is not None:
            mouths.add(mouth)
    return segs, mouths


def _orient(segs: Sequence[Seg], mouths: set[LatLon]) -> tuple[dict[LatLon, float], list[Edge]]:
    """Each node's length to the nearest mouth, and every line turned to run downstream (towards the nearer end to a mouth)."""
    links: dict[LatLon, list[tuple[LatLon, float]]] = collections.defaultdict(list)
    for seg in segs:
        length = seg.length_m
        links[seg.points[0]].append((seg.points[-1], length))
        links[seg.points[-1]].append((seg.points[0], length))
    distance: dict[LatLon, float] = {}
    queue = [(0.0, node) for node in sorted(mouths)]
    heapq.heapify(queue)
    while queue:
        far, node = heapq.heappop(queue)
        if node in distance:
            continue
        distance[node] = far
        for other, length in links[node]:
            if other not in distance:
                heapq.heappush(queue, (far + length, other))
    edges = []
    for seg in sorted(segs, key=lambda s: (s.points[0], s.points[-1], s.code)):
        first, last = seg.points[0], seg.points[-1]
        d_first, d_last = distance.get(first, math.inf), distance.get(last, math.inf)
        forward = d_first > d_last if d_first != d_last else first > last
        edges.append(
            Edge(
                seg,
                seg.river_hint,
                "river",
                list(seg.points) if forward else list(reversed(seg.points)),
                "lough" if seg.source == "join" else None,
            )
        )
    return distance, edges


def _claim(edges: list[Edge], rivers: Sequence[RiverDef], known: frozenset[str]) -> list[Contest]:
    """Gives each edge its river, in this order (CK-354 rule 9): the river that lists it in `segments` (by either of its codes);
    failing that, the river that lists its Irish name; failing that, the river that lists its water body; failing that, a river
    that runs on into it: the unnamed Irish lines (through loughs and tidal water, and the few the EPA does not name) and the
    joins the tool added that run on from a river's end, the larger river taking a line two rivers both reach. Returns the lines
    that two rivers' entries both reach, with the river that has each. Raises `WatersError` for a code the survey does not have and
    for a line listed in the `segments` of two rivers."""
    by_segment: dict[str, str] = {}
    for river in rivers:
        for code in river.segments:
            if code not in known:
                message = f"river {river.id}: the survey has no segment {code!r}"
                raise WatersError(message)
            by_segment[code] = river.id
    by_name = {n: r.id for r in rivers for n in r.epa_names}
    by_code = {c: r.id for r in rivers for c in r.rwb_codes}
    contests: list[Contest] = []
    for edge in edges:
        if edge.river is not None:
            continue
        listed = [(c, by_segment[c]) for c in edge.seg.codes if c in by_segment]
        if len({river for _, river in listed}) > 1:
            (code_a, river_a), (code_b, river_b) = listed[0], listed[1]
            message = f"line {edge.seg.label()} is in the segments of rivers {river_a} (as {code_a}) and {river_b} (as {code_b})"
            raise WatersError(message)
        named = by_name.get(edge.seg.name or "")
        bodied = by_code.get(edge.seg.water_body or "")
        owner, how = (
            (listed[0][1], "segments")
            if listed
            else (named, "name")
            if named
            else (bodied, "water body")
        )
        edge.river = owner
        contests += [
            Contest(edge.seg, owner or "", other, how)
            for other in sorted({r for r in (named, bodied) if r and r != owner})
        ]
    starting: dict[LatLon, list[Edge]] = collections.defaultdict(list)
    for edge in edges:
        starting[edge.start].append(edge)
    biggest = {r.id: max([e.seg.order for e in edges if e.river == r.id] or [0]) for r in rivers}
    for river in sorted((r.id for r in rivers), key=lambda i: (-biggest[i], i)):
        queue = collections.deque(sorted(e.end for e in edges if e.river == river))
        seen: set[LatLon] = set()
        while queue:
            node = queue.popleft()
            if node in seen:
                continue
            seen.add(node)
            for edge in starting[node]:
                runs_on = (edge.seg.name is None and edge.seg.irish) or edge.seg.source == "join"
                if edge.river is None and runs_on:
                    edge.river = river
                    queue.append(edge.end)
    return contests


@dataclass
class Graph:
    """The survey lines of a build, joined, oriented downstream and measured: what depends on the terrain, not on the list."""

    distance: dict[LatLon, float]
    edges: list[Edge]
    known: frozenset[str] = (
        frozenset()
    )  # every segment code the survey has, before a correction leaves any out


def prepare_graph(
    segments: Sequence[Seg], terrain: Terrain, corrections: Sequence[Correction] = ()
) -> Graph:
    """Segments at the written precision, the lines both surveys have made one, corrections applied, loughs joined, every line
    turned to run downstream."""
    cleaned = _cleaned(segments)
    known = frozenset(code for seg in cleaned for code in seg.codes)
    joined, mouths = _join_loughs(_corrected(cleaned, corrections), terrain)
    distance, edges = _orient(joined, mouths)
    return Graph(distance, edges, known)


class _RiverErrors(Exception):
    """The rivers a build could not make, each with why."""

    def __init__(
        self, problems: dict[str, str], blockers: dict[str, list[tuple[str, str]]]
    ) -> None:
        super().__init__(problems)
        self.problems = problems
        self.blockers = blockers


def build_network(  # noqa: PLR0913 -- the list, the terrain and each optional rule are separate inputs
    segments: Sequence[Seg],
    rivers: Sequence[RiverDef],
    terrain: Terrain,
    settings: Settings | None = None,
    corrections: Sequence[Correction] = (),
    reservoir_lakes: frozenset[str] = frozenset(),
    *,
    tolerant: bool = False,
    graph: Graph | None = None,
) -> Network:
    """The network of the listed rivers (design 4.1, format rules 2 and 3). Raises `WatersError` naming each river that cannot
    be built; with `tolerant` those rivers are left out and named in `Network.failed` instead."""
    failed: dict[str, str] = {}
    blockers: dict[str, list[tuple[str, str]]] = {}
    graph = graph or prepare_graph(segments, terrain, corrections)
    while True:
        listed = [r for r in rivers if r.id not in failed]
        try:
            network = _build_once(graph, listed, terrain, settings or Settings(), reservoir_lakes)
        except _RiverErrors as errors:
            if not tolerant:
                message = "; ".join(f"river {r}: {m}" for r, m in sorted(errors.problems.items()))
                raise WatersError(message) from None
            failed.update(errors.problems)
            for river_id, found in errors.blockers.items():
                known = blockers.setdefault(river_id, [])
                known += [b for b in found if b not in known]
            continue
        network.failed = failed
        network.blockers = blockers
        return network


def _close_at_sea(edges: list[Edge], terrain: Terrain) -> list[Edge]:
    """Cuts each line at its first vertex in open sea and drops the lines that lie beyond one."""
    dead: set[LatLon] = set()
    kept: list[Edge] = []
    for edge in edges:
        if terrain.open_sea(edge.start):
            dead.add(edge.end)
            continue
        cut = next((i for i, p in enumerate(edge.points) if i > 0 and terrain.open_sea(p)), None)
        if cut is not None and cut < len(edge.points) - 1:
            dead.add(edge.end)
            edge.points = edge.points[: cut + 1]
        kept.append(edge)
    while True:
        beyond = [e for e in kept if e.start in dead]
        if not beyond:
            return kept
        dead.update(e.end for e in beyond)
        kept = [e for e in kept if e.start not in dead]


def _build_once(  # noqa: PLR0912, PLR0915 -- one pass over the graph reads best in one place
    graph: Graph,
    rivers: Sequence[RiverDef],
    terrain: Terrain,
    settings: Settings,
    reservoir_lakes: frozenset[str],
) -> Network:
    distance = graph.distance
    edges = [
        Edge(e.seg, e.seg.river_hint, e.kind, list(e.points), e.synthetic) for e in graph.edges
    ]
    for edge in edges:
        edge.kind = "lough" if edge.synthetic else seg_kind(edge.seg, reservoir_lakes)
    contests = _claim(edges, rivers, graph.known)
    problems = {
        r.id: "its source matches no segment of the survey"
        for r in rivers
        if not any(e.river == r.id for e in edges)
    }
    blockers: dict[str, list[tuple[str, str]]] = {}
    selected = _close_at_sea([e for e in edges if e.river is not None], terrain)
    chosen = {id(e) for e in selected}
    starts: dict[LatLon, list[Edge]] = collections.defaultdict(list)
    enters: dict[LatLon, list[Edge]] = collections.defaultdict(list)
    leaving: dict[LatLon, list[Edge]] = collections.defaultdict(list)
    for edge in selected:
        starts[edge.start].append(edge)
        enters[edge.end].append(edge)
    for edge in edges:
        leaving[edge.start].append(edge)

    def far(node: LatLon) -> float:
        return distance.get(node, 0.0)

    main = {
        node: min(out, key=lambda e: (far(e.end), e.seg.code, e.points))
        for node, out in starts.items()
    }
    follows: dict[int, Edge] = {}
    for edge in selected:
        onward = main.get(edge.end)
        if (
            onward is not None
            and len(enters[edge.end]) == 1
            and onward.river == edge.river
            and onward.kind == edge.kind
            and edge.seg.course is None
            and onward.seg.course is None
        ):
            follows[id(edge)] = onward
    continued = {id(e) for e in follows.values()}
    stretches: list[Stretch] = []
    stretch_of: dict[int, Stretch] = {}
    for edge in sorted(
        (e for e in selected if id(e) not in continued),
        key=lambda e: (-far(e.start), e.start, e.points, e.seg.code),
    ):
        chain = [edge]
        while id(chain[-1]) in follows:
            chain.append(follows[id(chain[-1])])
        points = list(chain[0].points)
        for link in chain[1:]:
            points.extend(link.points[1:])
        stretch = Stretch(
            river=edge.river or "",
            kind=edge.kind,
            points=points,
            length_m=sum(e.length_m for e in chain),
            order=max(e.seg.order for e in chain),
            codes=tuple(e.seg.code for e in chain),
            start=edge.start,
            finish=chain[-1].end,
            distance=far(edge.start),
            edges=chain,
        )
        stretches.append(stretch)
        stretch_of.update({id(member): stretch for member in chain})
    for river in rivers:
        own = sorted(
            (s for s in stretches if s.river == river.id),
            key=lambda s: (-round(s.distance, 3), s.start, s.finish),
        )
        for number, stretch in enumerate(own, 1):
            stretch.id = f"{river.id}-{number:02d}"
    network = Network()
    for stretch in stretches:
        onward = main.get(stretch.finish)
        if onward is not None:
            stretch.ends = stretch_of[id(onward)].id
        elif terrain.open_sea(stretch.finish):
            stretch.ends = "sea"
        elif unlisted := [
            e for e in leaving[stretch.finish] if e.river is None and id(e) not in chosen
        ]:
            other = unlisted[0].seg
            at = f"{other.label()} at {stretch.finish[0]:.5f}, {stretch.finish[1]:.5f}"
            problems.setdefault(
                stretch.river,
                f"it ends at {other.name or other.water_body or other.code}, which is not in the list (segment {at}; list its name, its water body or its segment code)",
            )
            blocker = (
                ("epa_names", other.name)
                if other.name
                else ("rwb_codes", other.water_body)
                if other.water_body
                else ("segments", other.code)
            )
            if blocker not in blockers.setdefault(stretch.river, []):
                blockers[stretch.river].append(blocker)
        elif (run := terrain.sea_run(stretch.finish)) is not None:
            stretch.survey_end = stretch.finish
            stretch.run = run.points
            stretch.points.extend(run.points)
            stretch.note = "its last run to the open sea is a line through sea cells the tool added"
        else:
            problems.setdefault(stretch.river, _no_way_to_sea(stretch.finish, terrain))
    if problems:
        raise _RiverErrors(problems, blockers)
    pins = {node for node, out in starts.items() if len(out) > 1}
    for stretch in stretches:
        if any(e.synthetic for e in stretch.edges):
            stretch.note = "the survey has no line through this lough: the ends are joined by a straight line the tool added"
        # thinning keeps the stretch's last survey point and every point the run adds (rule 4): a line between two sea points can
        # cross land
        kept = pins | set(stretch.run) | ({stretch.survey_end} if stretch.survey_end else set())
        thinned = thin(stretch.points, settings.simplify_m, kept)
        if (
            stretch.kind == "lough"
            and lough_has_lake(stretch.points, terrain)
            and not lough_has_lake(thinned, terrain)
        ):
            thinned = stretch.points  # thinning would move every midpoint out of the lake
        stretch.points = thinned
    network.stretches = sorted(stretches, key=lambda s: (s.river, s.id))
    network.rivers = sorted({s.river for s in stretches})
    network.straight_lough = [s.id for s in network.stretches if any(e.synthetic for e in s.edges)]
    network.sea_runs = {
        s.id: path_length_m([s.survey_end, *s.run]) for s in network.stretches if s.survey_end
    }
    network.contests = contests
    return network


def _no_way_to_sea(finish: LatLon, terrain: Terrain) -> str:
    """Rule 5: why a stretch that ends short of open sea gets no run: its last point, and how many cells it is from the sea."""
    where = f"{finish[0]:.5f}, {finish[1]:.5f}"
    far = terrain.sea_distance(finish)
    if far == 0:
        return (
            f"it ends at {where} in sea that has no way to open sea (a pool), 0 cells from the sea"
        )
    cells = f"{far} cells" if far is not None else f"more than {MOUTH_REACH_CELLS} cells"
    return f"it ends inland at {where}, {cells} from the sea, short of open sea or a listed river: it needs a hand-drawn course or the sea it lacks"


# ---- widths --------------------------------------------------------------------------------------------------------------


def width_of(area_km2: float, a: float, b: float) -> float:
    """Design 4.5: a x area^b metres."""
    return a * area_km2**b


def final_width(width: float) -> float:
    """A width to a tenth of a metre, never below 0.5 m (so it is above zero, format rule 2)."""
    return max(MIN_WIDTH_M, round(width, 1))


def polygon_mean_width_m(polygon: Polygon) -> float:
    """A tidal outline's mean width: twice its area over its perimeter, shell and holes, in metres."""
    mean = math.radians(
        sum(p[0] for ring in polygon for p in ring) / max(1, sum(len(r) for r in polygon))
    )
    area = perimeter = 0.0
    for index, ring in enumerate(polygon):
        flat = [
            (p[1] * math.cos(mean) * GROUND_M_PER_DEGREE, p[0] * GROUND_M_PER_DEGREE) for p in ring
        ]
        shoelace = (
            abs(
                sum(
                    a[0] * b[1] - b[0] * a[1]
                    for a, b in zip(flat, [*flat[1:], flat[0]], strict=True)
                )
            )
            / 2
        )
        area += shoelace if index == 0 else -shoelace
        perimeter += sum(math.dist(a, b) for a, b in zip(flat, [*flat[1:], flat[0]], strict=True))
    return 2 * area / perimeter if perimeter else 0.0


def point_in_polygon(point: LatLon, polygon: Polygon) -> bool:
    """Even-odd containment over every ring."""
    inside = False
    for ring in polygon:
        for (lat_0, lon_0), (lat_1, lon_1) in zip(ring, [*ring[1:], ring[0]], strict=True):
            if (lat_0 > point[0]) != (lat_1 > point[0]) and point[1] < lon_0 + (
                point[0] - lat_0
            ) * (lon_1 - lon_0) / (lat_1 - lat_0):
                inside = not inside
    return inside


def tidal_width(stretch: Stretch, tidal: Sequence[Outline]) -> float:
    """The width of the surveyed tidal outline round the stretch's middle point, at most 800 m; 0 where there is none."""
    middle = stretch.points[len(stretch.points) // 2]
    for outline in tidal:
        for polygon in outline.polygons:
            if point_in_polygon(middle, polygon):
                return min(MAX_TIDAL_WIDTH_M, polygon_mean_width_m(polygon))
    return 0.0


def stretch_widths(
    network: Network,
    areas: dict[str, tuple[float, float]],
    settings: Settings,
    tidal: Sequence[Outline],
    corrections: Sequence[Correction],
) -> dict[str, tuple[float, float]]:
    """Rule 8: each stretch's two widths, a tidal stretch at least as wide as its outline, then the corrections."""
    widths: dict[str, tuple[float, float]] = {}
    for stretch in network.stretches:
        first, last = (width_of(a, settings.a, settings.b) for a in areas[stretch.id])
        if stretch.kind == "tidal":
            floor = tidal_width(stretch, tidal)
            first, last = max(first, floor), max(last, floor)
        for correction in corrections:
            if correction.river != stretch.river or correction.stretch not in (None, stretch.id):
                continue
            if correction.width_m is not None:
                first, last = correction.width_m
            elif correction.width_factor is not None:
                first, last = first * correction.width_factor, last * correction.width_factor
        widths[stretch.id] = (final_width(first), final_width(last))
    return widths


def rivers_toml(
    waters: Waters, network: Network, widths: dict[str, tuple[float, float]], waters_sha: str
) -> str:
    """The network in the new format: fixed order, five decimals for points, one for widths."""
    steps = ", ".join(str(s) for s in waters.settings.tier_steps)
    lines = [
        "# Built by assets/map/ireland/import_waters.py from waters.toml; do not edit it, change waters.toml and build again.",
        f"# waters.toml sha256 {waters_sha}",
        "format = 2",
        "",
        "[tiers]",
        f"show_from_step = [{steps}]",
    ]
    by_id = {r.id: r for r in waters.rivers}
    for identifier in network.rivers:
        river = by_id[identifier]
        lines += [
            "",
            "[[river]]",
            f"id = {toml_text(river.id)}",
            f"name = {toml_text(river.name)}",
            f"tier = {river.tier}",
        ]
    for stretch in network.stretches:
        first, last = widths[stretch.id]
        lines += [
            "",
            "[[stretch]]",
            f"id = {toml_text(stretch.id)}",
            f"river = {toml_text(stretch.river)}",
            f"kind = {toml_text(stretch.kind)}",
        ]
        lines += [f"width_m = [{first:.1f}, {last:.1f}]", f"ends = {toml_text(stretch.ends)}"]
        if stretch.note:
            lines.append(f"note = {toml_text(stretch.note)}")
        lines.append("points = [")
        lines += [f"  [{lat:.5f}, {lon:.5f}]," for lat, lon in stretch.points]
        lines.append("]")
    return "\n".join(lines) + "\n"


# ---- the format contract -------------------------------------------------------------------------------------------------


def lough_has_lake(points: Sequence[LatLon], terrain: Terrain) -> bool:
    """Format rule 3, as the loader does it on the written points: each end projected to fixed point first, the midpoint the
    integer mean of the two (rounded down), and its cell a lake cell."""
    origin = terrain.crop.origin
    for first, second in pairwise(points):
        (x_0, y_0), (x_1, y_1) = fixed_point(first), fixed_point(second)
        mid_x, mid_y = (x_0 + x_1) // 2, (y_0 + y_1) // 2
        if terrain.is_lake(
            (mid_x // FIXED_UNITS_PER_CELL - origin[0], mid_y // FIXED_UNITS_PER_CELL - origin[1])
        ):
            return True
    return False


def _lough_line_reason(
    code: str, outline: Outline | None, listed: set[str], terrain: Terrain
) -> str:
    """Why one survey lake code of a lough line has no lough under the line (rule 11, r4), with its name and detail."""
    if outline is None:
        return f"the survey gives the line no lake outline (lake code {code})"
    label = f"{outline.name} ({outline.code})"
    if outline.code not in listed:
        return (
            f"its lake has an outline that is not in the list: {label}, {outline.area_km2:.2f} km2"
        )
    cells = cover_cells(outline.polygons, terrain.crop)
    if any(terrain.is_lake(c) for c in cells):
        return f"its lough is in the list and on the map, but no written segment midpoint of this stretch is in a lake cell: {label}"
    return f"its lough is in the list, but no cell of it is lake on the map: {label}, its outline covers {len(cells)} cells"


def write_loughs_without_lake_as_rivers(
    network: Network, terrain: Terrain, waters: Waters, lakes: Sequence[Outline]
) -> None:
    """Rule 7 (r4): a stretch is a lough only where the map has the lough under it. A stretch the survey marks as a lough line,
    with no segment midpoint in a lake cell of the terrain the mask will make, is written as a river (ends, id and neighbours
    unchanged) and `network.lough_as_river` gives its reason: no survey outline; an outline not in the list; or the lough is on
    the map and no written segment midpoint of the stretch is in a lake cell."""
    by_code = {o.code: o for o in lakes}
    listed = {lough.source for lough in waters.loughs}
    for stretch in network.stretches:
        if stretch.kind != "lough" or lough_has_lake(stretch.points, terrain):
            continue
        codes = sorted({e.seg.lake_code for e in stretch.edges if e.seg.lake_code})
        parts = [_lough_line_reason(code, by_code.get(code), listed, terrain) for code in codes]
        if not codes:
            parts = ["the survey gives the line no lake outline (it has no lake code)"]
        elif any(not e.seg.lake_code for e in stretch.edges):
            parts.append("some of its segments carry no lake code")
        reason = "; ".join(parts)
        stretch.kind = "river"
        stretch.note = "the survey marks this line as a lough, but the map has no lough under it: written as a river"
        network.lough_as_river[stretch.id] = f"river {stretch.river}: {reason}"


def format_problems(  # noqa: PLR0912 -- one check after another, each naming the river and stretch
    waters: Waters,
    network: Network,
    widths: dict[str, tuple[float, float]],
    terrain: Terrain,
) -> list[str]:
    """What is wrong with a built network under CK-347's format rules 2 and 3, on the final terrain and as it will be written
    (thinned, rounded, corrected): one message per problem, each naming the river and the stretch. Empty when it is valid."""
    problems: list[str] = []
    by_id = {s.id: s for s in network.stretches}
    for river in waters.rivers:
        if not any(s.river == river.id for s in network.stretches):
            problems.append(f"river {river.id}: it has no stretch (a river with none is refused)")
    for stretch in network.stretches:
        where = f"river {stretch.river}, stretch {stretch.id}"
        if not STRETCH_ID.fullmatch(stretch.id):
            problems.append(f"{where}: the id is not 1 to {MAX_ID_LENGTH} of a-z, 0-9 and -")
        for width in widths[stretch.id]:
            if not 0 < width <= MAX_WIDTH_M:
                problems.append(
                    f"{where}: width {width:.1f} m is not above 0 and at most {MAX_WIDTH_M:g} m"
                )
                break
        points = stretch.points
        if len(points) < 2:  # noqa: PLR2004
            problems.append(f"{where}: it has fewer than two points")
            continue
        if any(terrain.crop.cell_containing(p) is None for p in points):
            problems.append(f"{where}: a point lies outside the crop")
        if any(fixed_point(a) == fixed_point(b) for a, b in pairwise(points)):
            problems.append(f"{where}: two consecutive points are the same fixed point")
        if stretch.kind == "lough" and not lough_has_lake(points, terrain):
            problems.append(
                f"{where}: it is a lough stretch but no segment midpoint lies in a lake cell of the final terrain "
                f"(survey lake {', '.join(sorted({e.seg.lake_code for e in stretch.edges if e.seg.lake_code})) or 'none'})"
            )
        if stretch.ends == "sea":
            if not terrain.open_sea(points[-1]):
                problems.append(f"{where}: its sea end is not in open sea")
        elif stretch.ends not in by_id or stretch.ends == stretch.id:
            problems.append(f"{where}: it ends at {stretch.ends!r}, which is not another stretch")
        elif fixed_point(points[-1]) != fixed_point(by_id[stretch.ends].points[0]):
            problems.append(f"{where}: its last point is not the first point of {stretch.ends}")
    for stretch in network.stretches:
        seen: set[str] = set()
        here = stretch
        while here.ends != "sea" and here.ends in by_id and here.id not in seen:
            seen.add(here.id)
            here = by_id[here.ends]
        if here.ends != "sea" and here.ends in by_id:
            problems.append(
                f"river {stretch.river}, stretch {stretch.id}: following its ends goes in a circle"
            )
    return problems


# ---- the cells -----------------------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class LoughCells:
    """A listed lough with the cells its survey outline covers."""

    name: str
    mode: str
    keep_existing: bool
    cells: frozenset[Cell]


@dataclass
class WaterPlan:
    """What rule 9 decided: the cells that change, and what the report says about them."""

    change: dict[Cell, int] = field(default_factory=dict)
    sea_new: dict[Cell, str] = field(default_factory=dict)
    sea_new_from: dict[Cell, int] = field(default_factory=dict)
    tidal_unjoined: dict[Cell, str] = field(default_factory=dict)
    sea_cut_off: set[Cell] = field(default_factory=set)
    lough_new: dict[str, set[Cell]] = field(default_factory=dict)
    lough_cells: dict[str, int] = field(default_factory=dict)
    returned: dict[Cell, int] = field(default_factory=dict)
    returned_by: dict[str, set[Cell]] = field(default_factory=dict)
    kept: dict[Cell, str] = field(default_factory=dict)
    kept_by: dict[str, set[Cell]] = field(default_factory=dict)
    would_sea: set[Cell] = field(default_factory=set)
    would_lough: set[Cell] = field(default_factory=set)
    would_return: set[Cell] = field(default_factory=set)
    sea_entries: dict[str, frozenset[Cell]] = field(
        default_factory=dict
    )  # each [[sea]] entry's cells (CK-354 rule 10)


def _nearest_land(terrain: Sequence[Sequence[int]], cell: Cell) -> int:
    """The terrain of the nearest cell that is land today, by the larger of the row and column distances; among cells equally
    near the first in row order, then in column order."""
    height, width = len(terrain), len(terrain[0])
    for radius in range(1, max(height, width)):
        for row in range(cell[1] - radius, cell[1] + radius + 1):
            for column in range(cell[0] - radius, cell[0] + radius + 1):
                if (
                    max(abs(row - cell[1]), abs(column - cell[0])) == radius
                    and 0 <= row < height
                    and 0 <= column < width
                    and terrain[row][column] not in (SEA, LAKE)
                ):
                    return terrain[row][column]
    message = f"no land anywhere to give cell {cell} back to"
    raise WatersError(message)


def decide_water(  # noqa: PLR0915 -- the sets of rule 9, then each cell decided once and what the report says about it
    terrain: Sequence[Sequence[int]],
    tidal: dict[Cell, str],
    loughs: Sequence[LoughCells],
    kept: dict[Cell, str] | None = None,
) -> WaterPlan:
    """Rule 9: the sea and the loughs, each cell decided once from every outline and every exception together."""
    kept = kept or {}
    height, width = len(terrain), len(terrain[0])

    def class_at(cell: Cell) -> int:
        return terrain[cell[1]][cell[0]]

    def inside(cell: Cell) -> bool:
        return 0 <= cell[0] < width and 0 <= cell[1] < height

    plan = WaterPlan()
    plan.kept = {c: why for c, why in kept.items() if inside(c)}
    old_sea = {(c, r) for r in range(height) for c in range(width) if terrain[r][c] == SEA}
    old_lake = {(c, r) for r in range(height) for c in range(width) if terrain[r][c] == LAKE}

    def reached_from_edge(candidates: set[Cell]) -> set[Cell]:
        border = {c for c in old_sea if c[0] in (0, width - 1) or c[1] in (0, height - 1)}
        return flood(border, lambda c: c in old_sea or c in candidates)

    land_tidal = {c for c in tidal if inside(c) and class_at(c) not in (SEA, LAKE)}
    candidates = land_tidal - set(plan.kept)
    reached = reached_from_edge(candidates)
    sea_new = candidates & reached
    plan.tidal_unjoined = {c: tidal[c] for c in sorted(candidates - reached)}
    plan.sea_cut_off = old_sea - reached
    plan.sea_new = {c: tidal[c] for c in sorted(sea_new)}
    plan.sea_new_from = {c: class_at(c) for c in sorted(sea_new)}
    surveyed_all: set[Cell] = set().union(*(lough.cells for lough in loughs)) if loughs else set()
    surveyed_redrawn: set[Cell] = (
        set().union(*(lough.cells for lough in loughs if lough.mode == "redrawn"))
        if loughs
        else set()
    )
    surveyed_kept: set[Cell] = (
        set().union(*(lough.cells for lough in loughs if lough.keep_existing)) if loughs else set()
    )
    patch_of: dict[Cell, int] = {}
    patches: list[set[Cell]] = []
    for cell in sorted(old_lake, key=lambda c: (c[1], c[0])):
        if cell not in patch_of:
            patch = flood({cell}, lambda c: c in old_lake)
            patches.append(patch)
            patch_of.update({c: len(patches) - 1 for c in patch})
    redrawn_patches = {i for i, p in enumerate(patches) if p & surveyed_redrawn}
    kept_patches = {i for i in redrawn_patches if patches[i] & surveyed_kept}
    giving_back: set[Cell] = set()
    for index in sorted(redrawn_patches - kept_patches):
        giving_back |= {c for c in patches[index] if c not in surveyed_all}
    returning = giving_back - set(plan.kept)
    plan.would_return = giving_back & set(plan.kept)
    water = {
        c
        for c in surveyed_all
        if inside(c) and class_at(c) not in (SEA, LAKE) and c not in sea_new and c not in plan.kept
    }
    for cell in sorted(sea_new):
        plan.change[cell] = SEA
    for cell in sorted(water):
        plan.change[cell] = LAKE
    for cell in sorted(returning):
        plan.returned[cell] = _nearest_land(terrain, cell)
        plan.change[cell] = plan.returned[cell]
    for lough in loughs:
        plan.lough_cells[lough.name] = len(lough.cells)
        plan.lough_new[lough.name] = {c for c in lough.cells if c in water}
        touched = {
            i for i in redrawn_patches if lough.mode == "redrawn" and patches[i] & lough.cells
        }
        plan.returned_by[lough.name] = {
            c for i in touched for c in patches[i] if c in plan.returned
        }
        plan.kept_by[lough.name] = {
            c for i in touched if lough.keep_existing for c in patches[i] if c not in surveyed_all
        }
    # what a kept cell would have become with its exception lifted: sea only where the tidal cell is joined to the open sea
    # (the kept cells counted as candidates), then lough where it is surveyed water and not sea
    kept_tidal = set(plan.kept) & land_tidal
    explicit = {c for c, why in plan.kept.items() if why.startswith("keep ")}
    if kept_tidal:
        # a [[keep]] cell lifted alone, and a location lifted with every [[keep]] still in force
        with_explicit_in_force = reached_from_edge(land_tidal - explicit)
        all_lifted = reached_from_edge(land_tidal)
        plan.would_sea = (kept_tidal & explicit & all_lifted) | (
            (kept_tidal - explicit) & with_explicit_in_force
        )
    else:
        plan.would_sea = set()
    plan.would_lough = {
        c
        for c in plan.kept
        if c in surveyed_all and class_at(c) not in (SEA, LAKE) and c not in plan.would_sea
    }
    return plan


def mask_cells(
    pixels: Sequence[Sequence[RGB]],
    keys: dict[str, RGB],
    paint: str,
    size: tuple[int, int] | None = None,
) -> list[list[bool]]:
    """The cells of a `water.png` mask that an edit painting `paint` covers: those whose pixel is that terrain's key colour.

    The image must be `size` (width, height) when it is given, and no colour may be anything but black and the terrain keys."""
    if BLACK in keys.values():
        message = "a terrain key is black, which the mask uses for 'no change'"
        raise WatersError(message)
    if paint not in keys:
        message = f"mask: paint {paint!r} is not a terrain with a key"
        raise WatersError(message)
    if size is not None and (len(pixels), len(pixels[0]) if pixels else 0) != (size[1], size[0]):
        message = f"mask is {len(pixels[0]) if pixels else 0} x {len(pixels)} pixels, the crop is {size[0]} x {size[1]}"
        raise WatersError(message)
    allowed = {BLACK, *keys.values()}
    for row in pixels:
        for pixel in row:
            if tuple(pixel) not in allowed:
                message = (
                    f"mask has the colour {tuple(pixel)}, which is neither black nor a terrain key"
                )
                raise WatersError(message)
    return [[tuple(pixel) == keys[paint] for pixel in row] for row in pixels]


# ---- drained area --------------------------------------------------------------------------------------------------------

NEIGHBOURS8 = ((-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1))
FILL_STEP_M = 0.001


def _filled_levels(
    elevation: Sequence[Sequence[float]], terrain: Sequence[Sequence[int]], crop: Crop
) -> tuple[list[float], list[bool]]:
    """Each cell's level once depressions are filled from the sea and the crop's edge (so water passes through loughs and ends only at the
    sea or at the edge), and which cells are sea (design 4.5)."""
    height, width = crop.height, crop.width
    count = height * width
    sea = [terrain[i // width][i % width] == SEA for i in range(count)]
    level = [math.inf] * count
    seen = bytearray(count)
    queue: list[tuple[float, int]] = []
    for index in range(count):
        row, column = divmod(index, width)
        if sea[index] or row in (0, height - 1) or column in (0, width - 1):
            level[index] = -1e9 if sea[index] else elevation[row][column]
            seen[index] = 1
            queue.append((level[index], index))
    heapq.heapify(queue)
    while queue:
        here, index = heapq.heappop(queue)
        row, column = divmod(index, width)
        for d_r, d_c in NEIGHBOURS8:
            r, c = row + d_r, column + d_c
            if 0 <= r < height and 0 <= c < width and not seen[r * width + c]:
                seen[r * width + c] = 1
                level[r * width + c] = max(elevation[r][c], here + FILL_STEP_M)
                heapq.heappush(queue, (level[r * width + c], r * width + c))
    return level, sea


def line_cell_indices(points: Sequence[LatLon], crop: Crop) -> set[int]:
    """The cells (as row * width + column) a line passes through, exactly: every cell the line has a piece of positive length in, and the
    cell of each end point (a point on a cell's boundary belongs to the cell to its east or south, as the loader's projection has it).
    Cells outside the crop are left out."""
    cells: set[int] = set()
    scaled = [crop.cell_of(*p) for p in points]

    def add(column: float, row: float) -> None:
        c, r = math.floor(column), math.floor(row)
        if 0 <= c < crop.width and 0 <= r < crop.height:
            cells.add(r * crop.width + c)

    add(*scaled[0])
    for (x_0, y_0), (x_1, y_1) in pairwise(scaled):
        d_x, d_y = x_1 - x_0, y_1 - y_0
        crossings = {0.0, 1.0}
        for start, delta in ((x_0, d_x), (y_0, d_y)):
            if delta:
                low, high = sorted((start, start + delta))
                crossings.update(
                    t
                    for k in range(math.ceil(low), math.floor(high) + 1)
                    if 0.0 < (t := (k - start) / delta) < 1.0
                )
        ordered = sorted(crossings)
        for first, second in pairwise(ordered):
            middle = (first + second) / 2
            add(x_0 + d_x * middle, y_0 + d_y * middle)
        add(x_1, y_1)
    return cells


@dataclass
class Drainage:
    """The drained areas of a build (CK-355): what each stretch drains at its two points, what each surveyed line has at its lower end, and
    how the ground that is not sea adds up (km2): to lines, straight to the sea, and off the crop's edge over land."""

    areas: dict[str, tuple[float, float]] = field(default_factory=dict)
    lines: dict[str, float] = field(default_factory=dict)
    ground: float = 0.0
    collected: float = 0.0
    to_sea: float = 0.0
    off_edge: float = 0.0


def _successors(graph: Graph, network: Network) -> dict[str, str | None]:
    """Where each surveyed line sends its area (rule 1): a written line into the next line of its stretch, at the end of its stretch into
    the first line of the stretch it ends at (or to the sea); a line in no written stretch into one line leaving its lower end, a written
    line before an unwritten one, then the nearer a mouth, then the first by segment code."""
    written: dict[str, tuple[Stretch, int]] = {}
    for stretch in network.stretches:
        for index, edge in enumerate(stretch.edges):
            written[edge.seg.code] = (stretch, index)
    first_line = {s.id: s.edges[0].seg.code for s in network.stretches if s.edges}
    leaving: dict[LatLon, list[Edge]] = collections.defaultdict(list)
    for edge in graph.edges:
        leaving[edge.start].append(edge)
    result: dict[str, str | None] = {}
    for edge in graph.edges:
        code = edge.seg.code
        if code in written:
            stretch, index = written[code]
            if index + 1 < len(stretch.edges):
                result[code] = stretch.edges[index + 1].seg.code
            else:
                result[code] = first_line.get(stretch.ends)
            continue
        options = [e for e in leaving.get(edge.end, ()) if e.seg.code != code]
        if not options:
            result[code] = None
            continue
        best = min(
            options,
            key=lambda e: (
                e.seg.code not in written,
                graph.distance.get(e.end, math.inf),
                e.seg.code,
            ),
        )
        result[code] = best.seg.code
    return result


def _collect_into_lines(  # noqa: PLR0913 -- the graph, the three grids, the totals and the written geometry are separate inputs
    graph: Graph,
    elevation: Sequence[Sequence[float]],
    terrain: Sequence[Sequence[int]],
    crop: Crop,
    result: Drainage,
    written: dict[str, Sequence[LatLon]] | None = None,
) -> dict[str, float]:
    """Rule 1, the cells: each cell that is not sea sends its own area down the slopes until it comes to a cell that holds a line of the
    graph (the line of the highest stream order, then the first by segment code, where several share a cell) or to the sea; what leaves the
    crop's edge over land belongs to nothing. A written line is read as the builder wrote it (`written`: its points after the cut at the
    first open sea), so ground beyond a sea end is not its own. Fills `result`'s ground, collected, to_sea and off_edge, and returns each
    line's own cells."""
    height, width = crop.height, crop.width
    level, sea = _filled_levels(elevation, terrain, crop)
    holder: dict[int, tuple[int, str]] = {}
    for edge in graph.edges:
        key = (-max(edge.seg.order, edge.seg.twin_order), edge.seg.code)
        for cell in line_cell_indices((written or {}).get(edge.seg.code, edge.points), crop):
            if cell not in holder or key < holder[cell]:
                holder[cell] = key
    cell_km2 = [
        (CELL_M * math.cos(math.radians(crop.row_latitude(r))) / 1000.0) ** 2 for r in range(height)
    ]
    flow = [0.0 if sea[i] else cell_km2[i // width] for i in range(height * width)]
    result.ground = sum(flow)
    collected: dict[str, float] = collections.defaultdict(float)
    for index in sorted(range(height * width), key=lambda i: (-level[i], i)):
        if sea[index]:
            continue
        if index in holder:
            collected[holder[index][1]] += flow[index]
            result.collected += flow[index]
            continue
        row, column = divmod(index, width)
        best, steepest = -1, 0.0
        for d_r, d_c in NEIGHBOURS8:
            r, c = row + d_r, column + d_c
            if 0 <= r < height and 0 <= c < width:
                drop = (level[index] - level[r * width + c]) / math.hypot(d_r, d_c)
                if drop > steepest:
                    best, steepest = r * width + c, drop
        if best >= 0:
            flow[best] += flow[index]
        else:
            result.off_edge += flow[index]
    result.to_sea = sum(flow[i] for i in range(height * width) if sea[i])
    return collected


def _along_the_survey(
    successors: dict[str, str | None], own: dict[str, float]
) -> tuple[dict[str, float], dict[str, float]]:
    """Rule 1, the lines: each line has its own cells plus what the lines whose successor it is send into its upper end. Returns the area at
    each line's lower end and what was sent into each line's upper end. Lines are taken sources first; a loop of lines that pass on to one
    another (the survey's mazes) is cut at the first line by segment code."""
    successors = dict(successors)
    waiting = collections.Counter(t for t in successors.values() if t is not None)
    lower = dict(own)
    sent_in: dict[str, float] = collections.defaultdict(float)
    ready = sorted(code for code in successors if waiting[code] == 0)
    done: set[str] = set()
    while len(done) < len(successors):
        while ready:
            code = ready.pop(0)
            done.add(code)
            lower[code] += sent_in.get(code, 0.0)
            target = successors[code]
            if target is not None:
                sent_in[target] += lower[code]
                waiting[target] -= 1
                if waiting[target] == 0:
                    ready.append(target)
                    ready.sort()
        if len(done) < len(successors):
            successors[min(c for c in successors if c not in done)] = None
            waiting = collections.Counter(
                t for c, t in successors.items() if t is not None and c not in done
            )
            ready = sorted(c for c in successors if c not in done and waiting[c] == 0)
    return lower, dict(sent_in)


def drained_areas(
    graph: Graph,
    network: Network,
    elevation: Sequence[Sequence[float]],
    terrain: Sequence[Sequence[int]],
    crop: Crop,
) -> Drainage:
    """CK-355 rules 1 and 2: the ground that reaches the lower end of each surveyed line, and so the area at each stretch's two points.

    Every cell that is not sea counts with its own area; it goes down the slopes to the first line of the graph or the sea
    (`_collect_into_lines`), and along the survey from line to line as the builder wrote the rivers (`_successors`,
    `_along_the_survey`). A stretch has, at its last point, the area at the lower end of its last line and, at its first, what is sent
    into the upper end of its first line; a headwater's first point is at most its own area and at least 1 km2. No stretch is changed.
    """
    result = Drainage()
    written = {e.seg.code: e.points for stretch in network.stretches for e in stretch.edges}
    own = _collect_into_lines(graph, elevation, terrain, crop, result, written)
    successors = _successors(graph, network)
    lower, sent_in = _along_the_survey(
        successors, {code: own.get(code, 0.0) for code in successors}
    )
    result.lines = lower
    arriving = {edge.end for edge in graph.edges}
    for stretch in network.stretches:
        first_code, last_code = stretch.edges[0].seg.code, stretch.edges[-1].seg.code
        last = lower[last_code]
        sent = sent_in.get(first_code, 0.0)
        first = sent if stretch.start in arriving else min(last, max(HEADWATER_FLOOR_KM2, sent))
        result.areas[stretch.id] = (first, last)
    return result


# ---- the whole build -----------------------------------------------------------------------------------------------------


@dataclass
class Surveys:
    """The surveys as plain values: river segments of stream order 4 and above, tidal and lake outlines, and what is missing."""

    rivers: list[Seg] = field(default_factory=list)
    tidal: list[Outline] = field(default_factory=list)
    lakes: list[Outline] = field(default_factory=list)
    coastal: list[Outline] = field(default_factory=list)
    layers: list[dict[str, Any]] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)


@dataclass
class MapGrid:
    """The map's terrain classes and elevations (metres) by row, with its crop."""

    crop: Crop
    terrain: list[list[int]]
    elevation: list[list[float]]


@dataclass
class BuildResult:
    """What `build` writes, and what produced it."""

    rivers_toml: str
    water: list[list[int]]
    report: str
    network: Network
    plan: WaterPlan
    drainage: Drainage = field(default_factory=lambda: Drainage())


def _outline_by_code(
    outlines_: Sequence[Outline], code: str, label: str, survey: str = "lake"
) -> Outline:
    for outline in outlines_:
        if outline.code == code:
            return outline
    noun = "lake" if survey == "lake" else "outline"
    message = f"{label}: the {survey} survey has no {noun} with the code {code!r}"
    raise WatersError(message)


def sea_entry_cells(entry: SeaDef, coastal: Sequence[Outline], crop: Crop) -> frozenset[Cell]:
    """Rule 10: the cells of a `[[sea]]` entry: those more than half inside a coastal outline of the survey, or the cells that hold
    its points."""
    label = f"[[sea]] {entry.name}"
    if entry.source is not None:
        outline = _outline_by_code(coastal, entry.source, label, "coastal")
        return frozenset(cover_cells(outline.polygons, crop))
    cells = set()
    for point in entry.points:
        cell = crop.cell_containing(point)
        if cell is None:
            message = f"{label}: the point {point[0]}, {point[1]} is outside the crop"
            raise WatersError(message)
        cells.add(cell)
    return frozenset(cells)


def _plan_for(
    waters: Waters, surveys: Surveys, grid: MapGrid, locations: dict[str, LatLon]
) -> WaterPlan:
    crop = grid.crop
    tidal: dict[Cell, str] = {}
    for outline in sorted(surveys.tidal, key=lambda o: (o.name, o.code)):
        for cell in sorted(cover_cells(outline.polygons, crop)):
            tidal.setdefault(cell, outline.name)
    sea_entries = {
        entry.name: sea_entry_cells(entry, surveys.coastal, crop) for entry in waters.seas
    }
    for (
        name,
        cells,
    ) in sea_entries.items():  # rule 11: the cells join the tidal cells and are decided with them
        for cell in sorted(cells):
            tidal.setdefault(cell, name)
    loughs = []
    for entry in waters.loughs:
        outline = _outline_by_code(surveys.lakes, entry.source, f"[[lough]] {entry.name}")
        loughs.append(
            LoughCells(
                entry.name,
                entry.mode,
                entry.keep_existing,
                frozenset(cover_cells(outline.polygons, crop)),
            )
        )
    for reservoir in waters.reservoirs:
        _outline_by_code(surveys.lakes, reservoir.source, f"[[reservoir]] {reservoir.name}")
        if any(lough.source == reservoir.source for lough in waters.loughs):
            message = f"[[reservoir]] {reservoir.name}: the same lake is also a [[lough]]"
            raise WatersError(message)
    kept: dict[Cell, str] = {}
    for keep in waters.keeps:
        cell = crop.cell_containing(keep.point)
        if cell is None:
            message = f"[[keep]] {keep.id}: the point is outside the crop"
            raise WatersError(message)
        kept.setdefault(cell, f"keep {keep.id}")
    for name, point in sorted(locations.items()):
        cell = crop.cell_containing(point)
        if cell is not None:
            kept.setdefault(cell, f"location {name}")
    plan = decide_water(grid.terrain, tidal, loughs, kept)
    plan.sea_entries = sea_entries
    return plan


def apply_plan(terrain: Sequence[Sequence[int]], plan: WaterPlan) -> list[list[int]]:
    """The terrain after the plan."""
    rows = [list(row) for row in terrain]
    for (column, row), terrain_class in plan.change.items():
        rows[row][column] = terrain_class
    return rows


def water_rows(terrain: Sequence[Sequence[int]], plan: WaterPlan) -> list[list[int]]:
    """The mask: the new terrain class where a cell changes, UNCHANGED everywhere else."""
    rows = [[UNCHANGED] * len(row) for row in terrain]
    for (column, row), terrain_class in plan.change.items():
        rows[row][column] = terrain_class
    return rows


def build_waters(  # noqa: PLR0913 -- the list, the surveys, the map and the optional inputs of one build
    waters: Waters,
    surveys: Surveys,
    grid: MapGrid,
    locations: dict[str, LatLon] | None = None,
    waters_sha: str = "",
    area_source: Callable[[Graph, Network], Drainage] | None = None,
) -> BuildResult:
    """`build`: the cells, the network, the widths and the report, from plain values. The same inputs give the same text."""
    plan = _plan_for(waters, surveys, grid, locations or {})
    final = Terrain(grid.crop, apply_plan(grid.terrain, plan))
    reservoir_lakes = frozenset(r.source for r in waters.reservoirs)
    graph = prepare_graph(surveys.rivers, final, waters.corrections)
    network = build_network(
        surveys.rivers,
        waters.rivers,
        final,
        waters.settings,
        waters.corrections,
        reservoir_lakes,
        graph=graph,
    )
    write_loughs_without_lake_as_rivers(network, final, waters, surveys.lakes)
    drainage = (
        area_source(graph, network)
        if area_source
        else drained_areas(graph, network, grid.elevation, final.rows, grid.crop)
    )
    areas = drainage.areas
    widths = stretch_widths(network, areas, waters.settings, surveys.tidal, waters.corrections)
    problems = format_problems(waters, network, widths, final)
    if problems:
        message = (
            "the network breaks the river format (CK-347 rules 2 and 3), nothing written: "
            + "; ".join(problems)
        )
        raise WatersError(message)
    text = rivers_toml(waters, network, widths, waters_sha)
    report = waters_report(
        waters, surveys, grid, plan, network, areas, widths, locations or {}, waters_sha, drainage
    )
    return BuildResult(text, water_rows(grid.terrain, plan), report, network, plan, drainage)


# ---- the report ----------------------------------------------------------------------------------------------------------


def _cells(cells: Iterable[Cell], terrain: dict[Cell, int] | None = None) -> str:
    """Cells as `column,row` (from the crop's top-left corner), in row then column order; with `terrain`, `column,row=terrain`."""
    ordered = sorted(cells, key=lambda c: (c[1], c[0]))
    if terrain is None:
        return " ".join(f"{c},{r}" for c, r in ordered)
    return " ".join(f"{c},{r}={NAMES[terrain[(c, r)]]}" for c, r in ordered)


def _by_terrain(cells: Iterable[int]) -> str:
    counts = collections.Counter(cells)
    return ", ".join(f"{counts[i]} {NAMES[i]}" for i in range(len(NAMES)) if counts[i]) or "none"


def _sea_entries_report(waters: Waters, grid: MapGrid, plan: WaterPlan) -> list[str]:
    """Rule 13: each `[[sea]]` entry's cells that become sea (by the terrain they were), its cells that are not joined, its kept cells,
    and whether it changes nothing."""
    out: list[str] = []
    for entry in waters.seas:
        cells = plan.sea_entries.get(entry.name, frozenset())
        became = {c for c in cells if c in plan.sea_new}
        unjoined = {c for c in cells if c in plan.tidal_unjoined}
        kept = {c for c in cells if c in plan.kept and grid.terrain[c[1]][c[0]] not in (SEA, LAKE)}
        how = f"outline {entry.source}" if entry.source else f"{len(entry.points)} points"
        out.append(f"- {entry.name} ({how}): {len(cells)} cells: {entry.reason}")
        out.append(
            f"  - become sea: {len(became)} cells ({_by_terrain(plan.sea_new_from[c] for c in became)})"
        )
        if became:
            out.append(f"  - cells and the terrain they were: {_cells(became, plan.sea_new_from)}")
        out.append(f"  - not joined to the sea: {len(unjoined)} cells")
        if unjoined:
            out.append(f"  - not joined: {_cells(unjoined)}")
        out.append(f"  - kept (a location or a [[keep]] holds them): {len(kept)} cells")
        if kept:
            out.append(f"  - kept cells: {_cells(kept)}")
        if not became:
            out.append("  - this entry changes nothing")
    return out


def _contests_report(network: Network) -> list[str]:
    """Rule 7: for each river, the lines it lists (by its water body, or by its name) that another river has, with their length."""
    out: list[str] = []
    by_other: dict[str, list[Contest]] = collections.defaultdict(list)
    for contest in network.contests:
        by_other[contest.other].append(contest)
    for other, contests in sorted(by_other.items()):
        total = sum(c.seg.length_m for c in contests) / 1000.0
        out.append(f"- river {other}: {len(contests)} lines, {total:.2f} km, are another river's")
        out += [
            f"  - {c.seg.label()}: {c.seg.length_m / 1000.0:.2f} km, river {c.river} has it by its {c.how}"
            for c in sorted(contests, key=lambda c: (c.river, c.seg.code))
        ]
    return out


TWICE_APART_M = 25.0
TWICE_LENGTH_M = 300.0
TWICE_BIN_DEGREES = 0.002  # the grid the candidate segments are found in; its padding exceeds 25 m at every latitude of the island
TWICE_PAD_DEGREES = 0.0005
Flat = tuple[float, float]


def _within(start: float, slope: float, low: float, high: float) -> tuple[float, float] | None:
    """The parameters t at which `low <= start + slope * t <= high`, as an interval, or None."""
    if abs(slope) < 1e-12:  # noqa: PLR2004 -- a line parallel to the bound
        return (-math.inf, math.inf) if low <= start <= high else None
    first, second = (low - start) / slope, (high - start) / slope
    return (min(first, second), max(first, second))


def _meet(
    a: tuple[float, float] | None, b: tuple[float, float] | None
) -> tuple[float, float] | None:
    if a is None or b is None:
        return None
    low, high = max(a[0], b[0]), min(a[1], b[1])
    return (low, high) if low <= high else None


def _near_interval(
    start: Flat, finish: Flat, b_start: Flat, b_finish: Flat
) -> tuple[float, float] | None:
    """The part of a line segment (parameters 0 to 1) that lies within 25 m of another segment, exactly: the segment's intersection with
    the capsule round the other (a strip along it and a disc at each end), which is one interval because the capsule is convex."""
    d_x, d_y = finish[0] - start[0], finish[1] - start[1]
    parts: list[tuple[float, float]] = []
    for centre in (b_start, b_finish):
        o_x, o_y = start[0] - centre[0], start[1] - centre[1]
        a = d_x * d_x + d_y * d_y
        b = 2 * (o_x * d_x + o_y * d_y)
        c = o_x * o_x + o_y * o_y - TWICE_APART_M**2
        if a > 0 and (disc := b * b - 4 * a * c) >= 0:
            root = math.sqrt(disc)
            parts.append(((-b - root) / (2 * a), (-b + root) / (2 * a)))
    length = math.hypot(b_finish[0] - b_start[0], b_finish[1] - b_start[1])
    if length > 0:
        e_x, e_y = (b_finish[0] - b_start[0]) / length, (b_finish[1] - b_start[1]) / length
        o_x, o_y = start[0] - b_start[0], start[1] - b_start[1]
        along = _within(o_x * e_x + o_y * e_y, d_x * e_x + d_y * e_y, 0.0, length)
        across = _within(
            o_y * e_x - o_x * e_y, d_y * e_x - d_x * e_y, -TWICE_APART_M, TWICE_APART_M
        )
        if (strip := _meet(along, across)) is not None:
            parts.append(strip)
    if not parts:
        return None
    return _meet((min(p[0] for p in parts), max(p[1] for p in parts)), (0.0, 1.0))


def _ground_from(origin: LatLon, scale: float, point: LatLon) -> Flat:
    """A point in ground metres from `origin` on the local scale (the mean latitude of the segment, as `ground_m` measures)."""
    return ((point[1] - origin[1]) * scale, (point[0] - origin[0]) * GROUND_M_PER_DEGREE)


def _bin_cells(a: LatLon, b: LatLon, pad: float) -> Iterable[tuple[int, int]]:
    """The grid cells a segment's box, padded by `pad` degrees, touches."""
    low_row = math.floor((min(a[0], b[0]) - pad) / TWICE_BIN_DEGREES)
    high_row = math.floor((max(a[0], b[0]) + pad) / TWICE_BIN_DEGREES)
    low_column = math.floor((min(a[1], b[1]) - pad) / TWICE_BIN_DEGREES)
    high_column = math.floor((max(a[1], b[1]) + pad) / TWICE_BIN_DEGREES)
    return (
        (column, row)
        for row in range(low_row, high_row + 1)
        for column in range(low_column, high_column + 1)
    )


def _union_length(intervals: Iterable[tuple[float, float]]) -> float:
    """The total length of a union of intervals."""
    covered, reach = 0.0, -math.inf
    for low, high in sorted(intervals):
        if high > max(low, reach):
            covered += high - max(low, reach)
        reach = max(reach, high)
    return covered


def _beside_totals(network: Network) -> dict[tuple[int, int], float]:
    """For each (stretch, other stretch), the metres of the stretch's own line that lie within 25 m of the other's line."""
    bins: dict[tuple[int, int], list[tuple[int, LatLon, LatLon]]] = collections.defaultdict(list)
    for index, stretch in enumerate(network.stretches):
        for first, last in pairwise(stretch.points):
            for key in _bin_cells(first, last, 0.0):
                bins[key].append((index, first, last))
    totals: dict[tuple[int, int], float] = collections.defaultdict(float)
    for index, stretch in enumerate(network.stretches):
        for first, last in pairwise(stretch.points):
            scale = math.cos(math.radians((first[0] + last[0]) / 2)) * GROUND_M_PER_DEGREE
            start, finish = _ground_from(first, scale, first), _ground_from(first, scale, last)
            found: dict[int, list[tuple[float, float]]] = collections.defaultdict(list)
            seen: set[tuple[int, LatLon, LatLon]] = set()
            for key in _bin_cells(first, last, TWICE_PAD_DEGREES):
                for candidate in bins.get(key, ()):
                    if candidate[0] == index or candidate in seen:
                        continue
                    seen.add(candidate)
                    other, b_first, b_last = candidate
                    if (
                        near := _near_interval(
                            start,
                            finish,
                            _ground_from(first, scale, b_first),
                            _ground_from(first, scale, b_last),
                        )
                    ) is not None:
                        found[other].append(near)
            share = ground_m(first, last)
            for other, intervals in found.items():
                totals[(index, other)] += _union_length(intervals) * share
    return totals


def courses_built_twice(network: Network) -> list[tuple[str, str, float]]:
    """Rule 9: every pair of stretches that run within 25 m of each other for 300 m or more, with that length, longest first. Distance is
    local ground distance (each segment measured on the scale of its own latitude, as `ground_m` does). For each segment of a stretch
    the part within 25 m of each other stretch is found exactly, where the segment enters and leaves the band round that stretch's line
    segments, and the lengths are summed: so a course's vertices do not change what it is credited. The length for a pair is the
    lesser of what each stretch runs beside the other."""
    beside = _beside_totals(network)
    pairs = []
    for (first, second), length_first in beside.items():
        if first < second:
            length = min(length_first, beside.get((second, first), 0.0))
            if length >= TWICE_LENGTH_M:
                pairs.append((network.stretches[first].id, network.stretches[second].id, length))
    return sorted(pairs, key=lambda item: (-item[2], item[0], item[1]))


def _twice_report(network: Network) -> list[str]:
    """Rule 9: the section "Courses built twice": information, not a refusal."""
    found = courses_built_twice(network)
    out = [f"- {a} and {b}: about {length:.0f} m side by side" for a, b, length in found] or [
        "- none"
    ]
    out.append(
        f"- total: {len(found)} pairs, about {sum(length for _, _, length in found) / 1000.0:.2f} km"
    )
    return out


def waters_report(  # noqa: PLR0912, PLR0913, PLR0915 -- one section after another, from one build
    waters: Waters,
    surveys: Surveys,
    grid: MapGrid,
    plan: WaterPlan,
    network: Network,
    areas: dict[str, tuple[float, float]],
    widths: dict[str, tuple[float, float]],
    locations: dict[str, LatLon],
    waters_sha: str,
    drainage: Drainage | None = None,
) -> str:
    """`waters-report.md`: what slice 4 puts before the owner."""
    out = [
        "# Waters report",
        "",
        f"Built from waters.toml (sha256 {waters_sha or 'not given'}). Nothing in Ireland's committed map data has changed.",
        "A cell is written `column,row` from the crop's top-left corner (the pixel of `water.png`); `column,row=terrain` is the terrain it takes or had.",
        "",
    ]
    out += ["## Surveys used", ""]
    out += [
        f"- {layer['title']}: {layer['features']} features, read {layer['day']}, sha256 {layer['sha256']}, {layer['licence']}"
        for layer in surveys.layers
    ] or ["- (none recorded)"]
    out += ["", "## Rivers", ""]
    by_river = collections.defaultdict(list)
    for stretch in network.stretches:
        by_river[stretch.river].append(stretch)
    for river in waters.rivers:
        stretches = by_river.get(river.id, [])
        if not stretches:
            continue
        mouth = max(stretches, key=lambda s: areas[s.id][1])
        length = sum(s.length_m for s in stretches) / 1000.0
        out.append(
            f"- **{river.name}** ({river.id}): tier {river.tier}, {len(stretches)} stretches, {length:.1f} km, drains {areas[mouth.id][1]:.0f} km2, {widths[mouth.id][1]:.1f} m wide at its mouth"
        )
        for correction in waters.corrections:
            if correction.river == river.id:
                out.append(f"  - correction {correction.describe()}")
    if drainage is not None and drainage.ground:
        out += [
            "",
            "## Drained area",
            "",
            f"- ground that is not sea: {drainage.ground:.0f} km2, of which {drainage.collected:.0f} km2 reaches a surveyed line, "
            f"{drainage.to_sea:.0f} km2 goes straight to the sea and {drainage.off_edge:.0f} km2 leaves the crop's edge over land",
        ]
    out += ["", "## Sea", ""]
    estuaries = collections.defaultdict(list)
    for cell, name in plan.sea_new.items():
        estuaries[name].append(cell)
    for name, cells in sorted(estuaries.items()):
        out.append(
            f"- {name}: {len(cells)} cells ({_by_terrain(plan.sea_new_from[c] for c in cells)})"
        )
        out.append(f"  - cells and the terrain they were: {_cells(cells, plan.sea_new_from)}")
    if not estuaries:
        out.append("- no cell becomes sea")
    out.append(
        f"- tidal cells left as river because they are not joined to the sea: {len(plan.tidal_unjoined)}"
    )
    unjoined = collections.defaultdict(list)
    for cell, name in plan.tidal_unjoined.items():
        unjoined[name].append(cell)
    out += [f"  - {name}: {_cells(cells)}" for name, cells in sorted(unjoined.items())]
    out.append(
        f"- sea cells already in the map that are cut off from the open sea: {len(plan.sea_cut_off)}"
    )
    if plan.sea_cut_off:
        out.append(f"  - cells: {_cells(plan.sea_cut_off)}")
    out += ["", "## Sea added by the owner", ""]
    out += _sea_entries_report(waters, grid, plan) or ["- no [[sea]] entry"]
    out += ["", "## Lines two rivers' entries both reach", ""]
    out += _contests_report(network) or ["- none"]
    out += ["", "## Courses built twice", ""]
    out += _twice_report(network)
    out += ["", "## Loughs", ""]
    for lough in waters.loughs:
        out.append(
            f"- {lough.name} ({lough.mode}): {plan.lough_cells[lough.name]} cells in the survey, {len(plan.lough_new[lough.name])} become lough"
        )
        if plan.lough_new[lough.name]:
            out.append(f"  - cells that become lough: {_cells(plan.lough_new[lough.name])}")
        returned = plan.returned_by[lough.name]
        if lough.mode == "redrawn":
            out.append(
                f"  - returned to land: {len(returned)} cells ({_by_terrain(plan.returned[c] for c in returned)})"
            )
            if returned:
                out.append(
                    f"  - returned cells and the land each takes: {_cells(returned, plan.returned)}"
                )
        kept = plan.kept_by[lough.name]
        if lough.keep_existing:
            out.append(f"  - kept by keep_existing: {len(kept)} cells: {lough.reason}")
            if kept:
                out.append(f"  - kept cells: {_cells(kept)}")
    for cell, why in sorted(plan.kept.items(), key=lambda item: (item[0][1], item[0][0])):
        changes = [
            words
            for words, cells in (
                ("become sea", plan.would_sea),
                ("become lough", plan.would_lough),
                ("been returned to land", plan.would_return),
            )
            if cell in cells
        ]
        if changes or why.startswith(
            "keep "
        ):  # a location is listed only where something would have changed
            out.append(
                f"- kept cell {cell}: {why}"
                + (f" (would have {' and '.join(changes)})" if changes else "")
            )
    out += ["", "## Reservoirs left out", ""]
    out += [f"- {r.name} ({r.source}): {r.reason}" for r in waters.reservoirs] or ["- none"]
    out += ["", "## Lough lines written as rivers (the map has no lough under them)", ""]
    out += [f"- {i}: {why}" for i, why in sorted(network.lough_as_river.items())] or ["- none"]
    out += ["", "## Added by the tool", ""]
    out += [f"- straight lough stretch: {i}" for i in network.straight_lough] or [
        "- no straight lough stretch"
    ]
    out += [
        f"- run to the open sea through sea cells: {i}, {length / 1000.0:.2f} km"
        for i, length in sorted(network.sea_runs.items())
    ] or ["- no run to the open sea"]
    if network.sea_runs:
        out.append(
            f"- {len(network.sea_runs)} runs added, the longest {max(network.sea_runs.values()) / 1000.0:.2f} km"
        )
    if locations:
        out += ["", "## Locations", ""]
        for name, point in sorted(locations.items()):
            cell = grid.crop.cell_containing(point)
            if cell is None:
                continue
            # only a cell the location itself keeps: where a [[keep]] holds the cell, removing the location changes nothing
            location_keeps = plan.kept.get(cell, "").startswith("location ")
            if location_keeps and (cell in plan.would_sea or cell in plan.would_lough):
                out.append(f"- {name} stands on a cell that would have become water, kept as it is")
            elif any(
                (cell[0] + d_c, cell[1] + d_r) in plan.change
                and plan.change[(cell[0] + d_c, cell[1] + d_r)] in (SEA, LAKE)
                for d_r in (-1, 0, 1)
                for d_c in (-1, 0, 1)
            ):
                out.append(f"- {name} stands beside a cell that becomes water")
    out += ["", "## What Northern Ireland lacks", ""]
    out += [f"- {line}" for line in surveys.missing] or ["- nothing recorded"]
    return "\n".join(out) + "\n"


# ---- propose -------------------------------------------------------------------------------------------------------------

RESERVOIR_WORDS = ("reservoir", " dam", "hmwb")
RESERVOIR_NAMES = (
    "poulaphouca",
    "pollaphuca",
    "inniscarra",
    "carrigadrohid",
    "carrigdrohid",
    "assaroe",
    "parteen",
    "leixlip",
    "golden falls",
)
OWNER_KEEPS = (
    (
        "island-iniscealtra",
        "Inis Cealtra, Lough Derg: the owner's edit island-iniscealtra makes the monastery's cell land",
        (52.91422, -8.44964),
    ),
    (
        "island-inisfallen",
        "Inisfallen, Lough Leane: the owner's edit island-inisfallen makes the monastery's cell land",
        (52.04671, -9.53495),
    ),
)
NEAGH_REASON = "the owner's edits lough-neagh-higher and lough-neagh-margin set the lough's higher level by hand"


def slug(name: str) -> str:
    """A river id from a survey name."""
    text = re.sub(r"[^a-z0-9]+", "-", name.casefold()).strip("-")
    return text[:40].strip("-") or "river"


def is_reservoir(name: str) -> bool:
    """Whether an outline's name says reservoir or dam, or is one design 4.7 names."""
    lowered = f" {name.casefold()}"
    return any(w in lowered for w in RESERVOIR_WORDS) or any(n in lowered for n in RESERVOIR_NAMES)


def tier_of(area_km2: float) -> int:
    """Tier 1 from 1,200 km2, tier 2 from 300 km2, otherwise tier 3."""
    return 1 if area_km2 >= TIER1_KM2 else 2 if area_km2 >= TIER2_KM2 else 3


def unique_id(base: str, used: set[str]) -> str:
    """A river id not yet used: the base, or the base with a number."""
    identifier, number = base, 1
    while identifier in used:
        number += 1
        identifier = f"{base[:36]}-{number}"
    return identifier


def candidate_rivers(segments: Sequence[Seg]) -> list[RiverDef]:
    """Rivers of the Irish survey with 15 km or more at order 5 and above, and Northern Ireland's water bodies with a line of
    order 5 or above (each a river with an empty name). `segments` are the lines of rule 7 (`pair_surveys`): a line of both surveys
    counts for the river of its Irish name where that river is proposed, and otherwise for its water body; a water body left with
    no line of its own is not proposed."""
    length: collections.Counter[str] = collections.Counter()
    for seg in segments:
        if seg.name and seg.irish and seg.epa_order >= PROPOSE_ORDER:
            length[seg.name] += round(seg.length_m)
    rivers: list[RiverDef] = []
    for name in sorted(n for n, m in length.items() if m >= PROPOSE_LENGTH_M):
        rivers.append(
            RiverDef(unique_id(slug(name), {r.id for r in rivers}), name, 3, epa_names=(name,))
        )
    proposed = {r.name for r in rivers}
    bodies = sorted(
        {
            s.water_body
            for s in segments
            if s.source in ("ni", "both")
            and s.water_body
            and s.ni_order >= PROPOSE_ORDER
            and s.name not in proposed
        }
    )
    for code in bodies:
        identifier = unique_id(f"ni-{slug(code)[-30:]}", {r.id for r in rivers})
        rivers.append(RiverDef(identifier, "", 3, rwb_codes=(code,)))
    return rivers


def propose_waters(  # noqa: PLR0912, PLR0915 -- one rule from the surveys to the text of the list
    surveys: Surveys,
    grid: MapGrid,
    settings: Settings | None = None,
    area_source: Callable[[Graph, Network], Drainage] | None = None,
) -> str:
    """`propose`: a first `waters.toml` by the rule the mock-ups used, for the owner to cut and add to."""
    settings = settings or Settings()
    tidal: dict[Cell, str] = {}
    for outline in sorted(surveys.tidal, key=lambda o: (o.name, o.code)):
        for cell in sorted(cover_cells(outline.polygons, grid.crop)):
            tidal.setdefault(cell, outline.name)
    plan = decide_water(grid.terrain, tidal, [], {})
    final = Terrain(grid.crop, apply_plan(grid.terrain, plan))
    paired = _cleaned(surveys.rivers)
    candidates = candidate_rivers(paired)
    graph = prepare_graph(surveys.rivers, final)
    added: dict[str, str] = {}
    segments_added: dict[str, list[str]] = {}
    tried: set[tuple[str, str]] = set()
    while True:
        network = build_network(
            surveys.rivers, candidates, final, settings, (), frozenset(), tolerant=True, graph=graph
        )
        listed = {value for river in candidates for value in (*river.epa_names, *river.rwb_codes)}
        wanted: dict[tuple[str, str], str] = {}
        for river, found in sorted(network.blockers.items()):
            for (
                blocker
            ) in found:  # every river that ends at a line is weighed, whatever else it ends at
                if blocker not in tried and (blocker[0] == "segments" or blocker[1] not in listed):
                    wanted.setdefault(
                        blocker, river
                    )  # two rivers at one line: the first by id has it
        if not wanted:
            break
        tried.update(wanted)
        for (kind, value), river in sorted(wanted.items()):
            if kind == "segments":
                # rule 14: a line with neither a name nor a water body is listed in the segments of the river that ends at it
                position = next(i for i, r in enumerate(candidates) if r.id == river)
                candidates[position] = replace(
                    candidates[position], segments=(*candidates[position].segments, value)
                )
                segments_added.setdefault(river, []).append(value)
                continue
            used = {r.id for r in candidates}
            base = slug(value) if kind == "epa_names" else f"ni-{slug(value)[-30:]}"
            identifier = unique_id(base, used)
            candidates.append(
                RiverDef(
                    identifier,
                    value if kind == "epa_names" else "",
                    3,
                    epa_names=(value,) if kind == "epa_names" else (),
                    rwb_codes=(value,) if kind == "rwb_codes" else (),
                )
            )
            added[identifier] = f"added so that {river} reaches the sea"
    areas = (
        area_source(graph, network)
        if area_source
        else drained_areas(graph, network, grid.elevation, final.rows, grid.crop)
    ).areas
    tiers = {
        r.id: tier_of(
            max((areas[s.id][1] for s in network.stretches if s.river == r.id), default=0.0)
        )
        for r in candidates
    }
    chosen = {
        value
        for river in candidates
        for value in (*river.epa_names, *river.rwb_codes, *river.segments)
    }
    near: set[Cell] = set()
    for seg in paired:
        if (
            (seg.name in chosen)
            or (seg.water_body in chosen)
            or any(c in chosen for c in seg.codes)
        ):
            for point in seg.points:
                cell = grid.crop.cell_containing(point)
                if cell is not None:
                    near.update(
                        (cell[0] + d_c, cell[1] + d_r) for d_r in (-1, 0, 1) for d_c in (-1, 0, 1)
                    )
    lakes_today = {
        (c, r) for r, row in enumerate(grid.terrain) for c, value in enumerate(row) if value == LAKE
    }
    lines = [
        "# waters.toml: the hand-kept list of Ireland's rivers and loughs, proposed by `import_waters.py propose`.",
        "# The owner cuts and adds to it (slice 4). Northern Ireland's rivers have no names in the survey: fill each `name` in before building.",
        "",
        "[settings]",
        f"a = {settings.a}",
        f"b = {settings.b}",
        f"simplify_m = {settings.simplify_m}",
        f"tier_steps = [{', '.join(str(s) for s in settings.tier_steps)}]",
    ]
    for river in candidates:
        if river.id in added:
            lines += ["", f"# {added[river.id]}"]
        if river.id in segments_added:
            count = len(segments_added[river.id])
            lines += [
                "",
                f"# {count} segment{'s' if count != 1 else ''} with no name and no water body added so that it reaches the sea",
            ]
        lines += [
            "",
            f"# drains about {max((areas[s.id][1] for s in network.stretches if s.river == river.id), default=0.0):.0f} km2"
            if river.id not in network.failed
            else f"# not built: {network.failed[river.id]}",
        ]
        lines += [
            "[[river]]",
            f"id = {toml_text(river.id)}",
            f"name = {toml_text(river.name)}",
            f"tier = {tiers[river.id]}",
        ]
        parts = [
            f"{key} = [{', '.join(toml_text(v) for v in values)}]"
            for key, values in (
                ("epa_names", river.epa_names),
                ("rwb_codes", river.rwb_codes),
                ("segments", river.segments),
            )
            if values
        ]
        lines.append(f"source = {{ {', '.join(parts)} }}")
    loughs: list[tuple[Outline, str, bool]] = []
    reservoirs = []
    lake_cells = {o.code: cover_cells(o.polygons, grid.crop) for o in surveys.lakes}
    for outline in sorted(surveys.lakes, key=lambda o: (o.name, o.code)):
        cells = lake_cells[outline.code]
        if not cells:
            continue
        if is_reservoir(outline.name):
            reservoirs.append(outline)
        elif cells & lakes_today:  # rule 5: a lough that overlaps lake cells the map has is redrawn
            loughs.append((outline, "redrawn", "neagh" in outline.name.casefold()))
        elif outline.area_km2 >= LOUGH_MIN_KM2 and cells & near:
            loughs.append((outline, "added", False))
    loughs.sort(key=lambda item: (item[0].name, item[0].code))
    for outline, mode, neagh in loughs:
        lines += [
            "",
            "[[lough]]",
            f"name = {toml_text(outline.name)}",
            f"source = {toml_text(outline.code)}",
            f"mode = {toml_text(mode)}",
        ]
        if neagh:
            lines += ["keep_existing = true", f"reason = {toml_text(NEAGH_REASON)}"]
    for keep_id, reason, point in OWNER_KEEPS:
        lines += [
            "",
            "[[keep]]",
            f"id = {toml_text(keep_id)}",
            f"reason = {toml_text(reason)}",
            f"point = [{point[0]}, {point[1]}]",
        ]
    for outline in reservoirs:
        lines += [
            "",
            "[[reservoir]]",
            f"name = {toml_text(outline.name)}",
            f"source = {toml_text(outline.code)}",
            'reason = "to be checked"',
        ]
    return "\n".join(lines) + "\n"


# ---- Irish Grid and the downloads that use it ----------------------------------------------------------------------------

AIRY_A, AIRY_B = 6_377_340.189, 6_356_034.447938534
WGS_A, WGS_F = 6_378_137.0, 1 / 298.257223563
GRID_LAT0, GRID_LON0, GRID_K0, GRID_EAST0, GRID_NORTH0 = (
    math.radians(53.5),
    math.radians(-8.0),
    1.000035,
    200_000.0,
    250_000.0,
)
# OSi's seven parameters, TM65 to WGS84: translations (m), rotations (arc-seconds), scale (ppm).
HELMERT = (482.5, -130.6, 564.6, -1.042, -0.214, -0.631, 8.15)


def _meridian_arc(phi: float) -> float:
    n = (AIRY_A - AIRY_B) / (AIRY_A + AIRY_B)
    d_phi, s_phi = phi - GRID_LAT0, phi + GRID_LAT0
    return (
        AIRY_B
        * GRID_K0
        * (
            (1 + n + 5 / 4 * n**2 + 5 / 4 * n**3) * d_phi
            - (3 * n + 3 * n**2 + 21 / 8 * n**3) * math.sin(d_phi) * math.cos(s_phi)
            + (15 / 8 * n**2 + 15 / 8 * n**3) * math.sin(2 * d_phi) * math.cos(2 * s_phi)
            - 35 / 24 * n**3 * math.sin(3 * d_phi) * math.cos(3 * s_phi)
        )
    )


def irish_grid_to_wgs84(easting: float, northing: float) -> LatLon:
    """EPSG:29902 (TM65 / Irish Grid) to WGS84 latitude and longitude: the inverse transverse Mercator on the Airy Modified
    ellipsoid, then the seven-parameter shift. The EPA's own reprojection (served as EPSG:4326) agrees to under a metre."""
    ee = (AIRY_A**2 - AIRY_B**2) / AIRY_A**2
    phi = GRID_LAT0 + (northing - GRID_NORTH0) / (AIRY_A * GRID_K0)
    for _ in range(50):
        step = northing - GRID_NORTH0 - _meridian_arc(phi)
        phi += step / (AIRY_A * GRID_K0)
        if abs(step) < 1e-5:  # noqa: PLR2004
            break
    sin, cos, tan = math.sin(phi), math.cos(phi), math.tan(phi)
    nu = AIRY_A * GRID_K0 / math.sqrt(1 - ee * sin * sin)
    rho = AIRY_A * GRID_K0 * (1 - ee) / (1 - ee * sin * sin) ** 1.5
    eta = nu / rho - 1
    d_e = easting - GRID_EAST0
    lat = (
        phi
        - tan / (2 * rho * nu) * d_e**2
        + tan / (24 * rho * nu**3) * (5 + 3 * tan**2 + eta - 9 * tan**2 * eta) * d_e**4
        - tan / (720 * rho * nu**5) * (61 + 90 * tan**2 + 45 * tan**4) * d_e**6
    )
    lon = (
        GRID_LON0
        + d_e / (cos * nu)
        - (nu / rho + 2 * tan**2) / (6 * cos * nu**3) * d_e**3
        + (5 + 28 * tan**2 + 24 * tan**4) / (120 * cos * nu**5) * d_e**5
        - (61 + 662 * tan**2 + 1320 * tan**4 + 720 * tan**6) / (5040 * cos * nu**7) * d_e**7
    )
    nu_airy = AIRY_A / math.sqrt(1 - ee * math.sin(lat) ** 2)
    x, y, z = (
        nu_airy * math.cos(lat) * math.cos(lon),
        nu_airy * math.cos(lat) * math.sin(lon),
        nu_airy * (1 - ee) * math.sin(lat),
    )
    d_x, d_y, d_z, r_x, r_y, r_z, scale = HELMERT
    r_x, r_y, r_z = (math.radians(v / 3600) for v in (r_x, r_y, r_z))
    factor = 1 + scale * 1e-6
    x2 = d_x + factor * (x - r_z * y + r_y * z)
    y2 = d_y + factor * (r_z * x + y - r_x * z)
    z2 = d_z + factor * (-r_y * x + r_x * y + z)
    e2 = WGS_F * (2 - WGS_F)
    radius = math.hypot(x2, y2)
    latitude = math.atan2(z2, radius * (1 - e2))
    for _ in range(10):
        latitude = math.atan2(
            z2 + e2 * WGS_A / math.sqrt(1 - e2 * math.sin(latitude) ** 2) * math.sin(latitude),
            radius,
        )
    return math.degrees(latitude), math.degrees(math.atan2(y2, x2))


def _grid_pairs(text: str, *, comma: bool) -> list[list[float]]:
    """GeoJSON positions (longitude, latitude) from GML coordinates in Irish Grid: `x y x y` or `x,y x,y`."""
    if comma:
        pairs = [tuple(float(v) for v in item.split(",")[:2]) for item in text.split()]
    else:
        numbers = [float(v) for v in text.split()]
        pairs = list(zip(numbers[0::2], numbers[1::2], strict=True))
    result = []
    for easting, northing in pairs:
        latitude, longitude = irish_grid_to_wgs84(easting, northing)
        result.append([round(longitude, 7), round(latitude, 7)])
    return result


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def gml_collection(data: bytes, min_order: int | None = None) -> dict[str, Any]:
    """A GML feature collection in Irish Grid (DAERA's river segments and transitional waters downloads) as GeoJSON in
    longitude and latitude, with each feature's plain text properties. Lines and polygons only; with `min_order` only the
    features whose STRAHLER is at least that."""
    root = ElementTree.fromstring(data)
    features = []
    for member in root:
        if _local(member.tag) != "featureMember":
            continue
        feature = member[0]
        props: dict[str, Any] = {
            _local(c.tag): (c.text or "").strip() for c in feature if len(c) == 0
        }
        if min_order is not None and int(props.get("STRAHLER") or 0) < min_order:
            continue
        if props.get("STRAHLER"):
            props["STRAHLER"] = int(props["STRAHLER"])
        if "OBJECTID" in props:
            props["OBJECTID"] = int(props["OBJECTID"])
        polygons, lines = [], []
        for node in feature.iter():
            tag = _local(node.tag)
            if node.get("srsName", "EPSG:29902") != "EPSG:29902":
                message = f"GML feature in {node.get('srsName')}, not Irish Grid (EPSG:29902)"
                raise WatersError(message)
            if tag == "LineString":
                lines.append(
                    _grid_pairs(
                        next(c for c in node if _local(c.tag) in ("posList", "coordinates")).text
                        or "",
                        comma=False,
                    )
                )
            elif tag == "Polygon":
                rings = [
                    _grid_pairs(ring.text or "", comma=True)
                    for ring in node.iter()
                    if _local(ring.tag) == "coordinates"
                ]
                polygons.append(rings)
        if lines:
            geometry: dict[str, Any] = {"type": "MultiLineString", "coordinates": lines}
        elif polygons:
            geometry = {"type": "MultiPolygon", "coordinates": polygons}
        else:
            message = f"GML feature {props.get('OBJECTID', props.get('EU_CD', '?'))} has no line or polygon"
            raise WatersError(message)
        features.append({"type": "Feature", "properties": props, "geometry": geometry})
    return {"type": "FeatureCollection", "features": features}


def gml_from_archive(archive: Path, min_order: int | None = None) -> dict[str, Any]:
    """The one .gml in a downloaded zip, as `gml_collection` reads it."""
    with zipfile.ZipFile(archive) as bundle:
        names = [n for n in bundle.namelist() if n.lower().endswith(".gml")]
        if len(names) != 1:
            message = f"{archive.name} holds {len(names)} .gml files, not one"
            raise WatersError(message)
        return gml_collection(bundle.read(names[0]), min_order)


# ---- the survey layers, their licences and `fetch` ----------------------------------------------------------------------

EPA_OWS = "https://gis.epa.ie/geoserver/EPA/ows"
NI_SERVICES = "https://services-eu1.arcgis.com/kswen6BYexuc1SUk/arcgis/rest/services"
PAGE_SIZE = 1500
MANIFEST = "waters-manifest.json"
EPA_LICENCE = "Creative Commons CC BY 4.0 (<https://creativecommons.org/licenses/by/4.0/>)"
EPA_ATTRIBUTION = (
    "Contains Environmental Protection Agency (Ireland) data licensed under CC BY 4.0."
)
OGL_LICENCE = "UK Open Government Licence v3.0 (<https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/>)"


@dataclass(frozen=True)
class Layer:
    """A survey layer: where it is, and the evidence for its licence (rule 3). Only a confirmed layer is fetched or used."""

    key: str
    title: str
    url: str
    licence: str
    attribution: str
    evidence: str
    confirmed: bool = True
    pages: Callable[[int], str] | None = None  # an ArcGIS layer, one page of features at an offset
    archive: bool = False  # a zip of GML in Irish Grid, converted when fetched
    min_order: int | None = (
        None  # for the river segments of an archive: the least stream order kept
    )


def wfs_url(type_name: str, cql: str | None = None) -> str:
    """A WFS request for a whole layer as GeoJSON in latitude and longitude."""
    url = f"{EPA_OWS}?service=WFS&version=2.0.0&request=GetFeature&typeNames=EPA:{type_name}&outputFormat=application/json&srsName=EPSG:4326"
    return url + (f"&cql_filter={urllib.parse.quote(cql)}" if cql else "")


def arcgis_url(service: str, layer: int, where: str, offset: int = 0, size: int = PAGE_SIZE) -> str:
    """One page of an ArcGIS feature service layer as GeoJSON, in a stable order."""
    query = urllib.parse.urlencode(
        {
            "where": where,
            "outFields": "*",
            "outSR": 4326,
            "orderByFields": "OBJECTID",
            "resultOffset": offset,
            "resultRecordCount": size,
            "f": "geojson",
        }
    )
    return f"{NI_SERVICES}/{service}/FeatureServer/{layer}/query?{query}"


EPA_EVIDENCE = 'the EPA\'s own catalogue record (<https://gis.epa.ie/geonetwork/srv/api/records/{record}/formatters/xml>, linked from the WFS capabilities) says "Data is for public use under Creative Commons CC-By 4.0" and links the licence; read 2026-11-16'
DAERA_PAGE = "<https://www.daera-ni.gov.uk/articles/wmu-spatial-datasets>"
NOT_SERVICE = "The same dataset's ArcGIS service ({service}) carries no licence text of its own and is not used; the download is an older edition, so its features differ from the service's."
LAYERS = (
    Layer(
        "epa-rivers",
        "Irish EPA river network routes (EPA:WATER_RIVNETROUTES, stream order 4 and above, and every line through a lough or tidal water)",
        wfs_url("WATER_RIVNETROUTES", "ORDER_>=4 OR Continua IN ('L','T','C')"),
        EPA_LICENCE,
        EPA_ATTRIBUTION,
        EPA_EVIDENCE.format(record="c4043e19-38ec-4120-a588-8cd01ac94a9c"),
    ),
    Layer(
        "epa-lakes",
        "Irish EPA lake water bodies (EPA:WFD_LAKEWATERBODIES_CYCLE3)",
        wfs_url("WFD_LAKEWATERBODIES_CYCLE3"),
        EPA_LICENCE,
        EPA_ATTRIBUTION,
        EPA_EVIDENCE.format(record="00e92dc9-cb26-4858-889c-0b9b6ea9c9be"),
    ),
    Layer(
        "epa-tidal",
        "Irish EPA transitional water bodies (EPA:WFD_TRANSITIONALWATERBODIES_CYCLE3)",
        wfs_url("WFD_TRANSITIONALWATERBODIES_CYCLE3"),
        EPA_LICENCE,
        EPA_ATTRIBUTION,
        EPA_EVIDENCE.format(record="5b3cd1d5-b2d7-4210-a821-0f6231129d58"),
    ),
    Layer(
        "epa-coastal",
        "Irish EPA coastal water bodies (EPA:WFD_COASTALWATERBODIES_CYCLE3), the outlines a [[sea]] entry may take",
        wfs_url("WFD_COASTALWATERBODIES_CYCLE3"),
        EPA_LICENCE,
        EPA_ATTRIBUTION,
        EPA_EVIDENCE.format(record="0b6c2e90-e56f-4cce-927e-57e86e9a60a3").replace(
            "read 2026-11-16", "read 2026-11-19"
        ),
    ),
    Layer(
        "ni-rivers",
        "DAERA Northern Ireland River Segments (OpenDataNI download riversegmentgml.zip, stream order 4 and above)",
        "https://www.daera-ni.gov.uk/sites/default/files/publications/doe/riversegmentgml.zip",
        OGL_LICENCE,
        "Contains public sector information licensed under the Open Government Licence v3.0. Source: Northern Ireland Environment Agency Water Management Unit, DAERA.",
        f"DAERA's own page {DAERA_PAGE} links 'Northern Ireland River Segments' to NIEA Water Management Unit's OpenDataNI record (<https://www.opendatani.gov.uk/@northern-ireland-environment-agency-water-management-unit/https-www-daera-ni-gov-uk-sites-default-files-publications-doe-riversegmentgml-zip>), whose licence is the UK Open Government Licence, and whose files are this download; read 2026-11-16. "
        + NOT_SERVICE.format(service="RiverSegments"),
        archive=True,
        min_order=MIN_ORDER,
    ),
    Layer(
        "ni-lakes",
        "DAERA Lakes of Northern Ireland (Lakes_of_Northern_Ireland)",
        arcgis_url("Lakes_of_Northern_Ireland", 0, "1=1"),
        OGL_LICENCE,
        "DAERA NIEA, contains OSNI information licensed under OGL v3.0.",
        "the publisher's ArcGIS item 5acc99bfd09e432a9db6d72272b96174 (owner DAERA_OpenContent) names this exact service and states the Open Government Licence v3.0, crediting DAERA/NIEA and OSNI; DAERA's Water Management Unit datasets page links the same record. Found by ck-reviewer-1, read 2026-11-16 (a copy of the item is kept with the CK-348 spec reviews, sha256 26e341c2...)",
        pages=lambda offset: arcgis_url("Lakes_of_Northern_Ireland", 0, "1=1", offset),
    ),
    Layer(
        "ni-tidal",
        "DAERA Northern Ireland Transitional Waters 2009-2015 (OpenDataNI download transitionalwaterbodiesgml.zip)",
        "https://admin.opendatani.gov.uk/dataset/42e0d654-e973-4c45-83a4-c19c57b33a90/resource/3dbb9f72-79db-41ee-afd5-cb5b935914db/download/transitionalwaterbodiesgml.zip",
        OGL_LICENCE,
        "Contains public sector information licensed under the Open Government Licence v3.0. Source: Northern Ireland Environment Agency, DAERA.",
        f"DAERA's own page {DAERA_PAGE} links 'Northern Ireland Transitional Waters (2009 - 2015)' to the OpenDataNI record (<https://www.opendatani.gov.uk/@northern-ireland-environment-agency-water-management-unit/norther>), whose licence is the UK Open Government Licence, and whose files include this download; read 2026-11-16. "
        + NOT_SERVICE.format(service="TransitionalWaterBodiesNewURLs layer 1"),
        archive=True,
    ),
)


def _download(url: str, path: Path) -> None:
    from ireland_grid import download  # noqa: PLC0415 -- needs numpy, which only `fetch` borrows

    download(url, path)


def _stored(url: str, path: Path) -> dict[str, Any]:
    _download(url, path)
    result: dict[str, Any] = json.loads(path.read_text(encoding="utf-8"))
    return result


def _fetch_layer(layer: Layer, source: Path) -> tuple[list[str], list[Any], dict[str, str]]:
    """The addresses read, the features, and the SHA-256 of any archive."""
    if layer.archive:
        archive = source / f"{layer.key}.zip"
        _download(layer.url, archive)
        collection = gml_from_archive(archive, layer.min_order)
        return (
            [layer.url],
            collection["features"],
            {"archive_sha256": hashlib.sha256(archive.read_bytes()).hexdigest()},
        )
    if layer.pages is not None:
        urls: list[str] = []
        features: list[Any] = []
        offset = 0
        while True:
            page_url = layer.pages(offset)
            page = source / f"{layer.key}.page{offset // PAGE_SIZE}.json"
            body = _stored(page_url, page)
            page.unlink()
            urls.append(page_url)
            features += body["features"]
            if (
                not body.get("properties", {}).get("exceededTransferLimit", False)
                and len(body["features"]) < PAGE_SIZE
            ):
                return urls, features, {}
            offset += PAGE_SIZE
    target = source / f"{layer.key}.json"
    body = _stored(layer.url, target)
    if body.get("numberMatched", len(body["features"])) != len(body["features"]):
        message = f"fetch: {layer.key} returned {len(body['features'])} of {body['numberMatched']} features"
        raise WatersError(message)
    return [layer.url], body["features"], {}


def fetch(source: Path, *, force: bool, today: str | None = None) -> dict[str, Any]:
    """`fetch`: the confirmed layers into the cache, and a manifest of each one's address, day, features and SHA-256."""
    used = [layer for layer in LAYERS if layer.confirmed]
    outputs = (
        [source / f"{layer.key}.{'zip' if layer.archive else 'json'}" for layer in used]
        + [source / f"{layer.key}.json" for layer in used if layer.archive]
        + [source / MANIFEST]
    )
    present = [p.name for p in outputs if p.exists()]
    if present and not force:
        message = f"fetch: {', '.join(present)} already in {source}; pass --force to fetch again"
        raise WatersError(message)
    source.mkdir(parents=True, exist_ok=True)
    for path in outputs:
        path.unlink(missing_ok=True)
    day = today or date.today().isoformat()
    manifest: dict[str, Any] = {"fetched": day, "layers": [], "left_out": []}
    for layer in LAYERS:
        if not layer.confirmed:
            manifest["left_out"].append(
                {
                    "key": layer.key,
                    "title": layer.title,
                    "reason": f"licence not confirmed: {layer.evidence}",
                }
            )
            continue
        urls, fetched, extra = _fetch_layer(layer, source)
        # the server's per-request feature ids are dropped, so the same data is the same bytes
        features = [
            {
                "type": "Feature",
                "properties": f.get("properties", {}),
                "geometry": f.get("geometry"),
            }
            for f in fetched
        ]
        target = source / f"{layer.key}.json"
        target.write_text(
            json.dumps({"type": "FeatureCollection", "features": features}, separators=(",", ":")),
            encoding="utf-8",
        )
        manifest["layers"].append(
            {
                "key": layer.key,
                "title": layer.title,
                "addresses": urls,
                "day": day,
                "features": len(features),
                "sha256": hashlib.sha256(target.read_bytes()).hexdigest(),
                "licence": layer.licence,
                **extra,
            }
        )
    (source / MANIFEST).write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return manifest


def load_surveys(source: Path) -> Surveys:
    """The cache as plain values; a layer that was not fetched is named in `missing`."""
    manifest_path = source / MANIFEST
    if not manifest_path.exists():
        message = (
            f"{manifest_path} is missing: run `import_waters.py fetch --source {source}` first"
        )
        raise WatersError(message)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    present = {layer["key"]: layer for layer in manifest["layers"]}

    def read(key: str) -> dict[str, Any] | None:
        return (
            json.loads((source / f"{key}.json").read_text(encoding="utf-8"))
            if key in present
            else None
        )

    surveys = Surveys()
    for key, parse in (("epa-rivers", epa_segments), ("ni-rivers", ni_segments)):
        if (data := read(key)) is not None:
            surveys.rivers += [
                s
                for s in parse(data)
                if s.order >= MIN_ORDER or (s.source == "epa" and s.continua in ("L", "T", "C"))
            ]
    for key, source_name, code, name, area, target in (
        (
            "epa-lakes",
            "epa",
            "EU_CD",
            "NAME",
            lambda p: float(p.get("AreaKm2") or 0),
            surveys.lakes,
        ),
        (
            "ni-lakes",
            "ni",
            "lake_code",
            "name",
            lambda p: float(p.get("squ_metre") or 0) / 1e6,
            surveys.lakes,
        ),
        (
            "epa-tidal",
            "epa",
            "EU_CD",
            "NAME",
            lambda p: float(p.get("AreaKm2") or 0),
            surveys.tidal,
        ),
        (
            "epa-coastal",
            "epa",
            "EU_CD",
            "NAME",
            lambda p: float(p.get("AreaKm2") or 0),
            surveys.coastal,
        ),
        (
            "ni-tidal",
            "ni",
            "EU_CD",
            "NAME",
            lambda p: float(p.get("Shape_Area") or 0) / 1e6,
            surveys.tidal,
        ),
    ):
        if (data := read(key)) is not None:
            target.extend(outlines(data, source_name, code, name, area))
    surveys.layers = [
        {
            "title": v["title"],
            "features": v["features"],
            "day": v["day"],
            "sha256": v["sha256"],
            "licence": v["licence"],
        }
        for v in manifest["layers"]
    ]
    if "ni-tidal" not in present:
        surveys.missing.append(
            "Northern Ireland's tidal outlines: no layer with a confirmed licence, so its estuaries are unchanged and slice 4 authors them as polygon edits"
        )
    if "epa-coastal" not in present:
        surveys.missing.append(
            "The coastal outlines: not in the cache, so a [[sea]] entry can open a mouth only by its points"
        )
    surveys.missing.append(
        "Northern Ireland's river names: the survey has none, so every Northern Ireland river is named by hand in waters.toml"
    )
    return surveys


# ---- the map's images ----------------------------------------------------------------------------------------------------

ELEVATION_OFFSET = 11_000


def terrain_keys(map_dir: Path) -> dict[str, RGB]:
    """The key colour of each terrain name from `terrain.toml`."""
    rules = tomllib.loads((map_dir / "terrain.toml").read_text(encoding="utf-8"))
    keys = {}
    for entry in rules["terrain"]:
        text = entry["key"].lstrip("#")
        keys[entry["name"]] = (int(text[0:2], 16), int(text[2:4], 16), int(text[4:6], 16))
    return keys


def _pixels(image: Image.Image) -> Iterable[Any]:
    """The pixels of an image in row order (Pillow renamed `getdata`)."""
    return getattr(image, "get_flattened_data", image.getdata)()


def read_map(map_dir: Path) -> MapGrid:
    """The map's crop, terrain classes and elevations (metres)."""
    from PIL import Image  # noqa: PLC0415 -- only the images need it

    document = tomllib.loads((map_dir / "map.toml").read_text(encoding="utf-8"))["crop"]
    crop = Crop(
        (document["origin"][0], document["origin"][1]), document["width"], document["height"]
    )
    classes = {key: NAMES.index(name) for name, key in terrain_keys(map_dir).items()}
    with Image.open(map_dir / "terrain.png") as image:
        if image.size != (crop.width, crop.height):
            message = f"terrain.png is {image.size}, map.toml says {(crop.width, crop.height)}"
            raise WatersError(message)
        pixels = list(_pixels(image.convert("RGB")))
    with Image.open(map_dir / "elevation.png") as image:
        heights = list(_pixels(image))
    terrain = []
    for row in range(crop.height):
        try:
            terrain.append([classes[pixels[row * crop.width + c]] for c in range(crop.width)])
        except KeyError as error:
            message = f"terrain.png has a pixel that is not a key of terrain.toml: {error}"
            raise WatersError(message) from None
    elevation = [
        [float(heights[r * crop.width + c]) - ELEVATION_OFFSET for c in range(crop.width)]
        for r in range(crop.height)
    ]
    return MapGrid(crop, terrain, elevation)


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
STORED_BLOCK = 65_535


def _chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def png_bytes(rows: Sequence[Sequence[int]], keys: dict[str, RGB]) -> bytes:
    """`water.png` as a fixed PNG encoding: 4-bit palette (black, then the seven terrain keys in `NAMES` order), filter 0 on
    every row, and zlib *stored* blocks of up to 65,535 bytes, so the bytes are the same whatever Pillow or zlib is installed."""
    width, height = len(rows[0]), len(rows)
    raw = bytearray()
    for row in rows:
        raw.append(0)
        indices = [0 if value == UNCHANGED else value + 1 for value in row]
        indices += [0] * (width % 2)
        raw.extend(high << 4 | low for high, low in zip(indices[0::2], indices[1::2], strict=True))
    stream = bytearray(b"\x78\x01")
    for start in range(0, len(raw), STORED_BLOCK):
        block = raw[start : start + STORED_BLOCK]
        final = start + STORED_BLOCK >= len(raw)
        stream += struct.pack("<BHH", 1 if final else 0, len(block), len(block) ^ 0xFFFF) + block
    stream += struct.pack(">I", zlib.adler32(bytes(raw)))
    palette = b"".join(bytes(colour) for colour in (BLACK, *(keys[name] for name in NAMES)))
    return (
        PNG_SIGNATURE
        + _chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 4, 3, 0, 0, 0))
        + _chunk(b"PLTE", palette)
        + _chunk(b"IDAT", bytes(stream))
        + _chunk(b"IEND", b"")
    )


def write_water_png(path: Path, rows: Sequence[Sequence[int]], keys: dict[str, RGB]) -> None:
    """`water.png`: the key colour of the new terrain where a cell changes, black everywhere else (`png_bytes`)."""
    path.write_bytes(png_bytes(rows, keys))


def read_locations(path: Path) -> dict[str, LatLon]:
    """Locations from a scenario's `locations.toml`: id, lat and lon."""
    document = tomllib.loads(path.read_text(encoding="utf-8"))
    return {
        entry["id"]: (float(entry["lat"]), float(entry["lon"]))
        for entry in document.get("location", [])
    }


# ---- the command line ----------------------------------------------------------------------------------------------------


def _refuse_existing(paths: Sequence[Path], force: bool) -> None:
    present = [p.name for p in paths if p.exists()]
    if present and not force:
        message = f"{', '.join(present)} already exists in {paths[0].parent}; pass --force to overwrite it"
        raise SystemExit(message)


def main(argv: Sequence[str] | None = None) -> int:
    """Runs one of the three commands."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", maxsplit=1)[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("fetch", "propose", "build"):
        command = commands.add_parser(name)
        command.add_argument(
            "--source", type=Path, required=True, help="the download cache (not committed)"
        )
        command.add_argument("--force", action="store_true", help="overwrite an existing output")
        if name != "fetch":
            command.add_argument(
                "--out", type=Path, required=True, help="where the outputs are written"
            )
            command.add_argument(
                "--map",
                type=Path,
                default=HERE,
                help="the map directory to read (default: beside this script)",
            )
        if name == "build":
            command.add_argument(
                "--waters", type=Path, help="the list (default: waters.toml in the map directory)"
            )
            command.add_argument(
                "--locations",
                type=Path,
                help="a scenario's locations.toml: each location's cell keeps its terrain",
            )
    arguments = parser.parse_args(argv)
    try:
        if arguments.command == "fetch":
            manifest = fetch(arguments.source, force=arguments.force)
            for layer in manifest["layers"]:
                print(f"{layer['key']}: {layer['features']} features, sha256 {layer['sha256']}")
            for layer in manifest["left_out"]:
                print(f"{layer['key']}: left out, {layer['reason']}")
            return 0
        surveys = load_surveys(arguments.source)
        grid = read_map(arguments.map)
        arguments.out.mkdir(parents=True, exist_ok=True)
        if arguments.command == "propose":
            target = arguments.out / "waters.toml"
            _refuse_existing([target], arguments.force)
            target.write_text(propose_waters(surveys, grid), encoding="utf-8")
            print(f"wrote {target}")
            return 0
        waters_path = arguments.waters or arguments.map / "waters.toml"
        text = waters_path.read_text(encoding="utf-8")
        outputs = [arguments.out / n for n in ("rivers.toml", "water.png", "waters-report.md")]
        _refuse_existing(outputs, arguments.force)
        locations = read_locations(arguments.locations) if arguments.locations else {}
        result = build_waters(
            parse_waters(text),
            surveys,
            grid,
            locations,
            hashlib.sha256(text.encode("utf-8")).hexdigest(),
        )
        outputs[0].write_text(result.rivers_toml, encoding="utf-8")
        write_water_png(outputs[1], result.water, terrain_keys(arguments.map))
        outputs[2].write_text(result.report, encoding="utf-8")
        for path in outputs:
            print(f"wrote {path} sha256 {hashlib.sha256(path.read_bytes()).hexdigest()}")
    except WatersError as error:
        raise SystemExit(f"import_waters: {error}") from None
    return 0


if __name__ == "__main__":
    sys.exit(main())
