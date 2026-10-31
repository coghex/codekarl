//! The store: off-screen pixels, batches and leases, limits as behaviour, and store-wide failures.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::too_many_lines,
    reason = "the raw control records Vulkan commands on the case's own objects, each destroyed at the end of the case"
)]

use std::collections::HashMap;
use std::time::Instant;

use ck_gpu_vulkan::{
    Completed, StoreCleanup, TextureLimits, TextureStore, TextureTestHooks, TextureWakes,
    UploadItem, readback_image, vk,
};

use super::{Bytes, Kit, PATIENCE, pattern};

/// Offers one pattern image under `tag`.
fn offer(
    port: &ck_gpu_vulkan::UploadPort<u32>,
    tag: u32,
    width: u32,
    height: u32,
) -> std::sync::Arc<Bytes> {
    let pixels = pattern(width, height, u8::try_from(tag % 251).expect("a small tag"));
    let item = UploadItem {
        tag,
        width,
        height,
        pixels: pixels.clone(),
    };
    assert!(
        port.offer(item).is_ok(),
        "the prepared window has room for tag {tag}"
    );
    pixels
}

/// Reads every completed texture back from its production image and compares it with its source.
fn check_pixels(
    kit: &mut Kit,
    done: Vec<Completed<u32>>,
    sources: &HashMap<u32, std::sync::Arc<Bytes>>,
) -> usize {
    let mut checked = 0;
    for completed in done {
        match completed {
            Completed::Ready(texture) => {
                let read = texture
                    .readback(&mut kit.queue)
                    .expect("the readback works");
                let source = &sources[&texture.tag];
                assert_eq!(
                    read.len(),
                    source.0.len(),
                    "tag {} reads back {}x{}",
                    texture.tag,
                    texture.width,
                    texture.height
                );
                assert!(
                    read == source.0,
                    "tag {} ({}x{}) does not read back exactly",
                    texture.tag,
                    texture.width,
                    texture.height
                );
                // A second readback finds the sampled layout the first restored.
                let again = texture
                    .readback(&mut kit.queue)
                    .expect("a second readback works");
                assert!(
                    again == read,
                    "the second readback of tag {} differs",
                    texture.tag
                );
                checked += 1;
            }
            Completed::Failed { tag, reason } => panic!("tag {tag} failed: {reason}"),
        }
    }
    checked
}

fn end(mut owner: ck_gpu_vulkan::TextureOwner<u32>) {
    assert_eq!(
        owner.shutdown(Instant::now() + PATIENCE),
        StoreCleanup::Proven,
        "the end is proven"
    );
}

/// Test 2: Off-screen pixels: 1×1, 33×17 and 256×256 images in distinct colours come back exactly, read from the
/// production image (which the readback restores to the sampled layout, so a second one finds it).
///
/// Negative control: `negative_control_an_upload_without_its_final_barrier_is_reported`.
#[test]
fn images_of_different_sizes_read_back_exactly() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let mut sources = HashMap::new();
    for (tag, (width, height)) in [(1_u32, 1_u32), (33, 17), (256, 256)]
        .into_iter()
        .enumerate()
    {
        let tag = u32::try_from(tag).expect("a small tag");
        sources.insert(tag, offer(&port, tag, width, height));
    }
    let done = kit.drive(&mut owner, 3);
    assert_eq!(check_pixels(&mut kit, done, &sources), 3);
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 2: An image over the 16 MiB batch target (2,100 × 2,100) is uploaded alone.
#[test]
fn an_image_over_the_batch_target_is_uploaded_alone() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let mut sources = HashMap::new();
    sources.insert(0, offer(&port, 0, 2100, 2100));
    let done = kit.drive(&mut owner, 1);
    assert_eq!(check_pixels(&mut kit, done, &sources), 1);
    assert_eq!(
        owner.stats().batches_submitted,
        1,
        "the image took a batch of its own"
    );
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 2: A batch of 128 mixed images, each correct.
#[test]
fn a_batch_of_128_mixed_images_reads_back_exactly() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let mut sources = HashMap::new();
    for tag in 0..128_u32 {
        let width = 1 + (tag * 7) % 61;
        let height = 1 + (tag * 13) % 47;
        sources.insert(tag, offer(&port, tag, width, height));
    }
    let done = kit.drive(&mut owner, 128);
    assert_eq!(check_pixels(&mut kit, done, &sources), 128);
    drop(port);
    end(owner);
    kit.finish();
}

/// Negative control for the pixel checks: an upload that omits its final barrier leaves the image in
/// `TRANSFER_DST_OPTIMAL`; the readback's first barrier names `SHADER_READ_ONLY_OPTIMAL` as the old layout, so
/// validation reports the layout mismatch at submit. The log is kept as such: this case asserts that it is
/// **not** empty, in an instance of its own.
#[test]
fn negative_control_an_upload_without_its_final_barrier_is_reported() {
    let mut kit = Kit::new(false);
    let context = kit.context.clone();
    let device = context.device();
    let (width, height) = (4_u32, 4_u32);
    let bytes = u64::from(width) * u64::from(height) * 4;
    let memory_properties = context.memory_properties();
    let find = |bits: u32, wanted: vk::MemoryPropertyFlags| {
        (0..memory_properties.memory_type_count)
            .find(|index| {
                bits & (1 << index) != 0
                    && memory_properties.memory_types[*index as usize]
                        .property_flags
                        .contains(wanted)
            })
            .expect("a memory type")
    };
    // SAFETY (all raw calls below): the case's own objects on its own device, destroyed at the end.
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(
            vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
        )
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let control_image = unsafe { device.create_image(&image_info, None) }.expect("image");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let requirements = unsafe { device.get_image_memory_requirements(control_image) };
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let image_memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(find(
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                )),
            None,
        )
    }
    .expect("image memory");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe { device.bind_image_memory(control_image, image_memory, 0) }.expect("bind");
    let staging_info = vk::BufferCreateInfo::default()
        .size(bytes)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC);
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let staging = unsafe { device.create_buffer(&staging_info, None) }.expect("staging");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let staging_requirements = unsafe { device.get_buffer_memory_requirements(staging) };
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let staging_memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(staging_requirements.size)
                .memory_type_index(find(
                    staging_requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )),
            None,
        )
    }
    .expect("staging memory");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe { device.bind_buffer_memory(staging, staging_memory, 0) }.expect("bind staging");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let pool = unsafe {
        device.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(context.family()),
            None,
        )
    }
    .expect("pool");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    let cmd = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .command_buffer_count(1),
        )
    }
    .expect("cmd")[0];
    let fence =
        // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
        unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.expect("fence");
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe {
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .expect("begin");
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(control_image)
                .subresource_range(range)],
        );
        device.cmd_copy_buffer_to_image(
            cmd,
            staging,
            control_image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
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
        // The final barrier to SHADER_READ_ONLY_OPTIMAL is omitted on purpose.
        device.end_command_buffer(cmd).expect("end");
    }
    kit.queue.submit_commands(cmd, fence).expect("submit");
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe { device.wait_for_fences(&[fence], true, 5_000_000_000) }.expect("the upload finishes");
    let before = kit.log.messages().len();
    // The readback assumes the store's contract (the sampled layout) and so names the wrong old layout.
    let outcome = readback_image(&context, &mut kit.queue, control_image, width, height);
    let messages = kit.log.messages();
    println!("negative control validation log: {messages:#?}");
    assert!(outcome.is_ok() || outcome.is_err(), "the call returns");
    assert!(
        messages.len() > before,
        "validation reports the missing final barrier (readback outcome {outcome:?})"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.to_lowercase().contains("layout")),
        "the report names the layout mismatch: {messages:?}"
    );
    // SAFETY: the case's own objects on its own device, destroyed at the end of the case.
    unsafe {
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        device.free_memory(staging_memory, None);
        device.destroy_image(control_image, None);
        device.free_memory(image_memory, None);
    }
    // The log is kept as the negative control's: it is not empty, so `finish` is not used.
    let Kit {
        instance,
        queue,
        context: kit_context,
        ..
    } = kit;
    drop(queue);
    drop(kit_context);
    drop(context);
    // The context was the last holder, so the instance can go.
    super::super::destroy(instance);
}

/// Test 3: Batches and leases: 2,048 distinct 32×32 images go through bounded batches on two command pools for the
/// whole run (every submitted command buffer is one of the two leases' own). The worker never submits: it
/// holds no `RenderQueue` (the type is neither `Sync` nor `Clone`, and only `TextureOwner::submit` takes one),
/// a compile-time fact, not a runtime one. Throughput is measured and reported, not gated.
#[test]
fn two_thousand_images_run_on_two_command_pools() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let leases = owner.lease_objects();
    assert_eq!(leases.len(), 2, "two leases, so two command pools in all");
    assert_ne!(leases[0].0, leases[1].0);
    let mut sources = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut textures = Vec::new();
    let mut offered = 0_u32;
    let started = Instant::now();
    while textures.len() < 2048 {
        while offered < 2048 && port.room() > 0 {
            sources.insert(offered, offer(&port, offered, 32, 32));
            offered += 1;
        }
        if owner.submit(&mut kit.queue) {
            seen.insert(owner.submitted_command_buffer().expect("a batch in flight"));
        }
        let mut out = Vec::new();
        owner.poll(&mut out);
        for completed in out {
            match completed {
                Completed::Ready(texture) => textures.push(texture),
                Completed::Failed { tag, reason } => panic!("tag {tag} failed: {reason}"),
            }
        }
        assert!(
            started.elapsed() < PATIENCE,
            "{} of 2048 uploaded",
            textures.len()
        );
    }
    let elapsed = started.elapsed();
    let stats = owner.stats();
    println!(
        "allocations in use / limit: {:?}; texture_bytes_peak {}",
        kit.context.allocation_count(),
        stats.texture_bytes_peak
    );
    println!(
        "uploaded 2048 32x32 images in {elapsed:?}: {:.0} images/s, {:.1} MiB/s, {} batches",
        2048.0 / elapsed.as_secs_f64(),
        2048.0 * 4096.0 / 1_048_576.0 / elapsed.as_secs_f64(),
        stats.batches_submitted,
    );
    assert!(seen.len() <= 2, "{} distinct command buffers", seen.len());
    assert!(
        seen.iter()
            .all(|buffer| leases.iter().any(|lease| lease.1 == *buffer))
    );
    assert!(
        stats.batches_submitted >= 16,
        "128 images at most per batch: {} batches",
        stats.batches_submitted
    );
    assert_eq!(stats.batches_submitted, stats.batches_completed);
    assert_eq!(stats.textures_created, 2048);
    // Every image is distinct and correct (all of them, from the production images).
    for texture in &textures {
        let read = texture
            .readback(&mut kit.queue)
            .expect("the readback works");
        assert!(
            read == sources[&texture.tag].0,
            "tag {} reads back wrongly",
            texture.tag
        );
    }
    drop(textures);
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 4: A small `texture_bytes` fails the overflowing item with a texture-bytes reason while the rest are
/// `Ready`. Negative control: without the ceiling the large image would be `Ready` too (the third assertion
/// is the one that would fail).
#[test]
fn a_small_texture_budget_fails_the_overflowing_item_alone() {
    let mut kit = Kit::new(false);
    let limits = TextureLimits {
        texture_bytes: 64 << 20,
        ..TextureLimits::default()
    };
    let (port, mut owner) =
        TextureStore::start::<u32>(kit.context.clone(), limits, TextureWakes::default())
            .expect("the store starts");
    let mut sources = HashMap::new();
    for (tag, (width, height)) in [
        (0_u32, (16_u32, 16_u32)),
        (1, (16, 16)),
        (2, (2900, 2900)),
        (3, (16, 16)),
    ] {
        sources.insert(tag, offer(&port, tag, width, height));
    }
    let done = kit.drive(&mut owner, 4);
    let mut ready = Vec::new();
    for completed in done {
        match completed {
            Completed::Ready(texture) => ready.push(texture),
            Completed::Failed { tag, reason } => {
                assert_eq!(tag, 2, "only the large image overflows: {reason}");
                assert!(reason.contains("texture_bytes"), "{reason}");
            }
        }
    }
    let mut tags: Vec<u32> = ready.iter().map(|texture| texture.tag).collect();
    tags.sort_unstable();
    assert_eq!(tags, vec![0, 1, 3], "the rest are Ready");
    for texture in &ready {
        let read = texture
            .readback(&mut kit.queue)
            .expect("the readback works");
        assert!(read == sources[&texture.tag].0);
    }
    assert!(
        owner.stats().texture_bytes_peak <= 64 << 20,
        "the ceiling holds"
    );
    drop(ready);
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 4: A lowered allocation-count limit fails an item with the count reason, naming in-use and limit; the next
/// image is placed in the block that exists.
#[test]
fn a_lowered_allocation_count_limit_fails_an_item_with_the_count_reason() {
    let mut kit = Kit::new(false);
    let hooks = TextureTestHooks {
        allocation_count_limit: Some(2),
        ..TextureTestHooks::default()
    };
    let (port, mut owner) = TextureStore::start_with_test_hooks::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
        hooks,
    )
    .expect("the store starts");
    let mut sources = HashMap::new();
    sources.insert(0, offer(&port, 0, 16, 16));
    let mut ready = kit.drive(&mut owner, 1);
    // Staging for lease one and the first block are the two allocations; the large image needs a third (its
    // staging buffer is replaced by a bigger one, which is not a net allocation, then the dedicated block).
    sources.insert(1, offer(&port, 1, 2900, 2900));
    ready.extend(kit.drive(&mut owner, 1));
    sources.insert(2, offer(&port, 2, 16, 16));
    ready.extend(kit.drive(&mut owner, 1));
    let mut good = Vec::new();
    for completed in ready {
        match completed {
            Completed::Ready(texture) => good.push(texture),
            Completed::Failed { tag, reason } => {
                assert_eq!(
                    tag, 1,
                    "only the image that needed a third allocation fails: {reason}"
                );
                assert!(reason.contains("allocation count"), "{reason}");
                assert!(
                    reason.contains("2 of 2"),
                    "names in-use and limit: {reason}"
                );
            }
        }
    }
    let mut tags: Vec<u32> = good.iter().map(|texture| texture.tag).collect();
    tags.sort_unstable();
    assert_eq!(tags, vec![0, 2]);
    assert_eq!(kit.context.allocation_count(), (2, 2), "nothing leaked");
    drop(good);
    drop(port);
    end(owner);
    assert_eq!(
        kit.context.allocation_count().0,
        0,
        "the end freed every allocation"
    );
    kit.finish();
}

/// Test 4: An oversized image that waits while the other lease is submitted completes once that lease returns, with
/// no deadlock: lease A is in flight (its fence signalled, not yet polled), lease B holds a closed envelope, so
/// the worker has no lease for the oversized image until `poll` returns A and `submit` takes B.
#[test]
fn an_oversized_image_waits_for_a_lease_and_completes_without_deadlock() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let mut sources = HashMap::new();
    let wait_for = |owner: &ck_gpu_vulkan::TextureOwner<u32>| {
        let started = Instant::now();
        while !owner.envelope_ready_unsubmitted() {
            assert!(started.elapsed() < PATIENCE, "a batch was never closed");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    for tag in 0..4 {
        sources.insert(tag, offer(&port, tag, 20, 20));
    }
    wait_for(&owner);
    assert!(owner.submit(&mut kit.queue), "batch A is submitted");
    for tag in 4..6 {
        sources.insert(tag, offer(&port, tag, 20, 20));
    }
    wait_for(&owner);
    assert!(
        !owner.submit(&mut kit.queue),
        "batch B waits while A is in flight (one batch at a time)"
    );
    sources.insert(6, offer(&port, 6, 2100, 2100));
    assert!(
        owner.test_wait_submitted(PATIENCE),
        "the GPU finished batch A, which the owner has not polled yet"
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        owner.envelope_ready_unsubmitted(),
        "B is still closed and A still holds its lease"
    );
    assert!(
        !owner.submit(&mut kit.queue),
        "A has not been polled, so B cannot go"
    );
    let done = kit.drive(&mut owner, 7);
    assert_eq!(check_pixels(&mut kit, done, &sources), 7);
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 5: An injected allocation failure, after the budget admitted the allocation, latches once: the batch's
/// items fail, offers are refused, and the end is proven.
#[test]
fn an_injected_allocation_failure_latches_once() {
    let mut kit = Kit::new(false);
    let hooks = TextureTestHooks {
        fail_allocation_at: Some(2),
        ..TextureTestHooks::default()
    };
    // A batch target of 64 MiB lets the large image join the small one's batch.
    let limits = TextureLimits {
        max_upload_bytes_per_batch: 64 << 20,
        ..TextureLimits::default()
    };
    let (port, mut owner) = TextureStore::start_with_test_hooks::<u32>(
        kit.context.clone(),
        limits,
        TextureWakes::default(),
        hooks,
    )
    .expect("the store starts");
    // Batch A (tag 9, small) takes the first allocation, a block, and is held closed so the next two images
    // wait together. Tag 0 (small) joins tag 1 (over half a block, so a dedicated allocation, the second one,
    // which is the injected failure) in one batch.
    offer(&port, 9, 16, 16);
    while !owner.envelope_ready_unsubmitted() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    offer(&port, 0, 16, 16);
    offer(&port, 1, 3200, 3200);
    let started = Instant::now();
    let mut done = Vec::new();
    while done.len() < 3 {
        owner.submit(&mut kit.queue);
        owner.poll(&mut done);
        assert!(started.elapsed() < PATIENCE, "{} of 3 outcomes", done.len());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let mut failed = Vec::new();
    for completed in &done {
        match completed {
            Completed::Failed { tag, reason } => {
                assert!(
                    reason.contains("injected allocation failure"),
                    "the whole batch fails with the latched reason: {reason}"
                );
                failed.push(*tag);
            }
            Completed::Ready(texture) => {
                assert_eq!(texture.tag, 9, "only the batch before the failure is Ready");
            }
        }
    }
    failed.sort_unstable();
    assert_eq!(failed, vec![0, 1]);
    let latched = owner.take_failure().expect("a failure was latched");
    assert!(latched.contains("injected allocation failure"));
    assert_eq!(owner.take_failure(), None, "it is reported once");
    assert!(
        port.offer(UploadItem {
            tag: 9,
            width: 1,
            height: 1,
            pixels: pattern(1, 1, 9),
        })
        .is_err(),
        "offers are refused"
    );
    assert_eq!(
        owner.held_images(),
        1,
        "the image made before the failure is kept for the end"
    );
    drop(done);
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 5: An injected submit failure (out of memory) is a never-submitted envelope: its items fail, offers are
/// refused and the end is proven, with the batch's images destroyed by the release.
#[test]
fn an_injected_out_of_memory_submit_is_never_submitted_and_the_end_is_proven() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    offer(&port, 0, 8, 8);
    let started = Instant::now();
    while !owner.envelope_ready_unsubmitted() {
        assert!(started.elapsed() < PATIENCE, "a batch was never closed");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    kit.queue
        .inject_submit_error(Some(vk::Result::ERROR_OUT_OF_HOST_MEMORY));
    assert!(!owner.submit(&mut kit.queue), "the submit failed");
    let mut done = Vec::new();
    owner.poll(&mut done);
    assert_eq!(done.len(), 1);
    for completed in &done {
        match completed {
            Completed::Failed { reason, .. } => {
                assert!(reason.contains("vkQueueSubmit"), "{reason}");
            }
            Completed::Ready(texture) => panic!("tag {} is Ready", texture.tag),
        }
    }
    assert!(owner.take_failure().is_some());
    assert_eq!(
        owner.held_images(),
        1,
        "the never-submitted batch is kept for the end"
    );
    assert_eq!(owner.stats().batches_submitted, 0);
    drop(done);
    drop(port);
    end(owner);
    kit.finish();
}

/// Test 5: A submit that loses the device is uncertain: the end is `Unproven` and every object is retained,
/// including the context (so the instance is kept for the rest of the process).
#[test]
fn a_submit_that_loses_the_device_leaves_the_end_unproven() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    offer(&port, 0, 8, 8);
    let started = Instant::now();
    while !owner.envelope_ready_unsubmitted() {
        assert!(started.elapsed() < PATIENCE, "a batch was never closed");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    kit.queue
        .inject_submit_error(Some(vk::Result::ERROR_DEVICE_LOST));
    assert!(!owner.submit(&mut kit.queue));
    let outcome = owner.shutdown(Instant::now() + std::time::Duration::from_millis(200));
    assert!(
        matches!(outcome, StoreCleanup::Unproven { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        owner.held_images(),
        1,
        "the image is retained, not destroyed"
    );
    // The log is clean so far: nothing was destroyed under a possible GPU use.
    super::super::assert_log_clean(&kit.log);
    // Dropping the owner retains the graph and the context; the instance is kept for the process.
    drop(port);
    drop(owner);
    let Kit {
        instance,
        loader,
        queue,
        context,
        ..
    } = kit;
    drop(queue);
    drop(context);
    std::mem::forget(instance);
    std::mem::forget(loader);
}

/// Test 8: A worker held by the stall gate gives `Unproven` (everything is retained); once the gate opens the same
/// owner proves the end, the held item having failed as "store stopping".
#[test]
fn a_worker_held_by_the_stall_gate_gives_unproven_and_then_proven() {
    let kit = Kit::new(false);
    let gate: ck_gpu_vulkan::StallGate =
        std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let reached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hooks = TextureTestHooks {
        stall_gate: Some(gate.clone()),
        stall_reached: Some(reached.clone()),
        ..TextureTestHooks::default()
    };
    let (port, mut owner) = TextureStore::start_with_test_hooks::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
        hooks,
    )
    .expect("the store starts");
    offer(&port, 0, 8, 8);
    // The worker acknowledges that it took the item and reached the gate, so the stop below drains nothing waiting.
    super::wait_for_gate(&reached);
    let proof = owner.prove(Instant::now() + std::time::Duration::from_millis(200));
    assert!(!proof.is_complete(), "the worker has not finished");
    {
        let (flag, changed) = &*gate;
        *flag.lock().expect("gate") = true;
        changed.notify_all();
    }
    let proof = owner.prove(Instant::now() + PATIENCE);
    assert!(
        proof.is_complete(),
        "the worker finished once the gate opened"
    );
    let mut out = Vec::new();
    owner.poll(&mut out);
    assert_eq!(out.len(), 1);
    match &out[0] {
        Completed::Failed { reason, .. } => assert!(reason.contains("store stopping"), "{reason}"),
        Completed::Ready(texture) => panic!("tag {} is Ready", texture.tag),
    }
    owner.release(proof);
    drop(out);
    drop(port);
    drop(owner);
    kit.finish();
}

/// Test 8: CK-154: a room hook that calls `UploadPort::room()` while the worker ends a batch because the store is
/// stopping does not deadlock (hooks run after the lock is released), and the end is proven.
#[test]
fn a_room_hook_that_calls_room_during_a_stop_does_not_deadlock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let kit = Kit::new(false);
    let gate: ck_gpu_vulkan::StallGate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hooks = TextureTestHooks {
        stall_gate: Some(gate.clone()),
        stall_reached: Some(reached.clone()),
        ..TextureTestHooks::default()
    };
    let slot: Arc<Mutex<Option<ck_gpu_vulkan::UploadPort<u32>>>> = Arc::new(Mutex::new(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let rooms = Arc::new(Mutex::new(Vec::new()));
    let hook = {
        let (slot, calls, rooms) = (slot.clone(), calls.clone(), rooms.clone());
        Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            if let Some(port) = slot.lock().expect("slot").as_ref() {
                rooms.lock().expect("rooms").push(port.room());
            }
        }) as Arc<dyn Fn() + Send + Sync>
    };
    let wakes = TextureWakes {
        room: Some(hook),
        ..TextureWakes::default()
    };
    let (port, mut owner) = TextureStore::start_with_test_hooks::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        wakes,
        hooks,
    )
    .expect("the store starts");
    *slot.lock().expect("slot") = Some(port.clone());
    offer(&port, 0, 8, 8);
    super::wait_for_gate(&reached);
    assert!(
        !owner
            .prove(Instant::now() + std::time::Duration::from_millis(100))
            .is_complete(),
        "the held worker keeps the end unproven"
    );
    {
        let (flag, changed) = &*gate;
        *flag.lock().expect("gate") = true;
        changed.notify_all();
    }
    let proof = owner.prove(Instant::now() + PATIENCE);
    assert!(
        proof.is_complete(),
        "no deadlock: the worker finished and the end is proven"
    );
    assert!(calls.load(Ordering::SeqCst) >= 1, "the room hook ran");
    assert!(
        !rooms.lock().expect("rooms").is_empty(),
        "and called room() without deadlock"
    );
    owner.release(proof);
    *slot.lock().expect("slot") = None;
    drop(port);
    drop(owner);
    kit.finish();
}

/// Uploads `count` images of `width` x `height` (one shared source) through a fresh store on `context`, timing
/// the run from the first offer to the last completion. The end is proven.
fn upload_run(
    context: &std::sync::Arc<ck_gpu_vulkan::VulkanContext>,
    queue: &mut ck_gpu_vulkan::RenderQueue,
    count: u32,
    width: u32,
    height: u32,
) -> (std::time::Duration, ck_gpu_vulkan::TextureStats) {
    let (port, mut owner) = TextureStore::start::<u32>(
        context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let pixels = pattern(width, height, 3);
    let mut textures = Vec::new();
    let mut offered = 0_u32;
    let started = Instant::now();
    while textures.len() < count as usize {
        while offered < count && port.room() > 0 {
            let item = UploadItem {
                tag: offered,
                width,
                height,
                pixels: pixels.clone(),
            };
            assert!(port.offer(item).is_ok());
            offered += 1;
        }
        owner.submit(queue);
        let mut out = Vec::new();
        owner.poll(&mut out);
        for completed in out {
            match completed {
                Completed::Ready(texture) => textures.push(texture),
                Completed::Failed { tag, reason } => panic!("tag {tag} failed: {reason}"),
            }
        }
        assert!(
            started.elapsed() < PATIENCE,
            "{} of {count} uploaded",
            textures.len()
        );
    }
    let elapsed = started.elapsed();
    let stats = owner.stats();
    drop(textures);
    drop(port);
    end(owner);
    (elapsed, stats)
}

/// Throughput is measured and reported, never gated (the owner: Mac times are a sanity check, not acceptance).
/// Once under validation and synchronization validation (as every case runs), once on a plain instance, which
/// is the figure to read.
#[test]
fn upload_throughput_is_measured_and_reported() {
    let shapes = [
        (2048_u32, 32_u32, 32_u32),
        (256, 256, 256),
        (16, 1024, 1024),
    ];
    let mut kit = Kit::new(false);
    for (count, width, height) in shapes {
        let (elapsed, stats) = upload_run(&kit.context, &mut kit.queue, count, width, height);
        let mib = f64::from(count) * f64::from(width) * f64::from(height) * 4.0 / 1_048_576.0;
        println!(
            "throughput[validated+sync] {count} x {width}x{height}: {elapsed:?}, {:.0} images/s, {:.1} MiB/s, {} batches, texture peak {} B, staging peak {} B",
            f64::from(count) / elapsed.as_secs_f64(),
            mib / elapsed.as_secs_f64(),
            stats.batches_submitted,
            stats.texture_bytes_peak,
            stats.staging_bytes_peak,
        );
    }
    kit.finish();
    let loader = ck_gpu_vulkan::load_loader().expect("the Vulkan loader loads");
    let instance = ck_gpu_vulkan::create_instance(&loader, &[]).expect("a plain instance");
    let context = ck_gpu_vulkan::VulkanContext::new(&instance, None).expect("a context");
    let mut queue = context.take_queue().expect("the queue");
    for (count, width, height) in shapes {
        let (elapsed, stats) = upload_run(&context, &mut queue, count, width, height);
        let mib = f64::from(count) * f64::from(width) * f64::from(height) * 4.0 / 1_048_576.0;
        println!(
            "throughput[plain] {count} x {width}x{height}: {elapsed:?}, {:.0} images/s, {:.1} MiB/s, {} batches, texture peak {} B, staging peak {} B",
            f64::from(count) / elapsed.as_secs_f64(),
            mib / elapsed.as_secs_f64(),
            stats.batches_submitted,
            stats.texture_bytes_peak,
            stats.staging_bytes_peak,
        );
    }
    drop(queue);
    drop(context);
    // SAFETY: there is no surface (null) and nothing else holds the instance.
    let outcome = unsafe {
        ck_gpu_vulkan::destroy_surface_and_instance(instance, vk::SurfaceKHR::null(), None)
    };
    assert!(outcome.is_ok(), "the instance was destroyed");
}

/// A texture the store handed out keeps its image, its range and the pool alive after the store's release:
/// the release does not free the pooled blocks under it, and dropping the texture afterwards returns the range
/// (and frees the block) with a clean validation log.
#[test]
fn a_texture_that_outlives_the_stores_release_stays_valid_until_dropped() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let source = offer(&port, 0, 33, 17);
    let mut done = kit.drive(&mut owner, 1);
    drop(port);
    end(owner);
    let Some(Completed::Ready(texture)) = done.pop() else {
        panic!("the image is Ready");
    };
    let read = texture
        .readback(&mut kit.queue)
        .expect("the readback works");
    assert!(
        read == source.0,
        "the image still reads back after the store's release"
    );
    drop(texture);
    assert_eq!(
        kit.context.allocation_count().0,
        0,
        "the last range freed its block"
    );
    kit.finish();
}

/// A pixel source whose `pixels()` answers a wrong length on the calls in `lie` (counting from 0) and the
/// exact bytes on every other call.
struct Lying {
    bytes: Vec<u8>,
    longer: Vec<u8>,
    lie: std::ops::Range<usize>,
    calls: std::sync::atomic::AtomicUsize,
    shorter: bool,
}

impl Lying {
    fn new(bytes: Vec<u8>, lie: std::ops::Range<usize>, shorter: bool) -> Self {
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[0xEE; 4096]);
        Self {
            bytes,
            longer,
            lie,
            calls: std::sync::atomic::AtomicUsize::new(0),
            shorter,
        }
    }
}

impl ck_gpu_vulkan::PixelSource for Lying {
    fn pixels(&self) -> &[u8] {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.lie.contains(&call) {
            &self.bytes
        } else if self.shorter {
            &self.bytes[..self.bytes.len() - 1]
        } else {
            &self.longer
        }
    }
}

/// Offers `source` as tag 0, then an honest image as tag 1, and returns the two outcomes by tag. The store must
/// not be latched and its end must be proven afterwards.
fn lying_run(source: Lying) -> Completed<u32> {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let item = UploadItem {
        tag: 0,
        width: 8,
        height: 8,
        pixels: std::sync::Arc::new(source),
    };
    assert!(port.offer(item).is_ok());
    let mut first = kit.drive(&mut owner, 1);
    assert_eq!(first.len(), 1, "exactly one outcome for the item");
    let first = first.pop().expect("one outcome");
    assert_eq!(
        owner.peek_failure(),
        None,
        "a bad source never latches the store"
    );
    // A later honest upload still works.
    let honest = offer(&port, 1, 8, 8);
    let mut second = kit.drive(&mut owner, 1);
    match second.pop().expect("one outcome") {
        Completed::Ready(texture) => {
            assert_eq!(texture.tag, 1);
            assert!(
                texture
                    .readback(&mut kit.queue)
                    .expect("the readback works")
                    == honest.0
            );
        }
        Completed::Failed { tag, reason } => panic!("the honest tag {tag} failed: {reason}"),
    }
    drop(port);
    end(owner);
    if let Completed::Ready(texture) = first {
        drop(texture);
        kit.finish();
        panic!("the lying source's item was Ready");
    }
    kit.finish();
    first
}

/// A source that is wrong when the worker classifies it and right on every later call must fail its item with the
/// classification's reason, once: the worker does not ask again (it used to, and `expect_err` panicked the worker),
/// and the store is not latched. Shorter and longer. The first three calls lie: the batch formation's and the
/// classification's, and the one that builds the formation's refusal text (a refused source is asked for its
/// length again in the message); every later call is right, which is what the old second question met.
#[test]
fn a_source_that_is_wrong_when_classified_and_right_later_fails_its_item_once() {
    for shorter in [true, false] {
        let source = Lying::new(pattern(8, 8, 5).0.clone(), 0..3, shorter);
        let outcome = lying_run(source);
        match outcome {
            Completed::Failed { tag, reason } => {
                assert_eq!(tag, 0);
                assert!(
                    reason.contains("bytes, expected"),
                    "the classification's own reason: {reason}"
                );
            }
            Completed::Ready(_) => panic!("a source that failed its classification is not Ready"),
        }
    }
}

/// The final-copy witness: a source that is right for the batch formation and the classification and wrong only
/// on the call that supplies the copy's slice (the third) fails with the changed-length reason, and the store
/// writes nothing outside the staging range. Shorter and longer. (The calls before the copy are those two; if the
/// worker's call pattern ever changes this case fails and says so, instead of passing on an earlier rejection.)
#[test]
fn a_source_that_changes_length_only_at_the_copy_fails_with_the_changed_length_reason() {
    for shorter in [true, false] {
        let source = Lying::new(pattern(8, 8, 5).0.clone(), 2..3, shorter);
        let outcome = lying_run(source);
        match outcome {
            Completed::Failed { tag, reason } => {
                assert_eq!(tag, 0);
                assert!(
                    reason.contains("changed length after it was checked"),
                    "the final-copy guard is what failed it: {reason}"
                );
            }
            Completed::Ready(_) => panic!("a source that lied at the copy is not Ready"),
        }
    }
}

/// An uncertain readback keeps everything it may reference. With the submit failing as if the device were lost,
/// `readback` returns an error and the texture is dropped afterwards: its image and its range are **not**
/// destroyed (the pool's allocation stays, the device owner stays), so the log is clean. A failure that left
/// nothing in flight (out of memory) cleans up: the texture is still usable and its drop frees everything.
#[test]
fn an_uncertain_readback_retains_its_image_and_the_device_owner() {
    let mut kit = Kit::new(false);
    let (port, mut owner) = TextureStore::start::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
    )
    .expect("the store starts");
    let source = offer(&port, 0, 8, 8);
    let mut done = kit.drive(&mut owner, 1);
    drop(port);
    end(owner);
    let Some(Completed::Ready(texture)) = done.pop() else {
        panic!("the image is Ready");
    };
    // Out of memory: nothing was submitted, the readback cleans up, the texture still reads back.
    kit.queue
        .inject_submit_error(Some(vk::Result::ERROR_OUT_OF_HOST_MEMORY));
    assert!(texture.readback(&mut kit.queue).is_err());
    assert!(texture.readback(&mut kit.queue).expect("usable again") == source.0);
    assert_eq!(
        kit.context.allocation_count().0,
        1,
        "only the image's block is allocated: the failed readback freed its buffer"
    );
    // Device loss: the outcome is uncertain, so the readback's objects, the image and the owner are retained.
    kit.queue
        .inject_submit_error(Some(vk::Result::ERROR_DEVICE_LOST));
    assert!(texture.readback(&mut kit.queue).is_err());
    let holders_before_drop = std::sync::Arc::strong_count(&kit.context);
    drop(texture);
    assert_eq!(
        kit.context.allocation_count().0,
        2,
        "the image's block stays allocated (its range was not returned), and so does the readback's buffer"
    );
    assert!(
        std::sync::Arc::strong_count(&kit.context) >= holders_before_drop - 1
            && std::sync::Arc::strong_count(&kit.context) > 1,
        "the device owner is retained after the texture is dropped"
    );
    super::super::assert_log_clean(&kit.log);
    // The graph is retained for the process: the instance is kept too.
    let Kit {
        instance,
        loader,
        queue,
        ..
    } = kit;
    drop(queue);
    std::mem::forget(instance);
    std::mem::forget(loader);
}

/// Test 4: The allocation count is a limit, not a failure of Vulkan: with the count lowered to two, lease A in
/// flight holds its staging buffer and the image block, and the batch on the second lease cannot allocate its own
/// staging buffer. That batch's item fails alone with the count reason; the store is not latched, offers are
/// still admitted, and lease A's batch completes.
#[test]
fn a_staging_buffer_over_the_count_limit_fails_its_batch_alone() {
    let mut kit = Kit::new(false);
    let hooks = TextureTestHooks {
        allocation_count_limit: Some(2),
        ..TextureTestHooks::default()
    };
    let (port, mut owner) = TextureStore::start_with_test_hooks::<u32>(
        kit.context.clone(),
        TextureLimits::default(),
        TextureWakes::default(),
        hooks,
    )
    .expect("the store starts");
    let first = offer(&port, 0, 8, 8);
    let started = Instant::now();
    while !owner.envelope_ready_unsubmitted() {
        assert!(started.elapsed() < PATIENCE, "a batch was never closed");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(owner.submit(&mut kit.queue), "batch A is in flight");
    // Batch B goes to the other lease, which has no staging buffer, and the count is full.
    offer(&port, 1, 8, 8);
    while !owner.envelope_ready_unsubmitted() {
        assert!(started.elapsed() < PATIENCE, "batch B was never closed");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        owner.take_failure(),
        None,
        "the count is a limit, not a latched failure"
    );
    let done = kit.drive(&mut owner, 2);
    let mut ready = Vec::new();
    for completed in done {
        match completed {
            Completed::Ready(texture) => ready.push(texture),
            Completed::Failed { tag, reason } => {
                assert_eq!(tag, 1, "only batch B fails: {reason}");
                assert!(reason.contains("allocation count"), "{reason}");
                assert!(
                    reason.contains("2 of 2"),
                    "names in-use and limit: {reason}"
                );
            }
        }
    }
    assert_eq!(ready.len(), 1);
    // (No readback here: it allocates its own host buffer, which the lowered count would refuse.)
    assert_eq!(ready[0].tag, 0);
    assert_eq!(first.0.len(), 8 * 8 * 4);
    assert_eq!(owner.peek_failure(), None, "the store was never latched");
    drop(ready);
    drop(port);
    end(owner);
    kit.finish();
}
