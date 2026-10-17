//! Issue and revalidate [`ActionTicket`]s.
//!
//! Hard-gate direction (not fully landed): treat occluded / front-layer /
//! disabled / hidden / wrong ancestry / stale ticket as *impossible* before
//! ranking, keep blocked candidates as evidence, and rank only the viable set.
//! Until that split lands, [`crate::guard`] still mixes some safety into
//! ranking (including `buried_better_label`); the ticket is still the lease
//! the executor must consume.

use std::sync::atomic::{AtomicU64, Ordering};

use aui_core::{Action, InteractionManifold, InteractionRegion, RegionId};
use aui_protocol::{ActionTicket, GuardCandidate, TicketInvalid};

use crate::world::WorldSnapshot;

static NEXT_TICKET_ID: AtomicU64 = AtomicU64::new(1);

/// Stable hash of the world variables compared across observations.
pub fn world_fingerprint(world: &WorldSnapshot) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    mix(
        &mut hash,
        world
            .focused()
            .map(|id| id.as_str().as_bytes())
            .unwrap_or(b"-"),
    );
    for entry in world.front_layer().entries() {
        mix(&mut hash, entry.id.as_str().as_bytes());
        mix(&mut hash, &[u8::from(entry.modal)]);
    }
    mix(&mut hash, b"|clickable|");
    for id in world.clickable() {
        mix(&mut hash, id.as_str().as_bytes());
    }
    mix(&mut hash, b"|occluded|");
    for id in world.occluded() {
        mix(&mut hash, id.as_str().as_bytes());
    }
    hash
}

fn mix(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(0x100000001b3);
}

/// Build a ticket for an Allow decision.
pub fn issue_ticket(
    snapshot_id: u64,
    action: Action,
    target: &GuardCandidate,
    region: &InteractionRegion,
    world: &WorldSnapshot,
) -> ActionTicket {
    ActionTicket {
        ticket_id: NEXT_TICKET_ID.fetch_add(1, Ordering::Relaxed),
        snapshot_id,
        action,
        target_id: target.id.clone(),
        target_role: target.role,
        target_label: target.label.clone(),
        target_fingerprint: region.fingerprint().bits(),
        world_fingerprint: world_fingerprint(world),
        dest: None,
        cell: None,
    }
}

/// Revalidate a ticket against the world observed **now**.
///
/// Call this at the executor boundary immediately before clicking. On `Ok`
/// the host may press **exactly** `ticket.target_id` for `ticket.action`.
pub fn revalidate(
    ticket: &ActionTicket,
    manifold: &InteractionManifold,
    focused: Option<RegionId>,
) -> Result<(), TicketInvalid> {
    // Same target-scoped envelope the hard gate used when issuing the ticket.
    let now = WorldSnapshot::of_target(manifold, focused, &ticket.target_id);
    if world_fingerprint(&now) != ticket.world_fingerprint {
        return Err(TicketInvalid::WorldChanged);
    }
    let Some(region) = manifold.get(&ticket.target_id) else {
        return Err(TicketInvalid::TargetGone);
    };
    if region.role() != ticket.target_role
        || region.label() != ticket.target_label
        || region.fingerprint().bits() != ticket.target_fingerprint
    {
        return Err(TicketInvalid::TargetChanged);
    }
    if let Some(dest) = &ticket.dest {
        let now = WorldSnapshot::of_target(manifold, None, &dest.id);
        if world_fingerprint(&now) != dest.world_fingerprint {
            return Err(TicketInvalid::WorldChanged);
        }
        let Some(region) = manifold.get(&dest.id) else {
            return Err(TicketInvalid::TargetGone);
        };
        if region.role() != dest.role
            || region.label() != dest.label
            || region.fingerprint().bits() != dest.fingerprint
        {
            return Err(TicketInvalid::TargetChanged);
        }
    }
    Ok(())
}

/// Host boundary: revalidate, then invoke `press` for the exact ticket target.
///
/// **Not one-shot**: nothing stops the same ticket from being consumed twice.
/// Use [`consume_ticket_once`] (host harnesses) or
/// `aui_agent::execute_ticketed` (owned loop) instead.
#[deprecated(
    since = "0.1.0",
    note = "not one-shot; use consume_ticket_once or aui_agent::execute_ticketed"
)]
pub fn consume_ticket<E>(
    ticket: &ActionTicket,
    manifold: &InteractionManifold,
    focused: Option<RegionId>,
    mut press: impl FnMut(&RegionId, Action) -> Result<(), E>,
) -> Result<(), ConsumeError<E>> {
    revalidate(ticket, manifold, focused).map_err(ConsumeError::Invalid)?;
    press(&ticket.target_id, ticket.action).map_err(ConsumeError::Press)
}

/// Failure consuming a ticket at the executor boundary.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConsumeError<E> {
    Invalid(TicketInvalid),
    Press(E),
}

impl<E: std::fmt::Display> std::fmt::Display for ConsumeError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(err) => write!(f, "ticket invalid: {err}"),
            Self::Press(err) => write!(f, "press failed: {err}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ConsumeError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(err) => Some(err),
            Self::Press(err) => Some(err),
        }
    }
}

/// Tracks one-shot consumption of ticket ids for a single agent/executor session.
#[derive(Clone, Debug, Default)]
pub struct TicketLedger {
    consumed: std::collections::BTreeSet<u64>,
}

impl TicketLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_consumed(&self, ticket_id: u64) -> bool {
        self.consumed.contains(&ticket_id)
    }

    pub fn mark_consumed(&mut self, ticket_id: u64) -> Result<(), TicketInvalid> {
        if !self.consumed.insert(ticket_id) {
            return Err(TicketInvalid::TicketConsumed);
        }
        Ok(())
    }
}

/// Host boundary, one-shot. Same order as `aui_agent::execute_ticketed`
/// (ADR 0003 §1, ADR 0005):
///
/// 1. refuse a ticket the ledger already consumed (`ticket-consumed`);
/// 2. [`revalidate`] against `manifold` (the host's observation **now**);
///    a stale ticket is **not** consumed, so the host re-observes and gets a
///    new ticket from a new decision;
/// 3. mark the ticket consumed **before** `press`;
/// 4. invoke `press` for the exact `ticket.target_id` / `ticket.action`.
///
/// Because consumption precedes input, a press that fails or partially reaches
/// the page can never be replayed with the same lease: the second call returns
/// `ticket-consumed` without calling `press`. Retrying means a new decision and
/// a new ticket.
pub fn consume_ticket_once<E>(
    ledger: &mut TicketLedger,
    ticket: &ActionTicket,
    manifold: &InteractionManifold,
    focused: Option<RegionId>,
    mut press: impl FnMut(&RegionId, Action) -> Result<(), E>,
) -> Result<(), ConsumeError<E>> {
    if ledger.is_consumed(ticket.ticket_id) {
        return Err(ConsumeError::Invalid(TicketInvalid::TicketConsumed));
    }
    revalidate(ticket, manifold, focused).map_err(ConsumeError::Invalid)?;
    ledger
        .mark_consumed(ticket.ticket_id)
        .map_err(ConsumeError::Invalid)?;
    press(&ticket.target_id, ticket.action).map_err(ConsumeError::Press)
}

#[cfg(test)]
mod tests {
    use aui_core::{
        Action, InteractionManifold, InteractionRegion, LocateQuery, Rect, RegionFlags, RegionId,
        RegionParts, Role, SourceMask, UnitInterval,
    };

    use crate::world::WorldSnapshot;
    use aui_protocol::GuardReason;

    use super::*;
    use crate::{guard, GuardDecision, GuardRequest};

    fn button(id: &str, label: &str, y: f64) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.to_owned(),
            rect: Rect::try_new(10.0, y, 80.0, 24.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::none(),
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
    fn allow_issues_a_ticket_that_revalidates_on_same_world() {
        let m = manifold(vec![button("ok", "Sign in", 100.0)]);
        let decision = guard(
            &m,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()).snapshot_id(7),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, target, .. } = decision else {
            panic!("expected allow, got {decision:?}");
        };
        assert_eq!(ticket.snapshot_id, 7);
        assert_eq!(ticket.target_id.as_str(), target.id.as_str());
        assert_eq!(ticket.action, Action::Click);
        revalidate(&ticket, &m, None).unwrap();
    }

    #[test]
    fn revalidate_fails_when_a_nearby_clickable_appears() {
        let before = manifold(vec![button("ok", "Sign in", 100.0)]);
        let decision = guard(
            &before,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, .. } = decision else {
            panic!("expected allow");
        };
        // y=200 is within the 160px neighborhood radius of y=100 (centers ~112 vs ~212).
        let after = manifold(vec![
            button("ok", "Sign in", 100.0),
            button("extra", "Notify", 200.0),
        ]);
        assert_eq!(
            revalidate(&ticket, &after, None),
            Err(TicketInvalid::WorldChanged)
        );
    }

    #[test]
    fn revalidate_ignores_unrelated_far_banner() {
        let before = manifold(vec![button("ok", "Sign in", 100.0)]);
        let decision = guard(
            &before,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, .. } = decision else {
            panic!("expected allow");
        };
        // Far below the target: outside neighborhood radius.
        let after = manifold(vec![
            button("ok", "Sign in", 100.0),
            button("cookie", "Accept", 500.0),
        ]);
        revalidate(&ticket, &after, None).unwrap();
    }

    #[test]
    #[allow(deprecated)]
    fn consume_ticket_presses_only_after_revalidate() {
        let m = manifold(vec![button("ok", "Sign in", 100.0)]);
        let decision = guard(
            &m,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, .. } = decision else {
            panic!("expected allow");
        };
        let mut pressed = None;
        consume_ticket(&ticket, &m, None, |id, action| {
            pressed = Some((id.clone(), action));
            Ok::<(), GuardReason>(())
        })
        .unwrap();
        assert_eq!(
            pressed,
            Some((RegionId::try_new("ok").unwrap(), Action::Click))
        );
    }

    #[test]
    fn consume_ticket_once_refuses_second_press() {
        let m = manifold(vec![button("ok", "Sign in", 100.0)]);
        let decision = guard(
            &m,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, .. } = decision else {
            panic!("expected allow");
        };
        let mut ledger = TicketLedger::new();
        let mut presses = 0u32;
        consume_ticket_once(&mut ledger, &ticket, &m, None, |_id, _action| {
            presses += 1;
            Ok::<(), GuardReason>(())
        })
        .unwrap();
        assert_eq!(presses, 1);
        let err = consume_ticket_once(&mut ledger, &ticket, &m, None, |_id, _action| {
            presses += 1;
            Ok::<(), GuardReason>(())
        })
        .unwrap_err();
        match err {
            ConsumeError::Invalid(TicketInvalid::TicketConsumed) => {}
            other => panic!("expected TicketConsumed, got {other}"),
        }
        assert_eq!(presses, 1);
    }

    fn allowed_sign_in(m: &InteractionManifold) -> ActionTicket {
        let decision = guard(
            m,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, .. } = decision else {
            panic!("expected allow, got {decision:?}");
        };
        ticket
    }

    /// R1 regression: a press that fails after the lease was taken must not be
    /// retryable with the same ticket (consume happens before press).
    #[test]
    fn consume_ticket_once_marks_consumed_before_press_so_failed_press_cannot_retry() {
        let m = manifold(vec![button("ok", "Sign in", 100.0)]);
        let ticket = allowed_sign_in(&m);
        let mut ledger = TicketLedger::new();
        let mut presses = 0u32;
        let mut retried_press = false;
        let err = consume_ticket_once(&mut ledger, &ticket, &m, None, |_id, _action| {
            presses += 1;
            Err::<(), GuardReason>(GuardReason::Ambiguous)
        })
        .unwrap_err();
        assert!(
            matches!(err, ConsumeError::Press(GuardReason::Ambiguous)),
            "{err}"
        );
        assert_eq!(presses, 1);
        assert!(ledger.is_consumed(ticket.ticket_id));
        // Retry with the same lease: refused, press never called again.
        let err = consume_ticket_once(&mut ledger, &ticket, &m, None, |_id, _action| {
            presses += 1;
            retried_press = true;
            Ok::<(), GuardReason>(())
        })
        .unwrap_err();
        assert!(
            matches!(err, ConsumeError::Invalid(TicketInvalid::TicketConsumed)),
            "{err}"
        );
        assert_eq!(presses, 1);
        assert!(!retried_press);
    }

    /// A stale ticket is refused without consuming it and without pressing.
    #[test]
    fn consume_ticket_once_stale_does_not_consume_or_press() {
        let before = manifold(vec![button("ok", "Sign in", 100.0)]);
        let ticket = allowed_sign_in(&before);
        let after = manifold(vec![button("ok", "Sign out", 100.0)]);
        let mut ledger = TicketLedger::new();
        let mut presses = 0u32;
        let err = consume_ticket_once(&mut ledger, &ticket, &after, None, |_id, _action| {
            presses += 1;
            Ok::<(), GuardReason>(())
        })
        .unwrap_err();
        assert!(
            matches!(err, ConsumeError::Invalid(TicketInvalid::TargetChanged)),
            "{err}"
        );
        assert_eq!(presses, 0);
        assert!(!ledger.is_consumed(ticket.ticket_id));
    }

    /// Each bound target attribute is checked on its own: a ticket whose role,
    /// label, or region fingerprint alone disagrees with the live region is
    /// `target-changed` (kills `||` → `&&` in `revalidate`).
    #[test]
    fn revalidate_checks_role_label_and_fingerprint_independently() {
        let m = manifold(vec![button("ok", "Sign in", 100.0)]);
        let ticket = allowed_sign_in(&m);
        revalidate(&ticket, &m, None).unwrap();

        let mut role = ticket.clone();
        role.target_role = Role::Link;
        assert_eq!(
            revalidate(&role, &m, None),
            Err(TicketInvalid::TargetChanged)
        );

        let mut label = ticket.clone();
        label.target_label = "Sign out".into();
        assert_eq!(
            revalidate(&label, &m, None),
            Err(TicketInvalid::TargetChanged)
        );

        let mut fingerprint = ticket.clone();
        fingerprint.target_fingerprint ^= 1;
        assert_eq!(
            revalidate(&fingerprint, &m, None),
            Err(TicketInvalid::TargetChanged)
        );
    }

    /// The world fingerprint separates many distinct worlds (focus id and
    /// clickable sets). Pins the FNV mix and its segment separator.
    #[test]
    fn world_fingerprint_has_no_collisions_across_distinct_worlds() {
        let mut seen = std::collections::BTreeMap::new();
        for n in 1..=12usize {
            let regions: Vec<_> = (0..n)
                .map(|i| button(&format!("b{i}"), &format!("B{i}"), 10.0 + 40.0 * i as f64))
                .collect();
            let m = manifold(regions);
            for f in 0..=n {
                let focused = (f < n).then(|| RegionId::try_new(format!("b{f}")).unwrap());
                let world = WorldSnapshot::of(&m, focused);
                let key = format!("n={n} f={f}");
                if let Some(prev) = seen.insert(world_fingerprint(&world), key.clone()) {
                    panic!("fingerprint collision: {prev} vs {key}");
                }
            }
        }
        // Segment boundaries matter: focus `ab` + clickable `c` is not focus
        // `a` + clickable `bc`.
        let ab = manifold(vec![button("ab", "X", 10.0), button("c", "Y", 300.0)]);
        let a = manifold(vec![button("a", "X", 10.0), button("bc", "Y", 300.0)]);
        assert_ne!(
            world_fingerprint(&WorldSnapshot::of(
                &ab,
                Some(RegionId::try_new("ab").unwrap())
            )),
            world_fingerprint(&WorldSnapshot::of(
                &a,
                Some(RegionId::try_new("a").unwrap())
            )),
        );
    }

    #[test]
    fn consume_error_display_and_source_are_exact() {
        use std::error::Error as _;
        // `E = TicketInvalid` only because it implements `Error`; any host
        // error type works the same.
        let invalid: ConsumeError<TicketInvalid> =
            ConsumeError::Invalid(TicketInvalid::WorldChanged);
        assert_eq!(invalid.to_string(), "ticket invalid: world-changed");
        assert_eq!(invalid.source().unwrap().to_string(), "world-changed");
        let press: ConsumeError<TicketInvalid> = ConsumeError::Press(TicketInvalid::TargetGone);
        assert_eq!(press.to_string(), "press failed: target-gone");
        assert_eq!(press.source().unwrap().to_string(), "target-gone");
    }

    #[test]
    fn revalidate_fails_when_target_label_changes() {
        let before = manifold(vec![button("ok", "Sign in", 100.0)]);
        let decision = guard(
            &before,
            &GuardRequest::click(LocateQuery::new().text("Sign in").unwrap()),
        )
        .unwrap();
        let GuardDecision::Allow { ticket, .. } = decision else {
            panic!("expected allow");
        };
        let after = manifold(vec![button("ok", "Sign out", 100.0)]);
        assert_eq!(
            revalidate(&ticket, &after, None),
            Err(TicketInvalid::TargetChanged)
        );
    }
}
