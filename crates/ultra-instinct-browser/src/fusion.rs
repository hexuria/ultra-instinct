//! Fuse a DOM node and an accessibility node that describe one control.
//!
//! Fusion runs label folding, then two passes.
//!
//! 0. A text element that labels one control (`<label for>`, a `<label>`
//!    wrapping its control, or a generic element a control names with
//!    `aria-labelledby`, linked by `extract`) is folded into it
//!    ([`fold_labels`]): one region, not two. The label's other targets (a
//!    link inside its text) stay regions of their own.
//! 1. A DOM node and an accessibility node with the same backend node id are
//!    the same control and always merge. Chrome gives both trees that key.
//! 2. Only nodes left over from pass 1 are paired heuristically, when all of
//!    these hold:
//!    - label Jaccard is at least [`MIN_LABEL_JACCARD`], or either label is empty
//!    - roles are equal, or either role is [`Role::Generic`]
//!    - intersection-over-union is at least [`MIN_IOU`], or centers are at most
//!      [`MAX_CENTROID_PX`] pixels apart
//!
//! Pass 2 is greedy: each DOM node takes the compatible accessibility node
//! with the highest IoU, then the smaller centroid distance, then the lower
//! index. Leftovers stay separate.
//!
//! A DOM region's parent is its nearest kept DOM ancestor that is also a
//! region in this manifold. The stored rectangle is the DOM rectangle
//! when both exist, so a one-pixel accessibility shift does not move the region.
//! The fused id is `n{backendNodeId}` from the DOM node. The session's identity
//! map may carry an earlier stable id over it. It is not derived
//! from the rectangle, the enabled bit, or the label.
//!
//! Accepted downside: two same-label, same-role controls whose centers are
//! within 8px can fuse even when IoU is low. A second accessibility node for
//! the same DOM node is left over; it is not deleted. Two AX-only nodes that
//! share a backend id are kept as `ax{id}` and `ax{id}-{k}`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use ultra_instinct_core::{
    token_jaccard, InteractionManifold, InteractionRegion, Rect, RegionFlags, RegionId,
    RegionParts, Role, SourceMask, UnitInterval,
};
use ultra_instinct_geometry::is_fully_offscreen;

use crate::error::BrowserError;
use crate::extract::{AxElement, DomElement};

pub const MIN_LABEL_JACCARD: f64 = 0.5;
pub const MIN_IOU: f64 = 0.5;
pub const MAX_CENTROID_PX: f64 = 8.0;

#[derive(Clone, Debug)]
pub(crate) struct RawNode {
    pub dom_node_id: Option<i64>,
    pub backend_node_id: Option<i64>,
    pub role: Role,
    pub label: String,
    pub rect: Rect,
    pub actions: Vec<ultra_instinct_core::Action>,
    pub disabled: bool,
    pub hidden: bool,
    pub readonly: bool,
    pub modal: bool,
    pub from_dom: bool,
    pub from_ax: bool,
    /// Backend ids of kept DOM ancestors, nearest first.
    pub ancestors: Vec<i64>,
    /// The one control this text element labels (DOM nodes only).
    pub labels: Option<LabelTarget>,
}

/// The control a label names, as `extract` saw it in the DOM, and how the
/// pointer sees that control (`sight`, filled in by observe).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LabelTarget {
    pub control: i64,
    pub role: Role,
    pub actions: Vec<ultra_instinct_core::Action>,
    /// The control is disabled or hidden in markup.
    pub off: bool,
    /// A `<label>` element (clicking it clicks the control), not an
    /// `aria-labelledby` text.
    pub label_element: bool,
    pub sight: Sight,
}

/// Compact-walk evidence about a labelled control's own box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Sight {
    /// No evidence (no compact walk, an untagged node, `display: none`, or
    /// no hit test): nothing is folded.
    #[default]
    Unknown,
    /// Rendered with a box, not transparent, and a hit test at its center
    /// lands on it.
    Visible,
    /// Rendered but not where a pointer meets it: no box (zero size),
    /// `opacity: 0`, or something else under its center.
    Faint,
}

impl RawNode {
    pub(crate) fn from_dom(element: &DomElement, rect: Rect) -> Self {
        Self {
            dom_node_id: Some(element.node_id),
            backend_node_id: Some(element.backend_node_id),
            role: element.role,
            label: element.label.clone(),
            rect,
            actions: element.actions.clone(),
            disabled: element.disabled,
            hidden: element.hidden,
            readonly: element.readonly,
            modal: element.modal,
            from_dom: true,
            from_ax: false,
            ancestors: element.ancestors.clone(),
            labels: element.labels.clone(),
        }
    }

    pub(crate) fn from_ax(element: &AxElement, rect: Rect) -> Self {
        Self {
            dom_node_id: None,
            backend_node_id: element.backend_dom_node_id,
            role: element.role,
            label: element.name.clone(),
            rect,
            actions: crate::extract::actions_for_role(element.role),
            disabled: element.disabled,
            hidden: false,
            readonly: false,
            modal: element.modal,
            from_dom: false,
            from_ax: true,
            ancestors: Vec::new(),
            labels: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NodeBinding {
    pub dom_node_id: Option<i64>,
    pub backend_node_id: Option<i64>,
    pub center_x: f64,
    pub center_y: f64,
}

pub(crate) fn fuse(
    viewport: Rect,
    dom_nodes: &[RawNode],
    ax_nodes: &[RawNode],
) -> Result<(InteractionManifold, BTreeMap<RegionId, NodeBinding>), BrowserError> {
    let (dom_nodes, ax_nodes) = fold_labels(viewport, dom_nodes, ax_nodes);
    let (dom_nodes, ax_nodes) = (&*dom_nodes, &*ax_nodes);
    let mut used = vec![false; ax_nodes.len()];
    // backend id -> ax indices, so the deterministic pass-1 join is a map
    // lookup, not a linear scan per dom node.
    let mut by_backend: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (index, ax) in ax_nodes.iter().enumerate() {
        if let Some(backend) = ax.backend_node_id {
            by_backend.entry(backend).or_default().push(index);
        }
    }
    let mut joined: Vec<Option<usize>> = vec![None; dom_nodes.len()];
    for (dom_index, dom) in dom_nodes.iter().enumerate() {
        let Some(backend) = dom.backend_node_id else {
            continue;
        };
        if let Some(&ax_index) = by_backend
            .get(&backend)
            .and_then(|indices| indices.iter().find(|&&index| !used[index]))
        {
            used[ax_index] = true;
            joined[dom_index] = Some(ax_index);
        }
    }
    let mut fused = Vec::new();
    for (dom_index, dom) in dom_nodes.iter().enumerate() {
        if let Some(ax_index) = joined[dom_index] {
            fused.push(merge(dom, &ax_nodes[ax_index]));
            continue;
        }
        let mut best: Option<(usize, f64, f64)> = None;
        for (index, ax) in ax_nodes.iter().enumerate() {
            if used[index] || !compatible(dom, ax) {
                continue;
            }
            let overlap = iou(dom.rect, ax.rect);
            let distance = dom.rect.center().distance(ax.rect.center());
            let replace = match best {
                None => true,
                Some((_, best_iou, best_distance)) => {
                    overlap > best_iou
                        || ((overlap - best_iou).abs() < 1e-12 && distance < best_distance)
                }
            };
            if replace {
                best = Some((index, overlap, distance));
            }
        }
        if let Some((index, _, _)) = best {
            used[index] = true;
            fused.push(merge(dom, &ax_nodes[index]));
        } else {
            fused.push(dom.clone());
        }
    }
    for (index, ax) in ax_nodes.iter().enumerate() {
        if !used[index] {
            fused.push(ax.clone());
        }
    }

    let mut ids = Vec::with_capacity(fused.len());
    let mut used: BTreeSet<RegionId> = BTreeSet::new();
    for node in &fused {
        ids.push(unique_region_id(node, &mut used)?);
    }
    let present: std::collections::BTreeSet<&RegionId> = ids.iter().collect();
    let mut regions = Vec::new();
    let mut bindings = BTreeMap::new();
    for (node, id) in fused.iter().zip(&ids) {
        let parent = node
            .ancestors
            .iter()
            .filter_map(|backend| RegionId::try_new(format!("n{backend}")).ok())
            .find(|candidate| present.contains(candidate));
        let id = id.clone();
        let region = to_region(&id, node, viewport, parent)?;
        let center = region.rect().center();
        bindings.insert(
            id.clone(),
            NodeBinding {
                dom_node_id: node.dom_node_id,
                backend_node_id: node.backend_node_id,
                center_x: center.x(),
                center_y: center.y(),
            },
        );
        regions.push(region);
    }
    let manifold = InteractionManifold::try_new(viewport, regions, 0)
        .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))?;
    Ok((manifold, bindings))
}

fn merge(dom: &RawNode, ax: &RawNode) -> RawNode {
    let label = if ax.label.is_empty() {
        dom.label.clone()
    } else {
        ax.label.clone()
    };
    let mut actions = dom.actions.clone();
    actions.extend(ax.actions.iter().copied());
    RawNode {
        dom_node_id: dom.dom_node_id,
        backend_node_id: dom.backend_node_id.or(ax.backend_node_id),
        role: if dom.role == Role::Generic {
            ax.role
        } else {
            dom.role
        },
        label,
        rect: dom.rect,
        actions,
        disabled: dom.disabled || ax.disabled,
        hidden: dom.hidden || ax.hidden,
        readonly: dom.readonly || ax.readonly,
        modal: dom.modal || ax.modal,
        from_dom: true,
        from_ax: true,
        ancestors: dom.ancestors.clone(),
        labels: None,
    }
}

/// Fold each label into the one control it names, so a field is one region.
///
/// - **Forward** (the usual case): the control is in this observation,
///   offered (enabled, shown, a box with area, not fully offscreen, not
///   disabled in either tree), and [`Sight::Visible`]. The label's nodes are
///   dropped; the control keeps its own name, box, and id.
/// - **Reverse** (a custom checkbox: the native input is zero-size,
///   transparent, or covered, and the styled `<label>` is what a reader
///   clicks): a [`Sight::Faint`] checkbox or radio, enabled, named by a
///   `<label>` element that holds no other click target. The label's node
///   stays (its box, id, and binding: a click on it toggles the control)
///   and takes the control's role and claims; the control's nodes are
///   dropped. The label's control state is the control's (compact walk).
/// - Otherwise nothing changes: a label for nothing, for a disabled or
///   hidden control, for a control not observed, or without evidence.
///
/// Inner targets of a label (a link in its text) are their own nodes and
/// are never dropped. The only visible clickable box is never dropped.
fn fold_labels<'a>(
    viewport: Rect,
    dom_nodes: &'a [RawNode],
    ax_nodes: &'a [RawNode],
) -> (Cow<'a, [RawNode]>, Cow<'a, [RawNode]>) {
    use ultra_instinct_core::Action;
    let unchanged = (Cow::Borrowed(dom_nodes), Cow::Borrowed(ax_nodes));
    if dom_nodes.iter().all(|node| node.labels.is_none()) {
        return unchanged;
    }
    let clicks = |node: &RawNode| node.actions.contains(&Action::Click);
    let mut index = BTreeMap::new();
    // Kept ancestor → (nodes under it, click targets under it).
    let mut held: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    for (i, node) in dom_nodes.iter().enumerate() {
        if let Some(backend) = node.backend_node_id {
            index.entry(backend).or_insert(i);
        }
        for ancestor in &node.ancestors {
            let entry = held.entry(*ancestor).or_default();
            entry.0 += 1;
            entry.1 += usize::from(clicks(node));
        }
    }
    let ax_disabled: BTreeSet<i64> = ax_nodes
        .iter()
        .filter(|ax| ax.disabled)
        .filter_map(|ax| ax.backend_node_id)
        .collect();
    let mut dropped = BTreeSet::new();
    let mut reversed = BTreeMap::new();
    let mut reversed_controls = BTreeSet::new();
    for (i, node) in dom_nodes.iter().enumerate() {
        let (Some(target), Some(label)) = (&node.labels, node.backend_node_id) else {
            continue;
        };
        if target.off || ax_disabled.contains(&target.control) {
            continue;
        }
        let control = index.get(&target.control).map(|&j| &dom_nodes[j]);
        // What the label holds besides its own control.
        let inside = control.is_some_and(|c| c.ancestors.contains(&label));
        let (all, click_targets) = held.get(&label).copied().unwrap_or_default();
        let others = all - usize::from(inside);
        let other_clicks = click_targets - usize::from(inside && control.is_some_and(clicks));
        // Text named by `aria-labelledby` folds only when it is plain text:
        // nothing inside, and no click of its own (markup, a listener, or
        // its own pointer cursor, all on the node by now).
        if !target.label_element && (others > 0 || clicks(node)) {
            continue;
        }
        let offered = control.is_some_and(|c| {
            !c.disabled
                && !c.hidden
                && !c.rect.is_zero_area()
                && !is_fully_offscreen(c.rect, viewport)
        });
        match target.sight {
            Sight::Visible if offered => {
                dropped.insert(label);
            }
            Sight::Faint
                if target.label_element
                    && target.role == Role::Checkbox
                    && other_clicks == 0
                    && reversed_controls.insert(target.control) =>
            {
                reversed.insert(i, target);
                dropped.insert(target.control);
            }
            _ => {}
        }
    }
    if dropped.is_empty() {
        return unchanged;
    }
    let kept = |node: &&RawNode| {
        !node
            .backend_node_id
            .is_some_and(|backend| dropped.contains(&backend))
    };
    let dom = dom_nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| kept(node))
        .map(|(i, node)| {
            let mut node = node.clone();
            if let Some(target) = reversed.get(&i) {
                node.role = target.role;
                for action in &target.actions {
                    if !node.actions.contains(action) {
                        node.actions.push(*action);
                    }
                }
            }
            node
        })
        .collect();
    let ax = ax_nodes.iter().filter(kept).cloned().collect();
    (Cow::Owned(dom), Cow::Owned(ax))
}

fn compatible(dom: &RawNode, ax: &RawNode) -> bool {
    // Cheapest first: rect math and role equality reject most pairs before
    // the allocating token_jaccard runs.
    geometry_compatible(dom.rect, ax.rect)
        && roles_compatible(dom.role, ax.role)
        && labels_compatible(&dom.label, &ax.label)
}

fn labels_compatible(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return true;
    }
    token_jaccard(left, right) >= MIN_LABEL_JACCARD
}

fn roles_compatible(left: Role, right: Role) -> bool {
    left == right || left == Role::Generic || right == Role::Generic
}

fn geometry_compatible(left: Rect, right: Rect) -> bool {
    iou(left, right) >= MIN_IOU || left.center().distance(right.center()) <= MAX_CENTROID_PX
}

fn iou(left: Rect, right: Rect) -> f64 {
    let x1 = left.x().max(right.x());
    let y1 = left.y().max(right.y());
    let x2 = left.right().min(right.right());
    let y2 = left.bottom().min(right.bottom());
    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = left.width() * left.height() + right.width() * right.height() - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn preferred_id(node: &RawNode) -> String {
    if node.from_dom {
        match node.backend_node_id {
            Some(id) => format!("n{id}"),
            None => match node.dom_node_id {
                Some(id) => format!("dom{id}"),
                None => "dom-unknown".to_owned(),
            },
        }
    } else {
        match node.backend_node_id {
            Some(id) => format!("ax{id}"),
            None => "ax-unknown".to_owned(),
        }
    }
}

/// Prefer `n{backend}` / `ax{backend}`. A second AX-only node that would
/// collide is minted as `{preferred}-{k}` so observe does not fail. Two DOM
/// nodes with one backend are still a duplicate: that is a Chrome bug, not a
/// second AX node.
fn unique_region_id(
    node: &RawNode,
    used: &mut BTreeSet<RegionId>,
) -> Result<RegionId, BrowserError> {
    let preferred = preferred_id(node);
    let mut candidate = RegionId::try_new(&preferred)
        .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))?;
    if used.insert(candidate.clone()) {
        return Ok(candidate);
    }
    if node.from_dom {
        return Err(BrowserError::DuplicateRegion(format!(
            "duplicate region id `{preferred}`"
        )));
    }
    let mut k = 2u32;
    loop {
        candidate = RegionId::try_new(format!("{preferred}-{k}"))
            .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))?;
        if used.insert(candidate.clone()) {
            return Ok(candidate);
        }
        k += 1;
    }
}

fn to_region(
    id: &RegionId,
    node: &RawNode,
    viewport: Rect,
    parent: Option<RegionId>,
) -> Result<InteractionRegion, BrowserError> {
    let mut sources = SourceMask::NONE;
    if node.from_dom {
        sources = sources.union(SourceMask::DOM);
    }
    if node.from_ax {
        sources = sources.union(SourceMask::ACCESSIBILITY);
    }
    let mut flags = RegionFlags::none();
    flags.set_disabled(node.disabled);
    flags.set_hidden(node.hidden);
    flags.set_readonly(node.readonly);
    // A modal bit on a non-dialog says nothing about layering; drop it.
    flags.set_modal(node.modal && node.role == Role::Dialog);
    if is_fully_offscreen(node.rect, viewport) {
        flags.set_offscreen(true);
    }
    InteractionRegion::try_new(RegionParts {
        id: id.clone(),
        role: node.role,
        label: node.label.clone(),
        rect: node.rect,
        actions: node.actions.clone(),
        parent,
        sources,
        flags,
        temporal_stability: UnitInterval::ONE,
    })
    .map_err(|err| BrowserError::BadViewport(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::Action;

    fn rect(x: f64, y: f64) -> Rect {
        Rect::try_new(x, y, 80.0, 32.0).unwrap()
    }

    fn dom(label: &str, role: Role, x: f64) -> RawNode {
        RawNode {
            dom_node_id: Some(10),
            backend_node_id: Some(100),
            role,
            label: label.into(),
            rect: rect(x, 200.0),
            actions: vec![Action::Click],
            disabled: false,
            hidden: false,
            readonly: false,
            modal: false,
            from_dom: true,
            from_ax: false,
            ancestors: Vec::new(),
            labels: None,
        }
    }

    fn ax(label: &str, role: Role, x: f64) -> RawNode {
        RawNode {
            dom_node_id: None,
            backend_node_id: Some(100),
            role,
            label: label.into(),
            rect: rect(x, 200.0),
            actions: vec![Action::Click],
            disabled: false,
            hidden: false,
            readonly: false,
            modal: false,
            from_dom: false,
            from_ax: true,
            ancestors: Vec::new(),
            labels: None,
        }
    }

    fn with_backend(mut node: RawNode, backend: i64) -> RawNode {
        node.backend_node_id = Some(backend);
        node
    }

    fn viewport() -> Rect {
        Rect::try_viewport(0.0, 0.0, 1280.0, 720.0).unwrap()
    }

    #[test]
    fn one_pixel_shift_merges_and_different_labels_do_not() {
        let (merged, bindings) = fuse(
            viewport(),
            &[dom("Sign in", Role::Button, 400.0)],
            &[ax("Sign in", Role::Button, 401.0)],
        )
        .unwrap();
        assert_eq!(merged.len(), 1);
        let region = merged.get_str("n100").unwrap();
        assert_eq!(region.label(), "Sign in");
        assert_eq!(region.rect().x(), 400.0);
        assert!(region.sources().contains(SourceMask::DOM));
        assert!(region.sources().contains(SourceMask::ACCESSIBILITY));
        assert_eq!(bindings.get(region.id()).unwrap().dom_node_id, Some(10));

        let (separate, _) = fuse(
            viewport(),
            &[dom("Sign in", Role::Button, 400.0)],
            &[with_backend(ax("Cancel", Role::Button, 401.0), 101)],
        )
        .unwrap();
        assert_eq!(
            separate.len(),
            2,
            "different labels on different nodes must not fuse"
        );
        assert!(separate.get_str("n100").is_some());
        assert!(separate.get_str("ax101").is_some());
    }

    #[test]
    fn role_mismatch_does_not_merge_and_empty_label_may() {
        let (separate, _) = fuse(
            viewport(),
            &[dom("Sign in", Role::Button, 400.0)],
            &[with_backend(ax("Sign in", Role::Link, 400.0), 101)],
        )
        .unwrap();
        assert_eq!(separate.len(), 2);

        let (merged, _) = fuse(
            viewport(),
            &[dom("Sign in", Role::Button, 400.0)],
            &[ax("", Role::Button, 401.0)],
        )
        .unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged.get_str("n100").unwrap().label(), "Sign in");
    }

    #[test]
    fn same_backend_node_joins_even_when_labels_differ() {
        // `<a>Read more<span class="sr-only"> about our pricing plans</span></a>`:
        // the DOM text and the accessible name differ, but it is one control.
        let (merged, _) = fuse(
            viewport(),
            &[dom("Read more", Role::Link, 400.0)],
            &[ax("Read more about our pricing plans", Role::Link, 400.0)],
        )
        .unwrap();
        assert_eq!(merged.len(), 1);
        let region = merged.get_str("n100").unwrap();
        assert_eq!(region.label(), "Read more about our pricing plans");
        assert!(region.sources().contains(SourceMask::ACCESSIBILITY));
    }

    #[test]
    fn heuristic_pass_only_sees_unjoined_nodes() {
        // ax200 is joined to its own DOM node by backend id, so the heuristic
        // pass cannot also give it to n100 even though it overlaps.
        let mut other = dom("Sign in", Role::Button, 401.0);
        other.backend_node_id = Some(200);
        other.dom_node_id = Some(20);
        let (merged, _) = fuse(
            viewport(),
            &[dom("Sign in", Role::Button, 400.0), other],
            &[with_backend(ax("Sign in", Role::Button, 401.0), 200)],
        )
        .unwrap();
        assert_eq!(merged.len(), 2);
        assert!(!merged
            .get_str("n100")
            .unwrap()
            .sources()
            .contains(SourceMask::ACCESSIBILITY));
        assert!(merged
            .get_str("n200")
            .unwrap()
            .sources()
            .contains(SourceMask::ACCESSIBILITY));
    }

    #[test]
    fn two_ax_nodes_with_one_backend_id_stay_separate() {
        // Reproduce: two accessibility-only nodes share a backend and no DOM
        // partner. Observe must not fail with DuplicateRegion.
        let left = with_backend(ax("Sign in", Role::Button, 400.0), 100);
        let right = with_backend(ax("Cancel", Role::Button, 500.0), 100);
        let (merged, _) = fuse(viewport(), &[], &[left, right]).unwrap();
        assert_eq!(merged.len(), 2);
        let ids: Vec<_> = merged.ids().map(|id| id.as_str().to_owned()).collect();
        assert_eq!(ids, ["ax100", "ax100-2"]);
        assert_eq!(merged.get_str("ax100").unwrap().label(), "Sign in");
        assert_eq!(merged.get_str("ax100-2").unwrap().label(), "Cancel");
    }

    fn label(backend: i64, control: i64, role: Role, sight: Sight) -> RawNode {
        let mut node = with_backend(dom("Remember me", Role::Generic, 100.0), backend);
        node.rect = Rect::try_new(100.0, 400.0, 120.0, 20.0).unwrap();
        node.labels = Some(LabelTarget {
            control,
            role,
            actions: crate::extract::actions_for_role(role),
            off: false,
            label_element: true,
            sight,
        });
        node
    }

    fn fold(dom: &[RawNode], ax: &[RawNode]) -> InteractionManifold {
        fuse(viewport(), dom, ax).unwrap().0
    }

    fn ids(manifold: &InteractionManifold) -> Vec<&str> {
        manifold.ids().map(RegionId::as_str).collect()
    }

    #[test]
    fn a_label_folds_into_the_visible_control_it_names() {
        let field = dom("", Role::Checkbox, 400.0);
        let mut link = with_backend(dom("terms", Role::Link, 150.0), 52);
        link.ancestors = vec![50];
        let label = label(50, 100, Role::Checkbox, Sight::Visible);
        let label_ax = with_backend(ax("Remember me", Role::Generic, 100.0), 50);
        let field_ax = ax("Remember me", Role::Checkbox, 400.0);
        let merged = fold(&[label, field, link], &[label_ax, field_ax]);
        assert_eq!(
            ids(&merged),
            ["n100", "n52"],
            "label nodes in both trees go"
        );
        assert_eq!(merged.get_str("n100").unwrap().label(), "Remember me");
        assert_eq!(merged.get_str("n52").unwrap().parent(), None);
    }

    #[test]
    fn a_label_stays_for_an_off_unobserved_or_unseen_control() {
        let field = || dom("", Role::TextField, 400.0);
        let mut off = label(50, 100, Role::TextField, Sight::Visible);
        off.labels.as_mut().unwrap().off = true;
        let mut disabled = field();
        disabled.disabled = true;
        let mut ax_disabled = ax("Email", Role::TextField, 400.0);
        ax_disabled.disabled = true;
        let visible = || label(50, 100, Role::TextField, Sight::Visible);
        let cases = [
            (vec![off, field()], vec![]),
            (vec![visible(), disabled], vec![]),
            (vec![visible(), field()], vec![ax_disabled]),
            (vec![visible()], vec![]),
            (
                vec![label(50, 100, Role::TextField, Sight::Unknown), field()],
                vec![],
            ),
            // A faint text field: typing into its label would type nowhere.
            (
                vec![label(50, 100, Role::TextField, Sight::Faint), field()],
                vec![],
            ),
        ];
        for (dom, ax) in cases {
            let merged = fold(&dom, &ax);
            assert!(merged.get_str("n50").is_some(), "{:?}", ids(&merged));
            assert_eq!(merged.get_str("n50").unwrap().role(), Role::Generic);
        }
    }

    #[test]
    fn labelledby_text_folds_only_when_it_is_plain_text() {
        let text = || {
            let mut node = label(50, 100, Role::TextField, Sight::Visible);
            node.labels.as_mut().unwrap().label_element = false;
            node.actions = vec![Action::Focus];
            node
        };
        let field = || dom("", Role::TextField, 400.0);
        assert_eq!(ids(&fold(&[text(), field()], &[])), ["n100"]);
        // Its own click (a pointer cursor, a listener) keeps it a target.
        let mut clickable = text();
        clickable.actions.push(Action::Click);
        assert_eq!(ids(&fold(&[clickable, field()], &[])), ["n100", "n50"]);
        // So does another node inside it.
        let mut inner = with_backend(dom("more", Role::Generic, 150.0), 52);
        inner.ancestors = vec![50];
        inner.actions = vec![Action::Focus];
        assert_eq!(
            ids(&fold(&[text(), field(), inner], &[])),
            ["n100", "n50", "n52"]
        );
    }

    #[test]
    fn a_faint_checkbox_folds_into_its_label_box() {
        let hidden_input = dom("", Role::Checkbox, 104.0);
        let input_ax = ax("Remember me", Role::Checkbox, 104.0);
        let merged = fold(
            &[
                label(50, 100, Role::Checkbox, Sight::Faint),
                hidden_input.clone(),
            ],
            &[input_ax],
        );
        assert_eq!(ids(&merged), ["n50"], "the only visible box is kept");
        let region = merged.get_str("n50").unwrap();
        assert_eq!(
            (region.role(), region.rect().width()),
            (Role::Checkbox, 120.0)
        );
        assert!(region.actions().contains(&Action::Toggle));
        // Not observed at all (a zero-size input): the label still stands for it.
        assert_eq!(
            ids(&fold(&[label(50, 100, Role::Checkbox, Sight::Faint)], &[])),
            ["n50"]
        );
        // A second label for the same input is left as it was.
        let second = label(51, 100, Role::Checkbox, Sight::Faint);
        let merged = fold(
            &[
                label(50, 100, Role::Checkbox, Sight::Faint),
                second,
                hidden_input.clone(),
            ],
            &[],
        );
        assert_eq!(ids(&merged), ["n50", "n51"]);
        assert_eq!(merged.get_str("n51").unwrap().role(), Role::Generic);
        // A label that holds a link of its own is not one click target.
        let mut link = with_backend(dom("terms", Role::Link, 150.0), 52);
        link.ancestors = vec![50];
        let merged = fold(
            &[
                label(50, 100, Role::Checkbox, Sight::Faint),
                hidden_input,
                link,
            ],
            &[],
        );
        assert_eq!(ids(&merged), ["n100", "n50", "n52"]);
    }

    #[test]
    fn parent_is_the_nearest_ancestor_that_is_a_region() {
        let mut nav = dom("Sidebar", Role::Navigation, 0.0);
        nav.backend_node_id = Some(50);
        nav.dom_node_id = Some(5);
        let mut child = dom("Settings", Role::Button, 400.0);
        // 77 is a kept ancestor with no box, so it is not a region.
        child.ancestors = vec![77, 50];
        let (merged, _) = fuse(viewport(), &[nav, child], &[]).unwrap();
        assert_eq!(
            merged
                .get_str("n100")
                .unwrap()
                .parent()
                .map(RegionId::as_str),
            Some("n50")
        );
        assert_eq!(merged.get_str("n50").unwrap().parent(), None);
    }
}
