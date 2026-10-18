//! Colour words for colour-only targets: a box or shape whose only name is
//! its paint is called what a reader would call it ("light green"), not
//! `rgb(140, 242, 140)`. Pure functions over CSS colour strings.

/// `(r, g, b)` of a CSS colour: `rgb()` / `rgba()` with alpha above zero,
/// `#rgb` / `#rrggbb`, or a basic keyword. `None` for anything else,
/// including fully transparent colours.
pub(crate) fn parse(css: &str) -> Option<(u8, u8, u8)> {
    let css = css.trim().to_ascii_lowercase();
    if let Some(inner) = css
        .strip_prefix("rgba(")
        .or_else(|| css.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        let channel = |i: usize| {
            parts
                .get(i)?
                .parse::<f64>()
                .ok()
                .map(|v| v.clamp(0.0, 255.0) as u8)
        };
        if parts.len() >= 4 && parts[3].parse::<f64>().ok()? <= 0.0 {
            return None;
        }
        return Some((channel(0)?, channel(1)?, channel(2)?));
    }
    if let Some(hex) = css.strip_prefix('#') {
        let digit = |s: &str| u8::from_str_radix(s, 16).ok();
        return match hex.len() {
            3 => {
                let d = |i: usize| digit(&hex[i..=i]).map(|v| v * 17);
                Some((d(0)?, d(1)?, d(2)?))
            }
            6 => Some((digit(&hex[0..2])?, digit(&hex[2..4])?, digit(&hex[4..6])?)),
            _ => None,
        };
    }
    KEYWORDS
        .iter()
        .find(|(name, _)| *name == css)
        .map(|(_, rgb)| *rgb)
}

const KEYWORDS: [(&str, (u8, u8, u8)); 20] = [
    ("black", (0, 0, 0)),
    ("white", (255, 255, 255)),
    ("gray", (128, 128, 128)),
    ("grey", (128, 128, 128)),
    ("silver", (192, 192, 192)),
    ("red", (255, 0, 0)),
    ("maroon", (128, 0, 0)),
    ("orange", (255, 165, 0)),
    ("yellow", (255, 255, 0)),
    ("olive", (128, 128, 0)),
    ("lime", (0, 255, 0)),
    ("green", (0, 128, 0)),
    ("teal", (0, 128, 128)),
    ("cyan", (0, 255, 255)),
    ("aqua", (0, 255, 255)),
    ("blue", (0, 0, 255)),
    ("navy", (0, 0, 128)),
    ("magenta", (255, 0, 255)),
    ("purple", (128, 0, 128)),
    ("pink", (255, 192, 203)),
];

/// Largest RGB distance at which a colour is called by a basic keyword.
const KEYWORD_NEAR: f64 = 24.0;

/// The colour family a reader would name: black / white / gray for
/// unsaturated colours, else a hue word (red, orange, yellow, green, cyan,
/// blue, purple, magenta, pink, brown), with "light" or "dark" when the
/// colour is far from mid lightness.
pub(crate) fn family(r: u8, g: u8, b: u8) -> String {
    // A colour at (or right by) a basic keyword is called by it: (128, 0,
    // 128) is "purple", not a dark magenta.
    let near = KEYWORDS.iter().find(|(_, (kr, kg, kb))| {
        let d = |a: u8, b: u8| (f64::from(a) - f64::from(b)).powi(2);
        (d(r, *kr) + d(g, *kg) + d(b, *kb)).sqrt() <= KEYWORD_NEAR
    });
    if let Some((name, _)) = near {
        return match *name {
            "lime" => "green",
            "aqua" => "cyan",
            "grey" => "gray",
            "maroon" => "dark red",
            "navy" => "dark blue",
            other => other,
        }
        .to_owned();
    }
    let (h, s, l) = hsl(r, g, b);
    if s < 0.15 || l < 0.08 || l > 0.95 {
        return match l {
            l if l < 0.2 => "black",
            l if l > 0.85 => "white",
            l if l < 0.45 => "dark gray",
            l if l > 0.7 => "light gray",
            _ => "gray",
        }
        .to_owned();
    }
    let hue = match h {
        h if !(15.0..345.0).contains(&h) => "red",
        h if h < 40.0 => "orange",
        h if h < 70.0 => "yellow",
        h if h < 165.0 => "green",
        h if h < 195.0 => "cyan",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        h if h < 320.0 => "magenta",
        _ => "pink",
    };
    // Dark oranges and reds read as brown.
    if matches!(hue, "orange" | "red") && l < 0.35 && s < 0.8 {
        return "brown".to_owned();
    }
    match l {
        l if l > 0.72 => format!("light {hue}"),
        l if l < 0.28 => format!("dark {hue}"),
        _ => hue.to_owned(),
    }
}

/// [`family`] of a CSS colour string, when it parses.
pub(crate) fn name(css: &str) -> Option<String> {
    parse(css).map(|(r, g, b)| family(r, g, b))
}

/// Hue in degrees, saturation and lightness in 0..=1.
fn hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_named_the_way_a_reader_names_them() {
        let n = |css: &str| name(css).unwrap();
        assert_eq!(n("rgb(255, 165, 0)"), "orange");
        assert_eq!(n("rgb(255, 0, 0)"), "red");
        assert_eq!(n("rgb(0, 0, 0)"), "black");
        assert_eq!(n("rgb(128, 0, 128)"), "purple");
        assert_eq!(n("rgb(140, 242, 140)"), "light green");
        assert_eq!(n("rgb(43, 182, 43)"), "green");
        assert_eq!(n("rgb(11, 11, 193)"), "blue");
        assert_eq!(n("rgb(201, 201, 243)"), "light blue");
        assert_eq!(n("rgb(175, 70, 70)"), "red");
        assert_eq!(n("#8b4513"), "brown");
        assert_eq!(n("#fff"), "white");
        assert_eq!(n("magenta"), "magenta");
        assert_eq!(n("rgba(10, 200, 10, 0.5)"), "green");
        assert_eq!(name("rgba(0, 0, 0, 0)"), None, "transparent names nothing");
        assert_eq!(name("currentColor"), None);
    }
}
