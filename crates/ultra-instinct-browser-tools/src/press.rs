//! What every index press shares: its policy (the server's plus this call's
//! `confirm`) and how a refusal from the executor or the host-side consume
//! becomes the tool error a host matches on.

use serde_json::Value;
use ultra_instinct_agent::ExecError;
use ultra_instinct_browser::BrowserError;
use ultra_instinct_core::{Action, InteractionRegion};
use ultra_instinct_guard::{host_of, ConsumeError, Policy};
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
            host: host_of(label_or_url).unwrap_or_else(|| label_or_url.to_owned()),
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

/// Executor failures: the user's policy (confirm / domain) is its typed
/// refusal for `label` on `url`; ticket drift is a stale ref; other ticket
/// and gate refusals are `TicketInvalid` (never silently retried);
/// observe / dispatch / page rejection are `Browser`.
pub(crate) fn exec_error(err: ExecError, label: &str, url: Option<&str>) -> ToolError {
    match err {
        ExecError::Gate(reason @ (GuardReason::DomainNotAllowed | GuardReason::NeedsConfirm)) => {
            refusal_for(reason, label, url)
        }
        ExecError::Ticket(invalid) => ticket_error(invalid, err.to_string()),
        ExecError::Gate(_) => ToolError::TicketInvalid(err.to_string()),
        other => ToolError::Browser(other.to_string()),
    }
}

/// Host-side consume failures, the same way: a policy refusal for `label`
/// on `url`, an invalid lease as `TicketInvalid` / stale ref, a page-side
/// press failure as `Browser` (the ticket is already consumed).
pub(crate) fn consume_error(
    err: ConsumeError<BrowserError>,
    label: &str,
    url: Option<&str>,
) -> ToolError {
    match err {
        ConsumeError::Refused(reason) => refusal_for(reason, label, url),
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
) -> Result<(Option<String>, Option<InteractionRegion>, bool), ToolError> {
    let mut session = server.take_session(key)?;
    let observed = if now || session.is_stale() {
        best_effort(server, |_| session.observe().is_ok())
    } else {
        true
    };
    let (url, focused, known) = match (observed, session.fresh_manifold(), session.page()) {
        (true, Some(manifold), Some(page)) => (
            page.url().map(str::to_owned),
            page.focused().and_then(|id| manifold.get(id)).cloned(),
            true,
        ),
        _ => (None, None, false),
    };
    keep(server, key, session);
    Ok((url, focused, known))
}

/// Input with no target (`browser_keyboard`, `browser_send_key` /
/// `browser_hold_key` without an index) under the policy, before any of it
/// is sent: the page host as a press checks it (off the list, or an
/// unreadable URL while the list is set, is `domain_not_allowed`), and
/// `key` on the focused region is checked by the guard's press rule
/// ([`Policy::press_label`]: Enter / Space on a button-like control, Enter
/// in a form control that submits a form). An unknown focus with a confirm
/// list refuses an activating key.
pub(crate) fn check_focused(
    policy: &Policy,
    (url, focused, known): &(Option<String>, Option<InteractionRegion>, bool),
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
    let Some(key) = key.filter(|_| !policy.confirm_press.is_empty()) else {
        return Ok(());
    };
    match focused {
        Some(region) => {
            let label = policy
                .press_label(region, Action::Key, Some(key))
                .unwrap_or(region.label());
            policy
                .check(url.as_deref(), region, Action::Key, Some(key))
                .map_err(|reason| refuse(reason, label))
        }
        // No observation: whether this key presses anything is not known.
        None if !known && matches!(key, "Enter" | "Return" | " " | "Space" | "Spacebar") => {
            Err(refuse(GuardReason::NeedsConfirm, ""))
        }
        None => Ok(()),
    }
}
