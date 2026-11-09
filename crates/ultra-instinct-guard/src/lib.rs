//! Hard gate, tickets, and host guard decisions for ultra-instinct.
//!
//! [`gate()`](fn@crate::gate) is the one hard gate: the agent path and every
//! MCP / CLI preflight call refuse disabled, readonly, hidden, occluded,
//! front-layer, offscreen, missing and unsupported targets through it.
//! [`ActionTicket`]s bind an allowed choice to the world it was made in; the
//! executor must [`revalidate`] a ticket against a fresh observation before it
//! presses the exact target.
//!
//! [`guard()`] is the host preflight (MCP and CLI `guard`). It finds its
//! target by exact label, never by score:
//!
//! - **match**: a region matches when its label equals the query text after
//!   trimming, collapsing whitespace and ignoring case; a query role must be
//!   equal; `within` keeps only descendants of that container. A query with
//!   no text leaves the label unconstrained;
//! - **unique or refuse**: no match refuses `missing-target`; more than one
//!   visible match escalates `ambiguous`; a host-proposed id must be one of
//!   the matches (else `front-layer` when it is buried, `proposed-not-top`
//!   otherwise);
//! - **hard checks**: disabled, hidden, occluded, behind the front layer
//!   ([`world`]), offscreen, then the same [`gate::check`] the agent uses.
//!   Allow carries a [`WorldSnapshot::of_target`] ticket;
//! - **world change** ([`GuardRequest::seen_world`]): when focus, open dialogs,
//!   the clickable id set, or the occluded set differ from the observation the
//!   host decided on, the guard escalates with [`GuardReason::WorldChanged`]
//!   before it looks for a target.

#![forbid(unsafe_code)]

pub mod gate;
pub mod policy;
pub mod ticket;
pub mod world;

use std::fmt;

use ultra_instinct_core::{Action, InteractionManifold, InteractionRegion, LocateQuery, RegionId};
use ultra_instinct_geometry::{Availability, RegionState, Visibility};
use ultra_instinct_protocol::MatcherConfidence;

pub use gate::{
    check as gate_check, check_drop_target, gate, gate_drag, gate_drag_to_cell, gate_key,
    gate_point,
};
pub use policy::{
    consume_ticket_once_checked, host_of, is_activating_key, HostPattern, LabelGlob, Policy,
    PolicyError, Refusal, FORM_SUBMIT_UNKNOWN, UNOBSERVED_FOCUS,
};
#[allow(deprecated)]
pub use ticket::consume_ticket;
pub use ticket::{
    consume_ticket_once, issue_ticket, revalidate, world_fingerprint, ConsumeError, TicketLedger,
};
pub use ultra_instinct_protocol::{
    ActionTicket, GuardCandidate, GuardDecision, GuardEvidence, GuardReason, TicketDest,
    TicketInvalid,
};
pub use world::{
    blocker, neighborhood_of, with_front_layer, FrontLayer, LayerEntry, WorldSnapshot,
};

/// Most matches a decision lists as candidates.
const MAX_CANDIDATES: usize = 5;

/// What the host asked to do. Click is the only action the guard evaluates.
#[derive(Clone, Debug, PartialEq)]
pub struct GuardRequest {
    action: Action,
    query: LocateQuery,
    /// Optional host-proposed region id. When set, it must be one of the
    /// label matches.
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
    Confidence(ultra_instinct_protocol::ProtocolError),
    UnsupportedAction(Action),
    /// The query used a field the exact-label guard does not support (`near`
    /// or `position`).
    RemovedQueryField(&'static str),
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confidence(err) => write!(f, "{err}"),
            Self::UnsupportedAction(action) => {
                write!(f, "guard does not evaluate action `{action}`")
            }
            Self::RemovedQueryField(field) => write!(
                f,
                "`{field}` was removed with the matcher; name the region with `proposed` \
                 or scope the label with `within`"
            ),
        }
    }
}

impl std::error::Error for GuardError {}

impl From<ultra_instinct_protocol::ProtocolError> for GuardError {
    fn from(value: ultra_instinct_protocol::ProtocolError) -> Self {
        Self::Confidence(value)
    }
}

/// Find the target by exact label (and role / `within`), unique or refuse,
/// then run the hard checks and issue an `of_target` ticket.
pub fn guard(
    manifold: &InteractionManifold,
    request: &GuardRequest,
) -> Result<GuardDecision, GuardError> {
    if request.action != Action::Click {
        return Err(GuardError::UnsupportedAction(request.action));
    }
    let query = request.query();
    if query.near_ref().is_some() {
        return Err(GuardError::RemovedQueryField("near"));
    }
    if query.position_ref().is_some() {
        return Err(GuardError::RemovedQueryField("position"));
    }
    let matches = matches_of(manifold, query);
    let candidates = candidates_of(&matches);
    if let Some(seen) = request.seen_world_ref() {
        let now = WorldSnapshot::of(manifold, request.focused_id().cloned());
        if seen != &now {
            return Ok(GuardDecision::Escalate {
                reason: GuardReason::WorldChanged,
                candidates,
            });
        }
    }
    if matches.is_empty() {
        return Ok(GuardDecision::Refuse {
            reason: GuardReason::MissingTarget,
            candidates: Vec::new(),
        });
    }

    let target = if let Some(proposed) = request.proposed_id() {
        match matches.iter().find(|region| region.id() == proposed) {
            Some(region) => *region,
            None => {
                // The host picked a region the front layer blocks (the classic
                // "click behind the modal"). Say so instead of "not a match".
                let buried = manifold
                    .get(proposed)
                    .is_some_and(|region| blocker(manifold, region).is_some());
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
    } else {
        let viewport = manifold.viewport();
        let mut shown = matches
            .iter()
            .filter(|region| RegionState::of(viewport, region).visibility() != Visibility::Hidden);
        match (shown.next(), shown.next()) {
            (Some(_), Some(_)) => {
                return Ok(GuardDecision::Escalate {
                    reason: GuardReason::Ambiguous,
                    candidates,
                })
            }
            (Some(region), None) => *region,
            // Every match is hidden: the hidden check below refuses it.
            (None, _) => matches[0],
        }
    };

    // Evidence as a person sees the page: regions behind an open dialog read
    // occluded.
    let effective = with_front_layer(manifold);
    let shown = effective
        .get(target.id())
        .expect("same ids in raw and effective");
    let evidence = evidence_of(effective.viewport(), shown);
    let refuse = |reason| {
        Ok(GuardDecision::Refuse {
            reason,
            candidates: candidates.clone(),
        })
    };
    if !evidence.enabled {
        return refuse(GuardReason::Disabled);
    }
    if evidence.hidden {
        return refuse(GuardReason::Hidden);
    }
    if target.flags().occluded() && !evidence.offscreen {
        return refuse(GuardReason::Occluded);
    }
    if !evidence.offscreen && blocker(manifold, target).is_some() {
        return refuse(GuardReason::FrontLayer);
    }
    if evidence.offscreen {
        return refuse(GuardReason::Offscreen);
    }
    // Shared hard-gate checks (the function the agent path uses), so MCP
    // Allow tickets stay aligned with `gate`.
    if let Err(reason) = crate::gate::check(manifold, target, request.action()) {
        return refuse(reason);
    }

    let chosen = candidate_of(target);
    let ticket = issue_ticket(
        request.snapshot_id_value(),
        request.action(),
        &chosen,
        target,
        &WorldSnapshot::of_target(manifold, request.focused_id().cloned(), &chosen.id),
    );
    Ok(GuardDecision::Allow {
        target: chosen,
        confidence: MatcherConfidence::try_new(1.0)?,
        margin: None,
        evidence,
        ticket,
    })
}

/// Regions whose label equals the query text (trimmed, whitespace collapsed,
/// case ignored), with the query role and under `within`, in manifold order.
fn matches_of<'a>(
    manifold: &'a InteractionManifold,
    query: &LocateQuery,
) -> Vec<&'a InteractionRegion> {
    let text = query.text_ref().map(normalize_label);
    manifold
        .regions()
        .filter(|region| {
            text.as_ref()
                .is_none_or(|text| normalize_label(region.label()) == *text)
        })
        .filter(|region| query.role_ref().is_none_or(|role| region.role() == role))
        .filter(|region| {
            query
                .within_ref()
                .is_none_or(|container| descends_from(manifold, region, container))
        })
        .collect()
}

/// Trimmed, inner whitespace runs collapsed to one space, lowercased.
fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `container` is a strict ancestor of `region` in the parent chain. The walk
/// takes at most one step per region, so a malformed cycle cannot loop.
fn descends_from(
    manifold: &InteractionManifold,
    region: &InteractionRegion,
    container: &RegionId,
) -> bool {
    let mut parent = region.parent();
    for _ in 0..manifold.regions().count() {
        match parent {
            Some(id) if id == container => return true,
            Some(id) => parent = manifold.get(id).and_then(InteractionRegion::parent),
            None => return false,
        }
    }
    false
}

fn candidates_of(matches: &[&InteractionRegion]) -> Vec<GuardCandidate> {
    matches
        .iter()
        .take(MAX_CANDIDATES)
        .map(|region| candidate_of(region))
        .collect()
}

fn candidate_of(region: &InteractionRegion) -> GuardCandidate {
    GuardCandidate {
        id: region.id().clone(),
        role: region.role(),
        label: region.label().to_owned(),
        confidence: 1.0,
    }
}

fn evidence_of(viewport: ultra_instinct_core::Rect, region: &InteractionRegion) -> GuardEvidence {
    let state = RegionState::of(viewport, region);
    GuardEvidence {
        visible: state.visibility() == Visibility::Visible,
        enabled: state.availability() == Availability::Enabled,
        occluded: state.visibility() == Visibility::Occluded,
        hidden: state.visibility() == Visibility::Hidden,
        offscreen: state.visibility() == Visibility::Offscreen,
        role: region.role(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        parse_fixture, Rect, RegionFlags, RegionParts, Role, SourceMask, UnitInterval, Zone,
    };

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

    fn query(text: &str) -> LocateQuery {
        LocateQuery::new().text(text).unwrap().role(Role::Button)
    }

    fn button(text: &str) -> GuardRequest {
        GuardRequest::click(query(text))
    }

    fn id(raw: &str) -> RegionId {
        RegionId::try_new(raw).unwrap()
    }

    fn reason(decision: &GuardDecision) -> GuardReason {
        match decision {
            GuardDecision::Refuse { reason, .. } | GuardDecision::Escalate { reason, .. } => {
                *reason
            }
            other => panic!("expected refuse or escalate, got {other:?}"),
        }
    }

    fn allowed(decision: &GuardDecision) -> &str {
        match decision {
            GuardDecision::Allow { target, .. } => target.id.as_str(),
            other => panic!("expected allow, got {other:?}"),
        }
    }

    #[test]
    fn a_clear_button_is_allowed() {
        let m = manifold(vec![
            region("n1", "Sign in", 100.0, ""),
            region("n2", "Cancel", 400.0, ""),
        ]);
        match guard(&m, &button("Sign in")).unwrap() {
            GuardDecision::Allow {
                target,
                confidence,
                margin,
                ticket,
                ..
            } => {
                assert_eq!(target.id.as_str(), "n1");
                assert_eq!(target.confidence, 1.0);
                assert_eq!(confidence.get(), 1.0);
                assert_eq!(margin, None);
                assert_eq!(ticket.target_id.as_str(), "n1");
            }
            other => panic!("expected allow, got {other:?}"),
        }
    }

    #[test]
    fn a_different_label_is_a_missing_target() {
        let m = manifold(vec![region("n1", "Cancel", 100.0, "")]);
        // Close is not equal.
        for text in ["Sign in", "Cancel now", "Cance"] {
            match guard(&m, &button(text)).unwrap() {
                GuardDecision::Refuse {
                    reason: GuardReason::MissingTarget,
                    candidates,
                } => assert!(candidates.is_empty(), "{text}"),
                other => panic!("{text}: expected missing-target, got {other:?}"),
            }
        }
        // A role mismatch is not a match either.
        let link = GuardRequest::click(LocateQuery::new().text("Cancel").unwrap().role(Role::Link));
        assert_eq!(
            reason(&guard(&m, &link).unwrap()),
            GuardReason::MissingTarget
        );
    }

    #[test]
    fn twins_escalate_ambiguous() {
        let m = manifold(vec![
            region("n1", "Send", 100.0, ""),
            region("n2", "Send", 400.0, ""),
        ]);
        match guard(&m, &button("Send")).unwrap() {
            GuardDecision::Escalate {
                reason: GuardReason::Ambiguous,
                candidates,
            } => {
                let ids: Vec<_> = candidates.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, ["n1", "n2"]);
                assert!(candidates.iter().all(|c| c.confidence == 1.0));
            }
            other => panic!("expected escalate ambiguous, got {other:?}"),
        }
        // Proposing one twin makes it the target.
        let req = button("Send").proposed(id("n2"));
        assert_eq!(allowed(&guard(&m, &req).unwrap()), "n2");
    }

    #[test]
    fn candidates_stop_at_five() {
        let m = manifold(
            (0..7)
                .map(|i| region(&format!("n{i}"), "Send", 10.0 + 100.0 * f64::from(i), ""))
                .collect(),
        );
        match guard(&m, &button("Send")).unwrap() {
            GuardDecision::Escalate { candidates, .. } => assert_eq!(candidates.len(), 5),
            other => panic!("expected escalate, got {other:?}"),
        }
    }

    #[test]
    fn a_hidden_twin_is_ignored() {
        let m = manifold(vec![
            region("n1", "Send", 100.0, "hidden"),
            region("n2", "Send", 400.0, ""),
        ]);
        assert_eq!(allowed(&guard(&m, &button("Send")).unwrap()), "n2");
        // Proposing the hidden twin makes it the target, and it is refused.
        let req = button("Send").proposed(id("n1"));
        assert_eq!(reason(&guard(&m, &req).unwrap()), GuardReason::Hidden);
        // All hidden: the first match is the target and the hidden check
        // refuses it.
        let m = manifold(vec![
            region("n1", "Send", 100.0, "hidden"),
            region("n2", "Send", 400.0, "hidden"),
        ]);
        assert_eq!(
            reason(&guard(&m, &button("Send")).unwrap()),
            GuardReason::Hidden
        );
    }

    #[test]
    fn within_scopes_twins() {
        let m =
            parse_fixture(include_str!("../../../fixtures/twin-suspend-rows.manifold")).unwrap();
        let scoped = |container: &str| GuardRequest::click(query("Suspend").within(id(container)));
        assert_eq!(
            allowed(&guard(&m, &scoped("row-beta")).unwrap()),
            "beta-suspend"
        );
        assert_eq!(
            allowed(&guard(&m, &scoped("row-alpha")).unwrap()),
            "alpha-suspend"
        );
        // A deeper ancestor holds both twins.
        assert_eq!(
            reason(&guard(&m, &scoped("servers")).unwrap()),
            GuardReason::Ambiguous
        );
        // The container is not its own descendant.
        let row = GuardRequest::click(
            LocateQuery::new()
                .text("beta")
                .unwrap()
                .within(id("row-beta")),
        );
        assert_eq!(
            reason(&guard(&m, &row).unwrap()),
            GuardReason::MissingTarget
        );
    }

    #[test]
    fn case_and_whitespace_are_ignored() {
        let m = manifold(vec![region("n1", "  Sign\tin ", 100.0, "")]);
        for text in ["sign in", "SIGN   IN", " Sign in\n"] {
            assert_eq!(
                allowed(&guard(&m, &button(text)).unwrap()),
                "n1",
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_disabled_target_is_refused() {
        let m = manifold(vec![region("n1", "Save", 100.0, "disabled")]);
        assert_eq!(
            reason(&guard(&m, &button("Save")).unwrap()),
            GuardReason::Disabled
        );
    }

    #[test]
    fn a_buried_page_button_under_a_dialog_is_refused_front_layer() {
        // The dialog's "Delete project permanently" is not the label asked
        // for, so the guard never reroutes the click into the dialog.
        let m = parse_fixture(
            "viewport w=1440 h=900\n\
             region id=page-delete role=button label=\"Delete project\" x=1200 y=780 w=160 h=36 actions=click sources=dom\n\
             region id=dlg role=dialog label=\"Are you sure?\" x=520 y=300 w=400 h=240 actions=focus sources=dom flags=modal\n\
             region id=dlg-delete role=button label=\"Delete project permanently\" x=560 y=480 w=200 h=36 actions=click parent=dlg sources=dom\n",
        )
        .unwrap();
        match guard(&m, &button("Delete project")).unwrap() {
            GuardDecision::Refuse {
                reason: GuardReason::FrontLayer,
                candidates,
            } => {
                let ids: Vec<_> = candidates.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, ["page-delete"]);
            }
            other => panic!("expected refuse front-layer, got {other:?}"),
        }
        // Proposing the dialog button is not a match for that label, and it
        // is not buried.
        let req = button("Delete project").proposed(id("dlg-delete"));
        assert_eq!(
            reason(&guard(&m, &req).unwrap()),
            GuardReason::ProposedNotTop
        );
    }

    #[test]
    fn a_proposed_non_match_is_proposed_not_top() {
        let m = manifold(vec![
            region("n1", "Sign in", 100.0, ""),
            region("n2", "Cancel", 400.0, ""),
        ]);
        let req = button("Sign in").proposed(id("n2"));
        match guard(&m, &req).unwrap() {
            GuardDecision::Refuse {
                reason: GuardReason::ProposedNotTop,
                candidates,
            } => assert_eq!(candidates.len(), 1),
            other => panic!("expected proposed-not-top, got {other:?}"),
        }
    }

    #[test]
    fn near_and_position_are_errors() {
        let m = manifold(vec![region("n1", "Send", 100.0, "")]);
        let near = GuardRequest::click(query("Send").near(Some(id("n1"))));
        let err = guard(&m, &near).unwrap_err();
        assert_eq!(err, GuardError::RemovedQueryField("near"));
        let message = err.to_string();
        assert!(
            message.contains("proposed") && message.contains("within"),
            "{message}"
        );
        let position = GuardRequest::click(query("Send").position(Zone::Left));
        assert_eq!(
            guard(&m, &position).unwrap_err(),
            GuardError::RemovedQueryField("position")
        );
    }
}
