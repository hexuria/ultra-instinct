//! observe, act, verify, diff, and inspect. Locate stays in the parent module.
//!
//! `--cdp` with no value uses [`ultra_instinct_browser::DEFAULT_CDP_HTTP`].
//! A fixture whose first non-whitespace byte is `{` is a CDP replay script.
//! Anything else is a manifold fixture. Live Chrome is only contacted when
//! `--cdp` is set.

use std::fs;
use std::path::Path;

use ultra_instinct_browser::{
    BrowserSession, Expectation, ReplayTransport, WebSocketTransport, DEFAULT_CDP_HTTP,
};
use ultra_instinct_core::{parse_fixture, InteractionManifold, LocateQuery, RegionId, Role, Zone};
use ultra_instinct_guard::{guard, GuardDecision, GuardRequest};
use ultra_instinct_observe::diff;

use crate::{set_once, CliError};

pub(crate) fn load_source(
    fixture: Option<&str>,
    cdp: Option<&str>,
) -> Result<InteractionManifold, CliError> {
    if fixture.is_some() && cdp.is_some() {
        return Err(CliError::DuplicateFlag("--cdp"));
    }
    if let Some(url) = cdp {
        let mut session = BrowserSession::new(connect_live(url)?);
        return session
            .observe()
            .cloned()
            .map_err(|err| CliError::Browser(err.to_string()));
    }
    let Some(path) = fixture else {
        return Err(CliError::MissingSource);
    };
    load_fixture(path)
}

pub(crate) fn observe_command(args: &[String]) -> Result<String, CliError> {
    let source = parse_source(args)?;
    let manifold = load_source(source.fixture.as_deref(), source.cdp.as_deref())?;
    Ok(render_regions(&manifold))
}

pub(crate) fn inspect_command(args: &[String]) -> Result<String, CliError> {
    let mut region: Option<String> = None;
    let mut rest = Vec::new();
    for arg in args {
        if region.is_none() && !arg.starts_with("--") {
            region = Some(arg.clone());
        } else {
            rest.push(arg.clone());
        }
    }
    let region = region.ok_or(CliError::MissingRegion)?;
    let source = parse_source(&rest)?;
    let manifold = load_source(source.fixture.as_deref(), source.cdp.as_deref())?;
    let found = manifold
        .get_str(&region)
        .ok_or_else(|| CliError::UnknownRegion(region.clone()))?;
    Ok(format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        found.id(),
        found.role(),
        found.label(),
        found.rect().x(),
        found.rect().y(),
        found.rect().width(),
        found.rect().height()
    ))
}

pub(crate) fn act_command(args: &[String]) -> Result<String, CliError> {
    // Deprecated alias of guard: never clicks.
    guard_command(args)
}

pub(crate) fn guard_command(args: &[String]) -> Result<String, CliError> {
    let mut text: Option<String> = None;
    let mut role: Option<String> = None;
    let mut position: Option<String> = None;
    let mut proposed: Option<String> = None;
    let mut fixture: Option<String> = None;
    let mut cdp: Option<String> = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--fixture=") {
            set_once("fixture", &mut fixture, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--cdp=") {
            set_once("cdp", &mut cdp, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--text=") {
            set_once("text", &mut text, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--target=") {
            set_once("text", &mut text, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--role=") {
            set_once("role", &mut role, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--position=") {
            set_once("position", &mut position, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--proposed=") {
            set_once("proposed", &mut proposed, value.to_owned())?;
        } else if arg == "--json" {
            if json {
                return Err(CliError::DuplicateFlag("--json"));
            }
            json = true;
        } else if arg == "--cdp" {
            if cdp.is_some() {
                return Err(CliError::DuplicateFlag("--cdp"));
            }
            let next = args.get(index + 1);
            if next.is_some_and(|value| !value.starts_with("--")) {
                cdp = Some(next.unwrap().clone());
                index += 1;
            } else {
                cdp = Some(DEFAULT_CDP_HTTP.to_owned());
            }
        } else if arg == "--fixture"
            || arg == "--text"
            || arg == "--target"
            || arg == "--role"
            || arg == "--position"
            || arg == "--proposed"
        {
            index += 1;
            let flag: &'static str = match arg.as_str() {
                "--fixture" => "--fixture",
                "--text" | "--target" => "--text",
                "--role" => "--role",
                "--position" => "--position",
                "--proposed" => "--proposed",
                other => return Err(CliError::UnknownFlag(other.to_owned())),
            };
            let Some(value) = args.get(index) else {
                return Err(CliError::MissingValue(flag));
            };
            if value.starts_with("--") {
                return Err(CliError::MissingValue(flag));
            }
            match flag {
                "--fixture" => set_once("fixture", &mut fixture, value.clone())?,
                "--text" | "--target" => set_once("text", &mut text, value.clone())?,
                "--role" => set_once("role", &mut role, value.clone())?,
                "--position" => set_once("position", &mut position, value.clone())?,
                "--proposed" => set_once("proposed", &mut proposed, value.clone())?,
                _ => unreachable!(),
            }
        } else if arg.starts_with("--") {
            return Err(CliError::UnknownFlag(arg.clone()));
        } else if text.is_none() {
            // positional: target text, or legacy `region press` form
            if arg == "press" || arg == "click" {
                // ignore legacy verb; target must already be set via --text/--target
            } else {
                text = Some(arg.clone());
            }
        } else if arg == "press" || arg == "click" {
            // legacy verb after a region id — treat previous positional as proposed id
            proposed = text.take();
            // need text from somewhere else; leave error if missing below
        } else {
            return Err(CliError::UnknownFlag(arg.clone()));
        }
        index += 1;
    }
    let text = text.ok_or(CliError::EmptyText)?;
    let mut query = LocateQuery::new()
        .text(text)
        .map_err(|_| CliError::EmptyText)?;
    if let Some(role) = role {
        let parsed = Role::parse(&role).ok_or(CliError::UnknownRole(role))?;
        query = query.role(parsed);
    }
    if let Some(position) = position {
        let parsed = Zone::parse(&position).ok_or(CliError::UnknownPosition(position))?;
        query = query.position(parsed);
    }
    let mut request = GuardRequest::click(query);
    if let Some(raw) = proposed {
        let id = RegionId::try_new(&raw).map_err(|_| CliError::UnknownRegion(raw.clone()))?;
        request = request.proposed(id);
    }
    let manifold = load_source(fixture.as_deref(), cdp.as_deref())?;
    let decision = guard(&manifold, &request).map_err(|err| CliError::Locate(err.to_string()))?;
    Ok(render_decision(&decision, json))
}

fn render_decision(decision: &GuardDecision, json: bool) -> String {
    match decision {
        GuardDecision::Allow {
            target,
            confidence,
            margin,
            evidence,
            ticket,
        } => {
            let _ticket_id = ticket.ticket_id;
            if json {
                format!(
                    "{{\"decision\":\"allow\",\"id\":\"{}\",\"label\":\"{}\",\"confidence\":{},\"margin\":{},\"enabled\":{},\"visible\":{}}}\n",
                    target.id.as_str(),
                    target.label.replace('"', "\\\""),
                    confidence.get(),
                    margin.map(|m| m.get()).unwrap_or(0.0),
                    evidence.enabled,
                    evidence.visible,
                )
            } else {
                format!(
                    "allow\t{}\t{}\t{:.4}\n",
                    target.id.as_str(),
                    target.label,
                    confidence.get()
                )
            }
        }
        GuardDecision::Refuse { reason, candidates }
        | GuardDecision::Escalate { reason, candidates } => {
            let kind = decision.as_str();
            let top = candidates
                .first()
                .map(|c| format!("{}\t{}", c.id.as_str(), c.label))
                .unwrap_or_default();
            if json {
                format!(
                    "{{\"decision\":\"{}\",\"reason\":\"{}\",\"candidates\":{}}}\n",
                    kind,
                    reason.as_str(),
                    candidates.len(),
                )
            } else {
                format!("{}\t{}\t{top}\n", kind, reason.as_str())
            }
        }
        _ => format!("{}\n", decision.as_str()),
    }
}

pub(crate) fn verify_command(args: &[String]) -> Result<String, CliError> {
    let mut expect_text: Option<String> = None;
    let mut expect_absent: Option<String> = None;
    let mut fixture: Option<String> = None;
    let mut cdp: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--expect-text=") {
            set_once("expect-text", &mut expect_text, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--expect-absent=") {
            set_once("expect-absent", &mut expect_absent, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--fixture=") {
            set_once("fixture", &mut fixture, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--cdp=") {
            set_once("cdp", &mut cdp, value.to_owned())?;
        } else if arg == "--expect-text"
            || arg == "--expect-absent"
            || arg == "--fixture"
            || arg == "--cdp"
        {
            let flag = match arg.as_str() {
                "--expect-text" => "--expect-text",
                "--expect-absent" => "--expect-absent",
                "--fixture" => "--fixture",
                "--cdp" => "--cdp",
                _ => "--flag",
            };
            if arg == "--cdp" {
                let next = args.get(index + 1);
                if next.is_some_and(|value| !value.starts_with("--")) {
                    set_once("cdp", &mut cdp, next.unwrap().clone())?;
                    index += 1;
                } else {
                    set_once("cdp", &mut cdp, DEFAULT_CDP_HTTP.to_owned())?;
                }
            } else {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(CliError::MissingValue(flag));
                };
                if value.starts_with("--") {
                    return Err(CliError::MissingValue(flag));
                }
                let slot = match arg.as_str() {
                    "--expect-text" => &mut expect_text,
                    "--expect-absent" => &mut expect_absent,
                    "--fixture" => &mut fixture,
                    _ => unreachable!("flag matched"),
                };
                let name = match arg.as_str() {
                    "--expect-text" => "expect-text",
                    "--expect-absent" => "expect-absent",
                    "--fixture" => "fixture",
                    _ => "flag",
                };
                set_once(name, slot, value.clone())?;
            }
        } else {
            return Err(CliError::UnknownFlag(arg.clone()));
        }
        index += 1;
    }
    let manifold = load_source(fixture.as_deref(), cdp.as_deref())?;
    if let Some(text) = expect_text {
        let expectation = Expectation::text_present(text).map_err(|err| match err {
            ultra_instinct_browser::VerifyError::EmptyExpectation => CliError::EmptyText,
            other => CliError::Browser(other.to_string()),
        })?;
        return match ultra_instinct_browser::verify(&manifold, &expectation) {
            Ok(()) => Ok("verified\n".to_owned()),
            Err(ultra_instinct_browser::VerifyError::ExpectedTextMissing { expected }) => {
                Err(CliError::ExpectedTextMissing { expected })
            }
            Err(other) => Err(CliError::Browser(other.to_string())),
        };
    }
    if let Some(id) = expect_absent {
        let region = RegionId::try_new(&id).map_err(|_| CliError::UnknownRegion(id))?;
        return match ultra_instinct_browser::verify(&manifold, &Expectation::region_absent(region))
        {
            Ok(()) => Ok("verified\n".to_owned()),
            Err(ultra_instinct_browser::VerifyError::RegionStillPresent { id }) => {
                Err(CliError::RegionStillPresent { id })
            }
            Err(other) => Err(CliError::Browser(other.to_string())),
        };
    }
    Err(CliError::MissingExpect)
}

pub(crate) fn diff_command(args: &[String]) -> Result<String, CliError> {
    let mut before: Option<String> = None;
    let mut after: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--before=") {
            set_once("before", &mut before, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--after=") {
            set_once("after", &mut after, value.to_owned())?;
        } else if arg == "--before" || arg == "--after" {
            let flag = if arg == "--before" {
                "--before"
            } else {
                "--after"
            };
            let name = if arg == "--before" { "before" } else { "after" };
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(CliError::MissingValue(flag));
            };
            if value.starts_with("--") {
                return Err(CliError::MissingValue(flag));
            }
            let slot = if arg == "--before" {
                &mut before
            } else {
                &mut after
            };
            set_once(name, slot, value.clone())?;
        } else {
            return Err(CliError::UnknownFlag(arg.clone()));
        }
        index += 1;
    }
    let before = load_fixture(&before.ok_or(CliError::MissingValue("--before"))?)?;
    let after = load_fixture(&after.ok_or(CliError::MissingValue("--after"))?)?;
    let delta = diff(&before, &after);
    let mut out = String::new();
    for id in delta.added() {
        out.push_str(&format!("added\t{id}\n"));
    }
    for id in delta.removed() {
        out.push_str(&format!("removed\t{id}\n"));
    }
    for change in delta.changed() {
        let fields: Vec<_> = change
            .fields()
            .iter()
            .map(|field| format!("{field:?}"))
            .collect();
        out.push_str(&format!("changed\t{}\t{}\n", change.id(), fields.join(",")));
    }
    if out.is_empty() {
        out.push_str("unchanged\n");
    }
    Ok(out)
}

struct Source {
    fixture: Option<String>,
    cdp: Option<String>,
}

fn parse_source(args: &[String]) -> Result<Source, CliError> {
    let mut fixture = None;
    let mut cdp = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--fixture=") {
            set_once("fixture", &mut fixture, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--cdp=") {
            set_once("cdp", &mut cdp, value.to_owned())?;
        } else if arg == "--fixture" {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(CliError::MissingValue("--fixture"));
            };
            if value.starts_with("--") {
                return Err(CliError::MissingValue("--fixture"));
            }
            set_once("fixture", &mut fixture, value.clone())?;
        } else if arg == "--cdp" {
            if cdp.is_some() {
                return Err(CliError::DuplicateFlag("--cdp"));
            }
            let next = args.get(index + 1);
            if next.is_some_and(|value| !value.starts_with("--")) {
                cdp = Some(next.unwrap().clone());
                index += 1;
            } else {
                cdp = Some(DEFAULT_CDP_HTTP.to_owned());
            }
        } else {
            return Err(CliError::UnknownFlag(arg.clone()));
        }
        index += 1;
    }
    if fixture.is_none() && cdp.is_none() {
        return Err(CliError::MissingSource);
    }
    Ok(Source { fixture, cdp })
}

fn load_fixture(path: &str) -> Result<InteractionManifold, CliError> {
    let body = read_path(path)?;
    if body.trim_start().starts_with('{') {
        let transport =
            ReplayTransport::parse(&body).map_err(|err| CliError::Browser(err.to_string()))?;
        let mut session = BrowserSession::new(transport);
        session
            .observe()
            .cloned()
            .map_err(|err| CliError::Browser(err.to_string()))
    } else {
        parse_fixture(&body).map_err(|err| CliError::Fixture(err.to_string()))
    }
}

fn read_path(path: &str) -> Result<String, CliError> {
    fs::read_to_string(Path::new(path)).map_err(|err| CliError::Io {
        path: path.to_owned(),
        message: err.to_string(),
    })
}

fn connect_live(url: &str) -> Result<WebSocketTransport, CliError> {
    WebSocketTransport::connect(url).map_err(|err| CliError::Browser(err.to_string()))
}

fn render_regions(manifold: &InteractionManifold) -> String {
    let mut out = String::new();
    for region in manifold.regions() {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            region.id(),
            region.role(),
            region.label(),
            region.rect().x(),
            region.rect().y(),
            region.rect().width(),
            region.rect().height()
        ));
    }
    out
}
