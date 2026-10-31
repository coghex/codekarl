//! A fake backend: it records each call, holds the sink's latest value for a "render thread" that can be
//! stalled, and either draws the quad it is given or only clears.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ck_window::{CleanupResult, PixelSize, PresenterReport, WindowDesc};

use super::support::{Gate, LatestSink, lock};
use crate::{Gpu2d, Phase, PixelRect, QuadSink, SceneBridge};

/// What the fake saw, in order.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Event {
    NewSink,
    /// `start` was entered: what the sink already held, before the fake's thread was spawned.
    Start {
        held: Option<u64>,
        quad: bool,
        size: PixelSize,
    },
    SetClear([f32; 3]),
    /// What the sink held when the call was forwarded.
    SetPixelSize {
        width: u32,
        height: u32,
        held: Option<u64>,
    },
    RequestRedraw,
    /// The bridge's phase when the stop request reached the backend.
    RequestStop {
        phase: Phase,
    },
    PollEnd(Duration),
}

/// A frame the fake's render thread drew.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Frame {
    pub(super) generation: u64,
    /// Whether this backend drew the quad of the snapshot (a clear-only backend never does).
    pub(super) drew_quad: bool,
    pub(super) destination: Option<PixelRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    DrawsQuad,
    ClearOnly,
}

/// What a test reads after the fake has moved into a renderer.
pub(super) struct FakeState {
    pub(super) events: Mutex<Vec<Event>>,
    pub(super) frames: Mutex<Vec<Frame>>,
    pub(super) reports: Mutex<VecDeque<PresenterReport>>,
    pub(super) sink: Mutex<Option<Arc<LatestSink>>>,
    pub(super) end: Mutex<Option<CleanupResult>>,
    /// The render thread draws its first frame once this is open.
    pub(super) go: Gate,
    /// The render thread ends once this is open.
    pub(super) stop: Gate,
    /// The render thread holds the snapshot it drew "uploading" until this is open.
    pub(super) upload: Gate,
}

impl FakeState {
    pub(super) fn events(&self) -> Vec<Event> {
        lock(&self.events).clone()
    }

    pub(super) fn frames(&self) -> Vec<Frame> {
        lock(&self.frames).clone()
    }

    /// Every snapshot the sink was given, as (generation, whether it held a quad).
    pub(super) fn sink_history(&self) -> Vec<(u64, bool)> {
        self.sink().history()
    }

    pub(super) fn sink(&self) -> Arc<LatestSink> {
        lock(&self.sink).clone().expect("the fake made a sink")
    }
}

pub(super) struct FakeGpu {
    pub(super) state: Arc<FakeState>,
    mode: Mode,
    bridge: Arc<SceneBridge>,
    fail_start: Option<String>,
    thread: Option<JoinHandle<()>>,
}

impl FakeGpu {
    pub(super) fn new(mode: Mode, bridge: &Arc<SceneBridge>) -> Self {
        Self::build(mode, bridge, true)
    }

    /// A fake whose render thread keeps the snapshot it drew until `state.upload` is opened: a stalled consumer.
    pub(super) fn stalled(mode: Mode, bridge: &Arc<SceneBridge>) -> Self {
        Self::build(mode, bridge, false)
    }

    fn build(mode: Mode, bridge: &Arc<SceneBridge>, uploads: bool) -> Self {
        let state = Arc::new(FakeState {
            events: Mutex::new(Vec::new()),
            frames: Mutex::new(Vec::new()),
            reports: Mutex::new(VecDeque::new()),
            sink: Mutex::new(None),
            end: Mutex::new(None),
            go: Gate::default(),
            stop: Gate::default(),
            upload: Gate::default(),
        });
        if uploads {
            state.upload.open();
        }
        Self {
            state,
            mode,
            bridge: Arc::clone(bridge),
            fail_start: None,
            thread: None,
        }
    }

    /// A fake whose `start` fails with `message`.
    pub(super) fn failing(message: &str, bridge: &Arc<SceneBridge>) -> Self {
        let mut gpu = Self::new(Mode::DrawsQuad, bridge);
        gpu.fail_start = Some(message.to_owned());
        gpu
    }

    fn record(&self, event: Event) {
        lock(&self.state.events).push(event);
    }
}

impl Gpu2d for FakeGpu {
    type Handles = ();

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.record(Event::NewSink);
        let sink = LatestSink::new();
        *lock(&self.state.sink) = Some(Arc::clone(&sink));
        sink
    }

    fn start(
        &mut self,
        (): (),
        _desc: &WindowDesc,
        pixel_size: PixelSize,
        _sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        let sink = self.state.sink();
        let held = sink.latest();
        self.record(Event::Start {
            held: held.as_ref().map(|snapshot| snapshot.generation),
            quad: held
                .as_ref()
                .is_some_and(|snapshot| snapshot.quad.is_some()),
            size: pixel_size,
        });
        if let Some(message) = &self.fail_start {
            return Err(message.clone());
        }
        let (state, mode) = (Arc::clone(&self.state), self.mode);
        self.thread = Some(thread::spawn(move || {
            state.go.wait();
            // The frame takes the latest value, and holds it while it uploads.
            let uploading = sink.take();
            if let Some(snapshot) = &uploading {
                lock(&state.frames).push(Frame {
                    generation: snapshot.generation,
                    drew_quad: mode == Mode::DrawsQuad && snapshot.quad.is_some(),
                    destination: snapshot.quad.as_ref().map(|quad| quad.destination),
                });
            }
            lock(&state.reports).push_back(PresenterReport::FirstFrameQueued);
            state.upload.wait();
            drop(uploading);
            state.stop.wait();
            *lock(&state.end) = Some(CleanupResult::Proven);
        }));
        Ok(())
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.record(Event::SetClear(rgb));
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        let held = self
            .state
            .sink()
            .latest()
            .map(|snapshot| snapshot.generation);
        self.record(Event::SetPixelSize {
            width,
            height,
            held,
        });
    }

    fn request_redraw(&mut self) {
        self.record(Event::RequestRedraw);
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        lock(&self.state.reports).pop_front()
    }

    fn request_stop(&mut self) {
        self.record(Event::RequestStop {
            phase: self.bridge.phase(),
        });
        self.state.stop.open();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.record(Event::PollEnd(within));
        if let Some(thread) = self.thread.take_if(|thread| thread.is_finished()) {
            thread.join().unwrap();
        }
        lock(&self.state.end).clone()
    }
}

impl Drop for FakeGpu {
    fn drop(&mut self) {
        // Never leave the fake's thread waiting.
        self.state.go.open();
        self.state.upload.open();
        self.state.stop.open();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
