//! The page as one short line per control: `browser_get_state` with
//! `format: "text"` (the default).
//!
//! ```text
//! Sign up  https://shop.example/join
//! @e1 link "Log in"
//! @e2 textbox "Email"  value=""
//! @e3 textbox "Password"
//! @e4 checkbox "Send me news"  checked
//! @e5 button "Sign up"
//! 5 controls · snapshot 12
//! ```
//!
//! `@eN` is the same 1-based reading-order index every index-taking
//! `browser_*` tool accepts (`5`, `"5"`, or `"@e5"`). Control state is read
//! from what observe already recorded ([`InteractionRegion::state`]); it is
//! evidence only. Password, hidden, and file inputs never show a value
//! (observe never reads a password value in the first place).

use ultra_instinct_browser::PageState;
use ultra_instinct_core::{InteractionManifold, InteractionRegion, RegionId, Role};

/// Longest label or value printed, in characters; longer ones end in `…`.
const MAX_TEXT_CHARS: usize = 100;

/// Header (title and URL), one line per control in `order`, footer.
pub(crate) fn page_text(
    page: Option<&PageState>,
    manifold: &InteractionManifold,
    order: &[RegionId],
    snapshot: u64,
) -> String {
    let title = page
        .and_then(PageState::title)
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let url = page.and_then(PageState::url).filter(|u| !u.is_empty());
    let mut out = match (title, url) {
        (Some(title), Some(url)) => format!("{title}  {url}"),
        (Some(title), None) => title.to_owned(),
        (None, Some(url)) => url.to_owned(),
        (None, None) => "(untitled)".to_owned(),
    };
    out.push('\n');
    let mut count = 0usize;
    for (i, id) in order.iter().enumerate() {
        if let Some(region) = manifold.get(id) {
            out.push_str(&control_line(i + 1, region));
            out.push('\n');
            count += 1;
        }
    }
    let noun = if count == 1 { "control" } else { "controls" };
    out.push_str(&format!("{count} {noun} · snapshot {snapshot}"));
    out
}

/// `@e5 button "Sign up"  checked`: ref, role, quoted label, then state.
pub(crate) fn control_line(index: usize, region: &InteractionRegion) -> String {
    let mut line = format!(
        "@e{index} {} {}",
        role_word(region.role()),
        quoted(region.label())
    );
    let state = region.state();
    let secret = crate::secrets::sensitive(region);
    if let Some(value) = state.value.as_deref().filter(|_| !secret) {
        line.push_str(&format!("  value={}", quoted(value)));
    }
    match state.checked {
        Some(true) => line.push_str("  checked"),
        Some(false) => line.push_str("  unchecked"),
        None => {}
    }
    match state.expanded {
        Some(true) => line.push_str("  expanded"),
        Some(false) => line.push_str("  collapsed"),
        None => {}
    }
    if state.current == Some(true) {
        line.push_str("  selected");
    }
    if let Some(selected) = state.selected.as_deref() {
        line.push_str(&format!("  selected={}", quoted(selected)));
    }
    line
}

/// ARIA-style role words (`textbox`, `menuitem`); the rest as observed.
fn role_word(role: Role) -> &'static str {
    match role {
        Role::TextField => "textbox",
        Role::MenuItem => "menuitem",
        other => other.as_str(),
    }
}

/// A JSON string literal, cut to [`MAX_TEXT_CHARS`].
fn quoted(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = if text.chars().count() > MAX_TEXT_CHARS {
        let mut cut: String = text.chars().take(MAX_TEXT_CHARS).collect();
        cut.push('…');
        cut
    } else {
        text
    };
    serde_json::Value::String(cut).to_string()
}
