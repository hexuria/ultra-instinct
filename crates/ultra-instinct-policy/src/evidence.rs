//! Integer evidence for browser candidates. No float thresholds.

use instinct_core::Confidence;
use instinct_lexicon::overlap;
use instinct_text::{normalize, NormalizeConfig, Normalized};
use ultra_instinct_core::{tokenize, ActionKind, ObservedAction, Role};

/// Everything scoring needs from one goal string, computed once: whitespace
/// tokens, the whitespace-insensitive fold, instruction tokens (conditional
/// clauses stripped), and the Instinct-normalized form. Scoring used to redo
/// each of these — including the normalize — for every candidate action.
struct TextView<'a> {
    tokens: Vec<String>,
    folded: String,
    instruction: Vec<String>,
    normalized: Option<Normalized<'a>>,
}

impl<'a> TextView<'a> {
    fn of(text: &'a str) -> Self {
        Self {
            tokens: tokenize(text),
            folded: fold(text),
            instruction: instruction_tokens(text),
            normalized: normalize(text, NormalizeConfig::default()).ok(),
        }
    }
}

/// The goal plus its stripped target phrase ([`target_phrase`] when it
/// differs from the goal), shared by every candidate scored in one decide.
pub(crate) struct GoalView<'a> {
    goal: TextView<'a>,
    phrase: Option<TextView<'a>>,
}

impl<'a> GoalView<'a> {
    /// `phrase` must be `target_phrase(goal)` evaluated by the caller — the
    /// owned string has to outlive this view, so it cannot live inside it.
    pub(crate) fn of(goal: &'a str, phrase: Option<&'a str>) -> Self {
        Self {
            goal: TextView::of(goal),
            phrase: phrase.map(TextView::of),
        }
    }
}

/// Score how well `action` matches `goal`. Returns Instinct confidence millis 0..=1000.
///
/// Label evidence is the better of the whole goal and its *target phrase*
/// (goal minus quoted payload, leading operation verb, and the preposition
/// after them): in `Type "rust" into Search` the verb is operation evidence
/// and `"rust"` is the TextResolver's payload; only `Search` names a target.
///
/// Test-facing wrapper that builds its own view; the decide hot path uses
/// [`score_action_with`] on a shared [`GoalView`].
#[cfg(test)]
pub fn score_action(goal: &str, action: &ObservedAction) -> Confidence {
    let phrase = target_phrase(goal).filter(|p| *p != goal);
    let view = GoalView::of(goal, phrase.as_deref());
    score_action_with(&view, action)
}

pub(crate) fn score_action_with(view: &GoalView, action: &ObservedAction) -> Confidence {
    let best = score_action_best_with(view, action);
    // Only an exact label can saturate: "Skip navigation" contains the goal
    // "Skip" but must stay a margin below the button labelled "Skip".
    if best == Confidence::MAX && !exact_label_with(view, action) {
        return Confidence::saturating(i32::from(NON_EXACT_CAP));
    }
    best
}

/// Ceiling for a label that is not exactly the goal (or its target phrase):
/// Instinct Standard's margin below `MAX`, still above its min confidence.
const NON_EXACT_CAP: i16 = 849;

fn exact_label_with(view: &GoalView, action: &ObservedAction) -> bool {
    let label_folded = fold(action.label());
    view.goal.folded == label_folded
        || view
            .phrase
            .as_ref()
            .is_some_and(|p| p.folded == label_folded)
        || (action.kind().is_control()
            && (view.goal.folded == fold(action.kind().as_str())
                || control_keyword_hit_tokens(&view.goal.instruction, action.kind())))
}

fn score_action_best_with(view: &GoalView, action: &ObservedAction) -> Confidence {
    let whole = score_action_text_with(&view.goal, action);
    match &view.phrase {
        Some(phrase) => {
            let p = score_action_text_with(phrase, action);
            if p.get() > whole.get() {
                p
            } else {
                whole
            }
        }
        None => whole,
    }
}

/// Operation verbs stripped from the front of a goal for target evidence.
const LEADING_VERBS: &[&str] = &[
    "click", "press", "tap", "type", "enter", "fill", "input", "select", "choose", "pick", "open",
];

/// Connectives dropped right after a stripped verb or quoted payload.
const CONNECTIVES: &[&str] = &["into", "in", "on", "to", "the", "a", "an", "from", "as"];

/// Leading verbs whose quoted text names the target, not a payload:
/// `Click "Learn more"` clicks the link labelled Learn more.
const CLICK_VERBS: &[&str] = &["click", "press", "tap", "open"];

/// The goal with quoted payloads, a leading operation verb, and the
/// connectives that followed them removed. `None` when nothing is left.
/// After a click verb, quotes only delimit the target name and are unwrapped.
pub fn target_phrase(goal: &str) -> Option<String> {
    let names_target = goal
        .split_whitespace()
        .next()
        .is_some_and(|first| CLICK_VERBS.iter().any(|v| first.eq_ignore_ascii_case(v)));
    // Drop quoted segments ("…" or “…”), marking where they were.
    let mut unquoted = String::with_capacity(goal.len());
    let mut chars = goal.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\u{201c}' {
            let close = if c == '"' { '"' } else { '\u{201d}' };
            if names_target {
                unquoted.push(' ');
                for d in chars.by_ref() {
                    if d == close {
                        break;
                    }
                    unquoted.push(d);
                }
                unquoted.push(' ');
                continue;
            }
            for d in chars.by_ref() {
                if d == close {
                    break;
                }
            }
            unquoted.push_str(" \u{0} ");
        } else {
            unquoted.push(c);
        }
    }
    let mut words: Vec<&str> = unquoted.split_whitespace().collect();
    let mut stripped = false;
    if let Some(first) = words.first() {
        if LEADING_VERBS.iter().any(|v| first.eq_ignore_ascii_case(v)) {
            words.remove(0);
            stripped = true;
        }
    }
    let mut out = Vec::new();
    let mut after_marker = stripped;
    for w in words {
        if w == "\u{0}" {
            after_marker = true;
            continue;
        }
        if after_marker && CONNECTIVES.iter().any(|c| w.eq_ignore_ascii_case(c)) {
            continue;
        }
        after_marker = false;
        out.push(w);
    }
    let phrase = out.join(" ");
    if phrase.is_empty() {
        None
    } else {
        Some(phrase)
    }
}

fn score_action_text_with(goal: &TextView, action: &ObservedAction) -> Confidence {
    let label = action.label();
    let kind = action.kind();
    let label_folded = fold(label);

    if goal.folded == label_folded {
        return Confidence::MAX;
    }
    if kind.is_control() && goal.folded == fold(kind.as_str()) {
        return Confidence::MAX;
    }
    // Control verbs ("done", "wait", "scroll down") pick the control itself.
    // Operation verbs on target-bound kinds ("type", "select") are operation
    // evidence (see `score_operation`), not a cap on label evidence.
    if kind.is_control() && control_keyword_hit_tokens(&goal.instruction, kind) {
        return Confidence::saturating(920);
    }

    let mut score = Confidence::ZERO;

    if let Some(q) = goal.normalized.as_ref() {
        if let Ok(c) = normalize(label, NormalizeConfig::default()) {
            if let Ok(ov) = overlap(q, &c) {
                // Overlap measures how much of the label the goal holds. A short
                // label that names a sliver of the goal is scaled by goal coverage.
                let ov = if !sliver_scaled(kind) || covers_most_tokens(&goal.tokens, label) {
                    ov
                } else {
                    scale_by_coverage_tokens(ov, &goal.tokens, label)
                };
                score = score.saturating_add(ov);
            }
        }
    }

    let g = &goal.folded;
    // A label inside the goal is evidence only when it covers most of the
    // goal: a one-word chip ("kabisado") inside a long title goal is not.
    if !label_folded.is_empty()
        && g.contains(&label_folded)
        && (!sliver_scaled(kind) || covers_most_tokens(&goal.tokens, label))
    {
        score = score.saturating_add(Confidence::saturating(250));
    } else if !g.is_empty() && label_folded.contains(g.as_str()) {
        score = score.saturating_add(Confidence::saturating(200));
    }

    if !goal.tokens.is_empty() {
        let label_tokens = tokenize(label);
        let hits = goal
            .tokens
            .iter()
            .filter(|t| label_tokens.iter().any(|lt| lt == *t))
            .count();
        if hits == goal.tokens.len() && hits > 0 {
            score = score.saturating_add(Confidence::saturating(300));
        } else if hits > 0 {
            let part = i32::try_from(hits.saturating_mul(150) / goal.tokens.len()).unwrap_or(0);
            score = score.saturating_add(Confidence::saturating(part));
        }
    }

    if let Some(role) = action.role() {
        if role_mentioned(&goal.folded, role) {
            score = score.saturating_add(Confidence::saturating(80));
        }
    }

    if kind == ActionKind::Done && done_language_tokens(&goal.instruction) {
        score = score.saturating_add(Confidence::saturating(100));
    }

    score
}

/// Click targets are named by the whole target phrase, so a label that
/// names a sliver of it is scaled down. TYPE / SELECT goals may carry an
/// unquoted payload ("type rust ownership in the Search box") whose words
/// do not name the field, so their labels are not scaled.
fn sliver_scaled(kind: ActionKind) -> bool {
    kind == ActionKind::Click
}

/// `conf` times the fraction of `goal` tokens that occur in `label`.
fn scale_by_coverage_tokens(conf: Confidence, goal_tokens: &[String], label: &str) -> Confidence {
    if goal_tokens.is_empty() {
        return Confidence::ZERO;
    }
    let label_tokens = tokenize(label);
    let hits = goal_tokens
        .iter()
        .filter(|t| label_tokens.contains(t))
        .count();
    let scaled = i64::from(conf.get()) * i64::try_from(hits).unwrap_or(0)
        / i64::try_from(goal_tokens.len()).unwrap_or(1);
    Confidence::saturating(i32::try_from(scaled).unwrap_or(0))
}

/// At least two thirds of `goal`'s tokens occur in `label`.
fn covers_most_tokens(goal_tokens: &[String], label: &str) -> bool {
    if goal_tokens.is_empty() {
        return false;
    }
    let label_tokens = tokenize(label);
    let hits = goal_tokens
        .iter()
        .filter(|t| label_tokens.contains(t))
        .count();
    hits * 3 >= goal_tokens.len() * 2
}

fn covers_most(goal: &str, label: &str) -> bool {
    covers_most_tokens(&tokenize(goal), label)
}

/// The executed `label` names what the clause asked for: it covers at least
/// two thirds of the clause's target phrase (the clause itself when it has none).
/// The agent uses this before treating a step's effect as the clause done.
pub fn label_covers_target(clause: &str, label: &str) -> bool {
    let phrase = target_phrase(clause).unwrap_or_else(|| clause.to_owned());
    covers_most(&phrase, label)
}

/// `label` names the clause's target and little else: it covers two thirds
/// of the target phrase, and the phrase covers two thirds of the label.
/// "Skip" names "click Skip"; "Skip navigation" does not.
pub fn label_names_target(clause: &str, label: &str) -> bool {
    let phrase = target_phrase(clause).unwrap_or_else(|| clause.to_owned());
    covers_most(&phrase, label) && covers_most(label, &phrase)
}

/// Score an operation kind given the goal's memoized view and the best
/// target (if any) for that kind.
pub(crate) fn score_operation_with(
    view: &GoalView,
    kind: ActionKind,
    best_target: Option<&ObservedAction>,
) -> Confidence {
    let mut score = Confidence::ZERO;
    if control_keyword_hit_tokens(&view.goal.instruction, kind) {
        score = score.saturating_add(Confidence::saturating(920));
    }
    if view.goal.folded == fold(kind.as_str()) {
        return Confidence::MAX;
    }
    if let Some(target) = best_target {
        let t = score_action_with(view, target);
        if t.get() > score.get() {
            score = t;
        }
        // The goal names a different target-bound operation ("type … into
        // Search" offers CLICK and TYPE_TEXT on the same field): an unnamed
        // operation cannot outrank the named one on label evidence alone.
        let named = named_target_operations_tokens(&view.goal.instruction);
        if !named.is_empty() && !named.contains(&kind) {
            score = Confidence::saturating(i32::from(score.get().min(UNNAMED_OPERATION_CAP)));
        }
    } else if kind.is_control() {
        // Weak prior so controls remain choosable when goal is vague.
        score = score.saturating_add(Confidence::saturating(50));
    }
    score
}

#[cfg(test)]
fn eq_fold(a: &str, b: &str) -> bool {
    fold(a) == fold(b)
}

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn instruction_tokens(goal: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for clause in goal.split(['.', ';', '!', '?', ',', '\n']) {
        let clause_tokens = tokenize(clause);
        if matches!(
            clause_tokens.first().map(String::as_str),
            Some("if" | "unless" | "otherwise")
        ) {
            continue;
        }
        tokens.extend(clause_tokens);
    }
    tokens
}

fn matches_instruction_tokens(tokens: &[String], keys: &[&[&str]]) -> bool {
    keys.iter().any(|key| {
        tokens.windows(key.len()).any(|window| {
            window
                .iter()
                .zip(key.iter())
                .all(|(token, expected)| token.as_str() == *expected)
        })
    })
}

fn control_keyword_hit_tokens(tokens: &[String], kind: ActionKind) -> bool {
    let keys: &[&[&str]] = match kind {
        ActionKind::Done => &[&["done"], &["finished"], &["complete"], &["completed"]],
        ActionKind::Blocked => &[&["blocked"], &["stuck"], &["impossible"]],
        ActionKind::Wait => &[&["wait"], &["pause"], &["loading"]],
        ActionKind::ScrollDown => &[
            &["scroll", "down"],
            &["scroll", "page"],
            &["page", "down"],
            &["scrolldown"],
            &["pagedown"],
        ],
        ActionKind::ScrollUp => &[
            &["scroll", "up"],
            &["page", "up"],
            &["scrollup"],
            &["pageup"],
        ],
        ActionKind::Click => &[&["click"], &["press"], &["tap"]],
        ActionKind::TypeText => &[&["type"], &["enter"], &["fill"], &["input"]],
        ActionKind::Select => &[&["select"], &["choose"], &["pick"]],
        _ => &[],
    };
    matches_instruction_tokens(tokens, keys)
}

/// Test-facing goal-string form of [`control_keyword_hit_tokens`].
#[cfg(test)]
fn control_keyword_hit(goal: &str, kind: ActionKind) -> bool {
    control_keyword_hit_tokens(&instruction_tokens(goal), kind)
}

/// Score ceiling for a target-bound operation the goal did not name when it
/// named another one. Below Instinct Standard's min confidence.
const UNNAMED_OPERATION_CAP: i16 = 500;

/// Target-bound operations whose verb appears in the goal.
fn named_target_operations_tokens(tokens: &[String]) -> Vec<ActionKind> {
    [ActionKind::Click, ActionKind::TypeText, ActionKind::Select]
        .into_iter()
        .filter(|kind| control_keyword_hit_tokens(tokens, *kind))
        .collect()
}

fn role_mentioned(goal_folded: &str, role: Role) -> bool {
    let g = goal_folded;
    let key = match role {
        Role::Button => "button",
        Role::Link => "link",
        Role::TextField | Role::ComboBox => "text",
        Role::Checkbox => "checkbox",
        Role::MenuItem | Role::Option => "menu",
        Role::Tab => "tab",
        Role::Dialog => "dialog",
        Role::Navigation => "nav",
        Role::ListBox => "list",
        _ => return false,
    };
    g.contains(key)
}

/// Test-facing goal-string form of [`done_language_tokens`].
#[cfg(test)]
fn done_language(goal: &str) -> bool {
    done_language_tokens(&instruction_tokens(goal))
}

fn done_language_tokens(instruction: &[String]) -> bool {
    matches_instruction_tokens(
        instruction,
        &[
            &["done"],
            &["finish"],
            &["finished"],
            &["complete"],
            &["completed"],
            &["success"],
            &["successful"],
            &["submitted"],
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_phrase_strips_verb_payload_and_connectives() {
        assert_eq!(target_phrase("Click Go").as_deref(), Some("Go"));
        assert_eq!(
            target_phrase(r#"Click "Learn more""#).as_deref(),
            Some("Learn more")
        );
        assert_eq!(
            target_phrase(r#"Type "rust ownership" into Search"#).as_deref(),
            Some("Search")
        );
        assert_eq!(
            target_phrase(r#"Select "Business" in Cabin class"#).as_deref(),
            Some("Cabin class")
        );
        // No verb: connectives inside a label are kept.
        assert_eq!(target_phrase("Sign in").as_deref(), Some("Sign in"));
        assert_eq!(target_phrase("click").as_deref(), None);
    }

    #[test]
    fn fold_strips_case_and_space() {
        assert!(eq_fold("Sign In", "sign in"));
        assert!(!eq_fold("Sign In", "Sign Out"));
    }

    #[test]
    fn control_keywords_match_whole_token_sequences() {
        assert!(!control_keyword_hit(
            "Open the city center map",
            ActionKind::TypeText
        ));
        assert!(!control_keyword_hit("Express checkout", ActionKind::Click));
        assert!(control_keyword_hit(
            "Type \"man\" into City then click Manila",
            ActionKind::TypeText
        ));
        assert!(control_keyword_hit(
            "Type \"man\" into City then click Manila",
            ActionKind::Click
        ));
        assert!(control_keyword_hit(
            "Click Save. Say done when finished",
            ActionKind::Done
        ));
    }

    #[test]
    fn scroll_keywords_match_adjacent_tokens_and_compact_forms() {
        assert!(control_keyword_hit("scroll down", ActionKind::ScrollDown));
        assert!(control_keyword_hit(
            "Scroll  Down please",
            ActionKind::ScrollDown
        ));
        assert!(control_keyword_hit("page up", ActionKind::ScrollUp));
        assert!(control_keyword_hit("scrolldown", ActionKind::ScrollDown));
        assert!(control_keyword_hit("pagedown", ActionKind::ScrollDown));
        assert!(control_keyword_hit("pageup", ActionKind::ScrollUp));
        assert!(control_keyword_hit("scrollup", ActionKind::ScrollUp));
    }

    #[test]
    fn conditional_clauses_do_not_trigger_control_or_done_language() {
        let goal = "Star the email \"Weekly sync\". If the task cannot be done on this site, stop and say so (give up) without changing anything.";
        assert!(!control_keyword_hit(goal, ActionKind::Done));
        assert!(!done_language(goal));
        assert!(!control_keyword_hit(
            "If it is loading, click Retry",
            ActionKind::Wait
        ));
        assert!(control_keyword_hit(
            "If it is loading, click Retry",
            ActionKind::Click
        ));
    }
}
