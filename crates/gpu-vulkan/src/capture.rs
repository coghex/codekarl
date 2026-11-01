//! The tests' copy of a drawn frame (`TestHooks::capture_frames`): the swap chain's images get `TRANSFER_SRC` usage,
//! and each frame's command buffer ends by copying its image to a host-visible buffer. When that frame's submit is
//! proven, the bytes are handed to the test. This is the only way a test sees the pixels the **surface path** drew
//! (an off-screen readback is not proof of it). Nothing here exists unless a test asks.

use std::sync::{Arc, Mutex};

use ash::vk;

use crate::gpu::{Failure, Gpu};
use crate::readback::allocate;

/// One drawn frame, copied to the host.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct FrameCapture {
    /// The generation of the scene the frame drew (`None` without a slot).
    pub generation: Option<u64>,
    /// The number of the frame's submission: the one a [`DrawnRecord`](crate::DrawnRecord) of the same frame carries, so a test can
    /// tell which queued frame these pixels are.
    pub submission: u64,
    /// The frame's size in pixels.
    pub size: (u32, u32),
    /// The swap chain's format; the bytes are that format's pixels (four or eight bytes each), rows top first.
    pub format: vk::Format,
    /// The pixels.
    pub bytes: Vec<u8>,
}

/// The frames a test collects.
#[doc(hidden)]
pub type FrameCaptures = Arc<Mutex<Vec<FrameCapture>>>;

/// The host-visible buffer one frame at a time is copied into. The frames are submitted one at a time, so it is
/// read (once the submit is proven) before the next frame's copy is recorded.
pub(crate) struct Capture {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub(crate) extent: vk::Extent2D,
    bytes: u64,
}

/// Bytes per pixel of a format the capture can size, `None` for any other (that frame is not copied).
fn pixel_bytes(format: vk::Format) -> Option<u64> {
    match format {
        vk::Format::B8G8R8A8_UNORM
        | vk::Format::B8G8R8A8_SRGB
        | vk::Format::R8G8B8A8_UNORM
        | vk::Format::R8G8B8A8_SRGB
        | vk::Format::A2B10G10R10_UNORM_PACK32
        | vk::Format::A2R10G10B10_UNORM_PACK32 => Some(4),
        vk::Format::R16G16B16A16_SFLOAT | vk::Format::R16G16B16A16_UNORM => Some(8),
        _ => None,
    }
}

fn range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// The commands a capture records, in the order it records them. The command buffer is behind this so a test can read
/// what `record` records instead of what a device would be told; `Recording` is the one implementation that records
/// into a command buffer, and passes every argument on unchanged.
pub(crate) trait Commands {
    /// A `vkCmdPipelineBarrier` with these barriers (no memory barriers, no dependency flags).
    fn barrier(
        &mut self,
        from: vk::PipelineStageFlags,
        to: vk::PipelineStageFlags,
        buffers: &[vk::BufferMemoryBarrier<'_>],
        images: &[vk::ImageMemoryBarrier<'_>],
    );

    /// A `vkCmdCopyImageToBuffer` of `region`, the image in `TRANSFER_SRC_OPTIMAL`.
    fn copy_image_to_buffer(
        &mut self,
        image: vk::Image,
        buffer: vk::Buffer,
        region: vk::BufferImageCopy,
    );
}

/// A command buffer in the recording state, on its device.
struct Recording<'a> {
    device: &'a ash::Device,
    cmd: vk::CommandBuffer,
}

impl Commands for Recording<'_> {
    fn barrier(
        &mut self,
        from: vk::PipelineStageFlags,
        to: vk::PipelineStageFlags,
        buffers: &[vk::BufferMemoryBarrier<'_>],
        images: &[vk::ImageMemoryBarrier<'_>],
    ) {
        // SAFETY: the command buffer is recording, outside a render pass; the barriers name this device's buffer and image.
        unsafe {
            self.device.cmd_pipeline_barrier(
                self.cmd,
                from,
                to,
                vk::DependencyFlags::empty(),
                &[],
                buffers,
                images,
            );
        }
    }

    fn copy_image_to_buffer(
        &mut self,
        image: vk::Image,
        buffer: vk::Buffer,
        region: vk::BufferImageCopy,
    ) {
        // SAFETY: the image is in `TRANSFER_SRC_OPTIMAL` and the buffer holds `extent` pixels of the swap chain's
        // format (`pixel_bytes` bytes each).
        unsafe {
            self.device.cmd_copy_image_to_buffer(
                self.cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
        }
    }
}

/// The image, from the render pass's `PRESENT_SRC_KHR` to the copy's source layout.
fn to_source(image: vk::Image) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
        .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
        .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range())
}

/// The image, back to `PRESENT_SRC_KHR` once the copy has read it.
fn to_present(image: vk::Image) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::TRANSFER_READ)
        .dst_access_mask(vk::AccessFlags::empty())
        .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
        .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range())
}

/// The copy's writes to the whole capture buffer, made available to the host's reads. The fence that proves the
/// submit finished does not do this, and coherent memory does not either: it only spares the invalidate.
fn to_host(buffer: vk::Buffer) -> vk::BufferMemoryBarrier<'static> {
    vk::BufferMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::HOST_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE)
}

/// Records the whole capture of `image`: it to the copy's source layout, the copy into the capture buffer, the
/// copy's writes made visible to the host, and the image back to `PRESENT_SRC_KHR`. `Gpu::record_capture` is a call of
/// this and nothing else, so what a test sees recorded here is what a frame's command buffer gets.
fn record(commands: &mut impl Commands, capture: &Capture, image: vk::Image) {
    commands.barrier(
        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        vk::PipelineStageFlags::TRANSFER,
        &[],
        &[to_source(image)],
    );
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: capture.extent.width,
            height: capture.extent.height,
            depth: 1,
        });
    commands.copy_image_to_buffer(image, capture.buffer, region);
    commands.barrier(
        vk::PipelineStageFlags::TRANSFER,
        vk::PipelineStageFlags::HOST,
        &[to_host(capture.buffer)],
        &[],
    );
    commands.barrier(
        vk::PipelineStageFlags::TRANSFER,
        vk::PipelineStageFlags::BOTTOM_OF_PIPE,
        &[],
        &[to_present(image)],
    );
}

impl Gpu {
    /// Creates the capture buffer for `extent` (once).
    pub(crate) fn ensure_capture(&mut self, extent: vk::Extent2D) -> Result<(), Failure> {
        let Some(pixel) = pixel_bytes(self.format.format) else {
            return Ok(());
        };
        if self.capture.is_some() {
            return Ok(());
        }
        let bytes = u64::from(extent.width) * u64::from(extent.height) * pixel;
        let info = vk::BufferCreateInfo::default()
            .size(bytes)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is live and `info` is valid for the call.
        let buffer = unsafe { self.device.create_buffer(&info, None) }
            .map_err(|code| ("vkCreateBuffer", code))?;
        // SAFETY: the buffer belongs to the device.
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let memory = match allocate(
            &self.device,
            &self.memory,
            requirements,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            false,
        ) {
            Ok(memory) => memory,
            Err(failure) => {
                // SAFETY: the buffer was just created and nothing uses it.
                unsafe { self.device.destroy_buffer(buffer, None) };
                return Err(failure);
            }
        };
        // SAFETY: the memory is fresh and at least the size the buffer requires.
        if let Err(code) = unsafe { self.device.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: nothing uses the buffer.
            unsafe { self.device.destroy_buffer(buffer, None) };
            // SAFETY: the buffer bound to nothing; the memory is fresh and unused.
            unsafe { self.device.free_memory(memory, None) };
            return Err(("vkBindBufferMemory", code));
        }
        self.capture = Some(Capture {
            buffer,
            memory,
            extent,
            bytes,
        });
        Ok(())
    }

    /// Whether the next frame of `extent` is copied.
    pub(crate) fn captures(&self, extent: vk::Extent2D) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|capture| capture.extent == extent)
    }

    /// Records the copy of `image` (in `PRESENT_SRC_KHR` after the render pass) into the capture buffer, makes the
    /// copy visible to the host, and gives the image back to `PRESENT_SRC_KHR`.
    pub(crate) fn record_capture(&self, cmd: vk::CommandBuffer, image: vk::Image) {
        let Some(capture) = &self.capture else {
            return;
        };
        record(
            &mut Recording {
                device: &self.device,
                cmd,
            },
            capture,
            image,
        );
    }

    /// The captured frame's bytes, once its submit is proven finished.
    pub(crate) fn read_capture(&self) -> Option<(Vec<u8>, (u32, u32))> {
        let capture = self.capture.as_ref()?;
        let length = usize::try_from(capture.bytes).ok()?;
        // SAFETY: the memory is host visible and coherent, not mapped, and the submit that wrote it is proven.
        let mapped = unsafe {
            self.device.map_memory(
                capture.memory,
                0,
                capture.bytes,
                vk::MemoryMapFlags::empty(),
            )
        }
        .ok()?;
        // SAFETY: `mapped` points at `length` readable bytes that nothing writes.
        let bytes = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), length) }.to_vec();
        // SAFETY: the memory was mapped just above.
        unsafe { self.device.unmap_memory(capture.memory) };
        Some((bytes, (capture.extent.width, capture.extent.height)))
    }

    /// Destroys the capture buffer: the teardown's.
    pub(crate) fn destroy_capture(&mut self) {
        if let Some(capture) = self.capture.take() {
            // SAFETY: proven unused at the proven end.
            unsafe { self.device.destroy_buffer(capture.buffer, None) };
            // SAFETY: the buffer bound to it is destroyed.
            unsafe { self.device.free_memory(capture.memory, None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use ash::vk::Handle as _;

    use super::*;

    /// What a barrier carried, copied out of the borrowed structures.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct BufferBarrier {
        buffer: vk::Buffer,
        src_access: vk::AccessFlags,
        dst_access: vk::AccessFlags,
        src_family: u32,
        dst_family: u32,
        offset: u64,
        size: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ImageBarrier {
        image: vk::Image,
        src_access: vk::AccessFlags,
        dst_access: vk::AccessFlags,
        old: vk::ImageLayout,
        new: vk::ImageLayout,
        src_family: u32,
        dst_family: u32,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Command {
        Barrier {
            from: vk::PipelineStageFlags,
            to: vk::PipelineStageFlags,
            buffers: Vec<BufferBarrier>,
            images: Vec<ImageBarrier>,
        },
        Copy {
            image: vk::Image,
            buffer: vk::Buffer,
            extent: (u32, u32),
        },
    }

    /// Every command `record` makes, in order.
    #[derive(Default)]
    struct Log(Vec<Command>);

    impl Commands for Log {
        fn barrier(
            &mut self,
            from: vk::PipelineStageFlags,
            to: vk::PipelineStageFlags,
            buffers: &[vk::BufferMemoryBarrier<'_>],
            images: &[vk::ImageMemoryBarrier<'_>],
        ) {
            self.0.push(Command::Barrier {
                from,
                to,
                buffers: buffers
                    .iter()
                    .map(|b| BufferBarrier {
                        buffer: b.buffer,
                        src_access: b.src_access_mask,
                        dst_access: b.dst_access_mask,
                        src_family: b.src_queue_family_index,
                        dst_family: b.dst_queue_family_index,
                        offset: b.offset,
                        size: b.size,
                    })
                    .collect(),
                images: images
                    .iter()
                    .map(|i| ImageBarrier {
                        image: i.image,
                        src_access: i.src_access_mask,
                        dst_access: i.dst_access_mask,
                        old: i.old_layout,
                        new: i.new_layout,
                        src_family: i.src_queue_family_index,
                        dst_family: i.dst_queue_family_index,
                    })
                    .collect(),
            });
        }

        fn copy_image_to_buffer(
            &mut self,
            image: vk::Image,
            buffer: vk::Buffer,
            region: vk::BufferImageCopy,
        ) {
            self.0.push(Command::Copy {
                image,
                buffer,
                extent: (region.image_extent.width, region.image_extent.height),
            });
        }
    }

    const BUFFER: u64 = 0xb0f0;
    const IMAGE: u64 = 0x1a9e;

    /// The commands `record` makes for a capture of a 6 x 4 frame. The handles are made up: nothing uses a device.
    fn recorded() -> Vec<Command> {
        let capture = Capture {
            buffer: vk::Buffer::from_raw(BUFFER),
            memory: vk::DeviceMemory::null(),
            extent: vk::Extent2D {
                width: 6,
                height: 4,
            },
            bytes: 6 * 4 * 4,
        };
        let mut log = Log::default();
        record(&mut log, &capture, vk::Image::from_raw(IMAGE));
        log.0
    }

    /// The position of the one command that is a buffer barrier.
    fn host_barrier(commands: &[Command]) -> usize {
        let at: Vec<usize> = commands
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, Command::Barrier { buffers, .. } if !buffers.is_empty()))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(
            at.len(),
            1,
            "exactly one command carries a buffer barrier: {commands:?}"
        );
        at[0]
    }

    #[test]
    fn the_capture_buffer_is_made_visible_to_the_host_after_the_copy() {
        let commands = recorded();
        let barrier = host_barrier(&commands);
        let copy = commands
            .iter()
            .position(|c| matches!(c, Command::Copy { .. }))
            .expect("the capture copies the image");
        assert!(copy < barrier, "the barrier follows the copy: {commands:?}");
        assert_eq!(
            commands[barrier],
            Command::Barrier {
                from: vk::PipelineStageFlags::TRANSFER,
                to: vk::PipelineStageFlags::HOST,
                buffers: vec![BufferBarrier {
                    buffer: vk::Buffer::from_raw(BUFFER),
                    src_access: vk::AccessFlags::TRANSFER_WRITE,
                    dst_access: vk::AccessFlags::HOST_READ,
                    src_family: vk::QUEUE_FAMILY_IGNORED,
                    dst_family: vk::QUEUE_FAMILY_IGNORED,
                    offset: 0,
                    size: vk::WHOLE_SIZE,
                }],
                images: vec![],
            }
        );
    }

    #[test]
    fn the_copy_reads_the_image_into_the_buffer_the_barrier_names() {
        let commands = recorded();
        let Command::Barrier { buffers, .. } = &commands[host_barrier(&commands)] else {
            unreachable!("host_barrier finds a barrier");
        };
        let copies: Vec<&Command> = commands
            .iter()
            .filter(|c| matches!(c, Command::Copy { .. }))
            .collect();
        assert_eq!(
            copies,
            [&Command::Copy {
                image: vk::Image::from_raw(IMAGE),
                buffer: buffers[0].buffer,
                extent: (6, 4),
            }]
        );
    }

    #[test]
    fn the_image_still_goes_to_the_copy_source_and_back_to_present() {
        let commands = recorded();
        let image = vk::Image::from_raw(IMAGE);
        let image_barriers: Vec<&Command> = commands
            .iter()
            .filter(|c| matches!(c, Command::Barrier { images, .. } if !images.is_empty()))
            .collect();
        assert_eq!(
            image_barriers,
            [
                &Command::Barrier {
                    from: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    to: vk::PipelineStageFlags::TRANSFER,
                    buffers: vec![],
                    images: vec![ImageBarrier {
                        image,
                        src_access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                        dst_access: vk::AccessFlags::TRANSFER_READ,
                        old: vk::ImageLayout::PRESENT_SRC_KHR,
                        new: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        src_family: vk::QUEUE_FAMILY_IGNORED,
                        dst_family: vk::QUEUE_FAMILY_IGNORED,
                    }],
                },
                &Command::Barrier {
                    from: vk::PipelineStageFlags::TRANSFER,
                    to: vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    buffers: vec![],
                    images: vec![ImageBarrier {
                        image,
                        src_access: vk::AccessFlags::TRANSFER_READ,
                        dst_access: vk::AccessFlags::empty(),
                        old: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        new: vk::ImageLayout::PRESENT_SRC_KHR,
                        src_family: vk::QUEUE_FAMILY_IGNORED,
                        dst_family: vk::QUEUE_FAMILY_IGNORED,
                    }],
                },
            ]
        );
    }

    #[test]
    fn the_capture_records_these_four_commands_in_this_order() {
        let kinds: Vec<&str> = recorded()
            .iter()
            .map(|c| match c {
                Command::Copy { .. } => "copy",
                Command::Barrier { buffers, .. } if !buffers.is_empty() => "to host",
                Command::Barrier { images, .. }
                    if images[0].new == vk::ImageLayout::PRESENT_SRC_KHR =>
                {
                    "to present"
                }
                Command::Barrier { .. } => "to source",
            })
            .collect();
        assert_eq!(kinds, ["to source", "copy", "to host", "to present"]);
    }
}
