"""Contracts for the world backdrop's geometry and land sampling."""

import importlib.util
import sys
import tempfile
import tomllib
import unittest
from datetime import date, timedelta
from pathlib import Path

import numpy as np
from PIL import Image

PATH = Path(__file__).resolve().parents[1] / "assets/map/backdrop/geometry.py"
sys.path.insert(0, str(PATH.parent))
spec = importlib.util.spec_from_file_location("backdrop_geometry", PATH)
assert spec is not None and spec.loader is not None
geometry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(geometry)


class GeometryTests(unittest.TestCase):
    def test_bounds_and_centres(self) -> None:
        grid = geometry.Grid(64, 75, -60)
        self.assertEqual((grid.columns, grid.first_row, grid.rows), (747, 132, 398))
        self.assertEqual(grid.block_of(53.0956, -7.9101), (356, 242))
        self.assertEqual(grid.block_of(0, 180), grid.block_of(0, -180))
        lat, lon = grid.centre(746, 373)
        self.assertAlmostEqual(lat, -0.1612, places=4)
        self.assertAlmostEqual(lon, -179.9187, places=4)
        large = geometry.Grid(4096, 75, -60)
        self.assertEqual((large.columns, large.first_row, large.rows), (12, 2, 7))

    def test_invalid_settings_name_the_setting(self) -> None:
        for size in (0, 8, 15, 17, 24, 8208, 16384):
            with self.assertRaisesRegex(ValueError, "block_cells"):
                geometry.Grid(size, 75, -60)
        for north, south in ((60, 75), (86, 0), (90, 0), (0, -90)):
            with self.assertRaisesRegex(ValueError, "bounds"):
                geometry.Grid(64, north, south)

    def test_polygon_union_and_holes(self) -> None:
        outer = [[-2, -2], [2, -2], [2, 2], [-2, 2], [-2, -2]]
        hole = [[-1, -1], [1, -1], [1, 1], [-1, 1], [-1, -1]]
        self.assertTrue(geometry.on_land(1.5, 0, [[outer, hole]]))
        self.assertFalse(geometry.on_land(0, 0, [[outer, hole]]))
        self.assertTrue(geometry.on_land(0, 0, [[outer, hole], [hole]]))
        self.assertFalse(geometry.on_land(3, 0, [[outer]]))

    def test_lattice_half_and_wrapped_last_column(self) -> None:
        grid = geometry.Grid(8192, 1, -1)
        col, row = 0, grid.first_row
        x0 = col * grid.block_cells * geometry.CELL
        x1 = (col + 0.5) * grid.block_cells * geometry.CELL
        y0 = row * grid.block_cells * geometry.CELL
        y1 = (row + 1) * grid.block_cells * geometry.CELL
        north, west = geometry.unproject(x0, y0)
        south, east = geometry.unproject(x1, y1)
        ring = [[west, south], [east, south], [east, north], [west, north], [west, south]]
        counts = geometry.land_counts(grid, [[ring]])
        self.assertEqual(counts[0, 0], 32)
        _, narrower_east = geometry.unproject((col + 3 / 8) * grid.block_cells * geometry.CELL, y1)
        narrow = [
            [west, south],
            [narrower_east, south],
            [narrower_east, north],
            [west, north],
            [west, south],
        ]
        self.assertEqual(geometry.land_counts(grid, [[narrow]])[0, 0], 24)
        # Removing the central quarter as a hole and adding it back tests both subtraction and union.
        _, middle_west = geometry.unproject((col + 1 / 8) * grid.block_cells * geometry.CELL, y0)
        _, middle_east = geometry.unproject((col + 3 / 8) * grid.block_cells * geometry.CELL, y0)
        hole = [
            [middle_west, south],
            [middle_east, south],
            [middle_east, north],
            [middle_west, north],
            [middle_west, south],
        ]
        self.assertEqual(geometry.land_counts(grid, [[ring, hole]])[0, 0], 16)
        self.assertEqual(geometry.land_counts(grid, [[ring, hole], [hole]])[0, 0], 32)
        # The partial last column wraps its final sample points to the western edge.
        last = grid.columns - 1
        expected = sum(geometry.on_land(lon, lat, [[ring]]) for lat, lon in grid.samples(last, row))
        self.assertEqual(counts[0, last], expected)
        self.assertGreater(expected, 0)

    def test_real_land_sample_counts(self) -> None:
        grid = geometry.Grid(64, 75, -60)
        path = PATH.parent / "sources/ne_50m_land.geojson"
        counts = geometry.land_counts(grid, geometry.load_polygons(path))
        points = [
            (53.0956, -7.9101, 64),
            (54.3489, -6.6545, 64),
            (52, -12, 0),
            (51.94, -10.24, 9),
            (54.6, -6.4, 64),
            (54.2, -4.5, 19),
            (62, -6.9, 2),
            (64.1, -21.9, 29),
            (52.5, 5.5, 64),
            (31.5, 35.5, 64),
            (42, 51, 0),
            (-1, 33, 64),
            (51.4779, 0, 64),
            (0, 0, 0),
            (0, 179.99, 0),
            (0, -180, 0),
        ]
        for lat, lon, expected in points:
            col, row = grid.block_of(lat, lon)
            self.assertEqual(counts[row - grid.first_row, col], expected, (lat, lon))

    def test_land_threshold(self) -> None:
        self.assertTrue(geometry.is_land(32))
        self.assertFalse(geometry.is_land(24))


class NormalReductionTests(unittest.TestCase):
    def test_median_rounding_and_missing(self) -> None:
        reduction = importlib.import_module("reduction")

        self.assertEqual(
            reduction.pressure_median(np.array([101000, 101200, 97000, 101100, 101300])), 101100
        )
        self.assertEqual(
            reduction.pressure_median(np.array([100000, 100100, 100200, 100300])), 100150
        )
        self.assertEqual(reduction.pressure_median(np.array([100000, 100001])), 100001)
        np.testing.assert_array_equal(reduction.half_away(np.array([-0.5, 0.5, -0.4])), [-1, 1, 0])
        self.assertEqual(reduction.air_mean(np.array([273.15, 275.15])), 100)
        with self.assertRaisesRegex(ValueError, "missing"):
            reduction.pressure_median(np.array([100000, np.nan]))
        with self.assertRaisesRegex(ValueError, "missing"):
            reduction.air_mean(np.array([273.15, np.nan]))

    def test_sea_needs_all_months_and_years(self) -> None:
        reduction = importlib.import_module("reduction")

        values = np.ones((47, 12, 1, 2)) * 10
        values[5, 3, 0, 1] = np.nan
        result = reduction.sea_normals(values)
        self.assertEqual(result[:, 0, 0].tolist(), [11000] * 12)
        self.assertEqual(result[:, 0, 1].tolist(), [0] * 12)
        values[:] = np.nan
        with self.assertRaisesRegex(ValueError, "sea_mean.png.*month 1"):
            reduction.sea_normals(values)

    def test_axis_order(self) -> None:
        reduction = importlib.import_module("reduction")

        field = np.array([[1, 2, 3], [4, 5, 6]])
        result = reduction.order_axes(field, np.array([-1, 1]), np.array([2, 0, 1]))
        np.testing.assert_array_equal(result, [[5, 6, 4], [2, 3, 1]])
        western_longitudes = np.array([-180, -90, 0, 90])
        southern_first = np.array([[1, 2, 3, 4], [5, 6, 7, 8]])
        canonical = reduction.order_axes(southern_first, np.array([-1, 1]), western_longitudes)
        np.testing.assert_array_equal(canonical, [[7, 8, 5, 6], [3, 4, 1, 2]])
        np.testing.assert_array_equal(
            canonical,
            reduction.order_axes(southern_first[::-1], np.array([1, -1]), western_longitudes),
        )

    def test_february_dates_include_leap_day_without_assuming_order(self) -> None:
        reduction = importlib.import_module("reduction")
        for year, count in ((1836, 29), (1837, 28)):
            dates = [date(year, 2, 1) + timedelta(days=day) for day in range(count)][::-1]
            indexes = reduction.month_indices(dates, year, 2, "pressure.nc")
            self.assertEqual([dates[i].day for i in indexes], list(range(1, count + 1)))
            with self.assertRaisesRegex(ValueError, "pressure.nc.*month 2"):
                reduction.month_indices(dates[:-1], year, 2, "pressure.nc")

    def test_wrong_digest_names_the_file(self) -> None:
        fetch = importlib.import_module("fetch")
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "wrong.nc"
            path.write_bytes(b"wrong")
            with self.assertRaisesRegex(ValueError, "wrong.nc.*SHA-256"):
                fetch.verify(path, "0" * 64)

    def test_fill_and_interpolation(self) -> None:
        build = importlib.import_module("build")

        np.testing.assert_array_equal(
            build.fill(np.array([[10, np.nan, np.nan, np.nan, 20.0]]), 1), [[10, 10, 15, 20, 20]]
        )
        with self.assertRaisesRegex(ValueError, "sea_mean.png.*month 4"):
            build.fill(np.full((2, 3), np.nan), 4)
        self.assertEqual(build.bilinear(10, 20, 30, 40, 0.25, 0.5), 22.5)
        neighbours = np.array([[1.0, 2.0, 3.0], [np.nan, np.nan, np.nan], [np.nan, np.nan, np.nan]])
        self.assertEqual(build.fill(neighbours, 1)[1, 1], 2.0)
        field = np.array([[30.0, 40.0, 50.0], [10.0, 20.0, 30.0]])
        self.assertAlmostEqual(
            build.interpolate(field, 0.5, 300, north=1, spacing=120, lat_step=1), 30.0
        )


class RebuildTests(unittest.TestCase):
    def test_shipped_and_large_fixture_rebuild_byte_for_byte(self) -> None:
        build = importlib.import_module("build")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = PATH.parent / "sources"
            settings = PATH.parent / "settings.toml"
            output = root / "out"
            build.build(settings, source, output)
            manifest = tomllib.loads((output / "backdrop.toml").read_text())
            self.assertEqual(manifest["format"], 2)
            self.assertEqual(len(manifest["row_latitude_e4"]), manifest["rows"])
            self.assertEqual(
                (manifest["row_latitude_e4"][0], manifest["row_latitude_e4"][-1]), (749780, -599766)
            )
            for name in (
                "backdrop.toml",
                "land.png",
                "pressure.png",
                "air_temperature.png",
                "sea_temperature.png",
            ):
                self.assertEqual(
                    (output / name).read_bytes(), (PATH.parent / name).read_bytes(), name
                )
            large_settings = root / "settings.toml"
            large_settings.write_text(
                settings.read_text().replace("block_cells = 64", "block_cells = 4096")
            )
            build.build(large_settings, source, output)
            fixture = PATH.parents[3] / "crates/map/tests/fixtures/backdrop"
            for name in (
                "backdrop.toml",
                "land.png",
                "pressure.png",
                "air_temperature.png",
                "sea_temperature.png",
            ):
                self.assertEqual((output / name).read_bytes(), (fixture / name).read_bytes(), name)
            edge_settings = (
                settings.read_text()
                .replace("block_cells = 64", "block_cells = 8192")
                .replace("north = 75.0", "north = 85.0")
                .replace("south = -60.0", "south = -85.0")
            )
            large_settings.write_text(edge_settings)
            build.build(large_settings, source, output)
            fixture = PATH.parents[3] / "crates/map/tests/fixtures/backdrop8192"
            for name in (
                "backdrop.toml",
                "land.png",
                "pressure.png",
                "air_temperature.png",
                "sea_temperature.png",
            ):
                self.assertEqual((output / name).read_bytes(), (fixture / name).read_bytes(), name)


class ReportTests(unittest.TestCase):
    def test_complete_blocks_and_partial_coverage(self) -> None:
        report = importlib.import_module("report")
        # Crop contains two complete 16-cell blocks; backdrop covers the northern one.
        squares, compared = report.comparable_blocks(0, 0, 16, 32, 16, 0, 1, 2986)
        self.assertEqual(squares, [(0, 0), (0, 1)])
        self.assertEqual(compared, [(0, 0)])
        squares, compared = report.comparable_blocks(0, 0, 16, 32, 16, 3, 1, 2986)
        self.assertEqual((len(squares), len(compared)), (2, 0))
        self.assertEqual(report.comparable_blocks(0, 0, 8, 8, 16, 0, 1, 2986), ([], []))
        # The partial final world column is never compared.
        squares, _ = report.comparable_blocks(47760, 0, 16, 16, 16, 0, 1, 2986)
        self.assertEqual(squares, [])


class ReportCoverageTests(unittest.TestCase):
    def test_partial_and_absent_coverage(self) -> None:
        report = importlib.import_module("report")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            backdrop, region = root / "backdrop", root / "region"
            backdrop.mkdir()
            region.mkdir()
            manifest = """format = 2
projection = "spherical_mercator"
cell_metres = 839.0
block_cells = 16
first_row = 1000
rows = 1
columns = 2986
[layers]
land = "land.png"
pressure = "pressure.png"
air_temperature = "air_temperature.png"
sea_temperature = "sea_temperature.png"
"""
            (backdrop / "backdrop.toml").write_text(manifest)
            Image.fromarray(np.full((1, 2986), 255, dtype=np.uint8)).save(backdrop / "land.png")
            for name, value in (
                ("pressure", 41174),
                ("air_temperature", 1035),
                ("sea_temperature", 0),
            ):
                Image.fromarray(np.full((12, 2986), value, dtype=np.uint16)).save(
                    backdrop / f"{name}.png"
                )
            (region / "map.toml").write_text("""[crop]
origin = [24000, 16000]
width = 16
height = 32
[layers]
climate_temperature = "mean.png"
climate_half_range = "range.png"
terrain = "terrain.png"
terrain_rules = "terrain.toml"
""")
            (region / "terrain.toml").write_text('[[terrain]]\nkind = "sea"\nkey = "0a1a33"\n')
            Image.fromarray(np.full((32, 16), 1080, dtype=np.uint16)).save(region / "mean.png")
            Image.fromarray(np.full((32, 16), 40, dtype=np.uint16)).save(region / "range.png")
            Image.fromarray(np.full((32, 16, 3), 100, dtype=np.uint8)).save(region / "terrain.png")

            def position(i: int, j: int) -> tuple[float, float]:
                lat, lon = geometry.unproject((i + 0.5) * geometry.CELL, (j + 0.5) * geometry.CELL)
                return float(lat), float(lon)

            locs = [position(24001, 16001), position(24002, 16002), position(24001, 16020)]
            (root / "locations.toml").write_text(
                "".join(
                    f'[[location]]\nid = "p{n}"\nlat = {lat}\nlon = {lon}\n'
                    for n, (lat, lon) in enumerate(locs)
                )
            )
            (root / "points.toml").write_text(
                "".join(
                    f'[[point]]\nname = "p{n}"\nlat = {lat}\nlon = {lon}\n'
                    for n, (lat, lon) in enumerate(locs[:1] + locs[2:])
                )
            )
            text = report.generate(backdrop, region, root / "locations.toml", root / "points.toml")
            self.assertIn("2 of 3", text)
            self.assertIn("p2: map", text)
            self.assertIn("2 complete squares, 1 compared, 1 agree", text)
            self.assertIn("p1: block (1500, 1001); outside the backdrop", text)
            land = np.full((1, 2986), 255, dtype=np.uint8)
            land[0, 1500] = 0
            Image.fromarray(land).save(backdrop / "land.png")
            sea = np.zeros((12, 2986), dtype=np.uint16)
            sea[:, 1500] = 1000
            Image.fromarray(sea).save(backdrop / "sea_temperature.png")
            text = report.generate(backdrop, region, root / "locations.toml", root / "points.toml")
            self.assertIn("1 compared, 0 agree; differing: [(1500, 1000)]", text)
            self.assertIn("sea [0, 0, 0", text)
            (backdrop / "backdrop.toml").write_text(
                manifest.replace("first_row = 1000", "first_row = 1002")
            )
            text = report.generate(backdrop, region, root / "locations.toml", root / "points.toml")
            self.assertIn("no place lies inside the backdrop", text)
            self.assertIn("2 complete squares, 0 held; none compared", text)
            self.assertEqual(text.count("outside the backdrop"), 5)
            (root / "locations.toml").write_text('[[location]]\nid = "bad"\nlat = 0\nlon = 0\n')
            with self.assertRaisesRegex(ValueError, "bad.*outside region crop"):
                report.generate(backdrop, region, root / "locations.toml", root / "points.toml")


class IrelandReportTests(unittest.TestCase):
    def test_large_fixture_reports_all_places_and_no_complete_block(self) -> None:
        report = importlib.import_module("report")
        root = PATH.parents[3]
        text = report.generate(
            root / "crates/map/tests/fixtures/backdrop",
            root / "assets/map/ireland",
            root / "assets/scenarios/ireland-768/locations.toml",
            PATH.parent / "points.toml",
        )
        self.assertIn("125 of 125", text)
        self.assertIn("0 complete squares, 0 held; none compared", text)
        for name in ("Sea west of Kerry", "Valentia", "Malin Head", "Birr"):
            self.assertIn(f"{name}: block (5, 3)", text)


class ReportHalfShareTests(unittest.TestCase):
    def test_half_sea_and_place_figures_at_both_block_sizes(self) -> None:
        report = importlib.import_module("report")
        for size in (64, 128):
            with self.subTest(size=size), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                backdrop, region = root / "backdrop", root / "region"
                backdrop.mkdir()
                region.mkdir()
                columns = (geometry.WORLD_CELLS + size - 1) // size
                first, column = 200, 10
                (backdrop / "backdrop.toml").write_text(f"""format = 2
projection = "spherical_mercator"
cell_metres = 839.0
block_cells = {size}
first_row = {first}
rows = 2
columns = {columns}
[layers]
land = "land.png"
pressure = "pressure.png"
air_temperature = "air_temperature.png"
sea_temperature = "sea_temperature.png"
""")
                Image.fromarray(np.full((2, columns), 255, dtype=np.uint8)).save(
                    backdrop / "land.png"
                )
                Image.fromarray(np.full((24, columns), 41174, dtype=np.uint16)).save(
                    backdrop / "pressure.png"
                )
                air = np.repeat((1030 + np.arange(12, dtype=np.uint16))[:, None, None], 2, axis=1)
                air = np.repeat(air, columns, axis=2).reshape(24, columns)
                Image.fromarray(air).save(backdrop / "air_temperature.png")
                Image.fromarray(np.zeros((24, columns), dtype=np.uint16)).save(
                    backdrop / "sea_temperature.png"
                )
                x, y = column * size, first * size
                (region / "map.toml").write_text(f"""[crop]
origin = [{x}, {y}]
width = {size}
height = {2 * size}
[layers]
climate_temperature = "mean.png"
climate_half_range = "range.png"
terrain = "terrain.png"
terrain_rules = "terrain.toml"
""")
                (region / "terrain.toml").write_text('[[terrain]]\nkind = "sea"\nkey = "0a1a33"\n')
                Image.fromarray(np.full((2 * size, size), 1080, dtype=np.uint16)).save(
                    region / "mean.png"
                )
                Image.fromarray(np.full((2 * size, size), 40, dtype=np.uint16)).save(
                    region / "range.png"
                )
                terrain = np.full((2 * size, size, 3), 100, dtype=np.uint8)
                terrain[:size, : size // 2] = [10, 26, 51]
                terrain[size:, :] = [10, 26, 51]
                Image.fromarray(terrain).save(region / "terrain.png")
                lat, lon = geometry.unproject((x + 0.5) * geometry.CELL, (y + 0.5) * geometry.CELL)
                (root / "locations.toml").write_text(
                    f'[[location]]\nid = "witness"\nlat = {lat}\nlon = {lon}\n'
                )
                (root / "points.toml").write_text(
                    f'[[point]]\nname = "witness"\nlat = {lat}\nlon = {lon}\n'
                )
                text = report.generate(
                    backdrop, region, root / "locations.toml", root / "points.toml"
                )
                self.assertIn(
                    "witness: map (80, 40, 120); backdrop (36, 30, 41); differences (-44, -10, -79)",
                    text,
                )
                self.assertIn(
                    "Places summary (1 of 1): mean, least, greatest differences (year, coldest, warmest): [(-44.0, -44, -44), (-10.0, -10, -10), (-79.0, -79, -79)]",
                    text,
                )
                self.assertIn(
                    f"block ({column}, {first}): map land share 0.500000; backdrop land", text
                )
                self.assertIn(
                    f"2 complete squares, 2 compared, 1 agree; differing: [({column}, {first + 1})]",
                    text,
                )
                self.assertIn("sea land", text)


if __name__ == "__main__":
    unittest.main()
