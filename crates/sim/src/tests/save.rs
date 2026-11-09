use super::world::{
    Order, World, after, apply, expected, meta, plan, run_to, scheduler, simulation, submit_due,
};
use crate::{
    CanonicalError, CommandRecord, Date, Expected, GameTime, HashKind, Issuer, Save, SaveError,
    SaveMeta, Simulation, random::fnv1a, save::frame,
};

const MAGIC: &[u8] = b"CKSAVE\x00\x01";
const END: &[u8] = b"END\0";

/// A save taken at day 37 with one command pending and two in the journal.
fn simulation_at_day_37() -> Simulation<World, Order> {
    let plan = plan();
    let mut simulation = simulation(42);
    let mut next = 0;
    run_to(&mut simulation, &plan, &mut next, after(37, 0));
    submit_due(&mut simulation, &plan, &mut next);
    simulation
}

fn valid_save() -> Save<World> {
    simulation_at_day_37().save(meta(42))
}

fn valid_bytes() -> Vec<u8> {
    valid_save().to_bytes().unwrap()
}

fn load(bytes: &[u8]) -> Result<Save<World>, SaveError> {
    Save::from_bytes(bytes, &expected())
}

/// A section of a save file: its tag and its bytes.
type Section = ([u8; 4], Vec<u8>);

/// A change to a save that makes its commands inconsistent.
type Edit = fn(&mut Save<World>);

/// A save file taken apart as the format describes it, so a test can change one thing and put it back together.
struct Parts {
    header: String,
    sections: Vec<Section>,
}

fn take_apart(bytes: &[u8]) -> Parts {
    assert_eq!(&bytes[..8], MAGIC);
    let header_length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let header = String::from_utf8(bytes[12..12 + header_length].to_vec()).unwrap();
    let mut at = 12 + header_length;
    let mut sections = Vec::new();
    while &bytes[at..at + 4] != END {
        let tag: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        let length = usize::try_from(u64::from_le_bytes(
            bytes[at + 4..at + 12].try_into().unwrap(),
        ))
        .unwrap();
        sections.push((tag, bytes[at + 12..at + 12 + length].to_vec()));
        at += 12 + length + 8;
    }
    Parts { header, sections }
}

/// Puts the parts back together with every length and checksum correct.
fn put_together(parts: &Parts) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&u32::try_from(parts.header.len()).unwrap().to_le_bytes());
    out.extend_from_slice(parts.header.as_bytes());
    for (tag, payload) in &parts.sections {
        out.extend_from_slice(&frame(*tag, payload));
    }
    let checksum = fnv1a(&out);
    out.extend_from_slice(END);
    out.extend_from_slice(&checksum.to_le_bytes());
    out
}

fn with_header(edit: impl FnOnce(&str) -> String) -> Vec<u8> {
    let mut parts = take_apart(&valid_bytes());
    parts.header = edit(&parts.header);
    put_together(&parts)
}

fn with_sections(edit: impl FnOnce(&mut Vec<Section>)) -> Vec<u8> {
    let mut parts = take_apart(&valid_bytes());
    edit(&mut parts.sections);
    put_together(&parts)
}

fn section(sections: &mut [Section], tag: [u8; 4]) -> &mut Vec<u8> {
    &mut sections
        .iter_mut()
        .find(|(found, _)| *found == tag)
        .unwrap()
        .1
}

fn malformed(bytes: &[u8]) -> bool {
    matches!(load(bytes), Err(SaveError::MalformedHeader { .. }))
}

#[test]
fn the_file_starts_with_the_magic_and_ends_with_the_end_tag_and_a_checksum_of_everything_before() {
    let bytes = valid_bytes();
    assert_eq!(&bytes[..8], b"CKSAVE\x00\x01");
    let (body, end) = bytes.split_at(bytes.len() - 12);
    assert_eq!(&end[..4], b"END\0");
    assert_eq!(
        u64::from_le_bytes(end[4..].try_into().unwrap()),
        fnv1a(body)
    );
    let parts = take_apart(&bytes);
    assert_eq!(
        parts
            .sections
            .iter()
            .map(|(tag, _)| *tag)
            .collect::<Vec<_>>(),
        [*b"CLCK", *b"STAT", *b"PEND", *b"JRNL"]
    );
    // CLCK is the tick as a u64.
    assert_eq!(parts.sections[0].1, after(37, 0).hours().to_le_bytes());
}

#[test]
fn the_header_is_toml_with_the_documented_keys() {
    let header = take_apart(&valid_bytes()).header;
    let table: toml::Table = header.parse().unwrap();
    assert_eq!(table["format"].as_integer(), Some(1));
    assert_eq!(table["build"].as_str(), Some("test-build"));
    assert_eq!(table["scenario"].as_str(), Some("test-scenario"));
    assert_eq!(table["scenario_hash"].as_str(), Some("aa11"));
    assert_eq!(table["map_hash"].as_str(), Some("bb22"));
    assert_eq!(table["scripts_hash"].as_str(), Some("cc33"));
    assert_eq!(
        table["tick"].as_integer(),
        Some(i64::try_from(after(37, 0).hours()).unwrap())
    );
    assert_eq!(table["seed"].as_str(), Some("000000000000002a"));
    let start = table["start"].as_table().unwrap();
    assert_eq!(start["year"].as_integer(), Some(768));
    assert_eq!(start["month"].as_integer(), Some(1));
    assert_eq!(start["day"].as_integer(), Some(1));
    assert_eq!(start["hour"].as_integer(), Some(0));
}

#[test]
fn a_save_round_trips_and_re_encodes_to_the_same_bytes() {
    let save = valid_save();
    assert_eq!(save.pending.len(), 1);
    assert!(!save.journal.is_empty());
    let bytes = save.to_bytes().unwrap();
    let loaded = load(&bytes).unwrap();
    assert_eq!(loaded, save);
    assert_eq!(loaded.to_bytes().unwrap(), bytes);
    assert_eq!(loaded.digest().unwrap(), save.digest().unwrap());
}

#[test]
fn a_save_of_an_empty_history_round_trips() {
    let save = simulation(5).save(meta(5));
    assert_eq!(load(&save.to_bytes().unwrap()).unwrap(), save);
}

#[test]
fn the_seed_is_sixteen_lower_case_hex_digits_and_round_trips() {
    for (seed, text) in [
        (0, "0000000000000000"),
        (42, "000000000000002a"),
        (1 << 63, "8000000000000000"),
        (u64::MAX, "ffffffffffffffff"),
    ] {
        let bytes = simulation(seed).save(meta(seed)).to_bytes().unwrap();
        assert!(
            take_apart(&bytes)
                .header
                .contains(&format!("seed = \"{text}\"")),
            "{text}"
        );
        assert_eq!(load(&bytes).unwrap().meta.seed, seed);
    }
}

#[test]
fn any_other_form_of_the_seed_is_a_malformed_header() {
    for replacement in [
        "seed = \"2a\"",
        "seed = \"000000000000002A\"",
        "seed = \"0x000000000000002a\"",
        "seed = \"+00000000000002a\"",
        "seed = \"-00000000000002a\"",
        "seed = \"000000000000002g\"",
        "seed = \"0000000000000002a\"",
        "seed = \"\"",
        "seed = 42",
    ] {
        let bytes =
            with_header(|header| header.replace("seed = \"000000000000002a\"", replacement));
        assert!(malformed(&bytes), "{replacement}");
    }
}

#[test]
fn a_loaded_simulation_is_exactly_what_was_saved() {
    let original = simulation_at_day_37();
    let save = load(&original.save(meta(42)).to_bytes().unwrap()).unwrap();
    let loaded = Simulation::from_save(save, scheduler(None), apply).unwrap();
    assert_eq!(loaded.time(), original.time());
    assert_eq!(loaded.seed(), 42);
    assert_eq!(loaded.state(), original.state());
    assert_eq!(loaded.pending_len(), 1);
    assert_eq!(loaded.journal(), original.journal());
    assert_eq!(loaded.digest().unwrap(), original.digest().unwrap());
}

#[test]
fn the_digest_leaves_out_the_journal_and_covers_the_clock_state_and_pending_commands() {
    let original = simulation_at_day_37();
    let base = original.digest().unwrap();

    let mut save = original.save(meta(42));
    save.journal.clear();
    assert_eq!(save.digest().unwrap(), base);

    let mut changed = original.save(meta(42));
    changed.time = GameTime::from_hours(changed.time.hours() + 1);
    assert_ne!(changed.digest().unwrap(), base);
    let mut changed = original.save(meta(42));
    changed.state.hourly += 1;
    assert_ne!(changed.digest().unwrap(), base);
    let mut changed = original.save(meta(42));
    changed.pending.clear();
    assert_ne!(changed.digest().unwrap(), base);
}

#[test]
fn the_first_and_last_tick_are_saved_and_the_hour_after_is_refused() {
    let at = |hours: u64| {
        let mut save = simulation(1).save(meta(1));
        save.time = GameTime::from_hours(hours);
        save
    };
    for hours in [0, 87_651_215] {
        assert_eq!(
            load(&at(hours).to_bytes().unwrap()).unwrap().time.hours(),
            hours
        );
    }
    assert!(matches!(
        at(87_651_216).to_bytes(),
        Err(SaveError::MalformedHeader { .. })
    ));
    // A header tick outside 0 to 87,651,215 is refused on reading too.
    let tick = format!("tick = {}", after(37, 0).hours());
    for bad in ["tick = -1", "tick = 87651216", "tick = 9223372036854775807"] {
        assert!(
            malformed(&with_header(|header| header.replace(&tick, bad))),
            "{bad}"
        );
    }
}

#[test]
fn bad_magic_is_refused() {
    let mut bytes = valid_bytes();
    bytes[0] = b'X';
    assert_eq!(load(&bytes), Err(SaveError::BadMagic));
    let mut bytes = valid_bytes();
    bytes[7] = 2;
    assert_eq!(load(&bytes), Err(SaveError::BadMagic));
    assert_eq!(load(b"not a save at all"), Err(SaveError::BadMagic));
}

#[test]
fn an_unknown_format_is_refused() {
    let bytes = with_header(|header| header.replace("format = 1", "format = 2"));
    assert_eq!(load(&bytes), Err(SaveError::UnknownFormat { found: 2 }));
}

#[test]
fn a_different_build_is_refused() {
    let other = Expected {
        build: "another-build".to_owned(),
        ..expected()
    };
    assert_eq!(
        Save::<World>::from_bytes(&valid_bytes(), &other),
        Err(SaveError::BuildMismatch {
            expected: "another-build".to_owned(),
            found: "test-build".to_owned()
        })
    );
}

#[test]
fn each_hash_mismatch_names_the_hash() {
    for (which, edit) in [
        (
            HashKind::Scenario,
            (|e: &mut Expected| e.scenario_hash = "dead".to_owned()) as fn(&mut Expected),
        ),
        (HashKind::Map, |e| e.map_hash = "dead".to_owned()),
        (HashKind::Scripts, |e| e.scripts_hash = "dead".to_owned()),
        (HashKind::Rules, |e| e.rules_hash = "dead".to_owned()),
    ] {
        let mut wanted = expected();
        edit(&mut wanted);
        let Err(SaveError::HashMismatch {
            which: found,
            expected,
            ..
        }) = Save::<World>::from_bytes(&valid_bytes(), &wanted)
        else {
            panic!("{which:?} was not refused");
        };
        assert_eq!(found, which);
        assert_eq!(expected, "dead");
    }
}

#[test]
fn a_malformed_header_is_refused() {
    let header = take_apart(&valid_bytes()).header;
    let cases: Vec<(&str, String)> = vec![
        ("not toml", "this is = = not toml".to_owned()),
        (
            "missing key",
            header.replace("build = \"test-build\"\n", ""),
        ),
        ("unknown key", format!("extra = 1\n{header}")),
        (
            "a string tick",
            header.replace(&format!("tick = {}", after(37, 0).hours()), "tick = \"x\""),
        ),
        ("upper-case hash", header.replace("aa11", "AA11")),
        ("a non-hex hash", header.replace("bb22", "xx22")),
        ("an empty hash", header.replace("cc33", "")),
        (
            "a day that does not exist",
            header
                .replace("year = 768", "year = 769")
                .replace("day = 1", "day = 29")
                .replace("month = 1", "month = 2"),
        ),
        ("year 0", header.replace("year = 768", "year = 0")),
        ("hour 24", header.replace("hour = 0", "hour = 24")),
        (
            "a year over 65535",
            header.replace("year = 768", "year = 70000"),
        ),
    ];
    for (name, text) in cases {
        let mut parts = take_apart(&valid_bytes());
        parts.header = text;
        assert!(malformed(&put_together(&parts)), "{name}");
    }
    // A header that is not UTF-8 at all.
    let mut parts = take_apart(&valid_bytes());
    parts.header = "x".to_owned();
    let mut bytes = put_together(&parts);
    bytes[12] = 0xff;
    assert!(malformed(&bytes));
}

#[test]
fn an_m8_save_without_rules_hash_is_refused_by_the_missing_field_name() {
    let legacy = with_header(|header| header.replace("rules_hash = \"dd44\"\n", ""));
    let Err(SaveError::MalformedHeader { reason }) = load(&legacy) else {
        panic!("the M8-shaped header without rules_hash was not refused");
    };
    assert!(
        reason.contains("rules_hash"),
        "refusal should name the missing field: {reason}"
    );
}

#[test]
fn a_header_tick_that_disagrees_with_the_clock_is_refused() {
    let bytes = with_header(|header| {
        header.replace(
            &format!("tick = {}", after(37, 0).hours()),
            &format!("tick = {}", after(37, 0).hours() + 1),
        )
    });
    assert_eq!(
        load(&bytes),
        Err(SaveError::TickMismatch {
            header: after(37, 0).hours() + 1,
            clock: after(37, 0).hours()
        })
    );
}

#[test]
fn a_missing_section_is_refused_by_name() {
    for tag in [*b"CLCK", *b"STAT", *b"PEND", *b"JRNL"] {
        let bytes = with_sections(|sections| sections.retain(|(found, _)| *found != tag));
        assert_eq!(
            load(&bytes),
            Err(SaveError::MissingSection { tag }),
            "{tag:?}"
        );
    }
}

#[test]
fn sections_out_of_order_are_refused() {
    let bytes = with_sections(|sections| sections.swap(1, 2));
    assert_eq!(
        load(&bytes),
        Err(SaveError::SectionOutOfOrder {
            expected: *b"STAT",
            found: *b"PEND"
        })
    );
}

#[test]
fn an_unknown_section_is_refused() {
    let bytes = with_sections(|sections| sections.push((*b"XTRA", vec![1, 2, 3])));
    assert_eq!(
        load(&bytes),
        Err(SaveError::UnknownSection { tag: *b"XTRA" })
    );
    let bytes = with_sections(|sections| sections[3].0 = *b"JRNX");
    assert_eq!(
        load(&bytes),
        Err(SaveError::UnknownSection { tag: *b"JRNX" })
    );
}

#[test]
fn a_duplicate_section_is_refused() {
    let bytes = with_sections(|sections| {
        let again = sections[2].clone();
        sections.push(again);
    });
    assert_eq!(
        load(&bytes),
        Err(SaveError::DuplicateSection { tag: *b"PEND" })
    );
}

#[test]
fn a_changed_byte_in_any_section_is_a_checksum_mismatch() {
    let bytes = valid_bytes();
    let parts = take_apart(&bytes);
    let mut at = 12 + parts.header.len();
    for (tag, payload) in &parts.sections {
        let mut altered = bytes.clone();
        altered[at + 12] ^= 1;
        assert_eq!(
            load(&altered),
            Err(SaveError::ChecksumMismatch { tag: *tag })
        );
        at += 12 + payload.len() + 8;
    }
}

#[test]
fn a_changed_section_checksum_is_a_checksum_mismatch() {
    let bytes = valid_bytes();
    let parts = take_apart(&bytes);
    let at = 12 + parts.header.len() + 12 + parts.sections[0].1.len();
    let mut altered = bytes;
    altered[at] ^= 1;
    assert_eq!(
        load(&altered),
        Err(SaveError::ChecksumMismatch { tag: *b"CLCK" })
    );
}

#[test]
fn every_truncation_is_refused() {
    let bytes = valid_bytes();
    for length in 0..bytes.len() {
        let result = load(&bytes[..length]);
        assert!(
            matches!(result, Err(SaveError::Truncated | SaveError::BadMagic)),
            "cut at {length}: {result:?}"
        );
    }
    assert_eq!(load(&bytes[..bytes.len() - 1]), Err(SaveError::Truncated));
    assert_eq!(load(&bytes[..3]), Err(SaveError::Truncated));
}

#[test]
fn a_bad_end_checksum_is_refused() {
    let mut bytes = valid_bytes();
    *bytes.last_mut().unwrap() ^= 1;
    assert_eq!(load(&bytes), Err(SaveError::BadEndChecksum));
    // So is a change the section checksums cannot see: in the header.
    let mut bytes = valid_bytes();
    let at = bytes
        .windows(13)
        .position(|window| window == b"test-scenario")
        .unwrap();
    bytes[at] ^= 0x20;
    assert_eq!(load(&bytes), Err(SaveError::BadEndChecksum));
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = valid_bytes();
    bytes.extend_from_slice(&[0, 0, 0]);
    assert_eq!(load(&bytes), Err(SaveError::TrailingBytes { count: 3 }));
}

#[test]
fn a_section_that_does_not_decode_is_refused_by_name() {
    let short = with_sections(|sections| {
        section(sections, *b"STAT").truncate(5);
    });
    assert!(matches!(
        load(&short),
        Err(SaveError::Decode { section, error: CanonicalError::ShortInput { .. } }) if section == *b"STAT"
    ));

    let trailing = with_sections(|sections| section(sections, *b"STAT").push(0));
    assert_eq!(
        load(&trailing),
        Err(SaveError::Decode {
            section: *b"STAT",
            error: CanonicalError::TrailingBytes { count: 1 }
        })
    );

    let clock = with_sections(|sections| section(sections, *b"CLCK").push(0));
    assert!(matches!(
        load(&clock),
        Err(SaveError::Decode { section, .. }) if section == *b"CLCK"
    ));

    // The pending record's issuer byte sits after the u32 count, the time and the sequence: 4 + 8 + 8.
    let issuer = with_sections(|sections| section(sections, *b"PEND")[20] = 9);
    assert_eq!(
        load(&issuer),
        Err(SaveError::Decode {
            section: *b"PEND",
            error: CanonicalError::BadTag {
                ty: "Issuer",
                tag: 9
            }
        })
    );

    let journal = with_sections(|sections| section(sections, *b"JRNL").truncate(10));
    assert!(matches!(
        load(&journal),
        Err(SaveError::Decode { section, .. }) if section == *b"JRNL"
    ));
}

#[test]
fn commands_that_are_not_one_history_are_refused_on_writing_and_on_reading() {
    let corrupt = |edit: Edit| {
        let mut save = valid_save();
        edit(&mut save);
        save
    };
    let cases: [(&str, Edit); 5] = [
        ("a gap in the sequence", |save| {
            save.journal[1].sequence += 1;
        }),
        ("a repeated sequence", |save| save.journal[1].sequence = 0),
        ("time going backwards", |save| {
            save.journal[1].time = GameTime::from_hours(0);
        }),
        ("a command after the clock", |save| {
            save.journal[0].time = GameTime::from_hours(u64::MAX / 2);
        }),
        ("a pending command from an earlier tick", |save| {
            save.pending[0].time = save.journal[0].time;
        }),
    ];
    for (name, edit) in cases {
        let save = corrupt(edit);
        assert!(
            matches!(save.to_bytes(), Err(SaveError::InconsistentCommands { .. })),
            "{name} (write)"
        );
        // Put the same history into a file anyway, with correct checksums, and the reader refuses it too.
        let mut parts = take_apart(&valid_bytes());
        let mut journal = Vec::new();
        crate::Canonical::encode(&save.journal, &mut journal).unwrap();
        let mut pending = Vec::new();
        crate::Canonical::encode(&save.pending, &mut pending).unwrap();
        *section(&mut parts.sections, *b"JRNL") = journal;
        *section(&mut parts.sections, *b"PEND") = pending;
        assert!(
            matches!(
                load(&put_together(&parts)),
                Err(SaveError::InconsistentCommands { .. })
            ),
            "{name} (read)"
        );
    }
}

#[test]
fn a_pending_command_that_is_not_a_command_is_refused_when_the_simulation_is_built() {
    let mut save = valid_save();
    save.pending[0].bytes = vec![9];
    let sequence = save.pending[0].sequence;
    assert!(matches!(
        Simulation::from_save(save, scheduler(None), apply),
        Err(SaveError::Command { sequence: found, error: CanonicalError::BadTag { ty: "Order", tag: 9 } }) if found == sequence
    ));
}

#[test]
fn a_hash_that_is_not_lower_case_hex_cannot_be_written() {
    for bad in ["", "AA11", "xyz"] {
        let meta = SaveMeta {
            scenario_hash: bad.to_owned(),
            ..meta(1)
        };
        assert!(matches!(
            simulation(1).save(meta).to_bytes(),
            Err(SaveError::MalformedHeader { .. })
        ));
    }
    let meta = SaveMeta {
        start: Date {
            year: 769,
            month: 2,
            day: 29,
            hour: 0,
        },
        ..meta(1)
    };
    assert!(matches!(
        simulation(1).save(meta).to_bytes(),
        Err(SaveError::MalformedHeader { .. })
    ));
}

#[test]
fn odd_text_in_the_header_strings_survives() {
    let meta = SaveMeta {
        build: "build \"1\"\n\\ é \u{1f600}".to_owned(),
        scenario: "line one\nline two\t\"quoted\"".to_owned(),
        ..meta(1)
    };
    let wanted = Expected {
        build: meta.build.clone(),
        ..expected()
    };
    let save = simulation(1).save(meta);
    let loaded = Save::<World>::from_bytes(&save.to_bytes().unwrap(), &wanted).unwrap();
    assert_eq!(loaded, save);
}

#[test]
fn a_journal_record_keeps_the_issuer_and_the_exact_command_bytes() {
    let simulation = simulation_at_day_37();
    let record: &CommandRecord = &simulation.journal()[0];
    assert_eq!(record.issuer, Issuer::Player);
    assert_eq!(record.sequence, 0);
    assert_eq!(record.time, after(3, 5));
    assert_eq!(
        record.bytes,
        crate::Canonical::to_bytes(&Order::Set("a".to_owned(), 1)).unwrap()
    );
}
