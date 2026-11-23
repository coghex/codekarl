"""Contract tests for the travel network tool."""

import importlib.util
import tempfile
import unittest
from collections import defaultdict
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
        text = network.network_report(graph, links, [], Index(), (20000, 15000), proposed=True)
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
            "link": [{"id": "a", "stretches": [[1, "forward"]], "record": "seed"}],
            "join": [],
            "access": [],
        }
        text = network.network_report(
            graph, [{"id": "a", "category": "road", "level": 1}], [], Index(), (20000, 15000)
        )
        self.assertEqual(text.count("river,"), 1)

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
    def test_first_preview_has_four_candidates_and_connected_land(self) -> None:
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
            {"dala-2", "path-iniscealtra", "path-ciarraige_seat", "path-tory"},
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
            changed, {"dala-2", "path-iniscealtra", "path-ciarraige_seat", "path-tory"}
        )
        self.assertEqual(len(seed["completions"]), 12)

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
