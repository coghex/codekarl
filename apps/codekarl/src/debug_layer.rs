//! Host-owned debug line: its rate and one top-layer quad, kept out of the UI VM's scene.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use ck_render_2d::{QuadKey, Tint, TopLayerWriter};
use ck_resources::{CpuPixels, ResourceService, ResourceState, TextureKey, TextureRef};
use ck_window::WindowMetrics;

use crate::config::DebugSettings;

const SECOND: u64 = 1_000_000_000;
const UI_PIXEL_UNITS: f64 = 2.0;

pub(crate) struct DebugLayer {
    writer: TopLayerWriter,
    resources: ResourceService,
    font: Arc<crate::font::FontSet>,
    text_colour: [u8; 4],
    body_colour: [u8; 4],
    clock: ck_window::UptimeNs,
    frames: Option<Arc<dyn Fn(u64) -> Vec<u64> + Send + Sync>>,
    frame_cursor: u64,
    frame_samples: VecDeque<u64>,
    update_samples: VecDeque<u64>,
    strip_texture: Option<TextureRef<ResourceState>>,
    glyph_textures: HashMap<char, TextureRef<ResourceState>>,
    strip_quad: Option<QuadKey>,
    glyph_quads: Vec<(char, QuadKey)>,
    last_text: Option<String>,
    last_settings: Option<DebugSettings>,
    last_metrics: Option<(WindowMetrics, Option<u32>)>,
    last_tick: Option<u64>,
    calculated_at: u64,
}

impl DebugLayer {
    pub(crate) fn new(
        writer: TopLayerWriter,
        resources: ResourceService,
        font: Arc<crate::font::FontSet>,
        theme: Option<&crate::theme::Theme>,
        clock: ck_window::UptimeNs,
    ) -> Self {
        Self {
            writer,
            resources,
            font,
            text_colour: theme
                .and_then(|theme| theme.colour("text"))
                .and_then(parse_colour)
                .unwrap_or([0xec, 0xe3, 0xcb, 0xff]),
            body_colour: theme
                .and_then(|theme| theme.colour("body"))
                .and_then(parse_colour)
                .unwrap_or([0x15, 0x11, 0x0a, 0xff]),
            clock,
            frames: None,
            frame_cursor: 0,
            frame_samples: VecDeque::new(),
            update_samples: VecDeque::new(),
            strip_texture: None,
            glyph_textures: HashMap::new(),
            strip_quad: None,
            glyph_quads: Vec::new(),
            last_text: None,
            last_settings: None,
            last_metrics: None,
            last_tick: None,
            calculated_at: 0,
        }
    }

    pub(crate) fn set_frame_times(
        &mut self,
        reader: Option<Arc<dyn Fn(u64) -> Vec<u64> + Send + Sync>>,
    ) {
        self.frames = reader;
    }

    pub(crate) fn accepted_update(&mut self) {
        self.update_samples.push_back((self.clock)());
    }

    pub(crate) fn update(
        &mut self,
        settings: DebugSettings,
        metrics: WindowMetrics,
        k: Option<u32>,
    ) {
        let now = (self.clock)();
        let changed = self.last_settings != Some(settings);
        if changed {
            self.last_settings = Some(settings);
            self.calculated_at = now;
            self.last_text = None;
        }
        if self
            .last_tick
            .is_some_and(|last| now.saturating_sub(last) > SECOND)
            && !changed
        {
            self.calculated_at = now;
            self.last_text = None;
        }
        self.last_tick = Some(now);
        if let Some(reader) = &self.frames {
            for sample in reader(self.frame_cursor) {
                self.frame_cursor = self.frame_cursor.max(sample);
                self.frame_samples.push_back(sample);
            }
        }
        prune(&mut self.frame_samples, now);
        prune(&mut self.update_samples, now);
        if settings.level == 0 {
            self.remove_line();
            return;
        }
        let text = if now.saturating_sub(self.calculated_at) >= SECOND {
            self.calculated_at = now;
            let samples = if settings.rate == "frames" {
                &self.frame_samples
            } else {
                &self.update_samples
            };
            format!("{} {}", rate(samples), suffix(settings.rate))
        } else {
            self.last_text
                .clone()
                .unwrap_or_else(|| format!("-- {}", suffix(settings.rate)))
        };
        if self.last_text.as_deref() == Some(&text) {
            if self.last_metrics == Some((metrics, k)) {
                return;
            }
            if let Err(error) = self.reposition_line(&text, metrics, k) {
                crate::lifecycle::emit(
                    ck_log::Level::Warn,
                    "debug",
                    format_args!("line placement: {error}"),
                );
                return;
            }
            self.last_metrics = Some((metrics, k));
            return;
        }
        if let Err(error) = self.place_line(&text, metrics, k) {
            crate::lifecycle::emit(
                ck_log::Level::Warn,
                "debug",
                format_args!("line texture: {error}"),
            );
            return;
        }
        self.last_text = Some(text);
        self.last_metrics = Some((metrics, k));
    }

    fn place_line(
        &mut self,
        text: &str,
        metrics: WindowMetrics,
        k: Option<u32>,
    ) -> Result<(), String> {
        let scale = UI_PIXEL_UNITS;
        let content_width: u32 = text
            .chars()
            .map(|character| self.font.standard_glyph(character).advance)
            .sum();
        let strip_width = f64::from(content_width.saturating_add(6)) * scale;
        let strip_height = 15.0 * scale;
        let view_height = k.map_or(metrics.points.1, |k| {
            ck_render_2d::ui_scale::view_units(metrics.pixels, k).1
        });
        let strip_x = 8.0;
        let strip_y = f64::from(view_height) - 8.0 - strip_height;
        if self.strip_texture.is_none() {
            self.strip_texture = Some(pixel_texture(&self.resources, 1, 1, vec![255; 4])?);
        }
        if let Some(strip) = self.strip_quad {
            self.writer
                .set_rect(strip, strip_x, strip_y, strip_width, strip_height)
                .map_err(|error| error.to_string())?;
        } else {
            let strip = self
                .writer
                .create_quad(
                    self.strip_texture.as_ref().expect("created above").clone(),
                    strip_x,
                    strip_y,
                    strip_width,
                    strip_height,
                )
                .map_err(|error| error.to_string())?;
            self.writer
                .set_tint(strip, Tint::premultiplied(self.body_colour))
                .map_err(|error| error.to_string())?;
            self.strip_quad = Some(strip);
        }
        for (_, quad) in self.glyph_quads.drain(..) {
            let _ = self.writer.destroy_quad(quad);
        }
        let mut pen = 0.0;
        for character in text.chars() {
            let glyph = self.font.standard_glyph(character);
            if glyph.rows.iter().any(|row| row.contains('#')) {
                let texture = if let Some(texture) = self.glyph_textures.get(&character) {
                    texture.clone()
                } else {
                    let texture = pixel_texture(
                        &self.resources,
                        u32::try_from(glyph.width().max(1)).unwrap_or(1),
                        u32::try_from(glyph.rows.len()).unwrap_or(1),
                        rasterize_glyph(glyph),
                    )?;
                    self.glyph_textures.insert(character, texture.clone());
                    texture
                };
                let quad = self
                    .writer
                    .create_quad(
                        texture,
                        strip_x + 3.0 * scale + pen * scale,
                        strip_y + 2.0 * scale,
                        f64::from(u32::try_from(glyph.width()).unwrap_or(1)) * scale,
                        f64::from(u32::try_from(glyph.rows.len()).unwrap_or(1)) * scale,
                    )
                    .map_err(|error| error.to_string())?;
                self.writer
                    .set_tint(quad, Tint::premultiplied(self.text_colour))
                    .map_err(|error| error.to_string())?;
                self.glyph_quads.push((character, quad));
            }
            pen += f64::from(glyph.advance);
        }
        self.writer.publish();
        Ok(())
    }

    fn reposition_line(
        &mut self,
        text: &str,
        metrics: WindowMetrics,
        k: Option<u32>,
    ) -> Result<(), String> {
        let scale = UI_PIXEL_UNITS;
        let content_width: u32 = text
            .chars()
            .map(|character| self.font.standard_glyph(character).advance)
            .sum();
        let view_height = k.map_or(metrics.points.1, |k| {
            ck_render_2d::ui_scale::view_units(metrics.pixels, k).1
        });
        let strip_y = f64::from(view_height) - 8.0 - 15.0 * scale;
        if let Some(strip) = self.strip_quad {
            self.writer
                .set_rect(
                    strip,
                    8.0,
                    strip_y,
                    f64::from(content_width.saturating_add(6)) * scale,
                    15.0 * scale,
                )
                .map_err(|error| error.to_string())?;
        }
        let mut pen = 0.0;
        let mut glyph_quad_index = 0;
        for character in text.chars() {
            let glyph = self.font.standard_glyph(character);
            if glyph.rows.iter().any(|row| row.contains('#')) {
                let (_, quad) = self
                    .glyph_quads
                    .get(glyph_quad_index)
                    .ok_or_else(|| format!("missing quad for {character:?}"))?;
                glyph_quad_index += 1;
                self.writer
                    .set_rect(
                        *quad,
                        8.0 + 3.0 * scale + pen * scale,
                        strip_y + 2.0 * scale,
                        f64::from(u32::try_from(glyph.width()).unwrap_or(1)) * scale,
                        f64::from(u32::try_from(glyph.rows.len()).unwrap_or(1)) * scale,
                    )
                    .map_err(|error| error.to_string())?;
            }
            pen += f64::from(glyph.advance);
        }
        self.writer.publish();
        Ok(())
    }

    fn remove_line(&mut self) {
        self.last_text = None;
        let mut changed = false;
        if let Some(quad) = self.strip_quad.take() {
            let _ = self.writer.destroy_quad(quad);
            changed = true;
        }
        for (_, quad) in self.glyph_quads.drain(..) {
            let _ = self.writer.destroy_quad(quad);
            changed = true;
        }
        if changed {
            self.writer.publish();
        }
    }

    pub(crate) fn end(&mut self) {
        self.remove_line();
        self.strip_texture = None;
        self.glyph_textures.clear();
    }
}

fn pixel_texture(
    resources: &ResourceService,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
) -> Result<TextureRef<ResourceState>, String> {
    let reservation = resources
        .reserve_pixels(width, height)
        .map_err(|error| error.to_string())?;
    let pixels =
        CpuPixels::premultiplied(reservation, pixels).map_err(|error| error.to_string())?;
    let key = resources
        .create_from_pixels(pixels)
        .map_err(|error| error.error.to_string())?;
    retained_texture(resources, key)
}

fn retained_texture(
    resources: &ResourceService,
    key: TextureKey,
) -> Result<TextureRef<ResourceState>, String> {
    let reference = resources.retain(key).map_err(|error| error.to_string());
    let released = resources.release(key).map_err(|error| error.to_string());
    match (reference, released) {
        (Ok(reference), Ok(())) => Ok(reference),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn suffix(rate: &str) -> &'static str {
    if rate == "updates" { "updates" } else { "fps" }
}

fn prune(samples: &mut VecDeque<u64>, now: u64) {
    while samples
        .front()
        .is_some_and(|sample| now.saturating_sub(*sample) > SECOND)
    {
        samples.pop_front();
    }
}

fn rate(samples: &VecDeque<u64>) -> u64 {
    match samples.len() {
        0 => 0,
        1 => 1,
        n => {
            let span = samples
                .back()
                .unwrap_or(&0)
                .saturating_sub(*samples.front().unwrap_or(&0));
            if span == 0 {
                return u64::try_from(n).unwrap_or(u64::MAX).saturating_add(1);
            }
            let intervals = u64::try_from(n - 1).unwrap_or(u64::MAX);
            let rounded = intervals.saturating_mul(SECOND).saturating_add(span / 2) / span;
            rounded.min(u64::try_from(n).unwrap_or(u64::MAX).saturating_add(1))
        }
    }
}

fn parse_colour(text: &str) -> Option<[u8; 4]> {
    crate::render_api::parse_tint("debug", "colour", text)
        .ok()
        .map(Tint::rgba)
}

fn rasterize_glyph(glyph: &crate::font::Glyph) -> Vec<u8> {
    let width = glyph.width().max(1);
    let mut pixels = vec![0; width.saturating_mul(glyph.rows.len()).saturating_mul(4)];
    for (y, row) in glyph.rows.iter().enumerate() {
        for (x, pixel) in row.bytes().enumerate() {
            if pixel == b'#' {
                let offset = (y * width + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&[255; 4]);
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use ck_render_2d::{Filter, QuadSink, SceneBridge, Snapshot};
    use ck_resources::{Mode, ResourceLimits, ServiceWakes};
    use ck_window::{PixelRequest, PixelSize, WindowDesc};

    #[derive(Default)]
    struct Seen {
        latest: Mutex<Option<Snapshot>>,
        publications: AtomicUsize,
    }

    impl QuadSink for Seen {
        fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
            self.publications.fetch_add(1, Ordering::SeqCst);
            self.latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace(snapshot)
        }
    }

    fn test_layer(
        k: Option<u32>,
        pixels: PixelSize,
        points: (u32, u32),
        theme_text: Option<&str>,
    ) -> (
        DebugLayer,
        Arc<Seen>,
        ResourceService,
        Arc<AtomicU64>,
        Arc<SceneBridge>,
    ) {
        let bridge = SceneBridge::new(Filter::Nearest);
        bridge.set_ui_scale_limit(k);
        let writer = bridge.take_top_writer().expect("top writer");
        let seen = Arc::new(Seen::default());
        bridge
            .attach(
                &WindowDesc {
                    title: "debug test".to_owned(),
                    width: points.0,
                    height: points.1,
                    pixel_request: k.map(|_| PixelRequest {
                        size: (pixels.width, pixels.height),
                        min: None,
                    }),
                    ..WindowDesc::default()
                },
                pixels,
                Arc::clone(&seen) as Arc<dyn QuadSink>,
            )
            .expect("attached");
        let resources = ResourceService::with_wakes(
            ResourceLimits::DEFAULT,
            Mode::Headless,
            ServiceWakes::default(),
        )
        .expect("resource service");
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let fonts = Arc::new(crate::font::load_all(&assets).expect("shipped fonts"));
        let theme = theme_text.map(crate::theme::partial_for_test);
        let now = Arc::new(AtomicU64::new(0));
        let clock: ck_window::UptimeNs = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        (
            DebugLayer::new(writer, resources.clone(), fonts, theme.as_ref(), clock),
            seen,
            resources,
            now,
            bridge,
        )
    }

    fn line(
        settings: DebugSettings,
        pixels: PixelSize,
        points: (u32, u32),
        k: Option<u32>,
    ) -> Vec<[f64; 4]> {
        let (mut layer, seen, _, _, _) = test_layer(k, pixels, points, None);
        layer.update(settings, WindowMetrics { points, pixels }, k);
        let latest = seen.latest.lock().unwrap();
        let snapshot = latest.as_ref().expect("line publication");
        snapshot
            .top_layer
            .as_ref()
            .expect("top layer")
            .quads()
            .iter()
            .map(|quad| [quad.x, quad.y, quad.width, quad.height])
            .collect()
    }

    #[test]
    fn shown_rate_uses_interval_average_and_bounds_tight_samples() {
        assert_eq!(rate(&VecDeque::new()), 0);
        assert_eq!(rate(&VecDeque::from([7])), 1);
        assert_eq!(rate(&VecDeque::from([0, 2 * SECOND / 3])), 2);
        assert_eq!(rate(&VecDeque::from([0, 1_000_000])), 3);
        let thirty_hz = (1..=60)
            .map(|index| index * SECOND / 30)
            .collect::<VecDeque<_>>();
        let sixty_hz = (1..=120)
            .map(|index| index * SECOND / 60)
            .collect::<VecDeque<_>>();
        for edge in 0..1000 {
            let now = SECOND + edge * SECOND / 1000;
            let mut thirty = thirty_hz
                .iter()
                .copied()
                .filter(|sample| *sample <= now)
                .collect::<VecDeque<_>>();
            let mut sixty = sixty_hz
                .iter()
                .copied()
                .filter(|sample| *sample <= now)
                .collect::<VecDeque<_>>();
            prune(&mut thirty, now);
            prune(&mut sixty, now);
            assert_eq!(rate(&thirty), 30, "30 Hz edge {edge}");
            assert_eq!(rate(&sixty), 60, "60 Hz edge {edge}");
        }
        let first_second = (1..=60)
            .map(|index| index * SECOND / 60)
            .collect::<VecDeque<_>>();
        let mut after_pause = first_second;
        prune(&mut after_pause, SECOND + SECOND / 2);
        assert!((31..=32).contains(&rate(&after_pause)));
    }

    #[test]
    fn theme_colour_parser_accepts_six_digit_colours() {
        assert_eq!(parse_colour("#ece3cb"), Some([0xec, 0xe3, 0xcb, 0xff]));
        assert_eq!(
            parse_colour("#80402080"),
            Some(Tint::from_straight(0x80, 0x40, 0x20, 0x80).rgba())
        );
        assert_eq!(parse_colour("#bad"), None);
    }

    #[test]
    fn glyph_texture_pixels_follow_the_font_rows() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let fonts = crate::font::load_all(&assets).expect("shipped fonts");
        let glyph = fonts.standard_glyph('f');
        let pixels = rasterize_glyph(glyph);
        let width = glyph.width();
        assert_eq!(pixels.len(), width * glyph.rows.len() * 4);
        for (y, row) in glyph.rows.iter().enumerate() {
            for (x, mark) in row.bytes().enumerate() {
                let expected = if mark == b'#' { [255; 4] } else { [0; 4] };
                assert_eq!(
                    &pixels[(y * width + x) * 4..(y * width + x + 1) * 4],
                    &expected
                );
            }
        }
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the geometry uses exact integer test inputs"
    )]
    fn line_rectangle_uses_the_top_layers_ui_units_across_scales_and_backing_ratios() {
        for (k, pixels, points) in [
            (
                2,
                PixelSize {
                    width: 1280,
                    height: 800,
                },
                (1280, 800),
            ),
            (
                2,
                PixelSize {
                    width: 1280,
                    height: 800,
                },
                (640, 400),
            ),
            (
                3,
                PixelSize {
                    width: 1920,
                    height: 1200,
                },
                (1920, 1200),
            ),
            (
                3,
                PixelSize {
                    width: 1920,
                    height: 1200,
                },
                (960, 600),
            ),
            (
                3,
                PixelSize {
                    width: 2560,
                    height: 1440,
                },
                (2560, 1440),
            ),
            (
                4,
                PixelSize {
                    width: 2560,
                    height: 1600,
                },
                (1280, 800),
            ),
        ] {
            let rects = line(
                DebugSettings {
                    level: 1,
                    rate: "frames",
                },
                pixels,
                points,
                Some(k),
            );
            let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
            let fonts = crate::font::load_all(&assets).expect("shipped fonts");
            let expected_width = f64::from(
                "-- fps"
                    .chars()
                    .map(|ch| fonts.standard_glyph(ch).advance)
                    .sum::<u32>()
                    + 6,
            ) * 2.0;
            let view = ck_render_2d::ui_scale::view_units(pixels, k);
            assert_eq!(rects[0][0], 8.0, "left edge at k={k}");
            assert_eq!(
                rects[0][1],
                f64::from(view.1 - 38),
                "bottom margin at k={k}"
            );
            assert_eq!(rects[0][2], expected_width, "strip width at k={k}");
            assert_eq!(rects[0][3], 30.0, "strip height at k={k}");
        }
        let points_rects = line(
            DebugSettings {
                level: 1,
                rate: "frames",
            },
            PixelSize {
                width: 1600,
                height: 1200,
            },
            (800, 600),
            None,
        );
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let fonts = crate::font::load_all(&assets).expect("shipped fonts");
        let point_width = f64::from(
            "-- fps"
                .chars()
                .map(|ch| fonts.standard_glyph(ch).advance)
                .sum::<u32>()
                + 6,
        );
        assert_eq!(points_rects[0], [8.0, 562.0, point_width * 2.0, 30.0]);
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "debug line geometry uses exact integral test inputs"
    )]
    fn points_window_keeps_strip_padding_and_glyphs_at_ui_pixel_scale_after_resize() {
        let pixels = PixelSize {
            width: 1600,
            height: 1200,
        };
        let settings = DebugSettings {
            level: 1,
            rate: "frames",
        };
        let (mut layer, seen, _, _, _) = test_layer(None, pixels, (800, 600), None);
        layer.update(
            settings,
            WindowMetrics {
                points: (800, 600),
                pixels,
            },
            None,
        );
        let first = seen
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .top_layer
            .as_ref()
            .unwrap()
            .quads()
            .iter()
            .map(|quad| [quad.x, quad.y, quad.width, quad.height])
            .collect::<Vec<_>>();

        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let fonts = crate::font::load_all(&assets).expect("shipped fonts");
        let text = "-- fps";
        let content_width = text
            .chars()
            .map(|character| fonts.standard_glyph(character).advance)
            .sum::<u32>();
        assert_eq!(
            first[0],
            [8.0, 562.0, f64::from(content_width + 6) * 2.0, 30.0]
        );
        let mut pen = 0.0;
        let mut glyph_index = 1;
        for character in text.chars() {
            let glyph = fonts.standard_glyph(character);
            if glyph.rows.iter().any(|row| row.contains('#')) {
                assert_eq!(
                    first[glyph_index],
                    [
                        8.0 + 6.0 + pen * 2.0,
                        566.0,
                        f64::from(u32::try_from(glyph.width()).unwrap_or(1)) * 2.0,
                        f64::from(u32::try_from(glyph.rows.len()).unwrap_or(1)) * 2.0,
                    ],
                    "glyph {character:?} at first placement"
                );
                glyph_index += 1;
            }
            pen += f64::from(glyph.advance);
        }
        assert_eq!(glyph_index, first.len());

        let resized_pixels = PixelSize {
            width: 1600,
            height: 1400,
        };
        layer.update(
            settings,
            WindowMetrics {
                points: (800, 700),
                pixels: resized_pixels,
            },
            None,
        );
        let resized = seen
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .top_layer
            .as_ref()
            .unwrap()
            .quads()
            .iter()
            .map(|quad| [quad.x, quad.y, quad.width, quad.height])
            .collect::<Vec<_>>();
        assert_eq!(resized.len(), first.len());
        for (before, after) in first.iter().zip(&resized) {
            assert_eq!(after[0], before[0], "resize preserves left position");
            assert_eq!(
                after[1],
                before[1] + 100.0,
                "resize moves each quad with the strip"
            );
            assert_eq!(after[2], before[2], "resize preserves width");
            assert_eq!(after[3], before[3], "resize preserves height");
        }
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "integer geometry uses exact test values")]
    fn line_frame_has_the_strip_and_glyphs_with_independent_theme_fallbacks() {
        let cases = [
            (None, [0xec, 0xe3, 0xcb, 0xff], [0x15, 0x11, 0x0a, 0xff]),
            (
                Some("[colour]\ntext = '#112233'\nbody = '#445566'\n"),
                [0x11, 0x22, 0x33, 0xff],
                [0x44, 0x55, 0x66, 0xff],
            ),
            (
                Some("[colour]\ntext = '#112233'\n"),
                [0x11, 0x22, 0x33, 0xff],
                [0x15, 0x11, 0x0a, 0xff],
            ),
            (
                Some("[colour]\nbody = '#445566'\n"),
                [0xec, 0xe3, 0xcb, 0xff],
                [0x44, 0x55, 0x66, 0xff],
            ),
        ];
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let fonts = crate::font::load_all(&assets).expect("shipped fonts");
        let visible_glyphs = "-- fps"
            .chars()
            .filter(|character| {
                fonts
                    .standard_glyph(*character)
                    .rows
                    .iter()
                    .any(|row| row.contains('#'))
            })
            .count();
        for (theme, text, body) in cases {
            let (mut layer, seen, _, _, _) = test_layer(
                Some(4),
                PixelSize {
                    width: 2560,
                    height: 1600,
                },
                (1280, 800),
                theme,
            );
            layer.update(
                DebugSettings {
                    level: 1,
                    rate: "frames",
                },
                WindowMetrics {
                    points: (1280, 800),
                    pixels: PixelSize {
                        width: 2560,
                        height: 1600,
                    },
                },
                Some(4),
            );
            let latest = seen.latest.lock().unwrap();
            let quads = latest.as_ref().unwrap().top_layer.as_ref().unwrap().quads();
            assert_eq!(
                quads.len(),
                visible_glyphs + 1,
                "one strip and visible glyphs"
            );
            assert_eq!(quads[0].tint.rgba(), body, "body colour fallback");
            assert!(quads[1..].iter().all(|quad| quad.tint.rgba() == text));
            assert_eq!(
                [quads[0].x, quads[0].y, quads[0].height],
                [8.0, 762.0, 30.0]
            );
        }
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "test geometry uses integral units")]
    fn line_changes_publish_only_when_needed_reuse_glyph_images_and_release_at_end() {
        let pixels = PixelSize {
            width: 2560,
            height: 1600,
        };
        let metrics = WindowMetrics {
            points: (1280, 800),
            pixels,
        };
        let (mut layer, seen, resources, now, bridge) =
            test_layer(Some(4), pixels, metrics.points, None);
        let settings = DebugSettings {
            level: 1,
            rate: "frames",
        };
        layer.update(settings, metrics, Some(4));
        assert_eq!(
            resources.stats().registry.live,
            5,
            "strip plus four cached glyphs"
        );
        let times = Arc::new((1..=60).map(|i| i * SECOND / 60).collect::<Vec<_>>());
        layer.set_frame_times(Some({
            let times = Arc::clone(&times);
            Arc::new(move |cursor| {
                times
                    .iter()
                    .copied()
                    .filter(|time| *time > cursor)
                    .collect()
            })
        }));
        now.store(SECOND / 2, Ordering::SeqCst);
        layer.update(settings, metrics, Some(4));
        now.store(SECOND + 1, Ordering::SeqCst);
        layer.update(settings, metrics, Some(4));
        assert_eq!(layer.last_text.as_deref(), Some("60 fps"));
        assert_eq!(
            resources.stats().registry.live,
            7,
            "only the two new digits were made"
        );
        let after_rate = seen.publications.load(Ordering::SeqCst);
        layer.update(settings, metrics, Some(4));
        assert_eq!(seen.publications.load(Ordering::SeqCst), after_rate);

        let resized_pixels = PixelSize {
            width: 2560,
            height: 1800,
        };
        let resized = WindowMetrics {
            points: (1280, 900),
            pixels: resized_pixels,
        };
        layer.update(settings, resized, Some(4));
        {
            let latest = seen.latest.lock().unwrap();
            let strip = &latest.as_ref().unwrap().top_layer.as_ref().unwrap().quads()[0];
            assert_eq!([strip.x, strip.y, strip.height], [8.0, 862.0, 30.0]);
        }
        assert_eq!(
            resources.stats().registry.live,
            7,
            "resize reuses cached images"
        );
        bridge.set_ui_scale_limit(Some(3));
        layer.update(settings, resized, Some(3));
        {
            let latest = seen.latest.lock().unwrap();
            let snapshot = latest.as_ref().unwrap();
            assert_eq!(snapshot.metrics.ui_scale_k, Some(3));
            let strip = &snapshot.top_layer.as_ref().unwrap().quads()[0];
            assert_eq!([strip.x, strip.y, strip.height], [8.0, 1162.0, 30.0]);
        }
        assert_eq!(
            resources.stats().registry.live,
            7,
            "scale change reuses cached images"
        );
        layer.end();
        assert_eq!(
            resources.stats().registry.live,
            0,
            "end drops every texture reference"
        );
        let latest = seen.latest.lock().unwrap();
        assert!(
            latest
                .as_ref()
                .unwrap()
                .top_layer
                .as_ref()
                .unwrap()
                .quads()
                .is_empty()
        );
        let _teardown = resources.finish_teardown();
        assert_eq!(resources.stats().loaders.referenced_at_teardown, 0);
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "integer geometry uses exact test values")]
    fn updates_rate_uses_the_given_accepted_update_times() {
        let pixels = PixelSize {
            width: 800,
            height: 600,
        };
        let points = (800, 600);
        let metrics = WindowMetrics { points, pixels };
        let (mut layer, seen, _, now, _) = test_layer(None, pixels, points, None);
        let settings = DebugSettings {
            level: 1,
            rate: "updates",
        };
        layer.update(settings, metrics, None);
        assert_eq!(layer.last_text.as_deref(), Some("-- updates"));
        for index in 1..=60 {
            now.store(index * SECOND / 60, Ordering::SeqCst);
            layer.accepted_update();
        }
        now.store(SECOND, Ordering::SeqCst);
        layer.update(settings, metrics, None);
        assert_eq!(layer.last_text.as_deref(), Some("60 updates"));
        let fonts = crate::font::load_all(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
        )
        .unwrap();
        let expected_width = f64::from(
            "60 updates"
                .chars()
                .map(|character| fonts.standard_glyph(character).advance)
                .sum::<u32>()
                + 6,
        );
        let latest = seen.latest.lock().unwrap();
        assert_eq!(
            latest.as_ref().unwrap().top_layer.as_ref().unwrap().quads()[0].width,
            expected_width * 2.0
        );
    }

    #[test]
    fn level_zero_publishes_no_quad_and_admits_no_texture() {
        let pixels = PixelSize {
            width: 1280,
            height: 800,
        };
        let points = (1280, 800);
        let (mut layer, seen, resources, _, _) = test_layer(Some(2), pixels, points, None);
        let before = seen.publications.load(Ordering::SeqCst);
        layer.update(
            DebugSettings {
                level: 0,
                rate: "frames",
            },
            WindowMetrics { points, pixels },
            Some(2),
        );
        assert_eq!(seen.publications.load(Ordering::SeqCst), before);
        assert_eq!(resources.stats().registry.live, 0);
        assert!(
            seen.latest
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .top_layer
                .is_none()
        );
    }
}
