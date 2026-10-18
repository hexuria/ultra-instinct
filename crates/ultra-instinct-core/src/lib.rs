//! Core types for ultra-instinct.
//!
//! A caller builds an [`InteractionManifold`] of [`InteractionRegion`] values
//! and may derive a finite [`ActionSpace`] of [`ObservedAction`]s for a policy
//! to choose from. Ranking / Instinct / execution live in other crates. There is no
//! `unsafe` and no shared mutable state.

#![forbid(unsafe_code)]

mod action_space;
mod element_state;
mod error;
mod fixture;
mod id;
mod manifold;
mod query;
mod rect;
mod region;
mod text;
mod vocab;

pub use action_space::{
    cell_center_fraction, ActionId, ActionKind, ActionSpace, DragDest, ObservedAction, KEY_NAMES,
    SURFACE_GRID,
};
pub use element_state::ElementState;
pub use error::{CoreError, FixtureError};
pub use fixture::{parse_fixture, write_fixture};
pub use id::{RegionId, StateFingerprint, UnitInterval};
pub use manifold::InteractionManifold;
pub use query::LocateQuery;
pub use rect::{Point, Rect};
pub use region::{InteractionRegion, RegionParts};
pub use text::{token_jaccard, token_precision, token_recall, tokenize};
pub use vocab::{Action, RegionFlags, Relation, Role, SizeClass, SourceMask, Zone};

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: &str, label: &str, y: f64) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.to_owned(),
            rect: Rect::try_new(10.0, y, 40.0, 20.0).unwrap(),
            actions: vec![Action::Click, Action::Focus, Action::Click],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    #[test]
    fn rejects_empty_and_whitespace_ids() {
        assert_eq!(RegionId::try_new(""), Err(CoreError::EmptyId));
        assert_eq!(RegionId::try_new("nav settings"), Err(CoreError::InvalidId));
        assert_eq!(
            RegionId::try_new("").unwrap_err().to_string(),
            "region id must not be empty"
        );
    }

    #[test]
    fn rejects_bad_rects_and_zero_viewport() {
        assert_eq!(
            Rect::try_new(0.0, 0.0, -1.0, 2.0),
            Err(CoreError::NegativeExtent)
        );
        assert_eq!(
            Rect::try_new(f64::NAN, 0.0, 1.0, 1.0),
            Err(CoreError::NonFiniteCoordinate)
        );
        assert_eq!(
            Rect::try_viewport(0.0, 0.0, 0.0, 10.0),
            Err(CoreError::NonPositiveViewport)
        );
        assert!(Rect::try_new(0.0, 0.0, 0.0, 0.0).unwrap().is_zero_area());
    }

    #[test]
    fn stability_out_of_range_is_an_error() {
        assert_eq!(
            UnitInterval::try_new(1.1),
            Err(CoreError::StabilityOutOfRange)
        );
        assert_eq!(
            UnitInterval::try_new(f64::NAN),
            Err(CoreError::StabilityOutOfRange)
        );
        assert_eq!(
            UnitInterval::try_new(1.1).unwrap_err().to_string(),
            "temporal stability must be a finite value in [0, 1]"
        );
    }

    #[test]
    fn unknown_source_bits_are_rejected() {
        assert_eq!(
            SourceMask::try_from_bits(0b1_0000),
            Err(CoreError::UnknownSourceBits(0b1_0000))
        );
    }

    #[test]
    fn actions_are_sorted_and_deduped_and_fingerprint_is_stable() {
        let region = region("ok", "Settings", 4.0);
        assert_eq!(region.actions(), &[Action::Click, Action::Focus]);
        let again = region.fingerprint();
        let rebuilt = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("ok").unwrap(),
            role: Role::Button,
            label: "Settings".to_owned(),
            rect: Rect::try_new(10.0, 4.0, 40.0, 20.0).unwrap(),
            actions: vec![Action::Focus, Action::Click],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        assert_eq!(again, rebuilt.fingerprint());
        assert_ne!(region.fingerprint(), region_changed_label().fingerprint());
    }

    #[test]
    fn element_state_does_not_change_the_region_fingerprint() {
        let region = region("field", "Name", 4.0);
        let fingerprint = region.fingerprint();
        let state = ElementState {
            value: Some("Ana".to_owned()),
            ..ElementState::default()
        };

        let region = region.with_state(state.clone());

        assert_eq!(region.fingerprint(), fingerprint);
        assert_eq!(region.state(), &state);
    }

    fn region_changed_label() -> InteractionRegion {
        region("ok", "Profile", 4.0)
    }

    #[test]
    fn duplicate_region_id_is_rejected() {
        let viewport = Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap();
        let err = InteractionManifold::try_new(
            viewport,
            vec![region("a", "A", 0.0), region("a", "B", 10.0)],
            0,
        )
        .unwrap_err();
        assert_eq!(err, CoreError::DuplicateRegion("a".to_owned()));
        assert_eq!(err.to_string(), "duplicate region id `a`");
    }

    #[test]
    fn regions_iterate_in_id_order() {
        let viewport = Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap();
        let manifold = InteractionManifold::try_new(
            viewport,
            vec![region("b", "B", 0.0), region("a", "A", 10.0)],
            5,
        )
        .unwrap();
        let ids: Vec<_> = manifold.ids().map(RegionId::as_str).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(manifold.captured_at_ms(), 5);
    }

    #[test]
    fn tokenize_and_recall() {
        assert_eq!(tokenize("Open Settings!"), vec!["open", "settings"]);
        assert_eq!(token_recall("Settings", "Account Settings"), 1.0);
        assert_eq!(token_recall("Settings panel", "Settings"), 0.5);
        assert_eq!(token_jaccard("", ""), 1.0);
    }

    #[test]
    fn token_precision_counts_label_tokens_found_in_the_query() {
        assert_eq!(token_precision("Send", "Send"), 1.0);
        assert_eq!(token_precision("Send", "Send feedback"), 0.5);
        assert_eq!(token_precision("Send", "Send to device"), 1.0 / 3.0);
        assert_eq!(token_precision("Send", ""), 0.0);
        assert_eq!(token_precision("", ""), 1.0);
        assert_eq!(token_precision("Account Settings", "Settings"), 1.0);
    }

    #[test]
    fn fixture_parses_and_reports_line_errors() {
        let manifold = parse_fixture(
            r#"
            # comment
            viewport w=200 h=100
            region id=nav-settings role=button label="Settings" x=1 y=2 w=3 h=4 actions=click sources=dom,accessibility
            "#,
        )
        .unwrap();
        assert_eq!(manifold.len(), 1);
        assert_eq!(manifold.viewport().width(), 200.0);
        let err = parse_fixture("region id=a role=button label=A x=0 y=0 w=1 h=1\n").unwrap_err();
        assert_eq!(err, FixtureError::MissingViewport);
        assert_eq!(err.to_string(), "fixture is missing a viewport directive");
        let err = parse_fixture("viewport w=10 h=10\nnope\n").unwrap_err();
        assert_eq!(
            err,
            FixtureError::Line {
                line: 2,
                message: "unknown directive `nope`".to_owned(),
            }
        );
        let err = parse_fixture("viewport w=0 h=10\n").unwrap_err();
        match err {
            FixtureError::Line { line: 1, message } => {
                assert!(message.contains("positive"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
        let err = parse_fixture("viewport w=10 h=10\nviewport w=2 h=2\n").unwrap_err();
        assert_eq!(err, FixtureError::DuplicateViewport { line: 2 });
        assert_eq!(err.to_string(), "fixture line 2: duplicate viewport");
        let err = parse_fixture(
            "viewport w=10 h=10\nregion id=a role=button label=A x=0 y=0 w=1 h=1\nregion id=a role=button label=B x=1 y=1 w=1 h=1\n",
        )
        .unwrap_err();
        assert_eq!(
            err,
            FixtureError::DuplicateRegion {
                line: 3,
                id: "a".into(),
            }
        );
        let err = parse_fixture("viewport w=10 h=10\nregion id=a label=\"open\n").unwrap_err();
        assert_eq!(
            err,
            FixtureError::Line {
                line: 2,
                message: "unclosed quote".into(),
            }
        );
    }

    #[test]
    fn fixture_roundtrip_preserves_the_manifold() {
        let original = parse_fixture(include_str!("../../../fixtures/sidebar.manifold")).unwrap();
        let written = write_fixture(&original).unwrap();
        let again = parse_fixture(&written).unwrap();
        assert_eq!(original, again);

        let quoted = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("quote").unwrap(),
            role: Role::Button,
            label: "Say \"hi\"".into(),
            rect: Rect::try_new(1.5, 2.0, 3.0, 4.0).unwrap(),
            actions: vec![Action::Type, Action::Click],
            parent: Some(RegionId::try_new("nav").unwrap()),
            sources: SourceMask::DOM.union(SourceMask::CUA),
            flags: {
                let mut flags = RegionFlags::none();
                flags.set_disabled(true);
                flags.set_stale(true);
                flags
            },
            temporal_stability: UnitInterval::try_new(0.5).unwrap(),
        })
        .unwrap();
        let parent = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("nav").unwrap(),
            role: Role::Navigation,
            label: "Sidebar".into(),
            rect: Rect::try_new(0.0, 0.0, 10.0, 10.0).unwrap(),
            actions: vec![],
            parent: None,
            sources: SourceMask::NONE,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let viewport = Rect::try_viewport(0.0, 0.0, 100.0, 80.0).unwrap();
        let manifold = InteractionManifold::try_new(viewport, vec![quoted, parent], 0).unwrap();
        let parsed = parse_fixture(&write_fixture(&manifold).unwrap()).unwrap();
        assert_eq!(parsed, manifold);
        assert_eq!(parsed.get_str("quote").unwrap().label(), "Say \"hi\"");

        // strip_comment must honor \-escapes inside quotes: a label holding
        // both a quote and a '#' truncated the written line mid-token and
        // could not re-parse (found by the `fixture` fuzz target).
        let hash = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("hash").unwrap(),
            role: Role::Button,
            label: "a\"#b".into(),
            rect: Rect::try_new(0.0, 0.0, 1.0, 1.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let manifold = InteractionManifold::try_new(viewport, vec![hash], 0).unwrap();
        let written = write_fixture(&manifold).unwrap();
        let parsed = parse_fixture(&written).unwrap();
        assert_eq!(parsed, manifold);
        assert_eq!(parsed.get_str("hash").unwrap().label(), "a\"#b");
        // The same shape hand-written with a trailing comment still parses.
        let parsed = parse_fixture(
            "viewport w=10 h=10\nregion id=a role=button label=\"x\\\"#y\" x=0 y=0 w=1 h=1 # tail\n",
        )
        .unwrap();
        assert_eq!(parsed.get_str("a").unwrap().label(), "x\"#y");

        let shifted = InteractionManifold::try_new(
            Rect::try_viewport(4.0, 0.0, 100.0, 80.0).unwrap(),
            vec![region("only", "Only", 1.0)],
            0,
        )
        .unwrap();
        let err = write_fixture(&shifted).unwrap_err();
        assert_eq!(err, FixtureError::UnsupportedViewportOrigin);
        assert_eq!(
            err.to_string(),
            "fixture format cannot represent a viewport whose origin is not (0, 0)"
        );
    }

    #[test]
    fn empty_locate_text_is_rejected() {
        let err = LocateQuery::new().text("...").unwrap_err();
        assert_eq!(err, CoreError::EmptyQueryText);
        assert_eq!(
            err.to_string(),
            "locate text must contain at least one alphanumeric token"
        );
        let query = LocateQuery::new()
            .text("Settings")
            .unwrap()
            .role(Role::Button)
            .position(Zone::Left)
            .action(Action::Click);
        assert_eq!(query.text_ref(), Some("Settings"));
        assert_eq!(query.role_ref(), Some(Role::Button));
        assert_eq!(query.position_ref(), Some(Zone::Left));
        assert_eq!(query.action_ref(), Some(Action::Click));
    }

    #[test]
    fn point_rejects_non_finite() {
        assert_eq!(
            Point::try_new(f64::INFINITY, 0.0),
            Err(CoreError::NonFiniteCoordinate)
        );
    }
}
