//! The persistent list of manually placed low-pressure systems.

use ck_sim::{Canonical, CanonicalError, Reader};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Low {
    pub(crate) number: u64,
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) radius: i64,
    pub(crate) depth: i64,
    pub(crate) strongest: i64,
}

impl Canonical for Low {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.number.encode(out)?;
        self.x.encode(out)?;
        self.y.encode(out)?;
        self.radius.encode(out)?;
        self.depth.encode(out)?;
        self.strongest.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            number: u64::decode(input)?,
            x: i64::decode(input)?,
            y: i64::decode(input)?,
            radius: i64::decode(input)?,
            depth: i64::decode(input)?,
            strongest: i64::decode(input)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lows {
    pub(crate) next: u64,
    pub(crate) entries: Vec<Low>,
}

impl Default for Lows {
    fn default() -> Self {
        Self {
            next: 1,
            entries: Vec::new(),
        }
    }
}

#[cfg(test)]
impl Lows {
    pub(super) fn from_entries(next: u64, mut entries: Vec<Low>) -> Self {
        entries.sort_by_key(|low| low.number);
        Self { next, entries }
    }
}

impl Canonical for Lows {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.next.encode(out)?;
        self.entries.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            next: u64::decode(input)?,
            entries: Vec::decode(input)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_round_trips_with_next_number_and_all_fields() {
        let list = Lows {
            next: 8,
            entries: vec![Low {
                number: 4,
                x: 21_361_500,
                y: 15_777_500,
                radius: 500_000,
                depth: 3_000,
                strongest: 4_489,
            }],
        };
        let bytes = list.to_bytes().unwrap();
        assert_eq!(Lows::from_bytes(&bytes).unwrap(), list);
        assert_eq!(Lows::default().next, 1);
        assert!(Lows::default().entries.is_empty());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ending {
    pub(crate) number: u64,
    pub(crate) kind: EndKind,
    pub(crate) cell: (i64, i64),
    pub(crate) strongest: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EndKind {
    Merged(u64),
    Left,
}
