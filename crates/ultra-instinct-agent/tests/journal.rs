//! Golden journal test (issue #49, work item 1): a full mock run drains as
//! Run → Decision(s) → Step(s) → Finished, with the decision's offered menu,
//! ranked candidates, source, and step win flags all populated.

use ultra_instinct_agent::{step_is_win, AgentBuilder, JournalEvent, MockBrowser};
use ultra_instinct_core::{parse_fixture, InteractionManifold};
use ultra_instinct_policy::InstinctPolicy;

fn manifold(src: &str) -> InteractionManifold {
    parse_fixture(src).unwrap()
}

#[test]
fn journal_records_a_full_mock_run() {
    let before = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=no role=button label="Cancel" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let after = manifold(
        r#"
        viewport w=800 h=600
        region id=done role=button label="Finish" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser = MockBrowser::new(before);
    browser.set_on_press(after);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .max_policy_calls(4)
        .build("Continue");
    let outcome = agent.run();
    assert!(!outcome.steps().is_empty(), "{outcome:?}");

    let journal = agent.take_journal();
    assert!(agent.take_journal().is_empty(), "journal drains once");

    // Shape: Run first, Finished last, every middle event is a decision,
    // step, stale discard, or clause advance.
    let first = journal.first().expect("run line");
    match first {
        JournalEvent::Run {
            goal,
            clauses,
            policy,
        } => {
            assert_eq!(goal, "Continue");
            assert_eq!(clauses, &["Continue"]);
            assert_eq!(*policy, "instinct");
        }
        other => panic!("first event is the run line, not {other:?}"),
    }
    match journal.last().expect("finished line") {
        JournalEvent::Finished {
            outcome: recorded,
            policy_calls,
            stale_discards,
            duration_ms: _,
        } => {
            assert_eq!(*policy_calls, agent.policy_calls());
            assert_eq!(*stale_discards, agent.stale_discards());
            assert_eq!(recorded.steps().len(), outcome.steps().len());
        }
        other => panic!("last event is the finished line, not {other:?}"),
    }

    // The first decision carries the offered menu, the source, and a choice
    // inside that menu (or an abstain with ranked candidates).
    let decision = journal
        .iter()
        .find_map(|event| match event {
            JournalEvent::Decision {
                clause,
                mode,
                source,
                space,
                outcome,
                ..
            } => Some((clause, mode, source, space, outcome)),
            _ => None,
        })
        .expect("a decision was recorded");
    assert_eq!(*decision.0, "Continue");
    assert_eq!(*decision.1, "act");
    assert_eq!(*decision.2, "instinct");
    assert!(!decision.3.is_empty(), "offered menu recorded");
    match decision.4 {
        ultra_instinct_policy::PolicyOutcome::Choice(choice) => {
            assert!(decision.3.get(&choice.action_id).is_some());
        }
        ultra_instinct_policy::PolicyOutcome::Abstain { .. } => panic!("golden run decides"),
    }

    // Every recorded step carries the input kind and the win flag the
    // predicate gives it.
    let steps: Vec<_> = journal
        .iter()
        .filter_map(|event| match event {
            JournalEvent::Step {
                input,
                won,
                record,
                clause,
                ..
            } => Some((*input, *won, record, clause)),
            _ => None,
        })
        .collect();
    assert_eq!(steps.len(), outcome.steps().len());
    for (input, won, record, clause) in steps {
        assert_eq!(input, "click");
        assert_eq!(won, step_is_win(clause, record));
    }
}

/// A policy that errors still lands in the journal — the diary must show
/// WHY a run ended, not just that it did.
#[test]
fn a_failing_policy_writes_a_policy_error_event() {
    struct Boom;
    impl ultra_instinct_policy::BrowserPolicy for Boom {
        fn name(&self) -> &'static str {
            "boom"
        }
        fn decide(
            &mut self,
            _space: &ultra_instinct_core::ActionSpace,
            _goal: &ultra_instinct_policy::AgentGoal,
            _history: &[ultra_instinct_policy::HistoryEntry],
        ) -> Result<ultra_instinct_policy::PolicyOutcome, ultra_instinct_policy::PolicyError>
        {
            Err(ultra_instinct_policy::PolicyError::Internal(
                "kaboom".to_owned(),
            ))
        }
    }

    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut agent = AgentBuilder::new(MockBrowser::new(page), Boom)
        .max_steps(3)
        .build("Continue");
    assert!(agent.predict().is_err(), "the policy error propagates");

    let journal = agent.take_journal();
    let event = journal
        .iter()
        .find_map(|event| match event {
            JournalEvent::PolicyError {
                clause,
                source,
                error,
                ..
            } => Some((clause, source, error)),
            _ => None,
        })
        .expect("a policy_error event was journaled");
    assert_eq!(*event.0, "Continue");
    assert_eq!(*event.1, "boom");
    assert!(event.2.contains("kaboom"));
}
