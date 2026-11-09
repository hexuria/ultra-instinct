//! The executor boundary: execute exactly the ticketed action, or nothing.
//!
//! [`execute_ticketed`] is the only path from an [`ActionTicket`] to page
//! input in the agent loop. In order, immediately before input:
//!
//! 1. the ticket must not be consumed (one-shot ledger);
//! 2. the requested [`Input`] must be the ticket's action (no operation swap),
//!    a DRAG must release on the ticket's destination, and a surface-cell
//!    click must name the ticket's cell (neither rides any other input);
//! 3. the browser is observed **now** (after any text-resolution latency);
//! 4. the ticket is revalidated against that observation: same world
//!    fingerprint (focus, front layer, clickable set, occluded set), target
//!    still present with the same role / label / fingerprint;
//! 5. the hard gate re-runs on the fresh region (disabled, hidden, occluded,
//!    front-layer, offscreen, action claim), and on a DRAG's fresh drop
//!    target;
//! 6. the user's [`Policy`] checks the fresh page URL and the fresh region
//!    (allowed domains, confirm-before-press); an empty policy allows all;
//! 7. the ticket is marked consumed **before** dispatch, so a failed or
//!    partial dispatch can never be retried with the same lease;
//! 8. input goes to `ticket.target_id` — never a caller-supplied id.

use ultra_instinct_browser::PageState;
use ultra_instinct_core::{Action, InteractionManifold, RegionId};
use ultra_instinct_guard::{gate_check, revalidate, Policy, TicketLedger};
use ultra_instinct_protocol::{ActionTicket, GuardReason, TicketInvalid};

use crate::runtime::{BrowserRuntime, Input};

/// What actually ran, plus the fresh pre-input observation (verification's `before`).
#[derive(Clone, Debug)]
pub struct Executed {
    pub ticket_id: u64,
    pub target: RegionId,
    pub action: Action,
    pub before: InteractionManifold,
    pub before_page: Option<PageState>,
}

/// Why the executor refused (nothing was dispatched) or dispatch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecError {
    /// Lease invalid: consumed, mismatched, or stale against the fresh world.
    Ticket(TicketInvalid),
    /// Fresh hard gate failed on the ticket target.
    Gate(GuardReason),
    /// The user's policy refused the input on the fresh observation
    /// (`domain-not-allowed`, `needs-confirm`); `label` is what that look
    /// checked (`Policy::press_label`: the target, or the button its form
    /// submits with), the label a confirm must name.
    Policy { reason: GuardReason, label: String },
    /// Observation failed before input (nothing dispatched).
    Observe(String),
    /// The page refused the input (disabled / readonly / no unique option).
    /// The ticket is consumed; nothing changed.
    Rejected(String),
    /// Dispatch failed after it may have reached the page. Ticket consumed.
    Dispatch(String),
}

impl ExecError {
    /// Stale lease: the right response is observe → decide again, not failure.
    /// Only world / target drift is stale. A policy refusal
    /// ([`Self::Policy`]) is not: observing again gives the same answer.
    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            Self::Ticket(
                TicketInvalid::WorldChanged
                    | TicketInvalid::TargetChanged
                    | TicketInvalid::TargetGone
            )
        )
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ticket(t) => write!(f, "ticket {t}"),
            Self::Gate(r) | Self::Policy { reason: r, .. } => write!(f, "gate refused: {r}"),
            Self::Observe(m) => write!(f, "observe before input failed: {m}"),
            Self::Rejected(m) => write!(f, "page rejected input: {m}"),
            Self::Dispatch(m) => write!(f, "dispatch failed: {m}"),
        }
    }
}

impl std::error::Error for ExecError {}

/// Revalidate and consume `ticket`, then send `input` to its exact target.
/// No user policy: [`execute_ticketed_with`] and an empty [`Policy`].
pub fn execute_ticketed<B: BrowserRuntime + ?Sized>(
    browser: &mut B,
    ledger: &mut TicketLedger,
    ticket: &ActionTicket,
    input: &Input,
) -> Result<Executed, ExecError> {
    execute_ticketed_with(browser, ledger, ticket, input, &Policy::default())
}

/// [`execute_ticketed`] under the user's `policy`, checked on the fresh
/// observation after revalidation and the hard gate, before the ticket is
/// consumed. A refusal leaves the ticket unconsumed and dispatches nothing.
pub fn execute_ticketed_with<B: BrowserRuntime + ?Sized>(
    browser: &mut B,
    ledger: &mut TicketLedger,
    ticket: &ActionTicket,
    input: &Input,
    policy: &Policy,
) -> Result<Executed, ExecError> {
    if ledger.is_consumed(ticket.ticket_id) {
        return Err(ExecError::Ticket(TicketInvalid::TicketConsumed));
    }
    if input.action() != ticket.action {
        return Err(ExecError::Ticket(TicketInvalid::TicketMismatch));
    }
    // A drag releases exactly where its ticket says; a ticket without a
    // destination never drives a drag, and a destination never rides a
    // non-drag input.
    match (input, &ticket.dest) {
        (Input::Drag { to }, Some(dest)) if *to == dest.id && dest.cell.is_none() => {}
        (Input::DragToCell { to, cell }, Some(dest))
            if *to == dest.id && dest.cell == Some(*cell) => {}
        (Input::Drag { .. } | Input::DragToCell { .. }, _) | (_, Some(_)) => {
            return Err(ExecError::Ticket(TicketInvalid::TicketMismatch))
        }
        _ => {}
    }
    // A cell ticket clicks exactly its cell; a plain ticket never clicks a cell.
    let cell = match input {
        Input::ClickAt { cell } => Some(*cell),
        _ => None,
    };
    if cell != ticket.cell {
        return Err(ExecError::Ticket(TicketInvalid::TicketMismatch));
    }
    // A key ticket presses exactly its key; no other input carries a key.
    let key = match input {
        Input::Key(key) => Some(key.as_str()),
        _ => None,
    };
    if key != ticket.key.as_deref() {
        return Err(ExecError::Ticket(TicketInvalid::TicketMismatch));
    }
    let fresh = browser
        .observe()
        .map_err(|e| ExecError::Observe(e.to_string()))?
        .clone();
    let before_page = browser.page().cloned();
    let focused = browser.focused();
    revalidate(ticket, &fresh, focused).map_err(ExecError::Ticket)?;
    let region = fresh
        .get(&ticket.target_id)
        .ok_or(ExecError::Ticket(TicketInvalid::TargetGone))?;
    gate_check(&fresh, region, ticket.action).map_err(ExecError::Gate)?;
    if let Some(dest) = &ticket.dest {
        let drop = fresh
            .get(&dest.id)
            .ok_or(ExecError::Ticket(TicketInvalid::TargetGone))?;
        ultra_instinct_guard::check_drop_target(&fresh, drop).map_err(ExecError::Gate)?;
    }
    let key = ticket.key.as_deref();
    policy
        .check(
            before_page.as_ref().and_then(PageState::url),
            region,
            ticket.action,
            key,
        )
        .map_err(|reason| ExecError::Policy {
            reason,
            label: policy
                .press_label(region, ticket.action, key)
                .unwrap_or(region.label())
                .to_owned(),
        })?;

    ledger
        .mark_consumed(ticket.ticket_id)
        .map_err(ExecError::Ticket)?;
    let target = ticket.target_id.clone();
    browser.dispatch(&target, input).map_err(|e| match e {
        crate::AgentError::InputRejected(m) => ExecError::Rejected(m),
        other => ExecError::Dispatch(other.to_string()),
    })?;
    Ok(Executed {
        ticket_id: ticket.ticket_id,
        target,
        action: ticket.action,
        before: fresh,
        before_page,
    })
}

#[cfg(test)]
mod tests {
    use ultra_instinct_core::parse_fixture;
    use ultra_instinct_guard::gate;

    use super::*;
    use crate::runtime::MockBrowser;

    const PAGE: &str = r#"
        viewport w=800 h=600
        region id=go role=button label="Go" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=other role=button label="Other" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        region id=name role=text_field label="Name" x=10 y=90 w=200 h=24 actions=click,type sources=dom,accessibility
    "#;

    fn id(raw: &str) -> RegionId {
        RegionId::try_new(raw).unwrap()
    }

    #[test]
    fn executes_exact_ticket_target_once() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone());
        let mut ledger = TicketLedger::new();
        let t = gate(&m, &id("go"), Action::Click, None, 0).unwrap();
        let ran = execute_ticketed(&mut b, &mut ledger, &t, &Input::Click).unwrap();
        assert_eq!(ran.target, id("go"));
        assert_eq!(b.press_log(), &[(id("go"), Action::Click)]);
        // Second use: refused, no second press.
        let err = execute_ticketed(&mut b, &mut ledger, &t, &Input::Click).unwrap_err();
        assert_eq!(err, ExecError::Ticket(TicketInvalid::TicketConsumed));
        assert_eq!(b.press_log().len(), 1);
    }

    #[test]
    fn operation_swap_is_refused_without_input() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone());
        let mut ledger = TicketLedger::new();
        let t = gate(&m, &id("name"), Action::Click, None, 0).unwrap();
        let err = execute_ticketed(&mut b, &mut ledger, &t, &Input::Type("x".into())).unwrap_err();
        assert_eq!(err, ExecError::Ticket(TicketInvalid::TicketMismatch));
        assert!(b.input_log().is_empty());
        assert!(!ledger.is_consumed(t.ticket_id));
    }

    #[test]
    fn mutated_target_is_stale_and_not_pressed() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone());
        let mut ledger = TicketLedger::new();
        let t = gate(&m, &id("go"), Action::Click, None, 0).unwrap();
        // Rerender: same id, different label.
        b.replace_manifold(
            parse_fixture(&PAGE.replace("label=\"Go\"", "label=\"Delete\"")).unwrap(),
        );
        let err = execute_ticketed(&mut b, &mut ledger, &t, &Input::Click).unwrap_err();
        assert!(err.is_stale(), "{err}");
        assert!(b.press_log().is_empty());
    }

    #[test]
    fn modal_after_ticket_is_stale_and_not_pressed() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone());
        let mut ledger = TicketLedger::new();
        let t = gate(&m, &id("go"), Action::Click, None, 0).unwrap();
        let with_modal = format!(
            "{PAGE}\n        region id=dlg role=dialog label=\"Session expired\" x=200 y=100 w=300 h=200 actions=focus sources=dom,accessibility flags=modal\n        region id=ok role=button label=\"OK\" x=220 y=250 w=80 h=24 actions=click parent=dlg sources=dom,accessibility\n"
        );
        b.replace_manifold(parse_fixture(&with_modal).unwrap());
        let err = execute_ticketed(&mut b, &mut ledger, &t, &Input::Click).unwrap_err();
        assert_eq!(err, ExecError::Ticket(TicketInvalid::WorldChanged));
        assert!(b.press_log().is_empty());
    }

    /// The page left the allowlist between the ticket and the press: the
    /// executor refuses on the fresh URL, does not consume, presses nothing.
    #[test]
    fn fresh_url_off_the_allowlist_is_refused_unconsumed() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone()).with_page(PageState::new(
            "https://shop.example/cart",
            "Cart",
            None,
        ));
        let mut ledger = TicketLedger::new();
        let policy = Policy::from_lists("shop.example", "").unwrap();
        let t = gate(&m, &id("go"), Action::Click, None, 0).unwrap();
        b.replace_page(PageState::new("https://evil.test/cart", "Cart", None));
        let err =
            execute_ticketed_with(&mut b, &mut ledger, &t, &Input::Click, &policy).unwrap_err();
        assert_eq!(
            err,
            ExecError::Policy {
                reason: GuardReason::DomainNotAllowed,
                label: "Go".into()
            }
        );
        assert!(!err.is_stale());
        assert!(!ledger.is_consumed(t.ticket_id));
        assert!(b.input_log().is_empty());
        // Back on the allowed host the same ticket still runs once.
        b.replace_page(PageState::new("https://shop.example/cart", "Cart", None));
        execute_ticketed_with(&mut b, &mut ledger, &t, &Input::Click, &policy).unwrap();
        assert!(ledger.is_consumed(t.ticket_id));
    }

    /// Enter in a field submits its form: the executor's own fresh look
    /// decides which button that presses (form evidence is not in the
    /// fingerprint, so the ticket is not stale), and a confirm label there
    /// refuses unconsumed.
    #[test]
    fn enter_whose_fresh_form_submits_a_confirm_label_is_refused_unconsumed() {
        use ultra_instinct_core::{ElementState, FormSubmit};
        let with_form = |form: FormSubmit| {
            let mut m = parse_fixture(PAGE).unwrap();
            let mut parts = m.get(&id("name")).unwrap().to_parts();
            parts.actions.push(Action::Key);
            let region = ultra_instinct_core::InteractionRegion::try_new(parts)
                .unwrap()
                .with_state(ElementState {
                    form: Some(form),
                    ..ElementState::default()
                });
            m.replace(region);
            m
        };
        let seen = with_form(FormSubmit::NotInForm);
        let mut b = MockBrowser::new(seen.clone());
        let mut ledger = TicketLedger::new();
        let policy = Policy::from_lists("", "Pay*").unwrap();
        let t = ultra_instinct_guard::gate_key(&seen, &id("name"), "Enter", None, 0).unwrap();
        b.replace_manifold(with_form(FormSubmit::Button("Pay $18".into())));
        let enter = Input::Key("Enter".into());
        let err = execute_ticketed_with(&mut b, &mut ledger, &t, &enter, &policy).unwrap_err();
        assert_eq!(
            err,
            ExecError::Policy {
                reason: GuardReason::NeedsConfirm,
                label: "Pay $18".into()
            }
        );
        assert!(!ledger.is_consumed(t.ticket_id));
        assert!(b.input_log().is_empty());
        let confirmed = policy.with_confirmed("Pay $18");
        execute_ticketed_with(&mut b, &mut ledger, &t, &enter, &confirmed).unwrap();
        assert_eq!(b.input_log().len(), 1);
    }

    #[test]
    fn confirm_pattern_refuses_a_click_until_the_label_is_confirmed() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone());
        let mut ledger = TicketLedger::new();
        let policy = Policy::from_lists("", "g*").unwrap();
        let t = gate(&m, &id("go"), Action::Click, None, 0).unwrap();
        let err =
            execute_ticketed_with(&mut b, &mut ledger, &t, &Input::Click, &policy).unwrap_err();
        assert_eq!(
            err,
            ExecError::Policy {
                reason: GuardReason::NeedsConfirm,
                label: "Go".into()
            }
        );
        assert!(!ledger.is_consumed(t.ticket_id));
        assert!(b.press_log().is_empty());
        let confirmed = policy.with_confirmed("go");
        execute_ticketed_with(&mut b, &mut ledger, &t, &Input::Click, &confirmed).unwrap();
        assert_eq!(b.press_log(), &[(id("go"), Action::Click)]);
    }

    #[test]
    fn page_rejection_consumes_ticket() {
        let m = parse_fixture(PAGE).unwrap();
        let mut b = MockBrowser::new(m.clone());
        b.reject_next("readonly");
        let mut ledger = TicketLedger::new();
        let t = gate(&m, &id("name"), Action::Type, None, 0).unwrap();
        let err = execute_ticketed(&mut b, &mut ledger, &t, &Input::Type("x".into())).unwrap_err();
        assert_eq!(err, ExecError::Rejected("readonly".into()));
        assert!(ledger.is_consumed(t.ticket_id));
    }

    #[test]
    fn exec_error_display_is_exact() {
        let cases = [
            (
                ExecError::Ticket(TicketInvalid::TicketConsumed),
                "ticket ticket-consumed",
            ),
            (
                ExecError::Gate(GuardReason::FrontLayer),
                "gate refused: front-layer",
            ),
            (
                ExecError::Observe("socket closed".into()),
                "observe before input failed: socket closed",
            ),
            (
                ExecError::Rejected("readonly".into()),
                "page rejected input: readonly",
            ),
            (
                ExecError::Dispatch("timeout".into()),
                "dispatch failed: timeout",
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.to_string(), want);
        }
    }
}
