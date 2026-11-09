//! `run_goal`: the agent loop for a host that only links this crate.
//!
//! The same loop `ultra-instinct run --policy instinct` drives
//! (`AgentBuilder` … `run()`): Instinct chooses among the finite action
//! space, the deterministic text resolver fills payloads, every press goes
//! gate → ticket → `execute_ticketed`. No model, no feature, no async. The
//! guard [`Policy`] applies: a host off the domain allow-list, or a press
//! whose label needs a confirm, ends the run [`RunOutcome::Blocked`] with
//! nothing pressed.
//!
//! [`RunLimits::deadline`] is checked between steps; with [`run_goal`] each
//! CDP call also refuses once it has passed (one call already in flight is
//! bounded by the transport's own deadline).

use std::time::Instant;

use ultra_instinct_agent::{AgentBuilder, AgentOutcome, BrowserRuntime, StepRecord};
use ultra_instinct_browser::{BrowserSession, CdpError, CdpTransport};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_guard::Policy;
use ultra_instinct_policy::InstinctPolicy;

/// Bounds for one [`run_goal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimits {
    /// Steps (presses / inputs) at most. Default 60, the loop's own.
    pub max_steps: u32,
    /// Stop once this instant passes. Default: none.
    pub deadline: Option<Instant>,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_steps: 60,
            deadline: None,
        }
    }
}

/// One step the run took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStep {
    /// `CLICK`, `TYPE_TEXT`, `SELECT`, ….
    pub action: String,
    pub target_label: String,
    /// The verification result: `success`, `state-changed`, `navigation`,
    /// `dispatched-no-visible-change`, `wrong-effect`, ….
    pub effect: String,
}

/// How the run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Done {
        reason: String,
    },
    /// The policy refused (or the loop blocked): `label` is the target that
    /// needs a confirm or sits off the allow-list, when it was a policy refusal.
    Blocked {
        label: String,
        reason: String,
    },
    /// Instinct found no action it trusted.
    Abstained {
        why: String,
    },
    /// The browser failed, the step cap or the deadline was hit
    /// (`max steps exceeded`, `deadline exceeded`).
    Failed {
        error: String,
    },
}

/// What [`run_goal`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunReport {
    pub steps: Vec<RunStep>,
    pub outcome: RunOutcome,
}

/// Run the agent loop for `goal` on the page `transport` drives, under the
/// guard `policy`, with InstinctPolicy and the deterministic text resolver.
pub fn run_goal<T: CdpTransport>(
    transport: T,
    goal: &str,
    policy: Policy,
    limits: RunLimits,
) -> RunReport {
    let session = BrowserSession::new(Deadlined {
        inner: transport,
        deadline: limits.deadline,
    });
    run_goal_on(session, goal, policy, limits).0
}

/// [`run_goal`] on any [`BrowserRuntime`] (a live session, or the agent's
/// `MockBrowser` in tests); gives the browser back.
pub fn run_goal_on<B: BrowserRuntime>(
    browser: B,
    goal: &str,
    policy: Policy,
    limits: RunLimits,
) -> (RunReport, B) {
    let mut builder = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(limits.max_steps)
        .guard_policy(policy);
    if let Some(at) = limits.deadline {
        // A waiting clause never outlives the run.
        builder = builder
            .deadline(at)
            .max_wait(at.saturating_duration_since(Instant::now()));
    }
    let mut agent = builder.build(goal);
    let outcome = agent.run();
    let steps = outcome.steps().iter().map(step).collect();
    let outcome = match outcome {
        AgentOutcome::Done { reason, .. } => RunOutcome::Done { reason },
        AgentOutcome::Blocked { reason, .. } => RunOutcome::Blocked {
            label: agent
                .policy_refusal()
                .map(|(_, label)| label.to_owned())
                .unwrap_or_default(),
            reason,
        },
        AgentOutcome::Abstained { reason, .. } => RunOutcome::Abstained { why: reason },
        AgentOutcome::Failed { error, .. } => RunOutcome::Failed { error },
    };
    (RunReport { steps, outcome }, agent.into_browser())
}

fn step(record: &StepRecord) -> RunStep {
    RunStep {
        action: record.kind.as_str().to_owned(),
        target_label: record.label.clone(),
        effect: record.verification.as_str().to_owned(),
    }
}

/// A transport that takes no new call once the run's deadline passed.
pub(crate) struct Deadlined<T> {
    pub(crate) inner: T,
    pub(crate) deadline: Option<Instant>,
}

impl<T: CdpTransport> CdpTransport for Deadlined<T> {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        if self.deadline.is_some_and(|at| Instant::now() >= at) {
            return Err(CdpError::Transport {
                message: format!("the run's deadline passed before `{method}`"),
            });
        }
        self.inner.call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        self.inner.drain_events()
    }
}
