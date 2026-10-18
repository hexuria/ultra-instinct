//! Site and situation keys.
//!
//! A situation key scopes lessons: it is `fnv-1a-64` over
//! `host | path | clause | front_layer | roles | near-target labels`.
//! Numeric path segments collapse to `{n}` so `/post/123` and `/post/456`
//! share a lesson, but two different pages or clauses never do — a lesson
//! must stay per-situation, never global.

use crate::line::SiteLine;
use crate::line::Situation;

/// Build a [`SiteLine`] from a URL + title (both optional — fixtures and
/// mocks have neither).
#[must_use]
pub fn site_line(url: Option<&str>, title: Option<&str>) -> Option<SiteLine> {
    if url.is_none() && title.is_none() {
        return None;
    }
    let (host, path) = url.map(url_host_path).unwrap_or_default();
    Some(SiteLine {
        url: url.map(str::to_owned),
        title: title.map(str::to_owned),
        host,
        path,
    })
}

/// `(host, path)` where path has every all-digits segment → `{n}`.
fn url_host_path(url: &str) -> (Option<String>, Option<String>) {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let normalized = path
        .split('/')
        .map(|seg| {
            if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
                "{n}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    (
        (!host.is_empty()).then(|| host.to_owned()),
        Some(normalized.to_owned()),
    )
}

fn key_of(site: Option<&SiteLine>, situation: &Situation, clause: &str, near: bool) -> String {
    let mut key = String::with_capacity(128);
    if let Some(site) = site {
        key.push_str(site.host.as_deref().unwrap_or(""));
        key.push('|');
        key.push_str(site.path.as_deref().unwrap_or(""));
        key.push('|');
    }
    key.push_str(clause);
    key.push('|');
    key.push_str(if situation.front_layer { "fl" } else { "-" });
    key.push('|');
    key.push_str(&situation.roles.join(","));
    if near {
        key.push('|');
        key.push_str(&situation.near.join(","));
    }
    format!("{:016x}", fnv1a64(key.as_bytes()))
}

/// The per-situation lesson key: same page shape + same clause + same
/// neighborhood → same key. Descriptive lessons (words, places) use this.
#[must_use]
pub fn situation_key(site: Option<&SiteLine>, situation: &Situation, clause: &str) -> String {
    key_of(site, situation, clause, true)
}

/// The decision-time lesson key: same as [`situation_key`] minus `near`
/// (a post-decision artifact a deciding policy cannot know yet). Moves
/// and trust — the maps a live policy looks up — key on this so the
/// `lessons` command and the dojo agree on the same keys.
#[must_use]
pub fn context_key(site: Option<&SiteLine>, situation: &Situation, clause: &str) -> String {
    key_of(site, situation, clause, false)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_path_segments_collapse() {
        let site = site_line(Some("https://ex.com/post/123/x/7"), None).unwrap();
        assert_eq!(site.host.as_deref(), Some("ex.com"));
        assert_eq!(site.path.as_deref(), Some("/post/{n}/x/{n}"));
    }

    #[test]
    fn situation_key_scoped_by_page_and_clause() {
        let a = site_line(Some("https://a.com/x"), None);
        let b = site_line(Some("https://b.com/x"), None);
        let sit = Situation::default();
        assert_ne!(
            situation_key(a.as_ref(), &sit, "go"),
            situation_key(b.as_ref(), &sit, "go")
        );
        assert_ne!(
            situation_key(a.as_ref(), &sit, "go"),
            situation_key(a.as_ref(), &sit, "back")
        );
    }
}
