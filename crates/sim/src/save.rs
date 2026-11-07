//! The save file: a TOML header and checksummed binary sections, and the digest of the state it holds.
//!
//! ```text
//! "CKSAVE" 0x00 0x01
//! u32 length, then the UTF-8 TOML header
//! "CLCK", u64 length, the tick, u64 FNV-1a checksum
//! "STAT", u64 length, the state, u64 checksum
//! "PEND", u64 length, the pending commands, u64 checksum
//! "JRNL", u64 length, the journal, u64 checksum
//! "END\0", u64 FNV-1a checksum of everything before it
//! ```
//!
//! Loading either gives the whole save or a [`SaveError`] naming what is wrong; nothing loads halfway.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    calendar::{Date, GameTime},
    canonical::{Canonical, CanonicalError, Reader, encode_length},
    random::{Fnv1a, fnv1a},
    simulation::CommandRecord,
};

const MAGIC: [u8; 8] = *b"CKSAVE\x00\x01";
const FORMAT: i64 = 1;
const CLOCK: [u8; 4] = *b"CLCK";
const STATE: [u8; 4] = *b"STAT";
const PENDING: [u8; 4] = *b"PEND";
const JOURNAL: [u8; 4] = *b"JRNL";
const END: [u8; 4] = *b"END\0";
const SECTION_ORDER: [[u8; 4]; 4] = [CLOCK, STATE, PENDING, JOURNAL];

/// The header of a save: what identifies the game it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveMeta {
    /// The build id. A save loads only into the build that wrote it.
    pub build: String,
    /// The scenario id.
    pub scenario: String,
    /// The scenario content hash: lower-case hexadecimal, supplied by the caller.
    pub scenario_hash: String,
    /// The map content hash: lower-case hexadecimal, supplied by the caller.
    pub map_hash: String,
    /// The gameplay scripts' content hash: lower-case hexadecimal, supplied by the caller.
    pub scripts_hash: String,
    /// The date the scenario starts on.
    pub start: Date,
    /// The master seed.
    pub seed: u64,
}

/// What a loader insists on: the build and the content hashes of what it has loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The loader's build id.
    pub build: String,
    /// The loaded scenario's content hash.
    pub scenario_hash: String,
    /// The loaded map's content hash.
    pub map_hash: String,
    /// The loaded gameplay scripts' content hash.
    pub scripts_hash: String,
}

/// Which content hash disagreed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashKind {
    /// The scenario hash.
    Scenario,
    /// The map hash.
    Map,
    /// The gameplay scripts hash.
    Scripts,
}

impl fmt::Display for HashKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Scenario => "scenario",
            Self::Map => "map",
            Self::Scripts => "scripts",
        })
    }
}

/// Why a save could not be written or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file does not start with the save magic.
    BadMagic,
    /// The header's format is not one this build reads.
    UnknownFormat {
        /// The format the header names.
        found: i64,
    },
    /// The save was written by a different build.
    BuildMismatch {
        /// The loader's build.
        expected: String,
        /// The save's build.
        found: String,
    },
    /// A content hash differs from the loaded content.
    HashMismatch {
        /// Which hash.
        which: HashKind,
        /// The loader's hash.
        expected: String,
        /// The save's hash.
        found: String,
    },
    /// The header is not valid TOML of the expected shape, or holds an impossible value.
    MalformedHeader {
        /// What is wrong.
        reason: String,
    },
    /// The header's tick is not the clock section's tick.
    TickMismatch {
        /// The header's tick.
        header: u64,
        /// The clock section's tick.
        clock: u64,
    },
    /// A section is missing.
    MissingSection {
        /// The missing section's tag.
        tag: [u8; 4],
    },
    /// The sections are not in the required order.
    SectionOutOfOrder {
        /// The tag that belonged here.
        expected: [u8; 4],
        /// The tag found.
        found: [u8; 4],
    },
    /// A section has a tag this format does not have.
    UnknownSection {
        /// The unknown tag.
        tag: [u8; 4],
    },
    /// A section appears twice.
    DuplicateSection {
        /// The repeated tag.
        tag: [u8; 4],
    },
    /// A section's bytes do not match its checksum.
    ChecksumMismatch {
        /// The section's tag.
        tag: [u8; 4],
    },
    /// The file ends early.
    Truncated,
    /// The bytes before the end tag do not match the end checksum.
    BadEndChecksum,
    /// Bytes follow the end checksum.
    TrailingBytes {
        /// How many.
        count: usize,
    },
    /// A section's contents do not decode.
    Decode {
        /// The section's tag.
        section: [u8; 4],
        /// What was wrong.
        error: CanonicalError,
    },
    /// The journal and pending commands are not one consistent history: sequence numbers must run from 0 without a gap,
    /// times must not go backwards or past the clock, and pending commands wait at the clock's own tick.
    InconsistentCommands {
        /// The sequence number of the first entry that breaks the rule.
        sequence: u64,
    },
    /// A pending command's bytes do not decode as a command.
    Command {
        /// The command's sequence number.
        sequence: u64,
        /// What was wrong.
        error: CanonicalError,
    },
    /// The state could not be encoded.
    Encode(CanonicalError),
}

fn tag_name(tag: [u8; 4]) -> String {
    String::from_utf8_lossy(&tag)
        .trim_end_matches('\0')
        .to_owned()
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "this is not a save file"),
            Self::UnknownFormat { found } => write!(f, "save format {found} is not supported"),
            Self::BuildMismatch { expected, found } => {
                write!(f, "the save is from build `{found}`, not `{expected}`")
            }
            Self::HashMismatch {
                which,
                expected,
                found,
            } => write!(
                f,
                "the save's {which} hash `{found}` is not the loaded `{expected}`"
            ),
            Self::MalformedHeader { reason } => write!(f, "the save header is malformed: {reason}"),
            Self::TickMismatch { header, clock } => {
                write!(
                    f,
                    "the header says tick {header} but the clock section says {clock}"
                )
            }
            Self::MissingSection { tag } => write!(f, "the {} section is missing", tag_name(*tag)),
            Self::SectionOutOfOrder { expected, found } => write!(
                f,
                "the {} section comes where {} belongs",
                tag_name(*found),
                tag_name(*expected)
            ),
            Self::UnknownSection { tag } => write!(f, "`{}` is not a section", tag_name(*tag)),
            Self::DuplicateSection { tag } => {
                write!(f, "the {} section appears twice", tag_name(*tag))
            }
            Self::ChecksumMismatch { tag } => {
                write!(
                    f,
                    "the {} section does not match its checksum",
                    tag_name(*tag)
                )
            }
            Self::Truncated => write!(f, "the save ends early"),
            Self::BadEndChecksum => write!(f, "the save does not match its end checksum"),
            Self::TrailingBytes { count } => write!(f, "{count} bytes follow the save"),
            Self::Decode { section, error } => {
                write!(
                    f,
                    "the {} section does not decode: {error}",
                    tag_name(*section)
                )
            }
            Self::InconsistentCommands { sequence } => {
                write!(f, "command {sequence} breaks the order of the journal")
            }
            Self::Command { sequence, error } => {
                write!(f, "pending command {sequence} does not decode: {error}")
            }
            Self::Encode(error) => write!(f, "the state cannot be encoded: {error}"),
        }
    }
}

impl Error for SaveError {}

/// A save: the header, the tick, the state, the commands waiting for the next boundary and the journal of those applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Save<S> {
    /// The header.
    pub meta: SaveMeta,
    /// The tick the save was taken at, a boundary: the hour that runs next.
    pub time: GameTime,
    /// The simulation state.
    pub state: S,
    /// The commands queued for the next boundary.
    pub pending: Vec<CommandRecord>,
    /// Every command applied so far, in order.
    pub journal: Vec<CommandRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderToml {
    format: i64,
    build: String,
    scenario: String,
    scenario_hash: String,
    map_hash: String,
    scripts_hash: String,
    tick: i64,
    seed: String,
    start: StartToml,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartToml {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
}

#[derive(Deserialize)]
struct FormatProbe {
    format: i64,
}

fn malformed(reason: impl fmt::Display) -> SaveError {
    SaveError::MalformedHeader {
        reason: reason.to_string(),
    }
}

fn is_lower_hex(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The seed as exactly 16 lower-case hexadecimal digits: TOML integers are signed, and a seed uses the full `u64`.
fn seed_text(seed: u64) -> String {
    format!("{seed:016x}")
}

fn parse_seed(text: &str) -> Result<u64, SaveError> {
    if text.len() != 16 || !is_lower_hex(text) {
        return Err(malformed(format!(
            "seed `{text}` is not 16 lower-case hexadecimal digits"
        )));
    }
    u64::from_str_radix(text, 16).map_err(malformed)
}

fn check_meta(meta: &SaveMeta, tick: u64) -> Result<(), SaveError> {
    for (name, hash) in [
        ("scenario_hash", &meta.scenario_hash),
        ("map_hash", &meta.map_hash),
        ("scripts_hash", &meta.scripts_hash),
    ] {
        if !is_lower_hex(hash) {
            return Err(malformed(format!("{name} is not lower-case hexadecimal")));
        }
    }
    GameTime::from_date(meta.start).map_err(malformed)?;
    GameTime::from_hours(tick).date().map_err(malformed)?;
    Ok(())
}

fn write_header(meta: &SaveMeta, tick: u64) -> Result<String, SaveError> {
    check_meta(meta, tick)?;
    let header = HeaderToml {
        format: FORMAT,
        build: meta.build.clone(),
        scenario: meta.scenario.clone(),
        scenario_hash: meta.scenario_hash.clone(),
        map_hash: meta.map_hash.clone(),
        scripts_hash: meta.scripts_hash.clone(),
        tick: i64::try_from(tick).map_err(malformed)?,
        seed: seed_text(meta.seed),
        start: StartToml {
            year: meta.start.year,
            month: meta.start.month,
            day: meta.start.day,
            hour: meta.start.hour,
        },
    };
    toml::to_string(&header).map_err(malformed)
}

fn read_header(bytes: &[u8]) -> Result<(SaveMeta, u64), SaveError> {
    let text = str::from_utf8(bytes).map_err(malformed)?;
    let probe: FormatProbe = toml::from_str(text).map_err(malformed)?;
    if probe.format != FORMAT {
        return Err(SaveError::UnknownFormat {
            found: probe.format,
        });
    }
    let header: HeaderToml = toml::from_str(text).map_err(malformed)?;
    let tick = u64::try_from(header.tick).map_err(malformed)?;
    let meta = SaveMeta {
        build: header.build,
        scenario: header.scenario,
        scenario_hash: header.scenario_hash,
        map_hash: header.map_hash,
        scripts_hash: header.scripts_hash,
        start: Date {
            year: header.start.year,
            month: header.start.month,
            day: header.start.day,
            hour: header.start.hour,
        },
        seed: parse_seed(&header.seed)?,
    };
    check_meta(&meta, tick)?;
    Ok((meta, tick))
}

/// A section as written: its tag, the `u64` length, the bytes and their `u64` FNV-1a checksum.
pub(crate) fn frame(tag: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 20);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&fnv1a(payload).to_le_bytes());
    out
}

fn encode_records<'a>(
    records: impl ExactSizeIterator<Item = &'a CommandRecord>,
) -> Result<Vec<u8>, CanonicalError> {
    let mut out = Vec::new();
    encode_length(records.len(), &mut out)?;
    for record in records {
        record.encode(&mut out)?;
    }
    Ok(out)
}

/// The digest of a state: the FNV-1a hash of the framed `CLCK`, `STAT` and `PEND` sections, in order.
pub(crate) fn state_digest<'a, S: Canonical>(
    time: GameTime,
    state: &S,
    pending: impl ExactSizeIterator<Item = &'a CommandRecord>,
) -> Result<u64, CanonicalError> {
    let mut hash = Fnv1a::new();
    hash.update(&frame(CLOCK, &time.to_bytes()?));
    hash.update(&frame(STATE, &state.to_bytes()?));
    hash.update(&frame(PENDING, &encode_records(pending)?));
    Ok(hash.finish())
}

impl<S> Save<S> {
    /// Checks that the journal and the pending commands are one consistent history.
    pub(crate) fn check_commands(&self) -> Result<(), SaveError> {
        let mut previous = GameTime::from_hours(0);
        let journal = self.journal.iter().map(|record| (record, false));
        let pending = self.pending.iter().map(|record| (record, true));
        for (index, (record, is_pending)) in journal.chain(pending).enumerate() {
            let in_order = u64::try_from(index) == Ok(record.sequence)
                && record.time >= previous
                && record.time <= self.time
                && (!is_pending || record.time == self.time);
            if !in_order {
                return Err(SaveError::InconsistentCommands {
                    sequence: record.sequence,
                });
            }
            previous = record.time;
        }
        Ok(())
    }
}

impl<S: Canonical> Save<S> {
    /// The state digest of this save: see [`crate::Simulation::digest`].
    ///
    /// # Errors
    /// Returns [`CanonicalError::TooLong`] when the state is too long to encode.
    pub fn digest(&self) -> Result<u64, CanonicalError> {
        state_digest(self.time, &self.state, self.pending.iter())
    }

    /// The save file's bytes.
    ///
    /// # Errors
    /// Returns [`SaveError::MalformedHeader`] when the header holds a value a loader would refuse,
    /// [`SaveError::InconsistentCommands`] when the commands are not one history and [`SaveError::Encode`] when the state
    /// cannot be encoded.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SaveError> {
        self.check_commands()?;
        let header = write_header(&self.meta, self.time.hours())?;
        let header_length = u32::try_from(header.len()).map_err(malformed)?;
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&header_length.to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&frame(
            CLOCK,
            &self.time.to_bytes().map_err(SaveError::Encode)?,
        ));
        out.extend_from_slice(&frame(
            STATE,
            &self.state.to_bytes().map_err(SaveError::Encode)?,
        ));
        out.extend_from_slice(&frame(
            PENDING,
            &encode_records(self.pending.iter()).map_err(SaveError::Encode)?,
        ));
        out.extend_from_slice(&frame(
            JOURNAL,
            &encode_records(self.journal.iter()).map_err(SaveError::Encode)?,
        ));
        let checksum = fnv1a(&out);
        out.extend_from_slice(&END);
        out.extend_from_slice(&checksum.to_le_bytes());
        Ok(out)
    }

    /// Reads a save, insisting it belongs to the `expected` build and content.
    ///
    /// # Errors
    /// Returns the [`SaveError`] naming what is wrong; nothing is returned for a file with any fault.
    pub fn from_bytes(bytes: &[u8], expected: &Expected) -> Result<Self, SaveError> {
        if !bytes.starts_with(&MAGIC) {
            return Err(if MAGIC.starts_with(bytes) {
                SaveError::Truncated
            } else {
                SaveError::BadMagic
            });
        }
        let mut input = Reader::new(&bytes[MAGIC.len()..]);
        let header_length = input.length().map_err(|_| SaveError::Truncated)?;
        let header = input
            .take(header_length)
            .map_err(|_| SaveError::Truncated)?;
        let (meta, header_tick) = read_header(header)?;
        if meta.build != expected.build {
            return Err(SaveError::BuildMismatch {
                expected: expected.build.clone(),
                found: meta.build,
            });
        }
        for (which, wanted, found) in [
            (
                HashKind::Scenario,
                &expected.scenario_hash,
                &meta.scenario_hash,
            ),
            (HashKind::Map, &expected.map_hash, &meta.map_hash),
            (
                HashKind::Scripts,
                &expected.scripts_hash,
                &meta.scripts_hash,
            ),
        ] {
            if wanted != found {
                return Err(SaveError::HashMismatch {
                    which,
                    expected: wanted.clone(),
                    found: found.clone(),
                });
            }
        }

        let mut sections: Vec<([u8; 4], &[u8])> = Vec::new();
        let end_start = loop {
            let before = bytes.len() - input.remaining();
            let tag = input.take_array::<4>().map_err(|_| SaveError::Truncated)?;
            if tag == END {
                break before;
            }
            let length = u64::decode(&mut input).map_err(|_| SaveError::Truncated)?;
            let length = usize::try_from(length).map_err(|_| SaveError::Truncated)?;
            let payload = input.take(length).map_err(|_| SaveError::Truncated)?;
            let checksum = u64::decode(&mut input).map_err(|_| SaveError::Truncated)?;
            if checksum != fnv1a(payload) {
                return Err(SaveError::ChecksumMismatch { tag });
            }
            sections.push((tag, payload));
        };
        let [clock, state, pending, journal] = order_sections(&sections)?;
        let end_checksum = u64::decode(&mut input).map_err(|_| SaveError::Truncated)?;
        if end_checksum != fnv1a(&bytes[..end_start]) {
            return Err(SaveError::BadEndChecksum);
        }
        input.finish().map_err(|error| match error {
            CanonicalError::TrailingBytes { count } => SaveError::TrailingBytes { count },
            _ => SaveError::Truncated,
        })?;

        let time = decode_section::<GameTime>(CLOCK, clock)?;
        if time.hours() != header_tick {
            return Err(SaveError::TickMismatch {
                header: header_tick,
                clock: time.hours(),
            });
        }
        let save = Self {
            meta,
            time,
            state: decode_section(STATE, state)?,
            pending: decode_section(PENDING, pending)?,
            journal: decode_section(JOURNAL, journal)?,
        };
        save.check_commands()?;
        Ok(save)
    }
}

fn decode_section<T: Canonical>(section: [u8; 4], bytes: &[u8]) -> Result<T, SaveError> {
    T::from_bytes(bytes).map_err(|error| SaveError::Decode { section, error })
}

/// The four sections' bytes in the required order, or the fault in the sections found.
fn order_sections<'a>(sections: &[([u8; 4], &'a [u8])]) -> Result<[&'a [u8]; 4], SaveError> {
    for (index, (tag, _)) in sections.iter().enumerate() {
        if !SECTION_ORDER.contains(tag) {
            return Err(SaveError::UnknownSection { tag: *tag });
        }
        if sections[..index].iter().any(|(earlier, _)| earlier == tag) {
            return Err(SaveError::DuplicateSection { tag: *tag });
        }
    }
    for wanted in SECTION_ORDER {
        if !sections.iter().any(|(tag, _)| *tag == wanted) {
            return Err(SaveError::MissingSection { tag: wanted });
        }
    }
    let mut ordered: [&[u8]; 4] = [&[]; 4];
    for (index, wanted) in SECTION_ORDER.into_iter().enumerate() {
        let (tag, payload) = sections[index];
        if tag != wanted {
            return Err(SaveError::SectionOutOfOrder {
                expected: wanted,
                found: tag,
            });
        }
        ordered[index] = payload;
    }
    Ok(ordered)
}
