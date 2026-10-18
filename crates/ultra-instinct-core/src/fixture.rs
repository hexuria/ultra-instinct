use crate::error::{CoreError, FixtureError};
use crate::id::{RegionId, UnitInterval};
use crate::manifold::InteractionManifold;
use crate::rect::Rect;
use crate::region::{InteractionRegion, RegionParts};
use crate::vocab::{Action, RegionFlags, Role, SourceMask};

/// Serialize a manifold into the fixture line format.
///
/// This is the same grammar [`parse_fixture`] reads. It is not a second
/// parser. A viewport whose origin is not `(0, 0)` cannot be represented;
/// that returns [`FixtureError::UnsupportedViewportOrigin`] instead of
/// silently dropping the origin.
pub fn write_fixture(manifold: &InteractionManifold) -> Result<String, FixtureError> {
    let viewport = manifold.viewport();
    if viewport.x() != 0.0 || viewport.y() != 0.0 {
        return Err(FixtureError::UnsupportedViewportOrigin);
    }
    let mut out = String::new();
    out.push_str(&format!(
        "viewport w={} h={}\n",
        format_num(viewport.width()),
        format_num(viewport.height())
    ));
    for region in manifold.regions() {
        out.push_str("region");
        push_raw(&mut out, "id", region.id().as_str());
        push_raw(&mut out, "role", region.role().as_str());
        push_raw(&mut out, "label", &quote(region.label()));
        let rect = region.rect();
        push_raw(&mut out, "x", &format_num(rect.x()));
        push_raw(&mut out, "y", &format_num(rect.y()));
        push_raw(&mut out, "w", &format_num(rect.width()));
        push_raw(&mut out, "h", &format_num(rect.height()));
        if !region.actions().is_empty() {
            let actions = region
                .actions()
                .iter()
                .map(|action| action.as_str())
                .collect::<Vec<_>>()
                .join(",");
            push_raw(&mut out, "actions", &actions);
        }
        if let Some(parent) = region.parent() {
            push_raw(&mut out, "parent", parent.as_str());
        }
        if region.sources() != SourceMask::NONE {
            push_raw(&mut out, "sources", &format_sources(region.sources()));
        }
        if region.flags().bits() != 0 {
            push_raw(&mut out, "flags", &format_flags(region.flags()));
        }
        if region.temporal_stability().get() != UnitInterval::ONE.get() {
            push_raw(
                &mut out,
                "stability",
                &format_num(region.temporal_stability().get()),
            );
        }
        out.push('\n');
    }
    Ok(out)
}

fn push_raw(out: &mut String, key: &str, value: &str) {
    out.push(' ');
    out.push_str(key);
    out.push('=');
    out.push_str(value);
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' | '"' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn format_num(value: f64) -> String {
    // Debug for f64 is a round-trip format.
    format!("{value:?}")
}

fn format_sources(mask: SourceMask) -> String {
    let mut names = Vec::new();
    for source in mask.iter() {
        let name = match source.bits() {
            0b0001 => "dom",
            0b0010 => "accessibility",
            0b0100 => "screenshot",
            0b1000 => "cua",
            _ => continue,
        };
        names.push(name);
    }
    names.join(",")
}

fn format_flags(flags: RegionFlags) -> String {
    let mut names = Vec::new();
    if flags.disabled() {
        names.push("disabled");
    }
    if flags.hidden() {
        names.push("hidden");
    }
    if flags.occluded() {
        names.push("occluded");
    }
    if flags.offscreen() {
        names.push("offscreen");
    }
    if flags.stale() {
        names.push("stale");
    }
    if flags.ambiguous() {
        names.push("ambiguous");
    }
    if flags.detached() {
        names.push("detached");
    }
    if flags.modal() {
        names.push("modal");
    }
    if flags.readonly() {
        names.push("readonly");
    }
    names.join(",")
}

/// Parse a static manifold fixture.
///
/// ```text
/// viewport w=1440 h=900
/// region id=nav-settings role=button label="Settings" x=16 y=180 w=200 h=36 actions=click parent=nav sources=dom,accessibility
/// ```
///
/// Lines starting with `#` and blank lines are ignored. `viewport` is required
/// once. Region keys: `id`, `role`, `label`, `x`, `y`, `w`, `h`, and optional
/// `actions`, `parent`, `sources`, `flags`, `stability`.
pub fn parse_fixture(input: &str) -> Result<InteractionManifold, FixtureError> {
    let mut viewport: Option<Rect> = None;
    let mut regions = Vec::new();
    for (idx, raw) in input.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let tokens = split_tokens(line).map_err(|message| FixtureError::Line {
            line: line_no,
            message,
        })?;
        if tokens.is_empty() {
            continue;
        }
        match tokens[0].as_str() {
            "viewport" => {
                if viewport.is_some() {
                    return Err(FixtureError::DuplicateViewport { line: line_no });
                }
                viewport =
                    Some(
                        parse_viewport(&tokens[1..]).map_err(|message| FixtureError::Line {
                            line: line_no,
                            message,
                        })?,
                    );
            }
            "region" => {
                let region = parse_region(&tokens[1..]).map_err(|message| FixtureError::Line {
                    line: line_no,
                    message,
                })?;
                if regions
                    .iter()
                    .any(|existing: &InteractionRegion| existing.id() == region.id())
                {
                    return Err(FixtureError::DuplicateRegion {
                        line: line_no,
                        id: region.id().to_string(),
                    });
                }
                regions.push(region);
            }
            other => {
                return Err(FixtureError::Line {
                    line: line_no,
                    message: format!("unknown directive `{other}`"),
                });
            }
        }
    }
    let viewport = viewport.ok_or(FixtureError::MissingViewport)?;
    InteractionManifold::try_new(viewport, regions, 0).map_err(|err| match err {
        CoreError::DuplicateRegion(id) => FixtureError::DuplicateRegion { line: 0, id },
        other => FixtureError::Line {
            line: 0,
            message: other.to_string(),
        },
    })
}

fn strip_comment(line: &str) -> &str {
    // Quotes opened by an escaped `\"` do not toggle quote state, matching
    // split_tokens; without this a label holding `"` followed by `#` would
    // truncate the line mid-token and fail to re-parse.
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn split_tokens(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '\\' if in_quotes => {
                let Some(escaped) = chars.next() else {
                    return Err("trailing escape in quoted string".to_owned());
                };
                match escaped {
                    'n' => buf.push('\n'),
                    't' => buf.push('\t'),
                    other => buf.push(other),
                }
            }
            ch if ch.is_whitespace() && !in_quotes => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            ch => buf.push(ch),
        }
    }
    if in_quotes {
        return Err("unclosed quote".to_owned());
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    Ok(out)
}

fn parse_viewport(tokens: &[String]) -> Result<Rect, String> {
    let map = pairs(tokens)?;
    let width = required_f64(&map, "w")?;
    let height = required_f64(&map, "h")?;
    Rect::try_viewport(0.0, 0.0, width, height).map_err(|err| err.to_string())
}

fn parse_region(tokens: &[String]) -> Result<InteractionRegion, String> {
    let map = pairs(tokens)?;
    let id = RegionId::try_new(required(&map, "id")?).map_err(|err| err.to_string())?;
    let role = Role::parse(required(&map, "role")?)
        .ok_or_else(|| format!("unknown role `{}`", required(&map, "role").unwrap_or("")))?;
    let label = required(&map, "label")?.to_owned();
    let rect = Rect::try_new(
        required_f64(&map, "x")?,
        required_f64(&map, "y")?,
        required_f64(&map, "w")?,
        required_f64(&map, "h")?,
    )
    .map_err(|err| err.to_string())?;
    let actions = match optional(&map, "actions") {
        None => Vec::new(),
        Some("") => Vec::new(),
        Some(raw) => {
            let mut actions = Vec::new();
            for part in raw.split(',') {
                let action = Action::parse(part.trim())
                    .ok_or_else(|| format!("unknown action `{}`", part.trim()))?;
                actions.push(action);
            }
            actions
        }
    };
    let parent = match optional(&map, "parent") {
        None | Some("") => None,
        Some(raw) => Some(RegionId::try_new(raw).map_err(|err| err.to_string())?),
    };
    let sources = match optional(&map, "sources") {
        None => SourceMask::NONE,
        Some(raw) => SourceMask::parse_list(raw)?,
    };
    let flags = match optional(&map, "flags") {
        None => RegionFlags::none(),
        Some(raw) => RegionFlags::parse_list(raw)?,
    };
    let temporal_stability = match optional(&map, "stability") {
        None => UnitInterval::ONE,
        Some(raw) => {
            let value: f64 = raw
                .parse()
                .map_err(|_| format!("stability `{raw}` is not a number"))?;
            UnitInterval::try_new(value).map_err(|err| err.to_string())?
        }
    };
    InteractionRegion::try_new(RegionParts {
        id,
        role,
        label,
        rect,
        actions,
        parent,
        sources,
        flags,
        temporal_stability,
    })
    .map_err(|err| err.to_string())
}

fn pairs(tokens: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut map = Vec::new();
    for token in tokens {
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!("expected key=value, found `{token}`"));
        };
        if key.is_empty() {
            return Err(format!("empty key in `{token}`"));
        }
        if map.iter().any(|(existing, _)| existing == key) {
            return Err(format!("duplicate key `{key}`"));
        }
        map.push((key.to_owned(), value.to_owned()));
    }
    Ok(map)
}

fn optional<'a>(map: &'a [(String, String)], key: &str) -> Option<&'a str> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn required<'a>(map: &'a [(String, String)], key: &str) -> Result<&'a str, String> {
    optional(map, key).ok_or_else(|| format!("missing `{key}`"))
}

fn required_f64(map: &[(String, String)], key: &str) -> Result<f64, String> {
    let raw = required(map, key)?;
    raw.parse::<f64>()
        .map_err(|_| format!("`{key}` value `{raw}` is not a number"))
        .and_then(|value| {
            if value.is_finite() {
                Ok(value)
            } else {
                Err(format!("`{key}` must be finite"))
            }
        })
}
