//! The "what counts as a win" oracle (issue #49, open decision).
//!
//! A win is verified evidence, never a DONE choice:
//!
//! - **click** — verified effect (`Success` / `StateChanged` / `Navigation`)
//!   *and* the acted label names the clause's target
//!   ([`aui_policy::label_names_target`]);
//! - **type / select** — read-back `Success` (the observed value matches the
//!   payload);
//! - **scroll / wait / done / blocked** — never a win on their own; a
//!   `… while M` clause win is the marker leaving the screen, which is a
//!   clause-level fact owned by the loop, not this predicate.
//! - **human correction** — the strongest signal; recorded as a diary
//!   `correction` line by tools outside the loop, never written here.

use aui_core::ActionKind;
use aui_policy::label_names_target;

use crate::outcome::{StepRecord, VerificationKind};

/// Whether `record` counts as a verified win for `clause`.
#[must_use]
pub fn step_is_win(clause: &str, record: &StepRecord) -> bool {
    match record.kind {
        ActionKind::Click => {
            matches!(
                record.verification,
                VerificationKind::Success
                    | VerificationKind::StateChanged
                    | VerificationKind::Navigation
            ) && label_names_target(clause, &record.label)
        }
        ActionKind::TypeText | ActionKind::Select => {
            record.verification == VerificationKind::Success
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aui_core::ActionId;

    fn record(kind: ActionKind, label: &str, verification: VerificationKind) -> StepRecord {
        StepRecord {
            step: 1,
            action_id: ActionId::try_new(format!("{}:r1", kind.as_str())).unwrap(),
            kind,
            label: label.to_owned(),
            payload: None,
            verification,
            stale_retries: 0,
        }
    }

    #[test]
    fn click_needs_effect_and_named_target() {
        let win = record(ActionKind::Click, "Send", VerificationKind::Success);
        assert!(step_is_win("press send", &win));

        // Effect without the label naming the clause target is not a win.
        let unlabeled = record(ActionKind::Click, "OK", VerificationKind::Success);
        assert!(!step_is_win("press send", &unlabeled));

        // Named target without verified effect is not a win.
        let no_effect = record(ActionKind::Click, "Send", VerificationKind::NoVisibleChange);
        assert!(!step_is_win("press send", &no_effect));
    }

    #[test]
    fn type_and_select_win_on_read_back_only() {
        let typed = record(ActionKind::TypeText, "Message", VerificationKind::Success);
        assert!(step_is_win("type hello", &typed));
        let wrong = record(
            ActionKind::TypeText,
            "Message",
            VerificationKind::StateChanged,
        );
        assert!(!step_is_win("type hello", &wrong));
        let selected = record(ActionKind::Select, "Size", VerificationKind::Success);
        assert!(step_is_win("pick size", &selected));
    }

    #[test]
    fn controls_never_count_as_wins() {
        for kind in [
            ActionKind::ScrollDown,
            ActionKind::Wait,
            ActionKind::Done,
            ActionKind::Blocked,
        ] {
            let step = record(kind, "x", VerificationKind::Success);
            assert!(!step_is_win("anything", &step), "{kind:?}");
        }
    }
}
