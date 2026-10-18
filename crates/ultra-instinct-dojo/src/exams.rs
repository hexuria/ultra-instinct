//! The exam report (issue #49, work item 6): belts per pairing plus the
//! dojo's progress metric — remote policy calls per 100 decisions of the
//! same task, which must fall as lessons accumulate.
//!
//! Counts come from diary decision lines: each carries `source`, the arm
//! that answered. `instinct` and `dojo` are local; every other arm name
//! (`jev`, `clef`, `clef-flash`, `escalating:<remote>`, …) is a remote
//! call. Deterministic, offline, no clocks.

use std::collections::BTreeMap;

use crate::belt::{label_belts, move_belt};
use crate::lessons::LessonStore;
use crate::line::DiaryLine;

/// Arms that count as local decisions. Anything else on a decision line
/// is a remote (paid, escalated) call.
const LOCAL_SOURCES: [&str; 2] = ["instinct", "dojo"];

pub fn is_remote_source(source: &str) -> bool {
    !LOCAL_SOURCES.contains(&source)
}

/// What an exam reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExamReport {
    /// Decision lines read across all diaries.
    pub decisions: u32,
    /// Decisions answered by a remote arm.
    pub remote_calls: u32,
    /// `remote_calls * 100 / decisions` (0 when no decisions).
    pub remote_per_100: u32,
    /// Belt -> number of trust pairings holding it.
    pub belt_histogram: BTreeMap<&'static str, usize>,
    /// Move belts: `context key -> per-move belt list` (store order).
    pub move_belts: BTreeMap<String, Vec<&'static str>>,
}

/// Grade every trust pairing and count remote calls over the diaries.
pub fn examine(diaries: &[Vec<DiaryLine>], store: &LessonStore) -> ExamReport {
    let mut report = ExamReport::default();
    for lines in diaries {
        for line in lines {
            if let DiaryLine::Decision(decision) = line {
                report.decisions += 1;
                if is_remote_source(&decision.source) {
                    report.remote_calls += 1;
                }
            }
        }
    }
    report.remote_per_100 = report
        .remote_calls
        .checked_mul(100)
        .and_then(|c| c.checked_div(report.decisions))
        .unwrap_or(0);
    for labels in label_belts(store).values() {
        for belt in labels.values() {
            *report.belt_histogram.entry(belt.as_str()).or_default() += 1;
        }
    }
    for (key, moves) in &store.moves {
        report.move_belts.insert(
            key.clone(),
            moves
                .iter()
                .map(|mv| move_belt(store, key, mv).as_str())
                .collect(),
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::{Move, MoveStep, Trust};
    use crate::line::{DecisionLine, Situation};

    fn decision(source: &str) -> DiaryLine {
        DiaryLine::Decision(Box::new(DecisionLine {
            seq: 0,
            clause_index: 0,
            clause: "Click Go".to_owned(),
            mode: "act".to_owned(),
            source: source.to_owned(),
            site: None,
            situation: Situation::default(),
            offered: Vec::new(),
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
            history: Vec::new(),
            choice: None,
            abstain: Some("abstained".to_owned()),
        }))
    }

    fn store_with(wins: u32, losses: u32) -> LessonStore {
        let mut store = LessonStore::default();
        let key = "k".to_owned();
        store.trust.entry(key.clone()).or_default().insert(
            "Go".to_owned(),
            Trust {
                wins,
                losses,
                ..Trust::default()
            },
        );
        store.moves.entry(key).or_default().push(Move {
            steps: vec![MoveStep {
                clause: "Click Go".to_owned(),
                action_id: "CLICK:go".to_owned(),
                label: "Go".to_owned(),
            }],
            diaries: Vec::new(),
        });
        store
    }

    #[test]
    fn remote_calls_are_counted_per_100_decisions() {
        let diaries = vec![vec![
            decision("instinct"),
            decision("jev"),
            decision("dojo"),
            decision("clef-flash"),
        ]];
        let report = examine(&diaries, &LessonStore::default());
        assert_eq!(report.decisions, 4);
        assert_eq!(report.remote_calls, 2);
        assert_eq!(report.remote_per_100, 50);
    }

    #[test]
    fn belt_histogram_and_moves_report_grades() {
        let store = store_with(10, 0);
        let report = examine(&[], &store);
        assert_eq!(report.belt_histogram["ultra"], 1);
        assert_eq!(report.move_belts["k"], vec!["ultra"]);
    }

    #[test]
    fn an_empty_diary_reports_zero_calls() {
        let report = examine(&[], &LessonStore::default());
        assert_eq!(report.remote_per_100, 0);
    }
}
