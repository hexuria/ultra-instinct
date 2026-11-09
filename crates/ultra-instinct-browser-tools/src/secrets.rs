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
//! `browser_get_html` drops `value` attributes of text-like inputs. Pixels
//! cannot be masked: once a secret is typed, screenshots and PDFs hide the
//! sensitive fields' values around the capture ([`capture_hidden`]).

use std::collections::VecDeque;

use serde_json::Value;
use ultra_instinct_agent::{AgentError, BrowserRuntime, Input};
use ultra_instinct_browser::{PageState, ScrollDirection};
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

    /// `text` with every remembered secret masked (longest first). A secret
    /// shorter than 4 chars (a CVV, a PIN) is masked only as a whole token:
    /// not next to a letter or digit, so `123` hides in `cvv=123` but not
    /// in `41123`.
    pub(crate) fn scrub_str(&self, text: &str) -> Option<String> {
        let mut hits: Vec<&String> = self
            .pinned
            .iter()
            .chain(&self.plain)
            .filter(|s| text.contains(s.as_str()))
            .collect();
        if hits.is_empty() {
            return None;
        }
        hits.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut out = text.to_owned();
        for secret in hits {
            out = if secret.chars().count() < MIN_PLAIN_CHARS {
                mask_tokens(&out, secret)
            } else {
                out.replace(secret.as_str(), MASK)
            };
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

/// The session the executor drives for one `browser_type`. Just before
/// dispatch, the text becomes a secret when the target on the executor's
/// own fresh observation (the one it revalidated against) is sensitive, or
/// is missing from it (fail closed): `input_type` is not in the region
/// fingerprint, so a field that became a password input after the caller
/// looked still revalidates.
pub(crate) struct TypingInto<'a> {
    pub(crate) session: &'a mut LiveSession,
    pub(crate) secrets: &'a mut Secrets,
    /// `secrets_full` at dispatch: the ticket is spent, nothing was typed.
    pub(crate) refused: Option<ToolError>,
}

impl BrowserRuntime for TypingInto<'_> {
    fn observe(&mut self) -> Result<&InteractionManifold, AgentError> {
        BrowserRuntime::observe(self.session)
    }
    fn focused(&self) -> Option<RegionId> {
        BrowserRuntime::focused(self.session)
    }
    fn page(&self) -> Option<&PageState> {
        BrowserRuntime::page(self.session)
    }
    fn last_observation(&self) -> Option<&InteractionManifold> {
        BrowserRuntime::last_observation(self.session)
    }
    fn scroll(&mut self, direction: ScrollDirection) -> Result<(), AgentError> {
        BrowserRuntime::scroll(self.session, direction)
    }
    fn dispatch(&mut self, target: &RegionId, input: &Input) -> Result<(), AgentError> {
        if let Input::Type(text) = input {
            let secret = BrowserRuntime::last_observation(self.session)
                .and_then(|fresh| fresh.get(target))
                .is_none_or(sensitive);
            if let Err(err) = secret
                .then(|| self.secrets.remember(text, true))
                .transpose()
            {
                self.refused = Some(err);
                return Err(AgentError::InputRejected("secrets_full".into()));
            }
        }
        BrowserRuntime::dispatch(self.session, target, input)
    }
}

/// Hide each field (its own arguments) for a capture: `-webkit-text-security:
/// disc` inline, `!important`. Returns each `style` attribute as it was.
const HIDE_FUNCTION: &str = "function(){var prev=[];for(var i=0;i<arguments.length;i++){var e=arguments[i];prev.push(e.getAttribute('style'));e.style.setProperty('-webkit-text-security','disc','important');}return prev;}";

/// Put each field's `style` attribute back exactly (absent stays absent).
const RESTORE_FUNCTION: &str = "function(prev){for(var i=1;i<arguments.length;i++){var e=arguments[i],p=prev[i-1];if(p===null){e.removeAttribute('style');}else{e.setAttribute('style',p);}}return true;}";

/// The CDP object group the hidden fields' handles live in.
const HIDE_GROUP: &str = "ultra-instinct-capture";

/// Fields hidden for one capture; [`Hidden::restore`] puts them back.
pub(crate) struct Hidden {
    objects: Vec<Value>,
    prev: Value,
}

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
    serde_json::from_str(&body).map_err(|err| err.to_string())
}

/// Before a screenshot / PDF: observe, and hide the shown value of every
/// sensitive text field (by the observe rule, [`sensitive`]; password and
/// hidden inputs show none) with one `Runtime.callFunctionOn` whose fields
/// are its object arguments, never script text. Only the inline style
/// changes, never a value. `None`: no such field. A field that cannot be
/// resolved or hidden refuses ([`ToolError::SecretOnScreen`]).
pub(crate) fn hide_for_capture(session: &mut LiveSession) -> Result<Option<Hidden>, ToolError> {
    let manifold = session
        .observe()
        .map_err(|err| ToolError::Browser(err.to_string()))?
        .clone();
    let fields = manifold.regions().filter(|region| {
        let shown = !matches!(
            region.state().input_type.as_deref(),
            Some("password" | "hidden")
        );
        shown && region.actions().contains(&Action::Type) && sensitive(region)
    });
    let refuse = |message: String| ToolError::SecretOnScreen { message };
    let mut objects = Vec::new();
    for region in fields {
        let node = match session.node_ids(region.id()) {
            Ok((Some(id), _)) => serde_json::json!({"nodeId": id, "objectGroup": HIDE_GROUP}),
            Ok((None, Some(id))) => {
                serde_json::json!({"backendNodeId": id, "objectGroup": HIDE_GROUP})
            }
            _ => return Err(release(session, refuse("a field has no DOM node".into()))),
        };
        let resolved = call_json(session, "DOM.resolveNode", &node).map_err(&refuse);
        match resolved.map(|r| r["object"]["objectId"].clone()) {
            Ok(Value::String(id)) => objects.push(serde_json::json!({"objectId": id})),
            Ok(_) => return Err(release(session, refuse("no objectId".into()))),
            Err(err) => return Err(release(session, err)),
        }
    }
    let Some(first) = objects.first() else {
        return Ok(None);
    };
    let hide = serde_json::json!({
        "objectId": first["objectId"],
        "functionDeclaration": HIDE_FUNCTION,
        "arguments": objects,
        "returnByValue": true,
    });
    match call_json(session, "Runtime.callFunctionOn", &hide) {
        Ok(reply)
            if reply.get("exceptionDetails").is_none() && reply["result"]["value"].is_array() =>
        {
            let prev = reply["result"]["value"].clone();
            Ok(Some(Hidden { objects, prev }))
        }
        Ok(reply) => Err(release(
            session,
            refuse(format!("hide failed: {}", reply["exceptionDetails"])),
        )),
        Err(err) => Err(release(session, refuse(err))),
    }
}

/// Free the hidden fields' handles (best effort) and give back `err`.
fn release(session: &mut LiveSession, err: ToolError) -> ToolError {
    let group = serde_json::json!({"objectGroup": HIDE_GROUP});
    let _ = call_json(session, "Runtime.releaseObjectGroup", &group);
    err
}

impl Hidden {
    /// Put every hidden field's `style` back as it was, then free the
    /// handles. Runs after the capture whatever it returned.
    pub(crate) fn restore(self, session: &mut LiveSession) -> Result<(), String> {
        let mut arguments = vec![serde_json::json!({"value": self.prev})];
        arguments.extend(self.objects.iter().cloned());
        let restore = serde_json::json!({
            "objectId": self.objects[0]["objectId"],
            "functionDeclaration": RESTORE_FUNCTION,
            "arguments": arguments,
            "returnByValue": true,
        });
        let restored = call_json(session, "Runtime.callFunctionOn", &restore).and_then(|reply| {
            match reply.get("exceptionDetails") {
                None => Ok(()),
                Some(details) => Err(format!("restore failed: {details}")),
            }
        });
        let group = serde_json::json!({"objectGroup": HIDE_GROUP});
        let released = call_json(session, "Runtime.releaseObjectGroup", &group).map(|_| ());
        restored.and(released)
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
    let Some(hidden) = hidden else {
        return captured;
    };
    let restored = crate::host::best_effort(server, |_| hidden.restore(session));
    captured.map(|mut body| {
        if let Err(err) = restored {
            body["restore_error"] = Value::String(err);
        }
        body
    })
}

/// `text` with each whole-token occurrence of `secret` masked: one with no
/// letter or digit right before or after it.
fn mask_tokens(text: &str, secret: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = 0;
    for (at, _) in text.match_indices(secret) {
        let end = at + secret.len();
        let before = text[..at].chars().next_back();
        let after = text[end..].chars().next();
        if before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric) {
            continue;
        }
        out.push_str(&text[rest..at]);
        out.push_str(MASK);
        rest = end;
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
