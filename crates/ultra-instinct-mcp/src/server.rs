//! State one stdio server keeps between tool calls.
//!
//! [`Server`] owns live CDP sessions keyed by endpoint and a bounded ring of
//! recent snapshots. It is driven by one blocking stdin reader, so there is no
//! interleaving. A failed live call drops that session. There is no retry and
//! no reconnect loop; the next call opens a fresh socket. Fixture origins do
//! not keep a transport between calls. Nothing is written to disk.
//!
//! A [`TargetBackend`] lists, opens, switches and closes tabs and opens a
//! session's transport. The default is [`HttpBackend`]: Chrome's `/json`
//! endpoints plus a [`CdpConnector`] ([`WebSocketTransport::connect`] unless
//! [`Server::with_connector`] names another). Tests pass a connector that
//! returns a [`ultra_instinct_browser::ReplayTransport`], so the live-session
//! paths (reuse, stale observation, eviction, a dropped session) run without
//! Chrome. [`Server::with_pipe`] drives a caller-owned `--remote-debugging-pipe`
//! instead (`crate::PipeBackend`, ADR 0013): no HTTP endpoint at all.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde_json::Value;
use ultra_instinct_browser::{
    BrowserSession, CdpError, CdpTransport, PageState, WebSocketTransport,
};
use ultra_instinct_browser_tools::{
    BrowserHost, Downloads, HttpBackend, LiveSession, TargetBackend, ToolLimits, ToolState,
};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_core::InteractionManifold;
use ultra_instinct_guard::{Policy, TicketLedger};
use ultra_instinct_observe::history::{
    detect, HistoryError, SignedSnapshot, SnapshotId, SnapshotRing, StateSignature, TemporalSignal,
};

use crate::ToolError;

/// Live sessions kept at once. Opening another drops the oldest key.
pub const MAX_LIVE_SESSIONS: usize = 4;

/// Buffered CDP events kept per session key; oldest drop past this.
const MAX_BUFFERED_EVENTS: usize = 2048;

/// Comma list of hosts (`shop.example`, `*.shop.example`) the `browser_*`
/// tools may navigate to and give input on. Read by [`Server::new`].
pub const ENV_ALLOW_DOMAINS: &str = "ULTRA_INSTINCT_ALLOW_DOMAINS";

/// Comma list of label globs (`Pay*`) whose CLICK-family press needs a
/// `confirm` argument naming the label. Read by [`Server::new`].
pub const ENV_CONFIRM_PRESS: &str = "ULTRA_INSTINCT_CONFIRM_PRESS";

pub(crate) struct Snapshot {
    pub(crate) origin: String,
    /// Shared with the index presses that resolve in it (no copy per press).
    pub(crate) manifold: Rc<InteractionManifold>,
    pub(crate) page: PageState,
    /// Interactive elements `browser_get_state` showed (`max_elements`).
    pub(crate) shown: usize,
}

/// Opens the CDP transport for one endpoint.
pub type CdpConnector = Box<dyn FnMut(&str) -> Result<Box<dyn CdpTransport>, CdpError>>;

pub struct Server {
    backend: Box<dyn TargetBackend>,
    /// The last typed CDP failure (pipe closed, timeout, target gone, …) a
    /// session transport hit during the current call.
    cdp_failure: Rc<RefCell<Option<CdpError>>>,
    /// While [`Server::run_goal`] runs: session calls refuse past this.
    call_deadline: Rc<Cell<Option<std::time::Instant>>>,
    sessions: BTreeMap<String, LiveSession>,
    session_order: Vec<String>,
    history: SnapshotRing<Snapshot>,
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
    /// Session key → state the `browser_*` tools own (HAR taps, blocked
    /// URLs, open dialogs).
    tools: ToolState,
    /// The user's policy for navigation and presses, or why the configured
    /// one is invalid (then every navigate and press refuses).
    policy: Result<Policy, String>,
    /// Index presses must name the `snapshot` they were read from.
    require_snapshot: bool,
    limits: ToolLimits,
    downloads: Downloads,
    /// `Some`: only these tools run and `tools/list` shows only them.
    allowlist: Option<BTreeSet<String>>,
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
    /// A server whose `cdp` endpoints are live websockets, under the policy
    /// in [`ENV_ALLOW_DOMAINS`] / [`ENV_CONFIRM_PRESS`] (unset: allow all).
    pub fn new() -> Self {
        Self::with_connector(|url| {
            WebSocketTransport::connect(url)
                .map(|transport| Box::new(transport) as Box<dyn CdpTransport>)
        })
        .with_env_policy()
    }

    /// This server under the policy in [`ENV_ALLOW_DOMAINS`] /
    /// [`ENV_CONFIRM_PRESS`] (unset: allow all; unparsable: every call that
    /// needs the policy fails with the parse error).
    pub fn with_env_policy(mut self) -> Self {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        self.policy = Policy::from_lists(&var(ENV_ALLOW_DOMAINS), &var(ENV_CONFIRM_PRESS))
            .map_err(|err| format!("{ENV_ALLOW_DOMAINS} / {ENV_CONFIRM_PRESS}: {err}"));
        self
    }

    /// This server under `policy` instead of the environment's.
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = Ok(policy);
        self
    }

    /// A server whose `cdp` endpoints are opened by `connector`. Used by the
    /// mock environment tests; the stdio binary uses [`Server::new`].
    pub fn with_connector(
        connector: impl FnMut(&str) -> Result<Box<dyn CdpTransport>, CdpError> + 'static,
    ) -> Self {
        Self::with_backend(HttpBackend::new(connector))
    }

    /// A server whose tabs and sessions come from `backend`, under an empty
    /// policy (allow all).
    pub fn with_backend(backend: impl TargetBackend + 'static) -> Self {
        Self {
            backend: Box::new(backend),
            cdp_failure: Rc::new(RefCell::new(None)),
            call_deadline: Rc::new(Cell::new(None)),
            sessions: BTreeMap::new(),
            session_order: Vec::new(),
            history: SnapshotRing::default(),
            tab_binding: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            events: BTreeMap::new(),
            dropped: BTreeMap::new(),
            domains: BTreeMap::new(),
            tools: ToolState::default(),
            policy: Ok(Policy::default()),
            require_snapshot: false,
            limits: ToolLimits::default(),
            downloads: Downloads::default(),
            allowlist: None,
        }
    }

    /// Replace the policy now; open sessions stay open and the next call
    /// runs under `policy`. A `confirm` argument still lifts a confirm for
    /// its own call only.
    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = Ok(policy);
    }

    /// Output limits for the `browser_*` tools (see [`ToolLimits`] for the
    /// defaults and what happens at each).
    pub fn with_limits(mut self, limits: ToolLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Where `browser_download` may write: [`Downloads::Anywhere`] (default,
    /// any absolute path), [`Downloads::Off`] (refuses `downloads_off`), or
    /// [`Downloads::Dir`] (only files directly in that folder, else
    /// `path_not_allowed`).
    pub fn with_downloads(mut self, downloads: Downloads) -> Self {
        self.downloads = downloads;
        self
    }

    /// Only `names` run (any other gives `tool_not_allowed` before any CDP
    /// call) and `tools/list` lists only them. Default: every tool.
    pub fn with_tool_allowlist<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowlist = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// [`crate::run_goal`] on the current tab (the bound pipe tab, or the
    /// default endpoint's), under this server's policy. The session stays
    /// open for the next tool call, with the run's last observation.
    ///
    /// Not a tool, but the allow-list names it: with
    /// [`Server::with_tool_allowlist`] set, the run goes ahead only when the
    /// list holds `run_goal` (the loop clicks, types, selects, hovers, drags,
    /// scrolls and presses keys, whatever press tools the list names). Else
    /// it ends [`crate::RunOutcome::Failed`] with
    /// ``tool_not_allowed: tool `run_goal` is not allowed on this server``
    /// before any CDP call.
    ///
    /// Every payload the run types or selects is remembered before it is
    /// dispatched and masked in every later tool output, error and HAR, and
    /// in this report's labels and reasons: a sensitive field's at any
    /// length, any other field's from 4 chars (typed text is user data).
    /// [`crate::RunReport::typed`] still holds them: never log it.
    pub fn run_goal(&mut self, goal: &str, limits: crate::RunLimits) -> crate::RunReport {
        let failed = |error: String| crate::RunReport {
            steps: Vec::new(),
            outcome: crate::RunOutcome::Failed { error },
            typed: crate::TypedText::default(),
        };
        if !self.allows("run_goal") {
            let refused = ToolError::ToolNotAllowed {
                tool: "run_goal".into(),
            };
            return failed(format!("{}: {refused}", refused.code()));
        }
        let policy = match &self.policy {
            Ok(policy) => policy.clone(),
            Err(err) => return failed(err.clone()),
        };
        let endpoint = self
            .backend
            .endpoint()
            .unwrap_or(ultra_instinct_cdp::DEFAULT_CDP_HTTP)
            .to_owned();
        // Binds the first pipe tab when none is bound yet.
        let _ = self.fixed_arguments("browser_get_state", &Value::Null);
        let key = self.tab_binding.get(&endpoint).cloned().unwrap_or(endpoint);
        let session = match Host(self).take_session(&key) {
            Ok(session) => session,
            Err(err) => return failed(err.to_string()),
        };
        self.call_deadline.set(limits.deadline);
        let tools = &mut self.tools;
        let (mut report, mut session) =
            crate::run::run_goal_typing(session, goal, policy, limits, |typed| {
                tools
                    .remember_typed(&typed.text, typed.sensitive)
                    .map_err(|err| err.to_string())
            });
        self.call_deadline.set(None);
        let scrub = |text: &mut String| *text = self.tools.scrub_text(text);
        for step in &mut report.steps {
            scrub(&mut step.target_label);
        }
        match &mut report.outcome {
            crate::RunOutcome::Done { reason } | crate::RunOutcome::Abstained { why: reason } => {
                scrub(reason)
            }
            crate::RunOutcome::Blocked { label, reason } => {
                scrub(label);
                scrub(reason);
            }
            crate::RunOutcome::Failed { error } => scrub(error),
        }
        let events = session.transport_mut().drain_events();
        let mut host = Host(self);
        if !events.is_empty() {
            host.push_events(&key, events);
        }
        host.keep_session(&key, session);
        report
    }

    /// Whether `name` may run here (the allow-list, when one is set).
    pub fn allows(&self, name: &str) -> bool {
        self.allowlist
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name))
    }

    /// With `true`, every index press (click, type, select, check, focus,
    /// key on an index, right / middle / double / triple click, file upload,
    /// hover, drag, download) must pass the `snapshot` id its
    /// `browser_get_state` returned; an index alone refuses with
    /// `snapshot_required` before any CDP call. Default `false`.
    pub fn require_snapshot(mut self, on: bool) -> Self {
        self.require_snapshot = on;
        self
    }

    /// A server that drives the browser at the other end of a caller-owned
    /// `--remote-debugging-pipe` (ADR 0013), under an empty policy. No HTTP
    /// endpoint is used: tabs are `Target.*` calls, sessions are flat
    /// [`ultra_instinct_cdp::PipePage`]s, and every call's `cdp` argument is
    /// ignored (session keys are `pipe:<targetId>`). The first call binds the
    /// first page tab. One thread owns the server and the pipe.
    #[cfg(unix)]
    pub fn with_pipe(browser: ultra_instinct_cdp::PipeBrowser) -> Self {
        Self::with_backend(ultra_instinct_cdp::PipeBackend::new(browser))
    }

    /// One inbound JSON-RPC line. `None` means the client must not be answered.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        crate::rpc::handle_line_with(self, line)
    }

    /// Run one tool against this server's state.
    ///
    /// Every result and error, of every tool, leaves through one choke
    /// point that masks the text typed so far (`ToolState::scrub_result`).
    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
        let result = self.call_unmasked(name, arguments);
        self.tools.scrub_result(name, result)
    }

    fn call_unmasked(&mut self, name: &str, arguments: &Value) -> Result<Value, ToolError> {
        if !self.allows(name) {
            return Err(ToolError::ToolNotAllowed {
                tool: name.to_owned(),
            });
        }
        self.cdp_failure.borrow_mut().take();
        let fixed = self.fixed_arguments(name, arguments);
        let arguments = fixed.as_ref().unwrap_or(arguments);
        let result = crate::tools::dispatch(self, name, arguments);
        let seen = self.cdp_failure.borrow_mut().take();
        result.map_err(|err| {
            let err = err.or_cdp(seen.as_ref());
            ultra_instinct_browser_tools::fill_dialog(&mut Host(self), arguments, err)
        })
    }

    /// With a fixed-endpoint backend (the pipe): `cdp` names that endpoint
    /// for `browser_*` tools and the bound tab for observe / guard / verify
    /// (unless they read a fixture), and an unbound endpoint is bound to the
    /// first page tab. `None`: use the arguments as given.
    fn fixed_arguments(&mut self, name: &str, arguments: &Value) -> Option<Value> {
        let endpoint = self.backend.endpoint()?.to_owned();
        let mut fixed = match arguments {
            Value::Object(map) => map.clone(),
            Value::Null => serde_json::Map::new(),
            _ => return None,
        };
        if !self.tab_binding.contains_key(&endpoint) {
            // Errors surface on the call itself (a dead pipe, no tab).
            if let Some(first) = self
                .backend
                .list_pages(&endpoint)
                .ok()
                .and_then(|tabs| tabs.into_iter().next())
            {
                self.tab_binding.insert(endpoint.clone(), first.key);
            }
        }
        let cdp = if name.starts_with("browser_") {
            Some(endpoint.clone())
        } else if fixed.get("fixture").is_some_and(|f| !f.is_null()) {
            None
        } else {
            Some(self.tab_binding.get(&endpoint).cloned().unwrap_or(endpoint))
        };
        fixed.remove("cdp");
        if let Some(cdp) = cdp {
            fixed.insert("cdp".into(), Value::String(cdp));
        }
        Some(Value::Object(fixed))
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

    /// Newest snapshot recorded for `origin`, if it is still in the ring.
    #[allow(dead_code)]
    pub(crate) fn latest_for(&self, origin: &str) -> Option<SnapshotId> {
        self.history
            .iter_recent()
            .find(|(_, snapshot)| snapshot.origin == origin)
            .map(|(id, _)| id)
    }

    /// Endpoints with a kept live session, oldest first.
    pub fn live_sessions(&self) -> &[String] {
        &self.session_order
    }

    /// Remove `url` from the sessions map/order and every per-key host state
    /// map. [`ToolState`] is the trait's to drop (`BrowserHost::drop_session`).
    fn drop_keyed_state(&mut self, url: &str) {
        self.sessions.remove(url);
        self.session_order.retain(|key| key != url);
        self.ledgers.remove(url);
        self.events.remove(url);
        self.domains.remove(url);
        self.dropped.remove(url);
    }
}

/// The [`BrowserHost`] the `browser_*` tools run on: a crate-private view of
/// a [`Server`]. `Server` itself does not implement the trait, so its ledger,
/// event and session mutators stay out of the public API (one-shot ledger,
/// anti-drift #4).
pub(crate) struct Host<'a>(pub(crate) &'a mut Server);

impl BrowserHost for Host<'_> {
    fn record(
        &mut self,
        origin: &str,
        manifold: Rc<InteractionManifold>,
        page: PageState,
        shown: usize,
    ) -> SnapshotId {
        self.0.history.push(Snapshot {
            origin: origin.to_owned(),
            manifold,
            page,
            shown,
        })
    }

    /// The kept session for `url`, or a fresh connection.
    fn take_session(&mut self, url: &str) -> Result<LiveSession, ToolError> {
        let server = &mut *self.0;
        if let Some(session) = server.sessions.remove(url) {
            server.session_order.retain(|key| key != url);
            return Ok(session);
        }
        let transport = server
            .backend
            .connect(url)
            .map_err(|err| ToolError::cdp(&err))?;
        Ok(BrowserSession::new(Box::new(Recorded {
            inner: crate::run::Deadlined {
                inner: transport,
                deadline: None,
            },
            seen: Rc::clone(&server.cdp_failure),
            deadline: Rc::clone(&server.call_deadline),
        })))
    }

    /// Keep `session` for the next call on `url`, evicting the oldest key
    /// past [`MAX_LIVE_SESSIONS`].
    fn keep_session_raw(&mut self, url: &str, session: LiveSession) -> Option<String> {
        let server = &mut *self.0;
        let mut evicted = None;
        if server.sessions.len() >= MAX_LIVE_SESSIONS && !server.sessions.contains_key(url) {
            let oldest = server.session_order.remove(0);
            server.drop_keyed_state(&oldest);
            evicted = Some(oldest);
        }
        server.session_order.retain(|key| key != url);
        server.session_order.push(url.to_owned());
        server.sessions.insert(url.to_owned(), session);
        evicted
    }

    /// Drop a kept session without keeping a replacement. Also clears the
    /// key's ticket ledger, buffered events, and diagnostic-domain bits —
    /// they describe the dropped transport and must not outlive it.
    fn drop_session_raw(&mut self, url: &str) {
        self.0.drop_keyed_state(url);
    }

    /// Page-websocket url `browser_*` tools drive for `endpoint`, when bound.
    fn current_tab(&self, endpoint: &str) -> Option<&str> {
        self.0.tab_binding.get(endpoint).map(String::as_str)
    }

    /// Point `browser_*` calls on `endpoint` at this tab's websocket.
    fn set_current_tab(&mut self, endpoint: &str, ws_url: &str) {
        self.0
            .tab_binding
            .insert(endpoint.to_owned(), ws_url.to_owned());
    }

    /// Unbind `endpoint` (falls back to the endpoint's default page target).
    fn clear_current_tab(&mut self, endpoint: &str) {
        self.0.tab_binding.remove(endpoint);
    }

    /// The ticket ledger for a session key, fresh when absent.
    fn take_ledger(&mut self, key: &str) -> TicketLedger {
        self.0.ledgers.remove(key).unwrap_or_default()
    }

    /// Put the ledger back after a ticketed input.
    fn keep_ledger(&mut self, key: &str, ledger: TicketLedger) {
        self.0.ledgers.insert(key.to_owned(), ledger);
    }

    /// Buffer events under `key`, oldest dropped past the cap.
    fn push_events_raw(&mut self, key: &str, events: Vec<CdpEvent>) {
        let server = &mut *self.0;
        let buffer = server.events.entry(key.to_owned()).or_default();
        buffer.extend(events);
        if buffer.len() > MAX_BUFFERED_EVENTS {
            let drop = buffer.len() - MAX_BUFFERED_EVENTS;
            buffer.drain(..drop);
            *server.dropped.entry(key.to_owned()).or_default() += drop as u64;
        }
    }

    /// Events dropped for `key` by the buffer cap so far.
    fn dropped_events(&self, key: &str) -> u64 {
        self.0.dropped.get(key).copied().unwrap_or(0)
    }

    /// Drain all buffered events for `key`.
    fn take_events(&mut self, key: &str) -> Vec<CdpEvent> {
        self.0.events.remove(key).unwrap_or_default()
    }

    /// Diagnostic domains already enabled for `key` (bit0 console, bit1 network).
    fn enabled_domains(&self, key: &str) -> u8 {
        self.0.domains.get(key).copied().unwrap_or(0)
    }

    /// Mark a diagnostic domain bit as enabled for `key`.
    fn enable_domain(&mut self, key: &str, bit: u8) {
        *self.0.domains.entry(key.to_owned()).or_default() |= bit;
    }

    fn snapshot(&self, id: u64) -> Result<(&str, &Rc<InteractionManifold>, usize), ToolError> {
        self.0
            .snapshot(id)
            .map(|snapshot| (snapshot.origin.as_str(), &snapshot.manifold, snapshot.shown))
    }

    fn require_snapshot(&self) -> bool {
        self.0.require_snapshot
    }

    fn limits(&self) -> ToolLimits {
        self.0.limits
    }

    fn downloads(&self) -> Downloads {
        self.0.downloads.clone()
    }

    fn policy(&self) -> Result<&Policy, ToolError> {
        self.0
            .policy
            .as_ref()
            .map_err(|err| ToolError::InvalidArguments(err.clone()))
    }

    fn take_cdp_failure(&mut self) -> Option<CdpError> {
        self.0.cdp_failure.borrow_mut().take()
    }

    fn restore_cdp_failure(&mut self, failure: Option<CdpError>) {
        *self.0.cdp_failure.borrow_mut() = failure;
    }

    fn tool_state(&self) -> &ToolState {
        &self.0.tools
    }

    fn backend(&mut self) -> &mut dyn TargetBackend {
        &mut *self.0.backend
    }

    fn tool_state_mut(&mut self) -> &mut ToolState {
        &mut self.0.tools
    }
}

/// A session transport that notes the typed CDP failures it hits (pipe
/// closed, frame too large, target gone, timeout, dialog opened), so a call
/// whose error reached the tool as text (the agent path reports executor
/// failures as strings) still answers with the typed code.
struct Recorded {
    inner: crate::run::Deadlined<Box<dyn CdpTransport>>,
    seen: Rc<RefCell<Option<CdpError>>>,
    /// The server's run deadline (set only while `run_goal` runs).
    deadline: Rc<Cell<Option<std::time::Instant>>>,
}

impl CdpTransport for Recorded {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.inner.deadline = self.deadline.get();
        let result = self.inner.call(method, params_json);
        if let Err(err) = &result {
            if ToolError::from_cdp(err).is_some() {
                *self.seen.borrow_mut() = Some(err.clone());
            }
        }
        result
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        self.inner.drain_events()
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
