"""Contract tests for the travel network tool."""

import copy
import importlib.util
import json
import tempfile
import tomllib
import unittest
from collections import defaultdict
from collections.abc import Sequence
from fractions import Fraction
from itertools import pairwise
from pathlib import Path
from typing import Any, ClassVar
from unittest import mock

_PATH = Path(__file__).resolve().parents[1] / "assets/map/ireland/network.py"
_SPEC = importlib.util.spec_from_file_location("network", _PATH)
assert _SPEC is not None and _SPEC.loader is not None
network = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(network)


class MovementTests(unittest.TestCase):
    def test_two_cell_step_preserves_course_and_checks_both_middle_cells(self) -> None:
        land = [[True, True, False], [False, True, True]]
        course = [(0, 0), (2, 1)]
        self.assertTrue(network.legal_course(land, course))
        self.assertEqual(course, [(0, 0), (2, 1)])
        land[0][1] = False
        self.assertFalse(network.legal_course(land, course))

    def test_diagonal_cannot_cut_between_water_cells(self) -> None:
        land = [[True, False], [False, True]]
        self.assertFalse(network.legal_course(land, [(0, 0), (1, 1)]))
        self.assertFalse(network.legal_course(land, [(0, 0), (2, 0)]))


class RoutingTests(unittest.TestCase):
    def test_routes_through_waypoint_and_avoids_expensive_ground(self) -> None:
        land = [[True] * 7 for _ in range(5)]
        land[2][3] = False
        costs = [[1] * 7 for _ in range(5)]
        costs[1] = [20] * 7
        result = network.route(land, costs, [(0, 2), (3, 4), (6, 2)])
        self.assertIn((3, 4), result)
        self.assertNotIn((3, 2), result)
        self.assertGreaterEqual(min(y for _, y in result), 2)
        self.assertTrue(network.legal_course(land, result))
        self.assertEqual(result, network.route(land, costs, [(0, 2), (3, 4), (6, 2)]))

    def test_unreachable_waypoint_is_refused(self) -> None:
        land = [[True, False, True]]
        with self.assertRaisesRegex(ValueError, "no course"):
            network.route(land, [[1, 1, 1]], [(0, 0), (2, 0)])

    def test_earlier_ground_discount_can_change_route(self) -> None:
        land = [[True] * 7 for _ in range(3)]
        costs = [[100] * 7 for _ in range(3)]
        earlier = {(x, 0) for x in range(7)} | {(0, 1), (6, 1)}
        direct = network.route(land, costs, [(0, 1), (6, 1)])
        reused = network.route(land, costs, [(0, 1), (6, 1)], earlier=earlier, reuse=100_000)
        self.assertTrue(all(y == 1 for _, y in direct))
        self.assertTrue(any(y == 0 for _, y in reused))


class CompletionTests(unittest.TestCase):
    def test_first_legal_parent_endpoint_is_chosen_without_changing_parent(self) -> None:
        land = [[True] * 4 for _ in range(2)]
        parent = [(3, 0), (2, 1)]
        child = [(1, 0), (2, 0)]
        self.assertEqual(network.complete_join(land, parent, child), [(1, 0), (2, 0), (3, 0)])
        self.assertEqual(parent, [(3, 0), (2, 1)])

    def test_singleton_is_relocated_to_parent(self) -> None:
        land = [[True] * 4 for _ in range(2)]
        self.assertEqual(network.complete_join(land, [(3, 0), (2, 1)], [(2, 0)]), [(3, 0)])

    def test_refuses_when_neither_completion_is_legal(self) -> None:
        land = [[True, True, True, False], [True, False, False, True]]
        with self.assertRaisesRegex(ValueError, "no legal completion"):
            network.complete_join(land, [(3, 0), (2, 1)], [(2, 0)])

    def test_only_legal_endpoint_is_selected(self) -> None:
        land = [[True, True, True, False], [True, True, True, True]]
        self.assertEqual(
            network.complete_join(land, [(3, 0), (2, 1)], [(1, 0), (2, 0)]),
            [(1, 0), (2, 0), (2, 1)],
        )


class GroundMarkTests(unittest.TestCase):
    def test_diagonal_leaning_cell_changes_mark_but_unrelated_cell_does_not(self) -> None:
        terrain = [[1] * 4 for _ in range(4)]
        course = [(0, 0), (1, 1)]
        original = network.ground_mark(course, terrain)
        terrain[3][3] = 2
        self.assertEqual(original, network.ground_mark(course, terrain))
        terrain[0][1] = 2
        self.assertNotEqual(original, network.ground_mark(course, terrain))


class CostTests(unittest.TestCase):
    def test_slope_and_river_costs_can_change_the_course(self) -> None:
        land = [[True] * 5 for _ in range(3)]
        costs = [[100] * 5 for _ in range(3)]
        elevations = [[0] * 5 for _ in range(3)]
        elevations[1][2] = 10000
        steep = network.route(land, costs, [(0, 1), (4, 1)], elevation=elevations, slope=10_000_000)
        self.assertNotIn((2, 1), steep)
        crossings = {((1, 1), (2, 1)): 10000000000}
        diverted = network.route(land, costs, [(0, 1), (4, 1)], crossings=crossings)
        self.assertNotIn(((1, 1), (2, 1)), list(pairwise(diverted)))


class TerrainTests(unittest.TestCase):
    def test_land_uses_terrain_kind_including_offshore_place_cells(self) -> None:
        rules = [
            {"key": "000001", "kind": "land"},
            {"key": "000002", "kind": "lake"},
            {"key": "000003", "kind": "sea"},
        ]
        self.assertEqual(
            network.terrain_land([[(0, 0, 1), (0, 0, 2), (0, 0, 3)]], rules), [[True, False, False]]
        )
        with self.assertRaisesRegex(ValueError, "unknown terrain"):
            network.terrain_land([[(0, 0, 4)]], rules)

    def test_fertility_reduces_ground_cost_without_float_ties(self) -> None:
        costs = network.terrain_costs([["pasture", "pasture"]], [[0, 250]], {"pasture": "1"}, "0.5")
        self.assertEqual(costs, [[1250000, 750000]])

    def test_ground_mark_changes_when_a_river_crossing_changes(self) -> None:
        course = [(0, 0), (1, 0)]
        terrain = [[1, 1]]
        first = network.ground_mark(course, terrain, crossings=(("river", 20),))
        second = network.ground_mark(course, terrain, crossings=(("river", 21),))
        self.assertNotEqual(first, second)


class TopologyTests(unittest.TestCase):
    def test_shared_course_is_one_stretch_even_when_traversed_backwards(self) -> None:
        courses = {"a": [(0, 0), (1, 0), (2, 0), (3, 0)], "b": [(2, 0), (1, 0), (0, 0)]}
        stretches, links = network.split_courses(courses, set())
        self.assertEqual(len(stretches), 2)
        self.assertEqual(links["a"][0][0], links["b"][0][0])
        self.assertNotEqual(links["a"][0][1], links["b"][0][1])

    def test_loop_cuts_at_first_and_second_occurrence(self) -> None:
        courses = {"a": [(0, 0), (1, 0), (1, 1), (0, 1), (1, 0), (2, 0)]}
        stretches, links = network.split_courses(courses, {(1, 0)})
        lines = [stretches[index] for index, _ in links["a"]]
        self.assertEqual(len(lines), 3)
        self.assertEqual(lines[1][0], lines[1][-1])

    def test_ids_survive_and_removed_highest_id_is_not_reused(self) -> None:
        old = {((0, 0), (1, 0)): 3, ((1, 0), (2, 0)): 8}
        kept, next_id = network.allocate_ids([((0, 0), (1, 0))], old, 9)
        self.assertEqual(kept, {((0, 0), (1, 0)): 3})
        assigned, next_id = network.allocate_ids(
            [((0, 0), (1, 0)), ((1, 0), (3, 0))], kept, next_id
        )
        self.assertEqual(assigned[((1, 0), (3, 0))], 9)
        self.assertEqual(next_id, 10)


class CoordinateTests(unittest.TestCase):
    def test_cell_centres_give_ground_length_and_tidal_zero(self) -> None:
        origin = (20000, 15000)
        self.assertEqual(network.ground_length([(0, 0)], origin), 0)
        length = network.ground_length([(0, 0), (1, 0)], origin)
        self.assertGreater(length, 450)
        self.assertLess(length, 600)
        self.assertEqual(length, network.ground_length([(1, 0), (0, 0)], origin))


class InputTests(unittest.TestCase):
    def test_classification_uses_terrain_name_and_rejects_unknown_pixels(self) -> None:
        rules = [
            {"key": "010203", "kind": "land", "name": "pasture"},
            {"key": "040506", "kind": "lake", "name": "lake"},
        ]
        self.assertEqual(
            network.terrain_names([[(1, 2, 3), (4, 5, 6)]], rules), [["pasture", "lake"]]
        )
        with self.assertRaisesRegex(ValueError, "unknown terrain"):
            network.terrain_names([[(8, 8, 8)]], rules)

    def test_ireland_terrain_places_inishmacsaint_on_land(self) -> None:
        map_dir = Path(__file__).resolve().parents[1] / "assets/map/ireland"
        layers = network.read_rasters(map_dir)
        self.assertEqual(layers["terrain"][258][418], "pasture")
        self.assertTrue(layers["land"][258][418])


class RiverTests(unittest.TestCase):
    def test_crossing_uses_position_on_river_for_width(self) -> None:
        river = [(0, 0), (0, 20), (0, 100)]
        crossings = network.river_crossings([(-1, 50), (1, 50)], river, (10, 30))
        self.assertEqual(len(crossings), 1)
        self.assertEqual(crossings[0][1], 20)

    def test_parallel_segments_have_no_crossing(self) -> None:
        self.assertEqual(network.river_crossings([(0, 0), (2, 0)], [(0, 1), (2, 1)], (20, 30)), [])


class FreshnessTests(unittest.TestCase):
    def test_change_to_any_input_or_creation_of_absent_network_invalidates_preview(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            hand, raster, stored = root / "network.toml", root / "terrain.png", root / "roads.toml"
            hand.write_bytes(b"hand")
            raster.write_bytes(b"map")
            files = {"hand": hand, "raster": raster, "stored": stored}
            captured = network.snapshot(files)
            self.assertTrue(network.snapshot_matches(files, captured))
            raster.write_bytes(b"new map")
            self.assertFalse(network.snapshot_matches(files, captured))
            raster.write_bytes(b"map")
            stored.write_bytes(b"network")
            self.assertFalse(network.snapshot_matches(files, captured))


class StartingStateTests(unittest.TestCase):
    def test_shared_state_prefers_road_then_higher_level(self) -> None:
        self.assertEqual(
            network.starting_state([("path", 5), ("road", 1), ("road", 3)]), ("road", 3)
        )
        self.assertEqual(network.starting_state([("path", 1), ("path", 5)]), ("path", 5))


class HandListTests(unittest.TestCase):
    def test_refuses_unknown_key_bad_level_and_late_parent(self) -> None:
        land = [[True] * 4 for _ in range(2)]
        locations = {"a": (0, 0), "b": (3, 0)}
        first = {"id": "a_to_b", "from": "a", "to": "b", "category": "road", "level": 1}
        second = {
            "id": "child",
            "from": "a",
            "to": {"parent": "a_to_b", "cell": [1, 0]},
            "category": "path",
            "level": 1,
        }
        network.validate_hand_list({"link": [first, second]}, locations, land)
        with self.assertRaisesRegex(ValueError, "unknown"):
            network.validate_hand_list({"link": [{**first, "typo": 1}]}, locations, land)
        with self.assertRaisesRegex(ValueError, "level"):
            network.validate_hand_list({"link": [{**first, "level": 6}]}, locations, land)
        with self.assertRaisesRegex(ValueError, "parent"):
            network.validate_hand_list({"link": [second, first]}, locations, land)

    def test_same_node_with_via_is_refused_and_without_is_allowed(self) -> None:
        land = [[True] * 3]
        locations = {"a": (0, 0), "b": (2, 0)}
        parent = {"id": "parent", "from": "a", "to": "b", "category": "road", "level": 1}
        child = {
            "id": "child",
            "from": "a",
            "to": {"parent": "parent", "cell": [0, 0]},
            "category": "path",
            "level": 1,
        }
        network.validate_hand_list({"link": [parent, child]}, locations, land)
        with self.assertRaisesRegex(ValueError, "link child: same node"):
            network.validate_hand_list({"link": [parent, {**child, "via": ["b"]}]}, locations, land)

    def test_landing_has_exactly_one_access_and_must_be_land(self) -> None:
        land = [[True, False]]
        locations = {"isle": (0, 0)}
        access = [{"place": "isle", "quality": "offshore"}]
        with self.assertRaisesRegex(ValueError, "missing landing"):
            network.validate_hand_list({"access": access}, locations, land)
        with self.assertRaisesRegex(ValueError, "on water"):
            network.validate_hand_list(
                {"access": access, "landing": [{"place": "isle", "cell": [1, 0]}]}, locations, land
            )

    def test_white_island_completion_crosses_drumragh_at_recorded_width(self) -> None:
        import tomllib  # noqa: PLC0415  # Only this real-map witness needs TOML parsing.

        root = Path(__file__).resolve().parents[1]
        map_dir = root / "assets/map/ireland"
        crop = tomllib.loads((map_dir / "map.toml").read_text())["crop"]
        rivers = tomllib.loads((map_dir / "rivers.toml").read_text())["stretch"]
        drumragh = next(stretch for stretch in rivers if stretch["id"] == "drumragh-01")
        origin = tuple(crop["origin"])
        points = [network.project_point(lat, lon, origin) for lat, lon in drumragh["points"]]
        course = [(479_500_000, 223_500_000), (480_500_000, 224_500_000)]
        widths = tuple(Fraction(str(value)) for value in drumragh["width_m"])
        crossings = network.river_crossings(course, points, widths)
        self.assertEqual(len(crossings), 1)
        self.assertAlmostEqual(float(crossings[0][1]), 22.6, places=1)

    def test_pin_parent_and_landing_bounds_and_duplicate_access_are_named(self) -> None:
        import math  # noqa: PLC0415  # Project a small validation fixture.

        radius = 6_378_137.0
        half = math.pi * radius

        def point(x: int, y: int) -> tuple[float, float]:
            lon = math.degrees(((22436 + x + 0.5) * 839 - half) / radius)
            lat = math.degrees(
                2 * math.atan(math.exp((half - (14979 + y + 0.5) * 839) / radius)) - math.pi / 2
            )
            return lat, lon

        land = [[True, False, True], [True, True, True]]
        locations = {"a": (0, 0), "b": (2, 0)}
        road = {"id": "road", "from": "a", "to": "b", "category": "road", "level": 1}
        lat, lon = point(1, 0)
        with self.assertRaisesRegex(ValueError, "link road pin.*water"):
            network.validate_hand_list(
                {"link": [{**road, "via": [{"lat": lat, "lon": lon, "note": "pin"}]}]},
                locations,
                land,
                (22436, 14979),
            )
        with self.assertRaisesRegex(ValueError, "link road pin.*outside"):
            network.validate_hand_list(
                {"link": [{**road, "via": [{"lat": 0, "lon": 0, "note": "pin"}]}]},
                locations,
                land,
                (22436, 14979),
            )
        child = {
            "id": "child",
            "from": "a",
            "to": {"parent": "road", "cell": [1, 0]},
            "category": "path",
            "level": 1,
        }
        with self.assertRaisesRegex(ValueError, "link child parent.*water"):
            network.validate_hand_list({"link": [road, child]}, locations, land)
        access = [{"place": "a", "quality": "offshore"}]
        landing = {"place": "a", "cell": [0, 0]}
        with self.assertRaisesRegex(ValueError, "landing a: repeated"):
            network.validate_hand_list(
                {"access": access, "landing": [landing, landing]}, locations, land
            )
        with self.assertRaisesRegex(ValueError, "access a: repeated"):
            network.validate_hand_list({"access": access * 2}, locations, land)


class ProposalTests(unittest.TestCase):
    def test_moved_parent_blocks_child_and_propagates_to_descendant(self) -> None:
        land = [[True] * 5 for _ in range(3)]
        costs = [[1] * 5 for _ in range(3)]
        links = [
            {"id": "parent", "points": [(0, 0), (2, 2), (4, 0)]},
            {"id": "child", "points": [(0, 1), (2, 0)], "joins": [("parent", (2, 0))]},
            {"id": "grandchild", "points": [(1, 1), (0, 1)], "joins": [("child", (0, 1))]},
        ]
        baseline = {
            "parent": {"course": [(0, 0), (2, 0), (4, 0)], "points": [(0, 0), (4, 0)]},
            "child": {"course": [(0, 1), (2, 0)], "points": [(0, 1), (2, 0)]},
            "grandchild": {"course": [(1, 1), (0, 1)], "points": [(1, 1), (0, 1)]},
        }
        result = network.propose_courses(links, baseline, land, costs)
        self.assertIn("parent", result["candidates"])
        self.assertIn("child", result["blocked"])
        self.assertIn("grandchild", result["blocked"])
        self.assertIn("parent", result["blocked"]["child"])
        self.assertIn("child", result["blocked"]["grandchild"])

    def test_changed_pin_is_proposed_again_until_baseline_changes(self) -> None:
        land = [[True] * 4 for _ in range(3)]
        costs = [[1] * 4 for _ in range(3)]
        links = [{"id": "road", "points": [(0, 0), (2, 2), (3, 0)]}]
        baseline = {
            "road": {"course": [(0, 0), (1, 0), (2, 0), (3, 0)], "points": [(0, 0), (3, 0)]}
        }
        first = network.propose_courses(links, baseline, land, costs)
        again = network.propose_courses(links, baseline, land, costs)
        self.assertEqual(first, again)
        self.assertIn((2, 2), first["candidates"]["road"])
        self.assertEqual(baseline["road"]["points"], [(0, 0), (3, 0)])

    def test_flooded_support_cell_causes_reroute(self) -> None:
        land = [[True, False], [True, True]]
        costs = [[1, 1], [1, 1]]
        links = [{"id": "road", "points": [(0, 0), (1, 1)]}]
        baseline = {"road": {"course": [(0, 0), (1, 1)], "points": [(0, 0), (1, 1)]}}
        result = network.propose_courses(links, baseline, land, costs)
        self.assertEqual(result["candidates"]["road"], [(0, 0), (0, 1), (1, 1)])

    def test_new_blocked_child_and_descendant_have_no_candidate(self) -> None:
        land = [[True] * 5 for _ in range(3)]
        links = [
            {"id": "parent", "points": [(0, 0), (4, 0)], "joins": []},
            {"id": "new", "points": [(2, 2), (2, 1)], "joins": [("parent", (2, 1))]},
            {"id": "later", "points": [(1, 2), (2, 2)], "joins": [("new", (2, 2))]},
        ]
        baseline = {
            "parent": {
                "course": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)],
                "points": [(0, 0), (4, 0)],
            }
        }
        proposal = network.propose_courses(links, baseline, land, [[1] * 5 for _ in range(3)])
        self.assertFalse(proposal["complete"])
        self.assertIn("nearest", proposal["blocked"]["new"])
        self.assertIn("waiting for blocked parent new", proposal["blocked"]["later"])
        self.assertNotIn("new", proposal["candidates"])
        self.assertNotIn("later", proposal["candidates"])

    def test_only_links_with_changed_resolved_places_are_proposed(self) -> None:
        land = [[True] * 5 for _ in range(3)]
        costs = [[1] * 5 for _ in range(3)]
        links = [
            {"id": "end", "points": [(0, 0), (4, 1)], "joins": []},
            {"id": "through", "points": [(0, 2), (4, 1), (4, 2)], "joins": []},
            {"id": "untouched", "points": [(0, 2), (4, 2)], "joins": []},
        ]
        baseline = {
            "end": {"course": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)], "points": [(0, 0), (4, 0)]},
            "through": {
                "course": [(0, 2), (2, 1), (4, 0), (4, 1), (4, 2)],
                "points": [(0, 2), (4, 0), (4, 2)],
            },
            "untouched": {
                "course": [(0, 2), (1, 2), (2, 2), (3, 2), (4, 2)],
                "points": [(0, 2), (4, 2)],
            },
        }
        plan = network.propose_courses(links, baseline, land, costs)
        self.assertEqual(set(plan["candidates"]), {"end", "through"})


class StorageTests(unittest.TestCase):
    def test_toml_is_sorted_and_preserves_direction_and_tidal_connector(self) -> None:
        import tomllib  # noqa: PLC0415  # Only the storage witness parses TOML.

        graph: dict[str, Any] = {
            "format": 1,
            "hand_list_sha256": "a" * 64,
            "baseline_sha256": "b" * 64,
            "next_node_id": 3,
            "next_stretch_id": 2,
            "node": [
                {"id": 2, "kind": "place", "cell": [0, 0], "place": "isle"},
                {"id": 1, "kind": "landing", "cell": [0, 0], "place": "isle"},
            ],
            "stretch": [
                {
                    "id": 1,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0]],
                    "length_m": 0,
                    "category": "path",
                    "level": 1,
                    "tidal": True,
                }
            ],
            "link": [
                {"id": "road", "from_node": 1, "to_node": 1, "stretches": [], "record": "seed"}
            ],
            "join": [],
            "road": [],
            "access": [{"place": "isle", "quality": "offshore_at_high_tide", "landing": 1}],
        }
        text = network.dump_roads(graph)
        parsed = tomllib.loads(text)
        self.assertEqual(parsed["node"][0]["id"], 1)
        self.assertEqual(parsed["stretch"][0]["cells"], [[0, 0]])
        self.assertTrue(parsed["stretch"][0]["tidal"])
        self.assertEqual(parsed["link"][0]["stretches"], [])
        graph["node"].reverse()
        self.assertEqual(network.dump_roads(graph), text)

    def test_build_graph_joins_parent_and_keeps_offshore_place_separate(self) -> None:
        places = {"a": (0, 0), "b": (3, 0), "c": (1, 1), "isle": (2, 0)}
        access = {"isle": ("offshore", (2, 0))}
        links = [
            {
                "id": "parent",
                "from": "a",
                "to": "b",
                "category": "road",
                "level": 1,
                "record": "seed",
            },
            {
                "id": "child",
                "from": "c",
                "to": {"parent": "parent", "cell": [1, 0]},
                "category": "path",
                "level": 1,
                "record": "seed",
            },
        ]
        courses = {"parent": [(0, 0), (1, 0), (2, 0), (3, 0)], "child": [(1, 1), (1, 0)]}
        graph = network.make_graph(
            places, access, links, courses, [], (20000, 15000), "a" * 64, "b" * 64
        )
        isle = next(
            node
            for node in graph["node"]
            if node.get("place") == "isle" and node["kind"] == "place"
        )
        landing = next(
            node
            for node in graph["node"]
            if node.get("place") == "isle" and node["kind"] == "landing"
        )
        self.assertNotEqual(isle["id"], landing["id"])
        self.assertFalse(
            any(isle["id"] in (stretch["from"], stretch["to"]) for stretch in graph["stretch"])
        )
        self.assertEqual(graph["join"][0]["link"], "child")
        self.assertEqual(graph["join"][0]["parent"], "parent")
        self.assertEqual(
            network.dump_roads(graph),
            network.dump_roads(
                network.make_graph(
                    places,
                    access,
                    links,
                    courses,
                    [],
                    (20000, 15000),
                    "a" * 64,
                    "b" * 64,
                    previous=graph,
                )
            ),
        )

    def test_tidal_connector_can_be_one_cell_and_has_stable_id(self) -> None:
        places = {"a": (0, 0), "b": (2, 0), "tide": (1, 0)}
        access = {"tide": ("offshore_at_high_tide", (1, 0))}
        links = [
            {"id": "road", "from": "a", "to": "b", "category": "road", "level": 1, "record": "seed"}
        ]
        courses = {"road": [(0, 0), (1, 0), (2, 0)]}
        graph = network.make_graph(
            places, access, links, courses, [], (20000, 15000), "a" * 64, "b" * 64
        )
        tidal = next(stretch for stretch in graph["stretch"] if stretch["tidal"])
        self.assertEqual(tidal["cells"], [[1, 0]])
        self.assertEqual(tidal["length_m"], 0)
        again = network.make_graph(
            places, access, links, courses, [], (20000, 15000), "a" * 64, "b" * 64, previous=graph
        )
        self.assertEqual(network.dump_roads(graph), network.dump_roads(again))

    def test_graph_refuses_course_that_does_not_reach_declared_end(self) -> None:
        places = {"a": (0, 0), "b": (2, 0)}
        links = [
            {"id": "road", "from": "a", "to": "b", "category": "road", "level": 1, "record": "seed"}
        ]
        with self.assertRaisesRegex(ValueError, "road.*endpoint"):
            network.make_graph(
                places,
                {},
                links,
                {"road": [(0, 0), (1, 0)]},
                [],
                (20000, 15000),
                "a" * 64,
                "b" * 64,
            )

    def test_join_distance_uses_first_visit_on_looping_parent(self) -> None:
        places = {"a": (0, 0), "b": (3, 0), "c": (1, 2)}
        links = [
            {
                "id": "parent",
                "from": "a",
                "to": "b",
                "category": "road",
                "level": 1,
                "record": "seed",
            },
            {
                "id": "child",
                "from": "c",
                "to": {"parent": "parent", "cell": [1, 0]},
                "category": "path",
                "level": 1,
                "record": "seed",
            },
        ]
        courses = {
            "parent": [(0, 0), (1, 0), (1, 1), (0, 1), (1, 0), (2, 0), (3, 0)],
            "child": [(1, 2), (1, 1), (1, 0)],
        }
        graph = network.make_graph(
            places, {}, links, courses, [], (20000, 15000), "a" * 64, "b" * 64
        )
        self.assertEqual(
            graph["join"][0]["along_m"], network.ground_length([(0, 0), (1, 0)], (20000, 15000))
        )

    def test_replacing_junction_identity_does_not_reuse_changed_stretch_ids(self) -> None:
        places = {"a": (0, 0), "b": (2, 0), "c": (1, 1)}
        links = [
            {
                "id": "parent",
                "from": "a",
                "to": "b",
                "category": "road",
                "level": 1,
                "record": "seed",
            },
            {
                "id": "child",
                "from": "c",
                "to": {"parent": "parent", "cell": [1, 0]},
                "category": "path",
                "level": 1,
                "record": "seed",
            },
        ]
        courses = {"parent": [(0, 0), (1, 0), (2, 0)], "child": [(1, 1), (1, 0)]}
        old = network.make_graph(places, {}, links, courses, [], (20000, 15000), "a" * 64, "b" * 64)
        new = network.make_graph(
            {**places, "d": (1, 0)},
            {},
            links,
            courses,
            [],
            (20000, 15000),
            "a" * 64,
            "b" * 64,
            previous=old,
        )
        old_ids = {stretch["id"] for stretch in old["stretch"]}
        new_ids = {stretch["id"] for stretch in new["stretch"]}
        self.assertTrue(old_ids.isdisjoint(new_ids))

    def test_same_node_link_cannot_hide_a_nonempty_loop(self) -> None:
        places = {"a": (0, 0)}
        links = [
            {"id": "loop", "from": "a", "to": "a", "category": "path", "level": 1, "record": "seed"}
        ]
        with self.assertRaisesRegex(ValueError, "same node"):
            network.make_graph(
                places,
                {},
                links,
                {"loop": [(0, 0), (1, 0), (0, 0)]},
                [],
                (20000, 15000),
                "a" * 64,
                "b" * 64,
            )

    def test_tidal_connector_can_span_water_without_land_link(self) -> None:
        places = {"a": (0, 1), "tide": (2, 0)}
        access = {"tide": ("offshore_at_high_tide", (0, 0))}
        links = [
            {
                "id": "approach",
                "from": "a",
                "to": "tide",
                "category": "path",
                "level": 1,
                "record": "seed",
            }
        ]
        graph = network.make_graph(
            places,
            access,
            links,
            {"approach": [(0, 1), (0, 0)]},
            [],
            (20000, 15000),
            "a" * 64,
            "b" * 64,
        )
        tidal = next(stretch for stretch in graph["stretch"] if stretch["tidal"])
        self.assertEqual(tidal["cells"], [[0, 0], [2, 0]])
        self.assertEqual(tidal["length_m"], network.ground_length([(0, 0), (2, 0)], (20000, 15000)))
        self.assertGreater(tidal["length_m"], 0)
        self.assertNotIn(tidal["id"], [part[0] for part in graph["link"][0]["stretches"]])

    def test_parent_join_at_offshore_place_uses_landing_and_keeps_through_traffic(self) -> None:
        places = {"a": (0, 0), "b": (4, 0), "isle": (2, 1), "child": (1, 1)}
        access = {"isle": ("offshore", (2, 0))}
        links = [
            {
                "id": "parent",
                "from": "a",
                "to": "b",
                "category": "road",
                "level": 1,
                "record": "seed",
            },
            {
                "id": "child",
                "from": "child",
                "to": {"parent": "parent", "cell": [2, 0]},
                "category": "path",
                "level": 1,
                "record": "seed",
            },
        ]
        courses = {"parent": [(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)], "child": [(1, 1), (2, 0)]}
        graph = network.make_graph(
            places, access, links, courses, [], (20000, 15000), "a" * 64, "b" * 64
        )
        landing = next(
            n for n in graph["node"] if n.get("place") == "isle" and n["kind"] == "landing"
        )
        offshore = next(
            n for n in graph["node"] if n.get("place") == "isle" and n["kind"] == "place"
        )
        self.assertEqual(graph["join"][0]["node"], landing["id"])
        self.assertFalse(any(offshore["id"] in (s["from"], s["to"]) for s in graph["stretch"]))
        self.assertEqual(sum(landing["id"] in (s["from"], s["to"]) for s in graph["stretch"]), 3)


class ResolutionTests(unittest.TestCase):
    def test_offshore_end_and_through_waypoint_resolve_to_landing(self) -> None:
        locations = {"a": (0, 0), "isle": (2, 1), "b": (4, 0)}
        data = {
            "access": [{"place": "isle", "quality": "offshore"}],
            "landing": [{"place": "isle", "cell": [2, 0]}],
            "link": [
                {
                    "id": "route",
                    "from": "a",
                    "to": "b",
                    "via": ["isle"],
                    "category": "road",
                    "level": 1,
                },
                {"id": "approach", "from": "isle", "to": "a", "category": "path", "level": 1},
            ],
        }
        resolved = network.resolve_links(data, locations, (0, 0))
        self.assertEqual(resolved[0]["points"], [(0, 0), (2, 0), (4, 0)])
        self.assertEqual(resolved[1]["points"], [(2, 0), (0, 0)])

    def test_ireland_locations_project_to_the_scenario_cells(self) -> None:
        root = Path(__file__).resolve().parents[1]
        origin = (22436, 14979)
        locations = network.read_locations(
            root / "assets/scenarios/ireland-768/locations.toml", origin
        )
        self.assertEqual(len(locations), 125)
        self.assertEqual(locations["inishmacsaint"], (418, 258))


class SeedTests(unittest.TestCase):
    def test_import_completes_child_without_changing_parent_and_keeps_evidence(self) -> None:
        land = [[True] * 4 for _ in range(2)]
        links = [
            {"id": "parent", "points": [(3, 0), (2, 1)], "joins": []},
            {"id": "child", "points": [(1, 0), (3, 0)], "joins": [("parent", (3, 0))]},
        ]
        seed = {"parent": [(3, 0), (2, 1)], "child": [(1, 0), (2, 0)]}
        baseline, completions = network.import_seed(links, seed, land)
        self.assertEqual(baseline["parent"]["course"], seed["parent"])
        self.assertEqual(baseline["child"]["course"], [(1, 0), (2, 0), (3, 0)])
        self.assertEqual(completions, [{"link": "child", "from": (2, 0), "added": (3, 0)}])
        self.assertEqual(seed["child"], [(1, 0), (2, 0)])

    def test_singleton_seed_moves_place_to_road_without_a_stretch(self) -> None:
        land = [[True] * 4 for _ in range(2)]
        links = [
            {"id": "parent", "points": [(3, 0), (2, 1)], "joins": []},
            {"id": "child", "points": [(2, 0), (3, 0)], "joins": [("parent", (3, 0))]},
        ]
        seed = {"parent": [(3, 0), (2, 1)], "child": [(2, 0)]}
        baseline, _ = network.import_seed(links, seed, land)
        self.assertEqual(baseline["child"]["course"], [(3, 0)])
        self.assertEqual(baseline["child"]["points"], [(3, 0), (3, 0)])

    def test_index_returns_same_drumragh_crossing_as_polyline(self) -> None:
        import tomllib  # noqa: PLC0415  # This witness loads the shipped river network.

        root = Path(__file__).resolve().parents[1]
        map_dir = root / "assets/map/ireland"
        crop = tomllib.loads((map_dir / "map.toml").read_text())["crop"]
        rivers = tomllib.loads((map_dir / "rivers.toml").read_text())["stretch"]
        drumragh = next(stretch for stretch in rivers if stretch["id"] == "drumragh-01")
        index = network.RiverIndex([drumragh], tuple(crop["origin"]))
        crossings = index.crossings((479, 223), (480, 224))
        self.assertEqual(len(crossings), 1)
        self.assertEqual(crossings[0][0], "drumragh")
        self.assertAlmostEqual(float(crossings[0][2]), 22.6, places=1)

    def test_route_uses_a_lazily_calculated_crossing_charge(self) -> None:
        land = [[True] * 5 for _ in range(3)]
        costs = [[1] * 5 for _ in range(3)]
        seen = []

        def charge(a: tuple[int, int], b: tuple[int, int]) -> int:
            seen.append((a, b))
            return 1_000_000_000 if {a, b} == {(1, 1), (2, 1)} else 0

        path = network.route(land, costs, [(0, 1), (4, 1)], crossing_fn=charge)
        self.assertTrue(seen)
        self.assertNotIn(((1, 1), (2, 1)), list(pairwise(path)))

    def test_crossing_charge_uses_width_and_mockup_weights(self) -> None:
        class Crossing:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("river", Fraction(1, 2), Fraction(20))]

        charge = network.crossing_cost(Crossing(), "2", "0.125")
        self.assertEqual(charge((0, 0), (1, 0)), 9_000_000_000_000)

    def test_seed_waypoints_remain_independent_of_later_hand_edits(self) -> None:
        resolved = [{"id": "road", "points": [(0, 0), (3, 0)], "joins": []}]
        hand_links = [{"id": "road"}]
        seed_links = {"road": {"pins": [[1, 1]]}}
        seed_document = {"approved_points": {"road": [[0, 0], [1, 1], [3, 0]]}}
        points = network.approved_seed_points(seed_document, hand_links, resolved, seed_links)
        baseline, _ = network.import_seed(
            resolved,
            {"road": [(0, 0), (1, 1), (2, 1), (3, 0)]},
            [[True] * 4 for _ in range(2)],
            approved_points=points,
        )
        plan = network.propose_courses(
            resolved, baseline, [[True] * 4 for _ in range(2)], [[1] * 4 for _ in range(2)]
        )
        self.assertIn("road", plan["candidates"])

    def test_seed_preserves_original_parent_after_the_hand_list_changes(self) -> None:
        current = [
            {
                "id": "child",
                "points": [(0, 0), (2, 0)],
                "joins": [("new", (2, 0))],
                "join_ends": [("to", "new", (2, 0))],
            }
        ]
        seed = {"approved_joins": {"child": [{"end": "to", "parent": "old", "cell": [1, 0]}]}}
        resolved = network.seed_resolution(seed, current, {"child": [(0, 0), (1, 0)]})
        self.assertEqual(resolved[0]["join_ends"], [("to", "old", (1, 0))])
        self.assertEqual(resolved[0]["points"], [(0, 0), (1, 0)])


class ReportTests(unittest.TestCase):
    def test_report_uses_stored_stretch_once_and_includes_twenty_metre_crossing(self) -> None:
        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [
                    ("wide", Fraction(1, 2), Fraction(20)),
                    ("narrow", Fraction(1, 3), Fraction(199, 10)),
                ]

        graph = {
            "node": [
                {"id": 1, "kind": "place", "place": "a", "cell": [0, 0]},
                {"id": 2, "kind": "place", "place": "b", "cell": [1, 0]},
            ],
            "stretch": [
                {
                    "id": 1,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0], [1, 0]],
                    "length_m": 500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                    "crossings": [{"river": "wide", "width_tenths_m": 200, "kind": "ford"}],
                }
            ],
            "link": [
                {
                    "id": "a",
                    "from_node": 1,
                    "to_node": 2,
                    "stretches": [[1, "forward"]],
                    "record": "seed",
                },
                {
                    "id": "b",
                    "from_node": 2,
                    "to_node": 1,
                    "stretches": [[1, "backward"]],
                    "record": "seed",
                },
            ],
            "join": [],
            "road": [],
            "access": [],
        }
        links = [
            {"id": "a", "category": "road", "level": 1},
            {"id": "b", "category": "path", "level": 1},
        ]
        text = network.network_report(
            graph,
            links,
            [],
            Index(),
            (20000, 15000),
            proposed=True,
            crossing_entries=[{"river": "wide", "at": [1, 0], "kind": "ford", "note": "approved"}],
            crossing_hits={1: [{"point": (Fraction(1, 2), Fraction(0))}]},
        )
        self.assertIn("proposed", text.lower())
        self.assertEqual(text.count("wide,"), 1)
        self.assertNotIn("narrow", text)
        self.assertIn("2 links", text)
        self.assertIn("1 stretch", text)

    def test_starting_disagreement_is_read_from_stored_link_records(self) -> None:
        import json  # noqa: PLC0415  # The stored link record is an opaque JSON string.

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return []

        graph = {
            "node": [
                {"id": 1, "kind": "place", "place": "a", "cell": [0, 0]},
                {"id": 2, "kind": "place", "place": "b", "cell": [1, 0]},
            ],
            "stretch": [
                {
                    "id": 1,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0], [1, 0]],
                    "length_m": 500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [
                {
                    "id": "a",
                    "stretches": [[1, "forward"]],
                    "record": json.dumps({"starting_state": ["road", 1]}),
                },
                {
                    "id": "b",
                    "stretches": [[1, "backward"]],
                    "record": json.dumps({"starting_state": ["path", 1]}),
                },
            ],
            "join": [],
            "access": [],
        }
        edited_hand = [
            {"id": "a", "category": "road", "level": 1},
            {"id": "b", "category": "road", "level": 1},
        ]
        text = network.network_report(graph, edited_hand, [], Index(), (20000, 15000))
        self.assertIn("Starting-state disagreements: 1", text)

    def test_river_segment_seam_is_one_physical_crossing(self) -> None:
        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [
                    ("river", Fraction(1, 2), Fraction(20)),
                    ("river", Fraction(1, 2), Fraction(21)),
                ]

        hits, touches = network.group_river_hits([(0, 0), (1, 0)], Index(), (20000, 15000))
        self.assertEqual(len(hits), 1)
        self.assertEqual(hits[0]["width"], 21)
        self.assertEqual(touches, [])

    def test_crossing_at_one_line_endpoint_without_shared_cell_is_reported(self) -> None:
        stretches = [
            {"id": 1, "tidal": False, "cells": [[0, 0], [2, 1]]},
            {"id": 2, "tidal": False, "cells": [[1, 0], [1, 1]]},
            {"id": 3, "tidal": False, "cells": [[2, 0], [2, 2]]},
        ]
        crossings = network.nonconnecting_crossings(stretches)
        pairs = {(a, b) for a, b, _, _ in crossings}
        self.assertIn((1, 2), pairs)
        self.assertIn((1, 3), pairs)


class AcceptanceTests(unittest.TestCase):
    def test_stale_or_blocked_proposal_leaves_all_state_untouched(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline, road, pending, log, hand = [
                root / x
                for x in ("baseline.json", "roads.toml", "pending.json", "log.md", "hand.toml")
            ]
            baseline.write_text("old")
            hand.write_text("hand")
            log.write_text("history")
            files = {"baseline": baseline, "stored": road, "hand": hand}
            payload = {"complete": False, "inputs": network.snapshot(files)}
            pending.write_text(network.json_bytes(payload).decode())
            before = (baseline.read_bytes(), pending.read_bytes(), log.read_bytes())
            with self.assertRaisesRegex(ValueError, "blocked"):
                network.accept_pending(pending, files, root / "report.md", log, "owner record")
            self.assertEqual(
                (baseline.read_bytes(), pending.read_bytes(), log.read_bytes()), before
            )
            payload["complete"] = True
            pending.write_text(network.json_bytes(payload).decode())
            hand.write_text("changed")
            before = (baseline.read_bytes(), pending.read_bytes(), log.read_bytes())
            with self.assertRaisesRegex(ValueError, "changed"):
                network.accept_pending(pending, files, root / "report.md", log, "owner record")
            self.assertEqual(
                (baseline.read_bytes(), pending.read_bytes(), log.read_bytes()), before
            )
            self.assertFalse(road.exists())

    def test_complete_proposal_publishes_all_courses_and_records_acceptance(self) -> None:
        import json  # noqa: PLC0415  # Only the publication witness builds a pending JSON document.
        import tomllib  # noqa: PLC0415  # The witness reads the published TOML.

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline, road, pending, log, hand = [
                root / x
                for x in ("baseline.json", "roads.toml", "pending.json", "log.md", "hand.toml")
            ]
            baseline.write_text("old")
            hand.write_text("hand")
            log.write_text("history")
            files = {"baseline": baseline, "stored": road, "hand": hand}
            graph = {
                "format": 1,
                "hand_list_sha256": "",
                "baseline_sha256": "",
                "next_node_id": 1,
                "next_stretch_id": 1,
                "node": [],
                "stretch": [],
                "link": [
                    {"id": "a", "from_node": 1, "to_node": 1, "stretches": [], "record": "{}"}
                ],
                "join": [],
                "road": [],
                "access": [],
            }
            proposal = {
                "complete": True,
                "inputs": network.snapshot(files),
                "baseline": {"links": {"a": {"course": [[0, 0]]}}},
                "candidates": {"a": [[0, 0]]},
                "graph": graph,
                "report": "# Proposed travel network\n",
            }
            proposal["pictures"] = {}
            proposal["seal"] = network.proposal_seal(proposal)
            log.write_text(f"history\nProposal seal: {proposal['seal']}\n")
            pending.write_bytes(network.json_bytes(proposal))
            network.accept_pending(pending, files, root / "report.md", log, "owner record")
            self.assertFalse(pending.exists())
            self.assertEqual(
                json.loads(baseline.read_bytes())["links"]["a"]["record"], "owner record"
            )
            parsed = tomllib.loads(road.read_text())
            self.assertEqual(
                parsed["baseline_sha256"], network.hashlib.sha256(baseline.read_bytes()).hexdigest()
            )
            self.assertIn("owner record", parsed["link"][0]["record"])
            self.assertIn("Accepted: owner record", log.read_text())

    def test_failed_publication_restores_all_previous_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first, second, pending = root / "first", root / "second", root / "pending"
            first.write_bytes(b"old first")
            second.write_bytes(b"old second")
            pending.write_bytes(b"pending")
            original = Path.replace
            count = 0
            failure_at = 4

            def interrupted(path: Path, target: Path) -> Path:
                nonlocal count
                count += 1
                if count == failure_at:
                    raise OSError("injected write failure")
                return original(path, target)

            with (
                mock.patch.object(Path, "replace", interrupted),
                self.assertRaisesRegex(OSError, "injected"),
            ):
                network.commit_files({first: b"new first", second: b"new second", pending: None})
            self.assertEqual(first.read_bytes(), b"old first")
            self.assertEqual(second.read_bytes(), b"old second")
            self.assertEqual(pending.read_bytes(), b"pending")


class IrelandPreviewTests(unittest.TestCase):
    def test_first_preview_has_six_candidates_and_connected_land(self) -> None:
        import json  # noqa: PLC0415  # This witness reads the committed approved seed.

        root = Path(__file__).resolve().parents[1]
        map_dir = root / "assets/map/ireland"
        scenario = root / "assets/scenarios/ireland-768"
        context = network.load_context(map_dir, scenario)
        seed = json.loads((scenario / "network-seed.json").read_text())
        baseline = network.make_seed_baseline(context, seed)
        preview = network.plan_network(context, baseline, None, "a" * 64)
        self.assertTrue(preview["complete"])
        self.assertEqual(
            set(preview["candidates"]),
            {
                "dala-2",
                "path-iniscealtra",
                "path-ciarraige_seat",
                "path-tory",
                "path-taghmon",
                "path-ardmore",
            },
        )
        self.assertEqual(len(baseline["completions"]), 12)
        self.assertEqual(baseline["place_overrides"]["monaincha"]["to"], [419, 593])
        self.assertIn("drumragh, 22.6 m", preview["report"])

    def test_first_report_stays_identical_after_candidate_metadata_is_accepted(self) -> None:
        import json  # noqa: PLC0415  # The witness reads the seed but writes no stored network.

        root = Path(__file__).resolve().parents[1]
        map_dir, scenario = root / "assets/map/ireland", root / "assets/scenarios/ireland-768"
        context = network.load_context(map_dir, scenario)
        baseline = network.make_seed_baseline(
            context, json.loads((scenario / "network-seed.json").read_text())
        )
        preview = network.plan_network(context, baseline, None, "a" * 64)
        future = preview["baseline"]
        for name in preview["candidates"]:
            future["links"][name]["record"] = "fixture approval"
        rebuilt = network.plan_network(context, future, preview["graph"], "a" * 64)
        self.assertEqual(rebuilt["candidates"], {})
        self.assertEqual(
            preview["report"].replace("# Proposed travel network", "# Travel network", 1),
            rebuilt["report"],
        )
        self.assertIn("(superseded join)", rebuilt["report"])


class CommandTests(unittest.TestCase):
    def make_fixture(self, root: Path) -> tuple[Path, Path]:
        import json  # noqa: PLC0415  # The disposable command fixture writes its seed.
        import math  # noqa: PLC0415  # The fixture projects cell centres to scenario coordinates.

        from PIL import Image  # noqa: PLC0415  # PNGs are needed only by this command fixture.

        map_dir, scenario = root / "map", root / "scenario"
        map_dir.mkdir()
        scenario.mkdir()
        origin = (22436, 14979)
        (map_dir / "map.toml").write_text(
            "[crop]\norigin = [22436, 14979]\nwidth = 4\nheight = 3\n"
        )
        (map_dir / "terrain.toml").write_text(
            '[[terrain]]\nname = "pasture"\nkind = "land"\nkey = "010203"\n'
        )
        Image.new("RGB", (4, 3), (1, 2, 3)).save(map_dir / "terrain.png")
        Image.new("I;16", (4, 3), 11000).save(map_dir / "elevation.png")
        Image.new("L", (4, 3), 125).save(map_dir / "fertility.png")
        (map_dir / "rivers.toml").write_text("stretch = []\n")
        radius = 6_378_137.0
        half_world = math.pi * radius

        def position(x: int, y: int) -> tuple[float, float]:
            lon = math.degrees(((origin[0] + x + 0.5) * 839 - half_world) / radius)
            lat = math.degrees(
                2 * math.atan(math.exp((half_world - (origin[1] + y + 0.5) * 839) / radius))
                - math.pi / 2
            )
            return lat, lon

        a, b = position(0, 0), position(3, 0)
        (scenario / "locations.toml").write_text(
            f'[[location]]\nid = "a"\nlat = {a[0]}\nlon = {a[1]}\n\n[[location]]\nid = "b"\nlat = {b[0]}\nlon = {b[1]}\n'
        )
        (scenario / "network.toml").write_text(
            '[costs]\nterrain = { pasture = 1.0 }\nslope = 10.0\nfertile = 0.5\ncrossing_base = 2.0\ncrossing_per_m = 0.125\nreuse = 0.6\n\n[[link]]\nid = "road"\nfrom = "a"\nto = "b"\ncategory = "road"\nlevel = 1\n'
        )
        (scenario / "network-seed.json").write_text(
            json.dumps(
                {
                    "links": [
                        {"id": "road", "cells": [[0, 0], [1, 0], [2, 0], [3, 0]], "pins": []}
                    ],
                    "approved_points": {"road": [[0, 0], [3, 0]]},
                    "approved_joins": {"road": []},
                }
            )
        )
        return map_dir, scenario

    def test_first_empty_proposal_accept_unchanged_build_and_check(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            plan = network.build_network(map_dir, scenario, importing=True)
            self.assertTrue(plan["complete"])
            self.assertEqual(plan["candidates"], {})
            self.assertFalse((scenario / "roads.toml").exists())
            pending = scenario / "courses-pending.json"
            network.accept_pending(
                pending,
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture approval",
            )
            self.assertTrue((scenario / "roads.toml").exists())
            before = (scenario / "roads.toml").read_bytes()
            log = (scenario / "network-changes.md").read_bytes()
            network.build_network(map_dir, scenario)
            self.assertEqual((scenario / "roads.toml").read_bytes(), before)
            self.assertEqual((scenario / "network-changes.md").read_bytes(), log)
            network.check_network(map_dir, scenario)

    def test_removing_highest_stretch_never_reuses_its_id(self) -> None:
        import tomllib  # noqa: PLC0415  # Inspect the stored IDs in the disposable fixture.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            original = tomllib.loads((scenario / "roads.toml").read_text())
            old_id = max(s["id"] for s in original["stretch"])
            hand_path = scenario / "network.toml"
            original_hand = hand_path.read_text()
            hand_path.write_text(original_hand.split("[[link]]")[0])
            network.build_network(map_dir, scenario)
            network.check_network(map_dir, scenario)
            hand_path.write_text(original_hand.replace('id = "road"', 'id = "new-road"'))
            plan = network.build_network(map_dir, scenario)
            self.assertIn("new-road", plan["candidates"])
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture new",
            )
            updated = tomllib.loads((scenario / "roads.toml").read_text())
            self.assertGreater(min(s["id"] for s in updated["stretch"]), old_id)
            network.check_network(map_dir, scenario)

    def test_hand_list_rejects_invalid_references_and_keys_with_context(self) -> None:
        land = [[True, False, True], [True, True, True]]
        locations = {"a": (0, 0), "b": (2, 0)}
        link = {"id": "route", "from": "a", "to": "b", "category": "road", "level": 1}
        cases = [
            ({"link": [link, link]}, "link route: repeated"),
            ({"link": [{**link, "from": "missing"}]}, "link route: unknown place"),
            ({"link": [{**link, "road": "missing"}]}, "link route: unknown road"),
            ({"link": [{**link, "category": "boat"}]}, "link route: unknown category"),
            (
                {"link": [{**link, "to": {"parent": "later", "cell": [0, 0]}}]},
                "link route: unknown or later parent",
            ),
            ({"link": [{**link, "via": ["missing"]}]}, "link route: unknown place"),
            ({"access": [{"place": "a", "quality": "boat"}]}, "access a: unknown quality"),
            ({"landing": [{"place": "a", "cell": [0, 0]}]}, "landing a: repeated or no access"),
            ({"road": [{"id": "r", "name": "R"}, {"id": "r", "name": "R"}]}, "road r: repeated"),
        ]
        for document, message in cases:
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                network.validate_hand_list(document, locations, land)

    def test_tampered_pending_proposal_is_refused_without_publication(self) -> None:
        import json  # noqa: PLC0415  # This witness mutates a pending JSON document.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            pending = scenario / "courses-pending.json"
            proposal = json.loads(pending.read_text())
            proposal["graph"]["node"][0]["cell"] = [99, 99]
            pending.write_bytes(network.json_bytes(proposal))
            with self.assertRaisesRegex(ValueError, "seal"):
                network.accept_pending(
                    pending,
                    network.input_files(map_dir, scenario, include_seed=True),
                    scenario / "network-report.md",
                    scenario / "network-changes.md",
                    "fixture",
                )
            self.assertFalse((scenario / "roads.toml").exists())

    def test_blocked_first_import_recovers_after_person_moves_join(self) -> None:
        import json  # noqa: PLC0415  # This fixture amends the seed JSON.
        import math  # noqa: PLC0415  # Project fixture cell centres to scenario coordinates.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            radius = 6_378_137.0
            half = math.pi * radius

            def position(x: int, y: int) -> tuple[float, float]:
                lon = math.degrees(((22436 + x + 0.5) * 839 - half) / radius)
                lat = math.degrees(
                    2 * math.atan(math.exp((half - (14979 + y + 0.5) * 839) / radius)) - math.pi / 2
                )
                return lat, lon

            c = position(3, 1)
            with (scenario / "locations.toml").open("a") as stream:
                stream.write(f'\n[[location]]\nid = "c"\nlat = {c[0]}\nlon = {c[1]}\n')
            lat, lon = position(0, 2)
            hand = scenario / "network.toml"
            text = hand.read_text().replace(
                'to = "b"', f'to = "b"\nvia = [{{ lat = {lat}, lon = {lon}, note = "move road" }}]'
            )
            text += '\n[[link]]\nid = "child"\nfrom = "c"\nto = { parent = "road", cell = [1, 0] }\ncategory = "path"\nlevel = 1\n'
            hand.write_text(text)
            seed_path = scenario / "network-seed.json"
            seed = json.loads(seed_path.read_text())
            seed["links"].append({"id": "child", "cells": [[3, 1], [1, 0]], "pins": []})
            seed["approved_points"]["child"] = [[3, 1], [1, 0]]
            seed["approved_joins"]["child"] = [{"end": "to", "parent": "road", "cell": [1, 0]}]
            seed_path.write_text(json.dumps(seed))
            first = network.build_network(map_dir, scenario, importing=True)
            self.assertFalse(first["complete"])
            self.assertIn("child", first["blocked"])
            parent = first["candidates"]["road"]
            pending = scenario / "courses-pending.json"
            with self.assertRaisesRegex(ValueError, "blocked"):
                network.accept_pending(
                    pending,
                    network.input_files(map_dir, scenario, include_seed=True),
                    scenario / "network-report.md",
                    scenario / "network-changes.md",
                    "fixture",
                )
            repeated = network.build_network(map_dir, scenario)
            self.assertFalse(repeated["complete"])
            self.assertEqual(repeated["candidates"]["road"], parent)
            self.assertIn("child", repeated["blocked"])
            selected = parent[len(parent) // 2]
            hand.write_text(
                hand.read_text().replace("cell = [1, 0]", f"cell = [{selected[0]}, {selected[1]}]")
            )
            second = network.build_network(map_dir, scenario)
            self.assertTrue(second["complete"])
            self.assertEqual(second["candidates"]["road"], parent)
            self.assertIn("child", second["candidates"])
            self.assertFalse((scenario / "network-report.md").exists())
            network.accept_pending(
                pending,
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            network.build_network(map_dir, scenario)
            network.check_network(map_dir, scenario)

    def test_seeded_invalid_diagonal_is_retained_for_a_replacement(self) -> None:
        land = [[True, False], [True, True]]
        links = [{"id": "road", "points": [(0, 0), (1, 1)], "joins": []}]
        baseline, _ = network.import_seed(links, {"road": [(0, 0), (1, 1)]}, land)
        proposal = network.propose_courses(links, baseline, land, [[1, 1], [1, 1]])
        self.assertEqual(baseline["road"]["course"], [(0, 0), (1, 1)])
        self.assertEqual(proposal["candidates"]["road"], [(0, 0), (0, 1), (1, 1)])

    def test_check_rejects_stored_length_counter_and_hand_edits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            road = scenario / "roads.toml"
            original = road.read_text()
            hand = scenario / "network.toml"
            hand_original = hand.read_text()
            import re  # noqa: PLC0415  # Change just the selected serialized numeric field.

            for field in ("length_m", "next_stretch_id"):
                with self.subTest(field=field):
                    road.write_text(
                        re.sub(rf"(?m)^{field} = \d+", f"{field} = 0", original, count=1)
                    )
                    with self.assertRaises(ValueError):
                        network.check_network(map_dir, scenario)
                    road.write_text(original)
            hand.write_text(hand_original.replace("level = 1", "level = 2"))
            with self.assertRaises(ValueError):
                network.check_network(map_dir, scenario)
            hand.write_text(hand_original)
            network.check_network(map_dir, scenario)

    def test_every_first_proposal_input_is_bound_before_acceptance(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            files = network.input_files(map_dir, scenario, include_seed=True)
            pending, log = scenario / "courses-pending.json", scenario / "network-changes.md"
            for name, path in files.items():
                with self.subTest(input=name):
                    original = path.read_bytes() if path.exists() else None
                    original_baseline, original_pending, original_log = (
                        files["baseline"].read_bytes(),
                        pending.read_bytes(),
                        log.read_bytes(),
                    )
                    path.write_bytes((original or b"") + b"changed")
                    with self.assertRaisesRegex(ValueError, "changed"):
                        network.accept_pending(
                            pending, files, scenario / "network-report.md", log, "fixture"
                        )
                    self.assertEqual(pending.read_bytes(), original_pending)
                    self.assertEqual(log.read_bytes(), original_log)
                    if name != "baseline":
                        self.assertEqual(files["baseline"].read_bytes(), original_baseline)
                    if original is None:
                        path.unlink()
                    else:
                        path.write_bytes(original)

    def test_report_command_reads_the_stored_report(self) -> None:
        import contextlib  # noqa: PLC0415  # Capture command output in this witness.
        import io  # noqa: PLC0415  # The output is kept in memory.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(
                    network.main(
                        ["--map-dir", str(map_dir), "--scenario", str(scenario), "report"]
                    ),
                    1,
                )
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            with contextlib.redirect_stdout(output):
                self.assertEqual(
                    network.main(
                        ["--map-dir", str(map_dir), "--scenario", str(scenario), "report"]
                    ),
                    0,
                )
            self.assertIn("# Travel network", output.getvalue())

    def test_cost_change_is_passive_until_explicit_recalculation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            hand = scenario / "network.toml"
            hand.write_text(hand.read_text().replace("pasture = 1.0", "pasture = 1.5"))
            history = (scenario / "network-changes.md").read_bytes()
            plan = network.build_network(map_dir, scenario)
            self.assertEqual(plan["candidates"], {})
            self.assertEqual((scenario / "network-changes.md").read_bytes(), history)
            plan = network.build_network(map_dir, scenario, recalculate=frozenset({"road"}))
            self.assertEqual(set(plan["candidates"]), {"road"})
            self.assertIn("recalculation requested", plan["reasons"]["road"])
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture recalc",
            )
            network.build_network(map_dir, scenario)
            network.check_network(map_dir, scenario)

    def test_changed_preview_picture_is_refused(self) -> None:
        import json  # noqa: PLC0415  # Read the generated preview's picture inventory.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            pending = scenario / "courses-pending.json"
            proposal = json.loads(pending.read_text())
            picture = scenario / next(iter(proposal["pictures"]))
            picture.write_bytes(picture.read_bytes() + b"changed")
            with self.assertRaisesRegex(ValueError, "picture changed"):
                network.accept_pending(
                    pending,
                    network.input_files(map_dir, scenario, include_seed=True),
                    scenario / "network-report.md",
                    scenario / "network-changes.md",
                    "fixture",
                )
            self.assertFalse((scenario / "roads.toml").exists())

    def test_check_rejects_a_different_legal_stored_line(self) -> None:
        import re  # noqa: PLC0415  # Edit only the disposable stored graph's line and length.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            road = scenario / "roads.toml"
            original = road.read_text()
            replacement = "cells = [[0, 0], [1, 1], [2, 1], [3, 0]]"
            changed = re.sub(r"(?m)^cells = .*", replacement, original, count=1)
            length = network.ground_length([(0, 0), (1, 1), (2, 1), (3, 0)], (22436, 14979))
            changed = re.sub(r"(?m)^length_m = \d+", f"length_m = {length}", changed, count=1)
            self.assertNotEqual(changed, original)
            road.write_text(changed)
            with self.assertRaisesRegex(ValueError, "stored network link road"):
                network.check_network(map_dir, scenario)

    def test_impossible_course_refuses_without_writing_any_output(self) -> None:
        from PIL import Image  # noqa: PLC0415  # Paint the disposable map's impassable barrier.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            with (map_dir / "terrain.toml").open("a") as stream:
                stream.write('\n[[terrain]]\nname = "lake"\nkind = "lake"\nkey = "040506"\n')
            terrain_path = map_dir / "terrain.png"
            image = Image.open(terrain_path).convert("RGB")
            for row in range(3):
                image.putpixel((1, row), (4, 5, 6))
            image.save(terrain_path)
            with self.assertRaisesRegex(ValueError, "link road: no course"):
                network.build_network(map_dir, scenario, importing=True)
            for name in (
                "courses-approved.json",
                "courses-pending.json",
                "roads.toml",
                "network-changes.md",
                "network-report.md",
            ):
                self.assertFalse((scenario / name).exists())

    def test_two_recalculated_links_are_accepted_together(self) -> None:
        import json  # noqa: PLC0415  # Add a second seed link to the disposable fixture.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            hand = scenario / "network.toml"
            with hand.open("a") as stream:
                stream.write(
                    '\n[[link]]\nid = "back"\nfrom = "b"\nto = "a"\ncategory = "path"\nlevel = 1\n'
                )
            seed = scenario / "network-seed.json"
            document = json.loads(seed.read_text())
            document["links"].append(
                {"id": "back", "cells": [[3, 0], [2, 0], [1, 0], [0, 0]], "pins": []}
            )
            document["approved_points"]["back"] = [[3, 0], [0, 0]]
            document["approved_joins"]["back"] = []
            seed.write_text(json.dumps(document))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            plan = network.build_network(map_dir, scenario, recalculate=frozenset({"road", "back"}))
            self.assertEqual(set(plan["candidates"]), {"road", "back"})
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "both",
            )
            accepted = json.loads((scenario / "courses-approved.json").read_text())["links"]
            self.assertEqual(accepted["road"]["record"], "both")
            self.assertEqual(accepted["back"]["record"], "both")
            network.check_network(map_dir, scenario)

    def test_map_change_outside_and_under_a_course_is_incremental(self) -> None:
        from PIL import Image  # noqa: PLC0415  # Paint the disposable fixture's terrain.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            with (map_dir / "terrain.toml").open("a") as stream:
                stream.write('\n[[terrain]]\nname = "lake"\nkind = "lake"\nkey = "040506"\n')
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            terrain_path = map_dir / "terrain.png"
            image = Image.open(terrain_path).convert("RGB")
            image.putpixel((3, 2), (4, 5, 6))
            image.save(terrain_path)
            self.assertEqual(network.build_network(map_dir, scenario)["candidates"], {})
            image.putpixel((1, 0), (4, 5, 6))
            image.save(terrain_path)
            plan = network.build_network(map_dir, scenario)
            self.assertEqual(set(plan["candidates"]), {"road"})
            self.assertNotIn((1, 0), plan["candidates"]["road"])
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture reroute",
            )
            network.build_network(map_dir, scenario)
            network.check_network(map_dir, scenario)

    def test_flooded_diagonal_support_is_replaced_then_check_passes(self) -> None:
        import json  # noqa: PLC0415  # Set a diagonal approved seed in the disposable fixture.

        from PIL import Image  # noqa: PLC0415  # Flood one support cell after approval.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            with (map_dir / "terrain.toml").open("a") as stream:
                stream.write('\n[[terrain]]\nname = "lake"\nkind = "lake"\nkey = "040506"\n')
            seed = scenario / "network-seed.json"
            data = json.loads(seed.read_text())
            data["links"][0]["cells"] = [[0, 0], [1, 1], [2, 1], [3, 0]]
            seed.write_text(json.dumps(data))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            terrain_path = map_dir / "terrain.png"
            image = Image.open(terrain_path).convert("RGB")
            image.putpixel((1, 0), (4, 5, 6))
            image.save(terrain_path)
            plan = network.build_network(map_dir, scenario)
            self.assertIn("road", plan["candidates"])
            self.assertTrue(
                network.legal_course(
                    network.read_rasters(map_dir)["land"], plan["candidates"]["road"]
                )
            )
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture reroute",
            )
            network.build_network(map_dir, scenario)
            network.check_network(map_dir, scenario)

    def test_first_proposal_keeps_one_step_completion_with_no_reroute(self) -> None:
        import json  # noqa: PLC0415  # Add a seed child to the disposable fixture.
        import math  # noqa: PLC0415  # Project the child's cell centre.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            radius = 6_378_137.0
            half = math.pi * radius
            lon = math.degrees(((22436 + 3.5) * 839 - half) / radius)
            lat = math.degrees(
                2 * math.atan(math.exp((half - (14979 + 1.5) * 839) / radius)) - math.pi / 2
            )
            with (scenario / "locations.toml").open("a") as stream:
                stream.write(f'\n[[location]]\nid = "c"\nlat = {lat}\nlon = {lon}\n')
            with (scenario / "network.toml").open("a") as stream:
                stream.write(
                    '\n[[link]]\nid = "child"\nfrom = "c"\nto = { parent = "road", cell = [2, 1] }\ncategory = "path"\nlevel = 1\n'
                )
            seed_path = scenario / "network-seed.json"
            seed = json.loads(seed_path.read_text())
            seed["links"][0]["cells"] = [[0, 0], [1, 0], [2, 1], [3, 0]]
            seed["links"].append({"id": "child", "cells": [[3, 1], [2, 0]], "pins": []})
            seed["approved_points"]["child"] = [[3, 1], [2, 1]]
            seed["approved_joins"]["child"] = [{"end": "to", "parent": "road", "cell": [2, 1]}]
            seed_path.write_text(json.dumps(seed))
            plan = network.build_network(map_dir, scenario, importing=True)
            self.assertTrue(plan["complete"])
            self.assertEqual(plan["candidates"], {})
            self.assertIn("child: [2, 0] to [2, 1]", plan["report"])
            self.assertFalse((scenario / "roads.toml").exists())
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            network.build_network(map_dir, scenario)
            network.check_network(map_dir, scenario)

    def test_new_link_can_be_proposed_during_first_import(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            with (scenario / "network.toml").open("a") as stream:
                stream.write(
                    '\n[[link]]\nid = "new"\nfrom = "b"\nto = "a"\ncategory = "path"\nlevel = 1\n'
                )
            plan = network.build_network(map_dir, scenario, importing=True)
            self.assertTrue(plan["complete"])
            self.assertEqual(set(plan["candidates"]), {"new"})
            self.assertFalse((scenario / "roads.toml").exists())

    def test_cli_recalculate_all_requires_explicit_recalculation(self) -> None:
        import contextlib  # noqa: PLC0415  # Capture the expected proposal and refusal messages.
        import io  # noqa: PLC0415  # Keep command messages in memory.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            prefix = ["--map-dir", str(map_dir), "--scenario", str(scenario)]
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(network.main([*prefix, "build", "--all"]), 1)
                self.assertFalse((scenario / "courses-pending.json").exists())
                self.assertEqual(network.main([*prefix, "build", "--recalculate", "--all"]), 1)
            self.assertTrue((scenario / "courses-pending.json").exists())

    def test_level_change_keeps_course_and_stretch_id(self) -> None:
        import json  # noqa: PLC0415  # Compare the baseline before and after a state-only edit.
        import tomllib  # noqa: PLC0415  # Read the stored stretch IDs and levels.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            before = tomllib.loads((scenario / "roads.toml").read_text())
            baseline = json.loads((scenario / "courses-approved.json").read_text())
            hand = scenario / "network.toml"
            hand.write_text(hand.read_text().replace("level = 1", "level = 2"))
            plan = network.build_network(map_dir, scenario)
            after = tomllib.loads((scenario / "roads.toml").read_text())
            self.assertEqual(plan["candidates"], {})
            self.assertEqual(json.loads((scenario / "courses-approved.json").read_text()), baseline)
            self.assertEqual(
                [s["id"] for s in before["stretch"]], [s["id"] for s in after["stretch"]]
            )
            self.assertEqual({s["level"] for s in after["stretch"]}, {2})
            network.check_network(map_dir, scenario)

    def test_water_overlay_is_not_an_input_to_preview_or_acceptance(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make_fixture(Path(directory))
            network.build_network(map_dir, scenario, importing=True)
            (map_dir / "water.png").write_bytes(b"irrelevant overlay")
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            network.check_network(map_dir, scenario)


class SharedFixtureTests(unittest.TestCase):
    def test_import_unchanged_join_and_repeated_build_do_not_duplicate_history(self) -> None:
        import shutil  # noqa: PLC0415  # Work on the shared fixture without changing it.

        fixture = Path(__file__).resolve().parents[1] / "crates/map/tests/network"
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "fixture"
            shutil.copytree(fixture / "map", copy / "map")
            shutil.copytree(fixture / "scenarios", copy / "scenarios")
            map_dir, scenario = copy / "map/fixture", copy / "scenarios/fixture"
            (scenario / "roads.toml").unlink()
            (scenario / "courses-approved.json").unlink()
            plan = network.build_network(
                map_dir, scenario, importing=True, recalculate=frozenset({"child"})
            )
            self.assertIn("child", plan["candidates"])
            self.assertNotIn("  - Joins:", (scenario / "network-changes.md").read_text())
            network.build_network(map_dir, scenario, recalculate=frozenset({"child"}))
            log = scenario / "network-changes.md"
            before = log.read_bytes()
            pending = (scenario / "courses-pending.json").read_bytes()
            pictures = {p: p.read_bytes() for p in (scenario / "network-pictures").rglob("*.png")}
            network.build_network(map_dir, scenario, recalculate=frozenset({"child"}))
            self.assertEqual(log.read_bytes(), before)
            self.assertEqual((scenario / "courses-pending.json").read_bytes(), pending)
            self.assertEqual({p: p.read_bytes() for p in pictures}, pictures)

    def test_tool_rebuilds_shared_fixture_byte_for_byte(self) -> None:
        import shutil  # noqa: PLC0415  # The fixture is copied so its committed files stay untouched.
        import tomllib  # noqa: PLC0415  # Check the contract consumed by the Rust slice.

        fixture = Path(__file__).resolve().parents[1] / "crates/map/tests/network"
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "fixture"
            shutil.copytree(fixture / "map", copy / "map")
            shutil.copytree(fixture / "scenarios", copy / "scenarios")
            map_dir, scenario = copy / "map/fixture", copy / "scenarios/fixture"
            expected = (scenario / "roads.toml").read_bytes()
            parsed = tomllib.loads(expected.decode())
            self.assertEqual(len(parsed["road"][0]["links"]), 2)
            self.assertTrue(
                any(
                    stretch["tidal"] and len(stretch["cells"]) == 1 for stretch in parsed["stretch"]
                )
            )
            links = {link["id"]: link for link in parsed["link"]}
            singleton = links["singleton"]
            forward = links["parent"]["stretches"]
            backward = links["reverse"]["stretches"]
            self.assertEqual(
                [part[0] for part in forward], [part[0] for part in reversed(backward)]
            )
            self.assertTrue(
                all(a[1] != b[1] for a, b in zip(forward, reversed(backward), strict=True))
            )
            join = next(item for item in parsed["join"] if item["link"] == "singleton")
            self.assertEqual(
                join["along_m"], network.ground_length([(1, 3), (2, 3), (3, 3)], (22436, 14979))
            )
            nodes = {
                (node.get("place"), node["kind"]): node
                for node in parsed["node"]
                if "place" in node
            }
            self.assertEqual(nodes[("shore", "place")]["cell"], nodes[("shore", "landing")]["cell"])
            self.assertNotEqual(
                nodes[("isle", "place")]["cell"], nodes[("isle", "landing")]["cell"]
            )
            self.assertEqual(singleton["from_node"], singleton["to_node"])
            self.assertEqual(singleton["stretches"], [])
            self.assertTrue(
                any(stretch["cells"] == [[1, 5], [3, 6]] for stretch in parsed["stretch"])
            )
            (scenario / "roads.toml").unlink()
            proposal = network.build_network(map_dir, scenario)
            self.assertTrue(proposal["complete"])
            self.assertEqual(proposal["candidates"], {})
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture",
            )
            self.assertEqual((scenario / "roads.toml").read_bytes(), expected)
            network.check_network(map_dir, scenario)

    def test_check_rejects_corrupt_join_distance(self) -> None:
        import re  # noqa: PLC0415  # Mutate a copied fixture's stored join distance.
        import shutil  # noqa: PLC0415  # Preserve the shared fixture.

        fixture = Path(__file__).resolve().parents[1] / "crates/map/tests/network"
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "fixture"
            shutil.copytree(fixture / "map", copy / "map")
            shutil.copytree(fixture / "scenarios", copy / "scenarios")
            map_dir, scenario = copy / "map/fixture", copy / "scenarios/fixture"
            roads = scenario / "roads.toml"
            original = roads.read_text()
            roads.write_text(re.sub(r"(?m)^along_m = \d+", "along_m = 0", original, count=1))
            with self.assertRaisesRegex(ValueError, "stored network"):
                network.check_network(map_dir, scenario)

    def test_seed_completion_has_an_individual_review_picture(self) -> None:
        from PIL import (  # noqa: PLC0415  # This witness verifies the generated PNG.
            Image,
            ImageDraw,
        )

        hand = {
            "link": [{"id": "child", "from": "place", "to": {"parent": "parent", "cell": [3, 0]}}]
        }
        baseline = {
            "links": {
                "parent": {"course": [(3, 0), (2, 1)]},
                "child": {
                    "course": [(1, 0), (2, 0), (3, 0)],
                    "joins": [{"end": "to", "parent": "parent", "cell": [3, 0]}],
                },
            },
            "completions": [{"link": "child", "from": (2, 0), "added": (3, 0)}],
        }
        images, records = network.completion_artifacts(
            Image.new("RGB", (4, 4)), ImageDraw, hand, baseline, "test"
        )
        self.assertEqual(len(images), 1)
        self.assertIn("child", records[0])
        self.assertTrue(next(iter(images.values())).startswith(b"\x89PNG"))


class IrelandStoredTests(unittest.TestCase):
    map_dir: ClassVar[Path]
    scenario: ClassVar[Path]
    graph: ClassVar[dict[str, Any]]
    baseline: ClassVar[dict[str, Any]]
    nodes: ClassVar[dict[int, dict[str, Any]]]
    stretches: ClassVar[dict[int, dict[str, Any]]]
    links: ClassVar[dict[str, dict[str, Any]]]

    @classmethod
    def setUpClass(cls) -> None:
        import json  # noqa: PLC0415  # The committed baseline is JSON.
        import tomllib  # noqa: PLC0415  # The stored network is TOML.

        root = Path(__file__).resolve().parents[1]
        cls.map_dir = root / "assets/map/ireland"
        cls.scenario = root / "assets/scenarios/ireland-768"
        cls.graph = tomllib.loads((cls.scenario / "roads.toml").read_text())
        cls.baseline = json.loads((cls.scenario / "courses-approved.json").read_text())
        cls.nodes = {node["id"]: node for node in cls.graph["node"]}
        cls.stretches = {stretch["id"]: stretch for stretch in cls.graph["stretch"]}
        cls.links = {link["id"]: link for link in cls.graph["link"]}

    def reconstructed(self, link: dict[str, Any]) -> list[list[int]]:
        cells: list[list[int]] = []
        for sid, direction in link["stretches"]:
            stretch = self.stretches[sid]
            part = stretch["cells"] if direction == "forward" else stretch["cells"][::-1]
            if cells:
                self.assertEqual(cells[-1], part[0])
                cells.extend(part[1:])
            else:
                cells.extend(part)
        return cells or [self.nodes[link["from_node"]]["cell"]]

    def test_stored_lines_reconstruct_the_baseline_and_are_legal(self) -> None:
        layers = network.read_rasters(self.map_dir)
        self.assertEqual(len(self.links), 123)
        for name, link in self.links.items():
            with self.subTest(link=name):
                course = self.reconstructed(link)
                self.assertEqual(course, self.baseline["links"][name]["course"])
                self.assertTrue(
                    network.legal_course(layers["land"], [tuple(cell) for cell in course])
                )
        for name in ("path-antrim", "path-monaincha"):
            self.assertEqual(self.links[name]["stretches"], [])
            self.assertEqual(self.links[name]["from_node"], self.links[name]["to_node"])
        self.assertEqual(self.baseline["links"]["path-monaincha"]["course"], [[419, 593]])
        self.assertEqual(self.baseline["links"]["path-monaincha"]["original_cell"], [418, 592])

    def test_only_authorized_courses_and_dependent_join_change(self) -> None:
        import json  # noqa: PLC0415  # Compare committed data with the approved seed.

        context = network.load_context(self.map_dir, self.scenario)
        raw = json.loads((self.scenario / "network-seed.json").read_text())
        seed = network.make_seed_baseline(context, raw)
        changed = {
            name
            for name, entry in self.baseline["links"].items()
            if entry["course"] != [list(cell) for cell in seed["links"][name]["course"]]
        }
        self.assertEqual(
            changed,
            {
                "dala-2",
                "path-iniscealtra",
                "path-ciarraige_seat",
                "path-tory",
                "path-taghmon",
                "path-ardmore",
            },
        )
        self.assertEqual(len(seed["completions"]), 12)

    def test_new_paths_and_connectivity_without_shannon_crossings(self) -> None:
        import hashlib  # noqa: PLC0415  # Compare the unchanged stored courses.
        import json  # noqa: PLC0415  # Canonicalize their cells for the digest.
        from itertools import pairwise  # noqa: PLC0415  # Traverse stored cell edges.

        other = {
            name: entry["course"]
            for name, entry in self.baseline["links"].items()
            if name not in {"path-taghmon", "path-ardmore"}
        }
        digest = hashlib.sha256(
            json.dumps(other, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        self.assertEqual(digest, "d83b24c6a833bd94f9ca80d9ae917d860bf8c9ac41ba18e696a2f46db2da6028")
        wide_metres = 20
        context = network.load_context(self.map_dir, self.scenario)
        places = {node["place"]: node for node in self.nodes.values() if node["kind"] == "place"}
        for child, place, parent, old in (
            ("path-taghmon", "taghmon", "chualann-2", "st_mullins"),
            ("path-ardmore", "ardmore", "munster-2", "cloyne"),
        ):
            link = self.links[child]
            join = next(j for j in self.graph["join"] if j["link"] == child)
            self.assertEqual(link["from_node"], places[place]["id"])
            self.assertEqual(join["parent"], parent)
            self.assertEqual(link["to_node"], join["node"])
            self.assertNotEqual(link["to_node"], places[old]["id"])
            self.assertIn(self.nodes[join["node"]]["cell"], self.reconstructed(self.links[parent]))
            course = list(map(tuple, self.reconstructed(link)))
            self.assertFalse(
                [
                    (a, b)
                    for a, b in pairwise(course)
                    if any(width >= wide_metres for _, _, width in context["index"].crossings(a, b))
                ]
            )
        cuts = set()
        for name in ("mor-4", "cut-clonfert-lorrha"):
            for a, b in pairwise(map(tuple, self.reconstructed(self.links[name]))):
                if any(
                    river == "shannon" and width >= wide_metres
                    for river, _, width in context["index"].crossings(a, b)
                ):
                    cuts.add(frozenset((a, b)))
        self.assertEqual(len(cuts), 2)
        adjacent: dict[tuple[int, ...], set[tuple[int, ...]]] = defaultdict(set)
        for stretch in self.stretches.values():
            if not stretch["tidal"]:
                for a, b in pairwise(map(tuple, stretch["cells"])):
                    if frozenset((a, b)) not in cuts:
                        adjacent[a].add(b)
                        adjacent[b].add(a)
        seen = {tuple(places["tara"]["cell"])}
        stack = list(seen)
        while stack:
            new = adjacent[stack.pop()] - seen
            seen.update(new)
            stack.extend(new)
        access = {entry["place"] for entry in self.graph["access"]}
        for name, node in places.items():
            if name not in access:
                self.assertIn(tuple(node["cell"]), seen, name)

    def test_places_and_landings_share_one_land_network(self) -> None:
        place_nodes = {
            node["place"]: node for node in self.nodes.values() if node["kind"] == "place"
        }
        access = {entry["place"]: entry for entry in self.graph["access"]}
        self.assertEqual(len(place_nodes), 125)
        self.assertEqual(len(access), 24)
        self.assertEqual(sum(entry["quality"] == "offshore" for entry in access.values()), 22)
        adjacency: dict[int, set[int]] = defaultdict(set)
        for stretch in self.stretches.values():
            if not stretch["tidal"]:
                adjacency[stretch["from"]].add(stretch["to"])
                adjacency[stretch["to"]].add(stretch["from"])
        seen = {place_nodes["tara"]["id"]}
        stack = list(seen)
        while stack:
            for next_id in adjacency[stack.pop()] - seen:
                seen.add(next_id)
                stack.append(next_id)
        for name, node in place_nodes.items():
            if name in access:
                self.assertIn(access[name]["landing"], seen)
                self.assertNotIn(node["id"], seen)
                self.assertFalse(adjacency[node["id"]])
            else:
                self.assertIn(node["id"], seen)
        tides = [stretch for stretch in self.stretches.values() if stretch["tidal"]]
        self.assertEqual(len(tides), 2)
        self.assertEqual(place_nodes["inishmacsaint"]["cell"], [418, 258])
        self.assertEqual(self.nodes[access["inishmacsaint"]["landing"]]["cell"], [418, 259])

    def test_ireland_report_and_clean_check(self) -> None:
        report = (self.scenario / "network-report.md").read_text()
        self.assertIn("drumragh, 22.6 m", report)
        self.assertIn("path-white_island to on north-1", report)
        self.assertIn("path-iniscealtra", report)
        self.assertFalse((self.scenario / "courses-pending.json").exists())
        network.check_network(self.map_dir, self.scenario)

    def test_three_owner_requested_outcomes_on_stored_lines(self) -> None:
        import math  # noqa: PLC0415  # Only this geographical witness needs distances.
        import tomllib  # noqa: PLC0415  # The witness reads the stored river geometry.

        crop = tomllib.loads((self.map_dir / "map.toml").read_text())["crop"]
        origin = tuple(crop["origin"])
        rivers = tomllib.loads((self.map_dir / "rivers.toml").read_text())["stretch"]

        def hits(link: str, river: str) -> list[tuple[float, float, str]]:
            relevant = [stretch for stretch in rivers if stretch["river"] == river]
            course = [tuple(cell) for cell in self.reconstructed(self.links[link])]
            result = []
            for stretch in relevant:
                index = network.RiverIndex([stretch], origin)
                for a, b in pairwise(course):
                    for _, t, _ in index.crossings(a, b):
                        result.append(
                            (
                                a[0] + float(t) * (b[0] - a[0]),
                                a[1] + float(t) * (b[1] - a[1]),
                                stretch["kind"],
                            )
                        )
            return result

        self.assertEqual(hits("dala-2", "shannon"), [])
        maigue = hits("path-ciarraige_seat", "maigue")
        self.assertEqual(len(maigue), 1)
        adare = network.project_point(52.56, -8.79, origin)
        self.assertLess(
            math.hypot(maigue[0][0] - adare[0] / 1_000_000, maigue[0][1] - adare[1] / 1_000_000)
            * 500,
            1000,
        )
        self.assertTrue(all(kind != "tidal" for _, _, kind in maigue))
        swilly = hits("path-tory", "swilly")
        self.assertTrue(all(kind != "tidal" for _, _, kind in swilly))


class OverrideTests(unittest.TestCase):
    def test_seeded_place_override_does_not_hide_a_later_location_move(self) -> None:
        locations = {"place": (1, 1)}
        overrides = {"place": {"from": [1, 1], "to": [2, 1]}}
        self.assertEqual(network.apply_place_overrides(locations, overrides)["place"], (2, 1))
        self.assertEqual(
            network.apply_place_overrides({"place": (3, 1)}, overrides)["place"], (3, 1)
        )

    def test_change_distance_measures_to_old_segments_not_only_vertices(self) -> None:
        old = [(0, 0), (10, 0)]
        new = [(5, 1)]
        distance, position = network.furthest_from_course(new, old)
        self.assertAlmostEqual(distance, 1.0)
        self.assertEqual(position, (5.0, 1.0))


class CrossingGroupingTests(unittest.TestCase):
    def test_groups_from_first_hit_and_deduplicates_cell_centres(self) -> None:
        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return {
                    ((0, 0), (1, 0)): [("r", Fraction(1), Fraction(20))],
                    ((1, 0), (2, 0)): [
                        ("r", Fraction(0), Fraction(21)),
                        ("r", Fraction(1, 2), Fraction(19)),
                    ],
                }.get((start, end), [])

        with mock.patch.object(network, "ground_length", return_value=400):
            crossings, touches = network.group_river_hits([(0, 0), (1, 0), (2, 0)], Index(), (0, 0))
        self.assertEqual(crossings, [])
        self.assertEqual(len(touches), 1)
        self.assertEqual(touches[0]["width"], 21)
        self.assertEqual(touches[0]["first_step"], 0)
        self.assertEqual(touches[0]["last_step"], 1)

    def test_exact_500_metres_starts_new_group(self) -> None:
        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(0), Fraction(20))] if start[0] in (0, 1) else []

        with mock.patch.object(network, "ground_length", return_value=500):
            crossings, touches = network.group_river_hits([(0, 0), (1, 0), (2, 0)], Index(), (0, 0))
        self.assertEqual(len(crossings), 2)
        self.assertEqual(touches, [])

    def test_groups_are_measured_from_the_first_hit(self) -> None:
        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(0), Fraction(20))]

        with mock.patch.object(network, "ground_length", return_value=400):
            crossings, touches = network.group_river_hits(
                [(0, 0), (1, 0), (2, 0), (3, 0)], Index(), (0, 0)
            )
        self.assertEqual([h["distance"] for h in touches], [0])
        self.assertEqual([h["distance"] for h in crossings], [800])


class CrossingHandListTests(unittest.TestCase):
    def test_matches_only_unambiguous_wide_crossings(self) -> None:
        hits = [{"river": "r", "cell": (5, 5), "width": Fraction(20), "stretch": 7, "distance": 30}]
        entries = [{"river": "r", "at": [8, 5], "kind": "ford", "note": "approved"}]
        self.assertEqual(network.match_crossings(hits, entries, {"r"}), ["ford"])
        with self.assertRaisesRegex(ValueError, "two entries"):
            network.match_crossings(hits, entries * 2, {"r"})
        with self.assertRaisesRegex(ValueError, "two crossings"):
            network.match_crossings(hits * 2, entries, {"r"})
        with self.assertRaisesRegex(ValueError, "no entry"):
            network.match_crossings(hits, [], {"r"})
        with self.assertRaisesRegex(ValueError, "no wide crossing"):
            network.match_crossings([], entries, {"r"})
        with self.assertRaisesRegex(ValueError, "unknown river"):
            network.match_crossings([], entries, set())
        with self.assertRaisesRegex(ValueError, "invalid kind"):
            network.match_crossings(hits, [{**entries[0], "kind": "bridge"}], {"r"})


class CrossingSplitTests(unittest.TestCase):
    def test_wide_step_is_cut_and_link_order_preserved(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0], "place": "a"},
                {"id": 2, "kind": "place", "cell": [3, 0], "place": "b"},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0], [1, 0], [2, 0], [3, 0]],
                    "length_m": 1500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]]}],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(1, 2), Fraction(20))] if start == (1, 0) else []

        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 500 * (len(cells) - 1)
        ):
            network.apply_crossings(
                graph,
                Index(),
                [{"river": "r", "at": [2, 0], "kind": "none", "note": "approved"}],
                (0, 0),
                {"r"},
            )
        self.assertEqual(
            [s["cells"] for s in graph["stretch"]],
            [[[0, 0], [1, 0]], [[1, 0], [2, 0]], [[2, 0], [3, 0]]],
        )
        self.assertEqual([n["kind"] for n in graph["node"]], ["place", "place", "bank", "bank"])
        self.assertEqual(graph["stretch"][1]["crossings"][0]["kind"], "none")
        self.assertEqual([item[0] for item in graph["link"][0]["stretches"]], [3, 4, 5])

    def test_node_centre_is_refused_before_cutting(self) -> None:
        graph: dict[str, Any] = {
            "node": [{"id": 1, "kind": "junction", "cell": [1, 0]}],
            "stretch": [{"id": 3, "from": 1, "to": 2, "cells": [[1, 0], [2, 0]], "tidal": False}],
            "link": [],
            "next_node_id": 4,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(0), Fraction(20))]

        with self.assertRaisesRegex(ValueError, r"river r.*node 1.*stretches \[3\]"):
            network.apply_crossings(graph, Index(), [], (0, 0), {"r"})
        self.assertEqual(graph["next_node_id"], 4)

    def test_none_breaks_report_land_connectivity(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0], "place": "a"},
                {"id": 2, "kind": "place", "cell": [1, 0], "place": "b"},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0], [1, 0]],
                    "length_m": 500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                    "crossings": [{"river": "r", "width_tenths_m": 200, "kind": "none"}],
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]], "record": "seed"}],
            "join": [],
            "access": [],
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(1, 2), Fraction(20))]

        entries = [{"river": "r", "at": [1, 0], "kind": "none", "note": "owner"}]
        report = network.network_report(
            graph,
            [{"id": "x", "category": "road", "level": 1}],
            [],
            Index(),
            (0, 0),
            crossing_entries=entries,
            crossing_hits={3: [{"point": (Fraction(1, 2), Fraction(0))}]},
        )
        self.assertIn("## Places outside the largest land piece\n- b", report)
        self.assertIn("r, 20.0 m, none; owner", report)

    def test_junction_centre_refuses_every_approach_without_mutation(self) -> None:
        graph: dict[str, Any] = {
            "node": [{"id": 1, "kind": "junction", "cell": [1, 1]}],
            "stretch": [
                {"id": i + 1, "from": i + 2, "to": 1, "cells": [cell, [1, 1]], "tidal": False}
                for i, cell in enumerate([[0, 0], [0, 1], [2, 1], [2, 2]])
            ],
            "link": [],
            "next_node_id": 6,
            "next_stretch_id": 5,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(1), Fraction(20))]

        for kind in ("ford", "none"):
            before = copy.deepcopy(graph)
            with self.assertRaisesRegex(ValueError, r"river r.*node 1.*stretches \[1, 2, 3, 4\]"):
                network.apply_crossings(
                    graph,
                    Index(),
                    [{"river": "r", "at": [1, 1], "kind": kind, "note": "owner"}],
                    (0, 0),
                    {"r"},
                )
            self.assertEqual(graph, before)

    def test_overlapping_spans_merge_and_touching_spans_do_not(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0]},
                {"id": 2, "kind": "place", "cell": [4, 0]},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[i, 0] for i in range(5)],
                    "length_m": 400,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "backward"]]}],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return {
                    0: [("a", Fraction(1, 2), Fraction(20))],
                    1: [
                        ("a", Fraction(1, 3), Fraction(20)),
                        ("a", Fraction(2, 3), Fraction(20)),
                        ("b", Fraction(1, 2), Fraction(20)),
                    ],
                    2: [("c", Fraction(1, 2), Fraction(20))],
                }.get(start[0], [])

        entries = [
            {"river": r, "at": [x, 0], "kind": "none" if r == "b" else "ford", "note": "approved"}
            for r, x in [("a", 1), ("b", 2), ("c", 3)]
        ]
        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 100 * (len(cells) - 1)
        ):
            network.apply_crossings(graph, Index(), entries, (0, 0), {"a", "b", "c"})
        self.assertEqual(
            [s["cells"] for s in graph["stretch"]],
            [[[0, 0], [1, 0], [2, 0]], [[2, 0], [3, 0]], [[3, 0], [4, 0]]],
        )
        self.assertEqual([h["river"] for h in graph["stretch"][0]["crossings"]], ["a", "b"])
        self.assertEqual([item[0] for item in graph["link"][0]["stretches"]], [5, 4, 3])

    def test_narrow_crossing_is_a_wade_without_a_cut(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0]},
                {"id": 2, "kind": "place", "cell": [1, 0]},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0], [1, 0]],
                    "length_m": 500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]]}],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(1, 2), Fraction(199, 10))]

        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 500 * (len(cells) - 1)
        ):
            network.apply_crossings(graph, Index(), [], (0, 0), {"r"})
        self.assertEqual(
            graph["stretch"][0]["wades"], [{"river": "r", "width_tenths_m": 199, "along_m": 250}]
        )
        self.assertEqual(len(graph["stretch"]), 1)

    def test_centre_hit_belongs_to_preceding_step_and_merges(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0]},
                {"id": 2, "kind": "place", "cell": [3, 0]},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[i, 0] for i in range(4)],
                    "length_m": 1500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]]}],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return {
                    0: [("a", Fraction(1, 2), Fraction(20)), ("b", Fraction(1), Fraction(20))],
                    1: [("b", Fraction(0), Fraction(20))],
                }.get(start[0], [])

        entries = [
            {"river": r, "at": [1, 0], "kind": kind, "note": "approved"}
            for r, kind in [("a", "ford"), ("b", "none")]
        ]
        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 500 * (len(cells) - 1)
        ):
            network.apply_crossings(graph, Index(), entries, (0, 0), {"a", "b"})
        self.assertEqual(graph["stretch"][0]["cells"], [[0, 0], [1, 0]])
        self.assertEqual([c["river"] for c in graph["stretch"][0]["crossings"]], ["a", "b"])
        self.assertEqual(graph["stretch"][0]["crossings"][1]["kind"], "none")
        self.assertNotIn("crossings", graph["stretch"][1])

    def test_report_uses_crossings_assigned_to_each_cut_span(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0], "place": "a"},
                {"id": 2, "kind": "place", "cell": [3, 0], "place": "b"},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[i, 0] for i in range(4)],
                    "length_m": 1500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]], "record": "seed"}],
            "join": [],
            "access": [],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return {
                    0: [("x", Fraction(1), Fraction(20))],
                    1: [("x", Fraction(0), Fraction(20)), ("y", Fraction(1, 2), Fraction(20))],
                }.get(start[0], [])

        entries = [{"river": r, "at": [1, 0], "kind": "ford", "note": r} for r in ("x", "y")]
        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 500 * (len(cells) - 1)
        ):
            hits = network.apply_crossings(graph, Index(), entries, (0, 0), {"x", "y"})
            report = network.network_report(
                graph,
                [{"id": "x", "category": "road", "level": 1}],
                [],
                Index(),
                (0, 0),
                crossing_entries=entries,
                crossing_hits=hits,
            )
        self.assertIn("stretch 3: x, 20.0 m, ford; x", report)
        self.assertIn("stretch 4: y, 20.0 m, ford; y", report)

    def test_real_river_index_merges_three_hit_span_with_another_river(self) -> None:
        graph: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0]},
                {"id": 2, "kind": "place", "cell": [4, 0]},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[x, 0] for x in range(5)],
                    "length_m": 2000,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]]}],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }
        rivers = [
            {
                "river": "weave",
                "points": [(1000000, 0), (1000000, 1000000), (1600000, 0), (1600000, 1000000)],
                "width_m": [20, 20],
            },
            {"river": "other", "points": [(1750000, 0), (1750000, 1000000)], "width_m": [25, 25]},
        ]
        with mock.patch.object(
            network, "project_point", side_effect=lambda lat, lon, origin: (lat, lon)
        ):
            index = network.RiverIndex(rivers, (22436, 14979))
        entries = [
            {"river": "weave", "at": [1, 0], "kind": "ford", "note": "test"},
            {"river": "other", "at": [2, 0], "kind": "none", "note": "test"},
        ]
        network.apply_crossings(graph, index, entries, (22436, 14979), {"weave", "other"})
        crossing = next(s for s in graph["stretch"] if "crossings" in s)
        self.assertEqual(crossing["cells"], [[0, 0], [1, 0], [2, 0]])
        self.assertEqual([c["river"] for c in crossing["crossings"]], ["weave", "other"])
        self.assertEqual(crossing["crossings"][1]["kind"], "none")

    def test_shared_crossing_fixture_rebuilds_and_keeps_shared_links(self) -> None:
        scenario = (
            Path(__file__).resolve().parents[1] / "crates/map/tests/network/scenarios/fixture"
        )
        graph = tomllib.loads((scenario / "roads.toml").read_text())

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return {
                    ((1, 3), (2, 3)): [
                        ("wide-a", Fraction(1, 3), Fraction(20)),
                        ("wide-b", Fraction(2, 3), Fraction(30)),
                    ],
                    ((4, 6), (5, 5)): [("blocked", Fraction(1, 2), Fraction(25))],
                    ((6, 4), (6, 5)): [("narrow", Fraction(1, 2), Fraction(199, 10))],
                }.get((start, end), [])

        entries = [
            {"river": river, "at": cell, "kind": kind, "note": "test"}
            for river, cell, kind in [
                ("wide-a", [1, 3], "ford"),
                ("wide-b", [2, 3], "ford"),
                ("blocked", [5, 6], "none"),
            ]
        ]
        network.apply_crossings(
            graph, Index(), entries, (22436, 14979), {"wide-a", "wide-b", "blocked", "narrow"}
        )
        self.assertEqual(network.dump_roads(graph), (scenario / "crossings-roads.toml").read_text())
        links = {link["id"]: link for link in graph["link"]}
        self.assertEqual(
            [x[0] for x in links["parent"]["stretches"]],
            [x[0] for x in reversed(links["reverse"]["stretches"])],
        )

    def test_rebuilding_reuses_bank_and_piece_ids(self) -> None:
        base: dict[str, Any] = {
            "node": [
                {"id": 1, "kind": "place", "cell": [0, 0]},
                {"id": 2, "kind": "place", "cell": [3, 0]},
            ],
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[i, 0] for i in range(4)],
                    "length_m": 1500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
            "link": [{"id": "x", "stretches": [[3, "forward"]]}],
            "next_node_id": 3,
            "next_stretch_id": 4,
        }

        class Index:
            def crossings(
                self, start: tuple[int, int], end: tuple[int, int]
            ) -> list[tuple[str, Fraction, Fraction]]:
                return [("r", Fraction(1, 2), Fraction(20))] if start == (1, 0) else []

        entries = [{"river": "r", "at": [2, 0], "kind": "ford", "note": "approved"}]
        first = copy.deepcopy(base)
        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 500 * (len(cells) - 1)
        ):
            network.apply_crossings(first, Index(), entries, (0, 0), {"r"})
            uncut = network.uncut_previous(first)
            self.assertEqual(uncut["stretch"][0]["cells"], base["stretch"][0]["cells"])
            rebuilt = copy.deepcopy(base)
            rebuilt["next_node_id"] = first["next_node_id"]
            rebuilt["next_stretch_id"] = first["next_stretch_id"]
            network.apply_crossings(rebuilt, Index(), entries, (0, 0), {"r"}, previous=first)
        self.assertEqual(first, rebuilt)
        changed = copy.deepcopy(base)
        changed["next_node_id"] = first["next_node_id"]
        changed["next_stretch_id"] = first["next_stretch_id"]
        entries[0]["kind"] = "none"
        with mock.patch.object(
            network, "ground_length", side_effect=lambda cells, origin: 500 * (len(cells) - 1)
        ):
            network.apply_crossings(changed, Index(), entries, (0, 0), {"r"}, previous=first)
        self.assertEqual(changed["node"], first["node"])
        self.assertEqual(changed["link"], first["link"])
        self.assertEqual(
            [(s["id"], s["cells"]) for s in changed["stretch"]],
            [(s["id"], s["cells"]) for s in first["stretch"]],
        )
        self.assertEqual(changed["stretch"][1]["crossings"][0]["kind"], "none")

    def test_format_two_crossings_round_trip_as_toml(self) -> None:
        graph: dict[str, Any] = {
            "format": 2,
            "hand_list_sha256": "a",
            "baseline_sha256": "b",
            "next_node_id": 3,
            "next_stretch_id": 4,
            "stretch": [
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [[0, 0], [1, 0]],
                    "length_m": 500,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                    "crossings": [{"river": "r", "width_tenths_m": 200, "kind": "ford"}],
                    "wades": [{"river": "s", "width_tenths_m": 199, "along_m": 200}],
                }
            ],
        }
        loaded = tomllib.loads(network.dump_roads(graph))
        self.assertEqual(loaded["stretch"], graph["stretch"])


class IrelandCrossingTests(unittest.TestCase):
    def test_stored_crossings_and_courses(self) -> None:
        import json  # noqa: PLC0415  # Read the stored baseline for this scenario.

        root = Path(__file__).resolve().parents[1]
        scenario = root / "assets/scenarios/ireland-768"
        graph = tomllib.loads((scenario / "roads.toml").read_text())
        baseline = json.loads((scenario / "courses-approved.json").read_text())
        network.verify_stored_courses(graph, baseline)
        crossing = [s for s in graph["stretch"] if "crossings" in s]
        self.assertEqual(len(crossing), 45)
        self.assertEqual(sum(len(s["crossings"]) for s in crossing), 47)
        self.assertEqual(sum(any(c["kind"] == "none" for c in s["crossings"]) for s in crossing), 2)
        self.assertEqual(sum(len(s["crossings"]) == network._TWO for s in crossing), 2)
        report = (scenario / "network-report.md").read_text()
        self.assertIn("## Places outside the largest land piece\n- none", report)
        self.assertIn("owenmore-sligo", report)
        self.assertIn("robe", report)


# ---- Water: classes, the river graph, landings (CK-401) ----

_TERRAIN_KEYS = {
    "~": ("sea", "sea"),
    "l": ("lake", "lake"),
    ".": ("pasture", "land"),
    "m": ("marsh", "land"),
    "h": ("pasture", "land"),
}
_BARGE = {"id": "barge", "reach_m": 1000, "sea_going": False}
_CURRACH = {"id": "currach", "reach_m": 5000, "sea_going": True}
_ORIGIN = (22436, 14979)
_LAYER_LAND = 65_535
_BARGE_REACH_M = 1000
_CURRACH_REACH_M = 5000
_STRAIT_M = 500
_WITHIN_CELLS = 6
_Node = int | tuple[str, int]


def _latlon(x: Any, y: Any, origin: tuple[int, int] = _ORIGIN) -> tuple[float, float]:  # noqa: ANN401  # Fractions and floats.
    import math  # noqa: PLC0415  # The inverse of the tool's projection.

    radius = 6_378_137.0
    half = math.pi * radius
    east = (origin[0] + float(x)) * 839.0
    south = (origin[1] + float(y)) * 839.0
    lat = 2 * math.atan(math.exp((half - south) / radius)) - math.pi / 2
    return math.degrees(lat), math.degrees((east - half) / radius)


def survey(
    name: str,
    points: list[tuple[Any, Any]],
    *,
    kind: str = "river",
    width: tuple[float, float] = (30.0, 30.0),
    ends: str = "sea",
) -> dict[str, Any]:
    """A surveyed stretch from points in cell coordinates (a cell is a unit square)."""
    return {
        "id": name,
        "river": name.split("-")[0],
        "kind": kind,
        "width_m": list(width),
        "ends": ends,
        "points": [_latlon(x, y) for x, y in points],
    }


def water_context(
    rows: list[str],
    rivers: list[dict[str, Any]] | None = None,
    *,
    classes: list[dict[str, Any]] | None = None,
    extra: dict[str, Any] | None = None,
    heights: dict[tuple[int, int], int] | None = None,
) -> dict[str, Any]:
    """A small world: rows of '~' sea, 'l' lake, '.' land, 'm' marsh, 'h' high land (50 m)."""
    kinds = [[_TERRAIN_KEYS[c][1] for c in row] for row in rows]
    names = [[_TERRAIN_KEYS[c][0] for c in row] for row in rows]
    elevation = [[50 if c == "h" else 0 for c in row] for row in rows]
    for (x, y), value in (heights or {}).items():
        elevation[y][x] = value
    hand: dict[str, Any] = {"water_class": [dict(c) for c in (classes or [_BARGE, _CURRACH])]}
    hand.update(extra or {})
    layers = {
        "origin": _ORIGIN,
        "kind": kinds,
        "terrain": names,
        "elevation": elevation,
        "land": [[k not in {"sea", "lake"} for k in row] for row in kinds],
    }
    stretches = rivers or []
    return {
        "layers": layers,
        "hand": hand,
        "rivers": stretches,
        "index": network.RiverIndex(stretches, _ORIGIN),
        "hashes": {"terrain": "t" * 64, "rivers": "r" * 64},
    }


def water_graph(
    nodes: Sequence[tuple[int, str, tuple[int, int], str | None]],
    access: Sequence[tuple[str, str, int]] | None = None,
    stretches: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    """A graph of nodes (id, kind, cell, place), access entries (place, quality, landing node)."""
    return {
        "node": [
            {"id": i, "kind": k, "cell": list(c), **({"place": p} if p else {})}
            for i, k, c, p in nodes
        ],
        "stretch": stretches or [],
        "link": [],
        "access": [{"place": p, "quality": q, "landing": n} for p, q, n in access or []],
        "next_node_id": 100,
        "next_stretch_id": 100,
    }


def work_water(
    context: dict[str, Any],
    graph: dict[str, Any] | None = None,
    hits: dict[int, list[dict[str, Any]]] | None = None,
    previous: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    graph = graph if graph is not None else water_graph([])
    plan = network.plan_water(graph, context, hits or {}, previous)
    network.store_water(graph, plan, context, context["hashes"])
    return graph, plan


def kinds_of(*rows: str) -> list[list[str]]:
    return [[_TERRAIN_KEYS[c][1] for c in row] for row in rows]


class WaterLayerTests(unittest.TestCase):
    def test_distance_is_to_the_nearest_cell_that_is_not_sea_and_a_lake_counts_as_land(
        self,
    ) -> None:
        layer = network.sea_distance_layer(kinds_of("l~~~~~", "~~~~~."), _ORIGIN)
        one = (network.cell_side_um(0, _ORIGIN) + 500_000) // 1_000_000
        self.assertEqual((layer[0][0], layer[1][5]), (65_535, 65_535))
        self.assertEqual((layer[0][1], layer[1][4]), (one, one))
        # (2, 0) is two cells from the lake and three from the land; (3, 1) is two from the land.
        self.assertEqual(layer[0][2], network.sea_distance_layer(kinds_of("l~~"), _ORIGIN)[0][2])
        self.assertLess(layer[0][3], layer[0][2] * 2)

    def test_distance_is_straight_in_cells_so_a_diagonal_is_longer_than_a_side(self) -> None:
        layer = network.sea_distance_layer(kinds_of("~~~", "~~~", "~~."), _ORIGIN)
        side = network.cell_side_um(0, _ORIGIN)
        self.assertEqual(layer[1][2], (side + 500_000) // 1_000_000)
        self.assertGreater(layer[1][1], layer[1][2])

    def test_half_a_metre_goes_up_and_a_class_at_the_exact_distance_may_use_the_cell(self) -> None:
        side = network.cell_side_um(0, _ORIGIN)
        for cells in range(1, 6):
            layer = network.sea_distance_layer(kinds_of("." + "~" * cells), _ORIGIN)
            self.assertEqual(layer[0][cells], (cells * side + 500_000) // 1_000_000)
        # A half goes up: with a side of 474.5 m, two cells are 949 m exactly.
        self.assertEqual((2 * 474_500_000 + 500_000) // 1_000_000, 949)
        self.assertEqual((474_500_000 + 500_000) // 1_000_000, 475)
        distance = network.sea_distance_layer(kinds_of(".~~"), _ORIGIN)[0][2]
        for reach, usable in ((distance, True), (distance - 1, False)):
            context = water_context(
                [".~~"], classes=[{"id": "c", "reach_m": reach, "sea_going": True}]
            )
            _, plan = work_water(context)
            self.assertEqual((2, 0) in plan["usable"][0], usable, reach)

    def test_distance_is_cut_at_65534_and_land_is_65535(self) -> None:
        layer = network.sea_distance_layer(kinds_of("." + "~" * 300), _ORIGIN)[0]
        self.assertEqual(layer[0], 65_535)
        self.assertLess(layer[100], 65_534)
        self.assertEqual(layer[300], 65_534)
        self.assertEqual(max(v for v in layer if v != _LAYER_LAND), 65_534)

    def test_a_reach_beyond_65533_metres_is_refused(self) -> None:
        for reach in (65_534, -1):
            with self.assertRaisesRegex(ValueError, "reach_m"):
                network.validate_water(
                    {"water_class": [{"id": "x", "reach_m": reach, "sea_going": True}]}, {}
                )
        network.validate_water(
            {"water_class": [{"id": "x", "reach_m": 65_533, "sea_going": True}]}, {}
        )

    def test_the_layer_round_trips_as_one_channel_of_sixteen_bits(self) -> None:
        layer = [[0, 65_535, 1000], [65_534, 7, 8]]
        data = network.sea_distance_png(layer)
        self.assertEqual(network.read_sea_distance(data, 3, 2), layer)
        with self.assertRaisesRegex(ValueError, "size"):
            network.read_sea_distance(data, 2, 3)
        import io  # noqa: PLC0415  # Encode samples of the wrong format.

        from PIL import Image  # noqa: PLC0415  # Pillow is a tool dependency.

        stream = io.BytesIO()
        Image.new("L", (3, 2)).save(stream, format="PNG")
        with self.assertRaisesRegex(ValueError, "sample format"):
            network.read_sea_distance(stream.getvalue(), 3, 2)
        with self.assertRaisesRegex(ValueError, "malformed"):
            network.read_sea_distance(b"not a png", 3, 2)


def spans(graph: dict[str, Any], stretch: str) -> list[tuple[int, int, bool]]:
    """A stretch's reaches as beginning, end and whether each is usable."""
    return [(r["begin"], r["end"], r["usable"]) for r in graph["reach"] if r["stretch"] == stretch]


class RiverGraphTests(unittest.TestCase):
    def test_a_river_narrowing_across_one_lake_cell_is_usable_to_half_way(self) -> None:
        context = water_context(
            ["....", ".l..", "...."],
            [survey("r-1", [(0, 1.5), (3, 1.5)], width=(21, 19))],
        )
        graph, plan = work_water(context)
        self.assertEqual(
            spans(graph, "r-1"),
            [(0, 333_333, True), (333_333, 500_000, True), (500_000, 1_000_000, False)],
        )
        self.assertEqual([j["cell"] for j in graph["joint"]], [[0, 1], [1, 1], [1, 1], [2, 1]])
        self.assertEqual(
            [(j["reach"], j["cell"], j["classes"]) for j in graph["water_join"]],
            [(1, [1, 1], ["barge", "currach"])],
        )
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1])
        self.assertEqual(plan["bodies"][0]["sizes"], [[1, 3]])

    def test_a_river_widening_across_one_lake_cell_is_joined_at_half_way(self) -> None:
        context = water_context(
            ["....", ".l..", "...."],
            [survey("r-1", [(0, 1.5), (3, 1.5)], width=(19, 21))],
        )
        graph, _ = work_water(context)
        self.assertEqual(spans(graph, "r-1"), [(0, 500_000, False), (500_000, 1_000_000, True)])
        self.assertEqual([(j["reach"], j["cell"]) for j in graph["water_join"]], [(1, [1, 1])])

    def test_a_line_through_three_lake_cells_has_a_joint_and_a_join_for_each(self) -> None:
        context = water_context(
            ["......", ".lll..", "......"],
            [survey("r-1", [(0, 1.5), (6, 1.5)])],
        )
        graph, plan = work_water(context)
        self.assertEqual(
            spans(graph, "r-1"),
            [
                (0, 166_667, True),
                (166_667, 333_333, True),
                (333_333, 500_000, True),
                (500_000, 1_000_000, True),
            ],
        )
        self.assertEqual([j["cell"] for j in graph["water_join"]], [[1, 1], [2, 1], [3, 1]])
        self.assertTrue(all(r["length_mm"] > 0 for r in graph["reach"]))
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1])
        self.assertEqual(plan["bodies"][0]["sizes"], [[3, 5]])

    def test_a_stretch_with_two_distant_points_is_in_every_cell_between(self) -> None:
        context = water_context(
            ["..........", "..l..l....", ".........."],
            [survey("r-1", [(0.5, 1.5), (9.5, 1.5)])],
        )
        graph, _ = work_water(context)
        self.assertEqual([j["cell"] for j in graph["water_join"]], [[2, 1], [5, 1]])
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1])

    def test_open_cells_touching_only_at_a_corner_are_two_bodies(self) -> None:
        context = water_context(["l.", ".l"])
        graph, _ = work_water(context)
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [2, 2])

    def test_two_lines_in_one_land_cell_are_two_bodies(self) -> None:
        context = water_context(
            ["...", "...", "..."],
            [survey("a-1", [(0.2, 1.3), (2.8, 1.3)]), survey("b-1", [(0.2, 1.7), (2.8, 1.7)])],
        )
        graph, _ = work_water(context)
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [2, 2])

    def test_a_break_is_at_the_middle_of_the_first_run_and_splits_the_river(self) -> None:
        context = water_context(
            [".....", "..l..", "....."],
            [survey("r-1", [(0, 1.5), (5, 1.5)])],
            extra={"river_break": [{"stretch": "r-1", "cell": [2, 1], "name": "Falls"}]},
        )
        # The lake cell is the one the line is in: the break at its middle cuts the line there.
        graph, _ = work_water(context)
        item = graph["river_break"][0]
        above, below = graph["joint"][item["above"]], graph["joint"][item["below"]]
        self.assertEqual((above["stretch"], below["stretch"]), ("r-1", "r-1"))
        self.assertEqual((item["name"], item["cell"]), ("Falls", [2, 1]))
        # The lake cell holds the run from 0.4 to 0.6; its middle, 0.5, is the break. No reach spans it.
        self.assertEqual(
            spans(graph, "r-1"),
            [(0, 400_000, True), (400_000, 500_000, True), (500_000, 1_000_000, True)],
        )
        self.assertEqual(item["below"], item["above"] + 1)
        self.assertEqual(len(graph["joint"]), 5)
        # Each side of the break is joined to the lake cell, so that cell is the one way between them.
        self.assertEqual([j["cell"] for j in graph["water_join"]], [[2, 1], [2, 1]])
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1])

    def test_a_break_beside_a_lake_cell_the_line_is_not_in_does_not_rejoin_the_river(self) -> None:
        context = water_context(
            ["..l..", ".....", "....."],
            [survey("r-1", [(0, 1.5), (5, 1.5)])],
            extra={"river_break": [{"stretch": "r-1", "cell": [2, 1], "name": "Falls"}]},
        )
        graph, _ = work_water(context)
        # The river's two parts and the lake cell above it: three bodies.
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [3, 3])

    def test_a_break_in_a_cell_the_stretch_is_not_in_is_refused(self) -> None:
        context = water_context(
            ["....."],
            [survey("r-1", [(0, 0.5), (5, 0.5)])],
            extra={"river_break": [{"stretch": "r-1", "cell": [2, 3], "name": "Falls"}]},
        )
        with self.assertRaisesRegex(ValueError, "Falls.*not in cell"):
            work_water(context)

    def test_a_tributary_a_lough_and_its_outlet_are_one_body(self) -> None:
        context = water_context(
            ["........", "..lll...", "........"],
            [
                survey("main-1", [(4, 1.5), (8, 1.5)]),
                survey("lough-1", [(1.5, 1.5), (4, 1.5)], kind="lough", ends="main-1"),
                survey("trib-1", [(5, 0.2), (5, 1.5)], ends="main-1"),
            ],
        )
        graph, _ = work_water(context)
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1])

    def test_a_fork_round_an_island_with_a_break_on_one_arm_is_one_body(self) -> None:
        rows = [".........." for _ in range(4)]
        rows[1] = ".l......l."
        arms = [
            survey("north-1", [(1.5, 1.5), (3, 0.5), (7, 0.5), (8.5, 1.5)]),
            survey("south-1", [(1.5, 1.5), (3, 2.5), (7, 2.5), (8.5, 1.5)]),
        ]
        breaks = [{"stretch": "north-1", "cell": [5, 0], "name": "North falls"}]
        graph, _ = work_water(water_context(rows, arms, extra={"river_break": breaks}))
        # The north arm is cut in two, and the south arm is the only way between its parts.
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1])
        both = [*breaks, {"stretch": "south-1", "cell": [5, 2], "name": "South falls"}]
        graph, _ = work_water(water_context(rows, arms, extra={"river_break": both}))
        self.assertEqual([c["bodies"] for c in graph["water_class"]], [2, 2])

    def test_a_reach_between_a_crossing_and_a_break_0_4_metres_apart_is_400_mm(self) -> None:
        context = water_context(["..."], [survey("r-1", [(0, 0.5), (3, 0.5)])])
        line = network.WaterLine(
            network.RiverLine(context["rivers"][0], _ORIGIN), network.Fraction(20), []
        )
        # 0.4 m along the line, at this latitude, as a position in the tool's own units.
        side = network.cell_side_um(0, _ORIGIN) / 1e6
        start = network.Fraction(1_000_000)
        end = start + network.Fraction(0.4 / side * 1_000_000)
        self.assertEqual(network.reach_length_mm(line, start, end, _ORIGIN), 400)
        self.assertEqual(network.reach_length_mm(line, start, start + 1, _ORIGIN), 1)


def landings_at(graph: dict[str, Any], node: int) -> list[dict[str, Any]]:
    return [x for x in graph["landing"] if x["node"] == node]


class LandingTests(unittest.TestCase):
    def test_a_place_takes_the_nearest_shore_and_each_class_its_body(self) -> None:
        rows = ["~~~~~", "~...~", "~...~", "~~~~~"]
        graph, _ = work_water(water_context(rows), water_graph([(1, "place", (2, 1), "p")]))
        [landing] = landings_at(graph, 1)
        self.assertEqual(
            (landing["cell"], landing["shore"], landing["approach_m"], landing["bodies"]),
            ([2, 0], [2, 1], 0, [0, 0]),
        )
        self.assertNotIn("bank", landing)

    def test_an_approach_is_a_path_over_land_so_it_bends_round_a_lake_cell(self) -> None:
        rows = ["...~", ".l.~"]
        graph = water_graph(
            [(1, "place", (0, 1), "p"), (2, "landing", (0, 0), "p")],
            access=[("p", "offshore", 2)],
        )
        graph, _ = work_water(water_context(rows), graph)
        by_cell = {tuple(x["cell"]): x for x in landings_at(graph, 1)}
        side = network.cell_side_um(1, _ORIGIN)
        # The lake beside the place is a landing of its own; the sea is three steps away over land.
        self.assertEqual(by_cell[1, 1]["approach_m"], 0)
        self.assertEqual(by_cell[3, 0]["shore"], [2, 0])
        self.assertEqual(by_cell[3, 0]["approach_m"], (3 * side + 500_000) // 1_000_000)

    def test_a_place_that_reaches_two_barge_bodies_gets_a_landing_in_each(self) -> None:
        rows = ["...~", ".l.~"]
        graph = water_graph(
            [(1, "place", (0, 1), "p"), (2, "landing", (0, 0), "p")], [("p", "offshore", 2)]
        )
        graph, plan = work_water(water_context(rows), graph)
        barge = {x["bodies"][0] for x in landings_at(graph, 1)}
        self.assertEqual(len(barge), 2)
        self.assertEqual(plan["bodies"][0]["count"], 2)

    def test_a_junction_on_marsh_or_high_ground_has_no_landing_and_is_reported(self) -> None:
        rows = ["~m~", "~.~", "~h~", "~~~"]
        nodes = [
            (1, "junction", (1, 0), None),
            (2, "junction", (1, 2), None),
            (3, "junction", (1, 1), None),
        ]
        graph, plan = work_water(water_context(rows), water_graph(nodes))
        self.assertEqual(landings_at(graph, 1), [])
        self.assertEqual(landings_at(graph, 2), [])
        self.assertTrue(landings_at(graph, 3))
        self.assertEqual(
            sorted((r["node"], r["reason"]) for r in plan["refused"]),
            [(1, "marsh"), (2, "50 m above the water")],
        )

    def test_a_place_in_marsh_and_a_mainland_landing_node_on_marsh_each_get_one(self) -> None:
        rows = ["~m~m~"]
        nodes = [(1, "place", (1, 0), "p"), (2, "landing", (3, 0), "q"), (3, "place", (3, 0), "q")]
        graph, _ = work_water(water_context(rows), water_graph(nodes, [("q", "offshore", 2)]))
        self.assertTrue(landings_at(graph, 1))
        self.assertTrue(landings_at(graph, 2))

    def test_a_lake_shore_is_judged_by_its_height_above_the_lake_and_the_sea_by_its_height_above_zero(
        self,
    ) -> None:
        rows = ["l.~"]
        context = water_context(rows, heights={(0, 0): 100, (1, 0): 105})
        graph, plan = work_water(context, water_graph([(1, "junction", (1, 0), None)]))
        # 5 m above the lake is fine; 105 m above the sea at the other side is not.
        self.assertEqual([x["cell"] for x in landings_at(graph, 1)], [[0, 0]])
        self.assertEqual([r["reason"] for r in plan["refused"]], ["105 m above the water"])

    def test_a_river_in_the_nodes_own_cell_is_never_too_high(self) -> None:
        rows = ["...", ".h.", "..."]
        context = water_context(rows, [survey("r-1", [(0, 1.5), (3, 1.5)])])
        graph, plan = work_water(context, water_graph([(1, "junction", (1, 1), None)]))
        self.assertEqual(len(landings_at(graph, 1)), 1)
        self.assertEqual(plan["refused"], [])

    def test_a_junction_where_a_river_widens_past_20_m_gets_a_landing_half_way(self) -> None:
        rows = ["...", "...", "..."]
        context = water_context(rows, [survey("r-1", [(0, 1.5), (3, 1.5)], width=(19, 21))])
        graph, _ = work_water(context, water_graph([(1, "junction", (1, 1), None)]))
        [landing] = landings_at(graph, 1)
        reach = graph["reach"][
            next(i for i, r in enumerate(graph["reach"]) if r["from"] == landing["joint"])
        ]
        self.assertEqual((reach["begin"], reach["usable"]), (500_000, True))
        self.assertEqual((landing["bank"], landing["bodies"]), ("left", [0, 0]))

    def test_a_node_with_a_break_in_its_cell_gets_a_landing_on_each_side_on_one_bank(self) -> None:
        rows = ["...", "...", "..."]
        breaks = [{"stretch": "r-1", "cell": [1, 1], "name": "Falls"}]
        context = water_context(
            rows, [survey("r-1", [(0, 1.5), (3, 1.5)])], extra={"river_break": breaks}
        )
        graph, _ = work_water(context, water_graph([(1, "junction", (1, 1), None)]))
        landings = landings_at(graph, 1)
        self.assertEqual(len(landings), 2)
        self.assertEqual({x["bank"] for x in landings}, {"left"})
        self.assertEqual(sorted(x["run"] for x in landings), [0, 1])
        self.assertEqual(len({x["bodies"][0] for x in landings}), 2)

    def test_a_landing_keeps_its_id_when_a_joint_is_added_upstream(self) -> None:
        river = survey("r-1", [(0, 1.5), (6, 1.5)])
        nodes = [(1, "junction", (4, 1), None)]
        rows = ["......", "......", "......"]
        graph, _ = work_water(water_context(rows, [river]), water_graph(nodes))
        before = landings_at(graph, 1)[0]
        previous = {"landing": graph["landing"], "next_landing_id": graph["next_landing_id"]}
        again, _ = work_water(
            water_context(
                [".l....", ".l....", "......"][:0]
                or ["..l...", "......", "......"][:0]
                or ["......", ".l....", "......"],
                [river],
            ),
            water_graph(nodes),
            previous=previous,
        )
        after = landings_at(again, 1)[0]
        self.assertEqual(after["id"], before["id"])
        self.assertNotEqual(after["joint"], before["joint"])

    def test_the_two_places_held_as_shore_each_get_a_strait_with_the_classes_the_mark_allows(
        self,
    ) -> None:
        rows = ["~~~~~", "~...~", "~~~~~"]
        nodes = [(1, "place", (1, 1), "p"), (2, "landing", (2, 1), "p")]
        for marks, classes in (([], ["barge", "currach"]), ([{"place": "p"}], ["currach"])):
            context = water_context(rows, extra={"mark": marks})
            graph, _ = work_water(context, water_graph(nodes, [("p", "offshore", 2)]))
            self.assertEqual(
                [
                    (s["place"], s["from"], s["to"], s["length_m"], s["classes"])
                    for s in graph["strait"]
                ],
                [("p", 1, 2, 500, classes)],
            )
            # A marked place's landings have a body for the currach and none for the barge.
            bodies = [x["bodies"] for x in graph["landing"] if x["node"] in {1, 2}]
            self.assertTrue(all((b[0] < 0) == bool(marks) and b[1] >= 0 for b in bodies), bodies)

    def test_an_island_place_across_water_from_its_landing_has_no_strait(self) -> None:
        rows = ["~~~~~~~", "~.~~~.~", "~~~~~~~"]
        nodes = [(1, "place", (1, 1), "p"), (2, "landing", (5, 1), "p")]
        graph, _ = work_water(water_context(rows), water_graph(nodes, [("p", "offshore", 2)]))
        self.assertEqual(graph["strait"], [])

    def test_a_place_with_no_water_on_its_land_has_no_landing(self) -> None:
        graph, _ = work_water(water_context(["....."]), water_graph([(1, "place", (2, 0), "p")]))
        self.assertEqual(graph["landing"], [])


def crossing_world(  # noqa: PLR0913  # The world's parts and, for a rebuild, the stored graph before it.
    rows: list[str],
    rivers: list[dict[str, Any]],
    cells: list[tuple[int, int]],
    entries: list[tuple[str, tuple[int, int], str]],
    *,
    extra: dict[str, Any] | None = None,
    previous: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    """One road over rivers, cut by slice 3a's own tool, with its water worked."""
    context = water_context(rows, rivers, extra=extra)
    graph = water_graph(
        [(1, "place", cells[0], "a"), (2, "place", cells[-1], "b")],
        stretches=[
            {
                "id": 3,
                "from": 1,
                "to": 2,
                "cells": [list(c) for c in cells],
                "length_m": 1000 * len(cells),
                "category": "road",
                "level": 1,
                "tidal": False,
            }
        ],
    )
    graph["link"] = [{"id": "x", "stretches": [[3, "forward"]]}]
    hand = [{"river": r, "at": list(c), "kind": k, "note": "test"} for r, c, k in entries]
    hits = network.apply_crossings(
        graph, context["index"], hand, _ORIGIN, {r["river"] for r in rivers}
    )
    graph, plan = work_water(context, graph, hits, previous)
    return context, graph, plan


def crossing_stretches(graph: dict[str, Any]) -> list[dict[str, Any]]:
    return [s for s in graph["stretch"] if s.get("crossings")]


class CrossingEndTests(unittest.TestCase):
    ROWS: ClassVar[list[str]] = ["......", "......", "......"]
    ROAD: ClassVar[list[tuple[int, int]]] = [(0, 1), (1, 1), (2, 1), (3, 1)]

    def test_a_none_crossing_with_both_banks_allowed_has_both_ends_in_one_body(self) -> None:
        river = survey("r-1", [(2.2, 0), (2.2, 3)])
        _, graph, _ = crossing_world(self.ROWS, [river], self.ROAD, [("r", (2, 1), "none")])
        [stretch] = crossing_stretches(graph)
        ends = {x["end"]: x for x in graph["landing"] if x.get("crossing") == stretch["id"]}
        self.assertEqual(sorted(ends), ["from", "to"])
        self.assertEqual(stretch["landing_from"], ends["from"]["id"])
        self.assertEqual(stretch["landing_to"], ends["to"]["id"])
        self.assertEqual(ends["from"]["bodies"], ends["to"]["bodies"])
        self.assertEqual(ends["from"]["joint"], ends["to"]["joint"])
        # The river runs south: the road's west is its right bank, and the east its left.
        self.assertEqual((ends["from"]["bank"], ends["to"]["bank"]), ("right", "left"))
        self.assertEqual(
            (ends["from"]["node"], ends["to"]["node"]), (stretch["from"], stretch["to"])
        )

    def test_a_bank_that_is_refused_means_no_boat_crosses(self) -> None:
        river = survey("r-1", [(2.2, 0), (2.2, 3)])
        rows = ["......", "..m...", "......"]
        _, graph, plan = crossing_world(rows, [river], self.ROAD, [("r", (2, 1), "none")])
        [stretch] = crossing_stretches(graph)
        self.assertIn("landing_from", stretch)
        self.assertNotIn("landing_to", stretch)
        self.assertIn((stretch["to"], "marsh"), [(r["node"], r["reason"]) for r in plan["refused"]])
        report = network.water_report(graph, plan)
        self.assertIn(
            f"stretch {stretch['id']}: landings {stretch['landing_from']}, none; barge cannot cross; currach cannot cross",
            "\n".join(report),
        )

    def test_a_crossing_whose_nearest_point_is_a_break_has_no_landing_at_either_end(self) -> None:
        river = survey("r-1", [(2.2, 0), (2.2, 3)])
        breaks = [{"stretch": "r-1", "cell": [2, 1], "name": "Falls"}]
        _, graph, plan = crossing_world(
            self.ROWS, [river], self.ROAD, [("r", (2, 1), "none")], extra={"river_break": breaks}
        )
        [stretch] = crossing_stretches(graph)
        self.assertNotIn("landing_from", stretch)
        self.assertNotIn("landing_to", stretch)
        self.assertEqual(
            sorted(r["reason"] for r in plan["refused"] if "crossing" in r),
            ["the point is a break", "the point is a break"],
        )

    def test_a_stretch_over_two_rivers_that_do_not_meet_has_landings_and_no_boat_crosses(
        self,
    ) -> None:
        rivers = [
            survey("a-1", [(1.6, 0), (1.6, 3)]),
            survey("b-1", [(2.3, 0), (2.3, 3)], ends="sea"),
        ]
        _, graph, plan = crossing_world(
            self.ROWS, rivers, self.ROAD, [("a", (1, 1), "none"), ("b", (2, 1), "none")]
        )
        [stretch] = crossing_stretches(graph)
        self.assertEqual(len(stretch["crossings"]), 2)
        landings = {x["end"]: x for x in graph["landing"] if x.get("crossing") == stretch["id"]}
        self.assertEqual(sorted(landings), ["from", "to"])
        self.assertNotEqual(landings["from"]["bodies"], landings["to"]["bodies"])
        self.assertIsNotNone(plan)

    def test_two_crossing_stretches_meeting_at_a_node_have_a_landing_for_each_end(self) -> None:
        rivers = [
            survey("a-1", [(2.5, 0), (2.5, 3)]),
            survey("b-1", [(3.1, 0), (3.1, 3)]),
        ]
        road = [(0, 1), (1, 1), (2, 1), (3, 1), (4, 1)]
        rows = ["......", "......", "......"]
        _, graph, _ = crossing_world(
            rows, rivers, road, [("a", (2, 1), "none"), ("b", (3, 1), "none")]
        )
        first, second = sorted(crossing_stretches(graph), key=lambda s: s["cells"][0])
        self.assertEqual(first["to"], second["from"])
        by_id = {x["id"]: x for x in graph["landing"]}
        arriving, leaving = by_id[first["landing_to"]], by_id[second["landing_from"]]
        # The first river is met at the centre of the shared node's cell: it belongs to the stretch
        # arriving there, on the bank opposite the side the road comes from.
        self.assertEqual(arriving["bank"], "left")
        # The second river is met half way on, and the road lies on its west bank before it.
        self.assertEqual(leaving["bank"], "right")
        self.assertNotEqual(arriving["joint"], leaving["joint"])
        # The node also has landings of its own in each river it holds, apart from the crossing ends'.
        own = [x for x in landings_at(graph, first["to"]) if "crossing" not in x]
        self.assertEqual(len(own), 1)
        self.assertEqual(own[0]["survey"], "a-1")
        self.assertNotEqual(own[0]["id"], arriving["id"])

    def test_a_crossing_ends_landing_is_known_by_its_stretch_and_end_alone(self) -> None:
        for bank in ("left", "right"):
            for node in (1, 2, 17):
                key = network.landing_key(
                    {"node": node, "bank": bank, "crossing": 52, "end": "from"}
                )
                self.assertEqual(key, ("crossing", 52, "from"))
        self.assertNotEqual(
            network.landing_key({"node": 1, "crossing": 52, "end": "from"}),
            network.landing_key({"node": 1, "crossing": 52, "end": "to"}),
        )

    def test_a_crossing_end_keeps_its_id_when_its_bank_or_node_changes_on_a_rebuild(self) -> None:
        river = survey("r-1", [(2.2, 0), (2.2, 3)])
        _, graph, _ = crossing_world(self.ROWS, [river], self.ROAD, [("r", (2, 1), "none")])
        ends = {x["end"]: x for x in graph["landing"] if x.get("crossing") is not None}
        self.assertEqual(sorted(ends), ["from", "to"])
        flipped = {"left": "right", "right": "left"}
        # The survey or the approach now puts each end on the other bank and at another node, the crossing and its end the same.
        previous = {
            "landing": [
                {**x, "bank": flipped[x["bank"]], "node": x["node"] + 50} if "crossing" in x else x
                for x in graph["landing"]
            ],
            "next_landing_id": graph["next_landing_id"],
        }
        _, again, _ = crossing_world(
            self.ROWS, [river], self.ROAD, [("r", (2, 1), "none")], previous=previous
        )
        after = {x["end"]: x for x in again["landing"] if x.get("crossing") is not None}
        for end in ("from", "to"):
            self.assertEqual(after[end]["id"], ends[end]["id"], end)
            self.assertEqual(after[end]["bank"], ends[end]["bank"])
        self.assertEqual(again["next_landing_id"], graph["next_landing_id"])

    def test_in_a_group_whose_first_point_is_not_its_widest_the_end_is_at_the_nearest_point(
        self,
    ) -> None:
        # One river crossed three times within 500 m is one crossing; it widens as it goes, so the
        # first point is the narrowest and the group is wide at its last.
        river = survey(
            "r-1",
            [(1.6, 0), (1.6, 2), (2.0, 2), (2.0, 1), (2.4, 1), (2.4, 3)],
            width=(5, 40),
        )
        _, graph, _ = crossing_world(self.ROWS, [river], self.ROAD, [("r", (2, 1), "none")])
        [stretch] = crossing_stretches(graph)
        self.assertEqual(len(stretch["crossings"]), 1)
        self.assertNotIn("landing_from", stretch)
        self.assertIn("landing_to", stretch)


class WaterFormatTests(unittest.TestCase):
    """The stored format 3, built from the shared fixture with a river added beside its roads."""

    FIXTURE: ClassVar[Path] = Path(__file__).resolve().parents[1] / "crates/map/tests/network"

    def make(self, directory: str) -> tuple[Path, Path]:
        import shutil  # noqa: PLC0415  # Work on a copy of the shared fixture.

        copy = Path(directory) / "fixture"
        shutil.copytree(self.FIXTURE / "map", copy / "map")
        shutil.copytree(self.FIXTURE / "scenarios", copy / "scenarios")
        map_dir, scenario = copy / "map/fixture", copy / "scenarios/fixture"
        river = survey("sea-1", [(1, 8.5), (12, 8.5)], width=(30, 30))
        stretch = {**river, "points": [list(p) for p in river["points"]]}
        text = (
            "format = 2\n\n[tiers]\nshow_from_step = [0, 14, 24]\n\n"
            '[[river]]\nid = "sea"\nname = "Sea"\ntier = 3\n\n[[stretch]]\n'
            'id = "sea-1"\nriver = "sea"\nkind = "river"\nwidth_m = [30.0, 30.0]\nends = "sea"\n'
            f"points = {stretch['points']}\n"
        )
        (map_dir / "rivers.toml").write_text(text)
        network.build_network(map_dir, scenario)
        return map_dir, scenario

    def test_two_builds_give_the_same_bytes_and_ids(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make(directory)
            first = {n: (scenario / n).read_bytes() for n in ("roads.toml", "sea_distance.png")}
            network.build_network(map_dir, scenario)
            self.assertEqual({n: (scenario / n).read_bytes() for n in first}, first)
            network.check_network(map_dir, scenario)
            stored = tomllib.loads(first["roads.toml"].decode())
            self.assertEqual(stored["format"], 3)
            self.assertEqual(
                [(c["id"], c["bodies"]) for c in stored["water_class"]],
                [("barge", 3), ("currach", 3)],
            )
            self.assertEqual(stored["sea_distance"], "sea_distance.png")

    def test_a_build_that_fails_leaves_the_stored_network_the_layer_and_the_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make(directory)
            names = ("roads.toml", "sea_distance.png", "network-report.md")
            before = {n: (scenario / n).read_bytes() for n in names}
            hand = scenario / "network.toml"
            hand.write_text(hand.read_text().replace("reach_m = 1000", "reach_m = 900"))
            with (
                mock.patch.object(network, "network_report", side_effect=RuntimeError("fails")),
                self.assertRaisesRegex(RuntimeError, "fails"),
            ):
                network.build_network(map_dir, scenario)
            self.assertEqual({n: (scenario / n).read_bytes() for n in names}, before)
            original = Path.replace
            count = 0

            def interrupted(path: Path, target: Path) -> Path:
                nonlocal count
                count += 1
                if count == _WRITES_BEFORE_FAILURE:
                    raise OSError("injected write failure")
                return original(path, target)

            with (
                mock.patch.object(Path, "replace", interrupted),
                self.assertRaisesRegex(OSError, "injected"),
            ):
                network.build_network(map_dir, scenario)
            self.assertEqual({n: (scenario / n).read_bytes() for n in names}, before)

    def test_check_refuses_a_layer_that_is_missing_stale_malformed_or_changed(self) -> None:
        import io  # noqa: PLC0415  # Encode layers of other shapes.

        from PIL import Image  # noqa: PLC0415  # Pillow is a tool dependency.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make(directory)
            layer = scenario / "sea_distance.png"
            good = layer.read_bytes()
            values = network.read_sea_distance(good, 14, 10)

            def refuses(data: bytes | None, pattern: str) -> None:
                if data is None:
                    layer.unlink()
                else:
                    layer.write_bytes(data)
                with self.assertRaisesRegex(ValueError, pattern):
                    network.check_network(map_dir, scenario)
                layer.write_bytes(good)

            refuses(None, "missing")
            refuses(b"not a png", "malformed")
            eight = io.BytesIO()
            Image.new("L", (14, 10)).save(eight, format="PNG")
            refuses(eight.getvalue(), "sample format")
            other = io.BytesIO()
            Image.new("I;16", (13, 10)).save(other, format="PNG")
            refuses(other.getvalue(), "size")
            changed = [row[:] for row in values]
            changed[0][0] = 7
            refuses(network.sea_distance_png(changed), r"cell \(0, 0\) is 7")
            network.check_network(map_dir, scenario)

    def test_check_refuses_each_recorded_hash_when_its_file_changes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make(directory)
            rivers = map_dir / "rivers.toml"
            original = rivers.read_text()
            rivers.write_text(
                original + "# a comment changes the bytes and nothing else\n".replace("\\\n", "\n")
            )
            with self.assertRaisesRegex(ValueError, "rivers_sha256"):
                network.check_network(map_dir, scenario)
            rivers.write_text(original)
            from PIL import Image  # noqa: PLC0415  # Same pixels, other bytes.

            with Image.open(map_dir / "terrain.png") as image:
                image.convert("RGB").save(map_dir / "terrain.png", compress_level=1, optimize=False)
            with self.assertRaisesRegex(ValueError, "terrain_sha256"):
                network.check_network(map_dir, scenario)

    def test_check_refuses_a_river_graph_with_one_reach_changed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.make(directory)
            roads = scenario / "roads.toml"
            text = roads.read_text()
            self.assertIn("length_mm = ", text)
            mark = text.index("[[reach]]")
            line = text.index("length_mm = ", mark)
            end = text.index("\n".replace("\\\n", "\n"), line)
            roads.write_text(text[:line] + "length_mm = 1" + text[end:])
            with self.assertRaisesRegex(ValueError, "reach 0 differs"):
                network.check_network(map_dir, scenario)

    def test_the_report_gains_each_water_section_and_the_road_beside_water_with_no_node(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            _, scenario = self.make(directory)
            report = (scenario / "network-report.md").read_text()
            for heading in (
                "## Water classes",
                "## Bodies of water",
                "## Landings",
                "## Places without a landing",
                "## Shores refused",
                "## Crossing stretches by boat",
                "## Straits",
                "## Sea-going marks",
                "## Roads beside usable water with no node",
                "## Places joined by land and water",
            ):
                self.assertIn(heading, report)
            self.assertIn("- stretch 6: cell (3, 4)", report)

    def test_a_hand_list_with_a_river_width_for_a_class_is_refused(self) -> None:
        bad = {"water_class": [{**_BARGE, "river_width_m": 20}]}
        with self.assertRaisesRegex(ValueError, "water_class: unknown key river_width_m"):
            network.validate_water(bad, {})
        with self.assertRaisesRegex(ValueError, "water river_width_m"):
            network.validate_water({"water": {"river_width_m": -1}}, {})
        with self.assertRaisesRegex(ValueError, "mark q: repeated or unknown place"):
            network.validate_water({"mark": [{"place": "q"}]}, {"p": (0, 0)})
        with self.assertRaisesRegex(ValueError, "already in that cell"):
            network.validate_water(
                {
                    "river_break": [
                        {"stretch": "s", "cell": [1, 1], "name": "A"},
                        {"stretch": "s", "cell": [1, 1], "name": "B"},
                    ]
                },
                {},
            )


_WRITES_BEFORE_FAILURE = 3


class IrelandWaterTests(unittest.TestCase):
    """The statements of "It passes when", counted on the stored network of Ireland."""

    root: ClassVar[Path]
    graph: ClassVar[dict[str, Any]]
    layer: ClassVar[list[list[int]]]
    kinds: ClassVar[list[list[str]]]

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = Path(__file__).resolve().parents[1]
        scenario = cls.root / "assets/scenarios/ireland-768"
        cls.graph = tomllib.loads((scenario / "roads.toml").read_text())
        cls.kinds = network.read_rasters(cls.root / "assets/map/ireland")["kind"]
        data = (scenario / "sea_distance.png").read_bytes()
        cls.layer = network.read_sea_distance(data, len(cls.kinds[0]), len(cls.kinds))

    def landings_of(self, place: str, kind: str) -> list[dict[str, Any]]:
        nodes = [n["id"] for n in self.graph["node"] if n["kind"] == kind and n["place"] == place]
        return [x for x in self.graph["landing"] if x["node"] in nodes]

    def bodies(self, place: str, kind: str, number: int) -> set[int]:
        return {
            x["bodies"][number] for x in self.landings_of(place, kind) if x["bodies"][number] >= 0
        }

    def test_check_names_a_join_changed_between_a_cell_and_the_next_reach_and_does_not_crash(
        self,
    ) -> None:
        # The identity of a join holds its reach, the reach it runs on to and its cell, one of the last two absent. A join to a cell
        # changed into one to a reach (and back) leaves the two kinds of identity to be put in order together.
        import contextlib  # noqa: PLC0415  # Read what the command prints.
        import io  # noqa: PLC0415  # Catch the command's output.
        import shutil  # noqa: PLC0415  # Work on a copy of the shipped scenario.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = Path(directory) / "map", Path(directory) / "scenario"
            shutil.copytree(
                self.root / "assets/map/ireland",
                map_dir,
                ignore=shutil.ignore_patterns("__pycache__"),
            )
            shutil.copytree(self.root / "assets/scenarios/ireland-768", scenario)
            original = (scenario / "roads.toml").read_text()

            def checked() -> tuple[int, str]:
                errors = io.StringIO()
                with contextlib.redirect_stderr(errors), contextlib.redirect_stdout(io.StringIO()):
                    code = network.main(
                        ["--map-dir", str(map_dir), "--scenario", str(scenario), "check"]
                    )
                return code, errors.getvalue()

            self.assertEqual(checked(), (0, ""))
            graph = tomllib.loads(original)
            joins = graph["water_join"]
            to_cell = next(item for item in joins if "cell" in item)
            to_reach = next(item for item in joins if "to_reach" in item)
            for changed, other in ((to_cell, "to_reach"), (to_reach, "cell")):
                tampered = tomllib.loads(original)
                item = tampered["water_join"][joins.index(changed)]
                if other == "to_reach":
                    del item["cell"]
                    item["to_reach"] = item["reach"]
                else:
                    del item["to_reach"]
                    item["cell"] = [1, 1]
                (scenario / "roads.toml").write_text(network.dump_roads(tampered))
                code, errors = checked()
                self.assertEqual(code, 1, errors)
                self.assertRegex(errors, r"network: stored network water_join .* differs")
            (scenario / "roads.toml").write_text(original)
            self.assertEqual(checked(), (0, ""))

    def test_check_names_a_stored_identity_of_the_wrong_type_and_does_not_crash(self) -> None:
        # A table entry whose identity holds a string where the others hold a number is a difference to name, not a traceback.
        import contextlib  # noqa: PLC0415  # Read what the command prints.
        import io  # noqa: PLC0415  # Catch the command's output.
        import re  # noqa: PLC0415  # Edit the stored text of one entry.
        import shutil  # noqa: PLC0415  # Work on a copy of the shipped scenario.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = Path(directory) / "map", Path(directory) / "scenario"
            shutil.copytree(
                self.root / "assets/map/ireland",
                map_dir,
                ignore=shutil.ignore_patterns("__pycache__"),
            )
            shutil.copytree(self.root / "assets/scenarios/ireland-768", scenario)
            original = (scenario / "roads.toml").read_text()

            def checked() -> tuple[int, str]:
                errors = io.StringIO()
                with contextlib.redirect_stderr(errors), contextlib.redirect_stdout(io.StringIO()):
                    code = network.main(
                        ["--map-dir", str(map_dir), "--scenario", str(scenario), "check"]
                    )
                return code, errors.getvalue()

            self.assertEqual(checked(), (0, ""))
            for table, field in (("water_join", "reach"), ("landing", "id"), ("reach", "id")):
                # The text is edited, not the table: the tool's own writer would sort the entries and refuse the mix.
                tampered, count = re.subn(
                    rf"(\[\[{table}\]\]\n(?:[^\[]*?\n)?){field} = [^\n]+",
                    rf'\g<1>{field} = "broken"',
                    original,
                    count=1,
                )
                self.assertEqual(count, 1, (table, field))
                (scenario / "roads.toml").write_text(tampered)
                code, errors = checked()
                self.assertEqual(code, 1, (table, errors))
                self.assertRegex(
                    errors,
                    rf"network: stored network {table}\b.*(differs|malformed|not an integer)",
                )
                self.assertNotIn("Traceback", errors)
            (scenario / "roads.toml").write_text(original)
            self.assertEqual(checked(), (0, ""))

    def test_check_names_each_stored_id_or_counter_that_is_not_a_plain_integer(self) -> None:
        import contextlib  # noqa: PLC0415  # Read what the command prints.
        import io  # noqa: PLC0415  # Catch the command's output.
        import re  # noqa: PLC0415  # Edit the stored text of one entry.
        import shutil  # noqa: PLC0415  # Work on a copy of the shipped scenario.

        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = Path(directory) / "map", Path(directory) / "scenario"
            shutil.copytree(
                self.root / "assets/map/ireland",
                map_dir,
                ignore=shutil.ignore_patterns("__pycache__"),
            )
            shutil.copytree(self.root / "assets/scenarios/ireland-768", scenario)
            original = (scenario / "roads.toml").read_text()

            def checked() -> tuple[int, str]:
                output = io.StringIO()
                with contextlib.redirect_stderr(output), contextlib.redirect_stdout(io.StringIO()):
                    code = network.main(
                        ["--map-dir", str(map_dir), "--scenario", str(scenario), "check"]
                    )
                return code, output.getvalue()

            # The unchanged file passes; each change below is named by the table, the entry and the field with its type.
            self.assertEqual(checked(), (0, ""))
            graph = tomllib.loads(original)

            def edit(
                table: str,
                field: str,
                value: str,
                *,
                new_field: str | None = None,
                keep: bool = False,
            ) -> tuple[str, int]:
                """The stored text with the first entry of `table` that has `field` holding `value` (under `new_field`, and
                beside the old line when `keep`), and that entry's number."""
                number = next(i for i, item in enumerate(graph[table]) if field in item)
                lines, seen, found = original.split("\n"), -1, False
                for at, line in enumerate(lines):
                    if line == f"[[{table}]]":
                        seen += 1
                    elif seen == number and line.startswith(f"{field} = "):
                        new_line = f"{new_field or field} = {value}"
                        lines[at] = f"{line}\n{new_line}" if keep else new_line
                        found = True
                        break
                self.assertTrue(found, (table, field))
                return "\n".join(lines), number

            # Every case is a change to the stored text; the message names the table, the entry, the field and the type found.
            def case(  # noqa: PLR0913  # A case names its table, field, value and message, and may rename the key or keep the old line.
                table: str,
                field: str,
                value: str,
                message: str,
                new_field: str | None = None,
                keep: bool = False,
            ) -> tuple[str, str, str, str, str | None, bool]:
                return table, field, value, message, new_field, keep

            cases = [
                case("landing", "id", "[1]", "id is list, not an integer"),
                # A number the loader's type cannot hold: negative where it is unsigned, or past the width of its type.
                case("landing", "id", "-1", "id is -1, not an unsigned 64-bit integer"),
                case("node", "id", "-5", "id is -5, not an unsigned 64-bit integer"),
                case("stretch", "level", "256", "level is 256, not an unsigned 8-bit integer"),
                case(
                    "reach",
                    "begin",
                    "4294967296",
                    "begin is 4294967296, not an unsigned 32-bit integer",
                ),
                case("landing", "run", "-1", "run is -1, not an unsigned 32-bit integer"),
                case(
                    "joint",
                    "cell",
                    "[4294967296, 0]",
                    "cell[0] is 4294967296, not a signed 32-bit integer",
                ),
                case(
                    "landing",
                    "bodies",
                    "[0, 9223372036854775808]",
                    "bodies[1] is 9223372036854775808, not a signed 64-bit integer",
                ),
                case("landing", "id", "{ invalid = 1 }", "id is dict, not an integer"),
                case("landing", "id", '"broken"', "id is str, not an integer"),
                case("landing", "run", "[0]", "run is list, not an integer"),
                case("reach", "length_mm", "2026-01-01", "length_mm is date, not an integer"),
                case("reach", "length_mm", "1.5", "length_mm is float, not an integer"),
                case("reach", "usable", "1", "usable is int, not a boolean"),
                case("water_join", "reach", "1.0", "reach is float, not an integer"),
                case("joint", "cell", "[1]", "cell is list, not a list of 2"),
                case("stretch", "cells", "[[1, 2], [3]]", "cells[1] is list, not a list of 2"),
                case("stretch", "length_m", "true", "length_m is bool, not an integer"),
                case("strait", "classes", "[1]", "classes[0] is int, not a string"),
                case(
                    "link",
                    "stretches",
                    '[[[3], "forward"]]',
                    "stretches[0][0] is list, not an integer",
                ),
                case("link", "stretches", "[[3, 4]]", "stretches[0][1] is int, not a string"),
                # An unexpected key is refused by name, a date under it or not: the schema is the loader's, which rejects any other field.
                case(
                    "reach",
                    "length_mm",
                    "2026-01-01",
                    "length_mmm is not a stored field",
                    "length_mmm",
                    False,
                ),
                case(
                    "reach",
                    "length_mm",
                    "2026-01-01",
                    "surveyed_at is not a stored field",
                    "surveyed_at",
                    True,
                ),
                case(
                    "reach",
                    "length_mm",
                    "1",
                    "surveyed_at is not a stored field",
                    "surveyed_at",
                    True,
                ),
                case("landing", "id", "07:30:00", "extra is not a stored field", "extra", True),
            ]
            for table, field, value, message, new_field, keep in cases:
                tampered, number = edit(table, field, value, new_field=new_field, keep=keep)
                (scenario / "roads.toml").write_text(tampered)
                for command in ("check", "build"):
                    output = io.StringIO()
                    with (
                        contextlib.redirect_stderr(output),
                        contextlib.redirect_stdout(io.StringIO()),
                    ):
                        code = network.main(
                            ["--map-dir", str(map_dir), "--scenario", str(scenario), command]
                        )
                    self.assertEqual(code, 1, (command, table, field, output.getvalue()))
                    self.assertIn(
                        f"network: stored network {table} entry {number}: {message}",
                        output.getvalue(),
                    )
                    self.assertNotIn("Traceback", output.getvalue())
                    # A refused build leaves the stored file as it was changed, not repaired.
                    self.assertEqual((scenario / "roads.toml").read_text(), tampered)
            for field, value, message in (
                ("next_landing_id", '"broken"', "next_landing_id is str, not an integer"),
                ("next_landing_id", "100000.0", "next_landing_id is float, not an integer"),
                ("next_node_id", "true", "next_node_id is bool, not an integer"),
            ):
                tampered = re.sub(
                    rf"^{field} = .*$", f"{field} = {value}", original, count=1, flags=re.M
                )
                self.assertNotEqual(tampered, original)
                (scenario / "roads.toml").write_text(tampered)
                code, errors = checked()
                self.assertEqual(code, 1, (field, errors))
                self.assertIn(f"network: stored network {message}", errors)
                self.assertNotIn("Traceback", errors)
            (scenario / "roads.toml").write_text(original)
            self.assertEqual(checked(), (0, ""))

    def test_the_report_names_the_places_each_class_joins_and_those_it_does_not(self) -> None:
        report = (self.root / "assets/scenarios/ireland-768/network-report.md").read_text()
        section = report.split("## Places joined by land and water\n")[1]
        lines = section.splitlines()
        self.assertEqual(lines[0], "- barge: 118 of 125 places")
        self.assertTrue(lines[1].startswith("  - joined: aghaboe, ahenny, "))
        self.assertEqual(
            lines[2],
            "  - not joined: inishbofin, inishkea_north, inishmurray, killeany, rechru, "
            "skellig_michael, tory",
        )
        self.assertEqual(lines[3], "- currach: 125 of 125 places")
        self.assertEqual(lines[5], "  - not joined: none")
        # Every joined place is a scenario id and the two lists together are the 125 places.
        places = {n["place"] for n in self.graph["node"] if n["kind"] == "place"}
        for joined, missing in ((lines[1], lines[2]), (lines[4], lines[5])):
            ids = set(joined.split(": ", 1)[1].split(", "))
            ids |= set(missing.split(": ", 1)[1].split(", ")) - {"none"}
            self.assertEqual(ids, places)

    def test_the_stored_network_is_format_3_and_binds_the_files_it_was_built_from(self) -> None:
        import hashlib  # noqa: PLC0415  # The recorded hashes are of the files themselves.

        scenario = self.root / "assets/scenarios/ireland-768"
        for field, path in (
            ("terrain_sha256", self.root / "assets/map/ireland/terrain.png"),
            ("rivers_sha256", self.root / "assets/map/ireland/rivers.toml"),
            ("sea_distance_sha256", scenario / "sea_distance.png"),
        ):
            self.assertEqual(self.graph[field], hashlib.sha256(path.read_bytes()).hexdigest())
        self.assertEqual(self.graph["format"], 3)
        self.assertEqual([c["id"] for c in self.graph["water_class"]], ["barge", "currach"])
        self.assertEqual([b["name"] for b in self.graph["river_break"]], ["Assaroe", "Doonass"])
        self.assertEqual(self.graph.get("mark", []), [])

    def dry_adjacency(self, tidal: bool) -> defaultdict[_Node, set[_Node]]:
        """The nodes joined by land: stretches that need no boat (a ford at each wide crossing), tidal ones only when `tidal`."""
        adjacency: defaultdict[_Node, set[_Node]] = defaultdict(set)
        for stretch in self.graph["stretch"]:
            by_land = all(c["kind"] == "ford" for c in stretch.get("crossings", []))
            if by_land and (tidal or not stretch["tidal"]):
                adjacency[stretch["from"]].add(stretch["to"])
                adjacency[stretch["to"]].add(stretch["from"])
        return adjacency

    @staticmethod
    def reached(adjacency: defaultdict[_Node, set[_Node]], start: _Node) -> set[_Node]:
        seen, todo = {start}, [start]
        while todo:
            for nxt in adjacency[todo.pop()] - seen:
                seen.add(nxt)
                todo.append(nxt)
        return seen

    def places(self) -> dict[str, int]:
        return {n["place"]: n["id"] for n in self.graph["node"] if n["kind"] == "place"}

    def joined_places(self, number: int) -> set[str]:
        """The places joined to the mainland's largest piece of land by land, then by water for one class (counted afresh)."""
        adjacency = self.dry_adjacency(tidal=True)
        for landing in self.graph["landing"]:
            if landing["bodies"][number] >= 0:
                adjacency[landing["node"]].add(("body", landing["bodies"][number]))
                adjacency[("body", landing["bodies"][number])].add(landing["node"])
        boat = self.graph["water_class"][number]["id"]
        for strait in self.graph["strait"]:
            if boat in strait["classes"]:
                adjacency[strait["from"]].add(strait["to"])
                adjacency[strait["to"]].add(strait["from"])
        places = self.places()
        mainland = self.reached(adjacency, places["armagh"])
        return {place for place, node in places.items() if node in mainland}

    def test_all_125_places_are_joined_for_the_currach_and_the_barge_leaves_seven_out(self) -> None:
        places = self.places()
        self.assertEqual(len(places), 125)
        barge, currach = self.joined_places(0), self.joined_places(1)
        self.assertEqual(
            sorted(set(places) - barge),
            [
                "inishbofin",
                "inishkea_north",
                "inishmurray",
                "killeany",
                "rechru",
                "skellig_michael",
                "tory",
            ],
        )
        self.assertEqual((len(currach), len(barge)), (125, 118))
        # The committed report says the same.
        report = (self.root / "assets/scenarios/ireland-768/network-report.md").read_text()
        self.assertIn("- currach: 125 of 125 places", report)
        self.assertIn("- barge: 118 of 125 places", report)

    def test_by_land_alone_every_offshore_place_is_cut_off_and_only_the_tidal_stretches_join_two(
        self,
    ) -> None:
        places = self.places()
        access = {a["place"]: a for a in self.graph["access"]}
        offshore = {p for p, a in access.items() if a["quality"] == "offshore"}
        tidal_places = set(access) - offshore
        self.assertEqual(
            (len(offshore), sorted(tidal_places)), (22, ["illaunloughan", "inishkeel"])
        )
        armagh = places["armagh"]
        with_tidal = self.reached(self.dry_adjacency(tidal=True), armagh)
        without = self.reached(self.dry_adjacency(tidal=False), armagh)
        self.assertFalse({places[p] for p in offshore} & with_tidal)
        for place in tidal_places:
            self.assertIn(places[place], with_tidal, place)
            self.assertNotIn(places[place], without, place)

    def test_killeanys_road_reaches_its_landing_node_whose_currach_sea_landing_shares_its_body(
        self,
    ) -> None:
        places = self.places()
        access = {a["place"]: a for a in self.graph["access"]}
        landing_node = access["killeany"]["landing"]
        node = next(n for n in self.graph["node"] if n["id"] == landing_node)
        self.assertEqual(node["cell"], [163, 531])
        self.assertIn(landing_node, self.reached(self.dry_adjacency(tidal=True), places["armagh"]))
        currach_bodies = self.bodies("killeany", "place", 1)
        self.assertTrue(
            any(
                x.get("cell") is not None
                and self.kinds[x["cell"][1]][x["cell"][0]] == "sea"
                and x["bodies"][1] in currach_bodies
                for x in self.graph["landing"]
                if x["node"] == landing_node
            )
        )

    def test_ten_places_have_a_strait_and_no_other_place_has_one(self) -> None:
        self.assertEqual(
            sorted(s["place"] for s in self.graph["strait"]),
            [
                "church_island_currane",
                "church_island_valentia",
                "devenish",
                "hare_island",
                "illaunloughan",
                "inisfallen",
                "inishkeel",
                "inishmacsaint",
                "nendrum",
                "scattery",
            ],
        )
        self.assertTrue(
            all(
                s["length_m"] == _STRAIT_M and s["classes"] == ["barge", "currach"]
                for s in self.graph["strait"]
            )
        )

    def test_each_of_the_24_mainland_landing_nodes_has_a_landing_or_a_strait(self) -> None:
        straits = {s["place"] for s in self.graph["strait"]}
        landing_nodes = {x["node"] for x in self.graph["landing"]}
        self.assertEqual(len(self.graph["access"]), 24)
        for access in self.graph["access"]:
            self.assertTrue(
                access["landing"] in landing_nodes or access["place"] in straits, access["place"]
            )
        marsh = {
            "church_island_valentia",
            "inis_patraic",
            "inishkeel",
            "inishmurray",
            "killeany",
            "nendrum",
            "rechru",
            "spike_island",
            "tory",
        }
        by_place = {a["place"]: a["landing"] for a in self.graph["access"]}
        self.assertTrue(marsh <= set(by_place))
        for place in marsh - {"inishkeel", "nendrum", "church_island_valentia"}:
            self.assertIn(by_place[place], landing_nodes, place)

    def test_seven_island_places_share_a_barge_body_with_their_mainland_landing_and_seven_do_not(
        self,
    ) -> None:
        joined = [
            "white_island",
            "iniscealtra",
            "inchbofin",
            "inchcleraun",
            "inis_patraic",
            "spike_island",
            "inishglora",
        ]
        apart = [
            "inishkea_north",
            "rechru",
            "inishbofin",
            "inishmurray",
            "tory",
            "skellig_michael",
            "killeany",
        ]
        for place in joined:
            self.assertTrue(
                self.bodies(place, "place", 0) & self.bodies(place, "landing", 0), place
            )
        for place in apart:
            self.assertFalse(
                self.bodies(place, "place", 0) & self.bodies(place, "landing", 0), place
            )

    def test_the_barges_sea_is_11285_cells_in_23_pieces_and_the_largest_is_10416(self) -> None:
        sea = {
            (x, y)
            for y, row in enumerate(self.layer)
            for x, value in enumerate(row)
            if value <= _BARGE_REACH_M and self.kinds[y][x] == "sea"
        }
        self.assertEqual(len(sea), 11_285)
        pieces = self.pieces(sea)
        self.assertEqual((len(pieces), max(map(len, pieces))), (23, 10_416))
        self.assertEqual(
            sum(1 for row in self.layer for v in row if v <= _CURRACH_REACH_M and v != _LAYER_LAND),
            52_693,
        )

    @staticmethod
    def pieces(cells: set[tuple[int, int]]) -> list[set[tuple[int, int]]]:
        seen: set[tuple[int, int]] = set()
        result = []
        for start in sorted(cells):
            if start in seen:
                continue
            piece, stack = {start}, [start]
            seen.add(start)
            while stack:
                x, y = stack.pop()
                for nxt in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                    if nxt in cells and nxt not in seen:
                        seen.add(nxt)
                        piece.add(nxt)
                        stack.append(nxt)
            result.append(piece)
        return result

    def test_the_largest_piece_is_a_ring_round_the_island_and_holds_twelve_cells_apart(  # noqa: PLR0912  # The ring, its winding and the twelve removals are one statement.
        self,
    ) -> None:
        usable = {
            (x, y)
            for y, row in enumerate(self.layer)
            for x, value in enumerate(row)
            if self.kinds[y][x] == "lake" or (self.kinds[y][x] == "sea" and value <= _BARGE_REACH_M)
        }

        def moves(cell: tuple[int, int], within: set[tuple[int, int]]) -> list[tuple[int, int]]:
            x, y = cell
            found = [c for c in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)) if c in within]
            found += [
                (x + dx, y + dy)
                for dx, dy in ((1, 1), (1, -1), (-1, 1), (-1, -1))
                if (x + dx, y + dy) in within and (x + dx, y) in within and (x, y + dy) in within
            ]
            return found

        seen: set[tuple[int, int]] = set()
        big: set[tuple[int, int]] = set()
        for start in sorted(usable):
            if start in seen:
                continue
            piece, stack = {start}, [start]
            seen.add(start)
            while stack:
                for nxt in moves(stack.pop(), usable):
                    if nxt not in seen:
                        seen.add(nxt)
                        piece.add(nxt)
                        stack.append(nxt)
            if len(piece) > len(big):
                big = piece
        self.assertEqual(len(big & {c for c in usable if self.kinds[c[1]][c[0]] == "sea"}), 10_416)
        centre = (Fraction(400) + Fraction(1, 2), Fraction(530) + Fraction(1, 2))
        ray = centre[1] + Fraction(1, 4)

        def crossing(a: tuple[int, int], b: tuple[int, int]) -> int:
            ya, yb = a[1] + Fraction(1, 2), b[1] + Fraction(1, 2)
            if (ya < ray) == (yb < ray):
                return 0
            x = a[0] + Fraction(1, 2) + (ray - ya) / (yb - ya) * (b[0] - a[0])
            return 0 if x <= centre[0] else (1 if yb > ya else -1)

        start = min(big)
        seen_states = {(start, 0)}
        stack2 = [(start, 0)]
        winding = 0
        while stack2 and not winding:
            cell, turns = stack2.pop()
            for nxt in moves(cell, big):
                state = (nxt, turns + crossing(cell, nxt))
                if state not in seen_states:
                    seen_states.add(state)
                    stack2.append(state)
                    if nxt == start and state[1]:
                        winding = state[1]
        self.assertNotEqual(winding, 0)
        witnesses = (
            (476, 37),
            (649, 88),
            (663, 338),
            (646, 567),
            (603, 761),
            (405, 825),
            (183, 923),
            (37, 790),
            (115, 496),
            (86, 364),
            (135, 283),
            (290, 182),
        )
        for cell in witnesses:
            self.assertIn(cell, big)
        # With the piece's cells within six cells of any one of them taken away, the other eleven are still joined: by distance in
        # a circle and in a square, the spec saying "within six cells" of a cell and not which.
        for witness in witnesses:
            others = [cell for cell in witnesses if cell != witness]
            for square in (False, True):
                near = {
                    cell
                    for cell in big
                    if (
                        max(abs(cell[0] - witness[0]), abs(cell[1] - witness[1])) <= _WITHIN_CELLS
                        if square
                        else (cell[0] - witness[0]) ** 2 + (cell[1] - witness[1]) ** 2
                        <= _WITHIN_CELLS**2
                    )
                }
                rest = big - near
                joined = {others[0]}
                todo = [others[0]]
                while todo:
                    for nxt in moves(todo.pop(), rest):
                        if nxt not in joined:
                            joined.add(nxt)
                            todo.append(nxt)
                self.assertTrue(set(others) <= joined, (witness, square))

    def test_the_erne_and_the_shannon_are_different_bodies_above_and_below_their_breaks(
        self,
    ) -> None:
        cells = len(self.kinds) * len(self.kinds[0])
        width = len(self.kinds[0])
        joints, reaches = self.graph["joint"], {r["id"]: r for r in self.graph["reach"]}
        for number, item in enumerate(self.graph["water_class"]):
            sets = network._Sets(cells + len(joints))

            def open_cell(x: int, y: int, reach: int = item["reach_m"]) -> bool:
                return (
                    0 <= y < len(self.kinds)
                    and 0 <= x < width
                    and (
                        self.kinds[y][x] == "lake"
                        or (self.kinds[y][x] == "sea" and self.layer[y][x] <= reach)
                    )
                )

            for y in range(len(self.kinds)):
                for x in range(width):
                    if open_cell(x, y):
                        if open_cell(x + 1, y):
                            sets.union(y * width + x, y * width + x + 1)
                        if open_cell(x, y + 1):
                            sets.union(y * width + x, (y + 1) * width + x)
            for reach in self.graph["reach"]:
                if reach["usable"]:
                    sets.union(cells + reach["from"], cells + reach["to"])
            for join in self.graph["water_join"]:
                if "to_reach" in join:
                    sets.union(
                        cells + reaches[join["reach"]]["to"],
                        cells + reaches[join["to_reach"]]["from"],
                    )
                elif item["id"] in join["classes"]:
                    sets.union(
                        cells + reaches[join["reach"]]["from"],
                        join["cell"][1] * width + join["cell"][0],
                    )
            for item_break in self.graph["river_break"]:
                self.assertNotEqual(
                    sets.find(cells + item_break["above"]),
                    sets.find(cells + item_break["below"]),
                    (number, item_break),
                )


class RoadsideReportTests(unittest.TestCase):
    """Rule 22: the road stretches beside usable water at an allowed shore away from any node, where nothing can embark."""

    @staticmethod
    def gap(
        rows: list[str],
        rivers: list[dict[str, Any]],
        *,
        node_cells: tuple[tuple[int, int], tuple[int, int]] = ((0, 1), (2, 1)),
        tidal: bool = False,
    ) -> list[str]:
        cells = [(node_cells[0][0], 1), (1, 1), (node_cells[1][0], 1)]
        graph = water_graph(
            [(1, "junction", node_cells[0], None), (2, "junction", node_cells[1], None)],
            stretches=[
                {
                    "id": 10,
                    "from": 1,
                    "to": 2,
                    "tidal": tidal,
                    "cells": [list(c) for c in cells],
                }
            ],
        )
        graph, plan = work_water(water_context(rows, rivers), graph)
        report = network.water_report(graph, plan)
        start = report.index("## Roads beside usable water with no node") + 1
        end = next((i for i in range(start, len(report)) if not report[i]), len(report))
        section: list[str] = report[start:end]
        return section

    def test_an_allowed_lakeside_is_listed(self) -> None:
        self.assertEqual(self.gap([".l.", "...", "..."], []), ["- stretch 10: cell (1, 1)"])

    def test_a_usable_river_run_in_the_middle_cell_is_listed(self) -> None:
        river = survey("r-1", [(1.1, 1.25), (1.9, 1.25)])
        self.assertEqual(self.gap(["...", "...", "..."], [river]), ["- stretch 10: cell (1, 1)"])

    def test_a_marsh_shore_is_not_listed(self) -> None:
        self.assertEqual(self.gap([".l.", ".m.", "..."], []), ["- none"])

    def test_a_high_shore_is_not_listed(self) -> None:
        self.assertEqual(self.gap([".l.", ".h.", "..."], []), ["- none"])

    def test_a_marsh_cell_with_a_river_run_is_not_listed(self) -> None:
        river = survey("r-1", [(1.1, 1.25), (1.9, 1.25)])
        self.assertEqual(self.gap(["...", ".m.", "..."], [river]), ["- none"])

    def test_the_cell_of_a_node_and_a_tidal_stretch_are_not_listed(self) -> None:
        self.assertEqual(
            self.gap([".l.", "...", "..."], [], node_cells=((1, 1), (2, 1))), ["- none"]
        )
        self.assertEqual(self.gap([".l.", "...", "..."], [], tidal=True), ["- none"])


_RIVER_CASES: dict[str, tuple[list[str], dict[str, Any]]] = {
    "narrowing": (["....", ".l..", "...."], survey("r-1", [(0, 1.5), (3, 1.5)], width=(21, 19))),
    "widening": (["....", ".l..", "...."], survey("r-1", [(0, 1.5), (3, 1.5)], width=(19, 21))),
    "three_lakes": (["......", ".lll..", "......"], survey("r-1", [(0, 1.5), (6, 1.5)])),
    # A river widening from 19 m so that it reaches 20 m a thousandth of the stretch (997/3000 of it) before the lake cell it then
    # enters (the entry is at a third of the stretch), and one a thousandth after the entry (1003/3000), inside the cell.
    "usable_just_before": (
        ["....", ".l..", "...."],
        survey("r-1", [(0, 1.5), (3, 1.5)], width=(19.0, 22.00902708124373)),
    ),
    "usable_just_after": (
        ["....", ".l..", "...."],
        survey("r-1", [(0, 1.5), (3, 1.5)], width=(19.0, 21.99102691924227)),
    ),
}

_CROSSING_BREAK_RIVER = survey("r-1", [(2.2, 1.001684), (2.2, 3)])
_CROSSING_BREAK = [{"stretch": "r-1", "cell": [2, 1], "name": "Falls"}]


def crossing_break_case() -> tuple[str, str]:
    """The stored network for slice 3a's four-cell road over a river 0.4 m below a break, and the world it was made for."""
    _, graph, plan = crossing_world(
        CrossingEndTests.ROWS,
        [_CROSSING_BREAK_RIVER],
        CrossingEndTests.ROAD,
        [("r", (2, 1), "none")],
        extra={"river_break": _CROSSING_BREAK},
    )
    graph.update({"format": 3, "hand_list_sha256": "x", "baseline_sha256": "x"})
    # The road's one link runs over its three stretches, the crossing between two banks among them.
    graph["link"] = [
        {
            "id": "x",
            "from_node": 1,
            "to_node": 2,
            "stretches": [[stretch["id"], "forward"] for stretch in graph["stretch"]],
            "record": "{}",
        }
    ]
    points = [(round(x * 32768), round(y * 32768)) for x, y in plan["lines"]["r-1"].line.points]
    layer = [value for row in plan["layer_values"] for value in row]
    world = (
        f"rows = [{', '.join(json.dumps(row) for row in CrossingEndTests.ROWS)}]\n"
        f'layer = {layer}\nriver = "r-1"\npoints = {[list(point) for point in points]}\n'
    )
    return network.dump_roads(graph), world


def river_case(name: str) -> tuple[str, str]:
    """The stored network the tool makes for one small river world, and the world it was made for (rows, layer, line)."""
    rows, river = _RIVER_CASES[name]
    context = water_context(rows, [river])
    graph, plan = work_water(context)
    graph.update({"format": 3, "hand_list_sha256": "x", "baseline_sha256": "x"})
    points = [(round(x * 32768), round(y * 32768)) for x, y in plan["lines"]["r-1"].line.points]
    layer = [value for row in plan["layer_values"] for value in row]
    world = (
        f"rows = [{', '.join(json.dumps(row) for row in rows)}]\n"
        f'layer = {layer}\nriver = "r-1"\npoints = {[list(point) for point in points]}\n'
    )
    return network.dump_roads(graph), world


class RustRiverCaseTests(unittest.TestCase):
    """The river cases of the spec that the Rust loader's tests read are the tool's own output, stored."""

    ROOT: ClassVar[Path] = Path(__file__).resolve().parents[1]

    def test_the_committed_river_worlds_are_what_the_tool_makes(self) -> None:
        for name in _RIVER_CASES:
            roads, world = river_case(name)
            directory = self.ROOT / "crates/map/tests/water"
            self.assertEqual((directory / f"{name}.toml").read_text(), roads, name)
            self.assertEqual((directory / f"{name}.world.toml").read_text(), world, name)

    def test_the_tool_stores_each_case_as_the_loader_tests_assume(self) -> None:
        # The literals of the loader's tests, read from the tool's own graph: joints by cell, reaches by beginning, end and use, joins.
        expected = {
            "narrowing": (
                [[0, 1], [1, 1], [1, 1], [2, 1]],
                [(0, 333_333, True), (333_333, 500_000, True), (500_000, 1_000_000, False)],
                [(1, [1, 1])],
            ),
            "widening": (
                None,
                [(0, 500_000, False), (500_000, 1_000_000, True)],
                [(1, [1, 1])],
            ),
            "three_lakes": (
                None,
                [
                    (0, 166_667, True),
                    (166_667, 333_333, True),
                    (333_333, 500_000, True),
                    (500_000, 1_000_000, True),
                ],
                [(1, [1, 1]), (2, [2, 1]), (3, [3, 1])],
            ),
            "usable_just_before": (
                [[0, 1], [0, 1], [1, 1], [2, 1]],
                [(0, 332_333, False), (332_333, 333_333, True), (333_333, 1_000_000, True)],
                [(2, [1, 1])],
            ),
            "usable_just_after": (
                [[0, 1], [1, 1], [2, 1]],
                [(0, 334_333, False), (334_333, 1_000_000, True)],
                [(1, [1, 1])],
            ),
        }
        for name, (joint_cells, reach_spans, joins) in expected.items():
            graph = tomllib.loads(river_case(name)[0])
            if joint_cells is not None:
                self.assertEqual([j["cell"] for j in graph["joint"]], joint_cells, name)
            self.assertEqual(spans(graph, "r-1"), reach_spans, name)
            self.assertEqual([(j["reach"], j["cell"]) for j in graph["water_join"]], joins, name)

    def test_a_threshold_a_thousandth_from_a_joint_builds_reaches_of_some_length_and_one_body(
        self,
    ) -> None:
        for name in ("usable_just_before", "usable_just_after"):
            roads, _ = river_case(name)
            graph = tomllib.loads(roads)
            self.assertTrue(all(r["length_mm"] > 0 for r in graph["reach"]), name)
            self.assertEqual([c["bodies"] for c in graph["water_class"]], [1, 1], name)
            # Built again from the same inputs, the tool stores the same bytes, as `check` compares them.
            self.assertEqual(river_case(name)[0], roads, name)
        before = tomllib.loads(river_case("usable_just_before")[0])
        # The joint where the river becomes usable and the one at the lake cell's entry are a thousandth of the stretch apart (1,000
        # millionths):
        # two joints, 1,424 mm between them.
        self.assertEqual([r["length_mm"] for r in before["reach"]][1], 1424)

    def command_fixture(self, root: Path, name: str) -> tuple[Path, Path]:
        """The command fixture with the river case's lake cell and river in its map, ready for build, accept and check."""
        from PIL import Image  # noqa: I001, PLC0415  # Paint the lake cell into the disposable fixture's terrain.

        map_dir, scenario = CommandTests("make_fixture").make_fixture(root)
        with (map_dir / "terrain.toml").open("a") as stream:
            stream.write('[[terrain]]\nname = "lake"\nkind = "lake"\nkey = "040506"\n')
        terrain = Image.new("RGB", (4, 3), (1, 2, 3))
        terrain.putpixel((1, 1), (4, 5, 6))
        terrain.save(map_dir / "terrain.png")
        river = _RIVER_CASES[name][1]
        points = ", ".join(f"[{lat!r}, {lon!r}]" for lat, lon in river["points"])
        (map_dir / "rivers.toml").write_text(
            f'stretch = [{{ id = "r-1", river = "r", kind = "river", width_m = {river["width_m"]}, '
            f'ends = "sea", points = [{points}] }}]\n'
        )
        with (scenario / "network.toml").open("a") as stream:
            stream.write(
                "\n[water]\nriver_width_m = 20\nshore_height_m = 10\nstrait_m = 500\n"
                '\n[[water_class]]\nid = "barge"\nreach_m = 1000\nsea_going = false\n'
                '\n[[water_class]]\nid = "currach"\nreach_m = 5000\nsea_going = true\n'
            )
        return map_dir, scenario

    def test_the_near_joint_rivers_are_built_stored_checked_and_loaded_to_the_same_graph(
        self,
    ) -> None:
        expected = {
            "usable_just_before": [
                (0, 332_333, False),
                (332_333, 333_333, True),
                (333_333, 1_000_000, True),
            ],
            "usable_just_after": [(0, 334_333, False), (334_333, 1_000_000, True)],
        }
        for name, spans_expected in expected.items():
            with tempfile.TemporaryDirectory() as directory:
                map_dir, scenario = self.command_fixture(Path(directory), name)
                network.build_network(map_dir, scenario, importing=True)
                network.accept_pending(
                    scenario / "courses-pending.json",
                    network.input_files(map_dir, scenario, include_seed=True),
                    scenario / "network-report.md",
                    scenario / "network-changes.md",
                    "fixture approval",
                )
                stored = tomllib.loads((scenario / "roads.toml").read_text())
                # Built by the tool from the survey and the terrain, the river stores the literal reaches the Rust tests read.
                self.assertEqual(spans(stored, "r-1"), spans_expected, name)
                self.assertTrue(all(r["length_mm"] > 0 for r in stored["reach"]), name)
                network.build_network(map_dir, scenario)
                # The normal stored-network check passes on the stored file...
                network.check_network(map_dir, scenario)
                # ...and refuses it with one reach changed, naming the reach.
                original = (scenario / "roads.toml").read_text()
                tampered = tomllib.loads(original)
                tampered["reach"][1]["length_mm"] += 1
                (scenario / "roads.toml").write_text(network.dump_roads(tampered))
                with self.assertRaisesRegex(ValueError, r"stored network reach \d+ differs"):
                    network.check_network(map_dir, scenario)
                (scenario / "roads.toml").write_text(original)
                network.check_network(map_dir, scenario)

    def test_an_id_counter_at_the_end_of_u64_refuses_the_build_naming_the_counter(self) -> None:
        import re  # noqa: PLC0415  # Edit the stored counter and entry.

        largest = 2**64 - 1
        # Every allocator names its own counter and refuses the id after the last one the loader can read.
        for counter in ("next_node_id", "next_stretch_id", "next_landing_id"):
            with self.assertRaisesRegex(
                ValueError, rf"{counter} is {largest}: the next id would leave"
            ):
                network.allocate_ids([("a",)], {}, largest, counter)
            ids, following = network.allocate_ids([("a",)], {}, largest - 1, counter)
            self.assertEqual((ids, following), ({("a",): largest - 1}, largest))
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.command_fixture(Path(directory), "usable_just_before")
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture approval",
            )
            stored = (scenario / "roads.toml").read_text()
            # A stored network that has lost a landing must hand out a new one: at the last counter it refuses, and writes nothing.
            without = re.sub(r"\[\[landing\]\]\n(?:(?!\[\[)[^\n]*\n)*", "", stored, count=1)
            self.assertNotEqual(without, stored)
            at_end = re.sub(r"next_landing_id = \d+", f"next_landing_id = {largest}", without)
            (scenario / "roads.toml").write_text(at_end)
            before = {p: p.read_bytes() for p in scenario.iterdir() if p.is_file()}
            with self.assertRaisesRegex(ValueError, "next_landing_id is 18446744073709551615"):
                network.build_network(map_dir, scenario)
            self.assertEqual({p: p.read_bytes() for p in scenario.iterdir() if p.is_file()}, before)
            # One before the end it builds, and the counter it writes is the last one the loader reads.
            almost = re.sub(r"next_landing_id = \d+", f"next_landing_id = {largest - 1}", without)
            (scenario / "roads.toml").write_text(almost)
            network.build_network(map_dir, scenario)
            self.assertIn(f"next_landing_id = {largest}\n", (scenario / "roads.toml").read_text())

    def test_a_node_id_near_the_end_of_u64_builds_and_checks_and_joins_places_by_node_count(
        self,
    ) -> None:
        import re  # noqa: PLC0415  # Rewrite one node's id wherever the stored network names it.

        largest = 2**64 - 1
        with tempfile.TemporaryDirectory() as directory:
            map_dir, scenario = self.command_fixture(Path(directory), "usable_just_before")
            network.build_network(map_dir, scenario, importing=True)
            network.accept_pending(
                scenario / "courses-pending.json",
                network.input_files(map_dir, scenario, include_seed=True),
                scenario / "network-report.md",
                scenario / "network-changes.md",
                "fixture approval",
            )
            stored = (scenario / "roads.toml").read_text()
            graph = tomllib.loads(stored)
            old = max(node["id"] for node in graph["node"])
            new = largest - 1
            # The same network with its highest node numbered as the loader's u64 allows: the node's own entry, and every stretch end
            # and landing that names it.
            fields = {
                "node": ("id",),
                "stretch": ("from", "to"),
                "link": ("from_node", "to_node"),
                "landing": ("node",),
                "access": ("landing",),
                "strait": ("from", "to"),
            }
            table, lines = "", []
            for line in stored.split("\n"):
                header = re.match(r"^\[\[(\w+)\]\]$", line)
                if header:
                    table = header.group(1)
                field = re.match(r"^(\w+) = (\d+)$", line)
                renumbered = (
                    field and field.group(1) in fields.get(table, ()) and int(field.group(2)) == old
                )
                lines.append(f"{field.group(1)} = {new}" if renumbered and field else line)
            text = "\n".join(lines)
            text = re.sub(r"next_node_id = \d+", f"next_node_id = {largest}", text)
            self.assertNotEqual(text, stored)
            sparse = tomllib.loads(text)
            self.assertIn(new, [node["id"] for node in sparse["node"]])
            (scenario / "roads.toml").write_text(text)
            # Both commands read it, and the report joins the places as it did: connectivity is bounded by the node count.
            network.check_network(map_dir, scenario)
            network.build_network(map_dir, scenario)
            self.assertEqual((scenario / "roads.toml").read_text(), text)

            def joined_lines(document: dict[str, Any]) -> list[str]:
                full = {"strait": [], "landing": [], "stretch": [], **document}
                nodes = {node["id"]: node for node in full["node"]}
                joined: list[str] = network.joined_places(
                    full, {"names": ["barge", "currach"]}, nodes
                )
                return joined

            joined, dense = joined_lines(sparse), joined_lines(graph)
            self.assertEqual(joined, dense)

    def test_the_crossing_0_4_metres_from_a_break_lands_at_one_joint_with_a_400_mm_reach_to_the_break(
        self,
    ) -> None:
        graph = tomllib.loads(crossing_break_case()[0])
        [cut] = graph["river_break"]
        [crossing] = crossing_stretches(graph)
        ends = {x["end"]: x for x in graph["landing"] if x.get("crossing") == crossing["id"]}
        # Both ends land at joint 1; the reach from it to the break's upstream joint is 0.4 m, stored as 400 mm.
        self.assertEqual((ends["from"]["joint"], ends["to"]["joint"]), (1, 1))
        self.assertEqual((cut["above"], cut["below"]), (2, 3))
        [short] = [r for r in graph["reach"] if r["to"] == cut["above"]]
        self.assertEqual((short["from"], short["length_mm"], short["usable"]), (1, 400, True))
        self.assertEqual((short["begin"], short["end"]), (249_368, 249_789))
        self.assertEqual(ends["from"]["bodies"], ends["to"]["bodies"])

    def test_the_committed_crossing_break_world_is_what_the_tool_makes(self) -> None:
        roads, world = crossing_break_case()
        directory = self.ROOT / "crates/map/tests/water"
        self.assertEqual((directory / "crossing_break.toml").read_text(), roads)
        self.assertEqual((directory / "crossing_break.world.toml").read_text(), world)

    def test_a_reach_0_4_metres_long_between_two_joints_is_stored_as_400_mm_and_found_by_the_loader(
        self,
    ) -> None:
        import re  # noqa: PLC0415  # Read the stored numbers out of the Rust test's source.

        context = water_context(["..."], [survey("r-1", [(0, 0.5), (3, 0.5)])])
        line = network.WaterLine(
            network.RiverLine(context["rivers"][0], _ORIGIN), network.Fraction(20), []
        )
        side = network.cell_side_um(0, _ORIGIN) / 1e6
        start = network.Fraction(1_000_000)
        end = start + network.Fraction(0.4 / side * 1_000_000)
        graph = network.build_river_graph(
            {"r-1": line}, context["layers"]["kind"], _ORIGIN, {("r-1", start, 0), ("r-1", end, 0)}
        )
        # Four joints (the two ends of the stretch and the two near each other) and so three reaches, the middle one of 400 mm.
        self.assertEqual(len(graph["joints"]), 4)
        middle = graph["reaches"][1]
        self.assertEqual([r["length_mm"] for r in graph["reaches"]], [474_590, 400, 948_779])
        self.assertEqual(
            (middle["length_mm"], middle["from"], middle["to"], middle["usable"]),
            (400, 1, 2, True),
        )
        total = Fraction(line.line.total)
        begin, finish = (network._micro(start, total), network._micro(end, total))
        source = (self.ROOT / "crates/map/src/water_tests.rs").read_text()
        found = re.search(r"const NEAR_CUT: \[u32; 2\] = \[([\d_]+), ([\d_]+)\];", source)
        assert found is not None
        self.assertEqual(
            [int(found.group(1).replace("_", "")), int(found.group(2).replace("_", ""))],
            [begin, finish],
        )
        self.assertEqual((middle["begin"], middle["end"]), (begin, finish))


class RustWaterWorldTests(unittest.TestCase):
    """The stored world that the Rust loader's tests read is the one the tool makes."""

    ROOT: ClassVar[Path] = Path(__file__).resolve().parents[1]

    @staticmethod
    def world() -> tuple[str, dict[str, Any], dict[str, Any]]:
        rows = ["~~~~~~~~~~", "~........~", "~..l.....~", "~........~", "~........~", "~~~~~~~~~~"]
        river = survey("r-1", [(3.2, 1.2), (3.2, 4.8)], width=(30, 30))
        cells = [(1, 3), (2, 3), (3, 3), (4, 3), (5, 3), (6, 3)]
        breaks = [{"stretch": "r-1", "cell": [3, 4], "name": "Falls"}]
        context = water_context(
            rows, [river], extra={"river_break": breaks, "mark": [{"place": "isle"}]}
        )
        graph = water_graph(
            [
                (1, "place", cells[0], "a"),
                (2, "place", cells[-1], "b"),
                (3, "place", (7, 1), "isle"),
                (4, "landing", (8, 1), "isle"),
            ],
            [("isle", "offshore", 4)],
            stretches=[
                {
                    "id": 3,
                    "from": 1,
                    "to": 2,
                    "cells": [list(c) for c in cells],
                    "length_m": 3000,
                    "category": "road",
                    "level": 1,
                    "tidal": False,
                }
            ],
        )
        graph["link"] = [
            {"id": "x", "from_node": 1, "to_node": 2, "stretches": [[3, "forward"]], "record": "{}"}
        ]
        entries = [{"river": "r", "at": [3, 3], "kind": "none", "note": "t"}]
        hits = network.apply_crossings(graph, context["index"], entries, _ORIGIN, {"r"})
        graph, plan = work_water(context, graph, hits)
        graph.update({"format": 3, "hand_list_sha256": "x", "baseline_sha256": "x"})
        return network.dump_roads(graph), plan, graph

    def test_the_committed_stored_world_is_what_the_tool_makes(self) -> None:
        text, _, _ = self.world()
        self.assertEqual((self.ROOT / "crates/map/tests/water/world.toml").read_text(), text)

    def test_the_layer_and_the_river_in_the_rust_tests_are_the_tools_own(self) -> None:
        import re  # noqa: PLC0415  # Read the constants out of the Rust test's source.

        _, plan, _ = self.world()
        source = (self.ROOT / "crates/map/src/water_tests.rs").read_text()
        layer = re.search(r"const LAYER: \[u16; \d+\] = \[(.*?)\];", source, re.S)
        river = re.search(r"const RIVER: \[\(i32, i32\); \d+\] = \[(.*?)\];", source, re.S)
        assert layer is not None
        assert river is not None
        self.assertEqual(
            [int(v) for v in re.findall(r"\d+", layer.group(1))],
            [v for row in plan["layer_values"] for v in row],
        )
        points = [(round(x * 32768), round(y * 32768)) for x, y in plan["lines"]["r-1"].line.points]
        self.assertEqual(
            [int(v.replace("_", "")) for v in re.findall(r"[\d_]+", river.group(1))],
            [c for point in points for c in point],
        )
