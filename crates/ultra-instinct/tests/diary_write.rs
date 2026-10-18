//! Diary write-path tests: password payloads are redacted on the wire
//! and a failed policy call lands as a `policy_error` line.

use ultra_instinct_agent::{JournalEvent, StepRecord, VerificationKind};
use ultra_instinct_core::{
    Action, ActionId, ActionKind, ActionSpace, ElementState, InteractionManifold,
    InteractionRegion, Rect, RegionFlags, RegionId, RegionParts, Role, SourceMask, UnitInterval,
};
use ultra_instinct_dojo::{read_diary, DiaryLine};
use ultra_instinct_policy::{PolicyDecision, PolicyOutcome};

use ultra_instinct::diary::write_diary;

fn field_space(input_type: Option<&str>) -> ActionSpace {
    let state = ElementState {
        input_type: input_type.map(str::to_owned),
        ..ElementState::default()
    };
    let region = InteractionRegion::try_new(RegionParts {
        id: RegionId::try_new("pw").unwrap(),
        role: Role::TextField,
        label: "Password".to_owned(),
        rect: Rect::try_new(10.0, 10.0, 80.0, 24.0).unwrap(),
        actions: vec![Action::Type, Action::Click],
        parent: None,
        sources: SourceMask::DOM,
        flags: RegionFlags::none(),
        temporal_stability: UnitInterval::ONE,
    })
    .unwrap()
    .with_state(state);
    let manifold = InteractionManifold::try_new(
        Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap(),
        vec![region],
        0,
    )
    .unwrap();
    ActionSpace::from_manifold(&manifold)
}

fn decision(space: ActionSpace) -> JournalEvent {
    JournalEvent::Decision {
        clause_index: 0,
        clause: "Type the password".to_owned(),
        mode: "act",
        source: "instinct",
        site_url: None,
        site_title: None,
        front_layer: false,
        roles: vec!["textfield"],
        near: Vec::new(),
        space: std::sync::Arc::new(space),
        outcome: PolicyOutcome::Choice(PolicyDecision {
            action_id: ActionId::try_new("TYPE_TEXT:pw").unwrap(),
            kind: ActionKind::TypeText,
            target_label: "Password".to_owned(),
            confidence_millis: 900,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }),
    }
}

fn type_step(payload: Option<&str>) -> JournalEvent {
    JournalEvent::Step {
        clause_index: 0,
        clause: "Type the password".to_owned(),
        input: "type",
        won: true,
        record: StepRecord {
            step: 1,
            action_id: ActionId::try_new("TYPE_TEXT:pw").unwrap(),
            kind: ActionKind::TypeText,
            label: "Password".to_owned(),
            payload: payload.map(str::to_owned),
            verification: VerificationKind::Success,
            stale_retries: 0,
        },
    }
}

fn write(events: &[JournalEvent]) -> Vec<ultra_instinct_dojo::DiaryLine> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-diary-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let path = write_diary(&dir, events).unwrap();
    let lines = read_diary(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    lines
}

/// A payload bound for a password input is a secret — the diary
/// records only that something was typed.
#[test]
fn a_password_payload_is_redacted() {
    let lines = write(&[
        JournalEvent::Run {
            goal: "Type the password".to_owned(),
            clauses: vec!["Type the password".to_owned()],
            policy: "instinct",
        },
        decision(field_space(Some("password"))),
        type_step(Some("hunter2")),
    ]);
    let DiaryLine::Step(step) = lines
        .iter()
        .find(|l| matches!(l, DiaryLine::Step(_)))
        .expect("a step line")
    else {
        unreachable!()
    };
    assert_eq!(step.payload.as_deref(), Some("[redacted]"));
}

/// Any other input type keeps its payload — replay and lessons need it.
#[test]
fn an_ordinary_payload_survives() {
    let lines = write(&[
        JournalEvent::Run {
            goal: "Type the password".to_owned(),
            clauses: vec!["Type the password".to_owned()],
            policy: "instinct",
        },
        decision(field_space(Some("text"))),
        type_step(Some("Ana")),
    ]);
    let DiaryLine::Step(step) = lines
        .iter()
        .find(|l| matches!(l, DiaryLine::Step(_)))
        .expect("a step line")
    else {
        unreachable!()
    };
    assert_eq!(step.payload.as_deref(), Some("Ana"));
}

/// A policy that errored still lands in the diary as `policy_error`.
#[test]
fn a_failed_policy_call_is_journaled() {
    let lines = write(&[
        JournalEvent::Run {
            goal: "Click".to_owned(),
            clauses: vec!["Click".to_owned()],
            policy: "instinct",
        },
        JournalEvent::PolicyError {
            clause_index: 0,
            clause: "Click".to_owned(),
            source: "instinct",
            error: "policy internal: boom".to_owned(),
        },
    ]);
    let DiaryLine::PolicyError(line) = lines
        .iter()
        .find(|l| matches!(l, DiaryLine::PolicyError(_)))
        .expect("a policy_error line")
    else {
        unreachable!()
    };
    assert_eq!(line.error, "policy internal: boom");
    assert_eq!(line.source, "instinct");
}
