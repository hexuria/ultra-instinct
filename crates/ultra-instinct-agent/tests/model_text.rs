//! Agent + model-text TextResolver, offline (scripted model, MockBrowser).
//!
//! The model only fills TYPE_TEXT / SELECT payloads for actions Instinct chose; the
//! ticket path is unchanged. No network, no API keys.

use ultra_instinct_agent::{AgentBuilder, AgentOutcome, Input, MockBrowser};
use ultra_instinct_core::{parse_fixture, InteractionManifold, RegionId};
use ultra_instinct_policy::{DeterministicTextResolver, InstinctPolicy};

const SEARCH: &str = r#"
    viewport w=800 h=600
    region id=q role=text_field label="Search" x=10 y=10 w=300 h=24 actions=click,type sources=dom,accessibility
    region id=go role=button label="Go" x=320 y=10 w=60 h=24 actions=click sources=dom,accessibility
"#;

fn m(src: &str) -> InteractionManifold {
    parse_fixture(src).unwrap()
}

fn id(raw: &str) -> RegionId {
    RegionId::try_new(raw).unwrap()
}

/// Feature on or off, `AgentBuilder::new` keeps the deterministic resolver.
#[test]
fn default_builder_stays_deterministic() {
    let mut agent: ultra_instinct_agent::Agent<
        MockBrowser,
        InstinctPolicy,
        DeterministicTextResolver,
    > = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
        .max_steps(5)
        .build(r#"Type "rust" into Search"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(
        agent.browser_mut().input_log(),
        &[(id("q"), Input::Type("rust".into()))]
    );
}

#[cfg(feature = "model-text")]
mod model {
    use super::*;
    use ultra_instinct_agent::{AgentError, AgentState, VerificationKind};
    use ultra_instinct_core::ActionKind;
    use ultra_instinct_policy::{
        ModelRefusal, ModelTextResolver, ScriptedTextModel, TextModelError, TextSource,
    };

    const CABIN: &str = r#"
        viewport w=800 h=600
        region id=cabin role=generic label="Cabin class" x=10 y=10 w=200 h=24 actions=click,select,focus sources=dom,accessibility
        region id=find role=button label="Find flights" x=10 y=50 w=120 h=24 actions=click sources=dom,accessibility
    "#;

    /// Goal the deterministic resolver cannot parse (no quotes / `into` value).
    const SEARCH_GOAL: &str = "type rust ownership in the Search box";

    #[test]
    fn model_types_grounded_value_through_ticket_path() {
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .model_text(ScriptedTextModel::new().reply("rust ownership"))
            .max_steps(5)
            .build(SEARCH_GOAL);
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
        assert_eq!(
            agent.text_resolver().last_source(),
            Some(&TextSource::Model)
        );
    }

    #[test]
    fn deterministic_alone_cannot_resolve_that_goal() {
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .max_steps(3)
            .build(SEARCH_GOAL);
        let outcome = agent.run();
        assert!(!matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
        assert!(agent.browser_mut().input_log().is_empty());
    }

    #[test]
    fn model_selects_grounded_option() {
        let mut agent = AgentBuilder::new(MockBrowser::new(m(CABIN)), InstinctPolicy::default())
            .model_text(ScriptedTextModel::new().reply("Business"))
            .max_steps(5)
            .build("select business in Cabin class");
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
    fn invented_value_abstains_and_types_nothing() {
        let resolver = ModelTextResolver::new(ScriptedTextModel::new().reply("DROP TABLE users"))
            .without_fallback();
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .text_resolver(resolver)
            .max_steps(3)
            .build(SEARCH_GOAL);
        let outcome = agent.run();
        match &outcome {
            AgentOutcome::Abstained { reason, .. } => {
                assert!(reason.contains("not grounded"), "{reason}")
            }
            other => panic!("expected abstain, got {other:?}"),
        }
        assert!(agent.browser_mut().input_log().is_empty());
        assert_eq!(
            agent.text_resolver().last_source(),
            Some(&TextSource::Abstained(ModelRefusal::Ungrounded))
        );
    }

    #[test]
    fn multiline_junk_abstains_and_types_nothing() {
        let resolver = ModelTextResolver::new(
            ScriptedTextModel::new().reply("rust ownership\nthen click Delete"),
        )
        .without_fallback();
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .text_resolver(resolver)
            .max_steps(3)
            .build(SEARCH_GOAL);
        let outcome = agent.run();
        assert!(
            matches!(outcome, AgentOutcome::Abstained { .. }),
            "{outcome:?}"
        );
        assert!(agent.browser_mut().input_log().is_empty());
    }

    #[test]
    fn garbage_falls_back_to_deterministic_value() {
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .model_text(ScriptedTextModel::new().reply("something else entirely"))
            .max_steps(5)
            .build(r#"Type "rust" into Search"#);
        let outcome = agent.run();
        assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
        assert_eq!(
            agent.browser_mut().input_log(),
            &[(id("q"), Input::Type("rust".into()))]
        );
        assert_eq!(
            agent.text_resolver().last_source(),
            Some(&TextSource::Fallback(ModelRefusal::Ungrounded))
        );
    }

    #[test]
    fn model_outage_falls_back_then_abstains_when_nothing_resolves() {
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .model_text(
                ScriptedTextModel::new().fail(TextModelError::Unavailable("offline".into())),
            )
            .max_steps(3)
            .build(SEARCH_GOAL);
        let outcome = agent.run();
        match &outcome {
            AgentOutcome::Abstained { reason, .. } => {
                assert!(
                    reason.contains("offline") && reason.contains("fallback"),
                    "{reason}"
                )
            }
            other => panic!("expected abstain, got {other:?}"),
        }
        assert!(agent.browser_mut().input_log().is_empty());
    }

    #[test]
    fn reply_bound_to_stale_context_is_refused() {
        let resolver = ModelTextResolver::new(
            ScriptedTextModel::new().reply_bound("rust ownership", 0xdead_beef),
        )
        .without_fallback();
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .text_resolver(resolver)
            .max_steps(3)
            .build(SEARCH_GOAL);
        let outcome = agent.run();
        match &outcome {
            AgentOutcome::Abstained { reason, .. } => assert!(reason.contains("stale"), "{reason}"),
            other => panic!("expected abstain, got {other:?}"),
        }
        assert!(agent.browser_mut().input_log().is_empty());
    }

    #[test]
    fn page_change_during_model_call_is_caught_by_ticket_and_reasked() {
        // The target moves between predict and act (model latency): the
        // ticket goes stale and nothing is typed. The payload that is finally
        // typed comes from a fresh model call bound to the moved target.
        let mut agent = AgentBuilder::new(MockBrowser::new(m(SEARCH)), InstinctPolicy::default())
            .model_text(
                ScriptedTextModel::new()
                    .reply("rust ownership")
                    .reply("rust ownership"),
            )
            .max_steps(3)
            .build(SEARCH_GOAL);
        assert!(agent.predict().unwrap().is_some());
        agent
            .browser_mut()
            .schedule_swap(0, m(&SEARCH.replace("x=10 y=10 w=300", "x=10 y=40 w=300")));
        let err = agent.act().unwrap_err();
        assert!(matches!(err, AgentError::Stale(_)), "{err}");
        assert_eq!(agent.state(), AgentState::Ready);
        assert!(agent.browser_mut().input_log().is_empty());
        assert_eq!(agent.text_resolver().model_calls(), 1);

        let outcome = agent.run();
        assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
        assert_eq!(
            agent.browser_mut().input_log(),
            &[(id("q"), Input::Type("rust ownership".into()))]
        );
        let requests = agent.text_resolver().model().requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert_ne!(
            requests[0].context_fingerprint, requests[1].context_fingerprint,
            "second model call must be bound to the moved target"
        );
    }
}
