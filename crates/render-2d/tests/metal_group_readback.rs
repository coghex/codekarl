//! Scene groups on Metal, off screen (milestone 8, slice 4): the scenes of `group_readback` (real groups placed by the scene's own
//! placement) drawn through the presenter's readback and compared with the CPU reference pixel for pixel, then checked for what each
//! frame means: the raised group is on top, a hidden group draws nothing, an offset moves a quad by exactly its points times the
//! scale, and a clip cuts at its edge with the texels trimmed, not squashed.
//!
//! No window is opened; the target needs the GPU.

#![cfg(all(target_os = "macos", feature = "metal"))]

use ck_gpu_metal::{
    DestinationRect, MetalFilter, ReadbackImage, ReadbackInstance, ReadbackTexture, readback_scene,
};

mod api {
    pub(crate) use ck_render_2d::*;
}

#[path = "group_readback/mod.rs"]
mod group_readback;

use group_readback::{CLEAR, Instance, Picture, check, scenes};

fn draw(pictures: &[Picture], size: (u32, u32), instances: &[Instance]) -> Vec<u8> {
    let images: Vec<ReadbackImage<'_>> = pictures
        .iter()
        .map(|picture| ReadbackImage {
            width: picture.width,
            height: picture.height,
            pixels: &picture.bytes,
        })
        .collect();
    let instances: Vec<ReadbackInstance> = instances
        .iter()
        .map(|instance| ReadbackInstance {
            texture: ReadbackTexture::Image(instance.image),
            destination: DestinationRect {
                left: instance.destination[0],
                top: instance.destination[1],
                right: instance.destination[2],
                bottom: instance.destination[3],
            },
            source: instance.source,
            tint: instance.tint,
        })
        .collect();
    let mut pixels = readback_scene(
        size.0,
        size.1,
        CLEAR,
        None,
        MetalFilter::Nearest,
        &images,
        &instances,
    )
    .expect("the readback works");
    // The readback is BGRA8 (the drawable's order); the scenes and the reference are RGBA8.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    pixels
}

#[test]
fn groups_read_back_on_metal_exactly_and_mean_what_they_say() {
    check(&scenes(), &mut draw);
}
