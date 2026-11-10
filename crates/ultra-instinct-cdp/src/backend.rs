//! Where tabs and their CDP sessions come from.
//!
//! A [`TargetBackend`] lists, opens, activates and closes page tabs and
//! opens a session's transport. [`HttpBackend`] is Chrome's HTTP `/json`
//! endpoints plus a connector (a page websocket by default);
//! `SessionBackend` (unix) drives a caller-owned `--remote-debugging-pipe`
//! or the user's own Chrome over its browser websocket.

use crate::{activate_target, close_target, create_target, page_targets, CdpError, CdpTransport};

/// Opens the CDP transport for one session key.
pub type CdpConnector = Box<dyn FnMut(&str) -> Result<Box<dyn CdpTransport>, CdpError>>;

/// One page tab, as a [`TargetBackend`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageTab {
    pub target_id: String,
    pub url: String,
    pub title: String,
    /// The session key that drives this tab ([`TargetBackend::connect`]).
    pub key: String,
}

/// Where tabs and their CDP sessions come from: Chrome's HTTP `/json`
/// endpoints and page websockets ([`HttpBackend`]), or a caller-owned
/// `--remote-debugging-pipe` or a browser websocket (`SessionBackend`, unix).
///
/// `endpoint` is the `cdp` argument of the call (or the backend's fixed
/// [`TargetBackend::endpoint`]); a session key is what [`PageTab::key`] says.
pub trait TargetBackend {
    /// Page targets, in the browser's order.
    fn list_pages(&mut self, endpoint: &str) -> Result<Vec<PageTab>, CdpError>;
    /// Open a page on `url` and bring it to the front.
    fn create_page(&mut self, endpoint: &str, url: &str) -> Result<PageTab, CdpError>;
    fn activate_page(&mut self, endpoint: &str, target_id: &str) -> Result<(), CdpError>;
    /// Close a page; the browser's `success` flag.
    fn close_page(&mut self, endpoint: &str, target_id: &str) -> Result<bool, CdpError>;
    /// A transport for a session key (a page key, or an endpoint the
    /// backend resolves to its default page).
    fn connect(&mut self, key: &str) -> Result<Box<dyn CdpTransport>, CdpError>;
    /// `Some` when every call drives one browser whatever its `cdp` says:
    /// the host replaces `cdp` with this endpoint. `None` (HTTP): `cdp`
    /// names the endpoint, default [`crate::DEFAULT_CDP_HTTP`].
    fn endpoint(&self) -> Option<&str> {
        None
    }
}

/// The HTTP backend: tabs through Chrome's HTTP `/json/*` endpoints and a
/// browser websocket, sessions from a connector (a page websocket by default).
pub struct HttpBackend {
    connector: CdpConnector,
}

impl HttpBackend {
    pub fn new(
        connector: impl FnMut(&str) -> Result<Box<dyn CdpTransport>, CdpError> + 'static,
    ) -> Self {
        Self {
            connector: Box::new(connector),
        }
    }
}

impl TargetBackend for HttpBackend {
    fn list_pages(&mut self, endpoint: &str) -> Result<Vec<PageTab>, CdpError> {
        Ok(page_targets(endpoint)?
            .into_iter()
            .map(|t| PageTab {
                target_id: t.target_id,
                url: t.url,
                title: t.title,
                key: t.ws_url,
            })
            .collect())
    }

    fn create_page(&mut self, endpoint: &str, url: &str) -> Result<PageTab, CdpError> {
        let t = create_target(endpoint, url, true)?;
        Ok(PageTab {
            target_id: t.target_id,
            url: t.url,
            title: t.title,
            key: t.ws_url,
        })
    }

    fn activate_page(&mut self, endpoint: &str, target_id: &str) -> Result<(), CdpError> {
        activate_target(endpoint, target_id)
    }

    fn close_page(&mut self, endpoint: &str, target_id: &str) -> Result<bool, CdpError> {
        close_target(endpoint, target_id)
    }

    fn connect(&mut self, key: &str) -> Result<Box<dyn CdpTransport>, CdpError> {
        (self.connector)(key)
    }
}
