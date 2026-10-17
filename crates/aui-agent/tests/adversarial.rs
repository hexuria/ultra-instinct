//! Offline adversarial e2e for the owned loop (MockBrowser).
//!
//! Every test runs goal → observe → Instinct → gate → ticket → executor → verify
//! and asserts what was (not) dispatched to the page.

use aui_agent::{
    AgentBuilder, AgentError, AgentOutcome, AgentState, BrowserRuntime, Input, MockBrowser,
    VerificationKind,
};
use aui_browser::{PageState, ScrollDirection};
use aui_core::{parse_fixture, ActionId, ActionKind, ActionSpace, InteractionManifold, RegionId};
use aui_policy::{
    AgentGoal, BrowserPolicy, HistoryEntry, InstinctPolicy, PolicyDecision, PolicyError,
    PolicyOutcome,
};

fn m(src: &str) -> InteractionManifold {
    parse_fixture(src).unwrap()
}

fn id(raw: &str) -> RegionId {
    RegionId::try_new(raw).unwrap()
}

const SEARCH: &str = r#"
    viewport w=800 h=600
    region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
    region id=go role=button label="Go" x=320 y=10 w=60 h=24 actions=click sources=dom,accessibility
"#;

const CABIN: &str = r#"
    viewport w=800 h=600
    region id=cabin role=generic label="Cabin class" x=10 y=10 w=200 h=24 actions=click,select,focus sources=dom,accessibility
    region id=find role=button label="Find flights" x=10 y=50 w=120 h=24 actions=click sources=dom,accessibility
"#;

#[test]
fn type_text_executes_resolved_text_verifies_then_done() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(5)
        .build(r#"Type "rust ownership" into Search"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert_eq!(steps[0].kind, ActionKind::TypeText);
    assert_eq!(steps[0].verification, VerificationKind::Success);
    assert_eq!(
        agent.browser_mut().input_log(),
        &[(id("q"), Input::Type("rust ownership".into()))]
    );
}

#[test]
fn select_option_executes_and_verifies() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(CABIN)), InstinctPolicy::default())
        .max_steps(5)
        .build(r#"Select "Business" in Cabin class"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps()[0].kind, ActionKind::Select);
    assert_eq!(outcome.steps()[0].verification, VerificationKind::Success);
    assert_eq!(
        agent.browser_mut().input_log(),
        &[(id("cabin"), Input::Select("Business".into()))]
    );
}

#[test]
fn type_text_without_resolvable_value_never_types() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(3)
        .build("type into Search");
    let outcome = agent.run();
    // No value: TYPE_TEXT is withdrawn and the re-plan abstains (issue #70).
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    assert!(agent.browser_mut().input_log().is_empty());
}

#[test]
fn wrong_effect_is_detected_and_bounded() {
    let mut browser = MockBrowser::new(m(SEARCH));
    browser.override_value("rus"); // page mangles input (e.g. maxlength)
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(10)
        .build(r#"Type "rust" into Search"#);
    let outcome = agent.run();
    match &outcome {
        // Habituation (ADR 0008) freezes the repeated wrong-effect TYPE_TEXT
        // after 2 trailing failures (urge 500 < standard min 750): Abstained,
        // not the old 3-strike Blocked. Still bounded, nothing else typed.
        AgentOutcome::Abstained { steps, .. } => {
            assert_eq!(steps.len(), 2, "{steps:?}");
            assert!(steps
                .iter()
                .all(|s| s.verification == VerificationKind::WrongEffect));
        }
        other => panic!("expected bounded Abstained, got {other:?}"),
    }
    assert_eq!(agent.browser_mut().input_log().len(), 2);
}

#[test]
fn page_change_during_text_resolution_is_stale_and_types_nothing() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(3)
        .build(r#"Type "rust" into Search"#);
    let predicted = agent.predict().unwrap().expect("type prediction").clone();
    assert_eq!(predicted.payload.as_deref(), Some("rust"));
    // Field rerendered (new label) while the resolver was "thinking".
    agent
        .browser_mut()
        .schedule_swap(0, m(&SEARCH.replace("\"Search\"", "\"Search users\"")));
    let err = agent.act().unwrap_err();
    assert!(matches!(err, AgentError::Stale(_)), "{err}");
    assert_eq!(agent.state(), AgentState::Ready);
    assert!(agent.browser_mut().input_log().is_empty());
    assert_eq!(agent.stale_discards(), 1);
}

const PROJECT: &str = r#"
    viewport w=1440 h=900
    region id=page-delete role=button label="Delete project" x=1200 y=780 w=160 h=36 actions=click sources=dom,accessibility
    region id=page-cancel role=button label="Cancel" x=1060 y=780 w=100 h=36 actions=click sources=dom,accessibility
"#;

const PROJECT_MODAL: &str = r#"
    viewport w=1440 h=900
    region id=page-delete role=button label="Delete project" x=1200 y=780 w=160 h=36 actions=click sources=dom,accessibility
    region id=page-cancel role=button label="Cancel" x=1060 y=780 w=100 h=36 actions=click sources=dom,accessibility
    region id=confirm role=dialog label="Delete project?" x=520 y=300 w=400 h=240 actions=focus sources=dom,accessibility flags=modal
    region id=confirm-cancel role=button label="Cancel" x=560 y=480 w=100 h=36 actions=click parent=confirm sources=dom,accessibility
    region id=confirm-delete role=button label="Delete" x=780 y=480 w=100 h=36 actions=click parent=confirm sources=dom,accessibility
"#;

#[test]
fn modal_appearing_after_prediction_discards_and_never_clicks_background() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(PROJECT)), InstinctPolicy::default())
        .max_steps(3)
        .build("Delete project");
    let p = agent.predict().unwrap().expect("prediction").clone();
    assert_eq!(p.decision.action_id.as_str(), "CLICK:page-delete");
    agent.browser_mut().replace_manifold(m(PROJECT_MODAL));
    let err = agent.act().unwrap_err();
    assert!(matches!(err, AgentError::Stale(_)), "{err}");
    // Keep running on the modal page: background is never offered or pressed.
    let _ = agent.run();
    assert!(agent
        .browser_mut()
        .press_log()
        .iter()
        .all(|(target, _)| target.as_str() != "page-delete" && target.as_str() != "page-cancel"));
}

#[test]
fn background_behind_open_modal_is_not_in_action_space() {
    let space = aui_agent::Agent::<MockBrowser, InstinctPolicy>::action_space(&m(PROJECT_MODAL));
    assert!(space.get_str("CLICK:page-delete").is_none());
    assert!(space.get_str("CLICK:page-cancel").is_none());
    assert!(space.get_str("CLICK:confirm-delete").is_some());
    // Raw ActionSpace (no front layer) would have offered it.
    assert!(ActionSpace::from_manifold(&m(PROJECT_MODAL))
        .get_str("CLICK:page-delete")
        .is_some());
}

#[test]
fn covered_disabled_hidden_offscreen_targets_never_execute() {
    let page = m(r#"
        viewport w=800 h=600
        region id=a role=button label="Save" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility flags=occluded
        region id=b role=button label="Save" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility flags=disabled
        region id=c role=button label="Save" x=10 y=90 w=80 h=24 actions=click sources=dom,accessibility flags=hidden
        region id=d role=button label="Save" x=10 y=900 w=80 h=24 actions=click sources=dom,accessibility flags=offscreen
        region id=x role=button label="Help" x=10 y=130 w=80 h=24 actions=click sources=dom,accessibility
    "#);
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(3)
        .build("Save");
    let _ = agent.run();
    assert!(agent
        .browser_mut()
        .press_log()
        .iter()
        .all(|(t, _)| !["a", "b", "c", "d"].contains(&t.as_str())));
}

#[test]
fn twin_labels_in_different_rows_abstain_without_pressing() {
    let page = m(r#"
        viewport w=800 h=600
        region id=row1 role=generic label="Invoice 1001" x=0 y=0 w=800 h=40 actions=focus sources=dom
        region id=del1 role=button label="Delete" x=700 y=8 w=60 h=24 actions=click parent=row1 sources=dom,accessibility
        region id=row2 role=generic label="Invoice 1002" x=0 y=40 w=800 h=40 actions=focus sources=dom
        region id=del2 role=button label="Delete" x=700 y=48 w=60 h=24 actions=click parent=row2 sources=dom,accessibility
    "#);
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(3)
        .build("Delete");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn rerender_replacing_node_between_predict_and_act_is_stale() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(3)
        .build("Go");
    agent.predict().unwrap().expect("prediction");
    // Same label, new node id (framework re-mounted the button).
    agent
        .browser_mut()
        .replace_manifold(m(&SEARCH.replace("id=go ", "id=go2 ")));
    let err = agent.act().unwrap_err();
    assert!(matches!(err, AgentError::Stale(_)), "{err}");
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn focus_change_between_predict_and_act_is_stale() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(3)
        .build("Go");
    agent.predict().unwrap().expect("prediction");
    agent.browser_mut().set_focused(Some(id("q")));
    let err = agent.act().unwrap_err();
    assert!(matches!(err, AgentError::Stale(_)), "{err}");
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn page_rejecting_input_is_reported_not_retried_with_same_ticket() {
    let mut browser = MockBrowser::new(m(SEARCH));
    browser.reject_next("readonly");
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build(r#"Type "x" into Search"#);
    agent.predict().unwrap();
    let err = agent.act().unwrap_err();
    assert!(matches!(err, AgentError::InputRejected(_)), "{err}");
    assert!(agent.browser_mut().input_log().is_empty());
}

#[test]
fn scroll_and_wait_are_page_primitives_without_tickets() {
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(3)
        .build("scroll down");
    let p = agent.predict().unwrap().expect("scroll").clone();
    assert_eq!(p.decision.kind, ActionKind::ScrollDown);
    let rec = agent.act().unwrap();
    assert_eq!(rec.kind, ActionKind::ScrollDown);
    assert_eq!(rec.verification, VerificationKind::NoVisibleChange);
    assert_eq!(agent.browser_mut().scroll_log(), &[ScrollDirection::Down]);
    assert!(agent.browser_mut().press_log().is_empty());

    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(3)
        .build("wait");
    agent.predict().unwrap();
    let rec = agent.act().unwrap();
    assert_eq!(rec.kind, ActionKind::Wait);
    assert!(agent.browser_mut().press_log().is_empty());
}

/// Remote-shaped policy that returns an off-menu target.
struct OffMenu(&'static str, ActionKind);

impl BrowserPolicy for OffMenu {
    fn decide(
        &mut self,
        _space: &ActionSpace,
        _goal: &AgentGoal,
        _history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        Ok(PolicyOutcome::Choice(PolicyDecision {
            action_id: ActionId::try_new(self.0).unwrap(),
            kind: self.1,
            target_label: "x".into(),
            confidence_millis: 1000,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }))
    }
}

#[test]
fn off_menu_policy_choice_is_a_hard_error_and_never_executes() {
    for (raw, kind) in [
        ("CLICK:ghost", ActionKind::Click),
        ("#go > button", ActionKind::Click),
        ("CLICK:go", ActionKind::TypeText), // kind/action mismatch
    ] {
        let Ok(_) = ActionId::try_new(raw) else {
            continue;
        };
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), OffMenu(raw, kind))
            .max_steps(3)
            .build("whatever");
        let outcome = agent.run();
        match outcome {
            AgentOutcome::Failed { error, .. } => assert!(error.contains("policy"), "{error}"),
            other => panic!("{raw}: expected Failed, got {other:?}"),
        }
        assert!(agent.browser_mut().press_log().is_empty(), "{raw}");
    }
}

/// Runtime whose world flips on every observation (never settles).
struct Flicker {
    a: MockBrowser,
    b: MockBrowser,
    n: u32,
    dispatched: u32,
}

impl BrowserRuntime for Flicker {
    fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        self.n += 1;
        if self.n % 2 == 1 {
            self.a.observe()
        } else {
            self.b.observe()
        }
    }
    fn focused(&self) -> Option<RegionId> {
        None
    }
    fn page(&self) -> Option<&PageState> {
        None
    }
    fn dispatch(&mut self, _target: &RegionId, _input: &Input) -> Result<(), AgentError> {
        self.dispatched += 1;
        Ok(())
    }
    fn scroll(&mut self, _direction: ScrollDirection) -> Result<(), AgentError> {
        Ok(())
    }
    fn last_observation(&self) -> Option<&InteractionManifold> {
        // The last `observe` incremented `n`, so the odd/even mock that just
        // served owns the cached manifold.
        if self.n % 2 == 1 {
            self.a.last_observation()
        } else {
            self.b.last_observation()
        }
    }
}

#[test]
fn never_settling_page_hits_stale_bound_without_input() {
    let flicker = Flicker {
        a: MockBrowser::new(m(SEARCH)),
        b: MockBrowser::new(m(&format!(
            "{SEARCH}\n    region id=toast role=button label=\"Dismiss\" x=400 y=10 w=80 h=24 actions=click sources=dom,accessibility\n"
        ))),
        n: 0,
        dispatched: 0,
    };
    let mut agent = AgentBuilder::new(flicker, InstinctPolicy::default())
        .max_steps(20)
        .max_consecutive_stale(4)
        .build("Go");
    let outcome = agent.run();
    match outcome {
        AgentOutcome::Failed { error, .. } => assert!(error.contains("stale"), "{error}"),
        other => panic!("expected stale bound, got {other:?}"),
    }
    assert_eq!(agent.browser_mut().dispatched, 0);
    assert_eq!(agent.stale_discards(), 4);
}

#[test]
fn repeated_no_effect_click_is_bounded() {
    // Clicking changes nothing; policy would repeat. Habituation (ADR 0008)
    // freezes the repeat after 2 presses → Abstained before the old 3-strike
    // bound; the abstain then feeds the escalation tier when configured.
    let page = m(r#"
        viewport w=800 h=600
        region id=go role=button label="Refresh" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
    "#);
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(10)
        .build("Refresh");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    assert_eq!(agent.browser_mut().press_log().len(), 2);
}

#[test]
fn multi_step_type_then_click_runs_both_clauses() {
    use aui_agent::TickResult;
    // Keep Go on the page; add a result so the click is state-changed. Removing
    // Go would make the post-click predict abstain before Instinct can choose DONE.
    let after_go = m(r#"
        viewport w=800 h=600
        region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
        region id=go role=button label="Go" x=320 y=10 w=60 h=24 actions=click sources=dom,accessibility
        region id=result role=text label="ok" x=10 y=50 w=100 h=20 actions=focus sources=dom,accessibility
        "#);
    let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(10)
        .build(r#"Type "rust" into Search then click Go"#);
    assert_eq!(agent.clauses().len(), 2);
    let mut outcome = None;
    for _ in 0..20 {
        match agent.tick() {
            Ok(TickResult::Stepped(step)) if step.kind == ActionKind::TypeText => {
                agent.browser_mut().set_on_press(after_go.clone());
            }
            Ok(TickResult::Stepped(_)) | Ok(TickResult::StaleDiscarded { .. }) => {}
            Ok(TickResult::ClauseAdvanced { .. }) | Ok(TickResult::Rethink) => {}
            Ok(TickResult::Finished(o)) => {
                outcome = Some(o);
                break;
            }
            Err(e) => panic!("tick failed: {e}"),
        }
    }
    let outcome = outcome.expect("agent did not finish");
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].kind, ActionKind::TypeText);
    assert_eq!(steps[0].verification, VerificationKind::Success);
    assert_eq!(steps[1].kind, ActionKind::Click);
    assert_eq!(steps[1].label, "Go");
    assert_eq!(
        agent.browser_mut().input_log(),
        &[
            (id("q"), Input::Type("rust".into())),
            (id("go"), Input::Click),
        ]
    );
}

#[test]
fn readonly_flipped_after_predict_refuses_at_executor_gate() {
    let editable = m(r#"
        viewport w=800 h=600
        region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
        "#);
    let locked = m(r#"
        viewport w=800 h=600
        region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility flags=readonly
        "#);
    let mut agent = AgentBuilder::new(MockBrowser::new(editable), InstinctPolicy::default())
        .max_steps(3)
        .build(r#"Type "x" into Search"#);
    assert!(agent.predict().unwrap().is_some());
    // Between predict and act the field becomes readonly; executor fresh
    // observe + hard gate must refuse before any CDP/mock input.
    agent.browser_mut().replace_manifold(locked);
    let err = agent.act().unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("readonly") || msg.contains("refuse") || msg.contains("stale"),
        "{msg}"
    );
    assert!(agent.browser_mut().input_log().is_empty());
}

#[test]
fn autocomplete_type_then_click_option_with_ticket_revalidate() {
    use aui_agent::TickResult;
    let before = m(r#"
        viewport w=800 h=600
        region id=city role=combobox label="City" x=10 y=10 w=240 h=28 actions=click,type,select,focus sources=dom,accessibility
        "#);
    // After typing, a suggestion list appears (visible window only).
    let after_type = m(r#"
        viewport w=800 h=600
        region id=city role=combobox label="City" x=10 y=10 w=240 h=28 actions=click,type,select,focus sources=dom,accessibility
        region id=list role=listbox label="Suggestions" x=10 y=40 w=240 h=120 actions=focus sources=dom,accessibility
        region id=opt1 role=option label="Manila" x=10 y=40 w=240 h=28 actions=click,select,focus sources=dom,accessibility parent=list
        region id=opt2 role=option label="Cebu" x=10 y=70 w=240 h=28 actions=click,select,focus sources=dom,accessibility parent=list
        "#);
    let after_click = m(r#"
        viewport w=800 h=600
        region id=city role=combobox label="City" x=10 y=10 w=240 h=28 actions=click,type,select,focus sources=dom,accessibility
        region id=opt1 role=option label="Manila" x=10 y=40 w=240 h=28 actions=click,select,focus sources=dom,accessibility
        region id=picked role=text label="picked Manila" x=10 y=80 w=200 h=20 actions=focus sources=dom,accessibility
        "#);
    let mut browser = MockBrowser::new(before);
    // Typing opens the suggestion popup (next dispatch swaps the manifold).
    browser.set_on_press(after_type);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(10)
        .build(r#"Type "man" into City then click Manila"#);
    assert_eq!(agent.clauses().len(), 2);
    let mut outcome = None;
    for _ in 0..20 {
        match agent.tick() {
            Ok(TickResult::Stepped(step)) if step.kind == ActionKind::TypeText => {
                // The option click (ticketed, revalidated on fresh observe)
                // closes the popup.
                agent.browser_mut().set_on_press(after_click.clone());
            }
            Ok(TickResult::Stepped(_))
            | Ok(TickResult::StaleDiscarded { .. })
            | Ok(TickResult::ClauseAdvanced { .. })
            | Ok(TickResult::Rethink) => {}
            Ok(TickResult::Finished(o)) => {
                outcome = Some(o);
                break;
            }
            Err(e) => panic!("tick failed: {e}"),
        }
    }
    let outcome = outcome.expect("agent did not finish");
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].kind, ActionKind::TypeText);
    assert_eq!(steps[1].kind, ActionKind::Click);
    assert_eq!(steps[1].label, "Manila");
    assert_eq!(
        agent.browser_mut().input_log(),
        &[
            (id("city"), Input::Type("man".into())),
            (id("opt1"), Input::Click),
        ]
    );
}

#[test]
fn virtualized_list_scroll_then_click_newly_visible_row() {
    use aui_agent::TickResult;
    // Visible window only: Item 1..3 on screen. Item 50 is off-window (absent).
    let window_a = m(r#"
        viewport w=800 h=600
        region id=list role=listbox label="Rows" x=0 y=0 w=800 h=200 actions=focus sources=dom,accessibility
        region id=r1 role=option label="Alpha top" x=0 y=0 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        region id=r2 role=option label="Bravo mid" x=0 y=40 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        region id=r3 role=option label="Charlie low" x=0 y=80 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        "#);
    // After scroll, the recycler swaps the visible window (new ids / labels).
    let window_b = m(r#"
        viewport w=800 h=600
        region id=list role=listbox label="Rows" x=0 y=0 w=800 h=200 actions=focus sources=dom,accessibility
        region id=r48 role=option label="Xray far" x=0 y=0 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        region id=r49 role=option label="Yankee near" x=0 y=40 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        region id=r50 role=option label="Zebra target" x=0 y=80 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        "#);
    let after_click = m(r#"
        viewport w=800 h=600
        region id=list role=listbox label="Rows" x=0 y=0 w=800 h=200 actions=focus sources=dom,accessibility
        region id=r50 role=option label="Zebra target" x=0 y=80 w=800 h=40 actions=click,focus sources=dom,accessibility parent=list
        region id=picked role=text label="opened zebra" x=0 y=220 w=200 h=20 actions=focus sources=dom,accessibility
        "#);
    let mut browser = MockBrowser::new(window_a);
    // Predict observes window A once; the post-scroll re-observe sees the
    // recycled window B (Item 50 did not exist as a region before).
    browser.schedule_swap(1, window_b);
    browser.set_on_press(after_click);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(10)
        .build(r#"scroll down then click Zebra target"#);
    assert_eq!(agent.clauses().len(), 2);
    let mut outcome = None;
    for _ in 0..20 {
        match agent.tick() {
            Ok(TickResult::Stepped(_))
            | Ok(TickResult::StaleDiscarded { .. })
            | Ok(TickResult::ClauseAdvanced { .. })
            | Ok(TickResult::Rethink) => {}
            Ok(TickResult::Finished(o)) => {
                outcome = Some(o);
                break;
            }
            Err(e) => panic!("tick failed: {e}"),
        }
    }
    let outcome = outcome.expect("agent did not finish");
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert!(
        steps.iter().any(|s| s.kind == ActionKind::ScrollDown),
        "{steps:?}"
    );
    assert!(
        steps
            .iter()
            .any(|s| s.kind == ActionKind::Click && s.label == "Zebra target"),
        "{steps:?}"
    );
    assert_eq!(
        agent.browser_mut().input_log().last(),
        Some(&(id("r50"), Input::Click))
    );
}
