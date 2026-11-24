"""Travel network geometry and build tooling."""

import argparse
import copy
import datetime
import hashlib
import heapq
import importlib
import io
import json
import math
import sys
import tempfile
import tomllib
from collections.abc import Callable, Hashable
from fractions import Fraction
from functools import lru_cache
from itertools import pairwise
from pathlib import Path
from typing import Any, TypeVar

Cell = tuple[int, int]
Grid = list[list[int]]
Land = list[list[bool]]
_TWO = 2
_COST_SCALE = 1_000_000
_MAX_LEVEL = 5
_MAX_LATITUDE = 90
_MAX_LONGITUDE = 180
_MIN_RIVER_WIDTH_M = 20
Key = TypeVar("Key", bound=Hashable)


def legal_course(land: Land, cells: list[Cell]) -> bool:
    """Check the mock-up's sixteen moves without changing the course."""

    def on_land(cell: Cell) -> bool:
        x, y = cell
        return 0 <= y < len(land) and 0 <= x < len(land[y]) and land[y][x]

    if not cells or not all(on_land(cell) for cell in cells):
        return False
    for (x, y), (nx, ny) in pairwise(cells):
        dx, dy = nx - x, ny - y
        if (abs(dx), abs(dy)) not in {(1, 0), (0, 1), (1, 1), (2, 1), (1, 2)}:
            return False
        middle: tuple[Cell, ...]
        if abs(dx) == _TWO:
            middle = ((x + dx // 2, y), (x + dx // 2, ny))
        elif abs(dy) == _TWO:
            middle = ((x, y + dy // 2), (nx, y + dy // 2))
        elif dx and dy:
            middle = ((nx, y), (x, ny))
        else:
            middle = ()
        if not all(on_land(cell) for cell in middle):
            return False
    return True


_MOVES = tuple(
    (dx, dy)
    for dx in range(-2, 3)
    for dy in range(-2, 3)
    if (abs(dx), abs(dy)) in {(1, 0), (0, 1), (1, 1), (2, 1), (1, 2)}
)
_DISTANCE = {(1, 0): 1000000, (0, 1): 1000000, (1, 1): 1414214, (2, 1): 2236068, (1, 2): 2236068}


def route(  # noqa: PLR0913, PLR0912, PLR0915  # Route options are independent and search handles all move cases.
    land: Land,
    costs: Grid,
    waypoints: list[Cell],
    *,
    earlier: set[Cell] | frozenset[Cell] = frozenset(),
    reuse: int = _COST_SCALE,
    elevation: Grid | None = None,
    slope: int = 0,
    crossings: dict[tuple[Cell, Cell], int] | None = None,
    crossing_fn: Callable[[Cell, Cell], int] | None = None,
) -> list[Cell]:
    """Find a deterministic minimum integer-cost course through ordered cells."""
    crossings = crossings or {}
    if slope < 0:
        raise ValueError("slope must be nonnegative")
    if not 0 < reuse <= _COST_SCALE:
        raise ValueError("reuse must be between 1 and 1000000")
    if not waypoints or not all(legal_course(land, [p]) for p in waypoints):
        raise ValueError("no course: waypoint is not on land")
    if any(value < 0 for value in crossings.values()):
        raise ValueError("crossing costs must be nonnegative")
    minimum = min(
        costs[y][x] for y, row in enumerate(land) for x, is_land in enumerate(row) if is_land
    )
    if minimum <= 0:
        raise ValueError("ground costs must be positive")
    lower_bound = max(0, 2 * minimum * reuse // _COST_SCALE - 1)
    result = [waypoints[0]]
    for start, goal in pairwise(waypoints):

        def estimate(cell: Cell, target: Cell = goal) -> int:
            dx, dy = target[0] - cell[0], target[1] - cell[1]
            return math.isqrt((dx * dx + dy * dy) * 1_000_000_000_000) * lower_bound

        best = {start: 0}
        parent = {}
        queue = [(estimate(start), 0, start)]
        while queue:
            _, cost, current = heapq.heappop(queue)
            if cost != best[current]:
                continue
            if current == goal:
                break
            x, y = current
            for dx, dy in _MOVES:
                nxt = (x + dx, y + dy)
                if not legal_course(land, [current, nxt]):
                    continue
                nx, ny = nxt
                step = _DISTANCE[abs(dx), abs(dy)] * (costs[y][x] + costs[ny][nx])
                if elevation is not None:
                    climb = abs(int(elevation[ny][nx]) - int(elevation[y][x]))
                    step += (
                        (costs[y][x] + costs[ny][nx])
                        * climb
                        * slope
                        * _COST_SCALE
                        // (500 * _COST_SCALE)
                    )
                edge = (current, nxt) if current < nxt else (nxt, current)
                extra = crossings.get(edge, 0) + (crossing_fn(*edge) if crossing_fn else 0)
                if extra < 0:
                    raise ValueError("crossing costs must be nonnegative")
                step += extra
                if current in earlier and nxt in earlier:
                    step = step * reuse // _COST_SCALE
                if step <= 0:
                    raise ValueError("ground costs must be positive")
                candidate = cost + step
                if nxt not in best or candidate < best[nxt]:
                    best[nxt] = candidate
                    parent[nxt] = current
                    heapq.heappush(queue, (candidate + estimate(nxt), candidate, nxt))
        if goal not in best:
            raise ValueError(f"no course from {start} to {goal}")
        section = []
        cursor = goal
        while cursor != start:
            section.append(cursor)
            cursor = parent[cursor]
        result.extend(reversed(section))
    return result


def complete_join(land: Land, parent: list[Cell], child: list[Cell]) -> list[Cell]:
    """Complete a seed ending on the mock-up's marked ground beside a parent."""
    if not parent or not child:
        raise ValueError("empty seed course")
    endpoint = child[-1]
    if endpoint in parent:
        return list(child)
    for start, end in pairwise(parent):
        marked = ((start[0] + end[0]) // 2, (start[1] + end[1]) // 2)
        if endpoint != marked:
            continue
        for candidate in (start, end):
            if legal_course(land, [endpoint, candidate]):
                return [candidate] if len(child) == 1 else [*child, candidate]
    raise ValueError(f"no legal completion for {endpoint}")


def ground_mark(course: list[Cell], *layers: Grid, crossings: tuple[Any, ...] = ()) -> str:
    """Hash the course, its supporting cells, layer values, and river crossings."""
    support = set(course)
    for (x, y), (nx, ny) in pairwise(course):
        dx, dy = nx - x, ny - y
        if abs(dx) == _TWO:
            support.update(((x + dx // 2, y), (x + dx // 2, ny)))
        elif abs(dy) == _TWO:
            support.update(((x, y + dy // 2), (nx, y + dy // 2)))
        elif dx and dy:
            support.update(((nx, y), (x, ny)))
    values = [[x, y, *[layer[y][x] for layer in layers]] for x, y in sorted(support)]
    payload = [course, values, sorted(crossings)]
    return hashlib.sha256(
        json.dumps(payload, separators=(",", ":"), ensure_ascii=True).encode()
    ).hexdigest()


def terrain_land(rows: list[list[tuple[int, ...]]], rules: list[dict[str, str]]) -> Land:
    """Interpret terrain colours using the game's terrain kinds."""
    kinds = {tuple(bytes.fromhex(rule["key"])): rule["kind"] for rule in rules}
    result = []
    for row in rows:
        output = []
        for colour in row:
            if colour not in kinds:
                raise ValueError(f"unknown terrain colour {colour}")
            output.append(kinds[colour] not in {"sea", "lake"})
        result.append(output)
    return result


def terrain_costs(
    kinds: list[list[str]], fertility: Grid, prices: dict[str, str], discount: str
) -> Grid:
    """Apply the mock-up's fertility formula with fixed-point rational arithmetic."""
    discount_value = Fraction(discount)
    prices_fraction = {name: Fraction(value) for name, value in prices.items()}
    result = []
    cached: dict[tuple[str, int], int] = {}
    for names, values in zip(kinds, fertility, strict=True):
        row = []
        for name, value in zip(names, values, strict=True):
            if name not in prices_fraction:
                row.append(0)
                continue
            key = (name, value)
            if key not in cached:
                factor = prices_fraction[name] * (
                    1 + discount_value / 2 - discount_value * value / 250
                )
                amount = factor * 1_000_000
                cached[key] = (amount.numerator * 2 + amount.denominator) // (
                    2 * amount.denominator
                )
            row.append(cached[key])
        result.append(row)
    return result


def split_courses(  # noqa: PLR0912  # Node and shared-edge cases are resolved together.
    courses: dict[str, list[Cell]], named_nodes: set[Cell]
) -> tuple[list[tuple[Cell, ...]], dict[str, list[tuple[int, str]]]]:
    """Cut courses at nodes and share an identical stretch in either direction."""
    neighbours: dict[Cell, set[Cell]] = {}
    owners: dict[tuple[Cell, Cell], set[str]] = {}
    nodes = set(named_nodes)
    for name, course in courses.items():
        if not course:
            raise ValueError(f"{name}: empty course")
        nodes.update((course[0], course[-1]))
        seen: set[Cell] = set()
        for cell in course:
            if cell in seen:
                nodes.add(cell)
            seen.add(cell)
        for left, right in pairwise(course):
            if left == right:
                raise ValueError(f"{name}: repeated consecutive cell {left}")
            neighbours.setdefault(left, set()).add(right)
            neighbours.setdefault(right, set()).add(left)
            edge = (left, right) if left < right else (right, left)
            owners.setdefault(edge, set()).add(name)
    for cell, adjacent in neighbours.items():
        if len(adjacent) != _TWO:
            nodes.add(cell)
            continue
        a, b = sorted(adjacent)
        first = (a, cell) if a < cell else (cell, a)
        second = (b, cell) if b < cell else (cell, b)
        if owners[first] != owners[second]:
            nodes.add(cell)
    lines: list[tuple[Cell, ...]] = []
    ids: dict[tuple[Cell, ...], int] = {}
    links: dict[str, list[tuple[int, str]]] = {}
    for name, course in courses.items():
        entries: list[tuple[int, str]] = []
        start = 0
        for index in range(1, len(course)):
            if course[index] not in nodes:
                continue
            line = tuple(course[start : index + 1])
            reverse = line[::-1]
            key = min(line, reverse)
            if key not in ids:
                ids[key] = len(lines)
                lines.append(key)
            entries.append((ids[key], "forward" if line == key else "backward"))
            start = index
        links[name] = entries
    return lines, links


def _is_integer(value: object) -> bool:
    """A plain integer: not a float of the same value, and not a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


_ID_LIMIT = 2**64 - 1


def next_counter(name: str, counter: int) -> int:
    """The counter after handing out its id, refused when the counter would leave the range of the loader's `u64`."""
    if counter + 1 > _ID_LIMIT:
        raise ValueError(
            f"{name} is {counter}: the next id would leave the range of an unsigned 64-bit integer"
        )
    return counter + 1


def allocate_ids(
    keys: list[Key], previous: dict[Key, int], next_id: int, name: str = "id counter"
) -> tuple[dict[Key, int], int]:
    """Preserve existing IDs and allocate new IDs above the historical high water mark, up to the largest the loader reads."""
    if (
        not _is_integer(next_id)
        or any(not _is_integer(value) for value in previous.values())
        or len(set(previous.values())) != len(previous)
        or any(value >= next_id for value in previous.values())
    ):
        raise ValueError("invalid ID counter or duplicate ID")
    if len(set(keys)) != len(keys):
        raise ValueError("duplicate identity")
    result: dict[Key, int] = {}
    for key in keys:
        if key in previous:
            result[key] = previous[key]
        else:
            result[key] = next_id
            next_id = next_counter(name, next_id)
    return result, next_id


def cell_side_lengths_mm(origin_row: int, height: int) -> list[int]:
    """Ground length of each projected cell side at its row's midpoint."""
    radius = 6_378_137.0
    cell_m = 839.0
    return [
        math.floor(
            cell_m
            * 1000
            / math.cosh((math.pi * radius - (origin_row + row + 0.5) * cell_m) / radius)
            + 0.5
        )
        for row in range(height)
    ]


def ground_length(cells: list[Cell], origin: Cell) -> int:
    """Distance between cell centres on the spherical earth, rounded to metres."""
    radius = 6_378_137.0
    cell_m = 839.0
    half_world = math.pi * radius

    def centre(cell: Cell) -> tuple[float, float]:
        column, row = cell
        x = (origin[0] + column + 0.5) * cell_m
        y = (origin[1] + row + 0.5) * cell_m
        lon = (x - half_world) / radius
        lat = 2 * math.atan(math.exp((half_world - y) / radius)) - math.pi / 2
        return lat, lon

    total = 0.0
    for a, b in pairwise(cells):
        lat1, lon1 = centre(a)
        lat2, lon2 = centre(b)
        hav = math.sin((lat2 - lat1) / 2) ** 2 + (
            math.cos(lat1) * math.cos(lat2) * math.sin((lon2 - lon1) / 2) ** 2
        )
        total += 2 * radius * math.asin(min(1, math.sqrt(hav)))
    return math.floor(total + 0.5)


def terrain_names(
    rows: list[list[tuple[int, ...]]], rules: list[dict[str, str]]
) -> list[list[str]]:
    """Read terrain names from the same palette that defines land and water."""
    names = {tuple(bytes.fromhex(rule["key"])): rule["name"] for rule in rules}
    result = []
    for row in rows:
        output = []
        for colour in row:
            if colour not in names:
                raise ValueError(f"unknown terrain colour {colour}")
            output.append(names[colour])
        result.append(output)
    return result


def read_rasters(map_dir: Path) -> dict[str, Any]:
    """Read the game's terrain and numeric layers without consulting water.png."""
    image_module = importlib.import_module("PIL.Image")

    crop = tomllib.loads((map_dir / "map.toml").read_text(encoding="utf-8"))["crop"]
    rules = tomllib.loads((map_dir / "terrain.toml").read_text(encoding="utf-8"))["terrain"]
    width, height = crop["width"], crop["height"]

    def pixels(name: str, mode: str | None = None) -> list[Any]:
        with image_module.open(map_dir / name) as image:
            if image.size != (width, height):
                raise ValueError(f"{name}: size does not match map crop")
            converted = image.convert(mode) if mode is not None else image
            return list(getattr(converted, "get_flattened_data", converted.getdata)())

    colours = pixels("terrain.png", "RGB")
    rows = [colours[row * width : (row + 1) * width] for row in range(height)]
    names = terrain_names(rows, rules)
    land = terrain_land(rows, rules)
    kind_by_colour = {tuple(bytes.fromhex(rule["key"])): rule["kind"] for rule in rules}
    kinds = [[kind_by_colour[colour] for colour in row] for row in rows]
    elevation_values = pixels("elevation.png")
    fertility_values = pixels("fertility.png")
    elevation = [
        [int(elevation_values[y * width + x]) - 11000 for x in range(width)] for y in range(height)
    ]
    fertility = [
        [int(fertility_values[y * width + x]) for x in range(width)] for y in range(height)
    ]
    return {
        "origin": tuple(crop["origin"]),
        "terrain": names,
        "kind": kinds,
        "land": land,
        "elevation": elevation,
        "fertility": fertility,
    }


def river_crossings(
    course: list[Cell], river: list[Cell], widths: tuple[Fraction | int, Fraction | int]
) -> list[tuple[Fraction, Fraction]]:
    """Return crossing position on the course and interpolated river width."""

    def lengths(points: list[Cell]) -> list[int]:
        return [
            math.isqrt(((b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2) * 10**12)
            for a, b in pairwise(points)
        ]

    def cross(a: Cell, b: Cell) -> int:
        return a[0] * b[1] - a[1] * b[0]

    course_lengths = lengths(course)
    river_lengths = lengths(river)
    course_total, river_total = sum(course_lengths), sum(river_lengths)
    if not course_total or not river_total:
        return []
    found: set[tuple[Fraction, Fraction]] = set()
    course_before = 0
    for (a, b), road_length in zip(pairwise(course), course_lengths, strict=True):
        river_before = 0
        road_delta = (b[0] - a[0], b[1] - a[1])
        for (p, q), river_length in zip(pairwise(river), river_lengths, strict=True):
            river_delta = (q[0] - p[0], q[1] - p[1])
            delta = (p[0] - a[0], p[1] - a[1])
            denominator = cross(road_delta, river_delta)
            if denominator:
                along_road = Fraction(cross(delta, river_delta), denominator)
                along_river = Fraction(cross(delta, road_delta), denominator)
                if 0 <= along_road <= 1 and 0 <= along_river <= 1:
                    road_position = (course_before + along_road * road_length) / course_total
                    river_position = (river_before + along_river * river_length) / river_total
                    width = widths[0] + (widths[1] - widths[0]) * river_position
                    found.add((road_position, width))
            river_before += river_length
        course_before += road_length
    return sorted(found)


def snapshot(files: dict[str, Path]) -> dict[str, str | None]:
    """Bind a proposal to the bytes, or absence, of each named input."""
    return {
        name: hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
        for name, path in sorted(files.items())
    }


def snapshot_matches(files: dict[str, Path], captured: dict[str, str | None]) -> bool:
    """Compare every proposal input, including a previously absent stored network."""
    return snapshot(files) == captured


def starting_state(states: list[tuple[str, int]]) -> tuple[str, int]:
    """Choose the strongest starting state among links using one stretch."""
    if not states or any(
        kind not in {"road", "path"} or not 0 <= level <= _MAX_LEVEL for kind, level in states
    ):
        raise ValueError("invalid starting state")
    return max(states, key=lambda state: (state[0] == "road", state[1]))


def project_cell(lat: float, lon: float, origin: Cell) -> Cell:
    """Project latitude and longitude to the map crop's containing cell."""
    radius = 6_378_137.0
    half_world = math.pi * radius
    if not -_MAX_LATITUDE < lat < _MAX_LATITUDE or not -_MAX_LONGITUDE <= lon <= _MAX_LONGITUDE:
        raise ValueError("latitude or longitude outside the projection")
    x = radius * math.radians(lon) + half_world
    y = half_world - radius * math.log(math.tan(math.pi / 4 + math.radians(lat) / 2))
    return math.floor(x / 839) - origin[0], math.floor(y / 839) - origin[1]


def validate_hand_list(  # noqa: PLR0912, PLR0915  # Validation names the table and fault at each boundary.
    data: dict[str, Any], locations: dict[str, Cell], land: Land, origin: Cell = (0, 0)
) -> None:
    """Refuse malformed hand-list entries before a proposal writes anything."""

    def keys(
        table: str, entry: dict[str, Any], allowed: set[str], required: frozenset[str] = frozenset()
    ) -> None:
        if not isinstance(entry, dict):
            raise ValueError(f"{table}: expected a table")
        unknown = set(entry) - allowed
        if unknown:
            raise ValueError(f"{table}: unknown key {sorted(unknown)[0]}")
        missing = required - set(entry)
        if missing:
            raise ValueError(f"{table}: missing {sorted(missing)[0]}")

    def on_land(cell: Cell, label: str) -> None:
        if not legal_course(land, [cell]):
            raise ValueError(f"{label}: cell {cell} outside crop or on water")

    def cell_value(value: object, label: str) -> Cell:
        if (
            not isinstance(value, list)
            or len(value) != _TWO
            or any(type(n) is not int for n in value)
        ):
            raise ValueError(f"{label}: cell must have two integers")
        cell = (value[0], value[1])
        on_land(cell, label)
        return cell

    keys(
        "network",
        data,
        {
            "costs",
            "road",
            "access",
            "landing",
            "link",
            "crossing",
            "water",
            "water_class",
            "river_break",
            "mark",
        },
    )
    validate_water(data, locations)
    for entry in data.get("crossing", []):
        fields = {"river", "at", "kind", "note"}
        keys("crossing", entry, fields, frozenset(fields))
        if any(not isinstance(entry[name], str) or not entry[name] for name in fields - {"at"}):
            raise ValueError("crossing: river, kind and note must be nonempty strings")
        at = entry["at"]
        if (
            not isinstance(at, list)
            or len(at) != _TWO
            or any(type(value) is not int for value in at)
            or not 0 <= at[1] < len(land)
            or not 0 <= at[0] < len(land[0])
        ):
            raise ValueError("crossing: at must be a cell inside the crop")
        if entry["kind"] not in {"ford", "none"}:
            raise ValueError(f"crossing: invalid kind {entry['kind']}")
    if "costs" in data:
        costs = data["costs"]
        fields = {"terrain", "slope", "fertile", "crossing_base", "crossing_per_m", "reuse"}
        keys("costs", costs, fields, frozenset(fields))
        if not isinstance(costs["terrain"], dict) or not costs["terrain"]:
            raise ValueError("costs terrain: expected nonempty table")
        for name, number in costs["terrain"].items():
            if type(number) not in (int, float) or not math.isfinite(number) or number <= 0:
                raise ValueError(f"costs terrain {name}: expected a positive number")
        for name in fields - {"terrain"}:
            number = costs[name]
            if type(number) not in (int, float) or not math.isfinite(number) or number < 0:
                raise ValueError(f"costs {name}: expected a nonnegative number")
        if not 0 < costs["reuse"] <= 1:
            raise ValueError("costs reuse: expected a value above zero and at most one")
    roads: set[str] = set()
    for item in data.get("road", []):
        keys("road", item, {"id", "name"}, frozenset({"id", "name"}))
        name = item["id"]
        if name in roads:
            raise ValueError(f"road {name}: repeated id")
        roads.add(name)
    accesses: set[str] = set()
    for item in data.get("access", []):
        keys("access", item, {"place", "quality"}, frozenset({"place", "quality"}))
        name = item["place"]
        if name in accesses or name not in locations:
            raise ValueError(f"access {name}: repeated or unknown place")
        if item["quality"] not in {"offshore", "offshore_at_high_tide"}:
            raise ValueError(f"access {name}: unknown quality")
        accesses.add(name)
    landings: set[str] = set()
    landing_cells: dict[str, Cell] = {}
    for item in data.get("landing", []):
        keys("landing", item, {"place", "cell"}, frozenset({"place", "cell"}))
        name = item["place"]
        if name in landings or name not in accesses:
            raise ValueError(f"landing {name}: repeated or no access quality")
        landing_cells[name] = cell_value(item["cell"], f"landing {name}")
        landings.add(name)
    if landings != accesses:
        raise ValueError(f"access {sorted(accesses - landings)[0]}: missing landing")
    for name, cell in locations.items():
        on_land(cell, f"location {name}")
    seen: set[str] = set()
    for item in data.get("link", []):
        keys(
            "link",
            item,
            {"id", "road", "from", "to", "via", "category", "level", "note"},
            frozenset({"id", "from", "to", "category", "level"}),
        )
        name = item["id"]
        if name in seen:
            raise ValueError(f"link {name}: repeated id")
        if item.get("road") is not None and item["road"] not in roads:
            raise ValueError(f"link {name}: unknown road")
        if item.get("category") not in {"road", "path"}:
            raise ValueError(f"link {name}: unknown category")
        if type(item.get("level")) is not int or not 0 <= item["level"] <= _MAX_LEVEL:
            raise ValueError(f"link {name}: invalid level")
        resolved: list[tuple[str, str | None, Cell]] = []
        for end in (item["from"], item["to"]):
            if isinstance(end, str):
                if end not in locations:
                    raise ValueError(f"link {name}: unknown place {end}")
                resolved.append(("place", end, landing_cells.get(end, locations[end])))
            elif isinstance(end, dict):
                keys(f"link {name} parent", end, {"parent", "cell"}, frozenset({"parent", "cell"}))
                if end.get("parent") not in seen:
                    raise ValueError(f"link {name}: unknown or later parent {end.get('parent')}")
                resolved.append(
                    ("parent", None, cell_value(end.get("cell"), f"link {name} parent"))
                )
            else:
                raise ValueError(f"link {name}: invalid end")
        via = item.get("via", [])
        left, right = resolved
        same_node = left[2] == right[2] and (
            left[0] == "parent" or right[0] == "parent" or left[1] == right[1]
        )
        if same_node and via:
            raise ValueError(f"link {name}: same node cannot have a pin or through place")
        for waypoint in via:
            if isinstance(waypoint, str):
                if waypoint not in locations:
                    raise ValueError(f"link {name}: unknown place {waypoint}")
            elif isinstance(waypoint, dict):
                keys(
                    f"link {name} pin",
                    waypoint,
                    {"lat", "lon", "note"},
                    frozenset({"lat", "lon", "note"}),
                )
                if not isinstance(waypoint["note"], str) or not waypoint["note"].strip():
                    raise ValueError(f"link {name} pin: note is required")
                if any(
                    type(waypoint[key]) not in (int, float) or not math.isfinite(waypoint[key])
                    for key in ("lat", "lon")
                ):
                    raise ValueError(f"link {name} pin: invalid latitude or longitude")
                try:
                    pin = project_cell(waypoint["lat"], waypoint["lon"], origin)
                except ValueError as error:
                    raise ValueError(f"link {name} pin: {error}") from error
                on_land(pin, f"link {name} pin")
            else:
                raise ValueError(f"link {name}: invalid waypoint")
        seen.add(name)


def project_point(lat: float, lon: float, origin: Cell) -> Cell:
    """Project a geographic point to integer millionths of crop cells."""
    radius = 6_378_137.0
    half_world = math.pi * radius
    x = radius * math.radians(lon) + half_world
    y = half_world - radius * math.log(math.tan(math.pi / 4 + math.radians(lat) / 2))
    return (round((x / 839 - origin[0]) * 1_000_000), round((y / 839 - origin[1]) * 1_000_000))


def propose_courses(  # noqa: PLR0913, PLR0912  # Planning needs input layers, baselines and explicit recalculation.
    links: list[dict[str, Any]],
    baseline: dict[str, dict[str, Any]],
    land: Land,
    costs: Grid,
    *,
    ground_marks: dict[str, str] | None = None,
    recalculate: frozenset[str] = frozenset(),
    route_options: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Plan changed courses in parent order, retaining blocked descendants as evidence."""
    ground_marks = ground_marks or {}
    route_options = route_options or {}
    earlier: set[Cell] = set()
    candidates: dict[str, list[Cell]] = {}
    blocked: dict[str, str] = {}
    reasons: dict[str, list[str]] = {}
    effective: dict[str, list[Cell]] = {}
    for link in links:
        name: str = link["id"]
        points: list[Cell] = link["points"]
        for parent, join in link.get("joins", []):
            if parent in blocked:
                blocked[name] = f"waiting for blocked parent {parent}"
                break
            if parent not in effective:
                raise ValueError(f"link {name}: unknown or later parent {parent}")
            if join not in (points[0], points[-1]):
                raise ValueError(f"link {name}: join is not an endpoint")
            if join not in effective[parent]:
                nearest = sorted(
                    set(effective[parent]),
                    key=lambda c: ((c[0] - join[0]) ** 2 + (c[1] - join[1]) ** 2, c),
                )[:3]
                blocked[name] = f"parent {parent} does not reach {join}; nearest {nearest}"
                break
        if name in blocked:
            continue
        old = baseline.get(name)
        why = []
        if old is None:
            why.append("new link")
        else:
            if [tuple(point) for point in old.get("points", [])] != points:
                why.append("ends, through places, pins or join changed")
            if not legal_course(land, [tuple(cell) for cell in old["course"]]):
                why.append("approved course is no longer legal")
            if name in ground_marks and ground_marks[name] != old.get("ground_mark"):
                why.append("ground or river crossings changed")
        if name in recalculate:
            why.append("recalculation requested")
        changed = bool(why)
        if changed:
            try:
                course = route(land, costs, points, earlier=earlier, **route_options)
            except ValueError as error:
                raise ValueError(f"link {name}: {error}") from error
            candidates[name] = course
            reasons[name] = why
        else:
            assert old is not None
            course = [tuple(cell) for cell in old["course"]]
        effective[name] = course
        earlier.update(course)
        earlier.update(((a[0] + b[0]) // 2, (a[1] + b[1]) // 2) for a, b in pairwise(course))
    return {
        "candidates": candidates,
        "blocked": blocked,
        "reasons": reasons,
        "complete": not blocked,
    }


_WATER_TOP = (
    "river_width_mm",
    "terrain_sha256",
    "rivers_sha256",
    "sea_distance",
    "sea_distance_sha256",
    "next_landing_id",
)


def _by_id(item: dict[str, Any]) -> tuple[Any, ...]:
    return (item["id"],)


_TABLE_ORDER: dict[str, Callable[[dict[str, Any]], tuple[Any, ...]]] = {
    "join": lambda item: (item["link"], item["end"]),
    "access": lambda item: (item["place"],),
    "river_break": lambda item: (item["name"],),
    "mark": lambda item: (item["place"],),
    "water_join": lambda item: (
        item["reach"],
        item.get("to_reach", -1),
        tuple(item.get("cell", ())),
    ),
    "strait": lambda item: (item["from"], item["to"]),
}


def dump_roads(graph: dict[str, Any]) -> str:
    """Serialize the network in stable table and field order."""
    table_fields = {
        "node": ("id", "kind", "cell", "place"),
        "stretch": (
            "id",
            "from",
            "to",
            "cells",
            "length_m",
            "category",
            "level",
            "tidal",
            "crossings",
            "wades",
            "landing_from",
            "landing_to",
        ),
        "link": ("id", "road", "from_node", "to_node", "stretches", "record"),
        "join": ("link", "end", "parent", "node", "along_m"),
        "road": ("id", "name", "links"),
        "access": ("place", "quality", "landing"),
        "water_class": ("id", "reach_m", "sea_going", "bodies"),
        "river_break": ("name", "stretch", "cell", "above", "below"),
        "mark": ("place",),
        "joint": ("id", "stretch", "cell"),
        "reach": ("id", "stretch", "from", "to", "usable", "length_mm", "kind", "begin", "end"),
        "water_join": ("reach", "to_reach", "cell", "classes"),
        "landing": (
            "id",
            "node",
            "bank",
            "survey",
            "shore",
            "run",
            "cell",
            "joint",
            "crossing",
            "end",
            "approach_m",
            "bodies",
        ),
        "strait": ("place", "from", "to", "length_m", "classes"),
    }
    top_fields = (
        "format",
        "hand_list_sha256",
        "baseline_sha256",
        "next_node_id",
        "next_stretch_id",
        "cell_side_mm",
    )
    lines = ["# Generated by the network tool; do not edit by hand."]

    def toml_value(value: Any) -> str:  # noqa: ANN401  # Serialized graph values are heterogeneous.
        if isinstance(value, dict):
            return (
                "{ "
                + ", ".join(f"{key} = {toml_value(item)}" for key, item in value.items())
                + " }"
            )
        if isinstance(value, list):
            return "[" + ", ".join(toml_value(item) for item in value) + "]"
        return json.dumps(value, ensure_ascii=True)

    for field in (*top_fields, *_WATER_TOP):
        if field in graph:
            lines.append(f"{field} = {json.dumps(graph[field], ensure_ascii=True)}")
    for table, fields in table_fields.items():
        entries = graph.get(table, [])
        if table != "water_class":
            entries = sorted(entries, key=_TABLE_ORDER.get(table, _by_id))
        for entry in entries:
            lines.extend(("", f"[[{table}]]"))
            for field in fields:
                if field in entry:
                    lines.append(f"{field} = {toml_value(entry[field])}")
    return "\n".join(lines) + "\n"


def make_graph(  # noqa: PLR0913, PLR0915, PLR0912  # Graph construction binds nodes, paths and prior identities.
    places: dict[str, Cell],
    access: dict[str, tuple[str, Cell]],
    links: list[dict[str, Any]],
    courses: dict[str, list[Cell]],
    roads: list[dict[str, str]],
    origin: Cell,
    hand_hash: str,
    baseline_hash: str,
    *,
    previous: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Materialize nodes, shared stretches, links and joins from complete courses."""
    previous = previous or {}
    if set(courses) != {link["id"] for link in links} or len(courses) != len(links):
        raise ValueError("link and course IDs differ or repeat")
    for link in links:
        course = courses[link["id"]]
        if not course:
            raise ValueError(f"link {link['id']}: empty course")
        if len(course) > 1 and course[0] == course[-1]:
            raise ValueError(f"link {link['id']}: same node cannot have a nonempty course")
        for end, actual in (("from", course[0]), ("to", course[-1])):
            ref = link[end]
            expected = (
                (access[ref][1] if ref in access else places[ref])
                if isinstance(ref, str)
                else tuple(ref["cell"])
            )
            if actual != expected:
                raise ValueError(f"link {link['id']}: {end} endpoint differs from its course")
    named_land: dict[Cell, tuple[str, str]] = {}
    entries: list[dict[str, Any]] = []
    for place, cell in sorted(places.items()):
        entries.append({"kind": "place", "place": place, "cell": list(cell)})
        if place in access:
            landing = access[place][1]
            entries.append({"kind": "landing", "place": place, "cell": list(landing)})
            key = ("landing", place)
            ground_cell = landing
        else:
            key = ("place", place)
            ground_cell = cell
        if ground_cell in named_land:
            raise ValueError(f"two land nodes occupy {ground_cell}")
        named_land[ground_cell] = key
    lines, link_paths = split_courses(courses, set(named_land))
    endpoints = {cell for line in lines for cell in (line[0], line[-1])}
    for cell in sorted(endpoints - set(named_land)):
        entries.append({"kind": "junction", "cell": list(cell)})

    def node_key(entry: dict[str, Any]) -> tuple[Any, ...]:
        return (entry["kind"], entry.get("place", ""), tuple(entry["cell"]))

    old_nodes = {node_key(node): node["id"] for node in previous.get("node", [])}
    node_ids, next_node = allocate_ids(
        [node_key(entry) for entry in entries],
        old_nodes,
        previous.get("next_node_id", 1),
        "next_node_id",
    )
    for entry in entries:
        entry["id"] = node_ids[node_key(entry)]
    by_place = {
        (entry["kind"], entry.get("place")): entry["id"] for entry in entries if "place" in entry
    }
    ground_nodes = {cell: by_place[key] for cell, key in named_land.items()}
    ground_nodes.update(
        {tuple(entry["cell"]): entry["id"] for entry in entries if entry["kind"] == "junction"}
    )
    old_node_by_id = {node["id"]: node for node in previous.get("node", [])}

    def stretch_key(entry: dict[str, Any]) -> tuple[Any, ...]:
        cells = tuple(tuple(cell) for cell in entry["cells"])
        if entry["tidal"]:
            return ("tidal", old_node_by_id[entry["to"]]["place"], cells)
        if cells <= cells[::-1]:
            return ("land", cells, entry["from"], entry["to"])
        return ("land", cells[::-1], entry["to"], entry["from"])

    old_stretches = {stretch_key(stretch): stretch["id"] for stretch in previous.get("stretch", [])}
    stretch_entries: list[dict[str, Any]] = []
    stretch_keys: list[tuple[Any, ...]] = []
    users: dict[int, list[tuple[str, int]]] = {i: [] for i in range(len(lines))}
    for link in links:
        for index, _ in link_paths[link["id"]]:
            users[index].append((link["category"], link["level"]))
    for index, line in enumerate(lines):
        category, level = starting_state(users[index])
        stretch_keys.append(("land", line, ground_nodes[line[0]], ground_nodes[line[-1]]))
        stretch_entries.append(
            {
                "from": ground_nodes[line[0]],
                "to": ground_nodes[line[-1]],
                "cells": [list(c) for c in line],
                "length_m": ground_length(list(line), origin),
                "category": category,
                "level": level,
                "tidal": False,
            }
        )
    for place, (quality, landing) in sorted(access.items()):
        if quality != "offshore_at_high_tide":
            continue
        tidal_line = [landing] if landing == places[place] else [landing, places[place]]
        stretch_keys.append(("tidal", place, tuple(tidal_line)))
        stretch_entries.append(
            {
                "from": by_place[("landing", place)],
                "to": by_place[("place", place)],
                "cells": [list(c) for c in tidal_line],
                "length_m": ground_length(tidal_line, origin),
                "category": "path",
                "level": 0,
                "tidal": True,
            }
        )
    ids, next_stretch = allocate_ids(
        stretch_keys, old_stretches, previous.get("next_stretch_id", 1), "next_stretch_id"
    )
    for key, entry in zip(stretch_keys, stretch_entries, strict=True):
        entry["id"] = ids[key]
    graph_links = []
    joins = []
    links_by_id = {item["id"]: item for item in links}
    stretch_by_id = {entry["id"]: entry for entry in stretch_entries}
    for link in links:
        name = link["id"]
        course = courses[name]
        entry = {
            "id": name,
            "from_node": ground_nodes[course[0]],
            "to_node": ground_nodes[course[-1]],
            "stretches": [
                [
                    ids[("land", lines[i], ground_nodes[lines[i][0]], ground_nodes[lines[i][-1]])],
                    direction,
                ]
                for i, direction in link_paths[name]
            ],
            "record": link["record"],
        }
        if "road" in link:
            entry["road"] = link["road"]
        graph_links.append(entry)
    graph_by_id = {entry["id"]: entry for entry in graph_links}
    for link in links:
        for end in ("from", "to"):
            target = link[end]
            if not isinstance(target, dict):
                continue
            parent = target["parent"]
            if parent not in links_by_id:
                raise ValueError(f"link {link['id']}: unknown parent {parent}")
            node = ground_nodes[tuple(target["cell"])]
            parent_link = graph_by_id[parent]
            current = parent_link["from_node"]
            distance = 0
            for stretch_id, direction in parent_link["stretches"]:
                if current == node:
                    break
                stretch = stretch_by_id[stretch_id]
                distance += stretch["length_m"]
                current = stretch["to"] if direction == "forward" else stretch["from"]
            if current != node:
                raise ValueError(f"link {link['id']}: join absent from parent {parent}")
            joins.append(
                {
                    "link": link["id"],
                    "end": end,
                    "parent": parent,
                    "node": node,
                    "along_m": distance,
                }
            )
    return {
        "format": 1,
        "hand_list_sha256": hand_hash,
        "baseline_sha256": baseline_hash,
        "next_node_id": next_node,
        "next_stretch_id": next_stretch,
        "node": entries,
        "stretch": stretch_entries,
        "link": graph_links,
        "join": joins,
        "road": [
            {**road, "links": [link["id"] for link in links if link.get("road") == road["id"]]}
            for road in roads
        ],
        "access": [
            {"place": place, "quality": quality, "landing": by_place[("landing", place)]}
            for place, (quality, _) in sorted(access.items())
        ],
    }


def resolve_links(
    data: dict[str, Any], locations: dict[str, Cell], origin: Cell
) -> list[dict[str, Any]]:
    """Resolve land-side ends, ordered waypoints and declared parent joins."""
    landings = {item["place"]: tuple(item["cell"]) for item in data.get("landing", [])}

    def resolve(ref: Any) -> Cell:  # noqa: ANN401  # Hand-list ends have tagged string and table forms.
        if isinstance(ref, str):
            return landings.get(ref, locations[ref])
        if "parent" in ref:
            return tuple(ref["cell"])
        return project_cell(ref["lat"], ref["lon"], origin)

    result = []
    for link in data.get("link", []):
        refs = [link["from"], *link.get("via", []), link["to"]]
        joins = [
            (ref["parent"], resolve(ref))
            for ref in (link["from"], link["to"])
            if isinstance(ref, dict)
        ]
        join_ends = [
            (end, ref["parent"], resolve(ref))
            for end in ("from", "to")
            if isinstance((ref := link[end]), dict)
        ]
        result.append(
            {
                "id": link["id"],
                "points": [resolve(ref) for ref in refs],
                "joins": joins,
                "join_ends": join_ends,
            }
        )
    return result


def read_locations(path: Path, origin: Cell) -> dict[str, Cell]:
    """Project each scenario location once, retaining its scenario ID."""
    locations: dict[str, Cell] = {}
    for item in tomllib.loads(path.read_text(encoding="utf-8"))["location"]:
        name = item["id"]
        if name in locations:
            raise ValueError(f"location {name}: repeated id")
        try:
            locations[name] = project_cell(item["lat"], item["lon"], origin)
        except (ValueError, TypeError, OverflowError) as error:
            raise ValueError(f"location {name}: invalid coordinates: {error}") from error
    return locations


def import_seed(
    links: list[dict[str, Any]],
    seed: dict[str, list[Cell]],
    land: Land,
    *,
    approved_points: dict[str, list[Cell]] | None = None,
) -> tuple[dict[str, dict[str, Any]], list[dict[str, Any]]]:
    """Make the approved baseline, completing only seed endpoints beside parents."""
    if set(seed) != {link["id"] for link in links}:
        raise ValueError("seed and hand-list link IDs differ")
    approved_points = approved_points or {}
    baseline: dict[str, dict[str, Any]] = {}
    completions: list[dict[str, Any]] = []
    for link in links:
        name = link["id"]
        course = list(seed[name])
        joins = link.get(
            "join_ends", [("to", parent, cell) for parent, cell in link.get("joins", [])]
        )
        points = list(approved_points.get(name, link["points"]))
        for end, parent, _ in joins:
            if parent not in baseline:
                raise ValueError(f"link {name}: unknown or later seed parent {parent}")
            reverse = end == "from"
            work = course[::-1] if reverse else course
            original = work[-1]
            try:
                work = complete_join(land, baseline[parent]["course"], work)
            except ValueError as error:
                raise ValueError(f"link {name}: {error}") from error
            if work[-1] != original:
                completions.append({"link": name, "from": original, "added": work[-1]})
            course = work[::-1] if reverse else work
            points[0 if reverse else -1] = work[-1]
            if len(course) == 1 and original != work[-1]:
                points[0] = points[-1] = work[-1]
        if not course or any(
            (abs(b[0] - a[0]), abs(b[1] - a[1])) not in _DISTANCE for a, b in pairwise(course)
        ):
            raise ValueError(f"link {name}: invalid approved course geometry")
        baseline[name] = {
            "course": course,
            "points": points,
            "joins": [
                {"end": end, "parent": parent, "cell": points[0 if end == "from" else -1]}
                for end, parent, _ in joins
            ],
            "record": "approved as drawn (r20 and r21)",
        }
    return baseline, completions


def seed_points(
    hand_links: list[dict[str, Any]],
    resolved: list[dict[str, Any]],
    seed_links: dict[str, dict[str, Any]],
) -> dict[str, list[Cell]]:
    """Keep the places and pins the approved seed was drawn through, excluding later pins."""
    result = {}
    for hand, route in zip(hand_links, resolved, strict=True):
        name = hand["id"]
        if route["id"] != name:
            raise ValueError("hand list and resolved links differ in order")
        approved = {tuple(cell) for cell in seed_links[name].get("pins", [])}
        points: list[Cell] = route["points"]
        selected = [points[0]]
        for item, cell in zip(hand.get("via", []), points[1:-1], strict=True):
            if isinstance(item, str) or cell in approved:
                selected.append(cell)
        result[name] = [*selected, points[-1]]
    return result


class RiverIndex:
    """Spatial index of projected river segments and interpolated widths."""

    def __init__(self, stretches: list[dict[str, Any]], origin: Cell) -> None:
        self.segments: list[tuple[Any, ...]] = []
        self.bins: dict[Cell, list[int]] = {}
        for stretch in stretches:
            points = [project_point(lat, lon, origin) for lat, lon in stretch["points"]]
            lengths = [
                math.isqrt((b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2) for a, b in pairwise(points)
            ]
            total = sum(lengths)
            before = 0
            for (a, b), length in zip(pairwise(points), lengths, strict=True):
                if length:
                    index = len(self.segments)
                    self.segments.append(
                        (
                            a,
                            b,
                            stretch["river"],
                            before,
                            length,
                            total,
                            Fraction(str(stretch["width_m"][0])),
                            Fraction(str(stretch["width_m"][1])),
                            stretch.get("id", stretch["river"]),
                        )
                    )
                    x0, x1 = sorted((a[0] // 1_000_000, b[0] // 1_000_000))
                    y0, y1 = sorted((a[1] // 1_000_000, b[1] // 1_000_000))
                    for x in range(x0, x1 + 1):
                        for y in range(y0, y1 + 1):
                            self.bins.setdefault((x, y), []).append(index)
                before += length

    def crossings(self, start: Cell, end: Cell) -> list[tuple[str, Fraction, Fraction]]:
        """Find distinct intersections in order along a road step."""
        return sorted(self.scan(start, end), key=lambda item: (item[1], item[0], item[2]))

    def scan(
        self, start: Cell, end: Cell
    ) -> dict[tuple[str, Fraction, Fraction], tuple[str, Fraction]]:
        """Each distinct intersection with the surveyed stretch and position it was met at."""
        a = (start[0] * 1_000_000 + 500_000, start[1] * 1_000_000 + 500_000)
        b = (end[0] * 1_000_000 + 500_000, end[1] * 1_000_000 + 500_000)
        road = (b[0] - a[0], b[1] - a[1])

        def cross(left: Cell, right: Cell) -> int:
            return left[0] * right[1] - left[1] * right[0]

        indices: set[int] = set()
        for x in range(min(start[0], end[0]), max(start[0], end[0]) + 1):
            for y in range(min(start[1], end[1]), max(start[1], end[1]) + 1):
                indices.update(self.bins.get((x, y), ()))
        found: dict[tuple[str, Fraction, Fraction], tuple[str, Fraction]] = {}
        for index in indices:
            p, q, river, before, length, total, w0, w1, stretch = self.segments[index]
            direction = (q[0] - p[0], q[1] - p[1])
            offset = (p[0] - a[0], p[1] - a[1])
            divisor = cross(road, direction)
            if not divisor:
                continue
            t = Fraction(cross(offset, direction), divisor)
            u = Fraction(cross(offset, road), divisor)
            if 0 <= t <= 1 and 0 <= u <= 1:
                width = w0 + (w1 - w0) * (before + u * length) / total
                place = (stretch, before + u * length)
                key = (river, t, width)
                if key not in found or place < found[key]:
                    found[key] = place
        return found


_ROADS_FORMAT = 4
_CUT_FORMATS = (2, 3, 4)
_HIT_GROUP_DISTANCE_M = 500
_CROSSING_MATCH_CELLS = 3


def group_river_hits(
    cells: list[Cell], index: RiverIndex, origin: Cell
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Group distinct hits per river, preserving their step and distance along the line."""
    hits: dict[tuple[str, Fraction], dict[str, Any]] = {}
    before = 0
    for step, (start, end) in enumerate(pairwise(cells)):
        length = ground_length([start, end], origin)
        for river, t, width in index.crossings(start, end):
            distance = Fraction(before) + t * length
            key = (river, distance)
            if key in hits:
                hits[key]["width"] = max(hits[key]["width"], width)
            else:
                hits[key] = {
                    "river": river,
                    "distance": distance,
                    "width": width,
                    "step": max(0, step - 1) if t == 0 else step,
                    "t": t,
                    "point": (
                        Fraction(start[0]) + t * (end[0] - start[0]),
                        Fraction(start[1]) + t * (end[1] - start[1]),
                    ),
                }
        before += length
    by_river: dict[str, list[dict[str, Any]]] = {}
    for hit in hits.values():
        by_river.setdefault(hit["river"], []).append(hit)
    crossings, touches = [], []
    for hits_for_river in by_river.values():
        river_hits = hits_for_river.copy()
        river_hits.sort(key=lambda hit: hit["distance"])
        while river_hits:
            first = river_hits[0]["distance"]
            count = next(
                (
                    i
                    for i, hit in enumerate(river_hits)
                    if hit["distance"] - first >= _HIT_GROUP_DISTANCE_M
                ),
                len(river_hits),
            )
            group, river_hits = river_hits[:count], river_hits[count:]
            item = {
                "river": group[0]["river"],
                "distance": first,
                "width": max(hit["width"] for hit in group),
                "first_step": group[0]["step"],
                "last_step": group[-1]["step"],
                "t": group[0]["t"],
                "point": group[0]["point"],
                "members": group,
            }
            (crossings if count % 2 else touches).append(item)

    def key(hit: dict[str, Any]) -> tuple[Any, Any]:
        return hit["distance"], hit["river"]

    return sorted(crossings, key=key), sorted(touches, key=key)


def match_crossings(
    crossings: list[dict[str, Any]], entries: list[dict[str, Any]], rivers: set[str]
) -> list[str]:
    """Require a unique, reciprocal hand-list match for every wide crossing."""
    matches: list[list[int]] = []
    for number, entry in enumerate(entries):
        if entry["river"] not in rivers:
            raise ValueError(f"crossing entry {number}: unknown river {entry['river']}")
        if entry["kind"] not in {"ford", "none"}:
            raise ValueError(f"crossing entry {number}: invalid kind {entry['kind']}")
        at = entry["at"]
        found = [
            i
            for i, hit in enumerate(crossings)
            if hit["river"] == entry["river"]
            and max(abs(at[0] - hit["cell"][0]), abs(at[1] - hit["cell"][1]))
            <= _CROSSING_MATCH_CELLS
        ]
        if not found:
            raise ValueError(f"crossing entry {number}: no wide crossing: {entry}")
        if len(found) > 1:
            raise ValueError(
                f"crossing entry {number}: two crossings: {[crossings[i] for i in found]}"
            )
        matches.append(found)
    result = []
    for i, hit in enumerate(crossings):
        found = [j for j, match in enumerate(matches) if i in match]
        if not found:
            raise ValueError(f"wide crossing has no entry: {hit}")
        if len(found) > 1:
            raise ValueError(f"wide crossing has two entries: {[entries[j] for j in found]}")
        result.append(entries[found[0]]["kind"])
    return result


def uncut_previous(previous: dict[str, Any]) -> dict[str, Any]:
    """Recover course stretches from a format-two graph before assigning course IDs."""
    graph = {**previous, "stretch": [dict(s) for s in previous.get("stretch", [])]}
    banks = {node["id"] for node in previous.get("node", []) if node["kind"] == "bank"}
    graph["node"] = [node for node in previous.get("node", []) if node["id"] not in banks]
    for bank in sorted(banks):
        before = [s for s in graph["stretch"] if s["to"] == bank]
        after = [s for s in graph["stretch"] if s["from"] == bank]
        if len(before) != 1 or len(after) != 1:
            raise ValueError(f"bank {bank}: cannot recover course stretch")
        left, right = before[0], after[0]
        combined = {
            **left,
            "to": right["to"],
            "cells": left["cells"] + right["cells"][1:],
            "length_m": left["length_m"] + right["length_m"],
        }
        combined.pop("crossings", None)
        combined.pop("wades", None)
        graph["stretch"].remove(left)
        graph["stretch"].remove(right)
        graph["stretch"].append(combined)
    return graph


def apply_crossings(  # noqa: PLR0912, PLR0913, PLR0915  # Graph cutting handles all crossing boundaries together.
    graph: dict[str, Any],
    index: RiverIndex,
    entries: list[dict[str, Any]],
    origin: Cell,
    rivers: set[str],
    *,
    previous: dict[str, Any] | None = None,
) -> dict[int, list[dict[str, Any]]]:
    """Cut spans and return their assigned hits for the report."""
    nodes = {tuple(node["cell"]): node for node in graph["node"] if node["kind"] != "bank"}
    previous = previous or {}
    old_banks = {
        tuple(node["cell"]): node["id"]
        for node in previous.get("node", [])
        if node["kind"] == "bank"
    }
    old_pieces = {
        tuple(tuple(cell) for cell in stretch["cells"]): stretch["id"]
        for stretch in previous.get("stretch", [])
        if not stretch["tidal"]
    }
    stretches = graph["stretch"]
    wide: list[dict[str, Any]] = []
    per_stretch: dict[int, list[dict[str, Any]]] = {}
    for stretch in stretches:
        if stretch["tidal"]:
            continue
        cells = [tuple(cell) for cell in stretch["cells"]]
        for start, end in pairwise(cells):
            for river, t, width in index.crossings(start, end):
                cell = start if t == 0 else end if t == 1 else None
                if cell in nodes:
                    node = nodes[cell]
                    incident = [s["id"] for s in stretches if node["id"] in (s["from"], s["to"])]
                    raise ValueError(
                        f"river {river} width {float(width):.1f} at node {node['id']} cell {cell}, stretches {incident}"
                    )
        crossings, _touches = group_river_hits(cells, index, origin)
        per_stretch[stretch["id"]] = crossings
        for crossing in crossings:
            if crossing["width"] >= _MIN_RIVER_WIDTH_M:
                x, y = crossing["point"]
                crossing.update(
                    stretch=stretch["id"],
                    cell=(math.floor(x + Fraction(1, 2)), math.floor(y + Fraction(1, 2))),
                )
                wide.append(crossing)
    for crossing, kind in zip(wide, match_crossings(wide, entries, rivers), strict=True):
        crossing["kind"] = kind
    replacements: dict[int, list[int]] = {}
    output = []
    report_hits: dict[int, list[dict[str, Any]]] = {}
    for stretch in stretches:
        crossings = per_stretch.get(stretch["id"], [])
        spans: list[tuple[int, int, list[dict[str, Any]]]] = []
        for hit in (hit for hit in crossings if "kind" in hit):
            start, end = hit["first_step"], hit["last_step"] + 1
            if spans and start < spans[-1][1]:
                old_start, old_end, members = spans[-1]
                spans[-1] = (old_start, max(old_end, end), [*members, hit])
            else:
                spans.append((start, end, [hit]))
        cells = stretch["cells"]

        def wades(
            start: int,
            end: int,
            cells: list[list[int]] = cells,
            crossings: list[dict[str, Any]] = crossings,
        ) -> list[dict[str, Any]]:
            result = []
            for hit in crossings:
                step = hit["first_step"]
                if "kind" in hit or not start <= step < end:
                    continue
                a, b = tuple(cells[step]), tuple(cells[step + 1])
                axis = 0 if a[0] != b[0] else 1
                fraction = (hit["point"][axis] - a[axis]) / (b[axis] - a[axis])
                before = ground_length([tuple(c) for c in cells[start : step + 1]], origin)
                distance = round(before + fraction * ground_length([a, b], origin))
                length = ground_length([tuple(c) for c in cells[start : end + 1]], origin)
                result.append(
                    {
                        "river": hit["river"],
                        "width_tenths_m": min(199, round(hit["width"] * 10)),
                        "along_m": min(distance, length),
                    }
                )
            return result

        if not spans:
            stretch["wades"] = wades(0, len(cells) - 1)
            output.append(stretch)
            replacements[stretch["id"]] = [stretch["id"]]
            continue
        boundaries = sorted(
            {0, len(cells) - 1, *(n for start, end, _ in spans for n in (start, end))}
        )
        pieces = []
        for start, end in pairwise(boundaries):

            def node_at(
                offset: int, cells: list[list[int]] = cells, stretch: dict[str, Any] = stretch
            ) -> int:
                if offset == 0:
                    return stretch["from"]
                if offset == len(cells) - 1:
                    return stretch["to"]
                cell = tuple(cells[offset])
                if cell not in nodes:
                    node_id = old_banks.get(cell)
                    if node_id is None:
                        node_id = graph["next_node_id"]
                        graph["next_node_id"] = next_counter("next_node_id", node_id)
                    node = {"id": node_id, "kind": "bank", "cell": list(cell)}
                    graph["node"].append(node)
                    nodes[cell] = node
                return nodes[cell]["id"]

            sid = (
                stretch["id"]
                if start == 0
                else old_pieces.get(tuple(tuple(cell) for cell in cells[start : end + 1]))
            )
            if sid is None:
                sid = graph["next_stretch_id"]
                graph["next_stretch_id"] = next_counter("next_stretch_id", sid)
            part = {
                **stretch,
                "id": sid,
                "from": node_at(start),
                "to": node_at(end),
                "cells": cells[start : end + 1],
                "length_m": ground_length([tuple(c) for c in cells[start : end + 1]], origin),
            }
            part["wades"] = wades(start, end)
            for a, b, members in spans:
                if (start, end) == (a, b):
                    report_hits[sid] = members
                    for member in members:
                        member["span_start"] = start
                    part["crossings"] = [
                        {
                            "river": h["river"],
                            "width_tenths_m": round(h["width"] * 10),
                            "kind": h["kind"],
                        }
                        for h in members
                    ]
            output.append(part)
            pieces.append(sid)
        replacements[stretch["id"]] = pieces
    graph["stretch"] = output
    for link in graph["link"]:
        link["stretches"] = [
            [sid, direction]
            for old, direction in link["stretches"]
            for sid in (
                replacements[old] if direction == "forward" else reversed(replacements[old])
            )
        ]
    stretches_by_id = {stretch["id"]: stretch for stretch in output}
    links_by_id = {link["id"]: link for link in graph["link"]}
    for join in graph.get("join", []):
        parent = links_by_id[join["parent"]]
        current = parent["from_node"]
        distance = 0
        for sid, direction in parent["stretches"]:
            if current == join["node"]:
                break
            stretch = stretches_by_id[sid]
            distance += stretch["length_m"]
            current = stretch["to"] if direction == "forward" else stretch["from"]
        join["along_m"] = distance

    return report_hits


def crossing_cost(index: RiverIndex, base: str, per_m: str) -> Callable[[Cell, Cell], int]:
    """Make the fixed-point mock-up charge for the widest river on a step."""
    fixed = Fraction(base)
    width_cost = Fraction(per_m)
    if fixed < 0 or width_cost < 0:
        raise ValueError("river costs must be nonnegative")

    @lru_cache(maxsize=200_000)
    def cost(start: Cell, end: Cell) -> int:
        crossings = index.crossings(start, end)
        if not crossings:
            return 0
        width = max(item[2] for item in crossings)
        amount = (fixed + width_cost * width) * 2_000_000_000_000
        return (amount.numerator * 2 + amount.denominator) // (2 * amount.denominator)

    return cost


def nonconnecting_crossings(
    stretches: list[dict[str, Any]],
) -> list[tuple[int, int, Fraction, Fraction]]:
    """Find proper crossings between different lines that share no cell at the crossing."""
    bins: dict[Cell, list[int]] = {}
    segments: list[tuple[int, Cell, Cell]] = []
    result: set[tuple[int, int, Fraction, Fraction]] = set()

    def cross(a: Cell, b: Cell) -> int:
        return a[0] * b[1] - a[1] * b[0]

    for stretch in stretches:
        if stretch["tidal"]:
            continue
        sid = stretch["id"]
        for first, second in pairwise(stretch["cells"]):
            a, b = tuple(first), tuple(second)
            candidates: set[int] = set()
            for x in range(min(a[0], b[0]), max(a[0], b[0]) + 1):
                for y in range(min(a[1], b[1]), max(a[1], b[1]) + 1):
                    candidates.update(bins.get((x, y), ()))
            road = (b[0] - a[0], b[1] - a[1])
            for index in candidates:
                other, p, q = segments[index]
                if sid == other or {a, b} & {p, q}:
                    continue
                direction = (q[0] - p[0], q[1] - p[1])
                offset = (p[0] - a[0], p[1] - a[1])
                denominator = cross(road, direction)
                if not denominator:
                    continue
                t, u = (
                    Fraction(cross(offset, direction), denominator),
                    Fraction(cross(offset, road), denominator),
                )
                if 0 <= t <= 1 and 0 <= u <= 1:
                    result.add(
                        (min(sid, other), max(sid, other), a[0] + t * road[0], a[1] + t * road[1])
                    )
            index = len(segments)
            segments.append((sid, a, b))
            for x in range(min(a[0], b[0]), max(a[0], b[0]) + 1):
                for y in range(min(a[1], b[1]), max(a[1], b[1]) + 1):
                    bins.setdefault((x, y), []).append(index)
    return sorted(result)


def network_report(  # noqa: PLR0913, PLR0915, PLR0912  # A report brings together graph and crossing evidence.
    graph: dict[str, Any],
    links: list[dict[str, Any]],
    completions: list[dict[str, Any]],
    river_index: RiverIndex,
    origin: Cell,
    *,
    proposed: bool = False,
    course_graph: dict[str, Any] | None = None,
    crossing_entries: list[dict[str, Any]] | None = None,
    crossing_hits: dict[int, list[dict[str, Any]]] | None = None,
    water: dict[str, Any] | None = None,
) -> str:
    """Report pieces, joins, shared state, geometry and river crossings of a graph."""
    nodes = {node["id"]: node for node in graph["node"]}
    accesses = {item["place"]: item for item in graph.get("access", [])}
    adjacency: dict[int, set[int]] = {i: set() for i in nodes}
    for stretch in graph["stretch"]:
        if not stretch["tidal"] and all(c["kind"] == "ford" for c in stretch.get("crossings", [])):
            adjacency[stretch["from"]].add(stretch["to"])
            adjacency[stretch["to"]].add(stretch["from"])
    eligible = {
        i
        for i, node in nodes.items()
        if node["kind"] != "place" or node.get("place") not in accesses
    }
    lines = ["# " + ("Proposed travel network" if proposed else "Travel network"), "", "## Places"]
    for node in sorted(
        (n for n in nodes.values() if n["kind"] == "place"), key=lambda n: n["place"]
    ):
        name = node["place"]
        suffix = (
            f"; {accesses[name]['quality']}, landing {accesses[name]['landing']}"
            if name in accesses
            else ""
        )
        lines.append(f"- {name}: node {node['id']}{suffix}")
    lines.extend(("", "## Pieces of land"))
    pending = set(eligible)
    components = []
    while pending:
        start = min(pending)
        component = {start}
        stack = [start]
        pending.remove(start)
        while stack:
            for other in sorted(adjacency[stack.pop()] & pending):
                pending.remove(other)
                component.add(other)
                stack.append(other)
        components.append(component)
        names = sorted(nodes[i]["place"] for i in component if "place" in nodes[i])
        lines.append(f"- {len(component)} nodes: {', '.join(names) if names else '(no places)'}")
    largest = max(components, key=len, default=set())
    unreachable = sorted(
        nodes[i]["place"] for i in eligible - largest if nodes[i]["kind"] == "place"
    )
    lines.extend(
        ("", "## Places outside the largest land piece", *(f"- {name}" for name in unreachable))
    )
    if not unreachable:
        lines.append("- none")
    lines.extend(("", "## Joins"))
    for join in sorted(graph.get("join", []), key=lambda j: (j["link"], j["end"])):
        lines.append(
            f"- {join['link']} {join['end']} on {join['parent']}: node {join['node']}, {join['along_m']} m"
        )
    lines.extend(("", "## Seed completions"))
    try:
        metadata = [json.loads(link["record"]) for link in graph["link"]]
        if all("starting_state" in item for item in metadata):
            completions = sorted(
                (item["seed_completion"] for item in metadata if "seed_completion" in item),
                key=lambda item: item["link"],
            )
    except (json.JSONDecodeError, TypeError):
        pass
    for completion in completions:
        suffix = " (superseded join)" if completion.get("superseded") else ""
        lines.append(
            f"- {completion['link']}: {completion['from']} to {completion['added']}{suffix}"
        )
    lines.extend(
        (
            "",
            "## Totals",
            f"- {len(graph['link'])} links, {len(graph['stretch'])} stretches, {len(nodes)} nodes",
        )
    )
    totals: dict[tuple[str, int], int] = {}
    for stretch in graph["stretch"]:
        key = (stretch["category"], stretch["level"])
        totals[key] = totals.get(key, 0) + stretch["length_m"]
    for (kind, level), metres in sorted(totals.items()):
        lines.append(f"- {kind} level {level}: {metres / 1000:.3f} km")
    states = {link["id"]: (link["category"], link["level"]) for link in links}
    for link in graph["link"]:
        try:
            state = json.loads(link["record"]).get("starting_state")
        except (json.JSONDecodeError, AttributeError):
            state = None
        if state is not None:
            states[link["id"]] = tuple(state)
    used: dict[int, set[tuple[str, int]]] = {}
    for link in graph["link"]:
        for sid, _direction in link["stretches"]:
            used.setdefault(sid, set()).add(states[link["id"]])
    conflicts = sorted(sid for sid, values in used.items() if len(values) > 1)
    lines.append(f"- Starting-state disagreements: {', '.join(map(str, conflicts)) or 'none'}")
    lines.extend(("", "## Lines crossing without a shared cell"))
    crossings = nonconnecting_crossings(graph["stretch"])
    for a, b, x, y in crossings:
        lines.append(f"- stretches {a} and {b}: ({float(x):.3f}, {float(y):.3f})")
    lines.extend(("", "## Wide crossings"))
    for stretch in sorted(graph["stretch"], key=lambda item: item["id"]):
        if not stretch.get("crossings"):
            continue
        hits = (crossing_hits or {})[stretch["id"]]
        for crossing, hit in zip(stretch["crossings"], hits, strict=True):
            x, y = hit["point"]
            cell = (math.floor(x + Fraction(1, 2)), math.floor(y + Fraction(1, 2)))
            entry = next(
                entry
                for entry in crossing_entries or []
                if entry["river"] == crossing["river"]
                and max(abs(entry["at"][0] - cell[0]), abs(entry["at"][1] - cell[1]))
                <= _CROSSING_MATCH_CELLS
            )
            lines.append(
                f"- stretch {stretch['id']}: {crossing['river']}, {crossing['width_tenths_m'] / 10:.1f} m, {crossing['kind']}; {entry['note']}"
            )
    lines.extend(("", "## Touches"))
    for stretch in (course_graph or graph)["stretch"]:
        if stretch["tidal"]:
            continue
        cells = [tuple(c) for c in stretch["cells"]]
        _, touches = group_river_hits(cells, river_index, origin)
        for hit in touches:
            step = cells[hit["first_step"] : hit["first_step"] + 2]
            containing = next(
                (
                    item
                    for item in graph["stretch"]
                    if any([tuple(a), tuple(b)] == step for a, b in pairwise(item["cells"]))
                ),
                stretch,
            )
            offset = cells.index(tuple(containing["cells"][0]))
            before = sum(ground_length([a, b], origin) for a, b in pairwise(cells[: offset + 1]))
            lines.append(
                f"- stretch {containing['id']}, {round(hit['distance'] - before)} m: {hit['river']}, {float(hit['width']):.1f} m"
            )
    lines.extend(("", "## Wades"))
    wade_count = sum(len(stretch.get("wades", [])) for stretch in graph["stretch"])
    lines.append(f"- {wade_count} wades")
    for stretch in graph["stretch"]:
        for hit in stretch.get("wades", []):
            lines.append(
                f"- stretch {stretch['id']}, {hit['along_m']} m: {hit['river']}, {hit['width_tenths_m'] / 10:.1f} m"
            )
    if water is not None:
        lines.extend(water_report(graph, water))
    return "\n".join(lines) + "\n"


# ---- Water: classes, the river graph, landings (CK-401) ----

_CELL_M = 839.0
_SEA_CAP = 65_534
_NOT_SEA = 65_535
_MICRO = 1_000_000
_OPEN_KINDS = ("sea", "lake")
_SEA_LAYER = "sea_distance.png"
_DEFAULT_WATER = {"river_width_m": 20, "shore_height_m": 10, "strait_m": 500}
_PLACE_STEPS = 2
_ACCESS_STEPS = 4


def cell_side_um(row: int, origin: Cell) -> int:
    """Ground length of a cell's side at the latitude of a row's centre, in micrometres."""
    radius = 6_378_137.0
    half_world = math.pi * radius
    y = (origin[1] + row + 0.5) * _CELL_M
    lat = 2 * math.atan(math.exp((half_world - y) / radius)) - math.pi / 2
    return round(_CELL_M * _MICRO * math.cos(lat))


def cell_side_m(row: int, origin: Cell) -> int:
    """A cell side's ground length at a row, in whole metres, a half going up."""
    return (cell_side_um(row, origin) + _MICRO // 2) // _MICRO


def _squared_distance_rows(  # noqa: PLR0912  # The two passes of the exact transform share their stacks.
    is_land: list[list[bool]],
) -> list[list[int]]:
    """Exact squared distance, in cells, from each cell to the nearest cell that is not sea."""
    height, width = len(is_land), len(is_land[0]) if is_land else 0
    big = width + height + 2
    column = [[big] * width for _ in range(height)]
    for x in range(width):
        for y in range(height):
            if is_land[y][x]:
                column[y][x] = 0
            elif y:
                column[y][x] = min(column[y][x], column[y - 1][x] + 1)
        for y in range(height - 2, -1, -1):
            column[y][x] = min(column[y][x], column[y + 1][x] + 1)

    def f(x: int, i: int, g: list[int]) -> int:
        return (x - i) ** 2 + g[i] ** 2

    def sep(i: int, u: int, g: list[int]) -> int:
        return (u * u - i * i + g[u] ** 2 - g[i] ** 2) // (2 * (u - i))

    result = []
    for g in column:
        stack_s = [0] * width
        stack_t = [0] * width
        q = 0
        for u in range(1, width):
            while q >= 0 and f(stack_t[q], stack_s[q], g) > f(stack_t[q], u, g):
                q -= 1
            if q < 0:
                q = 0
                stack_s[0] = u
            else:
                w = 1 + sep(stack_s[q], u, g)
                if w < width:
                    q += 1
                    stack_s[q] = u
                    stack_t[q] = w
        row = [0] * width
        for u in range(width - 1, -1, -1):
            row[u] = f(u, stack_s[q], g)
            if u == stack_t[q]:
                q -= 1
        result.append(row)
    return result


def sea_distance_layer(kinds: list[list[str]], origin: Cell) -> list[list[int]]:
    """Each sea cell's distance from land in metres, cut at 65,534; every other cell 65,535."""
    squared = _squared_distance_rows([[kind != "sea" for kind in row] for row in kinds])
    layer = []
    for y, (row, kind_row) in enumerate(zip(squared, kinds, strict=True)):
        side = cell_side_um(y, origin)
        out = []
        for d2, kind in zip(row, kind_row, strict=True):
            if kind != "sea":
                out.append(_NOT_SEA)
                continue
            metres = (math.isqrt(d2 * _MICRO**2) * side + _MICRO**2 // 2) // _MICRO**2
            out.append(min(metres, _SEA_CAP))
        layer.append(out)
    return layer


def sea_distance_png(layer: list[list[int]]) -> bytes:
    """Encode the layer as one channel of 16 bits."""
    image_module = importlib.import_module("PIL.Image")
    height, width = len(layer), len(layer[0]) if layer else 0
    image = image_module.new("I;16", (width, height))
    image.putdata([value for row in layer for value in row])
    stream = io.BytesIO()
    image.save(stream, format="PNG", compress_level=9, optimize=False)
    return stream.getvalue()


def read_sea_distance(data: bytes, width: int, height: int) -> list[list[int]]:
    """Decode a layer, refusing anything but one channel of 16 bits at the map's size."""
    image_module = importlib.import_module("PIL.Image")
    try:
        with image_module.open(io.BytesIO(data)) as image:
            if image.mode != "I;16":
                raise ValueError(f"sea distance layer: sample format {image.mode}, not 16 bits")
            if image.size != (width, height):
                raise ValueError("sea distance layer: size does not match map crop")
            values = list(getattr(image, "get_flattened_data", image.getdata)())
    except OSError as error:
        raise ValueError(f"sea distance layer: malformed: {error}") from error
    return [values[row * width : (row + 1) * width] for row in range(height)]


class RiverLine:
    """One surveyed stretch in cell coordinates, with positions measured as the crossing scan does."""

    def __init__(self, stretch: dict[str, Any], origin: Cell) -> None:
        self.id: str = stretch["id"]
        self.river: str = stretch["river"]
        self.kind: str = stretch["kind"]
        self.ends: str = stretch["ends"]
        self.width = (Fraction(str(stretch["width_m"][0])), Fraction(str(stretch["width_m"][1])))
        raw = [project_point(lat, lon, origin) for lat, lon in stretch["points"]]
        self.points = [(Fraction(x, _MICRO), Fraction(y, _MICRO)) for x, y in raw]
        self.lengths = [
            math.isqrt((b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2) for a, b in pairwise(raw)
        ]
        self.before: list[int] = []
        running = 0
        for length in self.lengths:
            self.before.append(running)
            running += length
        self.total = running

    def point_at(self, position: Fraction) -> tuple[Fraction, Fraction]:
        """The exact point at a position along the line."""
        for index, length in enumerate(self.lengths):
            if length and position <= self.before[index] + length:
                a, b = self.points[index], self.points[index + 1]
                u = (position - self.before[index]) / length
                return a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u
        return self.points[-1]

    def segment_at(self, position: Fraction) -> int:
        """The index of the line's piece that holds a position."""
        for index, length in enumerate(self.lengths):
            if length and position <= self.before[index] + length:
                return index
        return len(self.lengths) - 1

    def width_at(self, position: Fraction) -> Fraction:
        """The width read along the line between its two end widths."""
        return self.width[0] + (self.width[1] - self.width[0]) * position / self.total

    def usable(self, narrowest: Fraction) -> tuple[Fraction, Fraction] | None:
        """The part of the line that is usable water, or none when it has no length."""
        total = Fraction(self.total)
        if not self.total:
            return None
        if self.kind in {"tidal", "lough"}:
            return Fraction(0), total
        w0, w1 = self.width
        if w0 == w1:
            return (Fraction(0), total) if w0 >= narrowest else None
        reach = total * (narrowest - w0) / (w1 - w0)
        low, high = (
            (max(Fraction(0), reach), total) if w1 > w0 else (Fraction(0), min(total, reach))
        )
        return (low, high) if low < high else None


def line_runs(line: RiverLine) -> list[tuple[Cell, Fraction, Fraction]]:
    """Each run of the line in a cell, in order, as the cell and its start and end positions."""
    runs: list[tuple[Cell, Fraction, Fraction]] = []
    for index, length in enumerate(line.lengths):
        if not length:
            continue
        p, q = line.points[index], line.points[index + 1]
        cuts = {Fraction(0), Fraction(1)}
        for axis in (0, 1):
            low, high = sorted((p[axis], q[axis]))
            if low == high:
                continue
            for k in range(math.floor(low) + 1, math.ceil(high)):
                cuts.add((k - p[axis]) / (q[axis] - p[axis]))
        ordered = sorted(cuts)
        for t0, t1 in pairwise(ordered):
            mid = (t0 + t1) / 2
            x = p[0] + (q[0] - p[0]) * mid
            y = p[1] + (q[1] - p[1]) * mid
            if x.denominator == 1 or y.denominator == 1:
                continue
            cell = (math.floor(x), math.floor(y))
            start = line.before[index] + t0 * length
            end = line.before[index] + t1 * length
            if runs and runs[-1][0] == cell and runs[-1][2] == start:
                runs[-1] = (cell, runs[-1][1], end)
            else:
                runs.append((cell, start, end))
    return runs


Position = Fraction
_SIDE_BELOW = 1


class WaterLine:
    """A surveyed stretch with its usable part, its breaks and its runs in cells."""

    def __init__(self, line: RiverLine, narrowest: Fraction, breaks: list[dict[str, Any]]) -> None:
        self.line = line
        self.usable = line.usable(narrowest)
        runs = line_runs(line)
        self.breaks: list[tuple[Position, str, Cell]] = []
        for entry in breaks:
            cell = (entry["cell"][0], entry["cell"][1])
            index = next((i for i, run in enumerate(runs) if run[0] == cell), None)
            if index is None:
                raise ValueError(f"break {entry['name']}: stretch {line.id} is not in cell {cell}")
            _, start, end = runs[index]
            middle = (start + end) / 2
            runs[index : index + 1] = [(cell, start, middle), (cell, middle, end)]
            self.breaks.append((middle, entry["name"], cell))
        self.breaks.sort()
        self.runs = runs
        seen: dict[Cell, int] = {}
        self.run_index: list[int] = []
        for cell, _, _ in runs:
            self.run_index.append(seen.get(cell, 0))
            seen[cell] = seen.get(cell, 0) + 1
        self.break_positions = {position for position, _, _ in self.breaks}

    def run_usable(self, index: int) -> tuple[Position, Position] | None:
        """The usable part of a run, when it has length."""
        if self.usable is None:
            return None
        _, start, end = self.runs[index]
        low, high = max(start, self.usable[0]), min(end, self.usable[1])
        return (low, high) if low < high else None

    def is_usable_at(self, position: Position) -> bool:
        """Whether a position lies in the usable part, which has length."""
        return self.usable is not None and self.usable[0] <= position <= self.usable[1]

    def side_of(self, index: int, position: Position) -> int:
        """Which side of a break a run's own start lies on: 1 when it starts at one."""
        start = self.runs[index][1]
        return _SIDE_BELOW if position == start and start in self.break_positions else 0

    def cell_at(self, position: Position) -> Cell:
        """The cell that holds a position, an edge going to the cell being entered."""
        for cell, start, end in self.runs:
            if start <= position < end:
                return cell
        for cell, start, end in self.runs:
            if start < position <= end:
                return cell
        x, y = self.line.point_at(position)
        return (math.floor(x), math.floor(y))


def ground_piece_m(
    a: tuple[Fraction, Fraction], b: tuple[Fraction, Fraction], origin: Cell
) -> float:
    """The ground length of a short straight piece between two points in cell coordinates."""
    radius = 6_378_137.0
    half_world = math.pi * radius
    ymid = (float(a[1]) + float(b[1])) / 2
    lat = 2 * math.atan(math.exp((half_world - (origin[1] + ymid) * _CELL_M) / radius))
    lat -= math.pi / 2
    dx, dy = float(b[0] - a[0]), float(b[1] - a[1])
    return math.hypot(dx, dy) * _CELL_M * math.cos(lat)


def reach_length_mm(wl: WaterLine, start: Position, end: Position, origin: Cell) -> int:
    """A reach's ground length in whole millimetres, rounded up; never nothing for two joints."""
    line = wl.line
    points = [line.point_at(start)]
    for position, vertex in zip([*line.before, line.total], line.points, strict=True):
        if start < position < end:
            points.append(vertex)
    points.append(line.point_at(end))
    metres = sum(ground_piece_m(a, b, origin) for a, b in pairwise(points))
    return max(1, math.ceil(metres * 1000 - 1e-6))


def _micro(position: Position, total: int) -> int:
    value = position * _MICRO / total
    return (value.numerator * 2 + value.denominator) // (2 * value.denominator)


def build_river_graph(  # noqa: PLR0912  # One pass makes joints, reaches and joins together.
    lines: dict[str, WaterLine],
    kinds: list[list[str]],
    origin: Cell,
    extras: set[tuple[str, Position, int]],
) -> dict[str, Any]:
    """The river graph: joints, the reaches between them, and what each joint is joined to."""
    joint_keys: list[tuple[str, Position, int]] = []
    for name in sorted(lines):
        wl = lines[name]
        total = Fraction(wl.line.total)
        keys: set[tuple[Position, int]] = {(Fraction(0), 0), (total, 0)}
        if wl.usable is not None:
            keys.update({(wl.usable[0], 0), (wl.usable[1], 0)})
        for position, _, _ in wl.breaks:
            keys.update({(position, 0), (position, _SIDE_BELOW)})
        for index, (cell, _, _) in enumerate(wl.runs):
            part = wl.run_usable(index)
            if part is not None and kinds[cell[1]][cell[0]] in _OPEN_KINDS:
                keys.add((part[0], wl.side_of(index, part[0])))
        keys.update((position, side) for stretch, position, side in extras if stretch == name)
        joint_keys.extend((name, position, side) for position, side in sorted(keys))
    # a joint at a break's position that is not the below side belongs to the part above it
    joints: list[dict[str, Any]] = []
    index_of: dict[tuple[str, Position, int], int] = {}
    for key in joint_keys:
        name, position, side = key
        index_of[key] = len(joints)
        joints.append(
            {
                "id": len(joints),
                "stretch": name,
                "cell": list(lines[name].cell_at(position)),
                "position": position,
                "side": side,
            }
        )
    reaches: list[dict[str, Any]] = []
    first_reach: dict[str, int] = {}
    last_reach: dict[str, int] = {}
    reach_from: dict[int, int] = {}
    for name in sorted(lines):
        wl = lines[name]
        mine = [j for j in joints if j["stretch"] == name]
        for left, right in pairwise(mine):
            if left["position"] == right["position"]:
                continue
            start, end = left["position"], right["position"]
            usable = wl.usable is not None and wl.usable[0] <= start and end <= wl.usable[1]
            total = wl.line.total
            entry = {
                "id": len(reaches),
                "stretch": name,
                "from": left["id"],
                "to": right["id"],
                "usable": usable,
                "length_mm": reach_length_mm(wl, start, end, origin),
                "kind": wl.line.kind,
                "begin": _micro(start, total),
                "end": _micro(end, total),
                "start": start,
                "finish": end,
            }
            first_reach.setdefault(name, entry["id"])
            last_reach[name] = entry["id"]
            reach_from[left["id"]] = entry["id"]
            reaches.append(entry)
    joins: list[dict[str, Any]] = []
    for name in sorted(lines):
        target = lines[name].line.ends
        if target in lines and name in last_reach and target in first_reach:
            last, first = reaches[last_reach[name]], reaches[first_reach[target]]
            if last["usable"] and first["usable"]:
                joins.append({"reach": last["id"], "to_reach": first["id"]})
        wl = lines[name]
        for index, (cell, _, _) in enumerate(wl.runs):
            part = wl.run_usable(index)
            if part is None or kinds[cell[1]][cell[0]] not in _OPEN_KINDS:
                continue
            joint = index_of[(name, part[0], wl.side_of(index, part[0]))]
            joins.append({"reach": reach_from[joint], "cell": list(cell)})
    return {
        "joints": joints,
        "reaches": reaches,
        "joins": joins,
        "index": index_of,
        "breaks": [
            {
                "name": entry_name,
                "stretch": name,
                "cell": list(cell),
                "above": index_of[(name, position, 0)],
                "below": index_of[(name, position, _SIDE_BELOW)],
            }
            for name in sorted(lines)
            for position, entry_name, cell in lines[name].breaks
        ],
    }


class _Sets:
    """A small union-find over integers."""

    def __init__(self, size: int) -> None:
        self.parent = list(range(size))

    def find(self, item: int) -> int:
        while self.parent[item] != item:
            self.parent[item] = self.parent[self.parent[item]]
            item = self.parent[item]
        return item

    def union(self, a: int, b: int) -> None:
        a, b = self.find(a), self.find(b)
        if a != b:
            self.parent[max(a, b)] = min(a, b)


def class_cells(
    kinds: list[list[str]], layer: list[list[int]], classes: list[dict[str, Any]]
) -> list[set[Cell]]:
    """For each class, the open cells it can use: every lake cell, and the sea within its reach."""
    result: list[set[Cell]] = []
    for item in classes:
        cells: set[Cell] = set()
        for y, row in enumerate(kinds):
            for x, kind in enumerate(row):
                if kind == "lake" or (kind == "sea" and layer[y][x] <= item["reach_m"]):
                    cells.add((x, y))
        result.append(cells)
    return result


def water_bodies(  # noqa: PLR0912  # Cells, reaches and joins are joined for each class in turn.
    graph: dict[str, Any], usable: list[set[Cell]], width: int, height: int
) -> list[dict[str, Any]]:
    """The bodies of each class: its usable water joined by the three ways of rule 7."""
    joints, reaches = graph["joints"], graph["reaches"]
    members = {r["from"] for r in reaches if r["usable"]} | {
        r["to"] for r in reaches if r["usable"]
    }
    result = []
    for number, cells in enumerate(usable):
        sets = _Sets(width * height + len(joints))
        for x, y in cells:
            if (x + 1, y) in cells:
                sets.union(y * width + x, y * width + x + 1)
            if (x, y + 1) in cells:
                sets.union(y * width + x, (y + 1) * width + x)
        for reach in reaches:
            if reach["usable"]:
                sets.union(width * height + reach["from"], width * height + reach["to"])
        for join in graph["joins"]:
            if "to_reach" in join:
                a = reaches[join["reach"]]["to"]
                b = reaches[join["to_reach"]]["from"]
                sets.union(width * height + a, width * height + b)
            elif tuple(join["cell"]) in cells:
                joint = reaches[join["reach"]]["from"]
                sets.union(width * height + joint, join["cell"][1] * width + join["cell"][0])
        ids: dict[int, int] = {}
        cell_body: dict[Cell, int] = {}
        joint_body: dict[int, int] = {}
        for x, y in sorted(cells, key=lambda c: (c[1], c[0])):
            cell_body[x, y] = ids.setdefault(sets.find(y * width + x), len(ids))
        for joint in sorted(members):
            joint_body[joint] = ids.setdefault(sets.find(width * height + joint), len(ids))
        sizes = [[0, 0] for _ in ids]
        for body in cell_body.values():
            sizes[body][0] += 1
        for body in joint_body.values():
            sizes[body][1] += 1
        result.append(
            {
                "class": number,
                "cells": cell_body,
                "joints": joint_body,
                "count": len(ids),
                "sizes": sizes,
            }
        )
    return result


def _side(
    direction: tuple[Fraction, Fraction],
    origin: tuple[Fraction, Fraction],
    point: tuple[Fraction, Fraction],
) -> str:
    """Which bank of a line facing downstream a point is on; left where it is on the line."""
    cross = direction[0] * (point[1] - origin[1]) - direction[1] * (point[0] - origin[0])
    return "right" if cross > 0 else "left"


def _opposite(side: str) -> str:
    return "left" if side == "right" else "right"


def _nearest_point_on_run(wl: WaterLine, index: int, centre: tuple[Fraction, Fraction]) -> Position:
    """The position on a run of the point nearest a cell's centre, the first when tied."""
    _, start, end = wl.runs[index]
    line = wl.line
    spots = [start]
    spots.extend(position for position in [*line.before, line.total] if start < position < end)
    spots.append(end)
    best: tuple[Fraction, Position] | None = None
    for a, b in pairwise(spots):
        pa, pb = line.point_at(a), line.point_at(b)
        dx, dy = pb[0] - pa[0], pb[1] - pa[1]
        length2 = dx * dx + dy * dy
        t = Fraction(0)
        if length2:
            t = ((centre[0] - pa[0]) * dx + (centre[1] - pa[1]) * dy) / length2
            t = min(Fraction(1), max(Fraction(0), t))
        position = a + (b - a) * t
        px, py = line.point_at(position)
        distance = (px - centre[0]) ** 2 + (py - centre[1]) ** 2
        if best is None or distance < best[0]:
            best = (distance, position)
    assert best is not None
    return best[1]


def run_bank(wl: WaterLine, index: int, shore: Cell) -> str:
    """The bank of a run: the side of the line its shore cell's centre is on, nearest the run."""
    centre = (Fraction(shore[0]) + Fraction(1, 2), Fraction(shore[1]) + Fraction(1, 2))
    position = _nearest_point_on_run(wl, index, centre)
    segment = wl.line.segment_at(position)
    a, b = wl.line.points[segment], wl.line.points[segment + 1]
    return _side((b[0] - a[0], b[1] - a[1]), wl.line.point_at(position), centre)


def land_steps(land: Land, start: Cell, steps: int) -> dict[Cell, int]:
    """Land cells reached from a cell in no more than so many steps over shared edges."""
    found = {start: 0}
    frontier = [start]
    for depth in range(1, steps + 1):
        following = []
        for x, y in frontier:
            for dx, dy in ((0, -1), (-1, 0), (1, 0), (0, 1)):
                cell = (x + dx, y + dy)
                if (
                    cell not in found
                    and 0 <= cell[1] < len(land)
                    and 0 <= cell[0] < len(land[0])
                    and land[cell[1]][cell[0]]
                ):
                    found[cell] = depth
                    following.append(cell)
        frontier = following
    return found


def water_settings(hand: dict[str, Any]) -> dict[str, Any]:
    """The hand list's water: classes, the narrowest river, shore height, strait length, marks."""
    water = {**_DEFAULT_WATER, **hand.get("water", {})}
    return {
        "river_width": Fraction(str(water["river_width_m"])),
        "shore_height": Fraction(str(water["shore_height_m"])),
        "strait_m": water["strait_m"],
        "classes": [dict(item) for item in hand.get("water_class", [])],
        "breaks": [dict(item) for item in hand.get("river_break", [])],
        "marks": {item["place"] for item in hand.get("mark", [])},
    }


def landing_key(entry: dict[str, Any]) -> tuple[Any, ...]:
    """What a landing keeps its id by: never a joint's number, and for a crossing end only its stretch and which end."""
    if "crossing" in entry:
        return ("crossing", entry["crossing"], entry["end"])
    base = (entry["node"], entry.get("bank", ""))
    if "survey" in entry:
        return (*base, "river", entry["survey"], tuple(entry["shore"]), entry["run"])
    return (*base, "open", entry["cell"][0], entry["cell"][1])


def _node_search(node: dict[str, Any], accesses: set[str]) -> tuple[int, bool]:
    """How many steps a node's search takes, and whether it takes any shore."""
    if node["kind"] == "place":
        return (_ACCESS_STEPS if node["place"] in accesses else _PLACE_STEPS), True
    if node["kind"] == "landing":
        return _ACCESS_STEPS, True
    return 0, False


def _reach_at(river: dict[str, Any], stretch: str, position: Position) -> dict[str, Any] | None:
    """The usable reach of a stretch that holds a position, if any."""
    for reach in river["reaches"]:
        if (
            reach["stretch"] == stretch
            and reach["usable"]
            and reach["start"] <= position <= reach["end"]
        ):
            return reach
    return None


def _centre(cell: Cell) -> tuple[Fraction, Fraction]:
    return Fraction(cell[0]) + Fraction(1, 2), Fraction(cell[1]) + Fraction(1, 2)


def _crossing_contact(  # Reads one crossing end's point, joint and bank together.
    stretch: dict[str, Any],
    members: list[dict[str, Any]],
    end: str,
    index: RiverIndex,
    lines: dict[str, WaterLine],
) -> tuple[str, Position, str]:
    """The surveyed stretch, position and bank of a crossing end, from slice 3a's own scan."""
    raw = [hit for item in members for hit in item["members"]]
    pick = min if end == "from" else max
    hit = pick(raw, key=lambda h: (h["distance"], h["river"]))
    cells = [tuple(c) for c in stretch["cells"]]
    offset = members[0]["span_start"]
    local = hit["step"] - offset
    found: tuple[str, Position] | None = None
    step = local
    for candidate in (local, local + 1):
        if not 0 <= candidate < len(cells) - 1:
            continue
        a, b = cells[candidate], cells[candidate + 1]
        for (river, t, _width), place in sorted(index.scan(a, b).items(), key=lambda i: i[1]):
            spot = (a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]))
            if river == hit["river"] and spot == hit["point"] and found is None:
                found, step = place, candidate
    if found is None:
        raise ValueError(f"crossing stretch {stretch['id']}: no surveyed point for its {end} end")
    name, position = found
    wl = lines[name]
    segment = wl.line.segment_at(position)
    a, b = wl.line.points[segment], wl.line.points[segment + 1]
    direction = (b[0] - a[0], b[1] - a[1])
    here = (hit["point"][0] + Fraction(1, 2), hit["point"][1] + Fraction(1, 2))

    def cross(cell: Cell) -> Fraction:
        spot = _centre(cell)
        return direction[0] * (spot[1] - here[1]) - direction[1] * (spot[0] - here[0])

    before = cells[step::-1] if end == "from" else cells[step + 1 :]
    after = cells[step + 1 :] if end == "from" else cells[step::-1]
    for cell in before:
        if cross(cell):
            return name, position, _side(direction, here, _centre(cell))
    for cell in after:
        if cross(cell):
            return name, position, _opposite(_side(direction, here, _centre(cell)))
    return name, position, "left"


def _roadside(
    graph: dict[str, Any],
    context: dict[str, Any],
    runs_at: dict[Cell, list[tuple[str, int, Position]]],
    usable: list[set[Cell]],
) -> list[dict[str, Any]]:
    """Each road stretch with a cell away from any node that is beside usable water at an allowed shore.

    The cell is a shore as a node's own cell is (rules 11 and 13): not marsh, with an open cell that some class can use sharing
    an edge with it that is not too far below it, or with a run of a river's usable part in it. Nothing can embark there.
    """
    layers = context["layers"]
    kinds, terrain, elevation = layers["kind"], layers["terrain"], layers["elevation"]
    height, width = len(kinds), len(kinds[0])
    limit = water_settings(context["hand"])["shore_height"]
    node_cells = {(item["cell"][0], item["cell"][1]) for item in graph["node"]}

    def embarks(cell: Cell) -> bool:
        if terrain[cell[1]][cell[0]] == "marsh":
            return False
        if runs_at.get(cell):
            return True
        for dx, dy in ((0, -1), (-1, 0), (1, 0), (0, 1)):
            spot = (cell[0] + dx, cell[1] + dy)
            if not (
                0 <= spot[0] < width
                and 0 <= spot[1] < height
                and kinds[spot[1]][spot[0]] in _OPEN_KINDS
                and any(spot in cells for cells in usable)
            ):
                continue
            water = elevation[spot[1]][spot[0]] if kinds[spot[1]][spot[0]] == "lake" else 0
            if elevation[cell[1]][cell[0]] - water <= limit:
                return True
        return False

    beside: list[dict[str, Any]] = []
    for stretch in sorted(graph["stretch"], key=lambda s: s["id"]):
        if stretch["tidal"]:
            continue
        for item in stretch["cells"]:
            cell = (item[0], item[1])
            if cell not in node_cells and embarks(cell):
                beside.append({"stretch": stretch["id"], "cell": cell})
                break
    return beside


def plan_water(  # noqa: PLR0912, PLR0915  # One pass turns the hand list's water into tables.
    graph: dict[str, Any],
    context: dict[str, Any],
    crossing_hits: dict[int, list[dict[str, Any]]],
    previous: dict[str, Any] | None,
) -> dict[str, Any]:
    """Work the water of a network once: the layer, bodies, landings, crossing ends and straits."""
    layers = context["layers"]
    origin, kinds, land = layers["origin"], layers["kind"], layers["land"]
    height, width = len(kinds), len(kinds[0])
    settings = water_settings(context["hand"])
    classes: list[dict[str, Any]] = settings["classes"]
    names = [item["id"] for item in classes]
    layer = sea_distance_layer(kinds, origin)
    png = sea_distance_png(layer)
    usable = class_cells(kinds, layer, classes)
    survey = {r["id"]: r for r in context["rivers"]}
    breaks_by: dict[str, list[dict[str, Any]]] = {}
    for entry in settings["breaks"]:
        if entry["stretch"] not in survey:
            raise ValueError(f"break {entry['name']}: unknown stretch {entry['stretch']}")
        breaks_by.setdefault(entry["stretch"], []).append(entry)
    lines = {
        name: WaterLine(RiverLine(item, origin), settings["river_width"], breaks_by.get(name, []))
        for name, item in survey.items()
    }
    base = build_river_graph(lines, kinds, origin, set())
    first = water_bodies(base, usable, width, height)
    by_stretch: dict[str, list[dict[str, Any]]] = {}
    for reach in base["reaches"]:
        if reach["usable"]:
            by_stretch.setdefault(reach["stretch"], []).append(reach)

    def river_body(stretch: str, position: Position, number: int, side: int = 0) -> int | None:
        for reach in by_stretch.get(stretch, []):
            if reach["start"] <= position <= reach["finish"] and not (
                side == _SIDE_BELOW and reach["finish"] == position
            ):
                return first[number]["joints"].get(reach["from"])
        return None

    runs_at: dict[Cell, list[tuple[str, int, Position]]] = {}
    for name in sorted(lines):
        for number in range(len(lines[name].runs)):
            part = lines[name].run_usable(number)
            if part is not None:
                runs_at.setdefault(lines[name].runs[number][0], []).append((name, number, part[0]))
    nodes = {item["id"]: item for item in graph["node"]}
    accesses = {item["place"]: item for item in graph.get("access", [])}
    marked = settings["marks"]

    def passes(item: dict[str, Any], number: int) -> bool:
        return item["place"] not in marked or classes[number]["sea_going"]

    landings: list[dict[str, Any]] = []
    refused: list[dict[str, Any]] = []
    seen_keys: set[tuple[Any, ...]] = set()

    def add(entry: dict[str, Any], bodies: list[int | None]) -> None:
        key = landing_key(entry)
        if key not in seen_keys:
            seen_keys.add(key)
            landings.append({**entry, "bodies": bodies})

    for node_id in sorted(nodes):
        node = nodes[node_id]
        steps, anywhere = _node_search(node, set(accesses))
        cell = (node["cell"][0], node["cell"][1])
        side_um = cell_side_um(cell[1], origin)
        contacts: list[dict[str, Any]] = []
        for shore, depth in sorted(
            land_steps(land, cell, steps).items(), key=lambda i: (i[1], i[0][1], i[0][0])
        ):
            approach = (depth * side_um + _MICRO // 2) // _MICRO
            reasons: set[str] = set()
            for dx, dy in ((0, -1), (-1, 0), (1, 0), (0, 1)):
                spot = (shore[0] + dx, shore[1] + dy)
                if (
                    0 <= spot[0] < width
                    and 0 <= spot[1] < height
                    and kinds[spot[1]][spot[0]] in _OPEN_KINDS
                    and any(spot in cells for cells in usable)
                ):
                    refusal = None
                    if not anywhere:
                        if layers["terrain"][shore[1]][shore[0]] == "marsh":
                            refusal = "marsh"
                        else:
                            water = (
                                layers["elevation"][spot[1]][spot[0]]
                                if kinds[spot[1]][spot[0]] == "lake"
                                else 0
                            )
                            rise = layers["elevation"][shore[1]][shore[0]] - water
                            if rise > settings["shore_height"]:
                                refusal = f"{rise} m above the water"
                    if refusal:
                        reasons.add(refusal)
                    else:
                        contacts.append(
                            {"type": "open", "cell": spot, "shore": shore, "approach": approach}
                        )
            for name, number, position in runs_at.get(shore, []):
                if not anywhere and layers["terrain"][shore[1]][shore[0]] == "marsh":
                    reasons.add("marsh")
                    continue
                contacts.append(
                    {
                        "type": "river",
                        "stretch": name,
                        "run": number,
                        "position": position,
                        "shore": shore,
                        "approach": approach,
                    }
                )
            refused.extend(
                {"node": node_id, "shore": list(shore), "reason": reason}
                for reason in sorted(reasons)
            )
        place_item = {"place": node.get("place")}
        for number in range(len(classes)):
            if node.get("place") and not passes(place_item, number):
                continue
            best: dict[int, tuple[Any, dict[str, Any]]] = {}
            for contact in contacts:
                if contact["type"] == "open":
                    body = first[number]["cells"].get(contact["cell"])
                    rank = (
                        contact["approach"],
                        0,
                        contact["shore"][1],
                        contact["shore"][0],
                        contact["cell"][1],
                        contact["cell"][0],
                        "",
                        Fraction(0),
                    )
                else:
                    body = river_body(
                        contact["stretch"],
                        contact["position"],
                        number,
                        lines[contact["stretch"]].side_of(contact["run"], contact["position"]),
                    )
                    rank = (
                        contact["approach"],
                        1,
                        contact["shore"][1],
                        contact["shore"][0],
                        0,
                        0,
                        contact["stretch"],
                        contact["position"],
                    )
                if body is not None and (body not in best or rank < best[body][0]):
                    best[body] = (rank, contact)
            for _, contact in best.values():
                entry: dict[str, Any] = {
                    "node": node_id,
                    "shore": list(contact["shore"]),
                    "approach_m": contact["approach"],
                }
                if contact["type"] == "open":
                    entry["cell"] = list(contact["cell"])
                    bodies = [
                        first[n]["cells"].get(contact["cell"])
                        if passes(place_item, n) or not node.get("place")
                        else None
                        for n in range(len(classes))
                    ]
                else:
                    wl = lines[contact["stretch"]]
                    entry["bank"] = run_bank(wl, contact["run"], contact["shore"])
                    entry["survey"] = contact["stretch"]
                    entry["run"] = wl.run_index[contact["run"]]
                    entry["_position"] = contact["position"]
                    entry["_side"] = wl.side_of(contact["run"], contact["position"])
                    bodies = [
                        river_body(contact["stretch"], contact["position"], n, entry["_side"])
                        if passes(place_item, n) or not node.get("place")
                        else None
                        for n in range(len(classes))
                    ]
                add(entry, bodies)
    ends: dict[tuple[int, str], dict[str, Any] | None] = {}
    for stretch in graph["stretch"]:
        if not stretch.get("crossings"):
            continue
        for end in ("from", "to"):
            name, position, bank = _crossing_contact(
                stretch, crossing_hits[stretch["id"]], end, context["index"], lines
            )
            node = nodes[stretch[end]]
            wl = lines[name]
            cell = (node["cell"][0], node["cell"][1])
            reason = None
            if not wl.is_usable_at(position):
                reason = "the river is not usable at the point"
            elif position in wl.break_positions:
                reason = "the point is a break"
            elif layers["terrain"][cell[1]][cell[0]] == "marsh":
                reason = "marsh"
            if reason:
                ends[stretch["id"], end] = None
                refused.append(
                    {
                        "node": node["id"],
                        "shore": list(cell),
                        "reason": reason,
                        "crossing": stretch["id"],
                        "end": end,
                    }
                )
                continue
            entry = {
                "node": node["id"],
                "shore": list(cell),
                "approach_m": 0,
                "bank": bank,
                "crossing": stretch["id"],
                "end": end,
                "_survey": name,
                "_position": position,
                "_side": 0,
            }
            bodies = [river_body(name, position, n) for n in range(len(classes))]
            add(entry, bodies)
            ends[stretch["id"], end] = entry
    extras = {
        (entry.get("survey", entry.get("_survey")), entry["_position"], entry["_side"])
        for entry in landings
        if "_position" in entry
    }
    river = build_river_graph(lines, kinds, origin, extras)
    final = water_bodies(river, usable, width, height)
    for entry in landings:
        if "_position" in entry:
            entry["joint"] = river["index"][
                (entry.get("survey", entry.get("_survey")), entry["_position"], entry["_side"])
            ]
    return {
        "layer": png,
        "layer_values": layer,
        "settings": settings,
        "usable": usable,
        "river": river,
        "bodies": final,
        "landings": landings,
        "refused": refused,
        "roadside": _roadside(graph, context, runs_at, usable),
        "ends": ends,
        "lines": lines,
        "classes": classes,
        "names": names,
        "previous": previous,
    }


def _same_land(land: Land, start: Cell, goal: Cell) -> bool:
    """Whether two land cells are joined through land cells sharing edges."""
    seen = {start}
    stack = [start]
    while stack:
        x, y = stack.pop()
        if (x, y) == goal:
            return True
        for dx, dy in ((0, -1), (-1, 0), (1, 0), (0, 1)):
            cell = (x + dx, y + dy)
            if (
                cell not in seen
                and 0 <= cell[1] < len(land)
                and 0 <= cell[0] < len(land[0])
                and land[cell[1]][cell[0]]
            ):
                seen.add(cell)
                stack.append(cell)
    return False


def store_water(
    graph: dict[str, Any], plan: dict[str, Any], context: dict[str, Any], hashes: dict[str, str]
) -> None:
    """Put the water into the graph's tables, ready to be stored."""
    layers = context["layers"]
    land = layers["land"]
    settings, classes, river = plan["settings"], plan["classes"], plan["river"]
    names, bodies = plan["names"], plan["bodies"]
    nodes = {item["id"]: item for item in graph["node"]}
    previous = plan["previous"] or {}
    keyed = [landing_key(entry) for entry in plan["landings"]]
    old = {landing_key(entry): entry["id"] for entry in previous.get("landing", [])}
    ids, following = allocate_ids(keyed, old, previous.get("next_landing_id", 1), "next_landing_id")
    stored = []
    for entry, key in zip(plan["landings"], keyed, strict=True):
        item = {k: v for k, v in entry.items() if not k.startswith("_")}
        item["id"] = ids[key]
        item["bodies"] = [-1 if body is None else body for body in entry["bodies"]]
        stored.append(item)
    by_end = {(item["crossing"], item["end"]): item["id"] for item in stored if "crossing" in item}
    for stretch in graph["stretch"]:
        if stretch.get("crossings"):
            for end in ("from", "to"):
                if (stretch["id"], end) in by_end:
                    stretch[f"landing_{end}"] = by_end[stretch["id"], end]
    marked = settings["marks"]
    straits = []
    for access in sorted(graph.get("access", []), key=lambda item: item["place"]):
        place_node = next(
            n for n in graph["node"] if n["kind"] == "place" and n.get("place") == access["place"]
        )
        landing_node = nodes[access["landing"]]
        if _same_land(land, tuple(place_node["cell"]), tuple(landing_node["cell"])):
            straits.append(
                {
                    "place": access["place"],
                    "from": place_node["id"],
                    "to": landing_node["id"],
                    "length_m": settings["strait_m"],
                    "classes": [
                        item["id"]
                        for item in classes
                        if access["place"] not in marked or item["sea_going"]
                    ],
                }
            )
    joins = []
    for join in river["joins"]:
        entry = dict(join)
        if "to_reach" in join:
            entry["classes"] = list(names)
        else:
            entry["classes"] = [
                item["id"]
                for item, cells in zip(classes, plan["usable"], strict=True)
                if tuple(join["cell"]) in cells
            ]
        joins.append(entry)
    graph.update(
        {
            "format": _ROADS_FORMAT,
            "river_width_mm": round(settings["river_width"] * 1000),
            "terrain_sha256": hashes["terrain"],
            "rivers_sha256": hashes["rivers"],
            "sea_distance": _SEA_LAYER,
            "sea_distance_sha256": hashlib.sha256(plan["layer"]).hexdigest(),
            "next_landing_id": following,
            "water_class": [
                {**item, "bodies": body["count"]}
                for item, body in zip(classes, bodies, strict=True)
            ],
            "river_break": river["breaks"],
            "mark": [{"place": place} for place in sorted(marked)],
            "joint": [
                {"id": j["id"], "stretch": j["stretch"], "cell": j["cell"]} for j in river["joints"]
            ],
            "reach": [
                {k: v for k, v in r.items() if k not in {"start", "finish"}}
                for r in river["reaches"]
            ],
            "water_join": joins,
            "landing": stored,
            "strait": straits,
        }
    )


_MAX_REACH_M = 65_533


def validate_water(data: dict[str, Any], locations: dict[str, Cell]) -> None:  # noqa: PLR0912
    """Refuse a malformed water section of the hand list, naming the entry and the fault."""

    def only(label: str, entry: object, allowed: set[str], required: set[str]) -> dict[str, Any]:
        if not isinstance(entry, dict):
            raise ValueError(f"{label}: expected a table")
        unknown = set(entry) - allowed
        if unknown:
            raise ValueError(f"{label}: unknown key {sorted(unknown)[0]}")
        missing = required - set(entry)
        if missing:
            raise ValueError(f"{label}: missing {sorted(missing)[0]}")
        return entry

    water = only("water", data.get("water", {}), set(_DEFAULT_WATER), set())
    for name, value in water.items():
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            raise ValueError(f"water {name}: expected a nonnegative number")
    if "strait_m" in water and not water["strait_m"]:
        raise ValueError("water strait_m: expected a length above nothing")
    seen: set[str] = set()
    for item in data.get("water_class", []):
        fields = {"id", "reach_m", "sea_going"}
        entry = only("water_class", item, fields, fields)
        name = entry["id"]
        if not isinstance(name, str) or not name or name in seen:
            raise ValueError(f"water_class {name}: id must be a new nonempty string")
        seen.add(name)
        reach = entry["reach_m"]
        if type(reach) is not int or not 0 <= reach <= _MAX_REACH_M:
            raise ValueError(f"water_class {name}: reach_m must be 0 to {_MAX_REACH_M}")
        if type(entry["sea_going"]) is not bool:
            raise ValueError(f"water_class {name}: sea_going must be true or false")
    names: set[str] = set()
    spots: set[tuple[str, tuple[int, int]]] = set()
    for item in data.get("river_break", []):
        entry = only("river_break", item, {"stretch", "cell", "name"}, {"stretch", "cell", "name"})
        name = entry["name"]
        cell = entry["cell"]
        if not isinstance(name, str) or not name or name in names:
            raise ValueError(f"river_break {name}: name must be a new nonempty string")
        if not isinstance(entry["stretch"], str) or not entry["stretch"]:
            raise ValueError(f"river_break {name}: stretch must be a nonempty string")
        if (
            not isinstance(cell, list)
            or len(cell) != _TWO
            or any(type(value) is not int for value in cell)
        ):
            raise ValueError(f"river_break {name}: cell must have two integers")
        spot = (entry["stretch"], (cell[0], cell[1]))
        if spot in spots:
            raise ValueError(f"river_break {name}: another break is already in that cell")
        names.add(name)
        spots.add(spot)
    marks: set[str] = set()
    for item in data.get("mark", []):
        entry = only("mark", item, {"place"}, {"place"})
        if entry["place"] not in locations or entry["place"] in marks:
            raise ValueError(f"mark {entry['place']}: repeated or unknown place")
        marks.add(entry["place"])


def water_report(graph: dict[str, Any], water: dict[str, Any]) -> list[str]:  # noqa: PLR0912, PLR0915
    """The report's water: bodies, landings, crossings by boat, straits, marks and gaps."""
    nodes = {item["id"]: item for item in graph["node"]}
    classes, names = water["classes"], water["names"]
    stored = {item["id"]: item for item in graph["landing"]}

    def label(node_id: int) -> str:
        node = nodes[node_id]
        return f"{node.get('place', '')} {node['kind']} node {node_id}".strip()

    lines = ["", "## Water classes"]
    for item, body in zip(classes, water["bodies"], strict=True):
        lines.append(
            f"- {item['id']}: reach {item['reach_m']} m, "
            f"{'sea-going' if item['sea_going'] else 'not sea-going'}, {body['count']} bodies"
        )
    if not classes:
        lines.append("- none")
    lines.extend(("", "## Bodies of water"))
    for item, body in zip(classes, water["bodies"], strict=True):
        for number, (cells, joints) in enumerate(body["sizes"]):
            lines.append(f"- {item['id']} body {number}: {cells} cells, {joints} joints")
    lines.extend(("", "## Landings"))
    by_node: dict[int, list[dict[str, Any]]] = {}
    for entry in sorted(graph["landing"], key=lambda e: e["id"]):
        by_node.setdefault(entry["node"], []).append(entry)
    for node_id in sorted(by_node):
        for entry in by_node[node_id]:
            if "crossing" in entry:
                contact = (
                    f"crossing stretch {entry['crossing']} {entry['end']} end, {entry['bank']} bank"
                )
            elif "survey" in entry:
                contact = f"river {entry['survey']} run {entry['run']}, {entry['bank']} bank"
            else:
                contact = f"open cell {tuple(entry['cell'])}"
            bodies = ", ".join(
                f"{n} {'none' if b < 0 else b}" for n, b in zip(names, entry["bodies"], strict=True)
            )
            lines.append(
                f"- {label(node_id)}: landing {entry['id']}, {contact}, shore {tuple(entry['shore'])}, "
                f"approach {entry['approach_m']} m; {bodies}"
            )
    lines.extend(("", "## Places without a landing"))
    strait_places = {item["place"] for item in graph["strait"]}
    missing = []
    for node in sorted(nodes.values(), key=lambda n: (n["kind"], n.get("place", ""), n["id"])):
        if node["kind"] == "place" and node["id"] not in by_node:
            missing.append(node)
    for node in missing:
        why = "a strait only" if node["place"] in strait_places else "no usable water reached"
        lines.append(f"- {node['place']}: {why}")
    if not missing:
        lines.append("- none")
    lines.extend(("", "## Shores refused"))
    for item in sorted(water["refused"], key=lambda r: (r["node"], r["shore"], r["reason"])):
        lines.append(f"- {label(item['node'])}: shore {tuple(item['shore'])}, {item['reason']}")
    if not water["refused"]:
        lines.append("- none")
    lines.extend(("", "## Crossing stretches by boat"))
    for stretch in sorted(graph["stretch"], key=lambda s: s["id"]):
        if not stretch.get("crossings"):
            continue
        ends = [stretch.get("landing_from"), stretch.get("landing_to")]
        verdicts = []
        for number, name in enumerate(names):
            bodies = [stored[e]["bodies"][number] if e is not None else -1 for e in ends]
            ok = bodies[0] >= 0 and bodies[0] == bodies[1]
            verdicts.append(f"{name} {'crosses' if ok else 'cannot cross'}")
        shown = ", ".join("none" if e is None else str(e) for e in ends)
        lines.append(f"- stretch {stretch['id']}: landings {shown}; {'; '.join(verdicts)}")
    lines.extend(("", "## Straits"))
    for item in graph["strait"]:
        lines.append(
            f"- {item['place']}: {item['length_m']} m, {label(item['from'])} to {label(item['to'])}; "
            f"{', '.join(item['classes']) or 'no class'}"
        )
    if not graph["strait"]:
        lines.append("- none")
    lines.extend(("", "## Sea-going marks"))
    lines.extend([f"- {item['place']}" for item in graph["mark"]] or ["- none"])
    lines.extend(("", "## Roads beside usable water with no node"))
    lines.extend(
        [f"- stretch {item['stretch']}: cell {item['cell']}" for item in water["roadside"]]
        or ["- none"]
    )
    lines.extend(("", "## Places joined by land and water"))
    lines.extend(joined_places(graph, water, nodes))
    return lines


def joined_places(graph: dict[str, Any], water: dict[str, Any], nodes: dict[int, Any]) -> list[str]:
    """For each class, the place nodes that land and water together join to the largest land piece."""
    # The sets are indexed by the node's place in the order of the stable ids, not by the id: a legal id may be as large as
    # the loader's u64 allows, and the connectivity here is bounded by the number of nodes.
    order = {node_id: number for number, node_id in enumerate(sorted(nodes))}
    sets = _Sets(len(order))
    land = _Sets(len(order))
    for stretch in graph["stretch"]:
        if all(c["kind"] == "ford" for c in stretch.get("crossings", [])):
            sets.union(order[stretch["from"]], order[stretch["to"]])
            land.union(order[stretch["from"]], order[stretch["to"]])
    accesses = {item["place"] for item in graph.get("access", [])}
    eligible = [
        i for i, n in nodes.items() if n["kind"] != "place" or n.get("place") not in accesses
    ]
    sizes: dict[int, int] = {}
    for i in eligible:
        sizes[land.find(order[i])] = sizes.get(land.find(order[i]), 0) + 1
    largest = max(sizes, key=lambda r: (sizes[r], -r), default=None)
    result = []
    for number, name in enumerate(water["names"]):
        joined = _Sets(len(sets.parent))
        joined.parent = list(sets.parent)
        members: dict[int, int] = {}
        for entry in graph["landing"]:
            body = entry["bodies"][number]
            if body < 0:
                continue
            if body in members:
                joined.union(order[members[body]], order[entry["node"]])
            else:
                members[body] = entry["node"]
        for strait in graph["strait"]:
            if name in strait["classes"]:
                joined.union(order[strait["from"]], order[strait["to"]])
        root = None if largest is None else joined.find(largest)
        places = sorted(n["place"] for n in nodes.values() if n["kind"] == "place")
        inside = [
            n["place"]
            for n in nodes.values()
            if n["kind"] == "place" and joined.find(order[n["id"]]) == root
        ]
        result.append(f"- {name}: {len(inside)} of {len(places)} places")
        result.append(f"  - joined: {', '.join(sorted(inside)) or 'none'}")
        outside = sorted(set(places) - set(inside))
        result.append(f"  - not joined: {', '.join(outside) or 'none'}")
    return result or ["- none"]


def json_bytes(value: Any) -> bytes:  # noqa: ANN401  # Proposal and graph records carry JSON tables.
    """Encode persisted JSON in a stable, machine-independent form."""
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    ).encode("utf-8")


def atomic_write(path: Path, data: bytes) -> None:
    """Replace a file only after its complete new contents have been written."""
    path.parent.mkdir(parents=True, exist_ok=True)
    name = ""
    try:
        with tempfile.NamedTemporaryFile(
            dir=path.parent, prefix=".network-", delete=False
        ) as stream:
            name = stream.name
            stream.write(data)
        Path(name).replace(path)
    finally:
        if name:
            Path(name).unlink(missing_ok=True)


def commit_files(replacements: dict[Path, bytes | None]) -> None:  # noqa: PLR0912  # Rollback handles missing files and retained backups.
    """Stage all outputs and roll back a failed multi-file publication."""
    staged: dict[Path, Path] = {}
    backups: list[tuple[Path, Path | None]] = []
    preserve_backups = False
    try:
        for path, data in replacements.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            if data is not None:
                with tempfile.NamedTemporaryFile(
                    dir=path.parent, prefix=".network-stage-", delete=False
                ) as stream:
                    staged[path] = Path(stream.name)
                    stream.write(data)
        for path, data in replacements.items():
            backup = None
            if path.exists():
                with tempfile.NamedTemporaryFile(
                    dir=path.parent, prefix=".network-backup-", delete=False
                ) as stream:
                    backup = Path(stream.name)
                backup.unlink()
                path.replace(backup)
            backups.append((path, backup))
            if data is not None:
                staged[path].replace(path)
    except BaseException as error:
        failures = []
        for path, backup in reversed(backups):
            try:
                path.unlink(missing_ok=True)
                if backup is not None:
                    backup.replace(path)
            except OSError as rollback_error:
                failures.append(f"{path}: {rollback_error}; backup {backup}")
        if failures:
            preserve_backups = True
            raise RuntimeError(
                "publication failed; restore retained backups: " + "; ".join(failures)
            ) from error
        raise
    finally:
        for path in staged.values():
            path.unlink(missing_ok=True)
        if not preserve_backups:
            for _, backup in backups:
                if backup is not None:
                    backup.unlink(missing_ok=True)


def proposal_seal(proposal: dict[str, Any]) -> str:
    """Bind pending content and picture hashes to its review entry."""
    return hashlib.sha256(
        json_bytes({key: value for key, value in proposal.items() if key != "seal"})
    ).hexdigest()


def layer_bytes(map_dir: Path) -> bytes:
    """The encoded sea distance layer of a map's terrain."""
    layers = read_rasters(map_dir)
    return sea_distance_png(sea_distance_layer(layers["kind"], layers["origin"]))


def accept_pending(
    pending: Path, files: dict[str, Path], report: Path, log: Path, record: str
) -> None:
    """Publish the complete pictured proposal if all of its inputs still match."""
    if not record.strip():
        raise ValueError("record must not be empty")
    if not pending.exists():
        raise ValueError("no proposal")
    proposal = json.loads(pending.read_bytes())
    if not proposal["complete"]:
        raise ValueError("proposal is blocked")
    if not snapshot_matches(files, proposal["inputs"]):
        raise ValueError("proposal inputs changed; build again")
    seal = proposal.get("seal")
    if (
        not seal
        or seal != proposal_seal(proposal)
        or not log.exists()
        or f"Proposal seal: {seal}" not in log.read_text(encoding="utf-8")
    ):
        raise ValueError("proposal seal does not match its review entry")
    for name, expected in proposal.get("pictures", {}).items():
        picture = (pending.parent / name).resolve()
        if (
            not picture.is_relative_to(pending.parent.resolve())
            or not picture.is_file()
            or hashlib.sha256(picture.read_bytes()).hexdigest() != expected
        ):
            raise ValueError(f"proposal picture changed: {name}")
    baseline = proposal["baseline"]
    for name in proposal["candidates"]:
        baseline["links"][name]["record"] = record
    baseline_bytes = json_bytes(baseline)
    graph = proposal["graph"]
    graph["baseline_sha256"] = hashlib.sha256(baseline_bytes).hexdigest()
    graph["hand_list_sha256"] = proposal["inputs"]["hand"]
    for link in graph["link"]:
        metadata = json.loads(link["record"])
        metadata.update(baseline["links"][link["id"]])
        link["record"] = json.dumps(
            metadata, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        )
    road_bytes = dump_roads(graph).encode("utf-8")
    report_bytes = (
        proposal["report"]
        .replace("# Proposed travel network", "# Travel network", 1)
        .encode("utf-8")
    )
    log_bytes = (log.read_bytes() if log.exists() else b"") + f"\nAccepted: {record}\n".encode()
    layers: dict[Path, bytes | None] = {}
    if "sea_distance_sha256" in graph:
        layer = layer_bytes(files["map"].parent)
        if hashlib.sha256(layer).hexdigest() != graph["sea_distance_sha256"]:
            raise ValueError("proposal sea distance layer differs from the map; build again")
        layers[files["sea_distance"]] = layer
    if not snapshot_matches(files, proposal["inputs"]):
        raise ValueError("proposal inputs changed during acceptance; build again")
    commit_files(
        {
            **layers,
            files["baseline"]: baseline_bytes,
            files["stored"]: road_bytes,
            report: report_bytes,
            log: log_bytes,
            pending: None,
        }
    )


def course_ground_mark(course: list[Cell], layers: dict[str, Any], index: RiverIndex) -> str:
    """Record every supporting cell and every river crossing of a course."""
    crossings: list[tuple[Any, ...]] = []
    for position, (start, end) in enumerate(pairwise(course)):
        crossings.extend(
            (position, river, str(t), str(width)) for river, t, width in index.crossings(start, end)
        )
    return ground_mark(
        course,
        layers["terrain"],
        layers["land"],
        layers["elevation"],
        layers["fertility"],
        crossings=tuple(crossings),
    )


def input_files(map_dir: Path, scenario: Path, *, include_seed: bool = False) -> dict[str, Path]:
    """Name every file that determines a proposal and its stored-ID state."""
    result = {
        "hand": scenario / "network.toml",
        "locations": scenario / "locations.toml",
        "baseline": scenario / "courses-approved.json",
        "stored": scenario / "roads.toml",
        "map": map_dir / "map.toml",
        "terrain_rules": map_dir / "terrain.toml",
        "terrain": map_dir / "terrain.png",
        "elevation": map_dir / "elevation.png",
        "fertility": map_dir / "fertility.png",
        "rivers": map_dir / "rivers.toml",
        "sea_distance": scenario / _SEA_LAYER,
    }
    if include_seed:
        result["seed"] = scenario / "network-seed.json"
    return result


def load_context(map_dir: Path, scenario: Path) -> dict[str, Any]:
    """Read the hand list, scenario and map layers used by all commands."""
    layers = read_rasters(map_dir)
    hand = tomllib.loads((scenario / "network.toml").read_text(encoding="utf-8"))
    hand.setdefault("link", [])
    places = read_locations(scenario / "locations.toml", layers["origin"])
    validate_hand_list(hand, places, layers["land"], layers["origin"])
    terrain_rules = tomllib.loads((map_dir / "terrain.toml").read_text(encoding="utf-8"))["terrain"]
    terrain_names_expected = {
        entry["name"] for entry in terrain_rules if entry["kind"] not in {"sea", "lake"}
    }
    terrain_names_given = set(hand["costs"]["terrain"])
    if terrain_names_expected != terrain_names_given:
        raise ValueError(
            f"costs terrain: unknown or missing keys {sorted(terrain_names_expected ^ terrain_names_given)}"
        )
    rivers = tomllib.loads((map_dir / "rivers.toml").read_text(encoding="utf-8"))["stretch"]
    index = RiverIndex(rivers, layers["origin"])
    costs = hand["costs"]
    ground_costs = terrain_costs(
        layers["terrain"],
        layers["fertility"],
        {k: str(v) for k, v in costs["terrain"].items()},
        str(costs["fertile"]),
    )
    options = {
        "reuse": round(Fraction(str(costs["reuse"])) * _COST_SCALE),
        "slope": round(Fraction(str(costs["slope"])) * _COST_SCALE),
        "elevation": layers["elevation"],
        "crossing_fn": crossing_cost(
            index, str(costs["crossing_base"]), str(costs["crossing_per_m"])
        ),
    }
    return {
        "layers": layers,
        "hand": hand,
        "places": places,
        "rivers": rivers,
        "index": index,
        "costs": ground_costs,
        "options": options,
        "hashes": {
            "terrain": hashlib.sha256((map_dir / "terrain.png").read_bytes()).hexdigest(),
            "rivers": hashlib.sha256((map_dir / "rivers.toml").read_bytes()).hexdigest(),
        },
    }


def approved_seed_points(
    seed: dict[str, Any],
    hand_links: list[dict[str, Any]],
    resolved: list[dict[str, Any]],
    seed_links: dict[str, dict[str, Any]],
) -> dict[str, list[Cell]]:
    """Read immutable waypoint provenance rather than inferring it from the current hand list."""
    stored = seed.get("approved_points")
    hand_ids = {link["id"] for link in hand_links}
    expected = set(seed_links)
    if not isinstance(stored, dict) or set(stored) != expected or not expected <= hand_ids:
        raise ValueError("seed approved_points must name every seed link")
    if [link["id"] for link in hand_links] != [link["id"] for link in resolved]:
        raise ValueError("hand list and resolved links differ in order")
    result = {}
    for name, values in stored.items():
        if (
            not isinstance(values, list)
            or len(values) < _TWO
            or any(
                not isinstance(cell, list)
                or len(cell) != _TWO
                or any(type(v) is not int for v in cell)
                for cell in values
            )
        ):
            raise ValueError(f"seed {name}: approved points must be cell pairs")
        result[name] = [(cell[0], cell[1]) for cell in values]
    return result


def seed_resolution(
    seed: dict[str, Any], resolved: list[dict[str, Any]], points: dict[str, list[Cell]]
) -> list[dict[str, Any]]:
    """Use the approved seed's parent links and joins, even after hand-list edits."""
    joins = seed.get("approved_joins")
    if not isinstance(joins, dict) or set(joins) != set(points):
        raise ValueError("seed approved_joins must name every seed link")
    result = []
    for route in resolved:
        name = route["id"]
        if name not in points:
            continue
        entries = joins[name]
        if not isinstance(entries, list):
            raise ValueError(f"seed {name}: approved joins must be a list")
        join_ends = []
        seen = set()
        for entry in entries:
            if (
                not isinstance(entry, dict)
                or set(entry) != {"end", "parent", "cell"}
                or entry["end"] not in {"from", "to"}
                or entry["end"] in seen
            ):
                raise ValueError(f"seed {name}: invalid approved join")
            cell = entry["cell"]
            if (
                not isinstance(cell, list)
                or len(cell) != _TWO
                or any(type(value) is not int for value in cell)
            ):
                raise ValueError(f"seed {name}: invalid approved join cell")
            seen.add(entry["end"])
            join_ends.append((entry["end"], entry["parent"], (cell[0], cell[1])))
        result.append({"id": name, "points": points[name], "join_ends": join_ends})
    return result


def make_seed_baseline(context: dict[str, Any], seed_document: dict[str, Any]) -> dict[str, Any]:
    """Import the approved seed and record completions and singleton relocations."""
    hand, layers, places, index = (
        context["hand"],
        context["layers"],
        context["places"],
        context["index"],
    )
    raw = {link["id"]: link for link in seed_document["links"]}
    if len(raw) != len(seed_document["links"]):
        raise ValueError("seed contains duplicate link IDs")
    resolved = resolve_links(hand, places, layers["origin"])
    points = approved_seed_points(seed_document, hand["link"], resolved, raw)
    courses = {name: [tuple(cell) for cell in entry["cells"]] for name, entry in raw.items()}
    seed_routes = seed_resolution(seed_document, resolved, points)
    baseline, completions = import_seed(
        seed_routes, courses, layers["land"], approved_points=points
    )
    overrides: dict[str, dict[str, list[int]]] = {}
    hand_by_id = {link["id"]: link for link in hand["link"]}
    for completion in completions:
        name = completion["link"]
        if len(courses[name]) != 1:
            continue
        entry = hand_by_id[name]
        place_from = isinstance(entry["from"], str)
        place = entry["from"] if place_from else entry["to"]
        if (
            not isinstance(place, str)
            or points[name][0 if place_from else -1] != completion["from"]
        ):
            raise ValueError(f"link {name}: singleton relocation has no matching seed place")
        overrides[place] = {"from": list(completion["from"]), "to": list(completion["added"])}
        baseline[name]["original_cell"] = list(completion["from"])
    for entry in baseline.values():
        entry["ground_mark"] = course_ground_mark(entry["course"], layers, index)
    return {"links": baseline, "completions": completions, "place_overrides": overrides}


def apply_place_overrides(
    places: dict[str, Cell], overrides: dict[str, dict[str, list[int]]]
) -> dict[str, Cell]:
    """Apply a seed relocation only while the authored place remains at its original cell."""
    result = dict(places)
    for name, entry in overrides.items():
        if name in result and result[name] == tuple(entry["from"]):
            result[name] = (entry["to"][0], entry["to"][1])
    return result


def plan_network(
    context: dict[str, Any],
    baseline: dict[str, Any],
    previous: dict[str, Any] | None,
    hand_hash: str,
    *,
    recalculate: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Plan a whole graph from the baseline without writing any input or output."""
    hand, layers, index = context["hand"], context["layers"], context["index"]
    places = apply_place_overrides(context["places"], baseline.get("place_overrides", {}))
    validate_hand_list(hand, places, layers["land"], layers["origin"])
    resolved = resolve_links(hand, places, layers["origin"])
    marks = {}
    for name, entry in baseline["links"].items():
        try:
            marks[name] = course_ground_mark(
                [tuple(cell) for cell in entry["course"]], layers, index
            )
        except IndexError:
            marks[name] = "outside crop"
    planning = propose_courses(
        resolved,
        baseline["links"],
        layers["land"],
        context["costs"],
        ground_marks=marks,
        recalculate=recalculate,
        route_options=context["options"],
    )
    result: dict[str, Any] = {
        **planning,
        "baseline": json.loads(json_bytes(baseline)),
        "graph": None,
        "report": "",
        "resolved": resolved,
        "initial": previous is None,
    }
    prospective = result["baseline"]["links"]
    for obsolete in set(prospective) - {link["id"] for link in hand["link"]}:
        del prospective[obsolete]
    if not planning["complete"]:
        return result
    resolved_by_id = {link["id"]: link for link in resolved}
    for name, route in resolved_by_id.items():
        if name in prospective and name not in planning["candidates"]:
            prospective[name]["joins"] = [
                {"end": end, "parent": parent, "cell": list(cell)}
                for end, parent, cell in route["join_ends"]
            ]
    for name, course in planning["candidates"].items():
        prospective[name] = {
            "course": course,
            "points": resolved_by_id[name]["points"],
            "joins": [
                {"end": end, "parent": parent, "cell": cell}
                for end, parent, cell in resolved_by_id[name]["join_ends"]
            ],
            "ground_mark": course_ground_mark(course, layers, index),
            "costs": hand["costs"],
            "record": "pending acceptance",
        }
    courses = {
        name: [tuple(cell) for cell in entry["course"]] for name, entry in prospective.items()
    }
    access = {
        entry["place"]: (
            entry["quality"],
            next(
                tuple(landing["cell"])
                for landing in hand.get("landing", [])
                if landing["place"] == entry["place"]
            ),
        )
        for entry in hand.get("access", [])
    }
    completions = []
    for completion in baseline.get("completions", []):
        course = courses.get(completion["link"], [])
        added = tuple(completion["added"])
        completions.append(
            {**completion, "superseded": not course or added not in (course[0], course[-1])}
        )
    completion_by_link = {item["link"]: item for item in completions}
    links = []
    for link in hand["link"]:
        metadata = {**prospective[link["id"]], "starting_state": [link["category"], link["level"]]}
        if link["id"] in completion_by_link:
            metadata["seed_completion"] = completion_by_link[link["id"]]
        links.append(
            {
                **link,
                "record": json.dumps(
                    metadata, sort_keys=True, separators=(",", ":"), ensure_ascii=False
                ),
            }
        )
    baseline_hash = hashlib.sha256(json_bytes(result["baseline"])).hexdigest()
    graph = make_graph(
        places,
        access,
        links,
        courses,
        hand.get("road", []),
        layers["origin"],
        hand_hash,
        baseline_hash,
        previous=uncut_previous(previous)
        if previous and previous.get("format") in _CUT_FORMATS
        else previous,
    )
    course_graph = copy.deepcopy(graph)
    crossing_hits = apply_crossings(
        graph,
        index,
        hand.get("crossing", []),
        layers["origin"],
        {river["river"] for river in context["rivers"]},
        previous=previous,
    )
    result["graph"] = graph
    water = plan_water(graph, context, crossing_hits, previous)
    store_water(graph, water, context, context["hashes"])
    graph["cell_side_mm"] = cell_side_lengths_mm(layers["origin"][1], len(layers["land"]))
    result["layer"] = water["layer"]
    result["report"] = network_report(
        graph,
        hand["link"],
        completions,
        index,
        layers["origin"],
        proposed=bool(planning["candidates"]) or previous is None,
        course_graph=course_graph,
        crossing_entries=hand.get("crossing", []),
        crossing_hits=crossing_hits,
        water=water,
    )
    return result


def furthest_from_course(new: list[Cell], old: list[Cell]) -> tuple[float, tuple[float, float]]:
    """Estimate maximum separation by sampling the new line at most 0.1 cell apart."""
    if not new or not old:
        raise ValueError("courses must be nonempty")

    def distance(point: tuple[float, float], a: Cell, b: Cell) -> float:
        dx, dy = b[0] - a[0], b[1] - a[1]
        denom = dx * dx + dy * dy
        t = (
            min(1.0, max(0.0, ((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / denom))
            if denom
            else 0.0
        )
        return math.hypot(point[0] - a[0] - t * dx, point[1] - a[1] - t * dy)

    old_segments = list(pairwise(old)) or [(old[0], old[0])]
    points: list[tuple[float, float]] = [(float(new[0][0]), float(new[0][1]))]
    for a, b in pairwise(new):
        steps = max(1, math.ceil(math.dist(a, b) * 10))
        points.extend(
            (a[0] + (b[0] - a[0]) * i / steps, a[1] + (b[1] - a[1]) * i / steps)
            for i in range(1, steps + 1)
        )
    return max((min(distance(point, a, b) for a, b in old_segments), point) for point in points)


def completion_artifacts(
    image: Any,  # noqa: ANN401  # Pillow is loaded only for rendering.
    drawing: Any,  # noqa: ANN401  # Pillow is loaded only for rendering.
    hand: dict[str, Any],
    baseline: dict[str, Any],
    tag: str,
) -> tuple[dict[str, bytes], list[str]]:
    """Render each completed seed step with its parent and a readable caption."""
    output_images: dict[str, bytes] = {}
    entries = {item["id"]: item for item in hand["link"]}
    records: list[str] = []
    for number, completion in enumerate(baseline.get("completions", []), 1):
        name = completion["link"]
        if name not in entries or name not in baseline["links"]:
            continue
        old_joins = baseline["links"][name].get("joins", [])
        if not old_joins or old_joins[0]["parent"] not in baseline["links"]:
            continue
        parent = old_joins[0]["parent"]
        parent_line = [tuple(cell) for cell in baseline["links"][parent]["course"]]
        child_line = [tuple(cell) for cell in baseline["links"][name]["course"]]
        original, added = tuple(completion["from"]), tuple(completion["added"])
        view = image.copy()
        pen = drawing.Draw(view)
        if len(parent_line) > 1:
            pen.line(parent_line, fill=(255, 255, 255), width=2)
        if len(child_line) > 1:
            pen.line(child_line, fill=(255, 205, 0), width=2)
        pen.line((original, added), fill=(255, 45, 58), width=2)
        x, y = added
        left, top = max(0, x - 18), max(0, y - 18)
        right, bottom = min(view.width, x + 19), min(view.height, y + 19)
        view = view.crop((left, top, right, bottom)).resize(
            ((right - left) * 12, (bottom - top) * 12)
        )
        pen = drawing.Draw(view)
        pen.rectangle((0, 0, view.width, 35), fill=(0, 0, 0))
        pen.text((5, 3), name + " → " + parent, fill=(255, 255, 255))
        pen.text((5, 18), f"{original} → {added}; red = added step", fill=(255, 255, 255))
        stream = io.BytesIO()
        view.save(stream, format="PNG")
        filename = f"network-pictures/{tag}/completion-{number:03}.png"
        output_images[filename] = stream.getvalue()
        records.append(f"- [{name}]({filename}): {original} → {added} on {parent}")
    return output_images, records


def proposal_artifacts(  # noqa: PLR0915, PLR0912  # Rendering gathers all candidate and blocked evidence.
    context: dict[str, Any], baseline: dict[str, Any], plan: dict[str, Any], tag: str
) -> tuple[dict[str, bytes], str]:
    """Render the proposed and blocked lines and their append-only change entry."""
    image_module = importlib.import_module("PIL.Image")
    drawing = importlib.import_module("PIL.ImageDraw")
    with image_module.open(context["map_dir"] / "terrain.png") as source:
        image = source.convert("RGB")
    draw = drawing.Draw(image)
    origin = context["layers"]["origin"]
    for river in context["rivers"]:
        points = [project_point(lat, lon, origin) for lat, lon in river["points"]]
        if len(points) > 1:
            draw.line(
                [(x / 1_000_000, y / 1_000_000) for x, y in points], fill=(29, 164, 236), width=1
            )
    before = {
        name: [tuple(cell) for cell in entry["course"]] for name, entry in baseline["links"].items()
    }
    after = dict(before)
    after.update(plan["candidates"])
    artifacts: dict[str, bytes] = {}
    log = [
        f"\n## Proposal {tag}",
        "",
        "Yellow: approved; red: candidate; blue: river; magenta: completion.",
    ]
    whole = image.copy()
    canvas = drawing.Draw(whole)
    for name, course in after.items():
        if name not in plan["blocked"] and len(course) > 1:
            canvas.line(course, fill=(245, 230, 180), width=1)
    for completion in baseline.get("completions", []):
        x, y = completion["added"]
        canvas.ellipse((x - 1, y - 1, x + 1, y + 1), fill=(255, 90, 255))
    for number, name in enumerate(sorted(plan["candidates"]), 1):
        old = before.get(name, [])
        new = plan["candidates"][name]
        crop = image.copy()
        for target in (canvas, drawing.Draw(crop)):
            if len(old) > 1:
                target.line(old, fill=(255, 205, 0), width=2)
            if len(new) > 1:
                target.line(new, fill=(255, 45, 58), width=2)
        cells = old + new
        left, top = max(0, min(x for x, _ in cells) - 25), max(0, min(y for _, y in cells) - 25)
        right, bottom = (
            min(crop.width, max(x for x, _ in cells) + 26),
            min(crop.height, max(y for _, y in cells) + 26),
        )
        crop = crop.crop((left, top, right, bottom))
        scale = min(5, max(1, 1200 // max(crop.size)))
        crop = crop.resize((crop.width * scale, crop.height * scale))
        output = io.BytesIO()
        crop.save(output, format="PNG")
        filename = f"network-pictures/{tag}/route-{number:03}.png"
        artifacts[filename] = output.getvalue()
        if old:
            furthest, location = furthest_from_course(new, old)
            delta = f"{ground_length(old, origin) / 1000:.3f} → {ground_length(new, origin) / 1000:.3f} km; furthest approximately {location}, {furthest:.2f} cells (sampled at most 0.1 cell apart)"
        else:
            delta = f"new course, {ground_length(new, origin) / 1000:.3f} km"
        why = "; ".join(plan["reasons"].get(name, []))
        log.append(f"- [{name}]({filename}): {why}; {delta}")
        old_joins = [
            {**join, "cell": list(join["cell"])}
            for join in baseline["links"].get(name, {}).get("joins", [])
        ]
        route = next(item for item in plan.get("resolved", []) if item["id"] == name)
        new_joins = [
            {"end": end, "parent": parent, "cell": list(cell)}
            for end, parent, cell in route["join_ends"]
        ]
        if old_joins != new_joins:
            log.append(f"  - Joins: {old_joins} → {new_joins}")
    resolved_by_id = {entry["id"]: entry for entry in plan.get("resolved", [])}
    for name, reason in sorted(plan["blocked"].items()):
        old = before.get(name, [])
        if old and not reason.startswith("waiting for") and len(old) > 1:
            canvas.line(old, fill=(255, 90, 255), width=2)
        for x, y in (resolved_by_id[name]["points"][0], resolved_by_id[name]["points"][-1]):
            canvas.ellipse((x - 2, y - 2, x + 2, y + 2), fill=(255, 90, 255))
        log.append(f"- Blocked {name}: {reason}")
    if plan.get("initial"):
        completion_images, completion_records = completion_artifacts(
            image, drawing, context["hand"], baseline, tag
        )
        artifacts.update(completion_images)
        if completion_records:
            log.extend(("", "### Seed completion pictures", *completion_records))
    output = io.BytesIO()
    whole.save(output, format="PNG")
    whole_name = f"network-pictures/{tag}/whole.png"
    artifacts[whole_name] = output.getvalue()
    log.append(f"- [Whole proposal]({whole_name})")
    if plan["complete"]:
        log.extend(("", plan["report"]))
    return artifacts, "\n".join(log) + "\n"


def build_network(  # noqa: PLR0912, PLR0915  # The command stages a proposal before touching persistent state.
    map_dir: Path,
    scenario: Path,
    *,
    importing: bool = False,
    recalculate: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Stage a complete or blocked proposal, or refresh an unchanged stored network."""
    files = input_files(map_dir, scenario, include_seed=importing)
    captured = snapshot(files)
    initial_snapshot = dict(captured)
    if importing:
        if files["baseline"].exists() or files["stored"].exists():
            raise ValueError("import requires no baseline or stored network")
    elif not files["baseline"].exists():
        raise ValueError("no baseline; import first")
    context = load_context(map_dir, scenario)
    context["map_dir"] = map_dir
    if importing:
        baseline = make_seed_baseline(context, json.loads(files["seed"].read_bytes()))
    else:
        baseline = json.loads(files["baseline"].read_bytes())
    previous = (
        tomllib.loads(files["stored"].read_text(encoding="utf-8"))
        if files["stored"].exists()
        else None
    )
    if previous is not None:
        check_stored_integers(previous)
    ids = {link["id"] for link in context["hand"]["link"]}
    if recalculate - ids:
        raise ValueError(f"unknown recalculate link {sorted(recalculate - ids)[0]}")
    hand_hash = captured["hand"]
    if hand_hash is None:
        raise ValueError("hand list is missing")
    plan = plan_network(context, baseline, previous, hand_hash, recalculate=recalculate)
    layer = plan.pop("layer", None)
    pending = scenario / "courses-pending.json"
    report = scenario / "network-report.md"
    log = scenario / "network-changes.md"
    if not snapshot_matches(files, captured):
        raise ValueError("inputs changed during build; retry")
    needs_proposal = previous is None or bool(plan["candidates"]) or bool(plan["blocked"])
    if not needs_proposal:
        baseline_changed = plan["baseline"] != json.loads(json_bytes(baseline))
        if baseline_changed:
            replacement_baseline = json_bytes(plan["baseline"])
            plan["graph"]["baseline_sha256"] = hashlib.sha256(replacement_baseline).hexdigest()
        else:
            replacement_baseline = None
            plan["graph"]["baseline_sha256"] = captured["baseline"]
        road_bytes = dump_roads(plan["graph"]).encode("utf-8")
        outputs: dict[Path, bytes | None] = {
            files["stored"]: road_bytes,
            report: plan["report"].encode("utf-8"),
            pending: None,
        }
        if layer is not None:
            outputs[files["sea_distance"]] = layer
        if replacement_baseline is not None:
            outputs[files["baseline"]] = replacement_baseline
        commit_files(outputs)
        return plan
    baseline_bytes = json_bytes(baseline)
    if importing:
        captured["baseline"] = hashlib.sha256(baseline_bytes).hexdigest()
    plan["inputs"] = captured
    plan["include_seed"] = importing
    artifacts, entry = proposal_artifacts(context, baseline, plan, "staging")
    evidence = {name: hashlib.sha256(data).hexdigest() for name, data in artifacts.items()}
    tag = hashlib.sha256(
        json_bytes({"inputs": captured, "entry": entry, "pictures": evidence})
    ).hexdigest()[:16]
    artifacts = {
        name.replace("network-pictures/staging/", f"network-pictures/{tag}/"): data
        for name, data in artifacts.items()
    }
    entry = entry.replace("network-pictures/staging/", f"network-pictures/{tag}/").replace(
        "## Proposal staging", f"## Proposal {tag}"
    )
    plan["pictures"] = {name: hashlib.sha256(data).hexdigest() for name, data in artifacts.items()}
    plan["seal"] = proposal_seal(plan)
    entry += f"\nProposal seal: {plan['seal']}\n"
    log_bytes = (
        log.read_bytes() if log.exists() else b"# Travel network changes\n"
    ) + entry.encode("utf-8")
    pending_bytes = json_bytes(plan)
    if not snapshot_matches(files, initial_snapshot):
        raise ValueError("inputs changed while rendering; retry")
    if pending.exists() and pending.read_bytes() == pending_bytes:
        if not log.exists() or f"Proposal seal: {plan['seal']}" not in log.read_text():
            raise ValueError("pending proposal has no matching log entry")
        for name, data in artifacts.items():
            if not (scenario / name).is_file() or (scenario / name).read_bytes() != data:
                raise ValueError(f"proposal picture changed: {name}")
        return plan
    if importing:
        atomic_write(files["baseline"], baseline_bytes)
    for name, data in artifacts.items():
        atomic_write(scenario / name, data)
    atomic_write(pending, pending_bytes)
    atomic_write(log, log_bytes)
    return plan


def verify_stored_courses(graph: dict[str, Any], baseline: dict[str, Any]) -> None:
    """Independently compare each stored link walk with its baseline course."""
    stretches = {item["id"]: item for item in graph.get("stretch", [])}
    nodes = {item["id"]: item for item in graph.get("node", [])}
    for link in graph.get("link", []):
        name = link["id"]
        if name not in baseline["links"]:
            raise ValueError(f"stored network link {name}: no baseline")
        cells: list[list[int]] = []
        try:
            for sid, direction in link["stretches"]:
                stretch = stretches[sid]
                if stretch["tidal"] or direction not in {"forward", "backward"}:
                    raise ValueError(
                        f"stored network link {name}: invalid stretch direction or tidal stretch"
                    )
                part = stretch["cells"] if direction == "forward" else stretch["cells"][::-1]
                if cells and cells[-1] != part[0]:
                    raise ValueError(f"stored network link {name}: disconnected stretches")
                cells.extend(part[1:] if cells else part)
            if not cells:
                cells = [nodes[link["from_node"]]["cell"]]
        except (KeyError, IndexError) as error:
            raise ValueError(f"stored network link {name}: missing node or stretch") from error
        if cells != baseline["links"][name]["course"]:
            raise ValueError(f"stored network link {name}: course differs from baseline")


# What the loader reads of a stored network, field by field (`crates/map/src/network.rs` and `water.rs`): a spec is ("int",),
# ("str",), ("bool",), ("tuple", (specs...)), ("list", spec) or ("dict", {field: spec}).
_STR, _BOOL = ("str",), ("bool",)
_U8, _U32, _U64 = ("uint", 8), ("uint", 32), ("uint", 64)
_I32, _I64 = ("sint", 32), ("sint", 64)
_CELL = ("tuple", (_I32, _I32))
_STORED_TOP: dict[str, tuple[Any, ...]] = {
    "format": _U32,
    "hand_list_sha256": _STR,
    "baseline_sha256": _STR,
    "next_node_id": _U64,
    "next_stretch_id": _U64,
    "cell_side_mm": ("list", _U64),
    "river_width_mm": _I64,
    "terrain_sha256": _STR,
    "rivers_sha256": _STR,
    "sea_distance": _STR,
    "sea_distance_sha256": _STR,
    "next_landing_id": _U64,
}
_STORED_TABLES: dict[str, dict[str, tuple[Any, ...]]] = {
    "node": {"id": _U64, "kind": _STR, "cell": _CELL, "place": _STR},
    "stretch": {
        "id": _U64,
        "from": _U64,
        "to": _U64,
        "cells": ("list", _CELL),
        "length_m": _U64,
        "category": _STR,
        "level": _U8,
        "tidal": _BOOL,
        "crossings": (
            "list",
            ("dict", {"river": _STR, "width_tenths_m": _U64, "kind": _STR}),
        ),
        "wades": (
            "list",
            ("dict", {"river": _STR, "width_tenths_m": _U64, "along_m": _U64}),
        ),
        "landing_from": _U64,
        "landing_to": _U64,
    },
    "link": {
        "id": _STR,
        "road": _STR,
        "from_node": _U64,
        "to_node": _U64,
        "stretches": ("list", ("tuple", (_U64, _STR))),
        "record": _STR,
    },
    "join": {"link": _STR, "end": _STR, "parent": _STR, "node": _U64, "along_m": _U64},
    "road": {"id": _STR, "name": _STR, "links": ("list", _STR)},
    "access": {"place": _STR, "quality": _STR, "landing": _U64},
    "water_class": {"id": _STR, "reach_m": _U32, "sea_going": _BOOL, "bodies": _U64},
    "river_break": {
        "name": _STR,
        "stretch": _STR,
        "cell": _CELL,
        "above": _U64,
        "below": _U64,
    },
    "mark": {"place": _STR},
    "joint": {"id": _U64, "stretch": _STR, "cell": _CELL},
    "reach": {
        "id": _U64,
        "stretch": _STR,
        "from": _U64,
        "to": _U64,
        "usable": _BOOL,
        "length_mm": _U64,
        "kind": _STR,
        "begin": _U32,
        "end": _U32,
    },
    "water_join": {
        "reach": _U64,
        "to_reach": _U64,
        "cell": _CELL,
        "classes": ("list", _STR),
    },
    "landing": {
        "id": _U64,
        "node": _U64,
        "bank": _STR,
        "survey": _STR,
        "shore": _CELL,
        "run": _U32,
        "cell": _CELL,
        "joint": _U64,
        "crossing": _U64,
        "end": _STR,
        "approach_m": _U64,
        "bodies": ("list", _I64),
    },
    "strait": {
        "place": _STR,
        "from": _U64,
        "to": _U64,
        "length_m": _U64,
        "classes": ("list", _STR),
    },
}
_KIND_NAMES = {"str": "a string", "bool": "a boolean"}


def _spec_problem(  # noqa: PLR0911, PLR0912  # One walk over the kinds of stored value.
    value: object, spec: tuple[Any, ...], path: str
) -> str | None:
    """Why a stored value is not what its spec says (naming the part and the type found), or None."""
    found = type(value).__name__
    kind = spec[0]
    if kind in {"uint", "sint"}:
        if not isinstance(value, int) or isinstance(value, bool):
            return f"{path} is {found}, not an integer"
        bits = spec[1]
        low, high = (0, 2**bits) if kind == "uint" else (-(2 ** (bits - 1)), 2 ** (bits - 1))
        if not low <= value < high:
            name = f"{'an unsigned' if kind == 'uint' else 'a signed'} {bits}-bit integer"
            return f"{path} is {value}, not {name}"
        return None
    if kind == "str":
        ok = isinstance(value, str)
    elif kind == "bool":
        ok = isinstance(value, bool)
    elif kind == "tuple":
        if not isinstance(value, list) or len(value) != len(spec[1]):
            return f"{path} is {found}, not a list of {len(spec[1])}"
        for number, (item, part) in enumerate(zip(value, spec[1], strict=True)):
            problem = _spec_problem(item, part, f"{path}[{number}]")
            if problem:
                return problem
        return None
    elif kind == "list":
        if not isinstance(value, list):
            return f"{path} is {found}, not a list"
        for number, item in enumerate(value):
            problem = _spec_problem(item, spec[1], f"{path}[{number}]")
            if problem:
                return problem
        return None
    else:
        if not isinstance(value, dict):
            return f"{path} is {found}, not a table"
        for name, part in spec[1].items():
            if name in value:
                problem = _spec_problem(value[name], part, f"{path}.{name}")
                if problem:
                    return problem
        for name in value:
            if name not in spec[1]:
                return f"{path}.{name} is not a stored field"
        return None
    return None if ok else f"{path} is {found}, not {_KIND_NAMES[kind]}"


def check_stored_integers(graph: dict[str, Any]) -> None:
    """Refuse a stored network whose values are not what the loader reads, naming the table, entry, field and type found.

    Everything that reads the stored network afterwards (the id allocators, the keys that hash entries, the course checks, the
    comparison of entries) can take its numbers as numbers and its lists as lists: a string, an array, a table, a date, a float
    of a whole value or a boolean where an integer belongs stops here.
    """
    for field in graph:
        if field not in _STORED_TOP and field not in _STORED_TABLES:
            raise ValueError(f"stored network {field} is not a stored field")
    for field, spec in _STORED_TOP.items():
        if field in graph:
            problem = _spec_problem(graph[field], spec, field)
            if problem:
                raise ValueError(f"stored network {problem}")
    for table, fields in _STORED_TABLES.items():
        entries = graph.get(table, [])
        if not isinstance(entries, list):
            raise ValueError(
                f"stored network {table} is {type(entries).__name__}, not a table list"
            )
        for number, entry in enumerate(entries):
            if not isinstance(entry, dict):
                raise ValueError(f"stored network {table} entry {number} is not a table")
            problem = _spec_problem(entry, ("dict", fields), "")
            if problem:
                raise ValueError(f"stored network {table} entry {number}: {problem.lstrip('.')}")
    _refuse_dates(graph, "")


def _refuse_dates(value: object, path: str) -> None:
    """Refuse a TOML date or time anywhere in a stored network: the comparison and the writer carry plain values only."""
    if isinstance(value, datetime.date | datetime.time):
        raise ValueError(
            f"stored network {path.lstrip('.')} is {type(value).__name__}, a date or time"
        )
    if isinstance(value, dict):
        for name, item in value.items():
            _refuse_dates(item, f"{path}.{name}")
    elif isinstance(value, list):
        for number, item in enumerate(value):
            _refuse_dates(item, f"{path}[{number}]")


_GRAPH_TABLES = (
    "node",
    "stretch",
    "link",
    "join",
    "road",
    "access",
    "water_class",
    "river_break",
    "mark",
    "joint",
    "reach",
    "water_join",
    "landing",
    "strait",
)
_IDENTITY = {
    "join": ("link", "end"),
    "access": ("place",),
    "river_break": ("name",),
    "mark": ("place",),
    "water_class": ("id",),
    "water_join": ("reach", "to_reach", "cell"),
    "strait": ("from", "to"),
}


def _part_order(part: Any) -> tuple[Any, ...]:  # noqa: ANN401  # A stored identity part may be of any JSON type.
    """A total order over the kinds of value a stored identity part can hold, so a malformed one still sorts."""
    if part is None:
        return (0, 0, "")
    if isinstance(part, bool):
        return (1, int(part), "")
    if isinstance(part, int | float):
        return (2, part, "")
    if isinstance(part, str):
        return (3, 0, part)
    if isinstance(part, tuple):
        return (4, 0, tuple(_part_order(item) for item in part))
    return (5, 0, repr(part))


def _identity_order(key: tuple[Any, ...]) -> tuple[Any, ...]:
    """A stable order for identities: an absent part sorts first, and a part of the wrong type sorts after the right ones."""
    return tuple(_part_order(part) for part in key)


def graph_difference(expected: dict[str, Any], actual: dict[str, Any]) -> str:
    """Name the first changed table entry or top-level field."""
    for table in _GRAPH_TABLES:
        fields = _IDENTITY.get(table, ("id",))

        def identity(
            item: dict[str, Any], field_names: tuple[str, ...] = fields
        ) -> tuple[Any, ...]:
            return tuple(
                tuple(item[key]) if isinstance(item.get(key), list) else item.get(key)
                for key in field_names
            )

        try:
            left = {identity(item): item for item in expected.get(table, [])}
            right = {identity(item): item for item in actual.get(table, [])}
        except (KeyError, TypeError) as error:
            return f"stored network {table}: malformed entry ({error})"
        if len(right) != len(actual.get(table, [])):
            return f"stored network {table}: repeated identity"
        for key in sorted(set(left) | set(right), key=_identity_order):
            if json_bytes(left.get(key)) != json_bytes(right.get(key)):
                return f"stored network {table} {'/'.join(map(str, key))} differs"
    for field in sorted(set(expected) | set(actual)):
        if field not in _GRAPH_TABLES and json_bytes(expected.get(field)) != json_bytes(
            actual.get(field)
        ):
            return f"stored network {field} differs"
    return "stored network serialization differs"


def check_layer(
    files: dict[str, Path], context: dict[str, Any], expected: bytes, stored: dict[str, Any]
) -> None:
    """Refuse a sea distance layer that is missing, stale, malformed or changed in any value."""
    path = files["sea_distance"]
    if not path.exists():
        raise ValueError(f"sea distance layer {path.name} is missing")
    actual = path.read_bytes()
    layers = context["layers"]
    height, width = len(layers["kind"]), len(layers["kind"][0])
    values = read_sea_distance(actual, width, height)
    wanted = sea_distance_layer(layers["kind"], layers["origin"])
    for y, (left, right) in enumerate(zip(values, wanted, strict=True)):
        for x, (a, b) in enumerate(zip(left, right, strict=True)):
            if a != b:
                raise ValueError(f"sea distance layer cell ({x}, {y}) is {a}, not {b}")
    for field, name, digest in (
        ("terrain_sha256", "terrain.png", context["hashes"]["terrain"]),
        ("rivers_sha256", "rivers.toml", context["hashes"]["rivers"]),
        ("sea_distance_sha256", path.name, hashlib.sha256(actual).hexdigest()),
    ):
        if stored.get(field) != digest:
            raise ValueError(f"stored network {field} is not the sha256 of {name}")
    if actual != expected:
        raise ValueError("sea distance layer differs from the one built from the terrain")


def check_network(map_dir: Path, scenario: Path) -> None:
    """Recompute the stored graph and refuse any pending, changed or corrupted state."""
    pending = scenario / "courses-pending.json"
    if pending.exists():
        raise ValueError("a proposal is pending or blocked")
    files = input_files(map_dir, scenario)
    if not files["baseline"].exists() or not files["stored"].exists():
        raise ValueError("baseline or stored network is missing")
    captured = snapshot(files)
    context = load_context(map_dir, scenario)
    baseline = json.loads(files["baseline"].read_bytes())
    previous = tomllib.loads(files["stored"].read_text(encoding="utf-8"))
    check_stored_integers(previous)
    verify_stored_courses(previous, baseline)
    hand_hash = captured["hand"]
    if hand_hash is None:
        raise ValueError("hand list is missing")
    plan = plan_network(context, baseline, previous, hand_hash)
    check_layer(files, context, plan.pop("layer"), previous)
    if plan["baseline"] != baseline:
        raise ValueError("baseline contains obsolete entries")
    if plan["candidates"] or plan["blocked"]:
        raise ValueError(
            f"links need a course: {sorted(set(plan['candidates']) | set(plan['blocked']))}"
        )
    plan["graph"]["baseline_sha256"] = captured["baseline"]
    if dump_roads(plan["graph"]).encode("utf-8") != files["stored"].read_bytes():
        raise ValueError(graph_difference(plan["graph"], previous))
    if not snapshot_matches(files, captured):
        raise ValueError("inputs changed during check")


def main(argv: list[str] | None = None) -> int:  # noqa: PLR0912, PLR0915  # The CLI routes the four state operations and report.
    """Run import, build, accept or check for a scenario."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--map-dir", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument(
        "--scenario",
        type=Path,
        default=Path(__file__).resolve().parents[3] / "assets/scenarios/ireland-768",
    )
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("import")
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--recalculate", action="append", nargs="?", const="*")
    build_parser.add_argument("--all", action="store_true")
    accept_parser = commands.add_parser("accept")
    accept_parser.add_argument("--record", required=True)
    commands.add_parser("check")
    commands.add_parser("report")
    args = parser.parse_args(argv)
    try:
        if args.command == "accept":
            pending = args.scenario / "courses-pending.json"
            if not pending.exists():
                raise ValueError("no proposal")
            document = json.loads(pending.read_bytes())
            files = input_files(
                args.map_dir, args.scenario, include_seed=document.get("include_seed", False)
            )
            accept_pending(
                pending,
                files,
                args.scenario / "network-report.md",
                args.scenario / "network-changes.md",
                args.record,
            )
            print("accepted proposal")
        elif args.command == "check":
            check_network(args.map_dir, args.scenario)
            print("network check passed")
        elif args.command == "report":
            report = args.scenario / "network-report.md"
            if not (args.scenario / "roads.toml").exists() or not report.exists():
                raise ValueError("no stored network report")
            print(report.read_text(encoding="utf-8"), end="")
        else:
            recalculate: frozenset[str] = frozenset()
            if args.command == "build":
                requested = args.recalculate or []
                if args.all:
                    if "*" not in requested:
                        raise ValueError("--all requires --recalculate")
                    document = tomllib.loads(
                        (args.scenario / "network.toml").read_text(encoding="utf-8")
                    )
                    recalculate = frozenset(link["id"] for link in document["link"])
                elif "*" in requested:
                    raise ValueError("--recalculate needs a link ID or --all")
                else:
                    recalculate = frozenset(requested)
            plan = build_network(
                args.map_dir,
                args.scenario,
                importing=args.command == "import",
                recalculate=recalculate,
            )
            names = sorted(set(plan["candidates"]) | set(plan["blocked"]))
            if names or not (args.scenario / "roads.toml").exists():
                print(
                    f"proposal {'complete' if plan['complete'] else 'blocked'}: {', '.join(names) or 'initial network'}",
                    file=sys.stderr,
                )
                return 1
            print("network is up to date")
    except (ValueError, OSError, KeyError, tomllib.TOMLDecodeError) as error:
        print(f"network: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
