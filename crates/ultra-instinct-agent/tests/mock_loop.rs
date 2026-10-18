//! Offline e2e: goal → observe → Instinct → guard → ticket → press → verify.

use ultra_instinct_agent::{AgentBuilder, AgentOutcome, MockBrowser, TickResult, VerificationKind};
use ultra_instinct_core::{
    parse_fixture, Action, ActionKind, ActionSpace, ElementState, InteractionManifold,
};
use ultra_instinct_policy::{
    AgentGoal, BrowserPolicy, DecisionEvidence, EvidenceWants, HistoryEntry, InstinctPolicy,
    PolicyDecision, PolicyError, PolicyOutcome, Screenshot,
};

fn manifold(src: &str) -> InteractionManifold {
    parse_fixture(src).unwrap()
}

#[test]
fn clicks_exact_label_and_sees_state_change() {
    let before = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=no role=button label="Cancel" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let after = manifold(
        r#"
        viewport w=800 h=600
        region id=done role=button label="Finish" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser = MockBrowser::new(before);
    browser.set_on_press(after);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("Continue");
    let outcome = agent.run();
    // First step clicks Continue; page changes. Next predict may abstain or pick Finish.
    // We at least require a stepped history with a click.
    assert!(
        !outcome.steps().is_empty()
            || matches!(
                outcome,
                AgentOutcome::Abstained { .. } | AgentOutcome::Done { .. }
            ),
        "{outcome:?}"
    );
    let steps = outcome.steps();
    if let Some(first) = steps.first() {
        assert_eq!(first.label, "Continue");
        assert_eq!(first.kind, ultra_instinct_core::ActionKind::Click);
        assert!(
            matches!(
                first.verification,
                VerificationKind::StateChanged
                    | VerificationKind::NoVisibleChange
                    | VerificationKind::Success
            ),
            "{:?}",
            first.verification
        );
    }
    assert_eq!(
        agent
            .browser_mut()
            .press_log()
            .first()
            .map(|(id, a)| (id.as_str(), *a)),
        Some(("go", Action::Click))
    );
}

#[test]
fn twin_delete_abstains_without_pressing() {
    let m = manifold(
        r#"
        viewport w=800 h=600
        region id=a role=button label="Delete" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=b role=button label="Delete" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let browser = MockBrowser::new(m);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("Delete");
    let outcome = agent.run();
    assert!(
        matches!(&outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn stale_world_discards_prediction_without_failing_task_permanently() {
    let before = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="UniqueGo" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let browser = MockBrowser::new(before);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(4)
        .build("UniqueGo");
    // Predict successfully.
    let pred = agent.predict().unwrap();
    assert!(pred.is_some());
    // Mutate world before act so ticket/world fingerprint fails.
    let mutated = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="UniqueGo" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=extra role=button label="Notify" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    agent.browser_mut().replace_manifold(mutated);
    let err = agent.act().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("stale") || msg.contains("world"), "{msg}");
    // Ready again — not terminal failure.
    assert_eq!(agent.state(), ultra_instinct_agent::AgentState::Ready);
}

#[test]
fn done_goal_terminates_without_press() {
    let m = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let browser = MockBrowser::new(m);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(2)
        .build("DONE");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn tick_stale_is_not_finished_failure() {
    let before = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="UniqueGo" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let browser = MockBrowser::new(before);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("UniqueGo");
    agent.predict().unwrap();
    let mutated = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="UniqueGo" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=x role=button label="X" x=10 y=40 w=40 h=20 actions=click sources=dom,accessibility
        "#,
    );
    agent.browser_mut().replace_manifold(mutated);
    // Force state Predicted still — act via tick
    // After predict, state is Predicted; mutate; tick will act.
    match agent.tick() {
        Ok(TickResult::StaleDiscarded { .. }) => {}
        Ok(other) => panic!("expected stale discarded, got {other:?}"),
        Err(e) => panic!("unexpected err {e}"),
    }
}

#[test]
fn repeated_no_effect_click_abstains_before_max_steps() {
    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser = MockBrowser::new(page.clone());
    browser.set_on_press(page);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(10)
        .build("Continue");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    assert_eq!(outcome.steps().len(), 2);
    assert!(outcome
        .steps()
        .iter()
        .all(|step| step.verification == VerificationKind::NoVisibleChange));
    assert_eq!(agent.browser_mut().press_log().len(), 2);
    assert!(outcome.steps().len() < 10);
}

#[derive(Default)]
struct FieldsThenDone;

impl BrowserPolicy for FieldsThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = match history.len() {
            0 => space
                .targets_of(ActionKind::TypeText)
                .find(|action| action.label() == "Full name"),
            1 => space
                .targets_of(ActionKind::TypeText)
                .find(|action| action.label() == "Street address"),
            _ => space.get_str(ActionKind::Done.as_str()),
        };
        let Some(action) = action else {
            return Ok(PolicyOutcome::Abstain {
                reason: "expected typing action unavailable".into(),
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

#[test]
fn deterministic_text_resolver_selects_literals_for_each_typed_field() {
    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=name role=text_field label="Full name" x=10 y=10 w=200 h=24 actions=type sources=dom
        region id=street role=text_field label="Street address" x=10 y=50 w=200 h=24 actions=type sources=dom
        "#,
    );
    let mut agent = AgentBuilder::new(MockBrowser::new(page), FieldsThenDone)
        .max_steps(3)
        .build(r#"Ship to "Ana Santos", street "12 Mabini St""#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(
        outcome
            .steps()
            .iter()
            .map(|step| step.payload.as_deref())
            .collect::<Vec<_>>(),
        [Some("Ana Santos"), Some("12 Mabini St")]
    );
}

#[derive(Default)]
struct WrongEffectThenDone;

impl BrowserPolicy for WrongEffectThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = if history.len() < 2 {
            space
                .targets_of(ActionKind::TypeText)
                .find(|action| action.label() == "Search")
        } else {
            space.get_str(ActionKind::Done.as_str())
        };
        let Some(action) = action else {
            return Ok(PolicyOutcome::Abstain {
                reason: "expected action unavailable".into(),
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

#[test]
fn wrong_effect_type_text_is_not_counted_as_successfully_typed() {
    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=search role=text_field label="Search" x=10 y=10 w=200 h=24 actions=type sources=dom
        "#,
    );
    let mut browser = MockBrowser::new(page);
    browser.override_value("rus");
    let mut agent = AgentBuilder::new(browser, WrongEffectThenDone)
        .max_steps(3)
        .build(r#"Type "rust" into Search"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps().len(), 2);
    assert!(outcome.steps().iter().all(|step| {
        step.kind == ActionKind::TypeText
            && step.verification == VerificationKind::WrongEffect
            && step.payload.as_deref() == Some("rust")
    }));
    assert_eq!(agent.browser_mut().input_log().len(), 2);
}

/// Compose opens a modal: the front layer buries the Compose button, so the
/// clause's own target leaves the action space after a verified effect.
fn compose_pages() -> (InteractionManifold, InteractionManifold) {
    let inbox = manifold(
        r#"
        viewport w=800 h=600
        region id=compose role=button label="Compose" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=row role=link label="Weekly report" x=10 y=60 w=400 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let draft = manifold(
        r#"
        viewport w=800 h=600
        region id=compose role=button label="Compose" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=row role=link label="Weekly report" x=10 y=60 w=400 h=24 actions=click sources=dom,accessibility
        region id=dlg role=dialog label="New Message" x=0 y=0 w=800 h=600 actions=focus sources=dom,accessibility flags=modal
        region id=send role=button label="Send" x=600 y=550 w=80 h=24 actions=click sources=dom,accessibility parent=dlg
        "#,
    );
    (inbox, draft)
}

#[test]
fn clause_with_verified_effect_advances_when_its_target_leaves_the_space() {
    let (inbox, draft) = compose_pages();
    let mut browser = MockBrowser::new(inbox);
    browser.set_on_press(draft);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("click Compose then click Send");
    agent.run();
    let pressed: Vec<&str> = agent
        .browser_mut()
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        pressed.get(..2),
        Some(&["compose", "send"][..]),
        "{pressed:?}"
    );
}

#[test]
fn last_clause_with_verified_effect_is_done_when_its_target_leaves_the_space() {
    let (inbox, draft) = compose_pages();
    let mut browser = MockBrowser::new(inbox);
    browser.set_on_press(draft);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("click Compose");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(agent.browser_mut().press_log().len(), 1);
}

const LONG_TITLE: &str =
    "IV OF SPADES performs Kabisado LIVE on Wish 107.5 Bus 3 minutes, 36 seconds";

fn results_page(with_title: bool) -> InteractionManifold {
    let title = if with_title {
        format!(
            "region id=title role=link label=\"{LONG_TITLE}\" x=10 y=300 w=500 h=24 actions=click sources=dom,accessibility\n"
        )
    } else {
        String::new()
    };
    manifold(&format!(
        r#"
        viewport w=800 h=600
        region id=chip role=link label="kabisado" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=home role=link label="Home" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        {title}"#
    ))
}

#[test]
fn one_word_chip_inside_a_long_goal_does_not_beat_the_named_title() {
    let mut browser = MockBrowser::new(results_page(true));
    browser.set_on_press(results_page(true));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("click IV OF SPADES performs Kabisado LIVE on Wish 107.5 Bus");
    agent.run();
    let first = agent
        .browser_mut()
        .press_log()
        .first()
        .map(|(id, _)| id.as_str().to_owned());
    assert_eq!(first.as_deref(), Some("title"));
}

#[test]
fn target_not_on_screen_scrolls_down_and_decides_again() {
    let mut browser = MockBrowser::new(results_page(false));
    // The title shows up once the agent has looked and scrolled.
    browser.schedule_swap(1, results_page(true));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("click IV OF SPADES performs Kabisado LIVE on Wish 107.5 Bus");
    agent.run();
    let browser = agent.browser_mut();
    assert!(!browser.scroll_log().is_empty());
    let first = browser
        .press_log()
        .first()
        .map(|(id, _)| id.as_str().to_owned());
    assert_eq!(first.as_deref(), Some("title"));
}

#[test]
fn target_never_found_abstains_after_bounded_scrolls() {
    let mut agent = AgentBuilder::new(
        MockBrowser::new(results_page(false)),
        InstinctPolicy::default(),
    )
    .max_steps(3)
    .build("click IV OF SPADES performs Kabisado LIVE on Wish 107.5 Bus");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    assert!(
        (1..=6).contains(&browser.scroll_log().len()),
        "{:?}",
        browser.scroll_log()
    );
}

/// Clicks the one-word chip once, then has nothing to offer.
struct ChipThenAbstain;

impl BrowserPolicy for ChipThenAbstain {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let chip = space
            .targets_of(ActionKind::Click)
            .find(|a| a.label() == "kabisado");
        match (history.is_empty(), chip) {
            (true, Some(action)) => Ok(PolicyOutcome::Choice(PolicyDecision {
                action_id: action.id().clone(),
                kind: action.kind(),
                target_label: action.label().to_owned(),
                confidence_millis: 1_000,
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            })),
            _ => Ok(PolicyOutcome::Abstain {
                reason: "target abstain".into(),
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            }),
        }
    }
}

#[test]
fn effect_from_a_label_that_misses_the_clause_is_not_done() {
    let mut browser = MockBrowser::new(results_page(false));
    browser.set_on_press(results_page(false));
    let mut agent = AgentBuilder::new(browser, ChipThenAbstain)
        .max_steps(3)
        .build("click IV OF SPADES performs Kabisado LIVE on Wish 107.5 Bus");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
}

#[test]
fn half_matching_channel_link_does_not_win_for_a_longer_title() {
    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=channel role=link label="IV OF SPADES" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        region id=home role=link label="Home" x=10 y=50 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(3)
        .build("click IV OF SPADES Kabisado Karaoke Version");
    let outcome = agent.run();
    assert!(
        matches!(outcome, AgentOutcome::Abstained { .. }),
        "{outcome:?}"
    );
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn matched_click_ends_the_clause_before_a_near_twin_on_the_next_page() {
    let next = manifold(
        r#"
        viewport w=800 h=600
        region id=twin role=link label="IV OF SPADES performs Suliranin LIVE on Wish 107.5 Bus 4 minutes, 12 seconds" x=10 y=300 w=500 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser =
        MockBrowser::new(results_page(true)).with_page(ultra_instinct_browser::PageState::new(
            "https://www.youtube.com/results?search_query=kabisado",
            "kabisado - YouTube",
            None,
        ));
    browser.set_on_press(next);
    browser.set_on_press_page(ultra_instinct_browser::PageState::new(
        "https://www.youtube.com/watch?v=kabisado",
        "Kabisado - YouTube",
        None,
    ));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("click IV OF SPADES performs Kabisado LIVE on Wish 107.5 Bus");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let pressed: Vec<&str> = agent
        .browser_mut()
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(pressed, ["title"]);
}

fn player(with_skip: bool) -> InteractionManifold {
    let skip = if with_skip {
        "region id=skip role=button label=\"Skip Ad\" x=600 y=400 w=80 h=24 actions=click sources=dom,accessibility\n"
    } else {
        ""
    };
    manifold(&format!(
        r#"
        viewport w=800 h=600
        region id=pause role=button label="Pause" x=10 y=500 w=40 h=24 actions=click sources=dom,accessibility
        {skip}"#
    ))
}

#[test]
fn optional_click_waits_for_its_target_and_clicks_without_scrolling() {
    let mut browser = MockBrowser::new(player(false));
    // The skip button shows up after a few re-checks (an ad's countdown).
    browser.schedule_swap(3, player(true));
    // Skipping removes the button.
    browser.set_on_press(player(false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("click Skip Ad if present");
    agent.run();
    let browser = agent.browser_mut();
    assert!(browser.pauses() > 0);
    assert!(browser.scroll_log().is_empty());
    let pressed: Vec<&str> = browser
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(pressed, ["skip"]);
}

#[test]
fn optional_click_is_skipped_when_its_target_never_appears() {
    let mut agent = AgentBuilder::new(MockBrowser::new(player(false)), InstinctPolicy::default())
        .max_steps(3)
        .max_wait_polls(5)
        .build("click Skip Ad if present");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    assert!(browser.scroll_log().is_empty());
    assert_eq!(browser.pauses(), 5);
}

#[test]
fn wait_for_ends_when_the_target_appears_then_next_clause_acts() {
    let mut browser = MockBrowser::new(player(false));
    browser.schedule_swap(2, player(true));
    browser.set_on_press(player(false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("wait for Skip Ad then click Skip Ad");
    agent.run();
    let browser = agent.browser_mut();
    let pressed: Vec<&str> = browser
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(pressed, ["skip"]);
    assert!(browser.pauses() >= 1);
}

#[test]
fn wait_for_times_out_without_acting() {
    let mut agent = AgentBuilder::new(MockBrowser::new(player(false)), InstinctPolicy::default())
        .max_wait_polls(4)
        .build("wait for Skip Ad");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    assert_eq!(browser.pauses(), 4);
}

/// A video page: a skip-link in the masthead, and optionally an ad overlay.
fn watch_page(sponsored: bool, skip: bool) -> InteractionManifold {
    let mut extra = String::new();
    if sponsored {
        extra.push_str("region id=badge role=generic label=\"Sponsored\" x=20 y=380 w=80 h=16 actions=focus sources=dom,accessibility\n");
    }
    if skip {
        extra.push_str("region id=skip role=button label=\"Skip\" x=600 y=400 w=80 h=24 actions=click sources=dom,accessibility\n");
    }
    manifold(&format!(
        r#"
        viewport w=800 h=600
        region id=skipnav role=button label="Skip navigation" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        region id=pause role=button label="Pause" x=10 y=500 w=40 h=24 actions=click sources=dom,accessibility
        {extra}"#
    ))
}

#[test]
fn exact_skip_button_beats_the_skip_navigation_link() {
    let mut browser = MockBrowser::new(watch_page(true, true));
    browser.set_on_press(watch_page(false, false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("click Skip");
    agent.run();
    let pressed: Vec<&str> = agent
        .browser_mut()
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(pressed, ["skip"]);
}

#[test]
fn skip_while_marker_clicks_skip_and_ends_when_the_ad_is_gone() {
    let mut browser = MockBrowser::new(watch_page(true, false));
    // Skip appears a few seconds into the ad; skipping ends the ad.
    browser.schedule_swap(3, watch_page(true, true));
    browser.set_on_press(watch_page(false, false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("click Skip if present while Sponsored");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let browser = agent.browser_mut();
    let pressed: Vec<&str> = browser
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(pressed, ["skip"]);
    assert!(browser.scroll_log().is_empty());
}

#[test]
fn skip_while_marker_keeps_going_for_a_second_ad() {
    let ad2 = manifold(
        r#"
        viewport w=800 h=600
        region id=skipnav role=button label="Skip navigation" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        region id=badge2 role=generic label="Sponsored" x=20 y=380 w=80 h=16 actions=focus sources=dom,accessibility
        region id=skip2 role=button label="Skip" x=600 y=400 w=80 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser = MockBrowser::new(watch_page(true, true));
    // Skipping ad 1 of 2 starts ad 2, which offers its own Skip.
    browser.set_on_press(ad2);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .max_wait_polls(3)
        .build("click Skip if present while Sponsored");
    agent.run();
    let pressed: Vec<&str> = agent
        .browser_mut()
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    // The clause did not stop after the first effective skip.
    assert_eq!(
        pressed.get(..2),
        Some(&["skip", "skip2"][..]),
        "{pressed:?}"
    );
}

#[test]
fn skip_while_marker_waits_out_an_unskippable_ad() {
    let mut browser = MockBrowser::new(watch_page(true, false));
    // A bumper with no Skip button ends on its own.
    browser.schedule_swap(4, watch_page(false, false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("click Skip if present while Sponsored");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    assert!(browser.pauses() >= 1);
    assert!(browser.scroll_log().is_empty());
}

#[test]
fn skip_while_marker_ends_after_a_short_grace_without_an_ad() {
    let mut agent = AgentBuilder::new(
        MockBrowser::new(watch_page(false, false)),
        InstinctPolicy::default(),
    )
    .build("click Skip if present while Sponsored");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    // Only the short grace (3 s of 250 ms re-checks) for an ad that has
    // not started yet.
    assert_eq!(browser.pauses(), 12);
}

#[test]
fn skip_while_marker_catches_an_ad_that_starts_after_the_page() {
    let mut browser = MockBrowser::new(watch_page(false, false));
    // The ad (already skippable) starts two checks after the page loads.
    browser.schedule_swap(2, watch_page(true, true));
    browser.set_on_press(watch_page(false, false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(5)
        .build("click Skip if present while Sponsored");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let pressed: Vec<&str> = agent
        .browser_mut()
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(pressed, ["skip"]);
}

#[test]
fn click_with_no_visible_change_is_retried_as_a_script_click() {
    // The page shows nothing after the first (pointer) click.
    let mut agent = AgentBuilder::new(MockBrowser::new(player(true)), InstinctPolicy::default())
        .max_steps(2)
        .build("click Skip Ad");
    agent.run();
    let inputs: Vec<(String, ultra_instinct_agent::Input)> = agent
        .browser_mut()
        .input_log()
        .iter()
        .map(|(id, input)| (id.as_str().to_owned(), input.clone()))
        .collect();
    assert_eq!(
        inputs,
        [
            ("skip".to_owned(), ultra_instinct_agent::Input::Click),
            ("skip".to_owned(), ultra_instinct_agent::Input::ScriptClick),
        ]
    );
}

#[test]
fn skip_while_marker_clicks_with_a_trusted_pointer() {
    let mut browser = MockBrowser::new(watch_page(true, true));
    browser.set_on_press(watch_page(false, false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .build("click Skip if present while Sponsored");
    agent.run();
    let inputs: Vec<ultra_instinct_agent::Input> = agent
        .browser_mut()
        .input_log()
        .iter()
        .map(|(_, i)| i.clone())
        .collect();
    assert_eq!(inputs, [ultra_instinct_agent::Input::Click]);
}

#[test]
fn long_skip_wait_is_not_cut_short_by_the_policy_call_budget() {
    let mut browser = MockBrowser::new(watch_page(true, false));
    browser.schedule_swap(200, watch_page(false, false));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_policy_calls(20)
        .max_wait_polls(1_000)
        .build("click Skip if present while Sponsored");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
}

const ONE_EDIT_IN_VIEW: &str = r#"
    viewport w=800 h=600
    region id=e1 role=link label="edit" x=10 y=900 w=40 h=24 actions=click sources=dom,accessibility flags=offscreen
    region id=e2 role=link label="edit" x=10 y=300 w=40 h=24 actions=click sources=dom,accessibility
    "#;

#[test]
fn twin_found_after_scrolling_abstains_instead_of_clicking_the_survivor() {
    let mut browser = MockBrowser::new(results_page(false));
    browser.schedule_swap(1, manifold(ONE_EDIT_IN_VIEW));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(3)
        .build("Click edit");
    let outcome = agent.run();
    match &outcome {
        AgentOutcome::Abstained { reason, .. } => {
            assert!(reason.contains("target ambiguous"), "{reason}")
        }
        other => panic!("expected ambiguity abstain, got {other:?}"),
    }
    assert!(agent.browser_mut().press_log().is_empty());
}

#[test]
fn visible_twins_abstain_without_scrolling() {
    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=e1 role=link label="edit" x=10 y=100 w=40 h=24 actions=click sources=dom,accessibility
        region id=e2 role=link label="edit" x=10 y=300 w=40 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(3)
        .build("Click edit");
    let outcome = agent.run();
    match &outcome {
        AgentOutcome::Abstained { reason, .. } => {
            assert!(reason.contains("target ambiguous"), "{reason}")
        }
        other => panic!("expected ambiguity abstain, got {other:?}"),
    }
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    assert!(
        browser.scroll_log().is_empty(),
        "{:?}",
        browser.scroll_log()
    );
}

/// A remote policy that picks the search box whenever TYPE_TEXT is offered.
struct PrefersTyping;

impl BrowserPolicy for PrefersTyping {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = if !history.is_empty() {
            space.get_str(ActionKind::Done.as_str())
        } else if let Some(typing) = space.targets_of(ActionKind::TypeText).next() {
            Some(typing)
        } else {
            space
                .targets_of(ActionKind::Click)
                .find(|a| a.label() == "Learn more")
        };
        let action = action.expect("an offered action");
        Ok(PolicyOutcome::Choice(PolicyDecision {
            action_id: action.id().clone(),
            kind: action.kind(),
            target_label: action.label().to_owned(),
            confidence_millis: 0,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }))
    }
}

#[test]
fn payloadless_type_text_choice_replans_without_typing() {
    let page = manifold(
        r#"
        viewport w=800 h=600
        region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
        region id=learn role=link label="Learn more" x=10 y=60 w=100 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser = MockBrowser::new(page.clone());
    browser.set_on_press(page);
    let mut agent = AgentBuilder::new(browser, PrefersTyping)
        .max_steps(3)
        .build("Click Learn more");
    let outcome = agent.run();
    assert!(
        !matches!(outcome, AgentOutcome::Failed { .. }),
        "{outcome:?}"
    );
    let browser = agent.browser_mut();
    assert!(
        browser.input_log().iter().all(|(id, _)| id.as_str() != "q"),
        "{:?}",
        browser.input_log()
    );
    let first = browser
        .press_log()
        .first()
        .map(|(id, _)| id.as_str().to_owned());
    assert_eq!(first.as_deref(), Some("learn"));
}

#[test]
fn visible_target_with_offscreen_twin_abstains_without_scrolling() {
    let mut agent = AgentBuilder::new(
        MockBrowser::new(manifold(ONE_EDIT_IN_VIEW)),
        InstinctPolicy::default(),
    )
    .max_steps(3)
    .build("Click edit");
    let outcome = agent.run();
    match &outcome {
        AgentOutcome::Abstained { reason, .. } => {
            assert!(reason.contains("target ambiguous"), "{reason}")
        }
        other => panic!("expected ambiguity abstain, got {other:?}"),
    }
    let browser = agent.browser_mut();
    assert!(browser.press_log().is_empty());
    assert!(
        browser.scroll_log().is_empty(),
        "{:?}",
        browser.scroll_log()
    );
}

fn with_hrefs(src: &str, hrefs: &[(&str, &str)]) -> InteractionManifold {
    let mut m = manifold(src);
    for (id, href) in hrefs {
        let region = m.get_str(id).unwrap().clone().with_state(ElementState {
            href: Some((*href).to_owned()),
            ..ElementState::default()
        });
        m.replace(region);
    }
    m
}

const ONE_ARRAY_IN_VIEW: &str = r#"
    viewport w=800 h=600
    region id=a1 role=link label="Array" x=10 y=300 w=60 h=24 actions=click sources=dom,accessibility
    region id=a2 role=link label="Array" x=300 y=2600 w=60 h=24 actions=click sources=dom,accessibility flags=offscreen
    "#;

#[test]
fn offscreen_twin_to_the_same_href_is_not_ambiguous() {
    let page = with_hrefs(
        ONE_ARRAY_IN_VIEW,
        &[
            ("a1", "https://x.test/Array"),
            ("a2", "https://x.test/Array"),
        ],
    );
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(3)
        .build("Click Array");
    agent.run();
    let presses = agent.browser_mut().press_log();
    assert!(!presses.is_empty());
    assert!(
        presses
            .iter()
            .all(|(id, action)| id.as_str() == "a1" && *action == Action::Click),
        "{presses:?}"
    );
}

#[test]
fn offscreen_twin_to_a_different_href_abstains() {
    let page = with_hrefs(
        ONE_ARRAY_IN_VIEW,
        &[
            ("a1", "https://x.test/Array"),
            ("a2", "https://x.test/Array#section"),
        ],
    );
    let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
        .max_steps(3)
        .build("Click Array");
    let outcome = agent.run();
    assert!(
        matches!(&outcome, AgentOutcome::Abstained { reason, .. } if reason.contains("target ambiguous")),
        "{outcome:?}"
    );
    assert!(agent.browser_mut().press_log().is_empty());
}

fn pressed(browser: &MockBrowser) -> Vec<String> {
    browser
        .press_log()
        .iter()
        .map(|(id, _)| id.as_str().to_owned())
        .collect()
}

fn run_static(src: &str, goal: &str, max_steps: u32) -> (AgentOutcome, Vec<String>, MockBrowser) {
    let page = manifold(src);
    let mut browser = MockBrowser::new(page.clone());
    browser.set_on_press(page);
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(max_steps)
        .build(goal);
    let outcome = agent.run();
    let presses = pressed(agent.browser_mut());
    (outcome, presses, agent.into_browser())
}

#[test]
fn optional_clause_with_twins_is_skipped_not_fatal() {
    let (outcome, presses, _) = run_static(
        r#"
        viewport w=800 h=600
        region id=a1 role=button label="Accept" x=10 y=100 w=80 h=24 actions=click sources=dom,accessibility
        region id=a2 role=button label="Accept" x=10 y=200 w=80 h=24 actions=click sources=dom,accessibility
        region id=home role=link label="Home" x=10 y=300 w=80 h=24 actions=click sources=dom,accessibility
        "#,
        "Click Accept if present then click Home",
        4,
    );
    assert!(!presses.iter().any(|p| p.starts_with('a')), "{presses:?}");
    assert_eq!(
        presses.first().map(String::as_str),
        Some("home"),
        "{outcome:?}"
    );
}

#[test]
fn modal_button_with_buried_offscreen_page_twin_is_clicked() {
    let (outcome, presses, _) = run_static(
        r#"
        viewport w=1440 h=900
        region id=page role=generic label="Files" x=0 y=0 w=1440 h=2000 actions=focus sources=dom
        region id=page-delete role=button label="Delete" x=1200 y=1780 w=160 h=36 actions=click,focus parent=page sources=dom,accessibility flags=offscreen
        region id=confirm role=dialog label="Delete file?" x=520 y=300 w=400 h=240 actions=focus sources=dom,accessibility flags=modal
        region id=confirm-cancel role=button label="Cancel" x=560 y=480 w=100 h=36 actions=click,focus parent=confirm sources=dom,accessibility
        region id=confirm-delete role=button label="Delete" x=780 y=480 w=100 h=36 actions=click,focus parent=confirm sources=dom,accessibility
        "#,
        "Click Delete",
        2,
    );
    assert_eq!(
        presses.first().map(String::as_str),
        Some("confirm-delete"),
        "{outcome:?}"
    );
}

#[test]
fn modal_confirm_fixture_click_cancel() {
    let (outcome, presses, _) = run_static(
        include_str!("../../../fixtures/modal-confirm.manifold"),
        "Click Cancel",
        2,
    );
    assert_eq!(
        presses.first().map(String::as_str),
        Some("confirm-cancel"),
        "{outcome:?}"
    );
}

#[test]
fn type_into_field_ignores_offscreen_same_label_button() {
    let (outcome, _, browser) = run_static(
        r#"
        viewport w=800 h=600
        region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
        region id=go role=button label="Search" x=10 y=900 w=80 h=24 actions=click sources=dom,accessibility flags=offscreen
        "#,
        r#"Type "rust" into Search"#,
        2,
    );
    assert!(
        browser
            .input_log()
            .iter()
            .any(|(id, i)| id.as_str() == "q" && i.payload() == Some("rust")),
        "{outcome:?}"
    );
}

#[test]
fn hidden_disabled_or_zero_area_offscreen_twins_do_not_count() {
    for twin in [
        r#"region id=e1 role=link label="edit" x=10 y=900 w=40 h=24 actions=click sources=dom,accessibility flags=offscreen,hidden"#,
        r#"region id=e1 role=link label="edit" x=10 y=900 w=40 h=24 actions=click sources=dom,accessibility flags=offscreen,disabled"#,
        r#"region id=e1 role=link label="edit" x=10 y=900 w=0 h=0 actions=click sources=dom,accessibility flags=offscreen"#,
    ] {
        let src = format!(
            "viewport w=800 h=600\n{twin}\nregion id=e2 role=link label=\"edit\" x=10 y=300 w=40 h=24 actions=click sources=dom,accessibility\n"
        );
        let (outcome, presses, _) = run_static(&src, "Click edit", 1);
        assert_eq!(
            presses.first().map(String::as_str),
            Some("e2"),
            "{twin}: {outcome:?}"
        );
    }
}

#[test]
fn placeholder_hrefs_are_not_one_destination() {
    for href in [
        "https://x.test/rows#",
        "javascript:void(0)",
        "JavaScript:del()",
    ] {
        let page = with_hrefs(ONE_ARRAY_IN_VIEW, &[("a1", href), ("a2", href)]);
        let mut agent = AgentBuilder::new(MockBrowser::new(page), InstinctPolicy::default())
            .max_steps(3)
            .build("Click Array");
        let outcome = agent.run();
        assert!(
            matches!(&outcome, AgentOutcome::Abstained { reason, .. } if reason.contains("target ambiguous")),
            "{href}: {outcome:?}"
        );
        assert!(agent.browser_mut().press_log().is_empty(), "{href}");
    }
}

/// Reads pixels and page text: records what it got, then DONE.
#[derive(Default)]
struct SeesThenDone {
    seen: std::rc::Rc<std::cell::RefCell<Vec<DecisionEvidence>>>,
}

impl BrowserPolicy for SeesThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        self.decide_with_evidence(space, goal, history, &DecisionEvidence::default())
    }

    fn wants_evidence(&self) -> EvidenceWants {
        EvidenceWants {
            screenshot: true,
            page_text: true,
        }
    }

    fn decide_with_evidence(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        _history: &[HistoryEntry],
        evidence: &DecisionEvidence,
    ) -> Result<PolicyOutcome, PolicyError> {
        self.seen.borrow_mut().push(evidence.clone());
        let done = space
            .get_str(ActionKind::Done.as_str())
            .expect("DONE offered");
        Ok(PolicyOutcome::Choice(PolicyDecision {
            action_id: done.id().clone(),
            kind: done.kind(),
            target_label: done.label().to_owned(),
            confidence_millis: 1_000,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }))
    }
}

const ONE_BUTTON: &str = r#"
    viewport w=800 h=600
    region id=go role=button label="Continue" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
    "#;

#[test]
fn policy_that_wants_evidence_gets_the_decision_screenshot_and_page_text() {
    let browser = MockBrowser::new(manifold(ONE_BUTTON))
        .with_screenshot(Screenshot::from_base64("/9j/AA=="))
        .with_page_text("x".repeat(ultra_instinct_agent::PAGE_TEXT_MAX_CHARS + 10));
    let policy = SeesThenDone::default();
    let seen = policy.seen.clone();
    let mut agent = AgentBuilder::new(browser, policy)
        .max_steps(3)
        .build("Finish");
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].screenshot.as_ref().map(Screenshot::base64),
        Some("/9j/AA==")
    );
    let page = seen[0].page.as_ref().expect("page view");
    assert_eq!(
        page.text.chars().count(),
        ultra_instinct_agent::PAGE_TEXT_MAX_CHARS,
        "capped"
    );
    assert_eq!(agent.into_browser().screenshots_taken(), 1);
}

#[test]
fn instinct_never_triggers_a_screenshot() {
    let browser =
        MockBrowser::new(manifold(ONE_BUTTON)).with_screenshot(Screenshot::from_base64("/9j/AA=="));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(2)
        .build("Continue");
    let _ = agent.run();
    assert_eq!(agent.into_browser().screenshots_taken(), 0);
}

fn remember_me(checked: bool) -> InteractionManifold {
    let mut page = manifold(
        r#"
        viewport w=800 h=600
        region id=remember role=checkbox label="Remember me" x=10 y=10 w=20 h=20 actions=click sources=dom,accessibility
        "#,
    );
    let region =
        page.get_str("remember")
            .unwrap()
            .clone()
            .with_state(ultra_instinct_core::ElementState {
                checked: Some(checked),
                ..ultra_instinct_core::ElementState::default()
            });
    page.replace(region);
    page
}

#[test]
fn checkbox_click_that_only_flips_checked_is_state_changed() {
    let mut browser = MockBrowser::new(remember_me(false));
    browser.set_on_press(remember_me(true));
    let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
        .max_steps(1)
        .build("Remember me");
    agent.predict().unwrap();
    let record = agent.act().unwrap();
    assert_eq!(record.verification, VerificationKind::StateChanged);
}

/// Types whenever TYPE_TEXT is offered, else DONE: the shape of a remote
/// model that keeps re-entering a field.
struct TypesWhileOffered;

impl BrowserPolicy for TypesWhileOffered {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        _history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = space
            .targets_of(ActionKind::TypeText)
            .next()
            .or_else(|| space.get_str(ActionKind::Done.as_str()))
            .expect("DONE offered");
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

fn name_field(value: Option<&str>) -> InteractionManifold {
    let mut page = manifold(
        r#"
        viewport w=800 h=600
        region id=name role=text_field label="Name" x=10 y=10 w=200 h=24 actions=type sources=dom
        "#,
    );
    let region = page
        .get_str("name")
        .unwrap()
        .clone()
        .with_state(ElementState {
            value: value.map(str::to_owned),
            ..ElementState::default()
        });
    page.replace(region);
    page
}

#[test]
fn a_field_already_holding_the_typed_value_is_not_offered_again() {
    let mut browser = MockBrowser::new(name_field(None));
    browser.set_on_press(name_field(Some("Ana")));
    let mut agent = AgentBuilder::new(browser, TypesWhileOffered)
        .max_steps(5)
        .build(r#"Type "Ana" into Name"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let typed: Vec<_> = outcome
        .steps()
        .iter()
        .filter(|s| s.kind == ActionKind::TypeText)
        .collect();
    assert_eq!(typed.len(), 1, "typed once, then moved on: {outcome:?}");
}

/// Types, then clicks Submit, then reports whether TYPE_TEXT is offered.
struct TypeSubmitLook(std::rc::Rc<std::cell::RefCell<Vec<bool>>>);

impl BrowserPolicy for TypeSubmitLook {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        _history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let mut seen = self.0.borrow_mut();
        seen.push(space.get_str("TYPE_TEXT:guess").is_some());
        let id = match seen.len() {
            1 => "TYPE_TEXT:guess",
            2 => "CLICK:go",
            _ => "DONE",
        };
        let action = space.get_str(id).expect("offered");
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

fn guess_page(value: Option<&str>, feedback: &str) -> InteractionManifold {
    let mut page = manifold(&format!(
        "viewport w=800 h=600\n\
         region id=guess role=text_field label=\"Guess\" x=10 y=10 w=200 h=24 actions=type sources=dom\n\
         region id=go role=button label=\"Submit\" x=10 y=50 w=80 h=24 actions=click sources=dom\n\
         region id=fb role=text label=\"{feedback}\" x=10 y=90 w=200 h=20 actions= sources=dom\n"
    ));
    let region = page
        .get_str("guess")
        .unwrap()
        .clone()
        .with_state(ElementState {
            value: value.map(str::to_owned),
            ..ElementState::default()
        });
    page.replace(region);
    page
}

#[test]
fn a_field_may_be_typed_again_after_the_page_answered() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut browser = MockBrowser::new(guess_page(None, "Pick a number"));
    browser.set_on_press(guess_page(Some("5"), "Pick a number"));
    let mut agent = AgentBuilder::new(browser, TypeSubmitLook(seen.clone()))
        .max_steps(5)
        .build(r#"Type "5" into Guess and press Submit"#);
    agent.predict().unwrap();
    agent.act().unwrap();
    agent
        .browser_mut()
        .set_on_press(guess_page(Some("5"), "Higher"));
    agent.predict().unwrap();
    let submit = agent.act().unwrap();
    assert_eq!(submit.verification, VerificationKind::StateChanged);
    agent.predict().unwrap();
    assert_eq!(
        *seen.borrow(),
        [true, false, true],
        "held value: hidden; after the page answered: offered again"
    );
}

#[test]
fn a_later_clause_may_type_another_value_into_the_same_field() {
    let mut browser = MockBrowser::new(name_field(None));
    browser.set_on_press(name_field(Some("Ana")));
    let mut agent = AgentBuilder::new(browser, TypesWhileOffered)
        .max_steps(3)
        .build(r#"Type "Ana" into Name then type "Bo" into Name"#);
    let outcome = agent.run();
    let typed: Vec<_> = outcome
        .steps()
        .iter()
        .filter(|s| s.kind == ActionKind::TypeText)
        .filter_map(|s| s.payload.as_deref())
        .collect();
    assert_eq!(typed.first(), Some(&"Ana"), "{outcome:?}");
    assert!(
        typed.contains(&"Bo"),
        "the second clause still types: {outcome:?}"
    );
}

type SeenProgress = std::rc::Rc<std::cell::RefCell<Vec<ultra_instinct_policy::Progress>>>;

/// Wraps a policy, keeping the progress each call saw.
struct RecordsProgress<P>(P, SeenProgress);

impl<P: BrowserPolicy> BrowserPolicy for RecordsProgress<P> {
    fn decide(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        self.0.decide(space, goal, history)
    }

    fn wants_evidence(&self) -> EvidenceWants {
        EvidenceWants {
            screenshot: true,
            page_text: false,
        }
    }

    fn decide_with_evidence(
        &mut self,
        space: &ActionSpace,
        goal: &AgentGoal,
        history: &[HistoryEntry],
        evidence: &DecisionEvidence,
    ) -> Result<PolicyOutcome, PolicyError> {
        self.1.borrow_mut().extend(evidence.progress.clone());
        self.decide(space, goal, history)
    }
}

fn typed_progress(label: &str) -> Vec<(String, String)> {
    let field = |value: Option<&str>| {
        let mut page = manifold(&format!(
            "viewport w=800 h=600\nregion id=f role=text_field label=\"{label}\" x=10 y=10 w=200 h=24 actions=type sources=dom\n"
        ));
        let region = page.get_str("f").unwrap().clone().with_state(ElementState {
            value: value.map(str::to_owned),
            ..ElementState::default()
        });
        page.replace(region);
        page
    };
    let seen = SeenProgress::default();
    let mut browser = MockBrowser::new(field(None));
    browser.set_on_press(field(Some("4821")));
    let mut agent = AgentBuilder::new(browser, RecordsProgress(TypesWhileOffered, seen.clone()))
        .max_steps(3)
        .build(format!(r#"Type "4821" into {label}"#));
    agent.run();
    let last = seen.borrow().last().cloned().expect("progress sent");
    last.typed
}

#[test]
fn progress_shows_typed_values_and_masks_only_secret_fields() {
    // The field still holds the value, so its TYPE is no longer offered;
    // the value is still reported as typed, unmasked.
    assert_eq!(
        typed_progress("Zip code"),
        [("TYPE_TEXT:f".to_owned(), "4821".to_owned())]
    );
    // A secret label masks it.
    assert_eq!(
        typed_progress("PIN"),
        [("TYPE_TEXT:f".to_owned(), "••••".to_owned())]
    );
}

/// Clicks the first offered click target once, then DONE.
struct ClickOnceThenDone;

impl BrowserPolicy for ClickOnceThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = if history.is_empty() {
            space.targets_of(ActionKind::Click).next()
        } else {
            space.get_str(ActionKind::Done.as_str())
        }
        .expect("offered");
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

#[test]
fn a_popup_that_renders_late_still_counts_as_the_clicks_effect() {
    let picker = manifold(
        r#"
        viewport w=800 h=600
        region id=date role=button label="Pick a date" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        "#,
    );
    let open = manifold(
        r#"
        viewport w=800 h=600
        region id=date role=button label="Pick a date" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        region id=d12 role=button label="12" x=10 y=40 w=20 h=20 actions=click sources=dom,accessibility
        "#,
    );
    let mut browser = MockBrowser::new(picker);
    // Observes: decide, executor's fresh look, after the click, the late look.
    browser.schedule_swap(3, open);
    let mut agent = AgentBuilder::new(browser, ClickOnceThenDone)
        .max_steps(3)
        .build("Pick a date");
    let outcome = agent.run();
    let first = outcome.steps().first().expect("one click").verification;
    assert_eq!(first, VerificationKind::StateChanged, "{outcome:?}");
}

#[test]
fn a_change_seen_only_on_the_late_look_does_not_satisfy_the_clause() {
    let page = r#"
        viewport w=800 h=600
        region id=send role=button label="Send" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        "#;
    let toast = r#"
        viewport w=800 h=600
        region id=send role=button label="Send" x=10 y=10 w=120 h=24 actions=click sources=dom,accessibility
        region id=t role=button label="Dismiss" x=400 y=500 w=80 h=24 actions=click sources=dom,accessibility
        "#;
    let seen = SeenProgress::default();
    let mut browser = MockBrowser::new(manifold(page));
    // Observes: decide, executor's fresh look, after the click, the late look.
    browser.schedule_swap(3, manifold(toast));
    let mut agent = AgentBuilder::new(browser, RecordsProgress(ClickOnceThenDone, seen.clone()))
        .max_steps(3)
        .build("Send then click Dismiss");
    agent.predict().unwrap();
    let record = agent.act().unwrap();
    assert_eq!(record.verification, VerificationKind::StateChanged);
    // Not credited: the next decision still sees the first clause open.
    agent.predict().unwrap();
    let progress = seen.borrow().last().cloned().expect("progress sent");
    assert_eq!(progress.current, 0);
    assert!(
        !progress.satisfied[0],
        "a late-only change keeps the clause open"
    );
}

/// Picks the given kind's first target once, then DONE.
struct OnceThenDone(ActionKind);

impl BrowserPolicy for OnceThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = if history.is_empty() {
            space.targets_of(self.0).next()
        } else {
            space.get_str(ActionKind::Done.as_str())
        }
        .expect("offered");
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

#[test]
fn hover_and_scroll_in_run_through_the_ticketed_executor() {
    let page = r#"
        viewport w=800 h=600
        region id=menu role=menu_item label="File" x=10 y=10 w=60 h=24 actions=click,hover sources=dom,accessibility
        region id=list role=generic label="Inbox" x=10 y=50 w=200 h=300 actions=scroll sources=dom
        "#;
    for (kind, input, target) in [
        (
            ActionKind::Hover,
            ultra_instinct_agent::Input::Hover,
            "menu",
        ),
        (
            ActionKind::ScrollIn,
            ultra_instinct_agent::Input::ScrollIn,
            "list",
        ),
    ] {
        let mut agent = AgentBuilder::new(MockBrowser::new(manifold(page)), OnceThenDone(kind))
            .max_steps(3)
            .build("Open the menu");
        let _ = agent.run();
        let browser = agent.into_browser();
        let log = browser.input_log();
        assert_eq!(log.len(), 1, "{kind:?}: {log:?}");
        assert_eq!((log[0].0.as_str(), &log[0].1), (target, &input));
    }
}

#[test]
fn drag_runs_end_to_end_through_the_two_ended_ticket() {
    let page = r#"
        viewport w=800 h=600
        region id=card role=generic label="Card" x=10 y=10 w=60 h=40 actions=drag sources=dom
        region id=bin role=generic label="Bin" x=400 y=300 w=120 h=80 sources=dom
        "#;
    let mut agent = AgentBuilder::new(
        MockBrowser::new(manifold(page)),
        OnceThenDone(ActionKind::Drag),
    )
    .max_steps(3)
    .build("Drag the card into the bin");
    let _ = agent.run();
    let browser = agent.into_browser();
    let log = browser.input_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].0.as_str(), "card");
    assert!(matches!(&log[0].1, ultra_instinct_agent::Input::Drag { to } if to.as_str() == "bin"));
}

/// Picks SELECT_TEXT on the paragraph, then DONE.
struct SelectsThenDone;

impl BrowserPolicy for SelectsThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let id = if history.is_empty() {
            "SELECT_TEXT:para"
        } else {
            "DONE"
        };
        let action = space.get_str(id).expect("offered");
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

#[test]
fn a_selection_verifies_by_the_selected_text() {
    let page = || {
        manifold(
            r#"
            viewport w=800 h=600
            region id=para role=text label="Nibh interdum curabitur" x=10 y=10 w=300 h=40 actions=select_text sources=dom
            "#,
        )
    };
    let run = |browser: MockBrowser| {
        let mut agent = AgentBuilder::new(browser, SelectsThenDone)
            .max_steps(1)
            .build("Highlight the text in the paragraph");
        agent.predict().unwrap();
        agent.act().unwrap().verification
    };
    assert_eq!(
        run(MockBrowser::new(page()).with_selection_on_select("Nibh interdum curabitur")),
        VerificationKind::Success
    );
    assert_eq!(
        run(MockBrowser::new(page())),
        VerificationKind::WrongEffect,
        "nothing selected"
    );
}

/// Picks SELECT_TEXT on the paragraph, then DONE.
struct SelectsThenDone;

impl BrowserPolicy for SelectsThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let id = if history.is_empty() {
            "SELECT_TEXT:para"
        } else {
            "DONE"
        };
        let action = space.get_str(id).expect("offered");
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

#[test]
fn a_selection_verifies_by_the_selected_text() {
    let page = || {
        manifold(
            r#"
            viewport w=800 h=600
            region id=para role=text label="Nibh interdum curabitur" x=10 y=10 w=300 h=40 actions=select_text sources=dom
            "#,
        )
    };
    let run = |browser: MockBrowser| {
        let mut agent = AgentBuilder::new(browser, SelectsThenDone)
            .max_steps(1)
            .build("Highlight the text in the paragraph");
        agent.predict().unwrap();
        agent.act().unwrap().verification
    };
    assert_eq!(
        run(MockBrowser::new(page()).with_selection_on_select("Nibh interdum curabitur")),
        VerificationKind::Success
    );
    assert_eq!(
        run(MockBrowser::new(page())),
        VerificationKind::WrongEffect,
        "nothing selected"
    );
}
