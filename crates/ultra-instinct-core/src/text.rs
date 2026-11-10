/// Lowercase tokens split on any character that is not alphanumeric.
/// Used by semantic scoring and by the hyperdimensional label binding.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                buf.push(lower);
            }
        } else if !buf.is_empty() {
            tokens.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        tokens.push(buf);
    }
    tokens
}

/// Fraction of `query` tokens that appear at least once in `candidate`.
///
/// Two empty tokenizations score `1.0` (nothing was asked, nothing contradicted).
/// A non-empty query against an empty candidate scores `0.0`.
pub fn token_recall(query: &str, candidate: &str) -> f64 {
    let wanted = tokenize(query);
    if wanted.is_empty() {
        return 1.0;
    }
    let have = tokenize(candidate);
    let hits = wanted
        .iter()
        .filter(|token| have.iter().any(|got| got == *token))
        .count();
    hits as f64 / wanted.len() as f64
}

/// Fraction of `candidate` tokens that appear at least once in `query`.
///
/// This is [`token_recall`] with the roles swapped: a label with words the
/// query did not ask for scores below `1.0`. Two empty tokenizations score
/// `1.0`. A non-empty query against an empty candidate scores `0.0`.
pub fn token_precision(query: &str, candidate: &str) -> f64 {
    let have = tokenize(candidate);
    if have.is_empty() {
        return if tokenize(query).is_empty() { 1.0 } else { 0.0 };
    }
    let wanted = tokenize(query);
    let hits = have
        .iter()
        .filter(|token| wanted.iter().any(|asked| asked == *token))
        .count();
    hits as f64 / have.len() as f64
}

/// Characters that render as nothing inside a word (zero-width space /
/// joiners, word joiner, BOM, combining grapheme joiner, soft hyphen):
/// "S\u{34F}p\u{34F}o…" reads "Sponsored". Observe drops them from labels
/// (the browser walk's JS `fold` carries the same list).
pub const INVISIBLE: [char; 7] = [
    '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{034F}', '\u{00AD}',
];

/// A label as matched: [`INVISIBLE`] characters dropped, whitespace runs
/// collapsed to one space, trimmed. Case is left to the caller.
pub fn normalize_label(label: &str) -> String {
    let kept: String = label.chars().filter(|c| !INVISIBLE.contains(c)).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Jaccard index over token sets. Empty/empty is `1.0`.
pub fn token_jaccard(left: &str, right: &str) -> f64 {
    let a = tokenize(left);
    let b = tokenize(right);
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut intersection = 0usize;
    let mut union = b.len();
    for token in &a {
        if b.iter().any(|other| other == token) {
            intersection += 1;
        } else {
            union += 1;
        }
    }
    if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_label_drops_invisible_characters_and_folds_whitespace() {
        assert_eq!(normalize_label("S\u{34F}p\u{AD}o\u{200B}n"), "Spon");
        assert_eq!(normalize_label("  a \u{FEFF}  B\t"), "a B");
        assert_eq!(normalize_label("\u{200B}\u{200C}"), "");
    }
}
