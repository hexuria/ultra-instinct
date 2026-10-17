//! Property / adversarial tests for the agent ticket world (`of_target`).
//!
//! Owners (impeccable audit R5, baseline `87ffc2d`):
//!
//! - the neighborhood radius boundary (`NEIGHBOR_RADIUS_PX` = 160, inclusive);
//! - focus is global in the target-scoped world: a focus-only change always
//!   invalidates a ticket, wherever focus moves;
//! - one-shot ledger refusals (`ticket-consumed`) are distinct from staleness.

use aui_core::{parse_fixture, Action, InteractionManifold, RegionId};
use aui_guard::{
    consume_ticket_once, gate, neighborhood_of, revalidate, world_fingerprint, ConsumeError,
    TicketLedger, WorldSnapshot,
};
use aui_protocol::{GuardReason, TicketInvalid};
use proptest::prelude::*;

fn id(raw: &str) -> RegionId {
    RegionId::try_new(raw).unwrap()
}

/// Target `go` centered at (50, 22). Optional root-level peer whose center is
/// exactly `dy` px below (vertical) or `dx` px right (horizontal) of it.
fn page(peer: Option<(u32, bool)>) -> InteractionManifold {
    let mut src = String::from(
        "viewport w=1200 h=900\n\
         region id=go role=button label=\"Go\" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility\n\
         region id=far role=button label=\"Far\" x=1000 y=800 w=80 h=24 actions=click sources=dom,accessibility\n",
    );
    if let Some((d, horizontal)) = peer {
        let (x, y) = if horizontal {
            (10 + d, 10)
        } else {
            (10, 10 + d)
        };
        src.push_str(&format!(
            "region id=peer role=button label=\"Peer\" x={x} y={y} w=80 h=24 actions=click sources=dom,accessibility\n"
        ));
    }
    parse_fixture(&src).unwrap()
}

#[test]
fn neighborhood_radius_boundary_159_160_161() {
    for (d, inside) in [(159, true), (160, true), (161, false)] {
        for horizontal in [false, true] {
            let m = page(Some((d, horizontal)));
            let n = neighborhood_of(&m, &id("go"));
            assert_eq!(
                n.contains(&id("peer")),
                inside,
                "d={d} horizontal={horizontal}"
            );
            assert!(!n.contains(&id("far")));
        }
    }
}

#[test]
fn peer_appearing_at_159_invalidates_but_161_does_not() {
    let before = page(None);
    let ticket = gate(&before, &id("go"), Action::Click, None, 0).unwrap();
    assert_eq!(
        revalidate(&ticket, &page(Some((159, false))), None),
        Err(TicketInvalid::WorldChanged)
    );
    revalidate(&ticket, &page(Some((161, false))), None).unwrap();
}

/// Parented layout: two lists, a child under the target, a root peer nearby.
const NESTED: &str = "viewport w=1200 h=900
region id=listA role=generic label=\"Inbox\" x=0 y=0 w=500 h=800 actions=focus sources=dom
region id=row1 role=button label=\"Archive 1\" x=10 y=10 w=80 h=24 actions=click parent=listA sources=dom,accessibility
region id=row9 role=button label=\"Archive 9\" x=10 y=700 w=80 h=24 actions=click parent=listA sources=dom,accessibility
region id=star role=button label=\"Star\" x=20 y=12 w=10 h=10 actions=click parent=row1 sources=dom,accessibility
region id=listB role=generic label=\"Spam\" x=500 y=0 w=500 h=800 actions=focus sources=dom
region id=other role=button label=\"Other\" x=10 y=50 w=80 h=24 actions=click parent=listB sources=dom,accessibility
region id=rootnear role=button label=\"Root\" x=100 y=10 w=80 h=24 actions=click sources=dom,accessibility
";

#[test]
fn parented_neighborhood_is_ancestors_same_parent_siblings_and_children() {
    let m = parse_fixture(NESTED).unwrap();
    let n = neighborhood_of(&m, &id("row1"));
    let want: Vec<RegionId> = ["listA", "row1", "row9", "star"]
        .iter()
        .map(|s| id(s))
        .collect();
    assert_eq!(n.into_iter().collect::<Vec<_>>(), want);
    // A child sees its ancestors (and no unrelated nearby peers).
    let n = neighborhood_of(&m, &id("star"));
    assert!(n.contains(&id("row1")) && n.contains(&id("listA")));
    assert!(!n.contains(&id("other")) && !n.contains(&id("rootnear")));
}

#[test]
fn parented_ticket_stale_on_far_sibling_not_on_nearby_foreign_row() {
    let before = parse_fixture(NESTED).unwrap();
    let ticket = gate(&before, &id("row1"), Action::Click, None, 0).unwrap();
    // Same parent, 700px away: still the target's neighborhood.
    let sibling = parse_fixture(&format!(
        "{NESTED}region id=row10 role=button label=\"Archive 10\" x=10 y=760 w=80 h=24 actions=click parent=listA sources=dom,accessibility\n"
    ))
    .unwrap();
    assert_eq!(
        revalidate(&ticket, &sibling, None),
        Err(TicketInvalid::WorldChanged)
    );
    // Different parent, 40px away: not the target's neighborhood.
    let foreign = parse_fixture(&format!(
        "{NESTED}region id=other2 role=button label=\"Other 2\" x=10 y=90 w=80 h=24 actions=click parent=listB sources=dom,accessibility\n"
    ))
    .unwrap();
    revalidate(&ticket, &foreign, None).unwrap();
}

proptest! {
    /// Inclusion is exactly `distance <= 160`, on both axes.
    #[test]
    fn neighborhood_inclusion_is_distance_le_radius(d in 24u32..400, horizontal: bool) {
        let m = page(Some((d, horizontal)));
        prop_assert_eq!(neighborhood_of(&m, &id("go")).contains(&id("peer")), d <= 160);
    }

    /// A new clickable root peer invalidates the ticket iff it lands inside the
    /// radius. Outside it the target-scoped world is unchanged.
    #[test]
    fn new_peer_is_stale_iff_inside_radius(d in 24u32..400, horizontal: bool) {
        let before = page(None);
        let ticket = gate(&before, &id("go"), Action::Click, None, 0).unwrap();
        let res = revalidate(&ticket, &page(Some((d, horizontal))), None);
        if d <= 160 {
            prop_assert_eq!(res, Err(TicketInvalid::WorldChanged));
        } else {
            prop_assert_eq!(res, Ok(()));
        }
    }

    /// Focus is not neighborhood-scoped: a focus-only change invalidates the
    /// ticket even when focus moves to a region far outside the neighborhood,
    /// or appears / disappears. Same focus revalidates.
    #[test]
    fn focus_only_change_is_world_changed_under_of_target(
        issued in 0usize..4,
        now in 0usize..4,
    ) {
        let m = page(Some((40, false)));
        let focus = |k: usize| match k {
            0 => None,
            1 => Some(id("go")),
            2 => Some(id("peer")),
            _ => Some(id("far")),
        };
        let ticket = gate(&m, &id("go"), Action::Click, focus(issued), 0).unwrap();
        let res = revalidate(&ticket, &m, focus(now));
        if issued == now {
            prop_assert_eq!(res, Ok(()));
        } else {
            prop_assert_eq!(res, Err(TicketInvalid::WorldChanged));
        }
        // The fingerprint itself carries focus in both world flavors.
        let a = WorldSnapshot::of_target(&m, focus(issued), &id("go"));
        let b = WorldSnapshot::of_target(&m, focus(now), &id("go"));
        prop_assert_eq!(world_fingerprint(&a) == world_fingerprint(&b), issued == now);
    }

    /// After a successful consume, every later attempt is `ticket-consumed`,
    /// whatever the world did meanwhile (stale or not) — the ledger is checked
    /// first, so a spent lease is never reported as stale.
    #[test]
    fn consumed_ticket_is_never_classified_stale(mutation in 0u8..4) {
        let before = page(None);
        let ticket = gate(&before, &id("go"), Action::Click, None, 0).unwrap();
        let mut ledger = TicketLedger::new();
        let mut presses = 0u32;
        consume_ticket_once(&mut ledger, &ticket, &before, None, |_, _| {
            presses += 1;
            Ok::<(), GuardReason>(())
        })
        .unwrap();
        let after = match mutation {
            0 => before.clone(),
            1 => page(Some((40, false))), // world changed
            2 => parse_fixture(
                "viewport w=1200 h=900\n\
                 region id=far role=button label=\"Far\" x=1000 y=800 w=80 h=24 actions=click sources=dom,accessibility\n",
            )
            .unwrap(), // target gone
            _ => page(Some((300, false))), // unrelated
        };
        let focused = if mutation == 3 { Some(id("far")) } else { None };
        let err = consume_ticket_once(&mut ledger, &ticket, &after, focused, |_, _| {
            presses += 1;
            Ok::<(), GuardReason>(())
        })
        .unwrap_err();
        prop_assert!(
            matches!(err, ConsumeError::Invalid(TicketInvalid::TicketConsumed)),
            "{}",
            err
        );
        prop_assert_eq!(presses, 1);
    }
}

// ---- Two-region (DRAG) tickets -------------------------------------------

fn board(dest_label: &str, dest_x: u32) -> InteractionManifold {
    parse_fixture(&format!(
        "viewport w=1200 h=900\n\
         region id=card role=generic label=\"Card\" x=10 y=10 w=60 h=40 actions=drag sources=dom\n\
         region id=bin role=generic label=\"{dest_label}\" x={dest_x} y=600 w=120 h=80 sources=dom\n\
         region id=other role=generic label=\"Other\" x=900 y=600 w=120 h=80 sources=dom\n"
    ))
    .unwrap()
}

#[test]
fn drag_ticket_binds_both_ends() {
    let m = board("Bin", 500);
    let ticket = aui_guard::gate_drag(&m, &id("card"), &id("bin"), None, 0).expect("allowed");
    assert_eq!(ticket.action, Action::Drag);
    assert_eq!(ticket.dest.as_ref().map(|d| d.id.as_str()), Some("bin"));
    assert_eq!(revalidate(&ticket, &m, None), Ok(()));
    // The destination changed (relabelled): stale, like a source change.
    assert_eq!(
        revalidate(&ticket, &board("Trash", 500), None),
        Err(TicketInvalid::TargetChanged)
    );
    // The destination moved far away: its world changed.
    assert!(matches!(
        revalidate(&ticket, &board("Bin", 20), None),
        Err(TicketInvalid::WorldChanged | TicketInvalid::TargetChanged)
    ));
}

#[test]
fn drag_needs_a_drag_claim_and_a_visible_destination() {
    let m = board("Bin", 500);
    assert_eq!(
        aui_guard::gate_drag(&m, &id("bin"), &id("card"), None, 0).unwrap_err(),
        GuardReason::UnsupportedAction,
        "a region that does not claim drag is not a source"
    );
    assert_eq!(
        aui_guard::gate_drag(&m, &id("card"), &id("card"), None, 0).unwrap_err(),
        GuardReason::UnsupportedAction,
        "a region is never its own drop target"
    );
    let hidden = parse_fixture(
        "viewport w=1200 h=900\n\
         region id=card role=generic label=\"Card\" x=10 y=10 w=60 h=40 actions=drag sources=dom\n\
         region id=bin role=generic label=\"Bin\" x=500 y=600 w=120 h=80 sources=dom flags=hidden\n",
    )
    .unwrap();
    assert_eq!(
        aui_guard::gate_drag(&hidden, &id("card"), &id("bin"), None, 0).unwrap_err(),
        GuardReason::Hidden
    );
}
