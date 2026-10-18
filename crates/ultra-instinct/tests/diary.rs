//! Golden diary tests (issue #49, work item 1): `--diary <dir>` writes one
//! schema-v1 JSONL per run — asserted on a checked-in CDP-replay fixture —
//! and every written line round-trips through `parse_line`.

use ultra_instinct_dojo::{parse_line, read_diary, DiaryLine};

fn fixture(name: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_owned()
}

fn seq_of(line: &DiaryLine) -> Option<u32> {
    match line {
        DiaryLine::Decision(l) => Some(l.seq),
        DiaryLine::Step(l) => Some(l.seq),
        DiaryLine::StaleDiscard(l) => Some(l.seq),
        DiaryLine::ClauseAdvanced(l) => Some(l.seq),
        _ => None,
    }
}

#[test]
fn diary_records_a_cdp_replay_run() {
    let fixture = fixture("agent-click-go.cdp.json");
    if !std::path::Path::new(&fixture).exists() {
        eprintln!("skip missing fixture {fixture}");
        return;
    }
    let dir = std::env::temp_dir().join(format!("ultra-instinct-diary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let out = ultra_instinct::execute(&[
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
    .unwrap_or_else(|e| panic!("{e}"));

    let path = out
        .lines()
        .find_map(|line| line.strip_prefix("diary: "))
        .unwrap_or_else(|| panic!("output carries the diary path: {out}"));
    let lines = read_diary(std::path::Path::new(path)).unwrap();

    // Golden shape: run line first, outcome line last.
    let run = match lines.first() {
        Some(DiaryLine::Run(run)) => run,
        other => panic!("first line is the run line, not {other:?}"),
    };
    assert_eq!(run.goal, "Click Go");
    assert_eq!(run.policy, "instinct");
    let outcome = match lines.last() {
        Some(DiaryLine::Outcome(outcome)) => outcome,
        other => panic!("last line is the outcome line, not {other:?}"),
    };
    assert!(
        ["done", "blocked", "abstained", "failed"].contains(&outcome.kind.as_str()),
        "{outcome:?}"
    );
    assert!(outcome.steps >= 1, "the run took steps: {outcome:?}");

    // seq is the 0-based line index inside the run — strictly increasing.
    let mut last = None;
    for line in &lines {
        if let Some(seq) = seq_of(line) {
            if let Some(prev) = last {
                assert!(seq > prev, "seq order: {prev} then {seq}");
            }
            last = Some(seq);
        }
    }

    // The first decision: source is the recorded policy arm, offered menu is
    // nonempty and contains the chosen action when it chose.
    let decision = lines
        .iter()
        .find_map(|line| match line {
            DiaryLine::Decision(d) => Some(d),
            _ => None,
        })
        .expect("a decision line");
    assert_eq!(decision.source, "instinct");
    assert_eq!(decision.clause, "Click Go");
    assert_eq!(decision.mode, "act");
    assert!(!decision.offered.is_empty());
    if let Some(choice) = &decision.choice {
        assert!(
            decision.offered.iter().any(|a| a.id == choice.action_id),
            "choice is on the recorded menu"
        );
        assert_eq!(choice.kind, "CLICK");
    } else {
        assert!(decision.abstain.is_some());
    }

    // At least one step; the click step won (verified effect + the label
    // names the clause target).
    let clicks: Vec<_> = lines
        .iter()
        .filter_map(|line| match line {
            DiaryLine::Step(s) if s.kind == "CLICK" => Some(s),
            _ => None,
        })
        .collect();
    assert!(!clicks.is_empty(), "a click step was recorded: {lines:?}");
    assert!(
        clicks.iter().any(|s| s.won && s.input == "click"),
        "the recorded click is a win: {clicks:?}"
    );

    // Every line round-trips through the untrusted-input parser.
    for line in &lines {
        let json = line.to_json();
        assert_eq!(&parse_line(&json, 1).unwrap(), line, "round-trip: {json}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn diary_flag_writes_for_dry_run_predict() {
    let dir = std::env::temp_dir().join(format!("ultra-instinct-diary-dry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.manifold");
    std::fs::write(
        &page,
        "viewport w=800 h=600\nregion id=go role=button label=\"Continue\" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility\n",
    )
    .unwrap();

    let out = ultra_instinct::execute(&[
        "run".to_owned(),
        "--goal".to_owned(),
        "Continue".to_owned(),
        "--policy".to_owned(),
        "instinct".to_owned(),
        "--fixture".to_owned(),
        page.to_str().unwrap().to_owned(),
        "--diary".to_owned(),
        dir.to_str().unwrap().to_owned(),
    ])
    .unwrap();
    let path = out
        .lines()
        .find_map(|line| line.strip_prefix("diary: "))
        .expect("dry-run also writes a diary");
    let lines = read_diary(std::path::Path::new(path)).unwrap();
    // A dry run records the run line and exactly one decision.
    assert!(matches!(lines.first(), Some(DiaryLine::Run(_))));
    assert_eq!(
        lines
            .iter()
            .filter(|l| matches!(l, DiaryLine::Decision(_)))
            .count(),
        1
    );

    let _ = std::fs::remove_dir_all(&dir);
}
