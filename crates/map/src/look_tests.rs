//! The default look's tables and rules (milestone 9, slice 10a): the strength vectors, the level table, the patterns, and the per-cell
//! strength with its edge and water cases.

use crate::look::{
    BAYER4_TRANSPOSED, BLUE16, D0, FERTILITY_LEVELS, LIGHT, SHADE_STEPS, TerrainShades,
    hillside_strength, hillside_strengths, land_colour, land_colour_with_steps, shade_index,
};
use crate::{Cell, Crop, Rgba8, Surface, TerrainStyle, WorldPixel};

/// A strength at the scale `65,536` (a row on the equator) and `d0 = 1 / 80`.
fn strength(delta: [i64; 2]) -> i8 {
    hillside_strength(delta, LIGHT, D0, 65_536)
}

#[test]
fn the_hillside_strength_vectors_of_the_spec() {
    for (delta, expected) in [
        ([0, 0], 0),
        ([0, 28], 8),
        ([0, -28], -8),
        ([0, 2000], 15),
        ([0, -2000], -15),
        ([-28, 0], -5),
        ([11, 26], 9),
        ([-11, -26], -9),
    ] {
        assert_eq!(strength(delta), expected, "{delta:?}");
    }
    // The curve is odd and never reaches 16: the strongest slope is 15.
    for delta in [
        [100_000, 100_000],
        [-100_000, 100_000],
        [i64::from(i32::MAX), 0],
    ] {
        assert!((-15..=15).contains(&strength(delta)), "{delta:?}");
    }
    // A wider cell (a smaller scale: nearer the pole) makes the same rise steeper.
    assert!(hillside_strength([0, 28], LIGHT, D0, 20_000) > strength([0, 28]));
    // The light vector's own direction (towards the light) is level ground's neighbour: a rise along it faces away.
    assert_eq!(strength([LIGHT[0].into(), LIGHT[1].into()]).signum(), -1);
}

#[test]
fn the_fertility_level_table_is_the_gamma_curve_in_integers() {
    for (index, level) in [
        (0, 0),
        (1, 0),
        (50, 4),
        (100, 25),
        (125, 45),
        (200, 146),
        (249, 253),
        (250, 256),
    ] {
        assert_eq!(FERTILITY_LEVELS[index], level, "stored fertility {index}");
    }
    assert!(
        FERTILITY_LEVELS[..=250]
            .windows(2)
            .all(|pair| pair[0] <= pair[1]),
        "monotone"
    );
    // `isqrt(65,536 i^5 / 250^5)` is `floor(256 (i / 250)^2.5)` exactly: no table entry is off by one against exact integer arithmetic.
    for (position, index) in (0..=250_u64).enumerate() {
        let level = u64::from(FERTILITY_LEVELS[position]);
        let exact = 65_536 * index.pow(5) / 250_u64.pow(5);
        assert!(
            level * level <= exact && exact < (level + 1) * (level + 1),
            "{index}"
        );
    }
    assert!(
        FERTILITY_LEVELS[251..].iter().all(|level| *level == 0),
        "251 to 255 are unused"
    );
    assert_eq!(FERTILITY_LEVELS[250], 256, "wider than a byte");
}

#[test]
fn the_patterns_are_the_signed_matrices() {
    let mut seen = [false; 256];
    for row in BLUE16 {
        for value in row {
            assert!(!seen[usize::from(value)], "{value} twice");
            seen[usize::from(value)] = true;
        }
    }
    assert!(
        seen.iter().all(|present| *present),
        "a permutation of 0 to 255"
    );
    assert_eq!(BLUE16[0][..4], [222, 146, 208, 158]);
    assert_eq!(BLUE16[7][..4], [31, 192, 16, 107]);
    assert_eq!(BLUE16[15][12..], [117, 59, 138, 19]);
    // The whole matrix, pinned by a checksum of every entry against its position (the matrix itself is the contract).
    let checksum: usize = BLUE16
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.iter()
                .enumerate()
                .map(move |(x, value)| usize::from(*value) * (y * 16 + x + 1))
        })
        .sum();
    assert_eq!(checksum, 4_165_015);
    assert_eq!(
        BAYER4_TRANSPOSED,
        [[0, 12, 3, 15], [8, 4, 11, 7], [2, 14, 1, 13], [10, 6, 9, 5]]
    );
    // The transpose of the standard Bayer matrix (the existing sea dither's, row = wy, column = wx).
    let standard = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    for (y, row) in standard.iter().enumerate() {
        for (x, value) in row.iter().enumerate() {
            assert_eq!(BAYER4_TRANSPOSED[x][y], *value);
        }
    }
}

fn land() -> Surface {
    Surface::Land {
        terrain: 0,
        usable: true,
    }
}

fn crop(width: u32, height: u32) -> Crop {
    Crop {
        origin: Cell { x: 10, y: 20 },
        width,
        height,
    }
}

/// Encoded elevations from metres.
fn raw(metres: &[i32]) -> Vec<u16> {
    metres
        .iter()
        .map(|metres| u16::try_from(metres + 11_000).expect("fits"))
        .collect()
}

#[test]
fn a_cells_strength_is_the_slope_over_its_neighbours_with_one_sided_edges() {
    // A 3 x 3 crop of land at heights (metres) 0 10 40 / 5 20 90 / 0 0 160, scale 65,536 for every row.
    let metres = [0, 10, 40, 5, 20, 90, 0, 0, 160];
    let cells = vec![land(); 9];
    let scales = [65_536; 3];
    let strengths = hillside_strengths(crop(3, 3), &raw(&metres), &cells, &scales, LIGHT, D0);
    let at = |x: usize, y: usize, delta: [i64; 2]| (strengths[y * 3 + x], strength(delta));
    // The centre: neighbours on both sides.
    let (got, want) = at(
        1,
        1,
        [
            i64::from(metres[5] - metres[3]),
            i64::from(metres[7] - metres[1]),
        ],
    );
    assert_eq!(got, want, "centre");
    // The west edge: one-sided, twice the step to the east. The north edge: twice the step to the south.
    let (got, want) = at(
        0,
        1,
        [
            2 * i64::from(metres[4] - metres[3]),
            i64::from(metres[6] - metres[0]),
        ],
    );
    assert_eq!(got, want, "west edge");
    let (got, want) = at(
        1,
        0,
        [
            i64::from(metres[2] - metres[0]),
            2 * i64::from(metres[4] - metres[1]),
        ],
    );
    assert_eq!(got, want, "north edge");
    // A corner is one-sided both ways.
    let (got, want) = at(
        2,
        2,
        [
            2 * i64::from(metres[8] - metres[7]),
            2 * i64::from(metres[8] - metres[5]),
        ],
    );
    assert_eq!(got, want, "south-east corner");
}

#[test]
fn a_water_neighbour_counts_as_height_zero_and_water_has_no_strength() {
    // The same 3 x 3, with the centre row's west cell sea and the cell below the centre a lake, both with deep elevations that
    // are ignored: water counts as height zero, and a land cell below sea level also counts as zero.
    let metres = [30, 30, 30, -500, 30, 30, 30, -200, 30];
    let mut cells = vec![land(); 9];
    cells[3] = Surface::Sea { depth: 500 };
    cells[7] = Surface::Lake;
    let strengths = hillside_strengths(crop(3, 3), &raw(&metres), &cells, &[65_536; 3], LIGHT, D0);
    assert_eq!(strengths[3], 0, "water has no strength");
    assert_eq!(strengths[7], 0);
    // The centre: 30 m of land east of a water cell (0), and 30 m of land north of a water cell (0).
    assert_eq!(strengths[4], strength([30, -30]));
    // A land cell below sea level reads as zero: the same map with the west cell as land at -500 m is the same.
    let mut land_cells = cells.clone();
    land_cells[3] = land();
    let again = hillside_strengths(
        crop(3, 3),
        &raw(&metres),
        &land_cells,
        &[65_536; 3],
        LIGHT,
        D0,
    );
    assert_eq!(again[4], strengths[4]);
}

#[test]
fn a_rows_scale_changes_the_strength_of_the_same_slope() {
    let metres = [0, 0, 0, 0, 20, 40, 0, 0, 0];
    let cells = vec![land(); 9];
    let equator = hillside_strengths(crop(3, 3), &raw(&metres), &cells, &[65_536; 3], LIGHT, D0);
    let north = hillside_strengths(crop(3, 3), &raw(&metres), &cells, &[30_000; 3], LIGHT, D0);
    assert!(north[4].abs() >= equator[4].abs());
    assert_ne!(north[4], equator[4]);
}

fn pasture() -> TerrainStyle {
    let colour = |hex: u32| {
        let [_, r, g, b] = hex.to_be_bytes();
        Rgba8::opaque(r, g, b)
    };
    TerrainStyle {
        solid: colour(0x00a0_8944),
        second: colour(0x0080_6c35),
        dark: colour(0x0015_110a),
        dark_lit: colour(0x002d_2314),
    }
}

fn hexes(colours: &[Rgba8]) -> Vec<String> {
    colours
        .iter()
        .map(|colour| format!("{:02x}{:02x}{:02x}", colour.0[0], colour.0[1], colour.0[2]))
        .collect()
}

#[test]
fn the_pasture_tones_are_the_specs_literal_vectors() {
    let shades = TerrainShades::of(&pasture());
    assert_eq!(
        hexes(&shades.tone),
        ["15110a", "43391d", "726131", "a08944"]
    );
    assert_eq!(hexes(&shades.shadow), ["392f18", "5c4e27", "806c35"]);
    assert_eq!(hexes(&shades.lit), ["2d2314", "534524", "7a6734"]);
    assert!(
        shades
            .tone
            .iter()
            .chain(&shades.shadow)
            .chain(&shades.lit)
            .all(|colour| colour.0[3] == 255),
        "every tone is opaque"
    );
}

#[test]
fn the_shade_index_is_three_times_the_level_with_blue_noise_on_the_remainder() {
    // Level 0 is shade 0 and level 256 is shade 3 for every blue-noise value (`v` is 768: wider than a byte).
    for blue in 0..=255 {
        assert_eq!(shade_index(0, blue, SHADE_STEPS), 0);
        assert_eq!(shade_index(256, blue, SHADE_STEPS), 3);
    }
    // Level 100: `v` 300, `k` 1, remainder 44: shade 2 where the blue value is below 44 and shade 1 elsewhere.
    for blue in 0..=255_u8 {
        assert_eq!(
            shade_index(100, blue, SHADE_STEPS),
            if blue < 44 { 2 } else { 1 },
            "blue {blue}"
        );
    }
    // Across a whole 16 x 16 tile, exactly 44 pixels take shade 2.
    let upper = BLUE16
        .iter()
        .flatten()
        .filter(|&&blue| shade_index(100, blue, SHADE_STEPS) == 2)
        .count();
    assert_eq!(upper, 44);
    // The levels in between never skip a shade, and a higher level never lowers a pixel's shade.
    for blue in 0..=255_u8 {
        let mut last = 0;
        for level in 0..=256 {
            let shade = shade_index(level, blue, SHADE_STEPS);
            assert!(
                shade >= last && shade - last <= 1,
                "level {level} blue {blue}"
            );
            last = shade;
        }
    }
}

/// A world pixel from tile coordinates.
fn at_tile(x: usize, y: usize) -> WorldPixel {
    WorldPixel {
        x: i32::try_from(x).expect("a tile coordinate"),
        y: i32::try_from(y).expect("a tile coordinate"),
    }
}

/// The first world pixel with the wanted pair of pattern values.
fn pixel_with(blue: impl Fn(u8) -> bool, bayer: impl Fn(u8) -> bool) -> WorldPixel {
    (0..16)
        .flat_map(|y| (0..16).map(move |x| (x, y)))
        .find(|&(x, y)| blue(BLUE16[y][x]) && bayer(BAYER4_TRANSPOSED[y % 4][x % 4]))
        .map(|(x, y)| at_tile(x, y))
        .expect("a pixel with the wanted values")
}

const ICE: (Rgba8, Rgba8) = (Rgba8::opaque(232, 233, 237), Rgba8::opaque(185, 188, 198));

#[test]
fn the_colour_function_steps_only_between_neighbouring_shades() {
    let style = pasture();
    let shades = TerrainShades::of(&style);
    // Stored fertility 200 (800 per mille) is level 146: `v` is 438, `k` 1, remainder 182, so shade 2 where the blue value is below
    // 182 and shade 1 elsewhere.
    assert_eq!(FERTILITY_LEVELS[200], 146);
    let at = |upper: bool, strength: i8| {
        // Bayer value 0 is below any non-zero strength: the pixel is a hit.
        let pixel = pixel_with(|blue| (blue < 182) == upper, |bayer| bayer == 0);
        land_colour(&style, ICE, 200 * 4, false, strength, pixel)
    };
    // Level ground: the tone of the shade.
    assert_eq!(at(true, 0), shades.tone[2]);
    assert_eq!(at(false, 0), shades.tone[1]);
    // The shadow side (negative strength): the shade's shadow tone, from the first shade up.
    assert_eq!(at(true, -1), shades.shadow[1]);
    assert_eq!(at(false, -1), shades.shadow[0]);
    // The lit side: the shade's lit tone, from the lit-dark tone up.
    assert_eq!(at(true, 1), shades.lit[2]);
    assert_eq!(at(false, 1), shades.lit[1]);
}

#[test]
fn the_extreme_shades_keep_the_old_looks_two_ends() {
    let style = pasture();
    let shades = TerrainShades::of(&style);
    let hit = pixel_with(|_| true, |bayer| bayer == 0);
    // Shade 0 on the shadow side stays dark (nothing darker to step from), and shade 3 on the lit side stays solid.
    assert_eq!(land_colour(&style, ICE, 0, false, -15, hit), shades.tone[0]);
    assert_eq!(
        land_colour(&style, ICE, 1000, false, 15, hit),
        shades.tone[3]
    );
    // Shade 0 on the lit side is the lit-dark tone; shade 3 on the shadow side is the second tone.
    assert_eq!(land_colour(&style, ICE, 0, false, 15, hit), style.dark_lit);
    assert_eq!(
        land_colour(&style, ICE, 1000, false, -15, hit),
        style.second
    );
}

#[test]
fn with_one_step_the_function_is_the_signed_r4_look() {
    let style = pasture();
    // r4's rule: bright where blue < level; a bright pixel on the shadow side with a hit is `second`, a dark one on the lit side with a
    // hit is `dark_lit`, otherwise `solid` or `dark`.
    for fertility_pm in [0_u16, 200, 400, 600, 1000] {
        let level = FERTILITY_LEVELS[usize::from(fertility_pm / 4)];
        for strength in [-15_i8, -8, -1, 0, 1, 8, 15] {
            for y in 0..16_usize {
                for x in 0..16_usize {
                    let pixel = at_tile(x, y);
                    let bright = u16::from(BLUE16[y][x]) < level;
                    let hit =
                        i16::from(BAYER4_TRANSPOSED[y % 4][x % 4]) < i16::from(strength).abs();
                    let expected = match (bright, strength.signum()) {
                        (true, -1) if hit => style.second,
                        (false, 1) if hit => style.dark_lit,
                        (true, _) => style.solid,
                        (false, _) => style.dark,
                    };
                    assert_eq!(
                        land_colour_with_steps(
                            &style,
                            ICE,
                            fertility_pm,
                            false,
                            strength,
                            pixel,
                            1
                        ),
                        expected,
                        "fertility {fertility_pm} strength {strength} pixel {x},{y}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_glacier_ignores_the_shades() {
    let style = pasture();
    let hit = pixel_with(|_| true, |bayer| bayer == 0);
    assert_eq!(land_colour(&style, ICE, 600, true, 0, hit), ICE.0);
    assert_eq!(land_colour(&style, ICE, 600, true, -5, hit), ICE.1);
    assert_eq!(land_colour(&style, ICE, 600, true, 5, hit), ICE.0);
}
