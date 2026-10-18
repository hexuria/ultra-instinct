//! Hard browser-integrity gate for a target a policy **already chose**.
//!
//! This is the agent path's guard. It does not rank and has no confidence
//! threshold: the policy (Instinct) decided *which* finite choice wins; the gate
//! answers only "may this exact region receive this exact action right now?"
//! Every check is a hard invalidity, never a score penalty:
//!
//! | check | reason |
//! |---|---|
//! | region not in the current observation | `missing-target` |
//! | region does not claim the action | `unsupported-action` |
//! | disabled | `disabled` |
//! | readonly (TYPE / SELECT) | `readonly` |
//! | hidden / zero area | `hidden` |
//! | hit-test / stacking occluded | `occluded` |
//! | behind an open dialog | `front-layer` |
//! | outside the viewport | `offscreen` |
//!
//! Readonly is observed ([`aui_core::RegionFlags::readonly`]) and refused
//! here for TYPE / SELECT before any CDP input. The browser input function
//! remains a second fail-closed line for attributes observe missed.
//!
//! On success the gate issues an [`ActionTicket`] bound to the action, the
//! target fingerprint, and the world fingerprint of this observation. The
//! executor must revalidate and consume it once ([`crate::revalidate`],
//! [`crate::TicketLedger`]).
//!
//! The ranked [`crate::guard`] entry point remains for the MCP / CLI preflight
//! surface, where a host proposes a label rather than a finite choice.

use aui_core::{Action, InteractionManifold, InteractionRegion, RegionId};
use aui_protocol::{ActionTicket, GuardCandidate, GuardReason, TicketDest};

use crate::ticket::issue_ticket;
use crate::world::{blocker, WorldSnapshot};

/// Gate `target` for `action` on `manifold` and issue a one-shot ticket.
pub fn gate(
    manifold: &InteractionManifold,
    target: &RegionId,
    action: Action,
    focused: Option<RegionId>,
    snapshot_id: u64,
) -> Result<ActionTicket, GuardReason> {
    let region = manifold.get(target).ok_or(GuardReason::MissingTarget)?;
    check(manifold, region, action)?;
    let candidate = GuardCandidate {
        id: region.id().clone(),
        role: region.role(),
        label: region.label().to_owned(),
        confidence: 1.0,
    };
    let world = WorldSnapshot::of_target(manifold, focused, target);
    Ok(issue_ticket(
        snapshot_id,
        action,
        &candidate,
        region,
        &world,
    ))
}

/// Gate a DRAG from `source` to `dest` and issue one ticket bound to both.
///
/// The source must claim Drag and pass every [`check`]; the destination must
/// be another region and pass [`check_drop_target`]. Revalidation re-checks
/// both neighborhoods, so a change at either end makes the ticket stale.
pub fn gate_drag(
    manifold: &InteractionManifold,
    source: &RegionId,
    dest: &RegionId,
    focused: Option<RegionId>,
    snapshot_id: u64,
) -> Result<ActionTicket, GuardReason> {
    if source == dest {
        return Err(GuardReason::UnsupportedAction);
    }
    let mut ticket = gate(manifold, source, Action::Drag, focused, snapshot_id)?;
    let region = manifold.get(dest).ok_or(GuardReason::MissingTarget)?;
    check_drop_target(manifold, region)?;
    // Focus is global and already bound by the source's world; the
    // destination binds its own neighborhood.
    let world = WorldSnapshot::of_target(manifold, None, dest);
    ticket.dest = Some(Box::new(TicketDest {
        id: region.id().clone(),
        role: region.role(),
        label: region.label().to_owned(),
        fingerprint: region.fingerprint().bits(),
        world_fingerprint: crate::ticket::world_fingerprint(&world),
        cell: None,
    }));
    Ok(ticket)
}

/// Gate a DRAG from `source` to grid cell `cell` of the surface `surface`
/// (moving a shape to a point of its drawing): [`gate_drag`]'s checks, plus
/// the surface must claim Point or be the source's parent (the drawing it
/// sits in), and the cell must lie on its grid. The ticket binds the cell;
/// the executor releases exactly there.
pub fn gate_drag_to_cell(
    manifold: &InteractionManifold,
    source: &RegionId,
    surface: &RegionId,
    cell: [u16; 2],
    focused: Option<RegionId>,
    snapshot_id: u64,
) -> Result<ActionTicket, GuardReason> {
    let region = manifold.get(surface).ok_or(GuardReason::MissingTarget)?;
    let drawn_on = manifold
        .get(source)
        .is_some_and(|src| src.parent() == Some(surface));
    if !(region.actions().contains(&Action::Point) || drawn_on)
        || cell[0] >= aui_core::SURFACE_GRID
        || cell[1] >= aui_core::SURFACE_GRID
    {
        return Err(GuardReason::UnsupportedAction);
    }
    let mut ticket = gate_drag(manifold, source, surface, focused, snapshot_id)?;
    if let Some(dest) = ticket.dest.as_mut() {
        dest.cell = Some(cell);
    }
    Ok(ticket)
}

/// Gate a click on one grid cell of a drawing surface. The surface must
/// claim Point and pass every [`check`] for Click; the ticket binds the cell.
pub fn gate_point(
    manifold: &InteractionManifold,
    surface: &RegionId,
    cell: [u16; 2],
    focused: Option<RegionId>,
    snapshot_id: u64,
) -> Result<ActionTicket, GuardReason> {
    let region = manifold.get(surface).ok_or(GuardReason::MissingTarget)?;
    if !region.actions().contains(&Action::Point)
        || cell[0] >= aui_core::SURFACE_GRID
        || cell[1] >= aui_core::SURFACE_GRID
    {
        return Err(GuardReason::UnsupportedAction);
    }
    let mut ticket = gate(manifold, surface, Action::Click, focused, snapshot_id)?;
    ticket.cell = Some(cell);
    Ok(ticket)
}

/// Gate one key press on `target`. The region must claim Key and pass every
/// [`check`]; `key` must be one of [`aui_core::KEY_NAMES`]. The ticket binds
/// the key: the executor presses exactly that key, nothing else.
pub fn gate_key(
    manifold: &InteractionManifold,
    target: &RegionId,
    key: &str,
    focused: Option<RegionId>,
    snapshot_id: u64,
) -> Result<ActionTicket, GuardReason> {
    if !aui_core::KEY_NAMES.contains(&key) {
        return Err(GuardReason::UnsupportedAction);
    }
    let mut ticket = gate(manifold, target, Action::Key, focused, snapshot_id)?;
    ticket.key = Some(key.to_owned());
    Ok(ticket)
}

/// Hard checks for a release point: it needs no claim of its own, but it
/// must be visible, uncovered, in front, and on screen.
pub fn check_drop_target(
    manifold: &InteractionManifold,
    region: &InteractionRegion,
) -> Result<(), GuardReason> {
    let flags = region.flags();
    if flags.hidden() || region.rect().is_zero_area() {
        return Err(GuardReason::Hidden);
    }
    if flags.occluded() {
        return Err(GuardReason::Occluded);
    }
    if blocker(manifold, region).is_some() {
        return Err(GuardReason::FrontLayer);
    }
    if flags.offscreen() {
        return Err(GuardReason::Offscreen);
    }
    Ok(())
}

/// The hard checks alone (no ticket). Exposed for tests and evidence.
pub fn check(
    manifold: &InteractionManifold,
    region: &InteractionRegion,
    action: Action,
) -> Result<(), GuardReason> {
    if !supports(region, action) {
        return Err(GuardReason::UnsupportedAction);
    }
    let flags = region.flags();
    if flags.disabled() {
        return Err(GuardReason::Disabled);
    }
    // Readonly is only a hard refuse for value-changing inputs. Click/focus
    // on a readonly field remains allowed (people can still select text).
    if flags.readonly() && matches!(action, Action::Type | Action::Select) {
        return Err(GuardReason::Readonly);
    }
    if flags.hidden() || region.rect().is_zero_area() {
        return Err(GuardReason::Hidden);
    }
    if flags.occluded() {
        return Err(GuardReason::Occluded);
    }
    if blocker(manifold, region).is_some() {
        return Err(GuardReason::FrontLayer);
    }
    if flags.offscreen() {
        return Err(GuardReason::Offscreen);
    }
    Ok(())
}

fn supports(region: &InteractionRegion, action: Action) -> bool {
    let claims = region.actions();
    match action {
        Action::Click => claims.contains(&Action::Click) || claims.contains(&Action::Toggle),
        other => claims.contains(&other),
    }
}

#[cfg(test)]
mod tests {
    use aui_core::parse_fixture;

    use super::*;
    use crate::{revalidate, TicketLedger};

    fn id(raw: &str) -> RegionId {
        RegionId::try_new(raw).unwrap()
    }

    const PAGE: &str = r#"
        viewport w=800 h=600
        region id=go role=button label="Go" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=off role=button label="Off" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility flags=disabled
        region id=cov role=button label="Covered" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility flags=occluded
        region id=gone role=button label="Gone" x=10 y=90 w=80 h=24 actions=click sources=dom,accessibility flags=hidden
        region id=far role=button label="Far" x=10 y=900 w=80 h=24 actions=click sources=dom,accessibility flags=offscreen
        region id=name role=text_field label="Name" x=10 y=130 w=200 h=24 actions=click,type sources=dom,accessibility
        region id=ro role=text_field label="Locked" x=10 y=170 w=200 h=24 actions=click,type,select sources=dom,accessibility flags=readonly
    "#;

    #[test]
    fn viable_click_and_type_issue_bound_tickets() {
        let m = parse_fixture(PAGE).unwrap();
        let t = gate(&m, &id("go"), Action::Click, None, 7).unwrap();
        assert_eq!(t.action, Action::Click);
        assert_eq!(t.target_id, id("go"));
        assert_eq!(t.snapshot_id, 7);
        revalidate(&t, &m, None).unwrap();
        let t = gate(&m, &id("name"), Action::Type, None, 7).unwrap();
        assert_eq!(t.action, Action::Type);
    }

    #[test]
    fn hard_invalid_targets_never_get_tickets() {
        let m = parse_fixture(PAGE).unwrap();
        assert_eq!(
            gate(&m, &id("off"), Action::Click, None, 0),
            Err(GuardReason::Disabled)
        );
        assert_eq!(
            gate(&m, &id("cov"), Action::Click, None, 0),
            Err(GuardReason::Occluded)
        );
        assert_eq!(
            gate(&m, &id("gone"), Action::Click, None, 0),
            Err(GuardReason::Hidden)
        );
        assert_eq!(
            gate(&m, &id("far"), Action::Click, None, 0),
            Err(GuardReason::Offscreen)
        );
        assert_eq!(
            gate(&m, &id("nope"), Action::Click, None, 0),
            Err(GuardReason::MissingTarget)
        );
        assert_eq!(
            gate(&m, &id("go"), Action::Type, None, 0),
            Err(GuardReason::UnsupportedAction)
        );
        assert_eq!(
            gate(&m, &id("name"), Action::Select, None, 0),
            Err(GuardReason::UnsupportedAction)
        );
    }

    #[test]
    fn background_control_behind_modal_is_front_layer() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=bg role=button label="Delete project" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
            region id=dlg role=dialog label="Confirm" x=200 y=100 w=300 h=200 actions=focus sources=dom,accessibility flags=modal
            region id=ok role=button label="Delete" x=220 y=250 w=80 h=24 actions=click parent=dlg sources=dom,accessibility
            "#,
        )
        .unwrap();
        assert_eq!(
            gate(&m, &id("bg"), Action::Click, None, 0),
            Err(GuardReason::FrontLayer)
        );
        assert!(gate(&m, &id("ok"), Action::Click, None, 0).is_ok());
    }

    #[test]
    fn gate_ticket_is_one_shot_in_ledger() {
        let m = parse_fixture(PAGE).unwrap();
        let t = gate(&m, &id("go"), Action::Click, None, 0).unwrap();
        let mut ledger = TicketLedger::new();
        ledger.mark_consumed(t.ticket_id).unwrap();
        assert!(ledger.mark_consumed(t.ticket_id).is_err());
    }

    #[test]
    fn readonly_field_refuses_type_and_select_before_input() {
        let m = parse_fixture(PAGE).unwrap();
        assert_eq!(
            gate(&m, &id("ro"), Action::Type, None, 0),
            Err(GuardReason::Readonly)
        );
        assert_eq!(
            gate(&m, &id("ro"), Action::Select, None, 0),
            Err(GuardReason::Readonly)
        );
        // Click on a readonly field is still allowed (select / focus).
        assert!(gate(&m, &id("ro"), Action::Click, None, 0).is_ok());
        // Editable field still types.
        assert!(gate(&m, &id("name"), Action::Type, None, 0).is_ok());
    }
}
