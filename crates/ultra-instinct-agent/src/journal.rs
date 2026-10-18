//! The battle journal: an owned snapshot of every decision, executed step,
//! stale discard, clause advance, and outcome of a run.
//!
//! The agent pushes events; the host drains them once via
//! [`Agent::take_journal`] and `ultra-instinct` maps them to `ultra-instinct-dojo` diary
//! lines. Plain Rust, no serde, no IO — the loop stays allocation-bounded
//! and the on-disk format stays a CLI concern.

use std::sync::Arc;

use ultra_instinct_core::ActionSpace;
use ultra_instinct_policy::PolicyOutcome;

use crate::outcome::{AgentOutcome, StepRecord};

/// One recorded moment of a run. `clause` / `mode` / `site` give the
/// context a replay or a lesson needs to reproduce the decision.
///
/// Adding a variant deliberately breaks the `ultra-instinct` diary mapper's
/// exhaustive match — a new event kind must name its diary line.
#[derive(Clone, Debug)]
pub enum JournalEvent {
    /// One per run: goal text, split clauses, deciding policy name.
    Run {
        goal: String,
        clauses: Vec<String>,
        policy: &'static str,
    },
    /// One per policy call: the offered space, the policy's ranked
    /// candidates, its verdict, and the situation it decided in.
    Decision {
        clause_index: usize,
        clause: String,
        /// `act` | `optional` | `optional-while` | `wait-for`.
        mode: &'static str,
        /// Arm that answered (`instinct`, `jev`, `clef-flash`, …).
        source: &'static str,
        site_url: Option<String>,
        site_title: Option<String>,
        /// A front-layer dialog was blocking regions at decide time.
        front_layer: bool,
        /// Sorted unique role names on the page.
        roles: Vec<&'static str>,
        /// Labels near the chosen (or top-ranked) target.
        near: Vec<String>,
        /// Shared with the [`Predicted`](crate::Predicted) the same decision produced — one
        /// allocation serves the journal and the act path.
        space: Arc<ActionSpace>,
        outcome: PolicyOutcome,
    },
    /// One per executed step: input kind, win flag, and the record
    /// (action, payload, verification) already kept for the outcome.
    Step {
        clause_index: usize,
        clause: String,
        /// `click` | `pointer` | `type` | `select` | `scroll-up` |
        /// `scroll-down` | `wait`.
        input: &'static str,
        /// See [`crate::win::step_is_win`].
        won: bool,
        record: StepRecord,
    },
    /// The policy call itself errored — journaled so the diary records
    /// the attempted decision, not just the failed run.
    PolicyError {
        clause_index: usize,
        clause: String,
        /// Arm that errored (`instinct`, `jev`, `clef-flash`, …).
        source: &'static str,
        error: String,
    },
    /// A prediction discarded because the ticket caught page drift.
    StaleDiscard { reason: String },
    /// A `then` clause boundary crossed.
    ClauseAdvanced { clause_index: usize, clause: String },
    /// One per run, last: final outcome plus budget counters.
    Finished {
        outcome: AgentOutcome,
        policy_calls: u32,
        stale_discards: u32,
        duration_ms: u64,
    },
}
