//! `run_goal`: the agent loop for a host that only links this crate.
//!
//! The same loop `ultra-instinct run --decision instinct` drives
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
//!
//! Every TYPE_TEXT / SELECT payload the run dispatches is in
//! [`RunReport::typed`] (secrets: never log it). [`crate::Server::run_goal`]
//! also masks each one in every later tool output; with the free
//! [`run_goal`] / [`run_goal_on`] masking is the caller's.

use std::fmt;
use std::time::Instant;

use ultra_instinct_agent::{
    AgentBuilder, AgentError, AgentOutcome, BrowserRuntime, FieldValue, Input, StepRecord,
};
use ultra_instinct_browser::{
    BrowserSession, CdpError, CdpTransport, PageDelta, PageState, ScrollDirection,
};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_core::{InteractionManifold, RegionId};
use ultra_instinct_guard::Policy;
use ultra_instinct_policy::InstinctPolicy;
use ultra_instinct_policy::{PageView, Screenshot};
use ultra_instinct_protocol::GuardReason;

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
    /// needs a confirm or sits off the allow-list, when it was a policy
    /// refusal, and `kind` says which (`None`: the loop blocked itself).
    Blocked {
        label: String,
        reason: String,
        kind: Option<BlockKind>,
    },
    /// Instinct found no action it trusted.
    Abstained {
        why: String,
    },
    /// The browser failed, the step cap or the deadline was hit
    /// (`max steps exceeded`, `deadline exceeded`). `code` is the
    /// [`ToolError`](crate::ToolError) code when [`crate::Server::run_goal`]
    /// refused before the loop started (`tool_not_allowed`,
    /// `invalid_arguments` for a bad policy, a session error's code); the
    /// loop's own failures are text only (`None`).
    Failed {
        error: String,
        code: Option<&'static str>,
    },
}

/// The guard refusal that ended a run [`RunOutcome::Blocked`], from the
/// guard's own reason (never parsed from text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// The target's label matches a confirm pattern (the outcome's `label`).
    NeedsConfirm,
    /// The page is off the domain allow-list; `host` is `None` when its URL
    /// could not be read.
    DomainNotAllowed { host: Option<String> },
}

impl BlockKind {
    /// The [`ToolError`](crate::ToolError) code of the same refusal:
    /// `needs_confirm` / `domain_not_allowed` (pinned to it by
    /// `tests/run_goal.rs`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NeedsConfirm => "needs_confirm",
            Self::DomainNotAllowed { .. } => "domain_not_allowed",
        }
    }
}

/// What [`run_goal`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunReport {
    pub steps: Vec<RunStep>,
    pub outcome: RunOutcome,
    /// Every payload the run dispatched. Must not be logged.
    pub typed: TypedText,
}

/// One TYPE_TEXT / SELECT payload a run dispatched. `Debug` hides `text`.
#[derive(Clone, PartialEq, Eq)]
pub struct Typed {
    /// The text sent, as typed. A secret when `sensitive`; user data always.
    pub text: String,
    /// The field holds a secret (`ultra_instinct_browser_tools::sensitive`,
    /// failing closed when the field was not in the observation).
    pub sensitive: bool,
}

impl fmt::Debug for Typed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Typed")
            .field("text", &"<redacted>")
            .field("sensitive", &self.sensitive)
            .finish()
    }
}

/// The payloads a run dispatched, in order: user data and secrets that
/// must not be logged. `Debug` prints only how many there are.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TypedText(Vec<Typed>);

impl TypedText {
    pub fn iter(&self) -> std::slice::Iter<'_, Typed> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for TypedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypedText({} redacted)", self.0.len())
    }
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
    run_goal_typing(browser, goal, policy, limits, |_| Ok(()))
}

/// [`run_goal_on`] with `on_type` told of every payload before it is
/// dispatched; an error refuses that input (the run then fails).
pub(crate) fn run_goal_typing<B: BrowserRuntime>(
    browser: B,
    goal: &str,
    policy: Policy,
    limits: RunLimits,
    on_type: impl FnMut(&Typed) -> Result<(), String>,
) -> (RunReport, B) {
    let browser = Typing {
        inner: browser,
        typed: Vec::new(),
        on_type,
    };
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
        AgentOutcome::Blocked { reason, .. } => {
            let refusal = agent.policy_refusal();
            let kind = refusal.and_then(|(guard, _)| match guard {
                GuardReason::NeedsConfirm => Some(BlockKind::NeedsConfirm),
                // The page's host, or where a form submit would have gone.
                GuardReason::DomainNotAllowed => Some(BlockKind::DomainNotAllowed {
                    host: agent.policy_refusal_host(),
                }),
                _ => None,
            });
            RunOutcome::Blocked {
                label: refusal
                    .map(|(_, label)| label.to_owned())
                    .unwrap_or_default(),
                reason,
                kind,
            }
        }
        AgentOutcome::Abstained { reason, .. } => RunOutcome::Abstained { why: reason },
        AgentOutcome::Failed { error, .. } => RunOutcome::Failed { error, code: None },
    };
    let browser = agent.into_browser();
    let report = RunReport {
        steps,
        outcome,
        typed: TypedText(browser.typed),
    };
    (report, browser.inner)
}

/// The run's browser, noting each payload before it is dispatched (so a
/// dispatch that fails half way is still known).
struct Typing<B, F> {
    inner: B,
    typed: Vec<Typed>,
    on_type: F,
}

impl<B: BrowserRuntime, F: FnMut(&Typed) -> Result<(), String>> BrowserRuntime for Typing<B, F> {
    fn dispatch(&mut self, target: &RegionId, input: &Input) -> Result<(), AgentError> {
        if let Some(text) = input.payload() {
            let typed = Typed {
                text: text.to_owned(),
                sensitive: self
                    .inner
                    .last_observation()
                    .and_then(|manifold| manifold.get(target))
                    .is_none_or(ultra_instinct_browser_tools::sensitive),
            };
            (self.on_type)(&typed).map_err(AgentError::InputRejected)?;
            self.typed.push(typed);
        }
        self.inner.dispatch(target, input)
    }

    fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        self.inner.observe()
    }
    fn focused(&self) -> Option<RegionId> {
        self.inner.focused()
    }
    fn page(&self) -> Option<&PageState> {
        self.inner.page()
    }
    fn scroll(&mut self, direction: ScrollDirection) -> Result<(), AgentError> {
        self.inner.scroll(direction)
    }
    fn settle(&mut self) {
        self.inner.settle()
    }
    fn last_observation(&self) -> Option<&InteractionManifold> {
        self.inner.last_observation()
    }
    fn pause(&mut self, ms: u64) {
        self.inner.pause(ms)
    }
    fn settle_after_input(&mut self, target: &RegionId, input: &Input) {
        self.inner.settle_after_input(target, input)
    }
    fn read_value(&mut self, target: &RegionId) -> Option<FieldValue> {
        self.inner.read_value(target)
    }
    fn selected_text(&mut self) -> Option<String> {
        self.inner.selected_text()
    }
    fn page_delta_between(&self, before: &PageState, after: &PageState) -> PageDelta {
        self.inner.page_delta_between(before, after)
    }
    fn screenshot(&mut self) -> Option<Screenshot> {
        self.inner.screenshot()
    }
    fn page_view(&mut self) -> Option<PageView> {
        self.inner.page_view()
    }
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
