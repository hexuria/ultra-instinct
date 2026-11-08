#[cfg(test)]
mod cli_execute {
    use std::path::Path;
    use ultra_instinct::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    fn fixture(name: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn usage_lists_the_product_commands_only() {
        let help = execute(&args(&["--help"])).unwrap();
        let first = help.lines().next().unwrap();
        assert_eq!(
            first,
            "ultra-instinct run|replay|lessons|exams|doctor|install|observe|guard|verify|mcp|skills|batch|session|open|snap|click|type|select|key|scroll|wait|shot|tabs|close|dialog|forward|reload|check|uncheck|focus|keyboard|get|is|count|state|cookies|storage|set|download|network"
        );
        assert!(
            help.contains("open <url> [--launch [--headed]] [--idle <secs>]")
                && help.contains("open --idle <secs> (1 to 86400, default 3600)"),
            "{help}"
        );
        for removed in ["locate", "inspect", "diff", "matcher", "hgra", "act is"] {
            assert!(!help.contains(removed), "{removed}: {help}");
        }
    }

    #[test]
    fn removed_commands_are_unknown() {
        for name in ["locate", "act", "inspect", "diff"] {
            let err =
                execute(&args(&[name, "--fixture", &fixture("sidebar.manifold")])).unwrap_err();
            assert_eq!(err, CliError::UnknownCommand(name.into()), "{name}");
        }
    }

    #[test]
    fn missing_source_is_a_specific_error() {
        let err = execute(&args(&["guard", "--target", "Settings"])).unwrap_err();
        assert_eq!(err, CliError::MissingSource);
        assert_eq!(
            err.to_string(),
            "command requires --fixture <path> or --cdp [url]"
        );
    }

    #[test]
    fn unknown_role_and_command_fail() {
        let err = execute(&args(&[
            "guard",
            "--fixture",
            "unused",
            "--target",
            "Save",
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
        let err = execute(&args(&["guard", "--fixture"])).unwrap_err();
        assert_eq!(err, CliError::MissingValue("--fixture"));
    }

    #[test]
    fn guard_allows_sign_in_on_a_cdp_fixture() {
        let path = fixture("sign-in.cdp.json");
        let stdout = execute(&args(&[
            "guard",
            "--target",
            "Sign in",
            "--role",
            "button",
            "--fixture",
            &path,
        ]))
        .unwrap();
        assert!(stdout.starts_with("allow\tn100\tSign in\t"), "{stdout}");
        let json = execute(&args(&[
            "guard",
            "--target",
            "Sign in",
            "--fixture",
            &path,
            "--json",
        ]))
        .unwrap();
        assert!(json.contains("\"decision\":\"allow\""), "{json}");
        assert!(json.contains("\"id\":\"n100\""), "{json}");
    }

    #[test]
    fn matcher_flags_are_gone() {
        let path = fixture("sidebar.manifold");
        for flag in ["--position", "--matcher", "--dims", "--near"] {
            let err = execute(&args(&[
                "guard",
                "--target",
                "Settings",
                "--fixture",
                &path,
                flag,
                "left",
            ]))
            .unwrap_err();
            assert_eq!(err, CliError::UnknownFlag(flag.into()), "{flag}");
        }
    }

    #[test]
    fn verify_missing_text_is_exact_and_welcome_succeeds() {
        let err = execute(&args(&[
            "verify",
            "--fixture",
            &fixture("sign-in.cdp.json"),
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
            &fixture("welcome.cdp.json"),
            "--expect-text",
            "Welcome",
        ]))
        .unwrap();
        assert_eq!(stdout, "verified\n");

        let err = execute(&args(&[
            "verify",
            "--fixture",
            &fixture("sign-in.cdp.json"),
            "--expect-absent",
            "n100",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::RegionStillPresent { id: "n100".into() });
        assert_eq!(err.to_string(), "region `n100` is still present");
    }

    #[test]
    fn empty_text_is_rejected() {
        let err = execute(&args(&[
            "guard",
            "--fixture",
            &fixture("sidebar.manifold"),
            "--target",
            "...",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::EmptyText);
        assert_eq!(
            err.to_string(),
            "text must contain at least one alphanumeric token"
        );
    }

    #[test]
    fn flag_parser_returns_exact_variants() {
        let path = fixture("sidebar.manifold");
        let err = execute(&args(&["guard", "--nope"])).unwrap_err();
        assert_eq!(err, CliError::UnknownFlag("--nope".into()));
        let err = execute(&args(&[
            "guard",
            "--target",
            "Settings",
            "--fixture",
            &path,
            "--json",
            "--json",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::DuplicateFlag("--json"));
        let err = execute(&args(&[
            "guard", "--target", "Settings", "--within", "a", "--within", "b",
        ]))
        .unwrap_err();
        assert_eq!(err, CliError::DuplicateFlag("--within"));
    }
}
