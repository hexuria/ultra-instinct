//! Golden for `aui exams`: the issue-#49 progress metric — remote calls
//! per 100 recorded decisions — plus belt reporting over the diaries.

use ultra_instinct_dojo::line::{DecisionLine, DiaryLine, Situation};

fn decision(source: &str) -> String {
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
    .to_json()
}

fn diary_dir() -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ultra-instinct-exams-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("0001-run.jsonl"),
        format!("{}\n{}\n", decision("instinct"), decision("jev")),
    )
    .unwrap();
    std::fs::write(
        dir.join("0002-run.jsonl"),
        format!("{}\n{}\n", decision("dojo"), decision("dojo")),
    )
    .unwrap();
    dir
}

#[test]
fn an_exam_reports_remote_calls_per_100_decisions_deterministically() {
    let dir = diary_dir();
    let args = ["--diary".to_owned(), dir.to_str().unwrap().to_owned()];
    let out = ultra_instinct::exams_cmd::exams_command(&args).unwrap();
    assert!(
        out.contains("4 decisions — 1 remote calls (25 per 100)"),
        "unexpected output: {out}"
    );
    assert!(out.contains("belts:"), "no belt line: {out}");
    assert_eq!(
        out,
        ultra_instinct::exams_cmd::exams_command(&args).unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
