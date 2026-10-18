//! Scripted-remote acceptance for issue #49, work item 5: a remote
//! choice that verifies becomes a lesson, and an identical rerun
//! decides locally — zero remote calls.

use std::cell::Cell;
use std::rc::Rc;

use ultra_instinct::dojo_policy::DojoPolicy;
use ultra_instinct_core::{parse_fixture, ActionId, ActionKind, ActionSpace};
use ultra_instinct_policy::{
    AgentGoal, BrowserPolicy, HistoryEntry, PolicyDecision, PolicyOutcome,
};

fn space() -> ActionSpace {
    let m = parse_fixture(
        r#"
        viewport w=800 h=600
        region id=alpha role=button label="Send" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=beta role=button label="Send" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    )
    .unwrap();
    ActionSpace::from_manifold(&m)
}

/// A scripted remote arm: always picks the first Send; its call counter is
/// shared so the test can prove whether it answered at all.
struct ScriptedRemote {
    calls: Rc<Cell<usize>>,
}

impl ScriptedRemote {
    fn armed() -> (Box<dyn BrowserPolicy>, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        (
            Box::new(Self {
                calls: calls.clone(),
            }),
            calls,
        )
    }
}

impl BrowserPolicy for ScriptedRemote {
    fn name(&self) -> &'static str {
        "scripted-remote"
    }

    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        _history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, ultra_instinct_policy::PolicyError> {
        self.calls.set(self.calls.get() + 1);
        let id = ActionId::try_new("CLICK:alpha").unwrap();
        let action = space.get(&id).unwrap();
        Ok(PolicyOutcome::Choice(PolicyDecision {
            action_id: id,
            kind: action.kind(),
            target_label: action.label().to_owned(),
            confidence_millis: 900,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }))
    }
}

fn verified_history() -> Vec<HistoryEntry> {
    vec![HistoryEntry {
        step: 1,
        action_id: ActionId::try_new("CLICK:alpha").unwrap(),
        kind: ActionKind::Click,
        label: "Send".to_owned(),
        verification: "success".to_owned(),
    }]
}

#[test]
fn a_verified_remote_choice_becomes_a_local_lesson() {
    let space = space();
    let goal = AgentGoal::new("Click Send");
    let (remote, calls) = ScriptedRemote::armed();
    let mut policy = DojoPolicy::new(Some(remote), ultra_instinct_dojo::LessonStore::default());

    // Twin labels: Instinct abstains on margin, the remote answers.
    let first = policy.decide(&space, &goal, &[]).unwrap();
    let choice = first.as_choice().expect("remote answers");
    assert_eq!(choice.target_label, "Send");
    assert_eq!(policy.decision_source(), "scripted-remote");
    assert_eq!(calls.get(), 1);

    // The step verifies; the next decide folds it into the store.
    let _ = policy.decide(&space, &goal, &verified_history()).unwrap();
    let store = policy.store();
    let key = store.moves.keys().next().expect("a move was learned");
    let mv = &store.moves[key][0];
    assert_eq!(mv.steps[0].action_id, "CLICK:alpha");
    assert_eq!(store.trust[key]["Send"].wins, 1);
    assert_eq!(policy.learned_count(), 1);
}

#[test]
fn identical_rerun_decides_locally_with_no_remote_call() {
    let space = space();
    let goal = AgentGoal::new("Click Send");

    // Round 1: learn through a remote.
    let (remote, calls) = ScriptedRemote::armed();
    let mut round1 = DojoPolicy::new(Some(remote), ultra_instinct_dojo::LessonStore::default());
    let _ = round1.decide(&space, &goal, &[]).unwrap();
    let _ = round1.decide(&space, &goal, &verified_history()).unwrap();
    assert_eq!(calls.get(), 1);
    let store = round1.into_store();

    // Persist + reload — the lesson survives the trip.
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-dojo-persist-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lessons.json");
    ultra_instinct_dojo::save_lessons(&store, &path).unwrap();
    let reloaded = ultra_instinct_dojo::load_lessons(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    // Round 2: identical rerun on a fresh policy — no remote call.
    let (remote2, calls2) = ScriptedRemote::armed();
    let mut round2 = DojoPolicy::new(Some(remote2), reloaded);
    let out = round2.decide(&space, &goal, &[]).unwrap();
    let choice = out.as_choice().expect("learned move replays");
    assert_eq!(choice.action_id.as_str(), "CLICK:alpha");
    assert_eq!(round2.decision_source(), "dojo");
    assert_eq!(calls2.get(), 0, "learned rerun must not call the remote");
}

#[test]
fn a_missing_verdict_keeps_the_remote_armed() {
    let space = space();
    let goal = AgentGoal::new("Click Send");
    let (remote, calls) = ScriptedRemote::armed();
    let mut policy = DojoPolicy::new(Some(remote), ultra_instinct_dojo::LessonStore::default());
    let _ = policy.decide(&space, &goal, &[]).unwrap();
    // No verdict arrives — the next abstain escalates again.
    let _ = policy.decide(&space, &goal, &[]).unwrap();
    assert_eq!(calls.get(), 2);
    assert!(
        policy.store().moves.is_empty(),
        "unverified choice never becomes a move"
    );
}

#[test]
fn a_verified_miss_is_a_loss_not_a_move() {
    let space = space();
    let goal = AgentGoal::new("Click Send");
    let (remote, calls) = ScriptedRemote::armed();
    let mut policy = DojoPolicy::new(Some(remote), ultra_instinct_dojo::LessonStore::default());
    let _ = policy.decide(&space, &goal, &[]).unwrap();

    let missed = vec![HistoryEntry {
        step: 1,
        action_id: ActionId::try_new("CLICK:alpha").unwrap(),
        kind: ActionKind::Click,
        label: "Send".to_owned(),
        verification: "wrong-effect".to_owned(),
    }];
    let _ = policy.decide(&space, &goal, &missed).unwrap();
    assert!(policy.store().moves.is_empty(), "a miss learns no move");
    let losses: u32 = policy
        .store()
        .trust
        .values()
        .flat_map(|t| t.values().map(|r| r.losses))
        .sum();
    assert_eq!(losses, 1);
    let _ = calls;
}
