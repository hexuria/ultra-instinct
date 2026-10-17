//! Finite action space derived from an [`InteractionManifold`].
//!
//! Phase 1 of the agent-runtime pivot: build the set of operations and targets
//! a policy may choose from. No ranking, no Instinct, no executor.
//!
//! Architectural invariants (from `browser-use/jev-ultrafast`, MIT):
//! - Every observation yields a dynamic finite action space.
//! - Policies may only choose offered operations / target ids.
//! - Target-bound actions refer to observed [`RegionId`]s, never CSS/JS.
//! - Terminal controls (`WAIT`, `DONE`, `BLOCKED`) and page scrolls have no target.
//!
//! Per-region [`crate::Action`] claims (click/type/select/…) are mapped into
//! agent-level [`ActionKind`]s. Hard-invalid regions (disabled, hidden,
//! occluded, offscreen, zero-area) are excluded from the offered target set;
//! they remain in the manifold for evidence / guard later.

use std::collections::BTreeMap;
use std::fmt;

use crate::element_state::ElementState;
use crate::error::CoreError;
use crate::id::RegionId;
use crate::manifold::InteractionManifold;
use crate::rect::Rect;
use crate::region::InteractionRegion;
use crate::vocab::{Action, Role};

/// Agent-level operation. Distinct from per-region [`Action`] capability claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ActionKind {
    Click,
    TypeText,
    Select,
    ScrollUp,
    ScrollDown,
    Wait,
    Done,
    Blocked,
}

impl ActionKind {
    /// The agent operation a region claim offers (`None` for focus/hover/scroll).
    pub const fn from_claim(claim: Action) -> Option<Self> {
        map_region_claim(claim)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Click => "CLICK",
            Self::TypeText => "TYPE_TEXT",
            Self::Select => "SELECT",
            Self::ScrollUp => "SCROLL_UP",
            Self::ScrollDown => "SCROLL_DOWN",
            Self::Wait => "WAIT",
            Self::Done => "DONE",
            Self::Blocked => "BLOCKED",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "CLICK" | "click" => Self::Click,
            "TYPE_TEXT" | "type_text" | "type" => Self::TypeText,
            "SELECT" | "select" => Self::Select,
            "SCROLL_UP" | "scroll_up" => Self::ScrollUp,
            "SCROLL_DOWN" | "scroll_down" => Self::ScrollDown,
            "WAIT" | "wait" => Self::Wait,
            "DONE" | "done" => Self::Done,
            "BLOCKED" | "blocked" => Self::Blocked,
            _ => return None,
        })
    }

    /// True when this kind never binds a manifold region.
    pub const fn is_control(self) -> bool {
        matches!(
            self,
            Self::ScrollUp | Self::ScrollDown | Self::Wait | Self::Done | Self::Blocked
        )
    }
}

impl fmt::Display for ActionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable id of one offered action inside an [`ActionSpace`].
///
/// Target-bound forms: `{KIND}:{region_id}` (e.g. `CLICK:nav-settings`).
/// Controls use the kind name alone (`WAIT`, `DONE`, …).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionId(String);

impl ActionId {
    pub fn try_new(raw: impl AsRef<str>) -> Result<Self, CoreError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(CoreError::EmptyId);
        }
        if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CoreError::InvalidId);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn for_target(kind: ActionKind, region: &RegionId) -> Self {
        Self(format!("{}:{}", kind.as_str(), region.as_str()))
    }

    fn for_control(kind: ActionKind) -> Self {
        Self(kind.as_str().to_owned())
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ActionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// One offered action bound to an observed region, or a terminal/page control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedAction {
    id: ActionId,
    kind: ActionKind,
    /// Present for target-bound kinds; absent for controls.
    target: Option<RegionId>,
    label: String,
    role: Option<Role>,
    /// Region fingerprint bits at observation time (0 for controls).
    target_fingerprint: u64,
    state: ElementState,
    /// Target box `[x, y, width, height]` in whole viewport pixels. Request
    /// evidence for a policy that reads the screenshot; never executed.
    bounds: Option<[i32; 4]>,
    /// For an unnamed target: the observed text that sits by it. Evidence
    /// only; [`Self::label`] stays the region label.
    near_text: Option<String>,
}

impl ObservedAction {
    /// Build one offered action directly. `from_manifold` uses this for every
    /// action it emits; the replay arena uses it to rebuild a recorded menu —
    /// the caller owns which actions belong on the menu.
    pub fn new(
        id: ActionId,
        kind: ActionKind,
        target: Option<RegionId>,
        label: impl Into<String>,
        role: Option<Role>,
        target_fingerprint: u64,
        state: ElementState,
    ) -> Self {
        Self {
            id,
            kind,
            target,
            label: label.into(),
            role,
            target_fingerprint,
            state,
            bounds: None,
            near_text: None,
        }
    }

    pub fn id(&self) -> &ActionId {
        &self.id
    }
    pub fn kind(&self) -> ActionKind {
        self.kind
    }
    pub fn target(&self) -> Option<&RegionId> {
        self.target.as_ref()
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn role(&self) -> Option<Role> {
        self.role
    }
    pub fn target_fingerprint(&self) -> u64 {
        self.target_fingerprint
    }
    pub fn state(&self) -> &ElementState {
        &self.state
    }
    pub fn bounds(&self) -> Option<[i32; 4]> {
        self.bounds
    }
    pub fn near_text(&self) -> Option<&str> {
        self.near_text.as_deref()
    }
}

/// Finite set of operations/targets a policy may choose from for one observation.
///
/// Stored in deterministic `ActionId` order. Construction never invents
/// selectors — only manifold region ids and fixed controls. Target boxes are
/// observed geometry carried as request evidence; nothing a policy returns
/// can carry a coordinate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionSpace {
    captured_at_ms: u64,
    actions: BTreeMap<ActionId, ObservedAction>,
}

impl ActionSpace {
    /// Build an action space from the current manifold.
    ///
    /// Hard-invalid regions are skipped for target-bound actions. Page-level
    /// controls (`SCROLL_*`, `WAIT`, `DONE`, `BLOCKED`) are always offered.
    pub fn from_manifold(manifold: &InteractionManifold) -> Self {
        let mut actions = BTreeMap::new();

        for region in manifold.regions() {
            if !region_is_viable_target(region) {
                continue;
            }
            for claim in region.actions() {
                let Some(kind) = map_region_claim(*claim) else {
                    continue;
                };
                // Readonly controls stay clickable (focus/select text) but are
                // not offered as TYPE_TEXT / SELECT targets.
                if region.flags().readonly()
                    && matches!(kind, ActionKind::TypeText | ActionKind::Select)
                {
                    continue;
                }
                let mut observed = target_action(kind, region);
                if observed.label.trim().is_empty() {
                    observed.near_text = near_text(manifold, region);
                }
                actions.insert(observed.id.clone(), observed);
            }
        }

        for kind in [
            ActionKind::ScrollUp,
            ActionKind::ScrollDown,
            ActionKind::Wait,
            ActionKind::Done,
            ActionKind::Blocked,
        ] {
            let observed = control_action(kind);
            actions.insert(observed.id.clone(), observed);
        }

        Self {
            captured_at_ms: manifold.captured_at_ms(),
            actions,
        }
    }

    /// Build a space from already-offered actions — e.g. the replay arena
    /// rebuilding a recorded menu. Viability filtering happened upstream;
    /// this only orders deterministically. `captured_at_ms` is 0: a recorded
    /// menu has no observation timestamp.
    pub fn from_actions(actions: Vec<ObservedAction>) -> Self {
        Self {
            captured_at_ms: 0,
            actions: actions
                .into_iter()
                .map(|action| (action.id.clone(), action))
                .collect(),
        }
    }

    pub fn captured_at_ms(&self) -> u64 {
        self.captured_at_ms
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn get(&self, id: &ActionId) -> Option<&ObservedAction> {
        self.actions.get(id)
    }

    pub fn get_str(&self, id: &str) -> Option<&ObservedAction> {
        let id = ActionId::try_new(id).ok()?;
        self.actions.get(&id)
    }

    pub fn actions(&self) -> impl Iterator<Item = &ObservedAction> {
        self.actions.values()
    }

    pub fn ids(&self) -> impl Iterator<Item = &ActionId> {
        self.actions.keys()
    }

    /// Target-bound actions of one kind, in id order.
    pub fn targets_of(&self, kind: ActionKind) -> impl Iterator<Item = &ObservedAction> {
        self.actions
            .values()
            .filter(move |a| a.kind == kind && a.target.is_some())
    }

    pub fn contains_kind(&self, kind: ActionKind) -> bool {
        self.actions.values().any(|a| a.kind == kind)
    }

    /// This space minus every action of `kind` (same capture time).
    pub fn without_kind(&self, kind: ActionKind) -> Self {
        Self {
            captured_at_ms: self.captured_at_ms,
            actions: self
                .actions
                .iter()
                .filter(|(_, a)| a.kind != kind)
                .map(|(id, a)| (id.clone(), a.clone()))
                .collect(),
        }
    }
}

fn region_is_viable_target(region: &InteractionRegion) -> bool {
    let flags = region.flags();
    if flags.disabled() || flags.hidden() || flags.occluded() || flags.offscreen() {
        return false;
    }
    if region.rect().is_zero_area() {
        return false;
    }
    true
}

const fn map_region_claim(claim: Action) -> Option<ActionKind> {
    match claim {
        Action::Click | Action::Toggle => Some(ActionKind::Click),
        Action::Type => Some(ActionKind::TypeText),
        Action::Select => Some(ActionKind::Select),
        // Focus/Hover/Scroll on a region are not agent operations; page scroll
        // is offered as SCROLL_UP / SCROLL_DOWN controls.
        Action::Focus | Action::Hover | Action::Scroll => None,
    }
}

fn target_action(kind: ActionKind, region: &InteractionRegion) -> ObservedAction {
    ObservedAction {
        id: ActionId::for_target(kind, region.id()),
        kind,
        target: Some(region.id().clone()),
        label: region.label().to_owned(),
        role: Some(region.role()),
        target_fingerprint: region.fingerprint().bits(),
        state: region.state().clone(),
        bounds: Some(pixel_box(region.rect())),
        near_text: None,
    }
}

fn pixel_box(rect: Rect) -> [i32; 4] {
    // Saturating float-to-int casts: an absurd coordinate cannot wrap.
    [rect.x(), rect.y(), rect.width(), rect.height()].map(|v| v.round() as i32)
}

/// Widest gap, in viewport pixels, between an unnamed target and text to its
/// left on the same row.
const NEAR_TEXT_ROW_GAP_PX: f64 = 160.0;
/// Widest gap between an unnamed target and text directly above it.
const NEAR_TEXT_ABOVE_GAP_PX: f64 = 40.0;
/// Longest near text carried, in characters.
const NEAR_TEXT_MAX_CHARS: usize = 80;

/// Observed text that names an unnamed target the way a sighted reader would
/// read it: the parent's own text ("Date: [ ]"), else the nearest text to the
/// left on the same row, else the nearest text directly above. Geometry only;
/// `None` when no text is that close.
fn near_text(manifold: &InteractionManifold, target: &InteractionRegion) -> Option<String> {
    let named = |region: &InteractionRegion| {
        region.id() != target.id() && !region.label().trim().is_empty()
    };
    let parent = target
        .parent()
        .and_then(|id| manifold.get(id))
        .filter(|parent| named(parent));
    let t = target.rect();
    let left = manifold
        .regions()
        .filter(|r| named(r))
        .filter_map(|r| {
            let rect = r.rect();
            let same_row = rect.y() < t.bottom() && t.y() < rect.bottom();
            let gap = t.x() - rect.right();
            (same_row && (0.0..=NEAR_TEXT_ROW_GAP_PX).contains(&gap)).then_some((gap, r))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let above = || {
        manifold
            .regions()
            .filter(|r| named(r))
            .filter_map(|r| {
                let rect = r.rect();
                let overlaps = rect.x() < t.right() && t.x() < rect.right();
                let gap = t.y() - rect.bottom();
                (overlaps && (0.0..=NEAR_TEXT_ABOVE_GAP_PX).contains(&gap)).then_some((gap, r))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
    };
    let region = parent.or_else(|| left.map(|(_, r)| r).or_else(|| above().map(|(_, r)| r)))?;
    let text: String = region
        .label()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(NEAR_TEXT_MAX_CHARS)
        .collect();
    Some(text)
}

fn control_action(kind: ActionKind) -> ObservedAction {
    let label = match kind {
        ActionKind::ScrollUp => "Scroll the page up".to_owned(),
        ActionKind::ScrollDown => "Scroll the page down".to_owned(),
        ActionKind::Wait => "Wait briefly for the page to settle".to_owned(),
        ActionKind::Done => "Every requirement is visibly satisfied".to_owned(),
        ActionKind::Blocked => "No supported operation can progress".to_owned(),
        _ => kind.as_str().to_owned(),
    };
    ObservedAction {
        id: ActionId::for_control(kind),
        kind,
        target: None,
        label,
        role: None,
        target_fingerprint: 0,
        state: ElementState::default(),
        bounds: None,
        near_text: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_fixture;
    use crate::{
        InteractionManifold, Rect, RegionFlags, RegionParts, Role, SourceMask, UnitInterval,
    };

    #[test]
    fn without_kind_drops_only_that_kind() {
        let space = ActionSpace::from_manifold(
            &parse_fixture(
                r#"
                viewport w=800 h=600
                region id=q role=text_field label="Search" x=10 y=10 w=200 h=24 actions=click,type sources=dom,accessibility
                "#,
            )
            .unwrap(),
        );
        let trimmed = space.without_kind(ActionKind::TypeText);
        assert!(trimmed.targets_of(ActionKind::TypeText).next().is_none());
        assert_eq!(trimmed.targets_of(ActionKind::Click).count(), 1);
        assert_eq!(trimmed.len(), space.len() - 1);
        assert_eq!(trimmed.captured_at_ms(), space.captured_at_ms());
    }

    #[test]
    fn action_kind_round_trips() {
        for kind in [
            ActionKind::Click,
            ActionKind::TypeText,
            ActionKind::Select,
            ActionKind::ScrollUp,
            ActionKind::ScrollDown,
            ActionKind::Wait,
            ActionKind::Done,
            ActionKind::Blocked,
        ] {
            assert_eq!(ActionKind::parse(kind.as_str()), Some(kind));
        }
        assert!(!ActionKind::Click.is_control());
        assert!(ActionKind::Wait.is_control());
    }

    #[test]
    fn sidebar_offers_stable_click_ids_and_excludes_invalid() {
        let m = parse_fixture(include_str!("../../../fixtures/sidebar.manifold")).unwrap();
        let space = ActionSpace::from_manifold(&m);

        // Stable click ids for viable nav buttons.
        for id in [
            "CLICK:nav-help",
            "CLICK:nav-settings",
            "CLICK:nav-profile",
            "CLICK:main-settings",
        ] {
            let action = space.get_str(id).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(action.kind(), ActionKind::Click);
            assert!(action.target().is_some());
        }

        // Hard-invalid / non-viable must not appear.
        for id in [
            "CLICK:nav-disabled",
            "CLICK:nav-hidden",
            "CLICK:offscreen-undo",
            "CLICK:zero-ghost",
        ] {
            assert!(space.get_str(id).is_none(), "should exclude {id}");
        }

        // Controls always present.
        for id in ["SCROLL_UP", "SCROLL_DOWN", "WAIT", "DONE", "BLOCKED"] {
            assert!(space.get_str(id).is_some(), "missing control {id}");
            assert!(space.get_str(id).unwrap().target().is_none());
        }

        // Deterministic id order.
        let ids: Vec<_> = space.ids().map(ActionId::as_str).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn unnamed_targets_carry_near_text_and_every_target_carries_its_box() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=row role=generic label="Date:" x=2 y=52 w=156 h=21 actions= sources=dom
            region id=date role=text_field label="" x=32.6 y=55 w=120 h=15 actions=type parent=row sources=dom
            region id=email-text role=text label="Email" x=10 y=100 w=50 h=20 actions= sources=dom
            region id=email role=text_field label="" x=80 y=100 w=200 h=20 actions=type sources=dom
            region id=far-text role=text label="Far away" x=10 y=200 w=50 h=20 actions= sources=dom
            region id=far role=text_field label="" x=400 y=200 w=100 h=20 actions=type sources=dom
            region id=heading role=heading label="  Shipping   address  " x=10 y=300 w=300 h=20 actions= sources=dom
            region id=street role=text_field label="" x=10 y=340 w=300 h=20 actions=type sources=dom
            region id=go role=button label="Go" x=320 y=100 w=40 h=20 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let near = |id: &str| space.get_str(id).unwrap().near_text().map(str::to_owned);
        assert_eq!(
            near("TYPE_TEXT:date").as_deref(),
            Some("Date:"),
            "parent text"
        );
        assert_eq!(
            near("TYPE_TEXT:email").as_deref(),
            Some("Email"),
            "same row, left"
        );
        assert_eq!(near("TYPE_TEXT:far"), None, "past the row gap");
        assert_eq!(
            near("TYPE_TEXT:street").as_deref(),
            Some("Shipping address"),
            "directly above, whitespace folded"
        );
        assert_eq!(
            near("CLICK:go"),
            None,
            "named targets keep their label only"
        );
        assert_eq!(space.get_str("TYPE_TEXT:date").unwrap().label(), "");
        assert_eq!(
            space.get_str("TYPE_TEXT:date").unwrap().bounds(),
            Some([33, 55, 120, 15])
        );
        assert_eq!(space.get_str("DONE").unwrap().bounds(), None);
    }

    #[test]
    fn near_text_row_and_above_gaps_are_inclusive() {
        let at = |gap_x: f64, gap_y: f64| {
            let m = parse_fixture(&format!(
                "viewport w=800 h=600\n\
                 region id=t role=text label=\"Name\" x=0 y=0 w=100 h=20 actions= sources=dom\n\
                 region id=f role=text_field label=\"\" x={} y={} w=50 h=20 actions=type sources=dom\n",
                100.0 + gap_x,
                20.0 + gap_y,
            ))
            .unwrap();
            ActionSpace::from_manifold(&m)
                .get_str("TYPE_TEXT:f")
                .unwrap()
                .near_text()
                .map(str::to_owned)
        };
        // Same row (y overlap) at the row limit and one pixel past it.
        assert_eq!(at(NEAR_TEXT_ROW_GAP_PX, -20.0).as_deref(), Some("Name"));
        assert_eq!(at(NEAR_TEXT_ROW_GAP_PX + 1.0, -20.0), None);
        // Directly above (x overlap) at the above limit and one pixel past it.
        assert_eq!(at(-100.0, NEAR_TEXT_ABOVE_GAP_PX).as_deref(), Some("Name"));
        assert_eq!(at(-100.0, NEAR_TEXT_ABOVE_GAP_PX + 1.0), None);
    }

    #[test]
    fn twin_suspend_rows_expose_both_click_targets() {
        let m =
            parse_fixture(include_str!("../../../fixtures/twin-suspend-rows.manifold")).unwrap();
        let space = ActionSpace::from_manifold(&m);

        let alpha = space.get_str("CLICK:alpha-suspend").unwrap();
        let beta = space.get_str("CLICK:beta-suspend").unwrap();
        assert_eq!(alpha.label(), "Suspend");
        assert_eq!(beta.label(), "Suspend");
        assert_ne!(alpha.target(), beta.target());
        assert_ne!(alpha.target_fingerprint(), beta.target_fingerprint());

        // Editable fields become TYPE_TEXT (and Click if claimed).
        assert!(space.get_str("TYPE_TEXT:alpha-host").is_some());
        assert!(space.get_str("TYPE_TEXT:beta-host").is_some());
        assert!(space.get_str("CLICK:alpha-host").is_some());
    }

    #[test]
    fn modal_confirm_still_lists_buried_page_buttons() {
        // Phase 1: ActionSpace lists what the manifold claims. Front-layer /
        // occlusion refusal is a guard/hard-gate concern (Phase 3), not
        // ActionSpace filtering — except explicit occluded flags.
        let m = parse_fixture(include_str!("../../../fixtures/modal-confirm.manifold")).unwrap();
        let space = ActionSpace::from_manifold(&m);

        assert!(space.get_str("CLICK:page-delete").is_some());
        assert!(space.get_str("CLICK:confirm-delete").is_some());
        assert!(space.get_str("CLICK:confirm-cancel").is_some());
        assert!(space.get_str("CLICK:page-cancel").is_some());
    }

    #[test]
    fn send_buttons_all_three_click_targets_stable() {
        let m = parse_fixture(include_str!("../../../fixtures/send-buttons.manifold")).unwrap();
        let space = ActionSpace::from_manifold(&m);
        for id in ["CLICK:a-feedback", "CLICK:b-device", "CLICK:z-send"] {
            let a = space.get_str(id).unwrap();
            assert_eq!(a.kind(), ActionKind::Click);
        }
        let clicks: Vec<_> = space
            .targets_of(ActionKind::Click)
            .map(|a| a.id().as_str())
            .collect();
        assert_eq!(
            clicks,
            ["CLICK:a-feedback", "CLICK:b-device", "CLICK:z-send"]
        );
    }

    #[test]
    fn empty_manifold_still_offers_controls() {
        let m = parse_fixture("viewport w=100 h=100\n").unwrap();
        let space = ActionSpace::from_manifold(&m);
        assert_eq!(space.len(), 5);
        assert!(space.contains_kind(ActionKind::Done));
        assert!(!space.contains_kind(ActionKind::Click));
    }

    #[test]
    fn target_actions_copy_state_and_controls_have_none() {
        let state = ElementState {
            value: Some("Ana".to_owned()),
            selected: Some("UTC".to_owned()),
            options: vec!["UTC".to_owned()],
            ..ElementState::default()
        };
        let region = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("field").unwrap(),
            role: Role::ComboBox,
            label: "Name".to_owned(),
            rect: Rect::try_new(10.0, 10.0, 80.0, 24.0).unwrap(),
            actions: vec![Action::Type, Action::Select],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
        .with_state(state.clone());
        let manifold = InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap(),
            vec![region],
            0,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&manifold);

        assert_eq!(space.get_str("TYPE_TEXT:field").unwrap().state(), &state);
        assert_eq!(space.get_str("SELECT:field").unwrap().state(), &state);
        assert!(space
            .actions()
            .filter(|action| action.kind().is_control())
            .all(|action| action.state().is_empty()));
    }
}
