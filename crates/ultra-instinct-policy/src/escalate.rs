//! Optional escalation after Instinct abstains. Fallback is feature-shaped, not required.

use crate::goal::AgentGoal;
use crate::types::{
    BrowserPolicy, DecisionEvidence, EvidenceWants, HistoryEntry, PolicyError, PolicyOutcome,
};
use ultra_instinct_core::ActionSpace;

/// Try `local` first; on [`PolicyOutcome::Abstain`], optionally call `fallback`.
///
/// Never turns an abstain into the top-ranked candidate silently. The fallback
/// must itself return Choice or Abstain through the same finite ActionSpace.
pub struct EscalatingPolicy<L, H> {
    pub local: L,
    pub fallback: Option<H>,
    last: &'static str,
}

impl<L, H> EscalatingPolicy<L, H> {
    pub fn new(local: L, fallback: Option<H>) -> Self {
        Self {
            local,
            fallback,
            last: "none",
        }
    }
}

impl<L> EscalatingPolicy<L, L> {
    pub fn local_only(local: L) -> Self {
        Self {
            local,
            fallback: None,
            last: "none",
        }
    }
}

impl<L, H> BrowserPolicy for EscalatingPolicy<L, H>
where
    L: BrowserPolicy,
    H: BrowserPolicy,
{
    fn decide(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let first = self.local.decide(space, goal, history)?;
        self.last = self.local.decision_source();
        if first.as_choice().is_some() {
            return Ok(first);
        }
        if let Some(fallback) = self.fallback.as_mut() {
            let outcome = fallback.decide(space, goal, history)?;
            self.last = fallback.decision_source();
            return Ok(outcome);
        }
        Ok(first)
    }

    fn name(&self) -> &'static str {
        "escalating"
    }

    /// The arm that answered the most recent decision — the diary records
    /// `instinct` vs `jev`/`clef-flash`, not the `escalating` wrapper.
    fn decision_source(&self) -> &'static str {
        self.last
    }

    fn wants_evidence(&self) -> EvidenceWants {
        let fallback = self.fallback.as_ref().map(H::wants_evidence);
        self.local.wants_evidence().or(fallback.unwrap_or_default())
    }

    fn decide_with_evidence(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
        evidence: &DecisionEvidence,
    ) -> Result<PolicyOutcome, PolicyError> {
        let first = self
            .local
            .decide_with_evidence(space, goal, history, evidence)?;
        self.last = self.local.decision_source();
        if first.as_choice().is_some() {
            return Ok(first);
        }
        if let Some(fallback) = self.fallback.as_mut() {
            let outcome = fallback.decide_with_evidence(space, goal, history, evidence)?;
            self.last = fallback.decision_source();
            return Ok(outcome);
        }
        Ok(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instinct_policy::InstinctPolicy;
    use crate::types::{PolicyDecision, RankedAction};
    use ultra_instinct_core::{parse_fixture, ActionId, ActionKind, ActionSpace};

    struct AlwaysAbstain;

    impl BrowserPolicy for AlwaysAbstain {
        fn decide(
            &mut self,
            _space: &ActionSpace,
            _goal: &AgentGoal,
            _history: &[HistoryEntry],
        ) -> Result<PolicyOutcome, PolicyError> {
            Ok(PolicyOutcome::Abstain {
                reason: "forced".into(),
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            })
        }
    }

    struct AlwaysChooseDone;

    impl BrowserPolicy for AlwaysChooseDone {
        fn decide(
            &mut self,
            _space: &ActionSpace,
            _goal: &AgentGoal,
            _history: &[HistoryEntry],
        ) -> Result<PolicyOutcome, PolicyError> {
            Ok(PolicyOutcome::Choice(PolicyDecision {
                action_id: ActionId::try_new("DONE").unwrap(),
                kind: ActionKind::Done,
                target_label: "done".into(),
                confidence_millis: 900,
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            }))
        }
    }

    #[test]
    fn abstain_without_fallback_stays_abstain() {
        let m = parse_fixture(
            r#"
            viewport w=100 h=100
            region id=a role=button label="A" x=1 y=1 w=10 h=10 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let mut policy: EscalatingPolicy<AlwaysAbstain, AlwaysAbstain> =
            EscalatingPolicy::local_only(AlwaysAbstain);
        let out = policy.decide(&space, &AgentGoal::new("A"), &[]).unwrap();
        assert!(matches!(out, PolicyOutcome::Abstain { .. }));
    }

    #[test]
    fn fallback_runs_only_on_abstain() {
        let m = parse_fixture(
            r#"
            viewport w=100 h=100
            region id=a role=button label="A" x=1 y=1 w=10 h=10 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let mut policy = EscalatingPolicy::new(AlwaysAbstain, Some(AlwaysChooseDone));
        let out = policy
            .decide(&space, &AgentGoal::new("whatever"), &[])
            .unwrap();
        assert_eq!(out.as_choice().unwrap().kind, ActionKind::Done);
    }

    #[test]
    fn instinct_choice_skips_fallback() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let mut policy = EscalatingPolicy::new(InstinctPolicy::default(), Some(AlwaysChooseDone));
        let out = policy
            .decide(&space, &AgentGoal::new("Continue"), &[])
            .unwrap();
        assert_eq!(out.as_choice().unwrap().target_label, "Continue");
    }

    #[test]
    fn ranked_action_type_exists() {
        let _ = RankedAction {
            id: ActionId::try_new("WAIT").unwrap(),
            kind: ActionKind::Wait,
            label: "Wait".into(),
            confidence_millis: 0,
        };
    }
}
