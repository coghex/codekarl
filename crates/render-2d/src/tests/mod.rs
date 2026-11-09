//! Unit tests: the pieces that need a test-only constructor for images (the drop observer, from `ck-resources`'s
//! `test-support` feature) or the bridge's internals. The decoder, its budgets and the assets rule are tested through
//! the public API in `ck-resources`'s `tests/decoder/`.

mod bridge;
mod clear_only;
mod fake;
mod groups;
mod idle;
mod legacy;
mod logging;
mod map;
mod map_mode;
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

/// `ck-render-2d`'s public items, under the name the group readback scenes (shared with the backends' integration targets) use.
mod api {
    pub(crate) use crate::*;
}

/// The scenes the Metal and Vulkan readback targets draw, run here against the CPU reference alone.
#[path = "../../tests/group_readback/mod.rs"]
mod group_readback;
