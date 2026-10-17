//! Loop messages for ultra-instinct.
//!
//! Product operations are observe, guard, verify. Ultra-Instinct does not click and
//! does not plan navigation. Locate / inspect / diff remain internal helpers.
//!
//! [`GuardDecision`] plus [`ActionTicket`] are the guard contract; the owned
//! agent loop lives in `aui-agent`. Legacy JEV task/result types and the
//! unused `Request` enum were removed (ADR 0003).

#![forbid(unsafe_code)]

mod guard;
mod ticket;
mod values;

pub use guard::{
    FirewallPhase, GuardCandidate, GuardDecision, GuardEvidence, GuardReason, FIREWALL_ORDER,
};
pub use ticket::{ActionTicket, TicketDest, TicketInvalid};
pub use values::{MatcherConfidence, ProtocolError, StateDelta};

/// Legacy six-phase loop. Prefer [`FirewallPhase`] / [`FIREWALL_ORDER`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoopPhase {
    Observe,
    Locate,
    Inspect,
    Act,
    Diff,
    Verify,
}

pub const LOOP_ORDER: [LoopPhase; 6] = [
    LoopPhase::Observe,
    LoopPhase::Locate,
    LoopPhase::Inspect,
    LoopPhase::Act,
    LoopPhase::Diff,
    LoopPhase::Verify,
];

impl LoopPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Locate => "locate",
            Self::Inspect => "inspect",
            Self::Act => "act",
            Self::Diff => "diff",
            Self::Verify => "verify",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firewall_order_is_observe_guard_verify() {
        let names: Vec<_> = FIREWALL_ORDER.iter().map(|p| p.as_str()).collect();
        assert_eq!(names, ["observe", "guard", "verify"]);
    }

    #[test]
    fn legacy_loop_order_still_names_six_verbs() {
        let names: Vec<_> = LOOP_ORDER.iter().map(|phase| phase.as_str()).collect();
        assert_eq!(
            names,
            ["observe", "locate", "inspect", "act", "diff", "verify"]
        );
    }

    #[test]
    fn guard_reasons_have_stable_names() {
        assert_eq!(GuardReason::LowConfidence.as_str(), "low-confidence");
        assert_eq!(GuardReason::Ambiguous.as_str(), "ambiguous");
        assert_eq!(GuardReason::MissingTarget.as_str(), "missing-target");
        assert_eq!(GuardReason::Disabled.as_str(), "disabled");
        assert_eq!(GuardReason::Readonly.as_str(), "readonly");
        assert_eq!(GuardReason::FrontLayer.as_str(), "front-layer");
        assert_eq!(GuardReason::WorldChanged.as_str(), "world-changed");
        assert_eq!(
            GuardReason::UnsupportedAction.as_str(),
            "unsupported-action"
        );
    }

    #[test]
    fn unit_confidence_rejects_outside_zero_to_one_exactly() {
        assert_eq!(MatcherConfidence::try_unit(0.0).unwrap().get(), 0.0);
        assert_eq!(
            MatcherConfidence::try_unit(1.000_001).unwrap_err(),
            ProtocolError::ConfidenceOutOfRange
        );
        assert_eq!(MatcherConfidence::try_new(-0.2).unwrap().get(), -0.2);
    }
}
