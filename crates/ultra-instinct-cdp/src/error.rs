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
        }
    }
}

impl std::error::Error for CdpError {}
