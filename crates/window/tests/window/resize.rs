//! The resizable window (milestone 9, slice 12b): the description's new fields, their validation, the one resolution of a size in
//! pixels against the main screen, and (The loop's handoff of the size is tested with the update scheduler, in `update.rs`.)

use ck_window::{
    MAX_DIMENSION, Notifier, OpenError, PixelRequest, Requests, ScreenSnapshot, SizeUnit,
    WindowDesc,
};

/// A window of `width` by `height` in `unit` that can be resized, with `min` as its minimum.
fn resizable(unit: SizeUnit, width: u32, height: u32, min: Option<(u32, u32)>) -> WindowDesc {
    WindowDesc {
        title: "resizable".to_owned(),
        width,
        height,
        clear: [0.0; 3],
        unit,
        resizable: true,
        min,
        ..WindowDesc::default()
    }
}

/// A screen of `backing` pixels per point whose visible area is `visible` points.
fn screen(backing: f64, visible: (u32, u32)) -> ScreenSnapshot {
    ScreenSnapshot {
        backing,
        visible_points: visible,
    }
}

fn reason(error: OpenError) -> String {
    match error {
        OpenError::InvalidDesc { reason } => reason,
        other @ OpenError::AlreadyOpened => panic!("not an invalid description: {other:?}"),
    }
}

#[test]
fn each_refused_description_names_its_field() {
    let big = ScreenSnapshot::unlimited();
    let cases = [
        // A minimum of 0, or above the largest dimension, on either axis.
        (
            resizable(SizeUnit::Points, 800, 600, Some((0, 100))),
            "min_width",
        ),
        (
            resizable(SizeUnit::Points, 800, 600, Some((100, 0))),
            "min_height",
        ),
        (
            resizable(
                SizeUnit::Points,
                MAX_DIMENSION,
                600,
                Some((MAX_DIMENSION + 1, 100)),
            ),
            "min_width",
        ),
        (
            resizable(
                SizeUnit::Pixels,
                800,
                MAX_DIMENSION,
                Some((100, MAX_DIMENSION + 1)),
            ),
            "min_height",
        ),
        // A minimum larger than the opening size, on either axis, in the same unit.
        (
            resizable(SizeUnit::Points, 800, 600, Some((801, 100))),
            "min_width",
        ),
        (
            resizable(SizeUnit::Pixels, 1280, 800, Some((100, 801))),
            "min_height",
        ),
        // A minimum on a window that cannot be resized.
        (
            WindowDesc {
                resizable: false,
                ..resizable(SizeUnit::Points, 800, 600, Some((100, 100)))
            },
            "min_width",
        ),
        // The size itself, in pixels as in points.
        (resizable(SizeUnit::Pixels, 0, 800, None), "width"),
        (
            resizable(SizeUnit::Pixels, 1280, MAX_DIMENSION + 1, None),
            "height",
        ),
    ];
    for (desc, field) in cases {
        let refused = reason(desc.clone().resolve(&big).expect_err("refused"));
        assert!(refused.contains(field), "{desc:?}: {refused}");
    }
    // A description in pixels that was not resolved never reaches the request slot.
    let requests = Requests::new();
    let writer = requests.writer(Notifier::new());
    let reader = requests.reader();
    let refused = writer
        .open(resizable(SizeUnit::Pixels, 1280, 800, Some((1280, 800))))
        .expect_err("not resolved");
    assert!(reason(refused).contains("unit"), "names the unit");
    assert_eq!(reader.take(), ck_window::RequestChanges::default());
    // A valid description in pixels with a minimum is accepted, and so is a resizable one with none.
    resizable(SizeUnit::Pixels, 1280, 800, Some((1280, 800)))
        .resolve(&big)
        .expect("valid");
    resizable(SizeUnit::Points, 640, 400, None)
        .resolve(&big)
        .expect("valid");
}

#[test]
fn a_size_in_pixels_resolves_once_by_ceiling_division_by_the_backing() {
    let wide = (4000, 3000);
    for (backing, size, want) in [
        // 1280x800 pixels at backing 2 are 640x400 points, the minimum too.
        (2.0, (1280, 800), (640, 400)),
        // The ceiling: 1281x801 pixels at backing 2 are 641x401 points.
        (2.0, (1281, 801), (641, 401)),
        // Backing 1: the same numbers.
        (1.0, (1280, 800), (1280, 800)),
        // Not square: 1600x900 pixels at backing 2 are 800x450 points.
        (2.0, (1600, 900), (800, 450)),
    ] {
        let resolved = resizable(SizeUnit::Pixels, size.0, size.1, Some(size))
            .resolve(&screen(backing, wide))
            .expect("resolves");
        let desc = resolved.desc;
        assert_eq!((desc.width, desc.height), want, "backing {backing}");
        assert_eq!(desc.min, Some(want), "the minimum resolves the same way");
        assert_eq!(
            desc.unit,
            SizeUnit::Points,
            "the resolved description is in points"
        );
        assert!(resolved.clamps.is_empty(), "no clamp: the screen is large");
        // What the request was survives, in pixels, for the backend's recomputation and for the UI scale.
        assert_eq!(
            desc.pixel_request,
            Some(PixelRequest {
                size,
                min: Some(size)
            })
        );
    }
    // A request in points carries no pixel request, and a resizable window need not have a minimum.
    let points = resizable(SizeUnit::Points, 900, 600, None)
        .resolve(&screen(2.0, (10, 10)))
        .expect("resolves");
    assert_eq!(points.desc.pixel_request, None);
    assert_eq!((points.desc.width, points.desc.height), (900, 600));
    assert!(
        points.clamps.is_empty(),
        "a request in points is never clamped"
    );
    let no_min = resizable(SizeUnit::Pixels, 1280, 800, None)
        .resolve(&screen(2.0, wide))
        .expect("resolves");
    assert_eq!(no_min.desc.min, None);
    assert_eq!(
        no_min.desc.pixel_request,
        Some(PixelRequest {
            size: (1280, 800),
            min: None
        })
    );
}

#[test]
fn a_smaller_screen_lowers_the_size_and_the_minimum_and_never_raises_either() {
    let request = resizable(SizeUnit::Pixels, 1280, 800, Some((1280, 800)));
    // Visible 600x380 points at backing 2: the opening size and the minimum are both 600x380, two log lines.
    let resolved = request
        .clone()
        .resolve(&screen(2.0, (600, 380)))
        .expect("resolves");
    assert_eq!((resolved.desc.width, resolved.desc.height), (600, 380));
    assert_eq!(resolved.desc.min, Some((600, 380)));
    assert_eq!(resolved.clamps.len(), 2, "{:?}", resolved.clamps);
    assert!(resolved.clamps[0].contains("640x400") && resolved.clamps[0].contains("600x380"));
    // The pixels as requested are kept, before the clamp.
    assert_eq!(
        resolved.desc.pixel_request,
        Some(PixelRequest {
            size: (1280, 800),
            min: Some((1280, 800))
        })
    );
    // Visible 1440x900: nothing to clamp, nothing to log.
    let large = request
        .resolve(&screen(2.0, (1440, 900)))
        .expect("resolves");
    assert_eq!((large.desc.width, large.desc.height), (640, 400));
    assert!(large.clamps.is_empty());
    // A minimum smaller than the clamped window stays as it was (an axis that fits is not touched).
    let one_axis = resizable(SizeUnit::Pixels, 1280, 800, Some((200, 800)))
        .resolve(&screen(2.0, (640, 300)))
        .expect("resolves");
    assert_eq!((one_axis.desc.width, one_axis.desc.height), (640, 300));
    assert_eq!(one_axis.desc.min, Some((100, 300)));
}
