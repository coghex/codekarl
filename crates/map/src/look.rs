//! The map's default look (milestone 9, slice 10a; `docs/map-design.md` §12): fertility as the share of bright pixels on a blue-noise
//! pattern, hillsides as an ordered dither that follows the slope against a fixed light, and glaciers in ice. Integers only, so that
//! the CPU reference and the GPU passes agree to the byte.
//!
//! - **Fertility.** A land cell stores its fertility ÷ 4 (0 through 250). [`FERTILITY_LEVELS`] turns that into a number of 256 (the
//!   γ 2.5 curve, `⌊256 × (i ÷ 250)^2.5⌋`, exactly), and a pixel is bright where the blue-noise tile ([`BLUE16`]) at its world pixel is
//!   below that level. Full colour is 100% fertile land, the dark colour 0%.
//! - **Shades.** A terrain has four shades from its dark colour to its full colour in thirds ([`SHADE_STEPS`], the owner's two
//!   in-between tones): a pixel's shade is three times the level, with the blue-noise tile deciding the remainder ([`shade_index`]).
//! - **Hillsides.** Each land cell has a signed strength from −15 through +15 ([`hillside_strengths`]): positive faces the light. A
//!   shaded pixel on the shadow side steps towards the second tone, and one below the full colour on the lit side towards the full
//!   colour from the lit-dark tone, where the transposed Bayer matrix ([`BAYER4_TRANSPOSED`]) at the world pixel is below the
//!   strength ([`TerrainShades`] lists the ten tones).
//! - **Glaciers** are cells flagged by the climate model: ice, with its second tone on the shadow side.

use crate::{Crop, Rgba8, Surface, TerrainStyle, WorldPixel};

/// The blue-noise tile (void-and-cluster, Ulichney): row = `wy mod 16`, column = `wx mod 16`. The matrix is the contract, not the
/// generator. It is a permutation of 0 through 255.
pub const BLUE16: [[u8; 16]; 16] = [
    [
        222, 146, 208, 158, 58, 88, 171, 32, 213, 132, 11, 145, 210, 80, 248, 173,
    ],
    [
        67, 5, 102, 251, 27, 197, 140, 65, 184, 236, 54, 95, 24, 182, 36, 99,
    ],
    [
        156, 189, 134, 76, 176, 116, 220, 2, 111, 79, 167, 201, 225, 151, 123, 233,
    ],
    [
        29, 53, 214, 13, 237, 47, 94, 152, 254, 22, 139, 42, 109, 14, 60, 199,
    ],
    [
        115, 229, 96, 124, 155, 70, 226, 35, 172, 198, 90, 240, 72, 159, 252, 86,
    ],
    [
        168, 143, 39, 203, 23, 190, 131, 207, 64, 122, 8, 181, 216, 127, 186, 3,
    ],
    [
        66, 239, 81, 163, 250, 87, 7, 108, 45, 235, 149, 55, 26, 92, 48, 211,
    ],
    [
        31, 192, 16, 107, 57, 180, 142, 245, 161, 74, 217, 105, 175, 243, 148, 121,
    ],
    [
        103, 174, 137, 215, 125, 44, 219, 21, 98, 185, 34, 135, 205, 10, 73, 224,
    ],
    [
        50, 247, 63, 9, 234, 165, 78, 200, 129, 1, 230, 82, 114, 40, 196, 157,
    ],
    [
        25, 84, 195, 153, 91, 28, 113, 56, 241, 166, 62, 154, 253, 170, 89, 128,
    ],
    [
        183, 221, 112, 41, 204, 255, 177, 147, 38, 100, 193, 15, 51, 209, 17, 238,
    ],
    [
        61, 0, 136, 169, 71, 130, 6, 228, 83, 212, 118, 223, 133, 68, 110, 144,
    ],
    [
        162, 104, 246, 20, 218, 49, 106, 188, 18, 141, 30, 178, 93, 242, 187, 37,
    ],
    [
        232, 194, 77, 179, 97, 150, 202, 69, 164, 249, 75, 43, 160, 4, 206, 85,
    ],
    [
        119, 52, 33, 126, 227, 12, 244, 120, 46, 101, 191, 231, 117, 59, 138, 19,
    ],
];

/// The transpose of the standard 4 x 4 Bayer matrix: row = `wy mod 4`, column = `wx mod 4`.
pub const BAYER4_TRANSPOSED: [[u8; 4]; 4] =
    [[0, 12, 3, 15], [8, 4, 11, 7], [2, 14, 1, 13], [10, 6, 9, 5]];

/// What a stored fertility of 250 (full) is.
const FERTILITY_FULL: u64 = 250;

/// The number of bright levels, of 256, for each stored fertility value 0 through 250 (fertility ÷ 4): `isqrt(⌊65,536 × i⁵ ÷ 250⁵⌋)`,
/// which is `⌊256 × (i ÷ 250)^2.5⌋` exactly. Entries 251 through 255 are unused (255 marks a glacier in the packed grid) and zero. The
/// value 256 (at 250) is why the element is wider than a byte.
pub const FERTILITY_LEVELS: [u16; 256] = fertility_levels();

#[expect(
    clippy::cast_possible_truncation,
    reason = "the index is at most 250 and the root at most 256, both far inside the types"
)]
const fn fertility_levels() -> [u16; 256] {
    let mut table = [0_u16; 256];
    let full = FERTILITY_FULL.pow(5);
    let mut index = 0_u64;
    while index <= FERTILITY_FULL {
        let squared = 65_536 * index.pow(5) / full;
        table[index as usize] = squared.isqrt() as u16;
        index += 1;
    }
    table
}

/// The unit vector towards the light at azimuth 337.5° (x east, y south), as `round(10⁴ sin a)` and `round(−10⁴ cos a)`: the shipped
/// data's `[look] light`, which `terrain.toml` states so that no trigonometry runs at load.
pub const LIGHT: [i32; 2] = [-3827, -9239];

/// The shipped soft-curve constant `d₀`, as the ratio `n / d` (one value for every zoom step): `[look] d0`.
pub const D0: [u32; 2] = [1, 80];

/// The strength, −15 through +15, of the hillside whose elevation rises by `delta` (`[Δx, Δy]`, metres across two cells, y south) at a
/// row whose cell width scale is `scale` (`ground_scales`, the cosine of the latitude × 65,536). Positive faces the light.
///
/// `LΔ = ℓx × Δx + ℓy × Δy`; `A = d0.d × |LΔ| × 65,536` and `B = d0.n × 20,000 × 839 × scale`; the strength is `0` for `LΔ = 0` and
/// otherwise `sign(−LΔ) × ⌊16 × A ÷ (A + B)⌋`: the signed soft curve `16 × d ÷ (|d| + d₀)` of the rise away from the light over the
/// ground distance. Arithmetic is in `i128`, so no value of the validated domain overflows.
#[must_use]
pub fn hillside_strength(delta: [i64; 2], light: [i32; 2], d0: [u32; 2], scale: u32) -> i8 {
    let light_delta =
        i128::from(light[0]) * i128::from(delta[0]) + i128::from(light[1]) * i128::from(delta[1]);
    if light_delta == 0 {
        return 0;
    }
    let a = i128::from(d0[1]) * light_delta.abs() * 65_536;
    let b = i128::from(d0[0]) * 20_000 * 839 * i128::from(scale);
    let magnitude = 16 * a / (a + b);
    let magnitude = i8::try_from(magnitude).unwrap_or(15);
    if light_delta > 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// The hillside strength of every cell of a crop, row-major: the elevation of water cells counts as zero and a cell below sea level
/// as zero, the rise is the difference between the two neighbours (one-sided, doubled, at the crop's edge), and `scales` is the row
/// scale of each crop row. Water cells have no strength (0).
///
/// # Panics
///
/// If `elevations` or `cells` is not one entry per crop cell, or `scales` one per row.
#[must_use]
pub fn hillside_strengths(
    crop: Crop,
    elevations: &[u16],
    cells: &[Surface],
    scales: &[u32],
    light: [i32; 2],
    d0: [u32; 2],
) -> Vec<i8> {
    let width = crop.width as usize;
    let height = crop.height as usize;
    assert_eq!(elevations.len(), width * height, "one elevation per cell");
    assert_eq!(cells.len(), width * height, "one surface per cell");
    assert_eq!(scales.len(), height, "one scale per row");
    // The height of a cell as the hillside sees it: metres above the surface, water and below-surface land as zero.
    let rise = |x: usize, y: usize| -> i64 {
        let index = y * width + x;
        match cells[index] {
            Surface::Land { .. } => (i64::from(elevations[index]) - 11_000).max(0),
            Surface::Sea { .. } | Surface::Lake => 0,
        }
    };
    let across = |low: Option<usize>,
                  centre: usize,
                  high: Option<usize>,
                  at: &dyn Fn(usize) -> i64|
     -> i64 {
        match (low, high) {
            (Some(low), Some(high)) => at(high) - at(low),
            (None, Some(high)) => 2 * (at(high) - at(centre)),
            (Some(low), None) => 2 * (at(centre) - at(low)),
            (None, None) => 0,
        }
    };
    let mut strengths = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            if !matches!(cells[y * width + x], Surface::Land { .. }) {
                strengths.push(0);
                continue;
            }
            let dx = across(
                x.checked_sub(1),
                x,
                (x + 1 < width).then_some(x + 1),
                &|column| rise(column, y),
            );
            let dy = across(
                y.checked_sub(1),
                y,
                (y + 1 < height).then_some(y + 1),
                &|row| rise(x, row),
            );
            strengths.push(hillside_strength([dx, dy], light, d0, scales[y]));
        }
    }
    strengths
}

/// The number of steps between a terrain's dark colour and its full colour: the owner's two in-between tones (four shades,
/// `owner-direction-map-look-r5.md`). With one step there are no in-between tones, and the rule is the signed r4 look exactly.
pub const SHADE_STEPS: u16 = 3;

/// `⌊(x × (steps − k) + y × k + steps ÷ 2 …) ÷ steps⌋` as the nearest rounding of the channel `x + (y − x) × k ÷ steps`: with three
/// steps `⌊(x × (3 − k) + y × k + 1) ÷ 3⌋`, and with one step `x` or `y`.
fn mix_channel(x: u8, y: u8, k: u16, steps: u16) -> u8 {
    let numerator =
        2 * (u32::from(x) * u32::from(steps - k) + u32::from(y) * u32::from(k)) + u32::from(steps);
    u8::try_from(numerator / (2 * u32::from(steps))).expect("a mix of two channels is a channel")
}

fn mix(x: Rgba8, y: Rgba8, k: u16, steps: u16) -> Rgba8 {
    Rgba8::opaque(
        mix_channel(x.0[0], y.0[0], k, steps),
        mix_channel(x.0[1], y.0[1], k, steps),
        mix_channel(x.0[2], y.0[2], k, steps),
    )
}

/// The ten tones one land terrain shows (computed once at load from its four colours, no new data keys), and what the GPU receives:
/// the four shades, the three that replace a bright shade on the shadow side of a hillside and the three that replace a dark shade on
/// the lit side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainShades {
    /// `tone[k] = mix(dark, solid, k)`: `tone[0]` is `dark`, `tone[3]` is `solid`.
    pub tone: [Rgba8; 4],
    /// `shadow[k − 1] = mix(dark, second, k)` for k = 1…3: the shadow side of a hillside, from the first shade up; `shadow[2]` is
    /// `second`.
    pub shadow: [Rgba8; 3],
    /// `lit[k] = mix(dark_lit, solid, k)` for k = 0…2: the lit side, up to the shade below the full colour; `lit[0]` is `dark_lit`.
    pub lit: [Rgba8; 3],
}

impl TerrainShades {
    /// The ten tones of a terrain's four colours.
    #[must_use]
    pub fn of(style: &TerrainStyle) -> Self {
        let steps = SHADE_STEPS;
        Self {
            tone: [0, 1, 2, 3].map(|k| mix(style.dark, style.solid, k, steps)),
            shadow: [1, 2, 3].map(|k| mix(style.dark, style.second, k, steps)),
            lit: [0, 1, 2].map(|k| mix(style.dark_lit, style.solid, k, steps)),
        }
    }
}

/// The shade index of a pixel: `v = steps × level`, `k = ⌊v ÷ 256⌋`, plus one where the blue-noise tile is below `v mod 256`, capped
/// at `steps`. `level` is [`FERTILITY_LEVELS`]' entry for the cell, 0 through 256, so `v` can be 768 (wider than a byte).
#[must_use]
pub fn shade_index(level: u16, blue: u8, steps: u16) -> u16 {
    let v = steps * level;
    let k = v / 256 + u16::from(u16::from(blue) < v % 256);
    k.min(steps)
}

/// The colour of a land pixel at world pixel `pixel` whose sample point is in the cell with fertility `fertility_pm` (thousandths),
/// glacier flag `glacier` and hillside strength `strength`, in `style` (`ice` and `ice_second` for a glacier): rule 4 of the slice
/// with [`SHADE_STEPS`] steps.
#[must_use]
pub fn land_colour(
    style: &TerrainStyle,
    ice: (Rgba8, Rgba8),
    fertility_pm: u16,
    glacier: bool,
    strength: i8,
    pixel: WorldPixel,
) -> Rgba8 {
    land_colour_with_steps(
        style,
        ice,
        fertility_pm,
        glacier,
        strength,
        pixel,
        SHADE_STEPS,
    )
}

/// [`land_colour`] with `steps` shades between dark and full colour (1 through 3). With one step the rule is the signed r4 look
/// exactly: a bright pixel is `solid`, its shadow-side hit `second`, and a lit-side dark hit `dark_lit`.
#[must_use]
pub fn land_colour_with_steps(
    style: &TerrainStyle,
    ice: (Rgba8, Rgba8),
    fertility_pm: u16,
    glacier: bool,
    strength: i8,
    pixel: WorldPixel,
    steps: u16,
) -> Rgba8 {
    let bayer = i16::from(
        BAYER4_TRANSPOSED[pixel.y.rem_euclid(4) as usize][pixel.x.rem_euclid(4) as usize],
    );
    let steepness = i16::from(strength).abs();
    let hit = bayer < steepness;
    if glacier {
        return if strength < 0 && hit { ice.1 } else { ice.0 };
    }
    let level = FERTILITY_LEVELS[usize::from(fertility_pm / 4).min(255)];
    let blue = BLUE16[pixel.y.rem_euclid(16) as usize][pixel.x.rem_euclid(16) as usize];
    let k = shade_index(level, blue, steps);
    if hit && strength < 0 && k > 0 {
        mix(style.dark, style.second, k, steps)
    } else if hit && strength > 0 && k < steps {
        mix(style.dark_lit, style.solid, k, steps)
    } else {
        mix(style.dark, style.solid, k, steps)
    }
}
