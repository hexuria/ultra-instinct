//! Index refs bound to what the caller saw.
//!
//! `browser_get_state` records the observation it printed and returns its
//! `snapshot` id. An index press that passes that id resolves the index in
//! that snapshot (not in whatever the page shows now) and mints its ticket
//! there: the ticket binds the region the caller saw, its role, label and
//! fingerprint (box included), and its target-scoped world. The press then
//! runs the usual boundary on a fresh observation (`execute_ticketed`'s
//! revalidate, or `revalidate` + gate before `consume_ticket_once`), so an
//! element that moved, changed, or left refuses as [`ToolError::StaleRef`]
//! and nothing is pressed. Focus is the one world fact taken from the page
//! now: the caller's own typing moves it between `get_state` and the press.
//!
//! Without `snapshot` an index resolves on a fresh observation, as before,
//! unless the host requires snapshots ([`BrowserHost::require_snapshot`]).

use std::ops::Deref;
use std::rc::Rc;

use serde_json::Value;
use ultra_instinct_core::{Action, InteractionManifold, RegionId};
use ultra_instinct_guard::{
    issue_ticket, revalidate, ActionTicket, GuardCandidate, TicketInvalid, WorldSnapshot,
};

use crate::browser_tools::interactive_index;
use crate::error::ToolError;
use crate::host::BrowserHost;

/// `browser_tools` index presses whose schema takes `snapshot` (the
/// `browser_extra` / `browser_env` ones add it in their own specs).
pub(crate) const SNAPSHOT_TOOLS: [&str; 12] = [
    "browser_select_dropdown",
    "browser_click",
    "browser_right_click",
    "browser_middle_click",
    "browser_double_click",
    "browser_triple_click",
    "browser_hover",
    "browser_drag",
    "browser_type",
    "browser_form_input",
    "browser_send_key",
    "browser_file_upload",
];

/// Schema of the `snapshot` argument.
pub(crate) fn snapshot_prop() -> Value {
    serde_json::json!({"type": ["integer", "string"], "description": "The snapshot id browser_get_state returned. The index is read in that snapshot and the press is bound to the element it showed: if that element moved, changed, or left, the call refuses with code stale_ref and presses nothing."})
}

/// The snapshot origin `browser_get_state` records for a session key.
pub(crate) fn origin(key: &str) -> String {
    format!("browser:{key}")
}

/// The optional `snapshot` argument: an id from `browser_get_state`.
pub(crate) fn snapshot_arg(arguments: &Value) -> Result<Option<u64>, ToolError> {
    match arguments.get("snapshot") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolError::BadSnapshot(n.to_string())),
        Some(Value::String(s)) => s
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| ToolError::BadSnapshot(s.clone())),
        Some(other) => Err(ToolError::BadSnapshot(other.to_string())),
    }
}

/// [`snapshot_arg`] for a press: the id must be recorded, and its absence
/// refuses when the host requires snapshots.
pub(crate) fn required_snapshot(
    server: &dyn BrowserHost,
    arguments: &Value,
) -> Result<Option<u64>, ToolError> {
    let snapshot = snapshot_arg(arguments)?;
    match snapshot {
        // An unknown or evicted id refuses before any CDP call.
        Some(id) => {
            server.snapshot(id)?;
        }
        None if server.require_snapshot() => return Err(ToolError::SnapshotRequired),
        None => {}
    }
    Ok(snapshot)
}

/// The region at 1-based `index` in `order`.
pub(crate) fn at_index(order: &[RegionId], index: usize) -> Result<RegionId, ToolError> {
    index
        .checked_sub(1)
        .and_then(|i| order.get(i))
        .cloned()
        .ok_or(ToolError::UnknownRef {
            index,
            indexed: order.len(),
        })
}

/// The manifold an index press mints its ticket on: the fresh observation,
/// borrowed, or a recorded snapshot, shared (the host keeps it in its ring
/// while the press mutates the host: ledger, secrets).
pub(crate) enum Seen<'a> {
    Fresh(&'a InteractionManifold),
    Snapshot(Rc<InteractionManifold>),
}

impl Seen<'_> {
    /// The snapshot's shared manifold; `None` for the fresh one.
    pub(crate) fn snapshot(self) -> Option<Rc<InteractionManifold>> {
        match self {
            Self::Fresh(_) => None,
            Self::Snapshot(manifold) => Some(manifold),
        }
    }
}

impl Deref for Seen<'_> {
    type Target = InteractionManifold;

    fn deref(&self) -> &InteractionManifold {
        match self {
            Self::Fresh(manifold) => manifold,
            Self::Snapshot(manifold) => manifold,
        }
    }
}

/// The manifold an index press mints its ticket on, and the region `index`
/// names there: the snapshot's (which must be this tab's) when one is given,
/// else `fresh`. In a snapshot only the elements the caller was shown are
/// indexed: one past them refuses [`ToolError::UnknownRef`].
pub(crate) fn resolve<'a>(
    server: &dyn BrowserHost,
    key: &str,
    snapshot: Option<u64>,
    fresh: &'a InteractionManifold,
    index: usize,
) -> Result<(Seen<'a>, RegionId), ToolError> {
    let Some(id) = snapshot else {
        let region = at_index(&interactive_index(fresh), index)?;
        return Ok((Seen::Fresh(fresh), region));
    };
    let (from, seen, shown) = server.snapshot(id)?;
    if from != origin(key) {
        return Err(ToolError::StaleRef {
            reason: "other-tab".into(),
        });
    }
    let mut order = interactive_index(seen);
    order.truncate(shown);
    let region = at_index(&order, index)?;
    Ok((Seen::Snapshot(Rc::clone(seen)), region))
}

/// A ticket error as the tool error: drift is a stale ref, the rest
/// (consumed, mismatch) stays `TicketInvalid` with `text`.
pub(crate) fn ticket_error(invalid: TicketInvalid, text: String) -> ToolError {
    match invalid {
        TicketInvalid::WorldChanged | TicketInvalid::TargetChanged | TicketInvalid::TargetGone => {
            ToolError::StaleRef {
                reason: invalid.to_string(),
            }
        }
        _ => ToolError::TicketInvalid(text),
    }
}

/// Revalidate `ticket` on the fresh observation (the host-boundary order:
/// before the hard gate and the consume).
pub(crate) fn still_seen(
    ticket: &ActionTicket,
    fresh: &InteractionManifold,
    focused: Option<RegionId>,
) -> Result<(), ToolError> {
    revalidate(ticket, fresh, focused).map_err(|invalid| ticket_error(invalid, String::new()))
}

/// `id` in `fresh` is the region the caller saw in `seen` (same role, label,
/// fingerprint and neighbourhood), through the same `revalidate` a ticket
/// gets. For indexed calls that consume no ticket (hover, a drag's
/// destination when only its source is ticketed).
pub(crate) fn same_region(
    seen: &InteractionManifold,
    fresh: &InteractionManifold,
    id: &RegionId,
    focused: Option<RegionId>,
) -> Result<(), ToolError> {
    let region = seen.get(id).ok_or(ToolError::StaleRef {
        reason: TicketInvalid::TargetGone.to_string(),
    })?;
    let candidate = GuardCandidate {
        id: id.clone(),
        role: region.role(),
        label: region.label().to_owned(),
        confidence: 1.0,
    };
    let world = WorldSnapshot::of_target(seen, focused.clone(), id);
    let ticket = issue_ticket(0, Action::Focus, &candidate, region, &world);
    still_seen(&ticket, fresh, focused)
}
