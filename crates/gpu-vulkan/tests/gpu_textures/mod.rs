//! The Vulkan texture store and revision ledger, off screen (milestone 5.5, issue 1): pooled image memory,
//! the upload worker and its two leases, batched uploads, the revision ledger and its descriptor sets, and the
//! proven end. A submodule of `gpu.rs`, so the group that runs that target runs these.
//!
//! Every case runs under `VK_LAYER_KHRONOS_validation` **with synchronization validation**, and requires an
//! empty validation log from the creation of the instance to its destruction. A negative control is a case
//! that shows the check it names would fail without the behaviour; its log is kept as such.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "the synchronization control records Vulkan commands on the case's own objects, each destroyed at the end of the case"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::{
    Completed, Instance, Loader, PixelSource, RenderQueue, TextureOwner, ValidationLogHandle,
    VulkanContext, load_loader,
};

use super::{assert_log_clean, destroy, validated_with};

mod ledger;
mod store;

/// An image's bytes.
pub(crate) struct Bytes(pub(crate) Vec<u8>);

impl PixelSource for Bytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

/// An RGBA8 image of `width` × `height` whose texel at (x, y) is `texel(x, y)`.
pub(crate) fn image(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Bytes> {
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&texel(x, y));
        }
    }
    Arc::new(Bytes(bytes))
}

/// A pattern that differs in every channel and by position, so a wrong texel, row or image is found.
pub(crate) fn pattern(width: u32, height: u32, seed: u8) -> Arc<Bytes> {
    image(width, height, |x, y| {
        [
            seed.wrapping_add((x % 251) as u8),
            seed.wrapping_mul(3).wrapping_add((y % 241) as u8),
            seed ^ ((x + y) % 239) as u8,
            255 - (seed % 7),
        ]
    })
}

/// How long a case waits for the GPU or the worker before it fails.
pub(crate) const PATIENCE: Duration = Duration::from_secs(30);

/// Waits until the worker acknowledged that it took a batch and reached the stall gate (`stall_reached`),
/// failing after [`PATIENCE`]: the handshake that replaces a sleep before a stop is proven.
pub(crate) fn wait_for_gate(reached: &std::sync::atomic::AtomicBool) {
    let started = Instant::now();
    while !reached.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            started.elapsed() < PATIENCE,
            "the worker never reached the stall gate"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A validated instance with a context and its queue.
pub(crate) struct Kit {
    pub(crate) loader: Loader,
    pub(crate) instance: Instance,
    pub(crate) log: ValidationLogHandle,
    pub(crate) context: Arc<VulkanContext>,
    pub(crate) queue: RenderQueue,
}

impl Kit {
    /// An instance with validation and synchronization validation, and a context. With `events`, the context
    /// enables the portability subset's host events (a held frame needs them); a refusal fails the case.
    pub(crate) fn new(events: bool) -> Self {
        let loader = load_loader().expect("the Vulkan loader loads");
        let instance = validated_with(&loader, true);
        let log = instance.validation_log();
        let context = VulkanContext::new_for_tests(
            &instance,
            None,
            ck_gpu_vulkan::TestFeatures {
                events,
                ..ck_gpu_vulkan::TestFeatures::default()
            },
        )
        .expect("a device context (events requested only when the case needs them)");
        let queue = context.take_queue().expect("the queue is handed out once");
        Self {
            loader,
            instance,
            log,
            context,
            queue,
        }
    }

    /// Drives `owner` (submit, poll) until `want` outcomes arrived, failing after [`PATIENCE`].
    pub(crate) fn drive(
        &mut self,
        owner: &mut TextureOwner<u32>,
        want: usize,
    ) -> Vec<Completed<u32>> {
        let mut out = Vec::new();
        let started = Instant::now();
        while out.len() < want {
            owner.submit(&mut self.queue);
            owner.poll(&mut out);
            assert!(
                started.elapsed() < PATIENCE,
                "only {} of {want} outcomes arrived",
                out.len()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        out
    }

    /// Destroys the instance after the context and the queue went, and requires an empty log. The caller
    /// dropped every store handle first.
    pub(crate) fn finish(self) {
        let Self {
            loader,
            instance,
            log,
            context,
            queue,
        } = self;
        drop(queue);
        assert_eq!(
            Arc::strong_count(&context),
            1,
            "something still holds the context: the device would not be destroyed"
        );
        drop(context);
        destroy(instance);
        assert_log_clean(&log);
        drop(loader);
    }
}

/// Synchronization validation needs validation, and says so; with both on, a missing barrier between two
/// writes of one buffer is reported as a synchronization hazard (negative control: the hazard report is what
/// the store's and the ledger's cases would show if their barriers were missing, and it is kept as such).
#[test]
fn synchronization_validation_is_on_and_reports_a_hazard() {
    use ck_gpu_vulkan::{InstanceOptions, create_instance_with, vk};

    // The creation below is refused before an instance exists, but the loader's layer variables can put the layer in any instance
    // this case makes, so it holds the gate from its start (the gate is reentrant).
    super::hold_layer_gate();
    let loader = load_loader().expect("the Vulkan loader loads");
    let refused = create_instance_with(
        &loader,
        &[],
        InstanceOptions {
            validation: false,
            synchronization: true,
        },
    )
    .expect_err("synchronization validation without validation");
    assert!(
        refused.message().contains("needs validation"),
        "{}",
        refused.message()
    );
    drop(loader);

    let mut kit = Kit::new(false);
    let context = kit.context.clone();
    let device = context.device();
    let info = vk::BufferCreateInfo::default()
        .size(256)
        .usage(vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::TRANSFER_SRC);
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let (buffer, memory) = unsafe {
        let buffer = device.create_buffer(&info, None).expect("buffer");
        let requirements = device.get_buffer_memory_requirements(buffer);
        let memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(
                        (0..context.memory_properties().memory_type_count)
                            .find(|index| requirements.memory_type_bits & (1 << index) != 0)
                            .expect("a memory type"),
                    ),
                None,
            )
            .expect("memory");
        device.bind_buffer_memory(buffer, memory, 0).expect("bind");
        (buffer, memory)
    };
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let (pool, cmd, fence) = unsafe {
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(context.family()),
                None,
            )
            .expect("pool");
        let cmd = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .expect("cmd")[0];
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .expect("fence");
        (pool, cmd, fence)
    };
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe {
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .expect("begin");
        // Two writes of the same bytes with no barrier between them: a write-after-write hazard.
        device.cmd_fill_buffer(cmd, buffer, 0, 128, 1);
        device.cmd_fill_buffer(cmd, buffer, 0, 128, 2);
        device.end_command_buffer(cmd).expect("end");
    }
    kit.queue.submit_commands(cmd, fence).expect("submit");
    // SAFETY: the fence belongs to this device.
    unsafe { device.wait_for_fences(&[fence], true, 5_000_000_000) }.expect("the commands finish");
    let messages = kit.log.messages();
    println!("negative control validation log: {messages:#?}");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("SYNC-HAZARD-WRITE-AFTER-WRITE")),
        "synchronization validation reports the missing barrier: {messages:?}"
    );
    // SAFETY: the commands finished.
    unsafe {
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(buffer, None);
        device.free_memory(memory, None);
    }
    let Kit {
        instance,
        queue,
        context: kit_context,
        ..
    } = kit;
    drop(queue);
    drop(kit_context);
    drop(context);
    destroy(instance);
}
