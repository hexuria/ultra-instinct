//! Arm D offline: Instinct abstains → remote (scripted) chooses → same gate/ticket.
#![cfg(feature = "remote")]

use ultra_instinct_agent::{AgentBuilder, AgentOutcome, MockBrowser};
use ultra_instinct_core::{parse_fixture, Action, RegionId};
use ultra_instinct_policy::{
    EscalatingPolicy, InstinctPolicy, RemotePolicy, ScriptedRemote, UnconfiguredRemote,
};

const TWINS: &str = r#"
    viewport w=800 h=600
    region id=a role=button label="Delete" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
    region id=b role=button label="Delete" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
"#;

#[test]
fn escalation_executes_remote_choice_through_the_same_ticket_path() {
    let remote = RemotePolicy::new(ScriptedRemote::new([
        r#"{"choice":{"id":"CLICK:b","kind":"CLICK"}}"#,
        r#"{"choice":{"id":"DONE","kind":"DONE"}}"#,
    ]));
    let policy = EscalatingPolicy::new(InstinctPolicy::default(), Some(remote));
    let mut browser = MockBrowser::new(parse_fixture(TWINS).unwrap());
    browser.set_on_press(
        parse_fixture(
            r#"
            viewport w=800 h=600
            region id=undo role=button label="Undo" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
            "#,
        )
        .unwrap(),
    );
    let mut agent = AgentBuilder::new(browser, policy)
        .max_steps(4)
        .build("Delete");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(
        agent.browser_mut().press_log(),
        &[(RegionId::try_new("b").unwrap(), Action::Click)]
    );
}

#[test]
fn off_menu_remote_reply_fails_without_input() {
    let remote = RemotePolicy::new(ScriptedRemote::new([
        r##"{"choice":{"id":"CLICK:#delete-all","kind":"CLICK"}}"##,
    ]));
    let policy = EscalatingPolicy::new(InstinctPolicy::default(), Some(remote));
    let mut agent = AgentBuilder::new(MockBrowser::new(parse_fixture(TWINS).unwrap()), policy)
        .max_steps(3)
        .build("Delete");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Failed { .. }),
        "{outcome:?}"
    );
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn unconfigured_remote_is_plain_instinct() {
    let policy = EscalatingPolicy::new(InstinctPolicy::default(), Some(UnconfiguredRemote));
    let mut agent = AgentBuilder::new(MockBrowser::new(parse_fixture(TWINS).unwrap()), policy)
        .max_steps(3)
        .build("Delete");
    assert!(matches!(agent.run(), AgentOutcome::Abstained { .. }));
    assert!(agent.browser_mut().press_log().is_empty());
}
