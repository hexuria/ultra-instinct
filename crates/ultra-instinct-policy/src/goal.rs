//! Agent goal: consumer-owned natural-language intent.
//!
//! Multi-step goals may use a minimal sequential form ("type X then click Go").
//! See [`split_sequential_clauses`]. This is not an LLM planner: only plain
//! `then` / `and then` conjunctions outside quotes are split.

use std::fmt;

/// What the agent is trying to accomplish. Opaque to Instinct except as evidence text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentGoal {
    text: String,
}

impl AgentGoal {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into().trim().to_owned(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl fmt::Display for AgentGoal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Split a goal into sequential clauses on `then` / `and then` (case-insensitive).
///
/// Limits (documented, not aspirational):
/// - Only the literal connectives `then` and `and then` outside of `"..."` /
///   `'...'` quotes are split. No `after that`, commas, or numbered lists.
/// - No branching, conditionals, or loops.
/// - Each clause is still a single Instinct intent (one action → DONE).
/// - Nested quotes are not supported beyond simple open/close pairs.
///
/// A goal with no connective returns a single-element vec (the trimmed goal).
/// Empty clauses are dropped.
pub fn split_sequential_clauses(goal: &str) -> Vec<String> {
    let chars: Vec<char> = goal.chars().collect();
    let n = chars.len();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < n {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            i += 1;
            continue;
        }
        if let Some(len) = match_connective(&chars[i..]) {
            let clause: String = chars[start..i].iter().collect();
            let trimmed = clause.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_owned());
            }
            i += len;
            start = i;
            continue;
        }
        i += 1;
    }
    let tail: String = chars[start..].iter().collect();
    let trimmed = tail.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_owned());
    }
    if parts.is_empty() {
        let t = goal.trim();
        if t.is_empty() {
            Vec::new()
        } else {
            vec![t.to_owned()]
        }
    } else {
        parts
    }
}

/// Returns the length in chars of a leading connective including surrounding spaces.
fn match_connective(rest: &[char]) -> Option<usize> {
    let s: String = rest.iter().collect();
    let lower = s.to_ascii_lowercase();
    for (pat, raw_len) in [(" and then ", 10usize), (" then ", 6usize)] {
        if lower.starts_with(pat) {
            return Some(raw_len);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_then_and_then_outside_quotes() {
        assert_eq!(
            split_sequential_clauses(r#"Type "hello then world" into Search then click Go"#),
            vec![
                r#"Type "hello then world" into Search"#.to_owned(),
                "click Go".to_owned()
            ]
        );
        assert_eq!(
            split_sequential_clauses("type rust into Search and then click Go"),
            vec!["type rust into Search".to_owned(), "click Go".to_owned()]
        );
        assert_eq!(
            split_sequential_clauses("Click Go"),
            vec!["Click Go".to_owned()]
        );
        assert_eq!(
            split_sequential_clauses("a then b then c"),
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
    }
}
