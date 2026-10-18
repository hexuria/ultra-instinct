//! State one stdio server keeps between tool calls.
//!
//! [`Server`] owns live CDP sessions keyed by endpoint and a bounded ring of
//! recent snapshots. It is driven by one blocking stdin reader, so there is no
//! interleaving. A failed live call drops that session. There is no retry and
//! no reconnect loop; the next call opens a fresh socket. Fixture origins do
//! not keep a transport between calls. Nothing is written to disk.
//!
//! A [`CdpConnector`] opens the transport for a `cdp` endpoint. The default
//! is [`WebSocketTransport::connect`]. Tests pass a connector that returns a
//! [`ultra_instinct_browser::ReplayTransport`], so the live-session paths (reuse,
//! stale observation, eviction, a dropped session) run without Chrome.
//!
//! The server also remembers the last few locate calls so a repeated
//! identical query on an unchanged page can be reported (see `repeat`).

use std::collections::BTreeMap;

use serde_json::Value;
use ultra_instinct_browser::{
    BrowserSession, CdpError, CdpTransport, PageState, WebSocketTransport,
};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_core::InteractionManifold;
use ultra_instinct_guard::TicketLedger;
use ultra_instinct_observe::history::{
    detect, HistoryError, SignedSnapshot, SnapshotId, SnapshotRing, StateSignature, TemporalSignal,
};

use crate::error::ToolError;
use crate::repeat::{LocateRepeats, QueryKey};

/// Live sessions kept at once. Opening another drops the oldest key.
pub const MAX_LIVE_SESSIONS: usize = 4;

/// Buffered CDP events kept per session key; oldest drop past this.
const MAX_BUFFERED_EVENTS: usize = 2048;

pub(crate) struct Snapshot {
    pub(crate) origin: String,
    pub(crate) manifold: InteractionManifold,
    pub(crate) page: PageState,
}

/// Opens the CDP transport for one endpoint.
pub type CdpConnector = Box<dyn FnMut(&str) -> Result<Box<dyn CdpTransport>, CdpError>>;

pub(crate) type LiveSession = BrowserSession<Box<dyn CdpTransport>>;

pub struct Server {
    connector: CdpConnector,
    sessions: BTreeMap<String, LiveSession>,
    session_order: Vec<String>,
    history: SnapshotRing<Snapshot>,
    locate_repeats: LocateRepeats,
    /// Endpoint → page-websocket url of the tab `browser_*` tools drive.
    tab_binding: BTreeMap<String, String>,
    /// Session key → one-shot ticket ledger for ticketed browser inputs.
    ledgers: BTreeMap<String, TicketLedger>,
    /// Session key → CDP events drained from transports between calls.
    events: BTreeMap<String, Vec<CdpEvent>>,
    /// Session key → CDP events dropped by the buffer cap (cumulative).
    dropped: BTreeMap<String, u64>,
    /// Session key → diagnostic domains already enabled (bit0 console, bit1 network).
    domains: BTreeMap<String, u8>,
}

impl SignedSnapshot for Snapshot {
    fn origin(&self) -> &str {
        &self.origin
    }

    fn state_signature(&self) -> StateSignature {
        StateSignature::from_manifold(&self.manifold)
    }
}

impl Server {
    /// A server whose `cdp` endpoints are live websockets.
    pub fn new() -> Self {
        Self::with_connector(|url| {
            WebSocketTransport::connect(url)
                .map(|transport| Box::new(transport) as Box<dyn CdpTransport>)
        })
    }

    /// A server whose `cdp` endpoints are opened by `connector`. Used by the
    /// mock environment tests; the stdio binary uses [`Server::new`].
    pub fn with_connector(
        connector: impl FnMut(&str) -> Result<Box<dyn CdpTransport>, CdpError> + 'static,
    ) -> Self {
        Self {
            connector: Box::new(connector),
            sessions: BTreeMap::new(),
            session_order: Vec::new(),
            history: SnapshotRing::default(),
            locate_repeats: LocateRepeats::default(),
            tab_binding: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            events: BTreeMap::new(),
            dropped: BTreeMap::new(),
            domains: BTreeMap::new(),
        }
    }

    /// One inbound JSON-RPC line. `None` means the client must not be answered.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        crate::rpc::handle_line_with(self, line)
    }

    /// Run one tool against this server's state.
    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
        crate::tools::dispatch(self, name, arguments)
    }

    pub(crate) fn record(
        &mut self,
        origin: &str,
        manifold: InteractionManifold,
        page: PageState,
    ) -> SnapshotId {
        self.history.push(Snapshot {
            origin: origin.to_owned(),
            manifold,
            page,
        })
    }

    pub(crate) fn snapshot(&self, id: u64) -> Result<&Snapshot, ToolError> {
        self.history
            .get(SnapshotId::new(id))
            .map_err(|err| match err {
                HistoryError::Evicted { id, oldest } => ToolError::SnapshotEvicted {
                    id: id.get(),
                    oldest: oldest.get(),
                },
                HistoryError::Unknown(id) => ToolError::UnknownSnapshot(id.get()),
                other => ToolError::Browser(other.to_string()),
            })
    }

    /// Signature no-op and loop signals for an act pair already in the ring.
    /// Data only. This does not retry.
    #[allow(dead_code)]
    pub(crate) fn temporal_signals(
        &self,
        before: SnapshotId,
        after: SnapshotId,
    ) -> Result<Vec<TemporalSignal>, HistoryError> {
        detect(&self.history, before, after)
    }

    /// Count this locate call among the recent ones with the same origin,
    /// page signature, and normalized query. Data only. This does not retry.
    pub(crate) fn record_locate(
        &mut self,
        origin: &str,
        manifold: &InteractionManifold,
        query: QueryKey,
    ) -> usize {
        self.locate_repeats
            .record(origin, StateSignature::from_manifold(manifold), query)
    }

    /// Newest snapshot recorded for `origin`, if it is still in the ring.
    #[allow(dead_code)]
    pub(crate) fn latest_for(&self, origin: &str) -> Option<SnapshotId> {
        self.history
            .iter_recent()
            .find(|(_, snapshot)| snapshot.origin == origin)
            .map(|(id, _)| id)
    }

    /// The kept session for `url`, or a fresh connection.
    pub(crate) fn take_session(&mut self, url: &str) -> Result<LiveSession, ToolError> {
        if let Some(session) = self.sessions.remove(url) {
            self.session_order.retain(|key| key != url);
            return Ok(session);
        }
        let transport = (self.connector)(url).map_err(|err| ToolError::Browser(err.to_string()))?;
        Ok(BrowserSession::new(transport))
    }

    /// Endpoints with a kept live session, oldest first.
    pub fn live_sessions(&self) -> &[String] {
        &self.session_order
    }

    /// Keep `session` for the next call on `url`.
    pub(crate) fn keep_session(&mut self, url: &str, session: LiveSession) {
        if self.sessions.len() >= MAX_LIVE_SESSIONS && !self.sessions.contains_key(url) {
            let oldest = self.session_order.remove(0);
            self.drop_keyed_state(&oldest);
        }
        self.session_order.retain(|key| key != url);
        self.session_order.push(url.to_owned());
        self.sessions.insert(url.to_owned(), session);
    }

    /// Drop a kept session without keeping a replacement. Also clears the
    /// key's ticket ledger, buffered events, and diagnostic-domain bits —
    /// they describe the dropped transport and must not outlive it.
    pub(crate) fn drop_session(&mut self, url: &str) {
        self.drop_keyed_state(url);
    }

    /// Remove `url` from the sessions map/order and every per-key state map.
    fn drop_keyed_state(&mut self, url: &str) {
        self.sessions.remove(url);
        self.session_order.retain(|key| key != url);
        self.ledgers.remove(url);
        self.events.remove(url);
        self.domains.remove(url);
        self.dropped.remove(url);
    }

    /// Page-websocket url `browser_*` tools drive for `endpoint`, when bound.
    pub(crate) fn current_tab(&self, endpoint: &str) -> Option<&str> {
        self.tab_binding.get(endpoint).map(String::as_str)
    }

    /// Point `browser_*` calls on `endpoint` at this tab's websocket.
    pub(crate) fn set_current_tab(&mut self, endpoint: &str, ws_url: &str) {
        self.tab_binding
            .insert(endpoint.to_owned(), ws_url.to_owned());
    }

    /// Unbind `endpoint` (falls back to the endpoint's default page target).
    pub(crate) fn clear_current_tab(&mut self, endpoint: &str) {
        self.tab_binding.remove(endpoint);
    }

    /// The ticket ledger for a session key, fresh when absent.
    pub(crate) fn take_ledger(&mut self, key: &str) -> TicketLedger {
        self.ledgers.remove(key).unwrap_or_default()
    }

    /// Put the ledger back after a ticketed input.
    pub(crate) fn keep_ledger(&mut self, key: &str, ledger: TicketLedger) {
        self.ledgers.insert(key.to_owned(), ledger);
    }

    /// Buffer events drained from a session's transport under `key`.
    pub(crate) fn push_events(&mut self, key: &str, events: Vec<CdpEvent>) {
        let buffer = self.events.entry(key.to_owned()).or_default();
        buffer.extend(events);
        if buffer.len() > MAX_BUFFERED_EVENTS {
            let drop = buffer.len() - MAX_BUFFERED_EVENTS;
            buffer.drain(..drop);
            *self.dropped.entry(key.to_owned()).or_default() += drop as u64;
        }
    }

    /// Events dropped for `key` by the buffer cap so far.
    pub(crate) fn dropped_events(&self, key: &str) -> u64 {
        self.dropped.get(key).copied().unwrap_or(0)
    }

    /// Drain all buffered events for `key`.
    pub(crate) fn take_events(&mut self, key: &str) -> Vec<CdpEvent> {
        self.events.remove(key).unwrap_or_default()
    }

    /// Diagnostic domains already enabled for `key` (bit0 console, bit1 network).
    pub(crate) fn enabled_domains(&self, key: &str) -> u8 {
        self.domains.get(key).copied().unwrap_or(0)
    }

    /// Mark a diagnostic domain bit as enabled for `key`.
    pub(crate) fn enable_domain(&mut self, key: &str, bit: u8) {
        *self.domains.entry(key.to_owned()).or_default() |= bit;
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
