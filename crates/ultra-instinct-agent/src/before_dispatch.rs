//! A runtime with one check in front of its raw input.
//!
//! [`BeforeDispatch`] wraps a [`BrowserRuntime`] for one
//! [`crate::execute_ticketed_with`] call: just before the executor's
//! dispatch (after its fresh observe, revalidate, gate and consume), `before`
//! sees that fresh observation, the target and the input, and an `Err`
//! stops the dispatch. The MCP host's `browser_type` uses it to decide on
//! the executor's own observation whether the text is a secret.
//!
//! Every other trait method is forwarded to the inner runtime as it is, so
//! none silently falls back to a trait default: the test below reads the
//! trait's method list from its source and fails when a new one is not
//! forwarded here.

use ultra_instinct_browser::{PageDelta, PageState, ScrollDirection};
use ultra_instinct_core::{InteractionManifold, RegionId};
use ultra_instinct_policy::{PageView, Screenshot};

use crate::error::AgentError;
use crate::runtime::{BrowserRuntime, FieldValue, Input};

/// `inner` with `before` run ahead of each dispatch (see the module docs).
pub struct BeforeDispatch<'a, R: ?Sized, F> {
    pub inner: &'a mut R,
    /// Called with `inner`'s last observation, the target and the input;
    /// `Err` is returned in place of the dispatch.
    pub before: F,
}

impl<R, F> BrowserRuntime for BeforeDispatch<'_, R, F>
where
    R: BrowserRuntime + ?Sized,
    F: FnMut(Option<&InteractionManifold>, &RegionId, &Input) -> Result<(), AgentError>,
{
    fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        self.inner.observe()
    }
    fn focused(&self) -> Option<RegionId> {
        self.inner.focused()
    }
    fn page(&self) -> Option<&PageState> {
        self.inner.page()
    }
    fn dispatch(&mut self, target: &RegionId, input: &Input) -> Result<(), AgentError> {
        (self.before)(self.inner.last_observation(), target, input)?;
        self.inner.dispatch(target, input)
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use ultra_instinct_core::Rect;

    use super::*;

    /// Every `BrowserRuntime` method, in the trait's order.
    const RUNTIME_METHODS: [&str; 14] = [
        "observe",
        "focused",
        "page",
        "dispatch",
        "scroll",
        "settle",
        "last_observation",
        "pause",
        "settle_after_input",
        "read_value",
        "selected_text",
        "page_delta_between",
        "screenshot",
        "page_view",
    ];

    /// The trait's method names, read from its source.
    fn declared_methods() -> Vec<String> {
        let source = include_str!("runtime.rs");
        let start = source
            .find("pub trait BrowserRuntime {")
            .expect("trait in source");
        let body = &source[start..];
        let body = &body[..body.find("\n}\n").expect("trait end")];
        body.lines()
            .filter_map(|line| line.trim().strip_prefix("fn "))
            .map(|rest| rest[..rest.find(['(', '<']).unwrap_or(rest.len())].to_owned())
            .collect()
    }

    /// Records each method it is asked and answers with values no trait
    /// default gives.
    struct Recorder {
        manifold: InteractionManifold,
        page: PageState,
        log: RefCell<Vec<&'static str>>,
    }

    impl Recorder {
        fn hit(&self, method: &'static str) {
            self.log.borrow_mut().push(method);
        }
    }

    impl BrowserRuntime for Recorder {
        fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
            self.hit("observe");
            Ok(&self.manifold)
        }
        fn focused(&self) -> Option<RegionId> {
            self.hit("focused");
            RegionId::try_new("f").ok()
        }
        fn page(&self) -> Option<&PageState> {
            self.hit("page");
            Some(&self.page)
        }
        fn dispatch(&mut self, _target: &RegionId, _input: &Input) -> Result<(), AgentError> {
            self.hit("dispatch");
            Ok(())
        }
        fn scroll(&mut self, _direction: ScrollDirection) -> Result<(), AgentError> {
            self.hit("scroll");
            Ok(())
        }
        fn settle(&mut self) {
            self.hit("settle");
        }
        fn last_observation(&self) -> Option<&InteractionManifold> {
            self.hit("last_observation");
            Some(&self.manifold)
        }
        fn pause(&mut self, _ms: u64) {
            self.hit("pause");
        }
        fn settle_after_input(&mut self, _target: &RegionId, _input: &Input) {
            self.hit("settle_after_input");
        }
        fn read_value(&mut self, _target: &RegionId) -> Option<FieldValue> {
            self.hit("read_value");
            Some(FieldValue {
                value: "v".into(),
                text: String::new(),
            })
        }
        fn selected_text(&mut self) -> Option<String> {
            self.hit("selected_text");
            Some("sel".into())
        }
        fn page_delta_between(&self, before: &PageState, after: &PageState) -> PageDelta {
            self.hit("page_delta_between");
            ultra_instinct_browser::page_delta(before, after)
        }
        fn screenshot(&mut self) -> Option<Screenshot> {
            self.hit("screenshot");
            Some(Screenshot::from_base64("AA=="))
        }
        fn page_view(&mut self) -> Option<PageView> {
            self.hit("page_view");
            Some(PageView::default())
        }
    }

    fn recorder() -> Recorder {
        let viewport = Rect::try_new(0.0, 0.0, 100.0, 100.0).unwrap();
        Recorder {
            manifold: InteractionManifold::try_new(viewport, Vec::new(), 0).unwrap(),
            page: PageState::unknown(None),
            log: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn forwards_every_runtime_method() {
        assert_eq!(
            declared_methods(),
            RUNTIME_METHODS,
            "BrowserRuntime changed: forward the new method in BeforeDispatch and list it here"
        );
        let mut inner = recorder();
        let mut checked = 0;
        let mut wrapped = BeforeDispatch {
            inner: &mut inner,
            before: |_: Option<&InteractionManifold>, _: &RegionId, _: &Input| {
                checked += 1;
                Ok(())
            },
        };
        let id = RegionId::try_new("f").unwrap();
        let page = PageState::unknown(None);
        assert!(wrapped.observe().is_ok());
        assert_eq!(wrapped.focused(), Some(id.clone()));
        assert!(wrapped.page().is_some());
        assert!(wrapped.dispatch(&id, &Input::Click).is_ok());
        assert!(wrapped.scroll(ScrollDirection::Down).is_ok());
        wrapped.settle();
        assert!(wrapped.last_observation().is_some());
        wrapped.pause(0);
        wrapped.settle_after_input(&id, &Input::Click);
        assert!(wrapped.read_value(&id).is_some());
        assert_eq!(wrapped.selected_text().as_deref(), Some("sel"));
        let _ = wrapped.page_delta_between(&page, &page);
        assert!(wrapped.screenshot().is_some());
        assert!(wrapped.page_view().is_some());
        assert_eq!(checked, 1, "the check ran once, before the dispatch");
        // The dispatch read the last observation for the check first.
        let mut expected = RUNTIME_METHODS.to_vec();
        expected.insert(3, "last_observation");
        assert_eq!(*inner.log.borrow(), expected);
    }

    #[test]
    fn a_refused_check_sends_nothing() {
        let mut inner = recorder();
        let mut wrapped = BeforeDispatch {
            inner: &mut inner,
            before: |_: Option<&InteractionManifold>, _: &RegionId, _: &Input| {
                Err(AgentError::InputRejected("no".into()))
            },
        };
        let id = RegionId::try_new("f").unwrap();
        assert!(wrapped.dispatch(&id, &Input::Click).is_err());
        assert!(!inner.log.borrow().contains(&"dispatch"));
    }
}
