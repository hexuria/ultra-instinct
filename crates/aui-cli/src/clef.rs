//! Cloudflare Workers AI Clef transport for `RemotePolicy` — feature `clef`.
//!
//! Same closed remote wire as `TypesafeTransport`: the System One body
//! (`state` + speculative `operation` / `<kind>_target` questions) built by
//! `build_questions`, sent through `typesafe-sdk`'s Clef provider to the
//! Workers AI endpoint `…/accounts/{id}/ai/run/@cf/cloudflare/{model}`.
//! `@cf/cloudflare/clef` (27B) and `@cf/cloudflare/clef-flash` (9B) are Clef
//! decision models in the Jev family: one forward pass scores every offered
//! option; there is no free-form text to parse. The SDK owns the URL, auth,
//! the `{success, errors, result}` envelope, and answer decoding.
//!
//! Answers are validated exactly like the jev leg (`choice` is an offered
//! key, the probabilities cover the offered set, sum ≈ 1, the choice is the
//! argmax) and mapped back into the closed reply: exactly
//! `{"choice":{"id","kind"}}` or `{"abstain"}`. The model can only ever pick
//! an offered id — anything malformed or off-menu is a hard error before it
//! becomes a `PolicyError`.
//!
//! Env: `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` (both required,
//! token needs Workers AI read). `CLEF_MODEL` overrides the model slug set by
//! `--policy clef` / `--policy clef-flash`. `CLEF_BASE_URL` replaces the
//! Workers AI prefix `https://api.cloudflare.com/client/v4`; a value ending
//! in `/accounts` (the form this variable used to take) still works, since
//! `/accounts/{id}/ai/run/@cf/cloudflare/{model}` is appended either way.
//!
//! Timeout bounds: one 60 s attempt per call, no retry. The call only asks
//! for a decision and runs before any ticket is issued, so a timeout never
//! repeats page input — the agent sees an error, not a silent double-act.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use typesafe_sdk::blocking::Client;
use typesafe_sdk::{ChoiceAnswer, Provider, Question, RetryPolicy, SystemOneResponse};

use aui_policy::RemoteTransport;

use crate::typesafe::{build_questions, compose_reply, BuiltQuestions};

const MODEL_ENV: &str = "CLEF_MODEL";
const BASE_URL_ENV: &str = "CLEF_BASE_URL";
const ACCOUNT_ENV: &str = "CLOUDFLARE_ACCOUNT_ID";
const API_KEY_ENV: &str = "CLOUDFLARE_API_TOKEN";
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Cloudflare Workers AI Clef transport: the System One body through the
/// `typesafe-sdk` Clef provider.
pub struct ClefTransport {
    client: Client,
    model: String,
}

impl ClefTransport {
    /// `model` is the Clef slug (`clef` or `clef-flash`); `CLEF_MODEL`
    /// overrides it. `CLEF_BASE_URL` replaces the Workers AI prefix (see the
    /// module docs).
    pub fn from_env(model: &str) -> Result<Self, String> {
        let account_id = required_env(ACCOUNT_ENV)?;
        let api_key = required_env(API_KEY_ENV)?;
        let model = std::env::var(MODEL_ENV)
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| model.to_owned());
        let mut builder = Client::builder()
            .api_key(api_key)
            .provider(Provider::Clef { account_id })
            .model(model.clone())
            .timeout(CALL_TIMEOUT)
            .retry(RetryPolicy::disabled());
        if let Some(base) = std::env::var(BASE_URL_ENV)
            .ok()
            .filter(|b| !b.trim().is_empty())
        {
            let base = base.trim().trim_end_matches('/');
            builder = builder.base_url(base.strip_suffix("/accounts").unwrap_or(base));
        }
        let client = builder
            .build()
            .map_err(|err| format!("clef client: {err}"))?;
        Ok(Self { client, model })
    }

    /// The model slug this transport calls (after `CLEF_MODEL` overrides).
    pub fn model(&self) -> &str {
        &self.model
    }
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{name} is not set"))
}

/// A Clef `choice` question needs at least two criteria (Workers AI rejects
/// a single-option dictionary with 422, and the SDK refuses it before
/// sending). A head with exactly one offered candidate is already decided,
/// so it never reaches the wire: `auto` maps head name to its sole candidate
/// and `reply_from_answers` answers it locally with probability 1.
struct ClefCall {
    state: Value,
    questions: Vec<(String, Question)>,
    criteria_keys: Vec<(String, Vec<String>)>,
    /// head name -> sole candidate, answered locally.
    auto: BTreeMap<String, String>,
}

/// The questions to send for one wire request, keeping `criteria_keys` for
/// answer validation.
fn call_for(request: &Value) -> Result<ClefCall, String> {
    let BuiltQuestions {
        state,
        questions,
        criteria_keys,
    } = build_questions(request)?;
    let mut sent = Vec::new();
    let mut auto = BTreeMap::new();
    for ((name, question), (_, keys)) in questions.into_iter().zip(criteria_keys.iter()) {
        if name != "operation" && keys.len() == 1 {
            auto.insert(name, keys[0].clone());
        } else {
            sent.push((name, question));
        }
    }
    Ok(ClefCall {
        state,
        questions: sent,
        criteria_keys,
        auto,
    })
}

/// Map a Clef response into the closed reply. Heads in `auto` were decided
/// locally (sole candidate) and answer themselves.
fn reply_from_answers(
    response: &SystemOneResponse,
    criteria_keys: &[(String, Vec<String>)],
    auto: &BTreeMap<String, String>,
) -> Result<String, String> {
    let local: BTreeMap<&str, ChoiceAnswer> = auto
        .iter()
        .map(|(head, candidate)| {
            (
                head.as_str(),
                ChoiceAnswer::new(candidate, 1.0, [(candidate.clone(), 1.0)]),
            )
        })
        .collect();
    compose_reply(criteria_keys, |head| match local.get(head) {
        Some(answer) => Ok(answer),
        None => response
            .choice(head)
            .map_err(|err| format!("clef answer `{head}`: {err}")),
    })
}

impl RemoteTransport for ClefTransport {
    fn call(&mut self, request_json: &str) -> Result<String, String> {
        let request: Value =
            serde_json::from_str(request_json).map_err(|err| format!("clef request: {err}"))?;
        let ClefCall {
            state,
            questions,
            criteria_keys,
            auto,
        } = call_for(&request)?;
        let response = self
            .client
            .system_one(state, questions)
            .map_err(|err| format!("clef call: {err}"))?;
        reply_from_answers(&response, &criteria_keys, &auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &str = r#"{
        "goal": "Send the message",
        "actions": [
            {"id": "CLICK:n12", "kind": "CLICK", "label": "Send", "role": "button"},
            {"id": "CLICK:n30", "kind": "CLICK", "label": "Cancel", "role": "button"},
            {"id": "TYPE_TEXT:n44", "kind": "TYPE_TEXT", "label": "Message", "role": "textbox"},
            {"id": "SCROLL_DOWN", "kind": "SCROLL_DOWN", "label": "Scroll the page down"},
            {"id": "DONE", "kind": "DONE", "label": "Every requirement is visibly satisfied"}
        ],
        "history": [{"step": 1, "id": "CLICK:n1", "verification": "success"}]
    }"#;

    fn call() -> ClefCall {
        call_for(&serde_json::from_str(REQUEST).unwrap()).unwrap()
    }

    /// A Clef `result` as the SDK decodes it.
    fn response(answers: Value) -> SystemOneResponse {
        serde_json::from_value(serde_json::json!({"model": "clef-flash", "answers": answers}))
            .unwrap()
    }

    fn reply(answers: Value) -> Result<String, String> {
        let ClefCall {
            criteria_keys,
            auto,
            ..
        } = call();
        reply_from_answers(&response(answers), &criteria_keys, &auto)
    }

    #[test]
    fn sends_the_systemone_shape_minus_sole_candidate_heads() {
        let ClefCall {
            state,
            questions,
            auto,
            ..
        } = call();
        assert_eq!(state["goal"], "Send the message");
        assert!(state["actions"].is_array() && state["recent_actions"].is_array());
        let questions: serde_json::Map<String, Value> = questions
            .into_iter()
            .map(|(name, question)| (name, serde_json::to_value(question).unwrap()))
            .collect();
        // TYPE_TEXT has one candidate: decided locally, never on the wire.
        assert_eq!(
            questions.keys().map(String::as_str).collect::<Vec<_>>(),
            ["operation", "click_target"]
        );
        assert_eq!(
            auto.get("type_text_target").map(String::as_str),
            Some("n44")
        );
        let operation = &questions["operation"];
        assert_eq!(operation["type"], "choice");
        assert_eq!(operation["instructions"]["goal"], "Send the message");
        let click = &questions["click_target"];
        assert!(click["instructions"]["rules"].is_array());
        let criteria = click["criteria"].as_object().unwrap();
        assert!(criteria.contains_key("n12") && criteria.contains_key("n30"));
    }

    #[test]
    fn valid_choice_maps_to_closed_reply() {
        let reply = reply(serde_json::json!({
            "operation": {
                "type": "choice",
                "choice": "CLICK",
                "confidence": 0.9,
                "probabilities": {"CLICK": 0.9, "TYPE_TEXT": 0.05, "SCROLL_DOWN": 0.03, "DONE": 0.02}
            },
            "click_target": {
                "type": "choice",
                "choice": "n12",
                "confidence": 0.8,
                "probabilities": {"n12": 0.8, "n30": 0.2}
            }
        }))
        .unwrap();
        assert_eq!(reply, r#"{"choice":{"id":"CLICK:n12","kind":"CLICK"}}"#);
    }

    #[test]
    fn sole_candidate_head_answers_locally() {
        let reply = reply(serde_json::json!({
            "operation": {
                "type": "choice",
                "choice": "TYPE_TEXT",
                "confidence": 0.9,
                "probabilities": {"CLICK": 0.05, "TYPE_TEXT": 0.9, "SCROLL_DOWN": 0.03, "DONE": 0.02}
            }
        }))
        .unwrap();
        assert_eq!(
            reply,
            r#"{"choice":{"id":"TYPE_TEXT:n44","kind":"TYPE_TEXT"}}"#
        );
    }

    #[test]
    fn control_choice_needs_no_target() {
        let reply = reply(serde_json::json!({
            "operation": {
                "type": "choice",
                "choice": "DONE",
                "confidence": 0.9,
                "probabilities": {"CLICK": 0.05, "TYPE_TEXT": 0.03, "SCROLL_DOWN": 0.02, "DONE": 0.9}
            }
        }))
        .unwrap();
        assert_eq!(reply, r#"{"choice":{"id":"DONE","kind":"DONE"}}"#);
    }

    #[test]
    fn off_menu_choice_is_a_hard_error() {
        let err = reply(serde_json::json!({
            "operation": {
                "type": "choice",
                "choice": "JUMP",
                "confidence": 0.9,
                "probabilities": {"CLICK": 0.1, "TYPE_TEXT": 0.0, "SCROLL_DOWN": 0.0, "DONE": 0.9}
            }
        }))
        .unwrap_err();
        assert!(err.contains("off-menu"), "{err}");
    }

    #[test]
    fn missing_target_head_is_a_hard_error() {
        let err = reply(serde_json::json!({
            "operation": {
                "type": "choice",
                "choice": "CLICK",
                "confidence": 0.9,
                "probabilities": {"CLICK": 0.9, "TYPE_TEXT": 0.05, "SCROLL_DOWN": 0.03, "DONE": 0.02}
            }
        }))
        .unwrap_err();
        assert!(err.contains("click_target"), "{err}");
    }
}
