//! The encoded input as the decoder sees it: capped, and reduced to the chunks the conversion needs.
//!
//! The PNG reader is handed a [`Stream`]. It first counts the bytes it reads and refuses the one past
//! the encoded cap (the behaviour of `take(cap + 1)`), then removes every chunk the pixel conversion does
//! not need *before* the decoder sees it. What is kept is every critical chunk (the decoder refuses one it
//! does not know) and `tRNS`; `PLTE` is critical. Text, ICC, gamma, sRGB, `eXIf`, animation and unknown
//! ancillary chunks are never stored, however large, so a tiny image with huge metadata costs only the
//! reading of its bytes. An animated image therefore decodes as its default image.

use std::cell::{Cell, RefCell};
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;

/// The first eight bytes of every PNG file.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// What the stream saw, for classifying a decoder error afterwards.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// More than the encoded cap was read.
    pub(crate) over_cap: Cell<bool>,
    /// The input ended while the decoder still wanted bytes.
    pub(crate) ended: Cell<bool>,
    /// How many bytes of the signature were read, and whether they were the signature's.
    pub(crate) signature: Cell<Signature>,
}

/// What the first bytes of the input were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Signature {
    /// Not enough bytes yet.
    #[default]
    Unknown,
    /// Eight bytes of the PNG signature.
    Valid,
    /// Not the PNG signature.
    Invalid,
    /// The input ended inside the signature, which so far was the start of a PNG's.
    CutShort,
}

/// Counts the bytes read and refuses the one past the cap.
struct Capped<R> {
    inner: R,
    taken: u64,
    cap: u64,
    seen: Rc<Seen>,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // At most cap + 1 bytes are ever read: the one past the cap is the error.
        let room = self.cap.saturating_add(1) - self.taken;
        let wanted = usize::try_from(room).map_or(buffer.len(), |room| room.min(buffer.len()));
        let count = self.inner.read(&mut buffer[..wanted])?;
        self.taken += count as u64;
        if self.taken > self.cap {
            self.seen.over_cap.set(true);
            return Err(io::Error::other("the encoded input is over the cap"));
        }
        if count == 0 && !buffer.is_empty() {
            self.seen.ended.set(true);
        }
        Ok(count)
    }
}

/// Where the filter is in the chunk structure.
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Reading the eight signature bytes.
    Signature,
    /// Reading a chunk's length and type.
    Header,
    /// Passing on the rest of a kept chunk (data and checksum).
    Pass(u64),
    /// Dropping the rest of an unneeded chunk (data and checksum).
    Drop(u64),
    /// Not a PNG (or past understanding): everything passes unchanged and the decoder reports it.
    Raw,
}

/// How far a header read got.
enum Head {
    /// All eight bytes are in.
    Complete,
    /// Some are; more are needed.
    More,
    /// The input ended; the partial header goes on as it is.
    Ended,
}

/// Removes the chunks the conversion does not need.
struct ChunkFilter<R> {
    inner: Capped<R>,
    phase: Phase,
    /// The signature or chunk header being read.
    head: [u8; 8],
    filled: usize,
    /// Header bytes still to hand on: `head[emitted..emit_end]`.
    emitted: usize,
    emit_end: usize,
}

/// Whether the decoder needs this chunk: every critical chunk, and the transparency chunk.
fn needed(kind: [u8; 4]) -> bool {
    // Bit 5 of the first byte is the ancillary bit.
    kind[0] & 0x20 == 0 || kind == *b"tRNS"
}

impl<R: Read> ChunkFilter<R> {
    /// Hands on the header bytes read so far, then goes to `phase`.
    fn emit_head(&mut self, phase: Phase) {
        self.emitted = 0;
        self.emit_end = self.filled;
        self.filled = 0;
        self.phase = phase;
    }

    /// Reads towards a whole signature or chunk header.
    fn fill_head(&mut self) -> io::Result<Head> {
        let count = self.inner.read(&mut self.head[self.filled..])?;
        if count == 0 {
            let seen = &self.inner.seen;
            if matches!(self.phase, Phase::Signature) {
                let cut = self.filled > 0 && SIGNATURE.starts_with(&self.head[..self.filled]);
                seen.signature.set(if cut {
                    Signature::CutShort
                } else {
                    Signature::Invalid
                });
            }
            // Whatever partial header there is goes on; the decoder reports the truncation.
            self.emit_head(Phase::Raw);
            return Ok(Head::Ended);
        }
        self.filled += count;
        Ok(if self.filled == self.head.len() {
            Head::Complete
        } else {
            Head::More
        })
    }

    fn header_complete(&mut self) {
        if matches!(self.phase, Phase::Signature) {
            if self.head == SIGNATURE {
                self.inner.seen.signature.set(Signature::Valid);
                self.emit_head(Phase::Header);
            } else {
                self.inner.seen.signature.set(Signature::Invalid);
                self.emit_head(Phase::Raw);
            }
            return;
        }
        let length = u64::from(u32::from_be_bytes([
            self.head[0],
            self.head[1],
            self.head[2],
            self.head[3],
        ]));
        let kind = [self.head[4], self.head[5], self.head[6], self.head[7]];
        // The data and the four checksum bytes follow the header.
        if needed(kind) {
            self.emit_head(Phase::Pass(length + 4));
        } else {
            self.filled = 0;
            self.phase = Phase::Drop(length + 4);
        }
    }
}

impl<R: Read> Read for ChunkFilter<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self.emitted < self.emit_end {
                let count = (self.emit_end - self.emitted).min(buffer.len());
                buffer[..count].copy_from_slice(&self.head[self.emitted..self.emitted + count]);
                self.emitted += count;
                return Ok(count);
            }
            match self.phase {
                Phase::Signature | Phase::Header => match self.fill_head()? {
                    Head::Complete => self.header_complete(),
                    Head::Ended if self.emitted == self.emit_end => return Ok(0),
                    Head::More | Head::Ended => {}
                },
                Phase::Pass(0) | Phase::Drop(0) => self.phase = Phase::Header,
                Phase::Pass(remaining) => {
                    let wanted =
                        usize::try_from(remaining).map_or(buffer.len(), |r| r.min(buffer.len()));
                    let count = self.inner.read(&mut buffer[..wanted])?;
                    self.phase = Phase::Pass(remaining - count as u64);
                    return Ok(count);
                }
                Phase::Drop(remaining) => {
                    let mut scratch = [0_u8; 4096];
                    let wanted =
                        usize::try_from(remaining).map_or(scratch.len(), |r| r.min(scratch.len()));
                    let count = self.inner.read(&mut scratch[..wanted])?;
                    if count == 0 {
                        return Ok(0);
                    }
                    self.phase = Phase::Drop(remaining - count as u64);
                }
                Phase::Raw => return self.inner.read(buffer),
            }
        }
    }
}

/// The reader the PNG decoder is given. The decoder's `Seek` bound is never used by the version in use, so
/// seeking is refused rather than supported.
pub(crate) struct Stream<R>(Rc<RefCell<ChunkFilter<R>>>);

/// Reads what the decoder left, so the whole input is held to the cap.
pub(crate) struct Drain<R>(Rc<RefCell<ChunkFilter<R>>>);

impl<R: Read> Stream<R> {
    /// The input as the decoder should see it, what it saw, and the means to read the rest.
    pub(crate) fn new(inner: R, cap: u64) -> (Self, Rc<Seen>, Drain<R>) {
        let seen = Rc::new(Seen::default());
        let filter = Rc::new(RefCell::new(ChunkFilter {
            inner: Capped {
                inner,
                taken: 0,
                cap,
                seen: Rc::clone(&seen),
            },
            phase: Phase::Signature,
            head: [0; 8],
            filled: 0,
            emitted: 0,
            emit_end: 0,
        }));
        (Self(Rc::clone(&filter)), seen, Drain(filter))
    }
}

impl<R: Read> Drain<R> {
    /// Reads to the end of the input, or to the cap, whichever is first.
    pub(crate) fn run(&self) -> io::Result<()> {
        let mut scratch = [0_u8; 4096];
        loop {
            if self.0.borrow_mut().read(&mut scratch)? == 0 {
                return Ok(());
            }
        }
    }
}

impl<R: Read> Read for Stream<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.borrow_mut().read(buffer)
    }
}

impl<R> Seek for Stream<R> {
    fn seek(&mut self, _position: SeekFrom) -> io::Result<u64> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}
