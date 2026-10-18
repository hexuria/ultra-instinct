//! Turn CDP JSON into raw nodes. This is the only parser of these result
//! shapes. Fusion does not parse CDP.
//!
//! `walk_dom` follows `children`, then open `shadowRoots`, then same-origin
//! `contentDocument` (iframe). That matches `DOM.getDocument` with
//! `pierce: true`. Closed shadow trees and cross-origin iframes stay invisible.

use std::collections::BTreeSet;

use serde_json::Value;

use aui_core::{Action, Rect, Role};

use aui_cdp::CdpError;

use crate::error::BrowserError;

#[derive(Clone, Debug)]
pub(crate) struct DomElement {
    pub node_id: i64,
    pub backend_node_id: i64,
    pub role: Role,
    pub label: String,
    pub actions: Vec<Action>,
    pub disabled: bool,
    pub hidden: bool,
    /// HTML `readonly` attribute or `aria-readonly="true"`.
    pub readonly: bool,
    /// `aria-modal="true"`. Only meaningful on a dialog.
    pub modal: bool,
    /// Backend ids of kept ancestors, nearest first.
    pub ancestors: Vec<i64>,
    /// `data-hu-k` injected by the compact walk, when observe ran it first.
    pub hu_k: Option<u32>,
    /// A text-less box kept only if the compact walk confirms a click hint;
    /// otherwise observe drops it (and spends no per-node call on it).
    pub needs_hint: bool,
    /// An HTML `canvas` or `svg` root. It claims `point` only with click
    /// evidence (a listener, markup, or a pointer cursor it sets itself).
    pub surface: bool,
    /// The label is only the words of its class or id ("more", "trash").
    pub named_by_markup: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AxElement {
    pub backend_dom_node_id: Option<i64>,
    pub role: Role,
    pub name: String,
    pub disabled: bool,
    pub focused: bool,
    /// The accessibility `modal` property. Chrome sets it on a dialog opened
    /// with `showModal()` or marked `aria-modal="true"`.
    pub modal: bool,
}

pub(crate) fn parse_viewport(result_json: &str) -> Result<aui_core::Rect, BrowserError> {
    let value = parse_json(result_json)?;
    let css = value
        .get("cssLayoutViewport")
        .ok_or_else(|| BrowserError::BadViewport("missing cssLayoutViewport".into()))?;
    let width = number(css, "clientWidth")
        .ok_or_else(|| BrowserError::BadViewport("missing clientWidth".into()))?;
    let height = number(css, "clientHeight")
        .ok_or_else(|| BrowserError::BadViewport("missing clientHeight".into()))?;
    // Region rects (box model, compact walk) are in viewport coordinates,
    // so the viewport sits at the origin; `pageX` / `pageY` is the scroll
    // offset, and using it would mark every visible region offscreen after
    // a scroll.
    Rect::try_viewport(0.0, 0.0, width, height)
        .map_err(|err| BrowserError::BadViewport(err.to_string()))
}

/// Kept interactive elements plus a parent map of every element node.
#[derive(Clone, Debug, Default)]
pub(crate) struct DomDocument {
    pub elements: Vec<DomElement>,
    /// Text-less boxes that become regions only with a compact click hint.
    pub hint_boxes: Vec<DomElement>,
    /// Every element backend id → its parent element backend id, when known.
    /// Used by hit-test to decide whether the node under a region's center is
    /// inside that region (the region owns the hit) or something else covers it.
    pub parent_of: std::collections::BTreeMap<i64, i64>,
    /// Element backend id → `data-hu-k`, for every element the compact walk
    /// tagged (present only when a compact eval ran before `DOM.getDocument`).
    pub hu_k_of_backend: std::collections::BTreeMap<i64, u32>,
    /// `data-hu-k` → element backend id, the reverse map for hit resolution.
    pub backend_of_hu_k: std::collections::BTreeMap<u32, i64>,
}

#[allow(dead_code)]
pub(crate) fn dom_elements(document_json: &str) -> Result<Vec<DomElement>, BrowserError> {
    Ok(dom_document(document_json)?.elements)
}

pub(crate) fn dom_document(document_json: &str) -> Result<DomDocument, BrowserError> {
    dom_document_with(document_json, &BTreeSet::new(), &BTreeSet::new())
}

/// [`dom_document`] with page evidence: `listeners` holds the backend ids
/// Chrome reports as responding to clicks (see [`clickable_backends`]);
/// `hidden` holds the `data-hu-k` of elements the compact walk found not
/// rendered (`display: none`), whose text names nothing.
pub(crate) fn dom_document_with(
    document_json: &str,
    listeners: &BTreeSet<i64>,
    hidden: &BTreeSet<u32>,
) -> Result<DomDocument, BrowserError> {
    let value = parse_json_deep(document_json)?;
    let root = value.get("root").ok_or_else(|| {
        BrowserError::Cdp(CdpError::BadJson {
            message: "DOM.getDocument result has no root".into(),
        })
    })?;
    let mut out = DomDocument::default();
    walk_dom(root, listeners, hidden, &mut out);
    drop_ambiguous_hu_k(&mut out);
    Ok(out)
}

/// Backend ids `DOMSnapshot.captureSnapshot` marks `isClickable`: Chrome's
/// own record of nodes with click, mouse, or pointer listeners (a handler
/// added by script with `addEventListener` included) and of links. Every
/// document in the snapshot counts (same-origin iframes are documents too).
pub(crate) fn clickable_backends(snapshot_json: &str) -> Result<BTreeSet<i64>, BrowserError> {
    let value = parse_json(snapshot_json)?;
    let documents = value
        .get("documents")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            BrowserError::Cdp(CdpError::BadJson {
                message: "DOMSnapshot.captureSnapshot result has no documents".into(),
            })
        })?;
    let mut out = BTreeSet::new();
    for document in documents {
        let nodes = document.get("nodes");
        let backends = nodes
            .and_then(|n| n.get("backendNodeId"))
            .and_then(Value::as_array);
        let clickable = nodes
            .and_then(|n| n.get("isClickable"))
            .and_then(|c| c.get("index"))
            .and_then(Value::as_array);
        let (Some(backends), Some(clickable)) = (backends, clickable) else {
            continue;
        };
        out.extend(
            clickable
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|i| backends.get(usize::try_from(i).ok()?))
                .filter_map(Value::as_i64),
        );
    }
    Ok(out)
}

/// A `data-hu-k` carried by more than one element (a page clone of a tagged
/// node, or a frame the walk could not reach forging a value) cannot be
/// joined to one compact record. Every element sharing it is untagged so it
/// takes the per-node fallback, and a compact hit naming it resolves to no
/// backend, which also falls back.
fn drop_ambiguous_hu_k(out: &mut DomDocument) {
    let mut seen = std::collections::BTreeMap::<u32, usize>::new();
    for k in out.hu_k_of_backend.values() {
        *seen.entry(*k).or_default() += 1;
    }
    let ambiguous: std::collections::BTreeSet<u32> = seen
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(k, _)| k)
        .collect();
    if ambiguous.is_empty() {
        return;
    }
    out.hu_k_of_backend.retain(|_, k| !ambiguous.contains(k));
    out.backend_of_hu_k.retain(|k, _| !ambiguous.contains(k));
    for element in &mut out.elements {
        if element.hu_k.is_some_and(|k| ambiguous.contains(&k)) {
            element.hu_k = None;
        }
    }
}

pub(crate) fn ax_elements(tree_json: &str) -> Result<Vec<AxElement>, BrowserError> {
    let value = parse_json(tree_json)?;
    let nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            BrowserError::Cdp(CdpError::BadJson {
                message: "Accessibility.getFullAXTree result has no nodes".into(),
            })
        })?;
    let mut out = Vec::new();
    for node in nodes {
        if node.get("ignored").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let role_value = node
            .get("role")
            .and_then(|role| role.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(role) = ax_role(role_value) else {
            continue;
        };
        let name = node
            .get("name")
            .and_then(|name| name.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let backend = node.get("backendDOMNodeId").and_then(Value::as_i64);
        let disabled = ax_flag(node, "disabled");
        let focused = ax_flag(node, "focused");
        let modal = ax_flag(node, "modal");
        out.push(AxElement {
            backend_dom_node_id: backend,
            role,
            name,
            disabled,
            focused,
            modal,
        });
    }
    Ok(out)
}

/// `None` when the node has no box (it is omitted, not stored as zero size).
pub(crate) fn content_rect(box_json: &str) -> Result<Option<Rect>, BrowserError> {
    let value = parse_json(box_json)?;
    let Some(content) = value
        .get("model")
        .and_then(|model| model.get("content"))
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    if content.len() < 8 {
        return Ok(None);
    }
    let mut xs = [0.0; 4];
    let mut ys = [0.0; 4];
    for index in 0..4 {
        xs[index] = content[index * 2].as_f64().ok_or_else(|| {
            BrowserError::Cdp(CdpError::BadJson {
                message: "box model content is not numeric".into(),
            })
        })?;
        ys[index] = content[index * 2 + 1].as_f64().ok_or_else(|| {
            BrowserError::Cdp(CdpError::BadJson {
                message: "box model content is not numeric".into(),
            })
        })?;
    }
    let min_x = xs.into_iter().fold(f64::INFINITY, f64::min);
    let max_x = xs.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.into_iter().fold(f64::INFINITY, f64::min);
    let max_y = ys.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let rect = Rect::try_new(min_x, min_y, max_x - min_x, max_y - min_y).map_err(|err| {
        BrowserError::Cdp(CdpError::BadJson {
            message: err.to_string(),
        })
    })?;
    Ok(Some(rect))
}

/// `backendNodeId` from `DOM.getNodeForLocation`.
pub(crate) fn location_backend(result_json: &str) -> Result<Option<i64>, BrowserError> {
    let value = parse_json(result_json)?;
    Ok(value.get("backendNodeId").and_then(Value::as_i64))
}

/// Name/value pairs from `CSS.getComputedStyleForNode`.
pub(crate) fn computed_style_pairs(
    result_json: &str,
) -> Result<Vec<(String, String)>, BrowserError> {
    let value = parse_json(result_json)?;
    let entries = value
        .get("computedStyle")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            BrowserError::Cdp(CdpError::BadJson {
                message: "CSS.getComputedStyleForNode result has no computedStyle".into(),
            })
        })?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let val = entry
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if !name.is_empty() {
            out.push((name, val));
        }
    }
    Ok(out)
}

pub(crate) fn object_id(resolve_json: &str) -> Result<String, BrowserError> {
    let value = parse_json(resolve_json)?;
    value
        .get("object")
        .and_then(|object| object.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(BrowserError::MissingObjectId)
}

/// `Runtime.callFunctionOn` reports a thrown click as `exceptionDetails`.
pub(crate) fn call_threw(call_json: &str) -> Result<bool, BrowserError> {
    let value = parse_json(call_json)?;
    Ok(value.get("exceptionDetails").is_some())
}

/// `kept_ancestors` holds the backend ids of kept elements above a node,
/// outermost first. Each kept element records them nearest first.
/// `parent_backend` is the nearest element ancestor (kept or not).
///
/// The walk is iterative — the document is parsed with an unbounded depth,
/// so a recursive descent would just move the stack wall from serde_json
/// into this walk. `Pop` restores the ancestor chain after a subtree.
enum DomWork<'a> {
    Visit(&'a Value, Option<i64>),
    Pop,
    /// Leave an element: restore the element chain.
    PopNode,
}

/// Longest row text added to a markup-named label.
const ROW_CONTEXT_MAX_CHARS: usize = 60;

fn walk_dom(
    root: &Value,
    listeners: &BTreeSet<i64>,
    hidden: &BTreeSet<u32>,
    out: &mut DomDocument,
) {
    let mut kept_ancestors: Vec<i64> = Vec::new();
    // Parallel to `kept_ancestors`: that kept element is a click control.
    let mut kept_controls: Vec<bool> = Vec::new();
    // Every element above the current node, outermost first.
    let mut chain: Vec<&Value> = Vec::new();
    let mut stack = vec![DomWork::Visit(root, None)];
    while let Some(work) = stack.pop() {
        let (node, parent_backend) = match work {
            DomWork::Pop => {
                kept_ancestors.pop();
                kept_controls.pop();
                continue;
            }
            DomWork::PopNode => {
                chain.pop();
                continue;
            }
            DomWork::Visit(node, parent_backend) => (node, parent_backend),
        };
        let node_type = node.get("nodeType").and_then(Value::as_i64).unwrap_or(1);
        let mut next_parent = parent_backend;
        let mut pushed_kept = false;
        if node_type == 1 {
            if let Some(backend) = node.get("backendNodeId").and_then(Value::as_i64) {
                if let Some(parent) = parent_backend {
                    out.parent_of.insert(backend, parent);
                }
                next_parent = Some(backend);
                if let Some(k) = attr_map(node)
                    .get(crate::compact::HU_K_ATTR)
                    .and_then(|raw| raw.parse::<u32>().ok())
                {
                    out.hu_k_of_backend.insert(backend, k);
                    out.backend_of_hu_k.insert(k, backend);
                }
            }
            let svg_part = node
                .get("nodeName")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "svg" || is_svg_shape(name));
            // An icon drawn inside a button or link is that control's face,
            // not a target of its own.
            let face = svg_part && kept_controls.iter().any(|&control| control);
            let listener = node
                .get("backendNodeId")
                .and_then(Value::as_i64)
                .is_some_and(|backend| listeners.contains(&backend));
            if let Some(mut element) = element_from(node, listener, hidden).filter(|_| !face) {
                element.ancestors = kept_ancestors.iter().rev().copied().collect();
                // "more" or "trash" repeats in every row: the row's own text
                // (the nearest ancestor with any) says which one this is.
                if element.named_by_markup {
                    if let Some(row) = chain
                        .iter()
                        .rev()
                        .map(|n| subtree_text(n, hidden))
                        .find(|t| !t.is_empty())
                    {
                        let row: String = row.chars().take(ROW_CONTEXT_MAX_CHARS).collect();
                        element.label = format!("{} · {row}", element.label);
                    }
                }
                if element.needs_hint {
                    // Not a kept element (nor anyone's kept ancestor) until
                    // observe confirms its click hint.
                    out.hint_boxes.push(element);
                } else {
                    kept_ancestors.push(element.backend_node_id);
                    kept_controls.push(!svg_part && element.actions.contains(&Action::Click));
                    pushed_kept = true;
                    out.elements.push(element);
                }
            }
        }
        // Traversal order is children, then shadow roots, then the iframe
        // document — pushed in reverse onto a LIFO stack, with the ancestor
        // pop last.
        if pushed_kept {
            stack.push(DomWork::Pop);
        }
        if node_type == 1 {
            chain.push(node);
            stack.push(DomWork::PopNode);
        }
        // Same-origin iframe document. Cross-origin frames omit
        // `contentDocument`.
        if let Some(doc) = node.get("contentDocument") {
            stack.push(DomWork::Visit(doc, next_parent));
        }
        // Open shadow roots (closed ones are absent). CDP only populates
        // `shadowRoots` when `DOM.getDocument` was called with `pierce: true`.
        if let Some(roots) = node.get("shadowRoots").and_then(Value::as_array) {
            for root in roots.iter().rev() {
                stack.push(DomWork::Visit(root, next_parent));
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children.iter().rev() {
                stack.push(DomWork::Visit(child, next_parent));
            }
        }
    }
}

/// `listener`: Chrome reports a click listener on this node (`isClickable`).
fn element_from(node: &Value, listener: bool, hidden: &BTreeSet<u32>) -> Option<DomElement> {
    let name = node.get("nodeName").and_then(Value::as_str).unwrap_or("");
    if matches!(
        name.to_ascii_uppercase().as_str(),
        "SCRIPT" | "STYLE" | "HEAD" | "HTML" | "BODY" | "#DOCUMENT"
    ) {
        return None;
    }
    let attributes = attr_map(node);
    let svg_shape = is_svg_shape(name);
    let surface = is_surface(name);
    let aria = attributes
        .get("aria-label")
        .cloned()
        .filter(|label| !label.trim().is_empty());
    let mut label = if svg_shape {
        shape_label(node, name, &attributes, aria)
    } else if surface {
        surface_label(node, name, aria)
    } else {
        aria.unwrap_or_else(|| direct_text(node))
    };
    let role_attr = attributes.get("role").cloned();
    // Marked draggable in markup: kept even when its text sits in children
    // (a list row whose label is an inner element), named by that text.
    let drag_marked = !svg_shape && !surface && drag_markup(&attributes);
    if drag_marked && label.is_empty() {
        label = subtree_text(node, hidden);
    }
    let kept = keep_element(name, role_attr.as_deref(), &label);
    // A box the page made clickable (a colored square, a card, a board
    // cell, an icon) is a region when Chrome reports a click listener on
    // it, or when a click hint confirms it.
    let plain_box = !svg_shape && !surface && !drag_marked && !kept && is_box(name);
    let listener_box = plain_box && listener;
    let hint_box = plain_box && !listener;
    if !svg_shape && !surface && !drag_marked && !plain_box && !kept {
        return None;
    }
    // Named like a button, from its content (text in child elements); a
    // box with none falls back to its `title`, then the words of its class or
    // id ("trash", "open search"), then its inline background colour (a
    // colored square).
    let mut named_by_markup = false;
    let mut from_markup = |attributes: &std::collections::BTreeMap<String, String>| {
        let words = markup_words(attributes);
        named_by_markup = words.is_some();
        words
    };
    if plain_box && label.is_empty() {
        label = Some(subtree_text(node, hidden))
            .filter(|text| !text.is_empty())
            .or_else(|| attribute_text(&attributes, "title"))
            .or_else(|| from_markup(&attributes))
            .or_else(|| inline_style(&attributes, "background-color"))
            .unwrap_or_default();
    }
    // An icon-only control (a link or button with no text) is named by its
    // title, alt text, or markup words. Form fields are named by their own
    // labels in the accessibility tree.
    if kept && label.is_empty() && names_from_markup(name) {
        label = attribute_text(&attributes, "title")
            .or_else(|| attribute_text(&attributes, "alt"))
            .or_else(|| from_markup(&attributes))
            .unwrap_or_default();
    }
    let role = if svg_shape || surface {
        Role::Image
    } else {
        dom_role(
            name,
            role_attr.as_deref(),
            attributes.get("type").map(String::as_str),
        )
    };
    let node_id = node.get("nodeId").and_then(Value::as_i64)?;
    let backend_node_id = node
        .get("backendNodeId")
        .and_then(Value::as_i64)
        .unwrap_or(node_id);
    let disabled = attributes.contains_key("disabled")
        || attributes.get("aria-disabled").map(String::as_str) == Some("true");
    let hidden = attributes.contains_key("hidden")
        || attributes.get("aria-hidden").map(String::as_str) == Some("true");
    let readonly = attributes.contains_key("readonly")
        || attributes.get("aria-readonly").map(String::as_str) == Some("true");
    let modal = attributes.get("aria-modal").map(String::as_str) == Some("true");
    let hu_k = attributes
        .get(crate::compact::HU_K_ATTR)
        .and_then(|raw| raw.parse::<u32>().ok());
    let mut actions = dom_actions(name, role);
    let marked_clickable = attribute_click_hint(&attributes);
    if svg_shape || marked_clickable || listener {
        add_click(&mut actions);
    }
    // A surface takes point clicks only with click evidence: a display-only
    // canvas (a chart, a visualiser) is not a target. A pointer cursor it
    // sets itself also counts, in observe.
    if surface && (marked_clickable || listener) {
        actions.push(Action::Point);
    }
    if opens_on_hover(&attributes, role) && !actions.contains(&Action::Hover) {
        actions.push(Action::Hover);
    }
    if attribute_drag_hint(&attributes, role) {
        add_drag(&mut actions);
    }
    let needs_hint = hint_box && !marked_clickable;
    debug_assert!(!(listener_box && needs_hint));
    Some(DomElement {
        node_id,
        backend_node_id,
        role,
        label,
        actions,
        disabled,
        hidden,
        readonly,
        modal,
        ancestors: Vec::new(),
        hu_k,
        needs_hint,
        surface,
        named_by_markup,
    })
}

/// SVG shape elements. CDP reports SVG tag names in lower case and HTML ones
/// in upper case, so the match is case-sensitive on purpose. Shapes inside
/// `defs` / `mask` / `clipPath` have no layout box and drop out as unboxed.
fn is_svg_shape(name: &str) -> bool {
    matches!(
        name,
        "circle" | "ellipse" | "rect" | "polygon" | "polyline" | "path" | "text"
    )
}

/// Drawing surfaces: an HTML `canvas` or an `svg` root. Any point inside one
/// may be what a goal means ("click the center", "the point (3, 4)").
fn is_surface(name: &str) -> bool {
    name == "svg" || name.eq_ignore_ascii_case("CANVAS")
}

/// A surface's `aria-label`, else its `<title>`, else "canvas" / "drawing".
fn surface_label(node: &Value, name: &str, aria: Option<String>) -> String {
    let title = node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|c| c.get("nodeName").and_then(Value::as_str) == Some("title"))
        .map(direct_text)
        .filter(|t| !t.is_empty());
    aria.or(title)
        .unwrap_or_else(|| if name == "svg" { "drawing" } else { "canvas" }.to_owned())
}

/// Plain elements a page commonly makes clickable by script: text boxes,
/// table cells and rows, and inline icon tags. Mirrored by `BOXES` in the
/// compact walk, which collects their evidence.
fn is_box(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "LABEL"
            | "SPAN"
            | "P"
            | "DIV"
            | "LI"
            | "TD"
            | "TH"
            | "TR"
            | "I"
            | "B"
            | "EM"
            | "STRONG"
            | "SMALL"
            | "FIGURE"
            | "SECTION"
            | "ARTICLE"
    )
}

/// Kept controls whose empty label may come from markup: links, buttons,
/// images, and anything else that is not a form field.
fn names_from_markup(name: &str) -> bool {
    !matches!(
        name.to_ascii_uppercase().as_str(),
        "INPUT" | "TEXTAREA" | "SELECT" | "OPTION"
    )
}

/// One attribute's trimmed value, when it is not blank.
fn attribute_text(
    attributes: &std::collections::BTreeMap<String, String>,
    name: &str,
) -> Option<String> {
    attributes
        .get(name)
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// Class-name words that say how an element looks or where it sits, not
/// what it does.
const LAYOUT_WORDS: [&str; 26] = [
    "active",
    "block",
    "clearfix",
    "col",
    "container",
    "disabled",
    "fa",
    "fas",
    "far",
    "flex",
    "float",
    "glyphicon",
    "grid",
    "hidden",
    "icon",
    "icons",
    "inline",
    "inner",
    "js",
    "left",
    "outer",
    "right",
    "row",
    "selected",
    "ui",
    "wrapper",
];
/// Most markup words in a name.
const MARKUP_WORDS_MAX: usize = 3;

/// An element's own name in markup: the words of its `class`, else of its
/// `id` (`fa fa-trash` → "trash", `mark-o` → "mark o"). Words are letters
/// only, at most 20 long, without layout words.
fn markup_words(attributes: &std::collections::BTreeMap<String, String>) -> Option<String> {
    let words = |raw: &str| -> Option<String> {
        let mut out: Vec<String> = Vec::new();
        for word in raw
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
        {
            let plain =
                (1..=20).contains(&word.len()) && word.chars().all(|c| c.is_ascii_alphabetic());
            if plain && !LAYOUT_WORDS.contains(&word.as_str()) && !out.contains(&word) {
                out.push(word);
            }
            if out.len() == MARKUP_WORDS_MAX {
                break;
            }
        }
        (!out.is_empty()).then(|| out.join(" "))
    };
    ["class", "id"]
        .into_iter()
        .find_map(|key| attributes.get(key).and_then(|raw| words(raw)))
}

/// What a shape is called: its `<title>`, `aria-label`, its own text (SVG
/// `text`), else its fill and kind ("red circle"), else its kind.
fn shape_label(
    node: &Value,
    name: &str,
    attributes: &std::collections::BTreeMap<String, String>,
    aria: Option<String>,
) -> String {
    let title = node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|c| c.get("nodeName").and_then(Value::as_str) == Some("title"))
        .map(direct_text)
        .filter(|t| !t.is_empty());
    if let Some(text) = title.or(aria) {
        return text;
    }
    if name == "text" {
        return direct_text(node);
    }
    let fill = attributes
        .get("fill")
        .cloned()
        .or_else(|| inline_style(attributes, "fill"))
        .filter(|f| !f.is_empty() && f != "none");
    match fill {
        Some(fill) => format!("{fill} {name}"),
        None => name.to_owned(),
    }
}

/// One declaration from the inline `style` attribute, value trimmed.
fn inline_style(
    attributes: &std::collections::BTreeMap<String, String>,
    property: &str,
) -> Option<String> {
    attributes.get("style")?.split(';').find_map(|decl| {
        let (key, value) = decl.split_once(':')?;
        (key.trim().eq_ignore_ascii_case(property)).then(|| value.trim().to_owned())
    })
}

/// The page marked a plain element clickable in markup: an `onclick`
/// attribute, or a `tabindex` of 0 or more (a keyboard stop, which a page
/// gives a custom control).
fn attribute_click_hint(attributes: &std::collections::BTreeMap<String, String>) -> bool {
    attributes.contains_key("onclick")
        || attributes
            .get("tabindex")
            .and_then(|t| t.trim().parse::<i32>().ok())
            .is_some_and(|t| t >= 0)
}

/// The element says it opens something: `aria-haspopup` (not "false"),
/// `aria-expanded`, or a menu item, whose submenus commonly open on hover.
fn opens_on_hover(attributes: &std::collections::BTreeMap<String, String>, role: Role) -> bool {
    attributes
        .get("aria-haspopup")
        .is_some_and(|v| !v.eq_ignore_ascii_case("false"))
        || attributes.contains_key("aria-expanded")
        || role == Role::MenuItem
}

/// The element is marked draggable: HTML `draggable="true"`, ARIA
/// `aria-grabbed`, a slider (its thumb drags), or the marker classes a common
/// widget library puts on draggable, sortable, slider, and resize handles.
fn attribute_drag_hint(
    attributes: &std::collections::BTreeMap<String, String>,
    role: Role,
) -> bool {
    role == Role::Slider || drag_markup(attributes)
}

/// Draggable in markup alone (no role): `draggable="true"`, `aria-grabbed`,
/// or a common widget library's drag / sort / slider / resize handle class.
/// Mirrored by `dragMarked` in the compact walk.
fn drag_markup(attributes: &std::collections::BTreeMap<String, String>) -> bool {
    const LIBRARY_MARKERS: [&str; 4] = [
        "ui-draggable",
        "ui-sortable-handle",
        "ui-slider-handle",
        "ui-resizable-handle",
    ];
    attributes
        .get("draggable")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        || attributes.contains_key("aria-grabbed")
        || attributes
            .get("class")
            .is_some_and(|c| c.split_whitespace().any(|t| LIBRARY_MARKERS.contains(&t)))
}

/// Longest label taken from an element's descendants.
const SUBTREE_TEXT_MAX_CHARS: usize = 80;

/// All shown text under `node`, in document order, whitespace folded and
/// capped. Subtrees in `hidden` (not rendered: `display: none`) are skipped.
fn subtree_text(node: &Value, hidden: &BTreeSet<u32>) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut seen = 0usize;
    let mut stack: Vec<&Value> = vec![node];
    while let Some(n) = stack.pop() {
        if n.get("nodeType").and_then(Value::as_i64) == Some(3) {
            if let Some(t) = n.get("nodeValue").and_then(Value::as_str) {
                parts.push(t);
                seen += t.len();
                // Enough raw text for the cap even after whitespace folds.
                if seen > SUBTREE_TEXT_MAX_CHARS * 8 {
                    break;
                }
            }
        }
        let name = n.get("nodeName").and_then(Value::as_str).unwrap_or("");
        if name.eq_ignore_ascii_case("SCRIPT") || name.eq_ignore_ascii_case("STYLE") {
            continue;
        }
        if hu_k_of(n).is_some_and(|k| hidden.contains(&k)) {
            continue;
        }
        if let Some(children) = n.get("children").and_then(Value::as_array) {
            stack.extend(children.iter().rev());
        }
    }
    parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SUBTREE_TEXT_MAX_CHARS)
        .collect()
}

/// Offer DRAG on an element marked draggable by markup or cursor.
pub(crate) fn add_drag(actions: &mut Vec<Action>) {
    if !actions.contains(&Action::Drag) {
        actions.push(Action::Drag);
    }
}

/// Offer CLICK on an element the page made clickable by markup or script.
pub(crate) fn add_click(actions: &mut Vec<Action>) {
    if !actions.contains(&Action::Click) {
        actions.push(Action::Click);
    }
}

/// Unlabeled generic containers are not regions. A control tag, an explicit
/// role, or a non-empty label on a textual element is.
fn keep_element(name: &str, role_attr: Option<&str>, label: &str) -> bool {
    if role_attr.is_some() {
        return true;
    }
    let upper = name.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "BUTTON"
            | "A"
            | "INPUT"
            | "TEXTAREA"
            | "SELECT"
            | "OPTION"
            | "NAV"
            | "IMG"
            | "H1"
            | "H2"
            | "H3"
            | "H4"
            | "H5"
            | "H6"
            | "DIALOG"
    ) {
        return true;
    }
    !label.is_empty() && matches!(upper.as_str(), "LABEL" | "SPAN" | "P" | "DIV" | "LI")
}

fn dom_role(name: &str, role_attr: Option<&str>, input_type: Option<&str>) -> Role {
    if let Some(role) = role_attr {
        if let Some(parsed) = Role::parse(&role.to_ascii_lowercase().replace('-', "_")) {
            return parsed;
        }
        if role.eq_ignore_ascii_case("textbox") {
            return Role::TextField;
        }
    }
    match name.to_ascii_uppercase().as_str() {
        "BUTTON" => Role::Button,
        "A" => Role::Link,
        "TEXTAREA" => Role::TextField,
        // Native <select> stays Generic (Chrome AX often says combobox); the
        // tag still adds Select/Click via `dom_actions`. ARIA listbox/option
        // use the dedicated roles.
        "SELECT" => Role::Generic,
        "OPTION" => Role::Option,
        "NAV" => Role::Navigation,
        "IMG" => Role::Image,
        "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => Role::Heading,
        "DIALOG" => Role::Dialog,
        "INPUT" => match input_type.unwrap_or("").to_ascii_lowercase().as_str() {
            "checkbox" => Role::Checkbox,
            "button" | "submit" => Role::Button,
            _ => Role::TextField,
        },
        _ => Role::Generic,
    }
}

/// DOM element claims. Native `<select>` / ARIA listbox get Select; options
/// get Click + Select so autocomplete popups are ticketable.
fn dom_actions(name: &str, role: Role) -> Vec<Action> {
    let mut actions = actions_for_role(role);
    if name.eq_ignore_ascii_case("SELECT") {
        for action in [Action::Click, Action::Select] {
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
    }
    actions
}

pub(crate) fn actions_for_role(role: Role) -> Vec<Action> {
    match role {
        Role::TextField => vec![Action::Click, Action::Focus, Action::Type],
        Role::ComboBox => {
            vec![Action::Click, Action::Focus, Action::Type, Action::Select]
        }
        Role::Checkbox => vec![Action::Click, Action::Focus, Action::Toggle],
        Role::Button | Role::Link | Role::MenuItem | Role::Tab => {
            vec![Action::Click, Action::Focus]
        }
        Role::Option => vec![Action::Click, Action::Focus, Action::Select],
        Role::ListBox => vec![Action::Focus, Action::Click, Action::Select],
        Role::Slider => vec![Action::Click, Action::Focus],
        _ => vec![Action::Focus],
    }
}

/// Roles that are not controls. `None` means skip the accessibility node.
fn ax_role(value: &str) -> Option<Role> {
    match value.to_ascii_lowercase().as_str() {
        "button" => Some(Role::Button),
        "link" => Some(Role::Link),
        "textbox" | "searchbox" | "textfield" => Some(Role::TextField),
        "combobox" => Some(Role::ComboBox),
        "listbox" => Some(Role::ListBox),
        "option" => Some(Role::Option),
        "checkbox" => Some(Role::Checkbox),
        "menuitem" => Some(Role::MenuItem),
        "navigation" => Some(Role::Navigation),
        "image" => Some(Role::Image),
        "heading" => Some(Role::Heading),
        "tab" => Some(Role::Tab),
        "slider" => Some(Role::Slider),
        "statictext" | "inlinetextbox" | "none" | "generic" | "rootwebarea"
        | "genericcontainer" | "inline" | "" => None,
        other => Role::parse(&other.replace('-', "_")).or(Some(Role::Generic)),
    }
}

fn ax_flag(node: &Value, name: &str) -> bool {
    let Some(properties) = node.get("properties").and_then(Value::as_array) else {
        return false;
    };
    properties.iter().any(|property| {
        property.get("name").and_then(Value::as_str) == Some(name)
            && property
                .get("value")
                .and_then(|value| value.get("value"))
                .and_then(Value::as_bool)
                == Some(true)
    })
}

/// URL and title of the current history entry. `None` when there is no
/// current entry, so the page state is unknown.
/// This does not read a timestamp: `Page.getNavigationHistory` has none.
pub(crate) fn navigation_entry(
    history_json: &str,
) -> Result<Option<(String, String)>, BrowserError> {
    let value = parse_json(history_json)?;
    let Some(entries) = value.get("entries").and_then(Value::as_array) else {
        return Ok(None);
    };
    let index = value
        .get("currentIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let Some(entry) = usize::try_from(index)
        .ok()
        .and_then(|index| entries.get(index))
    else {
        return Ok(None);
    };
    let url = entry
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let title = entry
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok(Some((url, title)))
}

fn direct_text(node: &Value) -> String {
    let Some(children) = node.get("children").and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for child in children {
        if child.get("nodeType").and_then(Value::as_i64) == Some(3) {
            if let Some(text) = child.get("nodeValue").and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_owned());
                }
            }
        }
    }
    parts.join(" ")
}

/// The node's `data-hu-k`, read without building its attribute map.
fn hu_k_of(node: &Value) -> Option<u32> {
    let attributes = node.get("attributes").and_then(Value::as_array)?;
    attributes
        .as_chunks::<2>()
        .0
        .iter()
        .find(|[name, _]| name.as_str() == Some(crate::compact::HU_K_ATTR))
        .and_then(|[_, value]| value.as_str()?.parse().ok())
}

fn attr_map(node: &Value) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let Some(attributes) = node.get("attributes").and_then(Value::as_array) else {
        return map;
    };
    let mut index = 0;
    while index + 1 < attributes.len() {
        if let (Some(key), Some(value)) =
            (attributes[index].as_str(), attributes[index + 1].as_str())
        {
            map.insert(key.to_owned(), value.to_owned());
        }
        index += 2;
    }
    map
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn parse_json(text: &str) -> Result<Value, BrowserError> {
    serde_json::from_str(text).map_err(|err| {
        BrowserError::Cdp(CdpError::BadJson {
            message: err.to_string(),
        })
    })
}

/// Only the document tree needs depth beyond serde_json's 128-level cap —
/// a real DOM nests ~2 JSON levels per element level, so observe died at
/// ~60-deep pages. Stack growth makes deep parse safe; every other CDP
/// payload stays bounded.
fn parse_json_deep(text: &str) -> Result<Value, BrowserError> {
    use serde::Deserialize;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    Value::deserialize(serde_stacker::Deserializer::new(&mut deserializer)).map_err(|err| {
        BrowserError::Cdp(CdpError::BadJson {
            message: err.to_string(),
        })
    })
}

#[cfg(test)]
mod pierce_tests {
    use super::*;

    #[test]
    fn scrolled_viewport_stays_at_the_origin() {
        let vp = parse_viewport(
            r#"{"cssLayoutViewport":{"clientWidth":900,"clientHeight":1042,"pageX":0,"pageY":834}}"#,
        )
        .unwrap();
        assert_eq!(
            (vp.x(), vp.y(), vp.width(), vp.height()),
            (0.0, 0.0, 900.0, 1042.0)
        );
    }

    #[test]
    fn onclick_or_nonnegative_tabindex_makes_text_clickable() {
        let span = |id: i64, attrs: &str| {
            format!(
                r##"{{"nodeId": {id}, "backendNodeId": {id}, "nodeType": 1, "nodeName": "SPAN",
                    "attributes": [{attrs}],
                    "children": [{{"nodeId": {t}, "backendNodeId": {t}, "nodeType": 3, "nodeName": "#text", "nodeValue": "word"}}]}}"##,
                t = id + 1000
            )
        };
        let json = format!(
            r##"{{"root": {{"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
                "children": [{}, {}, {}, {}, {}]}}}}"##,
            span(10, r#""onclick", "go()""#),
            span(20, r#""tabindex", "0""#),
            span(30, r#""tabindex", "-1""#),
            span(40, r#""tabindex", "x""#),
            span(50, ""),
        );
        let doc = dom_document(&json).unwrap();
        let clickable: Vec<(i64, bool)> = doc
            .elements
            .iter()
            .map(|e| (e.node_id, e.actions.contains(&Action::Click)))
            .collect();
        assert_eq!(
            clickable,
            [
                (10, true),
                (20, true),
                (30, false),
                (40, false),
                (50, false)
            ]
        );
    }

    #[test]
    fn list_items_with_text_are_regions_and_carry_drag_markers() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [{"nodeId": 2, "backendNodeId": 2, "nodeType": 1, "nodeName": "UL", "attributes": [],
            "children": [
              {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "LI", "attributes": ["class", "item ui-sortable-handle"],
               "children": [{"nodeId": 11, "backendNodeId": 11, "nodeType": 3, "nodeName": "#text", "nodeValue": "Ana"}]},
              {"nodeId": 20, "backendNodeId": 20, "nodeType": 1, "nodeName": "LI", "attributes": [],
               "children": [{"nodeId": 21, "backendNodeId": 21, "nodeType": 3, "nodeName": "#text", "nodeValue": "Bo"}]}
            ]}]}}"##;
        let doc = dom_document(json).unwrap();
        let items: Vec<(&str, bool)> = doc
            .elements
            .iter()
            .map(|e| (e.label.as_str(), e.actions.contains(&Action::Drag)))
            .collect();
        assert_eq!(items, [("Ana", true), ("Bo", false)]);

        // A marked row whose text lives in a child element is still a source.
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [{"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "LI", "attributes": ["class", "ui-sortable-handle"],
            "children": [{"nodeId": 11, "backendNodeId": 11, "nodeType": 1, "nodeName": "SPAN", "attributes": []},
                         {"nodeId": 12, "backendNodeId": 12, "nodeType": 1, "nodeName": "DIV", "attributes": [],
                          "children": [{"nodeId": 13, "backendNodeId": 13, "nodeType": 3, "nodeName": "#text", "nodeValue": " Charla "}]}]}]}}"##;
        let doc = dom_document(json).unwrap();
        let row = doc
            .elements
            .iter()
            .find(|e| e.node_id == 10)
            .expect("marked row kept");
        assert_eq!(row.label, "Charla");
        assert!(row.actions.contains(&Action::Drag));
    }

    #[test]
    fn popup_owners_claim_hover() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [
            {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "BUTTON", "attributes": ["aria-haspopup", "menu"],
             "children": [{"nodeId": 11, "backendNodeId": 11, "nodeType": 3, "nodeName": "#text", "nodeValue": "More"}]},
            {"nodeId": 20, "backendNodeId": 20, "nodeType": 1, "nodeName": "BUTTON", "attributes": ["aria-haspopup", "false"],
             "children": [{"nodeId": 21, "backendNodeId": 21, "nodeType": 3, "nodeName": "#text", "nodeValue": "Plain"}]},
            {"nodeId": 30, "backendNodeId": 30, "nodeType": 1, "nodeName": "LI", "attributes": ["role", "menuitem"],
             "children": [{"nodeId": 31, "backendNodeId": 31, "nodeType": 3, "nodeName": "#text", "nodeValue": "Edit"}]}
          ]}}"##;
        let doc = dom_document(json).unwrap();
        let hover: Vec<(i64, bool)> = doc
            .elements
            .iter()
            .map(|e| (e.node_id, e.actions.contains(&Action::Hover)))
            .collect();
        assert_eq!(hover, [(10, true), (20, false), (30, true)]);
    }

    #[test]
    fn an_icon_inside_a_button_is_the_buttons_face_not_a_target() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [
            {"nodeId": 2, "backendNodeId": 2, "nodeType": 1, "nodeName": "BUTTON", "attributes": ["aria-label", "Close"],
             "children": [{"nodeId": 3, "backendNodeId": 3, "nodeType": 1, "nodeName": "svg", "attributes": [],
               "children": [{"nodeId": 4, "backendNodeId": 4, "nodeType": 1, "nodeName": "path", "attributes": ["fill", "currentColor"]}]}]},
            {"nodeId": 5, "backendNodeId": 5, "nodeType": 1, "nodeName": "svg", "attributes": [],
             "children": [{"nodeId": 6, "backendNodeId": 6, "nodeType": 1, "nodeName": "circle", "attributes": ["fill", "red"]}]}
          ]}}"##;
        let doc = dom_document(json).unwrap();
        let kept: Vec<i64> = doc.elements.iter().map(|e| e.node_id).collect();
        assert_eq!(kept, [2, 5, 6], "the button's svg and path are not kept");
    }

    #[test]
    fn svg_shapes_are_clickable_images_named_by_title_text_or_fill() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [{"nodeId": 2, "backendNodeId": 2, "nodeType": 1, "nodeName": "svg", "attributes": [],
            "children": [
              {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "circle", "attributes": ["fill", "red", "r", "5"]},
              {"nodeId": 11, "backendNodeId": 11, "nodeType": 1, "nodeName": "rect", "attributes": ["style", "fill: blue; stroke: none"]},
              {"nodeId": 12, "backendNodeId": 12, "nodeType": 1, "nodeName": "polygon", "attributes": [],
               "children": [{"nodeId": 13, "backendNodeId": 13, "nodeType": 1, "nodeName": "title", "attributes": [],
                 "children": [{"nodeId": 14, "backendNodeId": 14, "nodeType": 3, "nodeName": "#text", "nodeValue": "Triangle"}]}]},
              {"nodeId": 15, "backendNodeId": 15, "nodeType": 1, "nodeName": "text", "attributes": [],
               "children": [{"nodeId": 16, "backendNodeId": 16, "nodeType": 3, "nodeName": "#text", "nodeValue": "7"}]},
              {"nodeId": 17, "backendNodeId": 17, "nodeType": 1, "nodeName": "path", "attributes": ["fill", "none"]}
            ]}]}}"##;
        let doc = dom_document(json).unwrap();
        let shapes: Vec<(i64, &str, Role, bool)> = doc
            .elements
            .iter()
            .map(|e| {
                (
                    e.node_id,
                    e.label.as_str(),
                    e.role,
                    e.actions.contains(&Action::Click),
                )
            })
            .collect();
        assert_eq!(
            shapes,
            [
                // A bare svg root has no click evidence: no click, no points.
                (2, "drawing", Role::Image, false),
                (10, "red circle", Role::Image, true),
                (11, "blue rect", Role::Image, true),
                (12, "Triangle", Role::Image, true),
                (15, "7", Role::Image, true),
                (17, "path", Role::Image, true),
            ]
        );
    }

    #[test]
    fn text_less_boxes_wait_for_a_click_hint_unless_markup_marks_them() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [
            {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "DIV", "attributes": ["onclick", "pick()", "style", "background-color: red;"]},
            {"nodeId": 20, "backendNodeId": 20, "nodeType": 1, "nodeName": "SPAN", "attributes": ["style", "background-color: green"]},
            {"nodeId": 30, "backendNodeId": 30, "nodeType": 1, "nodeName": "UL", "attributes": []},
            {"nodeId": 40, "backendNodeId": 40, "nodeType": 1, "nodeName": "DIV", "attributes": [],
             "children": [
               {"nodeId": 41, "backendNodeId": 41, "nodeType": 1, "nodeName": "SPAN", "attributes": [],
                "children": [{"nodeId": 42, "backendNodeId": 42, "nodeType": 3, "nodeName": "#text", "nodeValue": "Alexina"}]},
               {"nodeId": 43, "backendNodeId": 43, "nodeType": 1, "nodeName": "SPAN", "attributes": [],
                "children": [{"nodeId": 44, "backendNodeId": 44, "nodeType": 3, "nodeName": "#text", "nodeValue": " Lunch? "}]},
               {"nodeId": 45, "backendNodeId": 45, "nodeType": 1, "nodeName": "SCRIPT", "attributes": [],
                "children": [{"nodeId": 46, "backendNodeId": 46, "nodeType": 3, "nodeName": "#text", "nodeValue": "track()"}]}
             ]},
            {"nodeId": 50, "backendNodeId": 50, "nodeType": 1, "nodeName": "SPAN", "attributes": ["title", "Search"]}
          ]}}"##;
        let doc = dom_document(json).unwrap();
        let kept: Vec<(i64, &str)> = doc
            .elements
            .iter()
            .map(|e| (e.node_id, e.label.as_str()))
            .collect();
        assert_eq!(
            kept,
            [(10, "red"), (41, "Alexina"), (43, "Lunch?")],
            "markup hint keeps it now, named by its color"
        );
        let pending: Vec<(i64, &str)> = doc
            .hint_boxes
            .iter()
            .map(|e| (e.node_id, e.label.as_str()))
            .collect();
        assert_eq!(
            pending,
            [(20, "green"), (40, "Alexina Lunch?"), (50, "Search")],
            "waits for a compact hint, named by content, title, or color; \
             tags that are not boxes never"
        );
    }

    #[test]
    fn a_box_with_a_click_listener_is_kept_and_named_by_its_markup() {
        // Board cells, icon spans, and a table cell whose handlers were added
        // by script: markup shows nothing; Chrome's snapshot does.
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [
            {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "SPAN", "attributes": ["id", "ttt-0"]},
            {"nodeId": 20, "backendNodeId": 20, "nodeType": 1, "nodeName": "SPAN", "attributes": ["class", "reply"]},
            {"nodeId": 30, "backendNodeId": 30, "nodeType": 1, "nodeName": "I", "attributes": ["class", "fa fa-trash"]},
            {"nodeId": 40, "backendNodeId": 40, "nodeType": 1, "nodeName": "TD", "attributes": [],
             "children": [{"nodeId": 41, "backendNodeId": 41, "nodeType": 3, "nodeName": "#text", "nodeValue": "15"}]},
            {"nodeId": 50, "backendNodeId": 50, "nodeType": 1, "nodeName": "SPAN", "attributes": ["class", "like"]},
            {"nodeId": 60, "backendNodeId": 60, "nodeType": 1, "nodeName": "UL", "attributes": ["class", "menu"]}
          ]}}"##;
        let listeners = BTreeSet::from([10, 20, 30, 40, 60]);
        let doc = dom_document_with(json, &listeners, &BTreeSet::new()).unwrap();
        let kept: Vec<(i64, &str, bool)> = doc
            .elements
            .iter()
            .map(|e| {
                (
                    e.node_id,
                    e.label.as_str(),
                    e.actions.contains(&Action::Click),
                )
            })
            .collect();
        assert_eq!(
            kept,
            [
                (10, "ttt", true),
                (20, "reply", true),
                (30, "trash", true),
                (40, "15", true)
            ],
            "listener boxes are kept with CLICK; a list container is not a box"
        );
        let pending: Vec<i64> = doc.hint_boxes.iter().map(|e| e.node_id).collect();
        assert_eq!(pending, [50], "no listener: still waits for a compact hint");
    }

    #[test]
    fn a_markup_named_icon_carries_its_rows_text() {
        let row = |id: i64, who: &str| {
            format!(
                r##"{{"nodeId": {id}, "backendNodeId": {id}, "nodeType": 1, "nodeName": "DIV", "attributes": ["class", "post"],
                  "children": [
                    {{"nodeId": {a}, "backendNodeId": {a}, "nodeType": 1, "nodeName": "B", "attributes": [],
                      "children": [{{"nodeId": {t}, "backendNodeId": {t}, "nodeType": 3, "nodeName": "#text", "nodeValue": "{who}"}}]}},
                    {{"nodeId": {c}, "backendNodeId": {c}, "nodeType": 1, "nodeName": "DIV", "attributes": ["class", "controls"],
                      "children": [{{"nodeId": {i}, "backendNodeId": {i}, "nodeType": 1, "nodeName": "SPAN", "attributes": ["class", "more"]}}]}}
                  ]}}"##,
                a = id + 1,
                t = id + 2,
                c = id + 3,
                i = id + 4
            )
        };
        let json = format!(
            r##"{{"root": {{"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document", "children": [{}, {}]}}}}"##,
            row(10, "Gabie @cierra"),
            row(20, "Conner @a")
        );
        let doc = dom_document_with(&json, &BTreeSet::from([14, 24]), &BTreeSet::new()).unwrap();
        let icons: Vec<(i64, &str)> = doc
            .elements
            .iter()
            .filter(|e| e.named_by_markup)
            .map(|e| (e.node_id, e.label.as_str()))
            .collect();
        assert_eq!(
            icons,
            [(14, "more · Gabie @cierra"), (24, "more · Conner @a")],
            "the nearest ancestor with text says which row"
        );
    }

    #[test]
    fn text_that_is_not_rendered_names_nothing() {
        // A collapsed menu inside the row: its items are display:none.
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [{"nodeId": 2, "backendNodeId": 2, "nodeType": 1, "nodeName": "DIV", "attributes": [],
            "children": [
              {"nodeId": 3, "backendNodeId": 3, "nodeType": 1, "nodeName": "SPAN", "attributes": ["class", "more"]},
              {"nodeId": 4, "backendNodeId": 4, "nodeType": 1, "nodeName": "UL", "attributes": ["data-hu-k", "7"],
               "children": [{"nodeId": 5, "backendNodeId": 5, "nodeType": 3, "nodeName": "#text", "nodeValue": "Copy link"}]},
              {"nodeId": 6, "backendNodeId": 6, "nodeType": 3, "nodeName": "#text", "nodeValue": "Conner @a"}
            ]}]}}"##;
        let doc = dom_document_with(json, &BTreeSet::from([3]), &BTreeSet::from([7])).unwrap();
        let icon = doc.elements.iter().find(|e| e.node_id == 3).unwrap();
        assert_eq!(icon.label, "more · Conner @a");
    }

    #[test]
    fn an_icon_only_link_is_named_by_title_alt_or_markup_words() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [
            {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "A", "attributes": ["href", "#", "class", "icon star-outline"]},
            {"nodeId": 20, "backendNodeId": 20, "nodeType": 1, "nodeName": "BUTTON", "attributes": ["title", "Close"]},
            {"nodeId": 30, "backendNodeId": 30, "nodeType": 1, "nodeName": "IMG", "attributes": ["alt", "Logo", "class", "brand"]},
            {"nodeId": 40, "backendNodeId": 40, "nodeType": 1, "nodeName": "INPUT", "attributes": ["class", "search-box"]}
          ]}}"##;
        let doc = dom_document(json).unwrap();
        let labels: Vec<(i64, &str)> = doc
            .elements
            .iter()
            .map(|e| (e.node_id, e.label.as_str()))
            .collect();
        assert_eq!(
            labels,
            [(10, "star outline"), (20, "Close"), (30, "Logo"), (40, "")],
            "form fields keep their accessibility label"
        );
    }

    #[test]
    fn a_canvas_claims_points_only_with_click_evidence() {
        let json = r##"{"root": {"nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
          "children": [
            {"nodeId": 10, "backendNodeId": 10, "nodeType": 1, "nodeName": "CANVAS", "attributes": ["id", "click-canvas"]},
            {"nodeId": 20, "backendNodeId": 20, "nodeType": 1, "nodeName": "CANVAS", "attributes": []},
            {"nodeId": 30, "backendNodeId": 30, "nodeType": 1, "nodeName": "CANVAS", "attributes": ["tabindex", "0"]}
          ]}}"##;
        let doc = dom_document_with(json, &BTreeSet::from([20]), &BTreeSet::new()).unwrap();
        let points: Vec<(i64, bool, bool)> = doc
            .elements
            .iter()
            .map(|e| {
                (
                    e.node_id,
                    e.actions.contains(&Action::Click),
                    e.actions.contains(&Action::Point),
                )
            })
            .collect();
        assert_eq!(
            points,
            [(10, false, false), (20, true, true), (30, true, true)],
            "a display-only canvas is not a target"
        );
        assert!(doc.elements.iter().all(|e| e.surface));
    }

    #[test]
    fn markup_words_skip_layout_words_digits_and_repeats() {
        let words = |pairs: &[(&str, &str)]| {
            let attributes = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            markup_words(&attributes)
        };
        assert_eq!(words(&[("class", "fa fa-trash")]).as_deref(), Some("trash"));
        assert_eq!(
            words(&[("id", "open-search")]).as_deref(),
            Some("open search")
        );
        assert_eq!(
            words(&[("class", "hidden active"), ("id", "next_page")]).as_deref(),
            Some("next page"),
            "a class of only layout words falls through to the id"
        );
        assert_eq!(
            words(&[("class", "btn btn btn-x2 col-6")]).as_deref(),
            Some("btn")
        );
        assert_eq!(
            words(&[("class", "one two three four")]).as_deref(),
            Some("one two three")
        );
        assert_eq!(words(&[("class", "x9 42")]), None);
    }

    #[test]
    fn clickable_backends_reads_every_snapshot_document() {
        let snapshot = r#"{"documents": [
            {"nodes": {"backendNodeId": [1, 5, 9, 12], "isClickable": {"index": [1, 3]}}},
            {"nodes": {"backendNodeId": [40, 41], "isClickable": {"index": [0, 7]}}},
            {"nodes": {"backendNodeId": [50]}}
          ], "strings": []}"#;
        assert_eq!(
            clickable_backends(snapshot).unwrap(),
            BTreeSet::from([5, 12, 40]),
            "an index past the node list is ignored"
        );
        assert!(clickable_backends(r#"{"strings": []}"#).is_err());
    }

    #[test]
    fn open_shadow_roots_are_walked_when_present() {
        let json = r##"{
          "root": {
            "nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
            "children": [{
              "nodeId": 2, "backendNodeId": 2, "nodeType": 1, "nodeName": "DIV",
              "attributes": [],
              "children": [],
              "shadowRoots": [{
                "nodeId": 3, "backendNodeId": 3, "nodeType": 11, "nodeName": "#document-fragment",
                "children": [{
                  "nodeId": 10, "backendNodeId": 100, "nodeType": 1, "nodeName": "BUTTON",
                  "attributes": ["aria-label", "Shadow Go"],
                  "children": [{"nodeId": 11, "backendNodeId": 101, "nodeType": 3, "nodeName": "#text", "nodeValue": "Shadow Go"}]
                }]
              }]
            }]
          }
        }"##;
        let doc = dom_document(json).unwrap();
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].label, "Shadow Go");
        assert_eq!(doc.elements[0].backend_node_id, 100);
    }

    #[test]
    fn same_origin_iframe_content_document_is_walked() {
        let json = r##"{
          "root": {
            "nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
            "children": [{
              "nodeId": 2, "backendNodeId": 2, "nodeType": 1, "nodeName": "IFRAME",
              "attributes": [],
              "children": [],
              "contentDocument": {
                "nodeId": 3, "backendNodeId": 3, "nodeType": 9, "nodeName": "#document",
                "children": [{
                  "nodeId": 20, "backendNodeId": 200, "nodeType": 1, "nodeName": "BUTTON",
                  "attributes": ["aria-label", "Frame Save"],
                  "children": [{"nodeId": 21, "backendNodeId": 201, "nodeType": 3, "nodeName": "#text", "nodeValue": "Frame Save"}]
                }]
              }
            }]
          }
        }"##;
        let doc = dom_document(json).unwrap();
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].label, "Frame Save");
        assert_eq!(doc.elements[0].backend_node_id, 200);
    }

    #[test]
    fn combobox_and_option_roles_claim_type_and_select() {
        let json = r##"{
          "root": {
            "nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
            "children": [
              {
                "nodeId": 10, "backendNodeId": 100, "nodeType": 1, "nodeName": "INPUT",
                "attributes": ["role", "combobox", "aria-label", "City"],
                "children": []
              },
              {
                "nodeId": 20, "backendNodeId": 200, "nodeType": 1, "nodeName": "DIV",
                "attributes": ["role", "option", "aria-label", "Manila"],
                "children": []
              }
            ]
          }
        }"##;
        let doc = dom_document(json).unwrap();
        assert_eq!(doc.elements.len(), 2);
        assert_eq!(doc.elements[0].role, Role::ComboBox);
        assert!(doc.elements[0].actions.contains(&Action::Type));
        assert_eq!(doc.elements[1].role, Role::Option);
        assert!(doc.elements[1].actions.contains(&Action::Click));
        assert!(doc.elements[1].actions.contains(&Action::Select));
    }

    #[test]
    fn native_select_observes_as_generic_with_select_claim() {
        let json = r##"{
          "root": {
            "nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
            "children": [{
              "nodeId": 10, "backendNodeId": 100, "nodeType": 1, "nodeName": "SELECT",
              "attributes": ["aria-label", "Cabin"],
              "children": []
            }]
          }
        }"##;
        let doc = dom_document(json).unwrap();
        assert_eq!(doc.elements.len(), 1);
        assert_eq!(doc.elements[0].role, Role::Generic);
        assert!(doc.elements[0].actions.contains(&Action::Select));
    }

    /// A ~400-level DOM is real (deep component trees, virtualized parents,
    /// nested layouts) — observe must not die on serde_json's 128-level cap
    /// or on a recursive walk. Both walls are covered here.
    #[test]
    fn a_deeply_nested_document_parses_and_walks() {
        const DEPTH: usize = 400;
        let mut json = r##"{"root":{"nodeType":9,"nodeName":"#document","children":["##.to_owned();
        // Build one branch of DEPTH nested labelled DIVs, innermost first.
        let mut inner = r##"{"nodeId":0,"backendNodeId":0,"nodeType":3,"nodeName":"#text","nodeValue":"leaf"}"##.to_owned();
        for i in (0..DEPTH).rev() {
            inner = format!(
                r#"{{"nodeId":{nid},"backendNodeId":{nid},"nodeType":1,"nodeName":"DIV","attributes":["aria-label","depth {i}"],"children":[{inner}]}}"#,
                nid = i + 1,
                i = i,
                inner = inner,
            );
        }
        json.push_str(&inner);
        json.push_str("]}}");

        let doc = dom_document(&json).unwrap();
        assert_eq!(doc.elements.len(), DEPTH);
        // Ancestors are recorded nearest first: the deepest kept element
        // chains through every kept level above it.
        let deepest = &doc.elements[DEPTH - 1];
        assert_eq!(deepest.ancestors.len(), DEPTH - 1);
        assert_eq!(
            deepest.ancestors.first().copied(),
            Some((DEPTH - 1) as i64),
            "nearest ancestor is the level directly above"
        );
        // And the parent map links every element to its element parent.
        assert_eq!(doc.parent_of.len(), DEPTH - 1);
        assert_eq!(
            doc.parent_of.get(&(DEPTH as i64)).copied(),
            Some((DEPTH - 1) as i64)
        );
    }
}
