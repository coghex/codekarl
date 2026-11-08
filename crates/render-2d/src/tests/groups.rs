//! Scene groups: painter order, visibility, offsets, clips, hit regions, limits and the point domain (milestone 8, slice 4).
//!
//! A quad's identity in these tests is its width (`1 + id`), which a group's offset and clip never change.

use std::sync::Arc;

use ck_resources::{
    CpuPixels, HandleError, Mode, ResourceLimits, ResourceService, ResourceState, TextureRef,
    TokenKind,
};
use ck_window::PointerPosition;

use super::support::{RecordingSink, desc, pixels};
use crate::scene::EditScene;
use crate::{
    Filter, FrameQuad, GroupError, GroupKey, HitError, Metrics, POINT_DOMAIN, PointRect, QuadError,
    QuadKey, QuadSink, SceneBridge, SceneLimits, SceneLimitsError, SceneWriter,
};

fn service() -> ResourceService {
    ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("headless resources")
}

fn texture(service: &ResourceService) -> TextureRef<ResourceState> {
    let reservation = service.reserve_pixels(1, 1).expect("one pixel reservation");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("RGBA8 length");
    let key = service
        .create_from_pixels(pixels)
        .unwrap_or_else(|_| panic!("texture admitted"));
    service.retain(key).expect("public texture remains live")
}

/// A bridge attached to a recording sink (a 100 by 80 point window at 2 pixels per point) and its one writer.
struct Rig {
    bridge: Arc<SceneBridge>,
    writer: SceneWriter,
    sink: Arc<RecordingSink>,
    texture: TextureRef<ResourceState>,
    _service: ResourceService,
}

impl Rig {
    fn new() -> Self {
        Self::with(SceneLimits::DEFAULT)
    }

    fn with(limits: SceneLimits) -> Self {
        let service = service();
        let texture = texture(&service);
        let bridge = SceneBridge::with_limits(Filter::Nearest, limits).expect("valid limits");
        let sink = RecordingSink::new();
        let attached: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
        bridge
            .attach(&desc(100, 80), pixels(200, 160), attached)
            .expect("first attach");
        let writer = bridge.take_writer().expect("the one writer");
        Self {
            bridge,
            writer,
            sink,
            texture,
            _service: service,
        }
    }

    /// A quad with identity `id` (its width is `1 + id`) in `group`, at (0, 0).
    fn quad(&mut self, group: Option<GroupKey>, id: u32) -> QuadKey {
        self.quad_at(group, id, 0.0, 0.0)
    }

    fn quad_at(&mut self, group: Option<GroupKey>, id: u32, x: f64, y: f64) -> QuadKey {
        self.writer
            .create_quad_in(group, self.texture.clone(), x, y, f64::from(1 + id), 1.0)
            .expect("a quad")
    }

    /// Publishes and returns the frame's quads.
    fn frame(&mut self) -> Vec<FrameQuad> {
        assert!(self.writer.publish(), "the scene changed");
        self.sink
            .last()
            .expect("a snapshot")
            .scene
            .expect("a frame")
            .quads()
            .to_vec()
    }

    /// The identities of the frame's quads, bottom first.
    fn order(&mut self) -> Vec<u32> {
        self.frame().iter().map(identity).collect()
    }
}

fn identity(quad: &FrameQuad) -> u32 {
    let width = quad.width;
    assert!(width >= 1.0 && width.fract() == 0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small whole number"
    )]
    {
        width as u32 - 1
    }
}

fn position(x: f64, y: f64) -> PointerPosition {
    PointerPosition::from_points(x, y).expect("a position")
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> PointRect {
    PointRect::new(x, y, width, height)
}

fn hit_keys_at(rig: &Rig, x: f64, y: f64) -> Option<String> {
    rig.bridge
        .hit_regions()
        .expect("published regions")
        .key_at(position(x, y))
        .map(str::to_owned)
}

// ---------------------------------------------------------------------------------------------------------------------
// Tokens, limits
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn group_tokens_round_trip_and_are_refused_by_kind() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    let quad = rig.quad(None, 0);
    assert_eq!(GroupKey::from_token(&group.to_string()).unwrap(), group);
    assert_eq!(group.token().kind, TokenKind::Group);
    assert_eq!(
        GroupKey::from_token(&quad.to_string()),
        Err(HandleError::WrongKind {
            expected: TokenKind::Group,
            found: TokenKind::Quad
        })
    );
    assert_eq!(
        QuadKey::from_token(&group.to_string()),
        Err(HandleError::WrongKind {
            expected: TokenKind::Quad,
            found: TokenKind::Group
        })
    );
    assert!(GroupKey::from_token("ck1:group:zz").is_err());
}

#[test]
fn max_groups_is_validated_and_refuses_the_group_beyond_it_naming_the_limit() {
    assert_eq!(SceneLimits::default().max_groups, 4_096);
    for good in [1, 4_096, 65_536] {
        assert!(
            SceneLimits {
                max_groups: good,
                ..SceneLimits::DEFAULT
            }
            .validate()
            .is_ok(),
            "{good}"
        );
    }
    for bad in [0, 65_537] {
        let error = SceneLimits {
            max_groups: bad,
            ..SceneLimits::DEFAULT
        }
        .validate()
        .unwrap_err();
        assert_eq!(
            error,
            SceneLimitsError {
                field: "max_groups",
                value: u64::from(bad),
                min: 1,
                max: 65_536
            }
        );
    }
    let mut rig = Rig::with(SceneLimits {
        max_groups: 2,
        ..SceneLimits::DEFAULT
    });
    let first = rig.writer.create_group().unwrap();
    rig.writer.create_group().unwrap();
    let refused = rig.writer.create_group().unwrap_err();
    assert_eq!(refused, GroupError::Full { capacity: 2 });
    assert!(refused.to_string().contains("max_groups"), "{refused}");
    assert_eq!(rig.writer.group_count(), 2);
    // A destroyed group frees its place.
    rig.writer.destroy_group(first).unwrap();
    rig.writer.create_group().unwrap();
}

#[test]
fn max_quads_counts_the_quads_of_every_group() {
    let mut rig = Rig::with(SceneLimits {
        max_quads: 3,
        ..SceneLimits::DEFAULT
    });
    let a = rig.writer.create_group().unwrap();
    let b = rig.writer.create_group().unwrap();
    rig.quad(None, 0);
    rig.quad(Some(a), 1);
    rig.quad(Some(b), 2);
    let refused = rig
        .writer
        .create_quad_in(Some(a), rig.texture.clone(), 0.0, 0.0, 1.0, 1.0)
        .unwrap_err();
    assert_eq!(refused, QuadError::Full { capacity: 3 });
    assert_eq!(rig.writer.quad_count(), 3);
}

// ---------------------------------------------------------------------------------------------------------------------
// Painter order
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_quad_without_a_group_is_below_every_group_and_a_new_group_goes_on_top() {
    let mut rig = Rig::new();
    let base_first = rig.quad(None, 0);
    let first = rig.writer.create_group().unwrap();
    rig.quad(Some(first), 1);
    // Created later, but in the base group: still below every group.
    rig.quad(None, 2);
    let second = rig.writer.create_group().unwrap();
    rig.quad(Some(second), 3);
    rig.quad(Some(first), 4);
    assert_eq!(rig.order(), [0, 2, 1, 4, 3]);
    // The base quads keep their own creation order after the others change.
    rig.writer.destroy_quad(base_first).unwrap();
    assert_eq!(rig.order(), [2, 1, 4, 3]);
}

#[test]
fn raise_lower_and_raise_groups_give_literal_painter_orders() {
    let mut rig = Rig::new();
    rig.quad(None, 0);
    let groups: Vec<GroupKey> = (1..=3)
        .map(|id| {
            let group = rig.writer.create_group().unwrap();
            rig.quad(Some(group), id);
            group
        })
        .collect();
    let (g1, g2, g3) = (groups[0], groups[1], groups[2]);
    assert_eq!(rig.order(), [0, 1, 2, 3]);
    rig.writer.raise_group(g1).unwrap();
    assert_eq!(rig.order(), [0, 2, 3, 1]);
    rig.writer.lower_group(g1).unwrap();
    assert_eq!(rig.order(), [0, 1, 2, 3]);
    rig.writer.lower_group(g3).unwrap();
    assert_eq!(rig.order(), [0, 3, 1, 2]);
    // One edit: g3 and g1 go on top in that order, g1 topmost.
    rig.writer.raise_groups(&[g3, g1]).unwrap();
    assert_eq!(rig.order(), [0, 2, 3, 1]);
    // Raising the topmost group again changes nothing.
    rig.writer.raise_group(g1).unwrap();
    assert_eq!(rig.order(), [0, 2, 3, 1]);
    rig.writer.raise_groups(&[g1, g2]).unwrap();
    assert_eq!(rig.order(), [0, 3, 1, 2]);
}

#[test]
fn raise_groups_refuses_a_repeat_an_empty_list_and_a_stale_key_and_changes_nothing() {
    let mut rig = Rig::new();
    let a = rig.writer.create_group().unwrap();
    let b = rig.writer.create_group().unwrap();
    let dead = rig.writer.create_group().unwrap();
    rig.quad(Some(a), 0);
    rig.quad(Some(b), 1);
    rig.writer.destroy_group(dead).unwrap();
    assert_eq!(rig.order(), [0, 1]);
    assert_eq!(
        rig.writer.raise_groups(&[a, b, a]),
        Err(GroupError::BadList {
            reason: "names a group twice"
        })
    );
    assert_eq!(
        rig.writer.raise_groups(&[]),
        Err(GroupError::BadList { reason: "is empty" })
    );
    assert_eq!(
        rig.writer.raise_groups(&[a, dead]),
        Err(GroupError::Stale(dead))
    );
    // The scene did not change: nothing to publish, and the order is the same.
    assert!(!rig.writer.publish());
    rig.writer.raise_groups(&[b, a]).unwrap();
    assert_eq!(rig.order(), [1, 0]);
}

#[test]
fn a_hidden_group_is_absent_from_the_frame_and_returns_in_its_place() {
    let mut rig = Rig::new();
    let a = rig.writer.create_group().unwrap();
    let b = rig.writer.create_group().unwrap();
    rig.quad(Some(a), 0);
    rig.quad(Some(b), 1);
    rig.quad(Some(a), 2);
    assert_eq!(rig.order(), [0, 2, 1]);
    rig.writer.set_group_visible(a, false).unwrap();
    assert_eq!(rig.order(), [1]);
    // It keeps its order, offset and clip while hidden.
    rig.writer.set_group_offset(a, 5.0, 6.0).unwrap();
    rig.writer
        .set_group_clip(a, Some(rect(1.0, 2.0, 3.0, 4.0)))
        .unwrap();
    rig.writer.set_group_visible(a, true).unwrap();
    let frame = rig.frame();
    assert_eq!(frame.iter().map(identity).collect::<Vec<_>>(), [0, 2, 1]);
    assert_eq!((frame[0].x, frame[0].y), (5.0, 6.0));
    assert_eq!(frame[0].clip, Some(rect(1.0, 2.0, 3.0, 4.0)));
    assert_eq!(frame[2].clip, None);
}

// ---------------------------------------------------------------------------------------------------------------------
// Offset and clip
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn an_offset_translates_every_quad_of_the_group_and_only_that_group() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.quad_at(None, 0, 1.0, 2.0);
    rig.quad_at(Some(group), 1, 10.0, 20.0);
    rig.quad_at(Some(group), 2, -3.0, 0.5);
    rig.writer.set_group_offset(group, 100.0, -50.0).unwrap();
    let frame = rig.frame();
    assert_eq!(
        frame.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>(),
        [(1.0, 2.0), (110.0, -30.0), (97.0, -49.5)]
    );
    // The quads' own positions are unchanged by the offset: a move is relative to the group.
    rig.writer.set_group_offset(group, 0.0, 0.0).unwrap();
    assert_eq!(rig.frame()[1].x.to_bits(), 10.0_f64.to_bits());
}

#[test]
fn an_offset_that_is_not_whole_finite_and_inside_the_domain_is_refused_and_changes_nothing() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.quad(Some(group), 0);
    rig.writer.set_group_offset(group, 4.0, 5.0).unwrap();
    let _ = rig.frame();
    assert_eq!(
        rig.writer.set_group_offset(group, 1.5, 0.0),
        Err(GroupError::NotIntegral { field: "x" })
    );
    assert_eq!(
        rig.writer.set_group_offset(group, 0.0, 0.25),
        Err(GroupError::NotIntegral { field: "y" })
    );
    assert_eq!(
        rig.writer.set_group_offset(group, f64::NAN, 0.0),
        Err(GroupError::NotFinite { field: "x" })
    );
    assert_eq!(
        rig.writer.set_group_offset(group, 0.0, f64::INFINITY),
        Err(GroupError::NotFinite { field: "y" })
    );
    // The domain: ±8,388,607 is accepted, one more point is refused.
    assert!(
        rig.writer
            .set_group_offset(group, POINT_DOMAIN, -POINT_DOMAIN)
            .is_ok()
    );
    assert_eq!(
        rig.writer.set_group_offset(group, POINT_DOMAIN + 1.0, 0.0),
        Err(GroupError::OutsideDomain {
            field: "x",
            value: POINT_DOMAIN + 1.0
        })
    );
    assert_eq!(
        rig.writer.set_group_offset(group, 0.0, -POINT_DOMAIN - 1.0),
        Err(GroupError::OutsideDomain {
            field: "y",
            value: -POINT_DOMAIN - 1.0
        })
    );
    assert_eq!(POINT_DOMAIN.to_bits(), 8_388_607.0_f64.to_bits());
    let frame = rig.frame();
    assert_eq!((frame[0].x, frame[0].y), (POINT_DOMAIN, -POINT_DOMAIN));
}

#[test]
fn the_clip_is_absolute_and_the_offset_does_not_move_it() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.quad(Some(group), 0);
    let clip = rect(10.0, 20.0, 30.0, 40.0);
    rig.writer.set_group_clip(group, Some(clip)).unwrap();
    rig.writer.set_group_offset(group, 7.0, 9.0).unwrap();
    assert_eq!(rig.frame()[0].clip, Some(clip));
    rig.writer.set_group_clip(group, None).unwrap();
    assert_eq!(rig.frame()[0].clip, None);
}

#[test]
fn a_clip_is_validated_before_it_is_stored() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.quad(Some(group), 0);
    let good = rect(1.0, 2.0, 3.0, 4.0);
    rig.writer.set_group_clip(group, Some(good)).unwrap();
    let _ = rig.frame();
    let edge = POINT_DOMAIN;
    for (bad, expected) in [
        (
            rect(f64::NAN, 0.0, 1.0, 1.0),
            GroupError::NotFinite { field: "x" },
        ),
        (
            rect(0.0, f64::INFINITY, 1.0, 1.0),
            GroupError::NotFinite { field: "y" },
        ),
        (
            rect(0.0, 0.0, f64::NAN, 1.0),
            GroupError::NotFinite { field: "width" },
        ),
        (
            rect(0.0, 0.0, 0.0, 1.0),
            GroupError::BadSize {
                field: "width",
                value: 0.0,
            },
        ),
        (
            rect(0.0, 0.0, 1.0, -2.0),
            GroupError::BadSize {
                field: "height",
                value: -2.0,
            },
        ),
        (
            rect(-edge - 1.0, 0.0, 1.0, 1.0),
            GroupError::OutsideDomain {
                field: "left",
                value: -edge - 1.0,
            },
        ),
        (
            rect(edge, 0.0, 1.0, 1.0),
            GroupError::OutsideDomain {
                field: "right",
                value: edge + 1.0,
            },
        ),
        (
            rect(0.0, edge, 1.0, 1.0),
            GroupError::OutsideDomain {
                field: "bottom",
                value: edge + 1.0,
            },
        ),
    ] {
        assert_eq!(
            rig.writer.set_group_clip(group, Some(bad)),
            Err(expected),
            "{bad:?}"
        );
    }
    // The right edge exactly at the domain's edge is accepted.
    rig.writer
        .set_group_clip(group, Some(rect(edge - 10.0, 0.0, 10.0, 1.0)))
        .unwrap();
    // The refusals changed nothing: the accepted clip is the only change since the last frame.
    assert_eq!(rig.frame()[0].clip, Some(rect(edge - 10.0, 0.0, 10.0, 1.0)));
}

fn metrics(scale: u32) -> Metrics {
    Metrics {
        points_width: 100,
        points_height: 80,
        pixels: pixels(100 * scale, 80 * scale),
    }
}

fn placed_quad(
    texture: &TextureRef<ResourceState>,
    quad: (f64, f64, f64, f64),
    clip: Option<PointRect>,
    scale: u32,
) -> Option<crate::scene::Placement> {
    FrameQuad {
        texture: texture.clone(),
        x: quad.0,
        y: quad.1,
        width: quad.2,
        height: quad.3,
        clip,
        tint: crate::Tint::WHITE,
    }
    .placement(&metrics(scale))
}

fn destination(placement: &crate::scene::Placement) -> (i32, i32, i32, i32) {
    let d = placement.destination;
    (d.left, d.top, d.right, d.bottom)
}

fn source(placement: &crate::scene::Placement) -> (f32, f32, f32, f32) {
    let s = placement.source;
    (s.left, s.top, s.right, s.bottom)
}

#[test]
fn placement_against_a_clip_at_exact_pixel_edges_at_scale_one_and_two() {
    let service = service();
    let texture = texture(&service);
    let quad = (10.0, 10.0, 20.0, 20.0);
    let clip = rect(15.0, 12.0, 10.0, 30.0);
    // Scale 1: the visible part is x 15..25, y 12..30 (the quad ends at 30).
    let one = placed_quad(&texture, quad, Some(clip), 1).unwrap();
    assert_eq!(destination(&one), (15, 12, 25, 30));
    assert_eq!(source(&one), (0.25, 0.1, 0.75, 1.0));
    // Scale 2: every edge doubles, and the trimmed fractions are the same.
    let two = placed_quad(&texture, quad, Some(clip), 2).unwrap();
    assert_eq!(destination(&two), (30, 24, 50, 60));
    assert_eq!(source(&two), (0.25, 0.1, 0.75, 1.0));
    // No clip: the whole quad.
    let whole = placed_quad(&texture, quad, None, 1).unwrap();
    assert_eq!(destination(&whole), (10, 10, 30, 30));
    assert_eq!(source(&whole), (0.0, 0.0, 1.0, 1.0));
}

#[test]
fn a_clip_edge_is_rounded_by_the_rule_quads_use() {
    let service = service();
    let texture = texture(&service);
    let quad = (0.0, 0.0, 40.0, 40.0);
    // At scale 1 a half rounds away from zero: 10.5 -> 11 and 20.5 -> 21.
    let one = placed_quad(&texture, quad, Some(rect(10.5, 10.5, 10.0, 10.0)), 1).unwrap();
    assert_eq!(destination(&one), (11, 11, 21, 21));
    // At scale 2 the same clip is 21..41 (10.5 * 2 and 20.5 * 2 are whole).
    let two = placed_quad(&texture, quad, Some(rect(10.5, 10.5, 10.0, 10.0)), 2).unwrap();
    assert_eq!(destination(&two), (21, 21, 41, 41));
}

#[test]
fn a_clip_trims_the_source_exactly_as_the_window_edge_does() {
    let service = service();
    let texture = texture(&service);
    // The quad sticks out of the window on the left by 5 of 20 points: the window trims a quarter.
    let quad = (-5.0, 10.0, 20.0, 20.0);
    let by_window = placed_quad(&texture, quad, None, 1).unwrap();
    // The same cut made by a clip at x = 0 gives the same destination and the same source.
    let by_clip = placed_quad(
        &texture,
        (quad.0, quad.1, quad.2, quad.3),
        Some(rect(0.0, 0.0, 50.0, 80.0)),
        1,
    )
    .unwrap();
    assert_eq!(destination(&by_window), destination(&by_clip));
    assert_eq!(source(&by_window), source(&by_clip));
    assert_eq!(source(&by_window).0.to_bits(), 0.25_f32.to_bits());
    // Cut on the other side too: the quad is trimmed, not squashed (the span of the source matches the span of the destination).
    let both = placed_quad(
        &texture,
        (0.0, 0.0, 40.0, 40.0),
        Some(rect(10.0, 10.0, 20.0, 20.0)),
        1,
    )
    .unwrap();
    assert_eq!(destination(&both), (10, 10, 30, 30));
    assert_eq!(source(&both), (0.25, 0.25, 0.75, 0.75));
}

#[test]
fn a_quad_wholly_outside_its_clip_or_with_an_empty_overlap_is_not_placed() {
    let service = service();
    let texture = texture(&service);
    let quad = (10.0, 10.0, 20.0, 20.0);
    assert!(placed_quad(&texture, quad, Some(rect(50.0, 50.0, 5.0, 5.0)), 1).is_none());
    // Touching edges is no overlap.
    assert!(placed_quad(&texture, quad, Some(rect(30.0, 10.0, 10.0, 20.0)), 1).is_none());
    assert!(placed_quad(&texture, quad, Some(rect(0.0, 0.0, 10.0, 80.0)), 1).is_none());
    // A clip that rounds to no pixels at the window's scale clips everything.
    assert!(placed_quad(&texture, quad, Some(rect(15.0, 15.0, 0.25, 10.0)), 1).is_none());
    // The window still clips when the clip is larger than the window.
    let big = placed_quad(
        &texture,
        (90.0, 70.0, 20.0, 20.0),
        Some(rect(-1000.0, -1000.0, 5000.0, 5000.0)),
        1,
    )
    .unwrap();
    assert_eq!(destination(&big), (90, 70, 100, 80));
}

// ---------------------------------------------------------------------------------------------------------------------
// Destroying
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn destroying_a_group_removes_its_quads_and_hit_keys_and_makes_its_token_stale() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    let other = rig.writer.create_group().unwrap();
    rig.quad(None, 0);
    let inside_a = rig.quad(Some(group), 1);
    let inside_b = rig.quad(Some(group), 2);
    rig.quad(Some(other), 3);
    rig.writer
        .set_hit(
            "doomed",
            Some(vec![rect(0.0, 0.0, 10.0, 10.0)]),
            Some(group),
        )
        .unwrap();
    rig.writer
        .set_hit("kept", Some(vec![rect(20.0, 0.0, 10.0, 10.0)]), Some(other))
        .unwrap();
    assert_eq!(rig.order(), [0, 1, 2, 3]);
    assert_eq!(rig.writer.quad_count(), 4);
    rig.writer.destroy_group(group).unwrap();
    assert_eq!(rig.order(), [0, 3]);
    assert_eq!(rig.writer.quad_count(), 2);
    assert_eq!(rig.writer.group_count(), 1);
    assert_eq!(hit_keys_at(&rig, 5.0, 5.0), None);
    assert_eq!(hit_keys_at(&rig, 25.0, 5.0).as_deref(), Some("kept"));
    // Every token of the group is stale: the group, and the quads that were in it.
    assert_eq!(
        rig.writer.destroy_group(group),
        Err(GroupError::Stale(group))
    );
    assert_eq!(
        rig.writer.set_group_visible(group, false),
        Err(GroupError::Stale(group))
    );
    assert!(matches!(
        rig.writer.destroy_quad(inside_a),
        Err(QuadError::Stale(_))
    ));
    assert!(matches!(
        rig.writer.destroy_quad(inside_b),
        Err(QuadError::Stale(_))
    ));
    assert!(matches!(
        rig.writer
            .create_quad_in(Some(group), rig.texture.clone(), 0.0, 0.0, 1.0, 1.0),
        Err(QuadError::Group(GroupError::Stale(_)))
    ));
    // The key can be set again, in any group, now that its owner is gone; the slot's reuse does not revive the old token.
    let reused = rig.writer.create_group().unwrap();
    assert_eq!(reused.slot(), group.slot());
    assert_ne!(reused.generation(), group.generation());
    assert_eq!(rig.writer.raise_group(group), Err(GroupError::Stale(group)));
    rig.writer
        .set_hit(
            "doomed",
            Some(vec![rect(0.0, 0.0, 10.0, 10.0)]),
            Some(reused),
        )
        .unwrap();
}

#[test]
fn a_foreign_group_token_is_refused() {
    let mut rig = Rig::new();
    let mut other = Rig::new();
    let foreign = other.writer.create_group().unwrap();
    assert_eq!(
        rig.writer.raise_group(foreign),
        Err(GroupError::Foreign(foreign))
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Frame building
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn building_a_frame_visits_each_quad_once_across_groups() {
    let service = service();
    let texture = texture(&service);
    let registry = ck_resources::RegistryId::allocate().unwrap();
    let mut scene = EditScene::new(registry, 1_000, 8);
    let groups: Vec<GroupKey> = (0..3).map(|_| scene.create_group().unwrap()).collect();
    let mut expected = 0;
    for count in [7_usize, 11, 13, 17] {
        let group = match count {
            7 => None,
            11 => Some(groups[0]),
            13 => Some(groups[1]),
            _ => Some(groups[2]),
        };
        for _ in 0..count {
            scene
                .create(texture.clone(), 0.0, 0.0, 1.0, 1.0, group)
                .unwrap();
            expected += 1;
        }
    }
    scene.set_group_visible(groups[1], false).unwrap();
    let mut visits = 0;
    let mut target = Vec::new();
    scene.build_from(scene.visible_quads().inspect(|_| visits += 1), &mut target);
    assert_eq!(
        visits,
        expected - 13,
        "each visible quad once, hidden skipped"
    );
    assert_eq!(target.len(), visits);
}

// ---------------------------------------------------------------------------------------------------------------------
// Hit regions
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn hit_regions_follow_the_groups_offset_and_clip() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.writer
        .set_hit(
            "button",
            Some(vec![rect(10.0, 10.0, 20.0, 20.0)]),
            Some(group),
        )
        .unwrap();
    rig.writer
        .set_hit("plain", Some(vec![rect(60.0, 60.0, 10.0, 10.0)]), None)
        .unwrap();
    assert!(rig.writer.publish());
    assert_eq!(hit_keys_at(&rig, 15.0, 15.0).as_deref(), Some("button"));
    // The offset moves the region.
    rig.writer.set_group_offset(group, 20.0, 5.0).unwrap();
    assert!(rig.writer.publish());
    assert_eq!(hit_keys_at(&rig, 15.0, 15.0), None);
    assert_eq!(hit_keys_at(&rig, 35.0, 20.0).as_deref(), Some("button"));
    // The edges are exact: half-open, in pointer units.
    assert_eq!(hit_keys_at(&rig, 30.0, 15.0).as_deref(), Some("button"));
    assert_eq!(hit_keys_at(&rig, 49.99, 34.99).as_deref(), Some("button"));
    assert_eq!(hit_keys_at(&rig, 50.0, 20.0), None);
    // The clip is absolute and cuts the region: only x 40..45 survives.
    rig.writer
        .set_group_clip(group, Some(rect(40.0, 0.0, 5.0, 80.0)))
        .unwrap();
    assert!(rig.writer.publish());
    assert_eq!(hit_keys_at(&rig, 35.0, 20.0), None);
    assert_eq!(hit_keys_at(&rig, 41.0, 20.0).as_deref(), Some("button"));
    assert_eq!(hit_keys_at(&rig, 45.0, 20.0), None);
    let regions = rig.bridge.hit_regions().unwrap();
    assert_eq!(regions.regions().len(), 2);
    // A clip that leaves nothing of a rectangle drops it.
    rig.writer
        .set_group_clip(group, Some(rect(0.0, 0.0, 5.0, 5.0)))
        .unwrap();
    assert!(rig.writer.publish());
    assert_eq!(rig.bridge.hit_regions().unwrap().regions().len(), 1);
    assert_eq!(hit_keys_at(&rig, 65.0, 65.0).as_deref(), Some("plain"));
}

#[test]
fn hidden_groups_publish_no_regions_and_come_back_with_them() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.writer
        .set_hit(
            "button",
            Some(vec![rect(0.0, 0.0, 10.0, 10.0)]),
            Some(group),
        )
        .unwrap();
    assert!(rig.writer.publish());
    assert!(rig.bridge.hit_regions().unwrap().contains_key("button"));
    rig.writer.set_group_visible(group, false).unwrap();
    assert!(rig.writer.publish());
    assert!(!rig.bridge.hit_regions().unwrap().contains_key("button"));
    // Hidden regions still count against the caps, and the key stays the group's.
    assert_eq!(
        rig.writer
            .set_hit("button", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]), None),
        Err(HitError::GroupMismatch)
    );
    rig.writer.set_group_visible(group, true).unwrap();
    assert!(rig.writer.publish());
    assert!(rig.bridge.hit_regions().unwrap().contains_key("button"));
}

#[test]
fn regions_are_ordered_by_group_then_by_first_set_and_the_topmost_wins() {
    let mut rig = Rig::new();
    let low = rig.writer.create_group().unwrap();
    let high = rig.writer.create_group().unwrap();
    let all = rect(0.0, 0.0, 50.0, 50.0);
    // Set in an order that is not the final order: base, high, low, base again, low second key.
    rig.writer.set_hit("base_a", Some(vec![all]), None).unwrap();
    rig.writer
        .set_hit("high_a", Some(vec![all]), Some(high))
        .unwrap();
    rig.writer
        .set_hit("low_a", Some(vec![all]), Some(low))
        .unwrap();
    rig.writer.set_hit("base_b", Some(vec![all]), None).unwrap();
    rig.writer
        .set_hit("low_b", Some(vec![all]), Some(low))
        .unwrap();
    // Replacing a key keeps its place.
    rig.writer.set_hit("base_a", Some(vec![all]), None).unwrap();
    assert!(rig.writer.publish());
    let keys = |rig: &Rig| -> Vec<String> {
        rig.bridge
            .hit_regions()
            .unwrap()
            .regions()
            .iter()
            .map(|region| region.key().to_owned())
            .collect()
    };
    assert_eq!(keys(&rig), ["base_a", "base_b", "low_a", "low_b", "high_a"]);
    assert_eq!(hit_keys_at(&rig, 5.0, 5.0).as_deref(), Some("high_a"));
    rig.writer.raise_group(low).unwrap();
    assert!(rig.writer.publish());
    assert_eq!(keys(&rig), ["base_a", "base_b", "high_a", "low_a", "low_b"]);
    assert_eq!(hit_keys_at(&rig, 5.0, 5.0).as_deref(), Some("low_b"));
    // A removed key that is set again goes on top of its group.
    rig.writer.set_hit("low_a", None, None).unwrap();
    rig.writer
        .set_hit("low_a", Some(vec![all]), Some(low))
        .unwrap();
    assert!(rig.writer.publish());
    assert_eq!(keys(&rig), ["base_a", "base_b", "high_a", "low_b", "low_a"]);
}

#[test]
fn a_key_keeps_the_group_it_was_first_set_in() {
    let mut rig = Rig::new();
    let a = rig.writer.create_group().unwrap();
    let b = rig.writer.create_group().unwrap();
    let all = rect(0.0, 0.0, 10.0, 10.0);
    rig.writer.set_hit("key", Some(vec![all]), Some(a)).unwrap();
    assert_eq!(
        rig.writer.set_hit("key", Some(vec![all]), Some(b)),
        Err(HitError::GroupMismatch)
    );
    assert_eq!(
        rig.writer.set_hit("key", Some(vec![all]), None),
        Err(HitError::GroupMismatch)
    );
    rig.writer.set_hit("plain", Some(vec![all]), None).unwrap();
    assert_eq!(
        rig.writer.set_hit("plain", Some(vec![all]), Some(a)),
        Err(HitError::GroupMismatch)
    );
    // Removing needs no group; a named group must be the key's own.
    assert_eq!(
        rig.writer.set_hit("key", None, Some(b)),
        Err(HitError::GroupMismatch)
    );
    rig.writer.set_hit("key", None, None).unwrap();
    // Gone: it can now be set in another group.
    rig.writer.set_hit("key", Some(vec![all]), Some(b)).unwrap();
    // A stale group is refused by name.
    rig.writer.destroy_group(a).unwrap();
    assert!(matches!(
        rig.writer.set_hit("other", Some(vec![all]), Some(a)),
        Err(HitError::Group(GroupError::Stale(_)))
    ));
    // Nothing of the refusals reached the published regions.
    assert!(rig.writer.publish());
    let regions = rig.bridge.hit_regions().unwrap();
    assert_eq!(regions.regions().len(), 2);
}

#[test]
fn the_caps_count_rectangles_as_set_across_groups_and_before_clipping() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.writer
        .set_group_clip(group, Some(rect(0.0, 0.0, 1.0, 1.0)))
        .unwrap();
    let one = rect(500.0, 500.0, 1.0, 1.0);
    for index in 0..crate::HIT_RECTS_PER_SCENE / crate::HIT_RECTS_PER_KEY {
        let owner = if index % 2 == 0 { Some(group) } else { None };
        rig.writer
            .set_hit(
                &format!("key_{index:03}"),
                Some(vec![one; crate::HIT_RECTS_PER_KEY]),
                owner,
            )
            .unwrap();
    }
    // Every one is clipped away, but all count.
    assert!(matches!(
        rig.writer.set_hit("extra", Some(vec![one]), Some(group)),
        Err(HitError::SceneFull { .. })
    ));
    assert_eq!(
        rig.writer
            .set_hit("key_000", Some(vec![one; 17]), Some(group)),
        Err(HitError::BadRectCount { count: 17 })
    );
    rig.writer.set_hit("key_000", None, None).unwrap();
    rig.writer
        .set_hit("extra", Some(vec![one]), Some(group))
        .unwrap();
}

#[test]
fn a_hit_key_must_match_the_pattern_and_a_rectangle_must_have_an_extent() {
    let mut rig = Rig::new();
    let all = rect(0.0, 0.0, 10.0, 10.0);
    for bad in ["", "Bad", "a b", &"k".repeat(49)] {
        assert_eq!(
            rig.writer.set_hit(bad, Some(vec![all]), None),
            Err(HitError::BadKey),
            "{bad:?}"
        );
    }
    assert_eq!(
        rig.writer
            .set_hit("ok", Some(vec![rect(0.0, 0.0, 0.001, 1.0)]), None),
        Err(HitError::Empty)
    );
    assert_eq!(
        rig.writer.set_hit("ok", Some(Vec::new()), None),
        Err(HitError::BadRectCount { count: 0 })
    );
    assert!(matches!(
        rig.writer
            .set_hit("ok", Some(vec![rect(0.0, 0.0, -1.0, 1.0)]), None),
        Err(HitError::Group(GroupError::BadSize { field: "width", .. }))
    ));
    assert!(!rig.writer.publish(), "nothing changed");
}

// ---------------------------------------------------------------------------------------------------------------------
// The point domain
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_hit_edge_translated_to_the_domains_edge_is_accepted_and_one_point_further_is_refused() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    let edge = POINT_DOMAIN;
    rig.writer.set_group_offset(group, 100.0, 0.0).unwrap();
    // As set: a rectangle whose right edge is the domain's edge. Translated by 100 it would leave it, so the key is refused.
    assert!(matches!(
        rig.writer.set_hit(
            "far",
            Some(vec![rect(edge - 10.0, 0.0, 10.0, 1.0)]),
            Some(group)
        ),
        Err(HitError::Group(GroupError::OutsideDomain {
            field: "left",
            ..
        }))
    ));
    // Translated by 100 the right edge is exactly the domain's edge: accepted.
    rig.writer
        .set_hit(
            "far",
            Some(vec![rect(edge - 110.0, 0.0, 10.0, 1.0)]),
            Some(group),
        )
        .unwrap();
    // One point further: refused.
    assert!(matches!(
        rig.writer.set_hit(
            "farther",
            Some(vec![rect(edge - 109.0, 0.0, 10.0, 1.0)]),
            Some(group)
        ),
        Err(HitError::Group(GroupError::OutsideDomain {
            field: "right",
            ..
        }))
    ));
    // As set, outside the domain, with no offset at all: refused.
    assert!(matches!(
        rig.writer
            .set_hit("huge", Some(vec![rect(-edge - 1.0, 0.0, 1.0, 1.0)]), None),
        Err(HitError::Group(GroupError::OutsideDomain {
            field: "left",
            ..
        }))
    ));
    // The accepted key publishes exact edges.
    assert!(rig.writer.publish());
    let regions = rig.bridge.hit_regions().unwrap();
    assert_eq!(regions.regions().len(), 1);
    assert!(regions.key_contains("far", position(edge - 0.01, 0.5)));
    assert!(!regions.key_contains("far", position(edge - 100.0 - 10.5, 0.5)));
}

#[test]
fn an_offset_that_would_push_a_hit_rectangle_out_of_the_domain_is_refused_and_everything_stays() {
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.writer
        .set_hit("pin", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]), Some(group))
        .unwrap();
    rig.writer.set_group_offset(group, 50.0, 60.0).unwrap();
    assert!(rig.writer.publish());
    let before = rig.bridge.hit_regions().unwrap();
    let error = rig
        .writer
        .set_group_offset(group, POINT_DOMAIN, 0.0)
        .unwrap_err();
    assert_eq!(
        error,
        GroupError::HitOutsideDomain {
            key: "pin".to_owned(),
            edge: "right"
        }
    );
    assert!(error.to_string().contains("pin"), "{error}");
    // The old offset and regions stay: nothing to publish, and a publish after a change shows the old offset.
    assert!(!rig.writer.publish());
    rig.writer.set_group_visible(group, true).unwrap();
    assert!(rig.writer.publish());
    assert_eq!(*rig.bridge.hit_regions().unwrap(), *before);
    // Without the hit key the same offset is fine (the quads' own positions are unrestricted).
    rig.writer.set_hit("pin", None, None).unwrap();
    rig.writer
        .set_group_offset(group, POINT_DOMAIN, 0.0)
        .unwrap();
    // The offset at the negative boundary with a key whose left edge lands exactly on it.
    rig.writer
        .set_group_offset(group, -POINT_DOMAIN, 0.0)
        .unwrap();
    rig.writer
        .set_hit("edge", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]), Some(group))
        .unwrap();
    assert!(rig.writer.publish());
    assert!(
        rig.bridge
            .hit_regions()
            .unwrap()
            .key_contains("edge", position(-POINT_DOMAIN + 5.0, 5.0))
    );
    assert_eq!(
        rig.writer.set_group_offset(group, -POINT_DOMAIN - 1.0, 0.0),
        Err(GroupError::OutsideDomain {
            field: "x",
            value: -POINT_DOMAIN - 1.0
        })
    );
}

#[test]
fn a_far_quad_is_accepted_and_simply_not_placed() {
    let service = service();
    let texture = texture(&service);
    let mut rig = Rig::new();
    let group = rig.writer.create_group().unwrap();
    rig.quad_at(Some(group), 0, 1e300, -1e300);
    rig.writer
        .set_group_offset(group, POINT_DOMAIN, POINT_DOMAIN)
        .unwrap();
    let frame = rig.frame();
    assert!(frame[0].x.is_finite() && frame[0].y.is_finite());
    assert!(frame[0].place(&metrics(1)).is_none());
    drop(texture);
}

// ---------------------------------------------------------------------------------------------------------------------
// The scenes the backends read back (shared with `tests/*_group_readback.rs`)
// ---------------------------------------------------------------------------------------------------------------------

/// The five frames the backends draw place to exactly these instances, and the CPU reference of each means what it should (which
/// group is on top, what a hidden group draws, where an offset moves a quad, where a clip cuts and how the texels are trimmed).
/// This is the windowless half of the readback: the backend targets draw the very same instances and must equal the reference.
#[test]
fn the_readback_scenes_place_to_literal_instances_and_mean_what_they_say() {
    use super::group_readback::{Instance, Meaning, check, reference, scenes};
    let scenes = scenes();
    check(&scenes, &mut |pictures, instances| {
        reference(pictures, instances)
    });
    let instance = |image, destination, source| Instance {
        image,
        destination,
        source,
    };
    let frames: Vec<(Meaning, &[Instance])> = scenes
        .frames
        .iter()
        .map(|frame| (frame.meaning, frame.instances.as_slice()))
        .collect();
    let whole = [0.0, 0.0, 1.0, 1.0];
    assert_eq!(
        frames[0],
        (
            Meaning::GreenOnTop,
            &[
                instance(0, [0, 0, 40, 40], whole),
                instance(1, [20, 20, 60, 60], whole)
            ][..]
        )
    );
    assert_eq!(
        frames[1],
        (
            Meaning::RedOnTop,
            &[
                instance(1, [20, 20, 60, 60], whole),
                instance(0, [0, 0, 40, 40], whole)
            ][..]
        )
    );
    assert_eq!(
        frames[2],
        (
            Meaning::GreenHidden,
            &[instance(0, [0, 0, 40, 40], whole)][..]
        )
    );
    assert_eq!(
        frames[3],
        (
            Meaning::RedMoved,
            &[instance(0, [12, 8, 52, 48], whole)][..]
        )
    );
    // The content strip is cut at the clip: its destination is the clip's pixels and its source the two middle texels of four; the
    // chrome bar is above it.
    assert_eq!(
        frames[4],
        (
            Meaning::ChromeAndContent,
            &[
                instance(2, [24, 20, 56, 36], [0.25, 0.0, 0.75, 1.0]),
                instance(0, [0, 4, 80, 16], whole)
            ][..]
        )
    );
}
