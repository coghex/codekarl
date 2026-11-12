use super::*;

fn make_bindings(entries: &[(&str, &[&str])]) -> Result<Bindings, BindingsError> {
    Bindings::from_entries(entries.iter().map(|(action, bindings)| {
        (
            (*action).to_owned(),
            bindings
                .iter()
                .map(|binding| (*binding).to_owned())
                .collect(),
        )
    }))
}

fn bindings(entries: &[(&str, &[&str])]) -> Bindings {
    make_bindings(entries).unwrap_or_else(|error| panic!("valid binding fixture: {error}"))
}

fn point(x: i32, y: i32) -> PointerPosition {
    PointerPosition { x, y }
}

#[test]
fn pointer_ring_preserves_order_coalesces_only_unsnapshotted_neighbors_and_acknowledges_by_watermark()
 {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Down(point(0, 0)));
    mapper.pointer(PointerEvent::Move(point(256, 512)));
    mapper.pointer(PointerEvent::Move(point(512, 768)));
    mapper.pointer(PointerEvent::Scroll {
        position: point(512, 768),
        amount: 256,
        unit: ScrollUnit::Lines,
    });
    let snapshot = mapper.snapshot();
    assert_eq!(
        snapshot.pointer_events,
        [
            PointerEvent::Down(point(0, 0)),
            PointerEvent::Move(point(512, 768)),
            PointerEvent::Scroll {
                position: point(512, 768),
                amount: 256,
                unit: ScrollUnit::Lines
            },
        ]
    );

    mapper.pointer(PointerEvent::Scroll {
        position: point(512, 768),
        amount: 256,
        unit: ScrollUnit::Lines,
    });
    mapper.acknowledge(snapshot.sequence);
    let next = mapper.snapshot();
    assert_eq!(
        next.pointer_events,
        [PointerEvent::Scroll {
            position: point(512, 768),
            amount: 256,
            unit: ScrollUnit::Lines,
        }]
    );
}

#[test]
fn a_move_after_a_snapshot_is_not_coalesced_into_its_copied_move() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Move(point(10, 20)));
    let earlier = mapper.snapshot();
    assert_eq!(earlier.pointer_events, [PointerEvent::Move(point(10, 20))]);

    mapper.pointer(PointerEvent::Move(point(30, 40)));
    mapper.acknowledge(earlier.sequence);
    assert_eq!(
        mapper.snapshot().pointer_events,
        [PointerEvent::Move(point(30, 40))]
    );
}

#[test]
fn coalesced_scroll_saturates_and_snapshots_compare_equal_without_mutation() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Scroll {
        position: point(10, 20),
        amount: i32::MAX - 1,
        unit: ScrollUnit::Lines,
    });
    mapper.pointer(PointerEvent::Scroll {
        position: point(10, 20),
        amount: 8,
        unit: ScrollUnit::Lines,
    });
    let first = mapper.snapshot();
    assert_eq!(
        first.pointer_events[0],
        PointerEvent::Scroll {
            position: point(10, 20),
            amount: i32::MAX,
            unit: ScrollUnit::Lines,
        }
    );
    let second = mapper.snapshot();
    assert_eq!(second.sequence, first.sequence + 1);
    let mut second_payload = second;
    second_payload.sequence = first.sequence;
    assert_eq!(
        first, second_payload,
        "snapshot payload is stable until ack"
    );
}

#[test]
fn pointer_scroll_reversal_and_anchor_changes_start_new_records() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Scroll {
        position: point(1, 2),
        amount: 300,
        unit: ScrollUnit::Points,
    });
    mapper.pointer(PointerEvent::Scroll {
        position: point(1, 2),
        amount: 40,
        unit: ScrollUnit::Points,
    });
    mapper.pointer(PointerEvent::Scroll {
        position: point(1, 2),
        amount: -20,
        unit: ScrollUnit::Points,
    });
    mapper.pointer(PointerEvent::Scroll {
        position: point(3, 2),
        amount: -10,
        unit: ScrollUnit::Points,
    });
    assert_eq!(
        mapper.snapshot().pointer_events,
        [
            PointerEvent::Scroll {
                position: point(1, 2),
                amount: 340,
                unit: ScrollUnit::Points
            },
            PointerEvent::Scroll {
                position: point(1, 2),
                amount: -20,
                unit: ScrollUnit::Points
            },
            PointerEvent::Scroll {
                position: point(3, 2),
                amount: -10,
                unit: ScrollUnit::Points
            },
        ]
    );
}

#[test]
fn button_events_keep_order_update_position_and_never_change_left_held_state() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Down(point(1, 1)));
    mapper.pointer(PointerEvent::ButtonDown {
        button: PointerButton::Middle,
        position: point(2, 2),
    });
    mapper.pointer(PointerEvent::Move(point(3, 3)));
    mapper.pointer(PointerEvent::ButtonUp {
        button: PointerButton::Middle,
        position: point(4, 4),
    });
    mapper.pointer(PointerEvent::ButtonDown {
        button: PointerButton::Right,
        position: point(5, 5),
    });
    mapper.pointer(PointerEvent::Up(point(6, 6)));

    let snapshot = mapper.snapshot();
    assert_eq!(
        snapshot.pointer_events,
        [
            PointerEvent::Down(point(1, 1)),
            PointerEvent::ButtonDown {
                button: PointerButton::Middle,
                position: point(2, 2),
            },
            PointerEvent::Move(point(3, 3)),
            PointerEvent::ButtonUp {
                button: PointerButton::Middle,
                position: point(4, 4),
            },
            PointerEvent::ButtonDown {
                button: PointerButton::Right,
                position: point(5, 5),
            },
            PointerEvent::Up(point(6, 6)),
        ]
    );
    assert!(!snapshot.pointer_held);
    assert_eq!(snapshot.pointer_position, Some(point(6, 6)));

    let mut buttons_without_left = Mapper::new(bindings(&[]));
    buttons_without_left.pointer(PointerEvent::ButtonDown {
        button: PointerButton::Right,
        position: point(7, 8),
    });
    assert!(!buttons_without_left.snapshot().pointer_held);
}

#[test]
fn button_events_are_coalescing_barriers_and_focus_loss_does_not_release_them() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Move(point(1, 1)));
    mapper.pointer(PointerEvent::ButtonDown {
        button: PointerButton::Middle,
        position: point(2, 2),
    });
    mapper.pointer(PointerEvent::Move(point(3, 3)));
    mapper.pointer(PointerEvent::Scroll {
        position: point(3, 3),
        amount: 128,
        unit: ScrollUnit::Lines,
    });
    mapper.pointer(PointerEvent::ButtonUp {
        button: PointerButton::Right,
        position: point(4, 4),
    });
    mapper.pointer(PointerEvent::Scroll {
        position: point(4, 4),
        amount: 64,
        unit: ScrollUnit::Lines,
    });
    assert_eq!(
        mapper.snapshot().pointer_events,
        [
            PointerEvent::Move(point(1, 1)),
            PointerEvent::ButtonDown {
                button: PointerButton::Middle,
                position: point(2, 2),
            },
            PointerEvent::Move(point(3, 3)),
            PointerEvent::Scroll {
                position: point(3, 3),
                amount: 128,
                unit: ScrollUnit::Lines,
            },
            PointerEvent::ButtonUp {
                button: PointerButton::Right,
                position: point(4, 4),
            },
            PointerEvent::Scroll {
                position: point(4, 4),
                amount: 64,
                unit: ScrollUnit::Lines,
            },
        ]
    );
    assert_eq!(mapper.snapshot().pointer_position, Some(point(4, 4)));

    mapper.pointer(PointerEvent::Down(point(8, 8)));
    mapper.pointer(PointerEvent::ButtonDown {
        button: PointerButton::Middle,
        position: point(9, 9),
    });
    let before_focus_loss = mapper.snapshot().pointer_events.len();
    mapper.focus_lost();
    let reset = mapper.snapshot();
    assert_eq!(
        &reset.pointer_events[before_focus_loss..],
        &[PointerEvent::Up(point(9, 9))]
    );
}

#[test]
fn pointer_overflow_keeps_exact_current_state_and_focus_loss_appends_release() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::Down(point(0, 0)));
    for index in 0..=POINTER_CAPACITY {
        mapper.pointer(PointerEvent::Scroll {
            position: point(i32::try_from(index).expect("small event index fits"), 0),
            amount: 256,
            unit: ScrollUnit::Lines,
        });
    }
    let overflow = mapper.snapshot();
    assert!(overflow.pointer_overflow);
    assert!(overflow.pointer_held);
    assert_eq!(
        overflow.pointer_position,
        Some(point(
            i32::try_from(POINTER_CAPACITY).expect("ring capacity fits"),
            0
        ))
    );
    assert_eq!(overflow.pointer_events.len(), POINTER_CAPACITY);
    assert_eq!(
        overflow.pointer_events.first(),
        Some(&PointerEvent::Scroll {
            position: point(1, 0),
            amount: 256,
            unit: ScrollUnit::Lines,
        }),
        "overflow drops the oldest Down and then retains the newest ring entries"
    );
    assert!(
        !overflow
            .pointer_events
            .iter()
            .any(|event| matches!(event, PointerEvent::Down(_)))
    );

    mapper.acknowledge(overflow.sequence);
    mapper.focus_lost();
    let released = mapper.snapshot();
    assert!(!released.pointer_held);
    assert_eq!(
        released.pointer_events.last(),
        Some(&PointerEvent::Up(point(
            i32::try_from(POINTER_CAPACITY).expect("ring capacity fits"),
            0,
        )))
    );
    assert!(released.reset);
}

#[test]
fn button_events_participate_in_ring_overflow_without_changing_held_state() {
    let mut mapper = Mapper::new(bindings(&[]));
    mapper.pointer(PointerEvent::ButtonDown {
        button: PointerButton::Right,
        position: point(0, 0),
    });
    for index in 1..=POINTER_CAPACITY {
        mapper.pointer(PointerEvent::ButtonUp {
            button: PointerButton::Middle,
            position: point(i32::try_from(index).expect("small event index fits"), 0),
        });
    }
    let snapshot = mapper.snapshot();
    assert!(snapshot.pointer_overflow);
    assert!(!snapshot.pointer_held);
    assert_eq!(snapshot.pointer_events.len(), POINTER_CAPACITY);
    assert_eq!(
        snapshot.pointer_events.first(),
        Some(&PointerEvent::ButtonUp {
            button: PointerButton::Middle,
            position: point(1, 0),
        })
    );
    assert_eq!(
        snapshot.pointer_position,
        Some(point(
            i32::try_from(POINTER_CAPACITY).expect("capacity fits"),
            0
        ))
    );
}

#[test]
fn pointer_coordinates_use_finite_fixed_point_values_and_flooring_is_available_for_negative_positions()
 {
    assert_eq!(
        PointerPosition::from_points(-4.5, 2.25),
        Some(point(-1152, 576))
    );
    assert!(PointerPosition::from_points(f64::NAN, 0.0).is_none());
    assert!(PointerPosition::from_points(f64::MAX, 0.0).is_none());
    assert_eq!((-1_i32).div_euclid(4 * POINTER_FRACTION), -1);
}

fn action<'a>(snapshot: &'a InputSnapshot, name: &str) -> &'a ActionSnapshot {
    snapshot
        .actions
        .iter()
        .find(|state| state.name == name)
        .unwrap_or_else(|| panic!("missing action {name:?}"))
}

#[test]
fn all_81_key_names_round_trip_exactly_and_reject_character_aliases() {
    // Negative control: an omitted supported code or an extra character alias breaks this exact set.
    const EXPECTED: [&str; KEY_COUNT] = [
        "KeyA",
        "KeyB",
        "KeyC",
        "KeyD",
        "KeyE",
        "KeyF",
        "KeyG",
        "KeyH",
        "KeyI",
        "KeyJ",
        "KeyK",
        "KeyL",
        "KeyM",
        "KeyN",
        "KeyO",
        "KeyP",
        "KeyQ",
        "KeyR",
        "KeyS",
        "KeyT",
        "KeyU",
        "KeyV",
        "KeyW",
        "KeyX",
        "KeyY",
        "KeyZ",
        "Digit0",
        "Digit1",
        "Digit2",
        "Digit3",
        "Digit4",
        "Digit5",
        "Digit6",
        "Digit7",
        "Digit8",
        "Digit9",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Escape",
        "Space",
        "Enter",
        "Tab",
        "Backspace",
        "Delete",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Minus",
        "Equal",
        "BracketLeft",
        "BracketRight",
        "Backslash",
        "Semicolon",
        "Quote",
        "Comma",
        "Period",
        "Slash",
        "Backquote",
        "ShiftLeft",
        "ShiftRight",
        "ControlLeft",
        "ControlRight",
        "AltLeft",
        "AltRight",
        "MetaLeft",
        "MetaRight",
    ];
    assert_eq!(Key::ALL.len(), KEY_COUNT);
    for (index, key) in Key::ALL.into_iter().enumerate() {
        assert_eq!(key as usize, index);
        assert_eq!(Key::from_name(key.name()), Some(key));
        assert_eq!(key.name(), EXPECTED[index]);
    }
    let names: std::collections::HashSet<_> = Key::ALL.into_iter().map(Key::name).collect();
    assert_eq!(names.len(), 81);
    for unsupported in ["keya", "Key A", "A", "Escape ", "KeyAA", "Numpad1"] {
        assert_eq!(Key::from_name(unsupported), None, "{unsupported:?}");
    }
}

#[test]
fn defaults_fill_missing_actions_while_empty_entries_unbind_and_custom_names_sort() {
    // Negative control: treating an explicit empty vector as a missing action reinstates move_left.
    let configured = bindings(&[("move_left", &[]), ("z_custom", &["Alt+Shift+KeyQ"])]);
    assert!(
        configured
            .action("move_left")
            .is_some_and(|item| item.bindings().is_empty())
    );
    assert_eq!(
        configured
            .action("move_right")
            .map(|item| item.bindings().len()),
        Some(2)
    );
    assert_eq!(
        configured.action("close").map(|item| item.bindings().len()),
        Some(1)
    );
    assert!(configured.action("z_custom").is_some());
    let names: Vec<_> = configured
        .actions()
        .iter()
        .map(ActionBindings::name)
        .collect();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(configured.action("move_left").is_some());
}

#[test]
fn binding_grammar_reports_action_and_entry_for_each_rejected_form() {
    // Negative control: accepting any malformed prefix/key/whitespace/length case loses the named reason.
    let too_long = "x".repeat(65);
    for (entry, reason) in [
        ("KeyUnknown", "unknown physical key"),
        ("Command+KeyA", "unknown modifier prefix"),
        ("Control+Control+KeyA", "repeated"),
        ("Shift+Control+KeyA", "out of order"),
        ("Shift+ShiftLeft", "modifier as its main key"),
        ("", "empty"),
        ("Key A", "whitespace"),
        (too_long.as_str(), "64 bytes"),
    ] {
        let error = make_bindings(&[("run", &[entry])])
            .err()
            .unwrap_or_else(|| panic!("invalid entry {entry:?} was accepted"));
        let message = error.to_string();
        assert!(message.contains("action \"run\""), "{message}");
        assert!(message.contains(entry), "{message}");
        assert!(message.contains(reason), "{message}");
    }
    assert!(make_bindings(&[("Run", &[])]).is_err());
    assert!(make_bindings(&[("_run", &[])]).is_err());
    assert!(make_bindings(&[("a".repeat(33).as_str(), &[])]).is_err());
}

#[test]
fn entry_limits_duplicates_and_cross_action_sharing_are_validated() {
    // Negative control: allowing duplicate actions/bindings or an off-by-one action cap fails these boundaries.
    assert!(make_bindings(&[("run", &["KeyA", "KeyA"])]).is_err());
    assert!(make_bindings(&[("run", &[]), ("run", &[])]).is_err());
    assert!(
        make_bindings(&[(
            "run",
            &[
                "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI"
            ]
        )])
        .is_err()
    );

    let mut sixty_four_actions = (0..58)
        .map(|index| (format!("custom_{index:02}"), Vec::new()))
        .collect::<Vec<_>>();
    let exact = Bindings::from_entries(sixty_four_actions.clone());
    assert!(exact.is_ok(), "6 defaults plus 58 custom actions fit");
    sixty_four_actions.push(("custom_58".to_owned(), Vec::new()));
    let error = Bindings::from_entries(sixty_four_actions)
        .err()
        .unwrap_or_else(|| panic!("65 actions were accepted"));
    assert!(error.to_string().contains("custom_58"));
    assert!(error.to_string().contains("64 actions"));

    let shared = bindings(&[("first", &["KeyQ"]), ("second", &["KeyQ"])]);
    let mut mapper = Mapper::new(shared);
    mapper.key(Key::KeyQ, true, false);
    let snapshot = mapper.snapshot();
    assert!(action(&snapshot, "first").pressed);
    assert!(action(&snapshot, "second").pressed);
}

#[test]
fn an_action_is_held_as_the_or_of_its_bindings() {
    // Negative control: releasing one binding must not release an action whose other binding stays held.
    let mut mapper = Mapper::new(bindings(&[("move_left", &["KeyA", "ArrowLeft"])]));
    mapper.key(Key::KeyA, true, false);
    let first = mapper.snapshot();
    assert!(action(&first, "move_left").held);
    assert!(action(&first, "move_left").pressed);

    mapper.key(Key::ArrowLeft, true, false);
    mapper.key(Key::KeyA, false, false);
    let second = mapper.snapshot();
    assert!(action(&second, "move_left").held);
    assert!(!action(&second, "move_left").released);

    mapper.key(Key::ArrowLeft, false, false);
    let third = mapper.snapshot();
    assert!(!action(&third, "move_left").held);
    assert!(action(&third, "move_left").released);
    assert_eq!(
        third
            .transitions
            .iter()
            .filter(|edge| edge.action == "move_left" && edge.edge == Edge::Release)
            .count(),
        1
    );
}

#[test]
fn a_tap_latches_both_edges_and_repeat_rules_do_not_invent_edges() {
    let mut mapper = Mapper::new(bindings(&[("jump", &["KeyQ"])]));
    // Negative control: clearing on snapshot would lose this complete press/release before admission.
    mapper.key(Key::KeyQ, true, false);
    mapper.key(Key::KeyQ, false, false);
    let tap = mapper.snapshot();
    assert!(!action(&tap, "jump").held);
    assert!(action(&tap, "jump").pressed);
    assert!(action(&tap, "jump").released);
    assert_eq!(
        tap.transitions
            .iter()
            .filter(|edge| edge.action == "jump")
            .map(|edge| edge.edge)
            .collect::<Vec<_>>(),
        [Edge::Press, Edge::Release]
    );

    mapper.acknowledge(tap.sequence);
    mapper.key(Key::KeyQ, true, true); // a repeat for an unheld key recovers the missed initial down
    let down = mapper.snapshot();
    assert!(action(&down, "jump").held);
    assert!(action(&down, "jump").pressed);
    mapper.key(Key::KeyQ, true, true); // a repeat while already held is ignored
    let repeated = mapper.snapshot();
    assert_eq!(repeated.transitions.len(), 1);
}

#[test]
fn either_modifier_side_satisfies_prefixes_and_modifier_changes_recompute_actions() {
    // Negative control: recomputing only on main-key transitions leaves the chord held after Control-up.
    let mut mapper = Mapper::new(bindings(&[("combo", &["Control+KeyR"])]));
    mapper.key(Key::ControlLeft, true, false);
    mapper.key(Key::KeyR, true, false);
    let left = mapper.snapshot();
    assert!(action(&left, "combo").held);
    assert!(action(&left, "reload_texture").held);

    mapper.key(Key::ControlLeft, false, false);
    let released = mapper.snapshot();
    assert!(!action(&released, "combo").held);
    assert!(action(&released, "combo").released);
    assert!(action(&released, "reload_texture").held);

    mapper.key(Key::ControlRight, true, false);
    let right = mapper.snapshot();
    assert!(action(&right, "combo").held);
    assert!(action(&right, "reload_texture").held);
}

#[test]
fn focus_loss_releases_every_action_once_and_latches_reset() {
    // Negative control: preserving the held-key bitset across focus loss leaves movement or a chord stuck.
    let mut mapper = Mapper::new(bindings(&[("combo", &["Control+KeyQ"])]));
    mapper.key(Key::ControlLeft, true, false);
    mapper.key(Key::KeyQ, true, false);
    mapper.key(Key::KeyA, true, false);
    mapper.focus_lost();
    let reset = mapper.snapshot();
    assert!(reset.reset);
    assert!(reset.actions.iter().all(|state| !state.held));
    assert!(action(&reset, "combo").released);
    assert!(action(&reset, "move_left").released);
    let releases = reset
        .transitions
        .iter()
        .filter(|edge| edge.edge == Edge::Release)
        .count();
    assert_eq!(releases, 2);

    mapper.key(Key::KeyA, false, false);
    let after_up = mapper.snapshot();
    assert!(after_up.reset);
    assert_eq!(after_up.transitions.len(), reset.transitions.len());
}

#[test]
fn overflow_keeps_the_newest_256_edges_exact_latches_and_no_stuck_held_action() {
    // Negative control: dropping detailed edges without independent latches loses the tap; retaining all edges exceeds capacity.
    let mut mapper = Mapper::new(bindings(&[]));
    for _ in 0..300 {
        mapper.key(Key::KeyA, true, false);
        mapper.key(Key::KeyA, false, false);
    }
    let snapshot = mapper.snapshot();
    let left = action(&snapshot, "move_left");
    assert!(!left.held);
    assert!(left.pressed && left.released);
    assert_eq!(snapshot.transitions.len(), TRANSITION_CAPACITY);
    assert!(snapshot.overflow);
    // 300 taps emit 600 action edges; 600 - 256 = 344 dropped records.
    assert_eq!(snapshot.overflow_count, 344);
    assert_eq!(mapper.overflow_count(), 344);
    assert_eq!(
        snapshot.transitions.first().map(|edge| edge.edge),
        Some(Edge::Press)
    );
    assert_eq!(
        snapshot.transitions.last().map(|edge| edge.edge),
        Some(Edge::Release)
    );

    mapper.acknowledge(snapshot.sequence);
    let cleared = mapper.snapshot();
    assert!(!cleared.overflow);
    assert!(cleared.transitions.is_empty());
    assert_eq!(
        cleared.overflow_count, 344,
        "the diagnostic counter is cumulative"
    );
}

#[test]
fn close_press_counter_is_lossless_across_transition_overflow_and_snapshots() {
    // Negative control: deriving close presses from the lossy transition ring drops presses after overflow.
    let mut mapper = Mapper::new(bindings(&[]));
    for _ in 0..300 {
        mapper.key(Key::Escape, true, false);
        mapper.key(Key::Escape, false, false);
    }
    let first = mapper.snapshot();
    assert!(first.overflow);
    assert_eq!(mapper.take_close_presses(), 300);
    assert_eq!(mapper.take_close_presses(), 0);
    // Snapshot/acknowledgement state is independent of the dedicated close signal.
    mapper.acknowledge(first.sequence);
    assert_eq!(mapper.take_close_presses(), 0);
}

#[test]
fn snapshots_are_non_consuming_and_acknowledgement_preserves_newer_events() {
    // Negative control: snapshot-time clearing or an ack that clears all current edges loses later input.
    let mut mapper = Mapper::new(bindings(&[("tap", &["KeyQ"])]));
    mapper.key(Key::KeyQ, true, false);
    let first = mapper.snapshot();
    let repeated = mapper.snapshot();
    assert!(action(&repeated, "tap").pressed);
    assert_eq!(repeated.transitions.len(), 1);

    mapper.key(Key::KeyQ, false, false);
    let second = mapper.snapshot();
    assert!(action(&second, "tap").pressed);
    assert!(action(&second, "tap").released);
    assert_eq!(second.transitions.len(), 2);

    mapper.acknowledge(first.sequence);
    let after_first_ack = mapper.snapshot();
    assert!(!action(&after_first_ack, "tap").pressed);
    assert!(action(&after_first_ack, "tap").released);
    assert_eq!(after_first_ack.transitions.len(), 1);
    assert_eq!(after_first_ack.transitions[0].edge, Edge::Release);

    // A stale sequence and a future/unknown sequence cannot clear the still-pending release.
    mapper.acknowledge(first.sequence);
    mapper.acknowledge(u128::MAX);
    let unchanged = mapper.snapshot();
    assert!(action(&unchanged, "tap").released);
    assert_eq!(unchanged.transitions.len(), 1);
    mapper.acknowledge(second.sequence);
    let clear = mapper.snapshot();
    assert!(!action(&clear, "tap").released);
    assert!(clear.transitions.is_empty());
}

#[test]
fn mapper_is_send_for_single_owner_transfer() {
    // Negative control: a non-Send mapper field breaks the compile-time ownership-transfer assertion.
    fn assert_send<T: Send>() {}
    assert_send::<Mapper>();
}

// ---------------------------------------------------------------------------------------------
// resync (CK-121): the final key states and the loss latches of a native layer that discarded ordered detail
// ---------------------------------------------------------------------------------------------

fn states(held: &[Key], pressed: &[Key], released: &[Key]) -> KeyStates {
    let mut states = KeyStates::new();
    for key in held {
        states.set_held(*key);
    }
    for key in pressed {
        states.set_pressed(*key);
    }
    for key in released {
        states.set_released(*key);
    }
    states
}

fn defaults() -> Mapper {
    Mapper::new(bindings(&[]))
}

#[test]
fn key_states_keep_three_sets_and_ignore_bits_beyond_the_supported_keys() {
    let mut all = KeyStates::new();
    all.set_held(Key::KeyA);
    all.set_pressed(Key::MetaRight);
    all.set_released(Key::Escape);
    assert!(all.is_held(Key::KeyA) && !all.is_held(Key::KeyB));
    assert!(all.was_pressed(Key::MetaRight) && !all.was_pressed(Key::KeyA));
    assert!(all.was_released(Key::Escape) && !all.was_released(Key::KeyA));
    let words = KeyStates::from_words([u64::MAX; 2], [u64::MAX; 2], [u64::MAX; 2]);
    for key in Key::ALL {
        assert!(words.is_held(key) && words.was_pressed(key) && words.was_released(key));
    }
    // 81 keys: bits 17.. of the second word are not keys.
    assert_eq!(
        words,
        KeyStates::from_words([!0, 0x1_FFFF], [!0, 0x1_FFFF], [!0, 0x1_FFFF])
    );
}

#[test]
fn a_resync_recomputes_every_action_from_the_final_states_alone() {
    // The mapper believed Escape and KeyA held; the final states say KeyD and KeyW are held and nothing else.
    let mut mapper = defaults();
    mapper.key(Key::Escape, true, false);
    mapper.key(Key::KeyA, true, false);
    let _ = mapper.take_close_presses();
    let first = mapper.snapshot();
    mapper.acknowledge(first.sequence);
    mapper.resync(&states(&[Key::KeyD, Key::KeyW], &[], &[]));
    let after = mapper.snapshot();
    assert!(!action(&after, "close").held && action(&after, "close").released);
    assert!(!action(&after, "move_left").held && action(&after, "move_left").released);
    assert!(action(&after, "move_right").held && action(&after, "move_right").pressed);
    assert!(action(&after, "move_up").held && action(&after, "move_up").pressed);
    assert!(after.overflow, "the snapshot reports the loss of detail");
    assert_eq!(after.overflow_count, 1);
    assert_eq!(
        mapper.take_close_presses(),
        0,
        "a release is not a close press"
    );
    // Later events are consistent with the recovered key set.
    mapper.key(Key::KeyD, false, false);
    assert!(!action(&mapper.snapshot(), "move_right").held);
    // A resync with the same states changes no action.
    let mut again = defaults();
    again.resync(&states(&[Key::KeyD], &[], &[]));
    let before = again.snapshot();
    again.acknowledge(before.sequence);
    again.resync(&states(&[Key::KeyD], &[], &[]));
    let same = again.snapshot();
    assert!(action(&same, "move_right").held && !action(&same, "move_right").pressed);
}

#[test]
fn a_tap_lost_on_an_unprefixed_binding_latches_pressed_and_released_and_requests_one_close() {
    // An Escape tap entirely inside the discarded span: up now, press and release latched.
    let mut mapper = defaults();
    mapper.resync(&states(&[], &[Key::Escape], &[Key::Escape]));
    let snapshot = mapper.snapshot();
    let close = action(&snapshot, "close");
    assert!(!close.held && close.pressed && close.released, "{close:?}");
    assert_eq!(
        mapper.take_close_presses(),
        1,
        "a complete Escape tap still requests a close"
    );
    // A non-close tap: only the latches for its action.
    mapper.resync(&states(&[], &[Key::KeyR], &[Key::KeyR]));
    let snapshot = mapper.snapshot();
    let reload = action(&snapshot, "reload_texture");
    assert!(!reload.held && reload.pressed && reload.released);
    assert_eq!(mapper.take_close_presses(), 0);
    // A key still held at the end: pressed, and held, but no release (on a fresh mapper, so no earlier latch is
    // mistaken for it).
    let mut held = defaults();
    held.resync(&states(&[Key::KeyR], &[Key::KeyR], &[]));
    let snapshot = held.snapshot();
    let reload = action(&snapshot, "reload_texture");
    assert!(
        reload.held && reload.pressed && !reload.released,
        "a tap whose key is still down latches no release: {reload:?}"
    );
    // Both rules on one action make one close press, not two: Escape held now and its latch set.
    let mut both = defaults();
    both.resync(&states(&[Key::Escape], &[Key::Escape], &[]));
    assert_eq!(both.take_close_presses(), 1);
}

#[test]
fn a_prefixed_binding_never_gets_a_press_or_a_tap_from_the_latches() {
    let chords = bindings(&[
        ("reload_texture", &["Control+KeyR"]),
        ("close", &["Control+KeyR"]),
    ]);
    // Schedule A (a stale modifier): Control was held and released, R tapped: nothing held, R's latches set.
    let mut mapper = Mapper::new(chords.clone());
    mapper.resync(&states(&[], &[Key::KeyR], &[Key::KeyR]));
    let snapshot = mapper.snapshot();
    for name in ["reload_texture", "close"] {
        let state = action(&snapshot, name);
        assert!(
            !state.held && !state.pressed && !state.released,
            "{name}: {state:?}"
        );
    }
    assert_eq!(
        mapper.take_close_presses(),
        0,
        "no close from a chord that was not proven"
    );
    // Schedule B (order unknown): R tapped, then Control held: Control held now, R up, latches set.
    let mut mapper = Mapper::new(chords.clone());
    mapper.resync(&states(&[Key::ControlLeft], &[Key::KeyR], &[Key::KeyR]));
    let snapshot = mapper.snapshot();
    assert!(!action(&snapshot, "reload_texture").pressed && !action(&snapshot, "close").pressed);
    assert_eq!(mapper.take_close_presses(), 0);
    // Both keys physically held now: the chord overlaps now, so it is pressed, and closes once.
    let mut mapper = Mapper::new(chords);
    mapper.resync(&states(&[Key::ControlLeft, Key::KeyR], &[Key::KeyR], &[]));
    let snapshot = mapper.snapshot();
    assert!(action(&snapshot, "reload_texture").held && action(&snapshot, "close").held);
    assert_eq!(mapper.take_close_presses(), 1);
}

#[test]
fn a_resync_after_the_unprefixed_binding_of_another_action_taps_does_not_touch_unrelated_actions() {
    let mut mapper = Mapper::new(bindings(&[
        ("jump", &["Space"]),
        ("fire", &["Control+Space"]),
    ]));
    mapper.resync(&states(&[], &[Key::Space], &[Key::Space]));
    let snapshot = mapper.snapshot();
    let jump = action(&snapshot, "jump");
    assert!(jump.pressed && jump.released && !jump.held);
    let fire = action(&snapshot, "fire");
    assert!(!fire.pressed && !fire.released);
    assert!(!action(&snapshot, "close").pressed);
}

#[test]
fn a_tap_lost_in_a_resync_survives_the_acknowledgement_of_an_earlier_snapshot() {
    // Ordinary tap -> snapshot -> resync tap -> old acknowledgement: the resync's tap is newer than the snapshot.
    // Negative control: a latch that keeps the older edge's stamp is cleared by that acknowledgement.
    let mut mapper = defaults();
    mapper.key(Key::KeyR, true, false);
    mapper.key(Key::KeyR, false, false);
    let first = mapper.snapshot();
    assert!(action(&first, "reload_texture").pressed && action(&first, "reload_texture").released);
    mapper.resync(&states(&[], &[Key::KeyR], &[Key::KeyR]));
    mapper.acknowledge(first.sequence);
    let after = mapper.snapshot();
    let reload = action(&after, "reload_texture");
    assert!(
        reload.pressed && reload.released,
        "the lost tap is still pending: {reload:?}"
    );
    mapper.acknowledge(after.sequence);
    let clear = mapper.snapshot();
    let reload = action(&clear, "reload_texture");
    assert!(!reload.pressed && !reload.released, "{reload:?}");
}

#[test]
fn two_lost_taps_with_a_snapshot_between_them_are_not_merged_by_the_old_acknowledgement() {
    // Resync tap -> snapshot -> resync tap -> acknowledgement of the first snapshot.
    let mut mapper = defaults();
    mapper.resync(&states(&[], &[Key::KeyR], &[Key::KeyR]));
    let first = mapper.snapshot();
    assert!(action(&first, "reload_texture").pressed && action(&first, "reload_texture").released);
    mapper.resync(&states(&[], &[Key::KeyR], &[Key::KeyR]));
    mapper.acknowledge(first.sequence);
    let after = mapper.snapshot();
    let reload = action(&after, "reload_texture");
    assert!(
        reload.pressed && reload.released,
        "the second lost tap is still pending: {reload:?}"
    );
    mapper.acknowledge(after.sequence);
    let clear = mapper.snapshot();
    let reload = action(&clear, "reload_texture");
    assert!(!reload.pressed && !reload.released, "{reload:?}");
    // A key still down at the second resync: the held tap latches a press but no release, and an old acknowledgement
    // leaves the press.
    let mut held = defaults();
    held.key(Key::KeyR, true, false);
    held.key(Key::KeyR, false, false);
    let old = held.snapshot();
    held.resync(&states(&[Key::KeyR], &[Key::KeyR], &[]));
    held.acknowledge(old.sequence);
    let after = held.snapshot();
    let reload = action(&after, "reload_texture");
    assert!(reload.held && reload.pressed, "{reload:?}");
    // The close rules are unchanged: one close per resync, however many latches.
    let mut close = defaults();
    close.resync(&states(&[Key::Escape], &[Key::Escape], &[]));
    close.resync(&states(&[], &[Key::Escape], &[Key::Escape]));
    assert_eq!(close.take_close_presses(), 2);
}
