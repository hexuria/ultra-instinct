use ultra_instinct_browser::PageDelta;
use ultra_instinct_core::InteractionManifold;
use ultra_instinct_observe::diff;

use crate::outcome::VerificationKind;
use crate::runtime::FieldValue;

/// Classify post-action observation into a verification kind.
///
/// URL change → navigation; manifold delta → state-changed; title / focus
/// only → state-changed; a region whose observed control state (checked,
/// expanded, value, selected, options) changed → state-changed; nothing →
/// no visible change. Control state stays out of the region fingerprint
/// (tickets never go stale on it); it is read here as evidence only.
pub fn classify_delta(
    before_manifold: &InteractionManifold,
    after_manifold: &InteractionManifold,
    page: Option<&PageDelta>,
) -> VerificationKind {
    if let Some(p) = page {
        if p.url_changed() {
            return VerificationKind::Navigation;
        }
    }
    let delta = diff(before_manifold, after_manifold);
    if delta.is_empty() {
        if page.is_some_and(|p| p.title_changed() || p.focus_changed())
            || control_state_changed(before_manifold, after_manifold)
        {
            return VerificationKind::StateChanged;
        }
        // Either side's URL/title was unreadable: the page may have
        // navigated where we could not see it — not a no-effect.
        if page.is_some_and(|p| !p.is_known()) {
            return VerificationKind::Unknown;
        }
        return VerificationKind::NoVisibleChange;
    }
    VerificationKind::StateChanged
}

/// Some region present in both observations reports different control state.
fn control_state_changed(before: &InteractionManifold, after: &InteractionManifold) -> bool {
    before.regions().any(|region| {
        after
            .get(region.id())
            .is_some_and(|later| later.state() != region.state())
    })
}

/// TYPE_TEXT / SELECT postcondition from the field's value read back after
/// input. `None` when the runtime could not read it (fall back to the diff).
pub fn classify_value(expected: &str, value: Option<&FieldValue>) -> Option<VerificationKind> {
    let value = value?;
    Some(if value.matches(expected) {
        VerificationKind::Success
    } else {
        VerificationKind::WrongEffect
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{parse_fixture, ElementState};

    fn page() -> InteractionManifold {
        parse_fixture(
            r#"
            viewport w=800 h=600
            region id=box role=checkbox label="Remember me" x=10 y=10 w=20 h=20 actions=click sources=dom,accessibility
            region id=go role=button label="Go" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
            "#,
        )
        .unwrap()
    }

    fn with(manifold: &InteractionManifold, id: &str, state: ElementState) -> InteractionManifold {
        let mut next = manifold.clone();
        let region = next.get_str(id).unwrap().clone().with_state(state);
        next.replace(region);
        next
    }

    fn checked(on: bool) -> ElementState {
        ElementState {
            checked: Some(on),
            ..ElementState::default()
        }
    }

    #[test]
    fn unchanged_observation_is_no_visible_change() {
        let before = with(&page(), "box", checked(false));
        assert_eq!(
            classify_delta(&before, &before.clone(), None),
            VerificationKind::NoVisibleChange
        );
    }

    #[test]
    fn toggled_checkbox_is_state_changed_without_a_fingerprint_change() {
        let before = with(&page(), "box", checked(false));
        let after = with(&before, "box", checked(true));
        assert!(diff(&before, &after).is_empty(), "state is not fingerprint");
        assert_eq!(
            classify_delta(&before, &after, None),
            VerificationKind::StateChanged
        );
    }

    #[test]
    fn expanded_or_value_change_on_another_region_counts() {
        let before = page();
        for state in [
            ElementState {
                expanded: Some(true),
                ..ElementState::default()
            },
            ElementState {
                value: Some("x".to_owned()),
                ..ElementState::default()
            },
        ] {
            let after = with(&before, "go", state);
            assert_eq!(
                classify_delta(&before, &after, None),
                VerificationKind::StateChanged
            );
        }
    }
}
