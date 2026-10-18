//! Golden replay tests (issue #49, work item 2): the arena re-scores
//! recorded decisions deterministically — a checked-in divergence diary
//! pins agree / would-abstain / regress, and a fresh `--diary` run replays
//! to full agreement.

fn fixture(name: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn checked_in_divergence_diary_reports_all_verdicts() {
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-replay-fixture-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        fixture("diary-divergence.jsonl"),
        dir.join("divergence.jsonl"),
    )
    .unwrap();

    let out = ultra_instinct::execute(&[
        "replay".to_owned(),
        "--diary".to_owned(),
        dir.to_str().unwrap().to_owned(),
    ])
    .unwrap();

    assert!(out.contains("agree 1, would-abstain 1, regress 2"), "{out}");
    // Each divergence names the recorded verdict and the replayed one.
    assert!(
        out.contains("regress \"Click Go\": recorded CLICK:cancel"),
        "{out}"
    );
    assert!(
        out.contains("regress \"Click Go\": recorded abstain"),
        "{out}"
    );
    assert!(out.contains("would-abstain \"Click Go\""), "{out}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fresh_instinct_diary_replays_to_full_agreement() {
    let fixture = fixture("agent-click-go.cdp.json");
    if !std::path::Path::new(&fixture).exists() {
        eprintln!("skip missing fixture {fixture}");
        return;
    }
    let dir =
        std::env::temp_dir().join(format!("ultra-instinct-replay-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    ultra_instinct::execute(&[
        "run".to_owned(),
        "--goal".to_owned(),
        "Click Go".to_owned(),
        "--policy".to_owned(),
        "instinct".to_owned(),
        "--fixture".to_owned(),
        fixture,
        "--diary".to_owned(),
        dir.to_str().unwrap().to_owned(),
    ])
    .unwrap();

    let out = ultra_instinct::execute(&[
        "replay".to_owned(),
        "--diary".to_owned(),
        dir.to_str().unwrap().to_owned(),
    ])
    .unwrap();
    assert!(
        out.contains("2 decisions — agree 2, would-abstain 0, regress 0"),
        "{out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Learned trust enters scoring only through the capped evidence path
/// (issue #49, work item 4): a store that agrees with the recorded runs
/// must leave every verdict untouched — replay no-regressions with
/// evidence present. The bounded flip lives in ultra-instinct-policy unit tests.
#[test]
fn a_lesson_store_does_not_regress_recorded_runs() {
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-replay-lessons-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        fixture("diary-divergence.jsonl"),
        dir.join("divergence.jsonl"),
    )
    .unwrap();

    // Trust for the fixture's situation key: "Go" with a winning streak
    // learned from a proving diary. Bounded evidence, applied on every
    // decision — and the recorded verdicts must not move.
    let situation = ultra_instinct_dojo::Situation::default();
    let key = ultra_instinct_dojo::context_key(None, &situation, "Click Go");
    let mut store = ultra_instinct_dojo::LessonStore::default();
    store.trust.entry(key).or_default().insert(
        "Go".to_owned(),
        ultra_instinct_dojo::Trust {
            wins: 9,
            losses: 0,
            last_seen_ms: 1_791_000_000_000,
            diaries: vec!["training-run".to_owned()],
        },
    );
    let store_path = dir.join("lessons.json");
    ultra_instinct_dojo::save_lessons(&store, &store_path).unwrap();

    let with_lessons = ultra_instinct::execute(&[
        "replay".to_owned(),
        "--diary".to_owned(),
        dir.to_str().unwrap().to_owned(),
        "--lessons".to_owned(),
        store_path.to_str().unwrap().to_owned(),
    ])
    .unwrap();
    let baseline = ultra_instinct::execute(&[
        "replay".to_owned(),
        "--diary".to_owned(),
        dir.to_str().unwrap().to_owned(),
    ])
    .unwrap();

    for line in baseline.lines() {
        assert!(
            with_lessons.contains(line),
            "verdict moved: {line}\n{with_lessons}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
