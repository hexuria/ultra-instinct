//! The user's policy: which sites may receive input, which presses need a
//! confirm first.
//!
//! A [`Policy`] is a hard refusal next to the gate, never a score. An empty
//! policy allows everything (the behaviour without one).
//!
//! - **Domains** ([`Policy::allow_domains`]): when the list is non-empty, a
//!   page whose URL host matches no [`HostPattern`] gets no input
//!   ([`GuardReason::DomainNotAllowed`]). A URL with no host (`about:blank`,
//!   `file:///…`, fixtures) is allowed. An *unknown* URL (the observation
//!   could not read one) is refused while the list is non-empty.
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

use ultra_instinct_core::{Action, FormSubmit, InteractionManifold, InteractionRegion, RegionId};
use ultra_instinct_protocol::{ActionTicket, GuardReason, TicketInvalid};

use crate::ticket::{revalidate, ConsumeError, TicketLedger};

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
/// matches itself case-insensitively. Runs of whitespace count as one space.
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
        })
    }

    /// This policy with one more confirmed label.
    pub fn with_confirmed(mut self, label: impl Into<String>) -> Self {
        self.confirmed.push(label.into());
        self
    }

    /// May the browser go to (or give input on) `url`?
    pub fn check_url(&self, url: &str) -> Result<(), GuardReason> {
        if self.allow_domains.is_empty() {
            return Ok(());
        }
        match host_of(url) {
            None => Ok(()),
            Some(host) if self.allow_domains.iter().any(|p| p.matches(&host)) => Ok(()),
            Some(_) => Err(GuardReason::DomainNotAllowed),
        }
    }

    /// May `region` on the page at `url` receive `action`? `url` is `None`
    /// when the observation could not read it; `key` is a KEY's key (`None`
    /// for other actions; a KEY without one is taken as Enter).
    pub fn check(
        &self,
        url: Option<&str>,
        region: &InteractionRegion,
        action: Action,
        key: Option<&str>,
    ) -> Result<(), GuardReason> {
        match url {
            Some(url) => self.check_url(url)?,
            None if !self.allow_domains.is_empty() => return Err(GuardReason::DomainNotAllowed),
            None => {}
        }
        match self.press_label(region, action, key) {
            Some(label) if self.needs_confirm(label) => Err(GuardReason::NeedsConfirm),
            _ => Ok(()),
        }
    }

    /// The label a confirm is checked against when `region` gets `action`
    /// (`key` for a KEY), `None` when it presses nothing:
    ///
    /// - the CLICK family (click, toggle, surface point): the target's label;
    /// - an activating KEY (Enter / Space / NumpadEnter, any modifiers) on a
    ///   button-like target (it clicks, toggles or takes points, and takes
    ///   no text or select): the target's label;
    /// - Enter / NumpadEnter in any other form control: the label of the
    ///   button its form's implicit submission presses (observe evidence,
    ///   [`FormSubmit`]); none when it is in no form or its form does not
    ///   submit on Enter; [`FORM_SUBMIT_UNKNOWN`] when the form submits with
    ///   no button or observe could not read it (then any confirm pattern
    ///   refuses).
    ///
    /// Space in a field is text; other keys press nothing.
    pub fn press_label<'a>(
        &self,
        region: &'a InteractionRegion,
        action: Action,
        key: Option<&str>,
    ) -> Option<&'a str> {
        if is_press(action) {
            return Some(region.label());
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
        if !takes_text && acts.iter().any(|a| is_press(*a)) {
            return Some(region.label());
        }
        if !is_enter_key(key) {
            return None;
        }
        match &region.state().form {
            Some(FormSubmit::Button(label)) => Some(label.as_str()),
            Some(FormSubmit::NotInForm | FormSubmit::NoSubmitButton) => None,
            Some(FormSubmit::Implicit) | None => Some(FORM_SUBMIT_UNKNOWN),
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

/// The lowercase host of `url`, `None` when it has none (`about:blank`,
/// `file:///…`, `data:…`, a bare path). The authority ends at `/ ? # \`
/// (browsers read `\` as `/`); userinfo and port are dropped.
pub fn host_of(url: &str) -> Option<String> {
    let (_, rest) = url.trim().split_once("://")?;
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
        .check(url, region, ticket.action, ticket.key.as_deref())
        .map_err(ConsumeError::Refused)?;
    ledger
        .mark_consumed(ticket.ticket_id)
        .map_err(ConsumeError::Invalid)?;
    press(&ticket.target_id, ticket.action).map_err(ConsumeError::Press)
}

fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Lowercase, whitespace runs folded to one space, trimmed.
fn fold(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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
        // No host: allowed.
        assert_eq!(p.check_url("about:blank"), Ok(()));
        assert_eq!(p.check_url("file:///tmp/page.html"), Ok(()));
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

        let in_pay_form = field(Some(FormSubmit::Button("Pay $18".into())));
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
        let implicit = field(Some(FormSubmit::Implicit));
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
            FormSubmit::Button("Search".into()),
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
            ConsumeError::Refused(GuardReason::DomainNotAllowed)
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
