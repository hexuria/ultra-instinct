//! Host and label matching of the user policy (`ultra_instinct_guard::policy`).

use proptest::prelude::*;
use ultra_instinct_guard::{host_of, HostPattern, LabelGlob, Policy};
use ultra_instinct_protocol::GuardReason;

fn label() -> impl Strategy<Value = String> {
    "[A-Za-z0-9$]{1,8}( [A-Za-z0-9$]{1,8}){0,2}"
}

fn host() -> impl Strategy<Value = String> {
    proptest::collection::vec("[a-z0-9]{1,8}", 1..4).prop_map(|parts| parts.join("."))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// An exact pattern matches its host (any case, trailing dot) and nothing
    /// that only ends with it or adds a label in front.
    #[test]
    fn exact_host_matches_only_itself(h in host(), prefix in "[a-z0-9]{1,6}", upper: bool) {
        let pattern = HostPattern::parse(&h).unwrap();
        let shown = if upper { h.to_uppercase() } else { h.clone() };
        prop_assert!(pattern.matches(&shown));
        let dotted = format!("{h}.");
        prop_assert!(pattern.matches(&dotted));
        let subdomain = format!("{prefix}.{h}");
        let glued = format!("{prefix}{h}");
        prop_assert!(!pattern.matches(&subdomain));
        prop_assert!(!pattern.matches(&glued));
    }

    /// `*.h` matches every subdomain of `h`, never `h` itself and never a
    /// host that merely ends with the same letters.
    #[test]
    fn wildcard_matches_subdomains_only(h in host(), sub in host(), glue in "[a-z0-9]{1,6}") {
        let pattern = HostPattern::parse(&format!("*.{h}")).unwrap();
        let subdomain = format!("{sub}.{h}");
        let glued = format!("{glue}{h}");
        prop_assert!(pattern.matches(&subdomain));
        prop_assert!(!pattern.matches(&h));
        prop_assert!(!pattern.matches(&glued));
    }

    /// The host of a URL ignores scheme, userinfo, port, path, query, and a
    /// backslash; an allowlist answers on that host alone.
    #[test]
    fn url_host_is_the_authority_host(
        h in host(),
        evil in host(),
        port in proptest::option::of(1u16..65535),
        path in "[a-z0-9/]{0,12}",
    ) {
        let port = port.map(|p| format!(":{p}")).unwrap_or_default();
        let url = format!("https://user@{h}{port}/{path}?q=1#f");
        prop_assert_eq!(host_of(&url), Some(h.clone()));
        let policy = Policy::from_lists(&h, "").unwrap();
        prop_assert_eq!(policy.check_url(&url), Ok(()));
        if evil != h {
            // `evil` is the host; `h` only appears after the authority ends.
            for trick in [
                format!("https://{evil}/{h}"),
                format!("https://{evil}\\@{h}/"),
                format!("https://{h}@{evil}/"),
            ] {
                prop_assert_eq!(
                    policy.check_url(&trick),
                    Err(GuardReason::DomainNotAllowed),
                    "{}", trick
                );
            }
        }
    }

    /// A literal glob matches its label in any case and whitespace, and
    /// nothing longer; `*` in place of any slice still matches.
    #[test]
    fn glob_matches_its_label(l in label(), cut in 0usize..24, len in 0usize..24, tail in "[a-z]{1,4}") {
        let literal = LabelGlob::parse(&l).unwrap();
        prop_assert!(literal.matches(&l.to_uppercase()));
        let spaced = format!("  {}  ", l.replace(' ', "   "));
        prop_assert!(literal.matches(&spaced));
        let longer = format!("{l}{tail}");
        prop_assert!(!literal.matches(&longer));
        let chars: Vec<char> = l.chars().collect();
        let start = cut.min(chars.len());
        let end = (start + len).min(chars.len());
        let pattern: String = chars[..start]
            .iter()
            .chain(std::iter::once(&'*'))
            .chain(chars[end..].iter())
            .collect();
        if let Ok(glob) = LabelGlob::parse(&pattern) {
            prop_assert!(glob.matches(&l), "{} vs {}", pattern, l);
        }
        let any = LabelGlob::parse("*").unwrap();
        prop_assert!(any.matches(&l));
    }
}
