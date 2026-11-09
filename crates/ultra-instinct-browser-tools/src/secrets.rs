//! Secrets never leave the tools, failing closed.
//!
//! A field is [`sensitive`] when it is a password / hidden / file input, a
//! field whose label names a secret (PIN, CVV, card, one-time
//! code, …; `ultra_instinct_policy::is_secret_field`), or a text field whose
//! input type observe could not read. A sensitive field's value is never
//! printed (`browser_get_state`, `browser_get`), and text typed into one is
//! remembered ([`Secrets`]) and replaced by [`MASK`] in every later tool
//! output and error, the HAR file included, so a page that echoes it (a
//! console log, a GET form's URL, a confirmation line) does not leak it.
//! `browser_get_html` drops `value` attributes of text-like inputs. Pixels
//! cannot be masked: once a secret is typed, screenshots and PDFs hide the
//! sensitive fields' values around the capture ([`capture_hidden`]).

use std::collections::VecDeque;

use serde_json::Value;
use ultra_instinct_agent::Input;
use ultra_instinct_core::{Action, InteractionManifold, InteractionRegion, RegionId};

use crate::error::ToolError;
use crate::host::{LiveSession, ToolState};

/// What a remembered secret reads as in tool output.
pub const MASK: &str = "••••";

/// Shortest text typed into a non-sensitive field that is scrubbed: shorter
/// text would blank out ordinary words.
const MIN_PLAIN_CHARS: usize = 4;

/// Text typed into non-sensitive fields kept for scrubbing; the oldest such
/// entry drops past this. Sensitive entries are never dropped.
const MAX_PLAIN: usize = 32;

/// Distinct values typed into sensitive fields one server keeps; past it a
/// new one refuses (`secrets_full`) before it is typed, never evicting one.
pub const MAX_SENSITIVE_SECRETS: usize = 64;

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

/// Text typed during this server's life, masked in every later output.
#[derive(Default)]
pub(crate) struct Secrets {
    /// Typed into sensitive fields, any length: pinned, never evicted.
    pinned: Vec<String>,
    /// Typed into other fields (4 chars or more), newest [`MAX_PLAIN`].
    plain: VecDeque<String>,
}

impl Secrets {
    /// Remember `text` typed into a field (`sensitive`: one that holds a
    /// secret, or whose kind is unknown). A sensitive value is kept
    /// whatever its length and never evicted; past
    /// [`MAX_SENSITIVE_SECRETS`] a new one refuses, so the caller must not
    /// type it. Other text is kept from 4 chars, the oldest dropping first.
    pub(crate) fn remember(&mut self, text: &str, sensitive: bool) -> Result<(), ToolError> {
        if text.trim().is_empty() || self.pinned.iter().any(|t| t == text) {
            return Ok(());
        }
        if sensitive {
            if self.pinned.len() >= MAX_SENSITIVE_SECRETS {
                return Err(ToolError::SecretsFull {
                    limit: MAX_SENSITIVE_SECRETS,
                });
            }
            self.plain.retain(|t| t != text);
            self.pinned.push(text.to_owned());
        } else if text.chars().count() >= MIN_PLAIN_CHARS && !self.plain.iter().any(|t| t == text) {
            if self.plain.len() >= MAX_PLAIN {
                self.plain.pop_front();
            }
            self.plain.push_back(text.to_owned());
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.pinned.is_empty() && self.plain.is_empty()
    }

    /// `text` with every remembered secret masked (longest first), as typed
    /// or as a URL or HTML writes it ([`occurrence`]). A secret shorter than
    /// 4 chars (a CVV, a PIN) is masked only as a whole token: not next to a
    /// letter or digit (a `%XX` neighbour counts as the byte it encodes), so
    /// `123` hides in `cvv=123` but not in `41123`.
    pub(crate) fn scrub_str(&self, text: &str) -> Option<String> {
        let encoded = text.contains(['%', '+', '&']);
        let mut hits: Vec<&String> = self
            .pinned
            .iter()
            .chain(&self.plain)
            .filter(|s| text.contains(s.as_str()) || (encoded && may_be_encoded(text, s)))
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut out = text.to_owned();
        for secret in hits {
            out = mask(&out, secret, secret.chars().count() < MIN_PLAIN_CHARS);
        }
        (out != text).then_some(out)
    }

    /// Mask secrets in every string of `value`.
    pub(crate) fn scrub_value(&self, value: &mut Value) {
        if self.is_empty() {
            return;
        }
        match value {
            Value::String(text) => {
                if let Some(clean) = self.scrub_str(text) {
                    *text = clean;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.scrub_value(v)),
            Value::Object(map) => map.values_mut().for_each(|v| self.scrub_value(v)),
            _ => {}
        }
    }

    /// [`Self::scrub_value`] on `tool`'s result, but its one binary field:
    /// `browser_screenshot`'s `base64` and `browser_save_as_pdf`'s `data`
    /// (masking would corrupt the image; the MCP image item is built from
    /// that field). Any other `data` / `base64` key is text and is masked.
    pub(crate) fn scrub_tool_value(&self, tool: &str, value: &mut Value) {
        let binary = match tool {
            "browser_screenshot" => "base64",
            "browser_save_as_pdf" => "data",
            _ => return self.scrub_value(value),
        };
        let kept = value.as_object_mut().and_then(|map| map.remove(binary));
        self.scrub_value(value);
        if let (Some(kept), Some(map)) = (kept, value.as_object_mut()) {
            map.insert(binary.to_owned(), kept);
        }
    }
    /// `err` with secrets masked in its free text.
    pub(crate) fn scrub_error(&self, err: ToolError) -> ToolError {
        if self.is_empty() {
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

/// The check `browser_type` runs just before its dispatch (through
/// [`ultra_instinct_agent::BeforeDispatch`], which forwards every other
/// runtime method): the text becomes a secret when the target on the
/// executor's own fresh observation (the one it revalidated against) is
/// sensitive, or is missing from it (fail closed). `input_type` is not in
/// the region fingerprint, so a field that became a password input after
/// the caller looked still revalidates. `Err`: `secrets_full`, the ticket
/// is spent and nothing is typed.
pub(crate) fn remember_at_dispatch(
    secrets: &mut Secrets,
    fresh: Option<&InteractionManifold>,
    target: &RegionId,
    input: &Input,
) -> Result<(), ToolError> {
    match input {
        Input::Type(text) if fresh.and_then(|m| m.get(target)).is_none_or(sensitive) => {
            secrets.remember(text, true)
        }
        _ => Ok(()),
    }
}

/// Hide one field (`this`) for a capture: `-webkit-text-security: disc`
/// inline, `!important`. Returns its `style` attribute as it was.
const HIDE_FUNCTION: &str = "function(){var p=this.getAttribute('style');this.style.setProperty('-webkit-text-security','disc','important');return p;}";

/// Put a field's `style` attribute back exactly (absent stays absent).
const RESTORE_FUNCTION: &str =
    "function(p){if(p===null){this.removeAttribute('style');}else{this.setAttribute('style',p);}return true;}";

/// The CDP object group the hidden fields' handles live in.
const HIDE_GROUP: &str = "ultra-instinct-capture";

/// Fields hidden for one capture: each handle and the `style` it had.
/// [`Hidden::restore`] puts them back.
pub(crate) struct Hidden(Vec<(String, Value)>);

impl Secrets {
    /// This server typed a value it treats as a secret.
    pub(crate) fn any_pinned(&self) -> bool {
        !self.pinned.is_empty()
    }
}

fn call_json(session: &mut LiveSession, method: &str, params: &Value) -> Result<Value, String> {
    let body = session
        .transport_mut()
        .call(method, &params.to_string())
        .map_err(|err| err.to_string())?;
    let reply: Value = serde_json::from_str(&body).map_err(|err| err.to_string())?;
    match reply.get("exceptionDetails") {
        Some(details) => Err(format!("{method} threw: {details}")),
        None => Ok(reply),
    }
}

/// Before a screenshot / PDF: observe, and hide the shown value of every
/// sensitive text field (by the observe rule, [`sensitive`]; password and
/// hidden inputs show none), one `Runtime.callFunctionOn` on each field's
/// own handle (so a same-origin frame's field runs in its own world); no
/// field reaches script text. Only the inline style changes, never a value.
/// A field that cannot be resolved or hidden puts back the ones already
/// hidden and refuses ([`ToolError::SecretOnScreen`]).
pub(crate) fn hide_for_capture(session: &mut LiveSession) -> Result<Hidden, ToolError> {
    let manifold = session
        .observe()
        .map_err(|err| ToolError::Browser(err.to_string()))?
        .clone();
    let mut hidden = Hidden(Vec::new());
    for region in manifold.regions() {
        let shown = !matches!(
            region.state().input_type.as_deref(),
            Some("password" | "hidden")
        );
        if !(shown && region.actions().contains(&Action::Type) && sensitive(region)) {
            continue;
        }
        if let Err(message) = hide_one(session, region.id(), &mut hidden) {
            // Best effort: the refusal stands either way.
            let _ = hidden.restore(session);
            return Err(ToolError::SecretOnScreen { message });
        }
    }
    Ok(hidden)
}

fn hide_one(session: &mut LiveSession, id: &RegionId, hidden: &mut Hidden) -> Result<(), String> {
    let mut node = match session.node_ids(id) {
        Ok((Some(node), _)) => serde_json::json!({"nodeId": node}),
        Ok((None, Some(backend))) => serde_json::json!({"backendNodeId": backend}),
        _ => return Err(format!("{} has no DOM node", id.as_str())),
    };
    node["objectGroup"] = HIDE_GROUP.into();
    let resolved = call_json(session, "DOM.resolveNode", &node)?;
    let Some(object) = resolved["object"]["objectId"].as_str().map(str::to_owned) else {
        return Err(format!("{} resolved to no object", id.as_str()));
    };
    let hide = serde_json::json!({
        "objectId": object,
        "functionDeclaration": HIDE_FUNCTION,
        "returnByValue": true,
    });
    let reply = call_json(session, "Runtime.callFunctionOn", &hide)?;
    hidden.0.push((object, reply["result"]["value"].clone()));
    Ok(())
}

impl Hidden {
    /// Put every hidden field's `style` back as it was, then free the
    /// handles (when there are any). Runs after the capture whatever it
    /// returned; every field is tried, the first failure is reported.
    pub(crate) fn restore(self, session: &mut LiveSession) -> Result<(), String> {
        if self.0.is_empty() {
            return Ok(());
        }
        let mut first_err = Ok(());
        for (object, prev) in &self.0 {
            let restore = serde_json::json!({
                "objectId": object,
                "functionDeclaration": RESTORE_FUNCTION,
                "arguments": [{"value": prev}],
                "returnByValue": true,
            });
            let restored = call_json(session, "Runtime.callFunctionOn", &restore).map(|_| ());
            first_err = first_err.and(restored);
        }
        let group = serde_json::json!({"objectGroup": HIDE_GROUP});
        let released = call_json(session, "Runtime.releaseObjectGroup", &group).map(|_| ());
        first_err.and(released)
    }
}

/// A capture (`Page.captureScreenshot` / `printToPDF` inside `capture`)
/// with the page's sensitive field values hidden around it once this
/// server has typed a secret. The restore always runs after `capture`,
/// also when it failed; a failed restore is noted as `restore_error` on a
/// successful body (best effort: the capture's own result stands).
pub(crate) fn capture_hidden(
    server: &mut dyn crate::host::BrowserHost,
    session: &mut LiveSession,
    capture: impl FnOnce(&mut LiveSession) -> Result<Value, ToolError>,
) -> Result<Value, ToolError> {
    if !server.tool_state().secrets.any_pinned() {
        return capture(session);
    }
    let hidden = hide_for_capture(session)?;
    let captured = capture(session);
    let restored = crate::host::best_effort(server, |_| hidden.restore(session));
    captured.map(|mut body| {
        if let Err(err) = restored {
            body["restore_error"] = Value::String(err);
        }
        body
    })
}

/// Whether `text` holds the longest ASCII letter / digit run of `secret`,
/// which every encoded form keeps as it is (no run: maybe).
fn may_be_encoded(text: &str, secret: &str) -> bool {
    let run = secret
        .split(|c: char| !c.is_ascii_alphanumeric())
        .max_by_key(|run| run.len())
        .unwrap_or_default();
    run.is_empty() || text.contains(run)
}

/// The HTML entities a serializer writes for `c`.
fn entities(c: char) -> &'static [&'static str] {
    match c {
        '&' => &["&amp;", "&#38;"],
        '<' => &["&lt;", "&#60;"],
        '>' => &["&gt;", "&#62;"],
        '"' => &["&quot;", "&#34;"],
        '\'' => &["&#39;", "&apos;"],
        '\u{a0}' => &["&nbsp;"],
        _ => &[],
    }
}

/// The byte `%XX` (either hex case) at the start of `bytes` encodes.
fn percent_byte(bytes: &[u8]) -> Option<u8> {
    let hex = |b: u8| char::from(b).to_digit(16);
    match bytes {
        [b'%', hi, lo, ..] => Some(u8::try_from(hex(*hi)? * 16 + hex(*lo)?).ok()?),
        _ => None,
    }
}

/// Byte length of `secret` at `text[at..]`, as typed or as a URL or HTML
/// writes it: each char but an ASCII letter or digit may also be its UTF-8
/// bytes as `%XX` (either hex case, as `encodeURIComponent` or a form
/// writes it), a space also `+`, and `& < > " '` (and no-break space) an
/// HTML entity.
fn occurrence(text: &str, at: usize, secret: &str) -> Option<usize> {
    let mut i = at;
    for c in secret.chars() {
        let rest = &text[i..];
        // An entity first: `&amp;` also starts with a literal `&`.
        if let Some(entity) = entities(c).iter().find(|e| rest.starts_with(**e)) {
            i += entity.len();
            continue;
        }
        if rest.starts_with(c) {
            i += c.len_utf8();
            continue;
        }
        if c.is_ascii_alphanumeric() {
            return None;
        }
        if c == ' ' && rest.starts_with('+') {
            i += 1;
            continue;
        }
        let mut utf8 = [0u8; 4];
        for byte in c.encode_utf8(&mut utf8).bytes() {
            if percent_byte(&text.as_bytes()[i..]) != Some(byte) {
                return None;
            }
            i += 3;
        }
    }
    Some(i - at)
}

/// Whether the text right before (`before`) or after `text[at..]` is a
/// letter or digit, reading a `%XX` there as the byte it encodes.
fn word_next_to(text: &str, at: usize, before: bool) -> bool {
    let bytes = text.as_bytes();
    let encoded = if before {
        at.checked_sub(3)
            .and_then(|from| percent_byte(&bytes[from..]))
    } else {
        percent_byte(&bytes[at..])
    };
    match encoded {
        Some(byte) => !byte.is_ascii() || byte.is_ascii_alphanumeric(),
        None if before => text[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric),
        None => text[at..].chars().next().is_some_and(char::is_alphanumeric),
    }
}

/// `text` with each [`occurrence`] of `secret` masked; with `whole_token`,
/// only one with no letter or digit right before or after it.
fn mask(text: &str, secret: &str, whole_token: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut rest, mut at) = (0, 0);
    while at < text.len() {
        let found = occurrence(text, at, secret).filter(|&len| {
            len > 0
                && !(whole_token
                    && (word_next_to(text, at, true) || word_next_to(text, at + len, false)))
        });
        match found {
            Some(len) => {
                out.push_str(&text[rest..at]);
                out.push_str(MASK);
                at += len;
                rest = at;
            }
            None => at += text[at..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out.push_str(&text[rest..]);
    out
}

impl ToolState {
    /// Remember text typed outside the `browser_*` tools (a host's own
    /// agent loop): masked in every later output and error. `sensitive`
    /// as in [`sensitive`]; see [`MAX_SENSITIVE_SECRETS`] for the bound.
    pub fn remember_typed(&mut self, text: &str, sensitive: bool) -> Result<(), ToolError> {
        self.secrets.remember(text, sensitive)
    }

    /// `result` of `tool` with every remembered secret masked: every string
    /// of the body (but the tool's one binary field) and the error's text.
    /// A host runs every tool result through this one choke point.
    pub fn scrub_result(
        &self,
        tool: &str,
        result: Result<Value, ToolError>,
    ) -> Result<Value, ToolError> {
        match result {
            Ok(mut body) => {
                self.secrets.scrub_tool_value(tool, &mut body);
                Ok(body)
            }
            Err(err) => Err(self.secrets.scrub_error(err)),
        }
    }

    /// `text` with every remembered secret masked.
    pub fn scrub_text(&self, text: &str) -> String {
        self.secrets
            .scrub_str(text)
            .unwrap_or_else(|| text.to_owned())
    }
}

/// `document.documentElement.outerHTML` of a copy without the `value`
/// attribute of any text-like input (a framework may mirror typed text
/// there), so `browser_get_html` cannot carry a typed password.
pub(crate) const SAFE_HTML_JS: &str = "(function(){var d=document.documentElement.cloneNode(true);var s=d.querySelectorAll('input');for(var i=0;i<s.length;i++){var t=(s[i].getAttribute('type')||'text').toLowerCase();if(!/^(button|submit|reset|image|checkbox|radio)$/.test(t))s[i].removeAttribute('value');}return d.outerHTML;})()";
