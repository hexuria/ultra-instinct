use std::fmt;

/// Failure talking to a CDP endpoint or a replay script.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CdpError {
    /// The script has no further response for this method.
    NoScriptedResponse {
        method: String,
    },
    /// The script listed this method, but the JSON params differ.
    ParamsMismatch {
        method: String,
    },
    /// CDP returned an `error` object. Callers may try the next act tier.
    Protocol {
        message: String,
    },
    BadJson {
        message: String,
    },
    /// The script itself is not usable.
    BadScript {
        message: String,
    },
    Transport {
        message: String,
    },
    /// A JavaScript dialog (`alert` / `confirm` / `prompt` / `beforeunload`)
    /// opened while the call to `method` waited: the page answers nothing
    /// until the dialog is handled, so the wait stopped. The call was sent
    /// (an input counts as dispatched); its result is unknown. The
    /// `Page.javascriptDialogOpening` event stays in the transport's buffer.
    DialogOpened {
        method: String,
    },
    /// A pipe frame grew past the connection's `max_frame_bytes` before its
    /// NUL terminator. The pipe is closed for good after this: framing can
    /// no longer be trusted.
    FrameTooLarge {
        limit: usize,
    },
    /// The CDP pipe hit EOF, a broken pipe, or a read / write error. Every
    /// later call on the browser and its pages returns this again.
    PipeClosed {
        message: String,
    },
    /// The page's target detached (`Target.detachedFromTarget`) or was
    /// destroyed (`Target.targetDestroyed`); its session takes no calls.
    TargetGone {
        target_id: String,
    },
    /// No answer to the call to `method` within `after` (the transport's call
    /// deadline). A websocket or pipe stays usable: a late answer is dropped.
    Timeout {
        method: String,
        after: std::time::Duration,
    },
}

impl fmt::Display for CdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoScriptedResponse { method } => {
                write!(f, "no scripted CDP response for `{method}`")
            }
            Self::ParamsMismatch { method } => {
                write!(f, "scripted CDP params do not match the call to `{method}`")
            }
            Self::Protocol { message } => write!(f, "CDP error: {message}"),
            Self::BadJson { message } => write!(f, "invalid CDP JSON: {message}"),
            Self::BadScript { message } => write!(f, "invalid CDP script: {message}"),
            Self::Transport { message } => write!(f, "CDP transport: {message}"),
            Self::DialogOpened { method } => write!(
                f,
                "a JavaScript dialog opened while `{method}` ran; the call was sent and the page waits on the dialog"
            ),
            Self::FrameTooLarge { limit } => {
                write!(f, "CDP pipe frame exceeds {limit} bytes; pipe closed")
            }
            Self::PipeClosed { message } => write!(f, "CDP pipe closed: {message}"),
            Self::TargetGone { target_id } => {
                write!(f, "CDP target {target_id} detached or was destroyed")
            }
            Self::Timeout { method, after } => {
                let within = match after.subsec_millis() {
                    0 => format!("{} s", after.as_secs()),
                    _ => format!("{} ms", after.as_millis()),
                };
                write!(
                    f,
                    "CDP transport: no CDP result for `{method}` within {within}"
                )
            }
        }
    }
}

impl std::error::Error for CdpError {}
