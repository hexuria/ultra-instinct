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
    /// Move the pointer over a region that opens something on hover.
    Hover,
    /// Scroll a scrollable region's own content down by one view.
    ScrollIn,
    /// Drag a draggable region onto another observed region.
    Drag,
    /// Press one key from [`KEY_NAMES`] on a region that takes keys.
    Key,
    /// Select (highlight) all of a text block's text, as a triple-click does.
    SelectText,
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
            Self::Hover => "HOVER",
            Self::ScrollIn => "SCROLL_IN",
            Self::Drag => "DRAG",
            Self::Key => "KEY",
            Self::SelectText => "SELECT_TEXT",
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
            "HOVER" | "hover" => Self::Hover,
            "SCROLL_IN" | "scroll_in" => Self::ScrollIn,
            "DRAG" | "drag" => Self::Drag,
            "KEY" | "key" => Self::Key,
            "SELECT_TEXT" | "select_text" => Self::SelectText,
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
    /// DRAG only: where the dragged region is released.
    dest: Option<DragDest>,
    /// Surface point clicks only: the grid cell `[col, row]`.
    cell: Option<[u16; 2]>,
    /// KEY only: the key pressed, one of [`KEY_NAMES`].
    key: Option<&'static str>,
}

/// The release region of an offered DRAG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragDest {
    pub id: RegionId,
    pub label: String,
    pub fingerprint: u64,
    pub bounds: [i32; 4],
    /// A grid cell `[col, row]` of the destination surface: the release
    /// point. `None` releases on the region.
    pub cell: Option<[u16; 2]>,
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
            dest: None,
            cell: None,
            key: None,
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
    pub fn dest(&self) -> Option<&DragDest> {
        self.dest.as_ref()
    }
    /// The surface cell `[col, row]` of a point click ([`SURFACE_GRID`] wide).
    pub fn cell(&self) -> Option<[u16; 2]> {
        self.cell
    }
    /// KEY only: the key this action presses.
    pub fn key(&self) -> Option<&'static str> {
        self.key
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
        // A label several targets share ("Reply" under every message) does
        // not say which one is meant: those targets carry near text too.
        let mut label_count = BTreeMap::<&str, usize>::new();
        for region in manifold.regions().filter(|r| region_is_viable_target(r)) {
            *label_count.entry(region.label().trim()).or_default() += 1;
        }
        // An unnamed generic wrapper around a clickable control adds a second,
        // nameless way to press the same thing: the control is offered, the
        // wrapper's click is not.
        let wraps_click: std::collections::BTreeSet<&RegionId> = manifold
            .regions()
            .filter(|r| region_is_viable_target(r) && r.actions().contains(&Action::Click))
            .filter_map(|r| r.parent())
            .collect();

        for region in manifold.regions() {
            if !region_is_viable_target(region) {
                continue;
            }
            for claim in region.actions() {
                let Some(kind) = map_region_claim(*claim) else {
                    continue;
                };
                if kind == ActionKind::Click
                    && region.role() == Role::Generic
                    && region.label().trim().is_empty()
                    && wraps_click.contains(region.id())
                {
                    continue;
                }
                // Readonly controls stay clickable (focus/select text) but are
                // not offered as TYPE_TEXT / SELECT targets.
                if region.flags().readonly()
                    && matches!(kind, ActionKind::TypeText | ActionKind::Select)
                {
                    continue;
                }
                let mut observed = target_action(kind, region);
                let label = observed.label.trim();
                if label.is_empty() || label_count.get(label).is_some_and(|&n| n > 1) {
                    observed.near_text = near_text(manifold, region, &label_count);
                }
                actions.insert(observed.id.clone(), observed);
            }
        }

        for observed in drag_actions(manifold) {
            actions.insert(observed.id.clone(), observed);
        }
        for observed in point_actions(manifold) {
            actions.insert(observed.id.clone(), observed);
        }
        for observed in key_actions(manifold) {
            actions.insert(observed.id.clone(), observed);
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

    /// The same space without the actions `drop` names.
    pub fn without_ids(&self, drop: &[ActionId]) -> Self {
        Self {
            captured_at_ms: self.captured_at_ms,
            actions: self
                .actions
                .iter()
                .filter(|(id, _)| !drop.contains(id))
                .map(|(id, a)| (id.clone(), a.clone()))
                .collect(),
        }
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
        // A region that opens something on hover, and a region whose own
        // content scrolls. Page scroll stays the SCROLL_UP / SCROLL_DOWN controls.
        Action::Hover => Some(ActionKind::Hover),
        Action::Scroll => Some(ActionKind::ScrollIn),
        // A drag needs a destination: offered as source / destination pairs.
        // A surface's points are offered as a grid of cells.
        Action::SelectText => Some(ActionKind::SelectText),
        // Keys are offered one per key (`key_actions`).
        Action::Focus | Action::Drag | Action::Point | Action::Key => None,
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
        dest: None,
        cell: None,
        key: None,
    }
}

/// Cells per side of the click grid offered on a drawing surface.
pub const SURFACE_GRID: u16 = 8;
/// Most surfaces whose points are offered per observation.
const MAX_SURFACES: usize = 2;
/// Smallest surface side, in pixels, that gets a grid.
const MIN_SURFACE_PX: f64 = 40.0;

/// The fraction `(fx, fy)` of the surface box at a cell's center.
pub fn cell_center_fraction(cell: [u16; 2]) -> (f64, f64) {
    let side = f64::from(SURFACE_GRID);
    (
        (f64::from(cell[0]) + 0.5) / side,
        (f64::from(cell[1]) + 0.5) / side,
    )
}

/// Point clicks on drawing surfaces: each viable region claiming Point gets a
/// `SURFACE_GRID` x `SURFACE_GRID` grid of offered `CLICK:<surface>@c,r`
/// cells. The policy picks a cell id; it never sends coordinates.
fn point_actions(manifold: &InteractionManifold) -> Vec<ObservedAction> {
    let mut out = Vec::new();
    let surfaces = manifold
        .regions()
        .filter(|r| region_is_viable_target(r) && r.actions().contains(&Action::Point))
        .filter(|r| r.rect().width() >= MIN_SURFACE_PX && r.rect().height() >= MIN_SURFACE_PX)
        .take(MAX_SURFACES);
    for surface in surfaces {
        let rect = surface.rect();
        let (cw, ch) = (
            rect.width() / f64::from(SURFACE_GRID),
            rect.height() / f64::from(SURFACE_GRID),
        );
        for row in 0..SURFACE_GRID {
            for col in 0..SURFACE_GRID {
                let mut action = target_action(ActionKind::Click, surface);
                action.id = ActionId(format!("CLICK:{}@{col},{row}", surface.id()));
                action.cell = Some([col, row]);
                let x = rect.x() + f64::from(col) * cw;
                let y = rect.y() + f64::from(row) * ch;
                action.bounds = Some([x, y, cw, ch].map(|v| v.round() as i32));
                action.near_text = Some(format!(
                    "{} point col {col} row {row} of {}",
                    if surface.label().is_empty() {
                        "surface"
                    } else {
                        surface.label()
                    },
                    SURFACE_GRID
                ));
                out.push(action);
            }
        }
    }
    out
}

/// Every key a KEY action may press: a closed set, never page-supplied.
pub const KEY_NAMES: [&str; 10] = [
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Enter",
    "Escape",
];
/// Keys that move a slider's value.
const SLIDER_KEYS: [&str; 8] = [
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];
/// Keys that walk and pick a combobox's options.
const COMBOBOX_KEYS: [&str; 4] = ["ArrowDown", "ArrowUp", "Enter", "Escape"];
/// Most KEY actions offered per observation.
const MAX_KEY_ACTIONS: usize = 48;

/// The keys a region that claims Key takes, by what it is: a combobox walks
/// its options, a text field submits with Enter, anything else (a slider, a
/// slider handle) moves its value.
fn keys_for(region: &InteractionRegion) -> &'static [&'static str] {
    if region.role() == Role::ComboBox {
        &COMBOBOX_KEYS
    } else if region.actions().contains(&Action::Type) {
        &["Enter"]
    } else {
        &SLIDER_KEYS
    }
}

/// KEY offers `KEY:<region>#<key>`, one per key a viable Key region takes.
fn key_actions(manifold: &InteractionManifold) -> Vec<ObservedAction> {
    let mut out = Vec::new();
    for region in manifold
        .regions()
        .filter(|r| region_is_viable_target(r) && r.actions().contains(&Action::Key))
    {
        for key in keys_for(region) {
            if out.len() == MAX_KEY_ACTIONS {
                return out;
            }
            let mut action = target_action(ActionKind::Key, region);
            action.id = ActionId(format!("KEY:{}#{key}", region.id()));
            action.key = Some(key);
            out.push(action);
        }
    }
    out
}

/// Most drag sources offered per observation.
const MAX_DRAG_SOURCES: usize = 8;
/// Most destinations offered per drag source, nearest first. A sanity bound,
/// not a menu cut: a remote request sends sources and destinations once each
/// (not one entry per pair), so a far destination costs one entry.
const MAX_DROPS_PER_SOURCE: usize = 96;

/// DRAG offers: each viable region claiming Drag, paired with the other
/// viable regions as release points, nearest first, up to
/// [`MAX_DROPS_PER_SOURCE`]. The menu stays finite and every pair is an
/// explicit offered id `DRAG:<src>><dst>`.
fn drag_actions(manifold: &InteractionManifold) -> Vec<ObservedAction> {
    let viable: Vec<&InteractionRegion> = manifold
        .regions()
        .filter(|r| region_is_viable_target(r))
        .collect();
    let mut out = Vec::new();
    let sources = viable
        .iter()
        .filter(|r| r.actions().contains(&Action::Drag))
        .filter(|r| !r.id().as_str().contains('>'))
        .take(MAX_DRAG_SOURCES);
    for src in sources {
        let c = src.rect().center();
        let mut drops: Vec<&&InteractionRegion> = viable
            .iter()
            .filter(|d| d.id() != src.id() && !d.id().as_str().contains('>'))
            .collect();
        drops.sort_by(|a, b| {
            a.rect()
                .center()
                .distance(c)
                .total_cmp(&b.rect().center().distance(c))
                .then_with(|| a.id().cmp(b.id()))
        });
        for dst in drops.into_iter().take(MAX_DROPS_PER_SOURCE) {
            let mut action = target_action(ActionKind::Drag, src);
            action.id = ActionId(format!("DRAG:{}>{}", src.id(), dst.id()));
            action.dest = Some(DragDest {
                id: dst.id().clone(),
                label: dst.label().to_owned(),
                fingerprint: dst.fingerprint().bits(),
                bounds: pixel_box(dst.rect()),
                cell: None,
            });
            out.push(action);
        }
        // A source drawn on a surface can be moved to any cell of it ("drag
        // the circle up"): a surface that takes points, or the drawing the
        // source sits in.
        for surface in viable.iter().filter(|s| {
            s.id() != src.id()
                && (s.actions().contains(&Action::Point) || src.parent() == Some(s.id()))
                && s.rect().width() >= MIN_SURFACE_PX
                && s.rect().height() >= MIN_SURFACE_PX
                && s.rect().contains_point(c)
        }) {
            let rect = surface.rect();
            let (cw, ch) = (
                rect.width() / f64::from(SURFACE_GRID),
                rect.height() / f64::from(SURFACE_GRID),
            );
            for row in 0..SURFACE_GRID {
                for col in 0..SURFACE_GRID {
                    let mut action = target_action(ActionKind::Drag, src);
                    action.id = ActionId(format!("DRAG:{}>{}@{col},{row}", src.id(), surface.id()));
                    let x = rect.x() + f64::from(col) * cw;
                    let y = rect.y() + f64::from(row) * ch;
                    action.dest = Some(DragDest {
                        id: surface.id().clone(),
                        label: format!(
                            "{} point col {col} row {row} of {SURFACE_GRID}",
                            if surface.label().is_empty() {
                                "surface"
                            } else {
                                surface.label()
                            }
                        ),
                        fingerprint: surface.fingerprint().bits(),
                        bounds: [x, y, cw, ch].map(|v| v.round() as i32),
                        cell: Some([col, row]),
                    });
                    out.push(action);
                }
            }
        }
    }
    out
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
/// left on the same row, else to the right on it ("[Odd] 4 [Even]"), else
/// the nearest text directly above. Text several
/// regions share (another "Reply") tells nothing apart and is skipped.
/// Geometry only; `None` when no text is that close.
fn near_text(
    manifold: &InteractionManifold,
    target: &InteractionRegion,
    label_count: &BTreeMap<&str, usize>,
) -> Option<String> {
    let named = |region: &InteractionRegion| {
        let label = region.label().trim();
        region.id() != target.id()
            && !label.is_empty()
            && label_count.get(label).is_none_or(|&n| n <= 1)
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
    let right = || {
        manifold
            .regions()
            .filter(|r| named(r))
            .filter_map(|r| {
                let rect = r.rect();
                let same_row = rect.y() < t.bottom() && t.y() < rect.bottom();
                let gap = rect.x() - t.right();
                (same_row && (0.0..=NEAR_TEXT_ROW_GAP_PX).contains(&gap)).then_some((gap, r))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
    };
    let region = parent.or_else(|| {
        left.map(|(_, r)| r)
            .or_else(|| right().map(|(_, r)| r))
            .or_else(|| above().map(|(_, r)| r))
    })?;
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
        dest: None,
        cell: None,
        key: None,
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
            ActionKind::Hover,
            ActionKind::ScrollIn,
            ActionKind::Drag,
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
    fn an_unnamed_wrapper_around_a_clickable_control_is_not_a_second_target() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=menu role=generic label="" x=6 y=56 w=50 h=51 actions=click sources=dom
            region id=item role=menu_item label="Randee" x=10 y=93 w=36 h=11 actions=click,hover parent=menu sources=dom
            region id=card role=generic label="Flight 12" x=100 y=56 w=80 h=40 actions=click sources=dom
            region id=book role=button label="Book" x=110 y=70 w=40 h=20 actions=click parent=card sources=dom
            region id=lonely role=generic label="" x=300 y=56 w=40 h=40 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        assert!(
            space.get_str("CLICK:menu").is_none(),
            "nameless wrapper of a control"
        );
        assert!(space.get_str("CLICK:item").is_some());
        assert!(space.get_str("HOVER:item").is_some());
        assert!(
            space.get_str("CLICK:card").is_some(),
            "a named wrapper stays"
        );
        assert!(
            space.get_str("CLICK:lonely").is_some(),
            "a nameless box with no control inside stays"
        );
    }

    #[test]
    fn a_button_with_nothing_on_its_left_reads_the_text_on_its_right() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=odd1 role=button label="Odd" x=22 y=64 w=25 h=15 actions=click sources=dom
            region id=n1 role=text label="4" x=80 y=64 w=10 h=15 actions= sources=dom
            region id=even1 role=button label="Even" x=112 y=64 w=30 h=15 actions=click sources=dom
            region id=odd2 role=button label="Odd" x=22 y=98 w=25 h=15 actions=click sources=dom
            region id=n2 role=text label="-5" x=80 y=98 w=14 h=15 actions= sources=dom
            region id=even2 role=button label="Even" x=112 y=98 w=30 h=15 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let near = |id: &str| space.get_str(id).unwrap().near_text().map(str::to_owned);
        assert_eq!(near("CLICK:odd1").as_deref(), Some("4"), "right of it");
        assert_eq!(near("CLICK:even1").as_deref(), Some("4"), "left of it");
        assert_eq!(near("CLICK:odd2").as_deref(), Some("-5"));
    }

    #[test]
    fn targets_sharing_a_label_say_which_row_they_belong_to() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=ana role=text label="Ana: lunch?" x=10 y=10 w=200 h=20 actions= sources=dom
            region id=r1 role=button label="Reply" x=10 y=40 w=50 h=20 actions=click sources=dom
            region id=f1 role=button label="Forward" x=70 y=40 w=60 h=20 actions=click sources=dom
            region id=ben role=text label="Ben: notes" x=10 y=110 w=200 h=20 actions= sources=dom
            region id=r2 role=button label="Reply" x=10 y=140 w=50 h=20 actions=click sources=dom
            region id=f2 role=button label="Forward" x=70 y=140 w=60 h=20 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let near = |id: &str| space.get_str(id).unwrap().near_text().map(str::to_owned);
        assert_eq!(near("CLICK:r1").as_deref(), Some("Ana: lunch?"));
        assert_eq!(near("CLICK:r2").as_deref(), Some("Ben: notes"));
        assert_eq!(
            near("CLICK:f2").as_deref(),
            Some("Ben: notes"),
            "the Reply on its left is shared text, not a name"
        );
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
    fn hover_and_scroll_claims_offer_target_bound_hover_and_scroll_in() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=menu role=menu_item label="File" x=10 y=10 w=60 h=24 actions=click,hover sources=dom
            region id=list role=generic label="" x=10 y=50 w=200 h=300 actions=scroll sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let hover = space.get_str("HOVER:menu").expect("hover offered");
        assert_eq!(hover.kind(), ActionKind::Hover);
        assert!(!hover.kind().is_control());
        let scroll = space.get_str("SCROLL_IN:list").expect("scroll-in offered");
        assert_eq!(scroll.target().map(RegionId::as_str), Some("list"));
        assert!(
            space.get_str("CLICK:list").is_none(),
            "scrolling is not clicking"
        );
    }

    #[test]
    fn a_surface_offers_a_click_grid_of_cells() {
        let m = parse_fixture(
            "viewport w=800 h=600\n\
             region id=pad role=image label=\"canvas\" x=0 y=0 w=160 h=80 actions=point,click sources=dom\n\
             region id=tiny role=image label=\"canvas\" x=300 y=0 w=20 h=20 actions=point,click sources=dom\n",
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let cells: Vec<_> = space.actions().filter(|a| a.cell().is_some()).collect();
        assert_eq!(
            cells.len(),
            usize::from(SURFACE_GRID * SURFACE_GRID),
            "only the big surface"
        );
        let corner = space.get_str("CLICK:pad@7,7").expect("last cell offered");
        assert_eq!(corner.kind(), ActionKind::Click);
        assert_eq!(corner.target().map(RegionId::as_str), Some("pad"));
        assert_eq!(corner.bounds(), Some([140, 70, 20, 10]));
        assert!(space.get_str("CLICK:pad@8,0").is_none(), "off the grid");
        assert_eq!(cell_center_fraction([0, 7]), (0.0625, 0.9375));
    }

    #[test]
    fn regions_that_take_keys_offer_the_keys_they_take() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=handle role=generic label="" x=10 y=10 w=12 h=12 actions=drag,key sources=dom
            region id=q role=text_field label="Search" x=10 y=50 w=200 h=20 actions=type,key sources=dom
            region id=city role=combobox label="City" x=10 y=90 w=200 h=20 actions=type,key sources=dom
            region id=go role=button label="Go" x=10 y=130 w=40 h=20 actions=click sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let keys = |region: &str| -> Vec<&str> {
            space
                .targets_of(ActionKind::Key)
                .filter(|a| a.target().is_some_and(|t| t.as_str() == region))
                .filter_map(ObservedAction::key)
                .collect()
        };
        assert_eq!(
            keys("handle").len(),
            8,
            "a slider handle moves by every movement key"
        );
        assert!(keys("handle").contains(&"ArrowRight") && !keys("handle").contains(&"Enter"));
        assert_eq!(keys("q"), ["Enter"], "a field submits");
        assert_eq!(keys("city").len(), 4, "a combobox walks and picks");
        assert!(keys("go").is_empty(), "no key claim, no keys");
        let right = space.get_str("KEY:handle#ArrowRight").unwrap();
        assert_eq!(right.key(), Some("ArrowRight"));
        assert!(KEY_NAMES.contains(&right.key().unwrap()));
    }

    #[test]
    fn a_shape_on_a_drawing_can_be_moved_to_any_cell_of_it() {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=svg role=image label="drawing" x=0 y=50 w=160 h=160 actions=click,point sources=dom
            region id=dot role=image label="black circle" x=70 y=120 w=20 h=20 actions=click,drag sources=dom
            region id=chip role=generic label="Chip" x=400 y=400 w=20 h=20 actions=drag sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        let up = space.get_str("DRAG:dot>svg@4,0").expect("a cell above it");
        let dest = up.dest().unwrap();
        assert_eq!((dest.id.as_str(), dest.cell), ("svg", Some([4, 0])));
        assert_eq!(dest.bounds, [80, 50, 20, 20]);
        assert!(
            space.get_str("DRAG:dot>svg").is_some(),
            "the surface itself too"
        );
        assert!(
            space.get_str("DRAG:chip>svg@0,0").is_none(),
            "a source off the drawing gets no cells"
        );
        // A drawing with no click evidence still takes the shapes drawn in it.
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=art role=image label="drawing" x=0 y=50 w=160 h=160 sources=dom
            region id=star role=image label="black polygon" x=70 y=120 w=20 h=20 actions=click,drag parent=art sources=dom
            "#,
        )
        .unwrap();
        let space = ActionSpace::from_manifold(&m);
        assert!(
            space.get_str("DRAG:star>art@4,0").is_some(),
            "its own drawing"
        );
        assert!(
            !space
                .targets_of(ActionKind::Click)
                .any(|a| a.cell().is_some()),
            "no click cells without click evidence"
        );
    }

    #[test]
    fn drag_sources_pair_with_their_nearest_viable_regions() {
        let mut src = String::from(
            "viewport w=1200 h=900\n\
             region id=card role=generic label=\"Card\" x=0 y=0 w=20 h=20 actions=drag sources=dom\n\
             region id=gone role=generic label=\"Gone\" x=30 y=0 w=20 h=20 sources=dom flags=hidden\n",
        );
        for i in 0..(MAX_DROPS_PER_SOURCE + 4) {
            src.push_str(&format!(
                "region id=slot{i:03} role=generic label=\"Slot {i}\" x={} y={} w=20 h=20 sources=dom\n",
                40 + (i % 30) * 30,
                (i / 30) * 30
            ));
        }
        let space = ActionSpace::from_manifold(&parse_fixture(&src).unwrap());
        let drags: Vec<_> = space.targets_of(ActionKind::Drag).collect();
        assert_eq!(drags.len(), MAX_DROPS_PER_SOURCE, "capped, nearest first");
        let first = space
            .get_str("DRAG:card>slot000")
            .expect("nearest slot offered");
        assert_eq!(first.dest().map(|d| d.label.as_str()), Some("Slot 0"));
        assert!(
            space.get_str("DRAG:card>gone").is_none(),
            "hidden is not a destination"
        );
        assert!(
            space.get_str("DRAG:card>card").is_none(),
            "never onto itself"
        );
        assert!(space.get_str("DRAG:card>slot19").is_none(), "past the cap");
        assert!(
            space.get_str("CLICK:card").is_none(),
            "dragging is not clicking"
        );
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
