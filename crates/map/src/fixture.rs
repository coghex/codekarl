//! The map fixture of milestone 6, slice 6 (behind the `test-support` feature): a small asymmetric map that exercises every rule of
//! `docs/map-design.md` section 9.4 (rules 1 to 6 and 8), shared by `ck-map`'s own coverage test and by the Metal and Vulkan readback
//! tests, which compare each backend's map pass with [`reference_colour`] byte for byte.
//!
//! The crop is 24 x 16 cells at a real-looking global origin (so the sub-cell jitter, whose hash depends on the absolute position, is
//! exercised on large coordinates). It has every terrain type — the sea (with a shelf under the shallow threshold and deeper water),
//! a lake, pasture, forest, bog, marsh and a mountain — usable and unusable cells of each land type, elevations on both sides of every
//! relief and cap boundary, coast cells and interior cells, and land that touches the crop's edge so that the outside of the crop is a
//! water neighbour.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    BuildingKind, Cell, Confidence, Crop, FixedPoint, HouseStyle, LoadedMap, LoadedTerrain,
    Location, LocationKind, LocationKinds, MODE_UNCOLOURED, MODE_UNOWNED, MapData, ModeInputs,
    PackedGrid, Rgba8, RiverLine, STEP_SIZE, Scenario as LoadedScenario, ScenarioStart, Surface,
    TerrainKind, TerrainStyle, WorldPixel, reference_colour, reference_colour_with_elevation,
    reference_colour_with_mode,
};

/// The crop's global origin cell.
pub const ORIGIN: [i32; 2] = [24_100, 14_700];
/// The crop's size in cells.
pub const SIZE: [u32; 2] = [24, 16];
/// The shallow-sea threshold in metres.
pub const SHALLOW_THRESHOLD: u16 = 40;

/// What a terrain index is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Ordinary land.
    Land,
    /// The sea.
    Sea,
    /// A lake.
    Lake,
    /// A mountain.
    Mountain,
}

/// One terrain index's rules: its kind and, for land and mountains, its style (water carries an unused one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Terrain {
    /// Water, ordinary land or a mountain.
    pub kind: Kind,
    /// The style the reference reads for land and mountains.
    pub style: TerrainStyle,
}

/// The map as the reference reads it and as the GPU gets it.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The reference's input.
    pub data: MapData,
    /// Grid A: terrain index, usable bit, zero, 255 (RGBA8, row-major).
    pub grid_a: Vec<u8>,
    /// Grid B: high and low byte of `elevation + 11000`, zero, 255.
    pub grid_b: Vec<u8>,
    /// One entry per terrain index (grid A's red byte): 0 sea, 1 lake, 2 pasture, 3 forest, 4 bog, 5 marsh, 6 mountain.
    pub terrains: Vec<Terrain>,
}

/// The layout, one string per crop row, one letter per cell: `~` sea, `L` lake, `p` pasture, `f` forest, `b` bog, `m` marsh,
/// `M` mountain.
const LAYOUT: [&str; 16] = [
    "~~~~~~~~~~~~~~~~~~~~~~~~",
    "~~~~~~~~~~pppp~~~~~~~~~~",
    "~~~~~~pppppppppp~~~~~~~~",
    "~~~~ppppffffppppMMMM~~~~",
    "~~~pppLLLffffppMMMMMMp~~",
    "~~ppppLLLfffffpMMMMMMMpp",
    "~ppppbbbLLfffffpMMMMMMMp",
    "ppppbbbbbbffffppMMMMMppp",
    "ppppbbbbbbffffppppmmmppp",
    "pppppbbbbfffffppmmmmmppp",
    "~ppppppbbpppppppmmmmmppp",
    "~~pppppppppppppppmmmpppp",
    "~~~ppppppppppppppppppp~~",
    "~~~~~ppppppppppppppp~~~~",
    "~~~~~~~~pppppppp~~~~~~~~",
    "~~~~~~~~~~pppp~~~~~~~~~~",
];

/// A land style from its four source tones: `dark_lit` is `dark + 2 x (dark_second - dark)`, as the shipped data writes it.
const fn style(
    solid: [u8; 3],
    second: [u8; 3],
    dark: [u8; 3],
    dark_second: [u8; 3],
) -> TerrainStyle {
    TerrainStyle {
        solid: Rgba8::opaque(solid[0], solid[1], solid[2]),
        second: Rgba8::opaque(second[0], second[1], second[2]),
        dark: Rgba8::opaque(dark[0], dark[1], dark[2]),
        dark_lit: Rgba8::opaque(
            lit(dark[0], dark_second[0]),
            lit(dark[1], dark_second[1]),
            lit(dark[2], dark_second[2]),
        ),
    }
}

/// One channel of `dark + 2 x (dark_second - dark)`, clamped to a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 first"
)]
const fn lit(dark: u8, dark_second: u8) -> u8 {
    let value = dark as i32 + 2 * (dark_second as i32 - dark as i32);
    if value < 0 {
        0
    } else if value > 255 {
        255
    } else {
        value as u8
    }
}

/// The sea's colour.
pub const SEA: Rgba8 = Rgba8::opaque(0x05, 0x08, 0x0e);
/// The shallow sea's colour.
pub const SHALLOW_SEA: Rgba8 = Rgba8::opaque(0x0b, 0x14, 0x22);
/// The lake's colour.
pub const LAKE: Rgba8 = Rgba8::opaque(0x0b, 0x14, 0x22);
/// The river's colour (rivers are rule 7, slice 8's).
pub const RIVER: Rgba8 = Rgba8::opaque(0x3a, 0x78, 0xc2);

/// The terrain table of the mock-up 2 palette. Index 0 sea, 1 lake, then the land types.
#[must_use]
pub fn terrains() -> Vec<Terrain> {
    let water = Terrain {
        kind: Kind::Sea,
        style: style([0; 3], [0; 3], [0; 3], [0; 3]),
    };
    let land = |solid, second, dark, dark_second| Terrain {
        kind: Kind::Land,
        style: style(solid, second, dark, dark_second),
    };
    vec![
        water,
        Terrain {
            kind: Kind::Lake,
            ..water
        },
        land(
            [0xa0, 0x89, 0x44],
            [0x80, 0x6c, 0x35],
            [0x15, 0x11, 0x0a],
            [0x21, 0x1a, 0x0f],
        ),
        land(
            [0x36, 0x75, 0x34],
            [0x28, 0x5a, 0x27],
            [0x0a, 0x12, 0x0a],
            [0x12, 0x20, 0x12],
        ),
        land(
            [0x2c, 0x4c, 0x2d],
            [0x20, 0x3a, 0x22],
            [0x09, 0x0f, 0x0a],
            [0x10, 0x1a, 0x11],
        ),
        land(
            [0x5c, 0x3e, 0x25],
            [0x46, 0x2e, 0x1b],
            [0x11, 0x0b, 0x07],
            [0x1c, 0x13, 0x0c],
        ),
        Terrain {
            kind: Kind::Mountain,
            style: style(
                [0x7f, 0x81, 0x87],
                [0x60, 0x62, 0x68],
                [0x10, 0x11, 0x14],
                [0x1b, 0x1c, 0x20],
            ),
        },
    ]
}

/// The glacier colour of the fixture, and its second tone.
pub const ICE: Rgba8 = Rgba8::opaque(0xe8, 0xe9, 0xed);
/// The glacier's second tone, on the shadow side.
pub const ICE_SECOND: Rgba8 = Rgba8::opaque(0xb9, 0xbc, 0xc6);

/// Whether the cell at crop column `x` and row `y` is a glacier: a patch of the mountains in the north-east.
fn is_glacier(letter: char, x: i32, y: i32) -> bool {
    letter == 'M' && x >= 18 && y <= 6
}

/// A land cell's elevation in metres, shaped for the hillside rule: a saddle `(x - cx) * (y - cy) * 6` around a centre that changes
/// with the column band, so that slopes of every strength face both ways in every band (the strength grows away from the centre), and
/// the mountains are bumpy and steep.
fn land_metres(letter: char, x: i32, y: i32) -> i32 {
    if letter == 'M' {
        return 180 + (x * 73 + y * 41) % 17 * 55;
    }
    let (cx, cy) = match x {
        ..=9 => (6, 8),
        10..=15 => (12, 6),
        _ => (19, 9),
    };
    400 + (x - cx) * (y - cy) * 6
}

/// The map: its cells, elevations (in metres, then stored as `metres + 11000`), usable bits and the packed grids.
///
/// # Panics
///
/// If the layout is malformed (the fixture is a constant, so never).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the fixture's cells, grids and look data are built together"
)]
pub fn fixture() -> Fixture {
    let terrains = terrains();
    let (width, height) = (SIZE[0] as usize, SIZE[1] as usize);
    let mut cells = Vec::with_capacity(width * height);
    let mut elevations = Vec::with_capacity(width * height);
    let mut fertility = Vec::with_capacity(width * height);
    let mut glacier = Vec::with_capacity(width * height);
    let mut stored_levels = Vec::with_capacity(width * height);
    let mut terrain_indices = Vec::with_capacity(width * height);
    let mut grid_a = Vec::with_capacity(width * height * 4);
    let mut grid_b = Vec::with_capacity(width * height * 4);
    for (y, row) in LAYOUT.iter().enumerate() {
        assert_eq!(row.len(), width, "layout row {y}");
        for (x, letter) in row.chars().enumerate() {
            let (xi, yi) = (i32::try_from(x).expect("x"), i32::try_from(y).expect("y"));
            let glacier_cell = is_glacier(letter, xi, yi);
            // Fertility spread over every land type, 0 to 250 (stored fertility ÷ 4), a quarter of the cells at full fertility, so that
            // every shade of every terrain, and every shadow-side and lit-side tone, shows in the picture; none on a glacier.
            let stored = if glacier_cell {
                0
            } else if (xi + 2 * yi) % 4 == 0 {
                250
            } else {
                (xi * 53 + yi * 29) % 251
            };
            let usable = stored > 0;
            let fertility_pm = stored * 4;
            let (index, metres) = match letter {
                // A shelf: the depth runs from 0 to 128 metres around the 40 metre threshold.
                '~' => (0_u8, -((xi * 7 + yi * 13) % 17) * 8),
                'L' => (1, 60 + (xi * 5 + yi * 3) % 9 * 4),
                'p' => (2, land_metres(letter, xi, yi)),
                'f' => (3, land_metres(letter, xi, yi)),
                'b' => (4, land_metres(letter, xi, yi)),
                'm' => (5, land_metres(letter, xi, yi)),
                'M' => (6, land_metres(letter, xi, yi)),
                other => panic!("layout letter {other:?}"),
            };
            terrain_indices.push(index);
            let raw = u16::try_from(metres + 11_000).expect("elevation fits");
            elevations.push(raw);
            fertility.push(u16::try_from(fertility_pm).expect("fertility fits"));
            glacier.push(glacier_cell);
            stored_levels.push(if glacier_cell {
                255
            } else {
                u8::try_from(stored).expect("stored fertility fits")
            });
            cells.push(match terrains[usize::from(index)].kind {
                Kind::Sea => Surface::Sea {
                    depth: 11_000_u16.saturating_sub(raw),
                },
                Kind::Lake => Surface::Lake,
                Kind::Land | Kind::Mountain => Surface::Land {
                    terrain: u16::from(index) - 2,
                    usable,
                },
            });
            grid_b.extend_from_slice(&[raw.to_be_bytes()[0], raw.to_be_bytes()[1], 0, 255]);
        }
    }
    let crop = Crop {
        origin: Cell {
            x: ORIGIN[0],
            y: ORIGIN[1],
        },
        width: SIZE[0],
        height: SIZE[1],
    };
    let hillside = crate::look::hillside_strengths(
        crop,
        &elevations,
        &cells,
        &crate::scenario::ground_scales(crop),
        crate::look::LIGHT,
        crate::look::D0,
    );
    // Grid A: the terrain index, fertility ÷ 4 (255 for a glacier) and the hillside strength + 128.
    for ((index, stored), strength) in terrain_indices.iter().zip(&stored_levels).zip(&hillside) {
        let packed = u8::try_from(i16::from(*strength) + 128).expect("strength fits");
        grid_a.extend_from_slice(&[*index, *stored, packed, 255]);
    }
    let data = MapData {
        crop,
        cells,
        elevations,
        fertility,
        climate_temperature_tenths: vec![100; width * height],
        climate_gdd: vec![1000; width * height],
        climate_rainfall_mm: vec![1000; width * height],
        glacier,
        hillside,
        ice: ICE,
        ice_second: ICE_SECOND,
        terrains: terrains[2..].iter().map(|terrain| terrain.style).collect(),
        sea: SEA,
        shallow_sea: SHALLOW_SEA,
        lake: LAKE,
        river: RIVER,
        shallow_threshold: SHALLOW_THRESHOLD,
    };
    data.validate().expect("the fixture is a valid map");
    Fixture {
        data,
        grid_a,
        grid_b,
        terrains,
    }
}

/// A small scenario over [`fixture`]'s map for production-path location readbacks.
///
/// # Panics
/// Panics only if the fixture's fixed terrain indices or cell count stop fitting their storage types.
#[expect(
    clippy::too_many_lines,
    reason = "keep the shared fixture scenario's map, palette and locations together"
)]
#[must_use]
pub fn location_scenario() -> LoadedScenario {
    let map_fixture = fixture();
    let crop = map_fixture.data.crop;
    let loaded_map = LoadedMap {
        data: map_fixture.data.clone(),
        terrain_types: map_fixture
            .terrains
            .iter()
            .enumerate()
            .map(|(index, terrain)| {
                let (kind, palette_colour, style_index) = match terrain.kind {
                    Kind::Sea => (TerrainKind::Sea, map_fixture.data.sea, None),
                    Kind::Lake => (TerrainKind::Lake, map_fixture.data.lake, None),
                    Kind::Land => (
                        TerrainKind::Land,
                        terrain.style.solid,
                        Some(u16::try_from(index - 2).expect("land style index fits")),
                    ),
                    Kind::Mountain => (
                        TerrainKind::Mountain,
                        terrain.style.solid,
                        Some(u16::try_from(index - 2).expect("mountain style index fits")),
                    ),
                };
                LoadedTerrain {
                    key: [u8::try_from(index).expect("fixture terrain index fits"); 3],
                    kind,
                    palette_colour,
                    style_index,
                }
            })
            .collect(),
        grid_a: packed_grid(&map_fixture.grid_a, crop.width, crop.height),
        grid_b: packed_grid(&map_fixture.grid_b, crop.width, crop.height),
        rivers: Vec::new(),
    };
    let kinds = LocationKinds {
        kinds: vec![
            LocationKind {
                id: "town".into(),
                min_size: 1,
                max_size: 1,
                catchment_m: vec![1_000],
                house_count: vec![1],
                house_radius_cells_x256: vec![512],
                glyph: vec![vec!['#']],
                light: Rgba8::opaque(230, 30, 40),
                dark: Rgba8::opaque(180, 20, 30),
            },
            LocationKind {
                id: "village".into(),
                min_size: 1,
                max_size: 1,
                catchment_m: vec![500],
                house_count: vec![0],
                house_radius_cells_x256: vec![0],
                glyph: vec![vec!['#']],
                light: Rgba8::opaque(20, 210, 40),
                dark: Rgba8::opaque(10, 150, 20),
            },
        ],
        buildings: vec![BuildingKind {
            id: "church".into(),
            glyph: vec![vec!['#', 'd', '#']],
            light: Rgba8::opaque(170, 80, 50),
            dark: Rgba8::opaque(140, 60, 35),
        }],
        house: HouseStyle {
            roof: Rgba8::opaque(230, 220, 200),
            light: Rgba8::opaque(230, 220, 200),
            dark: Rgba8::opaque(40, 30, 20),
        },
    };
    let cell = |x: i32, y: i32| Cell {
        x: ORIGIN[0] + x,
        y: ORIGIN[1] + y,
    };
    let locations = vec![
        Location {
            id: "river-town".into(),
            name: "River Town".into(),
            cell: cell(11, 7),
            kind_index: 0,
            size: 1,
            building_indices: Vec::new(),
            catchment_m: None,
            source: "fixture".into(),
            confidence: Confidence::High,
        },
        Location {
            id: "hill-village".into(),
            name: "Hill Village".into(),
            cell: cell(8, 10),
            kind_index: 1,
            size: 1,
            building_indices: vec![0],
            catchment_m: None,
            source: "fixture".into(),
            confidence: Confidence::Medium,
        },
    ];
    LoadedScenario {
        id: "overlay_readback_fixture".into(),
        start: ScenarioStart {
            year: 768,
            month: 10,
            day: 9,
            hour: 0,
        },
        map: loaded_map,
        location_kinds: kinds,
        locations,
        catchment_owners: vec![
            None;
            usize::try_from(crop.width * crop.height)
                .expect("small fixture crop")
        ],
    }
}

/// A short test river crossing the supplied world pixel at `step`.
///
/// The segment spans three pixels on each side so it visibly crosses a marker or house while
/// remaining local to the fixture. Invalid steps or coordinates that do not fit fixed-point
/// storage return `None`.
#[must_use]
pub fn river_through_pixel(pixel: WorldPixel, step: u8) -> Option<RiverLine> {
    let pixel_size = i64::from(*STEP_SIZE.get(usize::from(step))?);
    let fixed = |x: i32, y: i32| {
        Some(FixedPoint {
            x: i32::try_from(i64::from(x).checked_mul(pixel_size)?).ok()?,
            y: i32::try_from(i64::from(y).checked_mul(pixel_size)?).ok()?,
        })
    };
    Some(RiverLine {
        points: vec![
            fixed(pixel.x.checked_sub(3)?, pixel.y)?,
            fixed(pixel.x.checked_add(3)?, pixel.y)?,
        ],
    })
}

fn packed_grid(bytes: &[u8], width: u32, height: u32) -> PackedGrid {
    PackedGrid {
        width,
        height,
        pixels: bytes
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect(),
    }
}

impl Fixture {
    /// The world pixel of the crop's top-left corner at `step`.
    ///
    /// # Panics
    ///
    /// If `step` is past the last zoom step.
    #[must_use]
    pub fn crop_top_left(step: u8) -> WorldPixel {
        let size = i64::from(STEP_SIZE[usize::from(step)]);
        let to_pixel = |cell: i32| {
            i32::try_from(i64::from(cell) * 32_768 / size).expect("the crop's pixel fits")
        };
        WorldPixel {
            x: to_pixel(ORIGIN[0]),
            y: to_pixel(ORIGIN[1]),
        }
    }

    /// The reference's colour of one world pixel (no rivers: rule 7 is slice 8's).
    ///
    /// # Panics
    ///
    /// If `step` is past the last zoom step or the pixel's sample point does not fit 32 bits.
    #[must_use]
    pub fn reference(&self, step: u8, pixel: WorldPixel) -> [u8; 4] {
        reference_colour(&self.data, step, pixel, &BTreeSet::new())
            .expect("the pixel's sample point fits")
            .0
    }

    /// The reference's image of a view: `view.0 x view.1` RGBA8 pixels, rows top first, from world pixel `top_left` at `step`.
    ///
    /// # Panics
    ///
    /// As [`Fixture::reference`].
    #[must_use]
    pub fn reference_image(&self, step: u8, top_left: [i32; 2], view: [u32; 2]) -> Vec<u8> {
        let mut image = Vec::with_capacity(view[0] as usize * view[1] as usize * 4);
        for y in 0..view[1] {
            for x in 0..view[0] {
                image.extend_from_slice(&self.reference(
                    step,
                    WorldPixel {
                        x: top_left[0] + i32::try_from(x).expect("x"),
                        y: top_left[1] + i32::try_from(y).expect("y"),
                    },
                ));
            }
        }
        image
    }

    /// This map with pasture's second-tone colour changed: the negative control. The reference, the grids and every other rule are
    /// untouched; only the colour a usable pasture pixel shows in its second tone differs.
    #[must_use]
    pub fn with_other_pasture_second_tone(&self) -> Fixture {
        let mut other = self.clone();
        other.terrains[2].style.second = Rgba8::opaque(0xff, 0x00, 0xff);
        other.data.terrains[0].second = Rgba8::opaque(0xff, 0x00, 0xff);
        other
    }
}

/// The places of the map-mode fixture: where each one is (a crop column and row) and how far its catchment reaches, in cells.
const MODE_PLACES: [([u32; 2], u32); 4] = [([6, 8], 4), ([17, 10], 4), ([10, 3], 3), ([20, 3], 2)];

/// The number of places of the map-mode fixture: three coloured and one with no capacity.
pub const MODE_PLACE_COUNT: usize = MODE_PLACES.len();

/// The owner grid of the map-mode fixture, one `u16` per crop cell, row-major: every cell belongs to the nearest of four places within
/// that place's reach (ties to the lower index), or to nobody (`0xFFFF`). The catchments touch and cross land, coast and the lake, and
/// two of them meet, so the blend has borders, interiors, edges against unowned cells and isolated cells.
///
/// # Panics
///
/// Never: the four places' indices fit `u16`.
#[must_use]
pub fn mode_owners() -> Vec<u16> {
    let mut owners = Vec::new();
    for y in 0..SIZE[1] {
        for x in 0..SIZE[0] {
            let mut best: Option<(u32, u16)> = None;
            for (index, (at, reach)) in MODE_PLACES.iter().enumerate() {
                let (dx, dy) = (x.abs_diff(at[0]), y.abs_diff(at[1]));
                let distance = dx * dx + dy * dy;
                if distance <= reach * reach && best.is_none_or(|(nearest, _)| distance < nearest) {
                    best = Some((distance, u16::try_from(index).expect("four places")));
                }
            }
            owners.push(best.map_or(MODE_UNOWNED, |(_, index)| index));
        }
    }
    owners
}

/// A value table for the map-mode fixture: `revision` 0 and 1 give different values, and the fourth place never has a colour.
#[must_use]
pub fn mode_values(revision: u8) -> Vec<u16> {
    match revision {
        0 => vec![0, 500, 1000, MODE_UNCOLOURED],
        _ => vec![1000, 123, 777, MODE_UNCOLOURED],
    }
}

impl Fixture {
    /// The reference's image of a view under a map mode ([`reference_colour_with_mode`], no rivers
    /// or locations): `view.0 x view.1` RGBA8 pixels, rows top first.
    ///
    /// # Panics
    ///
    /// As [`Fixture::reference`].
    #[must_use]
    pub fn reference_image_with_mode(
        &self,
        step: u8,
        top_left: [i32; 2],
        view: [u32; 2],
        mode: &ModeInputs<'_>,
    ) -> Vec<u8> {
        let mut image = Vec::with_capacity(view[0] as usize * view[1] as usize * 4);
        for y in 0..view[1] {
            for x in 0..view[0] {
                let pixel = WorldPixel {
                    x: top_left[0] + i32::try_from(x).expect("x"),
                    y: top_left[1] + i32::try_from(y).expect("y"),
                };
                image.extend_from_slice(
                    &reference_colour_with_mode(
                        &self.data,
                        step,
                        pixel,
                        &BTreeSet::new(),
                        &BTreeMap::new(),
                        Some(mode),
                    )
                    .expect("the pixel's sample point fits")
                    .0,
                );
            }
        }
        image
    }
}

impl Fixture {
    /// The reference image of a view in the elevation mode (milestone 9, slice 10b): row-major RGBA8, `view[0] x view[1]` map pixels
    /// from the world pixel `top_left`, with no rivers and no locations.
    ///
    /// # Panics
    ///
    /// As [`Fixture::reference`].
    #[must_use]
    pub fn reference_image_with_elevation(
        &self,
        step: u8,
        top_left: [i32; 2],
        view: [u32; 2],
    ) -> Vec<u8> {
        let mut image = Vec::with_capacity(view[0] as usize * view[1] as usize * 4);
        for y in 0..view[1] {
            for x in 0..view[0] {
                let pixel = WorldPixel {
                    x: top_left[0] + i32::try_from(x).expect("x"),
                    y: top_left[1] + i32::try_from(y).expect("y"),
                };
                image.extend_from_slice(
                    &reference_colour_with_elevation(
                        &self.data,
                        step,
                        pixel,
                        &BTreeSet::new(),
                        &BTreeMap::new(),
                    )
                    .expect("the pixel's sample point fits")
                    .0,
                );
            }
        }
        image
    }
}

/// One view the readback tests render: a zoom step, the world pixel at its top-left and its size in map pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    /// What the view exercises.
    pub name: &'static str,
    /// The zoom step.
    pub step: u8,
    /// The world pixel at the view's top-left.
    pub top_left: [i32; 2],
    /// The view's width and height in map pixels.
    pub view: [u32; 2],
}

impl Fixture {
    /// The views that exercise every rule: steps coarser than a cell and finer (the sub-cell detail on, in its three octaves), views
    /// that overlap the crop's edge and lie wholly outside it, odd view sizes, coast and interior pixels, mountains, the lake and the
    /// shallow shelf. The last scenario is the third again, after one far away: a pan away and back.
    #[must_use]
    pub fn scenarios() -> Vec<Scenario> {
        let at = |step: u8, dx: i32, dy: i32| {
            let corner = Self::crop_top_left(step);
            [corner.x + dx, corner.y + dy]
        };
        let scenario = |name, step, offset: [i32; 2], view| Scenario {
            name,
            step,
            top_left: at(step, offset[0], offset[1]),
            view,
        };
        vec![
            scenario("three cells per pixel, crop edge", 10, [-3, -2], [13, 9]),
            scenario("one and a half cells per pixel", 20, [-9, -6], [37, 23]),
            scenario("half a cell per pixel, two octaves", 36, [-7, -5], [61, 41]),
            scenario("a tenth of a cell, crop corner", 60, [-11, -7], [71, 45]),
            scenario("a tenth of a cell, inside", 60, [110, 60], [67, 53]),
            scenario("the finest step, lake and coast", 75, [190, 130], [67, 45]),
            scenario("mountains, caps and high ground", 45, [50, 8], [45, 33]),
            scenario("far away: all outside", 36, [-300, -300], [29, 31]),
            scenario("back from far away", 36, [-7, -5], [61, 41]),
        ]
    }
}
