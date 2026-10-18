//! Trust → evidence: turn a situation+label win/loss record into the
//! capped integer bonus `ultra-instinct-policy` applies inside Instinct's scoring.
//!
//! The formula is deliberately small and inspectable (issue #49, item 4):
//!
//! ```text
//! raw    = clamp((wins - losses) * 10, -75, +75)
//! age    = now - last_seen_ms
//! bonus  = raw                when age < 30 days
//!        = raw / 2            when age < 90 days
//!        = raw / 4            when age < 180 days
//!        = 0                  older — the dojo forgets
//! ```
//!
//! ±75 millis is well under the gap between the Standard profile's
//! `min_confidence` (750) and `min_margin` (150): a lesson can break a
//! near-tie but can never lift a candidate over the bar alone. Losses
//! subtract directly — a label that keeps failing loses its lift fast.
//! Trust is always looked up under a situation key; there is no global
//! trust.

use std::collections::BTreeMap;

use crate::lessons::Trust;

const CAP: i16 = 75;
const DAY_MS: u64 = 86_400_000;

/// The bounded evidence bonus for one trust record at `now_ms`.
pub fn trust_bonus(trust: &Trust, now_ms: u64) -> i16 {
    let raw = (i64::from(trust.wins) - i64::from(trust.losses)) * 10;
    let raw = raw.clamp(-i64::from(CAP), i64::from(CAP)) as i16;
    let age_ms = now_ms.saturating_sub(trust.last_seen_ms);
    let shift = if age_ms < 30 * DAY_MS {
        0
    } else if age_ms < 90 * DAY_MS {
        1
    } else if age_ms < 180 * DAY_MS {
        2
    } else {
        return 0;
    };
    raw >> shift
}

/// Label → bonus map for one situation's trust table, ready for
/// `InstinctPolicy::set_evidence_adjustments`. Zero bonuses are dropped.
pub fn label_bonus_map(table: &BTreeMap<String, Trust>, now_ms: u64) -> BTreeMap<String, i16> {
    table
        .iter()
        .filter_map(|(label, trust)| {
            let bonus = trust_bonus(trust, now_ms);
            (bonus != 0).then(|| (label.clone(), bonus))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust(wins: u32, losses: u32, last_seen_ms: u64) -> Trust {
        Trust {
            wins,
            losses,
            last_seen_ms,
            diaries: vec![],
        }
    }

    #[test]
    fn bonus_scales_with_wins_and_losses() {
        assert_eq!(trust_bonus(&trust(3, 0, 1000), 1000), 30);
        assert_eq!(trust_bonus(&trust(3, 2, 1000), 1000), 10);
        assert_eq!(trust_bonus(&trust(0, 4, 1000), 1000), -40);
        assert_eq!(trust_bonus(&trust(2, 2, 1000), 1000), 0);
    }

    #[test]
    fn bonus_is_capped() {
        assert_eq!(trust_bonus(&trust(100, 0, 0), 0), CAP);
        assert_eq!(trust_bonus(&trust(0, 100, 0), 0), -CAP);
    }

    #[test]
    fn bonus_decays_with_age() {
        let t = trust(7, 0, 1_000);
        assert_eq!(trust_bonus(&t, 1_000 + 29 * DAY_MS), 70);
        assert_eq!(trust_bonus(&t, 1_000 + 31 * DAY_MS), 35);
        assert_eq!(trust_bonus(&t, 1_000 + 100 * DAY_MS), 17);
        assert_eq!(trust_bonus(&t, 1_000 + 200 * DAY_MS), 0);
    }

    #[test]
    fn map_drops_zero_bonuses() {
        let mut table = BTreeMap::new();
        table.insert("Go".to_owned(), trust(2, 0, 10));
        table.insert("Cancel".to_owned(), trust(1, 1, 10));
        table.insert("Old".to_owned(), trust(5, 0, 0));
        let map = label_bonus_map(&table, 10 + 100 * DAY_MS);
        assert_eq!(map.get("Go"), Some(&5), "20 decayed >>2");
        assert!(!map.contains_key("Cancel"), "zero-raw labels drop");
        // "Old" was last seen at ms 0: age ~100d → 50 >> 2.
        assert_eq!(map.get("Old"), Some(&12));
    }
}
