//! `aui diff`: what changed between the last `aui snap` and the page now.
//!
//! Both sides are the text `aui snap` prints. A control is its role and
//! label (the n-th one with that pair, since labels repeat); its `@eN` ref
//! is left out of the comparison, because one new control shifts every ref
//! after it.

/// (role and label, state) of one `@eN role "label"  state` line.
fn control(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("@e")?.split_once(' ')?.1;
    Some(match rest.split_once("\"  ") {
        Some((named, state)) => (&rest[..=named.len()], state),
        None => (rest, ""),
    })
}

/// The n-th control of `lines` with each (role, label): (key, n, line).
fn controls(text: &str) -> Vec<((&str, usize), &str)> {
    let mut seen: Vec<(&str, usize)> = Vec::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((named, _)) = control(line) else {
            continue;
        };
        let nth = match seen.iter_mut().find(|(key, _)| *key == named) {
            Some((_, count)) => {
                *count += 1;
                *count
            }
            None => {
                seen.push((named, 0));
                0
            }
        };
        out.push(((named, nth), line));
    }
    out
}

/// The changes from snap `before` to snap `after`, one line each:
/// `+` a control that appeared (with its new ref), `-` one that went away,
/// `~` one whose state changed, `page:` a new title or URL. The last line
/// counts them and carries the new snapshot id.
pub fn diff_snaps(before: &str, after: &str) -> String {
    let (old, new) = (controls(before), controls(after));
    let mut out = String::new();
    let mut count = 0usize;
    let (was, is) = (before.lines().next(), after.lines().next());
    if was != is {
        out.push_str(&format!(
            "page: {} → {}\n",
            was.unwrap_or_default(),
            is.unwrap_or_default()
        ));
        count += 1;
    }
    for (key, line) in &new {
        match old.iter().find(|(other, _)| other == key) {
            None => out.push_str(&format!("+ {line}\n")),
            Some((_, earlier)) => {
                let (then, now) = (control(earlier), control(line));
                if then == now {
                    continue;
                }
                let state = |side: Option<(&str, &str)>| match side {
                    Some((_, "")) | None => "(no state)".to_owned(),
                    Some((_, state)) => state.to_owned(),
                };
                let head = line.split_once("\"  ").map_or(*line, |(head, _)| head);
                let quote = if head.ends_with('"') { "" } else { "\"" };
                out.push_str(&format!(
                    "~ {head}{quote}  {} → {}\n",
                    state(then),
                    state(now)
                ));
            }
        }
        count += 1;
    }
    for (key, line) in &old {
        if !new.iter().any(|(other, _)| other == key) {
            let named = control(line).map_or(*line, |(named, _)| named);
            out.push_str(&format!("- {named}\n"));
            count += 1;
        }
    }
    let snapshot = after
        .lines()
        .last()
        .and_then(|last| last.split_once("· ").map(|(_, id)| id))
        .unwrap_or("snapshot ?");
    match count {
        0 => format!("no changes · {snapshot}\n"),
        1 => format!("{out}1 change · {snapshot}\n"),
        n => format!("{out}{n} changes · {snapshot}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::diff_snaps;

    const BEFORE: &str = "Cart  https://shop.example/cart\n\
        @e1 link \"Home\"\n\
        @e2 checkbox \"Gift wrap\"  unchecked\n\
        @e3 button \"Remove\"\n\
        @e4 button \"Remove\"\n\
        @e5 textbox \"Coupon\"  value=\"\"\n\
        5 controls · snapshot 3";

    #[test]
    fn reports_appeared_disappeared_and_changed_controls() {
        let after = "Cart (1)  https://shop.example/cart\n\
            @e1 link \"Home\"\n\
            @e2 button \"Undo\"\n\
            @e3 checkbox \"Gift wrap\"  checked\n\
            @e4 button \"Remove\"\n\
            @e5 textbox \"Coupon\"  value=\"SAVE\"\n\
            5 controls · snapshot 4";
        assert_eq!(
            diff_snaps(BEFORE, after),
            "page: Cart  https://shop.example/cart → Cart (1)  https://shop.example/cart\n\
             + @e2 button \"Undo\"\n\
             ~ @e3 checkbox \"Gift wrap\"  unchecked → checked\n\
             ~ @e5 textbox \"Coupon\"  value=\"\" → value=\"SAVE\"\n\
             - button \"Remove\"\n\
             5 changes · snapshot 4\n"
        );
    }

    #[test]
    fn a_shifted_ref_alone_is_no_change() {
        let after = BEFORE.replace("snapshot 3", "snapshot 9");
        assert_eq!(diff_snaps(BEFORE, &after), "no changes · snapshot 9\n");
        let shifted = "Cart  https://shop.example/cart\n\
            @e1 button \"New\"\n\
            @e2 link \"Home\"\n\
            @e3 checkbox \"Gift wrap\"  unchecked\n\
            @e4 button \"Remove\"\n\
            @e5 button \"Remove\"\n\
            @e6 textbox \"Coupon\"  value=\"\"\n\
            6 controls · snapshot 4";
        assert_eq!(
            diff_snaps(BEFORE, shifted),
            "+ @e1 button \"New\"\n1 change · snapshot 4\n"
        );
    }

    #[test]
    fn state_that_comes_or_goes_is_a_change() {
        let before = "T  u\n@e1 button \"Menu\"\n1 control · snapshot 1";
        let after = "T  u\n@e1 button \"Menu\"  expanded\n1 control · snapshot 2";
        assert_eq!(
            diff_snaps(before, after),
            "~ @e1 button \"Menu\"  (no state) → expanded\n1 change · snapshot 2\n"
        );
    }
}
