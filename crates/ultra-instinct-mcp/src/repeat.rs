//! Caller-level repeat detection for `locate`.
//!
//! `detect` in `ultra-instinct-observe` compares page snapshots around an act. It
//! cannot see a caller that keeps asking the same question of a page that has
//! not changed. [`LocateRepeats`] can: it remembers the last
//! [`REPEAT_WINDOW`] locate calls as (origin, page signature, normalized
//! query) and counts how many of them match the current one.
//!
//! The threshold is [`REPEAT_THRESHOLD`] = 2. Locate is deterministic: the
//! same query on the same signature returns the same ranking, so the first
//! repeat is already a call that could not have told the caller anything new.
//! Waiting for a third would only let one more empty call through. The signal
//! is data. Locate still ranks and returns as before; nothing is retried or
//! refused.

use std::collections::VecDeque;

use ultra_instinct_core::{tokenize, LocateQuery};
use ultra_instinct_observe::history::StateSignature;

/// Identical locate calls (same origin, signature, and query) at which the
/// result carries `repeated_query`. The first call is 1.
pub const REPEAT_THRESHOLD: usize = 2;

/// Locate calls remembered across all origins. Older calls drop off.
pub const REPEAT_WINDOW: usize = 8;

/// A locate query with presentation noise removed: text becomes its
/// lower-case token list, so "Send", "send", and " Send! " are one query.
/// Role, position, action, matcher, and dims are kept as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueryKey {
    text: Vec<String>,
    role: Option<&'static str>,
    position: Option<&'static str>,
    action: Option<&'static str>,
    within: Option<String>,
    near: Option<String>,
    matcher: String,
    dims: Option<u64>,
}

impl QueryKey {
    pub(crate) fn new(query: &LocateQuery, matcher: &str, dims: Option<u64>) -> Self {
        Self {
            text: query.text_ref().map(tokenize).unwrap_or_default(),
            role: query.role_ref().map(|role| role.as_str()),
            position: query.position_ref().map(|zone| zone.as_str()),
            action: query.action_ref().map(|action| action.as_str()),
            within: query.within_ref().map(|id| id.as_str().to_owned()),
            near: query.near_ref().map(|id| id.as_str().to_owned()),
            matcher: matcher.to_owned(),
            dims,
        }
    }
}

struct Seen {
    origin: String,
    signature: StateSignature,
    query: QueryKey,
}

/// The last [`REPEAT_WINDOW`] locate calls.
#[derive(Default)]
pub(crate) struct LocateRepeats {
    recent: VecDeque<Seen>,
}

impl LocateRepeats {
    /// Record one locate call and return how many remembered calls, this one
    /// included, have the same origin, signature, and query.
    pub(crate) fn record(
        &mut self,
        origin: &str,
        signature: StateSignature,
        query: QueryKey,
    ) -> usize {
        let earlier = self
            .recent
            .iter()
            .filter(|seen| {
                seen.origin == origin && seen.signature == signature && seen.query == query
            })
            .count();
        if self.recent.len() == REPEAT_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(Seen {
            origin: origin.to_owned(),
            signature,
            query,
        });
        earlier + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{parse_fixture, Role, Zone};

    fn signature(fixture: &str) -> StateSignature {
        StateSignature::from_manifold(&parse_fixture(fixture).unwrap())
    }

    const ONE: &str = "viewport w=100 h=100\nregion id=a role=button label=\"Send\" x=0 y=0 w=10 h=10 actions=click sources=dom\n";
    const TWO: &str = "viewport w=100 h=100\nregion id=a role=button label=\"Sent\" x=0 y=0 w=10 h=10 actions=click sources=dom\n";

    fn send() -> QueryKey {
        QueryKey::new(
            &LocateQuery::new().text("Send").unwrap().role(Role::Button),
            "weighted",
            None,
        )
    }

    #[test]
    fn same_origin_signature_and_query_count_up() {
        let mut repeats = LocateRepeats::default();
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 1);
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 2);
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 3);
        assert_eq!(REPEAT_THRESHOLD, 2);
    }

    #[test]
    fn text_is_normalized_but_every_other_part_counts() {
        let mut repeats = LocateRepeats::default();
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 1);
        let noisy = QueryKey::new(
            &LocateQuery::new()
                .text(" send! ")
                .unwrap()
                .role(Role::Button),
            "weighted",
            None,
        );
        assert_eq!(repeats.record("cdp:x", signature(ONE), noisy), 2);
        let positioned = QueryKey::new(
            &LocateQuery::new()
                .text("Send")
                .unwrap()
                .role(Role::Button)
                .position(Zone::Left),
            "weighted",
            None,
        );
        assert_eq!(repeats.record("cdp:x", signature(ONE), positioned), 1);
        let link = QueryKey::new(
            &LocateQuery::new().text("Send").unwrap().role(Role::Link),
            "weighted",
            None,
        );
        assert_eq!(repeats.record("cdp:x", signature(ONE), link), 1);
        let hgra = QueryKey::new(
            &LocateQuery::new().text("Send").unwrap().role(Role::Button),
            "hgra",
            Some(512),
        );
        assert_eq!(repeats.record("cdp:x", signature(ONE), hgra), 1);
        let action = QueryKey::new(
            &LocateQuery::new()
                .text("Send")
                .unwrap()
                .role(Role::Button)
                .action(ultra_instinct_core::Action::Click),
            "weighted",
            None,
        );
        assert_eq!(repeats.record("cdp:x", signature(ONE), action), 1);
    }

    #[test]
    fn a_changed_page_or_another_origin_starts_over() {
        let mut repeats = LocateRepeats::default();
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 1);
        assert_eq!(repeats.record("cdp:x", signature(TWO), send()), 1);
        assert_eq!(repeats.record("cdp:y", signature(ONE), send()), 1);
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 2);
    }

    #[test]
    fn calls_older_than_the_window_are_forgotten() {
        let mut repeats = LocateRepeats::default();
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 1);
        for index in 0..REPEAT_WINDOW - 1 {
            repeats.record(&format!("cdp:other{index}"), signature(ONE), send());
        }
        // The first call is still the oldest of the last eight.
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 2);
        for index in 0..REPEAT_WINDOW {
            repeats.record(&format!("cdp:fill{index}"), signature(ONE), send());
        }
        assert_eq!(repeats.record("cdp:x", signature(ONE), send()), 1);
        assert_eq!(REPEAT_WINDOW, 8);
    }
}
