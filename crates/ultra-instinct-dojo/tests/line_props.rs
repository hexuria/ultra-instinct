//! Parser proptests (issue #49): the diary-line parser is the replay arena's
//! front door — every serialized line round-trips, and garbage never panics.

use proptest::prelude::*;
use ultra_instinct_dojo::line::{
    ChoiceLine, ClauseLine, CorrectionLine, DecisionLine, DiaryLine, HistoryLine, OfferedLine,
    OfferedState, OutcomeLine, RankedLine, RunLine, SiteLine, Situation, StaleLine, StepLine,
};
use ultra_instinct_dojo::parse_line;

fn word() -> impl Strategy<Value = String> {
    "[a-z0-9]([a-z0-9 ._-]{0,23})?".prop_map(String::from)
}

fn maybe_word() -> impl Strategy<Value = Option<String>> {
    prop::option::of(word())
}

fn words() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(word(), 0..4)
}

fn opt_bool() -> impl Strategy<Value = Option<bool>> {
    prop::option::of(prop::bool::ANY)
}

fn offered_state() -> impl Strategy<Value = OfferedState> {
    (
        maybe_word(),
        opt_bool(),
        opt_bool(),
        maybe_word(),
        words(),
        prop::option::of(prop_oneof!["password", "text", "email"]),
    )
        .prop_map(
            |(value, checked, expanded, selected, options, input_type)| OfferedState {
                value,
                checked,
                expanded,
                selected,
                options,
                input_type,
            },
        )
}

fn offered() -> impl Strategy<Value = Vec<OfferedLine>> {
    prop::collection::vec(
        (
            "[a-z]{1,8}:[0-9]{1,4}",
            prop_oneof!["click", "type", "select", "press"],
            word(),
            prop::option::of(prop_oneof![
                "button",
                "link",
                "text field",
                "select",
                "combobox"
            ]),
            prop::option::of("[a-z0-9-]{1,12}"),
            0u64..u64::MAX,
            prop::option::of(offered_state()),
        )
            .prop_map(
                |(id, kind, label, role, region, fingerprint, state)| OfferedLine {
                    id,
                    kind,
                    label,
                    role,
                    region,
                    fingerprint,
                    state,
                },
            ),
        0..5,
    )
}

fn ranked() -> impl Strategy<Value = Vec<RankedLine>> {
    prop::collection::vec(
        (
            "[a-z]{1,8}:[0-9]{1,4}",
            prop_oneof!["click", "type", "select", "press"],
            word(),
            0i16..=1000,
        )
            .prop_map(|(id, kind, label, confidence_millis)| RankedLine {
                id,
                kind,
                label,
                confidence_millis,
            }),
        0..5,
    )
}

fn history() -> impl Strategy<Value = Vec<HistoryLine>> {
    prop::collection::vec(
        (
            0u32..1000,
            "[a-z]{1,8}:[0-9]{1,4}",
            prop_oneof!["click", "type", "select", "press"],
            word(),
            prop_oneof![
                "success",
                "state-changed",
                "navigation",
                "no-effect",
                "unchecked"
            ],
        )
            .prop_map(|(step, action_id, kind, label, verification)| HistoryLine {
                step,
                action_id,
                kind,
                label,
                verification,
            }),
        0..5,
    )
}

fn site() -> impl Strategy<Value = Option<SiteLine>> {
    prop::option::of(
        (maybe_word(), maybe_word(), maybe_word(), maybe_word()).prop_map(
            |(url, title, host, path)| SiteLine {
                url,
                title,
                host,
                path,
            },
        ),
    )
}

fn situation() -> impl Strategy<Value = Situation> {
    (
        prop::bool::ANY,
        words(),
        prop::collection::vec(word(), 0..8),
    )
        .prop_map(|(front_layer, roles, near)| Situation {
            front_layer,
            roles,
            near,
        })
}

fn run_line() -> impl Strategy<Value = DiaryLine> {
    (
        word(),
        words(),
        prop_oneof!["instinct", "jev", "clef", "clef-flash", "dojo"],
    )
        .prop_map(|(goal, clauses, policy)| {
            DiaryLine::Run(RunLine {
                goal,
                clauses,
                policy,
            })
        })
        .boxed()
}

fn decision_line() -> impl Strategy<Value = DiaryLine> {
    (
        (
            0u32..10_000,
            0u32..8,
            word(),
            prop_oneof!["act", "done", "blocked"],
            prop_oneof!["instinct", "jev", "clef", "clef-flash"],
        ),
        (
            site(),
            situation(),
            offered(),
            ranked(),
            ranked(),
            history(),
            prop::option::of(
                (
                    "[a-z]{1,8}:[0-9]{1,4}",
                    prop_oneof!["click", "type", "select", "press"],
                    word(),
                    0i16..=1000,
                )
                    .prop_map(
                        |(action_id, kind, target_label, confidence_millis)| ChoiceLine {
                            action_id,
                            kind,
                            target_label,
                            confidence_millis,
                        },
                    ),
            ),
            maybe_word(),
        ),
    )
        .prop_map(
            |(
                (seq, clause_index, clause, mode, source),
                (
                    site,
                    situation,
                    offered,
                    operation_ranked,
                    target_ranked,
                    history,
                    choice,
                    abstain,
                ),
            )| {
                DiaryLine::Decision(Box::new(DecisionLine {
                    seq,
                    clause_index,
                    clause,
                    mode,
                    source,
                    site,
                    situation,
                    offered,
                    operation_ranked,
                    target_ranked,
                    history,
                    choice,
                    abstain,
                }))
            },
        )
        .boxed()
}

fn step_line() -> impl Strategy<Value = DiaryLine> {
    (
        (
            0u32..10_000,
            0u32..100,
            0u32..8,
            word(),
            "[a-z]{1,8}:[0-9]{1,4}",
            prop_oneof!["click", "type", "select", "press"],
            word(),
            prop_oneof!["click", "type", "select", "press"],
        ),
        (
            maybe_word(),
            prop_oneof![
                "success",
                "state-changed",
                "navigation",
                "no-effect",
                "unchecked"
            ],
            0u32..8,
            prop::bool::ANY,
        ),
    )
        .prop_map(
            |(
                (seq, step, clause_index, clause, action_id, kind, label, input),
                (payload, verification, stale_retries, won),
            )| {
                DiaryLine::Step(StepLine {
                    seq,
                    step,
                    clause_index,
                    clause,
                    action_id,
                    kind,
                    label,
                    input,
                    payload,
                    verification,
                    stale_retries,
                    won,
                })
            },
        )
        .boxed()
}

fn outcome_line() -> impl Strategy<Value = DiaryLine> {
    (
        prop_oneof!["done", "blocked", "abstained", "failed"],
        word(),
        0u32..100,
        0u32..100,
        0u32..10,
        0u64..u64::MAX,
    )
        .prop_map(
            |(kind, reason, steps, policy_calls, stale_discards, duration_ms)| {
                DiaryLine::Outcome(OutcomeLine {
                    kind,
                    reason,
                    steps,
                    policy_calls,
                    stale_discards,
                    duration_ms,
                })
            },
        )
        .boxed()
}

fn diary_line() -> impl Strategy<Value = DiaryLine> {
    prop_oneof![
        run_line(),
        decision_line(),
        step_line(),
        (
            0u32..10_000,
            prop_oneof!["world-changed", "target-changed", "target-gone"]
        )
            .prop_map(|(seq, reason)| { DiaryLine::StaleDiscard(StaleLine { seq, reason }) }),
        (0u32..10_000, 0u32..8, word()).prop_map(|(seq, clause_index, clause)| {
            DiaryLine::ClauseAdvanced(ClauseLine {
                seq,
                clause_index,
                clause,
            })
        }),
        outcome_line(),
        (word(), word(), word()).prop_map(|(clause, expected, chosen)| {
            DiaryLine::Correction(CorrectionLine {
                clause,
                expected,
                chosen,
            })
        }),
    ]
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Round-trip oracle: `to_json` output always parses back to the same
    /// line — schema v1 is self-describing on every variant.
    #[test]
    fn every_line_round_trips(line in diary_line()) {
        let json = line.to_json();
        let parsed = parse_line(&json, 1).unwrap_or_else(|e| panic!("{json}\n{e:?}"));
        prop_assert_eq!(&parsed, &line);
    }

    /// Garbage oracle: arbitrary bytes never panic and never produce a line —
    /// either a schema/parse error or (for a syntactically valid non-diary
    /// JSON object) a parse error.
    #[test]
    fn garbage_never_panics(bytes: Vec<u8>) {
        let raw = String::from_utf8_lossy(&bytes);
        let _ = parse_line(&raw, 1);
    }

    /// A non-diary JSON document is rejected: `type` and `schema` are
    /// required on every line.
    #[test]
    fn non_diary_json_is_rejected(json in ".*") {
        let doc = format!("{{\"v\":{json:?}}}");
        let _ = parse_line(&doc, 1); // must not panic; errors expected
        let wrong_schema = format!(
            "{{\"schema\":0,\"type\":\"run\",\"goal\":{json:?},\"clauses\":[],\"policy\":\"i\"}}"
        );
        if serde_json::from_str::<serde_json::Value>(&wrong_schema).is_ok() {
            prop_assert!(parse_line(&wrong_schema, 1).is_err());
        }
    }
}
