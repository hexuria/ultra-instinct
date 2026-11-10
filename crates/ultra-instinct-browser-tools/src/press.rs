//! What every index press shares: its policy (the server's plus this call's
//! `confirm`) and how a refusal from the executor or the host-side consume
//! becomes the tool error a host matches on.

use serde_json::Value;
use ultra_instinct_agent::ExecError;
use ultra_instinct_browser::BrowserError;
use ultra_instinct_core::{Action, InteractionRegion};
use ultra_instinct_guard::{
    is_activating_key, refused_place, ConsumeError, Policy, Refusal, UNOBSERVED_FOCUS,
};
use ultra_instinct_protocol::GuardReason;

use crate::bound::ticket_error;
use crate::error::ToolError;
use crate::host::{best_effort, keep, BrowserHost};

/// The server's policy plus this call's `confirm` label (this call only).
pub(crate) fn press_policy(
    server: &dyn BrowserHost,
    arguments: &Value,
) -> Result<Policy, ToolError> {
    let policy = server.policy()?.clone();
    Ok(match arguments.get("confirm") {
        None | Some(Value::Null) => policy,
        Some(Value::String(label)) => policy.with_confirmed(label),
        Some(other) => {
            return Err(ToolError::InvalidArguments(format!(
                "`confirm` must be a string, got {other}"
            )))
        }
    })
}

/// A policy refusal as the tool error a host matches on (not stale; a
/// retry gets the same answer): `needs-confirm: "Pay $18"` (the label is a
/// JSON string; pass it back as `confirm`) or `domain-not-allowed: <host>`.
pub(crate) fn refused(reason: GuardReason, label_or_url: &str) -> ToolError {
    match reason {
        GuardReason::NeedsConfirm if !label_or_url.is_empty() => ToolError::NeedsConfirm {
            label: label_or_url.to_owned(),
        },
        GuardReason::DomainNotAllowed if !label_or_url.is_empty() => ToolError::DomainNotAllowed {
            host: refused_place(label_or_url),
        },
        _ if label_or_url.is_empty() => ToolError::TicketInvalid(reason.to_string()),
        _ => ToolError::TicketInvalid(format!("{reason}: {label_or_url}")),
    }
}

/// [`refused`] with the right detail: the label for a confirm, the URL for
/// a domain.
pub(crate) fn refusal_for(reason: GuardReason, label: &str, url: Option<&str>) -> ToolError {
    match reason {
        GuardReason::NeedsConfirm => refused(reason, label),
        _ => refused(reason, url.unwrap_or("")),
    }
}

/// A guard [`Refusal`] as the tool error: its label for a confirm, the URL
/// it checked (the page, or a form's target) for a domain.
pub(crate) fn refusal_error(refusal: Refusal) -> ToolError {
    refusal_for(refusal.reason, &refusal.label, refusal.url.as_deref())
}

/// Executor failures: the user's policy (confirm / domain) is its typed
/// refusal for what the executor's fresh look checked; ticket drift is a
/// stale ref; other ticket and gate refusals are `TicketInvalid` (never
/// silently retried); observe / dispatch / page rejection are `Browser`.
pub(crate) fn exec_error(err: ExecError) -> ToolError {
    match err {
        // The label the executor's fresh look checked.
        ExecError::Policy { reason, label, url } => refusal_for(reason, &label, url.as_deref()),
        ExecError::Ticket(invalid) => ticket_error(invalid, err.to_string()),
        ExecError::Gate(_) => ToolError::TicketInvalid(err.to_string()),
        other => ToolError::Browser(other.to_string()),
    }
}

/// Host-side consume failures, the same way: a policy refusal as
/// [`refusal_error`], an invalid lease as `TicketInvalid` / stale ref, a
/// page-side press failure as `Browser` (the ticket is already consumed).
pub(crate) fn consume_error(err: ConsumeError<BrowserError>) -> ToolError {
    match err {
        ConsumeError::Refused(refusal) => refusal_error(refusal),
        ConsumeError::Invalid(invalid) => ticket_error(invalid, err.to_string()),
        other => ToolError::Browser(other.to_string()),
    }
}

/// The page's URL and focused region for input with no target, from an
/// observation taken now when `now` (or when a press ran since the last
/// one), else the last. A failed observation knows neither.
pub(crate) fn focus_now(
    server: &mut dyn BrowserHost,
    key: &str,
    now: bool,
) -> Result<(Option<String>, Option<InteractionRegion>), ToolError> {
    let mut session = server.take_session(key)?;
    let observed = if now || session.is_stale() {
        best_effort(server, |_| session.observe().is_ok())
    } else {
        true
    };
    let now = match (observed, session.fresh_manifold(), session.page()) {
        (true, Some(manifold), Some(page)) => (
            page.url().map(str::to_owned),
            page.focused().and_then(|id| manifold.get(id)).cloned(),
        ),
        _ => (None, None),
    };
    keep(server, key, session);
    Ok(now)
}

/// Input with no target (`browser_keyboard`, `browser_send_key` /
/// `browser_hold_key` without an index) under the policy, before any of it
/// is sent: the page host as a press checks it (off the list, or an
/// unreadable URL while the list is set, is `domain_not_allowed`), and
/// `key` on the focused region is checked by the guard's press rule
/// ([`Policy::press_label`]: Enter / Space on a button-like control, Enter
/// in a form control that submits a form, and a form submit's target host).
/// An activating key to a focus no observation names refuses: with an
/// allow-list `domain_not_allowed`, else with a confirm list
/// [`UNOBSERVED_FOCUS`].
pub(crate) fn check_focused(
    policy: &Policy,
    (url, focused): &(Option<String>, Option<InteractionRegion>),
    key: Option<&str>,
) -> Result<(), ToolError> {
    let refuse = |reason, label: &str| refusal_for(reason, label, url.as_deref());
    match url {
        Some(url) => policy.check_url(url).map_err(|r| refuse(r, ""))?,
        None if !policy.allow_domains.is_empty() => {
            return Err(refuse(GuardReason::DomainNotAllowed, ""))
        }
        None => {}
    }
    let Some(key) = key.filter(|_| !policy.is_empty()) else {
        return Ok(());
    };
    match focused {
        // The guard's own rule: the press label, and where a form submit
        // the key causes would go.
        Some(region) => policy
            .check_detail(url.as_deref(), region, Action::Key, Some(key))
            .map_err(refusal_error),
        None if !is_activating_key(key) => Ok(()),
        // No observed focus (no observation, or focus the observation does
        // not name, as in a cross-origin frame): what this key presses, and
        // where a form it submits goes, is not known.
        None if !policy.allow_domains.is_empty() => Err(refuse(GuardReason::DomainNotAllowed, "")),
        None if policy.needs_confirm(UNOBSERVED_FOCUS) => {
            Err(refuse(GuardReason::NeedsConfirm, UNOBSERVED_FOCUS))
        }
        None => Ok(()),
    }
}
