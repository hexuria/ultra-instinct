//! The user's policy: which sites may receive input, which presses need a
//! confirm first.
//!
//! A [`Policy`] is a hard refusal next to the gate, never a score. An empty
//! policy allows everything (the behaviour without one).
//!
//! - **Domains** ([`Policy::allow_domains`]): when the list is non-empty,
//!   input begins only on a page whose URL host matches a [`HostPattern`]
//!   (else [`GuardReason::DomainNotAllowed`]), and a press that submits a
//!   form (a submit button, or Enter in its field / box) is refused when
//!   the form is sent off the list (or to `javascript:`, or the target could
//!   not be read). Other navigation the input causes (a link) is checked at
//!   the next input there. Only `http` / `https` on a listed host passes,
//!   plus `about:blank` and a `blob:` by its origin ([`Policy::check_url`];
//!   [`Policy::refuse_blob`] refuses every `blob:` instead). An *unknown*
//!   URL (the observation could not read one) is refused while the list is
//!   non-empty.
//! - **Confirm** ([`Policy::confirm_press`]): a press whose label matches a
//!   [`LabelGlob`] is refused ([`GuardReason::NeedsConfirm`]) unless that
//!   label is in [`Policy::confirmed`]. A press is a CLICK-family input
//!   (click, toggle, surface point) on the target, an Enter / Space KEY on
//!   a button-like target, or an Enter KEY in a form control, which presses
//!   its form's submit button ([`Policy::press_label`]).
//!
//! Neither refusal is stale: observing again does not change the answer, so
//! a host never retries it with the same lease.

use std::fmt;

use ultra_instinct_core::{
    Action, FormSubmit, InteractionManifold, InteractionRegion, RegionId, Role,
};
use ultra_instinct_protocol::{ActionTicket, GuardReason, TicketInvalid};

use crate::ticket::{revalidate, ConsumeError, TicketLedger};
#[cfg(doc)]
use ultra_instinct_core::ElementState;

/// A bad allow-domain or confirm-press entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyError(String);

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PolicyError {}

/// `shop.example` (that host exactly) or `*.shop.example` (any subdomain of
/// it, not the bare host). Case-insensitive; a trailing dot is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPattern {
    host: String,
    subdomains: bool,
}

impl HostPattern {
    pub fn parse(raw: &str) -> Result<Self, PolicyError> {
        let trimmed = raw.trim();
        let (subdomains, host) = match trimmed.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, trimmed),
        };
        let host = normalize_host(host);
        let valid = !host.is_empty()
            && !host.starts_with('.')
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
        if !valid {
            return Err(PolicyError(format!(
                "allow-domain `{raw}` must be a host like shop.example or *.shop.example"
            )));
        }
        Ok(Self { host, subdomains })
    }

    pub fn matches(&self, host: &str) -> bool {
        let host = normalize_host(host);
        if self.subdomains {
            host.len() > self.host.len() + 1
                && host.ends_with(&self.host)
                && host.as_bytes()[host.len() - self.host.len() - 1] == b'.'
        } else {
            host == self.host
        }
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.subdomains {
            f.write_str("*.")?;
        }
        f.write_str(&self.host)
    }
}

/// A label pattern: `*` matches any run of characters, everything else
/// matches itself case-insensitively. Runs of whitespace count as one space;
/// invisible characters ([`ultra_instinct_core::INVISIBLE`]) count as none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelGlob {
    raw: String,
    folded: Vec<char>,
}

impl LabelGlob {
    pub fn parse(raw: &str) -> Result<Self, PolicyError> {
        let folded = fold(raw);
        if folded.is_empty() {
            return Err(PolicyError(
                "confirm-press pattern must not be empty".into(),
            ));
        }
        Ok(Self {
            raw: raw.trim().to_owned(),
            folded: folded.chars().collect(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn matches(&self, label: &str) -> bool {
        let label: Vec<char> = fold(label).chars().collect();
        glob(&self.folded, &label)
    }
}

impl fmt::Display for LabelGlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Which sites may receive input and which presses need a confirm.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Hosts the agent may give input to. Empty: any host.
    pub allow_domains: Vec<HostPattern>,
    /// Label patterns whose press needs a confirm ([`Policy::press_label`]).
    pub confirm_press: Vec<LabelGlob>,
    /// Labels the user confirmed (case-insensitive, whitespace-folded).
    pub confirmed: Vec<String>,
    /// With an allow-list set, refuse every `blob:` page, whatever its
    /// origin: its content is whatever a script made it. Off by default
    /// (a `blob:` passes by its origin); no effect without a list.
    pub refuse_blob: bool,
}

impl Policy {
    /// Allows everything.
    pub fn is_empty(&self) -> bool {
        self.allow_domains.is_empty() && self.confirm_press.is_empty()
    }

    /// A policy from two comma lists (the `ULTRA_INSTINCT_ALLOW_DOMAINS` /
    /// `ULTRA_INSTINCT_CONFIRM_PRESS` form). Empty items are skipped.
    pub fn from_lists(allow_domains: &str, confirm_press: &str) -> Result<Self, PolicyError> {
        let items = |list: &str| -> Vec<String> {
            list.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        };
        Ok(Self {
            allow_domains: items(allow_domains)
                .iter()
                .map(|h| HostPattern::parse(h))
                .collect::<Result<_, _>>()?,
            confirm_press: items(confirm_press)
                .iter()
                .map(|g| LabelGlob::parse(g))
                .collect::<Result<_, _>>()?,
            confirmed: Vec::new(),
            refuse_blob: false,
        })
    }

    /// This policy with one more confirmed label.
    pub fn with_confirmed(mut self, label: impl Into<String>) -> Self {
        self.confirmed.push(label.into());
        self
    }

    /// This policy with [`Policy::refuse_blob`] set to `on`.
    pub fn with_refuse_blob(mut self, on: bool) -> Self {
        self.refuse_blob = on;
        self
    }

    /// May the browser go to (or give input on) `url`? With an allow-list
    /// set, only an `http` / `https` URL on a listed host passes, plus
    /// `about:blank` (an empty page) and a `blob:` URL whose origin passes
    /// (none with [`Policy::refuse_blob`]); `data:`, `javascript:`, `file:`
    /// and every other scheme refuse.
    pub fn check_url(&self, url: &str) -> Result<(), GuardReason> {
        if self.allow_domains.is_empty() {
            return Ok(());
        }
        let url = url.trim();
        let scheme = scheme_of(url).map(str::to_ascii_lowercase);
        match scheme.as_deref() {
            Some("http" | "https") => match host_of(url) {
                Some(host) if self.allow_domains.iter().any(|p| p.matches(&host)) => Ok(()),
                _ => Err(GuardReason::DomainNotAllowed),
            },
            Some("blob") if self.refuse_blob => Err(GuardReason::DomainNotAllowed),
            Some("blob") => self.check_url(&url["blob:".len()..]),
            Some("about") if url.eq_ignore_ascii_case("about:blank") => Ok(()),
            _ => Err(GuardReason::DomainNotAllowed),
        }
    }

    /// May `region` on the page at `url` receive `action`? `url` is `None`
    /// when the observation could not read it; `key` is a KEY's key (`None`
    /// for other actions; a KEY without one is taken as Enter). The reason
    /// alone of [`Self::check_detail`].
    pub fn check(
        &self,
        url: Option<&str>,
        region: &InteractionRegion,
        action: Action,
        key: Option<&str>,
    ) -> Result<(), GuardReason> {
        self.check_detail(url, region, action, key)
            .map_err(|refusal| refusal.reason)
    }

    /// [`Self::check`] with what refused: the page's host (input begins
    /// only on an allowed host), then, for a press that submits a form, the
    /// host the form is sent to (an off-list, `javascript:` or unread target
    /// refuses while the allow-list is set), then the confirm patterns
    /// against [`Self::press_label`]. Other navigation the input causes is
    /// checked at the next input.
    pub fn check_detail(
        &self,
        url: Option<&str>,
        region: &InteractionRegion,
        action: Action,
        key: Option<&str>,
    ) -> Result<(), Refusal> {
        let press = self.press(region, action, key);
        let label = press.as_ref().map_or(region.label(), |p| p.0);
        let refuse = |reason, url: Option<&str>| Refusal {
            reason,
            label: label.to_owned(),
            url: url.map(str::to_owned),
        };
        let on_list = |url: &str| self.check_url(url).is_ok();
        match url {
            Some(url) if !on_list(url) => {
                return Err(refuse(GuardReason::DomainNotAllowed, Some(url)))
            }
            None if !self.allow_domains.is_empty() => {
                return Err(refuse(GuardReason::DomainNotAllowed, None))
            }
            _ => {}
        }
        let Some((label, submit)) = press else {
            return Ok(());
        };
        if let Some(target) = submit.filter(|_| !self.allow_domains.is_empty()) {
            let script = |t: &str| {
                t.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("javascript:")
            };
            match target.filter(|t| !t.is_empty()) {
                Some(t) if !script(t) && on_list(t) => {}
                t => return Err(refuse(GuardReason::DomainNotAllowed, t)),
            }
        }
        if self.needs_confirm(label) {
            return Err(refuse(GuardReason::NeedsConfirm, None));
        }
        Ok(())
    }

    /// The label a confirm is checked against when `region` gets `action`
    /// (`key` for a KEY), `None` when it presses nothing:
    ///
    /// - the CLICK family (click, toggle, surface point): the target's label;
    /// - an activating KEY (Enter / Space / NumpadEnter, any modifiers) on a
    ///   button-like target (it clicks, toggles or takes points, and takes
    ///   no text or select): the target's label, except Enter on a checkbox
    ///   / radio, which submits its form (below; the box's own label when it
    ///   is in no form);
    /// - Enter / NumpadEnter in any other form control: the label of the
    ///   button its form's implicit submission presses (observe evidence,
    ///   [`FormSubmit`]); none when it is in no form or its form does not
    ///   submit on Enter; [`FORM_SUBMIT_UNKNOWN`] when the submit button has
    ///   no name, the form submits with no button, or observe could not read
    ///   it (then any confirm pattern refuses).
    ///
    /// Space in a field is text; other keys press nothing.
    pub fn press_label<'a>(
        &self,
        region: &'a InteractionRegion,
        action: Action,
        key: Option<&str>,
    ) -> Option<&'a str> {
        self.press(region, action, key).map(|(label, _)| label)
    }

    /// [`Self::press_label`] and, when the press submits a form, where to:
    /// `Some(target)` (`None` / `""`: not read). A submit button submits
    /// when clicked or pressed by key ([`ElementState::submit_action`]);
    /// Enter in a form control submits its form ([`FormSubmit`]).
    fn press<'a>(
        &self,
        region: &'a InteractionRegion,
        action: Action,
        key: Option<&str>,
    ) -> Option<(&'a str, Option<Option<&'a str>>)> {
        let state = region.state();
        let own = || (region.label(), state.submit_action.as_deref().map(Some));
        if is_press(action) {
            return Some(own());
        }
        if action != Action::Key {
            return None;
        }
        let key = key.unwrap_or("Enter");
        if !is_activating_key(key) {
            return None;
        }
        let acts = region.actions();
        let takes_text = acts
            .iter()
            .any(|a| matches!(a, Action::Type | Action::Select));
        let button_like = !takes_text && acts.iter().any(|a| is_press(*a));
        // Enter on a box submits its form; only a button or link presses itself.
        let toggle = region.role() == Role::Checkbox || acts.contains(&Action::Toggle);
        if button_like && !(toggle && is_enter_key(key)) {
            return Some(own());
        }
        if !is_enter_key(key) {
            return None;
        }
        match &state.form {
            Some(FormSubmit::Button { label, action }) => {
                let label = if label.trim().is_empty() {
                    FORM_SUBMIT_UNKNOWN
                } else {
                    label
                };
                Some((label, Some(action.as_deref())))
            }
            Some(FormSubmit::Implicit { action }) => {
                Some((FORM_SUBMIT_UNKNOWN, Some(action.as_deref())))
            }
            Some(FormSubmit::NotInForm | FormSubmit::NoSubmitButton) => {
                button_like.then(|| (region.label(), None))
            }
            // Evidence observe could not read: an unknown submission.
            None => Some((FORM_SUBMIT_UNKNOWN, Some(None))),
        }
    }

    /// The first confirm pattern `label` matches, unless it was confirmed.
    pub fn confirm_pattern(&self, label: &str) -> Option<&LabelGlob> {
        if self.is_confirmed(label) {
            return None;
        }
        self.confirm_press.iter().find(|g| g.matches(label))
    }

    fn is_confirmed(&self, label: &str) -> bool {
        let folded = fold(label);
        self.confirmed.iter().any(|c| fold(c) == folded)
    }

    /// `label` needs a confirm: it matches a pattern, or it is
    /// [`FORM_SUBMIT_UNKNOWN`] / [`UNOBSERVED_FOCUS`] while any pattern is
    /// set; and it was not confirmed.
    pub fn needs_confirm(&self, label: &str) -> bool {
        if label == FORM_SUBMIT_UNKNOWN || label == UNOBSERVED_FOCUS {
            return !self.confirm_press.is_empty() && !self.is_confirmed(label);
        }
        self.confirm_pattern(label).is_some()
    }
}

/// A policy refusal with what refused: `label` names the press a confirm
/// must name (the target, or the button its form submits with); `url` is
/// what a domain refusal checked (the page, or where the form is sent;
/// `None` when it could not be read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub reason: GuardReason,
    pub label: String,
    pub url: Option<String>,
}

/// The label of a form submission whose button is not known: Enter in a
/// field whose form submits with no submit button, or whose form evidence
/// observe could not read. With any confirm pattern set it needs a confirm
/// of exactly this label.
pub const FORM_SUBMIT_UNKNOWN: &str = "(form submit)";

/// The label of an activating key sent to a focus no observation names (a
/// cross-origin frame, say): with any confirm pattern set it needs a
/// confirm of exactly this label.
pub const UNOBSERVED_FOCUS: &str = "(unobserved focus)";

/// The key, with any modifiers (`Control+Enter`), presses what it lands
/// on: Enter, NumpadEnter or Space.
pub fn is_activating_key(key: &str) -> bool {
    is_enter_key(key) || matches!(base_key(key), " " | "Space" | "Spacebar")
}

/// Enter or NumpadEnter, with any modifiers: it submits a field's form.
fn is_enter_key(key: &str) -> bool {
    matches!(base_key(key), "Enter" | "Return" | "NumpadEnter")
}

/// `Control+Enter` → `Enter`; `+` keeps its `+`.
fn base_key(key: &str) -> &str {
    match key.rsplit_once('+') {
        Some((_, base)) if !base.is_empty() => base,
        _ => key,
    }
}

/// The CLICK family: the presses a confirm applies to.
fn is_press(action: Action) -> bool {
    matches!(action, Action::Click | Action::Toggle | Action::Point)
}

/// The scheme of `url` (`https` in `https://…`, `data` in `data:…`), when
/// it starts with one: a letter, then letters, digits, `+ - .`, then `:`.
fn scheme_of(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
        .then_some(scheme)
}

/// What a domain refusal names for `url`: its host; else, for a URL with
/// no host, its scheme (`a blob: page`, `an https: URL with no host`); else
/// `(unknown url)`. Short whatever the URL holds (a `data:` URL is never
/// echoed).
pub fn refused_place(url: &str) -> String {
    if let Some(host) = refused_host(url) {
        return host;
    }
    let url = url.trim();
    match scheme_of(url).map(str::to_ascii_lowercase) {
        Some(web) if web == "http" || web == "https" => format!("an {web}: URL with no host"),
        Some(scheme) => {
            let article = if scheme.starts_with(['a', 'e', 'i', 'o', 'u']) {
                "an"
            } else {
                "a"
            };
            format!("{article} {scheme}: page")
        }
        None => "(unknown url)".to_owned(),
    }
}

/// The host of a scheme-less `host:port[/…]` (`shop.example:443/cart`,
/// `localhost:8080`, `10.0.0.1:80`): a name with a dot, `localhost`, or one
/// starting with a digit, then a port of 1 to 5 digits. `tel:5551234`,
/// `mailto:…` and `com.example.app:/x` are schemes, not hosts.
/// The host [`refused_place`] names, when it names one: [`host_of`], else
/// the host of a scheme-less `host:port` (`shop.example:443/cart`).
/// `None` when `refused_place` names a scheme or `(unknown url)` instead.
pub fn refused_host(url: &str) -> Option<String> {
    host_of(url).or_else(|| scheme_less_host(url.trim()))
}

fn scheme_less_host(url: &str) -> Option<String> {
    let (name, rest) = url.split_once(':')?;
    let port = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let looks_like_host = name.contains('.')
        || name.eq_ignore_ascii_case("localhost")
        || name.starts_with(|c: char| c.is_ascii_digit());
    let is_port = (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit());
    let plain = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let host = normalize_host(name);
    // `.:80` folds to nothing: no host.
    (looks_like_host && is_port && plain && !host.is_empty()).then_some(host)
}

/// The lowercase host of `url`, `None` when it has none (`about:blank`,
/// `file:///…`, `data:…`, a bare path). Only a real scheme followed by
/// `://` starts an authority, so a URL inside another (`data:…,https://h/`)
/// names no host. The authority ends at `/ ? # \` (browsers read `\` as
/// `/`); userinfo and port are dropped.
pub fn host_of(url: &str) -> Option<String> {
    let url = url.trim();
    let scheme = scheme_of(url)?;
    let rest = url[scheme.len() + 1..].strip_prefix("//")?;
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    let host = normalize_host(host);
    (!host.is_empty()).then_some(host)
}

/// Host consume, one-shot, with the policy: the order of
/// [`crate::consume_ticket_once`] plus a policy check on the same
/// observation before the ticket is marked consumed.
///
/// 1. refuse a consumed ticket (`ticket-consumed`);
/// 2. [`revalidate`] against `manifold`;
/// 3. [`Policy::check`] the target at `url` ([`ConsumeError::Refused`]; the
///    ticket stays unconsumed, and a retry gets the same answer);
/// 4. mark consumed, then `press`.
pub fn consume_ticket_once_checked<E>(
    ledger: &mut TicketLedger,
    ticket: &ActionTicket,
    manifold: &InteractionManifold,
    focused: Option<RegionId>,
    policy: &Policy,
    url: Option<&str>,
    mut press: impl FnMut(&RegionId, Action) -> Result<(), E>,
) -> Result<(), ConsumeError<E>> {
    if ledger.is_consumed(ticket.ticket_id) {
        return Err(ConsumeError::Invalid(TicketInvalid::TicketConsumed));
    }
    revalidate(ticket, manifold, focused).map_err(ConsumeError::Invalid)?;
    let region = manifold
        .get(&ticket.target_id)
        .ok_or(ConsumeError::Invalid(TicketInvalid::TargetGone))?;
    policy
        .check_detail(url, region, ticket.action, ticket.key.as_deref())
        .map_err(ConsumeError::Refused)?;
    ledger
        .mark_consumed(ticket.ticket_id)
        .map_err(ConsumeError::Invalid)?;
    press(&ticket.target_id, ticket.action).map_err(ConsumeError::Press)
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Lowercase, invisible characters dropped, whitespace runs folded to one
/// space, trimmed ([`ultra_instinct_core::normalize_label`]).
fn fold(text: &str) -> String {
    ultra_instinct_core::normalize_label(text).to_lowercase()
}

/// `*`-only glob over chars (iterative, linear backtracking).
fn glob(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::parse_fixture;

    const PAGE: &str = r#"
        viewport w=800 h=600
        region id=pay role=button label="Pay $18" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility
        region id=name role=text_field label="Pay note" x=10 y=50 w=200 h=24 actions=click,type sources=dom,accessibility
    "#;

    fn policy(allow: &str, confirm: &str) -> Policy {
        Policy::from_lists(allow, confirm).unwrap()
    }

    #[test]
    fn empty_policy_allows_everything() {
        let m = parse_fixture(PAGE).unwrap();
        let pay = m.get(&RegionId::try_new("pay").unwrap()).unwrap();
        let p = Policy::default();
        assert!(p.is_empty());
        assert_eq!(
            p.check(Some("https://any.where/"), pay, Action::Click, None),
            Ok(())
        );
        assert_eq!(p.check(None, pay, Action::Click, None), Ok(()));
        assert_eq!(p.check_url("https://evil.example/"), Ok(()));
    }

    #[test]
    fn domains_exact_and_subdomain_patterns() {
        let p = policy("shop.example, *.cdn.example", "");
        assert_eq!(p.check_url("https://shop.example/join"), Ok(()));
        assert_eq!(p.check_url("https://SHOP.example.:443/x"), Ok(()));
        assert_eq!(p.check_url("https://img.cdn.example/a.png"), Ok(()));
        let refused = Err(GuardReason::DomainNotAllowed);
        assert_eq!(p.check_url("https://cdn.example/"), refused);
        assert_eq!(p.check_url("https://www.shop.example/"), refused);
        assert_eq!(p.check_url("https://evilshop.example/"), refused);
        assert_eq!(p.check_url("https://shop.example.evil.test/"), refused);
        assert_eq!(p.check_url("https://shop.example@evil.test/"), refused);
        assert_eq!(p.check_url("https://evil.test\\@shop.example/"), refused);
        // An empty page passes; a blob: is its origin's.
        assert_eq!(p.check_url("about:blank"), Ok(()));
        assert_eq!(p.check_url("blob:https://shop.example/5f1c"), Ok(()));
        assert_eq!(p.check_url("blob:https://evil.test/5f1c"), refused);
        // With refuse_blob, no blob: passes, even on a listed origin.
        let strict = p.clone().with_refuse_blob(true);
        assert_eq!(strict.check_url("https://shop.example/join"), Ok(()));
        assert_eq!(strict.check_url("about:blank"), Ok(()));
        for blob in [
            "blob:https://shop.example/5f1c",
            " BLOB:https://shop.example/x",
            "blob:about:blank",
        ] {
            assert_eq!(strict.check_url(blob), refused, "{blob}");
        }
        assert_eq!(
            refused_place("blob:https://shop.example/5f1c"),
            "a blob: page"
        );
        assert_eq!(refused_place(" DATA:text/html,<b>long</b>"), "a data: page");
        assert_eq!(refused_place("about:srcdoc"), "an about: page");
        // A scheme-less host:port names its host; a scheme keeps its name.
        for (host_port, host) in [
            ("shop.example:443/cart", "shop.example"),
            ("LocalHost:8080", "localhost"),
            (" 10.0.0.1:80", "10.0.0.1"),
        ] {
            assert_eq!(refused_place(host_port), host, "{host_port}");
        }
        for (url, named) in [
            ("tel:5551234", "a tel: page"),
            ("mailto:1234@x.example", "a mailto: page"),
            ("com.example.app:/oauth", "a com.example.app: page"),
            ("view-source:https://x/", "a view-source: page"),
        ] {
            assert_eq!(refused_place(url), named, "{url}");
        }
        assert_eq!(refused_place("https:///x"), "an https: URL with no host");
        assert_eq!(refused_place("https://Evil.test/x"), "evil.test");
        // refused_host answers whether a host was named; `.:80` names none.
        assert_eq!(
            refused_host("shop.example:443/cart").as_deref(),
            Some("shop.example")
        );
        assert_eq!(
            refused_host("https://Evil.test/x").as_deref(),
            Some("evil.test")
        );
        for no_host in [
            "blob:https://shop.example/x",
            "tel:5551234",
            ".:80",
            "..:80",
            "",
        ] {
            assert_eq!(refused_host(no_host), None, "{no_host:?}");
        }
        for none in ["", "  ", "no-scheme", ":x", ".:80"] {
            assert_eq!(refused_place(none), "(unknown url)", "{none:?}");
        }
        // Without a list the switch changes nothing.
        let open = Policy::default().with_refuse_blob(true);
        assert_eq!(open.check_url("blob:https://evil.test/5f1c"), Ok(()));
        // Any other scheme, or a listed host inside another URL, refuses.
        for url in [
            "file:///tmp/page.html",
            "data:text/html,https://shop.example/",
            "data:text/html,<form action=https://evil.test>",
            "javascript:location='https://shop.example/'",
            "about:srcdoc",
            "chrome://settings",
            "view-source:https://shop.example/",
            "ftp://shop.example/",
            "//shop.example/",
            "shop.example",
            "",
        ] {
            assert_eq!(p.check_url(url), refused, "{url}");
        }
        assert_eq!(host_of("data:text/html,https://shop.example/"), None);
    }

    #[test]
    fn unknown_url_is_refused_only_with_an_allowlist() {
        let m = parse_fixture(PAGE).unwrap();
        let pay = m.get(&RegionId::try_new("pay").unwrap()).unwrap();
        assert_eq!(
            policy("shop.example", "").check(None, pay, Action::Click, None),
            Err(GuardReason::DomainNotAllowed)
        );
        assert_eq!(
            policy("", "Pay*").check(None, pay, Action::Type, None),
            Ok(())
        );
    }

    #[test]
    fn confirm_applies_to_click_family_and_is_lifted_by_the_label() {
        let m = parse_fixture(PAGE).unwrap();
        let pay = m.get(&RegionId::try_new("pay").unwrap()).unwrap();
        let note = m.get(&RegionId::try_new("name").unwrap()).unwrap();
        let p = policy("", "pay*");
        let url = Some("https://shop.example/");
        assert_eq!(
            p.check(url, pay, Action::Click, None),
            Err(GuardReason::NeedsConfirm)
        );
        assert_eq!(
            p.check(url, pay, Action::Toggle, None),
            Err(GuardReason::NeedsConfirm)
        );
        // Typing into a field whose label matches is not a press.
        assert_eq!(p.check(url, note, Action::Type, None), Ok(()));
        assert_eq!(
            p.confirm_pattern("Pay $18").map(LabelGlob::as_str),
            Some("pay*")
        );
        let confirmed = p.clone().with_confirmed("  pay   $18 ");
        assert_eq!(confirmed.check(url, pay, Action::Click, None), Ok(()));
        // Confirming one label does not confirm another match.
        assert_eq!(
            confirmed.check(url, note, Action::Click, None),
            Err(GuardReason::NeedsConfirm)
        );
    }

    /// An invisible character in a confirm pattern or in a label counts as
    /// none: "Pa\u{34F}y*" still gates the "Pay $18" press.
    #[test]
    fn a_hidden_character_in_a_pattern_still_gates_the_press() {
        let m = parse_fixture(PAGE).unwrap();
        let pay = m.get(&RegionId::try_new("pay").unwrap()).unwrap();
        let p = policy("", "Pa\u{34F}y*");
        assert_eq!(
            p.check(Some("https://shop.example/"), pay, Action::Click, None),
            Err(GuardReason::NeedsConfirm)
        );
        let glob = LabelGlob::parse("Pay $18").unwrap();
        assert!(glob.matches("P\u{200B}ay\u{AD} $18"));
        assert!(LabelGlob::parse("\u{200B}").is_err());
    }

    /// An activating KEY is a press: Enter / Space on a button-like target
    /// checks its label; Enter in a form control checks the label of the
    /// button its form submits with (observe evidence); Space in a field
    /// and other keys press nothing.
    #[test]
    fn activating_keys_are_presses_of_the_target_or_its_form() {
        use ultra_instinct_core::{ElementState, FormSubmit};
        let m = parse_fixture(PAGE).unwrap();
        let pay = m.get(&RegionId::try_new("pay").unwrap()).unwrap();
        let mut pay_key = pay.to_parts();
        pay_key.actions.push(Action::Key);
        let pay_key = InteractionRegion::try_new(pay_key).unwrap();
        let field = |form: Option<FormSubmit>| {
            let mut parts = m
                .get(&RegionId::try_new("name").unwrap())
                .unwrap()
                .to_parts();
            parts.label = "Email".into();
            parts.actions.push(Action::Key);
            InteractionRegion::try_new(parts)
                .unwrap()
                .with_state(ElementState {
                    form,
                    ..ElementState::default()
                })
        };
        let p = policy("", "pay*");
        let url = Some("https://shop.example/");
        let needs = Err(GuardReason::NeedsConfirm);
        for key in [
            "Enter",
            " ",
            "Space",
            "NumpadEnter",
            "Control+Enter",
            "Shift+ ",
        ] {
            assert_eq!(
                p.check(url, &pay_key, Action::Key, Some(key)),
                needs,
                "{key}"
            );
        }
        assert_eq!(p.check(url, &pay_key, Action::Key, Some("Tab")), Ok(()));
        assert_eq!(p.check(url, &pay_key, Action::Key, Some("Escape")), Ok(()));
        let confirmed = p.clone().with_confirmed("Pay $18");
        assert_eq!(
            confirmed.check(url, &pay_key, Action::Key, Some("Enter")),
            Ok(())
        );

        let in_pay_form = field(Some(FormSubmit::Button {
            label: "Pay $18".into(),
            action: None,
        }));
        assert_eq!(
            p.check(url, &in_pay_form, Action::Key, Some("Enter")),
            needs
        );
        assert_eq!(
            p.press_label(&in_pay_form, Action::Key, Some("Enter")),
            Some("Pay $18")
        );
        assert_eq!(
            confirmed.check(url, &in_pay_form, Action::Key, Some("Enter")),
            Ok(())
        );
        // Space and Tab in a field are not presses; neither is Enter in no
        // form, in a form with no submit button, or one that submits elsewhere.
        assert_eq!(p.check(url, &in_pay_form, Action::Key, Some(" ")), Ok(()));
        assert_eq!(p.check(url, &in_pay_form, Action::Key, Some("Tab")), Ok(()));
        // A form with no submit button that Enter still submits (at most
        // one field blocks implicit submission) names no button.
        let implicit = field(Some(FormSubmit::Implicit { action: None }));
        assert_eq!(p.check(url, &implicit, Action::Key, Some("Enter")), needs);
        assert_eq!(
            p.press_label(&implicit, Action::Key, Some("Enter")),
            Some(FORM_SUBMIT_UNKNOWN)
        );
        assert!(is_activating_key("Shift+ ") && is_activating_key("NumpadEnter"));
        assert!(!is_activating_key("Tab") && !is_activating_key("+"));
        assert!(p.needs_confirm(UNOBSERVED_FOCUS));
        assert!(!p
            .clone()
            .with_confirmed(UNOBSERVED_FOCUS)
            .needs_confirm(UNOBSERVED_FOCUS));
        assert!(!policy("", "").needs_confirm(UNOBSERVED_FOCUS));
        for form in [
            FormSubmit::NotInForm,
            FormSubmit::NoSubmitButton,
            FormSubmit::Button {
                label: "Search".into(),
                action: None,
            },
        ] {
            let f = field(Some(form.clone()));
            assert_eq!(
                p.check(url, &f, Action::Key, Some("Enter")),
                Ok(()),
                "{form:?}"
            );
        }
        // Unread form evidence fails closed while any pattern is set, until
        // the call confirms the placeholder label.
        let unknown = field(None);
        assert_eq!(p.check(url, &unknown, Action::Key, Some("Enter")), needs);
        assert_eq!(
            p.press_label(&unknown, Action::Key, Some("Enter")),
            Some(FORM_SUBMIT_UNKNOWN)
        );
        let lifted = p.clone().with_confirmed(FORM_SUBMIT_UNKNOWN);
        assert_eq!(
            lifted.check(url, &unknown, Action::Key, Some("Enter")),
            Ok(())
        );
        assert_eq!(
            policy("", "").check(url, &unknown, Action::Key, Some("Enter")),
            Ok(())
        );
        assert_eq!(
            policy("", "").check(url, &unknown, Action::Key, None),
            Ok(())
        );
    }

    /// Enter on a checkbox (or radio) in a form submits the form, which
    /// presses its submit button: that label is checked, not the box's.
    /// Space only toggles the box: its own label. A submit button with no
    /// name fails closed as an unknown submission.
    #[test]
    fn enter_on_a_box_submits_its_form_and_a_nameless_submit_fails_closed() {
        use ultra_instinct_core::{ElementState, FormSubmit, Role};
        let m = parse_fixture(PAGE).unwrap();
        let in_form = |role: Role, label: &str, actions: &[Action], form: FormSubmit| {
            let mut parts = m
                .get(&RegionId::try_new("pay").unwrap())
                .unwrap()
                .to_parts();
            parts.role = role;
            parts.label = label.into();
            parts.actions = actions.to_vec();
            InteractionRegion::try_new(parts)
                .unwrap()
                .with_state(ElementState {
                    form: Some(form),
                    ..ElementState::default()
                })
        };
        let p = policy("", "pay*, accept*");
        let url = Some("https://shop.example/");
        let pay = FormSubmit::Button {
            label: "Pay $18".into(),
            action: None,
        };
        let terms = in_form(
            Role::Checkbox,
            "Accept terms",
            &[Action::Click, Action::Toggle],
            pay.clone(),
        );
        assert_eq!(
            p.press_label(&terms, Action::Key, Some("Enter")),
            Some("Pay $18")
        );
        let confirmed = p.clone().with_confirmed("Accept terms");
        assert_eq!(
            confirmed.check(url, &terms, Action::Key, Some("Enter")),
            Err(GuardReason::NeedsConfirm)
        );
        assert_eq!(
            p.press_label(&terms, Action::Key, Some(" ")),
            Some("Accept terms")
        );
        // A box in no form: Enter is taken as pressing the box itself.
        let lone = in_form(
            Role::Checkbox,
            "Accept terms",
            &[Action::Click, Action::Toggle],
            FormSubmit::NotInForm,
        );
        assert_eq!(
            p.press_label(&lone, Action::Key, Some("Enter")),
            Some("Accept terms")
        );
        // A button presses itself, in a form or not.
        let go = in_form(Role::Button, "Continue", &[Action::Click], pay);
        assert_eq!(
            p.press_label(&go, Action::Key, Some("Enter")),
            Some("Continue")
        );
        // An icon-only submit button has no name: unknown, so it needs a confirm.
        for name in ["", "   "] {
            let field = in_form(
                Role::TextField,
                "Email",
                &[Action::Click, Action::Type, Action::Key],
                FormSubmit::Button {
                    label: name.into(),
                    action: None,
                },
            );
            assert_eq!(
                p.press_label(&field, Action::Key, Some("Enter")),
                Some(FORM_SUBMIT_UNKNOWN)
            );
            assert_eq!(
                p.check(url, &field, Action::Key, Some("Enter")),
                Err(GuardReason::NeedsConfirm)
            );
        }
    }

    /// A press that submits a form is checked against where the form is
    /// sent: an off-list, `javascript:` or unread target refuses
    /// `domain-not-allowed` naming it, while the allow-list is set; the
    /// page itself (an empty action) and on-list hosts pass.
    #[test]
    fn a_form_submit_to_an_off_list_host_is_refused() {
        use std::sync::Arc;
        use ultra_instinct_core::{ElementState, FormSubmit};
        let m = parse_fixture(PAGE).unwrap();
        let with = |id: &str, actions: &[Action], state: ElementState| {
            let mut parts = m.get(&RegionId::try_new(id).unwrap()).unwrap().to_parts();
            parts.actions = actions.to_vec();
            InteractionRegion::try_new(parts).unwrap().with_state(state)
        };
        let p = policy("shop.example", "");
        let url = Some("https://shop.example/join");
        let field = |action: Option<&str>| {
            with(
                "name",
                &[Action::Click, Action::Type, Action::Key],
                ElementState {
                    form: Some(FormSubmit::Button {
                        label: "Join".into(),
                        action: action.map(Arc::from),
                    }),
                    ..ElementState::default()
                },
            )
        };
        let enter =
            |region: &InteractionRegion| p.check_detail(url, region, Action::Key, Some("Enter"));
        let refused = |target: Option<&str>| {
            Err(Refusal {
                reason: GuardReason::DomainNotAllowed,
                label: "Join".into(),
                url: target.map(str::to_owned),
            })
        };
        assert_eq!(
            enter(&field(Some("https://evil.test/collect"))),
            refused(Some("https://evil.test/collect"))
        );
        assert_eq!(
            enter(&field(Some("javascript:send()"))),
            refused(Some("javascript:send()"))
        );
        assert_eq!(enter(&field(None)), refused(None));
        assert_eq!(enter(&field(Some("https://shop.example/join"))), Ok(()));
        // A submit button submits when clicked; the page itself passes.
        let submit = |target: &str| {
            with(
                "pay",
                &[Action::Click],
                ElementState {
                    submit_action: Some(target.into()),
                    ..ElementState::default()
                },
            )
        };
        assert_eq!(
            p.check(url, &submit("https://evil.test/pay"), Action::Click, None),
            Err(GuardReason::DomainNotAllowed)
        );
        assert_eq!(
            p.check(url, &submit(""), Action::Click, None),
            Err(GuardReason::DomainNotAllowed)
        );
        assert_eq!(
            p.check(
                url,
                &submit("https://shop.example/pay"),
                Action::Click,
                None
            ),
            Ok(())
        );
        // With no allow-list the target is not checked; typing never submits.
        let off = field(Some("https://evil.test/collect"));
        assert_eq!(
            policy("", "").check(url, &off, Action::Key, Some("Enter")),
            Ok(())
        );
        assert_eq!(p.check(url, &off, Action::Type, None), Ok(()));
        assert_eq!(p.check(url, &off, Action::Key, Some("Tab")), Ok(()));
    }

    #[test]
    fn bad_entries_are_errors() {
        assert!(HostPattern::parse("").is_err());
        assert!(HostPattern::parse("https://shop.example").is_err());
        assert!(HostPattern::parse("shop.example/path").is_err());
        assert!(HostPattern::parse("*.").is_err());
        assert!(HostPattern::parse("a*b.example").is_err());
        assert!(LabelGlob::parse("   ").is_err());
        assert!(!Policy::from_lists("ok.example,,", " , ")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn checked_consume_refuses_before_consuming() {
        let m = parse_fixture(PAGE).unwrap();
        let pay = RegionId::try_new("pay").unwrap();
        let ticket = crate::gate(&m, &pay, Action::Click, None, 0).unwrap();
        let mut ledger = TicketLedger::new();
        let mut pressed = 0;
        let err = consume_ticket_once_checked(
            &mut ledger,
            &ticket,
            &m,
            None,
            &policy("shop.example", ""),
            Some("https://evil.test/"),
            |_, _| {
                pressed += 1;
                Ok::<(), ()>(())
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConsumeError::Refused(Refusal {
                reason: GuardReason::DomainNotAllowed,
                ref label,
                url: Some(ref url),
            }) if label == "Pay $18" && url == "https://evil.test/"
        ));
        assert!(!ledger.is_consumed(ticket.ticket_id));
        assert_eq!(pressed, 0);
        consume_ticket_once_checked(
            &mut ledger,
            &ticket,
            &m,
            None,
            &policy("shop.example", ""),
            Some("https://shop.example/"),
            |_, _| {
                pressed += 1;
                Ok::<(), ()>(())
            },
        )
        .unwrap();
        assert!(ledger.is_consumed(ticket.ticket_id));
        assert_eq!(pressed, 1);
    }
}
