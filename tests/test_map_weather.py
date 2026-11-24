"""Contracts for the generated weather lookup tables and sea-distance layer."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import math
import shutil
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from types import SimpleNamespace

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
            _, half, _, _, _ = edits.climate_fields(state, settings["climate"])
            edits.compute_layers(state, settings)
            with Image.open(directory / "climate_half_range.png") as image:
                np.testing.assert_array_equal(np.asarray(image), np.floor(half * 10 + 0.5))
            manifest = tomllib.loads((directory / "map.toml").read_text())
            self.assertEqual(manifest["climate"]["warmest_month_phase_hundredths"], 650)
            self.assertEqual(manifest["layers"]["climate_sea_nearness"], "climate_sea_nearness.png")
            valid = copy.deepcopy(settings)
            valid["climate"]["warmest_month_phase"] = 0.07
            edits.compute_layers(state, valid)
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
                    edits.compute_layers(state, bad)

    def test_ireland_regenerates_without_changing_existing_layers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            destination = Path(tmp) / "map"
            shutil.copytree(MAP, destination)
            marker = (destination / "edits.applied").read_bytes()
            edits.recompute_layers(destination)
            for name in (
                "terrain.png",
                "elevation.png",
                "fertility.png",
                "climate_glacier.png",
                "climate_temperature.png",
                "climate_gdd.png",
                "climate_rainfall.png",
                "climate_half_range.png",
                "climate_sea_nearness.png",
            ):
                self.assertEqual((destination / name).read_bytes(), (MAP / name).read_bytes(), name)
            self.assertEqual((destination / "edits.applied").read_bytes(), marker)
            self.assertEqual(
                marker.decode().split()[0],
                hashlib.sha256((MAP / "edits.toml").read_bytes()).hexdigest(),
            )

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
        _, half_ranges, _, _, _ = edits.climate_fields(state, settings)
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


if __name__ == "__main__":
    unittest.main()
