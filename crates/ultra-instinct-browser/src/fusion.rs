//! Fuse a DOM node and an accessibility node that describe one control.
//!
//! Version 2 runs two passes.
//!
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
    }
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
