//! `run_goal`: the agent loop (InstinctPolicy, deterministic text, no
//! model) for a host that only links ultra-instinct-mcp.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ultra_instinct_agent::MockBrowser;
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_core::parse_fixture;
use ultra_instinct_mcp::{
    run_goal, run_goal_on, BlockKind, Policy, RunLimits, RunOutcome, RunStep, Server,
};

const SEARCH: &str = r#"
    viewport w=800 h=600
    region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
    region id=go role=button label="Go" x=320 y=10 w=60 h=24 actions=click sources=dom,accessibility
"#;

const PAY: &str = r#"
    viewport w=800 h=600
    region id=pay role=button label="Pay $18" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
    region id=back role=button label="Back to cart" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
"#;

fn mock(fixture: &str) -> MockBrowser {
    MockBrowser::new(parse_fixture(fixture).unwrap())
}

fn limits(max_steps: u32) -> RunLimits {
    RunLimits {
        max_steps,
        ..RunLimits::default()
    }
}

#[test]
fn a_goal_runs_to_done_with_its_steps_and_effects() {
    let (report, browser) = run_goal_on(
        mock(SEARCH),
        r#"Type "rust ownership" into Search"#,
        Policy::default(),
        limits(5),
    );
    assert!(
        matches!(report.outcome, RunOutcome::Done { .. }),
        "{report:?}"
    );
    assert_eq!(
        report.steps,
        [RunStep {
            action: "TYPE_TEXT".into(),
            target_label: "Search".into(),
            effect: "success".into(),
        }]
    );
    assert_eq!(browser.input_log().len(), 1);
}

#[test]
fn a_press_that_needs_a_confirm_ends_blocked_with_its_label_unpressed() {
    let policy = Policy::from_lists("", "Pay*").unwrap();
    let (report, browser) = run_goal_on(mock(PAY), "click Pay $18", policy.clone(), limits(3));
    assert!(
        matches!(&report.outcome, RunOutcome::Blocked { label, reason, kind: Some(BlockKind::NeedsConfirm) }
            if label == "Pay $18" && reason.starts_with("needs-confirm")),
        "{report:?}"
    );
    assert_eq!(BlockKind::NeedsConfirm.code(), "needs_confirm");
    assert!(report.steps.is_empty());
    assert!(browser.press_log().is_empty());
    // The domain allow-list blocks the same way.
    let off = Policy::from_lists("shop.example", "").unwrap();
    let (report, browser) = run_goal_on(mock(PAY), "click Back to cart", off, limits(3));
    let RunOutcome::Blocked { label, kind, .. } = &report.outcome else {
        panic!("{report:?}");
    };
    assert_eq!(label, "Back to cart");
    let kind = kind.as_ref().expect("a policy refusal");
    assert_eq!(
        kind,
        &BlockKind::DomainNotAllowed {
            host: Some("mock.invalid".into())
        }
    );
    assert_eq!(kind.code(), "domain_not_allowed");
    assert!(browser.input_log().is_empty());
}

#[test]
fn no_trusted_action_abstains() {
    let (report, browser) = run_goal_on(
        mock(SEARCH),
        "type into Search",
        Policy::default(),
        limits(3),
    );
    assert!(
        matches!(report.outcome, RunOutcome::Abstained { .. }),
        "{report:?}"
    );
    assert!(browser.input_log().is_empty());
}

#[test]
fn the_step_cap_ends_the_run_failed() {
    let (report, _) = run_goal_on(
        mock(SEARCH),
        r#"Type "rust" into Search then click Go"#,
        Policy::default(),
        limits(1),
    );
    assert_eq!(
        report.outcome,
        RunOutcome::Failed {
            error: "max steps exceeded".into(),
            code: None,
        },
        "{report:?}"
    );
    assert_eq!(report.steps.len(), 1);
}

type Log = Rc<RefCell<Vec<String>>>;

struct Logged {
    inner: ReplayTransport,
    log: Log,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push(method.to_owned());
        self.inner.call(method, params_json)
    }
}

fn page() -> PageSpec {
    PageSpec::of(
        &[Control::button(
            11,
            101,
            "Sign in",
            (20.0, 80.0, 120.0, 28.0),
        )],
        "http://127.0.0.1/form",
        "Form",
    )
}

#[test]
fn a_passed_deadline_ends_failed_before_any_cdp_call() {
    let log: Log = Rc::default();
    let transport = Logged {
        inner: ReplayTransport::parse(&ScriptBuilder::new().observe_compact(&page()).to_json())
            .unwrap(),
        log: Rc::clone(&log),
    };
    let report = run_goal(
        transport,
        "click Sign in",
        Policy::default(),
        RunLimits {
            max_steps: 5,
            deadline: Some(Instant::now()),
        },
    );
    assert_eq!(
        report.outcome,
        RunOutcome::Failed {
            error: "deadline exceeded".into(),
            code: None,
        }
    );
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

/// `Server::run_goal` drives the current tab under the server's policy and
/// keeps the session for the next tool call.
#[test]
fn server_run_goal_uses_the_current_tab_and_the_server_policy() {
    let log: Log = Rc::default();
    let shared = Rc::clone(&log);
    let mut script = Some(
        (0..4)
            .fold(ScriptBuilder::new(), |s, _| s.observe_compact(&page()))
            .to_json(),
    );
    let mut server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script)?,
            log: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    server.set_policy(Policy::from_lists("", "sign*").unwrap());
    let report = server.run_goal(
        "click Sign in",
        RunLimits {
            max_steps: 3,
            deadline: Some(Instant::now() + Duration::from_secs(30)),
        },
    );
    assert!(
        matches!(report.outcome, RunOutcome::Blocked { ref label, kind: Some(BlockKind::NeedsConfirm), .. } if label == "Sign in"),
        "{report:?}"
    );
    assert!(log
        .borrow()
        .iter()
        .all(|m| m != "Runtime.callFunctionOn" && m != "Input.dispatchMouseEvent"));
    assert_eq!(
        server.live_sessions(),
        [ultra_instinct_cdp::DEFAULT_CDP_HTTP]
    );
}
