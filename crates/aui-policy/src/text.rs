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
    /// Observed text that names the field when its label does not (its near
    /// text: "6 + 0 =", "Date:"). Evidence; not fingerprinted.
    pub field_context: Option<String>,
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
        let literals = value_literals(goal);
        // Every quoted literal names a control to press: the goal quotes
        // no value for this field.
        if literals.is_empty() && !quoted_literals(goal).is_empty() {
            return Err(TextError::Missing);
        }
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
    value_literals(context.goal.as_str())
        .into_iter()
        .map(|(_, literal)| literal)
        .find(|literal| !context.typed.contains(literal))
}

/// Verbs whose quoted object is a control to press, not a value to type
/// (`press "Submit"`, `click "Search"`).
const PRESS_VERBS: [&str; 5] = ["click", "hit", "press", "push", "tap"];

/// Quoted goal literals that are values: those not right after a press
/// verb. `type "Ana" then press "Submit"` has one value, "Ana".
fn value_literals(goal: &str) -> Vec<(usize, String)> {
    quoted_literals(goal)
        .into_iter()
        .filter(|(opening_quote, _)| {
            let before = &goal[..*opening_quote];
            ascii_words(before)
                .last()
                .is_none_or(|word| !PRESS_VERBS.contains(&word.as_str()))
        })
        .collect()
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

/// A resolved payload in the value format the field's `type` takes: a
/// date field holds `YYYY-MM-DD`, a time field `HH:MM` (24-hour), a month
/// field `YYYY-MM`. Its native value setter silently drops anything else
/// ("03/07/2016"), so the text is converted first. Text that does not parse
/// as that kind of value, and every other field, is returned unchanged.
pub fn form_value(input_type: Option<&str>, text: &str) -> String {
    let converted = match input_type {
        Some("date") => parse_date(text).map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
        Some("time") => parse_time(text).map(|(h, min)| format!("{h:02}:{min:02}")),
        Some("month") => parse_month(text).map(|(y, m)| format!("{y:04}-{m:02}")),
        _ => None,
    };
    converted.unwrap_or_else(|| text.to_owned())
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_named(word: &str) -> Option<u32> {
    let lower = word.to_ascii_lowercase();
    if lower.len() < 3 || !lower.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| lower.starts_with(m))
        .map(|i| i as u32 + 1)
}

fn date_parts(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

fn valid_date(y: u32, m: u32, d: u32) -> Option<(u32, u32, u32)> {
    ((1..=9999).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// `03/07/2016` (month first unless the first number cannot be a month),
/// `2016-03-07`, `March 7, 2016`, `7 Mar 2016`.
fn parse_date(text: &str) -> Option<(u32, u32, u32)> {
    let parts = date_parts(text);
    let numbers: Vec<(u32, usize)> = parts
        .iter()
        .filter_map(|p| Some((p.parse::<u32>().ok()?, p.len())))
        .collect();
    if let Some(month) = parts.iter().find_map(|p| month_named(p)) {
        let year = numbers.iter().find(|(_, len)| *len == 4)?.0;
        let day = numbers.iter().find(|(_, len)| *len <= 2)?.0;
        return valid_date(year, month, day);
    }
    let [(a, la), (b, _), (c, lc)] = numbers.as_slice() else {
        return None;
    };
    if *la == 4 {
        valid_date(*a, *b, *c)
    } else if *lc == 4 && *a > 12 {
        valid_date(*c, *b, *a)
    } else if *lc == 4 {
        valid_date(*c, *a, *b)
    } else {
        None
    }
}

/// `8:16 PM`, `08:16`, `20:16`, `8pm`, `12 AM`.
fn parse_time(text: &str) -> Option<(u32, u32)> {
    let lower = text.to_ascii_lowercase();
    let pm = lower.contains("pm") || lower.contains("p.m");
    let am = lower.contains("am") || lower.contains("a.m");
    let numbers: Vec<u32> = lower
        .split(|c: char| !c.is_ascii_digit())
        .filter(|w| !w.is_empty())
        .filter_map(|w| w.parse().ok())
        .collect();
    let (&hour, rest) = numbers.split_first()?;
    let minute = rest.first().copied().unwrap_or(0);
    let hour = match (am, pm) {
        (_, true) if (1..=11).contains(&hour) => hour + 12,
        (true, _) if hour == 12 => 0,
        (true, _) | (_, true) if !(1..=12).contains(&hour) => return None,
        _ => hour,
    };
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// `03/2016`, `2016-03`, `March 2016`.
fn parse_month(text: &str) -> Option<(u32, u32)> {
    let parts = date_parts(text);
    let year = parts
        .iter()
        .find(|p| p.len() == 4)
        .and_then(|p| p.parse::<u32>().ok())?;
    let month = parts.iter().find_map(|p| month_named(p)).or_else(|| {
        parts
            .iter()
            .filter(|p| p.len() <= 2)
            .find_map(|p| p.parse::<u32>().ok())
    })?;
    (1..=12).contains(&month).then_some((year, month))
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
            field_context: None,
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
            field_context: None,
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
            field_context: None,
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
            field_context: None,
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
            field_context: None,
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
    fn date_time_and_month_fields_get_their_own_value_format() {
        let date = |t| form_value(Some("date"), t);
        assert_eq!(date("03/07/2016"), "2016-03-07", "month first");
        assert_eq!(date("2016-03-07"), "2016-03-07");
        assert_eq!(
            date("25/12/2020"),
            "2020-12-25",
            "a first number above 12 is the day"
        );
        assert_eq!(date("March 7, 2016"), "2016-03-07");
        assert_eq!(date("7 Mar 2016"), "2016-03-07");
        assert_eq!(date("13/13/2016"), "13/13/2016", "not a date: unchanged");
        let time = |t| form_value(Some("time"), t);
        assert_eq!(time("8:16 PM"), "20:16");
        assert_eq!(time("08:05"), "08:05");
        assert_eq!(time("12 AM"), "00:00");
        assert_eq!(time("12:30 pm"), "12:30");
        assert_eq!(time("7pm"), "19:00");
        assert_eq!(time("25:00"), "25:00", "not a time: unchanged");
        assert_eq!(form_value(Some("month"), "March 2016"), "2016-03");
        assert_eq!(form_value(Some("month"), "03/2016"), "2016-03");
        assert_eq!(
            form_value(Some("text"), "03/07/2016"),
            "03/07/2016",
            "other fields: unchanged"
        );
        assert_eq!(form_value(None, "8:16 PM"), "8:16 PM");
    }

    #[test]
    fn a_quoted_control_to_press_is_not_a_value() {
        assert_eq!(
            resolve(r#"Type "Ana" into Name and press "Submit""#, "Name", &[]).unwrap(),
            "Ana"
        );
        assert_eq!(
            resolve(
                r#"Type "Ana" into Name and press "Submit""#,
                "Name",
                &["Ana"]
            )
            .unwrap_err(),
            TextError::Abstain("no quoted literal left for this field".into())
        );
        assert_eq!(
            resolve(
                r#"Find the 1st word, type that into the textbox and press "Submit"."#,
                "",
                &[]
            )
            .unwrap_err(),
            TextError::Missing,
            "only a control is quoted"
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
            field_context: None,
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
