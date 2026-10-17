//! Browser policy for Ultra-Instinct.
//!
//! Instinct owns **HOW** a finite choice is made (`CandidateSet` + `Scores` + `arbitrate`).
//! This crate owns **WHAT** browser evidence means. Hard invalidity (disabled,
//! hidden, occluded, …) is filtered **before** Instinct sees candidates — never as
//! score penalties.
//!
//! Pin: `instinct-*` `0.1.0` from crates.io (hexuria/instinct#35).

#![forbid(unsafe_code)]

mod escalate;
mod evidence;
mod goal;
mod instinct_policy;
#[cfg(feature = "model-text")]
mod model_text;
#[cfg(feature = "remote")]
mod remote;
mod text;
mod types;

pub use escalate::EscalatingPolicy;
pub use evidence::{label_covers_target, label_names_target};
pub use goal::{split_sequential_clauses, AgentGoal};
pub use instinct_policy::{InstinctPolicy, HABITUATION_STEP, TARGET_AMBIGUOUS, TRUST_CAP_MILLIS};
#[cfg(feature = "model-text")]
pub use model_text::{
    parse_command_reply, vet, CommandTextModel, Grounding, ModelRefusal, ModelTextResolver,
    ScriptedTextModel, TextModel, TextModelError, TextModelReply, TextModelRequest, TextSource,
    DEFAULT_MAX_CHARS,
};
#[deprecated(note = "renamed to InstinctPolicy")]
pub type PuaPolicy = InstinctPolicy;
#[cfg(feature = "remote")]
pub use remote::{
    parse_reply, request_json, RemotePolicy, RemoteTransport, ScriptedRemote, UnconfiguredRemote,
};
pub use text::{
    ground_select, DeterministicTextResolver, TextContext, TextError, TextResolution, TextResolver,
};
pub use types::{
    BrowserPolicy, HistoryEntry, PolicyContext, PolicyDecision, PolicyError, PolicyOutcome,
    RankedAction,
};

/// Documented Instinct version requirement this crate is pinned to.
pub const INSTINCT_VERSION_REQ: &str = "0.1.0";
