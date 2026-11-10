//! The tab backend for a session-multiplexing [`CdpBrowser`]: a caller-owned
//! CDP pipe, or Chrome's browser websocket.
//!
//! [`SessionBackend`] drives one browser: tabs are `Target.*` calls on the
//! browser connection, and a tab's session is a [`CdpPage`] attached with
//! `flatten: true`. Over the pipe ([`SessionBackend::new`]) session keys are
//! `pipe:<targetId>` and every call's `cdp` argument is replaced by
//! [`PIPE_ENDPOINT`]; attached to the user's own Chrome
//! ([`SessionBackend::attached`]) they are `attach:<targetId>` and
//! [`ATTACH_ENDPOINT`]. One thread owns the browser, its pages and the
//! server (`Rc`, no locks, no reader thread).
//!
//! **Attached is own-tabs-only.** An attached backend lists only the tabs it
//! opened that are still open, and refuses to activate, close or connect to
//! any other target ([`CdpError::NotOwned`]): the user's tabs are never read,
//! focused or closed. Each tab it opens is a new Chrome window
//! (`newWindow: true`, not activated), so the user's tabs stay as they are. A websocket message over `max_frame_bytes` ends the attached
//! session (the pipe skips one oversize frame and goes on).

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{CdpBrowser, CdpError, CdpEvent, CdpPage, CdpTransport, PageTab, TargetBackend};

/// The endpoint every call drives in pipe mode (`cdp` is ignored).
pub const PIPE_ENDPOINT: &str = "pipe";

/// The endpoint every call drives when attached to the user's own Chrome.
pub const ATTACH_ENDPOINT: &str = "attach";

/// How long dropping an attached backend waits for each tab it closes.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Tabs and sessions over one [`CdpBrowser`].
pub struct SessionBackend {
    browser: CdpBrowser,
    discovering: bool,
    /// The fixed endpoint, and the session-key prefix: `<endpoint>:`.
    endpoint: &'static str,
    /// Attached: the tabs this backend opened and has not closed. Only
    /// these are closed when it drops; the user's own tabs and their Chrome
    /// are never closed (no `Browser.close`).
    created: Option<BTreeSet<String>>,
}

/// The pipe backend's name before the websocket link: the same type.
pub type PipeBackend = SessionBackend;

impl SessionBackend {
    /// The pipe backend: keys `pipe:<targetId>`; dropping it closes nothing
    /// (whoever owns the pipe owns that Chrome).
    pub fn new(browser: CdpBrowser) -> Self {
        Self {
            browser,
            discovering: false,
            endpoint: PIPE_ENDPOINT,
            created: None,
        }
    }

    /// The backend for the user's own Chrome over its browser websocket:
    /// keys `attach:<targetId>`, only the tabs it opened (each in a new
    /// window). Dropping it closes those (each waited on for up to 2 s) and
    /// nothing else.
    pub fn attached(browser: CdpBrowser) -> Self {
        Self {
            browser,
            discovering: false,
            endpoint: ATTACH_ENDPOINT,
            created: Some(BTreeSet::new()),
        }
    }

    /// `Target.setDiscoverTargets` once, so `Target.targetDestroyed` reaches
    /// the pages (a closed tab's calls end with `TargetGone`).
    fn discover(&mut self) -> Result<(), CdpError> {
        if !self.discovering {
            self.browser
                .call("Target.setDiscoverTargets", r#"{"discover":true}"#)?;
            self.discovering = true;
        }
        Ok(())
    }

    /// Attached: refuse a target this backend did not open.
    fn owned(&self, target_id: &str) -> Result<(), CdpError> {
        match &self.created {
            Some(created) if !created.contains(target_id) => Err(CdpError::NotOwned {
                target_id: target_id.to_owned(),
            }),
            _ => Ok(()),
        }
    }

    fn tab(&self, target_id: String, url: String, title: String) -> PageTab {
        PageTab {
            key: format!("{}:{target_id}", self.endpoint),
            target_id,
            url,
            title,
        }
    }
}

impl Drop for SessionBackend {
    fn drop(&mut self) {
        for target_id in self.created.take().into_iter().flatten() {
            let params = json!({"targetId": target_id}).to_string();
            let _ = self
                .browser
                .call_within("Target.closeTarget", &params, CLOSE_WAIT);
        }
    }
}

impl TargetBackend for SessionBackend {
    fn list_pages(&mut self, _endpoint: &str) -> Result<Vec<PageTab>, CdpError> {
        self.discover()?;
        Ok(self
            .browser
            .targets()?
            .into_iter()
            .filter(|t| self.owned(&t.target_id).is_ok())
            .map(|t| self.tab(t.target_id, t.url, t.title))
            .collect())
    }

    fn create_page(&mut self, _endpoint: &str, url: &str) -> Result<PageTab, CdpError> {
        self.discover()?;
        let target_id = match &mut self.created {
            // A window of its own, not activated: the user's tab stays in front.
            Some(created) => {
                let target_id = self.browser.create_window(url)?;
                created.insert(target_id.clone());
                target_id
            }
            None => {
                let target_id = self.browser.create_target(url)?;
                self.browser.activate_target(&target_id)?;
                target_id
            }
        };
        Ok(self.tab(target_id, url.to_owned(), String::new()))
    }

    fn activate_page(&mut self, _endpoint: &str, target_id: &str) -> Result<(), CdpError> {
        self.owned(target_id)?;
        self.browser.activate_target(target_id)
    }

    fn close_page(&mut self, _endpoint: &str, target_id: &str) -> Result<bool, CdpError> {
        self.owned(target_id)?;
        let closed = self.browser.close_target(target_id)?;
        // Kept while still open, so the drop closes it.
        if let (true, Some(created)) = (closed, &mut self.created) {
            created.remove(target_id);
        }
        Ok(closed)
    }

    fn connect(&mut self, key: &str) -> Result<Box<dyn CdpTransport>, CdpError> {
        self.discover()?;
        let target_id = key
            .strip_prefix(self.endpoint)
            .and_then(|rest| rest.strip_prefix(':'))
            .ok_or_else(|| CdpError::Transport {
                message: format!(
                    "no page tab is bound on the {}; open one with browser_new_tab",
                    match self.endpoint {
                        PIPE_ENDPOINT => "pipe",
                        _ => "attached browser",
                    }
                ),
            })?;
        self.owned(target_id)?;
        let mut page = self.browser.page(target_id)?;
        // Like the websocket connect: trusted key / mouse input needs
        // emulated focus or it lands on whatever Chrome has focused.
        page.call(
            "Emulation.setFocusEmulationEnabled",
            &json!({"enabled": true}).to_string(),
        )?;
        Ok(Box::new(PipeTab {
            page,
            browser: self.browser.clone(),
            frames: BTreeSet::new(),
            downloads: BTreeSet::new(),
        }))
    }

    fn endpoint(&self) -> Option<&str> {
        Some(self.endpoint)
    }
}

/// Frames and downloads one tab tracks to claim its browser-level events.
const MAX_TRACKED: usize = 256;

/// A tab's transport: its page session, plus the browser-level
/// `Browser.download*` events (Chrome sends them without a `sessionId`) of
/// downloads this tab began, so `browser_download` sees them on the tab that
/// pressed, whichever tab drains first. A `downloadWillBegin` is this tab's
/// when its `frameId` is the tab's main frame (Chrome gives a page target's
/// main frame its target id) or a frame the page's `Page.frameAttached` /
/// `Page.frameNavigated` events named; a `downloadProgress` is when its
/// `guid` began here. Any other browser-level event stays on the browser's
/// queue for its own consumer.
struct PipeTab {
    page: CdpPage,
    browser: CdpBrowser,
    /// Child frames this tab's page reported.
    frames: BTreeSet<String>,
    /// Guids of downloads begun in this tab, until they finish.
    downloads: BTreeSet<String>,
}

impl PipeTab {
    fn track_frames(&mut self, events: &[CdpEvent]) {
        for event in events {
            let params = &event.params;
            let frame = match event.method.as_str() {
                "Page.frameAttached" => params.get("frameId"),
                "Page.frameNavigated" => params.get("frame").and_then(|f| f.get("id")),
                "Page.frameDetached" => {
                    if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                        self.frames.remove(id);
                    }
                    None
                }
                _ => None,
            };
            if let Some(id) = frame.and_then(Value::as_str) {
                if self.frames.len() < MAX_TRACKED && id != self.page.target_id() {
                    self.frames.insert(id.to_owned());
                }
            }
        }
    }
}

impl CdpTransport for PipeTab {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.page.call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        let mut events = self.page.drain_events();
        self.track_frames(&events);
        let main = self.page.target_id();
        let (frames, downloads) = (&self.frames, &mut self.downloads);
        events.extend(self.browser.take_events_where(|event| {
            if !event.method.starts_with("Browser.download") {
                return false;
            }
            let params = &event.params;
            let guid = params.get("guid").and_then(Value::as_str);
            let ours = match params.get("frameId").and_then(Value::as_str) {
                Some(frame) => frame == main || frames.contains(frame),
                None => guid.is_some_and(|g| downloads.contains(g)),
            };
            if let (true, Some(guid)) = (ours, guid) {
                match params.get("state").and_then(Value::as_str) {
                    Some("completed" | "canceled") => {
                        downloads.remove(guid);
                    }
                    _ if downloads.len() < MAX_TRACKED => {
                        downloads.insert(guid.to_owned());
                    }
                    _ => {}
                }
            }
            ours
        }));
        events
    }
}
