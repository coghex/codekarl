//! The canonical encoding: one exact byte form for every value, with no choices.
//!
//! Integers are little-endian at their declared width, a `bool` is one byte (0 or 1), strings and sequences carry a `u32`
//! length or count, and maps are `BTreeMap`s in key order. There are no floats. Decoding refuses anything the encoder
//! would not have written, so a value has exactly one byte form and re-encoding what was decoded gives the same bytes.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::calendar::GameTime;

/// Why a value could not be encoded or decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalError {
    /// The input ended before the value did.
    ShortInput {
        /// How many more bytes were needed.
        needed: usize,
        /// How many were left.
        available: usize,
    },
    /// A tag byte names no variant.
    BadTag {
        /// The type being decoded.
        ty: &'static str,
        /// The refused tag.
        tag: u8,
    },
    /// A string is not valid UTF-8.
    InvalidUtf8,
    /// A `bool` is neither 0 nor 1.
    BadBool {
        /// The refused byte.
        value: u8,
    },
    /// A map's keys are not in strictly ascending order, so the bytes are not the canonical form.
    KeysNotAscending,
    /// The value decoded, but bytes followed it.
    TrailingBytes {
        /// How many bytes were left over.
        count: usize,
    },
    /// A string or sequence is too long for its `u32` length.
    TooLong {
        /// The refused length.
        len: usize,
    },
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ShortInput { needed, available } => {
                write!(
                    f,
                    "the input ended early: {needed} more bytes needed, {available} left"
                )
            }
            Self::BadTag { ty, tag } => write!(f, "{tag} is not a tag of {ty}"),
            Self::InvalidUtf8 => write!(f, "a string is not valid UTF-8"),
            Self::BadBool { value } => write!(f, "{value} is not a boolean (0 or 1)"),
            Self::KeysNotAscending => write!(f, "a map's keys are not in strictly ascending order"),
            Self::TrailingBytes { count } => write!(f, "{count} bytes follow the value"),
            Self::TooLong { len } => write!(f, "a length of {len} does not fit in 32 bits"),
        }
    }
}

impl Error for CanonicalError {}

/// A cursor over the bytes being decoded.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader over `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// How many bytes are left.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.bytes.len()
    }

    /// The next `count` bytes.
    ///
    /// # Errors
    /// Returns [`CanonicalError::ShortInput`] when fewer are left; nothing is consumed.
    pub fn take(&mut self, count: usize) -> Result<&'a [u8], CanonicalError> {
        match self.bytes.split_at_checked(count) {
            Some((taken, rest)) => {
                self.bytes = rest;
                Ok(taken)
            }
            None => Err(CanonicalError::ShortInput {
                needed: count - self.bytes.len(),
                available: self.bytes.len(),
            }),
        }
    }

    /// The next `N` bytes as an array.
    ///
    /// # Errors
    /// Returns [`CanonicalError::ShortInput`] when fewer are left.
    pub fn take_array<const N: usize>(&mut self) -> Result<[u8; N], CanonicalError> {
        let mut array = [0; N];
        array.copy_from_slice(self.take(N)?);
        Ok(array)
    }

    /// A `u32` length or count.
    ///
    /// # Errors
    /// Returns [`CanonicalError::ShortInput`] when fewer than four bytes are left.
    pub fn length(&mut self) -> Result<usize, CanonicalError> {
        let length = u32::decode(self)?;
        // A `u32` always fits in `usize` on the 64-bit machines the game supports.
        Ok(usize::try_from(length).unwrap_or(usize::MAX))
    }

    /// Checks that every byte was used.
    ///
    /// # Errors
    /// Returns [`CanonicalError::TrailingBytes`] when some are left.
    pub const fn finish(&self) -> Result<(), CanonicalError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(CanonicalError::TrailingBytes {
                count: self.bytes.len(),
            })
        }
    }
}

/// Writes a `u32` length or count.
///
/// # Errors
/// Returns [`CanonicalError::TooLong`] when `len` does not fit.
pub fn encode_length(len: usize, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    let length = u32::try_from(len).map_err(|_| CanonicalError::TooLong { len })?;
    length.encode(out)
}

/// A value with one exact byte form.
pub trait Canonical: Sized {
    /// Appends the canonical bytes of `self` to `out`.
    ///
    /// # Errors
    /// Returns [`CanonicalError::TooLong`] when a string or sequence is too long to count in 32 bits.
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError>;

    /// Reads one value from the front of `input`.
    ///
    /// # Errors
    /// Returns the [`CanonicalError`] naming what is wrong with the bytes.
    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError>;

    /// The canonical bytes of `self`.
    ///
    /// # Errors
    /// As [`Canonical::encode`].
    fn to_bytes(&self) -> Result<Vec<u8>, CanonicalError> {
        let mut out = Vec::new();
        self.encode(&mut out)?;
        Ok(out)
    }

    /// The value whose canonical bytes are exactly `bytes`.
    ///
    /// # Errors
    /// Returns the [`CanonicalError`] naming what is wrong, including [`CanonicalError::TrailingBytes`].
    fn from_bytes(bytes: &[u8]) -> Result<Self, CanonicalError> {
        let mut input = Reader::new(bytes);
        let value = Self::decode(&mut input)?;
        input.finish()?;
        Ok(value)
    }
}

macro_rules! canonical_integer {
    ($($int:ty),*) => {$(
        impl Canonical for $int {
            fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
                out.extend_from_slice(&self.to_le_bytes());
                Ok(())
            }

            fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
                Ok(Self::from_le_bytes(input.take_array()?))
            }
        }
    )*};
}

canonical_integer!(u8, u16, u32, u64, i8, i16, i32, i64);

impl Canonical for bool {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        out.push(u8::from(*self));
        Ok(())
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(CanonicalError::BadBool { value }),
        }
    }
}

impl Canonical for String {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        encode_length(self.len(), out)?;
        out.extend_from_slice(self.as_bytes());
        Ok(())
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let length = input.length()?;
        let bytes = input.take(length)?;
        str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| CanonicalError::InvalidUtf8)
    }
}

/// A sequence is its `u32` count and then its elements. `Vec<u8>` is therefore a byte string: its length, then the bytes.
impl<T: Canonical> Canonical for Vec<T> {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        encode_length(self.len(), out)?;
        self.iter().try_for_each(|element| element.encode(out))
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let count = input.length()?;
        // The count comes from the input, so it must not decide how much to reserve.
        let mut elements = Vec::with_capacity(count.min(input.remaining()));
        for _ in 0..count {
            elements.push(T::decode(input)?);
        }
        Ok(elements)
    }
}

/// An `Option` is a tag byte, 0 for none and 1 for some, and then the value.
impl<T: Canonical> Canonical for Option<T> {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            None => 0_u8.encode(out),
            Some(value) => {
                1_u8.encode(out)?;
                value.encode(out)
            }
        }
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(None),
            1 => Ok(Some(T::decode(input)?)),
            tag => Err(CanonicalError::BadTag { ty: "Option", tag }),
        }
    }
}

/// A map is its `u32` count and then its entries in strictly ascending key order. Only `BTreeMap` is canonical: a hash
/// map's order is not defined.
impl<K: Canonical + Ord, V: Canonical> Canonical for BTreeMap<K, V> {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        encode_length(self.len(), out)?;
        self.iter().try_for_each(|(key, value)| {
            key.encode(out)?;
            value.encode(out)
        })
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let count = input.length()?;
        let mut map = Self::new();
        for _ in 0..count {
            let key = K::decode(input)?;
            let value = V::decode(input)?;
            if map.last_key_value().is_some_and(|(last, _)| *last >= key) {
                return Err(CanonicalError::KeysNotAscending);
            }
            map.insert(key, value);
        }
        Ok(map)
    }
}

impl Canonical for GameTime {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.hours().encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self::from_hours(u64::decode(input)?))
    }
}
