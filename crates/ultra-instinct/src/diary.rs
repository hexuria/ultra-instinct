//! Map the agent's `JournalEvent`s into `ultra-instinct-dojo` diary lines and write
//! one JSONL file per run under `--diary <dir>`.
//!
//! The agent never touches files; all IO lives here. Line `seq` is the
//! 0-based index inside the run so replay can order a torn tail.

use std::path::{Path, PathBuf};

use ultra_instinct_agent::{AgentOutcome, JournalEvent};
use ultra_instinct_core::{ActionSpace, ElementState};
use ultra_instinct_dojo::line::{
    ChoiceLine, ClauseLine, DecisionLine, DiaryLine, HistoryLine, OfferedLine, OfferedState,
    OutcomeLine, PolicyErrorLine, RankedLine, RunLine, Situation, StaleLine, StepLine,
};
use ultra_instinct_dojo::DiaryWriter;
use ultra_instinct_policy::{PolicyOutcome, RankedAction};

use crate::CliError;

/// Drain the journal into `<dir>/<stamp>-<slug>.jsonl`; returns the path.
/// One `JournalEvent` maps to exactly one `DiaryLine`.
pub fn write_diary(dir: &Path, events: &[JournalEvent]) -> Result<PathBuf, CliError> {
    let goal = events
        .iter()
        .find_map(|event| match event {
            JournalEvent::Run { goal, .. } => Some(goal.as_str()),
            _ => None,
        })
        .unwrap_or("run");
    let mut writer = DiaryWriter::create(dir, goal).map_err(|e| CliError::Io {
        path: dir.display().to_string(),
        message: e.to_string(),
    })?;
    // Steps inherit their action's observed input type from the decision
    // that produced them — needed to keep passwords out of the file.
    let mut last_space: Option<&ActionSpace> = None;
    // A decision's `history` line is the steps journaled before it — the
    // journal no longer snapshots the vec per decision (a quadratic clone
    // every policy call), so the writer rebuilds it here. The serialized
    // `history` field is unchanged.
    let mut history_so_far: Vec<HistoryLine> = Vec::new();
    for (seq, event) in events.iter().enumerate() {
        if let JournalEvent::Decision { space, .. } = event {
            last_space = Some(space.as_ref());
        }
        writer
            .write(&map_event(event, seq as u32, last_space, &history_so_far))
            .map_err(|e| CliError::Io {
                path: writer.path().display().to_string(),
                message: e.to_string(),
            })?;
        if let JournalEvent::Step { record, .. } = event {
            history_so_far.push(HistoryLine {
                step: record.step,
                action_id: record.action_id.as_str().to_owned(),
                kind: record.kind.as_str().to_owned(),
                label: record.label.clone(),
                verification: record.verification.as_str().to_owned(),
            });
        }
    }
    Ok(writer.path().to_path_buf())
}

fn map_event(
    event: &JournalEvent,
    seq: u32,
    last_space: Option<&ActionSpace>,
    history_so_far: &[HistoryLine],
) -> DiaryLine {
    match event {
        JournalEvent::Run {
            goal,
            clauses,
            policy,
        } => DiaryLine::Run(RunLine {
            goal: goal.clone(),
            clauses: clauses.clone(),
            policy: (*policy).to_owned(),
        }),
        JournalEvent::Decision {
            clause_index,
            clause,
            mode,
            source,
            site_url,
            site_title,
            front_layer,
            roles,
            near,
            space,
            outcome,
        } => {
            let (choice, abstain, operation_ranked, target_ranked) = match outcome {
                PolicyOutcome::Choice(decision) => (
                    Some(ChoiceLine {
                        action_id: decision.action_id.as_str().to_owned(),
                        kind: decision.kind.as_str().to_owned(),
                        target_label: decision.target_label.clone(),
                        confidence_millis: decision.confidence_millis,
                    }),
                    None,
                    &decision.operation_ranked,
                    &decision.target_ranked,
                ),
                PolicyOutcome::Abstain {
                    reason,
                    operation_ranked,
                    target_ranked,
                } => (None, Some(reason.clone()), operation_ranked, target_ranked),
            };
            DiaryLine::Decision(Box::new(DecisionLine {
                seq,
                clause_index: *clause_index as u32,
                clause: clause.clone(),
                mode: (*mode).to_owned(),
                source: (*source).to_owned(),
                site: ultra_instinct_dojo::site_line(site_url.as_deref(), site_title.as_deref()),
                situation: Situation {
                    front_layer: *front_layer,
                    roles: roles.iter().map(|role| (*role).to_owned()).collect(),
                    near: near.clone(),
                },
                offered: space
                    .actions()
                    .map(|action| OfferedLine {
                        id: action.id().as_str().to_owned(),
                        kind: action.kind().as_str().to_owned(),
                        label: action.label().to_owned(),
                        role: action.role().map(|role| role.as_str().to_owned()),
                        region: action.target().map(|id| id.as_str().to_owned()),
                        fingerprint: action.target_fingerprint(),
                        state: offered_state(action.state()),
                    })
                    .collect(),
                operation_ranked: ranked_lines(operation_ranked),
                target_ranked: ranked_lines(target_ranked),
                history: history_so_far.to_vec(),
                choice,
                abstain,
            }))
        }
        JournalEvent::Step {
            clause_index,
            clause,
            input,
            won,
            record,
        } => DiaryLine::Step(StepLine {
            seq,
            step: record.step,
            clause_index: *clause_index as u32,
            clause: clause.clone(),
            action_id: record.action_id.as_str().to_owned(),
            kind: record.kind.as_str().to_owned(),
            label: record.label.clone(),
            input: (*input).to_owned(),
            // A payload bound for a password input is a secret — the
            // diary keeps only that something was typed.
            payload: if last_space
                .and_then(|space| space.get(&record.action_id))
                .is_some_and(|a| a.state().input_type.as_deref() == Some("password"))
            {
                record.payload.as_ref().map(|_| "[redacted]".to_owned())
            } else {
                record.payload.clone()
            },
            verification: record.verification.as_str().to_owned(),
            stale_retries: record.stale_retries,
            won: *won,
        }),
        JournalEvent::PolicyError {
            clause_index,
            clause,
            source,
            error,
        } => DiaryLine::PolicyError(PolicyErrorLine {
            seq,
            clause_index: *clause_index as u32,
            clause: clause.clone(),
            source: (*source).to_owned(),
            error: error.clone(),
        }),
        JournalEvent::StaleDiscard { reason } => DiaryLine::StaleDiscard(StaleLine {
            seq,
            reason: reason.clone(),
        }),
        JournalEvent::ClauseAdvanced {
            clause_index,
            clause,
        } => DiaryLine::ClauseAdvanced(ClauseLine {
            seq,
            clause_index: *clause_index as u32,
            clause: clause.clone(),
        }),
        JournalEvent::Finished {
            outcome,
            policy_calls,
            stale_discards,
            duration_ms,
        } => {
            let (kind, reason) = match outcome {
                AgentOutcome::Done { reason, .. } => ("done", reason.clone()),
                AgentOutcome::Blocked { reason, .. } => ("blocked", reason.clone()),
                AgentOutcome::Abstained { reason, .. } => ("abstained", reason.clone()),
                AgentOutcome::Failed { error, .. } => ("failed", error.clone()),
            };
            DiaryLine::Outcome(OutcomeLine {
                kind: kind.to_owned(),
                reason,
                steps: outcome.steps().len() as u32,
                policy_calls: *policy_calls,
                stale_discards: *stale_discards,
                duration_ms: *duration_ms,
            })
        }
    }
}

fn ranked_lines(ranked: &[RankedAction]) -> Vec<RankedLine> {
    ranked
        .iter()
        .map(|entry| RankedLine {
            id: entry.id.as_str().to_owned(),
            kind: entry.kind.as_str().to_owned(),
            label: entry.label.clone(),
            confidence_millis: entry.confidence_millis,
        })
        .collect()
}

fn offered_state(state: &ElementState) -> Option<OfferedState> {
    if state.is_empty() {
        return None;
    }
    Some(OfferedState {
        value: state.value.clone(),
        checked: state.checked,
        expanded: state.expanded,
        selected: state.selected.clone(),
        options: state.options.clone(),
        input_type: state.input_type.clone(),
    })
}
