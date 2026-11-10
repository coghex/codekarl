//! Scene groups read back on a backend (milestone 8, slice 4): the scenes, the instances a real group scene places to, the CPU
//! reference and the checks, shared by the Metal and the Vulkan readback targets and by the library's own windowless test of this very
//! logic (where the "backend" is the CPU reference).
//!
//! Each scene is built through the public writer (`create_group`, `raise_group`, `set_group_visible`, `set_group_offset`,
//! `set_group_clip`, `raise_groups`) over a real resource service and a real bridge. Every published frame is placed with
//! [`FrameQuad::place`](ck_render_2d::FrameQuad::place) at the window's metrics (40 by 30 points at 2 pixels per point, an 80 by
//! 60 pixel target), and the placed quads, in painter order, are the instances a backend draws. The checks compare a backend's pixels
//! with the CPU reference **exactly**, and then pin what each frame means: which group shows on top, what a hidden group draws,
//! where an offset moves a quad, and where a clip cuts, with the texels trimmed rather than squashed.
//!
//! The includer declares `mod api` re-exporting `ck-render-2d`'s public items (`pub use ck_render_2d::*;` in an integration
//! target, `pub use crate::*;` in the library's tests).

use std::sync::{Arc, Mutex, PoisonError};

use ck_resources::{
    CpuPixels, Mode, ResourceLimits, ResourceService, ResourceState, TextureKey, TextureRef,
};
use ck_window::{PixelSize, WindowDesc};

use super::api::{Filter, Metrics, PointRect, QuadSink, SceneBridge, SceneWriter, Snapshot, Tint};

/// The clear colour (the same as the other scene readbacks').
pub(crate) const CLEAR: [f32; 3] = [0.1, 0.2, 0.3];

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
/// The theme's place-window colours (`assets/ui/theme.toml`): the near-black body, the cyan outline and title bar, the amber divider.
const BODY: [u8; 4] = [0x15, 0x11, 0x0a, 255];
const CYAN: [u8; 4] = [0x4f, 0xb3, 0xc8, 255];
const AMBER: [u8; 4] = [0xc9, 0x93, 0x2e, 255];

/// The clear colour as the byte pixel a target holds.
#[must_use]
pub(crate) fn clear_pixel() -> [u8; 4] {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a fraction from 0 to 1 scaled to 0..=255 and rounded"
    )]
    let byte = |channel: f32| (channel * 255.0).round() as u8;
    [byte(CLEAR[0]), byte(CLEAR[1]), byte(CLEAR[2]), 255]
}

/// An RGBA8 image.
pub(crate) struct Picture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bytes: Vec<u8>,
}

/// One placed quad: the image it shows, its destination in pixels and the visible part of the image as fractions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Instance {
    pub(crate) image: usize,
    pub(crate) destination: [i32; 4],
    pub(crate) source: [f32; 4],
    /// The quad's premultiplied RGBA8 tint (`[255; 4]` for none).
    pub(crate) tint: [u8; 4],
}

/// What the frame means, in the checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Meaning {
    /// Red below, green above, overlapping.
    GreenOnTop,
    /// The lower group raised: red above green.
    RedOnTop,
    /// The upper (green) group hidden.
    GreenHidden,
    /// Green hidden and the red group moved by (6, 4) points.
    RedMoved,
    /// The chrome and content pair, edited in one callback.
    ChromeAndContent,
    /// A scroll list's rows group scrolled by two whole rows.
    ListScrolledTwoRows,
    /// A scroll list's rows group scrolled by half a row.
    ListScrolledHalfRow,
    /// A place window's frame (a 40 by 30 UI pixel body, title bar and divider), one UI pixel `scale` points.
    PlaceWindow { scale: u32 },
    /// One line of white glyphs tinted near-black over a cyan bar.
    TintedGlyphs,
}

/// One published frame.
pub(crate) struct Frame {
    pub(crate) meaning: Meaning,
    /// The target's size in pixels.
    pub(crate) size: (u32, u32),
    pub(crate) instances: Vec<Instance>,
}

pub(crate) struct Scenes {
    pub(crate) pictures: Vec<Picture>,
    pub(crate) frames: Vec<Frame>,
}

fn solid(colour: [u8; 4]) -> Picture {
    Picture {
        width: 1,
        height: 1,
        bytes: colour.to_vec(),
    }
}

/// Four texels in a column: red, green, blue, yellow (a list row, with quarters that show how it was cut).
fn bands() -> Picture {
    Picture {
        width: 1,
        height: 4,
        bytes: [RED, GREEN, BLUE, YELLOW].concat(),
    }
}

/// Four texels in a row: yellow, green, blue, magenta.
fn strip() -> Picture {
    Picture {
        width: 4,
        height: 1,
        bytes: [YELLOW, GREEN, BLUE, MAGENTA].concat(),
    }
}

fn texture(
    service: &ResourceService,
    picture: &Picture,
) -> (TextureKey, TextureRef<ResourceState>) {
    let reservation = service
        .reserve_pixels(picture.width, picture.height)
        .expect("a pixel reservation");
    let pixels =
        CpuPixels::premultiplied(reservation, picture.bytes.clone()).expect("RGBA8 length");
    let key = service
        .create_from_pixels(pixels)
        .unwrap_or_else(|_| panic!("texture admitted"));
    (key, service.retain(key).expect("the texture is live"))
}

/// A sink that keeps the last snapshot.
#[derive(Default)]
struct Last(Mutex<Option<Snapshot>>);

impl QuadSink for Last {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(snapshot);
        None
    }
}

/// A bridge attached to a recording sink, `points` points across at 2 pixels per point, and its writer.
fn scene(points: (u32, u32)) -> (Arc<SceneBridge>, Arc<Last>, SceneWriter) {
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = Arc::new(Last::default());
    let attached: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
    bridge
        .attach(
            &WindowDesc {
                title: "groups".to_owned(),
                width: points.0,
                height: points.1,
                clear: CLEAR,
                ..WindowDesc::default()
            },
            PixelSize {
                width: points.0 * 2,
                height: points.1 * 2,
            },
            attached,
        )
        .expect("attached");
    let writer = bridge.take_writer().expect("the one writer");
    (bridge, sink, writer)
}

/// Publishes the writer's edits (one frame, which the call asserts) and returns the instances the frame places to, in painter order.
fn publish(writer: &mut SceneWriter, sink: &Last, keys: &[TextureKey], meaning: Meaning) -> Frame {
    assert!(writer.publish(), "{meaning:?}: the edits made one frame");
    let snapshot = sink
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .expect("a published snapshot");
    let metrics: Metrics = snapshot.metrics;
    let frame = snapshot.scene.expect("a scene frame");
    let instances = frame
        .quads()
        .iter()
        .filter_map(|quad| {
            let placed = quad.place(&metrics)?;
            let image = keys
                .iter()
                .position(|key| *key == placed.texture.key())
                .expect("a known texture");
            Some(Instance {
                image,
                destination: [
                    placed.destination.left,
                    placed.destination.top,
                    placed.destination.right,
                    placed.destination.bottom,
                ],
                source: [
                    placed.source.left,
                    placed.source.top,
                    placed.source.right,
                    placed.source.bottom,
                ],
                tint: quad.tint.rgba(),
            })
        })
        .collect();
    Frame {
        meaning,
        size: (metrics.pixels.width, metrics.pixels.height),
        instances,
    }
}

/// The scenes: the groups' three scenes, the list's, the place window at two scales and the tinted glyphs.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "the scenes are built in the order the frames are described, each with its writer calls beside the frame it publishes"
)]
pub(crate) fn scenes() -> Scenes {
    let service =
        ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("headless resources");
    let mut pictures = vec![solid(RED), solid(GREEN), strip(), bands()];
    pictures.extend(theme_pictures());
    let textures: Vec<(TextureKey, TextureRef<ResourceState>)> = pictures
        .iter()
        .map(|picture| texture(&service, picture))
        .collect();
    let keys: Vec<TextureKey> = textures.iter().map(|(key, _)| *key).collect();
    let mut frames = Vec::new();

    // Scene 1: a red quad in a group, a green quad in a later group, overlapping.
    let (_bridge, sink, mut writer) = scene((40, 30));
    let low = writer.create_group().expect("a group");
    let high = writer.create_group().expect("a group");
    writer
        .create_quad_in(Some(low), textures[0].1.clone(), 0.0, 0.0, 20.0, 20.0)
        .expect("the red quad");
    writer
        .create_quad_in(Some(high), textures[1].1.clone(), 10.0, 10.0, 20.0, 20.0)
        .expect("the green quad");
    frames.push(publish(&mut writer, &sink, &keys, Meaning::GreenOnTop));
    writer.raise_group(low).expect("raised");
    frames.push(publish(&mut writer, &sink, &keys, Meaning::RedOnTop));
    writer.set_group_visible(high, false).expect("hidden");
    frames.push(publish(&mut writer, &sink, &keys, Meaning::GreenHidden));
    writer.set_group_offset(low, 6.0, 4.0).expect("moved");
    frames.push(publish(&mut writer, &sink, &keys, Meaning::RedMoved));

    // Scene 2: the chrome and the content, edited in one callback and published once.
    let (_bridge, sink, mut writer) = scene((40, 30));
    let content = writer.create_group().expect("a group");
    let chrome = writer.create_group().expect("a group");
    writer
        .create_quad_in(Some(content), textures[2].1.clone(), 0.0, 0.0, 32.0, 8.0)
        .expect("the strip");
    writer
        .create_quad_in(Some(chrome), textures[0].1.clone(), 0.0, 0.0, 40.0, 6.0)
        .expect("the chrome bar");
    writer.set_group_offset(chrome, 0.0, 2.0).expect("moved");
    writer.set_group_offset(content, 4.0, 10.0).expect("moved");
    writer
        .set_group_clip(content, Some(PointRect::new(12.0, 10.0, 16.0, 8.0)))
        .expect("clipped");
    writer.raise_groups(&[content, chrome]).expect("raised");
    frames.push(publish(
        &mut writer,
        &sink,
        &keys,
        Meaning::ChromeAndContent,
    ));

    // Scene 3: a scroll list's rows group, as `list.lua` builds it. 20 rows 10 points high (a row is a `bands` quad: four colour
    // quarters), 5 in view in a list at (5, 5), 40 wide and 50 high, in a 50 by 60 point window. The rows group's offset is
    // `(5, 5 - s)` and its clip the list's visible rectangle (5, 5, 40, 50); only the rows that intersect it have quads.
    let (_bridge, sink, mut writer) = scene((50, 60));
    let rows = writer.create_group().expect("a group");
    writer
        .set_group_clip(rows, Some(PointRect::new(5.0, 5.0, 40.0, 50.0)))
        .expect("clipped");
    let mut quads = std::collections::BTreeMap::new();
    for (scroll, meaning) in [
        (20.0_f64, Meaning::ListScrolledTwoRows),
        (5.0_f64, Meaning::ListScrolledHalfRow),
    ] {
        // The rows that intersect the visible rectangle at this scroll, each created if it is not there, the others destroyed.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "row indices are small and non-negative"
        )]
        let (first, last) = (
            (scroll / 10.0).floor() as u32 + 1,
            (((scroll + 50.0) / 10.0).ceil() as u32).min(20),
        );
        let stale: Vec<u32> = quads
            .keys()
            .copied()
            .filter(|row| *row < first || *row > last)
            .collect();
        for row in stale {
            if let Some(quad) = quads.remove(&row) {
                writer.destroy_quad(quad).expect("destroyed");
            }
        }
        for row in first..=last {
            quads.entry(row).or_insert_with(|| {
                writer
                    .create_quad_in(
                        Some(rows),
                        textures[3].1.clone(),
                        0.0,
                        f64::from(row - 1) * 10.0,
                        40.0,
                        10.0,
                    )
                    .expect("a row")
            });
        }
        writer
            .set_group_offset(rows, 5.0, 5.0 - scroll)
            .expect("scrolled");
        frames.push(publish(&mut writer, &sink, &keys, meaning));
    }

    // Scenes 4 and 5: a place window's frame at one point and two points per UI pixel; scene 6: tinted glyphs over a cyan bar.
    for scale in [1_u32, 2] {
        let (_bridge, sink, mut writer) = scene((40 * scale, 30 * scale));
        place_window(&mut writer, &textures, scale);
        frames.push(publish(
            &mut writer,
            &sink,
            &keys,
            Meaning::PlaceWindow { scale },
        ));
    }
    let (_bridge, sink, mut writer) = scene((20, 20));
    let bar = writer
        .create_quad(textures[WHITE].1.clone(), 0.0, 0.0, 20.0, 20.0)
        .expect("the bar");
    writer
        .set_tint(bar, Tint::premultiplied(CYAN))
        .expect("tinted");
    let glyph = writer
        .create_quad(textures[GLYPH].1.clone(), 2.0, 2.0, 10.0, 14.0)
        .expect("the glyph");
    writer
        .set_tint(glyph, Tint::premultiplied(BODY))
        .expect("tinted");
    frames.push(publish(&mut writer, &sink, &keys, Meaning::TintedGlyphs));

    Scenes { pictures, frames }
}

// ---------------------------------------------------------------------------------------------------------------------
// The theme's frames (milestone 8, slice 7b)
// ---------------------------------------------------------------------------------------------------------------------

/// Picture indices of the theme's art in `Scenes::pictures`: the one white pixel, the cut-7 tiles and a white glyph.
const WHITE: usize = 4;
const TL_FILL: usize = 5;
const BR_FILL: usize = 6;
const TL_LINE: usize = 7;
const BR_LINE: usize = 8;
const GLYPH: usize = 9;

/// Cut-corner tile of `cut + 1` pixels square (§4.2): `top_left` or bottom-right, its line or its fill.
fn tile(cut: u32, top_left: bool, line: bool) -> Picture {
    let side = cut + 1;
    let mut bytes = Vec::new();
    for j in 0..side {
        for i in 0..side {
            let set = if line {
                i + j == cut
            } else if top_left {
                i + j > cut
            } else {
                i + j < cut
            };
            bytes.extend_from_slice(&if set { [255; 4] } else { [0; 4] });
        }
    }
    Picture {
        width: side,
        height: side,
        bytes,
    }
}

/// A white five by seven glyph (an "A") over clear texels.
fn glyph() -> Picture {
    let rows = [
        ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
    ];
    Picture {
        width: 5,
        height: 7,
        bytes: rows
            .iter()
            .flat_map(|row| row.chars())
            .flat_map(|cell| if cell == '#' { [255; 4] } else { [0; 4] })
            .collect(),
    }
}

/// The theme's pictures, in the order of the constants above.
fn theme_pictures() -> Vec<Picture> {
    vec![
        solid([255; 4]),
        tile(7, true, false),
        tile(7, false, false),
        tile(7, true, true),
        tile(7, false, true),
        glyph(),
    ]
}

/// One quad of a frame: the picture, whether it is tinted the fill or the line colour, and its rectangle in UI pixels
/// `(x, y, width, height)`.
struct Part {
    image: usize,
    line: bool,
    rect: (u32, u32, u32, u32),
}

/// The quads of a cut-7 frame of `w` by `h` UI pixels, cut at the top-left and, with `bottom_right`, at the bottom-right, in painter
/// order (the table of the design, §4.3).
fn frame_parts(w: u32, h: u32, bottom_right: bool) -> Vec<Part> {
    let c = 7;
    let mut parts = Vec::new();
    let mut add = |image, line, x1: u32, y1: u32, x2: u32, y2: u32| {
        parts.push(Part {
            image,
            line,
            rect: (x1, y1, x2 - x1 + 1, y2 - y1 + 1),
        });
    };
    add(TL_FILL, false, 0, 0, c, c);
    if bottom_right {
        add(BR_FILL, false, w - 1 - c, h - 1 - c, w - 1, h - 1);
    }
    add(WHITE, false, c + 1, 1, w - 2, c);
    add(
        WHITE,
        false,
        1,
        c + 1,
        w - 2,
        if bottom_right { h - 2 - c } else { h - 2 },
    );
    if bottom_right {
        add(WHITE, false, 1, h - 1 - c, w - 2 - c, h - 2);
    }
    add(TL_LINE, true, 0, 0, c, c);
    if bottom_right {
        add(BR_LINE, true, w - 1 - c, h - 1 - c, w - 1, h - 1);
    }
    add(WHITE, true, c + 1, 0, w - 1, 0);
    add(
        WHITE,
        true,
        w - 1,
        1,
        w - 1,
        if bottom_right { h - 2 - c } else { h - 1 },
    );
    add(
        WHITE,
        true,
        1,
        h - 1,
        if bottom_right { w - 2 - c } else { w - 2 },
        h - 1,
    );
    add(WHITE, true, 0, c + 1, 0, h - 1);
    parts
}

/// The place window's scene: the body (fill near-black, outline cyan), the title bar (cyan) of 15 UI pixels and the amber divider
/// under it, one UI pixel being `scale` points.
fn place_window(
    writer: &mut SceneWriter,
    textures: &[(TextureKey, TextureRef<ResourceState>)],
    scale: u32,
) {
    let scale_f = f64::from(scale);
    let mut add = |part: &Part, fill: [u8; 4], line: [u8; 4], dx: u32| {
        let (x, y, w, h) = part.rect;
        let quad = writer
            .create_quad(
                textures[part.image].1.clone(),
                f64::from(x + dx) * scale_f,
                f64::from(y) * scale_f,
                f64::from(w) * scale_f,
                f64::from(h) * scale_f,
            )
            .expect("a frame quad");
        writer
            .set_tint(
                quad,
                Tint::premultiplied(if part.line { line } else { fill }),
            )
            .expect("tinted");
    };
    for part in frame_parts(40, 30, true) {
        add(&part, BODY, CYAN, 0);
    }
    for part in frame_parts(40, 15, false) {
        add(&part, CYAN, CYAN, 0);
    }
    let divider = writer
        .create_quad(
            textures[WHITE].1.clone(),
            scale_f,
            15.0 * scale_f,
            38.0 * scale_f,
            scale_f,
        )
        .expect("the divider");
    writer
        .set_tint(divider, Tint::premultiplied(AMBER))
        .expect("tinted");
}

/// The pixels of a cut-7 frame by the design's rule (§4.2), independently of the quads: for each UI pixel of the `w` by `h` frame,
/// `Some(true)` on the outline, `Some(false)` in the fill and `None` outside.
fn frame_rule(w: u32, h: u32, bottom_right: bool) -> Vec<Option<bool>> {
    let c = 7_u32;
    let mut cells = vec![None; (w * h) as usize];
    let mut outline = |x: u32, y: u32| cells[(y * w + x) as usize] = Some(true);
    for x in c..w {
        outline(x, 0);
    }
    for y in 0..(if bottom_right { h - c } else { h }) {
        outline(w - 1, y);
    }
    for x in 0..(if bottom_right { w - c } else { w }) {
        outline(x, h - 1);
    }
    for y in c..h {
        outline(0, y);
    }
    for i in 0..=c {
        outline(i, c - i);
        if bottom_right {
            outline(w - 1 - i, h - 1 - c + i);
        }
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let inside_top_left = x + y > c;
            let inside_bottom_right = !bottom_right || (w - 1 - x) + (h - 1 - y) > c;
            if inside_top_left && inside_bottom_right && cells[(y * w + x) as usize].is_none() {
                cells[(y * w + x) as usize] = Some(false);
            }
        }
    }
    cells
}

/// The place window as the design says its pixels are, per UI pixel (40 by 30): the clear colour outside, the body's near-black fill and
/// cyan outline, the cyan title bar over them (rows 0 to 14), the amber divider on row 15 from column 1 to 38.
fn place_window_rule() -> Vec<[u8; 4]> {
    let mut pixels = vec![clear_pixel(); 40 * 30];
    for (at, cell) in frame_rule(40, 30, true).into_iter().enumerate() {
        match cell {
            Some(true) => pixels[at] = CYAN,
            Some(false) => pixels[at] = BODY,
            None => {}
        }
    }
    for (at, cell) in frame_rule(40, 15, false).into_iter().enumerate() {
        if cell.is_some() {
            pixels[at] = CYAN;
        }
    }
    for x in 1..39 {
        pixels[15 * 40 + x] = AMBER;
    }
    pixels
}

/// The reference: the clear, then each instance in painter order, nearest sampled at the target pixel's centre, tinted and composited
/// source-over (the pictures' texels are opaque or clear, so that is a replacement or nothing).
#[must_use]
pub(crate) fn reference(pictures: &[Picture], size: (u32, u32), instances: &[Instance]) -> Vec<u8> {
    let (width, height) = size;
    let mut frame = Vec::with_capacity(width as usize * height as usize * 4);
    for _ in 0..width * height {
        frame.extend_from_slice(&clear_pixel());
    }
    for instance in instances {
        let picture = &pictures[instance.image];
        let [left, top, right, bottom] = instance.destination;
        let [source_left, source_top, source_right, source_bottom] = instance.source;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            clippy::cast_possible_wrap,
            reason = "pixel coordinates in this target are tiny"
        )]
        for y in top.max(0)..bottom.min(height as i32) {
            for x in left.max(0)..right.min(width as i32) {
                let u = source_left
                    + ((x - left) as f32 + 0.5) / (right - left) as f32
                        * (source_right - source_left);
                let v = source_top
                    + ((y - top) as f32 + 0.5) / (bottom - top) as f32
                        * (source_bottom - source_top);
                let column = ((u * picture.width as f32).floor() as i64)
                    .clamp(0, i64::from(picture.width) - 1);
                let row = ((v * picture.height as f32).floor() as i64)
                    .clamp(0, i64::from(picture.height) - 1);
                let at = ((row * i64::from(picture.width) + column) * 4) as usize;
                let to = (y as usize * width as usize + x as usize) * 4;
                // The tint multiplies the premultiplied texel channel by channel (exact for the opaque white and clear texels the
                // theme's art is made of, with an opaque tint).
                let mut texel = [0_u8; 4];
                for (channel, out) in texel.iter_mut().enumerate() {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a product over 255, rounded, is at most 255"
                    )]
                    {
                        *out = ((u16::from(picture.bytes[at + channel])
                            * u16::from(instance.tint[channel])
                            + 127)
                            / 255) as u8;
                    }
                }
                // Source-over of the premultiplied texel (the theme's art has only opaque and clear texels, so this is a replacement
                // or nothing).
                let alpha = u16::from(texel[3]);
                for channel in 0..4 {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "source plus the attenuated destination is at most 255"
                    )]
                    {
                        frame[to + channel] = (u16::from(texel[channel])
                            + (u16::from(frame[to + channel]) * (255 - alpha) + 127) / 255)
                            as u8;
                    }
                }
            }
        }
    }
    frame
}

fn pixel(frame: &[u8], size: (u32, u32), x: u32, y: u32) -> [u8; 4] {
    let at = (y * size.0 + x) as usize * 4;
    [frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]
}

/// The bounding box `(left, top, right, bottom)`, right and bottom exclusive, of the pixels equal to `colour`, and how many there are.
fn bounds(
    frame: &[u8],
    size: (u32, u32),
    colour: [u8; 4],
) -> (Option<(u32, u32, u32, u32)>, usize) {
    let mut found: Option<(u32, u32, u32, u32)> = None;
    let mut count = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            if pixel(frame, size, x, y) == colour {
                count += 1;
                found = Some(found.map_or((x, y, x + 1, y + 1), |(l, t, r, b)| {
                    (l.min(x), t.min(y), r.max(x + 1), b.max(y + 1))
                }));
            }
        }
    }
    (found, count)
}

/// What draws one frame: the pictures, the target's size and the frame's instances in, the target's RGBA8 pixels out.
pub(crate) type Draw<'a> = dyn FnMut(&[Picture], (u32, u32), &[Instance]) -> Vec<u8> + 'a;

/// Checks every frame: the backend's pixels equal the reference exactly, and mean what the frame is meant to say. `draw` gives a
/// backend's pixels for one frame's instances.
#[expect(
    clippy::too_many_lines,
    reason = "the checks of one frame run in the order the frame is described: pixels, then each meaning"
)]
pub(crate) fn check(scenes: &Scenes, draw: &mut Draw<'_>) {
    let clear = clear_pixel();
    for frame in &scenes.frames {
        let meaning = frame.meaning;
        let size = frame.size;
        let pixels = draw(&scenes.pictures, size, &frame.instances);
        let expected = reference(&scenes.pictures, size, &frame.instances);
        assert_eq!(pixels.len(), expected.len(), "{meaning:?}: the size");
        let differing = pixels
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
            .filter(|(seen, wanted)| seen != wanted)
            .count();
        assert_eq!(
            differing, 0,
            "{meaning:?}: pixels differ from the reference"
        );
        // Scene 1 has the red quad at (0, 0)-(20, 20) points and the green one at (10, 10)-(30, 30): in pixels (0, 0)-(40, 40) and
        // (20, 20)-(60, 60), overlapping at (20, 20)-(40, 40).
        match meaning {
            Meaning::GreenOnTop => {
                assert_eq!(
                    pixel(&pixels, size, 30, 30),
                    GREEN,
                    "the later group is on top in the overlap"
                );
                assert_eq!(pixel(&pixels, size, 10, 10), RED);
                assert_eq!(pixel(&pixels, size, 50, 50), GREEN);
                assert_eq!(pixel(&pixels, size, 70, 10), clear);
            }
            Meaning::RedOnTop => {
                assert_eq!(
                    pixel(&pixels, size, 30, 30),
                    RED,
                    "raising the lower group changes the top pixel"
                );
                assert_eq!(pixel(&pixels, size, 50, 50), GREEN);
                assert_eq!(pixel(&pixels, size, 10, 10), RED);
            }
            Meaning::GreenHidden => {
                assert_eq!(pixel(&pixels, size, 30, 30), RED);
                assert_eq!(
                    pixel(&pixels, size, 50, 50),
                    clear,
                    "a hidden group draws nothing"
                );
                assert_eq!(bounds(&pixels, size, GREEN).1, 0);
                assert_eq!(bounds(&pixels, size, RED).0, Some((0, 0, 40, 40)));
            }
            Meaning::RedMoved => {
                // Moved by (6, 4) points, which is exactly (12, 8) pixels at 2 pixels per point.
                assert_eq!(bounds(&pixels, size, RED).0, Some((12, 8, 52, 48)));
                assert_eq!(pixel(&pixels, size, 11, 8), clear);
                assert_eq!(pixel(&pixels, size, 12, 8), RED);
                assert_eq!(pixel(&pixels, size, 51, 47), RED);
                assert_eq!(pixel(&pixels, size, 52, 47), clear);
                assert_eq!(bounds(&pixels, size, GREEN).1, 0);
            }
            Meaning::ListScrolledTwoRows => {
                // The list is at (5, 5)-(45, 55) points: pixels x 10..90, y 10..110, and 10 points = 20 pixels a row, in four
                // colour quarters of 5 pixels. Scrolled two whole rows, rows 3 to 7 fill the view exactly: no row is cut, and the
                // view's first and last pixels are a row's first and last.
                assert_eq!(
                    pixel(&pixels, size, 50, 9),
                    clear,
                    "above the list's top edge"
                );
                assert_eq!(
                    pixel(&pixels, size, 50, 10),
                    RED,
                    "the view's first pixel is a row's first"
                );
                assert_eq!(pixel(&pixels, size, 50, 15), GREEN);
                assert_eq!(pixel(&pixels, size, 50, 20), BLUE);
                assert_eq!(pixel(&pixels, size, 50, 25), YELLOW);
                assert_eq!(pixel(&pixels, size, 50, 30), RED, "the next row starts");
                assert_eq!(
                    pixel(&pixels, size, 50, 109),
                    YELLOW,
                    "the view's last pixel is a row's last"
                );
                assert_eq!(
                    pixel(&pixels, size, 50, 110),
                    clear,
                    "below the list's bottom edge"
                );
                assert_eq!(pixel(&pixels, size, 9, 50), clear, "left of the list");
                assert_eq!(pixel(&pixels, size, 90, 50), clear, "right of the list");
                assert_eq!(bounds(&pixels, size, RED).0, Some((10, 10, 90, 95)));
            }
            Meaning::ListScrolledHalfRow => {
                // Scrolled 5 points = 10 pixels: row 1 (local y 0..10 points, now 0..10 points on screen = pixels 0..20) shows
                // only its lower half below the list's top edge at pixel 10: the blue and yellow quarters, each 5 pixels, trimmed
                // from the source and not squashed into the 10. Row 6 starts at pixel 100 and shows its upper half to the bottom
                // edge at pixel 110: the red and green quarters.
                assert_eq!(
                    pixel(&pixels, size, 50, 9),
                    clear,
                    "above the list's top edge"
                );
                assert_eq!(
                    pixel(&pixels, size, 50, 10),
                    BLUE,
                    "row 1 is cut at the top edge: its blue quarter is first"
                );
                assert_eq!(pixel(&pixels, size, 50, 14), BLUE);
                assert_eq!(pixel(&pixels, size, 50, 15), YELLOW);
                assert_eq!(pixel(&pixels, size, 50, 19), YELLOW);
                assert_eq!(pixel(&pixels, size, 50, 20), RED, "row 2 starts whole");
                assert_eq!(
                    pixel(&pixels, size, 50, 100),
                    RED,
                    "row 6 starts at pixel 100"
                );
                assert_eq!(pixel(&pixels, size, 50, 104), RED);
                assert_eq!(pixel(&pixels, size, 50, 105), GREEN);
                assert_eq!(
                    pixel(&pixels, size, 50, 109),
                    GREEN,
                    "row 6 is cut at the bottom edge: its green quarter is last"
                );
                assert_eq!(
                    pixel(&pixels, size, 50, 110),
                    clear,
                    "below the list's bottom edge"
                );
                assert_eq!(bounds(&pixels, size, RED).0, Some((10, 20, 90, 105)));
            }
            Meaning::PlaceWindow { scale } => {
                // One UI pixel is `scale` points, 2 * scale target pixels: every target pixel is the design's pixel of its block.
                let rule = place_window_rule();
                let block = 2 * scale;
                assert_eq!(size, (40 * block, 30 * block), "scale {scale}");
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        assert_eq!(
                            pixel(&pixels, size, x, y),
                            rule[((y / block) * 40 + x / block) as usize],
                            "scale {scale}: target pixel ({x}, {y}), UI pixel ({}, {})",
                            x / block,
                            y / block
                        );
                    }
                }
                // The cut corners are the clear colour, the divider is amber, the title bar cyan.
                assert_eq!(pixel(&pixels, size, 0, 0), clear);
                assert_eq!(pixel(&pixels, size, 7 * block, 0), CYAN);
                assert_eq!(pixel(&pixels, size, 20 * block, 15 * block), AMBER);
                assert_eq!(pixel(&pixels, size, 20 * block, 20 * block), BODY);
                assert_eq!(pixel(&pixels, size, 39 * block + 1, 29 * block + 1), clear);
            }
            Meaning::TintedGlyphs => {
                // The glyph's set texels are exactly the near-black tint; the bar around them is cyan; the glyph's clear texels show
                // the bar. The glyph is at (2, 2) points, 10 by 14: two points (four pixels) per texel; the 18 set texels cover 18 * 16
                // target pixels.
                let (found, count) = bounds(&pixels, size, BODY);
                assert_eq!(count, 18 * 16, "the set texels, tinted exactly");
                assert_eq!(found, Some((4, 4, 24, 32)));
                assert_eq!(
                    pixel(&pixels, size, 4 + 4, 4),
                    BODY,
                    "the top row's middle texel"
                );
                assert_eq!(
                    pixel(&pixels, size, 4, 4),
                    CYAN,
                    "a clear texel shows the bar"
                );
                assert_eq!(bounds(&pixels, size, CYAN).1, 40 * 40 - 18 * 16);
                assert_eq!(bounds(&pixels, size, [255; 4]).1, 0, "no untinted white");
            }
            Meaning::ChromeAndContent => {
                // The chrome bar (0, 0, 40, 6) moved to y 2: pixels x 0..80, y 4..16.
                assert_eq!(bounds(&pixels, size, RED).0, Some((0, 4, 80, 16)));
                // The strip (0, 0, 32, 8) moved to (4, 10) is x 4..36, y 10..18 points; its texels are 8 points wide: yellow 4..12,
                // green 12..20, blue 20..28, magenta 28..36. The clip x 12..28 keeps exactly the green and the blue, each at its
                // own width, and cuts the rest: trimmed, not squashed.
                assert_eq!(bounds(&pixels, size, GREEN).0, Some((24, 20, 40, 36)));
                assert_eq!(bounds(&pixels, size, BLUE).0, Some((40, 20, 56, 36)));
                assert_eq!(
                    bounds(&pixels, size, YELLOW).1,
                    0,
                    "the clip cut the first texel"
                );
                assert_eq!(
                    bounds(&pixels, size, MAGENTA).1,
                    0,
                    "the clip cut the last texel"
                );
                assert_eq!(pixel(&pixels, size, 23, 25), clear);
                assert_eq!(pixel(&pixels, size, 56, 25), clear);
                assert_eq!(pixel(&pixels, size, 40, 35), BLUE);
                assert_eq!(pixel(&pixels, size, 39, 35), GREEN);
            }
        }
    }
}
