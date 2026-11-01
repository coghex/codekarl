//! The bindless ledger on the real device (milestone 5.5, issue 5): the mode the gate chooses and its notice, the tables' initial
//! state, publication at a boundary, the pinned version, retirement and rejection without waiting for the device, and the
//! fallback. A submodule of `gpu_bindless`.
//!
//! The final stage's table size (131,072 slots) is used throughout: this device passes the gate there (the support query says
//! no at that size, which the gate logs and does not obey). Every case runs under validation with synchronization validation
//! and requires an empty log (the one tolerated message aside).

use std::sync::Arc;
use std::time::Instant;

use ck_gpu_vulkan::{
    LedgerEvent, LedgerLimits, PixelSource, RenderQueue, RevisionState, SceneContent,
    SceneReadback, TextureLedger, TextureLimits, TextureTestHooks, VulkanFilter, VulkanFrame,
    checker_pixels, readback_slots, vk,
};

use super::super::gpu_scene::{
    Mode, Quads, Rig, SLOTS, Shown, WHOLE, checker_image, compat_frame, compat_instance, differing,
    instance, reference,
};
use super::super::gpu_textures::{Kit, PATIENCE, pattern};

/// Pumps until `done` holds, failing after [`PATIENCE`].
fn pump_until(
    ledger: &TextureLedger<u32>,
    queue: &mut RenderQueue,
    mut done: impl FnMut(&TextureLedger<u32>, &[LedgerEvent<u32>]) -> bool,
) -> Vec<LedgerEvent<u32>> {
    let mut events = Vec::new();
    let started = Instant::now();
    loop {
        ledger.pump(queue, &mut events);
        if done(ledger, &events) {
            return events;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "never held; events {events:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Offers `pixels` as revision `revision` of `key` and pumps until it is prepared.
fn offer_and_prepare(
    ledger: &TextureLedger<u32>,
    queue: &mut RenderQueue,
    key: u32,
    revision: u64,
    pixels: &Arc<super::super::gpu_textures::Bytes>,
) {
    ledger
        .offer(
            key,
            revision,
            16,
            16,
            Arc::clone(pixels) as Arc<dyn PixelSource>,
        )
        .expect("the offer is admitted");
    pump_until(ledger, queue, |_, events| {
        events.iter().any(|event| {
            matches!(event, LedgerEvent::Prepared { key: k, revision: r } if *k == key && *r == revision)
        })
    });
}

/// The cell a slot showing the checker reads as through the probe: the 64 x 64 checker drawn nearest into 8 x 8 pixels reads
/// texel (8x + 4, 8y + 4) at pixel (x, y).
fn checker_cell() -> Vec<u8> {
    let checker = checker_pixels();
    let mut cell = Vec::with_capacity(8 * 8 * 4);
    for y in 0..8_usize {
        for x in 0..8_usize {
            let at = ((8 * y + 4) * 64 + (8 * x + 4)) * 4;
            cell.extend_from_slice(&checker[at..at + 4]);
        }
    }
    cell
}

#[test]
fn the_final_scale_starts_in_the_bindless_mode_with_one_notice() {
    let rig = Rig::new(Mode::Bindless, 8, 8, 16);
    let ledger = rig.ledger();
    assert_eq!(ledger.bindless_slots(), SLOTS);
    let notice = ledger.take_binding_notice().expect("a binding notice");
    println!("binding notice: {notice:?}");
    assert!(!notice.fallback, "the device passes the gate: {notice:?}");
    assert!(
        notice
            .text
            .starts_with("bindless table of 131072 slots (vkGetDescriptorSetLayoutSupport: "),
        "{}",
        notice.text
    );
    assert!(
        ledger.take_binding_notice().is_none(),
        "the notice is taken once"
    );
    let tolerated = rig.kit.log.tolerated();
    println!("tolerated validation messages: {}", tolerated.len());
    assert!(tolerated.len() <= 1, "{tolerated:#?}");
    rig.finish();
}

/// All 131,072 slots of both versions read as the checker before anything is published (design §13.1: initialise both tables once
/// to the checker), in 4,096-slot calls of the probe.
#[test]
fn every_slot_of_both_versions_reads_as_the_checker_at_the_start() {
    let mut rig = Rig::new(Mode::Bindless, 8, 8, 16);
    let expected = checker_cell();
    let slots: Vec<u32> = (0..SLOTS).collect();
    let ledger = rig.ledger.as_ref().expect("the ledger exists");
    for version in 0..2 {
        for chunk in slots.chunks(4_096) {
            let cells =
                readback_slots(&rig.kit.context, &mut rig.kit.queue, ledger, version, chunk)
                    .expect("the probe reads back");
            assert_eq!(cells.len(), chunk.len());
            for (cell, slot) in cells.iter().zip(chunk) {
                assert_eq!(
                    cell, &expected,
                    "version {version} slot {slot} is not the checker"
                );
            }
        }
    }
    rig.finish();
}

/// A key draws the checker until the boundary that adopts it, then its own pixels, through the ordinary readback of a scene frame.
#[test]
fn a_published_key_draws_the_checker_until_its_boundary_and_then_its_own_pixels() {
    let mut rig = Rig::new(Mode::Bindless, 8, 8, 16);
    let image = pattern(16, 16, 40);
    let size = (64, 64);
    let list = vec![instance(1, [0, 0, 64, 64], WHOLE)];
    rig.prepare(1, 1, 16, 16, &image);
    // Committed, not adopted: the key still draws the checker.
    let (before, counters) = rig.draw(size, [0.0; 3], VulkanFilter::Nearest, list.clone());
    assert_eq!((counters.checker, counters.resolved), (1, 0));
    let checker = reference(
        size,
        [0.0; 3],
        VulkanFilter::Nearest,
        &rig.shown,
        &checker_image(),
        &list,
    );
    assert_eq!(
        differing(&before, &checker, 1),
        0,
        "the checker before the boundary"
    );
    rig.adopt_all();
    rig.shown.insert(
        1,
        Shown {
            width: 16,
            height: 16,
            pixels: Arc::clone(&image),
        },
    );
    let (after, counters) = rig.draw(size, [0.0; 3], VulkanFilter::Nearest, list.clone());
    assert_eq!((counters.checker, counters.resolved), (0, 1));
    let own = reference(
        size,
        [0.0; 3],
        VulkanFilter::Nearest,
        &rig.shown,
        &checker_image(),
        &list,
    );
    assert_eq!(
        differing(&after, &own, 1),
        0,
        "its own pixels after the boundary"
    );
    assert!(differing(&before, &after, 1) > 0, "the two differ");
    rig.finish();
}

/// The pinned version (spec r4, step by step): a held frame keeps drawing its replaced revision, the pinned version is not written
/// until the frame's proof, and only then is the old revision released. Nothing waits for the device.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the five steps of the sequence read best in order"
)]
fn a_held_frame_keeps_its_version_unwritten_until_its_proof() {
    let kit = Kit::new(true);
    let mut rig = Rig::with_kit(
        kit,
        Mode::Bindless,
        8,
        16,
        TextureLimits::default(),
        LedgerLimits {
            max_extra_revisions: 8,
            ..LedgerLimits::default()
        },
        TextureTestHooks::default(),
    );
    let (old, new) = (pattern(16, 16, 10), pattern(16, 16, 90));
    rig.show(1, 16, 16, Arc::clone(&old));
    let context = Arc::clone(&rig.kit.context);
    let ledger = rig.ledger.as_ref().expect("the ledger exists");
    let queue = &mut rig.kit.queue;
    let s_old = ledger.test_revision_slot(1, 1).expect("R_old has a slot");
    let (a, _) = ledger.test_current_table().expect("tables");
    let b = a ^ 1;
    let old_view = ledger.test_table_entry(a, s_old).expect("an entry");
    assert_ne!(old_view, 0, "version A names R_old's view at s_old");
    assert_eq!(
        ledger.test_table_entry(b, s_old),
        Some(old_view),
        "and so does B"
    );

    // 1. F1 opens (pinning A while Building), is built and recorded behind the host event; it is not submitted yet.
    let device = context.device();
    // SAFETY: the case's own event on its own device, destroyed at the end of the case.
    let event = unsafe { device.create_event(&vk::EventCreateInfo::default(), None) }
        .expect("the events feature is enabled");
    let frame = VulkanFrame::new(
        1,
        VulkanFilter::Nearest,
        None,
        Some(Arc::new(Quads(vec![instance(1, [0, 0, 64, 64], WHOLE)]))),
    );
    let mut f1 = SceneReadback::prepare(
        &context,
        ledger,
        (64, 64),
        [0.0; 3],
        &SceneContent::Frame {
            frame: &frame,
            held: None,
        },
        16,
        Some(event),
    )
    .expect("F1 is prepared");
    assert_eq!(
        f1.ticket()
            .and_then(ck_gpu_vulkan::FrameTicket::test_table_version),
        Some(a),
        "F1 binds the current version"
    );
    assert_eq!(ledger.test_table_pins(a), Some(1));

    // 2. While F1 is Building and unsubmitted, key 1 is reloaded: R_new is uploaded, gets a new slot and is committed. B is
    // written, A (pinned) is not, and nothing is adopted while a ticket is Building.
    offer_and_prepare(ledger, queue, 1, 2, &new);
    ledger.commit(1, 2).expect("commit");
    let s_new = ledger.test_revision_slot(1, 2).expect("R_new has a slot");
    assert_ne!(
        s_new, s_old,
        "R_new never takes R_old's slot while R_old lives"
    );
    let new_view = ledger.test_table_wanted(s_new).expect("wanted");
    pump_until(ledger, queue, |ledger, _| {
        ledger.test_table_entry(b, s_new) == Some(new_view)
    });
    assert_eq!(
        ledger.test_table_entry(a, s_new),
        Some(0),
        "the pinned version A is not written"
    );
    let mut adopted = Vec::new();
    ledger.adopt(256, &mut adopted);
    assert!(
        adopted.is_empty(),
        "nothing is adopted while a ticket is Building"
    );
    assert_eq!(
        ledger.test_revision_state(1, 2),
        Some(RevisionState::Committed)
    );

    // 3. F1 is submitted, held. The next boundary adopts R_new in B (the swap rule), and R_old is retired and sanitised.
    f1.submit(queue).expect("F1 is submitted");
    assert!(!f1.is_proven(ledger));
    ledger.adopt(256, &mut adopted);
    assert_eq!(adopted, [(1, 2)]);
    assert_eq!(
        ledger.test_current_table(),
        Some((b, 1)),
        "B is current at epoch 1"
    );
    assert_eq!(
        ledger.test_revision_state(1, 1),
        Some(RevisionState::Retired)
    );

    // 4. Passes run while F1 is held: B is written (s_old becomes the checker there), A is not, and R_old is not released.
    pump_until(ledger, queue, |ledger, _| {
        ledger.test_table_entry(b, s_old) == Some(0)
    });
    for _ in 0..20 {
        ledger.pump(queue, &mut Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        ledger.test_table_entry(a, s_old),
        Some(old_view),
        "A still names R_old's view while F1 is pending"
    );
    assert_eq!(
        ledger.test_revision_state(1, 1),
        Some(RevisionState::Retired)
    );
    assert!(!f1.is_proven(ledger));

    // 5. The hold is released: F1 shows R_old's pixels; only after its proof is A written, and then R_old is released.
    // SAFETY: the event belongs to this device.
    unsafe { device.set_event(event) }.expect("set the event");
    let (held_pixels, _) = f1.finish(queue, ledger).expect("F1 finishes");
    let mut shown = std::collections::HashMap::new();
    shown.insert(
        1,
        Shown {
            width: 16,
            height: 16,
            pixels: Arc::clone(&old),
        },
    );
    let list = vec![instance(1, [0, 0, 64, 64], WHOLE)];
    let expected_old = reference(
        (64, 64),
        [0.0; 3],
        VulkanFilter::Nearest,
        &shown,
        &checker_image(),
        &list,
    );
    assert_eq!(
        differing(&held_pixels, &expected_old, 1),
        0,
        "F1 shows R_old's pixels"
    );
    pump_until(ledger, queue, |ledger, _| {
        ledger.test_table_entry(a, s_old) == Some(0) && ledger.test_revision_state(1, 1).is_none()
    });
    // SAFETY: the event's frame is proven finished.
    unsafe { device.destroy_event(event, None) };
    drop(context);

    // A new frame on B shows R_new at the key.
    rig.shown.insert(
        1,
        Shown {
            width: 16,
            height: 16,
            pixels: Arc::clone(&new),
        },
    );
    let (fresh, _) = rig.draw((64, 64), [0.0; 3], VulkanFilter::Nearest, list.clone());
    let expected_new = reference(
        (64, 64),
        [0.0; 3],
        VulkanFilter::Nearest,
        &rig.shown,
        &checker_image(),
        &list,
    );
    assert_eq!(
        differing(&fresh, &expected_new, 1),
        0,
        "a new frame shows R_new"
    );
    rig.finish();
}

/// The epoch pin alone (milestone 5.5, issue 6; design §11.1): a bindless frame pins its table version, **not** each revision it
/// draws. The frame is held with the host event. Every texture it draws is then reloaded and retired. None of the replaced revisions
/// is released before the frame's proof (the version it pinned still names every one of them, and a pinned version is not written),
/// the held frame shows the old textures, and the frame holds no per-revision pin. After the proof every old revision is released.
///
/// Negative control (recorded mutation): `begin_frame` that does not pin the version lets the next passes write it, so the old
/// revisions' entries are sanitised and the old revisions released while the held frame can still sample them.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the held frame, the replacement of every texture and the release after the proof read best in order"
)]
fn a_held_frame_pins_its_table_version_alone_and_keeps_every_replaced_texture() {
    const KEYS: u32 = 4;
    let kit = Kit::new(true);
    let mut rig = Rig::with_kit(
        kit,
        Mode::Bindless,
        8,
        16,
        TextureLimits::default(),
        LedgerLimits {
            max_extra_revisions: 8,
            ..LedgerLimits::default()
        },
        TextureTestHooks::default(),
    );
    let seed =
        |key: u32, generation: u32| u8::try_from(key * 20 + generation * 70).expect("a byte");
    for key in 1..=KEYS {
        rig.show(key, 16, 16, pattern(16, 16, seed(key, 0)));
    }
    let context = Arc::clone(&rig.kit.context);
    let ledger = rig.ledger.as_ref().expect("the ledger exists");
    let queue = &mut rig.kit.queue;
    let (a, _) = ledger.test_current_table().expect("tables");
    let b = a ^ 1;
    let old_slots: Vec<u32> = (1..=KEYS)
        .map(|key| ledger.test_revision_slot(key, 1).expect("a slot"))
        .collect();
    let old_views: Vec<u64> = old_slots
        .iter()
        .map(|&slot| ledger.test_table_entry(a, slot).expect("an entry"))
        .collect();

    // The frame draws all four textures in a 2 x 2 grid and is recorded behind the host event.
    let device = context.device();
    // SAFETY: the case's own event on its own device, destroyed at the end of the case.
    let event = unsafe { device.create_event(&vk::EventCreateInfo::default(), None) }
        .expect("the events feature is enabled");
    let list: Vec<_> = (1..=KEYS)
        .map(|key| {
            let (column, row) = (
                i32::try_from((key - 1) % 2).expect("small"),
                i32::try_from((key - 1) / 2).expect("small"),
            );
            instance(
                key,
                [column * 64, row * 64, column * 64 + 64, row * 64 + 64],
                WHOLE,
            )
        })
        .collect();
    let frame = VulkanFrame::new(
        1,
        VulkanFilter::Nearest,
        None,
        Some(Arc::new(Quads(list.clone()))),
    );
    let mut f1 = SceneReadback::prepare(
        &context,
        ledger,
        (128, 128),
        [0.0; 3],
        &SceneContent::Frame {
            frame: &frame,
            held: None,
        },
        16,
        Some(event),
    )
    .expect("F1 is prepared");
    let ticket = f1.ticket().expect("F1 is Building or recorded");
    assert_eq!(
        ticket.test_table_version(),
        Some(a),
        "F1 binds the current version"
    );
    assert_eq!(
        ticket.test_revision_pins(),
        0,
        "the bindless frame holds no per-revision pin: its version is its pin"
    );
    assert_eq!(ledger.test_table_pins(a), Some(1));

    // Every texture it drew is reloaded and committed (written to B only: A is pinned), then F1 is submitted, held.
    let news: Vec<_> = (1..=KEYS)
        .map(|key| pattern(16, 16, seed(key, 1)))
        .collect();
    for key in 1..=KEYS {
        offer_and_prepare(ledger, queue, key, 2, &news[(key - 1) as usize]);
        ledger.commit(key, 2).expect("commit");
    }
    for key in 1..=KEYS {
        let new_slot = ledger.test_revision_slot(key, 2).expect("a slot");
        let new_view = ledger.test_table_wanted(new_slot).expect("wanted");
        pump_until(ledger, queue, |ledger, _| {
            ledger.test_table_entry(b, new_slot) == Some(new_view)
        });
    }
    f1.submit(queue).expect("F1 is submitted");
    let mut adopted = Vec::new();
    ledger.adopt(256, &mut adopted);
    assert_eq!(
        adopted.len(),
        KEYS as usize,
        "every replacement is adopted: {adopted:?}"
    );
    for key in 1..=KEYS {
        assert_eq!(
            ledger.test_revision_state(key, 1),
            Some(RevisionState::Retired),
            "key {key}: the old revision is retired"
        );
    }

    // Passes run while F1 is held: B sanitises the old slots, A (pinned) does not, and no old revision is released.
    pump_until(ledger, queue, |ledger, _| {
        old_slots
            .iter()
            .all(|&slot| ledger.test_table_entry(b, slot) == Some(0))
    });
    for _ in 0..30 {
        ledger.pump(queue, &mut Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    for key in 1..=KEYS {
        let slot = old_slots[(key - 1) as usize];
        assert_eq!(
            ledger.test_table_entry(a, slot),
            Some(old_views[(key - 1) as usize]),
            "key {key}: the pinned version still names the old view"
        );
        assert_eq!(
            ledger.test_revision_state(key, 1),
            Some(RevisionState::Retired),
            "key {key}: the old revision is not released while the held frame can sample it"
        );
    }
    assert!(!f1.is_proven(ledger));

    // The hold ends: F1 shows the OLD textures; only after its proof are the old revisions released.
    // SAFETY: the event belongs to this device.
    unsafe { device.set_event(event) }.expect("set the event");
    let (held_pixels, _) = f1.finish(queue, ledger).expect("F1 finishes");
    let mut shown = std::collections::HashMap::new();
    for key in 1..=KEYS {
        shown.insert(
            key,
            Shown {
                width: 16,
                height: 16,
                pixels: pattern(16, 16, seed(key, 0)),
            },
        );
    }
    let expected_old = reference(
        (128, 128),
        [0.0; 3],
        VulkanFilter::Nearest,
        &shown,
        &checker_image(),
        &list,
    );
    assert_eq!(
        differing(&held_pixels, &expected_old, 1),
        0,
        "F1 shows the old textures"
    );
    pump_until(ledger, queue, |ledger, _| {
        (1..=KEYS).all(|key| ledger.test_revision_state(key, 1).is_none())
    });
    // SAFETY: the event's frame is proven finished.
    unsafe { device.destroy_event(event, None) };
    drop(context);
    rig.finish();
}

/// The compatibility hold names only an image the table the next frame binds already shows (review of part B): a revision that
/// is Prepared but not yet applied in the current version, or whose slot was already sanitised, is refused; a legitimate hold
/// during a Building ticket resolves the held image in the version that ticket pinned.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the five holds and their pixels read best in order"
)]
fn a_hold_is_refused_unless_the_current_version_already_shows_the_image() {
    use ck_gpu_vulkan::LedgerError;
    let kit = Kit::new(true);
    let mut rig = Rig::with_kit(
        kit,
        Mode::Bindless,
        8,
        16,
        TextureLimits::default(),
        LedgerLimits {
            max_extra_revisions: 8,
            ..LedgerLimits::default()
        },
        TextureTestHooks::default(),
    );
    let (old, new) = (pattern(16, 16, 20), pattern(16, 16, 110));
    rig.show(1, 16, 16, Arc::clone(&old));
    let context = Arc::clone(&rig.kit.context);
    let ledger = rig.ledger.as_ref().expect("the ledger exists");
    let queue = &mut rig.kit.queue;
    let s_old = ledger.test_revision_slot(1, 1).expect("a slot");
    let (a, _) = ledger.test_current_table().expect("tables");
    let old_view = ledger.test_table_entry(a, s_old).expect("an entry");

    // A Building ticket pins A. Revision 2 is uploaded and Prepared meanwhile: its entry is written to the spare version only,
    // so the pinned current version does not show it, and a hold of it is refused.
    let ticket = ledger.begin_frame().expect("a ticket");
    assert_eq!(ticket.test_table_version(), Some(a));
    offer_and_prepare(ledger, queue, 1, 2, &new);
    let s_new = ledger.test_revision_slot(1, 2).expect("a slot");
    assert_eq!(
        ledger.test_table_entry(a, s_new),
        Some(0),
        "A does not show revision 2"
    );
    assert_eq!(
        ledger.hold_compat(1, 2),
        Err(LedgerError::UnknownRevision),
        "a revision the current version does not show is not held"
    );
    // The legitimate hold: revision 1 is shown by A, and the ticket resolves it to the slot A names as its view.
    ledger.hold_compat(1, 1).expect("revision 1 is shown by A");
    let held = ticket
        .resolve_revision(1, 1)
        .expect("the held revision resolves");
    assert_eq!(held.slot(), Some(s_old));
    assert_eq!(
        ledger.test_table_entry(a, s_old),
        Some(old_view),
        "and A names its view there"
    );
    ticket.discarded().expect("the ticket ends");

    // Revision 2 is committed and adopted; the hold keeps revision 1 (retired) in the tables.
    ledger.commit(1, 2).expect("commit");
    let mut adopted = Vec::new();
    pump_until(ledger, queue, |ledger, _| {
        ledger.adopt(256, &mut adopted);
        !adopted.is_empty()
    });
    assert_eq!(
        ledger.test_revision_state(1, 1),
        Some(RevisionState::Retired)
    );
    assert_eq!(
        ledger.test_table_wanted(s_old),
        Some(old_view),
        "a held revision is not sanitised"
    );

    // A frame is held pending and the hold moves to revision 2: revision 1 is sanitised while that frame pins it, and holding it
    // again is refused (its slot names the checker from now on).
    let device = context.device();
    // SAFETY: the case's own event on its own device, destroyed at the end of the case.
    let event = unsafe { device.create_event(&vk::EventCreateInfo::default(), None) }
        .expect("the events feature is enabled");
    let frame = VulkanFrame::new(
        1,
        VulkanFilter::Nearest,
        None,
        Some(Arc::new(Quads(vec![instance(1, [0, 0, 64, 64], WHOLE)]))),
    );
    let mut f1 = SceneReadback::prepare(
        &context,
        ledger,
        (64, 64),
        [0.0; 3],
        &SceneContent::Frame {
            frame: &frame,
            held: None,
        },
        16,
        Some(event),
    )
    .expect("F1 is prepared");
    f1.submit(queue).expect("F1 is submitted");
    ledger
        .hold_compat(1, 2)
        .expect("revision 2 is shown by the current version");
    assert_eq!(
        ledger.test_table_wanted(s_old),
        Some(0),
        "revision 1 is sanitised once the hold moved"
    );
    assert_eq!(
        ledger.hold_compat(1, 1),
        Err(LedgerError::UnknownRevision),
        "a sanitised revision is not held again"
    );
    // SAFETY: the event belongs to this device.
    unsafe { device.set_event(event) }.expect("set the event");
    f1.finish(queue, ledger).expect("F1 finishes");
    // SAFETY: the event's frame is proven finished.
    unsafe { device.destroy_event(event, None) };
    // The held image's pixels: the key retires, so it resolves to nothing, and the frame draws the held revision (2) from the
    // table the ticket pinned, at the rectangle it was drawn at.
    ledger.retire_key(1);
    let held = ck_gpu_vulkan::HeldCompat {
        key: 1,
        revision: 2,
        destination: ck_gpu_vulkan::DestinationRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        },
        source: WHOLE,
    };
    let frame = compat_frame(1, vec![compat_instance(1, [0, 0, 64, 64])]);
    let (pixels, counters) = ck_gpu_vulkan::readback_scene_with_held(
        &context,
        queue,
        ledger,
        (64, 64),
        [0.0; 3],
        &frame,
        16,
        Some(held),
    )
    .expect("the held image is drawn");
    assert_eq!((counters.resolved, counters.checker), (1, 0));
    let mut shown = std::collections::HashMap::new();
    shown.insert(
        1,
        Shown {
            width: 16,
            height: 16,
            pixels: Arc::clone(&new),
        },
    );
    let expected = reference(
        (64, 64),
        [0.0; 3],
        VulkanFilter::Nearest,
        &shown,
        &checker_image(),
        &[instance(1, [0, 0, 64, 64], WHOLE)],
    );
    assert_eq!(
        differing(&pixels, &expected, 1),
        0,
        "the held revision's own pixels, not the checker"
    );
    ledger.release_compat_hold();
    drop(context);
    rig.finish();
}

/// A replaced revision and a retired key: with passes only (no scene or Lua edit), and separately with frames continuing, the
/// slot reaches the checker in both versions before the revision is released, and is reused only after. The check each pass makes
/// is the invariant: a released revision's slot never names its view in either version.
#[test]
fn a_retired_revision_is_sanitised_in_both_versions_before_it_is_released_and_its_slot_reused() {
    for frames in [false, true] {
        let mut rig = Rig::new(Mode::Bindless, 8, 8, 16);
        let (first, second, third) = (
            pattern(16, 16, 5),
            pattern(16, 16, 70),
            pattern(16, 16, 130),
        );
        rig.show(1, 16, 16, Arc::clone(&first));
        let ledger = rig.ledger.as_ref().expect("the ledger exists");
        let s1 = ledger.test_revision_slot(1, 1).expect("a slot");
        let view1 = ledger.test_table_entry(0, s1).expect("an entry");
        assert_ne!(view1, 0, "revision 1 is in the table");
        let in_use = ledger.test_slots_in_use().expect("tables");
        // Replace revision 1 by 2: revision 1 is retired and sanitised.
        rig.prepare(1, 2, 16, 16, &second);
        rig.adopt_all();
        let list = vec![instance(1, [0, 0, 64, 64], WHOLE)];
        let started = Instant::now();
        loop {
            if frames {
                // Frames continue: each one pins the current version, then is proven by the pump inside the readback.
                let _ = rig.draw((64, 64), [0.0; 3], VulkanFilter::Nearest, list.clone());
            }
            let ledger = rig.ledger.as_ref().expect("the ledger exists");
            // The invariant, read after each pass: while revision 1 exists its slot is not reused; once it is gone, neither
            // version names its view (the slot is clear), and nothing else has taken the slot yet.
            if ledger.test_revision_state(1, 1).is_none() {
                for version in 0..2 {
                    assert_eq!(
                        ledger.test_table_entry(version, s1),
                        Some(0),
                        "frames {frames}: a released revision's slot names the checker in version {version}"
                    );
                }
                break;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "revision 1 was never released"
            );
            ledger.pump(&mut rig.kit.queue, &mut Vec::new());
        }
        // Only now is the slot free: the slots in use are back to the checker's, revision 2's and nothing else.
        let ledger = rig.ledger.as_ref().expect("the ledger exists");
        assert_eq!(ledger.test_slots_in_use(), Some(in_use), "frames: {frames}");
        offer_and_prepare(ledger, &mut rig.kit.queue, 2, 1, &third);
        assert_eq!(
            ledger.test_revision_slot(2, 1),
            Some(s1),
            "the freed slot is the next one reused"
        );
        // The key's retirement releases the last revision with no further edit.
        ledger.retire_key(1);
        let started = Instant::now();
        while ledger.test_revision_state(1, 2).is_some() {
            assert!(
                started.elapsed() < PATIENCE,
                "revision 2 was never released"
            );
            ledger.pump(&mut rig.kit.queue, &mut Vec::new());
        }
        rig.finish();
    }
}

/// A rejected prepared revision: its slot is sanitised in both versions before the revision is released and the slot freed.
#[test]
fn a_rejected_prepared_revision_leaves_the_checker_in_both_versions_before_it_is_released() {
    let mut rig = Rig::new(Mode::Bindless, 8, 8, 16);
    let image = pattern(16, 16, 33);
    let ledger = rig.ledger.as_ref().expect("the ledger exists");
    let queue = &mut rig.kit.queue;
    let before = ledger.test_slots_in_use().expect("tables");
    offer_and_prepare(ledger, queue, 4, 1, &image);
    let slot = ledger.test_revision_slot(4, 1).expect("a slot");
    assert_eq!(ledger.test_slots_in_use(), Some(before + 1));
    ledger.reject(4, 1).expect("reject");
    let started = Instant::now();
    while ledger.test_revision_state(4, 1).is_some() {
        assert!(started.elapsed() < PATIENCE, "never released");
        ledger.pump(queue, &mut Vec::new());
    }
    for version in 0..2 {
        assert_eq!(ledger.test_table_entry(version, slot), Some(0));
    }
    assert_eq!(
        ledger.test_slots_in_use(),
        Some(before),
        "the slot is free again"
    );
    rig.finish();
}

/// A device whose gate fails a row starts in the per-revision mode with one notice naming the row, and draws the same pixels.
/// The refusal is real: one slot more than the device's own per-stage update-after-bind sampled-image limit, with no seam.
#[test]
fn a_real_refusal_falls_back_with_one_notice_and_draws_the_same_pixels() {
    let probe = Rig::new(Mode::PerRevision, 8, 8, 16);
    let limit = probe
        .kit
        .context
        .bindless_caps()
        .limits
        .max_per_stage_descriptor_update_after_bind_sampled_images;
    probe.finish();
    let requested = limit.checked_add(1).expect("a limit below u32::MAX");
    let image = pattern(16, 16, 77);
    let list = vec![
        // Whole numbers of pixels per texel (4 and 1), so nearest sampling never lands on a texel edge.
        instance(1, [0, 0, 64, 64], WHOLE),
        // An unresolved key draws the whole checker over its destination, whatever its source.
        instance(9, [64, 64, 128, 128], WHOLE),
    ];
    let mut outputs = Vec::new();
    for mode in [Mode::Bindless, Mode::Auto(requested)] {
        let mut rig = Rig::new(mode, 8, 8, 16);
        let ledger = rig.ledger.as_ref().expect("the ledger exists");
        let notice = ledger.take_binding_notice().expect("a notice");
        println!("{mode:?}: {notice:?}");
        if mode == Mode::Bindless {
            assert_eq!(ledger.bindless_slots(), SLOTS);
            assert!(!notice.fallback);
        } else {
            assert_eq!(ledger.bindless_slots(), 0, "the per-revision mode");
            assert!(notice.fallback, "{notice:?}");
            assert!(
                notice.text.contains(&format!(
                    "maxPerStageDescriptorUpdateAfterBindSampledImages {limit} < {requested} requested"
                )),
                "{}",
                notice.text
            );
            assert!(
                notice.text.starts_with("bindless unavailable on "),
                "{}",
                notice.text
            );
        }
        assert!(ledger.take_binding_notice().is_none(), "exactly once");
        rig.show(1, 16, 16, Arc::clone(&image));
        // Nearest sampling is exact against the CPU reference; linear sampling is the device's own, compared between the modes.
        let (nearest, _) = rig.draw(
            (128, 128),
            [0.1, 0.2, 0.3],
            VulkanFilter::Nearest,
            list.clone(),
        );
        let expected = reference(
            (128, 128),
            [0.1, 0.2, 0.3],
            VulkanFilter::Nearest,
            &rig.shown,
            &checker_image(),
            &list,
        );
        assert_eq!(
            differing(&nearest, &expected, 1),
            0,
            "{mode:?} matches the reference"
        );
        let (linear, _) = rig.draw(
            (128, 128),
            [0.1, 0.2, 0.3],
            VulkanFilter::Linear,
            list.clone(),
        );
        outputs.push((nearest, linear));
        rig.finish();
    }
    assert_eq!(
        differing(&outputs[0].0, &outputs[1].0, 0),
        0,
        "the two modes draw identical nearest pixels"
    );
    assert_eq!(
        differing(&outputs[0].1, &outputs[1].1, 0),
        0,
        "the two modes draw identical linear pixels"
    );
}

/// No source file of the crate calls `vkDeviceWaitIdle` or `vkQueueWaitIdle` (design §13.3: never to reload, release or reuse a
/// slot). Comments may name them.
#[test]
fn no_source_waits_for_the_device_or_the_queue_to_go_idle() {
    fn scan(directory: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("a readable directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                scan(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file");
                for (number, line) in text.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default();
                    if code.contains("device_wait_idle") || code.contains("queue_wait_idle") {
                        found.push(format!("{}:{}", path.display(), number + 1));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    scan(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    assert!(found.is_empty(), "idle waits in the library: {found:?}");
}
