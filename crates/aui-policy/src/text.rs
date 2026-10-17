//! TextResolver: TYPE_TEXT payload generation is *not* Instinct.

use std::fmt;

use crate::goal::AgentGoal;

/// Context for resolving typed text. Fingerprint so stale resolves are dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextContext {
    pub goal: AgentGoal,
    pub field_label: String,
    pub field_role: String,
    /// Payloads successfully typed earlier in this run; not fingerprinted.
    pub typed: Vec<String>,
    /// Observation / action-space fingerprint bits the resolve was based on.
    pub context_fingerprint: u64,
    /// The page's visible text, only when the resolver asked for it
    /// ([`TextResolver::wants_page_text`]). Untrusted data; not fingerprinted.
    pub page_text: Option<String>,
    /// The field's INPUT `type`, lowercased (`password`, `email`, …).
    pub input_type: Option<String>,
}

impl TextContext {
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for b in self.goal.as_str().as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for b in self.field_label.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= self.context_fingerprint;
        hash = hash.wrapping_mul(0x100000001b3);
        hash
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextResolution {
    pub text: String,
    pub context_fingerprint: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextError {
    Ambiguous,
    Missing,
    Invalid(String),
    /// The resolver declined to produce a value (e.g. a model reply was
    /// refused and no fallback answered). The agent abstains; nothing is typed.
    Abstain(String),
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ambiguous => f.write_str("text resolution ambiguous"),
            Self::Missing => f.write_str("no text value found in goal/context"),
            Self::Invalid(msg) => write!(f, "invalid text: {msg}"),
            Self::Abstain(msg) => write!(f, "text resolver abstained: {msg}"),
        }
    }
}

impl std::error::Error for TextError {}

pub trait TextResolver {
    fn resolve(&mut self, context: &TextContext) -> Result<TextResolution, TextError>;

    /// Whether [`TextContext::page_text`] should be filled. Default: no.
    fn wants_page_text(&self) -> bool {
        false
    }
}

/// Label words that mark a secret field.
const SECRET_LABEL_WORDS: [&str; 10] = [
    "password", "passcode", "pin", "cvv", "cvc", "otp", "ssn", "card", "security", "one-time",
];

/// Whether a field holds a secret: a password / hidden / file input, or a
/// label naming a password, PIN, CVV, card, or one-time code. A derived value
/// never goes into one, and a value typed into one is masked in progress.
pub fn is_secret_field(input_type: Option<&str>, label: &str) -> bool {
    matches!(input_type, Some("password" | "hidden" | "file"))
        || label
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '-')
            .any(|word| SECRET_LABEL_WORDS.contains(&word))
}

/// Ground a SELECT payload in the observed enabled options.
pub fn ground_select(
    goal: &str,
    resolved: Option<&str>,
    options: &[String],
    selected: Option<&str>,
) -> Result<String, TextError> {
    if options.is_empty() {
        return resolved.map(str::to_owned).ok_or(TextError::Missing);
    }

    if let Some(resolved) = resolved {
        let resolved = resolved.trim().to_lowercase();
        let mut matches = options
            .iter()
            .filter(|option| option.to_lowercase() == resolved);
        if let Some(option) = matches.next() {
            if matches.next().is_some() {
                return Err(TextError::Ambiguous);
            }
            return Ok(option.clone());
        }
    }

    let lower_goal = goal.to_lowercase();
    let candidates: Vec<_> = options
        .iter()
        .filter_map(|option| {
            if option.trim().is_empty() {
                return None;
            }
            let lower = option.to_lowercase();
            contains_whole_phrase(&lower_goal, &lower).then_some((option.as_str(), lower))
        })
        .collect();
    let mut candidates: Vec<_> = candidates
        .iter()
        .filter(|(_, label)| {
            !candidates
                .iter()
                .any(|(_, other)| other != label && other.contains(label.as_str()))
        })
        .map(|(label, _)| *label)
        .collect();

    if candidates.len() > 1 {
        if let Some(selected) = selected {
            let selected = selected.to_lowercase();
            candidates.retain(|label| label.to_lowercase() != selected);
        }
    }

    match candidates.as_slice() {
        [option] => Ok((*option).to_owned()),
        [] => Err(TextError::Abstain(
            "no observed option matches the goal".into(),
        )),
        _ => Err(TextError::Ambiguous),
    }
}

fn contains_whole_phrase(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(start, found)| {
        let end = start + found.len();
        text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric())
            && text[end..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_alphanumeric())
    })
}

/// Deterministically extracts a field's text payload from the goal. No model.
#[derive(Clone, Debug, Default)]
pub struct DeterministicTextResolver;

impl TextResolver for DeterministicTextResolver {
    fn resolve(&mut self, context: &TextContext) -> Result<TextResolution, TextError> {
        let goal = context.goal.as_str();
        let literals = quoted_literals(goal);
        if !literals.is_empty() {
            let untyped: Vec<_> = literals
                .into_iter()
                .filter(|(_, literal)| !context.typed.contains(literal))
                .collect();
            if untyped.is_empty() {
                return Err(TextError::Abstain(
                    "no quoted literal left for this field".into(),
                ));
            }

            let label_words = ascii_words(&context.field_label);
            let selected = untyped
                .iter()
                .find(|(opening_quote, _)| {
                    let before = &goal[..*opening_quote];
                    let start = before
                        .char_indices()
                        .rev()
                        .nth(31)
                        .map_or(0, |(index, _)| index);
                    ascii_words(&before[start..])
                        .iter()
                        .rev()
                        .take(3)
                        .any(|word| label_words.contains(word))
                })
                .unwrap_or(&untyped[0]);
            return Ok(TextResolution {
                text: selected.1.clone(),
                context_fingerprint: context.fingerprint(),
            });
        }

        if let Some(quoted) = first_quoted(goal) {
            if quoted.is_empty() {
                return Err(TextError::Missing);
            }
            return Ok(TextResolution {
                text: quoted,
                context_fingerprint: context.fingerprint(),
            });
        }
        // Pattern: type <value> into <field>
        if let Some(v) = type_into_pattern(goal, &context.field_label) {
            return Ok(TextResolution {
                text: v,
                context_fingerprint: context.fingerprint(),
            });
        }
        // Pattern: fill <field> with <value>
        if let Some(v) = fill_with_pattern(goal, &context.field_label) {
            return Ok(TextResolution {
                text: v,
                context_fingerprint: context.fingerprint(),
            });
        }
        Err(TextError::Missing)
    }
}

/// The first double-quoted value in the goal not typed yet: an explicit
/// value the user gave, which beats anything read off the page.
#[cfg(feature = "model-text")]
pub(crate) fn untyped_quoted_value(context: &TextContext) -> Option<String> {
    quoted_literals(context.goal.as_str())
        .into_iter()
        .map(|(_, literal)| literal)
        .find(|literal| !context.typed.contains(literal))
}

fn quoted_literals(goal: &str) -> Vec<(usize, String)> {
    let mut literals = Vec::new();
    let mut cursor = 0;
    while cursor < goal.len() {
        let Some((offset, opening)) = goal[cursor..]
            .char_indices()
            .find(|(_, character)| matches!(*character, '"' | '“'))
        else {
            break;
        };
        let opening_quote = cursor + offset;
        let (closing, content_start) = match opening {
            '"' => ('"', opening_quote + 1),
            '“' => ('”', opening_quote + opening.len_utf8()),
            _ => unreachable!(),
        };
        let Some((offset, _)) = goal[content_start..]
            .char_indices()
            .find(|(_, character)| *character == closing)
        else {
            cursor = content_start;
            continue;
        };
        let content_end = content_start + offset;
        if content_end == content_start {
            cursor = content_end + closing.len_utf8();
            continue;
        }
        literals.push((opening_quote, goal[content_start..content_end].to_owned()));
        cursor = content_end + closing.len_utf8();
    }
    literals
}

fn ascii_words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != quote {
                i += 1;
            }
            if i < bytes.len() {
                return Some(s[start..i].to_owned());
            }
            return None;
        }
        i += 1;
    }
    None
}

fn type_into_pattern(goal: &str, field: &str) -> Option<String> {
    let lower = goal.to_ascii_lowercase();
    let field_l = field.to_ascii_lowercase();
    // "type VALUE into FIELD"
    let type_idx = lower.find("type ")?;
    let into_idx = lower.find(" into ")?;
    // "type into X" has no value between the verb and " into ".
    if into_idx < type_idx + 5 {
        return None;
    }
    let value = goal[type_idx + 5..into_idx].trim();
    let after = &lower[into_idx + 6..];
    if !after.contains(&field_l) && !field_l.is_empty() {
        // Still allow if field label empty / generic.
        if !field.is_empty() {
            return None;
        }
    }
    if value.is_empty() {
        return None;
    }
    Some(value.trim_matches(|c| c == '"' || c == '\'').to_owned())
}

fn fill_with_pattern(goal: &str, field: &str) -> Option<String> {
    let lower = goal.to_ascii_lowercase();
    let field_l = field.to_ascii_lowercase();
    if !field_l.is_empty() && !lower.contains(&field_l) {
        return None;
    }
    let with_idx = lower.find(" with ")?;
    let value = goal[with_idx + 6..].trim();
    if value.is_empty() {
        return None;
    }
    Some(value.trim_matches(|c| c == '"' || c == '\'').to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn secret_fields_by_type_or_label_word() {
        use super::is_secret_field;
        assert!(is_secret_field(Some("password"), "Anything"));
        assert!(is_secret_field(Some("text"), "Card number"));
        assert!(is_secret_field(None, "One-time code"));
        assert!(is_secret_field(None, "PIN"));
        assert!(!is_secret_field(Some("text"), "Username"));
        assert!(!is_secret_field(None, "Spinner"), "whole words only");
    }

    use super::*;

    #[test]
    fn quoted_value() {
        let mut r = DeterministicTextResolver;
        let ctx = TextContext {
            goal: AgentGoal::new(r#"Type "Zürich" into City"#),
            field_label: "City".into(),
            field_role: "text_field".into(),
            typed: Vec::new(),
            context_fingerprint: 1,
            page_text: None,
            input_type: None,
        };
        let res = r.resolve(&ctx).unwrap();
        assert_eq!(res.text, "Zürich");
    }

    #[test]
    fn type_into_pattern_works() {
        let mut r = DeterministicTextResolver;
        let ctx = TextContext {
            goal: AgentGoal::new("type hello world into Search"),
            field_label: "Search".into(),
            field_role: "text_field".into(),
            typed: Vec::new(),
            context_fingerprint: 2,
            page_text: None,
            input_type: None,
        };
        assert_eq!(r.resolve(&ctx).unwrap().text, "hello world");
    }

    #[test]
    fn type_into_without_value_is_missing_not_panic() {
        let mut r = DeterministicTextResolver;
        let ctx = TextContext {
            goal: AgentGoal::new("type into Search"),
            field_label: "Search".into(),
            field_role: "text_field".into(),
            typed: Vec::new(),
            context_fingerprint: 4,
            page_text: None,
            input_type: None,
        };
        assert_eq!(r.resolve(&ctx), Err(TextError::Missing));
    }

    #[test]
    fn missing_when_no_value() {
        let mut r = DeterministicTextResolver;
        let ctx = TextContext {
            goal: AgentGoal::new("click Submit"),
            field_label: "Email".into(),
            field_role: "text_field".into(),
            typed: Vec::new(),
            context_fingerprint: 3,
            page_text: None,
            input_type: None,
        };
        assert_eq!(r.resolve(&ctx), Err(TextError::Missing));
    }

    fn resolve(goal: &str, field_label: &str, typed: &[&str]) -> Result<String, TextError> {
        let context = TextContext {
            goal: AgentGoal::new(goal),
            field_label: field_label.into(),
            field_role: "text_field".into(),
            typed: typed.iter().map(|text| (*text).to_owned()).collect(),
            context_fingerprint: 5,
            page_text: None,
            input_type: None,
        };
        DeterministicTextResolver
            .resolve(&context)
            .map(|resolution| resolution.text)
    }

    #[test]
    fn compose_literals_follow_field_context_and_typed_history() {
        let goal = r#"Write a new email to "dana@acme.test" with the subject "Offsite budget" and the body "Draft numbers attached." and send it."#;
        assert_eq!(
            resolve(goal, "To recipients", &[]).unwrap(),
            "dana@acme.test"
        );
        assert_eq!(
            resolve(goal, "Subject", &["dana@acme.test"]).unwrap(),
            "Offsite budget"
        );
        assert_eq!(resolve(goal, "Subject", &[]).unwrap(), "Offsite budget");
        assert_eq!(
            resolve(goal, "Message", &["dana@acme.test", "Offsite budget"]).unwrap(),
            "Draft numbers attached."
        );
    }

    #[test]
    fn checkout_literals_follow_field_context_and_typed_history() {
        let goal = r#"Place the order in checkout. Ship to "Ana Santos", street "12 Mabini St", city "Quezon City", ZIP code "1100", with standard shipping, and accept the terms."#;
        assert_eq!(resolve(goal, "Full name", &[]).unwrap(), "Ana Santos");
        assert_eq!(
            resolve(goal, "Street address", &["Ana Santos"]).unwrap(),
            "12 Mabini St"
        );
        assert_eq!(
            resolve(goal, "City", &["Ana Santos", "12 Mabini St"]).unwrap(),
            "Quezon City"
        );
        assert_eq!(
            resolve(
                goal,
                "ZIP code",
                &["Ana Santos", "12 Mabini St", "Quezon City"]
            )
            .unwrap(),
            "1100"
        );
    }

    #[test]
    fn resolver_abstains_when_every_quoted_literal_was_typed() {
        let goal = r#"Set "first" then "second"."#;
        assert_eq!(
            resolve(goal, "Field", &["first", "second"]),
            Err(TextError::Abstain(
                "no quoted literal left for this field".into()
            ))
        );
    }

    #[test]
    fn resolver_supports_curly_quotes_and_multibyte_context() {
        assert_eq!(
            resolve("Type “Zürich” into City", "City", &[]).unwrap(),
            "Zürich"
        );
        assert_eq!(
            resolve("Ünïcödé façade naïve field \"x\"", "field", &[]).unwrap(),
            "x"
        );
    }

    #[test]
    fn empty_double_quotes_are_skipped_and_single_quotes_keep_the_old_path() {
        assert_eq!(
            resolve(r#"Type "" then "hello" into Search"#, "Search", &[]).unwrap(),
            "hello"
        );
        assert_eq!(
            resolve("Type 'hello' into Search", "Search", &[]).unwrap(),
            "hello"
        );
    }

    #[test]
    fn typed_history_does_not_change_the_context_fingerprint() {
        let mut context = TextContext {
            goal: AgentGoal::new(r#"Type "hello" into Search"#),
            field_label: "Search".into(),
            field_role: "text_field".into(),
            typed: Vec::new(),
            context_fingerprint: 9,
            page_text: None,
            input_type: None,
        };
        let fingerprint = context.fingerprint();
        context.typed.push("already typed".into());
        assert_eq!(context.fingerprint(), fingerprint);
    }

    #[test]
    fn select_without_observed_options_keeps_resolver_fallback() {
        assert_eq!(
            ground_select("Set cabin class", Some(" business "), &[], None).unwrap(),
            " business "
        );
        assert_eq!(
            ground_select("Set cabin class", None, &[], None),
            Err(TextError::Missing)
        );
    }

    #[test]
    fn select_resolved_value_matches_observed_option_case_insensitively() {
        let options = vec!["Economy".into(), "Business".into()];
        assert_eq!(
            ground_select("Set cabin class", Some("business"), &options, None).unwrap(),
            "Business"
        );
    }

    #[test]
    fn select_goal_fallback_uses_observed_option() {
        let options = vec!["Economy".into(), "Business".into()];
        assert_eq!(
            ground_select("Set cabin class to Business", Some("First"), &options, None).unwrap(),
            "Business"
        );
    }

    #[test]
    fn select_goal_prefers_longest_matching_option_label() {
        let options = vec!["Asia".into(), "Asia/Manila".into()];
        assert_eq!(
            ground_select("Set time zone to Asia/Manila", None, &options, None).unwrap(),
            "Asia/Manila"
        );
    }

    #[test]
    fn select_goal_ignores_current_selection_when_disambiguating() {
        let options = vec!["UTC".into(), "Asia/Manila".into()];
        assert_eq!(
            ground_select(
                "Change from UTC to Asia/Manila",
                None,
                &options,
                Some("UTC")
            )
            .unwrap(),
            "Asia/Manila"
        );
    }

    #[test]
    fn select_goal_requires_alphanumeric_phrase_boundaries() {
        let options = vec!["UTC".into()];
        assert!(matches!(
            ground_select("Set zone to UTCx", None, &options, None),
            Err(TextError::Abstain(_))
        ));
    }

    #[test]
    fn select_goal_with_multiple_options_is_ambiguous() {
        let options = vec!["UTC".into(), "Asia/Manila".into()];
        assert_eq!(
            ground_select("Change from UTC to Asia/Manila", None, &options, None),
            Err(TextError::Ambiguous)
        );
    }

    #[test]
    fn select_goal_with_no_matching_option_abstains() {
        let options = vec!["Economy".into(), "Business".into()];
        assert!(matches!(
            ground_select("Set cabin class to Premium", None, &options, None),
            Err(TextError::Abstain(_))
        ));
    }

    #[test]
    fn duplicate_resolved_option_labels_are_ambiguous() {
        let options = vec!["Business".into(), "Business".into()];
        assert_eq!(
            ground_select(
                "Set cabin class to Business",
                Some("business"),
                &options,
                None
            ),
            Err(TextError::Ambiguous)
        );
    }
}
