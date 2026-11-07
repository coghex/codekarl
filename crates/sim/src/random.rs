//! Counter-based random streams: a draw is a frozen mix of the master seed, a stream id, the tick and a counter.
//!
//! Nothing here is stateful across ticks. A stream's counter starts at 0 every tick, so a save taken at a tick boundary
//! needs only the master seed, and a stream's numbers depend on nothing another stream does.

use std::{error::Error, fmt};

use crate::calendar::GameTime;

const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The `SplitMix64` finaliser.
const fn finalise(mut z: u64) -> u64 {
    z ^= z >> 30;
    z = z.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The frozen 64-bit mixer: `finalise(x + 0x9E3779B97F4A7C15)` with wrapping arithmetic.
pub(crate) const fn mix(x: u64) -> u64 {
    finalise(x.wrapping_add(GOLDEN_GAMMA))
}

/// An incremental FNV-1a 64-bit hash.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fnv1a(u64);

impl Fnv1a {
    pub(crate) const fn new() -> Self {
        Self(FNV_OFFSET_BASIS)
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    pub(crate) const fn finish(self) -> u64 {
        self.0
    }
}

/// The FNV-1a 64-bit hash of `bytes`.
#[must_use]
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = Fnv1a::new();
    hash.update(bytes);
    hash.finish()
}

/// The id of the stream called `name`: the FNV-1a 64-bit hash of its UTF-8 bytes.
#[must_use]
pub fn stream_id(name: &str) -> u64 {
    fnv1a(name.as_bytes())
}

/// Why a draw or a stream lookup was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RandomError {
    /// The range holds no value: `below(0)`, or a lower bound above the upper. Nothing was drawn.
    EmptyRange,
    /// The system was not given a stream with this name.
    UnknownStream {
        /// The requested name.
        name: String,
    },
}

impl fmt::Display for RandomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRange => write!(f, "the range is empty"),
            Self::UnknownStream { name } => write!(f, "this system has no stream named `{name}`"),
        }
    }
}

impl Error for RandomError {}

/// One named random stream, at one tick.
///
/// A draw is `mix(mix(mix(mix(seed) ^ stream) ^ tick) ^ counter)`. The counter counts the draws made this tick, rejected
/// ones included, and starts again at 0 on the next tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream {
    name: String,
    id: u64,
    seed: u64,
    tick: GameTime,
    counter: u64,
}

impl Stream {
    /// The stream called `name` under the master `seed`, at `tick`, before its first draw.
    #[must_use]
    pub fn new(seed: u64, name: impl Into<String>, tick: GameTime) -> Self {
        let name = name.into();
        Self {
            id: stream_id(&name),
            name,
            seed,
            tick,
            counter: 0,
        }
    }

    /// The stream's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The stream's id, the hash of its name.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// How many draws this tick has made.
    #[must_use]
    pub const fn counter(&self) -> u64 {
        self.counter
    }

    /// Moves to a new tick: the counter starts again at 0.
    pub(crate) const fn start_tick(&mut self, seed: u64, tick: GameTime) {
        self.seed = seed;
        self.tick = tick;
        self.counter = 0;
    }

    /// The next raw 64-bit value of this tick.
    pub fn draw(&mut self) -> u64 {
        let value = mix(mix(mix(mix(self.seed) ^ self.id) ^ self.tick.hours()) ^ self.counter);
        self.counter = self.counter.wrapping_add(1);
        value
    }

    /// A value from `0` to `n - 1`, every one equally likely.
    ///
    /// A raw draw below the threshold `2^64 mod n` is discarded and drawn again, so the accepted values are a whole
    /// multiple of `n` in number. Each draw, discarded or not, advances the counter.
    ///
    /// # Errors
    /// Returns [`RandomError::EmptyRange`] for `n == 0`; nothing is drawn.
    pub fn below(&mut self, n: u64) -> Result<u64, RandomError> {
        if n == 0 {
            return Err(RandomError::EmptyRange);
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let raw = self.draw();
            if raw >= threshold {
                return Ok(raw % n);
            }
        }
    }

    /// A value from `m` to `n` inclusive, every one equally likely.
    ///
    /// # Errors
    /// Returns [`RandomError::EmptyRange`] when `m > n`; nothing is drawn.
    pub fn range(&mut self, m: u64, n: u64) -> Result<u64, RandomError> {
        if m > n {
            return Err(RandomError::EmptyRange);
        }
        let width = n - m;
        if width == u64::MAX {
            return Ok(self.draw());
        }
        Ok(m + self.below(width + 1)?)
    }

    /// A value from `m` to `n` inclusive, every one equally likely.
    ///
    /// The offset `range(0, n - m)` is added to `m` with wrapping arithmetic, so the full span needs no special case.
    ///
    /// # Errors
    /// Returns [`RandomError::EmptyRange`] when `m > n`; nothing is drawn.
    pub fn range_i64(&mut self, m: i64, n: i64) -> Result<i64, RandomError> {
        if m > n {
            return Err(RandomError::EmptyRange);
        }
        let width = n.cast_unsigned().wrapping_sub(m.cast_unsigned());
        Ok(m.wrapping_add(self.range(0, width)?.cast_signed()))
    }
}
