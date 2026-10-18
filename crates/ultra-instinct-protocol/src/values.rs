//! Wire value types shared by the guard, the agent, and the MCP adapter.
//!
//! The legacy JEV-facing `ComputerTask` / `ComputerResult` / `Intent` /
//! `Constraints` / `ExpectedOutcome` / `FallbackReason` / `ReportedExecutor`
//! types were deleted in the Phase 8 cleanup (ADR 0003): nothing in the
//! workspace, MCP, CLI, or bench constructed them. The agent loop
//! (`ultra-instinct-agent`) and [`crate::GuardDecision`] / [`crate::ActionTicket`]
//! are the contracts.

use std::fmt;

use ultra_instinct_core::RegionId;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtocolError {
    NonFiniteConfidence,
    /// A caller-supplied confidence outside `[0, 1]`.
    ConfidenceOutOfRange,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteConfidence => f.write_str("confidence must be finite"),
            Self::ConfidenceOutOfRange => f.write_str("confidence must be between 0 and 1"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Finite matcher total. Not a probability. Negative values are allowed
/// because penalties can drive a score below zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatcherConfidence(f64);

impl MatcherConfidence {
    pub fn try_new(value: f64) -> Result<Self, ProtocolError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(ProtocolError::NonFiniteConfidence)
        }
    }

    /// A confidence a caller hands to the act gate. It must be finite and in
    /// `[0, 1]`. A ranker total can be negative ([`Self::try_new`] allows
    /// that), but a caller value outside `[0, 1]` is not a locate result.
    pub fn try_unit(value: f64) -> Result<Self, ProtocolError> {
        let checked = Self::try_new(value)?;
        if (0.0..=1.0).contains(&value) {
            Ok(checked)
        } else {
            Err(ProtocolError::ConfidenceOutOfRange)
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Id-level change between two observations. Not a second diff algorithm:
/// a host fills this from `ultra-instinct-observe::diff`. Page flags are separate
/// and use `url_changed`, never a navigate field. `new` leaves those flags
/// unset so existing callers compile; set them with the `with_*` builders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateDelta {
    added: Vec<RegionId>,
    removed: Vec<RegionId>,
    changed: Vec<RegionId>,
    moved: Vec<RegionId>,
    text_changed: Vec<RegionId>,
    focus_changed: bool,
    url_changed: bool,
}

impl StateDelta {
    pub fn new(
        mut added: Vec<RegionId>,
        mut removed: Vec<RegionId>,
        mut changed: Vec<RegionId>,
    ) -> Self {
        added.sort();
        removed.sort();
        changed.sort();
        added.dedup();
        removed.dedup();
        changed.dedup();
        Self {
            added,
            removed,
            changed,
            moved: Vec::new(),
            text_changed: Vec::new(),
            focus_changed: false,
            url_changed: false,
        }
    }

    pub fn with_moved(mut self, mut ids: Vec<RegionId>) -> Self {
        ids.sort();
        ids.dedup();
        self.moved = ids;
        self
    }

    pub fn with_text_changed(mut self, mut ids: Vec<RegionId>) -> Self {
        ids.sort();
        ids.dedup();
        self.text_changed = ids;
        self
    }

    pub fn with_focus_changed(mut self, changed: bool) -> Self {
        self.focus_changed = changed;
        self
    }

    pub fn with_url_changed(mut self, changed: bool) -> Self {
        self.url_changed = changed;
        self
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), Vec::new(), Vec::new())
    }

    pub fn added(&self) -> &[RegionId] {
        &self.added
    }

    pub fn removed(&self) -> &[RegionId] {
        &self.removed
    }

    pub fn changed(&self) -> &[RegionId] {
        &self.changed
    }

    pub fn moved(&self) -> &[RegionId] {
        &self.moved
    }

    pub fn text_changed(&self) -> &[RegionId] {
        &self.text_changed
    }

    pub const fn focus_changed(&self) -> bool {
        self.focus_changed
    }

    pub const fn url_changed(&self) -> bool {
        self.url_changed
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.moved.is_empty()
            && self.text_changed.is_empty()
            && !self.focus_changed
            && !self.url_changed
    }
}
