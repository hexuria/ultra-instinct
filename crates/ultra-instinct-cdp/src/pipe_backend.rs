//! The tab backend for a caller-owned CDP pipe (ADR 0013 phase 2).
//!
//! [`PipeBackend`] drives one [`PipeBrowser`]: tabs are `Target.*` calls on
//! the browser connection, and a tab's session is a [`PipePage`] attached
//! with `flatten: true`. Session keys are `pipe:<targetId>`; every call's
//! `cdp` argument is replaced by [`PIPE_ENDPOINT`]. One thread owns the
//! browser, its pages and the server (rule 10: `Rc`, no locks, no reader
//! thread).

use serde_json::json;

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
        }))
    }

    fn endpoint(&self) -> Option<&str> {
        Some(PIPE_ENDPOINT)
    }
}

/// A tab's transport: its page session, plus browser-level download events
/// (Chrome may send `Browser.download*` without a `sessionId`), so
/// `browser_download` sees them on the tab that pressed.
struct PipeTab {
    page: PipePage,
    browser: PipeBrowser,
}

impl CdpTransport for PipeTab {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.page.call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        let mut events = self.page.drain_events();
        events.extend(
            self.browser
                .drain_events()
                .into_iter()
                .filter(|event| event.method.starts_with("Browser.download")),
        );
        events
    }
}
