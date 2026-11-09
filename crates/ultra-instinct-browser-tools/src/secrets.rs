//! Secrets never leave the tools, failing closed.
//!
//! A field is [`sensitive`] when it is a password / hidden / file input, a
//! field whose label names a secret (AGENTS.md 7a: PIN, CVV, card, one-time
//! code, …; `ultra_instinct_policy::is_secret_field`), or a text field whose
//! input type observe could not read. A sensitive field's value is never
//! printed (`browser_get_state`, `browser_get`), and text typed into one is
//! remembered ([`Secrets`]) and replaced by [`MASK`] in every later tool
//! output and error, the HAR file included, so a page that echoes it (a
//! console log, a GET form's URL, a confirmation line) does not leak it.
//! `browser_get_html` drops `value` attributes of text-like inputs.

use serde_json::Value;
use ultra_instinct_core::{Action, InteractionRegion};

use crate::error::ToolError;

/// What a remembered secret reads as in tool output.
pub const MASK: &str = "••••";

/// Shortest typed secret that is scrubbed: shorter text would blank out
/// ordinary words.
const MIN_SECRET_CHARS: usize = 4;

/// Typed secrets kept for scrubbing; the oldest drop past this.
const MAX_SECRETS: usize = 32;

/// Whether `region` holds a secret, failing closed.
pub fn sensitive(region: &InteractionRegion) -> bool {
    let state = region.state();
    ultra_instinct_policy::is_secret_field(state.input_type.as_deref(), region.label())
        // A text field whose type and value observe could not read (the
        // per-node fallback, a content-editable box): treat as secret.
        || (region.actions().contains(&Action::Type)
            && state.input_type.is_none()
            && state.value.is_none())
}

/// Values typed into sensitive fields during this server's life.
#[derive(Default)]
pub(crate) struct Secrets {
    typed: Vec<String>,
}

impl Secrets {
    /// Remember `text` (typed into a sensitive field) for scrubbing.
    pub(crate) fn remember(&mut self, text: &str) {
        if text.chars().count() < MIN_SECRET_CHARS || self.typed.iter().any(|t| t == text) {
            return;
        }
        if self.typed.len() >= MAX_SECRETS {
            self.typed.remove(0);
        }
        self.typed.push(text.to_owned());
    }

    /// `text` with every remembered secret masked (longest first).
    pub(crate) fn scrub_str(&self, text: &str) -> Option<String> {
        let mut hits: Vec<&String> = self
            .typed
            .iter()
            .filter(|s| text.contains(s.as_str()))
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut out = text.to_owned();
        for secret in hits {
            out = out.replace(secret.as_str(), MASK);
        }
        Some(out)
    }

    /// Mask secrets in every string of `value` but binary fields (`base64`,
    /// `data`: a screenshot or PDF).
    pub(crate) fn scrub_value(&self, value: &mut Value) {
        if self.typed.is_empty() {
            return;
        }
        match value {
            Value::String(text) => {
                if let Some(clean) = self.scrub_str(text) {
                    *text = clean;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.scrub_value(v)),
            Value::Object(map) => {
                for (key, v) in map.iter_mut() {
                    if !matches!(key.as_str(), "base64" | "data") {
                        self.scrub_value(v);
                    }
                }
            }
            _ => {}
        }
    }

    /// `err` with secrets masked in its free text.
    pub(crate) fn scrub_error(&self, err: ToolError) -> ToolError {
        if self.typed.is_empty() {
            return err;
        }
        let fix = |text: String| self.scrub_str(&text).unwrap_or(text);
        match err {
            ToolError::Browser(m) => ToolError::Browser(fix(m)),
            ToolError::InvalidArguments(m) => ToolError::InvalidArguments(fix(m)),
            ToolError::TicketInvalid(m) => ToolError::TicketInvalid(fix(m)),
            ToolError::Fixture(m) => ToolError::Fixture(fix(m)),
            ToolError::Io { path, message } => ToolError::Io {
                path: fix(path),
                message: fix(message),
            },
            ToolError::NeedsConfirm { label } => ToolError::NeedsConfirm { label: fix(label) },
            ToolError::DialogOpen {
                kind,
                message,
                detail,
            } => ToolError::DialogOpen {
                kind,
                message: fix(message),
                detail: fix(detail),
            },
            ToolError::PipeClosed { message } => ToolError::PipeClosed {
                message: fix(message),
            },
            other => other,
        }
    }
}

/// `document.documentElement.outerHTML` of a copy without the `value`
/// attribute of any text-like input (a framework may mirror typed text
/// there), so `browser_get_html` cannot carry a typed password.
pub(crate) const SAFE_HTML_JS: &str = "(function(){var d=document.documentElement.cloneNode(true);var s=d.querySelectorAll('input');for(var i=0;i<s.length;i++){var t=(s[i].getAttribute('type')||'text').toLowerCase();if(!/^(button|submit|reset|image|checkbox|radio)$/.test(t))s[i].removeAttribute('value');}return d.outerHTML;})()";
