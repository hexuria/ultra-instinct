//! MCP tools for the action-firewall product.
//!
//! Product tools: observe, guard, verify. Locate, inspect, and diff remain as
//! transitional helpers. `act` is deprecated: it runs the same decision path as
//! `guard` and never clicks.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use ultra_instinct_browser::{
    page_delta, verify, verify_delta, BrowserSession, Expectation, PageState, ReplayTransport,
    VerifyError,
};
use ultra_instinct_core::{
    parse_fixture, Action, InteractionManifold, InteractionRegion, LocateQuery, RegionId, Role,
    Zone,
};
use ultra_instinct_guard::{
    blocker, guard_with, with_front_layer, FrontLayer, GuardRequest, WorldSnapshot,
    MIN_ALLOW_CONFIDENCE,
};
use ultra_instinct_observe::{diff, history::SnapshotId, ManifoldDiff};
use ultra_instinct_protocol::{GuardDecision, StateDelta};
use ultra_instinct_resonance::{
    separating_zone, ContextScope, Match, RegionMatcher, RegionState, WeightedMatcher,
};
#[cfg(feature = "hgra")]
use ultra_instinct_resonance::{Dims, HgraMatcher, ResonanceModel};

use crate::error::ToolError;
use crate::repeat::{QueryKey, REPEAT_THRESHOLD};
use crate::server::Server;

const PRODUCT: &str = "ultra-instinct";

const _: () = assert!(ultra_instinct_resonance::TEXT_MISS_CAP < MIN_ALLOW_CONFIDENCE);

enum Origin {
    Fixture(String),
    Cdp(String),
}

/// The `near` argument: the focused region of the current observation (the
/// "cursor"), or an explicit region id.
enum Near {
    Focus,
    Region(RegionId),
}

impl Near {
    /// The anchor for this observation. `focus` with no focused region is no
    /// anchor, which is the default ranking.
    fn anchor(&self, page: &PageState) -> Option<RegionId> {
        match self {
            Self::Focus => page.focused().cloned(),
            Self::Region(id) => Some(id.clone()),
        }
    }
}

fn parse_near(arguments: &Value) -> Result<Option<Near>, ToolError> {
    match opt_str(arguments, "near")? {
        None => Ok(None),
        Some("focus") => Ok(Some(Near::Focus)),
        Some(raw) => RegionId::try_new(raw)
            .map(|id| Some(Near::Region(id)))
            .map_err(|_| ToolError::UnknownRegion(raw.to_owned())),
    }
}

fn with_near(query: LocateQuery, near: Option<&Near>, page: &PageState) -> LocateQuery {
    match near {
        Some(near) => query.near(near.anchor(page)),
        None => query,
    }
}

/// `{"focused": id|null, "front_layer": [{id, label, modal}]}` facts of one
/// observation, shared by observe, locate, and guard.
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

/// Explicit `matcher` arg wins; else `ULTRA_INSTINCT_MATCHER`; else weighted.
fn resolve_matcher_name(arguments: &Value) -> Result<String, ToolError> {
    if let Some(name) = opt_str(arguments, "matcher")? {
        return Ok(name.to_owned());
    }
    match std::env::var("ULTRA_INSTINCT_MATCHER") {
        Ok(name) if !name.is_empty() => Ok(name),
        _ => Ok("weighted".into()),
    }
}

#[cfg(feature = "hgra")]
fn hgra_matcher(arguments: &Value) -> Result<HgraMatcher, ToolError> {
    let dims = match opt_u64(arguments, "dims")? {
        Some(n) => {
            let width = usize::try_from(n).map_err(|_| ToolError::BadDims(n.to_string()))?;
            Dims::try_from_usize(width).map_err(|_| ToolError::BadDims(n.to_string()))?
        }
        None => Dims::DEFAULT,
    };
    Ok(HgraMatcher::new(dims, ResonanceModel::V1))
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
    if name.starts_with("browser_") {
        return crate::browser_tools::call(server, name, arguments);
    }
    match name {
        "navigate" => Err(ToolError::GoalNotAccepted),
        "observe" => observe(server, arguments),
        "locate" => locate(server, arguments),
        "inspect" => inspect(server, arguments),
        "guard" => guard_tool(server, arguments),
        // Deprecated: same decision as guard, never clicks.
        "act" => guard_tool(server, arguments),
        "diff" => diff_tool(server, arguments),
        "verify" => verify_tool(server, arguments),
        other => Err(ToolError::UnknownTool(other.to_owned())),
    }
}

fn observe(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let (snapshot, raw, page) = observe_origin(server, &resolve_origin(arguments)?)?;
    // State as a person sees it: regions behind an open dialog read occluded.
    let manifold = with_front_layer(&raw);
    let regions: Vec<Value> = manifold
        .regions()
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
    Ok(body)
}

fn locate(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let origin = resolve_origin(arguments)?;
    let (snapshot, raw, page) = observe_origin(server, &origin)?;
    let near = parse_near(arguments)?;
    let query = with_near(build_query(arguments)?, near.as_ref(), &page);
    let manifold = with_front_layer(&raw);
    let matcher_name = resolve_matcher_name(arguments)?;
    if arguments.get("dims").is_some() && matcher_name != "hgra" {
        return Err(ToolError::DimsRequireHgra);
    }
    let ranked = rank(manifold.as_ref(), &query, &matcher_name, arguments)?;
    let key = QueryKey::new(&query, &matcher_name, opt_u64(arguments, "dims")?);
    let count = server.record_locate(&origin_key(&origin), &raw, key);
    let top = ranked.first();
    let target = match top {
        Some(candidate) => target_of(&manifold, candidate.id()),
        None => Value::Null,
    };
    let confidence = top.map(Match::confidence);
    // Omit executed and verified. ComputerResult always sets both, so a locate
    // that carries executed: false looks like a refusal. This call does not press.
    let mut body = without_act_flags(outcome(
        "locate",
        target,
        None,
        false,
        false,
        empty_delta(),
        confidence,
        None,
        None,
    ));
    insert(&mut body, "matcher", json!(matcher_name));
    insert(&mut body, "benchmark", json!(false));
    insert(&mut body, "candidates", candidates_of(&manifold, &ranked));
    insert(&mut body, "scope", scope_json(&query, &manifold));
    insert(&mut body, "snapshot", json!(snapshot.get()));
    let signals = if count >= REPEAT_THRESHOLD {
        vec![repeated_query_json(&manifold, &ranked, count)]
    } else {
        Vec::new()
    };
    insert(&mut body, "signals", Value::Array(signals));
    Ok(body)
}

/// `repeated_query`: this exact query already ran on this exact page state.
/// Names the top and runner-up, each with a position zone that separates it
/// from the other (or null). Data only. The ranking above is unchanged.
fn repeated_query_json(manifold: &InteractionManifold, ranked: &[Match], count: usize) -> Value {
    let rect = |candidate: &Match| manifold.get(candidate.id()).map(|region| region.rect());
    let hint = |candidate: &Match, other: Option<&Match>| {
        let zone = match (rect(candidate), other.and_then(rect)) {
            (Some(own), Some(theirs)) => separating_zone(manifold.viewport(), own, theirs),
            _ => None,
        };
        json!({
            "id": candidate.id().as_str(),
            "suggested_position": zone.map(|zone| zone.as_str()),
        })
    };
    let top = ranked.first();
    let runner_up = ranked.get(1);
    json!({
        "kind": "repeated_query",
        "count": count,
        "top": top.map_or(Value::Null, |top| hint(top, runner_up)),
        "runner_up": runner_up.map_or(Value::Null, |runner| hint(runner, top)),
    })
}

fn rank(
    manifold: &InteractionManifold,
    query: &LocateQuery,
    matcher: &str,
    arguments: &Value,
) -> Result<Vec<Match>, ToolError> {
    match matcher {
        "weighted" => {
            if opt_u64(arguments, "dims")?.is_some() {
                return Err(ToolError::DimsRequireHgra);
            }
            WeightedMatcher::default()
                .rank(query, manifold)
                .map_err(|err| ToolError::Ranker(err.to_string()))
        }
        "hgra" => {
            #[cfg(feature = "hgra")]
            {
                hgra_matcher(arguments)?
                    .rank(query, manifold)
                    .map_err(|err| ToolError::Ranker(err.to_string()))
            }
            #[cfg(not(feature = "hgra"))]
            {
                let _ = arguments;
                Err(ToolError::UnknownMatcher(
                    "hgra is an experiment; build with ultra-instinct-resonance feature `hgra`"
                        .into(),
                ))
            }
        }
        other => Err(ToolError::UnknownMatcher(other.to_owned())),
    }
}

fn inspect(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let (snapshot, raw, _page) = observe_origin(server, &resolve_origin(arguments)?)?;
    let manifold = with_front_layer(&raw);
    let region = require_region(arguments)?;
    let found = manifold
        .get(&region)
        .ok_or_else(|| ToolError::UnknownRegion(region.to_string()))?;
    let target = json!({
        "id": found.id().as_str(),
        "role": found.role().as_str(),
        "label": found.label(),
        "state": state_json(manifold.as_ref(), found),
        "parent": found.parent().map_or(Value::Null, |id| json!(id.as_str())),
        "blocked_by": blocker(&raw, found).map_or(Value::Null, |dialog| json!(dialog.id().as_str())),
        "x": found.rect().x(),
        "y": found.rect().y(),
        "width": found.rect().width(),
        "height": found.rect().height(),
    });
    // Same omission as locate. Inspect reads one region and does not press.
    let mut body = without_act_flags(outcome(
        "inspect",
        target,
        None,
        false,
        false,
        empty_delta(),
        None,
        None,
        None,
    ));
    insert(&mut body, "snapshot", json!(snapshot.get()));
    Ok(body)
}

fn guard_tool(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let origin = resolve_origin(arguments)?;
    let base_query = build_guard_query(arguments)?;
    let near = parse_near(arguments)?;
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
    let matcher_name = resolve_matcher_name(arguments)?;
    if arguments.get("dims").is_some() && matcher_name != "hgra" {
        return Err(ToolError::DimsRequireHgra);
    }
    match matcher_name.as_str() {
        "weighted" | "hgra" => {}
        other => return Err(ToolError::UnknownMatcher(other.to_owned())),
    }
    #[cfg(not(feature = "hgra"))]
    if matcher_name == "hgra" {
        return Err(ToolError::UnknownMatcher(
            "hgra is an experiment; build with ultra-instinct-resonance feature `hgra`".into(),
        ));
    }
    // The observation the host decided on. Its world snapshot (focus, dialogs,
    // clickable ids, occluded set) is compared with the world observed now.
    let seen = match opt_snapshot(arguments, "seen_snapshot")? {
        Some(id) => {
            let seen = server.snapshot(id)?;
            if seen.origin != origin_key(&origin) {
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
    let query = with_near(base_query, near.as_ref(), &page);
    let mut request = GuardRequest::click(query.clone())
        .focused(page.focused().cloned())
        .snapshot_id(snapshot.get());
    if let Some(id) = proposed {
        request = request.proposed(id);
    }
    if let Some(seen) = seen {
        request = request.seen_world(seen);
    }
    let decision = match matcher_name.as_str() {
        "weighted" => guard_with(&manifold, &request, &WeightedMatcher::default())
            .map_err(|err| ToolError::Ranker(err.to_string()))?,
        #[cfg(feature = "hgra")]
        "hgra" => guard_with(&manifold, &request, &hgra_matcher(arguments)?)
            .map_err(|err| ToolError::Ranker(err.to_string()))?,
        _ => unreachable!("matcher vetted above"),
    };
    let mut body = decision_json("guard", snapshot, &manifold, decision);
    insert(&mut body, "matcher", json!(matcher_name));
    let (focused, front_layer) = world_json(&manifold, &page);
    insert(&mut body, "focused", focused);
    insert(&mut body, "front_layer", front_layer);
    insert(
        &mut body,
        "scope",
        scope_json(&query, with_front_layer(&manifold).as_ref()),
    );
    Ok(body)
}

/// `{"within": id|null, "near": anchor|null, "near_scope": id|null}`: the
/// context the ranking used. `near_scope` null with an anchor means the
/// anchor gave no scope and the default ranking ran.
fn scope_json(query: &LocateQuery, manifold: &InteractionManifold) -> Value {
    let scope = ContextScope::resolve(query, manifold);
    let id = |id: Option<&RegionId>| id.map_or(Value::Null, |id| json!(id.as_str()));
    json!({
        "within": id(scope.within()),
        "near": id(query.near_ref()),
        "near_scope": id(scope.near_scope()),
    })
}

fn build_guard_query(arguments: &Value) -> Result<LocateQuery, ToolError> {
    // Accept locate-style fields, or `target` as the text label.
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

fn diff_tool(server: &mut Server, arguments: &Value) -> Result<Value, ToolError> {
    let before_path = opt_str(arguments, "before")?;
    let after_path = opt_str(arguments, "after")?;
    let before_id = opt_snapshot(arguments, "before_snapshot")?;
    let after_id = opt_snapshot(arguments, "after_snapshot")?;
    let by_path = before_path.is_some() || after_path.is_some();
    let by_id = before_id.is_some() || after_id.is_some();
    let (delta, pages) = match (by_path, by_id) {
        (true, true) => return Err(ToolError::MixedDiffSources),
        (false, true) => {
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
            (diff(&before_m, &after_m), page_delta(&before_p, &after_p))
        }
        _ => {
            let (before_m, before_p) =
                load_observation(before_path.ok_or(ToolError::MissingBefore)?)?;
            let (after_m, after_p) = load_observation(after_path.ok_or(ToolError::MissingAfter)?)?;
            (diff(&before_m, &after_m), page_delta(&before_p, &after_p))
        }
    };
    Ok(outcome(
        "diff",
        Value::Null,
        None,
        false,
        false,
        delta_json(&delta, &pages),
        None,
        None,
        None,
    ))
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

fn delta_json(delta: &ManifoldDiff, page: &ultra_instinct_browser::PageDelta) -> Value {
    let changed: Vec<ultra_instinct_core::RegionId> = delta
        .changed()
        .iter()
        .map(|change| change.id().clone())
        .collect();
    let state = StateDelta::new(delta.added().to_vec(), delta.removed().to_vec(), changed)
        .with_moved(delta.moved().cloned().collect())
        .with_text_changed(delta.relabeled().cloned().collect())
        .with_focus_changed(page.focus_changed())
        .with_url_changed(page.url_changed());
    json!({
        "added": id_strings(state.added()),
        "removed": id_strings(state.removed()),
        "changed": id_strings(state.changed()),
        "moved": id_strings(state.moved()),
        "text_changed": id_strings(state.text_changed()),
        "focus_changed": state.focus_changed(),
        "url_changed": state.url_changed(),
        "title_changed": page.title_changed(),
    })
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
) -> Result<(SnapshotId, InteractionManifold, PageState), ToolError> {
    let (manifold, page) = match origin {
        Origin::Fixture(path) => load_observation(path)?,
        Origin::Cdp(url) => {
            let mut session = server.take_session(url)?;
            let manifold = session
                .observe()
                .cloned()
                .map_err(|err| ToolError::Browser(err.to_string()))?;
            let page = session.page().cloned().unwrap_or_else(PageState::blank);
            server.keep_session(url, session);
            (manifold, page)
        }
    };
    let id = server.record(&origin_key(origin), manifold.clone(), page.clone());
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

fn require_region(arguments: &Value) -> Result<RegionId, ToolError> {
    let raw = opt_str(arguments, "region")?.ok_or(ToolError::MissingRegion)?;
    RegionId::try_new(raw).map_err(|_| ToolError::UnknownRegion(raw.to_owned()))
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
    if let Some(position) = opt_str(arguments, "position")? {
        let parsed = Zone::parse(&position.to_ascii_lowercase())
            .ok_or_else(|| ToolError::UnknownPosition(position.to_owned()))?;
        query = query.position(parsed);
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

fn opt_u64(arguments: &Value, key: &str) -> Result<Option<u64>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolError::BadDims(number.to_string())),
        Some(other) => Err(ToolError::BadDims(other.to_string())),
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

fn without_act_flags(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        object.remove("executed");
        object.remove("verified");
    }
    body
}

fn insert(body: &mut Value, key: &str, value: Value) {
    body.as_object_mut()
        .expect("tool result is a JSON object")
        .insert(key.to_owned(), value);
}

fn empty_delta() -> Value {
    json!({"added": [], "removed": [], "changed": []})
}

fn target_of(manifold: &InteractionManifold, id: &RegionId) -> Value {
    match manifold.get(id) {
        Some(region) => json!({
            "id": region.id().as_str(),
            "role": region.role().as_str(),
            "label": region.label(),
        }),
        None => json!({"id": id.as_str()}),
    }
}

fn candidates_of(manifold: &InteractionManifold, ranked: &[Match]) -> Value {
    let rows: Vec<Value> = ranked
        .iter()
        .map(|candidate| {
            let region = manifold.get(candidate.id());
            json!({
                "rank": candidate.rank(),
                "id": candidate.id().as_str(),
                "role": region.map(|region| region.role().as_str()).unwrap_or("unknown"),
                "label": region.map(|region| region.label()).unwrap_or(""),
                "state": region.map_or(Value::Null, |region| state_json(manifold, region)),
                "confidence": candidate.confidence(),
            })
        })
        .collect();
    Value::Array(rows)
}

/// `{"availability": "enabled"|"disabled", "visibility": "visible"|"occluded"|"offscreen"|"hidden"}`.
fn state_json(manifold: &InteractionManifold, region: &InteractionRegion) -> Value {
    let state = RegionState::of(manifold.viewport(), region);
    json!({
        "availability": state.availability().as_str(),
        "visibility": state.visibility().as_str(),
    })
}

fn id_strings(ids: &[RegionId]) -> Vec<&str> {
    ids.iter().map(RegionId::as_str).collect()
}
