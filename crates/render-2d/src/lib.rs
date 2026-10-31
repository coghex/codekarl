//! `ck-render-2d`: images, the scene bridge and the renderer for the 2D drawing API.
//!
//! This is the only drawing API the rest of the project sees (`docs/render-design.md` §3). A backend implements
//! [`Gpu2d`]; the GPU crates never depend on this one. Without a feature the crate has no GPU dependency and no
//! `unsafe`:
//!
//! - [`ClearOnly`] wraps any `ck_window::Presenter` as a backend that draws no quad (milestone 4's Vulkan path wraps the
//!   application's accepted adapter);
//! - `MetalBackend` (feature `metal`, macOS) is the full backend over `ck-gpu-metal`;
//! - `VulkanSinkSide` (feature `vulkan`) is everything of a backend over `ck-gpu-vulkan` except the presenter: the
//!   legacy sink or resource-aware scene conversion, resident driver, caller-owned wake relay and once-only refusal (the
//!   application owns the presenter).
//!
//! - [`Image`]: premultiplied RGBA8, made only by decoding a PNG under the budgets of [`budget`]. Re-exported from
//!   `ck-resources`, which holds the decoder.
//! - [`Assets`]: the assets rule, which files the render API opens. Re-exported from `ck-resources`.
//! - [`Quad`] and the points-to-pixels snapshot ([`Snapshot`]).
//! - [`SceneBridge`], [`SceneWriter`] and [`QuadSink`]: the latest quad, written from the UI VM's thread and
//!   published to the GPU in pixels under one small lock.
//! - [`Gpu2d`] and [`Renderer`]: a `ck_window::Presenter` over one backend.

#![deny(unsafe_code)]

mod bridge;
pub mod budget;
mod clear_only;
mod filter;
mod legacy;
#[cfg(all(feature = "metal", target_os = "macos"))]
#[expect(
    unsafe_code,
    reason = "the module's unsafe sites: the constructor contracts of `MetalBackend::new` and `with_resources`, and the calls to `MetalPresenter::start_2d`, `start_scene_bindless` and the hooked start, each documented"
)]
mod metal;
#[cfg(all(feature = "metal", target_os = "macos"))]
mod metal_resident;
#[cfg(all(feature = "metal", target_os = "macos"))]
mod metal_scene;
mod quad;
mod renderer;
mod resident;
mod scene;
#[cfg(feature = "vulkan")]
mod vulkan;
#[cfg(feature = "vulkan")]
mod vulkan_resident;
#[cfg(feature = "vulkan")]
mod vulkan_scene;

pub use bridge::{
    AttachRefused, Phase, QuadSink, SceneBridge, SceneWriter, WriteCounts, WriterTaken,
};
pub use ck_resources::{Assets, AssetsError, DecodeError, DecodeLimits, Image, TooLarge};
pub use clear_only::ClearOnly;
pub use filter::{Filter, UnknownFilter};
pub use legacy::{Admission, LegacyError, LegacyLane};
/// The presenter's frame-timing types (milestone 5, issue 10b-1), which the application reads through the backend: the application
/// never names the GPU crate (`tests/source_guard.rs`).
#[cfg(all(feature = "metal", target_os = "macos"))]
pub use metal::timing::{
    EndError, FrameIntervals, FrameStats, Histogram, MetricsEndGuard, MetricsReading,
    MetricsWindow, Percentile, RequestError, SceneStats, Tracked, Windows, now_ns,
};
#[cfg(all(feature = "metal", target_os = "macos"))]
pub use metal::{BackendTestHooks, MetalBackend, ResourceBackendLimits, sink_over_slot};
#[cfg(all(feature = "metal", target_os = "macos"))]
pub use metal_resident::MetalResidentDriver;
pub use quad::{
    Metrics, PixelRect, PlacedQuad, Quad, QuadError, Snapshot, SourceRect, validate_rect,
};
pub use renderer::{Gpu2d, Renderer};
pub use resident::{
    LedgerOps, ResidentAdapter, ResidentBoundary, ResidentEvent, ResidentPassOutcome,
    ResidentRefusal, ResidentStats, WakeRelay,
};
pub use scene::{FrameQuad, PlacedFrameQuad, QuadKey, SceneFrame, SceneLimits, SceneLimitsError};
#[cfg(feature = "vulkan")]
pub use vulkan::VulkanResourceLimits;
#[cfg(feature = "vulkan")]
pub use vulkan::VulkanSinkSide;
#[cfg(feature = "vulkan")]
pub use vulkan_resident::VulkanResidentDriver;

#[cfg(test)]
mod tests;
