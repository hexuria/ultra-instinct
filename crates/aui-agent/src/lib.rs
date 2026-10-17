//! Ultra-Instinct agent loop: observe → ActionSpace → policy → gate → ticket →
//! executor (revalidate + consume) → input → observe → verify → history.
//!
//! The LLM (if any) does not orchestrate these steps over MCP. This crate owns
//! the loop. MCP remains an optional adapter elsewhere.

#![forbid(unsafe_code)]

mod agent;
mod error;
mod executor;
mod journal;
mod outcome;
mod runtime;
mod verify_map;
mod win;

pub use agent::{
    region_action, Agent, AgentBuilder, AgentState, Predicted, TickResult, DEFAULT_MAX_WAIT,
    DEFAULT_WAIT_POLLS, WAIT_POLL_MS,
};
pub use error::AgentError;
pub use executor::{execute_ticketed, ExecError, Executed};
pub use journal::JournalEvent;
pub use outcome::{AgentOutcome, StepRecord, VerificationKind};
pub use runtime::{BrowserRuntime, FieldValue, Input, MockBrowser, PAGE_TEXT_MAX_CHARS};
pub use verify_map::{classify_delta, classify_value};
pub use win::step_is_win;
