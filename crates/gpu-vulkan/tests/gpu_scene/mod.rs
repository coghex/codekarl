//! The scene presenter's drawing, off screen (milestone 5.5, issue 2a): the geometry, colour and painter order of an
//! instanced, batched scene read back through the presenter's own frame building and drawing code
//! (`readback_scene`), the interleaved samplers, the 32,768-texture baseline, and stable identity across movement and
//! reload. A submodule of `gpu.rs`, so the group that runs that target runs these.
//!
//! Every case runs under `VK_LAYER_KHRONOS_validation` **with synchronization validation** and requires an empty log
//! from the creation of the instance to its destruction. A negative control is a case, or a part of one, that shows the
//! check it names would fail without the behaviour.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::{
    BindingChoice, BindlessLimits, CheckerProgress, DestinationRect, HeldCompat, InstanceRole,
    LedgerCleanup, LedgerEvent, LedgerLimits, PixelSource, TextureLedger, TextureLimits,
    TextureTestHooks, TextureWakes, VulkanFilter, VulkanFrame, VulkanInstance, VulkanSceneSource,
    checker_pixels, readback_scene, readback_scene_with_held,
};

use super::gpu_textures::{Bytes, Kit, PATIENCE, pattern};

const PICK: Duration = Duration::from_millis(1);

/// How a case's ledger binds its textures. The cases that read pixels run in both, against the same reference: "the same scene
/// draws identical pixels" in the per-revision sets (CK-156/157) and through the bindless table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    PerRevision,
    Bindless,
    /// The gate decides for this slot count (a case that expects the fallback, or a small table).
    Auto(u32),
}

pub(crate) const MODES: [Mode; 2] = [Mode::PerRevision, Mode::Bindless];

/// The tables' size in the bindless mode: the final stage's, which the device gate accepts here.
pub(crate) const SLOTS: u32 = 131_072;

impl Mode {
    fn choice(self) -> BindingChoice {
        match self {
            Self::PerRevision => BindingChoice::PerRevision,
            Self::Bindless => BindingChoice::Auto(BindlessLimits {
                texture_slots: SLOTS,
            }),
            Self::Auto(texture_slots) => BindingChoice::Auto(BindlessLimits { texture_slots }),
        }
    }
}

/// The scene a case draws: its instances in painter order.
pub(crate) struct Quads(pub(crate) Vec<VulkanInstance<u32>>);

impl VulkanSceneSource<u32> for Quads {
    fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(VulkanInstance<u32>)) {
        for instance in &self.0 {
            out(*instance);
        }
    }
}

/// The checker as a pixel source.
struct CheckerBytes(Vec<u8>);

impl PixelSource for CheckerBytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

/// One shown image, as the reference needs it.
#[derive(Clone)]
pub(crate) struct Shown {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Arc<Bytes>,
}

/// A kit, a ledger with its checker resident, and the pixels of everything shown.
pub(crate) struct Rig {
    pub(crate) kit: Kit,
    pub(crate) ledger: Option<TextureLedger<u32>>,
    pub(crate) shown: HashMap<u32, Shown>,
    pub(crate) max_quads: usize,
}

pub(crate) fn instance(key: u32, destination: [i32; 4], source: [f32; 4]) -> VulkanInstance<u32> {
    VulkanInstance {
        key,
        destination: DestinationRect {
            left: destination[0],
            top: destination[1],
            right: destination[2],
            bottom: destination[3],
        },
        source,
        role: InstanceRole::Scene,
    }
}

pub(crate) const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

impl Rig {
    pub(crate) fn new(mode: Mode, max_textures: usize, extras: usize, max_quads: usize) -> Self {
        Self::with_limits(
            mode,
            max_textures,
            max_quads,
            TextureLimits::default(),
            LedgerLimits {
                max_extra_revisions: extras,
                ..LedgerLimits::default()
            },
        )
    }

    pub(crate) fn with_limits(
        mode: Mode,
        max_textures: usize,
        max_quads: usize,
        texture: TextureLimits,
        ledger_limits: LedgerLimits,
    ) -> Self {
        Self::with_hooks(
            mode,
            max_textures,
            max_quads,
            texture,
            ledger_limits,
            TextureTestHooks::default(),
        )
    }

    pub(crate) fn with_hooks(
        mode: Mode,
        max_textures: usize,
        max_quads: usize,
        texture: TextureLimits,
        ledger_limits: LedgerLimits,
        hooks: TextureTestHooks,
    ) -> Self {
        Self::with_kit(
            Kit::new(false),
            mode,
            max_textures,
            max_quads,
            texture,
            ledger_limits,
            hooks,
        )
    }

    /// A rig on `kit` (a case that holds frames makes it with the host-event feature).
    pub(crate) fn with_kit(
        kit: Kit,
        mode: Mode,
        max_textures: usize,
        max_quads: usize,
        texture: TextureLimits,
        ledger_limits: LedgerLimits,
        hooks: TextureTestHooks,
    ) -> Self {
        let ledger = TextureLedger::<u32>::start_with_binding(
            kit.context.clone(),
            texture,
            ledger_limits,
            max_textures,
            mode.choice(),
            TextureWakes::default(),
            hooks,
        )
        .expect("the ledger starts");
        match mode {
            Mode::Bindless => assert_eq!(
                ledger.bindless_slots(),
                SLOTS,
                "{mode:?}: this device passes the gate at {SLOTS}"
            ),
            Mode::PerRevision => assert_eq!(ledger.bindless_slots(), 0, "{mode:?}"),
            Mode::Auto(_) => {}
        }
        ledger
            .start_checker(64, 64, Arc::new(CheckerBytes(checker_pixels())))
            .expect("the checker is offered");
        let mut rig = Self {
            kit,
            ledger: Some(ledger),
            shown: HashMap::new(),
            max_quads,
        };
        rig.pump_until(|ledger, _| ledger.checker_progress() == CheckerProgress::Resident);
        rig
    }

    pub(crate) fn ledger(&self) -> &TextureLedger<u32> {
        self.ledger.as_ref().expect("the ledger exists")
    }

    /// Pumps until `done` is satisfied, failing after [`PATIENCE`].
    pub(crate) fn pump_until(
        &mut self,
        mut done: impl FnMut(&TextureLedger<u32>, &[LedgerEvent<u32>]) -> bool,
    ) -> Vec<LedgerEvent<u32>> {
        let mut events = Vec::new();
        let started = Instant::now();
        loop {
            let ledger = self.ledger.as_ref().expect("the ledger exists");
            ledger.pump(&mut self.kit.queue, &mut events);
            if done(ledger, &events) {
                return events;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "the condition never held; events so far: {} (first: {:?})",
                events.len(),
                events.first()
            );
            std::thread::sleep(PICK);
        }
    }

    /// Offers `pixels` as revision `revision` of `key`, pumps until it is prepared and commits it. Not adopted.
    pub(crate) fn prepare(
        &mut self,
        key: u32,
        revision: u64,
        width: u32,
        height: u32,
        pixels: &Arc<Bytes>,
    ) {
        self.ledger()
            .offer(
                key,
                revision,
                width,
                height,
                Arc::clone(pixels) as Arc<dyn PixelSource>,
            )
            .expect("the offer is admitted");
        self.pump_until(|_, events| {
            events.iter().any(|event| {
                matches!(event, LedgerEvent::Prepared { key: k, revision: r } if *k == key && *r == revision)
            })
        });
        self.ledger().commit(key, revision).expect("commit");
    }

    /// Adopts everything committed, at one boundary per call of the limit.
    pub(crate) fn adopt_all(&self) {
        let mut adopted = Vec::new();
        loop {
            let before = adopted.len();
            self.ledger().adopt(256, &mut adopted);
            if adopted.len() == before {
                break;
            }
        }
    }

    /// Shows `pixels` as the first revision of `key`.
    pub(crate) fn show(&mut self, key: u32, width: u32, height: u32, pixels: Arc<Bytes>) {
        self.prepare(key, 1, width, height, &pixels);
        self.adopt_all();
        self.shown.insert(
            key,
            Shown {
                width,
                height,
                pixels,
            },
        );
    }

    /// Draws `instances` off screen and returns the pixels with the counters.
    pub(crate) fn draw(
        &mut self,
        size: (u32, u32),
        clear: [f32; 3],
        filter: VulkanFilter,
        instances: Vec<VulkanInstance<u32>>,
    ) -> (Vec<u8>, ck_gpu_vulkan::FrameCounters) {
        let frame = VulkanFrame::new(1, filter, None, Some(Arc::new(Quads(instances))));
        let ledger = self.ledger.as_ref().expect("the ledger exists");
        readback_scene(
            &self.kit.context,
            &mut self.kit.queue,
            ledger,
            size,
            clear,
            &frame,
            self.max_quads,
        )
        .expect("the scene reads back")
    }

    /// Draws `frame` with the compatibility image `held` (the ledger's current hold), or reports why it was not drawn.
    pub(crate) fn try_draw(
        &mut self,
        size: (u32, u32),
        frame: &VulkanFrame<u32>,
        held: Option<HeldCompat<u32>>,
    ) -> Result<(Vec<u8>, ck_gpu_vulkan::FrameCounters), ck_gpu_vulkan::Error> {
        let ledger = self.ledger.as_ref().expect("the ledger exists");
        readback_scene_with_held(
            &self.kit.context,
            &mut self.kit.queue,
            ledger,
            size,
            [0.0; 3],
            frame,
            self.max_quads,
            held,
        )
    }

    pub(crate) fn finish(mut self) {
        let ledger = self.ledger.take().expect("the ledger exists");
        assert_eq!(
            ledger.shutdown(Instant::now() + PATIENCE),
            LedgerCleanup::Proven,
            "the end is proven"
        );
        drop(ledger);
        self.kit.finish();
    }
}

// ---- the CPU reference ----

/// A pixel of the reference: premultiplied RGBA8.
type Rgba = [u8; 4];

fn texel(image: &Shown, x: i64, y: i64) -> Rgba {
    let x = x.clamp(0, i64::from(image.width) - 1);
    let y = y.clamp(0, i64::from(image.height) - 1);
    let at = usize::try_from((y * i64::from(image.width) + x) * 4).expect("small");
    [
        image.pixels.0[at],
        image.pixels.0[at + 1],
        image.pixels.0[at + 2],
        image.pixels.0[at + 3],
    ]
}

/// Samples `image` at the texture coordinate `(u, v)` (fractions), clamped to the edge.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    reason = "image sides in these cases are tiny, and the bilinear weights read best with their usual letters"
)]
fn sample(image: &Shown, u: f32, v: f32, filter: VulkanFilter) -> [f32; 4] {
    match filter {
        VulkanFilter::Nearest => {
            let t = texel(
                image,
                (u * image.width as f32).floor() as i64,
                (v * image.height as f32).floor() as i64,
            );
            t.map(f32::from)
        }
        VulkanFilter::Linear => {
            let x = u * image.width as f32 - 0.5;
            let y = v * image.height as f32 - 0.5;
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let (a, b, c, d) = (
                texel(image, x0, y0),
                texel(image, x0 + 1, y0),
                texel(image, x0, y0 + 1),
                texel(image, x0 + 1, y0 + 1),
            );
            let mut out = [0.0_f32; 4];
            for channel in 0..4 {
                let top = f32::from(a[channel]) * (1.0 - fx) + f32::from(b[channel]) * fx;
                let bottom = f32::from(c[channel]) * (1.0 - fx) + f32::from(d[channel]) * fx;
                out[channel] = top * (1.0 - fy) + bottom * fy;
            }
            out
        }
    }
}

/// The reference frame: the clear, then each instance in the order given, premultiplied source-over, each texture
/// sampled with `filter` (the checker, which the cases name by key `u32::MAX`, always nearest).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    reason = "pixel coordinates in these cases are small"
)]
pub(crate) fn reference(
    size: (u32, u32),
    clear: [f32; 3],
    filter: VulkanFilter,
    shown: &HashMap<u32, Shown>,
    checker: &Shown,
    instances: &[VulkanInstance<u32>],
) -> Vec<u8> {
    let (width, height) = size;
    let mut frame = vec![0_u8; width as usize * height as usize * 4];
    for pixel in frame.chunks_exact_mut(4) {
        pixel[0] = (clear[0] * 255.0).round() as u8;
        pixel[1] = (clear[1] * 255.0).round() as u8;
        pixel[2] = (clear[2] * 255.0).round() as u8;
        pixel[3] = 255;
    }
    for instance in instances {
        let (image, how) = shown
            .get(&instance.key)
            .map_or((checker, VulkanFilter::Nearest), |s| (s, filter));
        let d = instance.destination;
        let [su, sv, eu, ev] = instance.source;
        let (dw, dh) = ((d.right - d.left) as f32, (d.bottom - d.top) as f32);
        for y in d.top.max(0)..d.bottom.min(height as i32) {
            for x in d.left.max(0)..d.right.min(width as i32) {
                let u = su + ((x - d.left) as f32 + 0.5) / dw * (eu - su);
                let v = sv + ((y - d.top) as f32 + 0.5) / dh * (ev - sv);
                let source = sample(image, u, v, how);
                let at = (y as usize * width as usize + x as usize) * 4;
                let alpha = source[3] / 255.0;
                for channel in 0..4 {
                    let blended = source[channel] + f32::from(frame[at + channel]) * (1.0 - alpha);
                    frame[at + channel] = blended.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    frame
}

/// The checker as the reference needs it.
pub(crate) fn checker_image() -> Shown {
    Shown {
        width: 64,
        height: 64,
        pixels: Arc::new(Bytes(checker_pixels())),
    }
}

/// How many pixels differ by more than `tolerance` in any channel.
pub(crate) fn differing(a: &[u8], b: &[u8], tolerance: u8) -> usize {
    assert_eq!(a.len(), b.len());
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(x, y)| {
            x.iter()
                .zip(y.iter())
                .any(|(p, q)| p.abs_diff(*q) > tolerance)
        })
        .count()
}

/// The instance list of case 1: 64 quads over 16 textures with adjacent and non-adjacent repeats, overlaps, partial
/// sources and clipped quads, on a 320 x 256 target. Every scale is a whole number of target pixels per texel, so
/// nearest sampling never lands on a texel edge.
fn sixty_four() -> Vec<VulkanInstance<u32>> {
    let mut list = Vec::new();
    for index in 0..64_u32 {
        // Repeats: pairs of equal keys in a row (adjacent), and the same keys again later (non-adjacent).
        let key = match index {
            0..=31 => index / 2,
            32..=47 => index - 32,
            _ => (index * 5) % 16,
        };
        let column = i32::try_from(index % 8).expect("small");
        let row = i32::try_from(index / 8).expect("small");
        // Quads of 40 x 40 pixels at a 36 x 30 pitch overlap their neighbours; the last columns and rows are clipped, and the
        // first starts outside the target.
        let left = column * 36 - 12;
        let top = row * 30 - 6;
        let source = match index % 4 {
            0 => WHOLE,
            1 => [0.25, 0.25, 0.75, 0.75],
            2 => [0.0, 0.5, 0.5, 1.0],
            _ => [0.5, 0.0, 1.0, 0.5],
        };
        list.push(instance(key, [left, top, left + 40, top + 40], source));
    }
    list
}

/// The adjacent-run count of `keys`: the batches a build must produce when every key resolves.
fn runs(list: &[VulkanInstance<u32>]) -> u64 {
    let mut runs = 0_u64;
    let mut previous = None;
    for instance in list {
        if previous != Some(instance.key) {
            runs += 1;
        }
        previous = Some(instance.key);
    }
    runs
}

/// Test 1: geometry, colour and order. 64 quads over 16 textures, read back exactly against the CPU reference; the
/// batch boundaries equal the adjacent-run count; painter order wins at every overlap.
///
/// Negative control: drawing the same quads sorted by texture changes an overlap's pixel, so the comparison would fail
/// for an implementation that sorts.
#[test]
fn sixty_four_quads_over_sixteen_textures_read_back_exactly_in_painter_order() {
    for mode in MODES {
        sixty_four_quads_over_sixteen_textures_read_back_exactly_in_painter_order_in(mode);
    }
}

fn sixty_four_quads_over_sixteen_textures_read_back_exactly_in_painter_order_in(mode: Mode) {
    let mut rig = Rig::new(mode, 32, 8, 128);
    for key in 0..16_u32 {
        rig.show(
            key,
            8,
            8,
            pattern(8, 8, u8::try_from(key * 13 + 1).expect("a byte")),
        );
    }
    let instances = sixty_four();
    let clear = [0.0, 0.25, 0.5];
    let (pixels, counters) = rig.draw((320, 256), clear, VulkanFilter::Nearest, instances.clone());
    let checker = checker_image();
    let expected = reference(
        (320, 256),
        clear,
        VulkanFilter::Nearest,
        &rig.shown,
        &checker,
        &instances,
    );
    assert_eq!(
        differing(&pixels, &expected, 0),
        0,
        "the frame equals the reference pixel for pixel"
    );
    assert_eq!(counters.instances, 64);
    assert_eq!(counters.skipped, 0);
    assert_eq!(counters.checker, 0);
    assert_eq!(
        counters.batches,
        runs(&instances),
        "one draw per adjacent run, nothing sorted by texture"
    );
    assert!(
        counters.batches > 16 && counters.batches < 64,
        "the list has adjacent repeats and non-adjacent ones: {} batches",
        counters.batches
    );
    // The negative control: sorting by texture (stable) changes the picture where quads overlap.
    let mut sorted = instances.clone();
    sorted.sort_by_key(|instance| instance.key);
    let reordered = reference(
        (320, 256),
        clear,
        VulkanFilter::Nearest,
        &rig.shown,
        &checker,
        &sorted,
    );
    assert!(
        differing(&pixels, &reordered, 0) > 0,
        "a texture-sorted frame differs from the painter-order one at an overlap"
    );
    rig.finish();
}

/// Test 1, interleaved samplers: in a linear frame the painter order is resident A, the checker, resident B, and B is a
/// 2 x 2 image magnified to 64 x 64, so nearest and linear differ. B must match the linear reference and the checker's
/// cells must stay sharp.
///
/// Negative control: a frame that kept nearest for B after the checker batch (the switch back omitted) is the nearest
/// reference, which differs from the linear one by far more than the tolerance.
#[test]
fn a_linear_frame_switches_back_to_linear_after_the_checker_batch() {
    for mode in MODES {
        a_linear_frame_switches_back_to_linear_after_the_checker_batch_in(mode);
    }
}

fn a_linear_frame_switches_back_to_linear_after_the_checker_batch_in(mode: Mode) {
    let mut rig = Rig::new(mode, 8, 2, 16);
    rig.show(1, 8, 8, pattern(8, 8, 40));
    // B: four distinct opaque texels.
    let b = Arc::new(Bytes(vec![
        255, 0, 0, 255, 0, 255, 0, 255, //
        0, 0, 255, 255, 255, 255, 0, 255,
    ]));
    rig.show(2, 2, 2, b);
    let instances = vec![
        instance(1, [0, 0, 8, 8], WHOLE),
        instance(99, [16, 0, 80, 64], WHOLE),
        instance(2, [88, 0, 152, 64], WHOLE),
    ];
    let (pixels, counters) = rig.draw((160, 64), [0.0; 3], VulkanFilter::Linear, instances.clone());
    assert_eq!(
        counters.batches, 3,
        "A, the checker and B are three batches"
    );
    assert_eq!(counters.checker, 1);
    let checker = checker_image();
    let linear = reference(
        (160, 64),
        [0.0; 3],
        VulkanFilter::Linear,
        &rig.shown,
        &checker,
        &instances,
    );
    assert!(
        differing(&pixels, &linear, 3) == 0,
        "B and the sharp checker match the linear reference ({} pixels differ)",
        differing(&pixels, &linear, 3)
    );
    // The checker's cells are sharp: an exact copy of the image at 1:1.
    for y in 0..64_usize {
        for x in 0..64_usize {
            let got = &pixels[(y * 160 + 16 + x) * 4..][..4];
            let want = &checker_pixels()[(y * 64 + x) * 4..][..4];
            assert_eq!(got, want, "checker texel ({x}, {y})");
        }
    }
    // The negative control.
    let nearest_b = reference(
        (160, 64),
        [0.0; 3],
        VulkanFilter::Nearest,
        &rig.shown,
        &checker,
        &instances,
    );
    assert!(
        differing(&pixels, &nearest_b, 8) > 1000,
        "a frame with B sampled nearest differs from the linear one in more than a thousand pixels"
    );
    rig.finish();
}

/// The identity an image encodes: its key in the three colour bytes, opaque.
pub(crate) fn identity_image(key: u32) -> Arc<Bytes> {
    let [r, g, b, _] = key.to_le_bytes();
    let mut bytes = Vec::with_capacity(32 * 32 * 4);
    for _ in 0..32 * 32 {
        bytes.extend_from_slice(&[r, g, b, 255]);
    }
    Arc::new(Bytes(bytes))
}

pub(crate) fn decode(pixel: &[u8]) -> u32 {
    u32::from_le_bytes([pixel[0], pixel[1], pixel[2], 0])
}

/// Test 2: the 32,768 baseline at 1920 x 1080: 32,768 distinct 32 x 32 images, each encoding its identity, drawn as
/// 65,536 quads (every image twice, the repeats far apart so none is adjacent) with the ordinary-set bound at
/// `max_textures` 32,768 + `max_extra_revisions` 1,024 + 1. Every quad decodes its own identity, with no checker and no
/// skipped instance. The times are reported, not gated.
///
/// Negative control: a quad whose set was written for another revision decodes that revision's identity, so comparing
/// against the expected one fails.
#[test]
fn the_baseline_of_32768_textures_decodes_every_identity() {
    for mode in MODES {
        the_baseline_of_32768_textures_decodes_every_identity_in(mode);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "upload, adopt, build, draw, decode and the control read best as one baseline"
)]
fn the_baseline_of_32768_textures_decodes_every_identity_in(mode: Mode) {
    const TEXTURES: u32 = 32_768;
    let mut rig = Rig::with_limits(
        mode,
        TEXTURES as usize,
        65_536,
        TextureLimits {
            max_prepared_images: 1_024,
            max_upload_images_per_batch: 512,
            ..TextureLimits::default()
        },
        LedgerLimits {
            max_extra_revisions: 1_024,
            max_adoptions_per_frame: 1_024,
        },
    );
    let started = Instant::now();
    let mut next = 0_u32;
    let mut committed = 0_u32;
    let mut prepared: Vec<u32> = Vec::new();
    while committed < TEXTURES {
        // Offer as much as the window has room for.
        while next < TEXTURES && rig.ledger().room() > 0 {
            rig.ledger()
                .offer(
                    next,
                    1,
                    32,
                    32,
                    identity_image(next) as Arc<dyn PixelSource>,
                )
                .expect("the offer is admitted");
            next += 1;
        }
        let mut events = Vec::new();
        let ledger = rig.ledger.as_ref().expect("the ledger exists");
        ledger.pump(&mut rig.kit.queue, &mut events);
        for event in events {
            match event {
                LedgerEvent::Prepared { key, .. } => prepared.push(key),
                other => panic!("unexpected {other:?}"),
            }
        }
        for key in prepared.drain(..) {
            rig.ledger().commit(key, 1).expect("commit");
            committed += 1;
        }
        let mut adopted = Vec::new();
        rig.ledger().adopt(1_024, &mut adopted);
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "the uploads stalled"
        );
        std::thread::sleep(Duration::from_micros(100));
    }
    rig.adopt_all();
    let uploaded = started.elapsed();
    // 65,536 quads of 7 x 4 pixels in a 274-column grid.
    let mut instances = Vec::with_capacity(65_536);
    for index in 0..65_536_u32 {
        let (column, row) = (
            i32::try_from(index % 274).expect("small"),
            i32::try_from(index / 274).expect("small"),
        );
        instances.push(instance(
            index % TEXTURES,
            [column * 7, row * 4, column * 7 + 7, row * 4 + 4],
            WHOLE,
        ));
    }
    let frame = VulkanFrame::new(
        1,
        VulkanFilter::Nearest,
        None,
        Some(Arc::new(Quads(instances.clone()))),
    );
    let drawn = Instant::now();
    let ledger = rig.ledger.as_ref().expect("the ledger exists");
    let (pixels, counters) = readback_scene(
        &rig.kit.context,
        &mut rig.kit.queue,
        ledger,
        (1920, 1080),
        [0.0; 3],
        &frame,
        65_536,
    )
    .expect("the scene reads back");
    let drawing = drawn.elapsed();
    println!(
        "baseline: {TEXTURES} textures uploaded and adopted in {uploaded:?}; 65,536 quads built, drawn and read back in {drawing:?}; {counters:?}"
    );
    assert_eq!(counters.instances, 65_536);
    assert_eq!(counters.skipped, 0, "no instance was skipped");
    assert_eq!(counters.checker, 0, "no quad drew the checker");
    assert_eq!(counters.resolved, 65_536);
    assert_eq!(
        counters.batches, 65_536,
        "no repeat is adjacent, so every instance is its own run"
    );
    for (index, quad) in instances.iter().enumerate() {
        let (x, y) = (
            usize::try_from(quad.destination.left + 3).expect("inside"),
            usize::try_from(quad.destination.top + 2).expect("inside"),
        );
        let got = decode(&pixels[(y * 1920 + x) * 4..][..4]);
        assert_eq!(
            got, quad.key,
            "quad {index} decodes the identity of its own texture"
        );
    }
    // The negative control: the check has teeth. A quad drawn with another texture's set decodes that identity.
    let wrong = decode(&pixels[((2 * 1920) + 3) * 4..][..4]);
    assert_ne!(wrong, 1, "quad 0 does not decode quad 1's identity");
    rig.finish();
}

/// Test 3: stable identity and no movement re-upload. Moving every quad between frames changes the pixels but not the
/// textures created or the upload batches submitted (the store's cumulative figures). A reload shows its new revision only
/// after adoption at a boundary.
///
/// Negative control: the second frame's pixels differ from the first's (the movement is real), so an unchanged count is
/// not an unchanged picture.
#[test]
fn movement_re_uploads_nothing_and_a_reload_shows_only_after_adoption() {
    for mode in MODES {
        movement_re_uploads_nothing_and_a_reload_shows_only_after_adoption_in(mode);
    }
}

fn movement_re_uploads_nothing_and_a_reload_shows_only_after_adoption_in(mode: Mode) {
    let mut rig = Rig::new(mode, 16, 4, 64);
    let red = Arc::new(Bytes([255, 0, 0, 255].repeat(16)));
    let green = Arc::new(Bytes([0, 255, 0, 255].repeat(16)));
    rig.show(1, 4, 4, Arc::clone(&red));
    rig.show(2, 4, 4, Arc::clone(&green));
    let first = vec![
        instance(1, [0, 0, 16, 16], WHOLE),
        instance(2, [16, 0, 32, 16], WHOLE),
    ];
    let (before, _) = rig.draw((64, 32), [0.0; 3], VulkanFilter::Nearest, first.clone());
    let stats = rig.ledger().upload_stats();
    let moved = vec![
        instance(1, [32, 16, 48, 32], WHOLE),
        instance(2, [48, 16, 64, 32], WHOLE),
    ];
    let (after, _) = rig.draw((64, 32), [0.0; 3], VulkanFilter::Nearest, moved);
    assert_ne!(before, after, "the movement changed the pixels");
    let later = rig.ledger().upload_stats();
    assert_eq!(
        (later.textures_created, later.batches_submitted),
        (stats.textures_created, stats.batches_submitted),
        "moving every quad created no texture and submitted no upload"
    );
    // A reload: revision 2 of key 1 is blue. Committed, it waits for the boundary.
    let blue = Arc::new(Bytes([0, 0, 255, 255].repeat(16)));
    rig.prepare(1, 2, 4, 4, &blue);
    let (still, _) = rig.draw((64, 32), [0.0; 3], VulkanFilter::Nearest, first.clone());
    assert_eq!(
        still, before,
        "a committed revision is not shown before the boundary"
    );
    rig.adopt_all();
    let (reloaded, _) = rig.draw((64, 32), [0.0; 3], VulkanFilter::Nearest, first);
    assert_ne!(
        reloaded, before,
        "after adoption the frame shows the new revision"
    );
    assert_eq!(&reloaded[..4], &[0, 0, 255, 255], "key 1 is blue now");
    assert_eq!(
        &reloaded[(16 * 4)..(16 * 4 + 4)],
        &[0, 255, 0, 255],
        "key 2 is untouched"
    );
    rig.finish();
}

/// The checker is a baseline item of the store with its own permanent descriptor set: it takes no ledger charge and no
/// prepared-window slot, is resident after a pump sees its upload complete, and starting it twice is refused. The end
/// releases it with the ledger.
///
/// Negative control: before the first pump it is pending and has no set, so a frame could not be attempted.
#[test]
fn the_checker_is_a_baseline_item_with_its_own_permanent_set() {
    let kit = Kit::new(false);
    let ledger = TextureLedger::<u32>::start_with_test_hooks(
        kit.context.clone(),
        TextureLimits::default(),
        LedgerLimits::default(),
        8,
        TextureWakes::default(),
        TextureTestHooks::default(),
    )
    .expect("the ledger starts");
    assert_eq!(ledger.checker_progress(), CheckerProgress::Pending);
    assert_eq!(
        ledger.checker_set(),
        None,
        "no set before the checker is resident"
    );
    let room = ledger.room();
    ledger
        .start_checker(64, 64, Arc::new(CheckerBytes(checker_pixels())))
        .expect("the checker is offered");
    assert!(
        ledger
            .start_checker(64, 64, Arc::new(CheckerBytes(checker_pixels())))
            .is_err(),
        "a second checker is refused"
    );
    assert_eq!(
        ledger.room(),
        room,
        "it took no slot of the prepared window"
    );
    let mut kit = kit;
    let mut events = Vec::new();
    let started = Instant::now();
    while ledger.checker_progress() == CheckerProgress::Pending {
        ledger.pump(&mut kit.queue, &mut events);
        assert!(
            started.elapsed() < PATIENCE,
            "the checker never became resident"
        );
        std::thread::sleep(PICK);
    }
    assert_eq!(ledger.checker_progress(), CheckerProgress::Resident);
    assert!(ledger.checker_set().is_some());
    assert!(
        ledger.checker_bytes() >= 64 * 64 * 4,
        "{}",
        ledger.checker_bytes()
    );
    assert!(
        events.is_empty(),
        "the checker is no key's: no event: {events:?}"
    );
    assert_eq!(ledger.room(), room);
    assert_eq!(
        ledger.extra_room(),
        LedgerLimits::default().max_extra_revisions
    );
    assert_eq!(ledger.test_live_revisions(), 0, "and no revision");
    assert_eq!(
        ledger.shutdown(Instant::now() + PATIENCE),
        LedgerCleanup::Proven
    );
    drop(ledger);
    kit.finish();
}

/// A checker whose upload fails is reported as such, once, and the ledger still ends proven.
#[test]
fn a_failing_checker_upload_is_reported_and_the_end_is_proven() {
    let mut kit = Kit::new(false);
    let ledger = TextureLedger::<u32>::start_with_test_hooks(
        kit.context.clone(),
        TextureLimits::default(),
        LedgerLimits::default(),
        8,
        TextureWakes::default(),
        TextureTestHooks {
            fail_baseline: true,
            ..TextureTestHooks::default()
        },
    )
    .expect("the ledger starts");
    ledger
        .start_checker(64, 64, Arc::new(CheckerBytes(checker_pixels())))
        .expect("the checker is offered");
    let mut events = Vec::new();
    let started = Instant::now();
    while ledger.checker_progress() == CheckerProgress::Pending {
        ledger.pump(&mut kit.queue, &mut events);
        assert!(started.elapsed() < PATIENCE, "the failure never arrived");
        std::thread::sleep(PICK);
    }
    match ledger.checker_progress() {
        CheckerProgress::Failed(reason) => assert!(reason.contains("checker"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ledger.checker_set(), None);
    assert_eq!(
        ledger.shutdown(Instant::now() + PATIENCE),
        LedgerCleanup::Proven
    );
    drop(ledger);
    kit.finish();
}

pub(crate) fn compat_instance(key: u32, destination: [i32; 4]) -> VulkanInstance<u32> {
    VulkanInstance {
        role: InstanceRole::Compat,
        ..instance(key, destination, WHOLE)
    }
}

pub(crate) fn compat_frame(key: u32, instances: Vec<VulkanInstance<u32>>) -> VulkanFrame<u32> {
    VulkanFrame::new(
        1,
        VulkanFilter::Nearest,
        None,
        Some(Arc::new(Quads(instances))),
    )
    .with_compat_key(Some(key))
}

fn pixel_at(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let at = (y * width + x) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// Test 8 (off screen): the compatibility quad is drawn only with a resolved texture, never the checker. With nothing to
/// draw and nothing held the frame is held back by the initial gate; with its image shown it is drawn; with the image
/// gone but held (the ledger's hold and the rectangle it was drawn at) the held image is drawn in its place; once the hold
/// is released the held image is no longer used (CK-150) and the frame is gated again.
///
/// Negative control: the same frame without the held image is gated (so the held draw is the difference), and a scene key
/// with nothing shown still draws the checker where the compatibility key may not.
#[test]
fn the_compatibility_quad_is_drawn_with_its_image_or_the_held_one_and_never_the_checker() {
    for mode in MODES {
        the_compatibility_quad_is_drawn_with_its_image_or_the_held_one_and_never_the_checker_in(
            mode,
        );
    }
}

fn the_compatibility_quad_is_drawn_with_its_image_or_the_held_one_and_never_the_checker_in(
    mode: Mode,
) {
    let mut rig = Rig::new(mode, 8, 2, 16);
    let green = Arc::new(Bytes([0, 255, 0, 255].repeat(16)));
    let red = Arc::new(Bytes([255, 0, 0, 255].repeat(16)));
    rig.show(1, 4, 4, red);
    let frame = compat_frame(
        5,
        vec![
            compat_instance(5, [0, 0, 16, 16]),
            instance(1, [16, 0, 32, 16], WHOLE),
        ],
    );
    let refused = rig
        .try_draw((32, 16), &frame, None)
        .expect_err("nothing to draw for the compatibility quad and nothing held");
    assert!(
        refused.message().contains("initial gate"),
        "{}",
        refused.message()
    );
    // The scene key with nothing shown draws the checker (the compatibility key may not).
    let scene_checker = VulkanFrame::new(
        1,
        VulkanFilter::Nearest,
        None,
        Some(Arc::new(Quads(vec![instance(9, [0, 0, 16, 16], WHOLE)]))),
    );
    let (checker_pixels_drawn, counters) =
        rig.try_draw((16, 16), &scene_checker, None).expect("drawn");
    assert_eq!(counters.checker, 1);
    assert_eq!(
        &checker_pixels_drawn[..4],
        &[255, 0, 255, 255],
        "magenta, the checker's first cell"
    );
    // The image is shown: drawn.
    rig.show(5, 4, 4, green);
    let (pixels, counters) = rig
        .try_draw((32, 16), &frame, None)
        .expect("drawn with its image");
    assert_eq!(
        pixel_at(&pixels, 32, 4, 4),
        [0, 255, 0, 255],
        "the compatibility image"
    );
    assert_eq!(
        pixel_at(&pixels, 32, 20, 4),
        [255, 0, 0, 255],
        "and the scene quad beside it"
    );
    assert_eq!((counters.resolved, counters.checker), (2, 0));
    // The presenter keeps this image: the ledger holds the revision. The key then retires, so it resolves to nothing.
    rig.ledger()
        .hold_compat(5, 1)
        .expect("the revision is held");
    assert_eq!(rig.ledger().compat_hold(), Some((5, 1)));
    rig.ledger().retire_key(5);
    let held = HeldCompat {
        key: 5,
        revision: 1,
        destination: DestinationRect {
            left: 0,
            top: 0,
            right: 16,
            bottom: 16,
        },
        source: WHOLE,
    };
    let (kept, _) = rig
        .try_draw((32, 16), &frame, Some(held))
        .expect("the held image is drawn while the current one is pending");
    assert_eq!(
        pixel_at(&kept, 32, 4, 4),
        [0, 255, 0, 255],
        "the held image, at the rectangle it was drawn at"
    );
    assert!(
        rig.try_draw((32, 16), &frame, None).is_err(),
        "negative control: without the held image the same frame is gated"
    );
    // CK-150: once the hold is released the held image is not used.
    rig.ledger().release_compat_hold();
    assert_eq!(rig.ledger().compat_hold(), None);
    assert!(
        rig.try_draw((32, 16), &frame, Some(held)).is_err(),
        "a released hold is not resolvable, so the frame is gated rather than drawn with a stale image"
    );
    rig.finish();
}

/// Test 8, the CK-150 hold rule at the ledger: a held-revision lookup on an open ticket is checked against the **current**
/// hold on every call. Releasing the hold, or holding another revision, makes the old lookup return `None`; it never
/// returns a wrong set.
#[test]
fn a_held_revision_lookup_follows_the_current_hold_and_never_returns_a_wrong_set() {
    for mode in MODES {
        a_held_revision_lookup_follows_the_current_hold_and_never_returns_a_wrong_set_in(mode);
    }
}

fn a_held_revision_lookup_follows_the_current_hold_and_never_returns_a_wrong_set_in(mode: Mode) {
    let mut rig = Rig::new(mode, 8, 4, 16);
    rig.show(5, 4, 4, Arc::new(Bytes([0, 255, 0, 255].repeat(16))));
    rig.ledger().hold_compat(5, 1).expect("held");
    // A second revision of the same key is adopted: revision 1 is retired and stays alive by the hold.
    rig.prepare(5, 2, 4, 4, &Arc::new(Bytes([0, 0, 255, 255].repeat(16))));
    rig.adopt_all();
    let ledger = rig.ledger();
    let ticket = ledger.begin_frame().expect("a ticket");
    let first = ticket
        .resolve_revision(5, 1)
        .expect("the held revision resolves although it was superseded");
    let first_binding = (first.slot(), first.descriptor_set());
    assert!(
        ticket.resolve_revision(5, 2).is_none(),
        "only the exact held revision resolves"
    );
    ledger
        .hold_compat(5, 2)
        .expect("holding another revision releases the first hold");
    assert!(
        ticket.resolve_revision(5, 1).is_none(),
        "the lookup follows the current hold: revision 1 is no longer held"
    );
    let second = ticket
        .resolve_revision(5, 2)
        .expect("the new hold resolves");
    assert_ne!(
        (second.slot(), second.descriptor_set()),
        first_binding,
        "and it is revision 2's own slot or set, not revision 1's"
    );
    ledger.release_compat_hold();
    assert!(
        ticket.resolve_revision(5, 2).is_none(),
        "released: nothing resolves"
    );
    ticket.discarded().expect("ends");
    rig.finish();
}

/// Test 8: the failure of an upload of the marked compatibility key is also the presenter's, once, before the ordinary
/// `UploadFailed` of the same failure; a failure of a key that is not marked produces only the ordinary event.
///
/// Negative control: the unmarked run.
#[test]
fn a_compatibility_upload_failure_is_reported_before_the_ordinary_one_and_only_for_the_marked_key()
{
    for marked in [true, false] {
        // The second image-memory allocation fails (the checker's block is the first): an image of 1100 x 1100 has an allocation
        // of its own (it is over the dedicated-allocation threshold).
        let mut rig = Rig::with_hooks(
            Mode::PerRevision,
            8,
            16,
            TextureLimits::default(),
            LedgerLimits::default(),
            TextureTestHooks {
                fail_allocation_at: Some(2),
                ..TextureTestHooks::default()
            },
        );
        rig.ledger().set_compat_key(marked.then_some(5));
        rig.ledger()
            .offer(
                5,
                1,
                1100,
                1100,
                Arc::new(Bytes([1, 2, 3, 255].repeat(1100 * 1100))) as Arc<dyn PixelSource>,
            )
            .expect("offered");
        let events = rig.pump_until(|_, events| {
            events
                .iter()
                .any(|event| matches!(event, LedgerEvent::UploadFailed { .. }))
        });
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                LedgerEvent::CompatUploadFailed { .. } => "compat",
                LedgerEvent::UploadFailed { .. } => "upload",
                _ => "other",
            })
            .collect();
        // (An injected allocation failure also latches the store, which adds its own `GpuFailure` after them.)
        if marked {
            assert_eq!(kinds[..2], ["compat", "upload"], "{events:?}");
        } else {
            assert_eq!(kinds[..1], ["upload"], "{events:?}");
            assert!(!kinds.contains(&"compat"), "{events:?}");
        }
        rig.finish();
    }
}
