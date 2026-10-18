use std::fmt;

use crate::error::CoreError;

/// Stable identity of one interaction region inside a manifold.
///
/// Ordering is lexicographic on the raw identifier bytes. Ranking uses that
/// order only as a tie-break after the resonance score.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(String);

impl RegionId {
    /// Accepts a non-empty id with no Unicode whitespace and no control characters.
    pub fn try_new(raw: impl AsRef<str>) -> Result<Self, CoreError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(CoreError::EmptyId);
        }
        if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CoreError::InvalidId);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RegionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Deterministic digest of the fields that define a region's observable state.
///
/// This is not a cryptographic commitment. It exists so two snapshots can be
/// compared without floating-point noise below one-thousandth of a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateFingerprint(u64);

impl StateFingerprint {
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl fmt::Display for StateFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// A finite value in the closed unit interval.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub const ZERO: Self = Self(0.0);
    pub const ONE: Self = Self(1.0);

    pub fn try_new(value: f64) -> Result<Self, CoreError> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(CoreError::StabilityOutOfRange)
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}
