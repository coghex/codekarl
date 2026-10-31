//! Registry identities and handle tokens: the string form a script holds.
//!
//! A token names one resource of one registry: `ck1:<kind>:<registry>:<slot>:<generation>`. It is a process-local
//! identity, not saved data, an asset ID or a security boundary (`docs/scene-design.md` §4.1: the Lua runtime stays
//! unsandboxed). A token that parses is only well formed; whether it names something usable is the registry's answer.

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest string [`Token`]'s parser looks at: a quad token is 51 bytes, and anything longer is refused before any
/// field is read.
pub const MAX_TOKEN_BYTES: usize = 64;

const VERSION: &str = "ck1";
/// Hexadecimal digits of the registry, slot and generation fields.
const REGISTRY_DIGITS: usize = 16;
const SLOT_DIGITS: usize = 8;
const GENERATION_DIGITS: usize = 16;

/// Identifies one registry for the life of the process. Zero is never issued, so an all-zero registry field is
/// always foreign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegistryId(u64);

/// The process-wide counter of registry identities: the next one to issue.
static NEXT_REGISTRY: AtomicU64 = AtomicU64::new(1);

/// Registry identities are used up: the counter would wrap, and it never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryIdsExhausted;

impl fmt::Display for RegistryIdsExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the process has issued every registry identity")
    }
}

impl std::error::Error for RegistryIdsExhausted {}

impl RegistryId {
    /// A fresh identity from the process-wide counter: no two registries, simultaneous or one after the other, share
    /// one.
    ///
    /// # Errors
    ///
    /// [`RegistryIdsExhausted`] when the counter is used up; it never wraps.
    pub fn allocate() -> Result<Self, RegistryIdsExhausted> {
        Self::allocate_from(&NEXT_REGISTRY)
    }

    /// `allocate` over a counter the caller owns (a unit test starts one at `u64::MAX`). A checked
    /// compare-and-swap advances it.
    pub(crate) fn allocate_from(counter: &AtomicU64) -> Result<Self, RegistryIdsExhausted> {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |next| {
                next.checked_add(1)
            })
            .map(Self)
            .map_err(|_| RegistryIdsExhausted)
    }

    /// The identity as a number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RegistryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What a token names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// A texture of a texture registry.
    Texture,
    /// A quad of a quad registry. `ck-resources` stores no quads; the kind is defined here so version 1 of the format
    /// does not change when quad handles arrive.
    Quad,
}

impl TokenKind {
    fn word(self) -> &'static str {
        match self {
            Self::Texture => "tex",
            Self::Quad => "quad",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Texture => "texture",
            Self::Quad => "quad",
        })
    }
}

/// A parsed handle token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token {
    /// What the token names.
    pub kind: TokenKind,
    /// The registry it belongs to.
    pub registry: RegistryId,
    /// The logical slot.
    pub slot: u32,
    /// The slot's generation, advanced each time the slot is reused.
    pub generation: u64,
}

/// The fields of a token that hold a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenField {
    /// The registry identity (16 digits).
    Registry,
    /// The slot (8 digits).
    Slot,
    /// The generation (16 digits).
    Generation,
}

impl fmt::Display for TokenField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Registry => "registry",
            Self::Slot => "slot",
            Self::Generation => "generation",
        })
    }
}

/// Why a string is not a token. Each refusal names the rule that failed and none echoes the input, so a hostile or huge
/// string cannot reach a log through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Longer than [`MAX_TOKEN_BYTES`]; refused before any field is read.
    TooLong {
        /// The string's length in bytes.
        length: usize,
    },
    /// Not exactly five `:`-separated fields.
    FieldCount,
    /// The first field is not `ck1`.
    UnknownVersion,
    /// The second field is not `tex` or `quad`.
    UnknownKind,
    /// A number field is not exactly its width in lowercase hexadecimal digits.
    Malformed(TokenField),
}

impl fmt::Display for TokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { length } => write!(
                formatter,
                "a handle token is at most {MAX_TOKEN_BYTES} bytes, this is {length}"
            ),
            Self::FieldCount => {
                formatter.write_str("a handle token has exactly five `:`-separated fields")
            }
            Self::UnknownVersion => formatter.write_str("the handle token's version is not ck1"),
            Self::UnknownKind => formatter.write_str("the handle token's kind is not tex or quad"),
            Self::Malformed(field) => write!(
                formatter,
                "the handle token's {field} field is not exactly its width in lowercase hexadecimal digits"
            ),
        }
    }
}

impl std::error::Error for TokenError {}

/// The value of a field of exactly `digits` bytes, each `0-9a-f`: no sign (`from_str_radix` would accept a leading
/// `+`), no uppercase, no whitespace, no prefix.
fn hex(text: &str, digits: usize, field: TokenField) -> Result<u64, TokenError> {
    let well_formed = text.len() == digits
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !well_formed {
        return Err(TokenError::Malformed(field));
    }
    u64::from_str_radix(text, 16).map_err(|_| TokenError::Malformed(field))
}

impl FromStr for Token {
    type Err = TokenError;

    /// Parses strictly: the length is checked first, then exactly five fields, the literal `ck1`, a known kind and three
    /// numbers of exactly their widths. Conversion is integer only.
    fn from_str(text: &str) -> Result<Self, TokenError> {
        if text.len() > MAX_TOKEN_BYTES {
            return Err(TokenError::TooLong { length: text.len() });
        }
        let mut fields = text.split(':');
        let (Some(version), Some(kind), Some(registry), Some(slot), Some(generation), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(TokenError::FieldCount);
        };
        if version != VERSION {
            return Err(TokenError::UnknownVersion);
        }
        let kind = match kind {
            "tex" => TokenKind::Texture,
            "quad" => TokenKind::Quad,
            _ => return Err(TokenError::UnknownKind),
        };
        let registry = hex(registry, REGISTRY_DIGITS, TokenField::Registry)?;
        let slot = u32::try_from(hex(slot, SLOT_DIGITS, TokenField::Slot)?)
            .map_err(|_| TokenError::Malformed(TokenField::Slot))?;
        let generation = hex(generation, GENERATION_DIGITS, TokenField::Generation)?;
        Ok(Self {
            kind,
            registry: RegistryId(registry),
            slot,
            generation,
        })
    }
}

impl fmt::Display for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{VERSION}:{}:{:016x}:{:08x}:{:016x}",
            self.kind.word(),
            self.registry.0,
            self.slot,
            self.generation
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(kind: TokenKind, registry: u64, slot: u32, generation: u64) -> Token {
        Token {
            kind,
            registry: RegistryId(registry),
            slot,
            generation,
        }
    }

    const TEXTURE: &str = "ck1:tex:0000000000000007:00000003:0000000000000002";

    #[test]
    fn a_token_round_trips_at_the_minimum_and_maximum_of_every_field() {
        for kind in [TokenKind::Texture, TokenKind::Quad] {
            for (registry, slot, generation) in [
                (0, 0, 0),
                (1, 1, 1),
                (u64::MAX, u32::MAX, u64::MAX),
                (u64::MAX, 0, 0),
                (0, u32::MAX, 0),
                (0, 0, u64::MAX),
            ] {
                let original = token(kind, registry, slot, generation);
                let text = original.to_string();
                assert_eq!(text.parse::<Token>(), Ok(original), "{text}");
            }
        }
    }

    #[test]
    fn the_exact_lengths_and_text() {
        assert_eq!(token(TokenKind::Texture, 7, 3, 2).to_string(), TEXTURE);
        assert_eq!(TEXTURE.len(), 50);
        let quad = token(TokenKind::Quad, u64::MAX, u32::MAX, u64::MAX).to_string();
        assert_eq!(quad, "ck1:quad:ffffffffffffffff:ffffffff:ffffffffffffffff");
        assert_eq!(quad.len(), 51);
        assert!(quad.len() <= MAX_TOKEN_BYTES);
    }

    /// Each string is refused with exactly its rule, and the refusal's message does not contain the string.
    #[test]
    fn each_malformed_string_is_refused_naming_its_rule_without_echoing_it() {
        let long = "x".repeat(65);
        let cases: [(String, TokenError); 14] = [
            (String::new(), TokenError::FieldCount),
            (long, TokenError::TooLong { length: 65 }),
            (TEXTURE.replace("ck1", "ck2"), TokenError::UnknownVersion),
            (TEXTURE.replace("tex", "mesh"), TokenError::UnknownKind),
            (
                TEXTURE.replace("0000000000000007", "000000000000000A"),
                TokenError::Malformed(TokenField::Registry),
            ),
            (
                TEXTURE.replace("0000000000000007", "+000000000000007"),
                TokenError::Malformed(TokenField::Registry),
            ),
            (
                TEXTURE.replace("00000003", "000003"),
                TokenError::Malformed(TokenField::Slot),
            ),
            (
                TEXTURE.replace("0000000000000002", "00000000000000002"),
                TokenError::Malformed(TokenField::Generation),
            ),
            (format!("{TEXTURE}:0"), TokenError::FieldCount),
            (format!(" {TEXTURE}"), TokenError::UnknownVersion),
            (
                format!("{TEXTURE} "),
                TokenError::Malformed(TokenField::Generation),
            ),
            (
                TEXTURE.replace("00000003", "0000000\u{e9}"),
                TokenError::Malformed(TokenField::Slot),
            ),
            (
                TEXTURE.replace("0000000000000007", "0x00000000000007"),
                TokenError::Malformed(TokenField::Registry),
            ),
            (TEXTURE.replace(":tex", ""), TokenError::FieldCount),
        ];
        for (text, expected) in cases {
            let error = text.parse::<Token>().expect_err(&text);
            assert_eq!(error, expected, "{text:?}");
            let message = error.to_string();
            assert!(
                !message.contains(text.as_str()) || text.is_empty(),
                "{message}"
            );
            assert!(!message.is_empty());
        }
    }

    #[test]
    fn a_long_string_is_refused_before_any_field_is_read() {
        // 65 bytes that would otherwise be a field-count error: the length rule wins.
        let text = format!("{TEXTURE}:{}", "0".repeat(14));
        assert_eq!(text.len(), 65);
        assert_eq!(
            text.parse::<Token>(),
            Err(TokenError::TooLong { length: 65 })
        );
        // 64 bytes is still looked at.
        let text = format!("{TEXTURE}:{}", "0".repeat(13));
        assert_eq!(text.parse::<Token>(), Err(TokenError::FieldCount));
        let huge = "ck1:".repeat(10_000);
        assert_eq!(
            huge.parse::<Token>(),
            Err(TokenError::TooLong { length: huge.len() })
        );
    }

    #[test]
    fn a_refusal_never_echoes_the_input() {
        let hostile = "ck1:tex:SECRET-SECRET-SEC:00000003:0000000000000002";
        let message = hostile.parse::<Token>().expect_err("malformed").to_string();
        assert!(!message.contains("SECRET"));
        let long = format!("SECRET{}", "x".repeat(100));
        let message = long.parse::<Token>().expect_err("long").to_string();
        assert!(!message.contains("SECRET"));
    }

    #[test]
    fn registry_identities_are_fresh_and_start_above_zero() {
        let first = RegistryId::allocate().expect("an identity");
        let second = RegistryId::allocate().expect("an identity");
        assert_ne!(first, second);
        assert!(first.get() >= 1 && second.get() > first.get());
    }

    #[test]
    fn a_counter_at_the_end_refuses_and_does_not_wrap() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(
            RegistryId::allocate_from(&counter),
            Ok(RegistryId(u64::MAX - 1))
        );
        assert_eq!(
            RegistryId::allocate_from(&counter),
            Err(RegistryIdsExhausted)
        );
        // Still refused, and the counter did not move or wrap to a value that was issued.
        assert_eq!(
            RegistryId::allocate_from(&counter),
            Err(RegistryIdsExhausted)
        );
        assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
        let zero = AtomicU64::new(u64::MAX);
        assert!(RegistryId::allocate_from(&zero).is_err());
    }
}
