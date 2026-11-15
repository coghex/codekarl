//! Pure window-mode geometry. Native backends apply this decision; tests never change a real window mode.

use std::fmt;

use crate::desc::MAX_DIMENSION;
use crate::desc::ScreenSnapshot;

/// The game's requested presentation mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    /// The ordinary titled, resizable window.
    #[default]
    Windowed,
    /// A borderless window using the screen's pixels.
    WindowedFullscreen,
    /// A borderless window presenting the chosen pixels in a shape-preserving viewport.
    Fullscreen,
}

/// How a resolution change is applied in the current mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeAction {
    /// Resize the ordinary window in points.
    Window,
    /// Keep the borderless screen window fixed.
    Ignore,
    /// Update the fullscreen drawable without resizing its window.
    Drawable,
}

impl WindowMode {
    /// The resolution-change path after any prior sequence of mode changes.
    #[must_use]
    pub const fn resize_action(self) -> ResizeAction {
        match self {
            Self::Windowed => ResizeAction::Window,
            Self::WindowedFullscreen => ResizeAction::Ignore,
            Self::Fullscreen => ResizeAction::Drawable,
        }
    }

    /// The stable configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windowed => "windowed",
            Self::WindowedFullscreen => "windowed_fullscreen",
            Self::Fullscreen => "fullscreen",
        }
    }

    /// The window and drawable geometry for this mode.
    #[must_use]
    pub fn layout(
        self,
        screen_points: (u32, u32),
        screen_pixels: (u32, u32),
        resolution_kept: (u32, u32),
    ) -> ModeLayout {
        let screen_pixels = (
            screen_pixels.0.min(MAX_DIMENSION),
            screen_pixels.1.min(MAX_DIMENSION),
        );
        let used = match self {
            Self::WindowedFullscreen => screen_pixels,
            Self::Windowed | Self::Fullscreen => (
                resolution_kept.0.min(screen_pixels.0),
                resolution_kept.1.min(screen_pixels.1),
            ),
        };
        match self {
            Self::Windowed => {
                let scale_x = f64::from(screen_pixels.0) / f64::from(screen_points.0.max(1));
                let scale_y = f64::from(screen_pixels.1) / f64::from(screen_points.1.max(1));
                let scale = if scale_x.is_finite() && scale_x > 0.0 {
                    scale_x
                } else {
                    scale_y.max(1.0)
                };
                let points = (
                    points_from_pixels(used.0, scale),
                    points_from_pixels(used.1, scale),
                );
                ModeLayout {
                    mode: self,
                    borderless: false,
                    outer_points: points,
                    picture: ModeRect {
                        x: 0,
                        y: 0,
                        width: points.0,
                        height: points.1,
                    },
                    points,
                    pixels: used,
                    resolution_kept,
                }
            }
            Self::WindowedFullscreen => ModeLayout {
                mode: self,
                borderless: true,
                outer_points: screen_points,
                picture: ModeRect {
                    x: 0,
                    y: 0,
                    width: screen_points.0,
                    height: screen_points.1,
                },
                points: screen_points,
                pixels: used,
                resolution_kept,
            },
            Self::Fullscreen => {
                let picture = fit_rect(screen_points, used);
                ModeLayout {
                    mode: self,
                    borderless: true,
                    outer_points: screen_points,
                    points: (picture.width, picture.height),
                    picture,
                    pixels: used,
                    resolution_kept,
                }
            }
        }
    }
}

impl ScreenSnapshot {
    /// Recompute a mode layout from the screen being used for that mode entry.
    #[must_use]
    pub fn mode_layout(self, mode: WindowMode, resolution_kept: (u32, u32)) -> ModeLayout {
        let backing = if self.backing.is_finite() && self.backing > 0.0 {
            self.backing
        } else {
            1.0
        };
        let pixels = |points: u32| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a finite, nonnegative screen-pixel conversion clamped to u32"
            )]
            {
                (f64::from(points) * backing)
                    .round()
                    .clamp(0.0, f64::from(u32::MAX)) as u32
            }
        };
        let points = match mode {
            WindowMode::Windowed => self.visible_points,
            WindowMode::WindowedFullscreen | WindowMode::Fullscreen => self.screen_points,
        };
        mode.layout(
            points,
            (pixels(points.0), pixels(points.1)),
            resolution_kept,
        )
    }
}

fn points_from_pixels(pixels: u32, scale: f64) -> u32 {
    let points = (f64::from(pixels) / scale)
        .ceil()
        .clamp(1.0, f64::from(MAX_DIMENSION));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the clamped value is finite and non-negative"
    )]
    let points = points as u32;
    points
}

impl fmt::Display for WindowMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&str> for WindowMode {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "windowed" => Ok(Self::Windowed),
            "windowed_fullscreen" => Ok(Self::WindowedFullscreen),
            "fullscreen" => Ok(Self::Fullscreen),
            _ => Err(format!(
                "window mode {value:?} is not accepted: use windowed, windowed_fullscreen or fullscreen"
            )),
        }
    }
}

/// A whole-point rectangle in the window's content.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ModeRect {
    /// Distance from the content's left edge.
    pub x: u32,
    /// Distance from the content's top edge.
    pub y: u32,
    /// Width in points.
    pub width: u32,
    /// Height in points.
    pub height: u32,
}

/// The mode's complete, coherent window and drawable report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeLayout {
    /// The selected mode.
    pub mode: WindowMode,
    /// Whether the native window has no title or border.
    pub borderless: bool,
    /// The whole content rectangle in points.
    pub outer_points: (u32, u32),
    /// Where the drawable is presented inside the content rectangle.
    pub picture: ModeRect,
    /// The points reported to the game; fullscreen reports the picture rectangle.
    pub points: (u32, u32),
    /// The unchanged drawable size in pixels.
    pub pixels: (u32, u32),
    /// The selected resolution before limiting it to this screen.
    pub resolution_kept: (u32, u32),
}

impl ModeLayout {
    /// Translate a whole-point position from the content origin into the picture's origin.
    #[must_use]
    pub fn pointer_from_window(self, position: (i32, i32)) -> (i32, i32) {
        (
            position.0 - i32::try_from(self.picture.x).unwrap_or(i32::MAX),
            position.1 - i32::try_from(self.picture.y).unwrap_or(i32::MAX),
        )
    }
}

fn fit_rect(screen: (u32, u32), pixels: (u32, u32)) -> ModeRect {
    let (screen_width, screen_height) = (screen.0.max(1), screen.1.max(1));
    let (pixel_width, pixel_height) = (pixels.0.max(1), pixels.1.max(1));
    let width_limited = u64::from(screen_width) * u64::from(pixel_height)
        <= u64::from(screen_height) * u64::from(pixel_width);
    let (width, height) = if width_limited {
        (
            screen_width,
            u32::try_from(
                u64::from(screen_width) * u64::from(pixel_height) / u64::from(pixel_width),
            )
            .unwrap_or(screen_height)
            .min(screen_height),
        )
    } else {
        (
            u32::try_from(
                u64::from(screen_height) * u64::from(pixel_width) / u64::from(pixel_height),
            )
            .unwrap_or(screen_width)
            .min(screen_width),
            screen_height,
        )
    };
    ModeRect {
        x: (screen_width - width) / 2,
        y: (screen_height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_DIMENSION, ModeRect, ResizeAction, ScreenSnapshot, WindowMode};

    #[test]
    fn resize_action_tracks_the_current_mode_after_round_trips() {
        let sequence = [
            WindowMode::Windowed,
            WindowMode::Fullscreen,
            WindowMode::Windowed,
            WindowMode::WindowedFullscreen,
            WindowMode::Windowed,
        ];
        assert_eq!(
            sequence.map(WindowMode::resize_action),
            [
                ResizeAction::Window,
                ResizeAction::Drawable,
                ResizeAction::Window,
                ResizeAction::Ignore,
                ResizeAction::Window,
            ]
        );
    }

    #[test]
    fn covering_layout_uses_the_whole_current_screen_and_windowed_keeps_the_visible_area() {
        let screen = ScreenSnapshot {
            backing: 2.0,
            screen_points: (1920, 1200),
            visible_points: (1920, 1080),
        };
        let covering = screen.mode_layout(WindowMode::WindowedFullscreen, (1280, 800));
        assert_eq!(covering.outer_points, (1920, 1200));
        assert_eq!(covering.points, (1920, 1200));
        assert_eq!(covering.pixels, (3840, 2400));

        let windowed = screen.mode_layout(WindowMode::Windowed, (1280, 800));
        assert_eq!(windowed.outer_points, (640, 400));
        assert_eq!(windowed.pixels, (1280, 800));

        let moved = ScreenSnapshot {
            backing: 1.0,
            screen_points: (2560, 1440),
            visible_points: (2560, 1400),
        }
        .mode_layout(WindowMode::Fullscreen, (1280, 800));
        assert_eq!(moved.outer_points, (2560, 1440));
        assert_eq!(moved.pixels, (1280, 800));
    }

    #[test]
    fn fullscreen_picture_fits_the_screen_without_stretching_and_centres_odd_remainders_right() {
        let layout = WindowMode::Fullscreen.layout((1920, 1080), (1920, 1080), (1281, 800));

        assert_eq!(layout.outer_points, (1920, 1080));
        assert_eq!(
            layout.picture,
            ModeRect {
                x: 95,
                y: 0,
                width: 1729,
                height: 1080
            }
        );
        assert_eq!(layout.points, (1729, 1080));
        assert_eq!(layout.pixels, (1281, 800));
        assert_eq!(layout.pointer_from_window((95, 0)), (0, 0));
        assert_eq!(layout.pointer_from_window((96, 540)), (1, 540));
        assert_eq!(layout.pointer_from_window((94, 540)), (-1, 540));
    }

    #[test]
    fn fullscreen_picture_uses_points_and_pixels_as_a_coherent_report() {
        let layout = WindowMode::Fullscreen.layout((1920, 1080), (1920, 1080), (1280, 800));

        assert_eq!(
            layout.picture,
            ModeRect {
                x: 96,
                y: 0,
                width: 1728,
                height: 1080
            }
        );
        assert_eq!(layout.points, (1728, 1080));
        assert_eq!(layout.pixels, (1280, 800));
        assert_eq!(layout.pointer_from_window((960, 540)), (864, 540));
        assert_eq!(layout.pointer_from_window((1824, 540)), (1728, 540));
    }

    #[test]
    fn windowed_fullscreen_uses_the_screen_and_windowed_uses_the_kept_size() {
        let borderless =
            WindowMode::WindowedFullscreen.layout((1920, 1080), (3840, 2160), (1600, 900));
        assert_eq!(
            borderless.picture,
            ModeRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(borderless.pixels, (3840, 2160));

        let windowed = WindowMode::Windowed.layout((1920, 1080), (3840, 2160), (1600, 900));
        assert_eq!(windowed.pixels, (1600, 900));
    }

    #[test]
    fn fullscreen_geometry_covers_tall_and_two_pixels_per_point_screens() {
        let tall = WindowMode::Fullscreen.layout((1920, 1200), (1920, 1200), (1920, 1080));
        assert_eq!(
            tall.picture,
            ModeRect {
                x: 0,
                y: 60,
                width: 1920,
                height: 1080
            }
        );

        let scaled = WindowMode::Fullscreen.layout((1920, 1080), (3840, 2160), (2560, 1600));
        assert_eq!(
            scaled.picture,
            ModeRect {
                x: 96,
                y: 0,
                width: 1728,
                height: 1080
            }
        );
        assert_eq!(scaled.pixels, (2560, 1600));
        assert_eq!(scaled.pointer_from_window((96, 0)), (0, 0));
        assert_eq!(scaled.pointer_from_window((960, 540)), (864, 540));
        assert_eq!(scaled.pointer_from_window((1824, 1080)), (1728, 1080));
        assert_eq!(scaled.pointer_from_window((95, 540)), (-1, 540));
    }

    #[test]
    fn fullscreen_drawable_is_screen_limited_and_never_larger_than_the_dimension_cap() {
        let small = WindowMode::Fullscreen.layout((1024, 768), (1024, 768), (1280, 800));
        assert_eq!(
            small.picture,
            ModeRect {
                x: 0,
                y: 0,
                width: 1024,
                height: 768
            }
        );
        assert_eq!(small.pixels, (1024, 768));

        let large =
            WindowMode::Fullscreen.layout((20_000, 12_000), (20_000, 12_000), (20_000, 12_000));
        assert_eq!(large.pixels, (MAX_DIMENSION, 12_000));
        assert_eq!(
            large.picture,
            ModeRect {
                x: 1808,
                y: 0,
                width: 16_384,
                height: 12_000
            }
        );
    }

    #[test]
    fn mode_names_are_stable_and_unknown_values_name_the_rejected_value() {
        for (text, mode) in [
            ("windowed", WindowMode::Windowed),
            ("windowed_fullscreen", WindowMode::WindowedFullscreen),
            ("fullscreen", WindowMode::Fullscreen),
        ] {
            assert_eq!(WindowMode::try_from(text).expect("known mode"), mode);
            assert_eq!(mode.as_str(), text);
        }
        assert!(
            WindowMode::try_from("exclusive")
                .expect_err("unknown value")
                .contains("exclusive")
        );
    }
}
