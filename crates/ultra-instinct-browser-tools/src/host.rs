//! What the `browser_*` tools need from the server that runs them.
//!
//! The host owns live CDP sessions, the current-tab binding, per-key ticket
//! ledgers, buffered CDP events, the snapshot ring and the user's policy.
//! The tools own [`ToolState`] (HAR taps, blocked URLs, open dialogs); the
//! host only stores it. The host supplies raw storage; the trait's default
//! [`BrowserHost::push_events`], [`BrowserHost::keep_session`] and
//! [`BrowserHost::drop_session`] forward each buffered event batch to
//! [`ToolState::tee`] and drop a key's entry with [`ToolState::remove`], so
//! no host can skip either.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::Value;
use ultra_instinct_browser::{BrowserSession, CdpTransport, PageState};
use ultra_instinct_cdp::{CdpEvent, TargetBackend, DEFAULT_CDP_HTTP};
use ultra_instinct_core::InteractionManifold;
use ultra_instinct_guard::{Policy, TicketLedger};
use ultra_instinct_observe::history::SnapshotId;

use crate::browser_env::EnvState;
use crate::browser_extra::Dialog;
use crate::error::ToolError;

/// One live CDP session the host keeps between calls.
pub type LiveSession = BrowserSession<Box<dyn CdpTransport>>;

/// Output limits the `browser_*` tools apply ([`BrowserHost::limits`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolLimits {
    /// Longest `browser_get_state` text, in chars; a longer one is cut with
    /// `… [truncated N chars]` and `truncated: true`. Default 100 000.
    pub max_state_chars: usize,
    /// Most elements `browser_get_state` lists (text lines or JSON
    /// `elements`); the rest are counted in `dropped_elements` with
    /// `truncated: true`. Default 1 000.
    pub max_elements: usize,
    /// Largest decoded screenshot. A PNG over it is taken once more as a
    /// JPEG (quality 70); still over: `output_too_large`. Default 16 MiB,
    /// which fits a 32 MiB pipe frame as base64 with room to spare.
    pub max_screenshot_bytes: usize,
    /// Longest `browser_wait`, whatever the argument says. Default 60 s.
    pub max_wait: Duration,
    /// Longest `browser_get_text` / `browser_get_html` text, in UTF-16
    /// units. Cut inside the page script, so the rest never crosses the
    /// wire; the text ends `… [truncated N chars]` with `truncated: true`.
    /// Default 1 000 000.
    pub max_text_chars: usize,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            max_state_chars: 100_000,
            max_elements: 1_000,
            max_screenshot_bytes: 16 << 20,
            max_wait: Duration::from_secs(60),
            max_text_chars: 1_000_000,
        }
    }
}

/// Where `browser_download` may put files ([`BrowserHost::downloads`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Downloads {
    /// Any absolute path the call names (the default).
    #[default]
    Anywhere,
    /// `browser_download` refuses with `downloads_off`.
    Off,
    /// Only files directly in this folder; Chrome also writes there.
    Dir(PathBuf),
}

/// `raw` as a path the caller chose: absolute, or `path_not_absolute`. No
/// tool invents a default path or resolves a relative one.
pub(crate) fn absolute_path(raw: &str) -> Result<&Path, ToolError> {
    let path = Path::new(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(ToolError::PathNotAbsolute {
            path: raw.to_owned(),
        })
    }
}

/// Per-session-key state the `browser_*` tools keep on their host.
#[derive(Default)]
pub struct ToolState {
    /// Session key → HAR tap and blocked URLs (`browser_env`).
    pub(crate) env: EnvState,
    /// Session key → the JavaScript dialog open on that tab (`browser_dialog`).
    pub(crate) dialogs: BTreeMap<String, Dialog>,
    /// Text typed into sensitive fields, masked in every output (all tabs).
    pub(crate) secrets: crate::secrets::Secrets,
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
///
/// Implementors supply raw storage (the `*_raw` methods). The tools call the
/// provided [`keep_session`](Self::keep_session),
/// [`drop_session`](Self::drop_session) and
/// [`push_events`](Self::push_events), which keep [`ToolState`] in step: the
/// HAR tap sees every buffered event and a key's tool state goes with its
/// session.
pub trait BrowserHost {
    /// The kept session for `url`, or a fresh connection.
    fn take_session(&mut self, url: &str) -> Result<LiveSession, ToolError>;

    /// Store `session` for the next call on `url`. When room had to be made,
    /// return the evicted key, whose per-key host state (session, ledger,
    /// events, domain bits) is already dropped. Do not touch [`ToolState`].
    fn keep_session_raw(&mut self, url: &str, session: LiveSession) -> Option<String>;

    /// Drop a kept session and every per-key host state (ledger, events,
    /// domain bits). Do not touch [`ToolState`].
    fn drop_session_raw(&mut self, url: &str);

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

    /// Append events to `key`'s buffer, applying the host's buffer cap. Do
    /// not touch [`ToolState`].
    fn push_events_raw(&mut self, key: &str, events: Vec<CdpEvent>);

    /// Drain all buffered events for `key`.
    fn take_events(&mut self, key: &str) -> Vec<CdpEvent>;

    /// Events dropped for `key` by the buffer cap so far.
    fn dropped_events(&self, key: &str) -> u64;

    /// Diagnostic domains already enabled for `key` (bit0 console, bit1 network).
    fn enabled_domains(&self, key: &str) -> u8;

    /// Mark a diagnostic domain bit as enabled for `key`.
    fn enable_domain(&mut self, key: &str, bit: u8);

    /// Record an observation in the host's snapshot ring. `shown`: how many
    /// of its interactive elements the caller was shown (`browser_get_state`
    /// lists at most `max_elements`); an index past it names nothing.
    fn record(
        &mut self,
        origin: &str,
        manifold: Rc<InteractionManifold>,
        page: PageState,
        shown: usize,
    ) -> SnapshotId;

    /// The origin, manifold and shown-element count of a snapshot the host
    /// recorded.
    fn snapshot(&self, id: u64) -> Result<(&str, &Rc<InteractionManifold>, usize), ToolError> {
        Err(ToolError::UnknownSnapshot(id))
    }

    /// Whether index presses must name the `snapshot` they were read from
    /// (an index-only press refuses with `snapshot_required`). Default: no.
    fn require_snapshot(&self) -> bool {
        false
    }

    /// The policy navigation and presses run under.
    fn policy(&self) -> Result<&Policy, ToolError>;

    /// Where tabs come from (list / create / activate / close).
    fn backend(&mut self) -> &mut dyn TargetBackend;

    /// Output limits. Default [`ToolLimits::default`].
    fn limits(&self) -> ToolLimits {
        ToolLimits::default()
    }

    /// Where downloads may go. Default [`Downloads::Anywhere`].
    fn downloads(&self) -> Downloads {
        Downloads::Anywhere
    }

    /// The tools' own state, stored on the host.
    fn tool_state(&self) -> &ToolState;

    /// Mutable access to the tools' own state.
    fn tool_state_mut(&mut self) -> &mut ToolState;

    /// Keep `session` for the next call on `url`. An evicted key's
    /// [`ToolState`] is dropped with it.
    fn keep_session(&mut self, url: &str, session: LiveSession) {
        if let Some(evicted) = self.keep_session_raw(url, session) {
            self.tool_state_mut().remove(&evicted);
        }
    }

    /// Drop a kept session without keeping a replacement, with every
    /// per-key state (ledger, events, domain bits, [`ToolState`]).
    fn drop_session(&mut self, url: &str) {
        self.drop_session_raw(url);
        self.tool_state_mut().remove(url);
    }

    /// Buffer events drained from a session's transport under `key`. The
    /// HAR tap ([`ToolState::tee`]) sees them before the buffer cap applies.
    fn push_events(&mut self, key: &str, events: Vec<CdpEvent>) {
        self.tool_state_mut().tee(key, &events);
        self.push_events_raw(key, events);
    }
}

/// The `cdp` endpoint a call names, or the default one.
pub(crate) fn endpoint_of(arguments: &Value) -> Result<String, ToolError> {
    let endpoint = match arguments.get("cdp") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(other) => {
            return Err(ToolError::InvalidArguments(format!(
                "`cdp` must be a string, got {other}"
            )))
        }
    };
    Ok(endpoint.unwrap_or(DEFAULT_CDP_HTTP).to_owned())
}

/// The page-websocket key a `cdp` endpoint currently drives.
pub(crate) fn session_key(server: &dyn BrowserHost, endpoint: &str) -> String {
    server
        .current_tab(endpoint)
        .map(str::to_owned)
        .unwrap_or_else(|| endpoint.to_owned())
}

/// Keep the session, buffering any events its transport saw during the call
/// (the buffer, and through it the HAR tap, sees every event exactly once).
pub(crate) fn keep(server: &mut dyn BrowserHost, key: &str, mut session: LiveSession) {
    let events = session.transport_mut().drain_events();
    if !events.is_empty() {
        server.push_events(key, events);
    }
    server.keep_session(key, session);
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
