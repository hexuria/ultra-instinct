//! Build a [`crate::ReplayTransport`] script in code.
//!
//! This is mock support for tests: it writes the CDP calls made by browser
//! session operations such as observe and press, in the order they make them.
//! Steps carry no `params`, so the replay does not check them. It does not talk
//! to Chrome. It is not a model of Chrome; it only emits response shapes that
//! extractors read.
//!
//! DOM nodes use tags that observe always keeps (`BUTTON`, `A`, `INPUT`,
//! `NAV`, `H1`), so the getBoxModel order is the document order.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

/// One DOM element. `rect` of `None` is a getBoxModel protocol error, which
/// observe treats as "no box" and omits.
#[derive(Clone, Debug)]
pub struct DomSpec {
    pub node_id: i64,
    pub backend: i64,
    pub tag: &'static str,
    pub label: String,
    pub rect: Option<(f64, f64, f64, f64)>,
    pub children: Vec<DomSpec>,
    /// Extra attributes, in order (for example `role`, `aria-label`,
    /// `aria-modal`). Empty for the plain kept tags.
    pub attributes: Vec<(String, String)>,
    /// Open shadow roots (emulates `DOM.getDocument` with `pierce: true`).
    pub shadow_roots: Vec<DomSpec>,
    /// Same-origin iframe `contentDocument` root (usually a `#document` node).
    pub content_document: Option<Box<DomSpec>>,
    /// Extra per-node fields merged into the compact observe record.
    pub compact_state: Option<Value>,
}

impl DomSpec {
    pub fn button(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            node_id,
            backend,
            tag: "BUTTON",
            label: label.to_owned(),
            rect: Some(rect),
            children: Vec::new(),
            attributes: Vec::new(),
            shadow_roots: Vec::new(),
            content_document: None,
            compact_state: None,
        }
    }

    /// A container `DIV` with an explicit `role` and `aria-label`, so observe
    /// keeps it as a region and its children get it as their parent.
    pub fn container(
        node_id: i64,
        backend: i64,
        role: &str,
        label: &str,
        rect: (f64, f64, f64, f64),
    ) -> Self {
        Self {
            node_id,
            backend,
            tag: "DIV",
            label: String::new(),
            rect: Some(rect),
            children: Vec::new(),
            attributes: vec![
                ("role".to_owned(), role.to_owned()),
                ("aria-label".to_owned(), label.to_owned()),
            ],
            shadow_roots: Vec::new(),
            content_document: None,
            compact_state: None,
        }
    }

    /// ARIA option (autocomplete / listbox popup row).
    pub fn option(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self::container(node_id, backend, "option", label, rect).with_attr("aria-label", label)
    }

    /// ARIA combobox field.
    pub fn combobox(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self::button(node_id, backend, label, rect)
            .with_tag("INPUT")
            .with_attr("role", "combobox")
            .with_attr("aria-label", label)
    }

    /// Attach open shadow roots (host is usually not kept unless labeled).
    pub fn with_shadow_roots(mut self, roots: Vec<DomSpec>) -> Self {
        self.shadow_roots = roots;
        self
    }

    /// Attach a same-origin iframe content document.
    pub fn with_content_document(mut self, document: DomSpec) -> Self {
        self.content_document = Some(Box::new(document));
        self
    }

    /// A `#document` wrapper for iframe `contentDocument`.
    pub fn document(node_id: i64, backend: i64, children: Vec<DomSpec>) -> Self {
        Self {
            node_id,
            backend,
            tag: "#document",
            label: String::new(),
            rect: None,
            children,
            attributes: Vec::new(),
            shadow_roots: Vec::new(),
            content_document: None,
            compact_state: None,
        }
    }

    /// An open `#document-fragment` shadow root.
    pub fn shadow_root(node_id: i64, backend: i64, children: Vec<DomSpec>) -> Self {
        Self {
            node_id,
            backend,
            tag: "#document-fragment",
            label: String::new(),
            rect: None,
            children,
            attributes: Vec::new(),
            shadow_roots: Vec::new(),
            content_document: None,
            compact_state: None,
        }
    }

    /// The same element with another kept tag (`A`, `INPUT`, `NAV`, `H1`).
    pub fn with_tag(mut self, tag: &'static str) -> Self {
        self.tag = tag;
        self
    }

    /// Add one attribute.
    pub fn with_attr(mut self, name: &str, value: &str) -> Self {
        self.attributes.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Add control-state fields to this node's compact observe record.
    pub fn with_compact_state(mut self, state: Value) -> Self {
        self.compact_state = Some(state);
        self
    }

    /// Nest `children` under this element.
    pub fn with_children(mut self, children: Vec<DomSpec>) -> Self {
        self.children = children;
        self
    }
}

/// One accessibility node. `rect` is read with getBoxModel by backend id.
#[derive(Clone, Debug)]
pub struct AxSpec {
    pub backend: Option<i64>,
    pub role: &'static str,
    pub name: String,
    pub rect: Option<(f64, f64, f64, f64)>,
    pub focused: bool,
    /// The accessibility `modal` property.
    pub modal: bool,
}

impl AxSpec {
    pub fn new(backend: i64, role: &'static str, name: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            backend: Some(backend),
            role,
            name: name.to_owned(),
            rect: Some(rect),
            focused: false,
            modal: false,
        }
    }

    /// Mark this node as the focused one.
    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
    }

    /// Mark this node as modal (a dialog opened with `showModal()`).
    pub fn modal(mut self) -> Self {
        self.modal = true;
        self
    }
}

/// One control seen by both DOM and accessibility, as Chrome reports a real
/// button or link: one DOM element and one AX node with the same backend id
/// and the same box.
#[derive(Clone, Debug)]
pub struct Control {
    pub node_id: i64,
    pub backend: i64,
    pub tag: &'static str,
    pub role: &'static str,
    pub label: String,
    pub rect: (f64, f64, f64, f64),
    pub focused: bool,
}

impl Control {
    pub fn button(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            node_id,
            backend,
            tag: "BUTTON",
            role: "button",
            label: label.to_owned(),
            rect,
            focused: false,
        }
    }

    pub fn link(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            tag: "A",
            role: "link",
            ..Self::button(node_id, backend, label, rect)
        }
    }

    pub fn text_field(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            tag: "INPUT",
            role: "textbox",
            ..Self::button(node_id, backend, label, rect)
        }
    }

    pub fn combobox(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            tag: "INPUT",
            role: "combobox",
            ..Self::button(node_id, backend, label, rect)
        }
    }

    pub fn option(node_id: i64, backend: i64, label: &str, rect: (f64, f64, f64, f64)) -> Self {
        Self {
            tag: "DIV",
            role: "option",
            ..Self::button(node_id, backend, label, rect)
        }
    }

    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
    }
}

/// The `Page.getNavigationHistory` step.
#[derive(Clone, Debug)]
pub enum HistorySpec {
    Entry {
        url: String,
        title: String,
    },
    /// A CDP error result. Page state is unknown.
    ProtocolError,
    /// A result with no entries. Page state is unknown.
    NoEntries,
}

/// One observation of a page.
#[derive(Clone, Debug)]
pub struct PageSpec {
    pub width: f64,
    pub height: f64,
    pub dom: Vec<DomSpec>,
    pub ax: Vec<AxSpec>,
    pub history: HistorySpec,
    /// `DOM.getNodeForLocation` override keyed by the clickable region's
    /// backend id. Absent keys return the region's own backend (clear hit).
    /// Use this to script a cookie banner or custom backdrop covering a
    /// control: map the buried control's backend to the overlay's backend.
    pub hit_overrides: BTreeMap<i64, i64>,
    /// Optional computed-style overrides keyed by DOM `nodeId`. Absent keys
    /// get a default static stacking style. Enables stacking-map fixtures
    /// without relying on hit-test overrides.
    pub style_overrides: BTreeMap<i64, Vec<(String, String)>>,
    /// Backend ids `DOMSnapshot.captureSnapshot` reports `isClickable` (a
    /// click listener added by script).
    pub listeners: BTreeSet<i64>,
}

impl PageSpec {
    /// A page of controls that both DOM and accessibility report.
    pub fn of(controls: &[Control], url: &str, title: &str) -> Self {
        let dom = controls
            .iter()
            .map(|control| {
                let mut spec = DomSpec::button(
                    control.node_id,
                    control.backend,
                    &control.label,
                    control.rect,
                )
                .with_tag(control.tag);
                // Emit ARIA role when the tag alone would not keep the right role
                // (combobox / option / listbox / dialog / …).
                if matches!(
                    control.role,
                    "combobox" | "option" | "listbox" | "dialog" | "menuitem" | "tab"
                ) {
                    spec = spec.with_attr("role", control.role);
                }
                if matches!(control.role, "combobox" | "option" | "listbox" | "dialog") {
                    spec = spec.with_attr("aria-label", &control.label);
                }
                spec
            })
            .collect();
        let ax = controls
            .iter()
            .map(|control| {
                let node = AxSpec::new(control.backend, control.role, &control.label, control.rect);
                if control.focused {
                    node.focused()
                } else {
                    node
                }
            })
            .collect();
        Self::new(dom, ax, url, title)
    }

    /// Replace the history step.
    pub fn with_history(mut self, history: HistorySpec) -> Self {
        self.history = history;
        self
    }

    pub fn new(dom: Vec<DomSpec>, ax: Vec<AxSpec>, url: &str, title: &str) -> Self {
        Self {
            width: 1280.0,
            height: 720.0,
            dom,
            ax,
            history: HistorySpec::Entry {
                url: url.to_owned(),
                title: title.to_owned(),
            },
            hit_overrides: BTreeMap::new(),
            style_overrides: BTreeMap::new(),
            listeners: BTreeSet::new(),
        }
    }

    /// Chrome reports a click listener on `backend`.
    pub fn listener(mut self, backend: i64) -> Self {
        self.listeners.insert(backend);
        self
    }

    /// Cover `target_backend`'s center with `hit_backend` (hit-test overlay).
    pub fn cover(mut self, target_backend: i64, hit_backend: i64) -> Self {
        self.hit_overrides.insert(target_backend, hit_backend);
        self
    }

    /// Set computed style for a DOM `nodeId` (stacking-map fixtures).
    pub fn style(mut self, node_id: i64, pairs: Vec<(&str, &str)>) -> Self {
        self.style_overrides.insert(
            node_id,
            pairs
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        );
        self
    }
}

/// A sequence of observations and presses, in call order.
#[derive(Clone, Debug, Default)]
pub struct ScriptBuilder {
    calls: Vec<Value>,
}

impl ScriptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The calls of one `observe`.
    pub fn observe(mut self, page: &PageSpec) -> Self {
        self.calls.push(result(
            "Page.getLayoutMetrics",
            json!({"cssLayoutViewport": {
                "clientWidth": page.width, "clientHeight": page.height, "pageX": 0, "pageY": 0
            }}),
        ));
        let children: Vec<Value> = page.dom.iter().map(dom_json).collect();
        self.calls.push(result(
            "DOM.getDocument",
            json!({"root": {
                "nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
                "children": children
            }}),
        ));
        self.calls.push(snapshot_call(page));
        let nodes: Vec<Value> = page.ax.iter().enumerate().map(ax_json).collect();
        self.calls.push(result(
            "Accessibility.getFullAXTree",
            json!({"nodes": nodes}),
        ));
        let mut flat = Vec::new();
        for node in &page.dom {
            flatten(node, &mut flat);
        }
        // Match `extract::dom_document`: only kept element nodes request a box.
        // Non-kept hosts (iframe, unlabeled DIV, document wrappers) are walked
        // for descendants but must not consume a getBoxModel slot.
        for node in flat {
            let kept =
                dom_node_kept(node) || (page.listeners.contains(&node.backend) && is_box(node.tag));
            if kept && node.tag != "#document" && node.tag != "#document-fragment" {
                self.calls.push(box_call(node.rect));
            }
        }
        for node in &page.ax {
            // Match `extract::ax_role`: skipped roles never request a box.
            if node.backend.is_some() && ax_role_kept(node.role) {
                self.calls.push(box_call(node.rect));
            }
        }
        self.calls.push(match &page.history {
            HistorySpec::Entry { url, title } => result(
                "Page.getNavigationHistory",
                json!({"currentIndex": 0, "entries": [{"id": 1, "url": url, "title": title}]}),
            ),
            HistorySpec::ProtocolError => error("Page.getNavigationHistory", "history failed"),
            HistorySpec::NoEntries => result("Page.getNavigationHistory", json!({})),
        });
        // Stacking map: CSS.enable + computed style per kept DOM node, in
        // RegionId order (same as BrowserSession::apply_stacking_occlusion).
        self.calls.push(result("CSS.enable", json!({})));
        for (node_id, pairs) in style_targets(page) {
            let computed = pairs
                .iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect::<Vec<_>>();
            self.calls.push(result(
                "CSS.getComputedStyleForNode",
                json!({"computedStyle": computed}),
            ));
            let _ = node_id; // params are not checked by ReplayTransport unless set
        }
        // Hit-test each clickable fused region in RegionId order (same order
        // BrowserSession::apply_hit_test_occlusion walks the manifold).
        for (_id, backend, _x, _y) in clickable_targets(page) {
            let hit = page.hit_overrides.get(&backend).copied().unwrap_or(backend);
            self.calls.push(result(
                "DOM.getNodeForLocation",
                json!({"backendNodeId": hit}),
            ));
        }
        self
    }

    /// The calls of one `observe` on the compact path: a single
    /// `Runtime.evaluate` returning the compact blob, then the constant
    /// calls. No `DOM.getBoxModel` / `CSS.*` / `DOM.getNodeForLocation`
    /// steps: tagged elements carry `data-hu-k` inside the document and all
    /// per-node evidence rides in the blob.
    ///
    /// `hit_overrides` must point at elements inside `page.dom` (they get a
    /// `data-hu-k`); an override whose overlay backend has no tag degrades
    /// to "no hit" rather than the scripted overlay.
    pub fn observe_compact(mut self, page: &PageSpec) -> Self {
        // Tag elements in flatten order (children → shadow → content
        // document), matching the JS walk's visit order.
        let mut dom = page.dom.clone();
        let mut next = 0u32;
        for node in &mut dom {
            tag_dom(node, &mut next);
        }
        let mut k_of_backend = BTreeMap::new();
        let mut flat = Vec::new();
        for node in &dom {
            flatten(node, &mut flat);
        }
        for node in &flat {
            if let Some((_, value)) = node
                .attributes
                .iter()
                .find(|(name, _)| name == crate::HU_K_ATTR)
            {
                if let Ok(k) = value.parse::<u32>() {
                    k_of_backend.insert(node.backend, k);
                }
            }
        }
        let mut nodes = serde_json::Map::new();
        for node in &flat {
            let Some(k) = k_of_backend.get(&node.backend).copied() else {
                continue;
            };
            let r = node
                .rect
                .map(|(x, y, w, h)| json!([x, y, w, h]))
                .unwrap_or(Value::Null);
            let s: Vec<Value> = page
                .style_overrides
                .get(&node.node_id)
                .cloned()
                .unwrap_or_else(default_stacking_style)
                .iter()
                .map(|(name, value)| json!([name, value]))
                .collect();
            let hit = page
                .hit_overrides
                .get(&node.backend)
                .and_then(|backend| k_of_backend.get(backend).copied())
                .unwrap_or(k);
            let mut record = json!({"r": r, "s": s, "h": if node.rect.is_some() { json!(hit) } else { Value::Null }});
            if let (Some(record), Some(state)) = (
                record.as_object_mut(),
                node.compact_state.as_ref().and_then(Value::as_object),
            ) {
                record.extend(
                    state
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            nodes.insert(k.to_string(), record);
        }
        self.calls.push(result(
            "Runtime.evaluate",
            json!({"result": {"type": "object", "value": {"nodes": nodes}}}),
        ));
        self.calls.push(result(
            "Page.getLayoutMetrics",
            json!({"cssLayoutViewport": {
                "clientWidth": page.width, "clientHeight": page.height, "pageX": 0, "pageY": 0
            }}),
        ));
        let children: Vec<Value> = dom.iter().map(dom_json).collect();
        self.calls.push(result(
            "DOM.getDocument",
            json!({"root": {
                "nodeId": 1, "backendNodeId": 1, "nodeType": 9, "nodeName": "#document",
                "children": children
            }}),
        ));
        self.calls.push(snapshot_call(page));
        let ax_nodes: Vec<Value> = page.ax.iter().enumerate().map(ax_json).collect();
        self.calls.push(result(
            "Accessibility.getFullAXTree",
            json!({"nodes": ax_nodes}),
        ));
        self.calls.push(match &page.history {
            HistorySpec::Entry { url, title } => result(
                "Page.getNavigationHistory",
                json!({"currentIndex": 0, "entries": [{"id": 1, "url": url, "title": title}]}),
            ),
            HistorySpec::ProtocolError => error("Page.getNavigationHistory", "history failed"),
            HistorySpec::NoEntries => result("Page.getNavigationHistory", json!({})),
        });
        self
    }

    /// A DOM click by node id that succeeds.
    pub fn dom_click(mut self, node_id: i64) -> Self {
        let object = format!("obj-{node_id}");
        self.calls.push(result(
            "DOM.resolveNode",
            json!({"object": {"type": "object", "objectId": object}}),
        ));
        self.calls.push(result(
            "Runtime.callFunctionOn",
            json!({"result": {"type": "undefined"}}),
        ));
        self
    }

    /// Trusted pointer click at a region center: move, press, release.
    pub fn pointer_click(mut self) -> Self {
        for _ in 0..3 {
            self.calls
                .push(result("Input.dispatchMouseEvent", json!({})));
        }
        self
    }

    /// CDP refuses the pointer move (protocol error), then the DOM click by
    /// node id succeeds.
    pub fn pointer_refused_then_dom_click(self, node_id: i64) -> Self {
        let mut next = self;
        next.calls.push(error(
            "Input.dispatchMouseEvent",
            "Input dispatch is not available",
        ));
        next.dom_click(node_id)
    }

    /// Both DOM click tiers fail with a protocol error (the node is gone), and
    /// the coordinate click is scripted.
    pub fn stale_node_then_coordinate_click(mut self) -> Self {
        self.calls
            .push(error("DOM.resolveNode", "No node with given id found"));
        self.calls
            .push(error("DOM.resolveNode", "No node with given id found"));
        self.calls
            .push(result("Input.dispatchMouseEvent", json!({})));
        self.calls
            .push(result("Input.dispatchMouseEvent", json!({})));
        self
    }

    /// A DOM input (type / select) by node id that the page accepts.
    pub fn dom_input(mut self, node_id: i64) -> Self {
        let object = format!("obj-{node_id}");
        self.calls.push(result(
            "DOM.resolveNode",
            json!({"object": {"type": "object", "objectId": object}}),
        ));
        self.calls.push(result(
            "Runtime.callFunctionOn",
            json!({"result": {"type": "boolean", "value": true}}),
        ));
        self
    }

    /// Read a by-value autocomplete signature through the bound DOM node.
    pub fn dom_read_autocomplete_signature(self, node_id: i64, value: Value) -> Self {
        self.dom_read_result(node_id, json!({"value":value}))
    }

    /// A `BrowserSession::ready_state` result.
    pub fn ready_state(mut self, value: &str) -> Self {
        self.calls.push(result(
            "Runtime.evaluate",
            json!({"result":{"value":value}}),
        ));
        self
    }

    /// A DOM input by node id that the page refuses (the function throws).
    pub fn dom_input_rejected(mut self, node_id: i64, message: &str) -> Self {
        let object = format!("obj-{node_id}");
        self.calls.push(result(
            "DOM.resolveNode",
            json!({"object": {"type": "object", "objectId": object}}),
        ));
        self.calls.push(result(
            "Runtime.callFunctionOn",
            json!({
                "result": {"type": "object", "subtype": "error"},
                "exceptionDetails": {"text": "Uncaught", "exception": {"description": format!("Error: {message}")}}
            }),
        ));
        self
    }

    /// Read-back of a field value (`BrowserSession::field_value`).
    pub fn dom_read_value(self, node_id: i64, value: &str, text: &str) -> Self {
        self.dom_read_result(node_id, json!({"type":"object","value":[value,text]}))
    }

    fn dom_read_result(mut self, node_id: i64, result_value: Value) -> Self {
        let object = format!("obj-{node_id}");
        self.calls.push(result(
            "DOM.resolveNode",
            json!({"object": {"type": "object", "objectId": object}}),
        ));
        self.calls.push(result(
            "Runtime.callFunctionOn",
            json!({"result": result_value}),
        ));
        self
    }

    /// One page scroll wheel event.
    pub fn scroll(mut self) -> Self {
        self.calls
            .push(result("Input.dispatchMouseEvent", json!({})));
        self
    }

    /// Append an arbitrary `method → result` step (generic escape for tools
    /// the named helpers do not cover, e.g. diagnostics or page input).
    pub fn call(mut self, method: &str, value: Value) -> Self {
        self.calls.push(result(method, value));
        self
    }

    /// Append a `method → protocol error` step.
    pub fn call_error(mut self, method: &str, message: &str) -> Self {
        self.calls.push(error(method, message));
        self
    }

    /// Number of scripted CDP calls so far.
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub fn to_json(&self) -> String {
        json!({"calls": self.calls}).to_string()
    }
}

fn result(method: &str, value: Value) -> Value {
    json!({"method": method, "result": value})
}

fn error(method: &str, message: &str) -> Value {
    json!({"method": method, "error": message})
}

fn box_call(rect: Option<(f64, f64, f64, f64)>) -> Value {
    match rect {
        Some((x, y, w, h)) => result(
            "DOM.getBoxModel",
            json!({"model": {"content": [x, y, x + w, y, x + w, y + h, x, y + h]}}),
        ),
        None => error("DOM.getBoxModel", "Could not compute box model."),
    }
}

fn flatten<'a>(node: &'a DomSpec, out: &mut Vec<&'a DomSpec>) {
    out.push(node);
    for child in &node.children {
        flatten(child, out);
    }
    for root in &node.shadow_roots {
        flatten(root, out);
    }
    if let Some(doc) = &node.content_document {
        flatten(doc, out);
    }
}

/// Assign `data-hu-k` in the same order the compact JS walk visits nodes:
/// the element itself, then children, then open shadow roots, then a
/// same-origin `contentDocument`. Document/fragment wrappers are not
/// elements and get no tag.
fn tag_dom(node: &mut DomSpec, next: &mut u32) {
    let is_element = node.tag != "#document" && node.tag != "#document-fragment";
    if is_element {
        node.attributes
            .push((crate::HU_K_ATTR.to_owned(), next.to_string()));
        *next += 1;
    }
    for child in &mut node.children {
        tag_dom(child, next);
    }
    for root in &mut node.shadow_roots {
        tag_dom(root, next);
    }
    if let Some(doc) = &mut node.content_document {
        tag_dom(doc, next);
    }
}

fn dom_json(node: &DomSpec) -> Value {
    // Document / fragment wrappers used as iframe contentDocument or shadow root.
    if node.tag == "#document" || node.tag == "#document-fragment" {
        let node_type = if node.tag == "#document" { 9 } else { 11 };
        let children: Vec<Value> = node.children.iter().map(dom_json).collect();
        return json!({
            "nodeId": node.node_id,
            "backendNodeId": node.backend,
            "nodeType": node_type,
            "nodeName": node.tag,
            "children": children,
        });
    }
    let mut children = Vec::new();
    if !node.label.is_empty() {
        children.push(json!({
            "nodeId": node.node_id * 1000 + 1,
            "backendNodeId": node.backend * 1000 + 1,
            "nodeType": 3,
            "nodeName": "#text",
            "nodeValue": node.label,
        }));
    }
    children.extend(node.children.iter().map(dom_json));
    let attributes: Vec<&str> = node
        .attributes
        .iter()
        .flat_map(|(name, value)| [name.as_str(), value.as_str()])
        .collect();
    let mut value = json!({
        "nodeId": node.node_id,
        "backendNodeId": node.backend,
        "nodeType": 1,
        "nodeName": node.tag,
        "attributes": attributes,
        "children": children,
    });
    if !node.shadow_roots.is_empty() {
        value["shadowRoots"] = Value::Array(node.shadow_roots.iter().map(dom_json).collect());
    }
    if let Some(doc) = &node.content_document {
        value["contentDocument"] = dom_json(doc);
    }
    value
}

fn ax_json((index, node): (usize, &AxSpec)) -> Value {
    let mut value = json!({
        "nodeId": format!("ax{index}"),
        "ignored": false,
        "role": {"value": node.role},
        "name": {"value": node.name},
    });
    if let Some(backend) = node.backend {
        value["backendDOMNodeId"] = json!(backend);
    }
    let mut properties = Vec::new();
    if node.focused {
        properties.push(json!({"name": "focused", "value": {"type": "boolean", "value": true}}));
    }
    if node.modal {
        properties.push(json!({"name": "modal", "value": {"type": "boolean", "value": true}}));
    }
    if !properties.is_empty() {
        value["properties"] = json!(properties);
    }
    value
}

/// Kept DOM nodes that receive a stacking style, in RegionId (`n{backend}`) order.
fn style_targets(page: &PageSpec) -> Vec<(i64, Vec<(String, String)>)> {
    let mut flat = Vec::new();
    for node in &page.dom {
        flatten(node, &mut flat);
    }
    let mut targets: Vec<(String, i64, i64)> = Vec::new();
    for node in flat {
        if node.rect.is_none() {
            continue;
        }
        // Only nodes observe keeps as regions get styles. Keep rule matches
        // extract::keep_element for the tags ScriptBuilder emits.
        if !dom_node_kept(node) {
            continue;
        }
        targets.push((format!("n{}", node.backend), node.node_id, node.backend));
    }
    targets.sort_by(|left, right| left.0.cmp(&right.0));
    targets
        .into_iter()
        .map(|(_id, node_id, _backend)| {
            let pairs = page
                .style_overrides
                .get(&node_id)
                .cloned()
                .unwrap_or_else(default_stacking_style);
            (node_id, pairs)
        })
        .collect()
}

fn default_stacking_style() -> Vec<(String, String)> {
    vec![
        ("z-index".into(), "auto".into()),
        ("position".into(), "static".into()),
        ("opacity".into(), "1".into()),
        ("transform".into(), "none".into()),
        ("filter".into(), "none".into()),
        ("isolation".into(), "auto".into()),
        ("mix-blend-mode".into(), "normal".into()),
        ("will-change".into(), "auto".into()),
        ("pointer-events".into(), "auto".into()),
    ]
}

/// `DOMSnapshot.captureSnapshot` for `page`: one document listing every
/// element backend, with `isClickable` on the page's listeners.
fn snapshot_call(page: &PageSpec) -> Value {
    let mut flat = Vec::new();
    for node in &page.dom {
        flatten(node, &mut flat);
    }
    let backends: Vec<i64> = flat.iter().map(|node| node.backend).collect();
    let clickable: Vec<usize> = backends
        .iter()
        .enumerate()
        .filter(|(_, backend)| page.listeners.contains(backend))
        .map(|(i, _)| i)
        .collect();
    result(
        "DOMSnapshot.captureSnapshot",
        json!({"documents": [{"nodes": {"backendNodeId": backends, "isClickable": {"index": clickable}}}], "strings": []}),
    )
}

/// Mirrors `extract::is_box` in aui-browser.
fn is_box(tag: &str) -> bool {
    matches!(
        tag.to_ascii_uppercase().as_str(),
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

fn dom_node_kept(node: &DomSpec) -> bool {
    let role_attr = node
        .attributes
        .iter()
        .find(|(name, _)| name == "role")
        .map(|(_, value)| value.as_str());
    let label = node.label.trim();
    if role_attr.is_some() {
        return true;
    }
    let tag = node.tag.to_ascii_uppercase();
    matches!(
        tag.as_str(),
        "BUTTON"
            | "A"
            | "INPUT"
            | "TEXTAREA"
            | "SELECT"
            | "OPTION"
            | "NAV"
            | "H1"
            | "H2"
            | "H3"
            | "H4"
            | "H5"
            | "H6"
    ) || (!label.is_empty() && matches!(tag.as_str(), "LABEL" | "SPAN" | "P" | "DIV"))
}

/// Clickable fused regions in `n{backend}` / `ax{backend}` id order.
///
/// Mirrors the session's hit-test targets closely enough for scripted pages:
/// DOM-kept clickable controls, then AX-only clickable leftovers.
fn clickable_targets(page: &PageSpec) -> Vec<(String, i64, f64, f64)> {
    let mut flat = Vec::new();
    for node in &page.dom {
        flatten(node, &mut flat);
    }
    let mut dom_backends = std::collections::BTreeSet::new();
    let mut targets: Vec<(String, i64, f64, f64)> = Vec::new();
    for node in flat {
        let Some(rect) = node.rect else {
            continue;
        };
        if !dom_role_is_clickable(node) {
            continue;
        }
        dom_backends.insert(node.backend);
        let (x, y, w, h) = rect;
        targets.push((
            format!("n{}", node.backend),
            node.backend,
            x + w / 2.0,
            y + h / 2.0,
        ));
    }
    for node in &page.ax {
        let Some(backend) = node.backend else {
            continue;
        };
        if dom_backends.contains(&backend) {
            continue;
        }
        if !ax_role_is_clickable(node.role) {
            continue;
        }
        let Some(rect) = node.rect else {
            continue;
        };
        let (x, y, w, h) = rect;
        targets.push((format!("ax{backend}"), backend, x + w / 2.0, y + h / 2.0));
    }
    targets.sort_by(|left, right| left.0.cmp(&right.0));
    targets
}

fn dom_role_is_clickable(node: &DomSpec) -> bool {
    let role_attr = node
        .attributes
        .iter()
        .find(|(name, _)| name == "role")
        .map(|(_, value)| value.as_str());
    let role = match role_attr {
        Some("button") | Some("link") | Some("menuitem") | Some("tab") | Some("slider") => {
            return true;
        }
        Some("textbox") | Some("searchbox") | Some("checkbox") | Some("combobox")
        | Some("option") | Some("listbox") => return true,
        Some("dialog") | Some("alertdialog") | Some("row") | Some("navigation") => return false,
        _ => node.tag.to_ascii_uppercase(),
    };
    matches!(
        role.as_str(),
        "BUTTON" | "A" | "INPUT" | "TEXTAREA" | "SELECT"
    )
}

fn ax_role_kept(role: &str) -> bool {
    !matches!(
        role.to_ascii_lowercase().as_str(),
        "statictext"
            | "inlinetextbox"
            | "none"
            | "generic"
            | "rootwebarea"
            | "genericcontainer"
            | "inline"
            | ""
    )
}

fn ax_role_is_clickable(role: &str) -> bool {
    matches!(
        role.to_ascii_lowercase().as_str(),
        "button"
            | "link"
            | "textbox"
            | "searchbox"
            | "textfield"
            | "combobox"
            | "listbox"
            | "option"
            | "checkbox"
            | "menuitem"
            | "tab"
            | "slider"
    )
}

#[cfg(test)]
mod hit_script_tests {
    use super::*;
    #[test]
    fn one_button_page_scripts_one_hit_test() {
        let page = PageSpec::of(
            &[Control::button(
                10,
                100,
                "Sign in",
                (100.0, 200.0, 80.0, 32.0),
            )],
            "http://x",
            "X",
        );
        let json = ScriptBuilder::new().observe(&page).to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let hits: Vec<_> = value["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["method"] == "DOM.getNodeForLocation")
            .collect();
        eprintln!("calls={}", value["calls"].as_array().unwrap().len());
        eprintln!("hits={hits:?}");
        assert_eq!(hits.len(), 1, "{json}");
        assert_eq!(hits[0]["result"]["backendNodeId"], 100);
    }
}

#[cfg(test)]
mod overlay_script_tests {
    use super::*;
    #[test]
    fn cookie_backdrop_script_shape() {
        let backdrop = DomSpec::container(
            50,
            500,
            "generic",
            "Cookie consent",
            (0.0, 0.0, 1440.0, 900.0),
        );
        let save = DomSpec::button(10, 100, "Save", (1200.0, 780.0, 100.0, 36.0));
        let accept = DomSpec::button(51, 510, "Accept all", (1200.0, 40.0, 120.0, 36.0));
        let mut page = PageSpec::new(
            vec![save, backdrop.with_children(vec![accept])],
            vec![
                AxSpec::new(100, "button", "Save", (1200.0, 780.0, 100.0, 36.0)),
                AxSpec::new(500, "generic", "Cookie consent", (0.0, 0.0, 1440.0, 900.0)),
                AxSpec::new(510, "button", "Accept all", (1200.0, 40.0, 120.0, 36.0)),
            ],
            "http://127.0.0.1/docs",
            "Docs",
        );
        page = page.cover(100, 500);
        let json = ScriptBuilder::new().observe(&page).to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let methods: Vec<_> = value["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["method"].as_str().unwrap())
            .collect();
        assert!(methods.contains(&"DOM.getNodeForLocation"));
        assert_eq!(clickable_targets(&page).len(), 2);
        let hits: Vec<_> = value["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["method"] == "DOM.getNodeForLocation")
            .map(|c| c["result"]["backendNodeId"].as_i64().unwrap())
            .collect();
        assert_eq!(hits, vec![500, 510]); // Save covered by backdrop 500
    }
}
