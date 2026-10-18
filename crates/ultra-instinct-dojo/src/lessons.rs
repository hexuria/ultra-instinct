//! The lesson store (issue #49, work item 3): what the dojo learned from
//! its battle diaries, versioned and keyed per situation — never global.
//!
//! Four lesson kinds:
//! - **Words**: in this situation, the phrase P resolved to element L
//!   (label aliases).
//! - **Places**: situation signatures the dojo has seen, with the proving
//!   diary ids.
//! - **Moves**: clause → winning action routines that worked here.
//! - **Trust**: per situation+label wins/losses/last-seen — the evidence
//!   work item 4 feeds back into Instinct.
//!
//! Every lesson carries the diary ids that prove it. `load` rejects a
//! different schema version rather than silently misreading it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{json, Map, Value};

use ultra_instinct_core::ActionKind;

use crate::diary::read_diary;
use crate::error::DojoError;
use crate::line::{DiaryLine, SiteLine};
use crate::site::context_key;

/// Lesson-store schema version. Bump on any layout change; `load` refuses
/// other versions — a stale store is rebuilt from diaries, never migrated
/// blindly.
pub const LESSON_SCHEMA: u32 = 2;

/// In this situation, `phrase` resolved to element `label`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Word {
    pub phrase: String,
    pub label: String,
    /// Diary ids that proved this alias.
    pub diaries: Vec<String>,
}

/// A recognized situation: where it was seen and by which diaries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Place {
    pub host: Option<String>,
    pub path: Option<String>,
    /// Sorted unique role names of the situation.
    pub roles: Vec<String>,
    /// Whether a front layer was blocking at decide time.
    pub front_layer: bool,
    pub diaries: Vec<String>,
}

/// One link of a winning routine: this clause was satisfied by this action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveStep {
    pub clause: String,
    pub action_id: String,
    pub label: String,
}

/// A routine that worked in this situation, in execution order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Move {
    pub steps: Vec<MoveStep>,
    pub diaries: Vec<String>,
}

/// Per situation+label record of verified wins and losses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trust {
    pub wins: u32,
    pub losses: u32,
    /// Unix millis of the most recent run that touched this pair (the
    /// diary filename stamp), for the item-4 decay.
    pub last_seen_ms: u64,
    pub diaries: Vec<String>,
}

/// The whole store, keyed by the decision-time context key
/// (`site::context_key` — site, clause, front layer, roles; `near`
/// excluded because a deciding policy cannot know it yet).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LessonStore {
    pub words: BTreeMap<String, Vec<Word>>,
    pub places: BTreeMap<String, Place>,
    pub moves: BTreeMap<String, Vec<Move>>,
    pub trust: BTreeMap<String, BTreeMap<String, Trust>>,
    /// Diary ids already distilled — `learn_diary`/`learn_lines` is
    /// idempotent, so a second pass over the same diary counts nothing.
    pub learned_diaries: BTreeSet<String>,
    /// `"<diary>:<step>"` already counted toward trust — the live dojo
    /// policy writes its credited steps here so an offline distill of the
    /// same diary never counts them again.
    pub seen_steps: BTreeSet<String>,
}

/// The id of a diary file: its stem (`<millis>-<slug>`).
pub fn diary_id(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("diary")
        .to_owned()
}

/// The run timestamp encoded in a diary filename's leading digits, for
/// `Trust::last_seen_ms`. 0 when the name carries no stamp.
pub fn diary_stamp_ms(path: &Path) -> u64 {
    diary_id(path)
        .split('-')
        .next()
        .and_then(|head| head.parse().ok())
        .unwrap_or(0)
}

/// Distill one diary into the store. Wins and losses are the recorded
/// `won` flags (the `step_is_win` oracle), keyed to the situation of the
/// decision that produced the step.
pub fn learn_diary(store: &mut LessonStore, path: &Path) -> Result<(), DojoError> {
    let lines = read_diary(path)?;
    learn_lines(store, &diary_id(path), diary_stamp_ms(path), &lines);
    Ok(())
}

/// Distill parsed lines (testing seam). Idempotent: a diary already in
/// `learned_diaries` counts nothing, and a step already in `seen_steps`
/// (credited live by the dojo policy) is skipped.
pub fn learn_lines(store: &mut LessonStore, diary: &str, stamp_ms: u64, lines: &[DiaryLine]) {
    if !store.learned_diaries.insert(diary.to_owned()) {
        return;
    }
    // Decision seq → (clause, situation key), so steps and corrections
    // bind to the situation that produced them (multi-clause runs change
    // situations mid-run).
    let mut decision_key: BTreeMap<u32, (String, String)> = BTreeMap::new();
    // Winning (clause, action) links in order, grouped by situation key.
    let mut moves_by_key: BTreeMap<String, Vec<MoveStep>> = BTreeMap::new();

    for line in lines {
        match line {
            DiaryLine::Decision(d) => {
                let key = context_key(d.site.as_ref(), &d.situation, &d.clause);
                decision_key.insert(d.seq, (d.clause.clone(), key.clone()));
                let place = store.places.entry(key.clone()).or_default();
                place.host = d.site.as_ref().and_then(|s: &SiteLine| s.host.clone());
                place.path = d.site.as_ref().and_then(|s| s.path.clone());
                place.roles = d.situation.roles.clone();
                place.front_layer = d.situation.front_layer;
                push_diary(&mut place.diaries, diary);
            }
            DiaryLine::Step(s) => {
                // Skip steps already credited live and kinds that carry no
                // label evidence: control steps (scroll/wait/done/blocked)
                // never resolve a clause target, and an unknown verdict is
                // unreadable state, not a loss.
                let step_key = format!("{diary}:{}", s.step);
                if store.seen_steps.contains(&step_key)
                    || ActionKind::parse(&s.kind).is_some_and(ActionKind::is_control)
                    || s.verification == "unknown"
                {
                    continue;
                }
                let Some(key) = decision_key
                    .range(..=s.seq)
                    .next_back()
                    .map(|(_, (_, k))| k.clone())
                else {
                    continue;
                };
                let trust = store
                    .trust
                    .entry(key.clone())
                    .or_default()
                    .entry(s.label.clone())
                    .or_default();
                if s.won {
                    trust.wins += 1;
                    moves_by_key.entry(key.clone()).or_default().push(MoveStep {
                        clause: s.clause.clone(),
                        action_id: s.action_id.clone(),
                        label: s.label.clone(),
                    });
                    // The clause phrase resolved to this label here.
                    let words = store.words.entry(key).or_default();
                    match words.iter_mut().find(|w| w.phrase == s.clause) {
                        Some(w) => {
                            w.label = s.label.clone();
                            push_diary(&mut w.diaries, diary);
                        }
                        None => words.push(Word {
                            phrase: s.clause.clone(),
                            label: s.label.clone(),
                            diaries: vec![diary.to_owned()],
                        }),
                    }
                } else {
                    trust.losses += 1;
                }
                trust.last_seen_ms = trust.last_seen_ms.max(stamp_ms);
                push_diary(&mut trust.diaries, diary);
                store.seen_steps.insert(step_key);
            }
            DiaryLine::Correction(c) => {
                // The strongest signal: the policy's pick was wrong, the
                // human's expectation is right. Bind to the LATEST
                // decision about this clause — the correction was written
                // against what the run most recently did, not its first
                // decision. Fallback: clause-only key via empty site.
                let key = decision_key
                    .iter()
                    .rev()
                    .find(|(_, (clause, _))| clause == &c.clause)
                    .map(|(_, (_, k))| k.clone())
                    .unwrap_or_else(|| context_key(None, &Default::default(), &c.clause));
                let trust_map = store.trust.entry(key).or_default();
                let loss = trust_map.entry(c.chosen.clone()).or_default();
                loss.losses += 1;
                loss.last_seen_ms = loss.last_seen_ms.max(stamp_ms);
                push_diary(&mut loss.diaries, diary);
                let win = trust_map.entry(c.expected.clone()).or_default();
                win.wins += 1;
                win.last_seen_ms = win.last_seen_ms.max(stamp_ms);
                push_diary(&mut win.diaries, diary);
            }
            _ => {}
        }
    }

    for (key, steps) in moves_by_key {
        if steps.is_empty() {
            continue;
        }
        let moves = store.moves.entry(key).or_default();
        match moves.iter_mut().find(|m| m.steps == steps) {
            Some(m) => push_diary(&mut m.diaries, diary),
            None => moves.push(Move {
                steps,
                diaries: vec![diary.to_owned()],
            }),
        }
    }
}

/// Write the store as one versioned JSON document.
pub fn save(store: &LessonStore, path: &Path) -> Result<(), DojoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| DojoError::Io {
            path: parent.to_path_buf(),
            message: format!("create lesson dir: {e}"),
        })?;
    }
    let doc = json!({
        "lessons": LESSON_SCHEMA,
        "words": words_json(&store.words),
        "places": places_json(&store.places),
        "moves": moves_json(&store.moves),
        "trust": trust_json(&store.trust),
        "learned_diaries": store.learned_diaries,
        "seen_steps": store.seen_steps,
    });
    let text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| doc.to_string());
    fs::write(path, format!("{text}\n")).map_err(|e| DojoError::Io {
        path: path.to_path_buf(),
        message: format!("write lessons: {e}"),
    })
}

/// Load a store; [`DojoError::Schema`] on a version mismatch. Missing file
/// yields an empty store — first run of a task has no lessons yet.
pub fn load(path: &Path) -> Result<LessonStore, DojoError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LessonStore::default());
        }
        Err(e) => {
            return Err(DojoError::Io {
                path: path.to_path_buf(),
                message: format!("read lessons: {e}"),
            })
        }
    };
    let doc: Value = serde_json::from_str(&text).map_err(|e| DojoError::Parse {
        line: 0,
        message: format!("lessons JSON: {e}"),
    })?;
    let obj = doc.as_object().ok_or_else(|| DojoError::Parse {
        line: 0,
        message: "lessons must be an object".to_owned(),
    })?;
    let schema = obj.get("lessons").and_then(Value::as_u64).unwrap_or(0);
    if schema != LESSON_SCHEMA as u64 {
        return Err(DojoError::Schema { found: schema });
    }
    Ok(LessonStore {
        words: parse_words(obj.get("words")),
        places: parse_places(obj.get("places")),
        moves: parse_moves(obj.get("moves")),
        trust: parse_trust(obj.get("trust")),
        learned_diaries: str_list(obj.get("learned_diaries")).into_iter().collect(),
        seen_steps: str_list(obj.get("seen_steps")).into_iter().collect(),
    })
}

fn push_diary(diaries: &mut Vec<String>, diary: &str) {
    if !diaries.iter().any(|d| d == diary) {
        diaries.push(diary.to_owned());
        diaries.sort();
    }
}

fn opt_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn words_json(words: &BTreeMap<String, Vec<Word>>) -> Value {
    let mut obj = Map::new();
    for (key, ws) in words {
        obj.insert(
            key.clone(),
            Value::Array(
                ws.iter()
                    .map(|w| json!({"phrase": w.phrase, "label": w.label, "diaries": w.diaries}))
                    .collect(),
            ),
        );
    }
    Value::Object(obj)
}

fn places_json(places: &BTreeMap<String, Place>) -> Value {
    let mut obj = Map::new();
    for (key, p) in places {
        let mut v = json!({"roles": p.roles, "front_layer": p.front_layer, "diaries": p.diaries});
        if let Some(h) = &p.host {
            v["host"] = json!(h);
        }
        if let Some(p2) = &p.path {
            v["path"] = json!(p2);
        }
        obj.insert(key.clone(), v);
    }
    Value::Object(obj)
}

fn moves_json(moves: &BTreeMap<String, Vec<Move>>) -> Value {
    let mut obj = Map::new();
    for (key, ms) in moves {
        obj.insert(
            key.clone(),
            Value::Array(
                ms.iter()
                    .map(|m| {
                        json!({
                            "steps": m.steps.iter().map(|s| json!({
                                "clause": s.clause,
                                "action_id": s.action_id,
                                "label": s.label,
                            })).collect::<Vec<_>>(),
                            "diaries": m.diaries,
                        })
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(obj)
}

fn trust_json(trust: &BTreeMap<String, BTreeMap<String, Trust>>) -> Value {
    let mut obj = Map::new();
    for (key, labels) in trust {
        let mut inner = Map::new();
        for (label, t) in labels {
            inner.insert(
                label.clone(),
                json!({
                    "wins": t.wins,
                    "losses": t.losses,
                    "last_seen_ms": t.last_seen_ms,
                    "diaries": t.diaries,
                }),
            );
        }
        obj.insert(key.clone(), Value::Object(inner));
    }
    Value::Object(obj)
}

fn parse_words(v: Option<&Value>) -> BTreeMap<String, Vec<Word>> {
    let mut out = BTreeMap::new();
    let Some(obj) = v.and_then(Value::as_object) else {
        return out;
    };
    for (key, list) in obj {
        let words = list
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(Word {
                            phrase: e.get("phrase")?.as_str()?.to_owned(),
                            label: e.get("label")?.as_str()?.to_owned(),
                            diaries: str_list(e.get("diaries")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(key.clone(), words);
    }
    out
}

fn parse_places(v: Option<&Value>) -> BTreeMap<String, Place> {
    let mut out = BTreeMap::new();
    let Some(obj) = v.and_then(Value::as_object) else {
        return out;
    };
    for (key, e) in obj {
        out.insert(
            key.clone(),
            Place {
                host: opt_str(e.get("host")),
                path: opt_str(e.get("path")),
                roles: str_list(e.get("roles")),
                front_layer: e
                    .get("front_layer")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                diaries: str_list(e.get("diaries")),
            },
        );
    }
    out
}

fn parse_moves(v: Option<&Value>) -> BTreeMap<String, Vec<Move>> {
    let mut out = BTreeMap::new();
    let Some(obj) = v.and_then(Value::as_object) else {
        return out;
    };
    for (key, list) in obj {
        let moves = list
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let steps = e
                            .get("steps")?
                            .as_array()?
                            .iter()
                            .filter_map(|s| {
                                Some(MoveStep {
                                    clause: s.get("clause")?.as_str()?.to_owned(),
                                    action_id: s.get("action_id")?.as_str()?.to_owned(),
                                    label: s.get("label")?.as_str()?.to_owned(),
                                })
                            })
                            .collect();
                        Some(Move {
                            steps,
                            diaries: str_list(e.get("diaries")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(key.clone(), moves);
    }
    out
}

fn parse_trust(v: Option<&Value>) -> BTreeMap<String, BTreeMap<String, Trust>> {
    let mut out = BTreeMap::new();
    let Some(obj) = v.and_then(Value::as_object) else {
        return out;
    };
    for (key, inner) in obj {
        let mut labels = BTreeMap::new();
        if let Some(map) = inner.as_object() {
            for (label, e) in map {
                labels.insert(
                    label.clone(),
                    Trust {
                        wins: e.get("wins").and_then(Value::as_u64).unwrap_or(0) as u32,
                        losses: e.get("losses").and_then(Value::as_u64).unwrap_or(0) as u32,
                        last_seen_ms: e.get("last_seen_ms").and_then(Value::as_u64).unwrap_or(0),
                        diaries: str_list(e.get("diaries")),
                    },
                );
            }
        }
        out.insert(key.clone(), labels);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{DecisionLine, OfferedLine, Situation, StepLine};

    fn decision(seq: u32, clause: &str) -> DiaryLine {
        DiaryLine::Decision(Box::new(DecisionLine {
            seq,
            clause_index: 0,
            clause: clause.to_owned(),
            mode: "act".to_owned(),
            source: "instinct".to_owned(),
            site: Some(SiteLine {
                url: Some("https://x.test/p".to_owned()),
                title: Some("p".to_owned()),
                host: Some("x.test".to_owned()),
                path: Some("/p".to_owned()),
            }),
            situation: Default::default(),
            offered: vec![OfferedLine {
                id: "CLICK:go".to_owned(),
                kind: "CLICK".to_owned(),
                label: "Go".to_owned(),
                role: Some("button".to_owned()),
                region: Some("go".to_owned()),
                fingerprint: 1,
                state: None,
            }],
            operation_ranked: vec![],
            target_ranked: vec![],
            history: vec![],
            choice: None,
            abstain: None,
        }))
    }

    fn step(seq: u32, clause: &str, won: bool) -> DiaryLine {
        DiaryLine::Step(StepLine {
            seq,
            step: seq,
            clause_index: 0,
            clause: clause.to_owned(),
            action_id: "CLICK:go".to_owned(),
            kind: "CLICK".to_owned(),
            label: "Go".to_owned(),
            input: "click".to_owned(),
            payload: None,
            verification: if won { "state-changed" } else { "no-effect" }.to_owned(),
            stale_retries: 0,
            won,
        })
    }

    #[test]
    fn learn_builds_words_places_moves_trust() {
        let mut store = LessonStore::default();
        let lines = vec![
            decision(1, "Click Go"),
            step(2, "Click Go", true),
            decision(3, "Click Go"),
            step(4, "Click Go", false),
        ];
        learn_lines(&mut store, "1-run", 1000, &lines);
        assert_eq!(store.places.len(), 1);
        let key = store.places.keys().next().unwrap().clone();
        assert_eq!(store.places[&key].diaries, ["1-run"]);
        let trust = &store.trust[&key]["Go"];
        assert_eq!((trust.wins, trust.losses, trust.last_seen_ms), (1, 1, 1000));
        assert_eq!(store.words[&key][0].phrase, "Click Go");
        assert_eq!(store.words[&key][0].label, "Go");
        assert_eq!(store.moves[&key].len(), 1);
        assert_eq!(store.moves[&key][0].steps[0].action_id, "CLICK:go");
    }

    #[test]
    fn distilling_one_diary_twice_counts_nothing_twice() {
        let mut store = LessonStore::default();
        let lines = vec![decision(1, "Click Go"), step(2, "Click Go", true)];
        learn_lines(&mut store, "1-run", 1000, &lines);
        learn_lines(&mut store, "1-run", 1000, &lines);
        let key = store.trust.keys().next().unwrap().clone();
        assert_eq!(store.trust[&key]["Go"].wins, 1);
        assert!(store.learned_diaries.contains("1-run"));
        assert!(store.seen_steps.contains("1-run:2"));
    }

    #[test]
    fn a_step_credited_live_is_skipped_offline() {
        let mut store = LessonStore::default();
        // The live dojo already folded step 2 into trust.
        store.seen_steps.insert("1-run:2".to_owned());
        let lines = vec![decision(1, "Click Go"), step(2, "Click Go", true)];
        learn_lines(&mut store, "1-run", 1000, &lines);
        assert!(store.trust.is_empty(), "no double credit");
        assert!(store.moves.is_empty(), "no double move");
        assert_eq!(store.places.len(), 1, "the situation itself still records");
    }

    #[test]
    fn identical_moves_dedupe_and_accumulate_diaries() {
        let mut store = LessonStore::default();
        let lines = vec![decision(1, "Click Go"), step(2, "Click Go", true)];
        learn_lines(&mut store, "1-a", 1, &lines);
        learn_lines(&mut store, "2-b", 2, &lines);
        let key = store.moves.keys().next().unwrap().clone();
        assert_eq!(store.moves[&key].len(), 1, "identical routine dedupes");
        assert_eq!(store.moves[&key][0].diaries, ["1-a", "2-b"]);
        let trust = &store.trust[&key]["Go"];
        assert_eq!(trust.wins, 2);
        assert_eq!(trust.last_seen_ms, 2);
    }

    #[test]
    fn save_load_round_trip_and_schema_guard() {
        let mut store = LessonStore::default();
        learn_lines(
            &mut store,
            "1-run",
            9,
            &[decision(1, "Click Go"), step(2, "Click Go", true)],
        );
        let dir =
            std::env::temp_dir().join(format!("ultra-instinct-lessons-{}", std::process::id()));
        let path = dir.join("lessons.json");
        save(&store, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, store);
        // Version mismatch is a schema error, never a silent misread.
        fs::write(&path, "{\"lessons\":0}\n").unwrap();
        assert!(matches!(load(&path), Err(DojoError::Schema { found: 0 })));
        // Missing file = empty store (first run of a task).
        assert_eq!(
            load(&dir.join("missing.json")).unwrap(),
            LessonStore::default()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A correction targets the run's LATEST decision about its clause —
    /// the page shape it was written against — not the first sighting.
    #[test]
    fn a_correction_binds_to_the_latest_decision_of_its_clause() {
        let site = SiteLine {
            url: Some("https://x.test/p".to_owned()),
            title: Some("p".to_owned()),
            host: Some("x.test".to_owned()),
            path: Some("/p".to_owned()),
        };
        let decision_on = |seq: u32, clause: &str, front: bool| {
            let mut line = decision(seq, clause);
            if let DiaryLine::Decision(d) = &mut line {
                d.situation.front_layer = front;
            }
            line
        };
        let lines = vec![
            decision_on(1, "Click Go", false),
            decision_on(2, "Click Go", true),
            DiaryLine::Correction(crate::CorrectionLine {
                clause: "Click Go".to_owned(),
                expected: "Go".to_owned(),
                chosen: "No".to_owned(),
            }),
        ];
        let mut store = LessonStore::default();
        learn_lines(&mut store, "1-run", 1000, &lines);

        let situation = |front: bool| Situation {
            front_layer: front,
            ..Default::default()
        };
        let first_key = context_key(Some(&site), &situation(false), "Click Go");
        let last_key = context_key(Some(&site), &situation(true), "Click Go");
        assert_ne!(first_key, last_key, "different situations, different keys");
        assert_eq!(store.trust[&last_key]["Go"].wins, 1);
        assert_eq!(store.trust[&last_key]["No"].losses, 1);
        let first = store.trust.get(&first_key);
        assert!(
            !first.is_some_and(|t| t.contains_key("Go") || t.contains_key("No")),
            "the first decision's situation stays untouched"
        );
    }
}
