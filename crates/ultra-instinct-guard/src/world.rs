//! The front layer: open dialogs that sit above page content.
//!
//! A text/role match is not enough to allow a click. A "Delete" button under
//! an open modal matches "Delete" as well as it did before the modal opened,
//! and a script click (`element.click()`, which Browser Use, CUA wrappers,
//! and our own fixture press all reach for) fires on it anyway. A person could
//! not click it. This module decides which regions the front layer blocks.
//!
//! Signals, all already in the manifold:
//!
//! - a region with [`Role::Dialog`] is a dialog (`role="dialog"`,
//!   `role="alertdialog"`, `<dialog>`, or the accessibility role);
//! - [`modal()`](fn@ultra_instinct_core::RegionFlags::modal) marks it modal (`aria-modal="true"` or the
//!   accessibility `modal` property);
//! - the parent chain says what is inside the dialog.
//!
//! Rules. A dialog that is hidden or off screen is not in the front layer.
//! For each dialog `D` in the front layer and each region `R` that is not
//! inside `D`:
//!
//! - `D` modal: `R` is blocked.
//! - `D` not modal: `R` is blocked when its center is inside `D`'s box. That
//!   is where a pointer click would land.
//!
//! "Inside" means `R` is `D` or a descendant of `D` through parent links.
//! Parent links are only authoritative when both `R` and `D` came from the
//! DOM (the DOM walk records every kept ancestor). When either side is
//! accessibility-only, its parent link is unknown, so "inside" falls back to
//! `R`'s box lying fully inside `D`'s box.
//!
//! Occlusion outside dialogs: observe builds a stacking map from computed
//! styles and hit-tests clickable centers (`DOM.getNodeForLocation`), then
//! marks buried regions `occluded` before the guard runs. Cookie banners and
//! custom backdrops therefore refuse as `occluded` rather than as dialog
//! `front-layer`.
//!
//! Known gaps (see docs/DECISIONS.md): stacking compares each kept region's
//! own style, not the full ancestor stacking-context chain through non-kept
//! nodes; canvas / cross-origin iframes / closed shadows are invisible; a
//! native `<dialog>` opened with `showModal()` without `aria-modal` is modal
//! only if Chrome's accessibility tree says so; two sibling modals block
//! each other's content, which refuses rather than guesses.

use std::borrow::Cow;
use std::collections::BTreeSet;

use ultra_instinct_core::{
    Action, InteractionManifold, InteractionRegion, Rect, RegionId, Role, SourceMask,
};
use ultra_instinct_resonance::{RegionState, Visibility};

/// One dialog in the front layer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerEntry {
    pub id: RegionId,
    pub modal: bool,
}

/// The open dialogs of one observation, in region-id order. Two observations
/// with different front layers are different worlds for the guard.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FrontLayer {
    entries: Vec<LayerEntry>,
}

impl FrontLayer {
    pub fn of(manifold: &InteractionManifold) -> Self {
        let entries = dialogs(manifold)
            .map(|region| LayerEntry {
                id: region.id().clone(),
                modal: region.flags().modal(),
            })
            .collect();
        Self { entries }
    }

    pub fn entries(&self) -> &[LayerEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The world variables the guard compares across observations.
///
/// Built from one manifold (and optional focused id). A host that decided on
/// an older observation passes that snapshot as [`crate::GuardRequest::seen_world`];
/// when any field differs from the world observed now, the guard escalates
/// [`ultra_instinct_protocol::GuardReason::WorldChanged`].
///
/// Compared fields:
///
/// - [`Self::focused`]: accessibility focus (cargo-runner cursor);
/// - [`Self::front_layer`]: open dialogs;
/// - [`Self::clickable`]: clickable region ids present;
/// - [`Self::occluded`]: regions already marked occluded by observe (hit-test
///   or stacking). Front-layer blocking applied only inside the guard is
///   tracked by [`Self::front_layer`], not by baking dialog occlusion here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WorldSnapshot {
    focused: Option<RegionId>,
    front_layer: FrontLayer,
    clickable: BTreeSet<RegionId>,
    occluded: BTreeSet<RegionId>,
}

impl WorldSnapshot {
    /// Whole-page world (MCP ranked `guard` / `seen_world`). Any new clickable
    /// or occluded id anywhere counts as a change.
    pub fn of(manifold: &InteractionManifold, focused: Option<RegionId>) -> Self {
        let clickable = manifold
            .regions()
            .filter(|region| region.actions().contains(&Action::Click))
            .map(|region| region.id().clone())
            .collect();
        let occluded = manifold
            .regions()
            .filter(|region| region.flags().occluded())
            .map(|region| region.id().clone())
            .collect();
        Self {
            focused,
            front_layer: FrontLayer::of(manifold),
            clickable,
            occluded,
        }
    }

    /// Target-scoped world for agent tickets: front layer stays global (a modal
    /// anywhere still invalidates), but clickable / occluded sets are limited
    /// to the target's local neighborhood (ancestors, same-parent siblings,
    /// children, and geometrically nearby root peers). An unrelated banner
    /// elsewhere does not force a stale discard.
    pub fn of_target(
        manifold: &InteractionManifold,
        focused: Option<RegionId>,
        target: &RegionId,
    ) -> Self {
        let neighborhood = neighborhood_of(manifold, target);
        let clickable = neighborhood
            .iter()
            .filter_map(|id| manifold.get(id))
            .filter(|region| region.actions().contains(&Action::Click))
            .map(|region| region.id().clone())
            .collect();
        let occluded = neighborhood
            .iter()
            .filter_map(|id| manifold.get(id))
            .filter(|region| region.flags().occluded())
            .map(|region| region.id().clone())
            .collect();
        Self {
            focused,
            front_layer: FrontLayer::of(manifold),
            clickable,
            occluded,
        }
    }

    pub fn focused(&self) -> Option<&RegionId> {
        self.focused.as_ref()
    }

    pub fn front_layer(&self) -> &FrontLayer {
        &self.front_layer
    }

    pub fn clickable(&self) -> &BTreeSet<RegionId> {
        &self.clickable
    }

    pub fn occluded(&self) -> &BTreeSet<RegionId> {
        &self.occluded
    }
}

/// The dialog that blocks `region`, if any. With several, the first in
/// region-id order, modal or not.
pub fn blocker<'a>(
    manifold: &'a InteractionManifold,
    region: &InteractionRegion,
) -> Option<&'a InteractionRegion> {
    dialogs(manifold).find(|dialog| blocks(manifold, dialog, region))
}

/// A copy of `manifold` where every region the front layer blocks carries the
/// `occluded` flag. Ranking this copy applies the versioned occluded penalty,
/// so a control inside the dialog outranks its buried twin, and
/// [`RegionState`] reports the buried one as occluded.
///
/// Nothing blocked (the common case on dialog-free pages) borrows the input
/// instead of cloning the whole manifold.
pub fn with_front_layer(manifold: &InteractionManifold) -> Cow<'_, InteractionManifold> {
    let blocked: Vec<RegionId> = manifold
        .regions()
        .filter(|region| !region.flags().occluded() && blocker(manifold, region).is_some())
        .map(|region| region.id().clone())
        .collect();
    if blocked.is_empty() {
        return Cow::Borrowed(manifold);
    }
    let mut out = manifold.clone();
    for id in blocked {
        let region = manifold.get(&id).expect("id taken from this manifold");
        let mut parts = region.to_parts();
        parts.flags.set_occluded(true);
        let updated =
            InteractionRegion::try_new(parts).expect("rebuilding a valid region cannot fail");
        out.replace(updated);
    }
    Cow::Owned(out)
}

/// Maximum center-to-center distance (CSS px) for two root-level regions to
/// count as "nearby siblings" when they share no parent link.
const NEIGHBOR_RADIUS_PX: f64 = 160.0;

/// Target + ancestors + same-parent siblings + children + nearby root peers.
pub fn neighborhood_of(manifold: &InteractionManifold, target: &RegionId) -> BTreeSet<RegionId> {
    let mut out = BTreeSet::new();
    out.insert(target.clone());
    let Some(target_region) = manifold.get(target) else {
        return out;
    };
    // Ancestors.
    let mut cursor = target_region.parent().cloned();
    while let Some(id) = cursor {
        out.insert(id.clone());
        cursor = manifold.get(&id).and_then(|r| r.parent().cloned());
    }
    let parent = target_region.parent().cloned();
    for region in manifold.regions() {
        if region.id() == target {
            continue;
        }
        // Explicit children of the target.
        if region.parent() == Some(target) {
            out.insert(region.id().clone());
            continue;
        }
        match (&parent, region.parent()) {
            (Some(p), Some(rp)) if p == rp => {
                out.insert(region.id().clone());
            }
            (None, None) if nearby(target_region, region) => {
                // Flat page: only geometrically nearby peers matter.
                out.insert(region.id().clone());
            }
            // Different parents, or one side parented and the other root:
            // not a neighbor.
            _ => {}
        }
    }
    out
}

fn nearby(target: &InteractionRegion, other: &InteractionRegion) -> bool {
    let tc = target.rect().center();
    let oc = other.rect().center();
    tc.distance(oc) <= NEIGHBOR_RADIUS_PX
}

fn dialogs(manifold: &InteractionManifold) -> impl Iterator<Item = &InteractionRegion> {
    manifold.regions().filter(move |region| {
        region.role() == Role::Dialog
            && !region.rect().is_zero_area()
            && RegionState::of(manifold.viewport(), region).visibility() != Visibility::Hidden
            && RegionState::of(manifold.viewport(), region).visibility() != Visibility::Offscreen
    })
}

fn blocks(
    manifold: &InteractionManifold,
    dialog: &InteractionRegion,
    region: &InteractionRegion,
) -> bool {
    if inside(manifold, dialog, region) {
        return false;
    }
    if dialog.flags().modal() {
        return true;
    }
    contains_point(dialog.rect(), region.rect())
}

fn inside(
    manifold: &InteractionManifold,
    dialog: &InteractionRegion,
    region: &InteractionRegion,
) -> bool {
    if region.id() == dialog.id() || manifold.is_within(region.id(), dialog.id()) {
        return true;
    }
    let authoritative =
        dialog.sources().contains(SourceMask::DOM) && region.sources().contains(SourceMask::DOM);
    !authoritative && contains_rect(dialog.rect(), region.rect())
}

fn contains_point(outer: Rect, inner: Rect) -> bool {
    let center = inner.center();
    center.x() >= outer.x()
        && center.x() <= outer.right()
        && center.y() >= outer.y()
        && center.y() <= outer.bottom()
}

fn contains_rect(outer: Rect, inner: Rect) -> bool {
    inner.x() >= outer.x()
        && inner.y() >= outer.y()
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{Action, RegionFlags, RegionParts, UnitInterval};

    #[allow(clippy::too_many_arguments)]
    fn region(
        id: &str,
        role: Role,
        label: &str,
        rect: (f64, f64, f64, f64),
        parent: Option<&str>,
        sources: SourceMask,
        flags: &str,
    ) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role,
            label: label.into(),
            rect: Rect::try_new(rect.0, rect.1, rect.2, rect.3).unwrap(),
            actions: vec![Action::Click],
            parent: parent.map(|p| RegionId::try_new(p).unwrap()),
            sources,
            flags: RegionFlags::parse_list(flags).unwrap(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn page(regions: Vec<InteractionRegion>) -> InteractionManifold {
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 1000.0, 800.0).unwrap(),
            regions,
            0,
        )
        .unwrap()
    }

    const DOM: SourceMask = SourceMask::DOM;
    const AX: SourceMask = SourceMask::ACCESSIBILITY;

    #[test]
    fn a_modal_blocks_everything_outside_it_and_nothing_inside() {
        let m = page(vec![
            region(
                "bg",
                Role::Button,
                "Delete",
                (20.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            region(
                "dlg",
                Role::Dialog,
                "Confirm",
                (300.0, 200.0, 400.0, 300.0),
                None,
                DOM,
                "modal",
            ),
            region(
                "ok",
                Role::Button,
                "Delete",
                (320.0, 440.0, 80.0, 30.0),
                Some("dlg"),
                DOM,
                "",
            ),
        ]);
        assert_eq!(
            blocker(&m, m.get_str("bg").unwrap()).unwrap().id().as_str(),
            "dlg"
        );
        assert!(blocker(&m, m.get_str("ok").unwrap()).is_none());
        assert!(blocker(&m, m.get_str("dlg").unwrap()).is_none());
        let effective = with_front_layer(&m);
        assert!(effective.get_str("bg").unwrap().flags().occluded());
        assert!(!effective.get_str("ok").unwrap().flags().occluded());
        assert_eq!(
            FrontLayer::of(&m).entries(),
            [LayerEntry {
                id: RegionId::try_new("dlg").unwrap(),
                modal: true
            }]
        );
    }

    #[test]
    fn a_dom_button_under_a_dom_dialog_box_is_blocked_even_when_geometrically_inside() {
        // The buried button's box is fully inside the dialog's box, but the
        // DOM parent chain says it is page content, not dialog content.
        let m = page(vec![
            region(
                "dlg",
                Role::Dialog,
                "Confirm",
                (300.0, 200.0, 400.0, 300.0),
                None,
                DOM,
                "",
            ),
            region(
                "under",
                Role::Button,
                "Save",
                (350.0, 250.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
        ]);
        assert!(blocker(&m, m.get_str("under").unwrap()).is_some());
    }

    #[test]
    fn a_non_modal_dialog_blocks_by_box_and_ax_only_dialogs_fall_back_to_containment() {
        let m = page(vec![
            region(
                "dlg",
                Role::Dialog,
                "New Message",
                (600.0, 300.0, 380.0, 480.0),
                None,
                AX,
                "",
            ),
            region(
                "covered",
                Role::Button,
                "Archive",
                (700.0, 400.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            region(
                "clear",
                Role::Button,
                "Reply",
                (20.0, 400.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            // Box fully inside an accessibility-only dialog: treated as its content.
            region(
                "send",
                Role::Button,
                "Send",
                (620.0, 740.0, 80.0, 30.0),
                None,
                AX,
                "",
            ),
        ]);
        // `covered` is DOM but the dialog is AX-only, so the fallback applies:
        // its box is inside the dialog box, so it counts as dialog content.
        assert!(blocker(&m, m.get_str("covered").unwrap()).is_none());
        assert!(blocker(&m, m.get_str("clear").unwrap()).is_none());
        assert!(blocker(&m, m.get_str("send").unwrap()).is_none());
        // A partly covered region whose center is under the dialog is blocked.
        let m = page(vec![
            region(
                "dlg",
                Role::Dialog,
                "New Message",
                (600.0, 300.0, 380.0, 480.0),
                None,
                AX,
                "",
            ),
            region(
                "wide",
                Role::Button,
                "Archive",
                (560.0, 400.0, 120.0, 30.0),
                None,
                DOM,
                "",
            ),
        ]);
        assert!(blocker(&m, m.get_str("wide").unwrap()).is_some());
    }

    #[test]
    fn hidden_dialogs_are_not_a_front_layer() {
        let m = page(vec![
            region(
                "dlg",
                Role::Dialog,
                "Confirm",
                (300.0, 200.0, 400.0, 300.0),
                None,
                DOM,
                "modal,hidden",
            ),
            region(
                "bg",
                Role::Button,
                "Delete",
                (20.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
        ]);
        assert!(FrontLayer::of(&m).is_empty());
        assert!(blocker(&m, m.get_str("bg").unwrap()).is_none());
    }

    #[test]
    fn target_scoped_world_ignores_unrelated_banner() {
        let before = page(vec![
            region(
                "go",
                Role::Button,
                "Go",
                (20.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            region(
                "search",
                Role::TextField,
                "Search",
                (20.0, 60.0, 200.0, 30.0),
                None,
                DOM,
                "",
            ),
        ]);
        let ticket_world =
            WorldSnapshot::of_target(&before, None, &RegionId::try_new("go").unwrap());
        let with_banner = page(vec![
            region(
                "go",
                Role::Button,
                "Go",
                (20.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            region(
                "search",
                Role::TextField,
                "Search",
                (20.0, 60.0, 200.0, 30.0),
                None,
                DOM,
                "",
            ),
            // Far away cookie banner — outside neighborhood radius.
            region(
                "cookie",
                Role::Button,
                "Accept cookies",
                (900.0, 700.0, 120.0, 40.0),
                None,
                DOM,
                "",
            ),
        ]);
        let after_world =
            WorldSnapshot::of_target(&with_banner, None, &RegionId::try_new("go").unwrap());
        assert_eq!(ticket_world, after_world);

        // Whole-page snapshot still sees the banner.
        assert_ne!(
            WorldSnapshot::of(&before, None),
            WorldSnapshot::of(&with_banner, None)
        );
    }

    #[test]
    fn target_scoped_world_sees_target_swap_and_modal() {
        let before = page(vec![region(
            "go",
            Role::Button,
            "Go",
            (20.0, 20.0, 80.0, 30.0),
            None,
            DOM,
            "",
        )]);
        let world = WorldSnapshot::of_target(&before, None, &RegionId::try_new("go").unwrap());

        // Nearby twin replaces the action space around the target.
        let swapped = page(vec![
            region(
                "go",
                Role::Button,
                "Go",
                (20.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            region(
                "go2",
                Role::Button,
                "Go now",
                (110.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
        ]);
        assert_ne!(
            world,
            WorldSnapshot::of_target(&swapped, None, &RegionId::try_new("go").unwrap())
        );

        // Modal anywhere is global front-layer.
        let modal = page(vec![
            region(
                "go",
                Role::Button,
                "Go",
                (20.0, 20.0, 80.0, 30.0),
                None,
                DOM,
                "",
            ),
            region(
                "dlg",
                Role::Dialog,
                "Confirm",
                (300.0, 200.0, 400.0, 300.0),
                None,
                DOM,
                "modal",
            ),
        ]);
        assert_ne!(
            world,
            WorldSnapshot::of_target(&modal, None, &RegionId::try_new("go").unwrap())
        );
    }
}
