//! The common window, owner against owner (milestone 5, issue 10b-1): the presenter's windows and the window loop's windows get the
//! same definition and adopt it at different times, and each admits a sample only by its own timestamp, so both cover exactly
//! `[start, end)`. Deterministic: the adoptions, the samples and the end are placed by the test, not by timing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_render_2d::{MetricsWindow as RenderWindow, Windows};
use ck_window::{LoopMetrics, MetricsWindow as LoopWindow, UptimeNs};

/// The instrumentation clock at the loop's first call, nanoseconds.
const T0: u64 = 5_000_000_000;
const MS: u64 = 1_000_000;

/// Both owners over one timeline measured in milliseconds after `T0`.
struct Owners {
    render: Windows<u64>,
    looped: LoopMetrics,
    base: Instant,
}

impl Owners {
    fn new() -> Self {
        let clock: UptimeNs = Arc::new(|| T0);
        let owners = Self {
            render: Windows::new(),
            looped: LoopMetrics::new(clock),
            base: Instant::now(),
        };
        // The loop's first call fixes the pairing of its instants with the clock: placed at the timeline's zero.
        owners.looped.boundary(owners.base);
        owners
    }

    fn at(&self, ms: u64) -> Instant {
        self.base + Duration::from_millis(ms)
    }

    /// The window both owners are asked for: from `start` ms, with `end` ms when given (the same definition to both).
    fn request(&mut self, start: u64, end: Option<u64>) {
        let render = RenderWindow {
            id: 1,
            start: T0 + start * MS,
            end: end.map(|end| T0 + end * MS),
        };
        self.render
            .request(render)
            .expect("the render owner accepts it");
        self.looped
            .request(LoopWindow {
                id: 1,
                start: render.start,
                end: render.end,
            })
            .expect("the loop owner accepts it");
    }

    fn render_adopts(&mut self, ms: u64) {
        self.render.boundary(T0 + ms * MS);
    }

    fn loop_adopts(&self, ms: u64) {
        self.looped.boundary(self.at(ms));
    }

    /// A frame committed at `ms` (the render owner's sample) and an update posted at `ms` (the loop's).
    fn sample(&mut self, ms: u64) {
        if let Some(tracked) = self.render.admit(T0 + ms * MS) {
            tracked.stats += 1;
        }
        self.looped.delivered(self.at(ms));
    }

    fn render_frames(&self) -> u64 {
        self.render.get(1).expect("kept").stats
    }

    fn loop_updates(&self) -> u64 {
        self.looped
            .snapshot(1)
            .expect("kept")
            .window
            .stats
            .delivered
    }
}

/// Samples every 10 ms from 0 to 600 ms, with the window `[250, 450)`: 20 of them lie inside.
fn timeline(owners: &mut Owners, adoptions: &[(u64, &str)]) {
    let mut pending = adoptions.to_vec();
    pending.sort_by_key(|(at, _)| *at);
    let mut next = 0;
    for ms in (0..=600).step_by(10) {
        while next < pending.len() && pending[next].0 <= ms {
            match pending[next].1 {
                "render" => owners.render_adopts(pending[next].0),
                _ => owners.loop_adopts(pending[next].0),
            }
            next += 1;
        }
        owners.sample(ms);
    }
}

#[test]
fn the_loops_adoption_delayed_while_frames_and_updates_flow_leaves_both_on_the_same_interval() {
    let mut owners = Owners::new();
    owners.request(250, Some(450));
    // The render thread adopts at once; the loop only at 240 ms (just before the window opens). Frames and updates were flowing the
    // whole time.
    timeline(&mut owners, &[(10, "render"), (240, "loop")]);
    assert_eq!(owners.render_frames(), 20, "250 .. 440 ms, every 10 ms");
    assert_eq!(
        owners.loop_updates(),
        20,
        "the same interval on the loop's side"
    );
    let render = owners.render.get(1).expect("kept");
    let looped = owners.looped.snapshot(1).expect("kept").window;
    assert_eq!(
        (render.window.start, render.window.end),
        (looped.window.start, looped.window.end)
    );
    assert!(!render.late_adopted && !looped.late_adopted);
    assert_eq!(render.adopted_at, Some(T0 + 10 * MS));
    assert_eq!(looped.adopted_at, Some(T0 + 240 * MS));
    // Outside the window, by their own timestamps: the render owner (adopted at 10 ms) saw 10 .. 240 ms and 450 .. 600 ms; the loop
    // (adopted at 240 ms) saw 240 ms and 450 .. 600 ms. The sample at 0 ms came before the render owner had any window.
    assert_eq!(render.outside, 24 + 16);
    assert_eq!(looped.outside, 1 + 16);
    assert_eq!(
        owners.looped.totals().delivered,
        61,
        "the whole run counts every update"
    );
}

#[test]
fn the_render_adoption_delayed_while_updates_are_delivered_is_the_same_the_other_way_round() {
    let mut owners = Owners::new();
    owners.request(250, Some(450));
    timeline(&mut owners, &[(10, "loop"), (240, "render")]);
    assert_eq!(owners.render_frames(), 20);
    assert_eq!(owners.loop_updates(), 20);
    let render = owners.render.get(1).expect("kept");
    assert_eq!(render.adopted_at, Some(T0 + 240 * MS));
    assert!(!render.late_adopted);
}

#[test]
fn samples_between_the_two_adoptions_but_before_start_are_in_neither_owner() {
    let mut owners = Owners::new();
    owners.request(250, Some(450));
    // The loop adopts at 100 ms, the render thread at 200 ms: the samples at 110 .. 190 ms are between the adoptions, before start.
    timeline(&mut owners, &[(100, "loop"), (200, "render")]);
    assert_eq!(owners.render_frames(), 20);
    assert_eq!(owners.loop_updates(), 20);
    // r2's rule, admission at the owner's adoption, would have put them in the earlier owner's window: the first admitted sample of
    // both is at the start itself.
    let render = owners.render.get(1).expect("kept");
    assert!(render.window.contains(T0 + 250 * MS) && !render.window.contains(T0 + 240 * MS));
}

#[test]
fn an_adoption_past_start_is_late_adopted_on_that_owner_only_and_the_window_stays_common() {
    let mut owners = Owners::new();
    owners.request(250, Some(450));
    timeline(&mut owners, &[(10, "render"), (300, "loop")]);
    let render = owners.render.get(1).expect("kept");
    let looped = owners.looped.snapshot(1).expect("kept").window;
    assert!(!render.late_adopted);
    assert!(looped.late_adopted, "adopted 50 ms after the window opened");
    // The loop missed the samples between start and its adoption (they had no window to enter): fewer than the render owner.
    assert_eq!(owners.render_frames(), 20);
    assert!(owners.loop_updates() < 20, "{}", owners.loop_updates());
    assert_eq!(
        (render.window.start, render.window.end),
        (looped.window.start, looped.window.end)
    );
}

#[test]
fn the_joint_end_admits_nothing_at_or_after_its_timestamp_on_either_owner_while_both_are_hammered()
{
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;

    use crate::metrics::{MARK_LEAD_NS, MetricsHub};

    for round in 0..40 {
        // One tick is one nanosecond of a shared clock: a sample's timestamp is the tick it took, so "before the end" is exact.
        let tick = Arc::new(AtomicU64::new(0));
        let clock: UptimeNs = {
            let tick = Arc::clone(&tick);
            Arc::new(move || T0 + tick.fetch_add(1, Ordering::SeqCst))
        };
        let hub = Arc::new(MetricsHub::new(clock, true));
        let crate::metrics::Marked::Started(window) = hub.mark() else {
            panic!("the first mark starts the window");
        };
        let render = Arc::new(Mutex::new(Windows::<u64>::new()));
        {
            let mut windows = render.lock().unwrap();
            windows
                .request(RenderWindow {
                    id: window.id,
                    start: window.start,
                    end: None,
                })
                .expect("accepted");
        }
        // Both owners adopt as the window opens: the clock jumps to the start, and the loop's first call pairs its instant with that
        // reading exactly (its `boundary` reads the clock next).
        let base = Instant::now();
        let adopted_at = MARK_LEAD_NS;
        tick.store(adopted_at, Ordering::SeqCst);
        hub.loop_metrics()
            .boundary(base + Duration::from_nanos(adopted_at));
        render.lock().unwrap().boundary(T0 + adopted_at);
        let stop = Arc::new(AtomicBool::new(false));
        let producers: Vec<_> = (0..2)
            .map(|which| {
                let (tick, render, hub, stop) = (
                    Arc::clone(&tick),
                    Arc::clone(&render),
                    Arc::clone(&hub),
                    Arc::clone(&stop),
                );
                thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        let t = tick.fetch_add(1, Ordering::SeqCst);
                        if which == 0 {
                            if let Some(tracked) = render.lock().unwrap().admit(T0 + t) {
                                tracked.stats += 1;
                            }
                        } else {
                            hub.loop_metrics().delivered(base + Duration::from_nanos(t));
                        }
                    }
                })
            })
            .collect();
        // Let them run, then end under both owners' locks, the way the loop's teardown does.
        loop {
            let on_render = render
                .lock()
                .unwrap()
                .get(window.id)
                .map_or(0, |tracked| tracked.stats);
            let on_loop = hub
                .loop_metrics()
                .snapshot(window.id)
                .map_or(0, |reading| reading.window.stats.delivered);
            if on_render >= 50 + round && on_loop >= 50 {
                break;
            }
            thread::yield_now();
        }
        let mut guard = render.lock().unwrap();
        let ended = hub
            .finish(&mut |id, end| guard.end(id, end).map_err(|error| format!("{error:?}")))
            .expect("the window ends");
        drop(guard);
        stop.store(true, Ordering::SeqCst);
        for producer in producers {
            producer.join().expect("producer ended");
        }
        let end = ended.end.expect("the end");
        let render = render.lock().unwrap();
        let on_render = render.get(window.id).expect("kept");
        let on_loop = hub.loop_metrics().snapshot(window.id).expect("kept").window;
        assert!(
            !on_render.end_overrun && !on_loop.end_overrun,
            "round {round}"
        );
        assert!(
            on_render.last_admitted_ns < end && on_loop.last_admitted_ns < end,
            "round {round}"
        );
        assert_eq!(
            (on_render.window.end, on_loop.window.end),
            (Some(end), Some(end))
        );
        assert!(
            on_render.stats >= 50 && on_loop.stats.delivered > 0,
            "round {round}"
        );
    }
}

/// The three copies of the window book agree (milestone 5.5, issue 6, part B): `ck-window`'s, the Metal presenter's and the Vulkan
/// presenter's keep the same rules, and the same samples, windows, late adoptions and ends give the same readings in all three.
/// (The Vulkan copy is a deliberate copy, named in CK-162's follow-up; this test is what keeps the three from drifting apart.)
#[cfg(feature = "vulkan")]
mod three_way {
    use ck_render_2d::vulkan_timing::{MetricsWindow as VulkanWindow, Windows as VulkanWindows};
    use ck_render_2d::{MetricsWindow as MetalWindow, Windows as MetalWindows};
    use ck_window::{MetricsWindow as LoopWindow, Windows as LoopWindows};

    /// One operation on a book.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        Request {
            id: u32,
            start: u64,
            end: Option<u64>,
        },
        Boundary {
            now: u64,
        },
        Admit {
            ns: u64,
        },
        End {
            id: u32,
            end: u64,
        },
    }

    /// What one book answers to an operation, and its kept windows afterwards, in plain values.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct State {
        answer: String,
        windows: Vec<[String; 2]>,
    }

    macro_rules! drive {
        ($books:expr, $window:ident, $op:expr) => {{
            let books = $books;
            let answer = match $op {
                Op::Request { id, start, end } => format!(
                    "{:?}",
                    books.request($window { id, start, end })
                ),
                Op::Boundary { now } => {
                    books.boundary(now);
                    String::new()
                }
                Op::Admit { ns } => {
                    let admitted = books.admit(ns).map(|tracked| {
                        tracked.stats += 1;
                        tracked.window.id
                    });
                    format!("{admitted:?}")
                }
                Op::End { id, end } => format!("{:?}", books.end(id, end)),
            };
            let windows = (1..=4_u32)
                .filter_map(|id| books.get(id).map(|tracked| (id, tracked)))
                .map(|(id, tracked)| {
                    [
                        format!("{id}"),
                        format!(
                            "{:?} adopted={:?} late={} outside={} dropped_late={} last={} overrun={} stats={}",
                            (tracked.window.start, tracked.window.end),
                            tracked.adopted_at,
                            tracked.late_adopted,
                            tracked.outside,
                            tracked.dropped_late,
                            tracked.last_admitted_ns,
                            tracked.end_overrun,
                            tracked.stats
                        ),
                    ]
                })
                .collect();
            State { answer, windows }
        }};
    }

    /// A deterministic mix of windows, adoptions, samples (inside, on the edges, outside) and ends.
    fn script(seed: u64) -> Vec<Op> {
        let mut state = seed;
        let mut next = move |bound: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % bound
        };
        let mut ops = Vec::new();
        let mut id = 0_u32;
        let mut clock = 1_000_u64;
        for _ in 0..400 {
            clock += next(40);
            match next(10) {
                0 => {
                    id = id % 4 + 1;
                    let start = clock + next(300);
                    let end = (next(2) == 0).then(|| start + 1 + next(500));
                    ops.push(Op::Request { id, start, end });
                }
                1 => ops.push(Op::Boundary { now: clock }),
                2 => ops.push(Op::End {
                    id: u32::try_from(next(5)).unwrap_or(0),
                    end: clock + next(400),
                }),
                _ => {
                    // Samples cluster on the edges of whatever windows exist: the half-open rule is what they test.
                    let edge = match next(4) {
                        0 => clock,
                        1 => clock.saturating_sub(1),
                        2 => clock + 1,
                        _ => clock + next(600),
                    };
                    ops.push(Op::Admit { ns: edge });
                }
            }
        }
        ops
    }

    #[test]
    fn the_same_operations_give_the_same_readings_in_all_three_books() {
        for seed in 1..=25 {
            let mut looped = LoopWindows::<u64>::new();
            let mut metal = MetalWindows::<u64>::new();
            let mut vulkan = VulkanWindows::<u64>::new();
            for (index, op) in script(seed).into_iter().enumerate() {
                let a = drive!(&mut looped, LoopWindow, op);
                let b = drive!(&mut metal, MetalWindow, op);
                let c = drive!(&mut vulkan, VulkanWindow, op);
                assert_eq!(
                    a, b,
                    "seed {seed}, operation {index} ({op:?}): ck-window against Metal"
                );
                assert_eq!(
                    b, c,
                    "seed {seed}, operation {index} ({op:?}): Metal against Vulkan"
                );
            }
        }
    }
}
