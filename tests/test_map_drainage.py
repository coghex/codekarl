"""Retained standard-library contracts for Ireland's production drainage module."""

from __future__ import annotations

import importlib.util
import math
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "assets" / "map" / "ireland" / "drainage.py"
SPEC = importlib.util.spec_from_file_location("ireland_drainage", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load production drainage module at {MODULE_PATH}")
drainage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(drainage)


def lough_channel(lough_cells: int) -> list[list[float]]:
    """Build a one-cell channel between crop-edge banks, with a flat lough and sea outlet."""
    elevations = [[1000.0, 1000.0, 1000.0] for _ in range(10)]
    elevations[0] = [1000.0, 1000.0, 1000.0]
    elevations[-1] = [0.0, 0.0, 0.0]
    elevations[1][1] = 100.0
    elevations[2][1] = 90.0
    for row in range(3, 3 + lough_cells):
        elevations[row][1] = 80.0
    outlet_row = 3 + lough_cells
    elevations[outlet_row][1] = 70.0
    elevations[outlet_row + 1][1] = 60.0
    elevations[7][1] = 50.0
    elevations[8][1] = 0.0
    return elevations


def two_loughs_to_separate_sea_cells() -> list[list[float]]:
    """Build two separated flat-lough streams with distinct sea outlets and land below."""
    elevations = [[1000.0] * 9 for _ in range(11)]
    for column in (2, 6):
        for row, elevation in zip(
            range(1, 8), (100.0, 90.0, 80.0, 80.0, 70.0, 60.0, 50.0), strict=True
        ):
            elevations[row][column] = elevation
        elevations[8][column] = 0.0
        elevations[9][column] = 2.0
    return elevations


class DrainageTests(unittest.TestCase):
    def test_flat_lough_passes_upstream_area_to_first_downstream_land(self) -> None:
        elevations = lough_channel(2)
        self.assertEqual(elevations[3][1], elevations[4][1])
        accumulated = drainage.flow_accumulation(elevations, [1.0] * len(elevations), 0.01)
        self.assertEqual(accumulated[5][1], 5.0)

    def test_one_more_flat_lough_cell_adds_its_area_downstream(self) -> None:
        two_cells = lough_channel(2)
        three_cells = lough_channel(3)
        two = drainage.flow_accumulation(two_cells, [1.0] * len(two_cells), 0.01)
        three = drainage.flow_accumulation(three_cells, [1.0] * len(three_cells), 0.01)
        self.assertEqual(two[5][1], 5.0)
        self.assertEqual(three[6][1], 6.0)
        self.assertEqual(three[6][1] - two[5][1], 1.0)

    def test_sea_separates_lough_flows_and_land_beyond_sea_starts_at_one(self) -> None:
        elevations = two_loughs_to_separate_sea_cells()
        accumulated = drainage.flow_accumulation(elevations, [1.0] * len(elevations), 0.01)
        self.assertEqual(accumulated[7][2], 24.0)
        self.assertEqual(accumulated[7][6], 19.0)
        self.assertEqual(accumulated[8][2], 34.0)
        self.assertEqual(accumulated[8][6], 26.0)
        self.assertEqual(accumulated[9][2], 1.0)
        self.assertEqual(accumulated[9][6], 1.0)

    def test_no_lough_grid_matches_explicit_accepted_routing_values(self) -> None:
        # A distinct no-lough grid is a control: this rule must preserve the accepted routing there.
        elevations = [
            [0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 8.0, 7.0, 8.0, 0.0],
            [0.0, 7.0, 6.0, 7.0, 0.0],
            [0.0, 8.0, 7.0, 8.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 0.0],
        ]
        accumulated = drainage.flow_accumulation(elevations, [1.0] * 5, 0.01)
        self.assertEqual(
            accumulated,
            [
                [0.0, 1.0, 2.0, 1.0, 0.0],
                [0.0, 1.0, 2.0, 1.0, 0.0],
                [1.0, 1.0, 1.0, 1.0, 1.0],
                [1.0, 1.0, 1.0, 1.0, 1.0],
                [0.0, 0.0, 1.0, 0.0, 0.0],
            ],
        )

    def test_apply_edits_calls_one_shared_production_implementation(self) -> None:
        source = (MODULE_PATH.parent / "apply_edits.py").read_text(encoding="utf-8")
        self.assertIn("drainage.flow_accumulation(", source)
        self.assertNotIn("def priority_flood(", source)
        self.assertNotIn("np.argsort(-filled", source)


class IslandEditTests(unittest.TestCase):
    def test_ck358_island_edit_regions_cover_their_target_cells(self) -> None:
        directory = ROOT / "assets" / "map" / "ireland"
        edits = tomllib.loads((directory / "edits.toml").read_text(encoding="utf-8"))["edit"]
        crop = tomllib.loads((directory / "map.toml").read_text(encoding="utf-8"))["crop"]
        expected = [
            ("island-inchbofin", (396, 460), "lake"),
            ("island-inchcleraun", (384, 451), "lake"),
            ("island-white-island", (420, 246), "lake"),
            ("island-hare-island", (394, 476), "lake"),
            ("island-inishmacsaint", (418, 258), "lake"),
            ("island-inis-patraic", (640, 450), "sea"),
            ("island-inishglora", (102, 310), "sea"),
        ]
        self.assertEqual([entry["id"] for entry in edits[-7:]], [item[0] for item in expected])
        earth = 6_378_137.0
        half = math.pi * earth
        for edit, (name, target, water) in zip(edits[-7:], expected, strict=True):
            with self.subTest(edit=name):
                self.assertEqual(edit["paint"], "pasture")
                self.assertEqual(edit["where"], {"terrain": [water]})
                lat, lon, radius = edit["region"]["circle"]
                self.assertLessEqual(radius, 0.3)
                covered = []
                for y in range(target[1] - 3, target[1] + 4):
                    for x in range(target[0] - 3, target[0] + 4):
                        longitude = math.degrees(
                            ((crop["origin"][0] + x + 0.5) * 839 - half) / earth
                        )
                        latitude = math.degrees(
                            2
                            * math.atan(
                                math.exp((half - (crop["origin"][1] + y + 0.5) * 839) / earth)
                            )
                            - math.pi / 2
                        )
                        dx = (longitude - lon) * math.cos(math.radians(lat)) * 111
                        dy = (latitude - lat) * 111
                        if math.hypot(dx, dy) <= radius:
                            covered.append((x, y))
                if name == "island-inishglora":
                    self.assertEqual(covered, [(102, 309), target])
                else:
                    self.assertEqual(covered, [target])


if __name__ == "__main__":
    unittest.main()
