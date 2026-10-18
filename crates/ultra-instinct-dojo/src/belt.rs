//! Belt exams (issue #49, work item 6): how proven a learned pairing is.
//!
//! A pairing (context, label) earns belts only on verified wins and is
//! demoted by losses — the grade is a pure function of the trust record,
//! so repeated misses pull a belt down without any bookkeeping of their
//! own. Belts are integers of experience, never confidence: they order
//! which learned move replays first and feed the exam report, nothing
//! more.

use std::collections::BTreeMap;

use crate::lessons::{LessonStore, Move};

/// Belt ranks, lowest to highest. `Ord` compares proven-ness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Belt {
    /// Seen working once, or too many losses to hold a higher grade.
    #[default]
    White,
    /// At least 2 net wins at ≥60%.
    Orange,
    /// At least 5 net wins at ≥75%.
    Blue,
    /// At least 10 net wins at ≥90%.
    Ultra,
}

impl Belt {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Orange => "orange",
            Self::Blue => "blue",
            Self::Ultra => "ultra",
        }
    }

    /// Grade a pairing. Deterministic, total, and demoting: every loss
    /// drags both the net count and the percentage back toward white.
    pub fn of(wins: u32, losses: u32) -> Belt {
        let total = wins.saturating_add(losses);
        if total == 0 {
            return Belt::White;
        }
        let net = wins.saturating_sub(losses);
        let pct = u64::from(wins) * 100 / u64::from(total);
        if net >= 10 && pct >= 90 {
            Belt::Ultra
        } else if net >= 5 && pct >= 75 {
            Belt::Blue
        } else if net >= 2 && pct >= 60 {
            Belt::Orange
        } else {
            Belt::White
        }
    }
}

/// The belt a learned move holds: the grade of the trust record for its
/// first step's label in this situation (a routine is only as proven as
/// its opening action).
pub fn move_belt(store: &LessonStore, key: &str, mv: &Move) -> Belt {
    let Some(step) = mv.steps.first() else {
        return Belt::White;
    };
    store
        .trust
        .get(key)
        .and_then(|t| t.get(&step.label))
        .map_or(Belt::White, |t| Belt::of(t.wins, t.losses))
}

/// Every pairing's belt, keyed like `store.trust`.
pub fn label_belts(store: &LessonStore) -> BTreeMap<String, BTreeMap<String, Belt>> {
    store
        .trust
        .iter()
        .map(|(key, labels)| {
            (
                key.clone(),
                labels
                    .iter()
                    .map(|(label, t)| (label.clone(), Belt::of(t.wins, t.losses)))
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn belts_climb_on_wins() {
        assert_eq!(Belt::of(0, 0), Belt::White);
        assert_eq!(Belt::of(1, 0), Belt::White);
        assert_eq!(Belt::of(2, 0), Belt::Orange);
        assert_eq!(Belt::of(5, 0), Belt::Blue);
        assert_eq!(Belt::of(10, 0), Belt::Ultra);
        assert!(
            Belt::White < Belt::Orange && Belt::Orange < Belt::Blue && Belt::Blue < Belt::Ultra
        );
    }

    #[test]
    fn losses_demote() {
        assert_eq!(Belt::of(10, 9), Belt::White); // 52% — collapses to white
        assert_eq!(Belt::of(5, 3), Belt::Orange); // 62%, net 2 — blue lost
        assert_eq!(Belt::of(11, 2), Belt::Blue); // 84%, net 9 — ultra lost
    }
}
