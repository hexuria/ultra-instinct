//! What the `browser_*` tools need from the server that runs them.
//!
//! The host owns live CDP sessions, the current-tab binding, per-key ticket
//! ledgers, buffered CDP events, the snapshot ring and the user's policy.
//! The tools own [`ToolState`] (HAR taps, blocked URLs, open dialogs); the
//! host only stores it, forwards each buffered event batch to
//! [`ToolState::tee`] and drops a key's entry with [`ToolState::remove`].

use std::collections::BTreeMap;

use ultra_instinct_browser::{BrowserSession, CdpTransport, PageState};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_core::InteractionManifold;
use ultra_instinct_guard::{Policy, TicketLedger};
use ultra_instinct_observe::history::SnapshotId;

use crate::browser_env::EnvState;
use crate::browser_extra::Dialog;
use crate::error::ToolError;

/// One live CDP session the host keeps between calls.
pub type LiveSession = BrowserSession<Box<dyn CdpTransport>>;

/// Per-session-key state the `browser_*` tools keep on their host.
#[derive(Default)]
pub struct ToolState {
    /// Session key → HAR tap and blocked URLs (`browser_env`).
    pub(crate) env: EnvState,
    /// Session key → the JavaScript dialog open on that tab (`browser_dialog`).
    pub(crate) dialogs: BTreeMap<String, Dialog>,
}

impl ToolState {
    /// Copy Network events into `key`'s HAR recording, if one runs. Called
    /// for every batch the host buffers, before its buffer cap applies.
    pub fn tee(&mut self, key: &str, events: &[CdpEvent]) {
        self.env.tee(key, events);
    }

    /// Forget everything kept for `key` (its session was dropped).
    pub fn remove(&mut self, key: &str) {
        self.env.remove(key);
        self.dialogs.remove(key);
    }
}

/// The state access the `browser_*` tools need. A session key is a CDP
/// endpoint or a tab's page-websocket url.
pub trait BrowserHost {
    /// The kept session for `url`, or a fresh connection.
    fn take_session(&mut self, url: &str) -> Result<LiveSession, ToolError>;

    /// Keep `session` for the next call on `url`.
    fn keep_session(&mut self, url: &str, session: LiveSession);

    /// Drop a kept session without keeping a replacement, with every
    /// per-key state (ledger, events, domain bits, [`ToolState`]).
    fn drop_session(&mut self, url: &str);

    /// Page-websocket url `browser_*` tools drive for `endpoint`, when bound.
    fn current_tab(&self, endpoint: &str) -> Option<&str>;

    /// Point `browser_*` calls on `endpoint` at this tab's websocket.
    fn set_current_tab(&mut self, endpoint: &str, ws_url: &str);

    /// Unbind `endpoint` (falls back to the endpoint's default page target).
    fn clear_current_tab(&mut self, endpoint: &str);

    /// The ticket ledger for a session key, fresh when absent.
    fn take_ledger(&mut self, key: &str) -> TicketLedger;

    /// Put the ledger back after a ticketed input.
    fn keep_ledger(&mut self, key: &str, ledger: TicketLedger);

    /// Buffer events drained from a session's transport under `key`. Must
    /// pass them to [`ToolState::tee`] first.
    fn push_events(&mut self, key: &str, events: Vec<CdpEvent>);

    /// Drain all buffered events for `key`.
    fn take_events(&mut self, key: &str) -> Vec<CdpEvent>;

    /// Events dropped for `key` by the buffer cap so far.
    fn dropped_events(&self, key: &str) -> u64;

    /// Diagnostic domains already enabled for `key` (bit0 console, bit1 network).
    fn enabled_domains(&self, key: &str) -> u8;

    /// Mark a diagnostic domain bit as enabled for `key`.
    fn enable_domain(&mut self, key: &str, bit: u8);

    /// Record an observation in the host's snapshot ring.
    fn record(
        &mut self,
        origin: &str,
        manifold: InteractionManifold,
        page: PageState,
    ) -> SnapshotId;

    /// The policy navigation and presses run under.
    fn policy(&self) -> Result<&Policy, ToolError>;

    /// The tools' own state, stored on the host.
    fn tool_state(&self) -> &ToolState;

    /// The tools' own state, stored on the host.
    fn tool_state_mut(&mut self) -> &mut ToolState;
}

impl dyn BrowserHost + '_ {
    /// The JavaScript dialog open on `key`'s tab, when one is.
    pub(crate) fn dialog(&self, key: &str) -> Option<&Dialog> {
        self.tool_state().dialogs.get(key)
    }

    /// Record (`Some`) or clear (`None`) the dialog open on `key`'s tab.
    pub(crate) fn set_dialog(&mut self, key: &str, dialog: Option<Dialog>) {
        let dialogs = &mut self.tool_state_mut().dialogs;
        match dialog {
            Some(dialog) => dialogs.insert(key.to_owned(), dialog),
            None => dialogs.remove(key),
        };
    }
}
