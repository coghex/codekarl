//! Fixed, Vulkan-only CK-168 diagnostic cells. This deliberately leaves the shared CK-130 layout untouched.

use crate::layout::{self, Placed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) name: &'static str,
    pub(crate) live_textures: u32,
    pub(crate) sampled_textures: u32,
    pub(crate) quads: u32,
    pub(crate) ordinary: bool,
    pub(crate) bindless_slots: u32,
    pub(crate) max_textures: usize,
}

const fn cell(
    name: &'static str,
    live_textures: u32,
    sampled_textures: u32,
    quads: u32,
    ordinary: bool,
    bindless_slots: u32,
    max_textures: usize,
) -> Cell {
    Cell {
        name,
        live_textures,
        sampled_textures,
        quads,
        ordinary,
        bindless_slots,
        max_textures,
    }
}

const A_CELLS: [Cell; 8] = [
    cell("a-bindless-1", 1, 1, 65_536, false, 131_072, 65_536),
    cell("a-per-revision-1", 1, 1, 65_536, true, 131_072, 65_536),
    cell("a-bindless-256", 256, 256, 65_536, false, 131_072, 65_536),
    cell(
        "a-per-revision-256",
        256,
        256,
        65_536,
        true,
        131_072,
        65_536,
    ),
    cell(
        "a-bindless-4096",
        4_096,
        4_096,
        65_536,
        false,
        131_072,
        65_536,
    ),
    cell(
        "a-per-revision-4096",
        4_096,
        4_096,
        65_536,
        true,
        131_072,
        65_536,
    ),
    cell(
        "a-bindless-32768",
        32_768,
        32_768,
        65_536,
        false,
        131_072,
        65_536,
    ),
    cell(
        "a-per-revision-32768",
        32_768,
        32_768,
        65_536,
        true,
        131_072,
        65_536,
    ),
];
const OTHER_CELLS: [Cell; 5] = [
    cell("b-bindless-1", 32_768, 1, 65_536, false, 131_072, 65_536),
    cell(
        "b-bindless-256",
        32_768,
        256,
        65_536,
        false,
        131_072,
        65_536,
    ),
    cell(
        "c-bindless-32768",
        32_768,
        32_768,
        32_768,
        false,
        131_072,
        65_536,
    ),
    cell(
        "d-bindless-33794",
        32_768,
        32_768,
        65_536,
        false,
        33_794,
        32_769,
    ),
    cell(
        "d-bindless-131072",
        32_768,
        32_768,
        65_536,
        false,
        131_072,
        32_769,
    ),
];

pub(crate) fn cells() -> impl Iterator<Item = Cell> {
    A_CELLS.into_iter().chain(OTHER_CELLS)
}

pub(crate) fn parse(name: &str) -> Option<Cell> {
    cells().find(|cell| cell.name == name)
}

/// Maps each diagnostic quad evenly over the requested sampled set while preserving CK-130's geometry.
pub(crate) fn placement(index: u32, sampled_textures: u32) -> Option<Placed> {
    if sampled_textures == 0 || sampled_textures > layout::SET_A || index >= layout::SCENE_QUADS {
        return None;
    }
    let mut placed = layout::scene_quad(index, true);
    placed.texture = index % sampled_textures;
    Some(placed)
}
