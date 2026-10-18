//! Page-level facts from one observation.
//!
//! `url` and `title` come from `Page.getNavigationHistory`. `None` means the
//! page state is unknown: the call was a CDP protocol error, the history had no
//! current entry, or the source is not a CDP page. Unknown is not the empty
//! string. `focused` is the stable id of the accessibility node whose `focused`
//! property is true. The history payload has no timestamp, so this does not set
//! `captured_at_ms`.

use ultra_instinct_core::RegionId;

/// URL, title, and focused region after one observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageState {
    url: Option<String>,
    title: Option<String>,
    focused: Option<RegionId>,
}

impl PageState {
    /// A known URL and title.
    pub fn new(
        url: impl Into<String>,
        title: impl Into<String>,
        focused: Option<RegionId>,
    ) -> Self {
        Self {
            url: Some(url.into()),
            title: Some(title.into()),
            focused,
        }
    }

    /// URL and title are unknown. Focus may still be known from the AX tree.
    pub fn unknown(focused: Option<RegionId>) -> Self {
        Self {
            url: None,
            title: None,
            focused,
        }
    }

    /// No URL, title, or focus. Used when the source is not a CDP page.
    pub fn blank() -> Self {
        Self::unknown(None)
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn focused(&self) -> Option<&RegionId> {
        self.focused.as_ref()
    }

    /// Both URL and title were read.
    pub fn is_known(&self) -> bool {
        self.url.is_some() && self.title.is_some()
    }
}

/// What changed between two [`PageState`] values. Not a region diff.
///
/// A URL or title change is reported only when both sides know it. If either
/// side is unknown, the change is `false` and [`PageDelta::is_known`] is
/// `false`, so an unknown state never claims a change and never claims no
/// effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageDelta {
    url_changed: bool,
    title_changed: bool,
    focus_changed: bool,
    known: bool,
}

impl PageDelta {
    pub const fn url_changed(self) -> bool {
        self.url_changed
    }

    pub const fn title_changed(self) -> bool {
        self.title_changed
    }

    pub const fn focus_changed(self) -> bool {
        self.focus_changed
    }

    /// Both page states had a URL and a title.
    pub const fn is_known(self) -> bool {
        self.known
    }

    pub const fn is_unchanged(self) -> bool {
        !self.url_changed && !self.title_changed && !self.focus_changed
    }
}

fn changed(before: Option<&str>, after: Option<&str>) -> bool {
    matches!((before, after), (Some(left), Some(right)) if left != right)
}

pub fn page_delta(before: &PageState, after: &PageState) -> PageDelta {
    PageDelta {
        url_changed: changed(before.url(), after.url()),
        title_changed: changed(before.title(), after.title()),
        focus_changed: before.focused != after.focused,
        known: before.is_known() && after.is_known(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_on_either_side_is_never_a_change_and_never_known() {
        let unknown = PageState::blank();
        let known = PageState::new("https://example.test/a", "A", None);
        let other = PageState::new("https://example.test/b", "B", None);
        for (before, after) in [(&unknown, &unknown), (&unknown, &known), (&known, &unknown)] {
            let delta = page_delta(before, after);
            assert!(!delta.url_changed(), "{before:?} {after:?}");
            assert!(!delta.title_changed(), "{before:?} {after:?}");
            assert!(!delta.is_known(), "{before:?} {after:?}");
        }
        let delta = page_delta(&known, &other);
        assert!(delta.url_changed() && delta.title_changed() && delta.is_known());
        let delta = page_delta(&known, &known);
        assert!(delta.is_unchanged() && delta.is_known());
        // An empty URL is a known value, not unknown.
        let empty = PageState::new("", "", None);
        assert!(page_delta(&empty, &known).url_changed());
    }
}
