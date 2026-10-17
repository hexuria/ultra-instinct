//! Browser runtime surface the agent drives: live CDP ([`BrowserSession`]) or
//! an in-memory [`MockBrowser`].
//!
//! [`BrowserRuntime::dispatch`] is raw target input. The agent never calls it
//! directly: every target-bound input goes through
//! [`crate::execute_ticketed`], which checks the one-shot ledger, re-observes,
//! revalidates the [`aui_guard`] ticket, and re-runs the hard gate first.

use std::collections::BTreeMap;
use std::time::Duration;

use aui_browser::{
    page_delta, BrowserError, BrowserSession, CdpTransport, PageDelta, PageState, ScrollDirection,
};
use aui_core::{Action, InteractionManifold, RegionId, Role};

use aui_policy::{PageView, Screenshot};

use crate::error::AgentError;

const AUTOCOMPLETE_MAX_POLLS: u32 = 10;
const AUTOCOMPLETE_POLL_MS: u64 = 25;

/// One input the executor may send to an exact observed target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A trusted pointer click at the target's center, as a user clicks:
    /// `HTMLElement.click()` neither moves focus nor fires `pointerdown` /
    /// `mousedown`, which many widgets listen for.
    Click,
    /// A script click (`HTMLElement.click()` on the observed node). Same
    /// ticket capability as [`Input::Click`]; used after a pointer click on
    /// the same target showed no visible change.
    ScriptClick,
    /// Replace the field value with this text.
    Type(String),
    /// Choose the one option whose value / label / text matches.
    Select(String),
    /// Move the trusted pointer over the target (no press).
    Hover,
    /// Scroll the target's own content down (a wheel at its center).
    ScrollIn,
    /// Press on the target, move to `to`'s center, release there.
    Drag { to: RegionId },
    /// A trusted click at the center of grid cell `[col, row]` of a drawing
    /// surface, placed from the surface's fresh box.
    ClickAt { cell: [u16; 2] },
}

impl Input {
    /// Region capability this input needs (and the ticket must carry).
    pub fn action(&self) -> Action {
        match self {
            Self::Click | Self::ScriptClick | Self::ClickAt { .. } => Action::Click,
            Self::Type(_) => Action::Type,
            Self::Select(_) => Action::Select,
            Self::Hover => Action::Hover,
            Self::ScrollIn => Action::Scroll,
            Self::Drag { .. } => Action::Drag,
        }
    }

    /// Text payload, if any.
    pub fn payload(&self) -> Option<&str> {
        match self {
            Self::Click
            | Self::ScriptClick
            | Self::Hover
            | Self::ScrollIn
            | Self::Drag { .. }
            | Self::ClickAt { .. } => None,
            Self::Type(text) | Self::Select(text) => Some(text),
        }
    }
}

/// Current value of a field after an input (for TYPE / SELECT verification).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldValue {
    /// `value` property (option value for a `<select>`).
    pub value: String,
    /// Selected option text for a `<select>`; otherwise empty.
    pub text: String,
}

impl FieldValue {
    /// True when `expected` is the value or (for selects) the visible text,
    /// exact or case-insensitive after trim.
    pub fn matches(&self, expected: &str) -> bool {
        let e = expected.trim();
        let eq = |a: &str| a == expected || a.trim().eq_ignore_ascii_case(e);
        eq(&self.value) || (!self.text.is_empty() && eq(&self.text))
    }
}

pub trait BrowserRuntime {
    fn observe(&mut self) -> Result<&InteractionManifold, AgentError>;
    fn focused(&self) -> Option<RegionId>;
    fn page(&self) -> Option<&PageState>;

    /// Raw input to `target`. Call only through [`crate::execute_ticketed`].
    fn dispatch(&mut self, target: &RegionId, input: &Input) -> Result<(), AgentError>;

    /// Page-level scroll primitive (no target).
    fn scroll(&mut self, direction: ScrollDirection) -> Result<(), AgentError>;

    /// Let the page settle after an input (navigation, rerender). Default: no-op.
    fn settle(&mut self) {}

    /// The last observation, without observing again. Default: none.
    fn last_observation(&self) -> Option<&InteractionManifold> {
        None
    }

    /// Pause between re-checks while a clause waits. Default: real sleep.
    fn pause(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Settle after a ticketed region input. Default: `settle()`.
    fn settle_after_input(&mut self, _target: &RegionId, _input: &Input) {
        self.settle()
    }

    /// Read a field's current value from the last observation's binding.
    /// `None` when the runtime cannot read it; verification then falls back
    /// to the manifold diff.
    fn read_value(&mut self, _target: &RegionId) -> Option<FieldValue> {
        None
    }

    fn page_delta_between(&self, before: &PageState, after: &PageState) -> PageDelta {
        page_delta(before, after)
    }

    /// Viewport screenshot for a policy that reads pixels. Read only, never
    /// page input. `None` when the runtime cannot capture one; the decision
    /// then runs without it. Default: none.
    fn screenshot(&mut self) -> Option<Screenshot> {
        None
    }

    /// Visible page text with URL and title, for a policy that reads it.
    /// Read only. `None` when the runtime cannot read it. Default: none.
    fn page_view(&mut self) -> Option<PageView> {
        None
    }
}

/// Longest page text handed to a policy, in characters (jev-ultrafast's
/// page-text budget).
pub const PAGE_TEXT_MAX_CHARS: usize = 6000;

fn page_view_of(text: &str, page: Option<&PageState>) -> PageView {
    PageView {
        url: page.and_then(|p| p.url()).map(str::to_owned),
        title: page.and_then(|p| p.title()).map(str::to_owned),
        text: text.chars().take(PAGE_TEXT_MAX_CHARS).collect(),
    }
}

fn browser_err(e: BrowserError) -> AgentError {
    match e {
        BrowserError::InputRejected(msg) => AgentError::InputRejected(msg),
        other => AgentError::Browser(other.to_string()),
    }
}

impl<T: CdpTransport> BrowserRuntime for BrowserSession<T> {
    fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        BrowserSession::observe(self).map_err(browser_err)
    }

    fn focused(&self) -> Option<RegionId> {
        BrowserSession::page(self).and_then(|p| p.focused().cloned())
    }

    fn last_observation(&self) -> Option<&InteractionManifold> {
        BrowserSession::manifold(self)
    }

    fn screenshot(&mut self) -> Option<Screenshot> {
        let shot = Screenshot::from_base64(BrowserSession::screenshot_jpeg(self).ok().flatten()?);
        Some(match BrowserSession::manifold(self).map(|m| m.viewport()) {
            Some(v) => shot.with_css_size(v.width().round() as i32, v.height().round() as i32),
            None => shot,
        })
    }

    fn page_view(&mut self) -> Option<PageView> {
        let text = BrowserSession::page_text(self).ok()?;
        Some(page_view_of(&text, BrowserSession::page(self)))
    }

    fn page(&self) -> Option<&PageState> {
        BrowserSession::page(self)
    }

    fn dispatch(&mut self, target: &RegionId, input: &Input) -> Result<(), AgentError> {
        match input {
            Input::Click => BrowserSession::pointer_click(self, target).map(|_| ()),
            Input::ScriptClick => BrowserSession::press(self, target, Action::Click).map(|_| ()),
            Input::Type(text) => BrowserSession::type_text(self, target, text).map(|_| ()),
            Input::Select(option) => {
                BrowserSession::select_option(self, target, option).map(|_| ())
            }
            Input::Hover => BrowserSession::hover(self, target),
            Input::ScrollIn => BrowserSession::scroll_region(self, target),
            Input::Drag { to } => BrowserSession::drag(self, target, to),
            Input::ClickAt { cell } => {
                let (fx, fy) = aui_core::cell_center_fraction(*cell);
                BrowserSession::pointer_click_at(self, target, fx, fy).map(|_| ())
            }
        }
        .map_err(browser_err)
    }

    fn scroll(&mut self, direction: ScrollDirection) -> Result<(), AgentError> {
        BrowserSession::scroll(self, direction).map_err(browser_err)
    }

    /// Poll `document.readyState` until `complete` (bounded). A replay
    /// transport without a scripted `Runtime.evaluate` returns immediately.
    fn settle(&mut self) {
        for attempt in 0..30 {
            match self.ready_state() {
                Ok(Some(state)) if state == "complete" => {
                    if attempt == 0 {
                        // Same-document rerender: give the framework a beat.
                        std::thread::sleep(Duration::from_millis(120));
                    }
                    return;
                }
                Ok(Some(_)) => std::thread::sleep(Duration::from_millis(100)),
                Ok(None) | Err(_) => return,
            }
        }
    }

    fn settle_after_input(&mut self, target: &RegionId, input: &Input) {
        self.settle();
        let role = BrowserSession::manifold(self)
            .and_then(|manifold| manifold.get(target))
            .map(|region| region.role());
        if !wants_autocomplete_settle(role, input) {
            return;
        }

        let mut previous = None;
        let mut first = None;
        for poll in 0..AUTOCOMPLETE_MAX_POLLS {
            match BrowserSession::autocomplete_options_signature(self, target) {
                Ok(Some(signature)) => {
                    if first.is_none() {
                        first = Some(signature.clone());
                    }
                    if autocomplete_settled(first.as_deref(), previous.as_deref(), &signature) {
                        return;
                    }
                    previous = Some(signature);
                }
                Ok(None) => {
                    self.settle();
                    return;
                }
                Err(_) => return,
            }
            if poll + 1 < AUTOCOMPLETE_MAX_POLLS {
                std::thread::sleep(Duration::from_millis(AUTOCOMPLETE_POLL_MS));
            }
        }
    }

    fn read_value(&mut self, target: &RegionId) -> Option<FieldValue> {
        let (value, text) = self.field_value(target).ok()??;
        Some(FieldValue { value, text })
    }
}

fn wants_autocomplete_settle(role: Option<Role>, input: &Input) -> bool {
    role == Some(Role::ComboBox) && matches!(input, Input::Type(_))
}

fn autocomplete_settled(first: Option<&str>, previous: Option<&str>, current: &str) -> bool {
    if previous != Some(current) {
        return false;
    }
    let Some((&scope, rest)) = current.as_bytes().split_first() else {
        return false;
    };
    let Ok(rest) = std::str::from_utf8(rest) else {
        return false;
    };
    let Some((count, _)) = rest.split_once(':') else {
        return false;
    };
    let Ok(count) = count.parse::<u32>() else {
        return false;
    };
    if count == 0 {
        return false;
    }
    match scope {
        b'o' => true,
        b'd' => Some(current) != first,
        _ => false,
    }
}

/// In-memory browser for offline agent tests. Mutators simulate UI changes.
#[derive(Clone, Debug)]
pub struct MockBrowser {
    manifold: InteractionManifold,
    focused: Option<RegionId>,
    page: PageState,
    /// When set, the next dispatch replaces the manifold with this snapshot.
    on_press: Option<InteractionManifold>,
    /// Page (URL / title) after the next dispatch: simulates a navigation.
    on_press_page: Option<PageState>,
    /// `(observes_remaining, next)`: swap after that many more observes.
    scheduled: Option<(u32, InteractionManifold)>,
    press_log: Vec<(RegionId, Action)>,
    input_log: Vec<(RegionId, Input)>,
    scroll_log: Vec<ScrollDirection>,
    values: BTreeMap<RegionId, FieldValue>,
    /// Page refuses the next dispatch with this message (nothing changes).
    reject_next: Option<String>,
    /// Value the page "really" ends up with, regardless of what was typed.
    value_override: Option<String>,
    observe_count: u32,
    pauses: u32,
    screenshot: Option<Screenshot>,
    screenshots_taken: u32,
    page_text: Option<String>,
}

impl MockBrowser {
    pub fn new(manifold: InteractionManifold) -> Self {
        Self {
            manifold,
            focused: None,
            // A readable page by default — unknown URL/title is the
            // exceptional live case; `with_page` simulates it.
            page: PageState::new("https://mock.invalid/", "mock", None),
            on_press: None,
            on_press_page: None,
            scheduled: None,
            press_log: Vec::new(),
            input_log: Vec::new(),
            scroll_log: Vec::new(),
            values: BTreeMap::new(),
            reject_next: None,
            value_override: None,
            observe_count: 0,
            pauses: 0,
            screenshot: None,
            screenshots_taken: 0,
            page_text: None,
        }
    }

    /// The page reads this visible text on request.
    pub fn with_page_text(mut self, text: impl Into<String>) -> Self {
        self.page_text = Some(text.into());
        self
    }

    /// The page captures this screenshot on request.
    pub fn with_screenshot(mut self, screenshot: Screenshot) -> Self {
        self.screenshot = Some(screenshot);
        self
    }

    /// Screenshots requested so far.
    pub fn screenshots_taken(&self) -> u32 {
        self.screenshots_taken
    }

    pub fn with_page(mut self, page: PageState) -> Self {
        self.page = page;
        self
    }

    pub fn set_on_press(&mut self, next: InteractionManifold) {
        self.on_press = Some(next);
    }

    /// After the next dispatch the page is `next` (a navigation).
    pub fn set_on_press_page(&mut self, next: PageState) {
        self.on_press_page = Some(next);
    }

    /// Replace the manifold after `observes` more observations return the
    /// current one (simulates a page that changes between predict and act,
    /// e.g. during text resolution latency).
    pub fn schedule_swap(&mut self, observes: u32, next: InteractionManifold) {
        self.scheduled = Some((observes, next));
    }

    pub fn reject_next(&mut self, message: impl Into<String>) {
        self.reject_next = Some(message.into());
    }

    pub fn override_value(&mut self, value: impl Into<String>) {
        self.value_override = Some(value.into());
    }

    pub fn set_focused(&mut self, id: Option<RegionId>) {
        self.focused = id;
    }

    pub fn press_log(&self) -> &[(RegionId, Action)] {
        &self.press_log
    }

    pub fn input_log(&self) -> &[(RegionId, Input)] {
        &self.input_log
    }

    pub fn scroll_log(&self) -> &[ScrollDirection] {
        &self.scroll_log
    }

    /// Waits taken by a waiting clause.
    pub fn pauses(&self) -> u32 {
        self.pauses
    }

    pub fn observe_count(&self) -> u32 {
        self.observe_count
    }

    pub fn replace_manifold(&mut self, manifold: InteractionManifold) {
        self.manifold = manifold;
    }
}

impl BrowserRuntime for MockBrowser {
    fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        self.observe_count += 1;
        if let Some((remaining, _)) = self.scheduled.as_mut() {
            if *remaining == 0 {
                let (_, next) = self.scheduled.take().expect("checked");
                self.manifold = next;
            } else {
                *remaining -= 1;
            }
        }
        Ok(&self.manifold)
    }

    fn focused(&self) -> Option<RegionId> {
        self.focused.clone()
    }

    fn page(&self) -> Option<&PageState> {
        Some(&self.page)
    }

    fn dispatch(&mut self, target: &RegionId, input: &Input) -> Result<(), AgentError> {
        if let Some(message) = self.reject_next.take() {
            return Err(AgentError::InputRejected(message));
        }
        self.press_log.push((target.clone(), input.action()));
        self.input_log.push((target.clone(), input.clone()));
        if let Some(payload) = input.payload() {
            let value = self
                .value_override
                .clone()
                .unwrap_or_else(|| payload.to_owned());
            let text = if matches!(input, Input::Select(_)) {
                value.clone()
            } else {
                String::new()
            };
            self.values
                .insert(target.clone(), FieldValue { value, text });
        }
        if let Some(next) = self.on_press.take() {
            self.manifold = next;
        }
        if let Some(next) = self.on_press_page.take() {
            self.page = next;
        }
        Ok(())
    }

    fn scroll(&mut self, direction: ScrollDirection) -> Result<(), AgentError> {
        self.scroll_log.push(direction);
        Ok(())
    }

    fn last_observation(&self) -> Option<&InteractionManifold> {
        Some(&self.manifold)
    }

    /// No real sleep in tests; each pause counts as one observation tick.
    fn pause(&mut self, _ms: u64) {
        self.pauses += 1;
    }

    fn read_value(&mut self, target: &RegionId) -> Option<FieldValue> {
        self.values.get(target).cloned()
    }

    fn screenshot(&mut self) -> Option<Screenshot> {
        self.screenshots_taken += 1;
        self.screenshot.clone()
    }

    fn page_view(&mut self) -> Option<PageView> {
        let text = self.page_text.clone()?;
        Some(page_view_of(&text, Some(&self.page)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocomplete_settle_is_only_for_combobox_typing() {
        assert!(wants_autocomplete_settle(
            Some(Role::ComboBox),
            &Input::Type("man".into())
        ));
        assert!(!wants_autocomplete_settle(
            Some(Role::TextField),
            &Input::Type("man".into())
        ));
        assert!(!wants_autocomplete_settle(
            Some(Role::ComboBox),
            &Input::Click
        ));
        assert!(!wants_autocomplete_settle(
            Some(Role::ComboBox),
            &Input::Select("Manila".into())
        ));
        assert!(!wants_autocomplete_settle(None, &Input::Type("man".into())));
    }

    #[test]
    fn autocomplete_settle_requires_stable_scoped_options() {
        assert!(autocomplete_settled(
            Some("o1:Manila"),
            Some("o1:Manila"),
            "o1:Manila"
        ));
        assert!(!autocomplete_settled(Some("o0:"), Some("o0:"), "o0:"));
        assert!(!autocomplete_settled(
            Some("d1:Canada"),
            Some("d1:Canada"),
            "d1:Canada"
        ));
        assert!(autocomplete_settled(
            Some("d1:Canada"),
            Some("d2:Canada\u{001f}Manila"),
            "d2:Canada\u{001f}Manila"
        ));
        assert!(!autocomplete_settled(
            Some("x1:Canada"),
            Some("x1:Canada"),
            "x1:Canada"
        ));
    }
}
