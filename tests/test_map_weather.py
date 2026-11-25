"""Contracts for the generated weather lookup tables and sea-distance layer."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import math
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from unittest import mock

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
MAP = ROOT / "assets/map/ireland"
sys.path.insert(0, str(MAP))
SPEC = importlib.util.spec_from_file_location("apply_edits", MAP / "apply_edits.py")
assert SPEC is not None and SPEC.loader is not None
edits = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(edits)
SCRIPT = importlib.util.spec_from_file_location(
    "generate_weather", ROOT / "tools/generate_weather.py"
)
assert SCRIPT is not None and SCRIPT.loader is not None
generator = importlib.util.module_from_spec(SCRIPT)
SCRIPT.loader.exec_module(generator)


class WeatherGenerationTests(unittest.TestCase):
    def test_tables_reproduce_committed_data_and_single_rounding_witnesses(self) -> None:
        text = (ROOT / "assets/game/weather.toml").read_text()
        self.assertEqual(generator.generate(), text)
        data = tomllib.loads(text)
        self.assertEqual(
            [
                data[name]
                for name in (
                    "wind_sea_friction",
                    "wind_sea_factor",
                    "wind_land_friction",
                    "wind_land_factor",
                    "wind_slope_blocks",
                    "wind_belt_latitude",
                )
            ],
            [15000, 690, 70000, 330, 2, 60000],
        )
        self.assertEqual(data["cosine"][630], 453990)
        self.assertEqual(data["tangent"][417], 890967)
        self.assertEqual(data["tangent"][850], 11430052)
        self.assertEqual(data["half_day"][439], 496)

    def test_layer_rounding_and_phase_validation(self) -> None:
        settings = tomllib.loads((MAP / "edits.toml").read_text())["settings"]
        lon, lat = np.meshgrid(np.linspace(-8.1, -7.9, 5), np.linspace(53.1, 52.9, 5))
        terrain = np.full((5, 5), edits.PASTURE)
        terrain[0, :] = terrain[-1, :] = edits.SEA
        terrain[:, 0] = terrain[:, -1] = edits.SEA
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "map.toml").write_text("[layers]\n")
            state = SimpleNamespace(
                terrain=terrain,
                longitude=lon,
                latitude=lat,
                elevation=np.zeros((5, 5)),
                slope=np.zeros((5, 5)),
                directory=directory,
                land=lambda: terrain == edits.PASTURE,
            )
            own = np.full((12, 5, 5), 10.0)
            _, half, _, _, _ = edits.climate_fields(state, settings["climate"], own)
            edits.compute_layers(state, settings, own)
            with Image.open(directory / "climate_half_range.png") as image:
                np.testing.assert_array_equal(np.asarray(image), np.floor(half * 10 + 0.5))
            manifest = tomllib.loads((directory / "map.toml").read_text())
            self.assertEqual(manifest["climate"]["warmest_month_phase_hundredths"], 650)
            self.assertEqual(manifest["layers"]["climate_sea_nearness"], "climate_sea_nearness.png")
            valid = copy.deepcopy(settings)
            valid["climate"]["warmest_month_phase"] = 0.07
            edits.compute_layers(state, valid, own)
            self.assertEqual(
                tomllib.loads((directory / "map.toml").read_text())["climate"][
                    "warmest_month_phase_hundredths"
                ],
                7,
            )
            for phase in (6.501, 12.0):
                bad = copy.deepcopy(settings)
                bad["climate"]["warmest_month_phase"] = phase
                with self.assertRaisesRegex(SystemExit, "warmest_month_phase"):
                    edits.compute_layers(state, bad, own)

    def test_whole_month_matches_generator_for_both_hemispheres(self) -> None:
        table = tomllib.loads((ROOT / "assets/game/weather.toml").read_text())["cosine"]
        settings = tomllib.loads((MAP / "edits.toml").read_text())["settings"]["climate"]
        lon, lat = np.meshgrid(np.linspace(-8.1, -7.9, 5), [53.1, 53.0, 0.0, -34.0, -34.1])
        terrain = np.full((5, 5), edits.PASTURE)
        terrain[0, :] = terrain[-1, :] = edits.SEA
        terrain[:, 0] = terrain[:, -1] = edits.SEA
        state = SimpleNamespace(
            terrain=terrain,
            longitude=lon,
            latitude=lat,
            elevation=np.zeros((5, 5)),
            slope=np.zeros((5, 5)),
            land=lambda: terrain == edits.PASTURE,
        )
        own = np.full((12, 5, 5), 10.0)
        _, half_ranges, _, _, _ = edits.climate_fields(state, settings, own)
        sample = (
            math.floor(half_ranges[1, 2] * 10 + 0.5),
            math.floor(half_ranges[3, 2] * 10 + 0.5),
        )
        for phase in (7, 650, 1199):
            for southern in (False, True):
                for half in (1, 7, 191, 250, 300, sample[int(southern)]):
                    for month in range(12):
                        shifted = phase + (600 if southern else 0)
                        angle = 3 * (100 * month - shifted)
                        value = generator.rounded(half * table[angle % 3600] / 1_000_000)
                        generator_month = (month + 6) % 12 if southern else month
                        original = half * math.cos(
                            2.0 * math.pi * (generator_month - phase / 100.0) / 12.0
                        )
                        self.assertLessEqual(abs(value - original), 0.51)

    def test_nearest_sea_is_diagonal_and_ignores_lake(self) -> None:
        lon, lat = np.meshgrid(np.array([0.0, 0.1, 0.2]), np.array([53.2, 53.1, 53.0]))
        terrain = np.full((3, 3), edits.PASTURE)
        terrain[0, 0] = edits.SEA
        terrain[1, 2] = edits.LAKE
        state = SimpleNamespace(terrain=terrain, longitude=lon, latitude=lat)
        result = edits.sea_nearness(state, 40.0)
        a, b = math.radians(53.0), math.radians(53.2)
        h = (
            math.sin((a - b) / 2) ** 2
            + math.cos(a) * math.cos(b) * math.sin(math.radians(0.2) / 2) ** 2
        )
        distance = 2 * 6378137 * math.asin(math.sqrt(h))
        self.assertEqual(result[2, 2], math.floor(1000 * math.exp(-distance / 40000) + 0.5))
        self.assertEqual(result[0, 0], 1000)
        terrain[0, 0] = edits.PASTURE
        self.assertTrue((edits.sea_nearness(state, 40.0) == 0).all())


BACKDROP = ROOT / "assets/map/backdrop"
CLIMATE = tomllib.loads((MAP / "edits.toml").read_text())["settings"]["climate"]
CHANGED = (
    "climate_temperature.png",
    "climate_half_range.png",
    "climate_gdd.png",
    "fertility.png",
)
# CK-417's table: the stored mean (tenths plus 1000), half range (tenths) and growing degree-days of seven places.
PLACES = {
    "Biorra": ((None, (22833, 15538)), 1094, 55, 1659),
    "Ard Mhacha": ((None, (22999, 15257)), 1086, 56, 1450),
    "Sceilg Mhichil": ((None, (22484, 15826)), 1104, 45, 1985),
    "Teamhair": (((53.5786, -6.6118), None), 1084, 55, 1377),
    "Caiseal": (((52.5200, -7.8906), None), 1093, 55, 1639),
    "Corcaigh": (((51.8985, -8.4756), None), 1104, 49, 1957),
    "Doire": (((54.9966, -7.3086), None), 1090, 49, 1492),
}
STALE_MARKER = "0" * 64 + "  edits.toml\n"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


MIDDLE = 10


def sea_grid(height: int, width: int, *cells: tuple[int, int]) -> np.ndarray:
    grid = np.zeros((height, width), dtype=bool)
    for cell in cells:
        grid[cell] = True
    return grid


class SeaSettingsTests(unittest.TestCase):
    def refused(self, climate: dict[str, object], name: str) -> None:
        with self.assertRaisesRegex(SystemExit, name):
            edits.check_sea_settings(climate)

    def test_the_shipped_settings_are_accepted_and_the_old_ones_are_gone(self) -> None:
        edits.check_sea_settings(CLIMATE)
        self.assertNotIn("ocean_effect_c", CLIMATE)
        self.assertNotIn("ocean_half_range_c", CLIMATE)
        self.assertEqual(CLIMATE["maritime_length_km"], 1200.0)
        self.assertEqual(
            [CLIMATE[name] for name in ("sea_pull", "sea_air_cooler_c", "sea_air_swing")],
            [0.84, 1.25, 0.8],
        )
        self.assertEqual((CLIMATE["sea_blend_cells"], CLIMATE["sea_blend_passes"]), (100, 3))

    def test_each_missing_setting_is_refused_by_name(self) -> None:
        for name in (
            "sea_backdrop",
            "sea_pull",
            "sea_air_cooler_c",
            "sea_air_swing",
            "sea_blend_cells",
            "sea_blend_passes",
        ):
            climate = dict(CLIMATE)
            del climate[name]
            self.refused(climate, name)

    def test_each_setting_outside_its_range_is_refused_by_name(self) -> None:
        bad = {
            "sea_pull": (0, -0.1, 1.01, True, "0.84"),
            "sea_air_cooler_c": (-0.1, 10.1, float("nan")),
            "sea_air_swing": (0.09, 3.1),
            "sea_blend_cells": (0, 501, 1.5, True),
            "sea_blend_passes": (0, 6, 2.5),
        }
        for name, values in bad.items():
            for value in values:
                climate = dict(CLIMATE)
                climate[name] = value
                with self.subTest(name=name, value=value):
                    self.refused(climate, name)
        for name, value in (
            ("sea_pull", 1),
            ("sea_air_cooler_c", 0),
            ("sea_air_cooler_c", 10),
            ("sea_air_swing", 0.1),
            ("sea_air_swing", 3),
            ("sea_blend_cells", 1),
            ("sea_blend_cells", 500),
            ("sea_blend_passes", 1),
            ("sea_blend_passes", 5),
        ):
            climate = dict(CLIMATE)
            climate[name] = value
            edits.check_sea_settings(climate)

    def test_a_table_still_carrying_a_removed_setting_is_refused(self) -> None:
        for name, value in (("ocean_effect_c", 4.2), ("ocean_half_range_c", [1, 2])):
            self.refused({**CLIMATE, name: value}, name)

    def test_compute_layers_checks_the_settings_before_it_reads_anything(self) -> None:
        settings = tomllib.loads((MAP / "edits.toml").read_text())["settings"]
        del settings["climate"]["sea_pull"]
        with self.assertRaisesRegex(SystemExit, "sea_pull"):
            edits.compute_layers(SimpleNamespace(), settings)


class BackdropReadingTests(unittest.TestCase):
    def stage(
        self, root: Path, *, cell_metres: float = 839.0, columns: int = 20, rows: int = 20
    ) -> Path:
        """A made-up map directory and backdrop side by side: blocks of 4, a crop of 4 by 4 cells at (40, 50)."""
        directory = root / "map"
        backdrop = root / "backdrop"
        directory.mkdir()
        backdrop.mkdir()
        (directory / "map.toml").write_text(
            'projection = "spherical_mercator"\ncell_metres = 839.0\n\n[crop]\norigin = [40, 50]\n'
            "width = 4\nheight = 4\n"
        )
        (backdrop / "backdrop.toml").write_text(
            f'format = 2\nprojection = "spherical_mercator"\ncell_metres = {cell_metres}\n'
            f"block_cells = 4\nfirst_row = 5\nrows = {rows}\ncolumns = {columns}\n\n[layers]\n"
            'land = "land.png"\nsea_temperature = "sea_temperature.png"\n'
        )
        Image.fromarray(np.zeros((rows, columns), dtype=np.uint8)).save(backdrop / "land.png")
        sea = np.full((12 * rows, columns), 1100, dtype=np.uint16)
        Image.fromarray(sea).save(backdrop / "sea_temperature.png")
        return directory

    def test_a_backdrop_of_one_temperature_gives_every_cell_that_temperature(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = self.stage(Path(tmp))
            own = edits.load_sea_temperatures(directory, {"sea_backdrop": "../backdrop"})
            self.assertEqual(own.shape, (12, 4, 4))
            np.testing.assert_allclose(own, 10.0, atol=1e-12)

    def test_a_missing_or_different_backdrop_is_refused_naming_the_setting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = self.stage(Path(tmp))
            with self.assertRaisesRegex(SystemExit, "sea_backdrop"):
                edits.load_sea_temperatures(directory, {"sea_backdrop": "../nowhere"})
            (Path(tmp) / "backdrop/sea_temperature.png").unlink()
            with self.assertRaisesRegex(SystemExit, "sea_backdrop"):
                edits.load_sea_temperatures(directory, {"sea_backdrop": "../backdrop"})
        with tempfile.TemporaryDirectory() as tmp:
            directory = self.stage(Path(tmp), cell_metres=840.0)
            with self.assertRaisesRegex(SystemExit, "sea_backdrop.*cell size"):
                edits.load_sea_temperatures(directory, {"sea_backdrop": "../backdrop"})
        with tempfile.TemporaryDirectory() as tmp:
            directory = self.stage(Path(tmp), columns=12)
            with self.assertRaisesRegex(SystemExit, "sea_backdrop.*window"):
                edits.load_sea_temperatures(directory, {"sea_backdrop": "../backdrop"})
        with tempfile.TemporaryDirectory() as tmp:
            directory = self.stage(Path(tmp), rows=11)
            with self.assertRaisesRegex(SystemExit, "sea_backdrop.*window"):
                edits.load_sea_temperatures(directory, {"sea_backdrop": "../backdrop"})

    def test_the_window_reaches_three_blocks_beyond_the_crops_blocks_both_ends_included(
        self,
    ) -> None:
        backdrop = {"block_cells": 10, "columns": 100, "first_row": 0, "rows": 100}
        # Cells 45 to 54 lie in blocks 4 and 5; rows 52 to 55 in blocks 5.
        self.assertEqual(edits.backdrop_window((45, 52), 10, 4, backdrop), (1, 8, 2, 8))
        # Ireland's crop, in blocks of 64 (CK-417): columns 347 to 365 and rows 231 to 252.
        ireland = tomllib.loads((BACKDROP / "backdrop.toml").read_text())
        self.assertEqual(
            edits.backdrop_window((22436, 14979), 744, 959, ireland), (347, 365, 231, 252)
        )
        for crop in ((15, 52), (45, 12), (975, 52), (45, 982)):
            with self.subTest(crop=crop), self.assertRaisesRegex(SystemExit, "sea_backdrop"):
                edits.backdrop_window(crop, 10, 4, backdrop)
        ireland["first_row"] = 232  # the window's first row is 231
        with self.assertRaisesRegex(SystemExit, "sea_backdrop"):
            edits.backdrop_window((22436, 14979), 744, 959, ireland)

    def test_a_land_block_takes_the_mean_of_the_values_its_neighbours_had_before_the_pass(
        self,
    ) -> None:
        rows, columns = np.meshgrid(np.arange(5.0), np.arange(5.0), indexing="ij")
        value = np.stack([rows * 10 + columns, rows * 10 + columns + 100])
        land = np.zeros((5, 5), dtype=bool)
        land[2, 2] = True
        field, passes = edits.fill_land_blocks(value, land)
        self.assertEqual(passes, 1)
        self.assertEqual(field[0, 2, 2], 22.0)
        self.assertEqual(field[1, 2, 2], 122.0)
        np.testing.assert_array_equal(field[:, ~land], value[:, ~land])
        # Two blocks from any sea: filled in the second pass, from the first pass's values.
        value = np.zeros((1, 3, 3))
        value[0, 0, 0], value[0, 0, 1] = 3.0, 9.0
        land = np.ones((3, 3), dtype=bool)
        land[0, 0] = land[0, 1] = False
        field, passes = edits.fill_land_blocks(value, land)
        self.assertEqual(passes, 2)
        np.testing.assert_array_equal(field[0], [[3.0, 9.0, 9.0], [6.0, 6.0, 9.0], [6.0, 7.0, 7.5]])

    def test_a_window_of_land_alone_is_refused(self) -> None:
        with self.assertRaisesRegex(SystemExit, "no sea block"):
            edits.fill_land_blocks(np.zeros((12, 3, 3)), np.ones((3, 3), dtype=bool))

    def test_a_cell_lies_between_the_four_block_centres_round_it(self) -> None:
        # The value of a block is its column, so a cell's own value is its place x among the blocks.
        field = np.broadcast_to(np.arange(3.0, 9.0), (1, 4, 6)).copy()
        for size, cells in ((64, (351, 352, 320)), (3, (15, 16, 17))):
            for cell in cells:
                own = edits.cell_sea_temperatures(field, size, (cell, 0), (1, 1), (3, -1))
                x = (cell + 0.5) / size - 0.5
                self.assertAlmostEqual(own[0, 0, 0], x, places=12, msg=f"{size} {cell}")
        # Blocks of 64: the two cells either side of block 5's centre weigh 127/128 and 1/128, the first cell of block 5 65/128.
        own = edits.cell_sea_temperatures(field, 64, (351, 0), (2, 1), (3, -1))
        self.assertEqual(own[0, 0, 0], 4 + 127 / 128)
        self.assertEqual(own[0, 0, 1], 5 + 1 / 128)
        self.assertEqual(
            edits.cell_sea_temperatures(field, 64, (320, 0), (1, 1), (3, -1))[0, 0, 0], 4 + 65 / 128
        )
        # Made-up blocks of 3: cell 16 is exactly at block 5's centre, cell 17 is 1/3 towards block 6.
        own = edits.cell_sea_temperatures(field, 3, (16, 0), (2, 1), (3, -1))
        self.assertEqual(own[0, 0, 0], 5.0)
        self.assertAlmostEqual(own[0, 0, 1], 5 + 1 / 3, places=12)


class SeaBlendTests(unittest.TestCase):
    def test_every_sea_at_one_temperature_gives_every_cell_that_temperature(self) -> None:
        sea = sea_grid(9, 15, *((row, 0) for row in range(9)))
        near = edits.blend_seas(np.full((12, 9, 15), 7.0), sea, 5, 3)
        np.testing.assert_allclose(near, 7.0, atol=1e-12)

    def test_the_nearer_sea_counts_more(self) -> None:
        sea = sea_grid(5, 21, *((row, column) for row in range(5) for column in (0, 20)))
        own = np.where(np.arange(21) < MIDDLE, 10.0, 20.0)[None, None, :] * np.ones((12, 5, 21))
        near = edits.blend_seas(own, sea, 4, 3)[0]
        np.testing.assert_allclose(near[:, 10], 15.0, atol=1e-9)
        for column in range(10):
            self.assertLess(near[2, column], near[2, 20 - column])
            self.assertAlmostEqual(near[2, column] + near[2, 20 - column], 30.0, places=9)

    def test_a_cell_with_no_sea_in_reach_stops_the_tool_though_the_float_share_is_not_nothing(
        self,
    ) -> None:
        sea = sea_grid(11, 11, (0, 4), (1, 4), (2, 1), (2, 2), (4, 1), (4, 2), (4, 3))
        reach = edits.sea_in_reach(sea, 2, 3)
        self.assertEqual(
            [tuple(int(v) for v in cell) for cell in np.argwhere(~reach)],
            [(8, 10), (9, 10), (10, 10)],
        )
        self.assertTrue(reach[8, 9])
        share = sea.astype(float)
        for _ in range(3):
            share = edits.box_blur(share, 2)
        self.assertGreater(share[8, 10], 0)
        self.assertLess(share[8, 10], edits.LEAST_SEA_SHARE)
        with self.assertRaisesRegex(SystemExit, r"sea_blend_cells.*\(8, 10\)"):
            edits.blend_weights(sea, 2, 3)
        # With the last column cut off every cell is in reach.
        weight = edits.blend_weights(sea[:, :10], 2, 3)
        self.assertTrue((weight > 0).all())

    def test_one_sea_cell_gives_the_least_share_a_cell_in_reach_can_have(self) -> None:
        lone = sea_grid(11, 11, (0, 0))
        share = lone.astype(float)
        for _ in range(3):
            share = edits.box_blur(share, 2)
        self.assertAlmostEqual(share[6, 6], (1 / 25) ** 3, places=12)

    def test_too_little_sea_for_the_arithmetic_stops_the_tool_though_in_reach(self) -> None:
        sea = sea_grid(301, 301, (0, 0))
        self.assertTrue(edits.sea_in_reach(sea, 100, 3).all())
        with self.assertRaisesRegex(SystemExit, "sea_blend_cells.*too little sea"):
            edits.blend_weights(sea, 100, 3)


class SeaFormulaTests(unittest.TestCase):
    def test_birrs_mean_and_half_range_to_the_stored_tenth(self) -> None:
        months = [9.69, 9.13, 9.00, 9.69, 11.03, 13.12, 14.98, 15.66, 15.05, 13.41, 11.84, 10.72]
        near = np.array(months).reshape(12, 1, 1)
        zonal = edits.interpolate(
            np.array([53.0956]), CLIMATE["latitude_deg"], CLIMATE["zonal_mean_c"]
        )
        continental = edits.interpolate(
            np.array([53.0956]), CLIMATE["latitude_deg"], CLIMATE["continental_half_range_c"]
        )
        self.assertAlmostEqual(float(zonal[0]), 6.1426, places=4)
        self.assertAlmostEqual(float(continental[0]), 16.4287, places=4)
        pull = CLIMATE["sea_pull"] * np.exp(-np.array([[71.2]]) / CLIMATE["maritime_length_km"])
        self.assertAlmostEqual(float(pull[0, 0]), 0.7916, places=4)
        mean, half = edits.sea_air_layers(
            near,
            pull,
            (zonal.reshape(1, 1), continental.reshape(1, 1)),
            np.array([[55.0]]),
            CLIMATE,
        )
        self.assertAlmostEqual(float(mean[0, 0]), 9.39, places=2)
        self.assertAlmostEqual(float(half[0, 0]), 5.53, places=2)
        self.assertEqual(math.floor(float(mean[0, 0]) * 10 + 0.5) + 1000, 1094)
        self.assertEqual(math.floor(float(half[0, 0]) * 10 + 0.5), 55)

    def test_a_cell_with_no_pull_has_the_zonal_mean_less_its_height_and_the_continental_range(
        self,
    ) -> None:
        mean, half = edits.sea_air_layers(
            np.full((12, 1, 1), 99.0),
            np.zeros((1, 1)),
            (np.array([[6.0]]), np.array([[16.0]])),
            np.array([[100.0]]),
            CLIMATE,
        )
        self.assertAlmostEqual(float(mean[0, 0]), 6.0 - 0.65)
        self.assertEqual(float(half[0, 0]), 16.0)


class NoStepsTests(unittest.TestCase):
    def layers(self) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
        return (
            np.full((3, 3), 1100.0),
            np.full((3, 3), 50.0),
            np.zeros((3, 3)),
            np.zeros((3, 3), dtype=bool),
        )

    def check(
        self,
        mean: np.ndarray,
        half: np.ndarray,
        height: np.ndarray,
        sea: np.ndarray,
        lapse: float = 6.5,
    ) -> None:
        edits.check_no_steps(mean, half, height, sea, lapse)

    def test_half_ranges_one_apart_pass_and_two_apart_fail_naming_the_half_range(self) -> None:
        mean, half, height, sea = self.layers()
        half[1, 1] = 51
        self.check(mean, half, height, sea)
        half[1, 1] = 52
        with self.assertRaisesRegex(SystemExit, "climate_half_range.*half range"):
            self.check(mean, half, height, sea)

    def test_means_one_tenth_apart_pass_and_two_apart_fail_naming_the_mean(self) -> None:
        mean, half, height, sea = self.layers()
        mean[1, 1] = 1101
        self.check(mean, half, height, sea)
        mean[1, 1] = 1102
        with self.assertRaisesRegex(SystemExit, "climate_temperature.*mean"):
            self.check(mean, half, height, sea)

    def test_the_height_part_is_added_back_and_1500_is_allowed(self) -> None:
        mean, half, height, sea = self.layers()
        # A lapse of 5 and heights 10 m apart: means 1 tenth apart are exactly at 1500; 11 m apart, 1550.
        mean[1, 1], height[1, 1] = 1101, 10
        self.check(mean, half, height, sea, lapse=5)
        height[1, 1] = 11
        with self.assertRaisesRegex(SystemExit, "climate_temperature"):
            self.check(mean, half, height, sea, lapse=5)
        # Exactly: a lapse that puts the step a hair over 1500 (1500.0000004) is refused, as is any above it.
        height[1, 1] = 10
        for lapse in (5.000000004, 5.001):
            with (
                self.subTest(lapse=lapse),
                self.assertRaisesRegex(SystemExit, r"climate_temperature.*\(0, 1\) and \(1, 1\)"),
            ):
                self.check(mean, half, height, sea, lapse=lapse)
        # The step is shown to the digit it failed by, so that "1500.0000004" is not printed as 1500.
        height[1, 1] = 10
        with self.assertRaisesRegex(SystemExit, r"differs by 1500\.0000004 ten-thousandths"):
            self.check(mean, half, height, sea, lapse=5.000000004)
        # A height below nothing counts as nothing.
        height[1, 1] = -500
        mean[1, 1] = 1100
        self.check(mean, half, height, sea, lapse=5)

    def test_the_lapse_is_the_decimal_written_in_the_toml_and_never_overflows(self) -> None:
        mean, half, height, sea = self.layers()
        # Written as 5.0000000000000001 the step is 1500.00000000000001; a plain float would read it as 5.0 and pass.
        mean[1, 1], height[1, 1] = 1101, 10
        written = edits.load_toml("lapse = 5.0000000000000001\nplain = 5.0\n")
        self.assertEqual(float(written["lapse"]), 5.0)
        self.check(mean, half, height, sea, lapse=written["plain"])
        with self.assertRaisesRegex(SystemExit, r"climate_temperature.*1500\.00000000000001"):
            self.check(mean, half, height, sea, lapse=written["lapse"])
        # A very precise lapse and a large step: native 64-bit integers would wrap (2000 x 10**16 x 10**4 does not fit).
        mean[1, 1], height[1, 1] = 1102, 0
        with self.assertRaisesRegex(SystemExit, "climate_temperature"):
            self.check(mean, half, height, sea, lapse=0.05000000000000001)
        with self.assertRaisesRegex(SystemExit, "climate_temperature"):
            self.check(mean, half, height, sea, lapse=0.05)
        # A step within the bound still passes on the wide path.
        mean[1, 1], height[1, 1] = 1101, 0
        self.check(mean, half, height, sea, lapse=0.05000000000000001)

    def test_the_settings_are_read_with_their_written_decimals(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp) / "ireland"
            shutil.copytree(MAP, directory)
            text = (directory / "edits.toml").read_text()
            self.assertIn("lapse_c_per_km = 6.5", text)
            (directory / "edits.toml").write_text(
                text.replace("lapse_c_per_km = 6.5", "lapse_c_per_km = 6.50000000000000001")
            )
            seen: list[dict[str, Any]] = []
            with mock.patch.object(
                edits, "apply_settings", lambda _state, settings: seen.append(settings)
            ):
                edits.recompute_layers(directory)
            self.assertEqual(seen[0]["climate"]["lapse_c_per_km"].text, "6.50000000000000001")

    def test_an_exact_zero_or_a_tiny_lapse_still_names_the_layer_and_the_pair(self) -> None:
        mean, half, height, sea = self.layers()
        mean[1, 1] = 1102  # a step of 2000 ten-thousandths at the same height, whatever the lapse
        for spelling in (
            "0.0",
            "-0.0",
            "0e0",
            "0e-4301",
            "0e-100001",  # an exact zero, past the length cap, is still zero
            "1e-4301",
            "1e-100000",
            "0.000000000000000000001",
        ):
            lapse = edits.load_toml(f"lapse = {spelling}\n")["lapse"]
            with (
                self.subTest(spelling=spelling),
                self.assertRaisesRegex(
                    SystemExit,
                    r"climate_temperature: mean of cells \(0, 1\) and \(1, 1\) differs by 2000 ",
                ),
            ):
                self.check(mean, half, height, sea, lapse=lapse)
        mean[1, 1] = 1101
        for spelling in ("0e-4301", "1e-4301"):
            self.check(
                mean, half, height, sea, lapse=edits.load_toml(f"lapse = {spelling}\n")["lapse"]
            )

    def test_a_very_precise_lapse_costs_no_memory_at_ireland_size(self) -> None:
        import tracemalloc  # noqa: PLC0415  # Only this test measures allocation.

        mean = np.full((959, 744), 1100.0)
        mean[500, 400] = 1102  # a step of 2000 at the same height
        height = np.zeros((959, 744))
        sea = np.zeros((959, 744), dtype=bool)
        lapse = edits.load_toml("lapse = 1e-100000\n")["lapse"]
        tracemalloc.start()
        try:
            with self.assertRaisesRegex(
                SystemExit, r"climate_temperature: mean of cells \(499, 400\) and \(500, 400\)"
            ):
                edits.check_no_steps(mean, np.full_like(mean, 50.0), height, sea, lapse)
            peak = tracemalloc.get_traced_memory()[1]
        finally:
            tracemalloc.stop()
        self.assertLess(peak, 400 * 1024 * 1024)

    def test_a_lapse_too_long_to_work_with_is_refused_naming_the_setting(self) -> None:
        mean, half, height, sea = self.layers()
        for spelling in ("1e-100001", "1e400", "-1e400"):
            lapse = edits.load_toml(f"lapse = {spelling}\n")["lapse"]
            with (
                self.subTest(spelling=spelling),
                self.assertRaisesRegex(SystemExit, "lapse_c_per_km"),
            ):
                self.check(mean, half, height, sea, lapse=lapse)

    def test_compute_layers_names_the_step_for_an_exponent_spelling_of_the_lapse(self) -> None:
        text = (MAP / "edits.toml").read_text()
        lon, lat = np.meshgrid(np.linspace(-8.1, -7.9, 7), np.linspace(53.1, 52.9, 7))
        terrain = np.full((7, 7), edits.PASTURE)
        terrain[0, :] = terrain[-1, :] = terrain[:, 0] = terrain[:, -1] = edits.SEA
        for spelling in ("0.0", "0e-4301", "1e-4301"):
            settings = edits.load_toml(
                text.replace("lapse_c_per_km = 6.5", f"lapse_c_per_km = {spelling}")
            )["settings"]
            # No half-range step (a flat continental table and no sea swing), and a pull that falls off within a few cells (a cell is about 2 km here).
            settings["climate"]["continental_half_range_c"] = [0] * 8
            settings["climate"]["maritime_length_km"] = 5.0
            with tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                (directory / "map.toml").write_text("[layers]\n")
                state = SimpleNamespace(
                    terrain=terrain,
                    longitude=lon,
                    latitude=lat,
                    elevation=np.zeros((7, 7)),
                    slope=np.zeros((7, 7)),
                    directory=directory,
                    land=lambda: terrain == edits.PASTURE,
                )
                with (
                    self.subTest(spelling=spelling),
                    self.assertRaisesRegex(
                        SystemExit,
                        r"climate_temperature: mean of cells \(\d, \d\) and \(\d, \d\) differs by ",
                    ),
                ):
                    edits.compute_layers(state, settings, np.full((12, 7, 7), 10.0))
                self.assertFalse((directory / "climate_temperature.png").exists())

    def test_compute_layers_hands_the_written_lapse_to_the_bound(self) -> None:
        settings = edits.load_toml((MAP / "edits.toml").read_text())["settings"]
        lon, lat = np.meshgrid(np.linspace(-8.1, -7.9, 5), np.linspace(53.1, 52.9, 5))
        terrain = np.full((5, 5), edits.PASTURE)
        terrain[0, :] = terrain[-1, :] = terrain[:, 0] = terrain[:, -1] = edits.SEA
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "map.toml").write_text("[layers]\n")
            state = SimpleNamespace(
                terrain=terrain,
                longitude=lon,
                latitude=lat,
                elevation=np.zeros((5, 5)),
                slope=np.zeros((5, 5)),
                directory=directory,
                land=lambda: terrain == edits.PASTURE,
            )
            with mock.patch.object(edits, "check_no_steps") as bound:
                edits.compute_layers(state, settings, np.full((12, 5, 5), 10.0))
            self.assertEqual(bound.call_args.args[4].text, "6.5")

    def test_a_lake_counts_as_land_and_a_sea_cell_is_not_checked(self) -> None:
        mean, half, height, sea = self.layers()
        half[1, 1] = 60
        with self.assertRaises(SystemExit):
            self.check(
                mean, half, height, sea
            )  # sea is False everywhere, as for land beside a lake
        sea[1, 1] = True
        self.check(mean, half, height, sea)

    def test_the_first_pair_in_row_order_is_named(self) -> None:
        mean, half, height, sea = self.layers()
        half[1, 1] = 55
        half[2, 0] = 60
        with self.assertRaisesRegex(SystemExit, r"\(0, 1\) and \(1, 1\)"):
            self.check(mean, half, height, sea)
        mean[0, 0] = 1300
        with self.assertRaisesRegex(SystemExit, "climate_half_range"):
            self.check(
                mean, half, height, sea
            )  # each is checked on its own: the half range is named first

    def test_compute_layers_checks_the_bound_before_it_writes(self) -> None:
        settings = tomllib.loads((MAP / "edits.toml").read_text())["settings"]
        lon, lat = np.meshgrid(np.linspace(-8.1, -7.9, 7), np.linspace(53.1, 52.9, 7))
        terrain = np.full((7, 7), edits.PASTURE)
        terrain[0, :] = terrain[-1, :] = terrain[:, 0] = terrain[:, -1] = edits.SEA
        own = np.full((12, 7, 7), 10.0)
        own[:, 3, 3] = 10.0
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "map.toml").write_text("[layers]\n")
            elevation = np.zeros((7, 7))
            elevation[3, 3] = (
                400.0  # a 400 m peak: a step of 2.6 degrees from the lapse alone, which is added back
            )
            state = SimpleNamespace(
                terrain=terrain,
                longitude=lon,
                latitude=lat,
                elevation=elevation,
                slope=np.zeros((7, 7)),
                directory=directory,
                land=lambda: terrain == edits.PASTURE,
            )
            edits.compute_layers(state, settings, own)  # the lapse's part is added back, so no step
            self.assertTrue((directory / "climate_temperature.png").exists())
            (directory / "climate_temperature.png").unlink()
            # A pull that falls off within a cell or two of the sea leaves the half range stepping by more than a tenth.
            settings["climate"]["maritime_length_km"] = 0.5
            with self.assertRaisesRegex(SystemExit, "climate_half_range"):
                edits.compute_layers(state, settings, own)
            self.assertFalse((directory / "climate_temperature.png").exists())


class IrelandSeaInfluenceTests(unittest.TestCase):
    def stage(self, root: Path) -> Path:
        """Ireland's directory and the backdrop's three inputs side by side, as shipped."""
        directory = root / "ireland"
        shutil.copytree(MAP, directory)
        backdrop = root / "backdrop"
        backdrop.mkdir()
        for name in ("backdrop.toml", "land.png", "sea_temperature.png"):
            shutil.copy(BACKDROP / name, backdrop / name)
        return directory

    def test_the_layers_are_reproduced_byte_for_byte_from_a_clean_copy(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = self.stage(Path(tmp))
            (directory / "edits.applied").write_text(STALE_MARKER)
            edits.recompute_layers(directory)
            names = sorted(path.name for path in MAP.glob("*.png"))
            for name in (*names, "map.toml"):
                self.assertEqual((directory / name).read_bytes(), (MAP / name).read_bytes(), name)
            # The plain run leaves the marker alone, even a stale one.
            self.assertEqual((directory / "edits.applied").read_text(), STALE_MARKER)
            # A second run, with the option, changes no image and refreshes the marker.
            edits.recompute_layers(directory, refresh_marker=True)
            for name in (*names, "map.toml"):
                self.assertEqual((directory / name).read_bytes(), (MAP / name).read_bytes(), name)
            digest = sha256(MAP / "edits.toml")
            self.assertEqual((directory / "edits.applied").read_text(), f"{digest}  edits.toml\n")
        self.assertEqual((MAP / "edits.applied").read_text(), f"{digest}  edits.toml\n")

    def test_the_option_is_for_the_layers_only_run(self) -> None:
        result = subprocess.run(
            [sys.executable, str(MAP / "apply_edits.py"), "--refresh-marker"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("--layers-only", result.stderr)

    def test_the_seven_places_hold_the_tables_values(self) -> None:
        crop = edits.Crop.of_map_toml(MAP / "map.toml")

        def stored(name: str) -> np.ndarray:
            with Image.open(MAP / name) as image:
                return np.asarray(image).astype(int)

        mean, half, gdd = (stored(n) for n in CHANGED[:3])
        for name, ((latlon, cell), want_mean, want_half, want_gdd) in PLACES.items():
            if latlon is not None:
                x, y = crop.cell_of(*latlon)
                row, column = math.floor(y), math.floor(x)
            else:
                assert cell is not None
                column, row = cell[0] - crop.origin[0], cell[1] - crop.origin[1]
            self.assertEqual(
                (mean[row, column], half[row, column], gdd[row, column]),
                (want_mean, want_half, want_gdd),
                name,
            )

    def test_sources_lists_the_new_hashes_and_the_backdrops_three_inputs(self) -> None:
        sources = (MAP / "SOURCES.md").read_text()
        for name in (*CHANGED, "edits.toml", "edits.applied"):
            self.assertIn(sha256(MAP / name), sources, name)
        for name in ("backdrop.toml", "land.png", "sea_temperature.png"):
            self.assertIn(sha256(BACKDROP / name), sources, name)
            self.assertIn(f"../backdrop/{name}", sources, name)

    def test_the_shipped_marker_is_the_hash_of_the_shipped_edits(self) -> None:
        self.assertEqual(
            (MAP / "edits.applied").read_text(), f"{sha256(MAP / 'edits.toml')}  edits.toml\n"
        )

    def test_the_shipped_layers_have_no_steps_over_land(self) -> None:
        def stored(name: str) -> np.ndarray:
            with Image.open(MAP / name) as image:
                return np.asarray(image).astype(np.float64)

        with Image.open(MAP / "terrain.png") as image:
            rgb = np.asarray(image.convert("RGB"))
        keys = edits.terrain_keys(MAP / "terrain.toml")
        sea = (rgb == keys["sea"]).all(axis=2)
        height = stored("elevation.png") - edits.ELEVATION_OFFSET
        edits.check_no_steps(
            stored("climate_temperature.png"),
            stored("climate_half_range.png"),
            height,
            sea,
            CLIMATE["lapse_c_per_km"],
        )


if __name__ == "__main__":
    unittest.main()
