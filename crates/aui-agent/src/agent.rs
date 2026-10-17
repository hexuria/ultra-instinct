//! Agent state machine: Ready ↔ Predicted → Done/Blocked.
//!
//! One tick:
//!
//! ```text
//! observe → ActionSpace (front layer applied) → policy (Instinct first)
//!   → DONE / BLOCKED?            terminal, no input
//!   → TYPE_TEXT / SELECT payload  TextResolver (never Instinct)
//!   → hard gate on the decided observation → ActionTicket
//!   → executor: ledger → fresh observe → revalidate → gate → consume → input
//!   → settle → observe → diff / value check → history
//! ```
//!
//! A stale ticket discards the prediction and returns to Ready (observe and
//! decide again). It is not a failed task, but consecutive stale discards are
//! bounded.
//!
//! Multi-step goals (`"type X then click Go"`) are split on `then` / `and then`
//! ([`aui_policy::split_sequential_clauses`]). Each clause is one Instinct
//! single-intent; when Instinct chooses DONE the agent advances to the next clause
//! instead of finishing. See that function's docs for the connective limits.

use aui_browser::ScrollDirection;
use aui_core::{
    Action, ActionId, ActionKind, ActionSpace, InteractionManifold, InteractionRegion, RegionId,
};
use aui_guard::{blocker, gate, neighborhood_of, with_front_layer, TicketLedger};
use aui_policy::{
    ground_select, label_covers_target, label_names_target, split_sequential_clauses, AgentGoal,
    BrowserPolicy, DeterministicTextResolver, HistoryEntry, PolicyDecision, PolicyOutcome,
    TextContext, TextError, TextResolver, TARGET_AMBIGUOUS,
};
#[cfg(feature = "model-text")]
use aui_policy::{ModelTextResolver, TextModel};

use crate::error::AgentError;
use crate::executor::{execute_ticketed, ExecError};
use crate::journal::JournalEvent;
use crate::outcome::{AgentOutcome, StepRecord, VerificationKind};
use crate::runtime::{BrowserRuntime, Input};
use crate::verify_map::{classify_delta, classify_value};
use crate::win::step_is_win;

/// Abstains re-decided on a fresh observation after a step before the
/// abstain is final.
const MAX_ABSTAIN_RETRIES: u32 = 3;

/// Pause between re-checks while a clause waits for its target.
pub const WAIT_POLL_MS: u64 = 1_000;

/// How long a `… while M` clause waits for M to first appear.
const MARKER_GRACE_MS: u64 = 3_000;

/// Pause between re-checks of a `… while M` clause.
pub const WHILE_POLL_MS: u64 = 250;

/// Default re-checks for a waiting clause.
pub const DEFAULT_WAIT_POLLS: u32 = 30;

/// Default wall-clock cap for a waiting clause.
pub const DEFAULT_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// How one `then` clause runs.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClauseMode {
    /// Act once (the default).
    Act,
    /// `… if present` / `if shown` / `if visible`: act when the target shows
    /// up within the wait budget; otherwise the clause is skipped. With
    /// `… while M`, the clause lasts exactly as long as a region named M is
    /// on screen (an ad marker): it acts whenever the target is there, keeps
    /// going after an effect (a second ad), and ends when M is gone.
    Optional { while_marker: Option<String> },
    /// `wait for X` / `wait until X`: no action; the clause ends when a
    /// target named X is on screen, or when the wait budget runs out.
    WaitFor(String),
}

/// Split a clause into the goal text the policy sees and its mode.
fn clause_mode(clause: &str) -> (String, ClauseMode) {
    let trimmed = clause.trim();
    let lower = trimmed.to_lowercase();
    for prefix in ["wait for ", "wait until "] {
        if lower.starts_with(prefix) {
            let target = trimmed[prefix.len()..].trim();
            let target = target
                .strip_suffix(" appears")
                .or_else(|| target.strip_suffix(" shows"))
                .unwrap_or(target)
                .trim();
            if !target.is_empty() {
                return (trimmed.to_owned(), ClauseMode::WaitFor(target.to_owned()));
            }
        }
    }
    // `<act> if present while <marker>`: split the marker off first.
    let (body, while_marker) = match lower.rfind(" while ") {
        Some(at) if at > 0 => {
            let marker = trimmed[at + " while ".len()..].trim();
            if marker.is_empty() {
                (trimmed, None)
            } else {
                (trimmed[..at].trim(), Some(marker.to_owned()))
            }
        }
        _ => (trimmed, None),
    };
    let body_lower = body.to_lowercase();
    for suffix in [" if present", " if shown", " if visible", " if available"] {
        if body_lower.ends_with(suffix) {
            let goal = body[..body.len() - suffix.len()].trim();
            if !goal.is_empty() {
                return (goal.to_owned(), ClauseMode::Optional { while_marker });
            }
        }
    }
    (trimmed.to_owned(), ClauseMode::Act)
}

/// Scrolls down per clause while its target is not on screen.
const MAX_FIND_SCROLLS: u32 = 6;

/// A visible region whose label names `marker` (any role: a "Sponsored"
/// badge is not clickable).
fn marker_on_screen(manifold: &InteractionManifold, marker: &str) -> bool {
    manifold.regions().any(|region| {
        let flags = region.flags();
        !flags.hidden()
            && !flags.offscreen()
            && !region.rect().is_zero_area()
            && label_covers_target(marker, region.label())
    })
}

/// Abstain reason when the policy picked TYPE_TEXT but the clause carries
/// no text value; the agent re-decides once without TYPE_TEXT targets.
const NO_TEXT_VALUE: &str = "text: no text value found in goal/context";

/// Wait before a second look at a click that showed no change (ms).
pub const LATE_RENDER_MS: u64 = 150;

/// What a remote policy sees instead of a value typed into a secret field.
const SECRET_MASK: &str = "••••";

/// Lowercase, whitespace-collapsed label for duplicate checks.
fn fold_label(label: &str) -> String {
    label
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Offscreen regions that carry `target`'s label and offer `kind`: look-alikes
/// the policy never saw (they are not in the action space), whether the
/// viewport or a scroll put them out of view. On-screen twins are in the
/// menu, so choosing between them stays the policy's call. Regions behind the
/// front layer (a modal) are not choices either, and two links to one `href`
/// are one choice.
fn page_twins(manifold: &InteractionManifold, target: &RegionId, kind: ActionKind) -> usize {
    let Some(region) = manifold.get(target) else {
        return 0;
    };
    let label = fold_label(region.label());
    if label.is_empty() {
        return 0;
    }
    let front = with_front_layer(manifold);
    front
        .regions()
        .filter(|other| other.id() != target)
        .filter(|other| {
            let flags = other.flags();
            flags.offscreen()
                && !flags.occluded()
                && !flags.hidden()
                && !flags.disabled()
                && !other.rect().is_zero_area()
        })
        .filter(|other| {
            other
                .actions()
                .iter()
                .any(|claim| ActionKind::from_claim(*claim) == Some(kind))
        })
        .filter(|other| fold_label(other.label()) == label)
        .filter(|other| !same_destination(region, other))
        .count()
}

/// Two links to one resolved `href`: clicking either has one consequence.
/// Placeholder links (`#`, `javascript:`) run script, so they name nothing.
fn same_destination(a: &InteractionRegion, b: &InteractionRegion) -> bool {
    fn destination(region: &InteractionRegion) -> Option<&str> {
        let href = region.state().href.as_deref()?;
        let placeholder = href.ends_with('#')
            || href
                .get(..11)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("javascript:"));
        (!placeholder).then_some(href)
    }
    matches!((destination(a), destination(b)), (Some(x), Some(y)) if x == y)
}

/// Abstains about *which* target, not about the operation.
fn is_target_abstain(reason: &str) -> bool {
    reason == "target abstain" || reason.starts_with("no viable targets")
}

/// Labels of the chosen (or top-ranked) target's neighborhood: ancestors,
/// siblings, children, nearby peers — the "situation" half of a lesson key.
/// Bounded at 8 labels; the target's own label is excluded.
fn near_labels(
    manifold: &InteractionManifold,
    space: &ActionSpace,
    outcome: &PolicyOutcome,
) -> Vec<String> {
    let target_id = match outcome {
        PolicyOutcome::Choice(decision) => Some(&decision.action_id),
        PolicyOutcome::Abstain { target_ranked, .. } => {
            target_ranked.first().map(|ranked| &ranked.id)
        }
    };
    let Some(action) = target_id.and_then(|id| space.get(id)) else {
        return Vec::new();
    };
    let Some(region) = action.target() else {
        return Vec::new();
    };
    neighborhood_of(manifold, region)
        .iter()
        .filter(|id| *id != region)
        .filter_map(|id| manifold.get(id))
        .take(8)
        .map(|region| region.label().to_owned())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Ready,
    Predicted,
    Done,
    Blocked,
}

/// A policy choice waiting for ticketed execution.
///
/// There is exactly **one** staleness barrier between this prediction and page
/// input: the [`aui_protocol::ActionTicket`] issued by
/// [`gate()`](fn@aui_guard::gate) on [`Self::manifold`] and revalidated by
/// [`crate::execute_ticketed`] against a fresh observation (target-scoped world
/// fingerprint + target role / label / fingerprint). A prediction carries no
/// fingerprint of its own, so nothing here implies a second pre-ticket check
/// (ADR 0005 §R2).
#[derive(Clone, Debug)]
pub struct Predicted {
    pub decision: PolicyDecision,
    /// TYPE_TEXT text or SELECT option, from the [`TextResolver`]. Its context
    /// (goal clause, target label / role / fingerprint) is checked once at
    /// resolution time; target drift after that is caught by the ticket.
    pub payload: Option<String>,
    pub space_captured_at_ms: u64,
    /// The observation the decision was made on (ticket is issued against it).
    pub manifold: InteractionManifold,
    /// The finite space the decision was made in — shared with the journal
    /// event, so act never rebuilds it from the manifold.
    pub space: std::sync::Arc<ActionSpace>,
    pub focused: Option<RegionId>,
}

pub struct Agent<B, P, T = DeterministicTextResolver> {
    browser: B,
    policy: P,
    text: T,
    goal: AgentGoal,
    /// Full goal split on `then` / `and then`. [`Self::goal`] is the active clause.
    clauses: Vec<String>,
    clause_index: usize,
    /// The active clause already ran a step with a verified effect.
    clause_effect: bool,
    /// Abstains re-decided on a fresh observation since the last step.
    abstain_retries: u32,
    /// Scrolls taken looking for the active clause's target.
    find_scrolls: u32,
    /// The active clause has no text value: TYPE_TEXT is not offered.
    no_text_value: bool,
    /// How the active clause runs (act, optional act, wait for a target).
    mode: ClauseMode,
    /// Re-checks spent waiting in the active clause.
    wait_polls: u32,
    max_wait_polls: u32,
    max_wait: std::time::Duration,
    /// When the active clause started waiting.
    wait_started: Option<std::time::Instant>,
    /// The active `… while M` clause has seen M on screen.
    marker_seen: bool,
    /// The next decision reuses the observation just taken.
    reuse_observation: bool,
    state: AgentState,
    predicted: Option<Predicted>,
    history: Vec<StepRecord>,
    policy_history: Vec<HistoryEntry>,
    ledger: TicketLedger,
    max_steps: u32,
    max_policy_calls: u32,
    max_consecutive_no_effect: u32,
    max_consecutive_stale: u32,
    steps_taken: u32,
    policy_calls: u32,
    consecutive_no_effect: u32,
    consecutive_stale: u32,
    stale_total: u32,
    /// The battle journal: owned snapshots of every decision, step,
    /// discard, clause advance, and outcome. Drained once by the host via
    /// [`Self::take_journal`]; `aui-cli` maps it to the `aui-dojo` diary.
    journal: Vec<JournalEvent>,
}

pub struct AgentBuilder<B, P, T = DeterministicTextResolver> {
    browser: B,
    policy: P,
    text: T,
    max_steps: u32,
    max_policy_calls: u32,
    max_consecutive_no_effect: u32,
    max_consecutive_stale: u32,
    max_wait_polls: u32,
    max_wait: std::time::Duration,
}

impl<B, P> AgentBuilder<B, P, DeterministicTextResolver> {
    pub fn new(browser: B, policy: P) -> Self {
        Self {
            browser,
            policy,
            text: DeterministicTextResolver,
            max_steps: 60,
            max_policy_calls: 120,
            max_consecutive_no_effect: 3,
            max_consecutive_stale: 5,
            max_wait_polls: DEFAULT_WAIT_POLLS,
            max_wait: DEFAULT_MAX_WAIT,
        }
    }
}

impl<B, P, T> AgentBuilder<B, P, T> {
    pub fn text_resolver<T2>(self, text: T2) -> AgentBuilder<B, P, T2> {
        AgentBuilder {
            browser: self.browser,
            policy: self.policy,
            text,
            max_steps: self.max_steps,
            max_policy_calls: self.max_policy_calls,
            max_consecutive_no_effect: self.max_consecutive_no_effect,
            max_consecutive_stale: self.max_consecutive_stale,
            max_wait_polls: self.max_wait_polls,
            max_wait: self.max_wait,
        }
    }

    /// Use a model for TYPE_TEXT / SELECT payloads (feature `model-text`).
    ///
    /// The model only fills the payload of an action Instinct already chose; the
    /// reply is context-bound, shape-checked, and grounded in the goal clause
    /// before it is used, then gated / ticketed like any payload. On model
    /// failure or a refused reply it falls back to
    /// [`DeterministicTextResolver`], and abstains when that also fails. For
    /// other fallback / grounding settings build a [`ModelTextResolver`] and
    /// pass it to [`Self::text_resolver`].
    #[cfg(feature = "model-text")]
    pub fn model_text<M: TextModel>(self, model: M) -> AgentBuilder<B, P, ModelTextResolver<M>> {
        self.text_resolver(ModelTextResolver::new(model))
    }

    pub fn max_steps(mut self, n: u32) -> Self {
        self.max_steps = n;
        self
    }

    pub fn max_policy_calls(mut self, n: u32) -> Self {
        self.max_policy_calls = n;
        self
    }

    pub fn max_consecutive_no_effect(mut self, n: u32) -> Self {
        self.max_consecutive_no_effect = n.max(1);
        self
    }

    pub fn max_consecutive_stale(mut self, n: u32) -> Self {
        self.max_consecutive_stale = n.max(1);
        self
    }

    /// Re-checks (one per [`WAIT_POLL_MS`]) a `wait for X` or
    /// `… if present` clause makes before it gives up and moves on.
    pub fn max_wait_polls(mut self, n: u32) -> Self {
        self.max_wait_polls = n;
        self
    }

    /// Wall-clock cap for one waiting clause; whichever of this and
    /// [`Self::max_wait_polls`] is reached first ends the wait.
    pub fn max_wait(mut self, limit: std::time::Duration) -> Self {
        self.max_wait = limit;
        self
    }

    pub fn build(self, goal: impl Into<String>) -> Agent<B, P, T>
    where
        P: BrowserPolicy,
    {
        let raw = goal.into();
        let clauses = split_sequential_clauses(&raw);
        let active = clauses.first().cloned().unwrap_or_default();
        let (active, mode) = clause_mode(&active);
        let goal_text = raw.clone();
        let mut agent = Agent {
            browser: self.browser,
            policy: self.policy,
            text: self.text,
            goal: AgentGoal::new(active),
            clauses: clauses.clone(),
            clause_index: 0,
            clause_effect: false,
            abstain_retries: 0,
            find_scrolls: 0,
            no_text_value: false,
            mode,
            wait_polls: 0,
            max_wait_polls: self.max_wait_polls,
            max_wait: self.max_wait,
            wait_started: None,
            marker_seen: false,
            reuse_observation: false,
            state: AgentState::Ready,
            predicted: None,
            history: Vec::new(),
            policy_history: Vec::new(),
            ledger: TicketLedger::new(),
            max_steps: self.max_steps,
            max_policy_calls: self.max_policy_calls,
            max_consecutive_no_effect: self.max_consecutive_no_effect,
            max_consecutive_stale: self.max_consecutive_stale,
            steps_taken: 0,
            policy_calls: 0,
            consecutive_no_effect: 0,
            consecutive_stale: 0,
            stale_total: 0,
            journal: Vec::new(),
        };
        agent.journal.push(JournalEvent::Run {
            goal: goal_text,
            clauses,
            policy: agent.policy.name(),
        });
        agent
    }
}

/// Region capability a target-bound agent operation needs.
pub fn region_action(kind: ActionKind) -> Option<Action> {
    match kind {
        ActionKind::Click => Some(Action::Click),
        ActionKind::TypeText => Some(Action::Type),
        ActionKind::Select => Some(Action::Select),
        ActionKind::Hover => Some(Action::Hover),
        ActionKind::ScrollIn => Some(Action::Scroll),
        ActionKind::Drag => Some(Action::Drag),
        _ => None,
    }
}

impl<B, P, T> Agent<B, P, T>
where
    B: BrowserRuntime,
    P: BrowserPolicy,
    T: TextResolver,
{
    pub fn state(&self) -> AgentState {
        self.state
    }

    pub fn history(&self) -> &[StepRecord] {
        &self.history
    }

    fn typed_text_history(&self) -> Vec<String> {
        self.history
            .iter()
            .filter(|record| {
                record.kind == ActionKind::TypeText
                    && record.verification == VerificationKind::Success
            })
            .filter_map(|record| record.payload.clone())
            .collect()
    }

    /// The run's progress for a remote policy: clauses, the active one,
    /// which are satisfied, and values typed so far (secret fields masked).
    fn progress(&self, space: &ActionSpace) -> aui_policy::Progress {
        let satisfied = (0..self.clauses.len())
            .map(|i| i < self.clause_index || (i == self.clause_index && self.clause_effect))
            .collect();
        let typed = self
            .history
            .iter()
            .filter(|r| {
                r.kind == ActionKind::TypeText && r.verification == VerificationKind::Success
            })
            .filter_map(|r| {
                let value = r.payload.clone()?;
                // A field no longer on the page is masked too: its type is unknown.
                let secret = space.get(&r.action_id).is_none_or(|a| {
                    matches!(
                        a.state().input_type.as_deref(),
                        Some("password" | "hidden" | "file")
                    )
                });
                Some((
                    r.action_id.as_str().to_owned(),
                    if secret {
                        SECRET_MASK.to_owned()
                    } else {
                        value
                    },
                ))
            })
            .collect();
        aui_policy::Progress {
            clauses: self.clauses.clone(),
            current: self.clause_index,
            satisfied,
            typed,
        }
    }

    /// TYPE_TEXT actions whose field still holds the value this run
    /// successfully typed into it.
    fn already_typed(&self, space: &ActionSpace) -> Vec<ActionId> {
        self.history
            .iter()
            .filter(|record| {
                record.kind == ActionKind::TypeText
                    && record.verification == VerificationKind::Success
            })
            .filter_map(|record| {
                let offered = space.get(&record.action_id)?;
                let typed = record.payload.as_deref()?;
                (offered.state().value.as_deref() == Some(typed)).then(|| record.action_id.clone())
            })
            .collect()
    }

    pub fn goal(&self) -> &AgentGoal {
        &self.goal
    }

    /// Sequential clauses the agent will run (length 1 when the goal has no `then`).
    pub fn clauses(&self) -> &[String] {
        &self.clauses
    }

    pub fn clause_index(&self) -> usize {
        self.clause_index
    }

    /// Drain the battle journal. Call once after the run; the host maps
    /// these events to `aui-dojo` diary lines (schema v1).
    pub fn take_journal(&mut self) -> Vec<JournalEvent> {
        std::mem::take(&mut self.journal)
    }

    /// How the active clause runs, for the diary.
    fn mode_name(&self) -> &'static str {
        match &self.mode {
            ClauseMode::Act => "act",
            ClauseMode::Optional { while_marker: None } => "optional",
            ClauseMode::Optional {
                while_marker: Some(_),
            } => "optional-while",
            ClauseMode::WaitFor(_) => "wait-for",
        }
    }

    /// Advance to the next `then` clause, if any. Returns true when advanced.
    fn advance_clause(&mut self) -> bool {
        let next = self.clause_index + 1;
        if next >= self.clauses.len() {
            return false;
        }
        self.clause_index = next;
        let (goal, mode) = clause_mode(&self.clauses[next]);
        self.goal = AgentGoal::new(goal);
        self.mode = mode;
        self.journal.push(JournalEvent::ClauseAdvanced {
            clause_index: next,
            clause: self.goal.as_str().to_owned(),
        });
        self.wait_polls = 0;
        self.wait_started = None;
        self.marker_seen = false;
        self.state = AgentState::Ready;
        self.predicted = None;
        self.consecutive_no_effect = 0;
        self.consecutive_stale = 0;
        self.clause_effect = false;
        self.abstain_retries = 0;
        self.find_scrolls = 0;
        self.no_text_value = false;
        true
    }

    /// The payload resolver (e.g. to inspect a model resolver's last source).
    pub fn text_resolver(&self) -> &T {
        &self.text
    }

    pub fn browser(&self) -> &B {
        &self.browser
    }

    pub fn browser_mut(&mut self) -> &mut B {
        &mut self.browser
    }

    /// Hand the browser back (e.g. to run the next goal on the same page).
    pub fn into_browser(self) -> B {
        self.browser
    }

    /// Hand the policy back (e.g. the dojo takes its grown lesson store).
    pub fn into_policy(self) -> P {
        self.policy
    }

    pub fn policy_calls(&self) -> u32 {
        self.policy_calls
    }

    /// Stale predictions discarded so far (ticket revalidation caught a change).
    pub fn stale_discards(&self) -> u32 {
        self.stale_total
    }

    pub fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        self.browser.observe()
    }

    /// The finite action space for an observation: front layer applied, so a
    /// control behind an open dialog is never offered.
    pub fn action_space(manifold: &InteractionManifold) -> ActionSpace {
        ActionSpace::from_manifold(with_front_layer(manifold).as_ref())
    }

    /// Policy decides on a fresh observation. Any previous prediction is discarded.
    pub fn predict(&mut self) -> Result<Option<&Predicted>, AgentError> {
        if !matches!(self.state, AgentState::Ready | AgentState::Predicted) {
            return Err(AgentError::InvalidState("predict from terminal state"));
        }
        // Re-checks of a waiting optional clause are bounded by the wait
        // budget, not the policy-call budget.
        let waiting = matches!(self.mode, ClauseMode::Optional { .. }) && self.wait_polls > 0;
        if self.policy_calls >= self.max_policy_calls && !waiting {
            return Err(AgentError::MaxPolicyCalls);
        }
        self.predicted = None;
        self.state = AgentState::Ready;

        // A `… while M` re-check that just observed hands that observation
        // to this decision (one observe per re-check). The executor still
        // observes fresh before any dispatch.
        let manifold = match self
            .reuse_observation
            .then(|| self.browser.last_observation().cloned())
            .flatten()
        {
            Some(m) => {
                self.reuse_observation = false;
                m
            }
            None => {
                self.reuse_observation = false;
                self.browser.observe()?.clone()
            }
        };
        let focused = self.browser.focused();
        let mut space = Self::action_space(&manifold);
        if self.no_text_value {
            space = space.without_kind(ActionKind::TypeText);
        }
        // Never offer to re-enter a value a field already holds: typing it
        // again cannot make progress and only burns steps.
        let retype = self.already_typed(&space);
        if !retype.is_empty() {
            space = space.without_ids(&retype);
        }
        let space = std::sync::Arc::new(space);
        self.policy_calls += 1;

        let (site_url, site_title) = self
            .browser
            .page()
            .map(|page| {
                (
                    page.url().map(str::to_owned),
                    page.title().map(str::to_owned),
                )
            })
            .unwrap_or_default();
        let mut roles: Vec<&'static str> = manifold
            .regions()
            .map(|region| region.role().as_str())
            .collect();
        roles.sort_unstable();
        roles.dedup();
        let front_layer = manifold
            .regions()
            .any(|region| blocker(&manifold, region).is_some());
        // Situational context for learning policies (the dojo); a no-op
        // for every other arm.
        self.policy.set_situation(&aui_policy::PolicyContext {
            site_url: site_url.clone(),
            site_title: site_title.clone(),
            front_layer,
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
        });
        // Pixels and page text are decision evidence only: the executor
        // still observes fresh and revalidates the ticket before dispatch.
        let wants = self.policy.wants_evidence();
        let evidence = aui_policy::DecisionEvidence {
            screenshot: wants
                .screenshot
                .then(|| self.browser.screenshot())
                .flatten(),
            page: wants.page_text.then(|| self.browser.page_view()).flatten(),
            // Built from the run's own history: no page reads.
            progress: wants.page_text.then(|| self.progress(&space)),
        };
        let outcome = match self.policy.decide_with_evidence(
            &space,
            &self.goal,
            &self.policy_history,
            &evidence,
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                // Journal the attempted decision too — a policy error
                // still belongs in the diary.
                self.journal.push(JournalEvent::PolicyError {
                    clause_index: self.clause_index,
                    clause: self.goal.as_str().to_owned(),
                    source: self.policy.decision_source(),
                    error: e.to_string(),
                });
                return Err(AgentError::Policy(e.to_string()));
            }
        };
        self.journal.push(JournalEvent::Decision {
            clause_index: self.clause_index,
            clause: self.goal.as_str().to_owned(),
            mode: self.mode_name(),
            source: self.policy.decision_source(),
            site_url,
            site_title,
            front_layer,
            roles,
            near: near_labels(&manifold, &space, &outcome),
            space: space.clone(),
            outcome: outcome.clone(),
        });

        let decision = match outcome {
            PolicyOutcome::Abstain { reason, .. } => return Err(AgentError::Abstain(reason)),
            PolicyOutcome::Choice(decision) => decision,
        };
        // Off-menu guard: whatever the policy (local or remote) returned must
        // be an offered action of the matching kind.
        let offered = space.get(&decision.action_id).ok_or_else(|| {
            AgentError::Policy(format!("off-menu action `{}`", decision.action_id))
        })?;
        if offered.kind() != decision.kind {
            return Err(AgentError::Policy(format!(
                "action `{}` is {} not {}",
                decision.action_id,
                offered.kind(),
                decision.kind
            )));
        }
        // No offscreen look-alike may exist: one twin on screen (by viewport
        // size or by scrolling) is not a choice the policy could make.
        if let Some(target) = offered.target() {
            let twins = page_twins(&manifold, target, decision.kind);
            if twins > 0 {
                return Err(AgentError::Abstain(format!(
                    "{TARGET_AMBIGUOUS}: {} matches on the page",
                    twins + 1
                )));
            }
        }
        match decision.kind {
            ActionKind::Done => {
                self.state = AgentState::Done;
                return Ok(None);
            }
            ActionKind::Blocked => {
                self.state = AgentState::Blocked;
                return Ok(None);
            }
            _ => {}
        }

        let mut payload = None;
        if decision.kind == ActionKind::Select && !offered.state().options.is_empty() {
            let ctx = TextContext {
                goal: self.goal.clone(),
                field_label: offered.label().to_owned(),
                field_role: "select".to_owned(),
                typed: self.typed_text_history(),
                context_fingerprint: offered.target_fingerprint(),
                page_text: None,
                input_type: offered.state().input_type.clone(),
            };
            let resolved = match self.text.resolve(&ctx) {
                Ok(resolution) => {
                    if resolution.context_fingerprint != ctx.fingerprint() {
                        return Err(AgentError::Text(
                            "resolution belongs to a different context".into(),
                        ));
                    }
                    Some(resolution.text)
                }
                // A deliberate decline (`Missing`/`Abstain`) stays quiet —
                // `ground_select` still gets its goal match. Genuine resolver
                // failures were silent before; journal them so a diary reader
                // can tell "declined" from "broken".
                Err(err @ (TextError::Ambiguous | TextError::Invalid(_))) => {
                    self.journal.push(JournalEvent::PolicyError {
                        clause_index: self.clause_index,
                        clause: self.goal.as_str().to_owned(),
                        source: "text-resolver",
                        error: err.to_string(),
                    });
                    None
                }
                Err(_) => None,
            };
            payload = Some(
                ground_select(
                    self.goal.as_str(),
                    resolved.as_deref(),
                    &offered.state().options,
                    offered.state().selected.as_deref(),
                )
                .map_err(|e| AgentError::Abstain(format!("select: {e}")))?,
            );
        } else if matches!(decision.kind, ActionKind::TypeText | ActionKind::Select) {
            let ctx = TextContext {
                goal: self.goal.clone(),
                field_label: offered.label().to_owned(),
                field_role: if decision.kind == ActionKind::Select {
                    "select".to_owned()
                } else {
                    offered
                        .role()
                        .map(|r| r.as_str().to_owned())
                        .unwrap_or_default()
                },
                typed: self.typed_text_history(),
                context_fingerprint: offered.target_fingerprint(),
                // Read only; the executor still observes fresh before input.
                page_text: if self.text.wants_page_text() {
                    self.browser.page_view().map(|page| page.text)
                } else {
                    None
                },
                input_type: offered.state().input_type.clone(),
            };
            let resolution = self.text.resolve(&ctx).map_err(|e| match e {
                // A resolver that declines (model refused, no fallback value)
                // abstains like Instinct: nothing typed, never a guessed value.
                TextError::Abstain(reason) => AgentError::Abstain(format!("text: {reason}")),
                // No value to type (a remote policy picked a search box on
                // a click goal): abstain, and the loop re-plans without it.
                TextError::Missing if decision.kind == ActionKind::TypeText => {
                    AgentError::Abstain(NO_TEXT_VALUE.to_owned())
                }
                other => AgentError::Text(other.to_string()),
            })?;
            if resolution.context_fingerprint != ctx.fingerprint() {
                return Err(AgentError::Text(
                    "resolution belongs to a different context".into(),
                ));
            }
            // The target is revalidated after resolution by the executor
            // (fresh observe + ticket), so resolver latency cannot go stale
            // unnoticed. The ticket binds the same target role / label /
            // fingerprint the text context was built from.
            payload = Some(resolution.text);
        }

        self.predicted = Some(Predicted {
            decision,
            payload,
            space_captured_at_ms: space.captured_at_ms(),
            manifold,
            space,
            focused,
        });
        self.state = AgentState::Predicted;
        Ok(self.predicted.as_ref())
    }

    /// Gate → ticket → executor (revalidate/consume/input) → verify.
    ///
    /// A stale ticket discards the prediction and returns
    /// [`AgentError::Stale`] with the agent back in Ready.
    pub fn act(&mut self) -> Result<StepRecord, AgentError> {
        if self.state != AgentState::Predicted {
            return Err(AgentError::InvalidState("act requires Predicted"));
        }
        if self.steps_taken >= self.max_steps {
            return Err(AgentError::MaxSteps);
        }
        let predicted = self
            .predicted
            .take()
            .ok_or(AgentError::InvalidState("missing prediction"))?;
        self.state = AgentState::Ready;

        let (record, input_name) = match self.try_act_once(&predicted) {
            Ok(pair) => pair,
            Err(AgentError::Stale(msg)) => {
                self.consecutive_stale += 1;
                self.stale_total += 1;
                self.journal.push(JournalEvent::StaleDiscard {
                    reason: msg.clone(),
                });
                return Err(AgentError::Stale(msg));
            }
            Err(e) => return Err(e),
        };
        self.consecutive_stale = 0;
        self.steps_taken += 1;
        if matches!(
            record.verification,
            VerificationKind::Success
                | VerificationKind::StateChanged
                | VerificationKind::Navigation
        ) && label_covers_target(self.goal.as_str(), &record.label)
        {
            self.clause_effect = true;
        }
        self.abstain_retries = 0;
        self.history.push(record.clone());
        self.journal.push(JournalEvent::Step {
            clause_index: self.clause_index,
            clause: self.goal.as_str().to_owned(),
            input: input_name,
            won: step_is_win(self.goal.as_str(), &record),
            record: record.clone(),
        });
        self.policy_history.push(HistoryEntry {
            step: record.step,
            action_id: record.action_id.clone(),
            kind: record.kind,
            label: record.label.clone(),
            verification: record.verification.as_str().to_owned(),
        });
        // No visible change is not a failure, but repeating it is how a loop
        // looks: it counts toward the bound like a wrong effect.
        if matches!(
            record.verification,
            VerificationKind::NoVisibleChange | VerificationKind::WrongEffect
        ) {
            self.consecutive_no_effect += 1;
            if self.consecutive_no_effect >= self.max_consecutive_no_effect {
                self.state = AgentState::Blocked;
            }
        } else {
            self.consecutive_no_effect = 0;
        }
        Ok(record)
    }

    fn try_act_once(
        &mut self,
        predicted: &Predicted,
    ) -> Result<(StepRecord, &'static str), AgentError> {
        let kind = predicted.decision.kind;
        let step = self.steps_taken + 1;
        let stale_retries = self.consecutive_stale;
        let record = |verification| StepRecord {
            step,
            action_id: predicted.decision.action_id.clone(),
            kind,
            label: predicted.decision.target_label.clone(),
            payload: predicted.payload.clone(),
            verification,
            stale_retries,
        };

        // Page-level controls: no target, no ticket.
        let scroll = match kind {
            ActionKind::ScrollUp => Some(ScrollDirection::Up),
            ActionKind::ScrollDown => Some(ScrollDirection::Down),
            _ => None,
        };
        if scroll.is_some() || kind == ActionKind::Wait {
            let before_page = self.browser.page().cloned();
            let input_name = match kind {
                ActionKind::ScrollUp => "scroll-up",
                ActionKind::ScrollDown => "scroll-down",
                _ => "wait",
            };
            if let Some(direction) = scroll {
                self.browser.scroll(direction)?;
            }
            self.browser.settle();
            self.browser.observe()?;
            // `last_observation` re-borrows the cached manifold — no clone.
            let after = self
                .browser
                .last_observation()
                .ok_or_else(|| AgentError::Browser("observe produced no manifold".into()))?;
            let after_page = self.browser.page().cloned();
            let page_d = match (before_page.as_ref(), after_page.as_ref()) {
                (Some(b), Some(a)) => Some(self.browser.page_delta_between(b, a)),
                _ => None,
            };
            return Ok((
                record(classify_delta(&predicted.manifold, after, page_d.as_ref())),
                input_name,
            ));
        }

        let action = region_action(kind)
            .ok_or(AgentError::InvalidState("non-executable kind reached act"))?;
        // The space the decision was made in travels with the prediction —
        // rebuilding it per act was a whole-space clone for one lookup.
        let offered = predicted
            .space
            .get(&predicted.decision.action_id)
            .ok_or_else(|| AgentError::Policy("prediction not in its own action space".into()))?;
        let target = offered
            .target()
            .cloned()
            .ok_or_else(|| AgentError::Guard("target-bound action missing region id".into()))?;

        let dest = offered.dest().map(|d| d.id.clone());
        let cell = offered.cell();
        let ticket = match (&dest, cell) {
            (Some(dest), _) => aui_guard::gate_drag(
                &predicted.manifold,
                &target,
                dest,
                predicted.focused.clone(),
                predicted.space_captured_at_ms,
            ),
            (None, Some(cell)) => aui_guard::gate_point(
                &predicted.manifold,
                &target,
                cell,
                predicted.focused.clone(),
                predicted.space_captured_at_ms,
            ),
            (None, None) => gate(
                &predicted.manifold,
                &target,
                action,
                predicted.focused.clone(),
                predicted.space_captured_at_ms,
            ),
        }
        .map_err(|reason| AgentError::Guard(format!("refuse: {reason}")))?;

        let input = match kind {
            // A pointer click on this same label just showed no visible
            // change: retry as a script click on the observed node. `… while
            // M` clauses act on time-critical overlay controls (an ad's skip
            // button) that ignore script clicks: they stay trusted.
            ActionKind::Click
                if cell.is_none()
                    && !matches!(
                        self.mode,
                        ClauseMode::Optional {
                            while_marker: Some(_)
                        }
                    )
                    && self.history.last().is_some_and(|step| {
                        step.kind == ActionKind::Click
                            && step.label == predicted.decision.target_label
                            && step.verification == VerificationKind::NoVisibleChange
                    }) =>
            {
                Input::ScriptClick
            }
            ActionKind::Click if cell.is_some() => Input::ClickAt {
                cell: cell.expect("checked"),
            },
            ActionKind::Click => Input::Click,
            ActionKind::TypeText => Input::Type(
                predicted
                    .payload
                    .clone()
                    .ok_or_else(|| AgentError::Text("TYPE_TEXT without resolved text".into()))?,
            ),
            ActionKind::Select => Input::Select(
                predicted
                    .payload
                    .clone()
                    .ok_or_else(|| AgentError::Text("SELECT without resolved option".into()))?,
            ),
            ActionKind::Hover => Input::Hover,
            ActionKind::ScrollIn => Input::ScrollIn,
            ActionKind::Drag => Input::Drag {
                to: dest.ok_or_else(|| AgentError::Guard("DRAG without a destination".into()))?,
            },
            _ => unreachable!("region_action filtered kinds"),
        };

        let input_name = match &input {
            Input::Click => "click",
            Input::ScriptClick => "script-click",
            Input::Hover => "hover",
            Input::ScrollIn => "scroll-in",
            Input::Drag { .. } => "drag",
            Input::ClickAt { .. } => "click-at",
            Input::Type(_) => "type",
            Input::Select(_) => "select",
        };
        let executed = match execute_ticketed(&mut self.browser, &mut self.ledger, &ticket, &input)
        {
            Ok(executed) => executed,
            Err(err) if err.is_stale() => return Err(AgentError::Stale(err.to_string())),
            Err(ExecError::Rejected(msg)) => return Err(AgentError::InputRejected(msg)),
            Err(ExecError::Gate(reason)) => {
                return Err(AgentError::Guard(format!("refuse at executor: {reason}")))
            }
            Err(other) => return Err(AgentError::Ticket(other.to_string())),
        };

        self.browser.settle_after_input(&executed.target, &input);
        self.browser.observe()?;
        let after_page = self.browser.page().cloned();
        let page_d = match (executed.before_page.as_ref(), after_page.as_ref()) {
            (Some(b), Some(a)) => Some(self.browser.page_delta_between(b, a)),
            _ => None,
        };
        // `read_value` takes `&mut` — take it before borrowing `after`.
        let value = input
            .payload()
            .and_then(|_| self.browser.read_value(&executed.target));
        let after = self
            .browser
            .last_observation()
            .ok_or_else(|| AgentError::Browser("observe produced no manifold".into()))?;
        let mut verification = match input.payload() {
            Some(expected) => classify_value(expected, value.as_ref())
                .unwrap_or_else(|| classify_delta(&executed.before, after, page_d.as_ref())),
            None => classify_delta(&executed.before, after, page_d.as_ref()),
        };
        // Popups, pickers, and menus often render a beat after the page
        // settles. Look once more before calling a click "no visible change".
        if verification == VerificationKind::NoVisibleChange && input.payload().is_none() {
            self.browser.pause(LATE_RENDER_MS);
            self.browser.observe()?;
            let later_page = self.browser.page().cloned();
            let page_d = match (executed.before_page.as_ref(), later_page.as_ref()) {
                (Some(b), Some(a)) => Some(self.browser.page_delta_between(b, a)),
                _ => None,
            };
            let later = self
                .browser
                .last_observation()
                .ok_or_else(|| AgentError::Browser("observe produced no manifold".into()))?;
            verification = classify_delta(&executed.before, later, page_d.as_ref());
        }
        Ok((record(verification), input_name))
    }

    /// One predict+act cycle.
    pub fn tick(&mut self) -> Result<TickResult, AgentError> {
        match self.state {
            AgentState::Done => return Ok(TickResult::Finished(self.finish_done("done"))),
            AgentState::Blocked => {
                return Ok(TickResult::Finished(self.finish_blocked("blocked".into())))
            }
            AgentState::Ready | AgentState::Predicted => {}
        }

        // `… if present while M`: the clause runs while M is on screen.
        if self.state == AgentState::Ready {
            if let ClauseMode::Optional {
                while_marker: Some(marker),
            } = self.mode.clone()
            {
                self.browser.observe()?;
                self.reuse_observation = true;
                let manifold = self
                    .browser
                    .last_observation()
                    .ok_or_else(|| AgentError::Browser("observe produced no manifold".into()))?;
                let on_screen = marker_on_screen(manifold, &marker);
                self.marker_seen |= on_screen;
                // An ad can start a moment after the page loads: give an
                // unseen marker a short grace before deciding there is none.
                if !on_screen
                    && !self.marker_seen
                    && self.waited_ms() < MARKER_GRACE_MS
                    && self.may_wait()
                {
                    self.reuse_observation = false;
                    self.wait_polls += 1;
                    self.browser.pause(self.poll_ms());
                    return Ok(TickResult::Rethink);
                }
                if !on_screen || !self.may_wait() {
                    self.reuse_observation = false;
                    if self.advance_clause() {
                        return Ok(TickResult::ClauseAdvanced {
                            next_clause: self.goal.as_str().to_owned(),
                        });
                    }
                    self.state = AgentState::Done;
                    return Ok(TickResult::Finished(self.finish_done("marker gone")));
                }
                // Another ad may follow a skipped one: an effect does not end
                // this clause, only the marker leaving does.
                self.clause_effect = false;
            }
        }

        // One clause is one intent: once an action that names the clause's
        // target navigated, the clause is done. Deciding again on the new
        // page would let a near-twin (a related video) win.
        if self.state == AgentState::Ready
            && self.clause_effect
            && self
                .history
                .last()
                .is_some_and(|step| step.verification == VerificationKind::Navigation)
        {
            if self.advance_clause() {
                return Ok(TickResult::ClauseAdvanced {
                    next_clause: self.goal.as_str().to_owned(),
                });
            }
            self.state = AgentState::Done;
            return Ok(TickResult::Finished(self.finish_done("clause satisfied")));
        }

        if self.state == AgentState::Ready {
            if let ClauseMode::WaitFor(target) = self.mode.clone() {
                return self.tick_wait_for(&target);
            }
        }

        if self.state == AgentState::Ready {
            let satisfied = self.clause_effect
                && !matches!(
                    self.mode,
                    ClauseMode::Optional {
                        while_marker: Some(_)
                    }
                );
            let optional = matches!(self.mode, ClauseMode::Optional { .. });
            let goal_text = self.goal.as_str().to_owned();
            match self.predict() {
                Ok(None) => {
                    if self.state == AgentState::Blocked {
                        return Ok(TickResult::Finished(
                            self.finish_blocked("policy chose BLOCKED".into()),
                        ));
                    }
                    // Single-intent clause satisfied. Multi-step: advance.
                    if self.advance_clause() {
                        return Ok(TickResult::ClauseAdvanced {
                            next_clause: self.goal.as_str().to_owned(),
                        });
                    }
                    return Ok(TickResult::Finished(self.finish_done("policy chose DONE")));
                }
                // The clause already had a matching effect: a further target
                // action would be a second intent (a near-twin), so the
                // clause is done instead. `… while M` clauses keep going.
                Ok(Some(_)) if satisfied => {
                    self.predicted = None;
                    self.state = AgentState::Ready;
                    if self.advance_clause() {
                        return Ok(TickResult::ClauseAdvanced {
                            next_clause: self.goal.as_str().to_owned(),
                        });
                    }
                    self.state = AgentState::Done;
                    return Ok(TickResult::Finished(self.finish_done("clause satisfied")));
                }
                Ok(Some(predicted))
                    if optional
                        && !predicted.decision.kind.is_control()
                        && !label_names_target(&goal_text, &predicted.decision.target_label) =>
                {
                    // An optional clause acts only on the target it names;
                    // a look-alike ("Skip navigation") means "not yet".
                    self.predicted = None;
                    self.state = AgentState::Ready;
                    if self.may_wait() {
                        self.wait_polls += 1;
                        self.browser.pause(self.poll_ms());
                        return Ok(TickResult::Rethink);
                    }
                    if self.advance_clause() {
                        return Ok(TickResult::ClauseAdvanced {
                            next_clause: self.goal.as_str().to_owned(),
                        });
                    }
                    self.state = AgentState::Done;
                    return Ok(TickResult::Finished(
                        self.finish_done("optional target never appeared"),
                    ));
                }
                Ok(Some(_)) => {}
                // A single-intent clause whose action already had a verified
                // effect is satisfied even when its target left the action
                // space (a dialog took the front layer): advance, never act.
                Err(AgentError::Abstain(reason)) => {
                    if reason == NO_TEXT_VALUE && !self.no_text_value {
                        self.no_text_value = true;
                        return Ok(TickResult::Rethink);
                    }
                    if self.clause_effect {
                        if self.advance_clause() {
                            return Ok(TickResult::ClauseAdvanced {
                                next_clause: self.goal.as_str().to_owned(),
                            });
                        }
                        self.state = AgentState::Done;
                        return Ok(TickResult::Finished(
                            self.finish_done("clause satisfied; target left the action space"),
                        ));
                    }
                    // After a step the page may still be rendering (a dialog
                    // opening): settle and decide again on a fresh observe.
                    if self.steps_taken > 0 && self.abstain_retries < MAX_ABSTAIN_RETRIES {
                        self.abstain_retries += 1;
                        self.browser.settle();
                        return Ok(TickResult::Rethink);
                    }
                    // An optional clause waits for its target instead of
                    // scrolling, then is skipped when it never shows up.
                    if matches!(self.mode, ClauseMode::Optional { .. })
                        && (is_target_abstain(&reason) || reason.starts_with(TARGET_AMBIGUOUS))
                    {
                        if self.may_wait() {
                            self.wait_polls += 1;
                            self.browser.pause(self.poll_ms());
                            return Ok(TickResult::Rethink);
                        }
                        if self.advance_clause() {
                            return Ok(TickResult::ClauseAdvanced {
                                next_clause: self.goal.as_str().to_owned(),
                            });
                        }
                        self.state = AgentState::Done;
                        return Ok(TickResult::Finished(
                            self.finish_done("optional target never appeared"),
                        ));
                    }
                    // The clause names a target that is not on screen yet:
                    // scroll down and decide again (bounded). Never clicks a
                    // weaker match; an operation abstain does not scroll.
                    if is_target_abstain(&reason)
                        && !self.clause_effect
                        && self.find_scrolls < MAX_FIND_SCROLLS
                    {
                        self.find_scrolls += 1;
                        self.browser.scroll(ScrollDirection::Down)?;
                        self.browser.settle();
                        return Ok(TickResult::Rethink);
                    }
                    return Ok(TickResult::Finished(AgentOutcome::Abstained {
                        steps: self.history.clone(),
                        reason: format!("abstain: {reason}"),
                    }));
                }
                Err(e) => return Err(e),
            }
        }

        match self.act() {
            Ok(record) => {
                if self.state == AgentState::Blocked {
                    return Ok(TickResult::Finished(self.finish_blocked(format!(
                        "{} consecutive no-effect/wrong-effect actions (last: {})",
                        self.consecutive_no_effect, record.label
                    ))));
                }
                Ok(TickResult::Stepped(record))
            }
            Err(AgentError::Stale(reason)) => {
                if self.consecutive_stale >= self.max_consecutive_stale {
                    return Err(AgentError::TooManyStale(self.consecutive_stale));
                }
                Ok(TickResult::StaleDiscarded { reason })
            }
            Err(e) => Err(e),
        }
    }

    pub fn run(&mut self) -> AgentOutcome {
        let started = std::time::Instant::now();
        loop {
            if self.steps_taken >= self.max_steps {
                let outcome = AgentOutcome::Failed {
                    steps: self.history.clone(),
                    error: AgentError::MaxSteps.to_string(),
                };
                self.finish_journal(outcome.clone(), started);
                return outcome;
            }
            match self.tick() {
                Ok(TickResult::Finished(outcome)) => {
                    self.finish_journal(outcome.clone(), started);
                    return outcome;
                }
                Ok(TickResult::Stepped(_))
                | Ok(TickResult::StaleDiscarded { .. })
                | Ok(TickResult::ClauseAdvanced { .. })
                | Ok(TickResult::Rethink) => continue,
                Err(e) => {
                    let outcome = AgentOutcome::Failed {
                        steps: self.history.clone(),
                        error: e.to_string(),
                    };
                    self.finish_journal(outcome.clone(), started);
                    return outcome;
                }
            }
        }
    }

    /// The last journal line of a run: outcome kind, budgets, wall time.
    fn finish_journal(&mut self, outcome: AgentOutcome, started: std::time::Instant) {
        self.journal.push(JournalEvent::Finished {
            outcome,
            policy_calls: self.policy_calls,
            stale_discards: self.stale_total,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
    }

    /// The waiting clause still has re-checks and wall-clock time left.
    /// Pause between re-checks: fast for `… while M` (an overlay control
    /// should go as soon as it shows), [`WAIT_POLL_MS`] otherwise.
    fn poll_ms(&self) -> u64 {
        match self.mode {
            ClauseMode::Optional {
                while_marker: Some(_),
            } => WHILE_POLL_MS,
            _ => WAIT_POLL_MS,
        }
    }

    /// Pause time spent waiting in the active clause.
    fn waited_ms(&self) -> u64 {
        u64::from(self.wait_polls) * self.poll_ms()
    }

    fn may_wait(&mut self) -> bool {
        let started = *self
            .wait_started
            .get_or_insert_with(std::time::Instant::now);
        // The poll budget is in the active clause's own period — a `while`
        // clause polls at WHILE_POLL_MS, so bounding it by WAIT_POLL_MS would
        // quietly grant 4x the intended poll slots. `max_wait` is still the
        // wall-clock cap.
        self.waited_ms() < u64::from(self.max_wait_polls) * self.poll_ms()
            && started.elapsed() < self.max_wait
    }

    /// One re-check of a `wait for X` clause. Never acts on the page.
    fn tick_wait_for(&mut self, target: &str) -> Result<TickResult, AgentError> {
        self.browser.observe()?;
        let manifold = self
            .browser
            .last_observation()
            .ok_or_else(|| AgentError::Browser("observe produced no manifold".into()))?;
        let space = Self::action_space(manifold);
        let present = space
            .actions()
            .filter(|action| action.target().is_some())
            .any(|action| label_covers_target(target, action.label()));
        if !present && self.may_wait() {
            self.wait_polls += 1;
            self.browser.pause(self.poll_ms());
            return Ok(TickResult::Rethink);
        }
        if self.advance_clause() {
            return Ok(TickResult::ClauseAdvanced {
                next_clause: self.goal.as_str().to_owned(),
            });
        }
        self.state = AgentState::Done;
        let reason = if present {
            "waited target appeared"
        } else {
            "wait timed out"
        };
        Ok(TickResult::Finished(self.finish_done(reason)))
    }

    fn finish_done(&self, reason: &str) -> AgentOutcome {
        AgentOutcome::Done {
            steps: self.history.clone(),
            reason: reason.to_owned(),
        }
    }

    fn finish_blocked(&self, reason: String) -> AgentOutcome {
        AgentOutcome::Blocked {
            steps: self.history.clone(),
            reason,
        }
    }
}

#[derive(Clone, Debug)]
pub enum TickResult {
    Stepped(StepRecord),
    StaleDiscarded {
        reason: String,
    },
    /// Multi-step goal moved to the next `then` clause; keep ticking.
    ClauseAdvanced {
        next_clause: String,
    },
    /// The policy abstained soon after a step; the page settled and the
    /// next tick decides again on a fresh observation (bounded).
    Rethink,
    Finished(AgentOutcome),
}
