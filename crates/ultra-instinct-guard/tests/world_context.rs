//! Adversarial world-context fixtures for the guard.
//!
//! These are the bar for "variable change", not the 5-case HGRA corpus
//! (`hgra_remeasure.rs`). That corpus only ranks labels on static pages that
//! never open a dialog and never repeat a label under two parents, so a
//! matcher can score 5/5 on it and still click behind a modal. Every fixture
//! here is built so a label-only gate gets it wrong, and each test first
//! shows that failure before it shows the context that fixes it.

use ultra_instinct_core::{parse_fixture, InteractionManifold, LocateQuery, RegionId, Role};
use ultra_instinct_guard::{
    guard, FrontLayer, GuardDecision, GuardReason, GuardRequest, WorldSnapshot,
};
use ultra_instinct_resonance::{RegionMatcher, WeightedMatcher};

const TWINS: &str = include_str!("../../../fixtures/twin-suspend-rows.manifold");
const MODAL: &str = include_str!("../../../fixtures/modal-confirm.manifold");
const CLOSED: &str = include_str!("../../../fixtures/modal-confirm-closed.manifold");

fn load(text: &str) -> InteractionManifold {
    parse_fixture(text).unwrap()
}

fn id(raw: &str) -> RegionId {
    RegionId::try_new(raw).unwrap()
}

fn button(text: &str) -> LocateQuery {
    LocateQuery::new().text(text).unwrap().role(Role::Button)
}

fn allowed(decision: &GuardDecision) -> &str {
    match decision {
        GuardDecision::Allow { target, .. } => target.id.as_str(),
        other => panic!("expected allow, got {other:?}"),
    }
}

fn refused(decision: &GuardDecision) -> GuardReason {
    match decision {
        GuardDecision::Refuse { reason, .. } => *reason,
        other => panic!("expected refuse, got {other:?}"),
    }
}

fn escalated(decision: &GuardDecision) -> GuardReason {
    match decision {
        GuardDecision::Escalate { reason, .. } => *reason,
        other => panic!("expected escalate, got {other:?}"),
    }
}

/// The label-only gate: what the guard did before world context. Top must
/// clear 0.55 and the margin 0.05 on the raw observation.
fn label_only_gate_allows(manifold: &InteractionManifold, query: &LocateQuery) -> Option<String> {
    let ranked = WeightedMatcher::default().rank(query, manifold).unwrap();
    let top = &ranked[0];
    let margin = ranked
        .get(1)
        .map_or(f64::INFINITY, |next| top.confidence() - next.confidence());
    (top.confidence() >= 0.55 && margin >= 0.05 - 1e-9).then(|| top.id().as_str().to_owned())
}

// ---------------------------------------------------------------- twins ---

#[test]
fn twin_suspend_without_context_cannot_be_resolved() {
    let m = load(TWINS);
    assert_eq!(label_only_gate_allows(&m, &button("Suspend")), None);
    let decision = guard(&m, &GuardRequest::click(button("Suspend"))).unwrap();
    assert_eq!(escalated(&decision), GuardReason::Ambiguous);
}

#[test]
fn twin_suspend_within_a_row_allows_that_row_only() {
    let m = load(TWINS);
    let beta = guard(
        &m,
        &GuardRequest::click(button("Suspend").within(id("row-beta"))),
    )
    .unwrap();
    assert_eq!(allowed(&beta), "beta-suspend");
    let alpha = guard(
        &m,
        &GuardRequest::click(button("Suspend").within(id("row-alpha"))),
    )
    .unwrap();
    assert_eq!(allowed(&alpha), "alpha-suspend");
    // Deeper ancestors constrain too, but the table holds both twins.
    let table = guard(
        &m,
        &GuardRequest::click(button("Suspend").within(id("servers"))),
    )
    .unwrap();
    assert_eq!(escalated(&table), GuardReason::Ambiguous);
}

#[test]
fn twin_suspend_near_the_focused_field_allows_the_focused_row() {
    let m = load(TWINS);
    let decision = guard(
        &m,
        &GuardRequest::click(button("Suspend").near(Some(id("beta-host")))),
    )
    .unwrap();
    assert_eq!(allowed(&decision), "beta-suspend");
    match decision {
        GuardDecision::Allow { margin, .. } => assert!(margin.unwrap().get() >= 0.05),
        _ => unreachable!(),
    }
}

#[test]
fn focus_outside_every_row_falls_back_to_the_default_ranking() {
    // Cargo-runner rule: no cursor context, default command. Here the default
    // is still ambiguous, so the guard escalates instead of guessing.
    let m = load(TWINS);
    for anchor in [Some(id("search")), Some(id("vanished")), None] {
        let decision = guard(
            &m,
            &GuardRequest::click(button("Suspend").near(anchor.clone())),
        )
        .unwrap();
        assert_eq!(escalated(&decision), GuardReason::Ambiguous, "{anchor:?}");
    }
}

#[test]
fn proposing_the_other_twin_under_context_refuses() {
    let m = load(TWINS);
    let request =
        GuardRequest::click(button("Suspend").within(id("row-beta"))).proposed(id("alpha-suspend"));
    assert_eq!(
        refused(&guard(&m, &request).unwrap()),
        GuardReason::ProposedNotTop
    );
}

// ---------------------------------------------------------- front layer ---

#[test]
fn a_page_button_under_an_open_modal_is_refused_not_allowed() {
    let m = load(MODAL);
    // Before world context, the gate allowed the buried button.
    assert_eq!(
        label_only_gate_allows(&m, &button("Delete project")).as_deref(),
        Some("page-delete")
    );
    let decision = guard(&m, &GuardRequest::click(button("Delete project"))).unwrap();
    assert_eq!(refused(&decision), GuardReason::FrontLayer);
    // Proposing it by id is refused for the same reason.
    let request = GuardRequest::click(button("Delete project")).proposed(id("page-delete"));
    assert_eq!(
        refused(&guard(&m, &request).unwrap()),
        GuardReason::FrontLayer
    );
}

#[test]
fn a_twin_label_resolves_to_the_dialog_and_the_buried_twin_is_refused() {
    let m = load(MODAL);
    // Raw ranking ties the two "Cancel" buttons: the old gate escalated.
    assert_eq!(label_only_gate_allows(&m, &button("Cancel")), None);
    let decision = guard(&m, &GuardRequest::click(button("Cancel"))).unwrap();
    assert_eq!(allowed(&decision), "confirm-cancel");
    // The host (for example a vision model that saw the page before the
    // dialog) proposes the page's Cancel. Refused: it is behind the dialog.
    let request = GuardRequest::click(button("Cancel")).proposed(id("page-cancel"));
    assert_eq!(
        refused(&guard(&m, &request).unwrap()),
        GuardReason::FrontLayer
    );
    // The dialog's own Delete is allowed over the buried "Delete project".
    let delete = guard(&m, &GuardRequest::click(button("Delete"))).unwrap();
    assert_eq!(allowed(&delete), "confirm-delete");
}

#[test]
fn the_same_page_without_the_dialog_allows_the_page_button() {
    let m = load(CLOSED);
    let decision = guard(&m, &GuardRequest::click(button("Delete project"))).unwrap();
    assert_eq!(allowed(&decision), "page-delete");
}

#[test]
fn a_host_that_decided_before_the_dialog_opened_is_escalated_world_changed() {
    let before = load(CLOSED);
    let now = load(MODAL);
    let seen = WorldSnapshot::of(&before, None);
    assert!(seen.front_layer().is_empty());
    // Even for a target inside the dialog: the host never saw it.
    for text in ["Delete project", "Cancel"] {
        let request = GuardRequest::click(button(text)).seen_world(seen.clone());
        assert_eq!(
            escalated(&guard(&now, &request).unwrap()),
            GuardReason::WorldChanged,
            "{text}"
        );
    }
    // A host that saw the dialog gets the normal decision.
    let request = GuardRequest::click(button("Cancel")).seen_world(WorldSnapshot::of(&now, None));
    assert_eq!(allowed(&guard(&now, &request).unwrap()), "confirm-cancel");
}

#[test]
fn world_changed_when_focus_moves_between_observations() {
    let m = load(TWINS);
    let seen = WorldSnapshot::of(&m, Some(id("beta-host")));
    let request = GuardRequest::click(button("Suspend").near(Some(id("alpha-host"))))
        .focused(Some(id("alpha-host")))
        .seen_world(seen);
    assert_eq!(
        escalated(&guard(&m, &request).unwrap()),
        GuardReason::WorldChanged
    );
    let same = GuardRequest::click(button("Suspend").near(Some(id("alpha-host"))))
        .focused(Some(id("alpha-host")))
        .seen_world(WorldSnapshot::of(&m, Some(id("alpha-host"))));
    assert_eq!(allowed(&guard(&m, &same).unwrap()), "alpha-suspend");
}

#[test]
fn world_changed_when_occluded_set_grows() {
    let clear = ultra_instinct_core::parse_fixture(
        "viewport w=1440 h=900\n\
         region id=save role=button label=\"Save\" x=1200 y=780 w=100 h=36 actions=click sources=dom\n\
         region id=accept role=button label=\"Accept all\" x=1200 y=40 w=120 h=36 actions=click sources=dom\n",
    )
    .unwrap();
    let covered = ultra_instinct_core::parse_fixture(
        "viewport w=1440 h=900\n\
         region id=save role=button label=\"Save\" x=1200 y=780 w=100 h=36 actions=click sources=dom flags=occluded\n\
         region id=accept role=button label=\"Accept all\" x=1200 y=40 w=120 h=36 actions=click sources=dom\n",
    )
    .unwrap();
    let seen = WorldSnapshot::of(&clear, None);
    let request = GuardRequest::click(button("Accept all")).seen_world(seen);
    assert_eq!(
        escalated(&guard(&covered, &request).unwrap()),
        GuardReason::WorldChanged
    );
}

#[test]
fn world_changed_when_a_clickable_region_appears() {
    let before = ultra_instinct_core::parse_fixture(
        "viewport w=800 h=600\n\
         region id=a role=button label=\"Go\" x=40 y=40 w=80 h=30 actions=click sources=dom\n",
    )
    .unwrap();
    let after = ultra_instinct_core::parse_fixture(
        "viewport w=800 h=600\n\
         region id=a role=button label=\"Go\" x=40 y=40 w=80 h=30 actions=click sources=dom\n\
         region id=b role=button label=\"Extra\" x=140 y=40 w=80 h=30 actions=click sources=dom\n",
    )
    .unwrap();
    let request = GuardRequest::click(button("Go")).seen_world(WorldSnapshot::of(&before, None));
    assert_eq!(
        escalated(&guard(&after, &request).unwrap()),
        GuardReason::WorldChanged
    );
}

#[test]
fn stacking_shaped_occlusion_refuses_under_a_higher_z_overlay() {
    let m = ultra_instinct_core::parse_fixture(
        "viewport w=1440 h=900\n\
         region id=save role=button label=\"Save\" x=1200 y=780 w=100 h=36 actions=click sources=dom flags=occluded\n\
         region id=toast role=button label=\"Dismiss\" x=1180 y=760 w=160 h=80 actions=click sources=dom\n",
    )
    .unwrap();
    assert!(FrontLayer::of(&m).is_empty());
    let req = GuardRequest::click(LocateQuery::new().text("Save").unwrap().role(Role::Button));
    assert_eq!(refused(&guard(&m, &req).unwrap()), GuardReason::Occluded);
    let toast = GuardRequest::click(
        LocateQuery::new()
            .text("Dismiss")
            .unwrap()
            .role(Role::Button),
    );
    assert!(matches!(
        guard(&m, &toast).unwrap(),
        GuardDecision::Allow { .. }
    ));
}

#[test]
fn hit_test_shaped_occlusion_refuses_even_without_a_dialog() {
    // Same shape observe would produce after DOM.getNodeForLocation says a
    // non-dialog overlay owns the center: flags=occluded, no Role::Dialog.
    let m = ultra_instinct_core::parse_fixture(
        "viewport w=1440 h=900\n\
         region id=save role=button label=\"Save\" x=1200 y=780 w=100 h=36 actions=click sources=dom,accessibility flags=occluded\n\
         region id=accept role=button label=\"Accept all\" x=1200 y=40 w=120 h=36 actions=click sources=dom,accessibility\n",
    )
    .unwrap();
    assert!(FrontLayer::of(&m).is_empty());
    let req = GuardRequest::click(LocateQuery::new().text("Save").unwrap().role(Role::Button));
    match guard(&m, &req).unwrap() {
        GuardDecision::Refuse {
            reason: GuardReason::Occluded,
            ..
        } => {}
        other => panic!("expected refuse occluded, got {other:?}"),
    }
    let accept = GuardRequest::click(
        LocateQuery::new()
            .text("Accept all")
            .unwrap()
            .role(Role::Button),
    );
    assert!(matches!(
        guard(&m, &accept).unwrap(),
        GuardDecision::Allow { .. }
    ));
}
