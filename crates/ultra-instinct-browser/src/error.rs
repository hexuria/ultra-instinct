use std::fmt;
use ultra_instinct_cdp::CdpError;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrowserError {
    Cdp(CdpError),
    NotObserved,
    UnknownRegion(String),
    UnsupportedAction(String),
    MissingObjectId,
    BadViewport(String),
    DuplicateRegion(String),
    Verify(crate::verify::VerifyError),
    /// The page refused an input (disabled, readonly, no unique option, …).
    /// Nothing was changed.
    InputRejected(String),
    /// Neither the node id nor the backend id of the observed region resolved.
    TargetUnresolved,
    /// `Page.navigate` reported an error.
    Navigation(String),
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cdp(err) => write!(f, "{err}"),
            Self::NotObserved => f.write_str("browser session has no observation yet"),
            Self::UnknownRegion(id) => write!(f, "unknown region `{id}`"),
            Self::UnsupportedAction(action) => {
                write!(f, "browser session cannot perform `{action}`")
            }
            Self::MissingObjectId => f.write_str("DOM.resolveNode returned no objectId"),
            Self::BadViewport(message) => write!(f, "viewport: {message}"),
            Self::DuplicateRegion(id) => write!(f, "duplicate fused region `{id}`"),
            Self::Verify(err) => write!(f, "{err}"),
            Self::InputRejected(message) => write!(f, "page rejected input: {message}"),
            Self::TargetUnresolved => f.write_str("observed node no longer resolves"),
            Self::Navigation(message) => write!(f, "navigation failed: {message}"),
        }
    }
}

impl std::error::Error for BrowserError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cdp(err) => Some(err),
            Self::Verify(err) => Some(err),
            _ => None,
        }
    }
}

impl From<CdpError> for BrowserError {
    fn from(value: CdpError) -> Self {
        Self::Cdp(value)
    }
}

/// How a press was sent. Declaration order is the preference order.
/// There is no focus mechanism: a focus is not a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActMechanism {
    /// `Runtime.callFunctionOn` of `function(){this.click()}`.
    DomSemantic,
    /// `Input.dispatchMouseEvent` at the region center.
    Coordinate,
}

impl ActMechanism {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DomSemantic => "dom-semantic",
            Self::Coordinate => "coordinate",
        }
    }
}

impl fmt::Display for ActMechanism {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
