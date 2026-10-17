//! Policy trait and decision types.

use std::fmt;

use aui_core::{ActionId, ActionKind, ActionSpace};

use crate::goal::AgentGoal;

/// One ranked candidate the policy considered (for trails / escalation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedAction {
    pub id: ActionId,
    pub kind: ActionKind,
    pub label: String,
    /// Instinct confidence millis 0..=1000 (not a probability).
    pub confidence_millis: i16,
}

/// Successful finite choice from a policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyDecision {
    pub action_id: ActionId,
    pub kind: ActionKind,
    pub target_label: String,
    pub confidence_millis: i16,
    pub operation_ranked: Vec<RankedAction>,
    pub target_ranked: Vec<RankedAction>,
}

impl PolicyDecision {
    pub fn is_terminal(&self) -> bool {
        matches!(self.kind, ActionKind::Done | ActionKind::Blocked)
    }
}

/// Policy result: choose, abstain, or hard error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyOutcome {
    Choice(PolicyDecision),
    Abstain {
        reason: String,
        operation_ranked: Vec<RankedAction>,
        target_ranked: Vec<RankedAction>,
    },
}

impl PolicyOutcome {
    pub fn as_choice(&self) -> Option<&PolicyDecision> {
        match self {
            Self::Choice(d) => Some(d),
            Self::Abstain { .. } => None,
        }
    }
}

/// Errors that prevent a decision attempt (not abstention).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyError {
    EmptyGoal,
    EmptyActionSpace,
    Internal(String),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGoal => f.write_str("agent goal is empty"),
            Self::EmptyActionSpace => f.write_str("action space has no candidates"),
            Self::Internal(msg) => write!(f, "policy internal: {msg}"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// Past executed / observed steps (consumer-owned; policy may use for repetition).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub step: u32,
    pub action_id: ActionId,
    pub kind: ActionKind,
    pub label: String,
    pub verification: String,
}

/// A viewport screenshot captured for one decision: base64 image file bytes,
/// as CDP `Page.captureScreenshot` returns them.
///
/// Evidence for a policy that reads pixels (a vision-language remote model).
/// It never names a target: the reply stays an offered id. Instinct never
/// asks for one.
#[derive(Clone, PartialEq, Eq)]
pub struct Screenshot {
    base64: String,
    css_size: Option<[i32; 2]>,
}

impl Screenshot {
    pub fn from_base64(base64: impl Into<String>) -> Self {
        Self {
            base64: base64.into(),
            css_size: None,
        }
    }

    /// The viewport's size in CSS pixels, the unit of action boxes. On a
    /// high-density screen the image has more pixels than this.
    pub fn with_css_size(mut self, width: i32, height: i32) -> Self {
        self.css_size = Some([width, height]);
        self
    }

    pub fn base64(&self) -> &str {
        &self.base64
    }

    pub fn css_size(&self) -> Option<[i32; 2]> {
        self.css_size
    }
}

impl fmt::Debug for Screenshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Screenshot({} base64 bytes)", self.base64.len())
    }
}

/// The page's visible text at decision time, with its URL and title. Page
/// text is untrusted data, never instructions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageView {
    pub url: Option<String>,
    pub title: Option<String>,
    pub text: String,
}

/// What a decision may see beyond the action menu. Evidence only: it never
/// names a target, and the reply stays an offered id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionEvidence {
    pub screenshot: Option<Screenshot>,
    pub page: Option<PageView>,
}

/// Which [`DecisionEvidence`] a policy reads. The agent captures nothing
/// else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvidenceWants {
    pub screenshot: bool,
    pub page_text: bool,
}

impl EvidenceWants {
    pub fn or(self, other: Self) -> Self {
        Self {
            screenshot: self.screenshot || other.screenshot,
            page_text: self.page_text || other.page_text,
        }
    }
}

/// Policy over a finite [`ActionSpace`].
pub trait BrowserPolicy {
    fn decide(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError>;

    /// Stable name recorded in the battle diary (`instinct`, `jev`,
    /// `clef-flash`, …). Default keeps old test policies honest.
    fn name(&self) -> &'static str {
        "policy"
    }

    /// Which arm produced the most recent decision. Escalating policies
    /// override this to report the arm that answered, not the wrapper.
    fn decision_source(&self) -> &'static str {
        self.name()
    }

    /// The situation the next `decide` runs in (site, front layer, roles
    /// — the decision-time half of what the dojo journals; `near` is a
    /// post-decision artifact and is not context). Default no-op: only
    /// policies that consult situational knowledge (the dojo) need it.
    fn set_situation(&mut self, _ctx: &PolicyContext) {}

    /// Which evidence [`Self::decide_with_evidence`] reads. Default: none,
    /// so the agent captures none.
    fn wants_evidence(&self) -> EvidenceWants {
        EvidenceWants::default()
    }

    /// [`Self::decide`] with the evidence the runtime could capture.
    /// Default: ignores it.
    fn decide_with_evidence(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
        _evidence: &DecisionEvidence,
    ) -> Result<PolicyOutcome, PolicyError> {
        self.decide(space, goal, history)
    }
}

/// Where a decision happens, as far as the caller can know before the
/// policy answers. Plain owned data — no browser types.
#[derive(Clone, Debug, Default)]
pub struct PolicyContext {
    pub site_url: Option<String>,
    pub site_title: Option<String>,
    pub front_layer: bool,
    /// Roles present on the page, sorted.
    pub roles: Vec<String>,
}
