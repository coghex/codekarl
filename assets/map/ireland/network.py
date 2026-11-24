"""Travel network geometry and build tooling."""

import argparse
import copy
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


def allocate_ids(
    keys: list[Key], previous: dict[Key, int], next_id: int
) -> tuple[dict[Key, int], int]:
    """Preserve existing IDs and allocate new IDs above the historical high water mark."""
    if len(set(previous.values())) != len(previous) or any(
        value >= next_id for value in previous.values()
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
            next_id += 1
    return result, next_id


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

    keys("network", data, {"costs", "road", "access", "landing", "link", "crossing"})
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
        ),
        "link": ("id", "road", "from_node", "to_node", "stretches", "record"),
        "join": ("link", "end", "parent", "node", "along_m"),
        "road": ("id", "name", "links"),
        "access": ("place", "quality", "landing"),
    }
    top_fields = (
        "format",
        "hand_list_sha256",
        "baseline_sha256",
        "next_node_id",
        "next_stretch_id",
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

    for field in top_fields:
        lines.append(f"{field} = {json.dumps(graph[field], ensure_ascii=True)}")
    for table, fields in table_fields.items():
        sort_keys = (
            ("link", "end") if table == "join" else (("place",) if table == "access" else ("id",))
        )
        for entry in sorted(
            graph.get(table, []), key=lambda item: tuple(item[k] for k in sort_keys)
        ):
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
        [node_key(entry) for entry in entries], old_nodes, previous.get("next_node_id", 1)
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
        stretch_keys, old_stretches, previous.get("next_stretch_id", 1)
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
        a = (start[0] * 1_000_000 + 500_000, start[1] * 1_000_000 + 500_000)
        b = (end[0] * 1_000_000 + 500_000, end[1] * 1_000_000 + 500_000)
        road = (b[0] - a[0], b[1] - a[1])

        def cross(left: Cell, right: Cell) -> int:
            return left[0] * right[1] - left[1] * right[0]

        indices: set[int] = set()
        for x in range(min(start[0], end[0]), max(start[0], end[0]) + 1):
            for y in range(min(start[1], end[1]), max(start[1], end[1]) + 1):
                indices.update(self.bins.get((x, y), ()))
        found: set[tuple[str, Fraction, Fraction]] = set()
        for index in indices:
            p, q, river, before, length, total, w0, w1 = self.segments[index]
            direction = (q[0] - p[0], q[1] - p[1])
            offset = (p[0] - a[0], p[1] - a[1])
            divisor = cross(road, direction)
            if not divisor:
                continue
            t = Fraction(cross(offset, direction), divisor)
            u = Fraction(cross(offset, road), divisor)
            if 0 <= t <= 1 and 0 <= u <= 1:
                width = w0 + (w1 - w0) * (before + u * length) / total
                found.add((river, t, width))
        return sorted(found, key=lambda item: (item[1], item[0], item[2]))


_ROADS_FORMAT = 2
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
                        graph["next_node_id"] += 1
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
                graph["next_stretch_id"] += 1
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
    return "\n".join(lines) + "\n"


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
    if not snapshot_matches(files, proposal["inputs"]):
        raise ValueError("proposal inputs changed during acceptance; build again")
    commit_files(
        {
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
        if previous and previous.get("format") == _ROADS_FORMAT
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
    graph["format"] = _ROADS_FORMAT
    result["graph"] = graph
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
    ids = {link["id"] for link in context["hand"]["link"]}
    if recalculate - ids:
        raise ValueError(f"unknown recalculate link {sorted(recalculate - ids)[0]}")
    hand_hash = captured["hand"]
    if hand_hash is None:
        raise ValueError("hand list is missing")
    plan = plan_network(context, baseline, previous, hand_hash, recalculate=recalculate)
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
        if replacement_baseline is not None:
            atomic_write(files["baseline"], replacement_baseline)
        atomic_write(files["stored"], road_bytes)
        atomic_write(report, plan["report"].encode("utf-8"))
        pending.unlink(missing_ok=True)
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


def graph_difference(expected: dict[str, Any], actual: dict[str, Any]) -> str:
    """Name the first changed table entry or top-level field."""
    for table in ("node", "stretch", "link", "join", "road", "access"):
        fields = (
            ("link", "end") if table == "join" else (("place",) if table == "access" else ("id",))
        )

        def identity(
            item: dict[str, Any], field_names: tuple[str, ...] = fields
        ) -> tuple[Any, ...]:
            return tuple(item[key] for key in field_names)

        try:
            left = {identity(item): item for item in expected.get(table, [])}
            right = {identity(item): item for item in actual.get(table, [])}
        except (KeyError, TypeError) as error:
            return f"stored network {table}: malformed entry ({error})"
        if len(right) != len(actual.get(table, [])):
            return f"stored network {table}: repeated identity"
        for key in sorted(set(left) | set(right)):
            if left.get(key) != right.get(key):
                return f"stored network {table} {'/'.join(map(str, key))} differs"
    for field in sorted(set(expected) | set(actual)):
        if field not in {"node", "stretch", "link", "join", "road", "access"} and expected.get(
            field
        ) != actual.get(field):
            return f"stored network {field} differs"
    return "stored network serialization differs"


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
    verify_stored_courses(previous, baseline)
    hand_hash = captured["hand"]
    if hand_hash is None:
        raise ValueError("hand list is missing")
    plan = plan_network(context, baseline, previous, hand_hash)
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
