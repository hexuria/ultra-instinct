#[cfg(test)]
mod cli_execute {
    use std::path::Path;
    use ultra_instinct::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    fn fixture() -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/sidebar.manifold")
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn missing_fixture_is_a_specific_error() {
        let err = execute(&args(&["locate", "--text", "Settings"])).unwrap_err();
        assert_eq!(err, CliError::MissingFixture);
        assert_eq!(err.to_string(), "locate requires --fixture <path>");
    }

    #[test]
    fn unknown_role_and_command_fail() {
        let err = execute(&args(&[
            "locate",
            "--fixture",
            "unused",
            "--role",
            "spaceship",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::UnknownRole("spaceship".into()));
        let err = execute(&args(&["fly"])).unwrap_err();
        assert_eq!(err, CliError::UnknownCommand("fly".into()));
        let err = execute(&args(&["mcp"])).unwrap_err();
        assert_eq!(err, CliError::McpIsStdio);
        assert_eq!(
            err.to_string(),
            "mcp serves JSON-RPC on stdio; run the ultra-instinct binary"
        );
        let err = execute(&args(&["locate", "--fixture"])).unwrap_err();
        assert_eq!(err, CliError::MissingValue("--fixture"));
    }

    #[test]
    fn locate_json_ranks_sidebar_settings_first() {
        let path = fixture();
        let stdout = execute(&args(&[
            "locate",
            "--fixture",
            &path,
            "--text",
            "Settings",
            "--role",
            "button",
            "--position",
            "left",
            "--json",
        ]))
        .unwrap();
        let nav = stdout.find("\"id\": \"nav-settings\"").unwrap();
        let main = stdout.find("\"id\": \"main-settings\"").unwrap();
        assert!(nav < main, "{stdout}");
        assert!(stdout.contains("\"product\": \"ultra-instinct\""));
        assert!(stdout.contains("\"rank\": 1"));
        assert!(!stdout.contains("hgra"));
    }

    fn cdp_fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn locate_sign_in_on_cdp_fixture_ranks_the_button_first() {
        let path = cdp_fixture("sign-in.cdp.json");
        let stdout = execute(&args(&["locate", "Sign in", "--fixture", &path])).unwrap();
        let first = stdout.lines().next().unwrap();
        assert!(first.starts_with("1\tn100\t"), "{stdout}");
        assert!(first.contains("Sign in"));
        let json = execute(&args(&["locate", "Sign in", "--fixture", &path, "--json"])).unwrap();
        assert!(json.contains("\"matcher\": \"weighted\""));
        assert!(json.find("\"id\": \"n100\"").unwrap() < json.find("\"id\": \"n200\"").unwrap());
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn act_refuses_a_ranked_target_inside_the_margin() {
        // sign-in.cdp.json has no press steps, so a press would fail.
        let path = cdp_fixture("sign-in.cdp.json");
        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &path,
            "--confidence",
            "1.0",
            "--runner-up=0.98",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            CliError::AmbiguousTarget {
                top_millis: 1000,
                runner_up_millis: 980,
                margin_millis: 20,
                minimum_margin_millis: 50,
            }
        );
        let press = cdp_fixture("sign-in-press.cdp.json");
        let stdout = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &press,
            "--confidence",
            "1.0",
            "--runner-up",
            "0.5",
        ]))
        .unwrap();
        assert_eq!(stdout, "n100\tpress\tdom-semantic\texecuted\n");
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn runner_up_needs_confidence() {
        let path = cdp_fixture("sign-in-press.cdp.json");
        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &path,
            "--runner-up",
            "0.5",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::RunnerUpNeedsConfidence);
        assert_eq!(err.to_string(), "--runner-up requires --confidence");
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn act_press_records_dom_semantic_and_low_confidence_is_exact() {
        let path = cdp_fixture("sign-in-press.cdp.json");
        let stdout = execute(&args(&["act", "n100", "press", "--fixture", &path])).unwrap();
        assert_eq!(stdout, "n100\tpress\tdom-semantic\texecuted\n");
        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &path,
            "--confidence",
            "0.49",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            CliError::ConfidenceBelowThreshold {
                confidence_millis: 490,
                minimum_millis: 550,
            }
        );
        assert_eq!(
            err.to_string(),
            "confidence 490 is below the act minimum 550"
        );
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn browser_use_act_sends_semantics_and_low_confidence_does_not_execute() {
        let path = cdp_fixture("sign-in.browser-use.json");
        let stdout = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &path,
            "--executor",
            "browser-use",
            "--confidence",
            "0.55",
        ]))
        .unwrap();
        assert_eq!(stdout, "n100\tpress\tbrowser-use-semantic\texecuted\n");
        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &path,
            "--executor",
            "browser-use",
            "--confidence",
            "0.49",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            CliError::ConfidenceBelowThreshold {
                confidence_millis: 490,
                minimum_millis: 550,
            }
        );
        let rejected = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp_fixture("sign-in-reject.browser-use.json"),
            "--executor",
            "browser-use",
        ]))
        .unwrap_err();
        assert_eq!(
            rejected,
            CliError::BrowserUseRejected {
                message: "control refused the semantic act".into(),
            }
        );
        assert_eq!(
            rejected.to_string(),
            "browser-use rejected the semantic act: control refused the semantic act"
        );
        let macos = execute(&args(&["act", "n100", "press", "--executor", "macos"])).unwrap_err();
        assert_eq!(
            macos,
            CliError::NotImplemented {
                executor: "macos".into(),
            }
        );
        assert_eq!(macos.to_string(), "macos executor is not implemented");
        let cua = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp_fixture("sign-in.cua.json"),
            "--executor",
            "cua",
            "--confidence",
            "0.55",
        ]))
        .unwrap();
        assert_eq!(cua, "n100\tpress\tcua-semantic\texecuted\n");
        let low = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp_fixture("sign-in.cua.json"),
            "--executor",
            "cua",
            "--confidence",
            "0.49",
        ]))
        .unwrap_err();
        assert_eq!(
            low,
            CliError::ConfidenceBelowThreshold {
                confidence_millis: 490,
                minimum_millis: 550,
            }
        );
        let rejected = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp_fixture("sign-in-reject.cua.json"),
            "--executor",
            "cua",
        ]))
        .unwrap_err();
        assert_eq!(
            rejected,
            CliError::CuaRejected {
                message: "control refused the semantic act".into(),
            }
        );
        assert_eq!(
            rejected.to_string(),
            "cua rejected the semantic act: control refused the semantic act"
        );
        let replay = execute(&args(&[
            "act",
            "n100",
            "press",
            "--cdp",
            "--executor",
            "cua",
        ]))
        .unwrap_err();
        assert_eq!(replay, CliError::CuaIsReplay);
        assert_eq!(
            replay.to_string(),
            "cua act uses a replay fixture, not a live CDP endpoint"
        );
        let unknown = execute(&args(&[
            "act",
            "n100",
            "press",
            "--executor",
            "navigate",
            "--fixture",
            &path,
        ]))
        .unwrap_err();
        assert_eq!(unknown, CliError::UnknownExecutor("navigate".into()));
    }

    #[test]
    fn verify_missing_text_is_exact_and_welcome_succeeds() {
        let err = execute(&args(&[
            "verify",
            "--fixture",
            &cdp_fixture("sign-in.cdp.json"),
            "--expect-text",
            "Welcome",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            CliError::ExpectedTextMissing {
                expected: "Welcome".into(),
            }
        );
        assert_eq!(err.to_string(), "expected text `Welcome` did not appear");
        let stdout = execute(&args(&[
            "verify",
            "--fixture",
            &cdp_fixture("welcome.cdp.json"),
            "--expect-text",
            "Welcome",
        ]))
        .unwrap();
        assert_eq!(stdout, "verified\n");
    }

    #[test]
    fn diff_reports_removed_sign_in() {
        let stdout = execute(&args(&[
            "diff",
            "--before",
            &cdp_fixture("sign-in.cdp.json"),
            "--after",
            &cdp_fixture("welcome.cdp.json"),
        ]))
        .unwrap();
        assert!(stdout.contains("removed\tn100\n"), "{stdout}");
        assert!(stdout.contains("added\tn300\n"), "{stdout}");
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn hgra_matcher_still_ranks_sidebar_settings_first() {
        let path = fixture();
        let stdout = execute(&args(&[
            "locate",
            "--fixture",
            &path,
            "--text",
            "Settings",
            "--role",
            "button",
            "--position",
            "left",
            "--matcher",
            "hgra",
        ]))
        .unwrap();
        assert!(
            stdout.lines().next().unwrap().contains("nav-settings"),
            "{stdout}"
        );
    }

    #[test]
    fn empty_text_is_rejected() {
        let err =
            execute(&args(&["locate", "--fixture", &fixture(), "--text", "..."])).unwrap_err();
        assert_eq!(err, CliError::EmptyText);
        assert_eq!(
            err.to_string(),
            "locate text must contain at least one alphanumeric token"
        );
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn flag_parser_returns_exact_variants() {
        let err = execute(&args(&["locate", "--nope"])).unwrap_err();
        assert_eq!(err, CliError::UnknownFlag("--nope".into()));
        assert_eq!(err.to_string(), "unknown flag `--nope`");

        let err = execute(&args(&["locate", "--json", "--json"])).unwrap_err();
        assert_eq!(err, CliError::DuplicateFlag("--json"));
        assert_eq!(err.to_string(), "duplicate flag --json");

        let path = fixture();
        let err = execute(&args(&["locate", "--fixture", &path, "--matcher", "nope"])).unwrap_err();
        assert_eq!(err, CliError::UnknownMatcher("nope".into()));
        assert_eq!(err.to_string(), "unknown matcher `nope`");

        let err = execute(&args(&[
            "locate",
            "--fixture",
            &path,
            "--matcher",
            "hgra",
            "--dims",
            "7",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::BadDims("7".into()));
        assert_eq!(err.to_string(), "unsupported dims `7`");

        let err = execute(&args(&["act"])).unwrap_err();
        assert_eq!(err, CliError::MissingRegion);
        assert_eq!(err.to_string(), "act requires a region id");

        let err = execute(&args(&["act", "n100"])).unwrap_err();
        assert_eq!(err, CliError::MissingVerb);
        assert_eq!(err.to_string(), "act requires a verb (`press`)");
    }

    #[ignore = "actuation/HGRA removed in action-firewall pivot"]
    #[test]
    fn replay_scripts_and_verify_return_exact_variants() {
        let cdp = cdp_fixture("sign-in.cdp.json");
        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp,
            "--executor",
            "browser-use",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            CliError::BrowserUseScript {
                message: "unexpected field `calls`".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "invalid browser-use script: unexpected field `calls`"
        );

        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp,
            "--executor",
            "cua",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            CliError::CuaScript {
                message: "unexpected field `calls`".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "invalid cua script: unexpected field `calls`"
        );

        let err = execute(&args(&[
            "act",
            "n100",
            "press",
            "--fixture",
            &cdp_fixture("sign-in.browser-use.json"),
            "--executor",
            "browser-use",
            "--confidence",
            "NaN",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::NonFiniteConfidence);
        assert_eq!(err.to_string(), "confidence must be finite");

        let err = execute(&args(&[
            "verify",
            "--fixture",
            &cdp,
            "--expect-absent",
            "n100",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::RegionStillPresent { id: "n100".into() });
        assert_eq!(err.to_string(), "region `n100` is still present");
    }
}
