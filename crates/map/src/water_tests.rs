//! The water of the stored network (format 3), loaded from the tool's own output for a small world and refused when any table
//! disagrees with what the loader works out for itself.

use crate::network::LandNetwork;
use crate::water::{Inputs, build};
use crate::{
    Cell, Crop, FixedPoint, LoadError, RiverNetwork, RiverStretch, RiverStretchKind, Surface, Water,
};

/// The stored network the tool makes for the world below (`tests/test_map_network.py` makes it again and compares).
const WORLD: &str = include_str!("../tests/water/world.toml");
const ROWS: [&str; 6] = [
    "~~~~~~~~~~",
    "~........~",
    "~..l.....~",
    "~........~",
    "~........~",
    "~~~~~~~~~~",
];
/// The sea-distance layer of that world, as the tool works it (a sea cell's metres; 65,535 elsewhere).
const LAYER: [u16; 60] = [
    671, 475, 475, 475, 475, 475, 475, 475, 475, 671, 475, 65535, 65535, 65535, 65535, 65535,
    65535, 65535, 65535, 475, 475, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 475,
    475, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 65535, 475, 475, 65535, 65535, 65535,
    65535, 65535, 65535, 65535, 65535, 475, 672, 475, 475, 475, 475, 475, 475, 475, 475, 672,
];
/// The river `r-1` of that world: one line from (3.2, 1.2) to (3.2, 4.8), in 1/32768 cell units.
const RIVER: [(i32, i32); 2] = [(104_858, 39_322), (104_858, 157_286)];

fn surfaces(rows: &[&str]) -> Vec<Surface> {
    rows.iter()
        .flat_map(|row| row.chars())
        .map(|c| match c {
            '~' => Surface::Sea { depth: 10 },
            'l' => Surface::Lake,
            _ => Surface::Land {
                terrain: 0,
                usable: true,
            },
        })
        .collect()
}

fn river_network(points: &[(i32, i32)], id: &str) -> RiverNetwork {
    RiverNetwork {
        show_from_step: [0, 14, 24],
        rivers: Vec::new(),
        stretches: vec![RiverStretch {
            id: id.to_owned(),
            river: 0,
            kind: RiverStretchKind::River,
            points: points.iter().map(|&(x, y)| FixedPoint { x, y }).collect(),
            width_first: 0,
            width_last: 0,
            ends: None,
            drawn_first: 0,
            drawn_last: 0,
        }],
    }
}

fn crop(rows: &[&str]) -> Crop {
    Crop {
        origin: Cell { x: 0, y: 0 },
        width: u32::try_from(rows[0].len()).unwrap(),
        height: u32::try_from(rows.len()).unwrap(),
    }
}

fn network(text: &str) -> LandNetwork {
    toml::from_str(text).unwrap()
}

fn load(text: &str) -> Result<(LandNetwork, Water), LoadError> {
    let n = network(text);
    let cells = surfaces(&ROWS);
    let rivers = river_network(&RIVER, "r-1");
    let water = build(
        &n,
        &Inputs {
            crop: crop(&ROWS),
            cells: &cells,
            layer: &LAYER,
            rivers: Some(&rivers),
        },
    )?;
    Ok((n, water))
}

fn refused(text: &str) -> String {
    load(text).unwrap_err().to_string()
}

#[test]
fn the_stored_world_loads_and_its_bodies_are_the_loaders_own() {
    let (n, water) = load(WORLD).unwrap();
    assert_eq!(n.classes.len(), 2);
    for class in 0..2 {
        assert_eq!(water.body_count(class), 3, "class {class}");
        // The sea round the island, the lake with the river above the break, and the river below it.
        let sea = water.cell_body(class, [0, 0]).unwrap();
        let lake = water.cell_body(class, [3, 2]).unwrap();
        assert_ne!(sea, lake);
        assert_eq!(water.cell_body(class, [9, 5]), Some(sea));
        assert_eq!(water.cell_body(class, [4, 2]), None, "land is no body");
        for joint in 0..3 {
            assert_eq!(water.joint_body(class, joint), Some(lake), "joint {joint}");
        }
        let below = water.joint_body(class, 4).unwrap();
        assert_ne!(below, lake);
        assert_eq!(water.joint_body(class, 5), Some(below));
        assert!(water.cell_usable(class, [3, 2]));
        assert!(!water.cell_usable(class, [4, 2]));
    }
}

#[test]
fn a_break_cuts_the_river_and_the_two_sides_are_two_joints_of_one_stretch() {
    let (n, _) = load(WORLD).unwrap();
    let falls = &n.breaks[0];
    assert_eq!(
        (falls.name.as_str(), falls.stretch.as_str(), falls.cell),
        ("Falls", "r-1", [3, 4])
    );
    assert_eq!(falls.below, falls.above + 1);
    // No reach has the break's two joints: the line is cut there.
    assert!(
        n.reaches
            .iter()
            .all(|r| !(r.from == falls.above && r.to == falls.below))
    );
}

#[test]
fn a_position_just_above_and_just_below_a_break_meet_at_the_break() {
    let (n, water) = load(WORLD).unwrap();
    let falls = &n.breaks[0];
    let above = n.reaches.iter().find(|r| r.to == falls.above).unwrap();
    let below = n.reaches.iter().find(|r| r.from == falls.below).unwrap();
    assert_eq!(above.end, below.begin);
    let end = water.position_along(above, above.length_mm).unwrap();
    let start = water.position_along(below, 0).unwrap();
    assert_eq!(end, start);
    // Half way down the line from (3.2, 1.2) to (3.2, 4.8) in fixed units, up to the survey's own rounding.
    assert_eq!(end.x, 104_858);
    let expected = 39_322 + (157_286 - 39_322) * i64::from(above.end) / 1_000_000;
    assert!((i64::from(end.y) - expected).abs() <= 1, "{end:?}");
}

#[test]
fn a_position_half_way_along_a_reach_with_a_bend_is_on_the_bend() {
    let text = "format = 3\nhand_list_sha256 = \"x\"\nbaseline_sha256 = \"x\"\nnext_node_id = 1\nnext_stretch_id = 1\n\
        river_width_mm = 20000\nterrain_sha256 = \"x\"\nrivers_sha256 = \"x\"\nsea_distance = \"s.png\"\n\
        sea_distance_sha256 = \"x\"\nnext_landing_id = 1\n\
        [[joint]]\nid = 0\nstretch = \"b-1\"\ncell = [0, 0]\n[[joint]]\nid = 1\nstretch = \"b-1\"\ncell = [1, 1]\n\
        [[reach]]\nid = 0\nstretch = \"b-1\"\nfrom = 0\nto = 1\nusable = false\nlength_mm = 4000\nkind = \"river\"\nbegin = 0\nend = 1000000\n";
    let rows = ["..", ".."];
    let n = network(text);
    let cells = surfaces(&rows);
    let rivers = river_network(&[(0, 0), (65_536, 0), (65_536, 65_536)], "b-1");
    let water = build(
        &n,
        &Inputs {
            crop: crop(&rows),
            cells: &cells,
            layer: &[65_535; 4],
            rivers: Some(&rivers),
        },
    )
    .unwrap();
    let reach = &n.reaches[0];
    let bend = FixedPoint { x: 65_536, y: 0 };
    assert_eq!(water.position_along(reach, 2000), Some(bend));
    // The straight line between the reach's ends passes through (32768, 32768): not the bend.
    assert_ne!(
        water.position_along(reach, 2000),
        Some(FixedPoint {
            x: 32_768,
            y: 32_768
        })
    );
    assert_eq!(
        water.position_along(reach, 1000),
        Some(FixedPoint { x: 32_768, y: 0 })
    );
    assert_eq!(
        water.position_along(reach, 0),
        Some(FixedPoint { x: 0, y: 0 })
    );
    assert_eq!(
        water.position_along(reach, 99_999),
        Some(FixedPoint {
            x: 65_536,
            y: 65_536
        })
    );
}

fn tiny(rows: &[&str]) -> Water {
    let text = "format = 3\nhand_list_sha256 = \"x\"\nbaseline_sha256 = \"x\"\nnext_node_id = 1\nnext_stretch_id = 1\n\
        river_width_mm = 20000\nterrain_sha256 = \"x\"\nrivers_sha256 = \"x\"\nsea_distance = \"s.png\"\n\
        sea_distance_sha256 = \"x\"\nnext_landing_id = 1\n\
        [[water_class]]\nid = \"c\"\nreach_m = 1000\nsea_going = false\nbodies = 0\n";
    let mut n = network(text);
    let cells = surfaces(rows);
    let layer = vec![100_u16; cells.len()];
    // The count of bodies is the loader's to work out: read it from the refusal, then store it.
    let found = match build(
        &n,
        &Inputs {
            crop: crop(rows),
            cells: &cells,
            layer: &layer,
            rivers: None,
        },
    ) {
        Ok(water) => return water,
        Err(error) => error.to_string(),
    };
    let count: usize = found
        .split("loader's ")
        .nth(1)
        .and_then(|rest| rest.trim().parse().ok())
        .unwrap();
    n.classes[0].bodies = count;
    build(
        &n,
        &Inputs {
            crop: crop(rows),
            cells: &cells,
            layer: &layer,
            rivers: None,
        },
    )
    .unwrap()
}

#[test]
fn a_corner_move_is_legal_only_where_both_cells_beside_the_corner_are_usable_too() {
    let open = tiny(&["~~", "~~"]);
    let mut moves = open.moves(0, [0, 0]);
    moves.sort_unstable();
    assert_eq!(moves, [[0, 1], [1, 0], [1, 1]]);
    // A land cell beside the corner takes the corner move away.
    let bent = tiny(&["~~", "~."]);
    let mut moves = bent.moves(0, [0, 0]);
    moves.sort_unstable();
    assert_eq!(moves, [[0, 1], [1, 0]]);
    // Two cells that touch only at a corner are two bodies, and no move joins them.
    let pair = tiny(&["~.", ".~"]);
    assert_eq!(pair.body_count(0), 2);
    assert!(pair.moves(0, [0, 0]).is_empty());
    assert!(pair.moves(0, [1, 1]).is_empty());
    assert!(tiny(&["~~", "~~"]).moves(0, [5, 5]).is_empty());
}

fn without_block(text: &str, header: &str, key: &str) -> String {
    let start = text.find(&format!("{header}\n{key}")).unwrap();
    let end = text[start + header.len()..]
        .find("\n[[")
        .map_or(text.len(), |at| start + header.len() + at + 1);
    format!("{}{}", &text[..start], &text[end..])
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "each refusal is one row of one table: the case list is the test"
)]
fn each_stored_table_that_disagrees_with_the_loader_is_refused_by_entry_and_fault() {
    let cases: Vec<(String, &str, &str)> = vec![
        (
            WORLD.replacen("river_width_mm = 20000", "river_width_mm = -1", 1),
            "river_width_mm",
            "negative",
        ),
        (
            WORLD.replacen("reach_m = 5000", "reach_m = 65534", 1),
            "water_class[currach]",
            "65,533",
        ),
        (
            WORLD.replacen("from = 0\nto = 1\n", "from = 99\nto = 1\n", 1),
            "reach[0]",
            "joint 99, which does not exist",
        ),
        (
            WORLD.replacen("[[water_join]]\nreach = 1", "[[water_join]]\nreach = 99", 1),
            "water_join[0]",
            "a reach that does not exist",
        ),
        (
            WORLD.replacen(
                "id = 1\nstretch = \"r-1\"\nfrom = 1\nto = 2\nusable = true",
                "id = 1\nstretch = \"r-1\"\nfrom = 1\nto = 2\nusable = false",
                1,
            ),
            "water_join[0]",
            "a join to a reach that is not usable",
        ),
        (
            WORLD.replacen("length_mm = 379717", "length_mm = 0", 1),
            "reach[0]",
            "length is nothing",
        ),
        (
            WORLD.replacen("begin = 0\nend = 222222", "begin = 300000\nend = 222222", 1),
            "reach[0]",
            "begins after it ends",
        ),
        (
            WORLD.replacen("end = 1000000", "end = 1000001", 1),
            "reach[3]",
            "ends outside its stretch",
        ),
        (
            WORLD.replacen(
                "stretch = \"r-1\"\ncell = [3, 2]",
                "stretch = \"x-1\"\ncell = [3, 2]",
                1,
            ),
            "joint[1]",
            "unknown surveyed stretch",
        ),
        (
            WORLD.replacen(
                "cell = [0, 3]\napproach_m = 0",
                "cell = [1, 3]\napproach_m = 0",
                1,
            ),
            "landing[1]",
            "contact is not usable water",
        ),
        (
            WORLD.replacen(
                "approach_m = 949\nbodies = [1, 1]",
                "approach_m = 949\nbodies = [0, 1]",
                1,
            ),
            "landing[2]",
            "stored body 0 for class barge is not what the loader finds, 1",
        ),
        (
            WORLD.replacen("bodies = [-1, 0]", "bodies = [0, 0]", 1),
            "landing[4]",
            "stored body 0 for class barge is not what the loader finds, -1",
        ),
        (
            WORLD.replacen(
                "classes = [\"currach\"]",
                "classes = [\"barge\", \"currach\"]",
                1,
            ),
            "strait[isle]",
            "classes are not every class less those a sea-going mark takes away",
        ),
        (
            WORLD.replacen("bodies = 3", "bodies = 2", 1),
            "water_class[barge]",
            "stored count of bodies 2 is not the loader's 3",
        ),
        (
            WORLD.replacen("landing_from = 8", "landing_from = 9", 1),
            "stretch[100]",
            "not keyed by that crossing stretch and end",
        ),
        (
            WORLD.replacen("landing_from = 8", "landing_from = 7", 1),
            "stretch[100]",
            "not keyed by that crossing stretch and end",
        ),
        (
            // The exact key of the crossing stretch and its end, held by a landing at the other end's node.
            WORLD.replacen("id = 8\nnode = 100\n", "id = 8\nnode = 101\n", 1),
            "stretch[100]",
            "or whose node is not that end's node",
        ),
        (
            WORLD.replacen("\nlanding_to = 9", "", 1),
            "landing[9]",
            "no crossing stretch names",
        ),
        (
            without_block(
                &without_block(WORLD, "[[landing]]", "id = 6"),
                "[[strait]]",
                "place",
            ),
            "access[isle]",
            "neither a landing nor a strait",
        ),
    ];
    for (text, entry, fault) in cases {
        let message = refused(&text);
        assert!(
            message.contains("roads.toml") && message.contains(entry) && message.contains(fault),
            "wanted {entry:?} and {fault:?}: {message}"
        );
    }
}

/// A small river world the tool stored: its rows, its sea-distance layer and its one surveyed line, in 1/32768 cell units.
#[derive(serde::Deserialize)]
struct CaseWorld {
    rows: Vec<String>,
    layer: Vec<u16>,
    river: String,
    points: Vec<[i32; 2]>,
}

fn load_case(roads: &str, world: &str) -> (LandNetwork, Water) {
    let world: CaseWorld = toml::from_str(world).unwrap();
    let rows = world.rows.iter().map(String::as_str).collect::<Vec<_>>();
    let n = network(roads);
    let cells = surfaces(&rows);
    let points = world
        .points
        .iter()
        .map(|p| (p[0], p[1]))
        .collect::<Vec<_>>();
    let rivers = river_network(&points, &world.river);
    let water = build(
        &n,
        &Inputs {
            crop: crop(&rows),
            cells: &cells,
            layer: &world.layer,
            rivers: Some(&rivers),
        },
    )
    .unwrap();
    (n, water)
}

fn narrowing() -> (LandNetwork, Water) {
    load_case(
        include_str!("../tests/water/narrowing.toml"),
        include_str!("../tests/water/narrowing.world.toml"),
    )
}

fn spans(n: &LandNetwork) -> Vec<(u32, u32, bool)> {
    n.reaches
        .iter()
        .map(|r| (r.begin, r.end, r.usable))
        .collect()
}

#[test]
fn a_river_narrowing_across_a_lake_cell_is_stored_usable_to_half_way_and_loads_as_one_body() {
    let (n, water) = narrowing();
    assert_eq!(
        n.joints.iter().map(|j| j.cell).collect::<Vec<_>>(),
        [[0, 1], [1, 1], [1, 1], [2, 1]]
    );
    assert_eq!(
        spans(&n),
        [
            (0, 333_333, true),
            (333_333, 500_000, true),
            (500_000, 1_000_000, false)
        ]
    );
    // The entry's joint is joined to the lake cell, and to that cell alone.
    assert_eq!(n.water_joins.len(), 1);
    let join = &n.water_joins[0];
    assert_eq!(
        (join.reach, join.to_reach, join.cell),
        (1, None, Some([1, 1]))
    );
    assert_eq!(join.classes, ["barge", "currach"]);
    for class in 0..2 {
        assert_eq!(water.body_count(class), 1, "class {class}");
        let body = water.cell_body(class, [1, 1]).unwrap();
        for joint in 0..3 {
            assert_eq!(water.joint_body(class, joint), Some(body), "joint {joint}");
        }
        // The part of the line after half way is too narrow: its end is in no body.
        assert_eq!(water.joint_body(class, 3), None);
    }
}

#[test]
fn a_river_widening_across_a_lake_cell_is_joined_at_half_way_and_loads_as_one_body() {
    let (n, water) = load_case(
        include_str!("../tests/water/widening.toml"),
        include_str!("../tests/water/widening.world.toml"),
    );
    assert_eq!(spans(&n), [(0, 500_000, false), (500_000, 1_000_000, true)]);
    assert_eq!(n.water_joins.len(), 1);
    let join = &n.water_joins[0];
    assert_eq!(
        (join.reach, join.to_reach, join.cell),
        (1, None, Some([1, 1]))
    );
    for class in 0..2 {
        assert_eq!(water.body_count(class), 1, "class {class}");
        let body = water.cell_body(class, [1, 1]).unwrap();
        // The joint at half way is the one the cell is joined to; the narrow part before it is in no body.
        let half = n.reaches[1].from;
        assert_eq!(water.joint_body(class, half), Some(body));
        assert_eq!(water.joint_body(class, 0), None);
    }
}

#[test]
fn a_line_through_three_lake_cells_has_a_join_for_each_and_the_three_cells_are_one_body() {
    let (n, water) = load_case(
        include_str!("../tests/water/three_lakes.toml"),
        include_str!("../tests/water/three_lakes.world.toml"),
    );
    assert_eq!(
        n.water_joins
            .iter()
            .map(|j| (j.reach, j.cell))
            .collect::<Vec<_>>(),
        [(1, Some([1, 1])), (2, Some([2, 1])), (3, Some([3, 1]))]
    );
    // Between two joints a reach has some length, however short.
    assert!(n.reaches.iter().all(|r| r.length_mm > 0 && r.usable));
    for class in 0..2 {
        assert_eq!(water.body_count(class), 1, "class {class}");
        let body = water.cell_body(class, [1, 1]).unwrap();
        for cell in [[2, 1], [3, 1]] {
            assert_eq!(water.cell_body(class, cell), Some(body), "cell {cell:?}");
        }
        for joint in 0..n.joints.len() {
            assert_eq!(water.joint_body(class, joint), Some(body), "joint {joint}");
        }
    }
}

/// The stored beginning and end, in millionths of the stretch, of a reach of 0.4 m (`tests/test_map_network.py` makes them again).
const NEAR_CUT: [u32; 2] = [333_333, 333_614];

#[test]
fn a_reach_of_four_tenths_of_a_metre_is_400_mm_between_two_joints_and_a_position_on_it_is_found() {
    let text = "format = 3\nhand_list_sha256 = \"x\"\nbaseline_sha256 = \"x\"\nnext_node_id = 1\nnext_stretch_id = 1\n\
        river_width_mm = 20000\nterrain_sha256 = \"x\"\nrivers_sha256 = \"x\"\nsea_distance = \"sea_distance.png\"\n\
        sea_distance_sha256 = \"x\"\nnext_landing_id = 1\n\
        [[joint]]\nid = 0\nstretch = \"b-1\"\ncell = [0, 0]\n[[joint]]\nid = 1\nstretch = \"b-1\"\ncell = [1, 0]\n\
        [[joint]]\nid = 2\nstretch = \"b-1\"\ncell = [1, 0]\n[[joint]]\nid = 3\nstretch = \"b-1\"\ncell = [2, 0]\n\
        [[reach]]\nid = 0\nstretch = \"b-1\"\nfrom = 0\nto = 1\nusable = true\nlength_mm = 474590\nkind = \"river\"\nbegin = 0\nend = 333333\n\
        [[reach]]\nid = 1\nstretch = \"b-1\"\nfrom = 1\nto = 2\nusable = true\nlength_mm = 400\nkind = \"river\"\nbegin = 333333\nend = 333614\n\
        [[reach]]\nid = 2\nstretch = \"b-1\"\nfrom = 2\nto = 3\nusable = true\nlength_mm = 948779\nkind = \"river\"\nbegin = 333614\nend = 1000000\n";
    let rows = ["..."];
    let n = network(text);
    let cells = surfaces(&rows);
    let rivers = river_network(&[(0, 16_384), (98_304, 16_384)], "b-1");
    let water = build(
        &n,
        &Inputs {
            crop: crop(&rows),
            cells: &cells,
            layer: &[65_535; 3],
            rivers: Some(&rivers),
        },
    )
    .unwrap();
    let reach = &n.reaches[1];
    assert_eq!((reach.begin, reach.end), (NEAR_CUT[0], NEAR_CUT[1]));
    assert_eq!((reach.length_mm, reach.from, reach.to), (400, 1, 2));
    let (start, middle, end) = (
        water.position_along(reach, 0).unwrap(),
        water.position_along(reach, 200).unwrap(),
        water.position_along(reach, 400).unwrap(),
    );
    // On the line, at its beginning and at its end, and between them in the middle; the next reach begins where this one ends.
    assert_eq!((start.y, middle.y, end.y), (16_384, 16_384, 16_384));
    assert!((i64::from(start.x) - 32_768).abs() <= 1, "{start:?}");
    assert!(
        start.x < middle.x && middle.x < end.x,
        "{start:?} {middle:?} {end:?}"
    );
    assert_eq!(water.position_along(&n.reaches[2], 0), Some(end));
    assert_eq!(water.position_along(&n.reaches[0], 474_590), Some(start));
}

#[test]
fn a_river_usable_a_thousandth_before_a_joint_is_stored_with_two_joints_between_and_loads_as_one_body()
 {
    let (n, water) = load_case(
        include_str!("../tests/water/usable_just_before.toml"),
        include_str!("../tests/water/usable_just_before.world.toml"),
    );
    // Becoming usable at 332,333 millionths and entering the lake cell at 333,333, a thousandth of the stretch on: two joints, 1,424 mm between.
    assert_eq!(
        spans(&n),
        [
            (0, 332_333, false),
            (332_333, 333_333, true),
            (333_333, 1_000_000, true)
        ]
    );
    assert_eq!(
        n.joints.iter().map(|j| j.cell).collect::<Vec<_>>(),
        [[0, 1], [0, 1], [1, 1], [2, 1]]
    );
    assert!(n.reaches.iter().all(|r| r.length_mm > 0));
    assert_eq!(n.reaches[1].length_mm, 1424);
    assert_eq!(n.water_joins.len(), 1);
    let join = &n.water_joins[0];
    assert_eq!(
        (join.reach, join.to_reach, join.cell),
        (2, None, Some([1, 1]))
    );
    for class in 0..2 {
        assert_eq!(water.body_count(class), 1, "class {class}");
        let body = water.cell_body(class, [1, 1]).unwrap();
        assert_eq!(water.joint_body(class, 0), None);
        for joint in 1..4 {
            assert_eq!(water.joint_body(class, joint), Some(body), "joint {joint}");
        }
    }
}

#[test]
fn a_river_usable_a_thousandth_after_a_joint_is_stored_inside_the_cell_and_loads_as_one_body() {
    let (n, water) = load_case(
        include_str!("../tests/water/usable_just_after.toml"),
        include_str!("../tests/water/usable_just_after.world.toml"),
    );
    // Becoming usable at 334,333 millionths, a thousandth of the stretch after the cell's entry: the one joint is joined to the cell.
    assert_eq!(spans(&n), [(0, 334_333, false), (334_333, 1_000_000, true)]);
    assert_eq!(
        n.joints.iter().map(|j| j.cell).collect::<Vec<_>>(),
        [[0, 1], [1, 1], [2, 1]]
    );
    assert!(n.reaches.iter().all(|r| r.length_mm > 0));
    assert_eq!(n.water_joins.len(), 1);
    let join = &n.water_joins[0];
    assert_eq!(
        (join.reach, join.to_reach, join.cell),
        (1, None, Some([1, 1]))
    );
    for class in 0..2 {
        assert_eq!(water.body_count(class), 1, "class {class}");
        let body = water.cell_body(class, [1, 1]).unwrap();
        assert_eq!(water.joint_body(class, 0), None);
        for joint in 1..3 {
            assert_eq!(water.joint_body(class, joint), Some(body), "joint {joint}");
        }
    }
}

#[test]
fn a_crossing_four_tenths_of_a_metre_below_a_break_lands_at_one_joint_with_a_400_mm_reach_to_the_break()
 {
    let (n, water) = load_case(
        include_str!("../tests/water/crossing_break.toml"),
        include_str!("../tests/water/crossing_break.world.toml"),
    );
    let falls = &n.breaks[0];
    assert_eq!(
        (falls.name.as_str(), falls.cell, falls.above, falls.below),
        ("Falls", [2, 1], 2, 3)
    );
    let crossing = n
        .stretches
        .iter()
        .find(|s| s.crossings.is_some())
        .expect("the road has a crossing stretch");
    let crossing_id = crossing.id;
    let ends: Vec<_> = n
        .landings
        .iter()
        .filter(|l| l.crossing == Some(crossing_id))
        .collect();
    assert_eq!(ends.len(), 2);
    // Both ends are at joint 1, in one body: the crossing contact is on the line 0.4 m below the break.
    assert!(ends.iter().all(|l| l.joint == Some(1)));
    assert_eq!(ends[0].bodies, ends[1].bodies);
    assert_eq!(crossing.landing_from, Some(ends[0].id));
    assert_eq!(crossing.landing_to, Some(ends[1].id));
    // The reach from that joint to the break's upstream joint is 0.4 m: stored as 400 mm, with two joints and no more.
    let short = n.reaches.iter().find(|r| r.to == falls.above).unwrap();
    assert_eq!((short.from, short.length_mm, short.usable), (1, 400, true));
    assert_eq!((short.begin, short.end), (249_368, 249_789));
    assert!(n.reaches.iter().all(|r| r.length_mm > 0));
    // A position on it is found, on the line (x = 2.2 cells) and between its ends.
    let (start, middle, end) = (
        water.position_along(short, 0).unwrap(),
        water.position_along(short, 200).unwrap(),
        water.position_along(short, 400).unwrap(),
    );
    for point in [start, middle, end] {
        assert_eq!(point.x, 72_090, "{point:?}");
    }
    assert!(
        start.y <= middle.y && middle.y <= end.y && start.y < end.y,
        "{start:?} {middle:?} {end:?}"
    );
    // The break's two sides are two joints of the stretch: nothing runs between them, and the position below the break is
    // where the line goes on.
    assert!(
        !n.reaches
            .iter()
            .any(|r| r.from == falls.above && r.to == falls.below)
    );
    let below = n.reaches.iter().find(|r| r.from == falls.below).unwrap();
    assert_eq!(short.end, below.begin);
    assert_eq!(
        water.position_along(short, short.length_mm),
        water.position_along(below, 0)
    );
    for class in 0..2 {
        let body = water.joint_body(class, 1).unwrap();
        assert_eq!(water.joint_body(class, falls.above), Some(body));
        assert_ne!(water.joint_body(class, falls.below), Some(body));
    }
}
