"""Writes the FIRST DRAFT of `rivers.toml`: the major, practically useful waterways of Ireland. It reproduces the first draft only.

    python3 derive_rivers.py [--source DIR] [--out DIR] [--force]

The committed `rivers.toml` is this script's output unchanged; any hand edit made after it would be listed in the guesswork log of
`SOURCES.md`, and running this script again would destroy it, so it writes into `--out` (default: next to this script) and refuses to
overwrite `rivers.toml` unless `--force` is given. It reads `elevation.png` as `derive_elevation.py` writes it, so run it before
`apply_edits.py`, which changes the elevation (sea paint) and reads the rivers. Needs `numpy`, `Pillow`, `elevation.png` and `map.toml` beside it, and the Natural Earth river files, which
are fetched into `--source` (default `ireland-source`) when missing and are not committed.

Each river is one line of latitude/longitude points, source first, in `RIVERS`:

* **Natural Earth** (public domain; `ne_10m_rivers_lake_centerlines`, `ne_10m_rivers_europe`) where it has the river: its pieces are
  joined, ordered from the higher end to the lower, simplified to `NE_ANCHOR_CELLS`, and the vertices that remain are used as anchors
  exactly as below, because its lines are coarse (a straight stretch of 10 km or more). It has the Shannon (to Limerick), the Barrow,
  the Suir and the Lower Bann.
* **Placed by hand, snapped to the valley** for the others: a few anchors per river (towns and bridges on it, in order from the source to
  the mouth, approximate to a kilometre or two) are each moved to the cell of the greatest drainage area within `SNAP_RADIUS` cells, and
  the line between two anchors follows the downhill path of the elevation grid (`elevation.png`, with depressions filled), which is
  where the valley floor is at 500 m resolution. Where that path does not reach the next anchor, the cheapest path between them over
  the grid is used, a step costing less where the drainage area is greater (the river is marked in the report); if even that fails, a
  straight line joins them.

OpenStreetMap geometry is not used (its licence is share-alike, ODbL). The lines are simplified with a tolerance of
`SIMPLIFY_CELLS` cells.
"""

from __future__ import annotations

import argparse
import heapq
import json
import math
from pathlib import Path
from typing import NamedTuple

import numpy as np
from ireland_grid import Array, Crop, download
from numpy.typing import NDArray
from PIL import Image

HERE = Path(__file__).resolve().parent

ELEVATION_OFFSET = 11_000
NATURAL_EARTH = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/{name}.geojson"
)
SNAP_RADIUS = 4
SIMPLIFY_CELLS = 0.6
# The vertices of a Natural Earth line, simplified to this tolerance in cells, are the anchors of the snapped line.
NE_ANCHOR_CELLS = 1.5
# A downhill path reaches an anchor when it passes within this many cells of it.
REACH_CELLS = 3
# The cheapest-path fallback searches this many cells around the two anchors, and a cell with no drainage costs 1 / CHANNEL_FLOOR a step.
WINDOW_CELLS = 12
CHANNEL_FLOOR = 0.15
NEIGHBOURS = ((-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1))

Point = tuple[float, float]


class River(NamedTuple):
    """One river: its name, and where its line comes from."""

    name: str
    # (file, name of the feature) of a Natural Earth source, or None.
    natural_earth: tuple[str, str] | None = None
    # Anchors (latitude, longitude), source first, for a river placed by hand.
    anchors: tuple[Point, ...] = ()
    note: str = ""
    # (south, north) latitudes: the pieces of the Natural Earth feature that lie in this band (a feature can hold two rivers).
    band: tuple[float, float] = (-90.0, 90.0)


RIVERS = (
    River(
        "Shannon",
        natural_earth=("ne_10m_rivers_lake_centerlines", "Shannon"),
        note="Natural Earth ends at Limerick.",
    ),
    River(
        "Erne",
        anchors=(
            (53.985, -7.51),
            (54.103, -7.446),
            (54.19, -7.56),
            (54.344, -7.636),
            (54.478, -8.092),
            (54.5, -8.19),
            (54.48, -8.26),
        ),
        note="Killeshandra, Belturbet, Upper and Lower Lough Erne, Belleek, Ballyshannon.",
    ),
    River(
        "Upper Bann",
        anchors=(
            (54.19, -6.16),
            (54.30, -6.19),
            (54.349, -6.27),
            (54.374, -6.338),
            (54.425, -6.44),
            (54.5, -6.43),
        ),
        note="Hilltown, Katesbridge, Banbridge, Gilford, Portadown, Lough Neagh.",
    ),
    River(
        "Lower Bann",
        natural_earth=("ne_10m_rivers_europe", "Bann"),
        note="Lough Neagh to the sea at Coleraine; the same Natural Earth feature holds the Ulster Blackwater, which is left out.",
        band=(54.65, 90.0),
    ),
    River(
        "Foyle",
        anchors=((54.826, -7.466), (54.935, -7.40), (54.995, -7.32), (55.055, -7.255)),
        note="Strabane, Derry, Lough Foyle.",
    ),
    River(
        "Moy",
        anchors=((53.938, -8.951), (53.98, -9.13), (54.115, -9.155), (54.21, -9.17)),
        note="Swinford, Foxford, Ballina, Killala Bay.",
    ),
    River(
        "Corrib",
        anchors=((53.308, -9.118), (53.285, -9.067), (53.269, -9.052)),
        note="Lough Corrib to Galway Bay.",
    ),
    River(
        "Boyne",
        anchors=(
            (53.384, -6.99),
            (53.416, -6.99),
            (53.555, -6.79),
            (53.652, -6.683),
            (53.711, -6.562),
            (53.715, -6.35),
            (53.725, -6.24),
        ),
        note="Carbury, Clonard, Trim, Navan, Slane, Drogheda.",
    ),
    River(
        "Liffey",
        anchors=(
            (53.17, -6.35),
            (53.14, -6.50),
            (53.17, -6.53),
            (53.13, -6.75),
            (53.31, -6.60),
            (53.37, -6.49),
            (53.347, -6.34),
            (53.346, -6.20),
        ),
        note="Kippure, Blessington, Kilcullen, Straffan, Leixlip, Dublin.",
    ),
    River(
        "Barrow",
        natural_earth=("ne_10m_rivers_europe", "Barrow"),
        note="Slieve Bloom to Waterford Harbour.",
    ),
    River(
        "Nore",
        anchors=(
            (52.94, -7.65),
            (52.84, -7.40),
            (52.79, -7.34),
            (52.654, -7.252),
            (52.52, -7.14),
            (52.395, -6.94),
        ),
        note="Borris-in-Ossory, Durrow, Kilkenny, Thomastown, New Ross.",
    ),
    River(
        "Suir", natural_earth=("ne_10m_rivers_europe", "Suir"), note="Templemore area to Waterford."
    ),
    River(
        "Slaney",
        anchors=((52.97, -6.46), (52.94, -6.71), (52.66, -6.66), (52.50, -6.56), (52.336, -6.46)),
        note="Wicklow Mountains, Baltinglass, Bunclody, Enniscorthy, Wexford.",
    ),
    River(
        "Munster Blackwater",
        anchors=(
            (52.13, -8.89),
            (52.135, -8.655),
            (52.138, -8.275),
            (52.135, -7.93),
            (52.14, -7.84),
            (51.95, -7.85),
        ),
        note="Banteer, Mallow, Fermoy, Lismore, Cappoquin, Youghal.",
    ),
    River(
        "Lee",
        anchors=(
            (51.838, -9.317),
            (51.838, -9.13),
            (51.905, -8.96),
            (51.895, -8.62),
            (51.898, -8.47),
            (51.85, -8.30),
        ),
        note="Gougane Barra, Inchigeelagh, Macroom, Cork, Cork Harbour.",
    ),
)


class Flow:
    """The drainage of the elevation grid: depressions filled, a downhill neighbour for every cell, and the drainage area."""

    def __init__(self, elevation: Array) -> None:
        self.shape = elevation.shape
        height, width = self.shape
        size = height * width
        flat = elevation.ravel()
        self.sea = flat < 1
        filled = flat.copy()
        self.down = np.full(size, -1, dtype=np.int64)
        done = np.zeros(size, dtype=bool)
        queue: list[tuple[float, int]] = []
        for cell in np.flatnonzero(self.sea):
            done[cell] = True
            queue.append((float(flat[cell]), int(cell)))
        heapq.heapify(queue)
        order: list[int] = []
        while queue:
            level, cell = heapq.heappop(queue)
            order.append(cell)
            row, column = divmod(cell, width)
            for d_row, d_column in NEIGHBOURS:
                r, c = row + d_row, column + d_column
                if 0 <= r < height and 0 <= c < width and not done[r * width + c]:
                    neighbour = r * width + c
                    done[neighbour] = True
                    self.down[neighbour] = cell
                    filled[neighbour] = max(flat[neighbour], level + 1e-3)
                    heapq.heappush(queue, (float(filled[neighbour]), neighbour))
        area = np.ones(size)
        for cell in reversed(order):
            if self.down[cell] >= 0:
                area[self.down[cell]] += area[cell]
        self.area = area

    def snap(self, row: int, column: int) -> tuple[int, int]:
        """The cell of the greatest drainage area within `SNAP_RADIUS` cells that is land."""
        height, width = self.shape
        best, found = -1.0, (row, column)
        for r in range(max(0, row - SNAP_RADIUS), min(height, row + SNAP_RADIUS + 1)):
            for c in range(max(0, column - SNAP_RADIUS), min(width, column + SNAP_RADIUS + 1)):
                cell = r * width + c
                if not self.sea[cell] and self.area[cell] > best:
                    best, found = self.area[cell], (r, c)
        return found

    def cheapest(
        self, start: tuple[int, int], target: tuple[int, int]
    ) -> list[tuple[int, int]] | None:
        """The cheapest path from `start` to `target` in a window around them, a step costing less where the drainage area is greater."""
        height, width = self.shape
        margin = WINDOW_CELLS
        top, bottom = (
            max(0, min(start[0], target[0]) - margin),
            min(height, max(start[0], target[0]) + margin + 1),
        )
        left, right = (
            max(0, min(start[1], target[1]) - margin),
            min(width, max(start[1], target[1]) + margin + 1),
        )
        scale = math.log1p(float(self.area.max()))
        best = {start: 0.0}
        came: dict[tuple[int, int], tuple[int, int]] = {}
        queue = [(0.0, start)]
        while queue:
            cost, cell = heapq.heappop(queue)
            if cell == target:
                path = [cell]
                while path[-1] != start:
                    path.append(came[path[-1]])
                return path[::-1]
            if cost > best.get(cell, math.inf):
                continue
            for d_row, d_column in NEIGHBOURS:
                row, column = cell[0] + d_row, cell[1] + d_column
                if (
                    not (top <= row < bottom and left <= column < right)
                    or self.sea[row * width + column]
                ):
                    continue
                drainage = math.log1p(float(self.area[row * width + column])) / scale
                step = math.hypot(d_row, d_column) / (CHANNEL_FLOOR + drainage)
                if cost + step < best.get((row, column), math.inf):
                    best[(row, column)] = cost + step
                    came[(row, column)] = cell
                    heapq.heappush(queue, (cost + step, (row, column)))
        return None

    def downstream(
        self, start: tuple[int, int], target: tuple[int, int]
    ) -> list[tuple[int, int]] | None:
        """The downhill path from `start`, ending within `REACH_CELLS` of `target`, or None if it does not get there."""
        width = self.shape[1]
        path = [start]
        cell = start[0] * width + start[1]
        for _ in range(self.shape[0] * width):
            row, column = divmod(cell, width)
            if math.hypot(row - target[0], column - target[1]) <= REACH_CELLS:
                return path
            cell = int(self.down[cell])
            if cell < 0 or self.sea[cell]:
                return None
            path.append(divmod(cell, width))
        return None


def douglas_peucker(points: list[Point], tolerance: float) -> list[Point]:
    """The points of a line kept by the Douglas-Peucker rule, ends included."""
    if len(points) < 3:  # noqa: PLR2004
        return points
    (x1, y1), (x2, y2) = points[0], points[-1]
    length = math.hypot(x2 - x1, y2 - y1)
    distances = [
        abs((x2 - x1) * (y1 - y) - (x1 - x) * (y2 - y1)) / length
        if length
        else math.hypot(x - x1, y - y1)
        for x, y in points[1:-1]
    ]
    worst = int(np.argmax(distances))
    if distances[worst] <= tolerance:
        return [points[0], points[-1]]
    split = worst + 1
    return douglas_peucker(points[: split + 1], tolerance)[:-1] + douglas_peucker(
        points[split:], tolerance
    )


def natural_earth_line(source: Path, river: River, flow: Flow, crop: Crop) -> list[Point]:
    """A Natural Earth river as one line of (column, row) cell positions, ordered from its higher end to its lower."""
    file, name = river.natural_earth or ("", "")
    path = source / f"{file}.geojson"
    download(NATURAL_EARTH.format(name=file), path)
    pieces: list[list[Point]] = []
    for feature in json.loads(path.read_text(encoding="utf-8"))["features"]:
        properties = feature["properties"]
        if name not in (properties.get("name"), properties.get("name_en")):
            continue
        geometry = feature["geometry"]
        lines = (
            [geometry["coordinates"]]
            if geometry["type"] == "LineString"
            else geometry["coordinates"]
        )
        for line in lines:
            if not all(river.band[0] <= lat <= river.band[1] for _lon, lat in line):
                continue
            cells = [crop.cell_of(lat, lon) for lon, lat in line]
            width, height = crop.width, crop.height
            if sum(0 <= x < width and 0 <= y < height for x, y in cells) * 2 > len(cells):
                pieces.append(cells)
    if not pieces:
        message = f"no Natural Earth feature named {name!r} in {file}"
        raise SystemExit(message)
    return order_pieces(pieces, flow)


def order_pieces(pieces: list[list[Point]], flow: Flow) -> list[Point]:
    """Joins pieces end to end (nearest end first, from the piece that starts the highest) and orients the whole line downhill."""
    remaining = [list(piece) for piece in pieces]

    def height(point: Point) -> float:
        row = min(max(int(point[1]), 0), flow.shape[0] - 1)
        column = min(max(int(point[0]), 0), flow.shape[1] - 1)
        return float(flow.area[row * flow.shape[1] + column])

    # The line starts at the end with the least drainage area (the source side).
    ends = [(height(piece[0]), i, False) for i, piece in enumerate(remaining)] + [
        (height(piece[-1]), i, True) for i, piece in enumerate(remaining)
    ]
    _, first, reverse = min(ends)
    line = remaining.pop(first)
    if reverse:
        line.reverse()
    while remaining:
        tail = line[-1]
        candidates = [
            (math.hypot(piece[end][0] - tail[0], piece[end][1] - tail[1]), i, end == -1)
            for i, piece in enumerate(remaining)
            for end in (0, -1)
        ]
        _, nearest, backwards = min(candidates)
        piece = remaining.pop(nearest)
        if backwards:
            piece.reverse()
        line.extend(piece)
    return line


def hand_line(anchors: tuple[Point, ...], flow: Flow, crop: Crop) -> tuple[list[Point], list[str]]:
    """A hand-placed river as one line of (column, row) cell positions, and the notes on its joins."""
    snapped = []
    for latitude, longitude in anchors:
        column, row = crop.cell_of(latitude, longitude)
        snapped.append(flow.snap(int(row), int(column)))
    line: list[Point] = []
    notes: list[str] = []
    for index in range(len(snapped) - 1):
        start, end = snapped[index], snapped[index + 1]
        path = flow.downstream(start, end)
        if path is None:
            path = flow.cheapest(start, end)
            notes.append(f"cheapest path from anchor {index + 1} to anchor {index + 2}")
        if path is None:
            notes[-1] = f"straight line from anchor {index + 1} to anchor {index + 2}"
            path = [start, end]
        line.extend((column + 0.5, row + 0.5) for row, column in path)
    row, column = snapped[-1]
    line.append((column + 0.5, row + 0.5))
    return line, notes


def to_toml(rivers: list[tuple[str, list[Point]]], crop: Crop) -> str:
    """The text of `rivers.toml`."""
    text = [
        "# Ireland's major, practically useful waterways (milestone 6, slice 5): one line of latitude and longitude points per river,",
        "# source first. A first draft, edited by hand with the owner; see SOURCES.md for where each line came from.\n",
    ]
    for name, line in rivers:
        text.append("[[river]]")
        text.append(f'name = "{name}"')
        text.append("points = [")
        for x, y in line:
            lon, lat = cell_position(crop, x, y)
            text.append(f"  {{ latitude = {lat:.5f}, longitude = {lon:.5f} }},")
        text.append("]\n")
    return "\n".join(text)


def cell_position(crop: Crop, x: float, y: float) -> tuple[float, float]:
    """Longitude and latitude (degrees) of a position in cell units from the crop's corner."""
    x_m = (crop.origin[0] + x) * 839.0
    y_m = (crop.origin[1] + y) * 839.0
    radius = 6_378_137.0
    half = math.pi * radius
    longitude = math.degrees((x_m - half) / radius)
    latitude = math.degrees(2 * math.atan(math.exp((half - y_m) / radius)) - math.pi / 2)
    return longitude, latitude


def derive(source: Path, out: Path, *, force: bool) -> None:
    """Writes `rivers.toml`."""
    target = out / "rivers.toml"
    if target.exists() and not force:
        message = f"rivers.toml already exists in {out} (the committed copy carries hand edits); pass --force to overwrite"
        raise SystemExit(message)
    crop = Crop.of_map_toml(out / "map.toml")
    with Image.open(out / "elevation.png") as image:
        elevation: NDArray[np.float64] = np.asarray(image, dtype=np.float64) - ELEVATION_OFFSET
    flow = Flow(elevation)
    rivers: list[tuple[str, list[Point]]] = []
    for river in RIVERS:
        if river.natural_earth is not None:
            # Natural Earth's lines are coarse (a vertex every few kilometres): its vertices become anchors, snapped to the valley.
            rough = douglas_peucker(natural_earth_line(source, river, flow, crop), NE_ANCHOR_CELLS)
            anchors = tuple(cell_position(crop, x, y)[::-1] for x, y in rough)
            line, notes = hand_line(anchors, flow, crop)
        else:
            line, notes = hand_line(river.anchors, flow, crop)
        kept = douglas_peucker(line, SIMPLIFY_CELLS)
        rivers.append((river.name, kept))
        origin = (
            "Natural Earth vertices, snapped to the valley"
            if river.natural_earth
            else "hand-placed anchors, snapped to the valley"
        )
        print(
            f"{river.name}: {len(line)} -> {len(kept)} points; {origin}; {'; '.join(notes) or 'no straight joins'}"
        )
    target.write_text(to_toml(rivers, crop), encoding="utf-8")


def main() -> None:
    """Parses the command line and derives."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", maxsplit=1)[0])
    parser.add_argument(
        "--source", type=Path, default=Path("ireland-source"), help="download cache directory"
    )
    parser.add_argument("--out", type=Path, default=HERE, help="where rivers.toml is written")
    parser.add_argument("--force", action="store_true", help="overwrite an existing rivers.toml")
    arguments = parser.parse_args()
    derive(arguments.source, arguments.out, force=arguments.force)


if __name__ == "__main__":
    main()
