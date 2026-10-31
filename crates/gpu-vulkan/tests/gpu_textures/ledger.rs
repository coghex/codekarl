//! The revision ledger off screen: transitions, frame proofs across fence reuse, descriptor sets and the end.
//!
//! The test frames are real command buffers: each binds the resolved revision's descriptor set (with the
//! ledger's pipeline layout; no pipeline is needed to bind) and copies from its image to a host buffer, restoring
//! the sampled layout as the store's readback does. A frame is held pending by a `VkEvent` the test sets from
//! the host (`vkCmdWaitEvents`), never by a sleep. The context comes from `new_for_tests` with `events: true`;
//! a refusal there fails the case, which is never skipped.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::too_many_lines,
    reason = "the test frames and the raw controls record Vulkan commands on the case's own objects, each destroyed at the end of the case"
)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::{
    FrameSubmission, LedgerCleanup, LedgerError, LedgerEvent, LedgerLimits, OfferRefusal,
    RevisionState, TextureLedger, TextureLimits, TextureTestHooks, TextureWakes, vk,
};

use super::{Bytes, Kit, PATIENCE, pattern};

const PICK: Duration = Duration::from_millis(1);

/// A host buffer a frame copies its textures into, one region per texture.
struct HostBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    regions: Vec<(u64, u64)>,
}

/// One submitted test frame.
#[derive(Debug)]
struct Frame {
    submission: FrameSubmission,
    host: usize,
}

/// A ledger, its kit and the raw objects the test frames use.
struct Rig {
    kit: Kit,
    ledger: Option<TextureLedger<u32>>,
    pool: vk::CommandPool,
    events: Vec<vk::Event>,
    buffers: Vec<HostBuffer>,
    sources: HashMap<(u32, u64), Arc<Bytes>>,
}

/// Records one test frame. A free function, so a ticket can be held while the queue is borrowed.
fn record(
    context: &ck_gpu_vulkan::VulkanContext,
    pool: vk::CommandPool,
    layout: vk::PipelineLayout,
    samplers: [vk::DescriptorSet; 2],
    textures: &[(vk::DescriptorSet, vk::Image, u32, u32)],
    event: Option<vk::Event>,
) -> (vk::CommandBuffer, HostBuffer) {
    let device = context.device();
    let mut regions = Vec::new();
    let mut size = 0_u64;
    for &(_, _, width, height) in textures {
        let bytes = u64::from(width) * u64::from(height) * 4;
        regions.push((size, bytes));
        size += bytes.div_ceil(256) * 256;
    }
    let info = vk::BufferCreateInfo::default()
        .size(size.max(256))
        .usage(vk::BufferUsageFlags::TRANSFER_DST);
    // SAFETY (all raw calls below): the case's own objects on its own device, destroyed in `Rig::finish`.
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let buffer = unsafe { device.create_buffer(&info, None) }.expect("host buffer");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let properties = context.memory_properties();
    let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    let memory_type = (0..properties.memory_type_count)
        .find(|index| {
            requirements.memory_type_bits & (1 << index) != 0
                && properties.memory_types[*index as usize]
                    .property_flags
                    .contains(wanted)
        })
        .expect("a host-visible coherent memory type");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )
    }
    .expect("host memory");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe { device.bind_buffer_memory(buffer, memory, 0) }.expect("bind");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let cmd = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .command_buffer_count(1),
        )
    }
    .expect("command buffer")[0];
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let transition = |image, old, new, src, dst| {
        vk::ImageMemoryBarrier::default()
            .src_access_mask(src)
            .dst_access_mask(dst)
            .old_layout(old)
            .new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
    };
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe {
        device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .expect("begin");
        if let Some(event) = event {
            device.cmd_wait_events(
                cmd,
                &[event],
                vk::PipelineStageFlags::HOST,
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::TRANSFER,
                &[],
                &[],
                &[],
            );
        }
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            layout,
            0,
            &[samplers[0]],
            &[],
        );
        for (position, &(set, image, width, height)) in textures.iter().enumerate() {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                1,
                &[set],
                &[],
            );
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[transition(
                    image,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::TRANSFER_READ,
                )],
            );
            device.cmd_copy_image_to_buffer(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[vk::BufferImageCopy::default()
                    .buffer_offset(regions[position].0)
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })],
            );
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[transition(
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::TRANSFER_READ,
                    vk::AccessFlags::SHADER_READ,
                )],
            );
        }
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &[],
            &[vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(buffer)
                .size(vk::WHOLE_SIZE)],
            &[],
        );
        device.end_command_buffer(cmd).expect("end");
    }
    (
        cmd,
        HostBuffer {
            buffer,
            memory,
            regions,
        },
    )
}

impl Rig {
    fn new(max_textures: usize, extras: usize) -> Self {
        Self::with(max_textures, extras, TextureTestHooks::default())
    }

    fn with(max_textures: usize, extras: usize, hooks: TextureTestHooks) -> Self {
        Self::with_wakes(max_textures, extras, hooks, TextureWakes::default())
    }

    fn with_wakes(
        max_textures: usize,
        extras: usize,
        hooks: TextureTestHooks,
        wakes: TextureWakes,
    ) -> Self {
        let kit = Kit::new(true);
        let ledger = TextureLedger::<u32>::start_with_test_hooks(
            kit.context.clone(),
            TextureLimits::default(),
            LedgerLimits {
                max_extra_revisions: extras,
                ..LedgerLimits::default()
            },
            max_textures,
            wakes,
            hooks,
        )
        .expect("the ledger starts");
        let info = vk::CommandPoolCreateInfo::default().queue_family_index(kit.context.family());
        // SAFETY: the case's own pool on its own device, destroyed in `finish`.
        let pool = unsafe { kit.context.device().create_command_pool(&info, None) }.expect("pool");
        Self {
            kit,
            ledger: Some(ledger),
            pool,
            events: Vec::new(),
            buffers: Vec::new(),
            sources: HashMap::new(),
        }
    }

    fn ledger(&self) -> &TextureLedger<u32> {
        self.ledger.as_ref().expect("the ledger exists")
    }

    /// Offers a pattern image and remembers its pixels.
    fn offer(&mut self, key: u32, revision: u64, width: u32, height: u32) {
        let seed = u8::try_from((u64::from(key) * 31 + revision * 7) % 251).expect("a byte");
        let pixels = pattern(width, height, seed);
        self.sources.insert((key, revision), pixels.clone());
        self.ledger()
            .offer(key, revision, width, height, pixels)
            .expect("the offer is admitted");
    }

    /// Pumps until `done` says the events so far are enough, failing after [`PATIENCE`].
    fn pump_until(
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
            assert!(started.elapsed() < PATIENCE, "events so far: {events:?}");
            std::thread::sleep(PICK);
        }
    }

    /// One pump.
    fn pump(&mut self, out: &mut Vec<LedgerEvent<u32>>) {
        let ledger = self.ledger.as_ref().expect("the ledger exists");
        ledger.pump(&mut self.kit.queue, out);
    }

    /// The first step of a pump alone: submit the closed upload batch.
    fn submit_upload(&mut self) -> bool {
        let ledger = self.ledger.as_ref().expect("the ledger exists");
        ledger.test_submit_upload(&mut self.kit.queue)
    }

    /// Pumps until `revision` of `key` is `Prepared`, with no upload failure on the way.
    fn prepared(&mut self, key: u32, revision: u64) {
        let events = self.pump_until(|_, events| {
            events.iter().any(|event| {
                matches!(event, LedgerEvent::Prepared { key: k, revision: r } if *k == key && *r == revision)
            })
        });
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, LedgerEvent::UploadFailed { .. })),
            "{events:?}"
        );
    }

    /// Pumps until every `(key, revision)` of `wanted` is `Prepared` (one pump may report several).
    fn prepared_all(&mut self, wanted: &[(u32, u64)]) {
        let mut missing: Vec<(u32, u64)> = wanted.to_vec();
        self.pump_until(|_, events| {
            for event in events {
                if let LedgerEvent::Prepared { key, revision } = event {
                    missing.retain(|(k, r)| !(k == key && r == revision));
                }
                assert!(
                    !matches!(event, LedgerEvent::UploadFailed { .. }),
                    "{event:?}"
                );
            }
            missing.is_empty()
        });
    }

    /// Prepared, committed and adopted: the key shows `revision`.
    fn show(&mut self, key: u32, revision: u64) {
        self.prepared(key, revision);
        self.ledger().commit(key, revision).expect("commit");
        let mut adopted = Vec::new();
        self.ledger().adopt(16, &mut adopted);
        assert!(adopted.contains(&(key, revision)), "{adopted:?}");
        assert_eq!(
            self.ledger().test_revision_state(key, revision),
            Some(RevisionState::Active)
        );
    }

    /// Offers an 8 x 8 image and shows it.
    fn show_new(&mut self, key: u32, revision: u64) {
        self.offer(key, revision, 8, 8);
        self.show(key, revision);
    }

    fn event(&mut self) -> vk::Event {
        // SAFETY: the case's own event on its own device, destroyed in `finish`.
        let event = unsafe {
            self.kit
                .context
                .device()
                .create_event(&vk::EventCreateInfo::default(), None)
        }
        .expect("the events feature is enabled");
        self.events.push(event);
        event
    }

    fn set_event(&self, event: vk::Event) {
        // SAFETY: the event belongs to this device.
        unsafe { self.kit.context.device().set_event(event) }.expect("set the event");
    }

    fn event_is_set(&self, event: vk::Event) -> bool {
        // SAFETY: the event belongs to this device.
        unsafe { self.kit.context.device().get_event_status(event) }.expect("event status")
    }

    /// Begins a ticket, resolves `keys`, records a frame (held by `event` when given) and submits it. A refused
    /// submit ends the ticket without submitting and returns the refusal.
    fn frame(&mut self, keys: &[u32], event: Option<vk::Event>) -> Result<Frame, LedgerError> {
        let ledger = self.ledger.as_ref().expect("the ledger exists");
        let ticket = ledger.begin_frame()?;
        let mut textures = Vec::new();
        for key in keys {
            let resolved = ticket
                .resolve(*key)
                .unwrap_or_else(|| panic!("key {key} resolves"));
            textures.push((
                resolved.descriptor_set(),
                resolved.image(),
                resolved.width(),
                resolved.height(),
            ));
        }
        let (cmd, host) = record(
            &self.kit.context,
            self.pool,
            ledger.pipeline_layout(),
            ledger.sampler_sets(),
            &textures,
            event,
        );
        self.buffers.push(host);
        let host = self.buffers.len() - 1;
        match ticket.submit(&mut self.kit.queue, cmd, &[], &[]) {
            Ok(submission) => Ok(Frame { submission, host }),
            Err(error) => {
                ticket.not_submitted().expect("a refused ticket ends");
                Err(error)
            }
        }
    }

    /// Pumps until the proof of `submission` is recorded.
    fn until_proven(&mut self, submission: FrameSubmission) -> Vec<LedgerEvent<u32>> {
        let ledger = self.ledger.take().expect("the ledger exists");
        let mut events = Vec::new();
        let started = Instant::now();
        while !ledger.frame_proven(submission) {
            ledger.pump(&mut self.kit.queue, &mut events);
            assert!(
                started.elapsed() < PATIENCE,
                "submission {} was never proven",
                submission.number()
            );
            std::thread::sleep(PICK);
        }
        self.ledger = Some(ledger);
        events
    }

    /// The bytes a frame copied for its `index`-th texture.
    fn read(&self, frame: &Frame, index: usize) -> Vec<u8> {
        let host = &self.buffers[frame.host];
        let (offset, length) = host.regions[index];
        let device = self.kit.context.device();
        // SAFETY: the memory is host-visible and coherent, not mapped, and the frame is proven finished.
        let pointer = unsafe {
            device.map_memory(host.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        }
        .expect("map");
        let mut bytes = vec![0_u8; usize::try_from(length).expect("small")];
        // SAFETY: the mapping holds the region the finished frame wrote.
        unsafe {
            std::ptr::copy_nonoverlapping(
                pointer
                    .cast::<u8>()
                    .add(usize::try_from(offset).expect("small")),
                bytes.as_mut_ptr(),
                bytes.len(),
            );
            device.unmap_memory(host.memory);
        }
        bytes
    }

    fn source(&self, key: u32, revision: u64) -> &[u8] {
        &self.sources[&(key, revision)].0
    }

    /// Ends the ledger (proven), destroys the case's raw objects and the kit.
    fn finish(mut self) {
        let ledger = self.ledger.take().expect("the ledger exists");
        assert_eq!(
            ledger.shutdown(Instant::now() + PATIENCE),
            LedgerCleanup::Proven,
            "the end is proven"
        );
        drop(ledger);
        let device = self.kit.context.device();
        // SAFETY: the ledger's end is proven, so every frame that used these finished.
        unsafe {
            for host in self.buffers.drain(..) {
                device.destroy_buffer(host.buffer, None);
                device.free_memory(host.memory, None);
            }
            for event in self.events.drain(..) {
                device.destroy_event(event, None);
            }
            device.destroy_command_pool(self.pool, None);
        }
        self.kit.finish();
    }
}

/// Test 6: Offer, `Prepared`, commit, adopt at the next boundary, `Active`; a frame then reads the adopted image
/// through the revision's own descriptor set.
///
/// Negative control: before commit and before adoption nothing resolves and an adoption pass adopts nothing
/// (the assertions that would fail if a prepared revision were visible).
#[test]
fn a_revision_is_shown_only_after_commit_and_adoption() {
    let mut rig = Rig::new(8, 4);
    rig.offer(1, 1, 16, 16);
    rig.prepared(1, 1);
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Prepared)
    );
    let mut adopted = Vec::new();
    rig.ledger().adopt(8, &mut adopted);
    assert!(adopted.is_empty(), "a prepared revision is not adopted");
    {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        assert!(
            ticket.resolve(1).is_none(),
            "nothing is shown before commit and adoption"
        );
        ticket.discarded().expect("ends");
    }
    rig.ledger().commit(1, 1).expect("commit");
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Committed)
    );
    {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        assert!(
            ticket.resolve(1).is_none(),
            "a committed revision waits for the boundary"
        );
        ticket.discarded().expect("ends");
    }
    rig.ledger().adopt(8, &mut adopted);
    assert_eq!(adopted, vec![(1, 1)]);
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Active)
    );
    let frame = rig.frame(&[1], None).expect("a frame");
    rig.until_proven(frame.submission);
    assert!(
        rig.read(&frame, 0) == rig.source(1, 1),
        "the frame reads the adopted image"
    );
    rig.finish();
}

/// Test 6: Reject: a prepared revision is rejected, released by a later pump, and was never shown.
#[test]
fn a_rejected_revision_is_released_and_never_shown() {
    let mut rig = Rig::new(8, 4);
    rig.offer(1, 1, 8, 8);
    rig.prepared(1, 1);
    rig.ledger().reject(1, 1).expect("reject");
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Retired)
    );
    rig.pump_until(|ledger, _| ledger.test_revision_state(1, 1).is_none());
    assert_eq!(rig.ledger().test_live_revisions(), 0);
    let ticket = rig.ledger().begin_frame().expect("a ticket");
    assert!(ticket.resolve(1).is_none());
    ticket.discarded().expect("ends");
    rig.finish();
}

/// Test 6: A reload before versus after commit. Before: a second prepared revision does not displace the first
/// prepared one, and committing it leaves the other prepared. After: a revision committed and not yet adopted
/// is retired by a newer commit and never shown.
#[test]
fn a_reload_before_and_after_commit() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    // Before commit: revisions 2 and 3 are both prepared; committing 3 leaves 2 prepared.
    rig.offer(1, 2, 8, 8);
    rig.offer(1, 3, 8, 8);
    rig.prepared_all(&[(1, 2), (1, 3)]);
    rig.ledger().commit(1, 3).expect("commit");
    assert_eq!(
        rig.ledger().test_revision_state(1, 2),
        Some(RevisionState::Prepared)
    );
    rig.ledger()
        .reject(1, 2)
        .expect("reject the uncommitted one");
    // After commit: 3 is committed and not adopted; committing a newer 4 retires it.
    rig.offer(1, 4, 8, 8);
    rig.prepared(1, 4);
    rig.ledger().commit(1, 4).expect("commit");
    assert_eq!(
        rig.ledger().test_revision_state(1, 3),
        Some(RevisionState::Retired)
    );
    let mut adopted = Vec::new();
    rig.ledger().adopt(8, &mut adopted);
    assert_eq!(
        adopted,
        vec![(1, 4)],
        "only the newest committed revision is adopted"
    );
    rig.pump_until(|ledger, _| {
        ledger.test_revision_state(1, 2).is_none()
            && ledger.test_revision_state(1, 3).is_none()
            && ledger.test_revision_state(1, 1).is_none()
    });
    let frame = rig.frame(&[1], None).expect("a frame");
    rig.until_proven(frame.submission);
    assert!(rig.read(&frame, 0) == rig.source(1, 4));
    assert_eq!(rig.ledger().test_live_revisions(), 1);
    rig.finish();
}

/// Test 6: The extras cap: past `max_extra_revisions` an offer is the protocol refusal.
#[test]
fn the_extras_cap_refuses_an_offer_past_reserved_room() {
    let mut rig = Rig::new(8, 1);
    rig.show_new(1, 1);
    assert_eq!(rig.ledger().extra_room(), 1);
    rig.offer(1, 2, 8, 8);
    assert_eq!(rig.ledger().extra_room(), 0);
    let refused = rig
        .ledger()
        .offer(1, 3, 8, 8, pattern(8, 8, 1))
        .expect_err("past the cap");
    assert!(matches!(refused, OfferRefusal::Protocol(_)), "{refused:?}");
    rig.prepared(1, 2);
    rig.ledger().reject(1, 2).expect("reject");
    rig.pump_until(|ledger, _| ledger.test_revision_state(1, 2).is_none());
    assert_eq!(
        rig.ledger().extra_room(),
        1,
        "the released extra frees its charge"
    );
    rig.finish();
}

/// Test 6: A retired revision is kept while a held frame pins it, and released after the event and the fence.
#[test]
fn a_retired_revision_is_kept_while_a_held_frame_pins_it() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    rig.offer(1, 2, 8, 8);
    rig.prepared(1, 2);
    rig.ledger().commit(1, 2).expect("commit");
    let event = rig.event();
    let held = rig.frame(&[1], Some(event)).expect("a held frame");
    let mut adopted = Vec::new();
    rig.ledger().adopt(8, &mut adopted);
    assert_eq!(adopted, vec![(1, 2)]);
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Retired)
    );
    for _ in 0..20 {
        let mut events = Vec::new();
        rig.pump(&mut events);
        std::thread::sleep(PICK);
    }
    assert!(!rig.event_is_set(event));
    assert!(!rig.ledger().frame_proven(held.submission));
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Retired),
        "kept while the held frame pins it"
    );
    rig.set_event(event);
    rig.pump_until(|ledger, _| ledger.test_revision_state(1, 1).is_none());
    assert!(rig.ledger().frame_proven(held.submission));
    assert!(
        rig.read(&held, 0) == rig.source(1, 1),
        "the held frame read the old revision"
    );
    rig.finish();
}

/// Test 6: Retirement progresses with no later edit: `pump` alone releases a retired key's revisions and reports
/// `KeyRetired`, including once a held frame's proof arrives.
#[test]
fn retirement_progresses_with_pump_alone() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    rig.show_new(2, 1);
    let event = rig.event();
    let held = rig.frame(&[1], Some(event)).expect("a held frame");
    rig.ledger().retire_key(1);
    rig.ledger().retire_key(2);
    let events = rig.pump_until(|_, events| {
        events
            .iter()
            .any(|e| matches!(e, LedgerEvent::KeyRetired { key: 2 }))
    });
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LedgerEvent::KeyRetired { key: 1 })),
        "key 1 is pinned by the held frame"
    );
    rig.set_event(event);
    let events = rig.pump_until(|_, events| {
        events
            .iter()
            .any(|e| matches!(e, LedgerEvent::KeyRetired { key: 1 }))
    });
    assert!(rig.ledger().frame_proven(held.submission));
    assert!(!events.is_empty());
    assert_eq!(rig.ledger().test_live_revisions(), 0);
    rig.finish();
}

/// Test 6: CK-150: an adopt while a ticket is Building adopts nothing, and a repeated resolve across `retire_key`
/// returns the cached revision while a first lookup after it returns `None`.
#[test]
fn ticket_coherence_across_adopt_and_retire_key() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    rig.show_new(2, 1);
    rig.offer(1, 2, 8, 8);
    rig.prepared(1, 2);
    rig.ledger().commit(1, 2).expect("commit");
    {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        let mut adopted = Vec::new();
        rig.ledger().adopt(8, &mut adopted);
        assert!(
            adopted.is_empty(),
            "an adopt while a ticket is Building adopts nothing"
        );
        let first = ticket.resolve(1).expect("key 1 resolves").revision();
        assert_eq!(first, 1);
        ticket.discarded().expect("ends");
    }
    let mut adopted = Vec::new();
    rig.ledger().adopt(8, &mut adopted);
    assert_eq!(
        adopted,
        vec![(1, 2)],
        "adoption resumes after the ticket ended"
    );
    {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        assert_eq!(ticket.resolve(1).expect("resolves").revision(), 2);
        rig.ledger().retire_key(1);
        rig.ledger().retire_key(2);
        assert_eq!(
            ticket
                .resolve(1)
                .expect("the cached revision survives retire_key")
                .revision(),
            2,
            "a repeated resolve across retire_key returns the cached revision"
        );
        assert!(
            ticket.resolve(2).is_none(),
            "a first lookup after retire_key returns None"
        );
        ticket.discarded().expect("ends");
    }
    rig.pump_until(|ledger, _| ledger.test_live_revisions() == 0);
    rig.finish();
}

/// Test 6: Frame proofs across fence reuse: submission *n* completes and `pump` records it; two more tickets reuse
/// and reset *n*'s fence, the last held by the event; `frame_proven(n)` stays true while `frame_proven(n + 2)`
/// is false until the event is set and a `pump` records it; a second ticket cannot submit while that frame is
/// pending.
///
/// Negative control (pure, no device, in `proofs.rs`): answering from the current fence status reports *n*
/// unproven after the reset, or *n + 2* proven early.
#[test]
fn frame_proofs_survive_fence_reuse() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    let first = rig.frame(&[1], None).expect("frame n");
    rig.until_proven(first.submission);
    assert!(rig.ledger().frame_proven(first.submission));
    let second = rig.frame(&[1], None).expect("frame n + 1 reuses n's fence");
    assert_eq!(second.submission.number(), first.submission.number() + 1);
    rig.until_proven(second.submission);
    let event = rig.event();
    let held = rig
        .frame(&[1], Some(event))
        .expect("frame n + 2 reuses and resets the fence again");
    assert_eq!(held.submission.number(), first.submission.number() + 2);
    for _ in 0..20 {
        let mut events = Vec::new();
        rig.pump(&mut events);
        std::thread::sleep(PICK);
    }
    assert!(!rig.event_is_set(event));
    assert!(
        rig.ledger().frame_proven(first.submission),
        "n stays proven after its fence was reset twice"
    );
    assert!(rig.ledger().frame_proven(second.submission));
    assert!(
        !rig.ledger().frame_proven(held.submission),
        "n + 2 is held by the event"
    );
    // The held frame is the one pending submission: a second ticket cannot submit.
    let busy = rig
        .frame(&[1], None)
        .expect_err("a second submit while one is pending");
    assert_eq!(busy, LedgerError::TicketBusy);
    rig.set_event(event);
    rig.until_proven(held.submission);
    assert!(rig.ledger().frame_proven(first.submission));
    assert!(rig.read(&held, 0) == rig.source(1, 1));
    rig.finish();
}

/// Test 7: Descriptor sets, ordered for one queue: the upload completes on the GPU before the held frame is
/// submitted, and the ledger observes it only after that frame is pending. A set is recycled only on release.
///
/// Negative control: recycling at retirement instead of release would put S1 on the free list at step 4, and
/// step 5's write would hit a set that a pending command buffer binds; the raw control at the end performs
/// that write and shows core validation reporting it.
#[test]
fn a_set_is_written_while_a_frame_is_pending_and_recycled_only_on_release() {
    let mut rig = Rig::new(8, 4);
    // 1. Key K's revision 1 is Active with set S1.
    rig.show_new(1, 1);
    let s1 = {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        let set = ticket.resolve(1).expect("K resolves").descriptor_set();
        ticket.discarded().expect("ends");
        set
    };
    // 2. Offer L's revision 1; submit its upload batch without observing it; the GPU finishes it.
    rig.offer(2, 1, 8, 8);
    let started = Instant::now();
    while !rig.ledger().envelope_ready_unsubmitted() {
        assert!(started.elapsed() < PATIENCE, "the batch was never closed");
        std::thread::sleep(PICK);
    }
    assert!(rig.submit_upload(), "the upload batch is submitted");
    assert!(
        rig.ledger().test_wait_upload(PATIENCE),
        "the GPU completed the batch"
    );
    assert_eq!(
        rig.ledger().test_revision_state(2, 1),
        Some(RevisionState::InFlight),
        "the ledger has not observed the completion, so L has no set"
    );
    // 3. Resolve K (revision 1, S1) and submit frame F, which binds S1 and waits on the unset event.
    let event = rig.event();
    let held = rig.frame(&[1], Some(event)).expect("frame F");
    // 4. retire_key(K): revision 1 is Retired and still pinned by F.
    rig.ledger().retire_key(1);
    assert_eq!(
        rig.ledger().test_revision_state(1, 1),
        Some(RevisionState::Retired)
    );
    // 5. pump observes L's upload and writes L's set while F is pending.
    rig.prepared(2, 1);
    assert!(!rig.event_is_set(event), "the event is unset");
    assert!(!rig.ledger().frame_proven(held.submission), "F is pending");
    rig.ledger().commit(2, 1).expect("commit");
    let mut adopted = Vec::new();
    rig.ledger().adopt(8, &mut adopted);
    let s2 = {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        let set = ticket.resolve(2).expect("L resolves").descriptor_set();
        ticket.discarded().expect("ends");
        set
    };
    assert_ne!(
        s2, s1,
        "L got a set of its own while S1 is still bound by F"
    );
    // 6. Set the event: a pump records F's proof and releases K's revision 1, putting S1 on the free list; key
    // M's next revision gets S1.
    rig.set_event(event);
    rig.pump_until(|ledger, _| ledger.test_revision_state(1, 1).is_none());
    assert!(rig.ledger().frame_proven(held.submission));
    rig.offer(3, 1, 8, 8);
    rig.show(3, 1);
    let s3 = {
        let ticket = rig.ledger().begin_frame().expect("a ticket");
        let set = ticket.resolve(3).expect("M resolves").descriptor_set();
        ticket.discarded().expect("ends");
        set
    };
    assert_eq!(s3, s1, "the released revision's set is reused");
    rig.finish();
}

/// Test 7: With `ordinary_sets` one below the keys offered, the extra key is `UploadFailed` naming the counts, the
/// shown revisions are unchanged, and after a retirement the key succeeds.
#[test]
fn descriptor_set_exhaustion_fails_one_revision_and_recovers() {
    // ordinary_sets = 2 textures + 1 extra + 1 = 4.
    let mut rig = Rig::new(2, 1);
    for key in 1..=4_u32 {
        rig.show_new(key, 1);
    }
    rig.offer(5, 1, 8, 8);
    let events = rig.pump_until(|_, events| {
        events
            .iter()
            .any(|e| matches!(e, LedgerEvent::UploadFailed { .. }))
    });
    let reason = events
        .iter()
        .find_map(|event| match event {
            LedgerEvent::UploadFailed { key: 5, reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("key 5 failed");
    assert!(
        reason.contains("4 in use") && reason.contains("4 configured"),
        "{reason}"
    );
    for key in 1..=4_u32 {
        assert_eq!(
            rig.ledger().test_revision_state(key, 1),
            Some(RevisionState::Active),
            "the shown revisions are unchanged"
        );
    }
    assert_eq!(
        rig.ledger().test_revision_state(5, 1),
        None,
        "nothing of the failed key leaks"
    );
    rig.ledger().retire_key(1);
    rig.pump_until(|ledger, _| ledger.test_revision_state(1, 1).is_none());
    rig.offer(5, 2, 8, 8);
    rig.show(5, 2);
    rig.finish();
}

/// Test 8: A stop with a submitted upload batch and a held frame proves within the deadline once the event is set.
#[test]
fn a_stop_with_a_held_frame_and_a_submitted_batch_proves_once_the_event_is_set() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    let event = rig.event();
    let held = rig.frame(&[1], Some(event)).expect("a held frame");
    // The upload batch is queued behind the held frame.
    rig.offer(2, 1, 8, 8);
    let started = Instant::now();
    while !rig.ledger().envelope_ready_unsubmitted() {
        assert!(started.elapsed() < PATIENCE, "the batch was never closed");
        std::thread::sleep(PICK);
    }
    assert!(rig.submit_upload());
    let context = rig.kit.context.clone();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        // SAFETY: the event belongs to this device and outlives the thread (the case joins it).
        unsafe { context.device().set_event(event) }.expect("set the event");
    });
    let begun = Instant::now();
    let ledger = rig.ledger.take().expect("the ledger exists");
    assert_eq!(
        ledger.shutdown(Instant::now() + PATIENCE),
        LedgerCleanup::Proven
    );
    assert!(
        begun.elapsed() >= Duration::from_millis(100),
        "the stop waited for the held frame"
    );
    assert!(
        ledger.frame_proven(held.submission),
        "the proof was recorded"
    );
    setter.join().expect("the setter finished");
    rig.ledger = Some(ledger);
    rig.finish_after_proven_end();
}

/// Test 8: A frame held past the deadline gives `Unproven`, and the objects remain (a read of the ledger's own live
/// maps, not a counter); once the event is set the same ledger proves the end.
#[test]
fn a_frame_held_past_the_deadline_leaves_the_end_unproven_and_the_objects_remain() {
    let mut rig = Rig::new(8, 4);
    rig.show_new(1, 1);
    let event = rig.event();
    let held = rig.frame(&[1], Some(event)).expect("a held frame");
    let outcome = rig
        .ledger()
        .shutdown(Instant::now() + Duration::from_millis(150));
    assert!(
        matches!(outcome, LedgerCleanup::Unproven { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        rig.ledger().test_live_revisions(),
        1,
        "the revision and its image remain"
    );
    assert!(!rig.ledger().frame_proven(held.submission));
    super::super::assert_log_clean(&rig.kit.log);
    rig.set_event(event);
    rig.finish();
}

/// Test 8: A worker held by the stall gate gives `Unproven`; once the gate opens the same ledger proves the end.
#[test]
fn a_worker_held_by_the_stall_gate_leaves_the_ledger_unproven() {
    let gate: ck_gpu_vulkan::StallGate =
        Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hooks = TextureTestHooks {
        stall_gate: Some(gate.clone()),
        stall_reached: Some(reached.clone()),
        ..TextureTestHooks::default()
    };
    let mut rig = Rig::with(8, 4, hooks);
    rig.offer(1, 1, 8, 8);
    super::wait_for_gate(&reached);
    let outcome = rig
        .ledger()
        .shutdown(Instant::now() + Duration::from_millis(150));
    assert!(
        matches!(outcome, LedgerCleanup::Unproven { .. }),
        "{outcome:?}"
    );
    {
        let (flag, changed) = &*gate;
        *flag.lock().expect("gate") = true;
        changed.notify_all();
    }
    rig.finish();
}

impl Rig {
    /// Like `finish`, for a ledger whose end was already proven and released.
    fn finish_after_proven_end(mut self) {
        let ledger = self.ledger.take().expect("the ledger exists");
        drop(ledger);
        let device = self.kit.context.device();
        // SAFETY: the ledger's end is proven, so every frame that used these finished.
        unsafe {
            for host in self.buffers.drain(..) {
                device.destroy_buffer(host.buffer, None);
                device.free_memory(host.memory, None);
            }
            for event in self.events.drain(..) {
                device.destroy_event(event, None);
            }
            device.destroy_command_pool(self.pool, None);
        }
        self.kit.finish();
    }
}

/// Negative control for the descriptor-set case: recycling at retirement instead of release would put S1 on
/// the free list at step 4, and step 5 would then write S1 while F is pending. This performs that write on raw
/// objects (a set bound by a command buffer that waits on an unset event) and shows core validation reporting
/// it (`vkUpdateDescriptorSets` on a set in use by a pending command buffer, without update-after-bind), not a
/// timeout. The log is kept as such: this case asserts that it is **not** empty.
#[test]
fn negative_control_a_set_written_while_a_pending_frame_binds_it_is_reported() {
    let mut kit = Kit::new(true);
    let context = kit.context.clone();
    let device = context.device();
    // SAFETY (all raw calls below): the case's own objects on its own device, destroyed at the end.
    let binding = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT | vk::ShaderStageFlags::COMPUTE)];
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let set_layout = unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding),
            None,
        )
    }
    .expect("layout");
    let layouts = [set_layout];
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let pipeline_layout = unsafe {
        device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts),
            None,
        )
    }
    .expect("pipeline layout");
    let sizes = [vk::DescriptorPoolSize::default()
        .ty(vk::DescriptorType::SAMPLED_IMAGE)
        .descriptor_count(1)];
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let pool = unsafe {
        device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(1)
                .pool_sizes(&sizes),
            None,
        )
    }
    .expect("pool");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let set = unsafe {
        device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&layouts),
        )
    }
    .expect("set")[0];
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .extent(vk::Extent3D {
            width: 4,
            height: 4,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let image = unsafe { device.create_image(&image_info, None) }.expect("image");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let properties = context.memory_properties();
    let memory_type = (0..properties.memory_type_count)
        .find(|index| requirements.memory_type_bits & (1 << index) != 0)
        .expect("a memory type");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )
    }
    .expect("memory");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe { device.bind_image_memory(image, memory, 0) }.expect("bind");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let view = unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                ),
            None,
        )
    }
    .expect("view");
    let write = |set| {
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let writes = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&info)];
        // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
    };
    write(set); // the first write is fine: no command buffer binds the set yet
    // A no-op compute shader (`void main() {}`), so the pending command buffer really uses the bound set.
    // `glslc` output for: a compute shader that fetches one texel of the set's sampled image.
    let words: Vec<u32> = vec![
        0x0723_0203,
        0x0001_0000,
        0x000d_000b,
        0x0000_0021,
        0x0000_0000,
        0x0002_0011,
        0x0000_0001,
        0x0006_000b,
        0x0000_0001,
        0x4c53_4c47,
        0x6474_732e,
        0x3035_342e,
        0x0000_0000,
        0x0003_000e,
        0x0000_0000,
        0x0000_0001,
        0x0005_000f,
        0x0000_0005,
        0x0000_0004,
        0x6e69_616d,
        0x0000_0000,
        0x0006_0010,
        0x0000_0004,
        0x0000_0011,
        0x0000_0001,
        0x0000_0001,
        0x0000_0001,
        0x0003_0003,
        0x0000_0002,
        0x0000_01c2,
        0x000b_0004,
        0x455f_4c47,
        0x735f_5458,
        0x6c70_6d61,
        0x656c_7265,
        0x745f_7373,
        0x7574_7865,
        0x665f_6572,
        0x7463_6e75,
        0x736e_6f69,
        0x0000_0000,
        0x000a_0004,
        0x475f_4c47,
        0x4c47_4f4f,
        0x7063_5f45,
        0x7473_5f70,
        0x5f65_6c79,
        0x656e_696c,
        0x7269_645f,
        0x6974_6365,
        0x0000_6576,
        0x0008_0004,
        0x475f_4c47,
        0x4c47_4f4f,
        0x6e69_5f45,
        0x6475_6c63,
        0x6964_5f65,
        0x7463_6572,
        0x0065_7669,
        0x0004_0005,
        0x0000_0004,
        0x6e69_616d,
        0x0000_0000,
        0x0004_0005,
        0x0000_0009,
        0x756c_6176,
        0x0000_0065,
        0x0003_0005,
        0x0000_000c,
        0x0067_6d69,
        0x0004_0047,
        0x0000_000c,
        0x0000_0021,
        0x0000_0000,
        0x0004_0047,
        0x0000_000c,
        0x0000_0022,
        0x0000_0000,
        0x0004_0047,
        0x0000_0020,
        0x0000_000b,
        0x0000_0019,
        0x0002_0013,
        0x0000_0002,
        0x0003_0021,
        0x0000_0003,
        0x0000_0002,
        0x0003_0016,
        0x0000_0006,
        0x0000_0020,
        0x0004_0017,
        0x0000_0007,
        0x0000_0006,
        0x0000_0004,
        0x0004_0020,
        0x0000_0008,
        0x0000_0007,
        0x0000_0007,
        0x0009_0019,
        0x0000_000a,
        0x0000_0006,
        0x0000_0001,
        0x0000_0000,
        0x0000_0000,
        0x0000_0000,
        0x0000_0001,
        0x0000_0000,
        0x0004_0020,
        0x0000_000b,
        0x0000_0000,
        0x0000_000a,
        0x0004_003b,
        0x0000_000b,
        0x0000_000c,
        0x0000_0000,
        0x0004_0015,
        0x0000_000e,
        0x0000_0020,
        0x0000_0001,
        0x0004_0017,
        0x0000_000f,
        0x0000_000e,
        0x0000_0002,
        0x0004_002b,
        0x0000_000e,
        0x0000_0010,
        0x0000_0000,
        0x0005_002c,
        0x0000_000f,
        0x0000_0011,
        0x0000_0010,
        0x0000_0010,
        0x0004_0015,
        0x0000_0013,
        0x0000_0020,
        0x0000_0000,
        0x0004_002b,
        0x0000_0013,
        0x0000_0014,
        0x0000_0000,
        0x0004_0020,
        0x0000_0015,
        0x0000_0007,
        0x0000_0006,
        0x0004_002b,
        0x0000_0006,
        0x0000_0018,
        0x4080_0000,
        0x0002_0014,
        0x0000_0019,
        0x0004_002b,
        0x0000_0013,
        0x0000_001d,
        0x0000_0001,
        0x0004_002b,
        0x0000_0013,
        0x0000_001e,
        0x0000_0d48,
        0x0004_0017,
        0x0000_001f,
        0x0000_0013,
        0x0000_0003,
        0x0006_002c,
        0x0000_001f,
        0x0000_0020,
        0x0000_001d,
        0x0000_001d,
        0x0000_001d,
        0x0005_0036,
        0x0000_0002,
        0x0000_0004,
        0x0000_0000,
        0x0000_0003,
        0x0002_00f8,
        0x0000_0005,
        0x0004_003b,
        0x0000_0008,
        0x0000_0009,
        0x0000_0007,
        0x0004_003d,
        0x0000_000a,
        0x0000_000d,
        0x0000_000c,
        0x0007_005f,
        0x0000_0007,
        0x0000_0012,
        0x0000_000d,
        0x0000_0011,
        0x0000_0002,
        0x0000_0010,
        0x0003_003e,
        0x0000_0009,
        0x0000_0012,
        0x0005_0041,
        0x0000_0015,
        0x0000_0016,
        0x0000_0009,
        0x0000_0014,
        0x0004_003d,
        0x0000_0006,
        0x0000_0017,
        0x0000_0016,
        0x0005_00ba,
        0x0000_0019,
        0x0000_001a,
        0x0000_0017,
        0x0000_0018,
        0x0003_00f7,
        0x0000_001c,
        0x0000_0000,
        0x0004_00fa,
        0x0000_001a,
        0x0000_001b,
        0x0000_001c,
        0x0002_00f8,
        0x0000_001b,
        0x0003_00e1,
        0x0000_001d,
        0x0000_001e,
        0x0002_00f9,
        0x0000_001c,
        0x0002_00f8,
        0x0000_001c,
        0x0001_00fd,
        0x0001_0038,
    ];
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let module = unsafe {
        device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
    }
    .expect("shader module");
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(c"main");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let pipeline = unsafe {
        device.create_compute_pipelines(
            vk::PipelineCache::null(),
            &[vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(pipeline_layout)],
            None,
        )
    }
    .expect("compute pipeline")[0];
    let event =
        // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
        unsafe { device.create_event(&vk::EventCreateInfo::default(), None) }.expect("event");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let command_pool = unsafe {
        device.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(context.family()),
            None,
        )
    }
    .expect("command pool");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let cmd = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .command_buffer_count(1),
        )
    }
    .expect("cmd")[0];
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe {
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .expect("begin");
        device.cmd_wait_events(
            cmd,
            &[event],
            vk::PipelineStageFlags::HOST,
            // The descriptor use is the compute dispatch, so the compute stage is what the event holds.
            vk::PipelineStageFlags::COMPUTE_SHADER,
            &[],
            &[],
            &[],
        );
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                )],
        );
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::COMPUTE,
            pipeline_layout,
            0,
            &[set],
            &[],
        );
        device.cmd_dispatch(cmd, 1, 1, 1);
        device.end_command_buffer(cmd).expect("end");
    }
    let fence =
        // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
        unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.expect("fence");
    kit.queue.submit_commands(cmd, fence).expect("submit");
    // The witness that the command buffer is pending when the set is written: its fence has not signalled and
    // the event it waits on is still unset (the dispatch that reads the set cannot have run).
    // SAFETY: the fence and the event belong to this device.
    let fence_ready = unsafe { device.get_fence_status(fence) }.expect("fence status");
    // SAFETY: as above.
    let event_set = unsafe { device.get_event_status(event) }.expect("event status");
    assert!(
        !fence_ready && !event_set,
        "the command buffer is pending at the write"
    );
    let before = kit.log.messages().len();
    write(set); // the control: the set is bound by a pending command buffer

    let messages = kit.log.messages();
    println!("negative control validation log: {messages:#?}");
    assert!(
        messages.len() > before,
        "core validation reports a write to a set a pending command buffer binds"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("VUID-vkUpdateDescriptorSets")),
        "the report names the descriptor set in use: {messages:?}"
    );
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe {
        device.set_event(event).expect("set");
        device
            .wait_for_fences(&[fence], true, 5_000_000_000)
            .expect("the frame finishes");
        device.destroy_fence(fence, None);
        device.destroy_command_pool(command_pool, None);
        device.destroy_event(event, None);
        device.destroy_pipeline(pipeline, None);
        device.destroy_shader_module(module, None);
        device.destroy_image_view(view, None);
        device.destroy_image(image, None);
        device.free_memory(memory, None);
        device.destroy_descriptor_pool(pool, None);
        device.destroy_pipeline_layout(pipeline_layout, None);
        device.destroy_descriptor_set_layout(set_layout, None);
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
    super::super::destroy(instance);
}

/// The resident set size of this process in KiB (`ps`), for the footprint figure.
fn resident_kib() -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("a number of KiB")
}

/// The defaults at the milestone's scale: 65,536 textures and 1,024 extra revisions make
/// `ordinary_sets = 66,561`. The pool is created at that size (a refusal would be a start failure naming the
/// counts), and the footprint is reported, not gated: the pool's memory is driver-private, so the figure is the
/// configured counts and the observed process memory.
#[test]
fn the_default_scale_descriptor_pool_is_created_and_its_footprint_reported() {
    let kit = Kit::new(false);
    let before = resident_kib();
    let started = Instant::now();
    let ledger = TextureLedger::<u32>::start(
        kit.context.clone(),
        TextureLimits::default(),
        LedgerLimits::default(),
        65_536,
        TextureWakes::default(),
    )
    .expect("the pool of 66,561 revision sets is created");
    let after = resident_kib();
    println!(
        "configured: ordinary_sets 66561 (65536 textures + 1024 extra revisions + 1), sampler sets 2, texture_bytes {}, staging_bytes {}; \
         ledger start {:?}, process memory {before} KiB -> {after} KiB (+{} KiB)",
        TextureLimits::default().texture_bytes,
        TextureLimits::default().staging_bytes,
        started.elapsed(),
        after.saturating_sub(before),
    );
    assert_eq!(
        ledger.shutdown(Instant::now() + PATIENCE),
        LedgerCleanup::Proven
    );
    drop(ledger);
    kit.finish();
}

/// A panic payload whose `Drop` panics, counting itself.
struct Bomb(&'static std::sync::atomic::AtomicUsize);

impl Drop for Bomb {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("a payload whose drop panics");
    }
}

static ROOM_BOMBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The ledger's room hook has the store's containment contract: a hook that panics with a payload whose `Drop`
/// panics too neither unwinds out of the ledger call that announced the room nor changes its state. Here `adopt`
/// turns an extra revision into the baseline, which opens room and calls the hook.
///
/// Negative control: with the earlier `let _ = catch_unwind(..)` the payload was dropped implicitly, its `Drop`
/// panicked, and the panic escaped `adopt` (the call below would unwind and the case would fail).
#[test]
fn a_room_hook_whose_panic_payload_drop_panics_is_contained() {
    use std::sync::atomic::Ordering;
    let wakes = TextureWakes {
        room: Some(Arc::new(|| std::panic::panic_any(Bomb(&ROOM_BOMBS)))),
        ..TextureWakes::default()
    };
    let mut rig = Rig::with_wakes(8, 4, TextureTestHooks::default(), wakes);
    rig.show_new(1, 1);
    rig.offer(1, 2, 8, 8);
    rig.prepared(1, 2);
    rig.ledger().commit(1, 2).expect("commit");
    let before = ROOM_BOMBS.load(Ordering::SeqCst);
    let mut adopted = Vec::new();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.ledger().adopt(8, &mut adopted);
    }));
    assert!(outcome.is_ok(), "the payload's Drop panic escaped `adopt`");
    assert_eq!(adopted, vec![(1, 2)], "the adoption itself happened");
    assert_eq!(
        ROOM_BOMBS.load(Ordering::SeqCst),
        before + 1,
        "the hook ran once and its payload was disposed of once"
    );
    let frame = rig.frame(&[1], None).expect("the ledger still works");
    rig.until_proven(frame.submission);
    assert!(rig.read(&frame, 0) == rig.source(1, 2));
    rig.finish();
}
