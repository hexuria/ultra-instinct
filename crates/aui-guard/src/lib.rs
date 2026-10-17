//! Action firewall decisions for ultra-instinct.
//!
//! Ultra-Instinct does not click. A host proposes a target; this crate ranks,
//! gates, and returns [`GuardDecision`]. [`GuardDecision::Allow`] carries an
//! [`ActionTicket`]. Browser Use (or another executor) must [`revalidate`] that
//! ticket against a fresh observation, then press the exact target.
//!
//! Hard-gate direction: occluded / front-layer / disabled / hidden / wrong
//! ancestry / stale ticket should become *impossible* before ranking (blocked
//! candidates stay as evidence). Ranking then chooses among viable candidates
//! only. Until that split lands, some safety still mixes into scores (see
//! `buried_better_label` in `decide`); do not rip that path in the same change
//! as the ticket lease.
//!
//! The guard judges the proposal against the world as it is now, not as the
//! host last saw it:
//!
//! - **front layer** ([`world`]): a target behind an open dialog is refused
//!   with [`GuardReason::FrontLayer`], and ranking runs on a copy where such
//!   regions carry the occluded penalty, so the dialog's own control wins;
//!   when a buried/occluded region still matches the query label better than
//!   that top (common once hit-test already set `occluded`), refuse rather
//!   than allowing the weaker dialog label;
//! - **context** (`LocateQuery::within` / `LocateQuery::near`): ancestry and
//!   the focused region scope twin labels;
//! - **world change** ([`GuardRequest::seen_world`]): when focus, open dialogs,
//!   the clickable id set, or the occluded set differ from the observation the
//!   host decided on, the guard escalates with [`GuardReason::WorldChanged`].

#![forbid(unsafe_code)]

pub mod gate;
pub mod ticket;
pub mod world;

use std::fmt;

use aui_core::{Action, InteractionManifold, InteractionRegion, LocateQuery, RegionId};
use aui_protocol::MatcherConfidence;
use aui_resonance::{
    default_matcher, weighted_semantic, Match, RegionMatcher, RegionState, TEXT_MISS_CAP,
};

pub use aui_protocol::{
    ActionTicket, GuardCandidate, GuardDecision, GuardEvidence, GuardReason, TicketDest,
    TicketInvalid,
};
pub use gate::{check as gate_check, check_drop_target, gate, gate_drag, gate_point};
#[allow(deprecated)]
pub use ticket::consume_ticket;
pub use ticket::{
    consume_ticket_once, issue_ticket, revalidate, world_fingerprint, ConsumeError, TicketLedger,
};
pub use world::{
    blocker, neighborhood_of, with_front_layer, FrontLayer, LayerEntry, WorldSnapshot,
};

/// Raw confidence below this never allows. Not a probability.
/// Host / MCP preflight allow floor. The owned agent path does **not** use
/// this: Instinct chooses among a finite action space, then [`gate()`](fn@crate::gate) applies
/// hard refuses only. Keep 0.55 so historical A5/A6 / combo benches stay
/// comparable; do not raise or remove without updating those arms.
pub const MIN_ALLOW_CONFIDENCE: f64 = 0.55;

/// Minimum raw gap between top and runner-up. Below this → ambiguous.
pub const MIN_ALLOW_MARGIN: f64 = 0.05;

/// Tolerance on the margin only (`0.6 - 0.55` is not exact in f64).
pub const MARGIN_EPSILON: f64 = 1e-9;

pub const MIN_ALLOW_CONFIDENCE_MILLIS: i32 = 550;
pub const MIN_ALLOW_MARGIN_MILLIS: i32 = 50;

const _: () = assert!(TEXT_MISS_CAP < MIN_ALLOW_CONFIDENCE);

/// What the host asked to do. Click is the only action the guard evaluates today.
#[derive(Clone, Debug, PartialEq)]
pub struct GuardRequest {
    action: Action,
    query: LocateQuery,
    /// Optional host-proposed region id. When set, it must be the top match.
    proposed: Option<RegionId>,
    /// Focused region of the *current* observation (from PageState).
    focused: Option<RegionId>,
    /// World of the observation the host decided on, if it said.
    seen_world: Option<WorldSnapshot>,
    /// Host observation id (MCP snapshot ring). Embedded in the Allow ticket.
    snapshot_id: u64,
}

impl GuardRequest {
    pub fn click(query: LocateQuery) -> Self {
        Self {
            action: Action::Click,
            query,
            proposed: None,
            focused: None,
            seen_world: None,
            snapshot_id: 0,
        }
    }

    /// Observation / snapshot id the host is deciding against.
    pub fn snapshot_id(mut self, id: u64) -> Self {
        self.snapshot_id = id;
        self
    }

    pub fn snapshot_id_value(&self) -> u64 {
        self.snapshot_id
    }

    pub fn proposed(mut self, id: RegionId) -> Self {
        self.proposed = Some(id);
        self
    }

    /// Focused region id of the observation being guarded (now).
    pub fn focused(mut self, id: Option<RegionId>) -> Self {
        self.focused = id;
        self
    }

    /// World snapshot the host saw when it chose this action. When it
    /// differs from the current world, the guard escalates `world-changed`.
    pub fn seen_world(mut self, world: WorldSnapshot) -> Self {
        self.seen_world = Some(world);
        self
    }

    pub fn seen_world_ref(&self) -> Option<&WorldSnapshot> {
        self.seen_world.as_ref()
    }

    pub fn focused_id(&self) -> Option<&RegionId> {
        self.focused.as_ref()
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn query(&self) -> &LocateQuery {
        &self.query
    }

    pub fn proposed_id(&self) -> Option<&RegionId> {
        self.proposed.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GuardError {
    Rank(aui_resonance::ResonanceError),
    Confidence(aui_protocol::ProtocolError),
    UnsupportedAction(Action),
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rank(err) => write!(f, "{err}"),
            Self::Confidence(err) => write!(f, "{err}"),
            Self::UnsupportedAction(action) => {
                write!(f, "guard does not evaluate action `{action}`")
            }
        }
    }
}

impl std::error::Error for GuardError {}

impl From<aui_resonance::ResonanceError> for GuardError {
    fn from(value: aui_resonance::ResonanceError) -> Self {
        Self::Rank(value)
    }
}

impl From<aui_protocol::ProtocolError> for GuardError {
    fn from(value: aui_protocol::ProtocolError) -> Self {
        Self::Confidence(value)
    }
}

/// Rank with the default matcher and decide allow / refuse / escalate.
pub fn guard(
    manifold: &InteractionManifold,
    request: &GuardRequest,
) -> Result<GuardDecision, GuardError> {
    guard_with(manifold, request, &default_matcher())
}

/// Rank with a caller-supplied matcher and decide.
pub fn guard_with<M: RegionMatcher>(
    manifold: &InteractionManifold,
    request: &GuardRequest,
    matcher: &M,
) -> Result<GuardDecision, GuardError> {
    if request.action != Action::Click {
        return Err(GuardError::UnsupportedAction(request.action));
    }
    // Rank on the world as a person sees it: regions behind an open dialog
    // carry the occluded penalty.
    let effective = with_front_layer(manifold);
    let ranked = matcher.rank(request.query(), effective.as_ref())?;
    // Ranking on the raw observation tells whether the best label match is
    // one the front layer buried.
    let raw_ranked = matcher.rank(request.query(), manifold)?;
    if let Some(seen) = request.seen_world_ref() {
        let now = WorldSnapshot::of(manifold, request.focused_id().cloned());
        if seen != &now {
            return Ok(GuardDecision::Escalate {
                reason: GuardReason::WorldChanged,
                candidates: candidates_of(&effective, &ranked),
            });
        }
    }
    decide(manifold, &effective, request, &ranked, &raw_ranked)
}

fn candidates_of(manifold: &InteractionManifold, ranked: &[Match]) -> Vec<GuardCandidate> {
    ranked
        .iter()
        .take(5)
        .filter_map(|m| candidate_from(manifold, m))
        .collect()
}

/// `raw` is the observation; `effective` is the same regions with the front
/// layer applied (see [`with_front_layer`]). Ranking ran on `effective`.
fn decide(
    raw: &InteractionManifold,
    effective: &InteractionManifold,
    request: &GuardRequest,
    ranked: &[Match],
    raw_ranked: &[Match],
) -> Result<GuardDecision, GuardError> {
    let candidates = candidates_of(effective, ranked);

    if ranked.is_empty() || candidates.is_empty() {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::MissingTarget,
            candidates,
        });
    }

    let top = &ranked[0];
    let top_conf = top.confidence();

    if let Some(proposed) = request.proposed_id() {
        if top.id() != proposed {
            // The host picked a region the front layer blocks (the classic
            // "click behind the modal"). Say so instead of "not top".
            let buried = raw
                .get(proposed)
                .is_some_and(|region| blocker(raw, region).is_some());
            return Ok(GuardDecision::Refuse {
                reason: if buried {
                    GuardReason::FrontLayer
                } else {
                    GuardReason::ProposedNotTop
                },
                candidates,
            });
        }
    }

    // The best match for what the host asked is behind the dialog, and a
    // weaker match (often the dialog's own confirm button) took the top only
    // because of the occluded penalty. Do not reroute the host's click into a
    // dialog it may not have seen: refuse. Only a blocked region can score
    // higher raw than the effective top, since unblocked scores are equal.
    if raw_ranked
        .iter()
        .any(|m| m.confidence() > top_conf + MARGIN_EPSILON)
    {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::FrontLayer,
            candidates,
        });
    }

    // Hit-test often marks the buried control occluded on the raw manifold
    // already, so the confidence reroute above never fires: both ranks apply
    // the same occluded penalty. Still refuse when any buried/occluded region
    // matches the query's label better than the effective top (HGRA + dialog
    // "Delete" vs buried "Delete project").
    //
    // Hard-gate TODO: move this (and front-layer / occluded / disabled) into a
    // pre-rank impossible set so ranking never sees buried candidates as
    // viable. Keep `buried_better_label` until that split; do not delete it in
    // the ActionTicket PR.
    let top_region_for_semantic = raw.get(top.id()).expect("ranked id comes from manifold");
    let top_semantic = weighted_semantic(request.query(), top_region_for_semantic);
    let buried_better_label = raw.regions().any(|region| {
        if region.id() == top.id() {
            return false;
        }
        let buried = region.flags().occluded() || blocker(raw, region).is_some();
        buried && weighted_semantic(request.query(), region) > top_semantic + MARGIN_EPSILON
    });
    if buried_better_label {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::FrontLayer,
            candidates,
        });
    }

    let top_region = effective
        .get(top.id())
        .expect("ranked id comes from manifold");
    let raw_region = raw.get(top.id()).expect("same ids in raw and effective");

    let evidence = evidence_of(effective.viewport(), top_region);
    if !evidence.enabled {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::Disabled,
            candidates,
        });
    }
    if evidence.hidden {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::Hidden,
            candidates,
        });
    }
    if raw_region.flags().occluded() && !evidence.offscreen {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::Occluded,
            candidates,
        });
    }
    if !evidence.offscreen && blocker(raw, raw_region).is_some() {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::FrontLayer,
            candidates,
        });
    }
    if evidence.offscreen {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::Offscreen,
            candidates,
        });
    }

    // Shared hard-gate checks (same function the agent path uses). Ranking
    // already filtered most of these; this keeps MCP Allow tickets aligned
    // with `gate` (including future hard refuses such as readonly on Type).
    if let Err(reason) = crate::gate::check(raw, raw_region, request.action()) {
        return Ok(GuardDecision::Refuse { reason, candidates });
    }

    if top_conf < MIN_ALLOW_CONFIDENCE {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::LowConfidence,
            candidates,
        });
    }

    let runner_up = ranked.get(1).map(|m| m.confidence());
    if let Some(second) = runner_up {
        let margin = top_conf - second;
        if !margin.is_finite() || margin < MIN_ALLOW_MARGIN - MARGIN_EPSILON {
            return Ok(GuardDecision::Escalate {
                reason: GuardReason::Ambiguous,
                candidates,
            });
        }
        let target = candidates[0].clone();
        let ticket = issue_ticket(
            request.snapshot_id_value(),
            request.action(),
            &target,
            raw_region,
            &WorldSnapshot::of_target(raw, request.focused_id().cloned(), &target.id),
        );
        return Ok(GuardDecision::Allow {
            target,
            confidence: MatcherConfidence::try_new(top_conf)?,
            margin: Some(MatcherConfidence::try_new(margin)?),
            evidence,
            ticket,
        });
    }

    let target = candidates[0].clone();
    let ticket = issue_ticket(
        request.snapshot_id_value(),
        request.action(),
        &target,
        raw_region,
        &WorldSnapshot::of_target(raw, request.focused_id().cloned(), &target.id),
    );
    Ok(GuardDecision::Allow {
        target,
        confidence: MatcherConfidence::try_new(top_conf)?,
        margin: None,
        evidence,
        ticket,
    })
}

fn candidate_from(manifold: &InteractionManifold, m: &Match) -> Option<GuardCandidate> {
    let region = manifold.regions().find(|r| r.id() == m.id())?;
    Some(GuardCandidate {
        id: region.id().clone(),
        role: region.role(),
        label: region.label().to_owned(),
        confidence: m.confidence(),
    })
}

fn evidence_of(viewport: aui_core::Rect, region: &InteractionRegion) -> GuardEvidence {
    let state = RegionState::of(viewport, region);
    use aui_resonance::{Availability, Visibility};
    GuardEvidence {
        visible: state.visibility() == Visibility::Visible,
        enabled: state.availability() == Availability::Enabled,
        occluded: state.visibility() == Visibility::Occluded,
        hidden: state.visibility() == Visibility::Hidden,
        offscreen: state.visibility() == Visibility::Offscreen,
        role: region.role(),
    }
}

/// Display helper: raw confidence as truncated millis.
pub fn display_millis(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let scaled = (value * 1000.0).trunc();
    if scaled > f64::from(i32::MAX) {
        i32::MAX
    } else if scaled < f64::from(i32::MIN) {
        i32::MIN
    } else {
        scaled as i32
    }
}

pub fn margin_millis(top: f64, runner_up: f64) -> i32 {
    display_millis(top - runner_up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aui_core::{Action, Rect, RegionFlags, RegionParts, Role, SourceMask, UnitInterval};

    fn region(id: &str, label: &str, x: f64, flags: &str) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.into(),
            rect: Rect::try_new(x, 10.0, 80.0, 24.0).unwrap(),
            actions: vec![Action::Click, Action::Focus],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::parse_list(flags).unwrap(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn manifold(regions: Vec<InteractionRegion>) -> InteractionManifold {
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 800.0, 600.0).unwrap(),
            regions,
            0,
        )
        .unwrap()
    }

    #[test]
    fn allows_a_clear_sign_in_button() {
        let m = manifold(vec![region("n1", "Sign in", 100.0, "")]);
        let req = GuardRequest::click(
            LocateQuery::new()
                .text("Sign in")
                .unwrap()
                .role(Role::Button),
        );
        let decision = guard(&m, &req).unwrap();
        assert!(
            matches!(decision, GuardDecision::Allow { .. }),
            "{decision:?}"
        );
    }

    #[test]
    fn refuses_low_confidence_text_miss() {
        let m = manifold(vec![region("n1", "Cancel", 100.0, "")]);
        let req = GuardRequest::click(
            LocateQuery::new()
                .text("Sign in")
                .unwrap()
                .role(Role::Button),
        );
        let decision = guard(&m, &req).unwrap();
        match decision {
            GuardDecision::Refuse {
                reason: GuardReason::LowConfidence | GuardReason::MissingTarget,
                ..
            } => {}
            other => panic!("expected refuse low/missing, got {other:?}"),
        }
    }

    #[test]
    fn escalates_identical_twin_buttons() {
        let m = manifold(vec![
            region("n1", "Send", 100.0, ""),
            region("n2", "Send", 400.0, ""),
        ]);
        let req = GuardRequest::click(LocateQuery::new().text("Send").unwrap().role(Role::Button));
        let decision = guard(&m, &req).unwrap();
        match decision {
            GuardDecision::Escalate {
                reason: GuardReason::Ambiguous,
                candidates,
            } => assert!(candidates.len() >= 2),
            other => panic!("expected escalate ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn refuses_disabled_target() {
        let m = manifold(vec![region("n1", "Save", 100.0, "disabled")]);
        let req = GuardRequest::click(LocateQuery::new().text("Save").unwrap().role(Role::Button));
        let decision = guard(&m, &req).unwrap();
        match decision {
            GuardDecision::Refuse {
                reason: GuardReason::Disabled,
                ..
            } => {}
            other => panic!("expected refuse disabled, got {other:?}"),
        }
    }

    #[test]
    fn does_not_reroute_a_buried_best_match_into_the_dialog() {
        // Without the reroute rule the occluded penalty would hand the top to
        // the dialog's "Delete project permanently" (0.917 vs 0.8, margin
        // above the gate) and the host's click would confirm a dialog it may
        // never have seen.
        let m = aui_core::parse_fixture(
            "viewport w=1440 h=900\n\
             region id=page-delete role=button label=\"Delete project\" x=1200 y=780 w=160 h=36 actions=click sources=dom\n\
             region id=dlg role=dialog label=\"Are you sure?\" x=520 y=300 w=400 h=240 actions=focus sources=dom flags=modal\n\
             region id=dlg-delete role=button label=\"Delete project permanently\" x=560 y=480 w=200 h=36 actions=click parent=dlg sources=dom\n",
        )
        .unwrap();
        let req = GuardRequest::click(
            LocateQuery::new()
                .text("Delete project")
                .unwrap()
                .role(Role::Button),
        );
        let effective = with_front_layer(&m);
        let ranked = default_matcher()
            .rank(req.query(), effective.as_ref())
            .unwrap();
        assert_eq!(ranked[0].id().as_str(), "dlg-delete");
        assert!(ranked[0].confidence() - ranked[1].confidence() >= MIN_ALLOW_MARGIN);
        match guard(&m, &req).unwrap() {
            GuardDecision::Refuse {
                reason: GuardReason::FrontLayer,
                ..
            } => {}
            other => panic!("expected refuse front-layer, got {other:?}"),
        }
    }

    #[cfg(feature = "hgra")]
    #[test]
    fn hgra_refuses_weaker_dialog_label_when_buried_exact_is_already_occluded() {
        use aui_resonance::HgraMatcher;
        let m = aui_core::parse_fixture(
            "viewport w=1440 h=900\n\
             region id=page-delete role=button label=\"Delete project\" x=1200 y=780 w=160 h=36 actions=click sources=dom flags=occluded\n\
             region id=dlg role=dialog label=\"Delete project?\" x=520 y=300 w=400 h=240 actions=focus sources=dom flags=modal\n\
             region id=dlg-delete role=button label=\"Delete\" x=720 y=480 w=100 h=36 actions=click parent=dlg sources=dom\n",
        )
        .unwrap();
        let req = GuardRequest::click(
            LocateQuery::new()
                .text("Delete project")
                .unwrap()
                .role(Role::Button),
        );
        let ranked = HgraMatcher::default()
            .rank(req.query(), with_front_layer(&m).as_ref())
            .unwrap();
        assert_eq!(
            ranked[0].id().as_str(),
            "dlg-delete",
            "precondition: HGRA still tops the short dialog label"
        );
        match guard_with(&m, &req, &HgraMatcher::default()).unwrap() {
            GuardDecision::Refuse {
                reason: GuardReason::FrontLayer | GuardReason::Occluded,
                ..
            } => {}
            other => panic!("expected refuse front-layer or occluded, got {other:?}"),
        }
    }

    #[test]
    fn refuses_weaker_dialog_label_when_buried_exact_is_already_occluded() {
        // Live modal shape: observe hit-test already marked the page button
        // occluded, dialog button is a shorter label. HGRA ranks the dialog
        // top; without the label rule the confidence reroute is a no-op.
        let m = aui_core::parse_fixture(
            "viewport w=1440 h=900\n\
             region id=page-delete role=button label=\"Delete project\" x=1200 y=780 w=160 h=36 actions=click sources=dom flags=occluded\n\
             region id=dlg role=dialog label=\"Delete project?\" x=520 y=300 w=400 h=240 actions=focus sources=dom flags=modal\n\
             region id=dlg-delete role=button label=\"Delete\" x=720 y=480 w=100 h=36 actions=click parent=dlg sources=dom\n",
        )
        .unwrap();
        let req = GuardRequest::click(
            LocateQuery::new()
                .text("Delete project")
                .unwrap()
                .role(Role::Button),
        );
        match guard(&m, &req).unwrap() {
            GuardDecision::Refuse {
                reason: GuardReason::FrontLayer | GuardReason::Occluded,
                ..
            } => {}
            other => panic!("expected refuse front-layer or occluded, got {other:?}"),
        }
    }

    #[test]
    fn refuses_when_proposed_is_not_top() {
        let m = manifold(vec![
            region("n1", "Sign in", 100.0, ""),
            region("n2", "Cancel", 400.0, ""),
        ]);
        let req = GuardRequest::click(
            LocateQuery::new()
                .text("Sign in")
                .unwrap()
                .role(Role::Button),
        )
        .proposed(RegionId::try_new("n2").unwrap());
        let decision = guard(&m, &req).unwrap();
        match decision {
            GuardDecision::Refuse {
                reason: GuardReason::ProposedNotTop,
                ..
            } => {}
            other => panic!("expected proposed-not-top, got {other:?}"),
        }
    }
}
