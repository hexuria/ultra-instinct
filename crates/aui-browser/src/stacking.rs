//! Approximate CSS paint order among kept regions.
//!
//! Observe already hit-tests each clickable center with
//! `DOM.getNodeForLocation` (cookie banners and unlabeled overlays). That
//! answers "what owns this pixel?" for one point. This module answers a
//! different question: among **kept** regions whose boxes overlap, which one
//! paints on top, using each node's own computed style.
//!
//! Paint key (honest approximation, not full CSS Appendix E):
//!
//! 1. tier 0: in-flow `position: static` that does not create a stacking
//!    context;
//! 2. tier 1: positioned / sticky / fixed, or a stacking context (`opacity`
//!    < 1, `transform`, `filter`, `isolation`, `mix-blend-mode`,
//!    `will-change`, or non-auto `z-index` on a positioned element);
//! 3. within a tier, higher `z-index` (auto → 0) wins;
//! 4. then later document order wins.
//!
//! A coverer occludes a clickable victim when all of these hold:
//!
//! - the coverer's box contains the victim's center;
//! - the coverer has `pointer-events` other than `none`;
//! - the coverer is neither an ancestor nor a descendant of the victim;
//! - the coverer's paint key is strictly greater than the victim's.
//!
//! Known CDP limits (see docs/DECISIONS.md): we do not rebuild the full
//! ancestor stacking-context chain through non-kept nodes; clip-path, mask,
//! canvas, cross-origin iframes, and closed shadow trees are invisible here.

use std::collections::{BTreeMap, BTreeSet};

use aui_core::{Action, InteractionManifold, InteractionRegion, Rect, RegionId};

/// Computed style fields that affect paint order for one kept region.
#[derive(Clone, Debug, PartialEq)]
pub struct StackingStyle {
    pub z_index: Option<i64>,
    pub position: PositionKind,
    pub opacity: f64,
    pub has_transform: bool,
    pub has_filter: bool,
    pub isolation: bool,
    pub mix_blend: bool,
    pub will_change_stacking: bool,
    /// `false` when `pointer-events: none` — the region cannot cover others.
    pub pointer_events: bool,
}

/// CSS `position` used for the stacking approximation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionKind {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

impl Default for StackingStyle {
    fn default() -> Self {
        Self {
            z_index: None,
            position: PositionKind::Static,
            opacity: 1.0,
            has_transform: false,
            has_filter: false,
            isolation: false,
            mix_blend: false,
            will_change_stacking: false,
            pointer_events: true,
        }
    }
}

impl StackingStyle {
    pub fn creates_context(&self) -> bool {
        matches!(self.position, PositionKind::Fixed | PositionKind::Sticky)
            || ((matches!(
                self.position,
                PositionKind::Absolute | PositionKind::Relative
            ) && self.z_index.is_some())
                || self.opacity < 1.0 - 1e-9
                || self.has_transform
                || self.has_filter
                || self.isolation
                || self.mix_blend
                || self.will_change_stacking)
    }

    /// Higher key paints above a lower key.
    pub fn paint_key(&self, tree_order: u32) -> (u8, i64, u32) {
        let tier = if self.creates_context() || self.position != PositionKind::Static {
            1
        } else {
            0
        };
        (tier, self.z_index.unwrap_or(0), tree_order)
    }
}

/// Parse `CSS.getComputedStyleForNode` result (`{computedStyle: [{name,value}]}`).
pub fn style_from_computed(computed: &[(String, String)]) -> StackingStyle {
    let mut style = StackingStyle::default();
    for (name, value) in computed {
        let name = name.as_str();
        let value = value.as_str();
        match name {
            "z-index" => {
                style.z_index = if value == "auto" {
                    None
                } else {
                    value.parse().ok()
                };
            }
            "position" => {
                style.position = match value {
                    "relative" => PositionKind::Relative,
                    "absolute" => PositionKind::Absolute,
                    "fixed" => PositionKind::Fixed,
                    "sticky" => PositionKind::Sticky,
                    _ => PositionKind::Static,
                };
            }
            "opacity" => {
                style.opacity = value.parse().unwrap_or(1.0);
            }
            "transform" => {
                style.has_transform = value != "none";
            }
            "filter" => {
                style.has_filter = value != "none";
            }
            "isolation" => {
                style.isolation = value == "isolate";
            }
            "mix-blend-mode" => {
                style.mix_blend = value != "normal";
            }
            "will-change" => {
                let lower = value.to_ascii_lowercase();
                style.will_change_stacking = lower.split(',').any(|part| {
                    let part = part.trim();
                    part == "transform" || part == "opacity" || part == "filter"
                });
            }
            "pointer-events" => {
                style.pointer_events = value != "none";
            }
            _ => {}
        }
    }
    style
}

/// Mark clickable regions whose center is covered by a higher-painting kept
/// region. Ancestors and descendants do not bury one another. Regions already
/// `occluded` stay occluded. Returns how many newly buried ids were marked.
#[cfg(test)]
pub fn apply_stacking_occlusion(
    manifold: &mut InteractionManifold,
    styles: &BTreeMap<RegionId, (StackingStyle, u32)>,
) -> usize {
    apply_stacking_occlusion_except(manifold, styles, &BTreeSet::new())
}

/// [`apply_stacking_occlusion`], except victims in `hit_owned`: the browser's
/// own hit test at their center landed on them, which beats this
/// approximation (an unkept fixed ancestor can lift a static field above
/// positioned page content).
/// Regions a pointer must reach: CLICK, DRAG, HOVER, SCROLL_IN, and
/// SELECT_TEXT targets. Their centers are the ones occlusion checks.
pub(crate) fn pointer_target(region: &InteractionRegion) -> bool {
    region.actions().iter().any(|a| {
        matches!(
            a,
            Action::Click | Action::Drag | Action::Hover | Action::Scroll | Action::SelectText
        )
    })
}

pub fn apply_stacking_occlusion_except(
    manifold: &mut InteractionManifold,
    styles: &BTreeMap<RegionId, (StackingStyle, u32)>,
    hit_owned: &BTreeSet<RegionId>,
) -> usize {
    let regions: Vec<InteractionRegion> = manifold.regions().cloned().collect();
    let mut buried: Vec<RegionId> = Vec::new();
    for victim in &regions {
        if !pointer_target(victim) || victim.flags().occluded() || hit_owned.contains(victim.id()) {
            continue;
        }
        let Some((victim_style, victim_order)) = styles.get(victim.id()) else {
            continue;
        };
        let victim_key = victim_style.paint_key(*victim_order);
        let center = victim.rect().center();
        for coverer in &regions {
            if coverer.id() == victim.id() {
                continue;
            }
            let Some((cover_style, cover_order)) = styles.get(coverer.id()) else {
                continue;
            };
            if !cover_style.pointer_events {
                continue;
            }
            if !contains_point(coverer.rect(), center) {
                continue;
            }
            if manifold.is_within(victim.id(), coverer.id())
                || manifold.is_within(coverer.id(), victim.id())
            {
                continue;
            }
            if cover_style.paint_key(*cover_order) > victim_key {
                buried.push(victim.id().clone());
                break;
            }
        }
    }
    let n = buried.len();
    for id in buried {
        let region = manifold
            .get(&id)
            .expect("id taken from this manifold")
            .clone();
        if region.flags().occluded() {
            continue;
        }
        let mut parts = region.to_parts();
        parts.flags.set_occluded(true);
        let updated =
            InteractionRegion::try_new(parts).expect("rebuilding a valid region cannot fail");
        manifold.replace(updated);
    }
    n
}

fn contains_point(outer: Rect, point: aui_core::Point) -> bool {
    point.x() >= outer.x()
        && point.x() <= outer.right()
        && point.y() >= outer.y()
        && point.y() <= outer.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aui_core::{
        Action, Rect, RegionFlags, RegionId, RegionParts, Role, SourceMask, UnitInterval,
    };

    fn button(id: &str, rect: (f64, f64, f64, f64), parent: Option<&str>) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: id.into(),
            rect: Rect::try_new(rect.0, rect.1, rect.2, rect.3).unwrap(),
            actions: vec![Action::Click],
            parent: parent.map(|p| RegionId::try_new(p).unwrap()),
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn page(regions: Vec<InteractionRegion>) -> InteractionManifold {
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 1440.0, 900.0).unwrap(),
            regions,
            0,
        )
        .unwrap()
    }

    #[test]
    fn higher_z_index_overlay_buries_the_button_under_its_center() {
        let mut m = page(vec![
            button("save", (1200.0, 780.0, 100.0, 36.0), None),
            button("toast", (1180.0, 760.0, 160.0, 80.0), None),
        ]);
        let mut styles = BTreeMap::new();
        styles.insert(
            RegionId::try_new("save").unwrap(),
            (StackingStyle::default(), 0),
        );
        let toast = StackingStyle {
            position: PositionKind::Fixed,
            z_index: Some(9999),
            ..Default::default()
        };
        styles.insert(RegionId::try_new("toast").unwrap(), (toast, 1));
        assert_eq!(apply_stacking_occlusion(&mut m, &styles), 1);
        assert!(m.get_str("save").unwrap().flags().occluded());
        assert!(!m.get_str("toast").unwrap().flags().occluded());
    }

    #[test]
    fn browser_hit_on_the_victim_beats_the_paint_estimate() {
        // A static field in a fixed dialog whose wrapper is not a kept
        // region: the estimate ranks a positioned inbox row above it, but the
        // browser's hit test at the field's center landed on the field.
        let mut m = page(vec![
            button("field", (400.0, 600.0, 340.0, 32.0), None),
            button("row", (0.0, 580.0, 900.0, 60.0), None),
        ]);
        let mut styles = BTreeMap::new();
        styles.insert(
            RegionId::try_new("field").unwrap(),
            (StackingStyle::default(), 9),
        );
        let row = StackingStyle {
            position: PositionKind::Relative,
            ..Default::default()
        };
        styles.insert(RegionId::try_new("row").unwrap(), (row, 1));
        let hit_owned = BTreeSet::from([RegionId::try_new("field").unwrap()]);
        assert_eq!(
            apply_stacking_occlusion_except(&mut m.clone(), &styles, &BTreeSet::new()),
            1
        );
        assert_eq!(
            apply_stacking_occlusion_except(&mut m, &styles, &hit_owned),
            0
        );
        assert!(!m.get_str("field").unwrap().flags().occluded());
    }

    #[test]
    fn pointer_events_none_does_not_bury() {
        let mut m = page(vec![
            button("save", (1200.0, 780.0, 100.0, 36.0), None),
            button("veil", (1180.0, 760.0, 160.0, 80.0), None),
        ]);
        let mut styles = BTreeMap::new();
        styles.insert(
            RegionId::try_new("save").unwrap(),
            (StackingStyle::default(), 0),
        );
        let veil = StackingStyle {
            position: PositionKind::Fixed,
            z_index: Some(9999),
            pointer_events: false,
            ..Default::default()
        };
        styles.insert(RegionId::try_new("veil").unwrap(), (veil, 1));
        assert_eq!(apply_stacking_occlusion(&mut m, &styles), 0);
        assert!(!m.get_str("save").unwrap().flags().occluded());
    }

    #[test]
    fn ancestor_does_not_bury_its_descendant() {
        let mut m = page(vec![
            button("card", (100.0, 100.0, 400.0, 200.0), None),
            button("ok", (120.0, 150.0, 80.0, 30.0), Some("card")),
        ]);
        let mut styles = BTreeMap::new();
        let card = StackingStyle {
            z_index: Some(1),
            position: PositionKind::Relative,
            ..Default::default()
        };
        styles.insert(RegionId::try_new("card").unwrap(), (card, 0));
        styles.insert(
            RegionId::try_new("ok").unwrap(),
            (StackingStyle::default(), 1),
        );
        assert_eq!(apply_stacking_occlusion(&mut m, &styles), 0);
        assert!(!m.get_str("ok").unwrap().flags().occluded());
    }

    #[test]
    fn descendant_does_not_bury_its_ancestor() {
        let rect = (100.0, 100.0, 200.0, 40.0);
        let mut field = button("field", rect, None).to_parts();
        field.role = Role::TextField;
        field.actions.push(Action::Type);
        let field = InteractionRegion::try_new(field).unwrap();

        let mut editor = button("editor", rect, Some("field")).to_parts();
        editor.role = Role::Generic;
        editor.actions.clear();
        let editor = InteractionRegion::try_new(editor).unwrap();

        let mut m = page(vec![field, editor]);
        let mut styles = BTreeMap::new();
        styles.insert(
            RegionId::try_new("field").unwrap(),
            (StackingStyle::default(), 0),
        );
        styles.insert(
            RegionId::try_new("editor").unwrap(),
            (StackingStyle::default(), 1),
        );
        assert_eq!(apply_stacking_occlusion(&mut m, &styles), 0);
        assert!(!m.get_str("field").unwrap().flags().occluded());
    }

    #[test]
    fn style_from_computed_reads_stacking_fields() {
        let style = style_from_computed(&[
            ("z-index".into(), "42".into()),
            ("position".into(), "fixed".into()),
            ("opacity".into(), "0.9".into()),
            ("transform".into(), "matrix(1, 0, 0, 1, 0, 0)".into()),
            ("filter".into(), "none".into()),
            ("isolation".into(), "auto".into()),
            ("mix-blend-mode".into(), "normal".into()),
            ("will-change".into(), "transform".into()),
            ("pointer-events".into(), "auto".into()),
        ]);
        assert_eq!(style.z_index, Some(42));
        assert_eq!(style.position, PositionKind::Fixed);
        assert!((style.opacity - 0.9).abs() < 1e-9);
        assert!(style.has_transform);
        assert!(!style.has_filter);
        assert!(style.will_change_stacking);
        assert!(style.pointer_events);
        assert!(style.creates_context());
    }
}
