use std::fmt;

#[derive(Debug)]
#[non_exhaustive]
pub enum AgentError {
    Browser(String),
    Policy(String),
    /// The policy abstained (no finite choice cleared Instinct's bar). Never
    /// turned into the top-ranked candidate.
    Abstain(String),
    Guard(String),
    Ticket(String),
    /// The ticket no longer matches the world; prediction discarded.
    Stale(String),
    /// The page refused the input (disabled / readonly / no unique option).
    InputRejected(String),
    Text(String),
    InvalidState(&'static str),
    MaxSteps,
    /// The run's deadline passed (checked between steps).
    DeadlineExceeded,
    MaxPolicyCalls,
    /// Consecutive stale discards hit the bound (page never settles).
    TooManyStale(u32),
    /// The user's policy refused the press (`domain-not-allowed`,
    /// `needs-confirm`). Not stale; the run ends BLOCKED.
    PolicyRefused {
        reason: ultra_instinct_protocol::GuardReason,
        label: String,
    },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Browser(m) => write!(f, "browser: {m}"),
            Self::Policy(m) => write!(f, "policy: {m}"),
            Self::Abstain(m) => write!(f, "abstain: {m}"),
            Self::Guard(m) => write!(f, "guard: {m}"),
            Self::Ticket(m) => write!(f, "ticket: {m}"),
            Self::Stale(m) => write!(f, "stale action discarded: {m}"),
            Self::InputRejected(m) => write!(f, "page rejected input: {m}"),
            Self::Text(m) => write!(f, "text: {m}"),
            Self::InvalidState(m) => write!(f, "invalid state: {m}"),
            Self::MaxSteps => f.write_str("max steps exceeded"),
            Self::DeadlineExceeded => f.write_str("deadline exceeded"),
            Self::MaxPolicyCalls => f.write_str("max policy calls exceeded"),
            Self::TooManyStale(n) => write!(f, "{n} consecutive stale predictions"),
            Self::PolicyRefused { reason, label } => {
                write!(f, "policy refused: {reason} {label:?}")
            }
        }
    }
}

impl std::error::Error for AgentError {}
