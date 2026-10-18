//! Live Chrome smoke for the owned loop. Ignored by default (not in CI).
//!
//! ```text
//! chrome --headless=new --remote-debugging-port=9222 --user-data-dir=/tmp/hu-smoke
//! ULTRA_INSTINCT_CDP=http://127.0.0.1:9222 cargo test -p ultra-instinct-agent --test live_smoke -- --ignored --nocapture
//! ```
//!
//! Opens an owned background tab, navigates to an inline `data:` page (no server), and
//! runs TYPE_TEXT → SELECT → CLICK → SCROLL goals through observe → Instinct →
//! gate → ticket → executor → verify, then checks the page DOM result.

use ultra_instinct_agent::{AgentBuilder, AgentOutcome, BrowserRuntime, VerificationKind};
use ultra_instinct_browser::{open_tab, BrowserSession, WebSocketTransport};
use ultra_instinct_core::{ActionKind, ActionSpace};
use ultra_instinct_policy::{
    AgentGoal, BrowserPolicy, HistoryEntry, InstinctPolicy, PolicyDecision, PolicyError,
    PolicyOutcome,
};

#[derive(Default)]
struct SelectThenDone;

impl BrowserPolicy for SelectThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = if history.is_empty() {
            space.targets_of(ActionKind::Select).next()
        } else {
            space.get_str(ActionKind::Done.as_str())
        };
        let Some(action) = action else {
            return Ok(PolicyOutcome::Abstain {
                reason: "select live action unavailable".into(),
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            });
        };
        Ok(PolicyOutcome::Choice(PolicyDecision {
            action_id: action.id().clone(),
            kind: action.kind(),
            target_label: action.label().to_owned(),
            confidence_millis: 1_000,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }))
    }
}

const PAGE: &str = "<!doctype html><title>HU live smoke</title><h1>Flight search</h1>\
<input id=q aria-label=Search style=width:300px>\
<select id=cabin aria-label='Cabin class'><option value=economy>Economy</option><option value=business>Business</option></select>\
<button id=go onclick=\"document.getElementById('out').textContent='Searched '+q.value+' in '+cabin.value\">Go</button>\
<p id=out></p><div style=height:3000px></div><button>Bottom</button>";

fn data_url() -> String {
    let mut out = String::from("data:text/html,");
    for b in PAGE.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn step(
    session: BrowserSession<WebSocketTransport>,
    goal: &str,
    kind: ActionKind,
    verification: VerificationKind,
) -> BrowserSession<WebSocketTransport> {
    let mut agent = AgentBuilder::new(session, InstinctPolicy::default())
        .max_steps(4)
        .build(goal);
    let outcome = agent.run();
    eprintln!("{goal}: {outcome:?}");
    assert!(
        matches!(outcome, AgentOutcome::Done { .. }),
        "{goal}: {outcome:?}"
    );
    let first = &outcome.steps()[0];
    assert_eq!(first.kind, kind, "{goal}");
    assert_eq!(first.verification, verification, "{goal}");
    agent.into_browser()
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn owned_loop_drives_type_select_click_scroll_on_live_chrome() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_owned());
    let mut session = BrowserSession::new(open_tab(&endpoint).unwrap());
    session.navigate(&data_url()).unwrap();
    session.settle();

    let session = step(
        session,
        r#"Type "rust ownership" into Search"#,
        ActionKind::TypeText,
        VerificationKind::Success,
    );
    let session = step(
        session,
        r#"Select "Business" in Cabin class"#,
        ActionKind::Select,
        VerificationKind::Success,
    );
    let mut session = step(
        session,
        "Click Go",
        ActionKind::Click,
        VerificationKind::StateChanged,
    );
    let m = BrowserRuntime::observe(&mut session).unwrap().clone();
    assert!(
        m.regions()
            .any(|r| r.label() == "Searched rust ownership in business"),
        "page result missing"
    );
    let _ = step(
        session,
        "scroll down",
        ActionKind::ScrollDown,
        VerificationKind::StateChanged,
    );
}

const HARDER: &str = r#"<!doctype html><title>HU harder</title>
<div id=host></div>
<script>
const host = document.getElementById('host');
const root = host.attachShadow({mode:'open'});
root.innerHTML = '<button aria-label="Shadow Ping">Shadow Ping</button>';
</script>
<input role=combobox aria-label=City id=city style=width:200px>
<div role=listbox aria-label=Suggestions>
  <div role=option aria-label=Manila>Manila</div>
</div>
<iframe id=frame srcdoc="<button aria-label='Frame Hi'>Frame Hi</button>"></iframe>
"#;

fn harder_data_url() -> String {
    let mut out = String::from("data:text/html,");
    for b in HARDER.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

const AUTOCOMPLETE: &str = r#"<!doctype html><title>HU autocomplete</title>
<input id=city role=combobox aria-label=City>
<div role=listbox aria-label=Country>
  <div role=option aria-label=Canada>Canada</div>
</div>
<div id=suggestions role=listbox aria-label=Suggestions></div>
<p id=out></p>
<script>
const city = document.getElementById('city');
city.addEventListener('input', () => setTimeout(() => {
  const option = document.createElement('div');
  option.setAttribute('role', 'option');
  option.setAttribute('aria-label', 'Manila');
  option.textContent = 'Manila';
  option.addEventListener('click', () => {
    city.value = 'Manila';
    option.setAttribute('aria-selected', 'true');
    document.getElementById('out').textContent = 'picked';
  });
  document.getElementById('suggestions').replaceChildren(option);
}, 250));
</script>
"#;

fn autocomplete_data_url(with_controls: bool) -> String {
    let html = if with_controls {
        AUTOCOMPLETE.replace(
            "role=combobox aria-label=City",
            "role=combobox aria-label=City aria-controls=suggestions",
        )
    } else {
        AUTOCOMPLETE.to_owned()
    };
    let mut out = String::from("data:text/html,");
    for b in html.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn harder_page_types_observe_shadow_iframe_combobox_on_live_chrome() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_owned());
    let mut session = BrowserSession::new(open_tab(&endpoint).unwrap());
    session.navigate(&harder_data_url()).unwrap();
    session.settle();
    let m = BrowserRuntime::observe(&mut session).unwrap().clone();
    let labels: Vec<_> = m.regions().map(|r| r.label().to_owned()).collect();
    eprintln!("labels: {labels:?}");
    assert!(
        labels.iter().any(|l| l == "Shadow Ping"),
        "open shadow button missing: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "City"),
        "combobox missing: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Manila"),
        "option missing: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Frame Hi"),
        "same-origin iframe button missing: {labels:?}"
    );
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn autocomplete_waits_for_delayed_option_on_live_chrome() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_owned());
    let mut session = BrowserSession::new(WebSocketTransport::connect(&endpoint).unwrap());
    session.navigate(&autocomplete_data_url(true)).unwrap();
    session.settle();

    let mut agent = AgentBuilder::new(session, InstinctPolicy::default())
        .max_steps(4)
        .build(r#"Type "man" into City then click Manila"#);
    let outcome = agent.run();
    eprintln!("delayed autocomplete: {outcome:?}");
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].kind, ActionKind::TypeText);
    assert_eq!(steps[1].kind, ActionKind::Click);
    assert_eq!(steps[1].label, "Manila");
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn autocomplete_ignores_unrelated_visible_options_on_live_chrome() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_owned());
    for with_controls in [true, false] {
        let mut session = BrowserSession::new(WebSocketTransport::connect(&endpoint).unwrap());
        session
            .navigate(&autocomplete_data_url(with_controls))
            .unwrap();
        session.settle();

        let mut agent = AgentBuilder::new(session, InstinctPolicy::default())
            .max_steps(4)
            .build(r#"Type "man" into City then click Manila"#);
        let outcome = agent.run();
        eprintln!("unrelated options with aria-controls={with_controls}: {outcome:?}");
        assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
        let steps = outcome.steps();
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert_eq!(steps[0].kind, ActionKind::TypeText);
        assert_eq!(steps[1].kind, ActionKind::Click);
        assert_eq!(steps[1].label, "Manila");
    }
}

const ELEMENT_STATE_PAGE: &str = r#"<!doctype html><title>HU element state</title>
<label>Name <input aria-label=Name value=Ana></label>
<label>Subscribe <input type=checkbox aria-label=Subscribe checked></label>
<label>Time zone <select aria-label="Time zone">
  <option>UTC</option><option>Asia/Manila</option><option disabled>Mars</option>
</select></label>
<label>Password <input type=password aria-label=Password value=hunter2></label>"#;

fn element_state_data_url() -> String {
    let mut out = String::from("data:text/html,");
    for b in ELEMENT_STATE_PAGE.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn live_observe_captures_control_state_without_password_values() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_owned());
    let mut session = BrowserSession::new(WebSocketTransport::connect(&endpoint).unwrap());
    session.navigate(&element_state_data_url()).unwrap();
    session.settle();

    let manifold = session.observe().unwrap().clone();
    let space = ActionSpace::from_manifold(&manifold);
    let name = space
        .targets_of(ActionKind::TypeText)
        .find(|action| action.label() == "Name")
        .unwrap();
    assert_eq!(name.state().value.as_deref(), Some("Ana"));
    let subscribe = space
        .targets_of(ActionKind::Click)
        .find(|action| action.label() == "Subscribe")
        .unwrap();
    assert_eq!(subscribe.state().checked, Some(true));
    let time_zone = space
        .targets_of(ActionKind::Select)
        .find(|action| action.label() == "Time zone")
        .unwrap();
    assert_eq!(
        time_zone.state().options,
        vec!["UTC".to_owned(), "Asia/Manila".to_owned()]
    );
    assert_eq!(time_zone.state().selected.as_deref(), Some("UTC"));
    let password = space
        .targets_of(ActionKind::TypeText)
        .find(|action| action.label() == "Password")
        .unwrap();
    assert!(password.state().value.is_none());
    for action in space.actions() {
        let state = action.state();
        assert!(state
            .value
            .iter()
            .chain(state.selected.iter())
            .chain(state.options.iter())
            .all(|value| !value.contains("hunter2")));
    }
}

const SELECT_GROUNDING_PAGE: &str = r#"<!doctype html><title>HU SELECT grounding</title>
<label>Time zone <select aria-label="Time zone">
  <option>UTC</option><option>Asia/Manila</option>
</select></label>"#;

fn select_grounding_data_url() -> String {
    let mut out = String::from("data:text/html,");
    for b in SELECT_GROUNDING_PAGE.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn live_select_uses_an_observed_option_for_an_unquoted_goal() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_owned());
    let mut session = BrowserSession::new(WebSocketTransport::connect(&endpoint).unwrap());
    session.navigate(&select_grounding_data_url()).unwrap();
    session.settle();

    let mut agent = AgentBuilder::new(session, SelectThenDone)
        .max_steps(2)
        .build("Set the time zone to Asia/Manila");
    let outcome = agent.run();
    eprintln!("observed-option SELECT: {outcome:?}");
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps().len(), 1, "{outcome:?}");
    assert_eq!(outcome.steps()[0].kind, ActionKind::Select);
    assert_eq!(outcome.steps()[0].verification, VerificationKind::Success);

    let region_id = BrowserRuntime::observe(agent.browser_mut())
        .unwrap()
        .regions()
        .find(|region| region.label() == "Time zone")
        .unwrap()
        .id()
        .clone();
    let actual = agent
        .browser_mut()
        .field_value(&region_id)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(actual, "Asia/Manila");
}
