//! The slice 6 fixture (`ck_map::fixture`, behind the `test-support` feature), which the Metal and Vulkan readback tests share, is worth comparing a GPU with only if
//! the reference's picture of it uses every rule. These tests say so, and pin the negative control the GPU tests use.

use std::collections::{BTreeMap, BTreeSet};

use ck_map::fixture::{Fixture, Kind, fixture};
use ck_map::look::TerrainShades;

/// Every colour the reference gives over all of the fixture's scenarios, with how many pixels show it.
fn colours() -> BTreeMap<[u8; 4], usize> {
    let fixture = fixture();
    let mut seen = BTreeMap::new();
    for scenario in Fixture::scenarios() {
        let image = fixture.reference_image(scenario.step, scenario.top_left, scenario.view);
        for pixel in image.chunks_exact(4) {
            *seen
                .entry([pixel[0], pixel[1], pixel[2], pixel[3]])
                .or_insert(0) += 1;
        }
    }
    seen
}

#[test]
fn the_fixture_shows_every_colour_of_the_rule_table() {
    let fixture = fixture();
    let seen = colours();
    let mut wanted = BTreeSet::new();
    wanted.insert(("the sea", fixture.data.sea.0));
    wanted.insert(("the shallow sea", fixture.data.shallow_sea.0));
    wanted.insert(("the lake", fixture.data.lake.0));
    wanted.insert(("the ice", fixture.data.ice.0));
    wanted.insert(("the ice second tone", fixture.data.ice_second.0));
    for (index, terrain) in fixture.terrains.iter().enumerate() {
        let style = terrain.style;
        match terrain.kind {
            Kind::Sea | Kind::Lake => {}
            Kind::Land | Kind::Mountain => {
                let shades = TerrainShades::of(&style);
                for tone in shades.tone {
                    wanted.insert(("a shade", tone.0));
                }
                for tone in shades.shadow {
                    wanted.insert(("a shadow-side tone", tone.0));
                }
                for tone in shades.lit {
                    wanted.insert(("a lit-side tone", tone.0));
                }
            }
        }
        assert!(index < 7, "the fixture has seven terrain types");
    }
    let missing: Vec<_> = wanted
        .iter()
        .filter(|(_, colour)| !seen.contains_key(colour))
        .collect();
    assert!(missing.is_empty(), "the reference never shows {missing:?}");
    // The rivers are rule 7 and slice 8's: their colour is never drawn here.
    assert!(!seen.contains_key(&fixture.data.river.0));
}

#[test]
fn every_scenario_has_more_than_one_colour_and_the_return_repeats_the_first_visit() {
    let fixture = fixture();
    let scenarios = Fixture::scenarios();
    for scenario in &scenarios {
        let image = fixture.reference_image(scenario.step, scenario.top_left, scenario.view);
        let distinct: BTreeSet<_> = image.chunks_exact(4).collect();
        let outside = scenario.name.contains("all outside");
        assert_eq!(distinct.len() == 1, outside, "{}", scenario.name);
    }
    // A pan away and back gives the same pixels: the reference is a pure function of the world pixel.
    let first = &scenarios[2];
    let last = scenarios.last().expect("scenarios");
    assert_eq!(
        (first.step, first.top_left, first.view),
        (last.step, last.top_left, last.view)
    );
    assert_eq!(
        fixture.reference_image(first.step, first.top_left, first.view),
        fixture.reference_image(last.step, last.top_left, last.view)
    );
}

#[test]
fn the_negative_control_changes_exactly_the_pixels_that_show_a_pasture_shadow_side_tone() {
    let fixture = fixture();
    let other = fixture.with_other_pasture_second_tone();
    // The pasture's second tone is the top of its shadow-side tones: all three change with it and nothing else does.
    let shadow = TerrainShades::of(&fixture.terrains[2].style)
        .shadow
        .map(|tone| tone.0);
    let mut changed = 0;
    for scenario in Fixture::scenarios() {
        let before = fixture.reference_image(scenario.step, scenario.top_left, scenario.view);
        let after = other.reference_image(scenario.step, scenario.top_left, scenario.view);
        for (old, new) in before.chunks_exact(4).zip(after.chunks_exact(4)) {
            assert_eq!(
                old != new,
                shadow.iter().any(|tone| old == tone),
                "{}",
                scenario.name
            );
            changed += usize::from(old != new);
        }
    }
    assert!(changed > 0, "the control changes something");
}
