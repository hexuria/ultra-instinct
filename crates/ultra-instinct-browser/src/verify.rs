//! Postconditions on a manifold. This does not click.
//! [`verify`] checks one snapshot. [`verify_delta`] checks a diff the host
//! already computed with `ultra-instinct-observe`, plus a page delta.

use std::fmt;

use ultra_instinct_core::{token_recall, tokenize, InteractionManifold, RegionId};
use ultra_instinct_observe::ManifoldDiff;

use crate::page::PageDelta;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expectation {
    TextPresent(String),
    RegionAbsent(RegionId),
    Appeared(RegionId),
    Disappeared(RegionId),
    UrlChanged,
}

impl Expectation {
    pub fn text_present(text: impl Into<String>) -> Result<Self, VerifyError> {
        let text = text.into();
        if tokenize(&text).is_empty() {
            return Err(VerifyError::EmptyExpectation);
        }
        Ok(Self::TextPresent(text))
    }

    pub fn region_absent(id: RegionId) -> Self {
        Self::RegionAbsent(id)
    }

    pub fn appeared(id: RegionId) -> Self {
        Self::Appeared(id)
    }

    pub fn disappeared(id: RegionId) -> Self {
        Self::Disappeared(id)
    }

    pub fn url_changed() -> Self {
        Self::UrlChanged
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum VerifyError {
    ExpectedTextMissing {
        expected: String,
    },
    RegionStillPresent {
        id: String,
    },
    EmptyExpectation,
    /// The diff is empty and the known page state is unchanged.
    NoEffect,
    RegionDidNotAppear {
        id: String,
    },
    RegionDidNotDisappear {
        id: String,
    },
    UrlUnchanged,
    /// [`Expectation::TextPresent`] and [`Expectation::RegionAbsent`] are snapshot checks.
    NotADeltaExpectation,
    /// [`Expectation::Appeared`], [`Expectation::Disappeared`], and [`Expectation::UrlChanged`] need [`verify_delta`].
    NeedsDelta,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedTextMissing { expected } => {
                write!(f, "expected text `{expected}` did not appear")
            }
            Self::RegionStillPresent { id } => write!(f, "region `{id}` is still present"),
            Self::EmptyExpectation => {
                f.write_str("expected text must contain at least one alphanumeric token")
            }
            Self::NoEffect => f.write_str("act changed nothing"),
            Self::RegionDidNotAppear { id } => write!(f, "region `{id}` did not appear"),
            Self::RegionDidNotDisappear { id } => write!(f, "region `{id}` did not disappear"),
            Self::UrlUnchanged => f.write_str("url did not change"),
            Self::NotADeltaExpectation => {
                f.write_str("text and region expectations are checked on a snapshot, not a delta")
            }
            Self::NeedsDelta => {
                f.write_str("appeared, disappeared, and url changes are checked with verify_delta")
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// Succeeds when every token of the expected text is present in some region
/// label, or when the named region id is gone.
pub fn verify(
    manifold: &InteractionManifold,
    expectation: &Expectation,
) -> Result<(), VerifyError> {
    match expectation {
        Expectation::TextPresent(expected) => {
            let found = manifold
                .regions()
                .any(|region| token_recall(expected, region.label()) == 1.0);
            if found {
                Ok(())
            } else {
                Err(VerifyError::ExpectedTextMissing {
                    expected: expected.clone(),
                })
            }
        }
        Expectation::RegionAbsent(id) => {
            if manifold.get(id).is_none() {
                Ok(())
            } else {
                Err(VerifyError::RegionStillPresent { id: id.to_string() })
            }
        }
        Expectation::Appeared(_) | Expectation::Disappeared(_) | Expectation::UrlChanged => {
            Err(VerifyError::NeedsDelta)
        }
    }
}

/// Delta postcondition. [`VerifyError::NoEffect`] when nothing in the region
/// diff or the page state changed and both page states are known, before the
/// specific expectation is checked. With an unknown page state an empty diff is
/// not called no-effect: the URL may have changed unseen.
pub fn verify_delta(
    regions: &ManifoldDiff,
    page: &PageDelta,
    expectation: &Expectation,
) -> Result<(), VerifyError> {
    if regions.is_empty() && page.is_unchanged() && page.is_known() {
        return Err(VerifyError::NoEffect);
    }
    match expectation {
        Expectation::Appeared(id) => {
            if regions.added().iter().any(|added| added == id) {
                Ok(())
            } else {
                Err(VerifyError::RegionDidNotAppear { id: id.to_string() })
            }
        }
        Expectation::Disappeared(id) => {
            if regions.removed().iter().any(|removed| removed == id) {
                Ok(())
            } else {
                Err(VerifyError::RegionDidNotDisappear { id: id.to_string() })
            }
        }
        Expectation::UrlChanged => {
            if page.url_changed() {
                Ok(())
            } else {
                Err(VerifyError::UrlUnchanged)
            }
        }
        Expectation::TextPresent(_) | Expectation::RegionAbsent(_) => {
            Err(VerifyError::NotADeltaExpectation)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        Action, InteractionRegion, Rect, RegionFlags, RegionParts, Role, SourceMask, UnitInterval,
    };
    use ultra_instinct_observe::diff;

    use crate::page::{page_delta, PageState};

    fn manifold(label: &str, id: &str) -> InteractionManifold {
        let region = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Heading,
            label: label.into(),
            rect: Rect::try_new(10.0, 10.0, 40.0, 20.0).unwrap(),
            actions: vec![Action::Focus],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap(),
            vec![region],
            0,
        )
        .unwrap()
    }

    #[test]
    fn missing_text_is_the_exact_error_and_present_text_succeeds() {
        let sign_in = manifold("Sign in", "n100");
        let err = verify(&sign_in, &Expectation::text_present("Welcome").unwrap()).unwrap_err();
        assert_eq!(
            err,
            VerifyError::ExpectedTextMissing {
                expected: "Welcome".into(),
            }
        );
        assert_eq!(err.to_string(), "expected text `Welcome` did not appear");

        let welcome = manifold("Welcome back", "n2");
        assert!(verify(&welcome, &Expectation::text_present("Welcome").unwrap()).is_ok());

        let err = Expectation::text_present("...").unwrap_err();
        assert_eq!(err, VerifyError::EmptyExpectation);
    }

    #[test]
    fn region_that_remains_is_the_exact_error() {
        let sign_in = manifold("Sign in", "n100");
        let err = verify(
            &sign_in,
            &Expectation::region_absent(RegionId::try_new("n100").unwrap()),
        )
        .unwrap_err();
        assert_eq!(err, VerifyError::RegionStillPresent { id: "n100".into() });
        assert_eq!(err.to_string(), "region `n100` is still present");
        assert!(verify(
            &sign_in,
            &Expectation::region_absent(RegionId::try_new("gone").unwrap()),
        )
        .is_ok());
    }

    #[test]
    fn executed_act_with_no_delta_is_no_effect() {
        let before = manifold("Sign in", "n100");
        let regions = diff(&before, &before);
        assert!(regions.is_empty());
        let known = PageState::new("https://example.test/", "Example", None);
        let page = page_delta(&known, &known);
        assert!(page.is_unchanged());
        let err = verify_delta(
            &regions,
            &page,
            &Expectation::appeared(RegionId::try_new("n300").unwrap()),
        )
        .unwrap_err();
        assert_eq!(err, VerifyError::NoEffect);
        assert_eq!(err.to_string(), "act changed nothing");
    }

    #[test]
    fn delta_expectations_are_exact_errors() {
        let before = manifold("Sign in", "n100");
        let after = manifold("Welcome", "n300");
        let regions = diff(&before, &after);
        let page = page_delta(&PageState::blank(), &PageState::blank());
        let n100 = RegionId::try_new("n100").unwrap();
        let n300 = RegionId::try_new("n300").unwrap();
        assert_eq!(
            verify_delta(&regions, &page, &Expectation::appeared(n100.clone())),
            Err(VerifyError::RegionDidNotAppear { id: "n100".into() })
        );
        assert_eq!(
            verify_delta(&regions, &page, &Expectation::disappeared(n300.clone())),
            Err(VerifyError::RegionDidNotDisappear { id: "n300".into() })
        );
        assert_eq!(
            verify_delta(&regions, &page, &Expectation::url_changed()),
            Err(VerifyError::UrlUnchanged)
        );
        assert_eq!(
            verify_delta(
                &regions,
                &page,
                &Expectation::text_present("Welcome").unwrap()
            ),
            Err(VerifyError::NotADeltaExpectation)
        );
        assert_eq!(
            verify_delta(&regions, &page, &Expectation::appeared(n300.clone())),
            Ok(())
        );
        assert_eq!(
            verify_delta(&regions, &page, &Expectation::disappeared(n100)),
            Ok(())
        );
        assert_eq!(
            verify(&after, &Expectation::appeared(n300)),
            Err(VerifyError::NeedsDelta)
        );
        assert_eq!(
            verify(&after, &Expectation::url_changed()),
            Err(VerifyError::NeedsDelta)
        );
        assert_eq!(VerifyError::UrlUnchanged.to_string(), "url did not change");
    }

    #[test]
    fn unknown_page_state_is_never_no_effect() {
        let before = manifold("Sign in", "n100");
        let regions = diff(&before, &before);
        let known = PageState::new("https://example.test/", "Example", None);
        for (left, right) in [
            (PageState::blank(), PageState::blank()),
            (PageState::blank(), known.clone()),
            (known.clone(), PageState::blank()),
        ] {
            let page = page_delta(&left, &right);
            assert_eq!(
                verify_delta(&regions, &page, &Expectation::url_changed()),
                Err(VerifyError::UrlUnchanged),
                "{left:?} {right:?}"
            );
        }
    }
}
