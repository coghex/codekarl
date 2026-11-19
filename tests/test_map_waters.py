"""CK-348: the rivers import tooling (`assets/map/ireland/import_waters.py`): the list, the network, the widths, the cells and the mask.

The tool's rules are plain functions over plain values, so these tests run in the project's own Python with small hand-written
inputs: no numpy, no Pillow, no download.
"""

import hashlib
import importlib
import importlib.util
import json
import math
import re
import struct
import sys
import tempfile
import tomllib
import types
import unittest
import zipfile
import zlib
from collections.abc import Callable, Iterable, Sequence
from itertools import pairwise
from pathlib import Path
from typing import Any, ClassVar
from unittest import mock

MODULE_PATH = Path(__file__).resolve().parents[1] / "assets/map/ireland/import_waters.py"


RIVER_GML = """<?xml version="1.0" encoding="UTF-8"?>
<gml:FeatureCollection xmlns:gml="http://www.opengis.net/gml" xmlns:fme="http://www.safe.com/gml/fme">
<gml:featureMember><fme:ehsgis_EHS_RiverSegment gml:id="a"><fme:OBJECTID>1</fme:OBJECTID><fme:RWB_CD>UKGBNI1NW393901002</fme:RWB_CD>
<fme:STRAHLER>4</fme:STRAHLER><gml:curveProperty><gml:LineString srsName="EPSG:29902" srsDimension="2"><gml:posList>189381.6903 400454.2952 189400.0 400500.0</gml:posList></gml:LineString></gml:curveProperty>
</fme:ehsgis_EHS_RiverSegment></gml:featureMember>
<gml:featureMember><fme:ehsgis_EHS_RiverSegment gml:id="b"><fme:OBJECTID>2</fme:OBJECTID><fme:RWB_CD>UKGBNI1NW393901003</fme:RWB_CD>
<fme:STRAHLER>2</fme:STRAHLER><gml:curveProperty><gml:LineString srsName="EPSG:29902" srsDimension="2"><gml:posList>189500.0 400500.0 189600.0 400600.0</gml:posList></gml:LineString></gml:curveProperty>
</fme:ehsgis_EHS_RiverSegment></gml:featureMember>
</gml:FeatureCollection>"""
WATER_GML = """<?xml version="1.0" encoding="utf-8" ?>
<ogr:FeatureCollection xmlns:ogr="http://ogr.maptools.org/" xmlns:gml="http://www.opengis.net/gml">
<gml:featureMember><ogr:TransitionalWaterBodies fid="a"><ogr:geometryProperty><gml:MultiPolygon srsName="EPSG:29902"><gml:polygonMember><gml:Polygon>
<gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>189000,400000 190000,400000 190000,401000 189000,400000</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs>
<gml:innerBoundaryIs><gml:LinearRing><gml:coordinates>189300,400200 189400,400200 189300,400300 189300,400200</gml:coordinates></gml:LinearRing></gml:innerBoundaryIs>
</gml:Polygon></gml:polygonMember></gml:MultiPolygon></ogr:geometryProperty><ogr:EU_CD>UKGBNI5NB010010</ogr:EU_CD><ogr:NAME>Bann Estuary</ogr:NAME><ogr:Shape_Area>500000</ogr:Shape_Area></ogr:TransitionalWaterBodies></gml:featureMember>
</ogr:FeatureCollection>"""


def load_module() -> types.ModuleType:
    spec = importlib.util.spec_from_file_location("import_waters", MODULE_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["import_waters"] = module
    spec.loader.exec_module(module)
    return module


W = load_module()
CROP = W.Crop((22436, 14979), 64, 32)
SEA_FROM = (
    48  # columns from here are sea; a cell is open sea from column 50 (the 5 x 5 box must be sea)
)
TERRAIN = W.Terrain(
    CROP, [[W.SEA if c >= SEA_FROM else W.PASTURE for c in range(64)] for r in range(32)]
)
CellList = Iterable[tuple[int, int]]
# The tool is loaded from its path, so what it returns is dynamically typed in these tests.
Dyn = Any


def centre(column: int, row: int) -> tuple[float, float]:
    point: tuple[float, float] = W.round_point(CROP.latlon_at(column + 0.5, row + 0.5))
    return point


def terrain_rows(
    sea: Callable[[int, int], bool] = lambda c, r: c >= SEA_FROM,
    lakes: CellList = (),
    width: int = 64,
    height: int = 32,
) -> list[list[int]]:
    rows = [[W.SEA if sea(c, r) else W.PASTURE for c in range(width)] for r in range(height)]
    for column, row in lakes:
        rows[row][column] = W.LAKE
    return rows


def segment(  # noqa: PLR0913  # the fields of a survey line, each optional
    code: str,
    cells: Sequence[tuple[int, int]],
    name: str | None = None,
    order: int = 5,
    continua: str | None = None,
    lake: str | None = None,
    source: str = "epa",
    body: str | None = None,
) -> Dyn:
    return W.Seg(
        code, source, order, tuple(centre(c, r) for c, r in cells), name, body, continua, lake
    )


def river(identifier: str, *names: str, tier: int = 3) -> Dyn:
    return W.RiverDef(identifier, identifier.title(), tier, epa_names=tuple(names))


def build(
    segments: Sequence[Dyn],
    rivers: Sequence[Dyn],
    rows: list[list[int]] | None = None,
    **options: Dyn,
) -> Dyn:
    return W.build_network(segments, rivers, W.Terrain(CROP, rows or terrain_rows()), **options)


def signature(network: Dyn) -> list[tuple[Any, ...]]:
    return [(s.id, s.river, s.kind, s.ends, tuple(s.points)) for s in network.stretches]


def reading(areas: dict[tuple[int, int], float]) -> Callable[[tuple[float, float], int], float]:
    """An area reader that gives the area of each cell, and 5 km2 anywhere else."""

    def read(point: tuple[float, float], reach: int) -> float:
        return areas.get(CROP.cell_containing(point), 5.0)

    return read


def check_network(case: unittest.TestCase, network: Dyn, terrain: Dyn | None = None) -> None:
    """Format rule 3: every `ends` names a stretch whose first point is this last point, and every stretch reaches the sea."""
    terrain = terrain or W.Terrain(CROP, terrain_rows())
    by_id = {s.id: s for s in network.stretches}
    for stretch in by_id.values():
        if stretch.ends == "sea":
            case.assertTrue(terrain.open_sea(stretch.points[-1]), stretch.id)
        else:
            case.assertIn(stretch.ends, by_id)
            case.assertEqual(stretch.points[-1], by_id[stretch.ends].points[0], stretch.id)
        seen: set[str] = set()
        here = stretch
        while here.ends != "sea":
            case.assertNotIn(here.id, seen)
            seen.add(here.id)
            here = by_id[here.ends]


# The river used by most network tests: a main river of two surveyed pieces, its tidal end and a tributary.
MAIN_1 = segment("m1", [(2, 10), (25, 10)], "Main")
MAIN_2 = segment("m2", [(25, 10), (46, 10)], "Main")
TIDAL = segment("t1", [(46, 10), (48, 10), (50, 10), (55, 10)], None, continua="T")
TRIB = segment("b1", [(25, 3), (25, 6), (25, 10)], "Trib")
MAIN = (MAIN_1, MAIN_2, TIDAL)


class WatersList(unittest.TestCase):
    GOOD = """
[settings]
a = 2.3

[[river]]
id = "suir"
name = "Suir"
tier = 1
source = { epa_names = ["Suir"] }

[[lough]]
name = "Neagh"
source = "UKNI1"
mode = "redrawn"
keep_existing = true
reason = "the owner's edit lough-neagh-higher"

[[keep]]
id = "island"
reason = "the owner's island edit"
point = [52.9, -8.4]

[[reservoir]]
name = "Poulaphouca"
source = "IE_EA_09_1"
reason = "modern"

[[correction]]
id = "shannon-bed"
reason = "wider than its area suggests"
confidence = "medium"
river = "suir"
width_factor = 2.2
"""

    def test_a_good_file_parses(self) -> None:
        waters = W.parse_waters(self.GOOD)
        self.assertEqual([r.id for r in waters.rivers], ["suir"])
        self.assertTrue(waters.loughs[0].keep_existing)
        self.assertEqual(waters.settings.tier_steps, (0, 14, 24))
        self.assertEqual(waters.corrections[0].width_factor, 2.2)

    def refused(self, text: str, *words: str) -> None:
        with self.assertRaises(W.WatersError) as raised:
            W.parse_waters(text)
        for word in words:
            self.assertIn(word, str(raised.exception))

    def test_an_unknown_key_is_refused_naming_the_entry(self) -> None:
        self.refused(
            self.GOOD.replace("tier = 1\n", 'tier = 1\ncolour = "blue"\n'),
            "[[river]] #1 (suir)",
            "colour",
        )
        self.refused(
            self.GOOD + '\n[[lough]]\nname = "Ree"\nsource = "x"\nmode = "added"\ndepth = 3\n',
            "[[lough]] #2 (Ree)",
            "depth",
        )
        self.refused("[settings]\nwidth = 3\n", "[settings]", "width")

    def test_an_empty_name_is_refused_naming_the_river(self) -> None:
        self.refused(
            self.GOOD.replace('name = "Suir"', 'name = ""'), "[[river]] #1 (suir)", "name is empty"
        )

    def test_a_correction_and_a_lough_are_checked(self) -> None:
        self.refused(
            self.GOOD.replace('river = "suir"\nwidth_factor', 'river = "nobody"\nwidth_factor'),
            "nobody",
            "not in the list",
        )
        self.refused(
            self.GOOD.replace('mode = "redrawn"', 'mode = "added"'), "keep_existing", "redrawn"
        )
        self.refused(
            self.GOOD.replace("width_factor = 2.2", 'width_factor = 2.2\nomit = ["a"]'),
            "exactly one",
        )
        self.refused(self.GOOD.replace('mode = "redrawn"', 'mode = "drained"'), "mode")

    def test_a_malformed_source_list_is_refused_naming_the_river_and_the_key(self) -> None:
        """Review 1, finding 4: a string, a table or a number is not a list of codes."""
        for key, value in (
            ("segments", '"AB"'),
            ("segments", "{ EPA = true }"),
            ("segments", "5"),
            ("epa_names", '"Suir"'),
        ):
            self.refused(
                self.GOOD.replace(
                    'source = { epa_names = ["Suir"] }', f"source = {{ {key} = {value} }}"
                ),
                "[[river]] #1 (suir)",
                key,
                "list of non-empty strings",
            )
        self.refused(
            self.GOOD.replace(
                'source = { epa_names = ["Suir"] }', 'source = { segments = ["A", ""] }'
            ),
            "segments",
        )
        mixed = W.parse_waters(
            self.GOOD.replace(
                'source = { epa_names = ["Suir"] }',
                'source = { epa_names = ["Suir"], rwb_codes = ["UKNI-B1"], segments = [".12204", "36_GBNI1"] }',
            )
        )
        river_def = mixed.rivers[0]
        self.assertEqual(
            (river_def.epa_names, river_def.rwb_codes, river_def.segments),
            (("Suir",), ("UKNI-B1",), (".12204", "36_GBNI1")),
        )

    def test_a_source_matching_no_segment_is_refused_naming_the_river(self) -> None:
        with self.assertRaises(W.WatersError) as raised:
            build([MAIN_1, MAIN_2, TIDAL], [river("ghost", "Nowhere")])
        self.assertIn("river ghost", str(raised.exception))
        self.assertIn("matches no segment", str(raised.exception))

    def test_a_river_ending_at_a_river_not_in_the_list_is_refused(self) -> None:
        with self.assertRaises(W.WatersError) as raised:
            build([*MAIN, TRIB], [river("trib", "Trib")])
        self.assertIn("river trib", str(raised.exception))
        self.assertIn("Main", str(raised.exception))
        self.assertIn("not in the list", str(raised.exception))

    def test_a_river_that_cannot_be_built_is_left_out_when_tolerant(self) -> None:
        network = build(
            [*MAIN, TRIB], [river("trib", "Trib"), river("main", "Main")], tolerant=True
        )
        self.assertEqual(network.failed, {})
        self.assertEqual({s.river for s in network.stretches}, {"main", "trib"})
        lost = segment("x", [(2, 20), (10, 20)], "Lost")
        network = build(
            [*MAIN, lost], [river("lost", "Lost"), river("main", "Main")], tolerant=True
        )
        self.assertEqual(set(network.failed), {"lost"})
        self.assertIn("inland", network.failed["lost"])
        self.assertEqual({s.river for s in network.stretches}, {"main"})


class Proposal(unittest.TestCase):
    """`propose`: a first list by the rule the mock-ups used."""

    def surveys(self) -> Dyn:
        long_river = [
            segment(f"a{i}", [(2 + 2 * i, 10), (4 + 2 * i, 10)], "Alpha", order=5)
            for i in range(20)
        ]
        long_river[-1] = segment("a19", [(40, 10), (45, 10)], "Alpha", order=5)
        short = [
            segment("s1", [(2, 20), (8, 20)], "Short", order=5),
            segment("s2", [(8, 20), (12, 20)], "Short", order=3),
        ]
        ni = [
            segment("n1.1", [(2, 25), (20, 25)], source="ni", order=5, body="UKNI-B1"),
            segment("n2.2", [(2, 28), (9, 28)], source="ni", order=4, body="UKNI-B2"),
        ]
        tidal = segment("t1", [(45, 10), (50, 10), (55, 10)], None, continua="T")

        def lake(code: str, name: str, area: float, cells: tuple[int, int, int, int]) -> Dyn:
            return W.Outline(code, name, area, (self.square(cells),), "epa")

        lakes = [
            lake(
                "L1", "Gill", 1.5, (10, 8, 13, 12)
            ),  # touched by Alpha, none of it a lake today: added
            lake("L2", "Redrawn", 2.0, (20, 14, 24, 17)),  # sits on today's lake: redrawn
            lake("L3", "Far", 3.0, (30, 28, 34, 31)),  # no river near it: left out
            lake("L4", "Lough Neagh", 5.0, (50 - 14, 20, 40, 23)),
            lake("L5", "Carrigadrohid Reservoir", 4.0, (6, 14, 9, 17)),
        ]
        surveys = W.Surveys(rivers=[*long_river, *short, *ni], tidal=[], lakes=lakes)
        surveys.rivers.append(tidal)
        return surveys

    @staticmethod
    def square(cells: tuple[int, int, int, int]) -> list[list[tuple[float, float]]]:
        c0, r0, c1, r1 = cells
        return [[CROP.latlon_at(c, r) for c, r in ((c0, r0), (c1, r0), (c1, r1), (c0, r1))]]

    def test_the_rule_gives_the_expected_rivers_tiers_loughs_and_reservoirs(self) -> None:
        rows = terrain_rows(
            lakes=[(c, r) for c in range(20, 24) for r in range(14, 17)]
            + [(c, r) for c in range(36, 40) for r in range(20, 23)]
        )
        grid = W.MapGrid(CROP, rows, [[10.0] * 64 for _ in range(32)])
        surveys = self.surveys()
        text = W.propose_waters(
            surveys,
            grid,
            area_source=lambda point, reach: 1500.0
            if abs(CROP.cell_of(*point)[1] - 10.5) < 1
            else 5.0,
        )
        document = __import__("tomllib").loads(text)
        names = {r["id"]: r for r in document["river"]}
        self.assertEqual(names["alpha"]["source"], {"epa_names": ["Alpha"]})
        self.assertEqual(
            names["alpha"]["tier"], 2
        )  # a headwater of 21 km can gather at most 20 x 21 + 40 = 460 km2
        self.assertNotIn("short", names)  # 4 km at order 5 and above is under 15 km
        ni = [r for r in document["river"] if r["name"] == ""]
        self.assertEqual(
            [r["source"] for r in ni], [{"rwb_codes": ["UKNI-B1"]}]
        )  # B2 has no segment of order 5
        loughs = {entry["name"]: entry for entry in document["lough"]}
        self.assertEqual(loughs["Gill"]["mode"], "added")
        self.assertEqual(loughs["Redrawn"]["mode"], "redrawn")
        self.assertNotIn("Far", loughs)
        self.assertTrue(loughs["Lough Neagh"]["keep_existing"])
        self.assertIn("lough-neagh-higher", loughs["Lough Neagh"]["reason"])
        self.assertEqual(
            [k["id"] for k in document["keep"]], ["island-iniscealtra", "island-inisfallen"]
        )
        reservoir = document["reservoir"][0]
        self.assertEqual(
            (reservoir["name"], reservoir["reason"]), ("Carrigadrohid Reservoir", "to be checked")
        )

    def test_any_overlap_with_todays_lake_makes_a_lough_redrawn_even_under_a_quarter_and_far_from_a_river(
        self,
    ) -> None:
        # 25 survey cells, one of them lake today: 4 per cent, well under a quarter, and no proposed river is near
        outline = W.Outline("L9", "Thin overlap", 3.0, (self.square((26, 20, 31, 25)),), "epa")
        surveys = self.surveys()
        surveys.lakes.append(outline)
        rows = terrain_rows(lakes=[(27, 22)])
        grid = W.MapGrid(CROP, rows, [[10.0] * 64 for _ in range(32)])
        document = tomllib.loads(W.propose_waters(surveys, grid, area_source=lambda p, r: 400.0))
        found = {entry["name"]: entry for entry in document["lough"]}
        self.assertEqual(found["Thin overlap"]["mode"], "redrawn")
        none = W.Outline("L8", "No overlap", 3.0, (self.square((26, 26, 31, 31)),), "epa")
        surveys.lakes.append(none)
        document = tomllib.loads(W.propose_waters(surveys, grid, area_source=lambda p, r: 400.0))
        self.assertNotIn(
            "No overlap", {entry["name"] for entry in document["lough"]}
        )  # not near a river, not today's lake

    def test_tiers_follow_the_drained_area(self) -> None:
        self.assertEqual([W.tier_of(a) for a in (1200, 1199.9, 300, 299.9, 0)], [1, 2, 2, 3, 3])

    def test_build_refuses_the_raw_proposal_for_its_empty_names(self) -> None:
        grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
        text = W.propose_waters(self.surveys(), grid, area_source=lambda point, reach: 400.0)
        with self.assertRaises(W.WatersError) as raised:
            W.parse_waters(text)
        self.assertIn("name is empty", str(raised.exception))
        named = text.replace('name = ""', 'name = "Placeholder"')
        self.assertEqual(len(W.parse_waters(named).rivers), 2)


class Network(unittest.TestCase):
    def test_pieces_in_any_order_and_direction_come_out_as_stretches_running_downstream(
        self,
    ) -> None:
        reversed_piece = W.Seg("m2", "epa", 5, tuple(reversed(MAIN_2.points)), "Main")
        for pieces in (
            [MAIN_1, MAIN_2, TIDAL],
            [TIDAL, MAIN_2, MAIN_1],
            [MAIN_1, reversed_piece, TIDAL],
        ):
            network = build(pieces, [river("main", "Main")])
            self.assertEqual([s.kind for s in network.stretches], ["river", "tidal"])
            self.assertEqual(network.stretches[0].points[0], centre(2, 10))
            check_network(self, network)

    def test_a_chain_of_one_kind_with_no_join_is_one_stretch(self) -> None:
        network = build([MAIN_1, MAIN_2], [river("main", "Main")])
        self.assertEqual(len(network.stretches), 1)
        self.assertEqual(network.stretches[0].id, "main-01")
        self.assertEqual(network.stretches[0].points[0], centre(2, 10))
        self.assertTrue(
            TERRAIN.open_sea(network.stretches[0].points[-1])
        )  # by a run through the sea to open sea
        self.assertEqual(network.stretches[0].ends, "sea")

    def test_a_confluence_makes_a_join_whose_printed_points_are_equal(self) -> None:
        network = build([*MAIN, TRIB], [river("main", "Main"), river("trib", "Trib")])
        check_network(self, network)
        by_id = {s.id: s for s in network.stretches}
        self.assertEqual(by_id["trib-01"].ends, "main-02")
        self.assertEqual(
            by_id["main-01"].ends, "main-02"
        )  # main ends where the tributary arrives, so it is cut there
        self.assertEqual(by_id["trib-01"].points[-1], by_id["main-02"].points[0])
        text = W.rivers_toml(
            W.Waters(rivers=(river("main", "Main"), river("trib", "Trib"))),
            network,
            {s.id: (3.0, 4.0) for s in network.stretches},
            "x",
        )
        self.assertEqual(
            text.count(
                f"[{by_id['trib-01'].points[-1][0]:.5f}, {by_id['trib-01'].points[-1][1]:.5f}]"
            ),
            3,
        )

    def test_a_chain_marked_river_lough_river_is_three_stretches_in_order_with_exact_joins(
        self,
    ) -> None:
        pieces = [
            segment("r1", [(2, 10), (10, 10)], "Main"),
            segment("r2", [(10, 10), (14, 10), (18, 10)], None, continua="L", lake="IE_X_1"),
            segment("r3", [(18, 10), (30, 10), (46, 10)], "Main"),
        ]
        network = build(pieces, [river("main", "Main")])
        self.assertEqual(
            [(s.id, s.kind) for s in network.stretches],
            [("main-01", "river"), ("main-02", "lough"), ("main-03", "river")],
        )
        self.assertEqual([s.ends for s in network.stretches[:2]], ["main-02", "main-03"])
        self.assertEqual(network.stretches[0].points[-1], network.stretches[1].points[0])
        self.assertEqual(network.stretches[1].points[-1], network.stretches[2].points[0])
        self.assertEqual(
            network.stretches[2].ends, "sea"
        )  # by a run through the sea: the survey line stops short of open sea
        self.assertEqual(list(network.sea_runs), ["main-03"])
        check_network(self, network)

    def test_a_river_that_turns_tidal_is_two_stretches(self) -> None:
        network = build(MAIN, [river("main", "Main")])
        self.assertEqual(
            [(s.kind, s.ends) for s in network.stretches], [("river", "main-02"), ("tidal", "sea")]
        )
        self.assertEqual(
            network.stretches[1].points[-1], centre(50, 10)
        )  # cut at the first point in open sea
        check_network(self, network)

    def test_a_hand_drawn_course_begins_and_ends_a_stretch(self) -> None:
        course = W.Correction(
            "bed",
            "the old bed",
            "medium",
            "main",
            replaces=("m2",),
            course=(centre(25, 10), centre(35, 12), centre(46, 10)),
        )
        network = build(MAIN, [river("main", "Main")], corrections=[course])
        self.assertEqual([s.kind for s in network.stretches], ["river", "river", "tidal"])
        self.assertIn(centre(35, 12), network.stretches[1].points)
        self.assertEqual(network.stretches[0].points[-1], network.stretches[1].points[0])
        check_network(self, network)
        with self.assertRaises(W.WatersError):
            build(
                MAIN,
                [river("main", "Main")],
                corrections=[W.Correction("x", "r", "m", "main", omit=("nothing",))],
            )

    def test_a_line_through_a_reservoir_is_a_river(self) -> None:
        pieces = [
            segment("r1", [(2, 10), (10, 10)], "Main"),
            segment("r2", [(10, 10), (18, 10)], None, continua="L", lake="IE_RES"),
            segment("r3", [(18, 10), (46, 10)], "Main"),
            TIDAL,
        ]
        self.assertEqual(
            [s.kind for s in build(pieces, [river("main", "Main")]).stretches],
            ["river", "lough", "river", "tidal"],
        )
        reservoir = frozenset({"IE_RES"})
        network = build(pieces, [river("main", "Main")], reservoir_lakes=reservoir)
        self.assertEqual([s.kind for s in network.stretches], ["river", "tidal"])

    def test_a_side_channel_starts_without_a_join_and_ends_at_one(self) -> None:
        island = [
            segment("a", [(2, 10), (20, 10)], "Main"),
            segment("b", [(20, 10), (24, 10), (28, 10)], "Main"),  # the main channel
            segment("c", [(20, 10), (24, 14), (28, 10)], "Main"),  # round the island
            segment("d", [(28, 10), (46, 10)], "Main"),
            TIDAL,
        ]
        network = build(island, [river("main", "Main")])
        check_network(self, network)
        ends = [s.ends for s in network.stretches]
        side = next(s for s in network.stretches if centre(24, 14) in s.points)
        self.assertNotIn(side.id, ends)  # nothing joins into the start of the side channel
        self.assertIn(
            side.points[0], [p for s in network.stretches if s is not side for p in s.points]
        )  # it starts on the main channel
        self.assertNotIn(
            side.points[0], [s.points[0] for s in network.stretches if s is not side]
        )  # on a stretch, not at its start
        self.assertEqual(
            side.ends, next(s.id for s in network.stretches if s.points[0] == side.points[-1])
        )

    def test_a_missing_line_through_a_lough_is_joined(self) -> None:
        rows = terrain_rows(lakes=[(c, 10) for c in range(20, 25)])
        pieces = [
            segment("up", [(2, 10), (19, 10)], source="ni", body="UP"),
            segment("down", [(25, 10), (46, 10)], source="ni", body="DOWN"),
            TIDAL,
        ]
        rivers = [W.RiverDef("main", "Main", 1, rwb_codes=("UP", "DOWN"))]
        network = build(pieces, rivers, rows)
        self.assertEqual([s.kind for s in network.stretches], ["river", "lough", "river", "tidal"])
        self.assertEqual(network.straight_lough, ["main-02"])
        self.assertEqual(len(network.stretches[1].points), 2)
        self.assertEqual(network.stretches[1].points[0], network.stretches[0].points[-1])
        check_network(self, network, W.Terrain(CROP, rows))

    def test_ends_one_unit_apart_are_one_node(self) -> None:
        """Two surveys round the same junction to either side of a rounding edge: about a metre apart, one node."""
        near = (centre(25, 10)[0] + 0.00001, centre(25, 10)[1])
        pieces = [
            segment("m1", [(2, 10), (25, 10)], "Main"),
            W.Seg("m2", "epa", 5, (near, *MAIN_2.points[1:]), "Main"),
            TIDAL,
        ]
        network = build(pieces, [river("main", "Main")])
        self.assertEqual(len(network.stretches), 2)  # river, tidal: no gap and no confluence
        self.assertEqual([s.kind for s in network.stretches], ["river", "tidal"])

    def test_a_river_line_that_stops_short_of_the_sea_is_refused_where_no_run_can_reach_it(
        self,
    ) -> None:
        """Rule 5: a line that ends 12 cells from the sea is never sent over land; the refusal names the river, the point and the
        distance."""
        with self.assertRaises(W.WatersError) as raised:
            build([segment("r", [(2, 10), (36, 10)], "Main")], [river("main", "Main")])
        message = str(raised.exception)
        self.assertIn("river main", message)
        self.assertIn(f"{centre(36, 10)[0]:.5f}, {centre(36, 10)[1]:.5f}", message)
        self.assertIn("12 cells from the sea", message)

    def test_a_tidal_line_that_ends_short_of_open_sea_takes_the_run_of_rule_4(self) -> None:
        """A tidal line is not a mouth for being tidal: the run through the sea is what takes it to open sea."""
        pieces = [
            segment("a", [(2, 10), (20, 10)], "Main"),
            segment(
                "b", [(20, 10), (30, 10)], None, order=5
            ),  # no name, not through a lough: the river runs on
            segment("c", [(30, 10), (46, 10)], None, continua="T"),
        ]
        network = build(pieces, [river("main", "Main")])
        self.assertEqual([s.kind for s in network.stretches], ["river", "tidal"])
        self.assertTrue(TERRAIN.open_sea(network.stretches[-1].points[-1]))
        self.assertEqual(list(network.sea_runs), ["main-02"])
        check_network(self, network)

    def test_ids_do_not_depend_on_the_input_order(self) -> None:
        first = signature(build([*MAIN, TRIB], [river("main", "Main"), river("trib", "Trib")]))
        for pieces in ([TRIB, TIDAL, MAIN_2, MAIN_1], [MAIN_2, TRIB, MAIN_1, TIDAL]):
            self.assertEqual(
                signature(build(pieces, [river("trib", "Trib"), river("main", "Main")])), first
            )

    def test_the_sea_end_by_a_run_through_the_sea(self) -> None:
        network = build([segment("r", [(2, 10), (46, 10)], "Main")], [river("main", "Main")])
        stretch = network.stretches[0]
        self.assertTrue(TERRAIN.open_sea(stretch.points[-1]))
        self.assertEqual(list(network.sea_runs), ["main-01"])
        self.assertIn("through sea cells", stretch.note)
        self.assertEqual(stretch.survey_end, centre(46, 10))
        check_network(self, network)

    def test_a_river_that_ends_inland_is_refused(self) -> None:
        with self.assertRaises(W.WatersError) as raised:
            build([segment("r", [(2, 10), (10, 10)], "Main")], [river("main", "Main")])
        self.assertIn("inland", str(raised.exception))

    def test_points_are_thinned_within_the_distance_keeping_the_ends_and_written_to_five_places(
        self,
    ) -> None:
        wiggly = [
            centre(2, 10),
            (centre(3, 10)[0] + 0.00002, centre(3, 10)[1]),
            centre(4, 10),
            (centre(5, 10)[0] - 0.00003, centre(5, 10)[1]),
            centre(6, 10),
        ]
        thinned = W.thin(wiggly, 8.0)
        self.assertEqual((thinned[0], thinned[-1]), (wiggly[0], wiggly[-1]))
        self.assertEqual(len(thinned), 2)
        zigzag = [centre(2, 10), centre(3, 11), centre(4, 10)]
        self.assertEqual(W.thin(zigzag, 8.0), zigzag)  # a kink of 400 m stays
        self.assertEqual(
            W.thin(wiggly, 8.0, pinned=[wiggly[2]]), [wiggly[0], wiggly[2], wiggly[-1]]
        )
        for point in thinned:
            self.assertEqual(point, W.round_point(point))
        text = W.rivers_toml(
            W.Waters(rivers=(river("main", "Main"),)),
            build([MAIN_1, MAIN_2], [river("main", "Main")]),
            {"main-01": (3.0, 4.0)},
            "x",
        )
        for line in text.splitlines():
            if line.startswith("  ["):
                self.assertRegex(line, r"^  \[\d+\.\d{5}, -\d+\.\d{5}\],$")


class Widths(unittest.TestCase):
    # Vertices every three cells and no thinning, so that the median of the areas read along a stretch is the value we give.
    RIVER = segment("w", [(c, 10) for c in (*range(2, 46, 3), 46)], "Main")
    SETTINGS = W.Settings(simplify_m=0.0)

    def widths(
        self,
        areas: dict[tuple[int, int], float],
        pieces: Sequence[Dyn] | None = None,
        *,
        settings: Dyn | None = None,
        tidal: Sequence[Dyn] = (),
        corrections: Sequence[Dyn] = (),
    ) -> tuple[Any, dict[str, tuple[float, float]]]:
        network = build(
            pieces or [self.RIVER], [river("main", "Main")], settings=settings or self.SETTINGS
        )
        read = reading(areas)
        return network, W.stretch_widths(
            network, W.stretch_areas(network, read), settings or self.SETTINGS, tidal, corrections
        )

    def test_the_rule_for_given_areas(self) -> None:
        self.assertAlmostEqual(W.width_of(100.0, 2.3, 0.4), 2.3 * 100**0.4)
        areas = {(c, 10): 400.0 for c in range(2, 51)}
        _, widths = self.widths(areas)
        self.assertEqual(widths["main-01"][1], round(2.3 * 400**0.4, 1))

    def test_a_headwaters_first_width(self) -> None:
        for read, expected_first in ((0.4, 1.0), (30.0, 30.0), (900.0, 120.0)):
            areas = {(c, 10): 120.0 for c in range(3, 51)}
            areas[(2, 10)] = read
            network, widths = self.widths(areas)
            first, last = W.stretch_areas(network, reading(areas))["main-01"]
            self.assertAlmostEqual(last, 120.0)
            self.assertAlmostEqual(
                first, expected_first
            )  # 1 km2 at least, and at most the stretch's own area
            self.assertEqual(widths["main-01"][0], W.final_width(2.3 * expected_first**0.4))

    def test_a_stretch_whose_own_area_is_under_a_square_kilometre_has_it_at_both_ends(self) -> None:
        network, widths = self.widths({(c, 10): 0.3 for c in range(2, 51)})
        self.assertEqual(W.stretch_areas(network, lambda point, reach: 0.3)["main-01"], (0.3, 0.3))
        self.assertEqual(widths["main-01"], (W.final_width(2.3 * 0.3**0.4),) * 2)

    def test_a_width_under_half_a_metre_is_written_as_half_a_metre(self) -> None:
        _, widths = self.widths(
            {(c, 10): 1.0 for c in range(2, 51)}, settings=W.Settings(a=0.1, simplify_m=0.0)
        )
        self.assertEqual(widths["main-01"], (0.5, 0.5))
        self.assertEqual(W.final_width(0.04), 0.5)
        self.assertEqual(W.final_width(3.14159), 3.1)

    def test_a_tributarys_last_stretch_cannot_read_the_main_rivers_area(self) -> None:
        areas = {(c, 10): 900.0 for c in range(2, 50)}
        areas.update(
            {(25, 3): 6.0, (25, 6): 9.0, (25, 10): 900.0}
        )  # the tributary's own last cell reads the main river
        network = build([*MAIN, TRIB], [river("main", "Main"), river("trib", "Trib")])
        computed = W.stretch_areas(
            network, lambda point, reach: areas.get(CROP.cell_containing(point), 5.0)
        )
        length_km = next(s.length_m for s in network.stretches if s.id == "trib-01") / 1000.0
        self.assertLessEqual(computed["trib-01"][1], 20 * length_km + 40 + 1e-9)
        self.assertLess(computed["trib-01"][1], 900.0)
        self.assertGreaterEqual(
            computed["main-02"][0], computed["trib-01"][1]
        )  # nothing below drains less than what arrives
        self.assertGreaterEqual(computed["main-02"][1], computed["main-01"][1])

    def test_a_tidal_stretch_takes_its_outlines_width_and_its_cap(self) -> None:
        def outline(half_height: float) -> Dyn:
            ring = [
                CROP.latlon_at(c, r)
                for c, r in (
                    (42.0, 10.5 - half_height),
                    (60.0, 10.5 - half_height),
                    (60.0, 10.5 + half_height),
                    (42.0, 10.5 + half_height),
                )
            ]
            return W.Outline("T1", "Estuary", 1.0, ([ring],), "epa")

        areas = {(c, 10): 50.0 for c in range(2, 56)}
        narrow = outline(
            0.3
        )  # 18 x 0.6 cells: twice area over perimeter is 0.58 of a cell, about 280 m
        mean = W.polygon_mean_width_m(narrow.polygons[0])
        self.assertAlmostEqual(mean, 2 * 18 * 0.6 / (2 * 18.6) * 839 * 0.5663, delta=15)
        _, widths = self.widths(areas, [self.RIVER, TIDAL], tidal=[narrow])
        self.assertEqual(widths["main-02"], (W.final_width(mean), W.final_width(mean)))
        self.assertLess(widths["main-01"][1], 20.0)  # the river above the estuary is not affected
        _, capped = self.widths(areas, [self.RIVER, TIDAL], tidal=[outline(10.0)])
        self.assertEqual(capped["main-02"], (800.0, 800.0))

    def test_each_kind_of_correction(self) -> None:
        areas = {(c, 10): 100.0 for c in range(2, 56)}
        corrections = [
            W.Correction("c1", "r", "high", "main", stretch="main-01", width_m=(10.0, 12.0)),
            W.Correction("c2", "r", "low", "main", width_factor=2.0),
        ]
        _, plain = self.widths(areas, [self.RIVER, TIDAL])
        _, corrected = self.widths(areas, [self.RIVER, TIDAL], corrections=corrections[:1])
        self.assertEqual(corrected["main-01"], (10.0, 12.0))
        self.assertEqual(corrected["main-02"], plain["main-02"])
        _, doubled = self.widths(areas, [self.RIVER, TIDAL], corrections=corrections[1:])
        self.assertEqual(doubled["main-02"], tuple(W.final_width(2 * w) for w in plain["main-02"]))
        self.assertIn("width 10.0 to 12.0 m", corrections[0].describe())
        omit = W.Correction("c3", "r", "low", "main", omit=("t1",))
        self.assertEqual(len(build(MAIN, [river("main", "Main")], corrections=[omit]).stretches), 1)


class Cells(unittest.TestCase):
    def polygon(
        self, c0: float, r0: float, c1: float, r1: float
    ) -> list[list[tuple[float, float]]]:
        return [[CROP.latlon_at(c, r) for c, r in ((c0, r0), (c1, r0), (c1, r1), (c0, r1))]]

    def test_more_than_half_and_exactly_half_of_a_cell(self) -> None:
        self.assertEqual(W.cover_cells([self.polygon(5.0, 5.0, 6.0, 6.0)], CROP), {(5, 5)})
        self.assertEqual(W.cover_cells([self.polygon(5.0, 5.0, 5.7, 6.0)], CROP), {(5, 5)})
        self.assertEqual(
            W.cover_cells([self.polygon(5.0, 5.0, 5.5, 6.0)], CROP), set()
        )  # exactly half
        self.assertEqual(W.cover_cells([self.polygon(5.0, 5.0, 5.4, 6.0)], CROP), set())
        shell_and_hole = self.polygon(2.0, 2.0, 8.0, 8.0) + self.polygon(4.0, 4.0, 6.0, 6.0)
        covered = W.cover_cells([shell_and_hole], CROP)
        self.assertNotIn((4, 4), covered)
        self.assertIn((3, 3), covered)
        self.assertEqual(len(covered), 36 - 4)

    def plan(
        self,
        rows: list[list[int]],
        tidal: CellList = (),
        loughs: Sequence[Dyn] = (),
        kept: dict[tuple[int, int], str] | None = None,
    ) -> Dyn:
        return W.decide_water(rows, dict.fromkeys(tidal, "Estuary"), loughs, kept)

    def small(self, lakes: CellList = (), width: int = 10, height: int = 8) -> list[list[int]]:
        # the sea is the last two columns
        return terrain_rows(lambda c, r: c >= width - 2, lakes, width, height)

    def test_a_tidal_cell_joined_to_the_sea_becomes_sea(self) -> None:
        plan = self.plan(self.small(), tidal=[(7, 3), (6, 3), (5, 3)])
        self.assertEqual(plan.change, {(7, 3): W.SEA, (6, 3): W.SEA, (5, 3): W.SEA})
        self.assertEqual(set(plan.sea_new.values()), {"Estuary"})
        self.assertEqual(plan.tidal_unjoined, {})

    def test_a_tidal_cell_joined_only_by_a_corner_or_cut_off_stays(self) -> None:
        plan = self.plan(
            self.small(), tidal=[(7, 2), (6, 3)]
        )  # (6, 3) touches the sea (8, 3)? no: (7, 2) joins, (6, 3) is a corner
        self.assertEqual(set(plan.change), {(7, 2)})
        self.assertEqual(set(plan.tidal_unjoined), {(6, 3)})
        plan = self.plan(self.small(), tidal=[(3, 3), (4, 3)])  # inland pool, not joined
        self.assertEqual(plan.change, {})
        self.assertEqual(set(plan.tidal_unjoined), {(3, 3), (4, 3)})

    def test_sea_cells_cut_off_from_the_open_sea_are_reported(self) -> None:
        rows = self.small()
        rows[4][3] = W.SEA
        plan = self.plan(rows)
        self.assertEqual(plan.sea_cut_off, {(3, 4)})
        self.assertEqual(plan.change, {})

    def test_a_lough_cell_is_added_on_land_but_not_over_the_sea(self) -> None:
        lough = W.LoughCells("Gill", "added", False, frozenset({(2, 2), (3, 2), (8, 2)}))
        plan = self.plan(self.small(), loughs=[lough])
        self.assertEqual(plan.change, {(2, 2): W.LAKE, (3, 2): W.LAKE})

    LAKE_TODAY = ((1, 1), (2, 1))

    def lough(self, name: str, cells: CellList, mode: str = "redrawn", keep: bool = False) -> Dyn:
        return W.LoughCells(name, mode, keep, frozenset(cells))

    def test_a_redrawn_lough_gives_back_cells_outside_the_survey_and_takes_the_land_nearest_them(
        self,
    ) -> None:
        rows = self.small(lakes=self.LAKE_TODAY)
        rows[0][0], rows[0][1] = (
            W.BOG,
            W.FOREST,
        )  # the nearest land to (1, 1) is a ring of eight; the first in row order is (0, 0)
        plan = self.plan(rows, loughs=[self.lough("A", [(2, 1), (3, 1)])])
        self.assertEqual(
            {c for c, v in plan.change.items() if v == W.LAKE}, {(3, 1)}
        )  # (2, 1) was lake already
        self.assertEqual(plan.returned, {(1, 1): W.BOG})
        self.assertEqual(plan.change[(1, 1)], W.BOG)
        self.assertEqual(plan.change[(3, 1)], W.LAKE)
        self.assertEqual(set(plan.change), {(1, 1), (3, 1)})

    def tie_rows(self, cells: dict[tuple[int, int], int]) -> list[list[int]]:
        """Lake today at (4, 4) and (5, 4); round (4, 4) every cell is sea or lake but those named."""
        rows = self.small(lakes=[(4, 4), (5, 4)], width=12)
        for column in (3, 4, 5):
            rows[3][column] = rows[5][column] = W.SEA
        rows[4][3] = W.SEA
        for (column, row), value in cells.items():
            rows[row][column] = value
        return rows

    def test_the_tie_for_nearest_land_goes_by_row_then_column(self) -> None:
        survey = [self.lough("A", [(5, 4)])]  # (4, 4) is the lake cell the survey does not hold
        row_first = self.plan(
            self.tie_rows({(5, 3): W.BOG, (3, 5): W.MARSH, (4, 5): W.MOUNTAIN}), loughs=survey
        )
        self.assertEqual(
            row_first.returned, {(4, 4): W.BOG}
        )  # row 3 comes before row 5, whatever the column
        column_second = self.plan(
            self.tie_rows({(3, 3): W.FOREST, (5, 3): W.BOG, (3, 5): W.MARSH}), loughs=survey
        )
        self.assertEqual(
            column_second.returned, {(4, 4): W.FOREST}
        )  # in one row, the smaller column
        self.assertEqual(column_second.change, {(4, 4): W.FOREST})

    def test_a_second_patch_of_lake_that_shares_no_cell_with_the_outline_is_untouched(self) -> None:
        rows = self.small(lakes=[(1, 1), (2, 1), (5, 5), (6, 5)])
        plan = self.plan(rows, loughs=[self.lough("A", [(2, 1), (3, 1)])])
        self.assertNotIn((5, 5), plan.change)
        self.assertNotIn((6, 5), plan.change)
        self.assertEqual(set(plan.change), {(1, 1), (3, 1)})

    def test_keep_existing_keeps_the_patch_and_still_gains_cells(self) -> None:
        plan = self.plan(
            self.small(lakes=self.LAKE_TODAY), loughs=[self.lough("A", [(2, 1), (3, 1)], keep=True)]
        )
        self.assertEqual(plan.change, {(3, 1): W.LAKE})
        self.assertEqual(plan.returned, {})

    def test_a_keep_point_and_a_location_cell_stay_as_they_are_inside_an_outline(self) -> None:
        kept = {(3, 1): "keep island", (6, 3): "location devenish"}
        plan = self.plan(
            self.small(),
            tidal=[(6, 3), (7, 3)],
            loughs=[self.lough("A", [(3, 1), (4, 1)], "added")],
            kept=kept,
        )
        self.assertEqual(plan.change, {(7, 3): W.SEA, (4, 1): W.LAKE})
        self.assertEqual(plan.would_sea, {(6, 3)})
        self.assertEqual(plan.would_lough, {(3, 1)})
        lake = self.small(lakes=self.LAKE_TODAY)
        plan = self.plan(lake, loughs=[self.lough("A", [(2, 1), (3, 1)])], kept={(1, 1): "keep"})
        self.assertEqual(plan.change, {(3, 1): W.LAKE})  # (1, 1) would have been returned to land

    PATCH = ((1, 1), (2, 1), (3, 1), (4, 1))

    def two_loughs(self, lakes: CellList, keep_a: bool = False, swap: bool = False) -> Dyn:
        a = self.lough("A", [(1, 1), (2, 1)], keep=keep_a)
        b = self.lough("B", [(3, 1), (4, 1)])
        return self.plan(self.small(lakes=lakes, width=12), loughs=[b, a] if swap else [a, b])

    def test_two_redrawn_loughs_on_one_patch_erase_nothing_the_other_keeps(self) -> None:
        for swap in (False, True):
            self.assertEqual(self.two_loughs(self.PATCH, swap=swap).change, {})
        longer = (*self.PATCH, (5, 1))
        for swap in (False, True):
            self.assertEqual(self.two_loughs(longer, swap=swap).change, {(5, 1): W.PASTURE})
            self.assertEqual(self.two_loughs(longer, keep_a=True, swap=swap).change, {})

    def test_every_case_appears_in_the_report_and_the_text_is_the_same_twice_and_in_either_order(
        self,
    ) -> None:
        rows = terrain_rows(lakes=(*self.PATCH, (5, 1)), width=64, height=32)
        grid = W.MapGrid(CROP, rows, [[10.0] * 64 for _ in range(32)])

        def survey(code: str, cells: Sequence[tuple[int, int]]) -> Dyn:
            ring = [
                CROP.latlon_at(c, r)
                for c, r in (
                    (cells[0][0], 1.0),
                    (cells[-1][0] + 1.0, 1.0),
                    (cells[-1][0] + 1.0, 2.0),
                    (cells[0][0], 2.0),
                )
            ]
            return W.Outline(code, code, 0.5, ([ring],), "epa")

        surveys = W.Surveys(lakes=[survey("A", [(1, 1), (2, 1)]), survey("B", [(3, 1), (4, 1)])])
        text = (
            '[[lough]]\nname = "{0}"\nsource = "{0}"\nmode = "redrawn"\n{2}\n'
            '[[lough]]\nname = "{1}"\nsource = "{1}"\nmode = "redrawn"\n'
            '[[keep]]\nid = "k"\nreason = "owner"\npoint = [{3}, {4}]\n'
        )
        point = centre(0, 0)
        forward = W.parse_waters(text.format("A", "B", "", *point))
        backward = W.parse_waters(text.format("B", "A", "", *point))
        first = W.build_waters(forward, surveys, grid, waters_sha="s")
        again = W.build_waters(forward, surveys, grid, waters_sha="s")
        swapped = W.build_waters(backward, surveys, grid, waters_sha="s")
        self.assertEqual(
            (first.rivers_toml, first.water, first.report),
            (again.rivers_toml, again.water, again.report),
        )
        self.assertEqual(first.water, swapped.water)
        self.assertEqual(first.plan.returned, {(5, 1): W.PASTURE})
        for words in (
            "A (redrawn)",
            "B (redrawn)",
            "returned to land: 1 cells (1 pasture)",
            "kept cell (0, 0): keep k",
            "## What Northern Ireland lacks",
        ):
            self.assertIn(words, first.report)
        kept = W.build_waters(
            W.parse_waters(
                text.format("A", "B", 'keep_existing = true\nreason = "Neagh by hand"', *point)
            ),
            surveys,
            grid,
        )
        self.assertEqual(kept.plan.returned, {})
        self.assertIn("kept by keep_existing: 1 cells: Neagh by hand", kept.report)

    def test_a_kept_location_is_listed_only_where_something_would_have_changed(self) -> None:
        rows = terrain_rows(lakes=(*self.PATCH, (5, 1)), width=64, height=32)
        grid = W.MapGrid(CROP, rows, [[10.0] * 64 for _ in range(32)])
        ring = [CROP.latlon_at(c, r) for c, r in ((1.0, 1.0), (5.0, 1.0), (5.0, 2.0), (1.0, 2.0))]
        surveys = W.Surveys(lakes=[W.Outline("A", "A", 0.5, ([ring],), "epa")])
        waters = W.parse_waters('[[lough]]\nname = "A"\nsource = "A"\nmode = "redrawn"\n')
        locations = {"on_the_edge": centre(5, 1), "nowhere": centre(30, 20)}
        report = W.build_waters(waters, surveys, grid, locations).report
        self.assertIn(
            "kept cell (5, 1): location on_the_edge (would have been returned to land)", report
        )
        self.assertNotIn("nowhere", report)

    def test_locations_are_reported_and_keep_their_cells(self) -> None:
        rows = terrain_rows(width=64, height=32)
        grid = W.MapGrid(CROP, rows, [[10.0] * 64 for _ in range(32)])
        ring = [
            CROP.latlon_at(c, r)
            for c, r in ((10.0, 10.0), (14.0, 10.0), (14.0, 12.0), (10.0, 12.0))
        ]
        surveys = W.Surveys(lakes=[W.Outline("L", "Lough", 1.0, ([ring],), "epa")])
        waters = W.parse_waters('[[lough]]\nname = "Lough"\nsource = "L"\nmode = "added"\n')
        locations = {"on": centre(11, 10), "beside": centre(9, 10), "far": centre(30, 20)}
        result = W.build_waters(waters, surveys, grid, locations)
        self.assertNotIn((11, 10), result.plan.change)
        self.assertEqual(result.water[10][11], W.UNCHANGED)
        self.assertIn("on stands on a cell that would have become water", result.report)
        self.assertIn("beside stands beside a cell that becomes water", result.report)
        self.assertNotIn("far stands", result.report)


class Refusals(unittest.TestCase):
    def test_an_existing_output_is_refused_naming_it_unless_forced(self) -> None:
        with tempfile.TemporaryDirectory(prefix="ck348-out-") as directory:
            target = Path(directory) / "rivers.toml"
            W._refuse_existing([target, Path(directory) / "water.png"], force=False)
            target.write_text("x", encoding="utf-8")
            with self.assertRaises(SystemExit) as raised:
                W._refuse_existing([target, Path(directory) / "water.png"], force=False)
            self.assertIn("rivers.toml", str(raised.exception))
            self.assertIn("--force", str(raised.exception))
            W._refuse_existing([target], force=True)


class FormatContract(unittest.TestCase):
    """The written network obeys CK-347 format rules 2 and 3 on the final terrain, or the build is refused before it writes."""

    RIVER_LOUGH_RIVER = (
        segment("r1", [(2, 10), (10, 10)], "Main"),
        segment("r2", [(10, 10), (14, 10), (18, 10)], None, continua="L", lake="IE_X_1"),
        segment("r3", [(18, 10), (30, 10), (46, 10)], "Main"),
        TIDAL,
    )

    def build(self, lakes: CellList = (), text: str = "", areas: float = 50.0) -> Dyn:
        waters = W.parse_waters(
            text
            or '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
        )
        grid = W.MapGrid(CROP, terrain_rows(lakes=lakes), [[10.0] * 64 for _ in range(32)])
        return W.build_waters(
            waters,
            W.Surveys(rivers=list(self.RIVER_LOUGH_RIVER)),
            grid,
            area_source=lambda p, r: areas,
        )

    def with_lakes(
        self,
        outline_code: str | None,
        listed: bool,
        lake_code: str | None = "IE_X_1",
        size: float = 5.0,
    ) -> Dyn:
        """The river-lough-river chain with a lake outline in the survey, listed in waters.toml or not; the lake itself lies elsewhere."""
        pieces = [
            segment("r1", [(2, 10), (10, 10)], "Main"),
            segment("r2", [(10, 10), (14, 10), (18, 10)], None, continua="L", lake=lake_code),
            segment("r3", [(18, 10), (30, 10), (46, 10)], "Main"),
            TIDAL,
        ]
        ring = [
            CROP.latlon_at(c, r)
            for c, r in ((20.0, 20.0), (20.0 + size, 20.0), (20.0 + size, 22.0), (20.0, 22.0))
        ]
        outlines = (
            [W.Outline(outline_code, "Lough Far", 0.4, ([ring],), "epa")] if outline_code else []
        )
        text = (
            '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
        )
        if listed:
            text += f'\n[[lough]]\nname = "Lough Far"\nsource = "{outline_code}"\nmode = "added"\n'
        grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
        surveys = W.Surveys(rivers=pieces, lakes=outlines)
        return W.build_waters(W.parse_waters(text), surveys, grid, area_source=lambda p, r: 50.0)

    def test_a_lough_line_with_no_lough_under_it_is_written_as_a_river_with_its_reason(
        self,
    ) -> None:
        cases = (
            # (outline code in the survey, listed in waters.toml, the line's lake code, words in the reason)
            (None, False, "IE_X_1", "the survey gives the line no lake outline (lake code IE_X_1)"),
            (None, False, None, "the survey gives the line no lake outline (it has no lake code)"),
            (
                "IE_X_1",
                False,
                "IE_X_1",
                "an outline that is not in the list: Lough Far (IE_X_1), 0.40 km2",
            ),
            (
                "IE_X_1",
                True,
                "IE_X_1",
                "in the list and on the map, but no written segment midpoint of this stretch is in a lake cell: Lough Far",
            ),
        )
        for outline, listed, code, words in cases:
            with self.subTest(words=words):
                result = self.with_lakes(outline, listed, code)
                self.assertEqual(
                    [s.kind for s in result.network.stretches], ["river", "river", "river", "tidal"]
                )
                self.assertEqual(
                    [s.id for s in result.network.stretches],
                    ["main-01", "main-02", "main-03", "main-04"],
                )
                self.assertEqual(
                    [s.ends for s in result.network.stretches],
                    ["main-02", "main-03", "main-04", "sea"],
                )  # not merged
                self.assertEqual(list(result.network.lough_as_river), ["main-02"])
                self.assertIn(words, result.network.lough_as_river["main-02"])
                self.assertIn(
                    "## Lough lines written as rivers (the map has no lough under them)\n\n- main-02: river main: ",
                    result.report,
                )
                self.assertIn(words, result.report)
                self.assertIn("written as a river", result.rivers_toml)
                self.assertNotIn('kind = "lough"', result.rivers_toml)

    def test_reason_3_says_no_midpoint_not_that_the_line_crosses_only_land(self) -> None:
        """Spec r6: the line may touch a lake cell (here its first point) while no written segment midpoint is in one."""
        pieces = [
            segment("r1", [(2, 10), (10, 10)], "Main"),
            segment("r2", [(10, 10), (14, 10), (18, 10)], None, continua="L", lake="IE_X_1"),
            segment("r3", [(18, 10), (46, 10)], "Main"),
            TIDAL,
        ]
        ring = [
            CROP.latlon_at(c, r)
            for c, r in ((10.0, 10.0), (11.0, 10.0), (11.0, 11.0), (10.0, 11.0))
        ]
        outline = W.Outline("IE_X_1", "Lough Edge", 0.2, ([ring],), "epa")
        text = (
            '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
            '\n[[lough]]\nname = "Lough Edge"\nsource = "IE_X_1"\nmode = "added"\n'
        )
        grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
        result = W.build_waters(
            W.parse_waters(text),
            W.Surveys(rivers=pieces, lakes=[outline]),
            grid,
            area_source=lambda p, r: 50.0,
        )
        self.assertEqual(
            result.plan.lough_new["Lough Edge"], {(10, 10)}
        )  # the lough is on the map, and the line starts in it
        reason = result.network.lough_as_river["main-02"]
        self.assertIn(
            "its lough is in the list and on the map, but no written segment midpoint of this stretch is in a lake cell: Lough Edge (IE_X_1)",
            reason,
        )
        self.assertNotIn("crosses only land", result.report)

    def test_a_listed_lough_with_no_cell_on_the_map_is_not_called_mapped(self) -> None:
        """Review 3, F1-R3: an outline of one eighth of a cell is a valid list entry that makes no lake cell."""
        mapped = self.with_lakes("IE_X_1", True)
        self.assertIn(
            "its lough is in the list and on the map, but no written segment midpoint of this stretch is in a lake cell: Lough Far (IE_X_1)",
            mapped.report,
        )
        empty = self.with_lakes("IE_X_1", True, size=0.125)
        self.assertEqual(empty.plan.lough_new["Lough Far"], set())
        self.assertIn(
            "- main-02: river main: its lough is in the list, but no cell of it is lake on the map: Lough Far (IE_X_1), its outline covers 0 cells",
            empty.report,
        )
        self.assertNotIn("in the list and on the map", empty.report)
        self.assertEqual(
            [s.kind for s in empty.network.stretches], ["river", "river", "river", "tidal"]
        )

    def test_a_merged_stretch_reports_every_survey_lake_code_name_and_area(self) -> None:
        """Review 4, F2-R4: adjacent lough segments of two survey lakes merge into one stretch; the report keeps both."""
        pieces = [
            segment("r1", [(2, 10), (10, 10)], "Main"),
            segment("r2", [(10, 10), (14, 10)], None, continua="L", lake="IE_A"),
            segment("r3", [(14, 10), (18, 10)], None, continua="L", lake="IE_B"),
            segment("r4", [(18, 10), (46, 10)], "Main"),
            TIDAL,
        ]

        def ring(c0: float, r0: float, c1: float, r1: float) -> list[tuple[float, float]]:
            return [CROP.latlon_at(c, r) for c, r in ((c0, r0), (c1, r0), (c1, r1), (c0, r1))]

        alpha = W.Outline("IE_A", "Lough Alpha", 2.0, ([ring(20.0, 20.0, 25.0, 22.0)],), "epa")
        beta = W.Outline("IE_B", "Lough Beta", 0.4, ([ring(30.0, 25.0, 35.0, 27.0)],), "epa")
        text = (
            '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
            '\n[[lough]]\nname = "Lough Alpha"\nsource = "IE_A"\nmode = "added"\n'
        )
        grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
        surveys = W.Surveys(rivers=pieces, lakes=[alpha, beta])
        result = W.build_waters(W.parse_waters(text), surveys, grid, area_source=lambda p, r: 50.0)
        self.assertEqual(
            [s.id for s in result.network.stretches], ["main-01", "main-02", "main-03", "main-04"]
        )
        reason = result.network.lough_as_river["main-02"]
        self.assertIn("Lough Alpha (IE_A)", reason)
        self.assertIn(
            "its lake has an outline that is not in the list: Lough Beta (IE_B), 0.40 km2", reason
        )
        self.assertIn(reason, result.report)

    def test_the_built_file_holds_no_lough_stretch_that_rule_3_would_refuse(self) -> None:
        result = self.with_lakes("IE_X_1", True)
        terrain = W.Terrain(CROP, W.apply_plan(terrain_rows(), result.plan))
        widths = {s.id: (3.0, 4.0) for s in result.network.stretches}
        self.assertEqual(W.format_problems(W.parse_waters(""), result.network, widths, terrain), [])
        self.assertFalse(any(s.kind == "lough" for s in result.network.stretches))

    def test_the_same_lough_with_its_lake_in_the_terrain_builds_and_passes(self) -> None:
        result = self.build(lakes=[(c, 10) for c in range(11, 18)])
        self.assertEqual(
            [s.kind for s in result.network.stretches], ["river", "lough", "river", "tidal"]
        )
        grid_terrain = W.Terrain(
            CROP, W.apply_plan(terrain_rows(lakes=[(c, 10) for c in range(11, 18)]), result.plan)
        )
        self.assertEqual(
            W.format_problems(
                W.parse_waters(""),
                result.network,
                {s.id: (3.0, 4.0) for s in result.network.stretches},
                grid_terrain,
            ),
            [],
        )

    def test_the_midpoint_is_the_mean_of_two_ends_rounded_down(self) -> None:
        terrain = W.Terrain(CROP, terrain_rows(lakes=[(12, 10)]))
        self.assertTrue(
            W.lough_has_lake([centre(11, 10), centre(14, 10)], terrain)
        )  # (11.5 + 14.5) / 2 = 13.0: cell 13, not 12
        # a segment from cell 10 to cell 14: centres 10.5 and 14.5 have mean 12.5, which is cell 12
        self.assertTrue(W.lough_has_lake([centre(10, 10), centre(14, 10)], terrain))
        self.assertFalse(W.lough_has_lake([centre(2, 10), centre(8, 10)], terrain))

    def test_the_midpoint_follows_the_loaders_fixed_point_arithmetic_at_a_cell_boundary(
        self,
    ) -> None:
        """Review 2, F2-R2: the loader projects each written end to fixed point (flooring), takes the integer mean rounded down,
        and looks up that cell; the continuous midpoint of the same two points lies in the next cell east."""
        first, second = (55.55182, -10.75592), (55.55182, -10.55243)
        self.assertEqual(
            W.fixed_point(first), (735821835, 490848243)
        )  # ck_map::project, as the review computed it
        self.assertEqual(W.fixed_point(second), (736706548, 490848243))
        mid_x = (735821835 + 736706548) // 2
        self.assertEqual(mid_x // 32768 - CROP.origin[0], 32)  # the loader's cell
        continuous = CROP.cell_of(*first)[0] + CROP.cell_of(*second)[0]
        self.assertEqual(math.floor(continuous / 2), 33)  # the arithmetic the tool used before
        only_33 = W.Terrain(CROP, terrain_rows(lakes=[(33, 0)]))
        only_32 = W.Terrain(CROP, terrain_rows(lakes=[(32, 0)]))
        self.assertFalse(W.lough_has_lake([first, second], only_33))
        self.assertTrue(W.lough_has_lake([first, second], only_32))
        lough = W.Stretch(
            id="main-01", river="main", kind="lough", points=[first, second], ends="sea"
        )
        waters = W.parse_waters(
            '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
        )
        network = W.Network([lough], ["main"])
        found = W.format_problems(waters, network, {"main-01": (3.0, 4.0)}, only_33)
        self.assertTrue(any("lough stretch but no segment midpoint" in m for m in found))

    def test_the_longest_supported_river_id_gives_valid_stretch_ids(self) -> None:
        identifier = "a" * 40
        text = f'[[river]]\nid = "{identifier}"\nname = "Long"\ntier = 1\nsource = {{ epa_names = ["Main"] }}\n'
        result = self.build(text=text)
        ids = [s.id for s in result.network.stretches]
        self.assertTrue(
            all(i.startswith(identifier + "-0") and len(i) == len(identifier) + 3 for i in ids)
        )
        with self.assertRaises(W.WatersError):
            W.parse_waters(text.replace("a" * 40, "a" * 41))
        waters = W.parse_waters(text)
        stretch = W.Stretch(
            id="b" * 48, river=identifier, kind="river", points=[centre(2, 10), centre(50, 10)]
        )
        terrain = W.Terrain(CROP, terrain_rows())
        ok = W.format_problems(
            waters, W.Network([stretch], [identifier]), {stretch.id: (3.0, 4.0)}, terrain
        )
        self.assertEqual(ok, [])  # 48 characters is the format's bound for a stretch id
        too_long = W.Stretch(id="b" * 49, river=identifier, kind="river", points=stretch.points)
        bad = W.format_problems(
            waters, W.Network([too_long], [identifier]), {too_long.id: (3.0, 4.0)}, terrain
        )
        self.assertTrue(any("id is not 1 to 48" in m for m in bad))

    def test_thinning_never_moves_every_midpoint_out_of_the_lake(self) -> None:
        pieces = [
            segment("r1", [(2, 10), (10, 10)], "Main"),
            segment("r2", [(10, 10), (14, 10), (18, 10)], None, continua="L", lake="IE_X_1"),
            segment("r3", [(18, 10), (46, 10)], "Main"),
        ]
        terrain = W.Terrain(CROP, terrain_rows(lakes=[(11, 10), (12, 10)]))
        network = W.build_network(
            pieces, [river("main", "Main")], terrain, W.Settings(simplify_m=8.0)
        )
        lough = next(s for s in network.stretches if s.kind == "lough")
        self.assertEqual(
            len(lough.points), 3
        )  # the collinear middle point stays: without it the midpoint leaves the lake
        self.assertTrue(W.lough_has_lake(lough.points, terrain))

    def test_a_width_above_ten_thousand_metres_is_refused_naming_the_stretch(self) -> None:
        text = (
            "[settings]\na = 10000\n\n"
            '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
        )
        with self.assertRaises(W.WatersError) as raised:
            self.build(lakes=[(c, 10) for c in range(11, 18)], text=text, areas=50.0)
        self.assertIn("stretch main-01", str(raised.exception))
        self.assertIn("at most 10000 m", str(raised.exception))
        correction = (
            text
            + '\n[[correction]]\nid = "wide"\nreason = "r"\nconfidence = "low"\nriver = "main"\nwidth_m = 20000\n'
        )
        with self.assertRaises(W.WatersError):
            W.parse_waters(
                correction
            )  # a correction cannot ask for more than the format allows either

    def test_the_other_rules_each_refuse_with_the_river_and_stretch(self) -> None:
        terrain = W.Terrain(CROP, terrain_rows())
        good = [centre(2, 10), centre(20, 10)]

        def one(**changes: Dyn) -> Dyn:
            fields = {
                "id": "main-01",
                "river": "main",
                "kind": "river",
                "points": good,
                "ends": "sea",
            }
            fields.update(changes)
            return W.Stretch(**fields)

        waters = W.parse_waters(
            '[[river]]\nid = "main"\nname = "Main"\ntier = 1\nsource = { epa_names = ["Main"] }\n'
        )

        def problems(*stretches: Dyn, widths: tuple[float, float] = (3.0, 4.0)) -> list[str]:
            network = W.Network(list(stretches), ["main"])
            found: list[str] = W.format_problems(
                waters, network, {s.id: widths for s in stretches}, terrain
            )
            return found

        self.assertTrue(any("sea end is not in open sea" in m for m in problems(one())))
        lough = problems(one(kind="lough", points=[centre(2, 10), centre(50, 10)]))
        self.assertTrue(any("lough stretch but no segment midpoint" in m for m in lough))
        sea_end = [centre(2, 10), centre(50, 10)]
        self.assertEqual(problems(one(points=sea_end)), [])
        self.assertTrue(
            any(
                "main-01: width 0.0 m" in m
                for m in problems(one(points=sea_end), widths=(0.0, 4.0))
            )
        )
        self.assertTrue(
            any(
                "width 10001.0 m" in m for m in problems(one(points=sea_end), widths=(3.0, 10001.0))
            )
        )
        self.assertTrue(
            any(
                "two consecutive points" in m
                for m in problems(one(points=[sea_end[0], sea_end[0], sea_end[1]]))
            )
        )
        self.assertTrue(
            any("outside the crop" in m for m in problems(one(points=[(0.0, 0.0), sea_end[1]])))
        )
        self.assertTrue(
            any("fewer than two points" in m for m in problems(one(points=[sea_end[0]])))
        )
        self.assertTrue(
            any("not another stretch" in m for m in problems(one(points=sea_end, ends="nowhere")))
        )
        self.assertTrue(
            any(
                "not the first point of main-02" in m
                for m in problems(
                    one(points=sea_end, ends="main-02"),
                    one(id="main-02", points=[centre(3, 10), sea_end[1]]),
                )
            )
        )
        loop = problems(
            one(points=[centre(2, 10), centre(20, 10)], ends="main-02"),
            one(id="main-02", points=[centre(20, 10), centre(2, 10)], ends="main-01"),
        )
        self.assertTrue(any("in a circle" in m for m in loop))
        self.assertTrue(any("river main: it has no stretch" in m for m in problems()))
        self.assertTrue(any("id is not" in m for m in problems(one(id="Main_01", points=sea_end))))


class Reports(unittest.TestCase):
    """Rule 11: the report names the cells, not only their number."""

    def test_the_report_lists_each_cell_with_its_terrain_and_its_lough_or_estuary(self) -> None:
        rows = terrain_rows(lakes=((1, 1), (2, 1), (3, 1), (4, 1), (5, 1)))
        rows[20][30] = W.SEA  # a pool cut off from the open sea
        rows[1][0] = (
            W.BOG
        )  # the land the cell given back would take: the first nearest in row order is (0, 0), pasture
        grid = W.MapGrid(CROP, rows, [[10.0] * 64 for _ in range(32)])

        def ring(c0: float, r0: float, c1: float, r1: float) -> list[tuple[float, float]]:
            return [CROP.latlon_at(c, r) for c, r in ((c0, r0), (c1, r0), (c1, r1), (c0, r1))]

        surveys = W.Surveys(
            lakes=[W.Outline("A", "Alpha", 0.5, ([ring(1.0, 1.0, 5.0, 2.0)],), "epa")],
            tidal=[
                W.Outline("T1", "Joined Bay", 1.0, ([ring(46.0, 5.0, 48.0, 6.0)],), "epa"),
                W.Outline("T2", "Pond Estuary", 1.0, ([ring(20.0, 20.0, 22.0, 21.0)],), "epa"),
            ],
        )
        waters = W.parse_waters('[[lough]]\nname = "Alpha"\nsource = "A"\nmode = "redrawn"\n')
        plan_report = W.build_waters(waters, surveys, grid).report
        self.assertIn("cells and the terrain they were: 46,5=pasture 47,5=pasture", plan_report)
        self.assertIn("- Pond Estuary: 20,20 21,20", plan_report)
        self.assertIn("cells: 30,20", plan_report)
        self.assertIn("returned cells and the land each takes: 5,1=pasture", plan_report)
        shrunk = W.Surveys(
            lakes=[W.Outline("A", "Alpha", 0.5, ([ring(1.0, 1.0, 4.0, 2.0)],), "epa")],
            tidal=surveys.tidal,
        )
        report = W.build_waters(waters, shrunk, grid).report
        self.assertIn("returned cells and the land each takes: 4,1=pasture 5,1=pasture", report)
        grown = W.Surveys(
            lakes=[W.Outline("A", "Alpha", 0.5, ([ring(1.0, 1.0, 4.0, 3.0)],), "epa")]
        )
        added = W.build_waters(waters, grown, grid).report
        self.assertIn("cells that become lough: 1,2 2,2 3,2", added)


class LocationImpact(unittest.TestCase):
    """Review 3, F2-R3: a location is reported as saved from flooding only where the mask would have flooded the cell."""

    def report(self, locations: Dyn, tidal_cells: tuple[float, float, float, float]) -> str:
        c0, r0, c1, r1 = tidal_cells
        ring = [CROP.latlon_at(c, r) for c, r in ((c0, r0), (c1, r0), (c1, r1), (c0, r1))]
        surveys = W.Surveys(tidal=[W.Outline("T", "Some Estuary", 1.0, ([ring],), "epa")])
        grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
        text: str = W.build_waters(W.Waters(), surveys, grid, locations).report
        return text

    def test_a_location_on_a_tidal_cell_joined_to_the_sea_is_reported(self) -> None:
        # cells (46..47, 5): (47, 5) touches the sea at column 48, and (46, 5) is joined through it
        report = self.report({"Joined": centre(46, 5)}, (46.0, 5.0, 48.0, 6.0))
        self.assertIn("Joined stands on a cell that would have become water", report)
        self.assertIn("location Joined (would have become sea)", report)

    def test_a_location_on_a_disconnected_tidal_cell_is_not(self) -> None:
        report = self.report({"Inland": centre(20, 20)}, (20.0, 20.0, 22.0, 21.0))
        self.assertIn(
            "- Some Estuary: 21,20", report
        )  # the kept cell is the location's; the other is left as river
        self.assertNotIn("Inland stands on a cell that would have become water", report)
        self.assertNotIn("would have become sea", report)
        plain = self.report({}, (20.0, 20.0, 22.0, 21.0))
        self.assertIn(
            "- Some Estuary: 20,20 21,20", plain
        )  # the mask puts water on neither cell with or without the location

    def test_a_location_behind_an_independent_keep_is_not_blamed_for_the_flooding(self) -> None:
        """Review 4, F3-R4: a [[keep]] at (47, 5) blocks the strip's only way to the sea, with or without the location."""
        ring = [
            CROP.latlon_at(c, r) for c, r in ((44.0, 5.0), (48.0, 5.0), (48.0, 6.0), (44.0, 6.0))
        ]
        surveys = W.Surveys(tidal=[W.Outline("T", "Some Estuary", 1.0, ([ring],), "epa")])
        grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
        point = centre(47, 5)
        waters = W.parse_waters(
            f'[[keep]]\nid = "k"\nreason = "owner"\npoint = [{point[0]}, {point[1]}]\n'
        )
        with_location = W.build_waters(waters, surveys, grid, {"Behind": centre(46, 5)})
        without = W.build_waters(waters, surveys, grid)
        self.assertEqual(
            with_location.water, without.water
        )  # the mask is the same, and (46, 5) is not water
        self.assertEqual(with_location.water[5][46], W.UNCHANGED)
        self.assertNotIn(
            "Behind stands on a cell that would have become water", with_location.report
        )
        self.assertNotIn("location Behind", with_location.report)
        self.assertIn(
            "keep k (would have become sea)", with_location.report
        )  # the keep itself would have flooded

    def test_a_location_sharing_a_cell_with_a_keep_is_not_reported_as_flooded(self) -> None:
        """Review 5, F1-R5: the [[keep]] holds the cell (it takes the label), so removing the location changes nothing."""
        point = centre(47, 5)
        for text, ring, words in (
            (
                f'[[keep]]\nid = "k"\nreason = "owner"\npoint = [{point[0]}, {point[1]}]\n',
                (46.0, 5.0, 48.0, 6.0),
                "sea",
            ),
            (
                f'[[lough]]\nname = "L"\nsource = "L1"\nmode = "added"\n\n[[keep]]\nid = "k"\nreason = "owner"\npoint = [{point[0]}, {point[1]}]\n',
                None,
                "lough",
            ),
        ):
            with self.subTest(words):
                lake = [
                    CROP.latlon_at(c, r)
                    for c, r in ((46.0, 5.0), (48.0, 5.0), (48.0, 6.0), (46.0, 6.0))
                ]
                tidal = (
                    [
                        W.Outline(
                            "T",
                            "Some Estuary",
                            1.0,
                            (
                                [
                                    [
                                        CROP.latlon_at(c, r)
                                        for c, r in (
                                            (46.0, 5.0),
                                            (48.0, 5.0),
                                            (48.0, 6.0),
                                            (46.0, 6.0),
                                        )
                                    ]
                                ],
                            ),
                            "epa",
                        )
                    ]
                    if ring
                    else []
                )
                lakes = [] if ring else [W.Outline("L1", "L", 0.5, ([lake],), "epa")]
                surveys = W.Surveys(tidal=tidal, lakes=lakes)
                grid = W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])
                waters = W.parse_waters(text)
                with_location = W.build_waters(waters, surveys, grid, {"Shared": centre(47, 5)})
                without = W.build_waters(waters, surveys, grid)
                self.assertEqual(
                    with_location.water, without.water
                )  # the keep holds (47, 5) either way
                self.assertNotIn(
                    "Shared stands on a cell that would have become water", with_location.report
                )
                self.assertIn("keep k (would have become", with_location.report)

    def test_the_location_makes_the_difference_only_at_the_exception(self) -> None:
        # a joined strip with the location in its middle: the cells beyond it are reached only through the kept cell
        report = self.report({"Mid": centre(46, 5)}, (44.0, 5.0, 48.0, 6.0))
        self.assertIn("location Mid (would have become sea)", report)


class PngEncoding(unittest.TestCase):
    """Rule 6: `water.png` has one specified encoding, tested here without Pillow or the zlib compressor."""

    KEYS: ClassVar[dict[str, tuple[int, int, int]]] = {
        "sea": (10, 26, 51),
        "lake": (47, 111, 184),
        "pasture": (160, 137, 68),
        "forest": (54, 117, 52),
        "bog": (44, 76, 45),
        "marsh": (92, 62, 37),
        "mountain": (127, 129, 135),
    }
    GOLDEN = (
        "89504e470d0a1a0a0000000d49484452000000030000000204030000006f5a7b2900000018504c54450000000a1a332f6fb8a089443675342c4c2d5c3e257f8187"
        "bd050e3100000011494441547801010600f9ff000120003070015b00c261c48d010000000049454e44ae426082"
    )

    def test_a_small_image_has_exactly_these_bytes(self) -> None:
        rows = [[W.UNCHANGED, W.SEA, W.LAKE], [W.PASTURE, W.UNCHANGED, W.MOUNTAIN]]
        data = W.png_bytes(rows, self.KEYS)
        self.assertEqual(data.hex(), self.GOLDEN)
        self.assertEqual(
            hashlib.sha256(data).hexdigest(),
            "01a5fab4d77a3dfc77a2e49b3a9a34fc0bd525dfa96feaf103a91ca71394fc89",
        )

    def chunks(self, data: bytes) -> list[tuple[bytes, bytes]]:
        self.assertEqual(data[:8], b"\x89PNG\r\n\x1a\n")
        found, position = [], 8
        while position < len(data):
            (length,) = struct.unpack(">I", data[position : position + 4])
            kind, body = (
                data[position + 4 : position + 8],
                data[position + 8 : position + 8 + length],
            )
            (crc,) = struct.unpack(">I", data[position + 8 + length : position + 12 + length])
            self.assertEqual(crc, zlib.crc32(kind + body))
            found.append((kind, body))
            position += 12 + length
        return found

    def test_a_large_image_uses_stored_blocks_that_decode_to_its_cells(self) -> None:
        width, height = (
            401,
            400,
        )  # odd width: each row is padded to whole bytes; 80,400 bytes: two stored blocks
        rows = [[(r * 7 + c * 3) % 8 - 1 for c in range(width)] for r in range(height)]
        data = W.png_bytes(rows, self.KEYS)
        kinds = [kind for kind, _ in self.chunks(data)]
        self.assertEqual(kinds, [b"IHDR", b"PLTE", b"IDAT", b"IEND"])
        header = dict(self.chunks(data))[b"IHDR"]
        self.assertEqual(struct.unpack(">IIBBBBB", header), (width, height, 4, 3, 0, 0, 0))
        stream = dict(self.chunks(data))[b"IDAT"]
        self.assertEqual(stream[:2], b"\x78\x01")
        self.assertEqual(
            (stream[2], stream[2 + 5 + 65_535]), (0, 1)
        )  # a stored block, then the final stored block
        raw = zlib.decompress(stream)
        stride = 1 + (width + 1) // 2
        self.assertEqual(len(raw), stride * height)
        for r in (0, 1, 200, 399):
            line = raw[r * stride : (r + 1) * stride]
            self.assertEqual(line[0], 0)  # filter type 0
            cells = []
            for byte in line[1:]:
                cells += [byte >> 4, byte & 15]
            self.assertEqual(cells[:width], [value + 1 for value in rows[r]])
        self.assertEqual(data, W.png_bytes(rows, self.KEYS))

    def test_a_png_reader_sees_the_same_pixels(self) -> None:
        try:
            image_module = importlib.import_module("PIL.Image")
        except ImportError:
            self.skipTest(
                "Pillow is not installed in this Python; the decoding above does not need it"
            )
        import io  # noqa: PLC0415  # only this test needs it

        rows = [[W.UNCHANGED, W.SEA, W.LAKE], [W.PASTURE, W.UNCHANGED, W.MOUNTAIN]]
        with image_module.open(io.BytesIO(W.png_bytes(rows, self.KEYS))) as image:
            pixels = list(image.convert("RGB").getdata())
        self.assertEqual(pixels[1], self.KEYS["sea"])
        self.assertEqual(pixels[4], (0, 0, 0))


class Mask(unittest.TestCase):
    KEYS: ClassVar[dict[str, tuple[int, int, int]]] = {
        "sea": (10, 26, 51),
        "lake": (47, 111, 184),
        "pasture": (160, 137, 68),
        "forest": (54, 117, 52),
    }

    def test_each_key_is_selected_only_by_the_edit_that_paints_it(self) -> None:
        black = (0, 0, 0)
        pixels = [
            [black, self.KEYS["sea"], self.KEYS["lake"]],
            [self.KEYS["pasture"], self.KEYS["forest"], black],
        ]
        for paint, expected in (
            ("sea", [[False, True, False], [False, False, False]]),
            ("lake", [[False, False, True], [False, False, False]]),
            ("pasture", [[False, False, False], [True, False, False]]),
            ("forest", [[False, False, False], [False, True, False]]),
        ):
            self.assertEqual(W.mask_cells(pixels, self.KEYS, paint, (3, 2)), expected)

    def test_a_wrong_size_or_a_stray_colour_is_refused(self) -> None:
        with self.assertRaises(W.WatersError) as raised:
            W.mask_cells([[(0, 0, 0)] * 3] * 2, self.KEYS, "sea", (4, 2))
        self.assertIn("3 x 2", str(raised.exception))
        with self.assertRaises(W.WatersError) as raised:
            W.mask_cells([[(0, 0, 0), (1, 2, 3)]], self.KEYS, "sea", (2, 1))
        self.assertIn("(1, 2, 3)", str(raised.exception))
        with self.assertRaises(W.WatersError):
            W.mask_cells([[(0, 0, 0)]], {**self.KEYS, "mountain": (0, 0, 0)}, "sea")

    def test_water_png_rows_use_the_new_class_and_black_elsewhere(self) -> None:
        plan = W.WaterPlan(change={(1, 0): W.SEA, (0, 1): W.PASTURE})
        self.assertEqual(
            W.water_rows([[2, 2], [1, 2]], plan), [[W.UNCHANGED, W.SEA], [W.PASTURE, W.UNCHANGED]]
        )


class Surveys(unittest.TestCase):
    def collection(self) -> dict[str, Any]:
        return {
            "features": [
                {
                    "properties": {
                        "Segment_Code": "01_1",
                        "EPA_Name": "Suir",
                        "ORDER_": 5,
                        "Continua": "L",
                        "Lake_Waterbody_Code": "IE_X",
                    },
                    "geometry": {
                        "type": "MultiLineString",
                        "coordinates": [[[-7.5, 52.4], [-7.4, 52.5]], [[-7.4, 52.5], [-7.3, 52.6]]],
                    },
                }
            ]
        }

    def test_epa_and_northern_ireland_segments_and_outlines_are_read(self) -> None:
        segments = W.epa_segments(self.collection())
        self.assertEqual([s.code for s in segments], ["01_1#1", "01_1#2"])
        self.assertEqual(
            (segments[0].name, segments[0].order, segments[0].continua, segments[0].lake_code),
            ("Suir", 5, "L", "IE_X"),
        )
        self.assertEqual(segments[0].points[0], (52.4, -7.5))  # GeoJSON is longitude, latitude
        ni = W.ni_segments(
            {
                "features": [
                    {
                        "id": 3,
                        "properties": {"OBJECTID": 3, "RWB_CD": "UKGBNI1", "STRAHLER": 6},
                        "geometry": {
                            "type": "LineString",
                            "coordinates": [[-6.5, 54.5], [-6.4, 54.6]],
                        },
                    }
                ]
            }
        )
        self.assertEqual(
            (ni[0].code, ni[0].water_body, ni[0].order, ni[0].name),
            ("UKGBNI1.3", "UKGBNI1", 6, None),
        )
        polygon = {
            "type": "MultiPolygon",
            "coordinates": [[[[-7.0, 52.0], [-7.0, 52.1], [-6.9, 52.1], [-7.0, 52.0]]]],
        }
        found = W.outlines(
            {
                "features": [
                    {
                        "properties": {"EU_CD": "T1", "NAME": "Bay", "AreaKm2": 2.5},
                        "geometry": polygon,
                    }
                ]
            },
            "epa",
            "EU_CD",
            "NAME",
            lambda p: float(p["AreaKm2"]),
        )
        self.assertEqual((found[0].code, found[0].name, found[0].area_km2), ("T1", "Bay", 2.5))

    def test_the_layers_in_use_each_carry_their_licence_evidence(self) -> None:
        used = [layer for layer in W.LAYERS if layer.confirmed]
        self.assertEqual(
            {layer.key for layer in used},
            {
                "epa-rivers",
                "epa-lakes",
                "epa-tidal",
                "epa-coastal",
                "ni-rivers",
                "ni-lakes",
                "ni-tidal",
            },
        )
        for layer in used:
            self.assertTrue(
                layer.evidence
                and layer.attribution
                and layer.licence.startswith(("Creative Commons", "UK Open Government")),
                layer.key,
            )
            self.assertRegex(layer.evidence, "read 2026-11-1[69]", layer.key)
        by_key = {layer.key: layer for layer in W.LAYERS}
        for key in (
            "ni-rivers",
            "ni-tidal",
        ):  # the services carry no licence text: only the downloads the publisher's page names are used
            self.assertTrue(by_key[key].archive and by_key[key].url.endswith(".zip"), key)
            self.assertNotIn("FeatureServer", by_key[key].url)
            self.assertIn("carries no licence text", by_key[key].evidence)
            self.assertIn("daera-ni.gov.uk/articles/wmu-spatial-datasets", by_key[key].evidence)
        self.assertNotIn("openstreetmap", " ".join(layer.url for layer in W.LAYERS).lower())

    def archive(self, directory: Path, name: str, gml: str) -> None:
        with zipfile.ZipFile(directory / name, "w") as bundle:
            bundle.writestr("data.gml", gml)

    def test_fetch_writes_a_manifest_leaves_out_an_unconfirmed_layer_and_converts_a_download(
        self,
    ) -> None:
        def stored(url: str, path: Path) -> dict[str, Any]:
            body = {
                "type": "FeatureCollection",
                "features": [{"id": 1, "properties": {}}],
                "numberMatched": 1,
            }
            path.write_text(json.dumps(body), encoding="utf-8")
            return body

        def download(url: str, path: Path) -> None:
            self.archive(path.parent, path.name, RIVER_GML if "river" in url else WATER_GML)

        unconfirmed = W.Layer(
            "x-extra",
            "An extra layer",
            "https://example.invalid/x",
            "not confirmed",
            "",
            "no licence text found",
            confirmed=False,
        )
        with tempfile.TemporaryDirectory(prefix="ck348-fetch-") as directory:
            source = Path(directory) / "cache"
            with (
                mock.patch.object(W, "_stored", stored),
                mock.patch.object(W, "_download", download),
                mock.patch.object(W, "LAYERS", (*W.LAYERS, unconfirmed)),
            ):
                manifest = W.fetch(source, force=False, today="2026-11-16")
                self.assertEqual(
                    sorted(entry["key"] for entry in manifest["layers"]),
                    [
                        "epa-coastal",
                        "epa-lakes",
                        "epa-rivers",
                        "epa-tidal",
                        "ni-lakes",
                        "ni-rivers",
                        "ni-tidal",
                    ],
                )
                self.assertEqual([entry["key"] for entry in manifest["left_out"]], ["x-extra"])
                self.assertIn("no licence text found", manifest["left_out"][0]["reason"])
                entry = next(e for e in manifest["layers"] if e["key"] == "epa-rivers")
                self.assertEqual((entry["day"], entry["features"]), ("2026-11-16", 1))
                self.assertEqual(
                    entry["sha256"],
                    hashlib.sha256((source / "epa-rivers.json").read_bytes()).hexdigest(),
                )
                self.assertTrue(entry["addresses"][0].startswith("https://"))
                self.assertFalse((source / "x-extra.json").exists())
                rivers = next(e for e in manifest["layers"] if e["key"] == "ni-rivers")
                self.assertEqual(rivers["features"], 1)  # the second segment is stream order 2
                self.assertEqual(
                    rivers["archive_sha256"],
                    hashlib.sha256((source / "ni-rivers.zip").read_bytes()).hexdigest(),
                )
                with self.assertRaises(W.WatersError) as raised:
                    W.fetch(source, force=False)
                self.assertIn("epa-rivers.json", str(raised.exception))
                W.fetch(source, force=True, today="2026-11-17")
                surveys = W.load_surveys(source)
            self.assertEqual([s.order for s in surveys.rivers if s.source == "ni"], [4])
            self.assertEqual([o.name for o in surveys.tidal if o.source == "ni"], ["Bann Estuary"])
            self.assertTrue(any("river names" in line for line in surveys.missing))
            self.assertFalse(any("tidal outlines" in line for line in surveys.missing))
            self.assertEqual(len(surveys.layers), 7)

    def test_irish_grid_agrees_with_the_epas_own_reprojection_to_a_centimetre(self) -> None:
        for (easting, northing), (latitude, longitude) in (
            ((189381.6903, 400454.2952), (54.85174094, -8.16606215)),
            ((111620.0, 65872.5193), (51.83864672, -9.28297508)),
            ((112115.9095, 65856.1755), (51.83857808, -9.27577802)),
        ):
            got = W.irish_grid_to_wgs84(easting, northing)
            self.assertAlmostEqual(got[0], latitude, places=6)
            self.assertAlmostEqual(got[1], longitude, places=6)

    def test_gml_downloads_are_read_as_geojson_in_longitude_and_latitude(self) -> None:
        rivers = W.gml_collection(RIVER_GML.encode(), min_order=4)
        self.assertEqual(len(rivers["features"]), 1)
        feature = rivers["features"][0]
        self.assertEqual(
            (
                feature["properties"]["RWB_CD"],
                feature["properties"]["STRAHLER"],
                feature["properties"]["OBJECTID"],
            ),
            ("UKGBNI1NW393901002", 4, 1),
        )
        self.assertAlmostEqual(
            feature["geometry"]["coordinates"][0][0][1], 54.85174094, places=6
        )  # latitude second, as GeoJSON
        segments = W.ni_segments(rivers)
        self.assertEqual(
            (segments[0].code, segments[0].order, segments[0].water_body),
            ("UKGBNI1NW393901002.1", 4, "UKGBNI1NW393901002"),
        )
        self.assertEqual(len(W.gml_collection(RIVER_GML.encode())["features"]), 2)
        water = W.gml_collection(WATER_GML.encode())
        outline = W.outlines(water, "ni", "EU_CD", "NAME", lambda p: float(p["Shape_Area"]) / 1e6)[
            0
        ]
        self.assertEqual(
            (outline.code, outline.name, len(outline.polygons), len(outline.polygons[0])),
            ("UKGBNI5NB010010", "Bann Estuary", 1, 2),
        )
        with self.assertRaises(W.WatersError):
            W.gml_collection(RIVER_GML.replace("EPSG:29902", "EPSG:2157").encode())

    def test_nothing_downloads_unless_fetch_runs(self) -> None:
        with mock.patch("urllib.request.urlopen", side_effect=AssertionError("a test downloaded")):
            W.parse_waters("")
            W.build_waters(
                W.Waters(),
                W.Surveys(),
                W.MapGrid(CROP, terrain_rows(), [[1.0] * 64 for _ in range(32)]),
            )


class Drainage(unittest.TestCase):
    def test_flow_passes_through_a_lough_and_ends_at_the_sea(self) -> None:
        width, height = 12, 7
        crop = W.Crop((22436, 14979), width, height)
        rows = terrain_rows(lambda c, r: c >= width - 2, [(5, 3), (6, 3)], width, height)
        slope = [[20.0 - 2 * c for c in range(width)] for _ in range(height)]
        flow = W.drainage_km2(slope, rows, crop)
        self.assertGreater(flow[3][8], flow[3][2])  # grows downstream, lough included
        self.assertAlmostEqual(flow[3][9], 10 * flow[3][0])  # ten cells of the row drain through it
        self.assertGreater(flow[3][7], flow[3][4])  # the lough's two cells pass their flow on
        self.assertAlmostEqual(flow[3][10], flow[3][9])  # the sea only receives


# ---- CK-354: the sea end and the two surveys -------------------------------------------------------------------------------


INLET_SEA_FROM = 52  # the inlet's open sea starts here: a cell is open from column 54
FAR_SEA_FROM = 60


def inlet_rows() -> list[list[int]]:
    """Land with open sea from column 54 on and a one-cell-wide inlet with two bends: (52, 10) west to (49, 10), south to (49, 12),
    west to (47, 12)."""
    rows = [[W.SEA if c >= INLET_SEA_FROM else W.PASTURE for c in range(64)] for _ in range(32)]
    for column, row in ((51, 10), (50, 10), (49, 10), (49, 11), (49, 12), (48, 12), (47, 12)):
        rows[row][column] = W.SEA
    return rows


def cells_walked(a: tuple[float, float], b: tuple[float, float]) -> list[tuple[int, int]]:
    """The cells a straight leg passes through, in order, sampled finely in cell units."""
    (x_0, y_0), (x_1, y_1) = CROP.cell_of(*a), CROP.cell_of(*b)
    cells: list[tuple[int, int]] = []
    for step in range(401):
        share = step / 400
        cell = (math.floor(x_0 + (x_1 - x_0) * share), math.floor(y_0 + (y_1 - y_0) * share))
        if not cells or cells[-1] != cell:
            cells.append(cell)
    return cells


def written_stretches(network: Dyn, river_def: Dyn) -> list[dict[str, Any]]:
    """The stretches of the built file, parsed back: what is written, after thinning and rounding."""
    text = W.rivers_toml(
        W.Waters(rivers=(river_def,)), network, {s.id: (3.0, 4.0) for s in network.stretches}, "x"
    )
    stretches: list[dict[str, Any]] = tomllib.loads(text)["stretch"]
    return stretches


def at(column: float, row: float) -> tuple[float, float]:
    point: tuple[float, float] = W.round_point(CROP.latlon_at(column, row))
    return point


class SeaEnd(unittest.TestCase):
    """CK-354 rules 1 to 6: where the sea is, the one mouth of a piece, the run through the sea, the refusal and the last reach."""

    def test_the_dead_end_of_a_tidal_side_line_is_not_a_mouth(self) -> None:
        channel_1 = segment("c1", [(30, 10), (35, 10)], None, continua="T")
        channel_2 = segment("c2", [(35, 10), (55, 10)], None, continua="T")
        side = segment("side", [(35, 4), (35, 10)], None, continua="T")
        pieces = [segment("m", [(2, 10), (30, 10)], "Main"), channel_1, channel_2, side]
        network = build(pieces, [river("main", "Main")])
        check_network(self, network)
        self.assertEqual([s.ends for s in network.stretches], ["main-02", "sea"])
        self.assertEqual(
            sum(1 for s in network.stretches if s.ends == "sea"), 1
        )  # the built file has one sea end
        self.assertEqual(
            {c for s in network.stretches for c in s.codes} & {"side"}, set()
        )  # unlisted, the side line is not built
        graph = W.prepare_graph(pieces, W.Terrain(CROP, terrain_rows()))
        side_edge = next(e for e in graph.edges if e.seg.code == "side")
        self.assertEqual(
            (side_edge.start, side_edge.end), (centre(35, 4), centre(35, 10))
        )  # it runs into its channel

    def test_a_listed_side_line_is_a_stretch_that_ends_at_the_channel(self) -> None:
        pieces = [
            segment("m", [(2, 10), (30, 10)], "Main"),
            segment("c1", [(30, 10), (35, 10)], None, continua="T"),
            segment("c2", [(35, 10), (55, 10)], None, continua="T"),
            segment("side", [(35, 4), (35, 10)], "Side", continua="T"),
        ]
        network = build(pieces, [river("main", "Main"), river("side", "Side")])
        check_network(self, network)
        by_id = {s.id: s for s in network.stretches}
        channel = next(s for s in network.stretches if s.points[0] == centre(35, 10))
        self.assertEqual(by_id["side-01"].ends, channel.id)
        self.assertEqual(channel.ends, "sea")

    def test_a_piece_that_reaches_no_open_sea_takes_the_loose_end_with_the_shorter_way(
        self,
    ) -> None:
        # (47, 12) is one cell from the sea and (46, 5) two: the way from (47, 12) is shorter
        pieces = [
            segment("s1", [(47, 12), (30, 8)], "Main"),
            segment("s2", [(30, 8), (46, 5)], "Main"),
        ]
        graph = W.prepare_graph(pieces, W.Terrain(CROP, terrain_rows()))
        self.assertEqual(graph.distance[centre(47, 12)], 0.0)
        self.assertGreater(graph.distance[centre(46, 5)], 0.0)

    def test_a_piece_with_no_sea_in_reach_takes_the_loose_end_nearer_the_sea(self) -> None:
        rows = terrain_rows(lambda c, r: c >= FAR_SEA_FROM)
        pieces = [
            segment("s1", [(10, 5), (20, 8)], "Main"),
            segment("s2", [(20, 8), (30, 20)], "Main"),
        ]
        graph = W.prepare_graph(pieces, W.Terrain(CROP, rows))
        self.assertEqual(
            graph.distance[centre(30, 20)], 0.0
        )  # 30 cells from the sea; (10, 5) is 50
        self.assertGreater(graph.distance[centre(10, 5)], 0.0)

    def test_a_line_that_ends_in_an_inlet_runs_round_the_bends_to_open_sea(self) -> None:
        terrain = W.Terrain(CROP, inlet_rows())
        pieces = [segment("r", [(20, 12), (47, 12)], "Main")]
        network = W.build_network(pieces, [river("main", "Main")], terrain)
        stretch = network.stretches[0]
        self.assertEqual(stretch.survey_end, centre(47, 12))
        self.assertEqual(stretch.run, (centre(49, 12), centre(49, 10), centre(54, 10)))
        self.assertEqual(stretch.ends, "sea")
        self.assertTrue(terrain.open_sea(stretch.points[-1]))

    def written_legs_of_the_run(self, network: Dyn, river_def: Dyn) -> list[tuple[Dyn, Dyn]]:
        [stretch] = written_stretches(network, river_def)
        points = [tuple(p) for p in stretch["points"]]
        end = network.stretches[0].survey_end
        index = next(i for i, p in enumerate(points) if p == end)
        return list(zip(points[index:], points[index + 1 :], strict=False))

    def test_every_leg_of_a_run_in_the_built_file_walks_through_sea_cells(self) -> None:
        """Review 1, finding 4: the check is on what is written, with a simplification that would remove the bend."""
        terrain = W.Terrain(CROP, inlet_rows())
        pieces = [segment("r", [(20, 12), (47, 12)], "Main")]
        for tolerance in (8.0, 100_000.0):
            network = W.build_network(
                pieces,
                [river("main", "Main")],
                terrain,
                W.Settings(simplify_m=tolerance),
            )
            legs = self.written_legs_of_the_run(network, river("main", "Main"))
            self.assertGreaterEqual(
                len(legs), 3, tolerance
            )  # both bends and the open-sea end are still there
            for start, finish in legs:
                for cell in cells_walked(start, finish):
                    self.assertTrue(terrain.is_sea(cell), (tolerance, start, finish, cell))
        written = [tuple(p) for p in written_stretches(network, river("main", "Main"))[0]["points"]]
        self.assertIn(centre(47, 12), written)  # the last survey point is kept too

    def test_a_line_that_ends_one_cell_from_the_sea_takes_one_step_then_the_run(self) -> None:
        terrain = W.Terrain(CROP, terrain_rows())
        network = W.build_network(
            [segment("r", [(2, 10), (47, 10)], "Main")], [river("main", "Main")], terrain
        )
        stretch = network.stretches[0]
        self.assertEqual(stretch.survey_end, centre(47, 10))
        legs = self.written_legs_of_the_run(network, river("main", "Main"))
        first_leg, *rest = legs
        self.assertEqual(first_leg[1], stretch.run[0])
        self.assertTrue(terrain.is_sea(CROP.cell_containing(first_leg[1])))  # the step lands in sea
        self.assertTrue(rest)
        for start, finish in rest:  # the step alone may cross land
            for cell in cells_walked(start, finish):
                self.assertTrue(terrain.is_sea(cell), (start, finish, cell))
        self.assertTrue(terrain.open_sea(stretch.points[-1]))

    def test_a_line_five_cells_from_the_sea_or_in_a_pool_is_refused_and_nothing_is_written(
        self,
    ) -> None:
        with self.assertRaises(W.WatersError) as raised:
            build([segment("r", [(2, 10), (43, 10)], "Main")], [river("main", "Main")])
        message = str(raised.exception)
        for words in (
            "river main",
            f"{centre(43, 10)[0]:.5f}, {centre(43, 10)[1]:.5f}",
            "5 cells from the sea",
        ):
            self.assertIn(words, message)
        rows = terrain_rows()
        rows[10][20] = rows[10][21] = W.SEA  # a pool: sea that no way through sea takes to open sea
        with self.assertRaises(W.WatersError) as raised:
            build([segment("r", [(2, 10), (20, 10)], "Main")], [river("main", "Main")], rows)
        for words in ("river main", f"{centre(20, 10)[0]:.5f}, {centre(20, 10)[1]:.5f}", "pool"):
            self.assertIn(words, str(raised.exception))

    def test_a_stretch_whose_onward_line_was_dropped_beyond_open_sea_takes_the_run(self) -> None:
        """Rule 5: it is not reported as ending at a line that is not in the list. The graph is made by hand: a line that crosses
        open sea and ends on an island, the line beyond it (dropped), and a side river that ends at the island."""
        rows = terrain_rows()
        for column in range(56, 61):
            for row in range(8, 13):
                rows[row][column] = W.PASTURE
        terrain = W.Terrain(CROP, rows)
        island, beyond = centre(58, 10), centre(60, 10)
        through = segment("e1", [(40, 10), (52, 10), (58, 10)], "Main")
        onward = segment("e2", [(58, 10), (60, 10)], "Main")
        side = segment("s", [(58, 14), (58, 10)], "Side")
        edges = [
            W.Edge(through, None, "river", list(through.points)),
            W.Edge(onward, None, "river", [island, beyond]),
            W.Edge(side, None, "river", list(side.points)),
        ]
        graph = W.Graph({}, edges, frozenset({"e1", "e2", "s"}))
        network = W.build_network(
            [], [river("main", "Main"), river("side", "Side")], terrain, graph=graph
        )
        self.assertEqual(list(network.sea_runs), ["side-01"])
        self.assertTrue(terrain.open_sea(network.stretches[-1].points[-1]))

    def test_a_course_may_be_a_rivers_last_reach(self) -> None:
        """Rule 6: the survey segments it replaces met the river at one point only: it starts there and its far end is free."""
        pieces = [
            segment("m1", [(2, 10), (20, 10)], "Main"),
            segment("m2", [(20, 10), (30, 10)], "Main"),
        ]
        course = W.Correction(
            "last-reach",
            "drawn by hand",
            "medium",
            "main",
            course=(centre(20, 10), at(30.5, 11.5), centre(46, 11)),
            replaces=("m2",),
        )
        network = build(pieces, [river("main", "Main")], corrections=(course,))
        check_network(self, network)
        points = [p for stretch in network.stretches for p in stretch.points]
        self.assertEqual(points[0], centre(2, 10))
        self.assertIn(centre(46, 11), points)  # the far end of the course is free
        self.assertEqual(network.stretches[-1].ends, "sea")  # rule 4's run follows from it

    def test_a_course_that_meets_the_river_nowhere_is_still_refused(self) -> None:
        pieces = [
            segment("m1", [(2, 10), (20, 10)], "Main"),
            segment("m2", [(20, 10), (30, 10)], "Main"),
        ]
        course = W.Correction(
            "far", "x", "low", "main", course=(centre(2, 30), centre(46, 11)), replaces=("m2",)
        )
        with self.assertRaises(W.WatersError):
            build(pieces, [river("main", "Main")], corrections=(course,))

    def test_the_same_inputs_give_the_same_bytes(self) -> None:
        terrain = W.Terrain(CROP, inlet_rows())
        pieces = [
            segment("r", [(20, 12), (47, 12)], "Main"),
            segment("t", [(20, 20), (20, 12)], "Branch"),
        ]
        rivers = [river("main", "Main"), river("branch", "Branch")]
        first = W.build_network(pieces, rivers, terrain)
        second = W.build_network(list(reversed(pieces)), list(reversed(rivers)), terrain)
        self.assertEqual(signature(first), signature(second))
        self.assertEqual(first.sea_runs, second.sea_runs)
        self.assertEqual([s.run for s in first.stretches], [s.run for s in second.stretches])
        self.assertTrue(first.sea_runs)


NI_LINE = [(2, 10), (46, 10)]


class TwoSurveys(unittest.TestCase):
    """CK-354 rules 7 to 9: one line that both surveys have; `segments`; the order of claims; DAERA's marks."""

    def pair(self) -> list[Dyn]:
        return [
            segment("E1", NI_LINE, "Main", continua="L", lake="IE_X_1"),
            segment("UKNI-B1.5", NI_LINE, None, source="ni", body="UKNI-B1"),
        ]

    def test_a_line_in_both_surveys_is_one_line_with_both_identities(self) -> None:
        merged = W._cleaned(self.pair())
        self.assertEqual(len(merged), 1)
        [line] = merged
        self.assertEqual(
            (
                line.code,
                line.twin,
                line.source,
                line.name,
                line.water_body,
                line.continua,
                line.lake_code,
            ),
            ("UKNI-B1.5", "E1", "both", "Main", "UKNI-B1", "L", "IE_X_1"),
        )
        self.assertEqual(line.codes, ("UKNI-B1.5", "E1"))

    def test_a_river_listed_by_the_name_and_one_by_the_water_body_build_the_line_once(self) -> None:
        for definition in (
            river("main", "Main"),
            W.RiverDef("ni", "Ni", 3, rwb_codes=("UKNI-B1",)),
        ):
            network = build(self.pair(), [definition])
            self.assertEqual(len(network.stretches), 1, definition.id)
            self.assertEqual(W.courses_built_twice(network), [])

    def test_two_lines_between_the_same_points_with_different_lengths_are_both_kept(self) -> None:
        detour = W.Seg(
            "UKNI-B1.6", "ni", 5, (centre(2, 10), at(24.5, 14.5), centre(46, 10)), None, "UKNI-B1"
        )
        merged = W._cleaned([segment("E1", NI_LINE, "Main"), detour])
        self.assertEqual(sorted(s.code for s in merged), ["E1", "UKNI-B1.6"])
        self.assertTrue(all(s.twin is None for s in merged))

    def test_of_two_irish_lines_that_fit_one_daera_line_the_closer_in_length_is_paired(
        self,
    ) -> None:
        near = W.Seg("E2", "epa", 5, (centre(2, 10), at(24.5, 10.9), centre(46, 10)), "Main")
        merged = W._cleaned(
            [near, segment("E1", NI_LINE, "Main"), segment("N1", NI_LINE, source="ni", body="B")]
        )
        self.assertEqual(sorted((s.code, s.twin) for s in merged), [("E2", None), ("N1", "E1")])

    def test_a_line_named_by_one_river_and_in_the_water_body_of_another_goes_to_the_name(
        self,
    ) -> None:
        pieces = [
            segment("E1", [(2, 10), (25, 10)], "Main"),
            segment("N1", [(2, 10), (25, 10)], None, source="ni", body="UKNI-B1"),
            segment("N2", [(25, 10), (46, 10)], None, source="ni", body="UKNI-B1"),
        ]
        rivers = [river("main", "Main"), W.RiverDef("ni", "Ni", 3, rwb_codes=("UKNI-B1",))]
        network = build(pieces, rivers)  # nothing is refused
        check_network(self, network)
        self.assertEqual({s.river for s in network.stretches}, {"main", "ni"})
        self.assertEqual(
            [(c.seg.code, c.river, c.other, c.how) for c in network.contests],
            [("N1", "main", "ni", "name")],
        )
        report = "\n".join(W._contests_report(network))
        self.assertIn("river ni: 1 lines", report)
        self.assertIn("N1 / E1", report)
        self.assertIn("river main has it by its name", report)

    def test_the_irish_code_of_a_merged_line_claims_omits_and_replaces_it(self) -> None:
        lead = segment("L0", [(2, 10), (25, 10)], "Main")
        both = [
            segment("E1", [(25, 10), (46, 10)], "Main"),
            segment("UKNI-B1.5", [(25, 10), (46, 10)], None, source="ni", body="UKNI-B1"),
        ]
        claimed = build([lead, *both], [W.RiverDef("r", "R", 3, segments=("E1", "L0"))])
        self.assertEqual(len(claimed.stretches), 1)
        for code in ("E1", "UKNI-B1.5"):
            omit = W.Correction("o", "x", "high", "main", omit=(code,))
            self.assertEqual([s.code for s in W._prepare([lead, *both], [omit])], ["L0"], code)
            course = W.Correction(
                "c", "x", "high", "main", course=(centre(25, 10), centre(46, 10)), replaces=(code,)
            )
            result = W._prepare([lead, *both], [course])
            self.assertEqual([s.code for s in result], ["L0", "course:c"], code)

    def test_a_line_in_the_segments_of_two_rivers_by_its_two_codes_is_refused(self) -> None:
        pieces = [
            segment("E1", NI_LINE, "Main"),
            segment("UKNI-B1.5", NI_LINE, None, source="ni", body="UKNI-B1"),
        ]
        rivers = [
            W.RiverDef("ra", "Ra", 3, segments=("E1",)),
            W.RiverDef("rb", "Rb", 3, segments=("UKNI-B1.5",)),
        ]
        with self.assertRaises(W.WatersError) as raised:
            build(pieces, rivers)
        for words in ("E1", "UKNI-B1.5", "ra", "rb"):
            self.assertIn(words, str(raised.exception))
        with self.assertRaises(W.WatersError) as raised:
            W.parse_waters(
                '[[river]]\nid = "a"\nname = "A"\ntier = 3\nsource = { segments = ["u"] }\n'
                '[[river]]\nid = "b"\nname = "B"\ntier = 3\nsource = { segments = ["u"] }\n'
            )
        self.assertIn("segment 'u' is in the segments of rivers a and b", str(raised.exception))

    def test_segments_win_over_a_name_and_a_water_body(self) -> None:
        other_1 = segment("E2", [(2, 12), (46, 12)], "Bn")
        other_2 = segment("N9", [(2, 14), (46, 14)], None, source="ni", body="UKNI-C1")
        rivers = [
            W.RiverDef("a", "A", 3, segments=("E1",)),
            W.RiverDef("b", "B", 3, epa_names=("Main", "Bn")),
            W.RiverDef("c", "C", 3, rwb_codes=("UKNI-B1", "UKNI-C1")),
        ]
        network = build([*self.pair(), other_1, other_2], rivers)
        self.assertEqual({s.river for s in network.stretches}, {"a", "b", "c"})
        self.assertEqual(
            sorted((c.other, c.river, c.how) for c in network.contests),
            [("b", "a", "segments"), ("c", "a", "segments")],
        )

    def test_a_line_with_no_name_and_no_water_body_is_listed_by_its_segment_code(self) -> None:
        pieces = [
            segment("a", [(2, 10), (25, 10)], "Main"),
            segment("u", [(25, 10), (46, 10)], None, source="ni"),
        ]
        network = build(pieces, [W.RiverDef("main", "Main", 3, ("Main",), (), ("u",))])
        check_network(self, network)
        self.assertEqual(network.stretches[0].codes, ("a", "u"))

    def test_an_unknown_segment_code_is_refused_and_an_unlisted_line_is_named_by_its_code(
        self,
    ) -> None:
        pieces = [
            segment("a", [(2, 10), (25, 10)], "Main"),
            segment("u", [(25, 10), (46, 10)], None, source="ni"),
        ]
        with self.assertRaises(W.WatersError) as raised:
            build(pieces, [W.RiverDef("main", "Main", 3, ("Main",), (), ("nope",))])
        self.assertIn("no segment 'nope'", str(raised.exception))
        with self.assertRaises(W.WatersError) as raised:
            build(pieces, [river("main", "Main")])
        message = str(raised.exception)
        self.assertIn("segment u at", message)
        self.assertIn(f"{centre(25, 10)[0]:.5f}, {centre(25, 10)[1]:.5f}", message)

    def test_daeras_lough_and_tidal_marks_are_read(self) -> None:
        def feature(code: str, kind: str, cells: list[tuple[int, int]]) -> Dyn:
            return {
                "properties": {
                    "OBJECTID": len(cells),
                    "RWB_CD": code,
                    "STRAHLER": 5,
                    "SEGMENT_TYPE": kind,
                },
                "geometry": {
                    "type": "LineString",
                    "coordinates": [[centre(c, r)[1], centre(c, r)[0]] for c, r in cells],
                },
            }

        read = W.ni_segments(
            {
                "features": [
                    feature("UKNI-B1", "Y", [(2, 10), (8, 10)]),
                    feature("UKNI-B1", "L", [(8, 10), (14, 10)]),
                    feature("", "T", [(14, 10), (46, 10)]),
                ]
            }
        )
        self.assertEqual([s.continua for s in read], [None, "L", "T"])
        self.assertEqual([s.water_body for s in read], ["UKNI-B1", "UKNI-B1", None])
        rows = terrain_rows(lakes=[(10, 10), (11, 10)])
        network = build(
            read,
            [W.RiverDef("ni", "Ni", 3, rwb_codes=("UKNI-B1",), segments=(read[2].code,))],
            rows,
        )
        self.assertEqual([s.kind for s in network.stretches], ["river", "lough", "tidal"])


class ProposalClosure(unittest.TestCase):
    """CK-354 rule 14: `propose` on lines of rule 7, and closing over a line that has only a segment code."""

    def grid(self) -> Dyn:
        return W.MapGrid(CROP, terrain_rows(), [[10.0] * 64 for _ in range(32)])

    @staticmethod
    def named(text: str) -> str:
        counter = iter(range(1, 100))
        return re.sub(r'name = ""', lambda _: f'name = "River {next(counter)}"', text)

    def propose(self, rivers: list[Dyn]) -> tuple[str, Dyn]:
        surveys = W.Surveys(rivers=rivers)
        text = W.propose_waters(surveys, self.grid(), area_source=lambda point, reach: 50.0)
        return text, surveys

    def built(self, text: str, surveys: Dyn) -> Dyn:
        # the owner's [[keep]] points lie outside the small crop of these tests
        text = re.sub(r"\n\[\[keep\]\]\n(?:[^\n\[][^\n]*\n)+", "\n", text)
        return W.build_waters(
            W.parse_waters(self.named(text)), surveys, self.grid(), area_source=lambda p, r: 50.0
        )

    def test_a_line_in_both_surveys_counts_for_its_irish_name_and_a_body_left_with_no_line_is_not_proposed(
        self,
    ) -> None:
        rivers = [
            segment("E1", NI_LINE, "Alpha", order=5),
            segment("N1", NI_LINE, None, source="ni", order=5, body="UKNI-B9"),
            segment("E2", [(2, 20), (8, 20)], "Tiny", order=5),
            segment("N2", [(2, 20), (8, 20)], None, source="ni", order=5, body="UKNI-B8"),
            segment("E3", [(8, 20), (46, 20)], "Tiny", order=5),
        ]
        rivers[2:] = [
            segment("E2", [(2, 20), (8, 20)], "Tiny", order=5),
            segment("N2", [(2, 20), (8, 20)], None, source="ni", order=5, body="UKNI-B8"),
            segment("N3", [(8, 20), (46, 20)], None, source="ni", order=5, body="UKNI-B8"),
        ]
        text, surveys = self.propose(rivers)
        document = tomllib.loads(text)
        sources = [r["source"] for r in document["river"]]
        self.assertEqual(
            sources, [{"epa_names": ["Alpha"]}, {"rwb_codes": ["UKNI-B8"]}]
        )  # no UKNI-B9
        result = self.built(text, surveys)  # nothing is refused
        self.assertEqual(
            {s.river for s in result.network.stretches}, {r["id"] for r in document["river"]}
        )
        alpha = next(r["id"] for r in document["river"] if r["name"] == "Alpha")
        codes = {c for s in result.network.stretches if s.river == alpha for c in s.codes}
        self.assertTrue(codes & {"UKNI-B1.5", "N1"})

    def tidal_tail(self, code: str, cells: list[tuple[int, int]]) -> Dyn:
        return segment(code, cells, None, source="ni", continua="T")

    def test_a_reach_that_ends_at_a_tidal_line_with_only_a_segment_code_is_closed_over(
        self,
    ) -> None:
        rivers = [
            segment("UKNI-B1.1", [(2, 10), (25, 10)], None, source="ni", order=5, body="UKNI-B1"),
            self.tidal_tail(".7", [(25, 10), (55, 10)]),
        ]
        text, surveys = self.propose(rivers)
        document = tomllib.loads(text)
        [entry] = document["river"]
        self.assertEqual(entry["source"], {"rwb_codes": ["UKNI-B1"], "segments": [".7"]})
        self.assertIn(
            "1 segment with no name and no water body added so that it reaches the sea", text
        )
        result = self.built(text, surveys)
        self.assertEqual(result.network.stretches[-1].ends, "sea")
        self.assertNotIn("not built", text)

    def test_two_such_lines_one_after_the_other_are_both_added(self) -> None:
        rivers = [
            segment("UKNI-B1.1", [(2, 10), (25, 10)], None, source="ni", order=5, body="UKNI-B1"),
            self.tidal_tail(".7", [(25, 10), (35, 10)]),
            self.tidal_tail(".8", [(35, 10), (55, 10)]),
        ]
        text, surveys = self.propose(rivers)
        [entry] = tomllib.loads(text)["river"]
        self.assertEqual(entry["source"]["segments"], [".7", ".8"])
        self.assertEqual(self.built(text, surveys).network.stretches[-1].ends, "sea")

    def test_of_two_rivers_ending_at_the_same_such_line_the_first_by_id_has_it(self) -> None:
        rivers = [
            segment("UKNI-A.1", [(2, 10), (25, 10)], None, source="ni", order=5, body="UKNI-A"),
            segment("UKNI-B.1", [(2, 14), (25, 10)], None, source="ni", order=5, body="UKNI-B"),
            self.tidal_tail(".7", [(25, 10), (55, 10)]),
        ]
        text, surveys = self.propose(rivers)
        document = tomllib.loads(text)
        by_id = {r["id"]: r for r in document["river"]}
        self.assertEqual(by_id["ni-ukni-a"]["source"]["segments"], [".7"])
        self.assertNotIn("segments", by_id["ni-ukni-b"]["source"])
        result = self.built(text, surveys)
        ends = {s.river: s.ends for s in result.network.stretches if s.river == "ni-ukni-b"}
        self.assertTrue(ends["ni-ukni-b"].startswith("ni-ukni-a"))  # the other ends at that river

    def test_a_merged_line_keeps_each_surveys_own_stream_order_for_eligibility(self) -> None:
        """Review 1, finding 1: a shared 25 km reach with order 5 in one survey and 4 in the other qualifies only the side at 5."""
        for epa_order, ni_order, expected in (
            (5, 4, [{"epa_names": ["Alpha"]}]),
            (4, 5, [{"rwb_codes": ["UKNI-B9"]}]),
        ):
            rivers = [
                segment("E1", NI_LINE, "Alpha", order=epa_order),
                segment("N1", NI_LINE, None, source="ni", order=ni_order, body="UKNI-B9"),
            ]
            candidates = W.candidate_rivers(W._cleaned(rivers))
            self.assertEqual(
                [
                    {
                        k: list(v)
                        for k, v in (("epa_names", r.epa_names), ("rwb_codes", r.rwb_codes))
                        if v
                    }
                    for r in candidates
                ],
                expected,
                (epa_order, ni_order),
            )
            text, _ = self.propose(rivers)
            self.assertEqual([r["source"] for r in tomllib.loads(text)["river"]], expected)

    def test_a_shared_line_goes_to_the_first_id_whatever_else_a_river_ends_at(self) -> None:
        """Review 1, finding 2: ni-a also ends at `.only-a`, and `.shared` is still ni-a's, the first id."""
        rivers = [
            segment("UKNI-A.1", [(2, 10), (25, 10)], None, source="ni", order=5, body="UKNI-A"),
            segment("UKNI-A.2", [(2, 16), (25, 16)], None, source="ni", order=5, body="UKNI-A"),
            segment("UKNI-B.1", [(2, 6), (25, 10)], None, source="ni", order=5, body="UKNI-B"),
            self.tidal_tail(".shared", [(25, 10), (55, 10)]),
            self.tidal_tail(".only-a", [(25, 16), (55, 16)]),
        ]
        text, surveys = self.propose(rivers)
        by_id = {r["id"]: r for r in tomllib.loads(text)["river"]}
        self.assertEqual(sorted(by_id["ni-ukni-a"]["source"]["segments"]), [".only-a", ".shared"])
        self.assertNotIn("segments", by_id["ni-ukni-b"]["source"])
        result = self.built(text, surveys)
        self.assertTrue(
            next(s for s in result.network.stretches if s.river == "ni-ukni-b").ends.startswith(
                "ni-ukni-a"
            )
        )


class SeaEntries(unittest.TestCase):
    """CK-354 rules 10, 11 and 13: `[[sea]]`."""

    @staticmethod
    def ring(c0: float, r0: float, c1: float, r1: float) -> list[list[tuple[float, float]]]:
        return [[CROP.latlon_at(c, r) for c, r in ((c0, r0), (c1, r0), (c1, r1), (c0, r1))]]

    def surveys(self) -> Dyn:
        return W.Surveys(
            tidal=[W.Outline("T", "Behind Bay", 1.0, (self.ring(40, 5, 44, 6),), "epa")],
            coastal=[W.Outline("C1", "Coast", 1.0, (self.ring(44, 5, 48, 6),), "epa")],
        )

    def build(self, text: str, rows: list[list[int]] | None = None, locations: Dyn = None) -> Dyn:
        grid = W.MapGrid(CROP, rows or terrain_rows(), [[10.0] * 64 for _ in range(32)])
        return W.build_waters(W.parse_waters(text), self.surveys(), grid, locations)

    def test_an_outline_opens_a_mouth_and_the_estuary_behind_it_joins(self) -> None:
        closed = self.build("")
        self.assertEqual(len(closed.plan.tidal_unjoined), 4)  # the bay is shut in by land
        opened = self.build('[[sea]]\nname = "Mouth"\nreason = "owner"\nsource = "C1"\n')
        self.assertEqual(opened.plan.tidal_unjoined, {})
        self.assertEqual({c for c, _ in opened.plan.sea_new}, set(range(40, 48)))
        self.assertIn("- Mouth (outline C1): 4 cells: owner", opened.report)
        self.assertIn("become sea: 4 cells (4 pasture)", opened.report)

    def test_points_open_a_mouth_narrower_than_a_cell(self) -> None:
        points = ", ".join(f"[{centre(c, 5)[0]}, {centre(c, 5)[1]}]" for c in (44, 45, 46, 47))
        opened = self.build(f'[[sea]]\nname = "Mouth"\nreason = "owner"\npoints = [{points}]\n')
        self.assertEqual(opened.plan.tidal_unjoined, {})
        self.assertIn("- Mouth (4 points): 4 cells: owner", opened.report)

    def test_a_kept_cell_and_a_lake_cell_do_not_change_and_are_reported(self) -> None:
        rows = terrain_rows(lakes=[(45, 5)])
        opened = self.build(
            '[[sea]]\nname = "Mouth"\nreason = "owner"\nsource = "C1"\n',
            rows,
            {"Here": centre(46, 5)},
        )
        self.assertNotIn((45, 5), opened.plan.change)  # a lake cell stays a lake
        self.assertNotIn((46, 5), opened.plan.change)  # the location's cell is kept
        self.assertEqual(opened.plan.change[(47, 5)], W.SEA)
        self.assertIn("kept (a location or a [[keep]] holds them): 1 cells", opened.report)
        self.assertIn("not joined to the sea: 1 cells", opened.report)  # (44, 5) is cut off by them

    def test_an_entry_that_changes_nothing_is_said_to(self) -> None:
        point = centre(60, 10)
        opened = self.build(
            f'[[sea]]\nname = "Idle"\nreason = "owner"\npoints = [[{point[0]}, {point[1]}]]\n'
        )
        self.assertIn("this entry changes nothing", opened.report)

    def test_an_unknown_key_a_missing_choice_an_unknown_code_and_a_point_outside_are_refused(
        self,
    ) -> None:
        for text, words in (
            (
                '[[sea]]\nname = "X"\nreason = "r"\nsource = "C1"\ncolour = 1\n',
                ("[[sea]] #1 (X)", "colour"),
            ),
            ('[[sea]]\nname = "X"\nreason = "r"\n', ("[[sea]] #1 (X)", "exactly one")),
            (
                '[[sea]]\nname = "X"\nreason = "r"\nsource = "C1"\npoints = [[1, 2]]\n',
                ("exactly one",),
            ),
        ):
            with self.assertRaises(W.WatersError) as raised:
                W.parse_waters(text)
            for word in words:
                self.assertIn(word, str(raised.exception))
        with self.assertRaises(W.WatersError) as raised:
            self.build('[[sea]]\nname = "X"\nreason = "r"\nsource = "Z9"\n')
        self.assertIn("[[sea]] X", str(raised.exception))
        self.assertIn("'Z9'", str(raised.exception))
        with self.assertRaises(W.WatersError) as raised:
            self.build('[[sea]]\nname = "X"\nreason = "r"\npoints = [[10.0, 10.0]]\n')
        self.assertIn("outside the crop", str(raised.exception))

    def test_the_same_inputs_give_the_same_bytes_and_the_location_is_named(self) -> None:
        text = '[[sea]]\nname = "Mouth"\nreason = "owner"\nsource = "C1"\n'
        first = self.build(text, locations={"Here": centre(45, 5)})
        second = self.build(text, locations={"Here": centre(45, 5)})
        self.assertEqual((first.report, first.rivers_toml), (second.report, second.rivers_toml))
        self.assertIn("Here stands on a cell that would have become water", first.report)


class CoursesTwice(unittest.TestCase):
    """CK-354 rule 9: the report's section "Courses built twice"."""

    @staticmethod
    def stretch(identifier: str, points: list[tuple[float, float]]) -> Dyn:
        return W.Stretch(id=identifier, points=points)

    def test_two_courses_ten_metres_apart_for_four_hundred_metres_are_listed_and_a_crossing_is_not(
        self,
    ) -> None:
        lat, lon = 53.0, -8.0
        step = 0.0064  # about 430 m of longitude here
        shift = 10 / 111_319.49  # 10 m of latitude
        network = W.Network(
            stretches=[
                self.stretch("a-01", [(lat, lon), (lat, lon + step)]),
                self.stretch("b-01", [(lat + shift, lon), (lat + shift, lon + step)]),
                self.stretch("c-01", [(lat - 0.01, lon + step / 2), (lat + 0.01, lon + step / 2)]),
            ]
        )
        [(first, second, length)] = W.courses_built_twice(network)
        self.assertEqual((first, second), ("a-01", "b-01"))
        self.assertGreaterEqual(length, 400.0)
        self.assertIn("a-01 and b-01", "\n".join(W._twice_report(network)))
        self.assertIn("1 pairs", "\n".join(W._twice_report(network)))

    @staticmethod
    def straight(
        lat: float, length_m: float, points: int, along_m: float = 0.0
    ) -> list[tuple[float, float]]:
        """A line along a parallel of latitude: `points` collinear points over `length_m` metres (the report's own flat scale), starting
        `along_m` metres east of longitude -8."""
        scale = 111_319.49 * math.cos(math.radians(lat))
        return [
            (lat, -8.0 + (along_m + length_m * i / (points - 1)) / scale) for i in range(points)
        ]

    def test_two_sparse_courses_24_8_metres_apart_are_found_as_the_dense_ones_are(self) -> None:
        """Review 2: nearness is to the other course's line segments, not between sampled points, so two straight courses given by their
        endpoints are listed just as the same courses with 180 collinear vertices are."""
        shift = 24.81 / 111_319.49
        found = []
        for points in (2, 181):
            network = W.Network(
                stretches=[
                    self.stretch("a-01", self.straight(53.4, 420.5, points)),
                    self.stretch("b-01", self.straight(53.4 + shift, 420.5, points, along_m=4.9)),
                ]
            )
            [(first, second, length)] = W.courses_built_twice(network)
            self.assertEqual((first, second), ("a-01", "b-01"), points)
            found.append(length)
        self.assertAlmostEqual(found[0], found[1], delta=15.0)
        self.assertAlmostEqual(
            found[0], 415.6, delta=15.0
        )  # the courses share 415.6 m of their length
        far = 26.0 / 111_319.49  # beyond 25 m: not listed, sparse or dense
        for points in (2, 181):
            network = W.Network(
                stretches=[
                    self.stretch("a-01", self.straight(53.4, 420.5, points)),
                    self.stretch("b-01", self.straight(53.4 + far, 420.5, points, along_m=4.9)),
                ]
            )
            self.assertEqual(W.courses_built_twice(network), [], points)

    @staticmethod
    def north_south(lat: float, lon: float, points: int) -> list[tuple[float, float]]:
        """A line due north over 0.005 degrees of latitude (556.6 m), `points` collinear points."""
        return [(lat + 0.005 * i / (points - 1), lon) for i in range(points)]

    def test_distance_is_local_ground_distance_in_the_north_and_the_south(self) -> None:
        """Review 3: at 55.5 N lines 23.96 m apart are within 25 m (a fixed 53.4 N scale said 25.22); at 51.5 N lines 25.64 m apart are not
        (it said 24.56)."""
        for lat, apart, listed in ((55.5, 23.96, True), (51.5, 25.64, False)):
            shift = apart / (111_319.49 * math.cos(math.radians(lat + 0.0025)))
            for points in (2, 40):
                network = W.Network(
                    stretches=[
                        self.stretch("a-01", self.north_south(lat, -8.0, points)),
                        self.stretch("b-01", self.north_south(lat, -8.0 + shift, points)),
                    ]
                )
                found = W.courses_built_twice(network)
                self.assertEqual(bool(found), listed, (lat, points))
                if listed:
                    self.assertAlmostEqual(found[0][2], 556.6, delta=2.0)

    def test_a_pair_near_the_300_metre_threshold_has_the_same_outcome_sparse_or_dense(self) -> None:
        """Review 3: parallel courses 24.49 m apart; the band round each reaches 5.03 m past the other's end, so a pair that overlaps by
        300 m is credited 305.03 m and one that overlaps by 288 m is credited 293.03 m, whatever the vertices."""
        shift = 24.49 / 111_319.49
        for along, listed in ((100.0, True), (112.0, False)):
            for points in (2, 181):
                network = W.Network(
                    stretches=[
                        self.stretch("a-01", self.straight(53.4, 400.0, points)),
                        self.stretch(
                            "b-01", self.straight(53.4 + shift, 400.0, points, along_m=along)
                        ),
                    ]
                )
                found = W.courses_built_twice(network)
                self.assertEqual(bool(found), listed, (along, points))
                if listed:
                    self.assertAlmostEqual(found[0][2], 305.03, delta=0.5)

    @staticmethod
    def plane(points: list[tuple[float, float]], lat: float = 53.4) -> list[tuple[float, float]]:
        """Points given in metres east and north of (lat, -8.0), as latitude and longitude."""
        scale = 111_319.49 * math.cos(math.radians(lat))
        return [(lat + y / 111_319.49, -8.0 + x / scale) for x, y in points]

    @staticmethod
    def with_midpoints(points: list[tuple[float, float]], times: int) -> list[tuple[float, float]]:
        """The same line with collinear vertices inserted into every segment, `times` rounds."""
        for _ in range(times):
            points = [
                q for a, b in pairwise(points) for q in (a, ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2))
            ] + [points[-1]]
        return points

    def listed(self, first: list[tuple[float, float]], second: list[tuple[float, float]]) -> float:
        """What the report credits a pair of courses, 0 where it lists none."""
        network = W.Network(stretches=[self.stretch("a-01", first), self.stretch("b-01", second)])
        found = W.courses_built_twice(network)
        return found[0][2] if found else 0.0

    def test_curved_courses_are_credited_alike_however_many_vertices_describe_them(self) -> None:
        """Two concentric arcs 20 m apart over about 400 m: the length is the same with every segment split into collinear pieces."""

        def arc(radius: float, steps: int) -> list[tuple[float, float]]:
            span = 400.0 / 500.0  # radians, so the inner arc is 400 m
            return self.plane(
                [
                    (radius * math.sin(span * i / steps), radius * math.cos(span * i / steps))
                    for i in range(steps + 1)
                ]
            )

        inner, outer = arc(500.0, 24), arc(520.0, 24)
        base = self.listed(inner, outer)
        self.assertGreater(base, 380.0)
        for times in (1, 3):
            self.assertAlmostEqual(
                self.listed(self.with_midpoints(inner, times), self.with_midpoints(outer, times)),
                base,
                delta=0.05,
            )

    def test_courses_that_cross_are_credited_only_for_the_length_near_the_crossing(self) -> None:
        """Crossing at 5 degrees the two lines stay within 25 m of each other for 2 x 25 / sin 5 = 573.7 m each; at 10 degrees for 288 m."""
        for degrees, expected in ((5.0, 573.7), (10.0, 0.0), (90.0, 0.0)):
            angle = math.radians(degrees)
            first = self.plane([(-1500.0, 0.0), (1500.0, 0.0)])
            second = self.plane(
                [
                    (-1500.0 * math.cos(angle), -1500.0 * math.sin(angle)),
                    (1500.0 * math.cos(angle), 1500.0 * math.sin(angle)),
                ]
            )
            self.assertAlmostEqual(self.listed(first, second), expected, delta=1.0, msg=degrees)
            self.assertAlmostEqual(
                self.listed(self.with_midpoints(first, 3), second), expected, delta=1.0, msg=degrees
            )

    def test_one_course_beside_another_for_the_whole_of_its_length_is_credited_what_the_shorter_runs(
        self,
    ) -> None:
        """A 400 m course 15 m from the middle of a 1,000 m one: credited 400 m (what the long one runs beside it is 400 m and the two band ends,
        which is more); a 200 m one is not listed; the same course twice is listed at its full length."""
        long = self.plane([(0.0, 0.0), (1000.0, 0.0)])
        inside = self.plane([(300.0, 15.0), (700.0, 15.0)])
        self.assertAlmostEqual(self.listed(long, inside), 400.0, delta=0.5)
        self.assertAlmostEqual(self.listed(inside, long), 400.0, delta=0.5)
        small = self.plane([(400.0, 15.0), (600.0, 15.0)])
        self.assertEqual(self.listed(long, small), 0.0)
        self.assertAlmostEqual(self.listed(long, long), 1000.0, delta=0.5)

    def test_a_course_that_bends_away_is_credited_only_while_it_is_within_25_metres(self) -> None:
        """A course 10 m from another for 400 m that then turns away at 45 degrees: its own length within 25 m is the 400 m and the 15 m of rise
        it takes to leave the band, 15 x sqrt 2 along the turn; the other course's is longer (to x = 390 + 25 sqrt 2), so the lesser is credited."""
        straight = self.plane([(0.0, 0.0), (2000.0, 0.0)])
        bent = self.plane([(0.0, 10.0), (400.0, 10.0), (1000.0, 610.0)])
        self.assertAlmostEqual(
            self.listed(straight, bent), 400.0 + 15.0 * math.sqrt(2.0), delta=0.5
        )

    def test_redundant_vertices_do_not_change_what_a_pair_is_credited(self) -> None:
        """Review 1, finding 3: the total is ground distance, whether a course has two points or forty."""
        shift = 8.9 / 111_319.49
        for points in (2, 40):
            short = W.Network(
                stretches=[
                    self.stretch("a-01", self.straight(53.4, 95.82, points)),
                    self.stretch("b-01", self.straight(53.4 + shift, 95.82, points)),
                ]
            )
            self.assertEqual(W.courses_built_twice(short), [], points)
        lengths = []
        for points in (2, 40):
            long = W.Network(
                stretches=[
                    self.stretch("a-01", self.straight(53.4, 400.0, points)),
                    self.stretch("b-01", self.straight(53.4 + shift, 400.0, points)),
                ]
            )
            [(_, _, length)] = W.courses_built_twice(long)
            lengths.append(length)
        self.assertAlmostEqual(lengths[0], lengths[1], delta=15.0)
        self.assertAlmostEqual(lengths[0], 400.0, delta=15.0)


if __name__ == "__main__":
    unittest.main()
