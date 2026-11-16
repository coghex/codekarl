"""Standard-library depression filling and D8 area accumulation for map fertility."""

from __future__ import annotations

import heapq
import math
from collections.abc import Sequence

NEIGHBOURS8 = (
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
)
SEA_SENTINEL_M = -1e9


def flow_accumulation(
    elevations: Sequence[Sequence[float]],
    ground_area_km2_by_row: Sequence[float],
    epsilon_m: float,
) -> list[list[float]]:
    """Fill depressions from sea and crop edges, then accumulate D8 upstream area.

    Sea is defined only by elevation below 1 m. Every other cell contributes its
    row's ground area and passes accumulated area downhill, including loughs.
    Crop-edge cells are outlets and do not pass area back into the crop.
    """
    return _flow(elevations, ground_area_km2_by_row, epsilon_m)


def _flow(  # noqa: PLR0912  # Keep fill and its D8 accumulation in one production path.
    elevations: Sequence[Sequence[float]],
    ground_area_km2_by_row: Sequence[float],
    epsilon_m: float,
) -> list[list[float]]:
    height = len(elevations)
    if height == 0:
        raise ValueError("elevations must contain at least one row")
    width = len(elevations[0])
    if width == 0 or any(len(row) != width for row in elevations):
        raise ValueError("elevations must be a non-empty rectangular grid")
    if len(ground_area_km2_by_row) != height:
        raise ValueError("ground_area_km2_by_row must have one value per elevation row")
    if epsilon_m <= 0 or not math.isfinite(epsilon_m):
        raise ValueError("epsilon_m must be finite and positive")

    surface = [[float(value) for value in row] for row in elevations]
    filled = [[math.inf] * width for _ in range(height)]
    sea = [[surface[row][column] < 1.0 for column in range(width)] for row in range(height)]
    seen = [[False] * width for _ in range(height)]
    queue: list[tuple[float, int, int]] = []

    for row in range(height):
        for column in range(width):
            edge = row in (0, height - 1) or column in (0, width - 1)
            if sea[row][column] or edge:
                level = SEA_SENTINEL_M if sea[row][column] else surface[row][column]
                filled[row][column] = level
                seen[row][column] = True
                heapq.heappush(queue, (level, row, column))

    while queue:
        level, row, column = heapq.heappop(queue)
        for d_row, d_column in NEIGHBOURS8:
            next_row, next_column = row + d_row, column + d_column
            if (
                0 <= next_row < height
                and 0 <= next_column < width
                and not seen[next_row][next_column]
            ):
                seen[next_row][next_column] = True
                next_level = max(surface[next_row][next_column], level + epsilon_m)
                filled[next_row][next_column] = next_level
                heapq.heappush(queue, (next_level, next_row, next_column))

    accumulation = [
        [0.0 if sea[row][column] else float(ground_area_km2_by_row[row]) for column in range(width)]
        for row in range(height)
    ]
    order = sorted(
        ((filled[row][column], row, column) for row in range(height) for column in range(width)),
        reverse=True,
    )
    for _level, row, column in order:
        if sea[row][column] or row in (0, height - 1) or column in (0, width - 1):
            continue
        best: tuple[int, int] | None = None
        drop = 0.0
        for d_row, d_column in NEIGHBOURS8:
            next_row, next_column = row + d_row, column + d_column
            if 0 <= next_row < height and 0 <= next_column < width:
                candidate = (filled[row][column] - filled[next_row][next_column]) / math.hypot(
                    d_row, d_column
                )
                if candidate > drop:
                    drop = candidate
                    best = next_row, next_column
        if best is not None:
            next_row, next_column = best
            accumulation[next_row][next_column] += accumulation[row][column]

    return accumulation
