//! Optional remote (Jev / System-One / any host model) fallback policy.
//!
//! Feature `remote`. No network stack and no TypeSafe/Jev dependency: the
//! consumer supplies a [`RemoteTransport`] that carries one JSON request to
//! its model and returns one JSON reply. Ultra-Instinct owns the wire contract and
//! enforces the same finite [`ActionSpace`] as Instinct:
//!
//! Request (one object):
//!
//! ```json
//! {"goal": "...",
//!  "actions": [{"id": "TYPE_TEXT:n4", "kind": "TYPE_TEXT", "label": "",
//!               "role": "text_field", "box": [32, 55, 120, 15],
//!               "near_text": "Name:", "state": {"value": "Ana"}}, ...],
//!  "history": [{"step": 1, "id": "TYPE_TEXT:n4", "kind": "TYPE_TEXT",
//!               "label": "Name", "verification": "success"}],
//!  "page": {"url": "...", "title": "...", "text": "visible page text"}}
//! ```
//!
//! Region-bound actions include their box `[x, y, w, h]` in viewport pixels,
//! `near_text` when the region has no label, and nonempty observed control
//! state, all as additive evidence. `page` is present when the transport
//! [`RemoteTransport::wants_evidence`] page text; its text is untrusted data.
//! A transport that wants a screenshot gets the decision's viewport
//! [`Screenshot`] beside the request. The reply remains closed to the offered
//! id and kind.
//!
//! Reply (exactly one of):
//!
//! ```json
//! {"choice": {"id": "CLICK:n12", "kind": "CLICK"}}
//! {"abstain": "why"}
//! ```
//!
//! Anything else is a hard [`PolicyError`]: an id not offered, a kind that does
//! not match the offered id, extra selector / coordinate / script fields, or
//! malformed JSON. A reply can never carry a CSS selector, coordinates, shell,
//! or JavaScript into execution because only offered ids are accepted.
//!
//! [`UnconfiguredRemote`] is the stub: it always abstains, so wiring an
//! `EscalatingPolicy<InstinctPolicy, UnconfiguredRemote>` changes nothing until a
//! real transport is plugged in.

use serde_json::{json, Map, Value};

use aui_core::{ActionKind, ActionSpace};

use crate::goal::AgentGoal;
use crate::types::{
    BrowserPolicy, DecisionEvidence, EvidenceWants, HistoryEntry, PageView, PolicyDecision,
    PolicyError, PolicyOutcome, Progress, Screenshot,
};

/// Carries one request to a remote model and returns its raw reply.
pub trait RemoteTransport {
    fn call(&mut self, request_json: &str) -> Result<String, String>;

    /// Which evidence this transport sends. Default: none.
    fn wants_evidence(&self) -> EvidenceWants {
        EvidenceWants::default()
    }

    /// [`Self::call`] with the viewport screenshot as evidence beside the
    /// request. The reply contract is unchanged. Default: ignores it.
    fn call_with_screenshot(
        &mut self,
        request_json: &str,
        _screenshot: Option<&Screenshot>,
    ) -> Result<String, String> {
        self.call(request_json)
    }
}

/// Remote policy over a consumer transport. See the module docs for the wire.
pub struct RemotePolicy<T> {
    transport: T,
    calls: u32,
    name: &'static str,
}

impl<T: RemoteTransport> RemotePolicy<T> {
    pub fn new(transport: T) -> Self {
        Self::named("remote", transport)
    }

    /// A remote policy whose decisions are recorded under `name` in the diary.
    pub fn named(name: &'static str, transport: T) -> Self {
        Self {
            transport,
            calls: 0,
            name,
        }
    }

    /// Remote calls made (escalations), for eval accounting.
    pub fn calls(&self) -> u32 {
        self.calls
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }
}

/// Build the request object for `space` / `goal` / `history`.
///
/// Each action carries `id`, `kind`, `label`, and — for region-bound
/// actions — `role`, `box`, `near_text` when unnamed, and nonempty observed
/// `state`. History includes the
/// executed action's `kind` and `label`. Extra fields are additive for
/// transports; a consumer may ignore them.
pub fn request_json(space: &ActionSpace, goal: &AgentGoal, history: &[HistoryEntry]) -> String {
    request_json_with_page(space, goal, history, None)
}

/// [`request_json`] plus the page's `url`, `title`, and visible `text`.
pub fn request_json_with_page(
    space: &ActionSpace,
    goal: &AgentGoal,
    history: &[HistoryEntry],
    page: Option<&PageView>,
) -> String {
    request_json_with_evidence(space, goal, history, page, None)
}

/// [`request_json_with_page`] plus the run's `progress`.
pub fn request_json_with_evidence(
    space: &ActionSpace,
    goal: &AgentGoal,
    history: &[HistoryEntry],
    page: Option<&PageView>,
    progress: Option<&Progress>,
) -> String {
    let actions: Vec<Value> = space
        .actions()
        .map(|a| {
            let mut action =
                json!({"id": a.id().as_str(), "kind": a.kind().as_str(), "label": a.label()});
            if let Some(role) = a.role() {
                action["role"] = json!(role.as_str());
            }
            if let Some(bounds) = a.bounds() {
                action["box"] = json!(bounds);
            }
            if let Some(near) = a.near_text() {
                action["near_text"] = json!(near);
            }
            let observed = a.state();
            if !observed.is_empty() {
                let mut state = Map::new();
                if let Some(value) = &observed.value {
                    state.insert("value".to_owned(), json!(value));
                }
                if let Some(checked) = observed.checked {
                    state.insert("checked".to_owned(), json!(checked));
                }
                if let Some(expanded) = observed.expanded {
                    state.insert("expanded".to_owned(), json!(expanded));
                }
                if let Some(selected) = &observed.selected {
                    state.insert("selected".to_owned(), json!(selected));
                }
                if !observed.options.is_empty() {
                    state.insert("options".to_owned(), json!(&observed.options));
                }
                action["state"] = Value::Object(state);
            }
            action
        })
        .collect();
    let history: Vec<Value> = history
        .iter()
        .map(|h| {
            json!({
                "step": h.step,
                "id": h.action_id.as_str(),
                "kind": h.kind.as_str(),
                "label": h.label,
                "verification": h.verification
            })
        })
        .collect();
    let mut request = json!({"goal": goal.as_str(), "actions": actions, "history": history});
    if let Some(page) = page {
        request["page"] = json!({"url": page.url, "title": page.title, "text": page.text});
    }
    if let Some(p) = progress {
        let typed: Vec<Value> = p
            .typed
            .iter()
            .map(|(id, value)| json!({"id": id, "value": value}))
            .collect();
        request["progress"] = json!({
            "clauses": p.clauses,
            "current": p.current,
            "satisfied": p.satisfied,
            "typed": typed,
        });
    }
    request.to_string()
}

/// Parse and validate a reply against the offered space.
pub fn parse_reply(space: &ActionSpace, reply: &str) -> Result<PolicyOutcome, PolicyError> {
    let bad = |m: &str| PolicyError::Internal(format!("remote reply: {m}"));
    let value: Value = serde_json::from_str(reply).map_err(|e| bad(&e.to_string()))?;
    let obj = value.as_object().ok_or_else(|| bad("not an object"))?;
    if obj.len() != 1 {
        return Err(bad("expected exactly one of `choice` or `abstain`"));
    }
    if let Some(reason) = obj.get("abstain") {
        let reason = reason
            .as_str()
            .ok_or_else(|| bad("abstain must be a string"))?;
        return Ok(PolicyOutcome::Abstain {
            reason: format!("remote: {reason}"),
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        });
    }
    let choice = obj
        .get("choice")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("expected `choice` object"))?;
    if choice.keys().any(|k| k != "id" && k != "kind") {
        return Err(bad("choice may only carry `id` and `kind`"));
    }
    let id = choice
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("choice.id missing"))?;
    let kind = choice
        .get("kind")
        .and_then(Value::as_str)
        .and_then(ActionKind::parse)
        .ok_or_else(|| bad("choice.kind missing or unknown"))?;
    let offered = space
        .get_str(id)
        .ok_or_else(|| bad(&format!("off-menu id `{id}`")))?;
    if offered.kind() != kind {
        return Err(bad(&format!("id `{id}` is {} not {kind}", offered.kind())));
    }
    Ok(PolicyOutcome::Choice(PolicyDecision {
        action_id: offered.id().clone(),
        kind,
        target_label: offered.label().to_owned(),
        // Remote confidence is not comparable to Instinct millis; never reported as such.
        confidence_millis: 0,
        operation_ranked: Vec::new(),
        target_ranked: Vec::new(),
    }))
}

impl<T: RemoteTransport> BrowserPolicy for RemotePolicy<T> {
    fn decide(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        self.decide_with_evidence(space, goal, history, &DecisionEvidence::default())
    }

    fn wants_evidence(&self) -> EvidenceWants {
        self.transport.wants_evidence()
    }

    fn decide_with_evidence(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
        evidence: &DecisionEvidence,
    ) -> Result<PolicyOutcome, PolicyError> {
        if goal.is_empty() {
            return Err(PolicyError::EmptyGoal);
        }
        self.calls += 1;
        let reply = self
            .transport
            .call_with_screenshot(
                &request_json_with_evidence(
                    space,
                    goal,
                    history,
                    evidence.page.as_ref(),
                    evidence.progress.as_ref(),
                ),
                evidence.screenshot.as_ref(),
            )
            .map_err(|e| PolicyError::Internal(format!("remote transport: {e}")))?;
        parse_reply(space, &reply)
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// Stub fallback: always abstains. The product builds and runs without a remote.
#[derive(Clone, Debug, Default)]
pub struct UnconfiguredRemote;

impl BrowserPolicy for UnconfiguredRemote {
    fn name(&self) -> &'static str {
        "unconfigured-remote"
    }

    fn decide(
        &mut self,
        _space: &ActionSpace,
        _goal: &AgentGoal,
        _history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        Ok(PolicyOutcome::Abstain {
            reason: "remote policy not configured".to_owned(),
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        })
    }
}

/// Test / replay transport: returns scripted replies in order and records requests.
#[derive(Clone, Debug, Default)]
pub struct ScriptedRemote {
    replies: std::collections::VecDeque<String>,
    requests: Vec<String>,
}

impl ScriptedRemote {
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(replies: I) -> Self {
        Self {
            replies: replies.into_iter().map(Into::into).collect(),
            requests: Vec::new(),
        }
    }

    pub fn requests(&self) -> &[String] {
        &self.requests
    }
}

impl RemoteTransport for ScriptedRemote {
    fn call(&mut self, request_json: &str) -> Result<String, String> {
        self.requests.push(request_json.to_owned());
        self.replies
            .pop_front()
            .ok_or_else(|| "no scripted reply".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escalate::EscalatingPolicy;
    use crate::instinct_policy::InstinctPolicy;
    use aui_core::{parse_fixture, ActionId, ElementState};

    fn twins() -> ActionSpace {
        ActionSpace::from_manifold(
            &parse_fixture(
                r#"
                viewport w=800 h=600
                region id=a role=button label="Delete" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
                region id=b role=button label="Delete" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
                "#,
            )
            .unwrap(),
        )
    }

    fn stateful_space() -> ActionSpace {
        let mut manifold = parse_fixture(
            r#"
            viewport w=800 h=600
            region id=name role=text_field label="Name" x=10 y=10 w=160 h=24 actions=type sources=dom
            region id=submit role=button label="Submit" x=10 y=50 w=80 h=24 actions=click sources=dom
            "#,
        )
        .unwrap();
        let name = manifold
            .get_str("name")
            .unwrap()
            .clone()
            .with_state(ElementState {
                value: Some("Ana".to_owned()),
                checked: Some(false),
                expanded: Some(true),
                selected: Some("UTC".to_owned()),
                options: vec!["UTC".to_owned()],
                input_type: None,
                href: None,
            });
        manifold.replace(name);
        ActionSpace::from_manifold(&manifold)
    }

    #[test]
    fn unconfigured_remote_keeps_instinct_abstain() {
        let mut p = EscalatingPolicy::new(InstinctPolicy::default(), Some(UnconfiguredRemote));
        let out = p.decide(&twins(), &AgentGoal::new("Delete"), &[]).unwrap();
        assert!(matches!(out, PolicyOutcome::Abstain { .. }), "{out:?}");
    }

    #[test]
    fn remote_choice_runs_only_after_instinct_abstains_and_must_be_offered() {
        let remote = RemotePolicy::new(ScriptedRemote::new([
            r#"{"choice":{"id":"CLICK:b","kind":"CLICK"}}"#,
        ]));
        let mut p = EscalatingPolicy::new(InstinctPolicy::default(), Some(remote));
        let out = p.decide(&twins(), &AgentGoal::new("Delete"), &[]).unwrap();
        assert_eq!(out.as_choice().unwrap().action_id.as_str(), "CLICK:b");
        let remote = p.fallback.as_ref().unwrap();
        assert_eq!(remote.calls(), 1);
        let req: Value = serde_json::from_str(&remote.transport().requests()[0]).unwrap();
        assert_eq!(req["goal"], "Delete");
        assert!(req["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "CLICK:a"));
    }

    #[test]
    fn request_includes_state_only_for_nonempty_actions_and_history_details() {
        let space = stateful_space();
        let history = [HistoryEntry {
            step: 1,
            action_id: ActionId::try_new("TYPE_TEXT:name").unwrap(),
            kind: ActionKind::TypeText,
            label: "Name".to_owned(),
            verification: "success".to_owned(),
        }];
        let request: Value = serde_json::from_str(&request_json(
            &space,
            &AgentGoal::new("Fill the name"),
            &history,
        ))
        .unwrap();
        let actions = request["actions"].as_array().unwrap();
        let name = actions
            .iter()
            .find(|action| action["id"] == "TYPE_TEXT:name")
            .unwrap();
        assert_eq!(
            name["state"],
            json!({
                "value":"Ana",
                "checked":false,
                "expanded":true,
                "selected":"UTC",
                "options":["UTC"]
            })
        );
        let submit = actions
            .iter()
            .find(|action| action["id"] == "CLICK:submit")
            .unwrap();
        assert!(submit.get("state").is_none());
        assert_eq!(submit["box"], json!([10, 50, 80, 24]));
        assert!(submit.get("near_text").is_none(), "named target");
        let done = actions
            .iter()
            .find(|action| action["id"] == "DONE")
            .unwrap();
        assert!(done.get("state").is_none());
        assert!(done.get("box").is_none(), "controls have no box");
        assert_eq!(request["history"][0]["kind"], "TYPE_TEXT");
        assert_eq!(request["history"][0]["label"], "Name");
    }

    /// Records the screenshot and page each call carried; replies with a
    /// fixed choice.
    struct SeeingRemote {
        seen: Vec<Option<String>>,
        pages: Vec<Value>,
        progress: Vec<Value>,
    }

    impl RemoteTransport for SeeingRemote {
        fn call(&mut self, request_json: &str) -> Result<String, String> {
            self.call_with_screenshot(request_json, None)
        }

        fn wants_evidence(&self) -> EvidenceWants {
            EvidenceWants {
                screenshot: true,
                page_text: true,
            }
        }

        fn call_with_screenshot(
            &mut self,
            request_json: &str,
            screenshot: Option<&Screenshot>,
        ) -> Result<String, String> {
            self.seen.push(screenshot.map(|s| s.base64().to_owned()));
            let request: Value = serde_json::from_str(request_json).unwrap();
            self.pages.push(request["page"].clone());
            self.progress.push(request["progress"].clone());
            Ok(r#"{"choice":{"id":"CLICK:a","kind":"CLICK"}}"#.to_owned())
        }
    }

    #[test]
    fn evidence_reaches_a_transport_that_wants_it_and_reply_stays_closed() {
        let mut p = EscalatingPolicy::new(
            InstinctPolicy::default(),
            Some(RemotePolicy::new(SeeingRemote {
                seen: Vec::new(),
                pages: Vec::new(),
                progress: Vec::new(),
            })),
        );
        assert_eq!(
            p.wants_evidence(),
            EvidenceWants {
                screenshot: true,
                page_text: true
            }
        );
        let evidence = DecisionEvidence {
            screenshot: Some(Screenshot::from_base64("/9j/AA==")),
            page: Some(PageView {
                url: Some("https://x.test/".to_owned()),
                title: None,
                text: "3 + 9 = ?".to_owned(),
            }),
            progress: Some(Progress {
                clauses: vec!["Type 12".into(), "Click Submit".into()],
                current: 1,
                satisfied: vec![true, false],
                typed: vec![("TYPE_TEXT:n1".into(), "12".into())],
            }),
        };
        let out = p
            .decide_with_evidence(&twins(), &AgentGoal::new("Delete"), &[], &evidence)
            .unwrap();
        assert_eq!(out.as_choice().unwrap().action_id.as_str(), "CLICK:a");
        p.decide(&twins(), &AgentGoal::new("Delete"), &[]).unwrap();
        let remote = p.fallback.as_ref().unwrap().transport();
        assert_eq!(remote.seen, [Some("/9j/AA==".to_owned()), None]);
        assert_eq!(
            remote.pages[0],
            json!({"url": "https://x.test/", "title": null, "text": "3 + 9 = ?"})
        );
        assert!(remote.pages[1].is_null(), "no page without evidence");
        assert_eq!(
            remote.progress[0],
            json!({"clauses": ["Type 12", "Click Submit"], "current": 1,
                   "satisfied": [true, false], "typed": [{"id": "TYPE_TEXT:n1", "value": "12"}]})
        );
        assert!(remote.progress[1].is_null());
    }

    #[test]
    fn transports_and_instinct_ask_for_no_evidence_by_default() {
        let none = EvidenceWants::default();
        assert_eq!(
            RemotePolicy::new(ScriptedRemote::new(Vec::<String>::new())).wants_evidence(),
            none
        );
        assert_eq!(InstinctPolicy::default().wants_evidence(), none);
        assert_eq!(
            EscalatingPolicy::new(InstinctPolicy::default(), Some(UnconfiguredRemote))
                .wants_evidence(),
            none
        );
    }

    #[test]
    fn off_menu_selector_coordinates_and_kind_mismatch_are_hard_errors() {
        let space = twins();
        for reply in [
            r#"{"choice":{"id":"CLICK:ghost","kind":"CLICK"}}"#,
            r#"{"choice":{"id":"CLICK:a","kind":"TYPE_TEXT"}}"#,
            r##"{"choice":{"id":"CLICK:a","kind":"CLICK","selector":"#delete"}}"##,
            r#"{"choice":{"id":"CLICK:a","kind":"CLICK","x":10,"y":20}}"#,
            r#"{"choice":{"id":"CLICK:a","kind":"CLICK"},"abstain":"x"}"#,
            r#"{"js":"document.body.remove()"}"#,
            r#"not json"#,
        ] {
            assert!(parse_reply(&space, reply).is_err(), "{reply}");
        }
        assert!(matches!(
            parse_reply(&space, r#"{"abstain":"unsure"}"#).unwrap(),
            PolicyOutcome::Abstain { .. }
        ));
    }
}
