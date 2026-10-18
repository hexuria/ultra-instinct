//! ActionTicket: the product boundary between guard Allow and host actuation.
//!
//! Ultra-Instinct MCP never clicks. On [`crate::GuardDecision::Allow`] the runtime
//! issues a ticket. The host (or an invisible executor interceptor) must
//! revalidate that ticket against a fresh observation, then click the **exact**
//! target. If the world or target fingerprint drifted, the ticket is invalid
//! and nothing is pressed.
//!
//! Direction (hard gates before rank): occluded / front-layer / disabled /
//! hidden / wrong ancestry / stale ticket should be impossible evidence, not
//! score penalties. Ranking only chooses among viable candidates. Today's
//! `buried_better_label` path remains until that split lands; the ticket is
//! the enforcement lease regardless.

use std::fmt;

use ultra_instinct_core::{Action, RegionId, Role};

/// Capability lease issued with Allow. Present at the executor boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionTicket {
    pub ticket_id: u64,
    /// Observation id the host decided on (0 when the host has no snapshot ring).
    pub snapshot_id: u64,
    pub action: Action,
    pub target_id: RegionId,
    pub target_role: Role,
    pub target_label: String,
    /// [`ultra_instinct_core::InteractionRegion::fingerprint`] bits at issue time.
    pub target_fingerprint: u64,
    /// Hash of the world variables (focus, front layer, clickable, occluded).
    pub world_fingerprint: u64,
    /// DRAG only: the release region, bound like the target.
    pub dest: Option<Box<TicketDest>>,
    /// Surface point clicks only: the grid cell `[col, row]` the ticket
    /// allows. The executor clicks exactly this cell.
    pub cell: Option<[u16; 2]>,
    /// KEY only: the one key the ticket allows (from `ultra_instinct_core::KEY_NAMES`).
    pub key: Option<String>,
}

/// The second end of a two-region ticket (DRAG). Revalidated like the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketDest {
    pub id: RegionId,
    pub role: Role,
    pub label: String,
    pub fingerprint: u64,
    /// World fingerprint of the destination's neighborhood at issue time.
    pub world_fingerprint: u64,
    /// A release on a grid cell `[col, row]` of a drawing surface; `None`
    /// releases on the region itself.
    pub cell: Option<[u16; 2]>,
}

impl ActionTicket {
    pub fn action(&self) -> Action {
        self.action
    }

    pub fn target_id(&self) -> &RegionId {
        &self.target_id
    }
}

/// Why a ticket cannot be consumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TicketInvalid {
    /// Focus, front layer, clickable set, or occluded set drifted.
    WorldChanged,
    /// The ticket's target id is gone from the current manifold.
    TargetGone,
    /// The target is present but role/label/fingerprint no longer match.
    TargetChanged,
    /// This ticket_id was already consumed (one-shot).
    TicketConsumed,
    /// Executor was asked to press a different target/action than the ticket.
    TicketMismatch,
}

impl TicketInvalid {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorldChanged => "world-changed",
            Self::TargetGone => "target-gone",
            Self::TargetChanged => "target-changed",
            Self::TicketConsumed => "ticket-consumed",
            Self::TicketMismatch => "ticket-mismatch",
        }
    }
}

impl fmt::Display for TicketInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for TicketInvalid {}
