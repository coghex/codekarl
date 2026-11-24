//! Non-negative integers of any size, as little-endian 32-bit limbs (CK-363, moved to the economy crate by CK-364, whose life-cycle
//! tables need it at load): the exact fallback of the chance product and of the
//! skill formulas when an intermediate does not fit 128 bits. Only the operations those need; correctness over speed, and the
//! callers take the 128-bit path first.
#![deny(clippy::float_arithmetic)]

use std::cmp::Ordering;

/// A non-negative integer, with no leading zero limb.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Big(Vec<u32>);

impl Big {
    /// The integer `value`.
    #[must_use]
    pub fn new(value: u128) -> Self {
        let mut limbs = Vec::new();
        let mut rest = value;
        while rest != 0 {
            limbs.push(u32::try_from(rest & u128::from(u32::MAX)).unwrap_or(0));
            rest >>= 32;
        }
        Self(limbs)
    }

    fn trim(mut limbs: Vec<u32>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self(limbs)
    }

    /// `self x other`.
    #[must_use]
    pub fn mul(&self, other: &Self) -> Self {
        let mut out = vec![0_u32; self.0.len() + other.0.len()];
        for (i, x) in self.0.iter().enumerate() {
            let mut carry = 0_u64;
            for (j, y) in other.0.iter().enumerate() {
                let total = u64::from(out[i + j]) + u64::from(*x) * u64::from(*y) + carry;
                out[i + j] = u32::try_from(total & u64::from(u32::MAX)).unwrap_or(0);
                carry = total >> 32;
            }
            out[i + other.0.len()] = u32::try_from(carry).unwrap_or(0);
        }
        Self::trim(out)
    }

    /// `self + other`.
    #[must_use]
    pub fn add(&self, other: &Self) -> Self {
        let mut out = Vec::with_capacity(self.0.len().max(other.0.len()) + 1);
        let mut carry = 0_u64;
        for i in 0..self.0.len().max(other.0.len()) {
            let total = u64::from(self.0.get(i).copied().unwrap_or(0))
                + u64::from(other.0.get(i).copied().unwrap_or(0))
                + carry;
            out.push(u32::try_from(total & u64::from(u32::MAX)).unwrap_or(0));
            carry = total >> 32;
        }
        out.push(u32::try_from(carry).unwrap_or(0));
        Self::trim(out)
    }

    /// `self - other`, or `None` when `other` is larger.
    #[must_use]
    pub fn sub(&self, other: &Self) -> Option<Self> {
        if self.cmp(other) == Ordering::Less {
            return None;
        }
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = 0_i64;
        for i in 0..self.0.len() {
            let mut diff =
                i64::from(self.0[i]) - i64::from(other.0.get(i).copied().unwrap_or(0)) - borrow;
            borrow = i64::from(diff < 0);
            if diff < 0 {
                diff += 1 << 32;
            }
            out.push(u32::try_from(diff).unwrap_or(0));
        }
        Some(Self::trim(out))
    }

    /// How `self` compares with `other`.
    #[must_use]
    #[expect(
        clippy::should_implement_trait,
        reason = "an inherent comparison: Big is not Ord, and its callers compare by name"
    )]
    pub fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }

    fn bits(&self) -> usize {
        self.0.last().map_or(0, |top| {
            self.0.len() * 32 - usize::try_from(top.leading_zeros()).unwrap_or(0)
        })
    }

    fn bit(&self, index: usize) -> bool {
        self.0
            .get(index / 32)
            .is_some_and(|limb| limb >> (index % 32) & 1 == 1)
    }

    /// `(self / divisor, self % divisor)` by binary long division; `divisor` must not be zero.
    #[must_use]
    pub fn divmod(&self, divisor: &Self) -> (Self, Self) {
        let mut quotient = vec![0_u32; self.0.len()];
        let mut rest = Self(Vec::new());
        for index in (0..self.bits()).rev() {
            rest = rest.add(&rest);
            if self.bit(index) {
                rest = rest.add(&Self::new(1));
            }
            if rest.cmp(divisor) != Ordering::Less {
                rest = rest.sub(divisor).unwrap_or_else(|| Self(Vec::new()));
                quotient[index / 32] |= 1 << (index % 32);
            }
        }
        (Self::trim(quotient), rest)
    }

    /// The value, if it fits 128 bits.
    #[must_use]
    pub fn to_u128(&self) -> Option<u128> {
        if self.0.len() > 4 {
            return None;
        }
        Some(
            self.0
                .iter()
                .rev()
                .fold(0_u128, |total, limb| (total << 32) | u128::from(*limb)),
        )
    }

    /// `self` to the power `exponent`.
    #[must_use]
    pub fn pow(&self, exponent: u32) -> Self {
        (0..exponent).fold(Self::new(1), |total, _| total.mul(self))
    }

    /// `round(self / divisor)` to the nearest integer, a tie up.
    #[must_use]
    pub fn round_div(&self, divisor: &Self) -> Self {
        let (quotient, rest) = self.divmod(divisor);
        if rest.add(&rest).cmp(divisor) == Ordering::Less {
            quotient
        } else {
            quotient.add(&Self::new(1))
        }
    }
}

/// The product of non-negative factors.
#[must_use]
pub fn product(factors: &[u128]) -> Big {
    factors
        .iter()
        .fold(Big::new(1), |total, factor| total.mul(&Big::new(*factor)))
}

impl ck_sim::Canonical for Big {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), ck_sim::CanonicalError> {
        self.0.encode(out)
    }

    fn decode(input: &mut ck_sim::Reader<'_>) -> Result<Self, ck_sim::CanonicalError> {
        let limbs = Vec::<u32>::decode(input)?;
        if limbs.last() == Some(&0) {
            return Err(ck_sim::CanonicalError::BadTag {
                ty: "Big's leading limb",
                tag: 0,
            });
        }
        Ok(Self(limbs))
    }
}
