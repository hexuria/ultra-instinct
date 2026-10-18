//! Battle diary line schema (`schema: 1`): one JSON object per line.
//!
//! Every line carries `"schema": 1` and `"type"`. The parser validates the
//! schema and refuses unknown types, so a newer diary never feeds an older
//! replay. Emission goes through [`DiaryLine::to_json`]; untrusted input goes
//! through [`parse_line`], which field-validates like the remote wire.
//!
//! What is never recorded: password values (observe never reads them) and
//! anything a human corrected — `correction` lines are written only by tools
//! outside the agent loop, for the strongest lesson signal.

use serde_json::{json, Map, Value};

use crate::error::DojoError;

/// The only schema version this build reads and writes.
pub const SCHEMA: u32 = 1;

/// One diary line.
#[derive(Clone, Debug, PartialEq)]
pub enum DiaryLine {
    /// One per run: goal, split clauses, deciding policy.
    Run(RunLine),
    /// One per policy decision: offered menu, ranked candidates, verdict.
    ///
    /// Boxed: this variant carries the whole offered menu (~430 bytes) while
    /// the others are small — keeps `DiaryLine` itself cheap to move around
    /// the replay arena.
    Decision(Box<DecisionLine>),
    /// One per executed step: input kind, payload, verification, win flag.
    Step(StepLine),
    /// A prediction discarded because the ticket caught drift.
    StaleDiscard(StaleLine),
    /// A `then` clause boundary crossed.
    ClauseAdvanced(ClauseLine),
    /// One per run, last: outcome kind, budgets, wall time.
    Outcome(OutcomeLine),
    /// A human correction (never written by the agent itself).
    Correction(CorrectionLine),
    /// A policy call that errored before producing an outcome — the
    /// decision is still journaled so the diary shows the failure.
    PolicyError(PolicyErrorLine),
}

/// Where the decision happened (URL-derived; absent on fixtures/mocks).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteLine {
    pub url: Option<String>,
    pub title: Option<String>,
    pub host: Option<String>,
    /// Path with every all-digits segment collapsed to `{n}`.
    pub path: Option<String>,
}

/// Page-situation summary attached to a decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Situation {
    /// A front layer (dialog) was blocking regions.
    pub front_layer: bool,
    /// Sorted unique role names present on the page.
    pub roles: Vec<String>,
    /// Labels of regions near the chosen / top-ranked target.
    pub near: Vec<String>,
}

/// One offered action from the finite `ActionSpace`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferedLine {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub role: Option<String>,
    /// Region this action binds (`None` for page-level controls). Lets the
    /// replay arena rebuild a target-bound `ObservedAction`.
    pub region: Option<String>,
    /// Region fingerprint bits (0 for controls).
    pub fingerprint: u64,
    /// Observed control state, when present.
    pub state: Option<OfferedState>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OfferedState {
    pub value: Option<String>,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub selected: Option<String>,
    pub options: Vec<String>,
    /// INPUT's `type`, lowercased — marks sensitive fields (`password`)
    /// without recording their values.
    pub input_type: Option<String>,
}

/// One ranked candidate as the policy saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedLine {
    pub id: String,
    pub kind: String,
    pub label: String,
    /// Instinct confidence millis 0..=1000 (not a probability).
    pub confidence_millis: i16,
}

/// An executed history entry as the policy saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLine {
    pub step: u32,
    pub action_id: String,
    pub kind: String,
    pub label: String,
    pub verification: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunLine {
    pub goal: String,
    pub clauses: Vec<String>,
    pub policy: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionLine {
    /// 0-based line index inside the run.
    pub seq: u32,
    pub clause_index: u32,
    pub clause: String,
    /// `act` | `optional` | `optional-while` | `wait-for`.
    pub mode: String,
    /// `instinct` | `jev` | `clef` | `clef-flash` | `escalating:<arm>` | …
    pub source: String,
    pub site: Option<SiteLine>,
    pub situation: Situation,
    pub offered: Vec<OfferedLine>,
    pub operation_ranked: Vec<RankedLine>,
    pub target_ranked: Vec<RankedLine>,
    pub history: Vec<HistoryLine>,
    /// Chosen action; `None` when the policy abstained.
    pub choice: Option<ChoiceLine>,
    /// Abstain reason; `None` when the policy chose.
    pub abstain: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceLine {
    pub action_id: String,
    pub kind: String,
    pub target_label: String,
    pub confidence_millis: i16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StepLine {
    pub seq: u32,
    pub step: u32,
    pub clause_index: u32,
    pub clause: String,
    pub action_id: String,
    pub kind: String,
    pub label: String,
    /// `click` | `pointer` | `type` | `select` | `scroll-up` | `scroll-down` | `wait`.
    pub input: String,
    /// TYPE_TEXT / SELECT payload. `"[redacted]"` when the target was a
    /// password input — the secret never reaches the diary.
    pub payload: Option<String>,
    /// `success` | `state-changed` | `navigation` | `no-effect` | …
    pub verification: String,
    pub stale_retries: u32,
    /// The step counted as a verified win for its clause (see `ultra_instinct_agent::win`).
    pub won: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleLine {
    pub seq: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClauseLine {
    pub seq: u32,
    pub clause_index: u32,
    pub clause: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutcomeLine {
    /// `done` | `blocked` | `abstained` | `failed`.
    pub kind: String,
    pub reason: String,
    pub steps: u32,
    pub policy_calls: u32,
    pub stale_discards: u32,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrectionLine {
    /// Clause the correction applies to.
    pub clause: String,
    /// What the human says the policy should have chosen.
    pub expected: String,
    /// What the policy actually chose ("" for abstain).
    pub chosen: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyErrorLine {
    pub seq: u32,
    pub clause_index: u32,
    pub clause: String,
    /// The arm that errored (`instinct`, `jev`, `clef-flash`, …).
    pub source: String,
    /// The policy's error text.
    pub error: String,
}

impl DiaryLine {
    /// Serialize to one JSON object (no trailing newline).
    pub fn to_json(&self) -> String {
        let mut obj = Map::new();
        obj.insert("schema".to_owned(), json!(SCHEMA));
        match self {
            Self::Run(line) => {
                obj.insert("type".to_owned(), json!("run"));
                obj.insert("goal".to_owned(), json!(line.goal));
                obj.insert("clauses".to_owned(), json!(line.clauses));
                obj.insert("policy".to_owned(), json!(line.policy));
            }
            Self::Decision(line) => {
                obj.insert("type".to_owned(), json!("decision"));
                obj.insert("seq".to_owned(), json!(line.seq));
                obj.insert("clause_index".to_owned(), json!(line.clause_index));
                obj.insert("clause".to_owned(), json!(line.clause));
                obj.insert("mode".to_owned(), json!(line.mode));
                obj.insert("source".to_owned(), json!(line.source));
                if let Some(site) = &line.site {
                    let mut s = Map::new();
                    if let Some(v) = &site.url {
                        s.insert("url".to_owned(), json!(v));
                    }
                    if let Some(v) = &site.title {
                        s.insert("title".to_owned(), json!(v));
                    }
                    if let Some(v) = &site.host {
                        s.insert("host".to_owned(), json!(v));
                    }
                    if let Some(v) = &site.path {
                        s.insert("path".to_owned(), json!(v));
                    }
                    obj.insert("site".to_owned(), Value::Object(s));
                }
                obj.insert("situation".to_owned(), situation_json(&line.situation));
                obj.insert(
                    "offered".to_owned(),
                    line.offered.iter().map(offered_json).collect(),
                );
                obj.insert(
                    "operation_ranked".to_owned(),
                    line.operation_ranked.iter().map(ranked_json).collect(),
                );
                obj.insert(
                    "target_ranked".to_owned(),
                    line.target_ranked.iter().map(ranked_json).collect(),
                );
                obj.insert(
                    "history".to_owned(),
                    line.history.iter().map(history_json).collect(),
                );
                if let Some(choice) = &line.choice {
                    obj.insert(
                        "choice".to_owned(),
                        json!({
                            "action_id": choice.action_id,
                            "kind": choice.kind,
                            "target_label": choice.target_label,
                            "confidence_millis": choice.confidence_millis,
                        }),
                    );
                }
                if let Some(reason) = &line.abstain {
                    obj.insert("abstain".to_owned(), json!(reason));
                }
            }
            Self::Step(line) => {
                obj.insert("type".to_owned(), json!("step"));
                obj.insert("seq".to_owned(), json!(line.seq));
                obj.insert("step".to_owned(), json!(line.step));
                obj.insert("clause_index".to_owned(), json!(line.clause_index));
                obj.insert("clause".to_owned(), json!(line.clause));
                obj.insert("action_id".to_owned(), json!(line.action_id));
                obj.insert("kind".to_owned(), json!(line.kind));
                obj.insert("label".to_owned(), json!(line.label));
                obj.insert("input".to_owned(), json!(line.input));
                if let Some(payload) = &line.payload {
                    obj.insert("payload".to_owned(), json!(payload));
                }
                obj.insert("verification".to_owned(), json!(line.verification));
                obj.insert("stale_retries".to_owned(), json!(line.stale_retries));
                obj.insert("won".to_owned(), json!(line.won));
            }
            Self::StaleDiscard(line) => {
                obj.insert("type".to_owned(), json!("stale_discard"));
                obj.insert("seq".to_owned(), json!(line.seq));
                obj.insert("reason".to_owned(), json!(line.reason));
            }
            Self::ClauseAdvanced(line) => {
                obj.insert("type".to_owned(), json!("clause_advanced"));
                obj.insert("seq".to_owned(), json!(line.seq));
                obj.insert("clause_index".to_owned(), json!(line.clause_index));
                obj.insert("clause".to_owned(), json!(line.clause));
            }
            Self::Outcome(line) => {
                obj.insert("type".to_owned(), json!("outcome"));
                obj.insert("kind".to_owned(), json!(line.kind));
                obj.insert("reason".to_owned(), json!(line.reason));
                obj.insert("steps".to_owned(), json!(line.steps));
                obj.insert("policy_calls".to_owned(), json!(line.policy_calls));
                obj.insert("stale_discards".to_owned(), json!(line.stale_discards));
                obj.insert("duration_ms".to_owned(), json!(line.duration_ms));
            }
            Self::Correction(line) => {
                obj.insert("type".to_owned(), json!("correction"));
                obj.insert("clause".to_owned(), json!(line.clause));
                obj.insert("expected".to_owned(), json!(line.expected));
                obj.insert("chosen".to_owned(), json!(line.chosen));
            }
            Self::PolicyError(line) => {
                obj.insert("type".to_owned(), json!("policy_error"));
                obj.insert("seq".to_owned(), json!(line.seq));
                obj.insert("clause_index".to_owned(), json!(line.clause_index));
                obj.insert("clause".to_owned(), json!(line.clause));
                obj.insert("source".to_owned(), json!(line.source));
                obj.insert("error".to_owned(), json!(line.error));
            }
        }
        Value::Object(obj).to_string()
    }
}

fn situation_json(s: &Situation) -> Value {
    let mut obj = Map::new();
    obj.insert("front_layer".to_owned(), json!(s.front_layer));
    obj.insert("roles".to_owned(), json!(s.roles));
    obj.insert("near".to_owned(), json!(s.near));
    Value::Object(obj)
}

fn offered_json(a: &OfferedLine) -> Value {
    let mut obj = Map::new();
    obj.insert("id".to_owned(), json!(a.id));
    obj.insert("kind".to_owned(), json!(a.kind));
    obj.insert("label".to_owned(), json!(a.label));
    if let Some(role) = &a.role {
        obj.insert("role".to_owned(), json!(role));
    }
    if let Some(region) = &a.region {
        obj.insert("region".to_owned(), json!(region));
    }
    obj.insert("fingerprint".to_owned(), json!(a.fingerprint));
    if let Some(state) = &a.state {
        let mut s = Map::new();
        if let Some(v) = &state.value {
            s.insert("value".to_owned(), json!(v));
        }
        if let Some(v) = state.checked {
            s.insert("checked".to_owned(), json!(v));
        }
        if let Some(v) = state.expanded {
            s.insert("expanded".to_owned(), json!(v));
        }
        if let Some(v) = &state.selected {
            s.insert("selected".to_owned(), json!(v));
        }
        if !state.options.is_empty() {
            s.insert("options".to_owned(), json!(state.options));
        }
        if let Some(v) = &state.input_type {
            s.insert("input_type".to_owned(), json!(v));
        }
        obj.insert("state".to_owned(), Value::Object(s));
    }
    Value::Object(obj)
}

fn ranked_json(r: &RankedLine) -> Value {
    json!({
        "id": r.id,
        "kind": r.kind,
        "label": r.label,
        "confidence_millis": r.confidence_millis,
    })
}

fn history_json(h: &HistoryLine) -> Value {
    json!({
        "step": h.step,
        "action_id": h.action_id,
        "kind": h.kind,
        "label": h.label,
        "verification": h.verification,
    })
}

// ---------------------------------------------------------------------------
// Parse (untrusted input: field-validated, schema-checked)

fn bad(line_no: usize, message: impl Into<String>) -> DojoError {
    DojoError::Parse {
        line: line_no,
        message: message.into(),
    }
}

fn str_field<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    line_no: usize,
) -> Result<&'a str, DojoError> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad(line_no, format!("missing string field `{key}`")))
}

fn opt_str_field(
    obj: &Map<String, Value>,
    key: &str,
    line_no: usize,
) -> Result<Option<String>, DojoError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| bad(line_no, format!("field `{key}` must be a string"))),
    }
}

fn u32_field(obj: &Map<String, Value>, key: &str, line_no: usize) -> Result<u32, DojoError> {
    obj.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| bad(line_no, format!("missing u32 field `{key}`")))
}

fn u64_field(obj: &Map<String, Value>, key: &str, line_no: usize) -> Result<u64, DojoError> {
    obj.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| bad(line_no, format!("missing u64 field `{key}`")))
}

fn bool_field(obj: &Map<String, Value>, key: &str, line_no: usize) -> Result<bool, DojoError> {
    obj.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| bad(line_no, format!("missing bool field `{key}`")))
}

fn str_list_field(
    obj: &Map<String, Value>,
    key: &str,
    line_no: usize,
) -> Result<Vec<String>, DojoError> {
    let arr = obj
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| bad(line_no, format!("missing array field `{key}`")))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| bad(line_no, format!("`{key}[{i}]` must be a string")))
        })
        .collect()
}

/// Parse one diary line. `line_no` is the 1-based line for error reporting.
///
/// # Errors
/// [`DojoError::Parse`] on malformed JSON, wrong schema, unknown type, or a
/// missing / mistyped required field.
pub fn parse_line(raw: &str, line_no: usize) -> Result<DiaryLine, DojoError> {
    let value: Value = serde_json::from_str(raw).map_err(|e| bad(line_no, format!("json: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, "not a JSON object"))?;
    let schema = obj
        .get("schema")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad(line_no, "missing `schema`"))?;
    if schema != u64::from(SCHEMA) {
        return Err(DojoError::Schema { found: schema });
    }
    let kind = str_field(obj, "type", line_no)?;
    match kind {
        "run" => Ok(DiaryLine::Run(RunLine {
            goal: str_field(obj, "goal", line_no)?.to_owned(),
            clauses: str_list_field(obj, "clauses", line_no)?,
            policy: str_field(obj, "policy", line_no)?.to_owned(),
        })),
        "decision" => Ok(DiaryLine::Decision(Box::new(DecisionLine {
            seq: u32_field(obj, "seq", line_no)?,
            clause_index: u32_field(obj, "clause_index", line_no)?,
            clause: str_field(obj, "clause", line_no)?.to_owned(),
            mode: str_field(obj, "mode", line_no)?.to_owned(),
            source: str_field(obj, "source", line_no)?.to_owned(),
            site: parse_site(obj.get("site"), line_no)?,
            situation: parse_situation(obj.get("situation"), line_no)?,
            offered: parse_list(obj.get("offered"), parse_offered, line_no)?,
            operation_ranked: parse_list(obj.get("operation_ranked"), parse_ranked, line_no)?,
            target_ranked: parse_list(obj.get("target_ranked"), parse_ranked, line_no)?,
            history: parse_list(obj.get("history"), parse_history, line_no)?,
            choice: parse_choice(obj.get("choice"), line_no)?,
            abstain: opt_str_field(obj, "abstain", line_no)?,
        }))),
        "step" => Ok(DiaryLine::Step(StepLine {
            seq: u32_field(obj, "seq", line_no)?,
            step: u32_field(obj, "step", line_no)?,
            clause_index: u32_field(obj, "clause_index", line_no)?,
            clause: str_field(obj, "clause", line_no)?.to_owned(),
            action_id: str_field(obj, "action_id", line_no)?.to_owned(),
            kind: str_field(obj, "kind", line_no)?.to_owned(),
            label: str_field(obj, "label", line_no)?.to_owned(),
            input: str_field(obj, "input", line_no)?.to_owned(),
            payload: opt_str_field(obj, "payload", line_no)?,
            verification: str_field(obj, "verification", line_no)?.to_owned(),
            stale_retries: u32_field(obj, "stale_retries", line_no)?,
            won: bool_field(obj, "won", line_no)?,
        })),
        "stale_discard" => Ok(DiaryLine::StaleDiscard(StaleLine {
            seq: u32_field(obj, "seq", line_no)?,
            reason: str_field(obj, "reason", line_no)?.to_owned(),
        })),
        "clause_advanced" => Ok(DiaryLine::ClauseAdvanced(ClauseLine {
            seq: u32_field(obj, "seq", line_no)?,
            clause_index: u32_field(obj, "clause_index", line_no)?,
            clause: str_field(obj, "clause", line_no)?.to_owned(),
        })),
        "outcome" => Ok(DiaryLine::Outcome(OutcomeLine {
            kind: str_field(obj, "kind", line_no)?.to_owned(),
            reason: str_field(obj, "reason", line_no)?.to_owned(),
            steps: u32_field(obj, "steps", line_no)?,
            policy_calls: u32_field(obj, "policy_calls", line_no)?,
            stale_discards: u32_field(obj, "stale_discards", line_no)?,
            duration_ms: u64_field(obj, "duration_ms", line_no)?,
        })),
        "correction" => Ok(DiaryLine::Correction(CorrectionLine {
            clause: str_field(obj, "clause", line_no)?.to_owned(),
            expected: str_field(obj, "expected", line_no)?.to_owned(),
            chosen: str_field(obj, "chosen", line_no)?.to_owned(),
        })),
        "policy_error" => Ok(DiaryLine::PolicyError(PolicyErrorLine {
            seq: u32_field(obj, "seq", line_no)?,
            clause_index: u32_field(obj, "clause_index", line_no)?,
            clause: str_field(obj, "clause", line_no)?.to_owned(),
            source: str_field(obj, "source", line_no)?.to_owned(),
            error: str_field(obj, "error", line_no)?.to_owned(),
        })),
        other => Err(bad(line_no, format!("unknown line type `{other}`"))),
    }
}

fn parse_site(value: Option<&Value>, line_no: usize) -> Result<Option<SiteLine>, DojoError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, "`site` must be an object"))?;
    Ok(Some(SiteLine {
        url: opt_str_field(obj, "url", line_no)?,
        title: opt_str_field(obj, "title", line_no)?,
        host: opt_str_field(obj, "host", line_no)?,
        path: opt_str_field(obj, "path", line_no)?,
    }))
}

fn parse_situation(value: Option<&Value>, line_no: usize) -> Result<Situation, DojoError> {
    let Some(value) = value else {
        return Ok(Situation::default());
    };
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, "`situation` must be an object"))?;
    Ok(Situation {
        front_layer: obj
            .get("front_layer")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        roles: str_list_field(obj, "roles", line_no).unwrap_or_default(),
        near: str_list_field(obj, "near", line_no).unwrap_or_default(),
    })
}

fn parse_list<T>(
    value: Option<&Value>,
    each: impl Fn(&Value, usize, usize) -> Result<T, DojoError>,
    line_no: usize,
) -> Result<Vec<T>, DojoError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let arr = value
        .as_array()
        .ok_or_else(|| bad(line_no, "expected an array"))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| each(v, line_no, i))
        .collect()
}

fn parse_offered(value: &Value, line_no: usize, index: usize) -> Result<OfferedLine, DojoError> {
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, format!("`offered[{index}]` must be an object")))?;
    let state = match obj.get("state") {
        Some(v) => {
            let s = v.as_object().ok_or_else(|| {
                bad(
                    line_no,
                    format!("`offered[{index}].state` must be an object"),
                )
            })?;
            Some(OfferedState {
                value: opt_str_field(s, "value", line_no)?,
                checked: s.get("checked").and_then(Value::as_bool),
                expanded: s.get("expanded").and_then(Value::as_bool),
                selected: opt_str_field(s, "selected", line_no)?,
                options: str_list_field(s, "options", line_no).unwrap_or_default(),
                input_type: opt_str_field(s, "input_type", line_no)?,
            })
        }
        None => None,
    };
    Ok(OfferedLine {
        id: str_field(obj, "id", line_no)?.to_owned(),
        kind: str_field(obj, "kind", line_no)?.to_owned(),
        label: str_field(obj, "label", line_no)?.to_owned(),
        region: opt_str_field(obj, "region", line_no)?,
        role: opt_str_field(obj, "role", line_no)?,
        fingerprint: obj.get("fingerprint").and_then(Value::as_u64).unwrap_or(0),
        state,
    })
}

fn parse_ranked(value: &Value, line_no: usize, index: usize) -> Result<RankedLine, DojoError> {
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, format!("`ranked[{index}]` must be an object")))?;
    let confidence = obj
        .get("confidence_millis")
        .and_then(Value::as_i64)
        .and_then(|v| i16::try_from(v).ok())
        .ok_or_else(|| {
            bad(
                line_no,
                format!("`ranked[{index}].confidence_millis` must be i16"),
            )
        })?;
    Ok(RankedLine {
        id: str_field(obj, "id", line_no)?.to_owned(),
        kind: str_field(obj, "kind", line_no)?.to_owned(),
        label: str_field(obj, "label", line_no)?.to_owned(),
        confidence_millis: confidence,
    })
}

fn parse_history(value: &Value, line_no: usize, index: usize) -> Result<HistoryLine, DojoError> {
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, format!("`history[{index}]` must be an object")))?;
    Ok(HistoryLine {
        step: u32_field(obj, "step", line_no)?,
        action_id: str_field(obj, "action_id", line_no)?.to_owned(),
        kind: str_field(obj, "kind", line_no)?.to_owned(),
        label: str_field(obj, "label", line_no)?.to_owned(),
        verification: str_field(obj, "verification", line_no)?.to_owned(),
    })
}

fn parse_choice(value: Option<&Value>, line_no: usize) -> Result<Option<ChoiceLine>, DojoError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let obj = value
        .as_object()
        .ok_or_else(|| bad(line_no, "`choice` must be an object"))?;
    let confidence = obj
        .get("confidence_millis")
        .and_then(Value::as_i64)
        .and_then(|v| i16::try_from(v).ok())
        .ok_or_else(|| bad(line_no, "`choice.confidence_millis` must be i16"))?;
    Ok(Some(ChoiceLine {
        action_id: str_field(obj, "action_id", line_no)?.to_owned(),
        kind: str_field(obj, "kind", line_no)?.to_owned(),
        target_label: str_field(obj, "target_label", line_no)?.to_owned(),
        confidence_millis: confidence,
    }))
}
