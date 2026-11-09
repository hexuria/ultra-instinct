//! The tab backend for a caller-owned CDP pipe.
//!
//! [`PipeBackend`] drives one [`PipeBrowser`]: tabs are `Target.*` calls on
//! the browser connection, and a tab's session is a [`PipePage`] attached
//! with `flatten: true`. Session keys are `pipe:<targetId>`; every call's
//! `cdp` argument is replaced by [`PIPE_ENDPOINT`]. One thread owns the
//! browser, its pages and the server (`Rc`, no locks, no reader thread).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::{CdpError, CdpEvent, CdpTransport, PageTab, PipeBrowser, PipePage, TargetBackend};

/// The endpoint every call drives in pipe mode (`cdp` is ignored).
pub const PIPE_ENDPOINT: &str = "pipe";

/// Session-key prefix of a pipe tab: `pipe:<targetId>`.
const KEY_PREFIX: &str = "pipe:";

/// Tabs and sessions over one [`PipeBrowser`].
pub struct PipeBackend {
    browser: PipeBrowser,
    discovering: bool,
}

impl PipeBackend {
    pub fn new(browser: PipeBrowser) -> Self {
        Self {
            browser,
            discovering: false,
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

    fn tab(target_id: String, url: String, title: String) -> PageTab {
        PageTab {
            key: format!("{KEY_PREFIX}{target_id}"),
            target_id,
            url,
            title,
        }
    }
}

impl TargetBackend for PipeBackend {
    fn list_pages(&mut self, _endpoint: &str) -> Result<Vec<PageTab>, CdpError> {
        self.discover()?;
        Ok(self
            .browser
            .targets()?
            .into_iter()
            .map(|t| Self::tab(t.target_id, t.url, t.title))
            .collect())
    }

    fn create_page(&mut self, _endpoint: &str, url: &str) -> Result<PageTab, CdpError> {
        self.discover()?;
        let target_id = self.browser.create_target(url)?;
        self.browser.activate_target(&target_id)?;
        Ok(Self::tab(target_id, url.to_owned(), String::new()))
    }

    fn activate_page(&mut self, _endpoint: &str, target_id: &str) -> Result<(), CdpError> {
        self.browser.activate_target(target_id)
    }

    fn close_page(&mut self, _endpoint: &str, target_id: &str) -> Result<bool, CdpError> {
        self.browser.close_target(target_id)
    }

    fn connect(&mut self, key: &str) -> Result<Box<dyn CdpTransport>, CdpError> {
        self.discover()?;
        let target_id = key
            .strip_prefix(KEY_PREFIX)
            .ok_or_else(|| CdpError::Transport {
                message: "no page tab is bound on the pipe; open one with browser_new_tab".into(),
            })?;
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
        Some(PIPE_ENDPOINT)
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
    page: PipePage,
    browser: PipeBrowser,
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
