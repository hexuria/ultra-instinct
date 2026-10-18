//! The dojo policy (issue #49, work item 5): lessons inside the loop.
//!
//! Decision order, closed as a [`BrowserPolicy`] so the agent loop's
//! guard → ticket → revalidate → executor chain is untouched:
//!
//! 1. credit the pending remote choice (verified win → lesson, verified
//!    miss → loss) from `history` before deciding again;
//! 2. `InstinctPolicy` with this situation's trust evidence applied
//!    (±[`ultra_instinct_policy::TRUST_CAP_MILLIS`]);
//! 3. a learned move replay: if `moves[key]` names a step for this clause
//!    whose action is still offered and not distrusted, choose it;
//! 4. remote escalation (armed only when a remote policy is given);
//!    a remote choice is remembered in `pending` until its step verdict
//!    shows up in `history`.
//!
//! On an identical rerun the learned move answers locally — zero remote
//! calls. Situation keys are `context_key` (near-free): the policy cannot
//! know `near` before deciding, so it never keys on it.

use std::collections::BTreeMap;

use ultra_instinct_core::{ActionId, ActionKind, ActionSpace};
use ultra_instinct_dojo::{
    context_key, label_bonus_map, site_line, trust_bonus, LessonStore, Move, MoveStep, Situation,
    Word,
};
use ultra_instinct_policy::{
    label_names_target, AgentGoal, BrowserPolicy, HistoryEntry, InstinctPolicy, PolicyContext,
    PolicyDecision, PolicyError, PolicyOutcome,
};

/// A remote choice whose step verdict has not arrived yet: the situation
/// key it was decided in, the clause, the offered action id, its label for
/// the trust entry, and the `history` length when it was chosen — the
/// watermark past which its own step must appear to count.
type Pending = (String, String, String, String, usize);

/// Instinct + lesson store + optional remote fallback.
pub struct DojoPolicy {
    local: InstinctPolicy,
    remote: Option<Box<dyn BrowserPolicy>>,
    store: LessonStore,
    ctx: PolicyContext,
    last: &'static str,
    pending: Option<Pending>,
    now_ms: u64,
    learned: usize,
    /// Step numbers credited this run — `run_cmd` merges them into
    /// `LessonStore::seen_steps` under the diary id it just wrote.
    credited: Vec<u32>,
}

impl DojoPolicy {
    pub fn new(remote: Option<Box<dyn BrowserPolicy>>, store: LessonStore) -> Self {
        Self {
            local: InstinctPolicy::default(),
            remote,
            store,
            ctx: PolicyContext::default(),
            last: "instinct",
            pending: None,
            now_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            learned: 0,
            credited: Vec::new(),
        }
    }

    /// The learned lessons this policy grew during the run (wins and
    /// losses credited from remote choices).
    pub fn learned_count(&self) -> usize {
        self.learned
    }

    /// The store (load it, run, then save back through [`Self::into_store`]).
    pub fn store(&self) -> &LessonStore {
        &self.store
    }

    /// Step numbers credited this run (a remote choice's verdict folded
    /// into trust). Persisted as `"<diary>:<step>"` in `seen_steps`.
    pub fn credited_steps(&self) -> &[u32] {
        &self.credited
    }

    /// Take the store back for persisting at run end.
    pub fn into_store(self) -> LessonStore {
        self.store
    }

    /// Test knob: pin the clock the trust decay reads.
    #[cfg(test)]
    pub fn set_now_ms(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// This decision's context key from the situation the agent pushed.
    fn key(&self, goal: &AgentGoal) -> String {
        let site = site_line(self.ctx.site_url.as_deref(), self.ctx.site_title.as_deref());
        let situation = Situation {
            front_layer: self.ctx.front_layer,
            roles: self.ctx.roles.clone(),
            near: Vec::new(),
        };
        context_key(site.as_ref(), &situation, goal.as_str())
    }

    /// Fold the pending remote choice's verdict into the store. Verified
    /// win → trust win + a Move + a Word; verified miss → trust loss.
    ///
    /// The pending choice resolves on the next decide only: its own step
    /// is the first same-id entry past the watermark. An older same-id
    /// step (or one a different arm executes later) is not its verdict —
    /// no entry means the choice stale-discarded or was superseded, so it
    /// is dropped with nothing credited.
    fn credit_pending(&mut self, history: &[HistoryEntry]) {
        let Some((key, clause, action_id, label, at_len)) = self.pending.take() else {
            return;
        };
        let Some(entry) = history
            .iter()
            .skip(at_len.min(history.len()))
            .find(|h| h.action_id.as_str() == action_id)
        else {
            return;
        };
        // An unreadable post-state is not a loss — the verdict never came.
        if entry.verification == "unknown" {
            return;
        }
        let won = match entry.kind {
            ActionKind::Click => {
                matches!(
                    entry.verification.as_str(),
                    "success" | "state-changed" | "navigation"
                ) && label_names_target(&clause, &label)
            }
            ActionKind::TypeText | ActionKind::Select => entry.verification == "success",
            _ => false,
        };
        let table = self.store.trust.entry(key.clone()).or_default();
        let rec = table.entry(label.clone()).or_default();
        if won {
            rec.wins += 1;
        } else {
            rec.losses += 1;
        }
        rec.last_seen_ms = rec.last_seen_ms.max(self.now_ms);
        self.learned += 1;
        self.credited.push(entry.step);
        if won {
            let steps = vec![MoveStep {
                clause: clause.clone(),
                action_id: action_id.clone(),
                label: label.clone(),
            }];
            let moves = self.store.moves.entry(key.clone()).or_default();
            if !moves.iter().any(|m| m.steps == steps) {
                moves.push(Move {
                    steps,
                    diaries: vec![],
                });
            }
            let words = self.store.words.entry(key).or_default();
            match words.iter_mut().find(|w| w.phrase == clause) {
                Some(w) => w.label = label,
                None => words.push(Word {
                    phrase: clause,
                    label,
                    diaries: vec![],
                }),
            }
        }
    }

    /// A learned move for this situation whose step covers this clause,
    /// is still offered, and isn't distrusted — the local answer that
    /// makes an identical rerun need zero remote calls.
    fn learned_choice(
        &self,
        space: &ActionSpace,
        key: &str,
        goal: &AgentGoal,
        abstain: &PolicyOutcome,
    ) -> Option<PolicyDecision> {
        let moves = self.store.moves.get(key);
        let trust_table = self.store.trust.get(key);
        // Higher belts replay first; ties keep store order (distilled
        // lessons sit earlier than in-run learned ones). No move entry is
        // not a miss — the word table below can still bind the clause.
        let mut ranked: Vec<&ultra_instinct_dojo::Move> =
            moves.map(|m| m.iter().collect()).unwrap_or_default();
        ranked.sort_by_key(|mv| {
            std::cmp::Reverse(ultra_instinct_dojo::move_belt(&self.store, key, mv))
        });
        let mut best: Option<(ActionId, String)> = None;
        for mv in ranked {
            for step in &mv.steps {
                if step.clause != goal.as_str() {
                    continue;
                }
                let Ok(id) = ActionId::try_new(step.action_id.as_str()) else {
                    continue;
                };
                // The id must still name the same control: a region id
                // that now offers a different label replays nothing.
                let Some(offered) = space.get(&id) else {
                    continue;
                };
                if offered.label() != step.label {
                    continue;
                }
                if let Some(trust) = trust_table.and_then(|t| t.get(&step.label)) {
                    if trust_bonus(trust, self.now_ms) < 0 {
                        continue;
                    }
                }
                best = Some((id, step.label.clone()));
                break;
            }
            if best.is_some() {
                break;
            }
        }
        // A learned word binds this clause's phrase to a label that
        // worked here — replay it only when exactly one offered action
        // still carries the label, so a twin never picks at random.
        let best = best.or_else(|| {
            self.store.words.get(key).and_then(|ws| {
                ws.iter()
                    .filter(|w| w.phrase == goal.as_str())
                    .filter(|w| {
                        trust_table
                            .and_then(|t| t.get(&w.label))
                            .is_none_or(|t| trust_bonus(t, self.now_ms) >= 0)
                    })
                    .find_map(|w| {
                        let mut carriers = space
                            .actions()
                            .filter(|a| !a.kind().is_control() && a.label() == w.label);
                        match (carriers.next(), carriers.next()) {
                            (Some(a), None) => Some((a.id().clone(), w.label.clone())),
                            _ => None,
                        }
                    })
            })
        });
        let (id, label) = best?;
        let action = space.get(&id)?;
        // Report the evidence Instinct saw for this action (the abstain's
        // own ranked list) — the lesson supplies the choice, not a score.
        let confidence = match abstain {
            PolicyOutcome::Abstain { target_ranked, .. } => target_ranked
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.confidence_millis)
                .unwrap_or(0),
            PolicyOutcome::Choice(_) => 0,
        };
        Some(PolicyDecision {
            action_id: id,
            kind: action.kind(),
            target_label: label,
            confidence_millis: confidence,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        })
    }
}

impl BrowserPolicy for DojoPolicy {
    fn name(&self) -> &'static str {
        "dojo"
    }

    fn decision_source(&self) -> &'static str {
        self.last
    }

    fn set_situation(&mut self, ctx: &PolicyContext) {
        self.ctx = ctx.clone();
    }

    fn decide(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        self.credit_pending(history);
        let key = self.key(goal);

        // Trust evidence into Instinct, capped — item 4's only channel.
        let adjustments: BTreeMap<String, i16> = self
            .store
            .trust
            .get(&key)
            .map(|table| label_bonus_map(table, self.now_ms))
            .unwrap_or_default();
        self.local.set_evidence_adjustments(adjustments);
        let first = self.local.decide(space, goal, history)?;
        self.last = self.local.decision_source();
        if first.as_choice().is_some() {
            return Ok(first);
        }

        if let Some(choice) = self.learned_choice(space, &key, goal, &first) {
            self.last = "dojo";
            return Ok(PolicyOutcome::Choice(choice));
        }

        if let Some(remote) = self.remote.as_mut() {
            let outcome = remote.decide(space, goal, history)?;
            self.last = remote.decision_source();
            if let Some(choice) = outcome.as_choice() {
                // Only element kinds earn a lesson: DONE / BLOCKED never
                // execute, and scroll / wait carry no target evidence — a
                // pending that can never resolve is worse than none.
                if !choice.kind.is_control() {
                    self.pending = Some((
                        key,
                        goal.as_str().to_owned(),
                        choice.action_id.as_str().to_owned(),
                        choice.target_label.clone(),
                        history.len(),
                    ));
                }
            }
            return Ok(outcome);
        }

        Ok(first)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use ultra_instinct_core::parse_fixture;
    use ultra_instinct_dojo::{Move, MoveStep};

    use super::*;

    /// Two same-label buttons — Instinct abstains on margin so the
    /// learned-move and remote arms are exercised.
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

    fn goal() -> AgentGoal {
        AgentGoal::new("Click Send")
    }

    fn entry(step: u32, id: &str, kind: ActionKind, verification: &str) -> HistoryEntry {
        HistoryEntry {
            step,
            action_id: ActionId::try_new(id).unwrap(),
            kind,
            label: "Send".to_owned(),
            verification: verification.to_owned(),
        }
    }

    struct ScriptedRemote {
        calls: Rc<Cell<usize>>,
        id: &'static str,
        kind: ActionKind,
        label: &'static str,
    }

    impl ScriptedRemote {
        fn armed() -> (Box<dyn BrowserPolicy>, Rc<Cell<usize>>) {
            Self::armed_kind("CLICK:alpha", ActionKind::Click, "Send")
        }

        fn armed_kind(
            id: &'static str,
            kind: ActionKind,
            label: &'static str,
        ) -> (Box<dyn BrowserPolicy>, Rc<Cell<usize>>) {
            let calls = Rc::new(Cell::new(0));
            (
                Box::new(Self {
                    calls: calls.clone(),
                    id,
                    kind,
                    label,
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
            _space: &ActionSpace,
            _goal: &AgentGoal,
            _history: &[HistoryEntry],
        ) -> Result<PolicyOutcome, PolicyError> {
            self.calls.set(self.calls.get() + 1);
            Ok(PolicyOutcome::Choice(PolicyDecision {
                action_id: ActionId::try_new(self.id).unwrap(),
                kind: self.kind,
                target_label: self.label.to_owned(),
                confidence_millis: 900,
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            }))
        }
    }

    /// A pending choice's step is the first same-id entry PAST the
    /// watermark — an older same-id entry is not its verdict.
    #[test]
    fn an_older_same_id_step_credits_nothing() {
        let space = space();
        let goal = goal();
        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), LessonStore::default());

        // Step 0 predates the pending point: it is a different arm's
        // verdict, not this choice's.
        let history = vec![entry(0, "CLICK:alpha", ActionKind::Click, "success")];
        let _ = policy.decide(&space, &goal, &history).unwrap();
        assert_eq!(calls.get(), 1);

        let _ = policy.decide(&space, &goal, &history).unwrap();
        assert_eq!(calls.get(), 2, "unresolved pending re-arms the remote");
        assert!(policy.credited_steps().is_empty());
        let losses_or_wins: u32 = policy
            .store()
            .trust
            .values()
            .flat_map(|t| t.values().map(|r| r.wins + r.losses))
            .sum();
        assert_eq!(losses_or_wins, 0, "a stale verdict never counts");
    }

    /// A pending entry that is superseded before its step lands credits
    /// at most once — and only the newest pending sees the new step.
    #[test]
    fn a_superseded_pending_credits_once() {
        let space = space();
        let goal = goal();
        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), LessonStore::default());

        let _ = policy.decide(&space, &goal, &[]).unwrap();
        let _ = policy.decide(&space, &goal, &[]).unwrap();
        assert_eq!(calls.get(), 2);

        // The newest pending's own step lands.
        let history = vec![entry(0, "CLICK:alpha", ActionKind::Click, "success")];
        let _ = policy.decide(&space, &goal, &history).unwrap();
        assert_eq!(policy.credited_steps(), &[0]);
        let wins: u32 = policy
            .store()
            .trust
            .values()
            .flat_map(|t| t.values().map(|r| r.wins))
            .sum();
        assert_eq!(wins, 1, "one choice, one credit");
    }

    /// DONE / BLOCKED / scroll / wait never become pending — they carry
    /// no target evidence, and their later arrival in history records
    /// neither a win nor a loss.
    #[test]
    fn a_control_kind_choice_is_never_pending() {
        let space = space();
        let goal = goal();
        let (remote, calls) = ScriptedRemote::armed_kind("WAIT", ActionKind::Wait, "");
        let mut policy = DojoPolicy::new(Some(remote), LessonStore::default());

        let _ = policy.decide(&space, &goal, &[]).unwrap();
        assert_eq!(calls.get(), 1);
        assert!(policy.pending.is_none(), "controls earn no pending");

        let history = vec![entry(0, "WAIT", ActionKind::Wait, "success")];
        let _ = policy.decide(&space, &goal, &history).unwrap();
        assert!(policy.credited_steps().is_empty());
        assert!(policy.store().trust.is_empty(), "controls score no trust");
    }

    /// An unknown post-state is an unreadable page, not a loss — the
    /// pending drops with nothing credited.
    #[test]
    fn an_unknown_verdict_is_not_a_loss() {
        let space = space();
        let goal = goal();
        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), LessonStore::default());

        let _ = policy.decide(&space, &goal, &[]).unwrap();
        let history = vec![entry(0, "CLICK:alpha", ActionKind::Click, "unknown")];
        let _ = policy.decide(&space, &goal, &history).unwrap();
        assert_eq!(calls.get(), 2, "remote stays armed after a dropped pending");
        assert!(policy.credited_steps().is_empty());
        assert!(policy.store().trust.is_empty());
    }

    /// A stored move replays only while the action id still names the
    /// same control — the offered label must equal the recorded one.
    #[test]
    fn a_drifted_label_replays_nothing() {
        let space = space();
        let goal = goal();

        // The situation key this policy will compute for the goal.
        let key = DojoPolicy::new(None, LessonStore::default()).key(&goal);
        let mut store = LessonStore::default();
        // The stored step claims CLICK:alpha is "Cancel" — the offered
        // action is still "Send". Nothing replays.
        store.moves.insert(
            key,
            vec![Move {
                steps: vec![MoveStep {
                    clause: goal.as_str().to_owned(),
                    action_id: "CLICK:alpha".to_owned(),
                    label: "Cancel".to_owned(),
                }],
                diaries: vec![],
            }],
        );

        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), store);
        let _ = policy.decide(&space, &goal, &[]).unwrap();
        assert_eq!(policy.decision_source(), "scripted-remote");
        assert_eq!(calls.get(), 1);
    }

    /// The matching control: identical id AND label does replay.
    #[test]
    fn a_matching_label_replays() {
        let space = space();
        let goal = goal();
        let key = DojoPolicy::new(None, LessonStore::default()).key(&goal);
        let mut store = LessonStore::default();
        store.moves.insert(
            key,
            vec![Move {
                steps: vec![MoveStep {
                    clause: goal.as_str().to_owned(),
                    action_id: "CLICK:alpha".to_owned(),
                    label: "Send".to_owned(),
                }],
                diaries: vec![],
            }],
        );

        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), store);
        let out = policy.decide(&space, &goal, &[]).unwrap();
        let choice = out.as_choice().expect("learned move replays");
        assert_eq!(choice.action_id.as_str(), "CLICK:alpha");
        assert_eq!(policy.decision_source(), "dojo");
        assert_eq!(calls.get(), 0);
    }

    /// A third button makes "Go" the space's only carrier of its label —
    /// a learned word pointing at it replays locally.
    fn trio_space() -> ActionSpace {
        let m = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=alpha role=button label="Send" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
            region id=beta role=button label="Send" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
            region id=gamma role=button label="Go" x=10 y=90 w=80 h=24 actions=click sources=dom,accessibility
            "#,
        )
        .unwrap();
        ActionSpace::from_manifold(&m)
    }

    /// No move recorded, but the words table learned that this clause
    /// resolved to a label — replay the label when exactly one offered
    /// action still carries it.
    #[test]
    fn a_learned_word_replays_its_unique_carrier() {
        let space = trio_space();
        let goal = goal();
        let key = DojoPolicy::new(None, LessonStore::default()).key(&goal);
        let mut store = LessonStore::default();
        store.words.insert(
            key,
            vec![ultra_instinct_dojo::Word {
                phrase: goal.as_str().to_owned(),
                label: "Go".to_owned(),
                diaries: vec![],
            }],
        );

        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), store);
        let out = policy.decide(&space, &goal, &[]).unwrap();
        let choice = out.as_choice().expect("learned word replays");
        assert_eq!(choice.action_id.as_str(), "CLICK:gamma");
        assert_eq!(choice.target_label, "Go");
        assert_eq!(policy.decision_source(), "dojo");
        assert_eq!(calls.get(), 0);
    }

    /// Two same-label carriers make a word ambiguous — a replay would be a
    /// coin flip, so the remote arm decides instead.
    #[test]
    fn an_ambiguous_word_replays_nothing() {
        let space = space();
        let goal = goal();
        let key = DojoPolicy::new(None, LessonStore::default()).key(&goal);
        let mut store = LessonStore::default();
        store.words.insert(
            key,
            vec![ultra_instinct_dojo::Word {
                phrase: goal.as_str().to_owned(),
                label: "Send".to_owned(),
                diaries: vec![],
            }],
        );

        let (remote, calls) = ScriptedRemote::armed();
        let mut policy = DojoPolicy::new(Some(remote), store);
        let _ = policy.decide(&space, &goal, &[]).unwrap();
        assert_eq!(policy.decision_source(), "scripted-remote");
        assert_eq!(calls.get(), 1);
    }
}
