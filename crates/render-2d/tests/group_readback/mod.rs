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

use super::api::{Filter, Metrics, PointRect, QuadSink, SceneBridge, SceneWriter, Snapshot};

/// The clear colour (the same as the other scene readbacks').
pub(crate) const CLEAR: [f32; 3] = [0.1, 0.2, 0.3];

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

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
            })
        })
        .collect();
    Frame {
        meaning,
        size: (metrics.pixels.width, metrics.pixels.height),
        instances,
    }
}

/// The scenes: two scenes, five frames.
#[must_use]
pub(crate) fn scenes() -> Scenes {
    let service =
        ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("headless resources");
    let pictures = vec![solid(RED), solid(GREEN), strip(), bands()];
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

    Scenes { pictures, frames }
}

/// The reference: the clear, then each instance in painter order, nearest sampled at the target pixel's centre (every picture is
/// opaque, so source-over is a replacement).
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
                frame[to..to + 4].copy_from_slice(&picture.bytes[at..at + 4]);
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
