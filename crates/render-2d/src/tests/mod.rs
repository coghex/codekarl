//! Unit tests: the pieces that need a test-only constructor for images (the drop observer, from `ck-resources`'s
//! `test-support` feature) or the bridge's internals. The decoder, its budgets and the assets rule are tested through
//! the public API in `ck-resources`'s `tests/decoder/`.

mod bridge;
mod clear_only;
mod fake;
mod idle;
mod legacy;
mod logging;
#[cfg(all(feature = "metal", target_os = "macos"))]
mod metal_refusal;
#[cfg(all(feature = "metal", target_os = "macos"))]
mod metal_scene;
#[cfg(all(feature = "metal", target_os = "macos"))]
mod metal_sink;
mod points;
mod renderer;
mod resident;
mod resources;
mod scene;
mod support;
#[cfg(feature = "vulkan")]
mod vulkan_refusal;
#[cfg(feature = "vulkan")]
mod vulkan_scene;
#[cfg(feature = "vulkan")]
mod vulkan_sink;
