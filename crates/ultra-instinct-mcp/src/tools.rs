//! MCP tools for the action-firewall product: observe, guard, verify.
//!
//! `guard` finds its target by exact label (and role / `within`), unique or
//! refuse, runs the hard gate, and never clicks. The locate, inspect, act and
//! diff helpers and the matcher are gone (ADR 0012).

use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::{
    page_delta, verify, verify_delta, BrowserSession, Expectation, PageState, ReplayTransport,
    VerifyError,
};
use ultra_instinct_core::{
    parse_fixture, Action, InteractionManifold, InteractionRegion, LocateQuery, RegionId, Role,
};
use ultra_instinct_geometry::RegionState;
use ultra_instinct_guard::{guard, with_front_layer, FrontLayer, GuardRequest, WorldSnapshot};
use ultra_instinct_observe::{diff, history::SnapshotId};
use ultra_instinct_protocol::GuardDecision;

use ultra_instinct_browser_tools::BrowserHost as _;

use crate::server::{Host, Server};
use crate::ToolError;

const PRODUCT: &str = "ultra-instinct";

/// Guard arguments the matcher understood. Refused by name so a caller that
/// still sends them learns why instead of getting an unscoped decision.
const REMOVED_GUARD_ARGUMENTS: [&str; 4] = ["near", "position", "matcher", "dims"];

enum Origin {
    Fixture(String),
    Cdp(String),
}

/// `{"focused": id|null, "front_layer": [{id, label, modal}]}` facts of one
/// observation, shared by observe and guard.
fn world_json(manifold: &InteractionManifold, page: &PageState) -> (Value, Value) {
    let focused = page.focused().map_or(Value::Null, |id| json!(id.as_str()));
    let layer: Vec<Value> = FrontLayer::of(manifold)
        .entries()
        .iter()
        .map(|entry| {
            json!({
                "id": entry.id.as_str(),
                "label": manifold.get(&entry.id).map_or("", |region| region.label()),
                "modal": entry.modal,
            })
        })
        .collect();
    (focused, Value::Array(layer))
}

/// Run one tool against a fresh [`Server`]. Keeps no state between calls.
pub fn call_tool(name: &str, arguments: &Value) -> Result<Value, ToolError> {
    Server::new().call_tool(name, arguments)
}

pub(crate) fn dispatch(
    server: &mut Server,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let owned;
    let arguments = match arguments {
        Value::Null => {
            owned = Value::Object(serde_json::Map::new());
            &owned
        }
        Value::Object(_) => arguments,
        _ => {
            return Err(ToolError::InvalidArguments(
                "arguments must be an object".into(),
            ))
        }
    };
    if arguments.get("goal").is_some()
        || arguments.get("steps").is_some()
        || arguments.get("navigate").is_some()
    {
        return Err(ToolError::GoalNotAccepted);
    }
    if arguments.get("x").is_some()
        || arguments.get("y").is_some()
        || arguments.get("coordinates").is_some()
    {
        return Err(ToolError::CoordinatesNotAccepted);
    }
    if let Some(result) = ultra_instinct_browser_tools::call(&mut Host(server), name, arguments) {
        return result;
    }
    match name {
        "navigate" => Err(ToolError::GoalNotAccepted),
        "observe" => observe(server, arguments),
        "guard" => guard_tool(server, arguments),
        "verify" => verify_tool(server, arguments),
        other => Err(ToolError::UnknownTool(other.to_owned())),
    }
}

fn observe(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let (snapshot, raw, page) = observe_origin(server, &resolve_origin(arguments)?)?;
    // State as a person sees it: regions behind an open dialog read occluded.
    let manifold = with_front_layer(&raw);
    // The host's element cap (`ToolLimits::max_elements`) bounds the list.
    let max = Host(server).limits().max_elements;
    let dropped = manifold.regions().count().saturating_sub(max);
    let regions: Vec<Value> = manifold
        .regions()
        .take(max)
        .map(|region| {
            json!({
                "id": region.id().as_str(),
                "role": region.role().as_str(),
                "label": region.label(),
                "parent": region.parent().map_or(Value::Null, |id| json!(id.as_str())),
                "state": state_json(manifold.as_ref(), region),
            })
        })
        .collect();
    let (focused, front_layer) = world_json(&raw, &page);
    let mut body = outcome(
        "observe",
        Value::Null,
        None,
        false,
        false,
        empty_delta(),
        None,
        None,
        None,
    );
    insert(&mut body, "regions", Value::Array(regions));
    insert(&mut body, "focused", focused);
    insert(&mut body, "front_layer", front_layer);
    insert(&mut body, "snapshot", json!(snapshot.get()));
    if dropped > 0 {
        insert(&mut body, "truncated", json!(true));
        insert(&mut body, "dropped_regions", json!(dropped));
    }
    Ok(body)
}

fn guard_tool(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    if let Some(removed) = REMOVED_GUARD_ARGUMENTS
        .iter()
        .find(|key| arguments.get(**key).is_some())
    {
        return Err(ToolError::InvalidArguments(format!(
            "`{removed}` was removed with the matcher; guard matches the exact label: \
             name the region with `proposed` or scope the label with `within`"
        )));
    }
    let origin = resolve_origin(arguments)?;
    let query = build_guard_query(arguments)?;
    let proposed = match opt_str(arguments, "proposed")? {
        Some(raw) => Some(raw),
        None => opt_str(arguments, "region")?,
    };
    let proposed = match proposed {
        Some(raw) => {
            Some(RegionId::try_new(raw).map_err(|_| ToolError::UnknownRegion(raw.to_owned()))?)
        }
        None => None,
    };
    // The observation the host decided on. Its world snapshot (focus, dialogs,
    // clickable ids, occluded set) is compared with the world observed now.
    // Recorded by observe / guard on this origin, or by `browser_get_state`
    // on the tab this endpoint drives.
    let state_origin = match &origin {
        Origin::Cdp(url) => Some(ultra_instinct_browser_tools::state_origin(
            &Host(server),
            url,
        )),
        Origin::Fixture(_) => None,
    };
    let seen = match opt_snapshot(arguments, "seen_snapshot")? {
        Some(id) => {
            let seen = server.snapshot(id)?;
            if seen.origin != origin_key(&origin)
                && state_origin.as_deref() != Some(seen.origin.as_str())
            {
                return Err(ToolError::InvalidArguments(format!(
                    "seen_snapshot {id} was observed from {}, not {}",
                    seen.origin,
                    origin_key(&origin)
                )));
            }
            Some(WorldSnapshot::of(
                &seen.manifold,
                seen.page.focused().cloned(),
            ))
        }
        None => None,
    };
    let (snapshot, manifold, page) = observe_origin(server, &origin)?;
    let mut request = GuardRequest::click(query)
        .focused(page.focused().cloned())
        .snapshot_id(snapshot.get());
    if let Some(id) = proposed {
        request = request.proposed(id);
    }
    if let Some(seen) = seen {
        request = request.seen_world(seen);
    }
    let decision =
        guard(&manifold, &request).map_err(|err| ToolError::InvalidArguments(err.to_string()))?;
    let mut body = decision_json("guard", snapshot, &manifold, decision);
    let (focused, front_layer) = world_json(&manifold, &page);
    insert(&mut body, "focused", focused);
    insert(&mut body, "front_layer", front_layer);
    Ok(body)
}

fn build_guard_query(arguments: &Value) -> Result<LocateQuery, ToolError> {
    // `text` or `target` is the label; role, action and within narrow it.
    let mut args = arguments.clone();
    if args.get("text").is_none() {
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if let Some(target) = target {
            if let Some(obj) = args.as_object_mut() {
                obj.insert("text".into(), json!(target));
            }
        }
    }
    if args.get("action").is_none() {
        if let Some(obj) = args.as_object_mut() {
            obj.insert("action".into(), json!("click"));
        }
    }
    build_query(&args)
}

fn decision_json(
    tool: &str,
    snapshot: SnapshotId,
    _manifold: &InteractionManifold,
    decision: GuardDecision,
) -> Value {
    let (target, confidence, fallback) = match &decision {
        GuardDecision::Allow {
            target, confidence, ..
        } => (
            json!({
                "id": target.id.as_str(),
                "role": target.role.as_str(),
                "label": target.label,
                "confidence": target.confidence,
            }),
            Some(confidence.get()),
            None,
        ),
        GuardDecision::Refuse { reason, .. } | GuardDecision::Escalate { reason, .. } => {
            (Value::Null, None, Some(reason.as_str()))
        }
        _ => (Value::Null, None, Some("unknown")),
    };
    let mut body = outcome(
        tool,
        target,
        Some("click"),
        false, // never clicks
        false,
        empty_delta(),
        confidence,
        fallback,
        None,
    );
    insert(&mut body, "decision", json!(decision.as_str()));
    insert(&mut body, "snapshot", json!(snapshot.get()));
    match decision {
        GuardDecision::Allow {
            margin,
            evidence,
            ticket,
            ..
        } => {
            if let Some(m) = margin {
                insert(&mut body, "margin", json!(m.get()));
            }
            insert(
                &mut body,
                "evidence",
                json!({
                    "visible": evidence.visible,
                    "enabled": evidence.enabled,
                    "occluded": evidence.occluded,
                    "hidden": evidence.hidden,
                    "offscreen": evidence.offscreen,
                    "role": evidence.role.as_str(),
                }),
            );
            insert(
                &mut body,
                "ticket",
                json!({
                    "ticket_id": ticket.ticket_id,
                    "snapshot_id": ticket.snapshot_id,
                    "action": ticket.action.as_str(),
                    "target_id": ticket.target_id.as_str(),
                    "target_role": ticket.target_role.as_str(),
                    "target_label": ticket.target_label,
                    "target_fingerprint": format!("{:016x}", ticket.target_fingerprint),
                    "world_fingerprint": format!("{:016x}", ticket.world_fingerprint),
                }),
            );
        }
        GuardDecision::Refuse { candidates, reason }
        | GuardDecision::Escalate { candidates, reason } => {
            insert(&mut body, "reason", json!(reason.as_str()));
            let cands: Vec<Value> = candidates
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id.as_str(),
                        "role": c.role.as_str(),
                        "label": c.label,
                        "confidence": c.confidence,
                    })
                })
                .collect();
            insert(&mut body, "candidates", json!(cands));
        }
        _ => {}
    }
    body
}

fn opt_snapshot(arguments: &Value, key: &str) -> Result<Option<u64>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolError::BadSnapshot(number.to_string())),
        Some(other) => Err(ToolError::BadSnapshot(other.to_string())),
    }
}

fn verify_tool(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let expect_text = opt_str(arguments, "expect_text")?;
    let expect_absent = opt_str(arguments, "expect_absent")?;
    let expect_appeared = opt_str(arguments, "expect_appeared")?;
    let expect_disappeared = opt_str(arguments, "expect_disappeared")?;
    let expect_url_changed = arguments
        .get("expect_url_changed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let before_id = opt_snapshot(arguments, "before_snapshot")?;
    let after_id = opt_snapshot(arguments, "after_snapshot")?;

    // Ticketed delta verify: before/after snapshots + expectation, optionally
    // bound to an Allow ticket's target/world fingerprints.
    if before_id.is_some() || after_id.is_some() {
        let before_id = before_id.ok_or(ToolError::MissingBefore)?;
        let after_id = after_id.ok_or(ToolError::MissingAfter)?;
        let (before_m, before_p, after_m, after_p) = {
            let before = server.snapshot(before_id)?;
            let after = server.snapshot(after_id)?;
            (
                before.manifold.clone(),
                before.page.clone(),
                after.manifold.clone(),
                after.page.clone(),
            )
        };
        if let Some(ticket_obj) = arguments.get("ticket") {
            let ticket = parse_ticket(ticket_obj)?;
            // Ticket must still describe the *before* world/target.
            ultra_instinct_guard::revalidate(&ticket, &before_m, before_p.focused().cloned())
                .map_err(|err| ToolError::TicketInvalid(err.to_string()))?;
        }
        let regions = diff(&before_m, &after_m);
        let pages = page_delta(&before_p, &after_p);
        let expectation = match (
            expect_text,
            expect_absent,
            expect_appeared,
            expect_disappeared,
            expect_url_changed,
        ) {
            (None, None, Some(id), None, false) => {
                let region =
                    RegionId::try_new(id).map_err(|_| ToolError::UnknownRegion(id.to_owned()))?;
                Expectation::appeared(region)
            }
            (None, None, None, Some(id), false) => {
                let region =
                    RegionId::try_new(id).map_err(|_| ToolError::UnknownRegion(id.to_owned()))?;
                Expectation::disappeared(region)
            }
            (None, None, None, None, true) => Expectation::url_changed(),
            (Some(text), None, None, None, false) => {
                // Snapshot text check on *after*.
                return verify_text(&after_m, text);
            }
            (None, Some(id), None, None, false) => return verify_absent(&after_m, id),
            _ => return Err(ToolError::MissingExpect),
        };
        match verify_delta(&regions, &pages, &expectation) {
            Ok(()) => return Ok(verified_ok()),
            Err(VerifyError::NoEffect) => {
                return Err(ToolError::VerifyNoEffect);
            }
            Err(other) => return Err(ToolError::Browser(other.to_string())),
        }
    }

    let (_snapshot, manifold, _page) = observe_origin(server, &resolve_origin(arguments)?)?;
    match (expect_text, expect_absent) {
        (None, None) => Err(ToolError::MissingExpect),
        (Some(_), Some(_)) => Err(ToolError::BothExpectations),
        (Some(text), None) => verify_text(&manifold, text),
        (None, Some(id)) => verify_absent(&manifold, id),
    }
}

fn parse_ticket(value: &Value) -> Result<ultra_instinct_protocol::ActionTicket, ToolError> {
    use ultra_instinct_core::{Action, Role};
    use ultra_instinct_protocol::ActionTicket;
    let obj = value
        .as_object()
        .ok_or_else(|| ToolError::InvalidArguments("ticket must be an object".into()))?;
    let req = |key: &str| -> Result<&Value, ToolError> {
        obj.get(key)
            .ok_or_else(|| ToolError::InvalidArguments(format!("ticket missing {key}")))
    };
    let ticket_id = req("ticket_id")?
        .as_u64()
        .ok_or_else(|| ToolError::InvalidArguments("ticket.ticket_id".into()))?;
    let snapshot_id = req("snapshot_id")?
        .as_u64()
        .ok_or_else(|| ToolError::InvalidArguments("ticket.snapshot_id".into()))?;
    let action_raw = req("action")?
        .as_str()
        .ok_or_else(|| ToolError::InvalidArguments("ticket.action".into()))?;
    let action = match action_raw {
        "click" => Action::Click,
        other => {
            return Err(ToolError::InvalidArguments(format!(
                "unsupported ticket action `{other}`"
            )))
        }
    };
    let target_id = RegionId::try_new(
        req("target_id")?
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("ticket.target_id".into()))?,
    )
    .map_err(|_| ToolError::UnknownRegion("ticket.target_id".into()))?;
    let role_raw = req("target_role")?
        .as_str()
        .ok_or_else(|| ToolError::InvalidArguments("ticket.target_role".into()))?;
    let target_role = Role::parse(role_raw)
        .ok_or_else(|| ToolError::InvalidArguments(format!("unknown ticket role `{role_raw}`")))?;
    let target_label = req("target_label")?
        .as_str()
        .ok_or_else(|| ToolError::InvalidArguments("ticket.target_label".into()))?
        .to_owned();
    let parse_hex = |key: &str| -> Result<u64, ToolError> {
        let raw = req(key)?
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments(format!("ticket.{key}")))?;
        u64::from_str_radix(raw, 16)
            .map_err(|_| ToolError::InvalidArguments(format!("ticket.{key} must be hex")))
    };
    Ok(ActionTicket {
        ticket_id,
        snapshot_id,
        action,
        target_id,
        target_role,
        target_label,
        target_fingerprint: parse_hex("target_fingerprint")?,
        world_fingerprint: parse_hex("world_fingerprint")?,
        // MCP tickets are single-target: the host never drags through them.
        dest: None,
        cell: None,
        key: None,
    })
}

fn verify_text(manifold: &InteractionManifold, text: &str) -> Result<Value, ToolError> {
    let expectation = Expectation::text_present(text).map_err(|err| match err {
        VerifyError::EmptyExpectation => ToolError::EmptyText,
        other => ToolError::Browser(other.to_string()),
    })?;
    match verify(manifold, &expectation) {
        Ok(()) => Ok(verified_ok()),
        Err(VerifyError::ExpectedTextMissing { expected }) => {
            Err(ToolError::ExpectedTextMissing { expected })
        }
        Err(other) => Err(ToolError::Browser(other.to_string())),
    }
}

fn verify_absent(manifold: &InteractionManifold, id: &str) -> Result<Value, ToolError> {
    let region = RegionId::try_new(id).map_err(|_| ToolError::UnknownRegion(id.to_owned()))?;
    match verify(manifold, &Expectation::region_absent(region)) {
        Ok(()) => Ok(verified_ok()),
        Err(VerifyError::RegionStillPresent { id }) => Err(ToolError::RegionStillPresent { id }),
        Err(other) => Err(ToolError::Browser(other.to_string())),
    }
}

fn verified_ok() -> Value {
    outcome(
        "verify",
        Value::Null,
        None,
        false,
        true,
        empty_delta(),
        None,
        None,
        None,
    )
}

fn resolve_origin(arguments: &Value) -> Result<Origin, ToolError> {
    let fixture = opt_str(arguments, "fixture")?;
    let cdp = opt_str(arguments, "cdp")?;
    match (fixture, cdp) {
        (Some(_), Some(_)) => Err(ToolError::DuplicateSource),
        (None, None) => Err(ToolError::MissingFixture),
        (Some(path), None) => Ok(Origin::Fixture(path.to_owned())),
        (None, Some(url)) => Ok(Origin::Cdp(url.to_owned())),
    }
}

fn origin_key(origin: &Origin) -> String {
    match origin {
        Origin::Fixture(path) => format!("fixture:{path}"),
        Origin::Cdp(url) => format!("cdp:{url}"),
    }
}

/// Observe `origin` and record it in the server's ring. A live session is kept
/// for the next call; a failed live call drops it.
fn observe_origin(
    server: &mut Server,
    origin: &Origin,
) -> Result<(SnapshotId, Rc<InteractionManifold>, PageState), ToolError> {
    let (manifold, page) = match origin {
        Origin::Fixture(path) => load_observation(path)?,
        Origin::Cdp(url) => {
            let mut session = Host(server).take_session(url)?;
            let manifold = match session.observe().cloned() {
                Ok(manifold) => manifold,
                Err(err) => {
                    // The session is dropped, but a dialog it saw still
                    // names itself in the error (`fill_dialog`).
                    let events = session.transport_mut().drain_events();
                    if !events.is_empty() {
                        Host(server).push_events(url, events);
                    }
                    return Err(ToolError::Browser(err.to_string()));
                }
            };
            let page = session.page().cloned().unwrap_or_else(PageState::blank);
            Host(server).keep_session(url, session);
            (manifold, page)
        }
    };
    let manifold = Rc::new(manifold);
    // observe / guard / verify snapshots are not index-bound (`shown` is moot).
    let id = Host(server).record(
        &origin_key(origin),
        Rc::clone(&manifold),
        page.clone(),
        usize::MAX,
    );
    Ok((id, manifold, page))
}

fn load_observation(path: &str) -> Result<(InteractionManifold, PageState), ToolError> {
    let body = read_path(path)?;
    if body.trim_start().starts_with('{') {
        let transport =
            ReplayTransport::parse(&body).map_err(|err| ToolError::Browser(err.to_string()))?;
        let mut session = BrowserSession::new(transport);
        let manifold = session
            .observe()
            .cloned()
            .map_err(|err| ToolError::Browser(err.to_string()))?;
        let page = session.page().cloned().unwrap_or_else(PageState::blank);
        Ok((manifold, page))
    } else {
        parse_fixture(&body)
            .map(|manifold| (manifold, PageState::blank()))
            .map_err(|err| ToolError::Fixture(err.to_string()))
    }
}

fn read_path(path: &str) -> Result<String, ToolError> {
    fs::read_to_string(Path::new(path)).map_err(|err| ToolError::Io {
        path: path.to_owned(),
        message: err.to_string(),
    })
}

fn build_query(arguments: &Value) -> Result<LocateQuery, ToolError> {
    let mut query = LocateQuery::new();
    if let Some(text) = opt_str(arguments, "text")? {
        query = query.text(text).map_err(|_| ToolError::EmptyText)?;
    }
    if let Some(role) = opt_str(arguments, "role")? {
        let parsed = Role::parse(&role.to_ascii_lowercase())
            .ok_or_else(|| ToolError::UnknownRole(role.to_owned()))?;
        query = query.role(parsed);
    }
    if let Some(action) = opt_str(arguments, "action")? {
        let parsed = Action::parse(&action.to_ascii_lowercase())
            .ok_or_else(|| ToolError::UnknownAction(action.to_owned()))?;
        query = query.action(parsed);
    }
    if let Some(within) = opt_str(arguments, "within")? {
        let id =
            RegionId::try_new(within).map_err(|_| ToolError::UnknownRegion(within.to_owned()))?;
        query = query.within(id);
    }
    Ok(query)
}

fn opt_str<'a>(arguments: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(ToolError::InvalidArguments(format!(
            "{key} must be a string"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn outcome(
    tool: &str,
    target: Value,
    action: Option<&str>,
    executed: bool,
    verified: bool,
    state_delta: Value,
    confidence: Option<f64>,
    fallback: Option<&str>,
    executor: Option<&str>,
) -> Value {
    json!({
        "product": PRODUCT,
        "tool": tool,
        "target": target,
        "action": action,
        "executed": executed,
        "verified": verified,
        "state_delta": state_delta,
        "confidence": confidence,
        "fallback": fallback,
        "executor": executor,
    })
}

fn insert(body: &mut Value, key: &str, value: Value) {
    body.as_object_mut()
        .expect("tool result is a JSON object")
        .insert(key.to_owned(), value);
}

fn empty_delta() -> Value {
    json!({"added": [], "removed": [], "changed": []})
}

/// `{"availability": "enabled"|"disabled", "visibility": "visible"|"occluded"|"offscreen"|"hidden"}`.
fn state_json(manifold: &InteractionManifold, region: &InteractionRegion) -> Value {
    let state = RegionState::of(manifold.viewport(), region);
    json!({
        "availability": state.availability().as_str(),
        "visibility": state.visibility().as_str(),
    })
}
