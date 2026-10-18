//! Temporal region matching and id-based manifold diffs.
//!
//! [`diff`] is identity-based: the same [`RegionId`] with different state is
//! `changed`, a missing id is `removed`, a new id is `added`. A rename therefore
//! shows up as removed plus added.
//!
//! [`match_regions`] is the identity hypothesis layer. Identical ids pair first.
//! Leftover regions pair greedily when a caller-supplied similarity is at least
//! [`STRUCTURAL_MATCH_THRESHOLD`]. The default [`structural_similarity`] uses
//! role, label Jaccard, and center distance. It does not allocate a hypervector;
//! pass a cosine callback from the resonance crate when you want that metric.
//!
//! [`history::SnapshotRing`] keeps a bounded number of recent snapshots in
//! memory so a host can diff by snapshot id.
//!
//! This crate does not observe a live browser or the macOS accessibility tree.

#![forbid(unsafe_code)]

pub mod history;

use ultra_instinct_core::{token_jaccard, InteractionManifold, InteractionRegion, RegionId};

/// Default acceptance threshold for [`structural_similarity`].
///
/// Same role and label with a distant center score `0.8`, which is below this
/// threshold, so two "Settings" buttons on opposite sides do not merge.
pub const STRUCTURAL_MATCH_THRESHOLD: f64 = 0.85;

/// How a before-region was paired with an after-region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchKind {
    /// Both snapshots used the same [`RegionId`].
    IdenticalId,
    /// Different ids, accepted by the similarity callback.
    Similarity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedPair {
    before: RegionId,
    after: RegionId,
    kind: MatchKind,
}

impl MatchedPair {
    pub fn before(&self) -> &RegionId {
        &self.before
    }
    pub fn after(&self) -> &RegionId {
        &self.after
    }
    pub fn kind(&self) -> MatchKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemporalMatching {
    pairs: Vec<MatchedPair>,
    added: Vec<RegionId>,
    removed: Vec<RegionId>,
}

impl TemporalMatching {
    pub fn pairs(&self) -> &[MatchedPair] {
        &self.pairs
    }
    pub fn added(&self) -> &[RegionId] {
        &self.added
    }
    pub fn removed(&self) -> &[RegionId] {
        &self.removed
    }
}

/// Which observable field differed between two regions that share an id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ChangedField {
    Role,
    Label,
    Rect,
    Actions,
    Parent,
    Sources,
    Flags,
    TemporalStability,
    Fingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionChange {
    id: RegionId,
    fields: Vec<ChangedField>,
}

impl RegionChange {
    pub fn id(&self) -> &RegionId {
        &self.id
    }
    pub fn fields(&self) -> &[ChangedField] {
        &self.fields
    }
}

/// Id-based diff. Lists are sorted by region id. Field lists follow
/// [`ChangedField`]'s declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifoldDiff {
    added: Vec<RegionId>,
    removed: Vec<RegionId>,
    changed: Vec<RegionChange>,
}

impl ManifoldDiff {
    pub fn added(&self) -> &[RegionId] {
        &self.added
    }
    pub fn removed(&self) -> &[RegionId] {
        &self.removed
    }
    pub fn changed(&self) -> &[RegionChange] {
        &self.changed
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    /// Ids that survived and whose only change is the rectangle.
    /// [`ChangedField::Fingerprint`] may be present because the fingerprint
    /// hashes the rectangle. Any other field means this is not a move.
    /// Reads [`Self::changed`]. This is not a second diff.
    pub fn moved(&self) -> impl Iterator<Item = &RegionId> {
        self.changed.iter().filter_map(|change| {
            let only_rect = change.fields.contains(&ChangedField::Rect)
                && change
                    .fields
                    .iter()
                    .all(|field| matches!(field, ChangedField::Rect | ChangedField::Fingerprint));
            if only_rect {
                Some(change.id())
            } else {
                None
            }
        })
    }

    /// Ids that survived with a different label, even if other fields also changed.
    /// Reads [`Self::changed`]. This is not a second diff.
    pub fn relabeled(&self) -> impl Iterator<Item = &RegionId> {
        self.changed.iter().filter_map(|change| {
            if change.fields.contains(&ChangedField::Label) {
                Some(change.id())
            } else {
                None
            }
        })
    }
}

pub fn diff(before: &InteractionManifold, after: &InteractionManifold) -> ManifoldDiff {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for region in before.regions() {
        if after.get(region.id()).is_none() {
            removed.push(region.id().clone());
        }
    }
    for region in after.regions() {
        match before.get(region.id()) {
            None => added.push(region.id().clone()),
            Some(previous) => {
                let fields = changed_fields(previous, region);
                if !fields.is_empty() {
                    changed.push(RegionChange {
                        id: region.id().clone(),
                        fields,
                    });
                }
            }
        }
    }
    added.sort();
    removed.sort();
    changed.sort_by(|left, right| left.id.cmp(&right.id));
    ManifoldDiff {
        added,
        removed,
        changed,
    }
}

pub fn changed_fields(before: &InteractionRegion, after: &InteractionRegion) -> Vec<ChangedField> {
    let mut fields = Vec::new();
    if before.role() != after.role() {
        fields.push(ChangedField::Role);
    }
    if before.label() != after.label() {
        fields.push(ChangedField::Label);
    }
    if rect_changed(before, after) {
        fields.push(ChangedField::Rect);
    }
    if before.actions() != after.actions() {
        fields.push(ChangedField::Actions);
    }
    if before.parent() != after.parent() {
        fields.push(ChangedField::Parent);
    }
    if before.sources() != after.sources() {
        fields.push(ChangedField::Sources);
    }
    if before.flags() != after.flags() {
        fields.push(ChangedField::Flags);
    }
    if before.temporal_stability().get().to_bits() != after.temporal_stability().get().to_bits() {
        fields.push(ChangedField::TemporalStability);
    }
    if before.fingerprint() != after.fingerprint() {
        fields.push(ChangedField::Fingerprint);
    }
    fields
}

fn rect_changed(before: &InteractionRegion, after: &InteractionRegion) -> bool {
    before.rect().quantize_milli() != after.rect().quantize_milli()
}

/// Role, label, and center proximity in pixels.
///
/// `0.4 * role + 0.4 * jaccard + 0.2 * closeness`. Closeness falls to zero at
/// 400 pixels of center distance. Identical regions score `1`.
pub fn structural_similarity(before: &InteractionRegion, after: &InteractionRegion) -> f64 {
    let role = if before.role() == after.role() {
        1.0
    } else {
        0.0
    };
    let label = token_jaccard(before.label(), after.label());
    let distance = before.rect().center().distance(after.rect().center());
    let closeness = (1.0 - distance / 400.0).max(0.0);
    0.4 * role + 0.4 * label + 0.2 * closeness
}

/// Pair regions across two snapshots.
///
/// Identical ids are always paired and do not consume the similarity budget.
/// Remaining pairs are considered in descending similarity, then by before id,
/// then by after id. A pair is kept when `similarity >= threshold` and neither
/// id is already used.
pub fn match_regions(
    before: &InteractionManifold,
    after: &InteractionManifold,
    mut similarity: impl FnMut(&InteractionRegion, &InteractionRegion) -> f64,
    threshold: f64,
) -> TemporalMatching {
    let mut pairs = Vec::new();
    let mut before_open = Vec::new();
    let mut after_open = Vec::new();
    for region in before.regions() {
        if after.get(region.id()).is_some() {
            pairs.push(MatchedPair {
                before: region.id().clone(),
                after: region.id().clone(),
                kind: MatchKind::IdenticalId,
            });
        } else {
            before_open.push(region.id().clone());
        }
    }
    for region in after.regions() {
        if before.get(region.id()).is_none() {
            after_open.push(region.id().clone());
        }
    }

    let mut candidates = Vec::new();
    for before_id in &before_open {
        for after_id in &after_open {
            let left = before
                .get(before_id)
                .expect("open id is in the before snapshot");
            let right = after
                .get(after_id)
                .expect("open id is in the after snapshot");
            let score = similarity(left, right);
            candidates.push((score, before_id.clone(), after_id.clone()));
        }
    }
    candidates.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });

    let mut used_before = Vec::new();
    let mut used_after = Vec::new();
    for (score, before_id, after_id) in candidates {
        if !score.is_finite() || score < threshold {
            continue;
        }
        if used_before.contains(&before_id) || used_after.contains(&after_id) {
            continue;
        }
        used_before.push(before_id.clone());
        used_after.push(after_id.clone());
        pairs.push(MatchedPair {
            before: before_id,
            after: after_id,
            kind: MatchKind::Similarity,
        });
    }

    let removed = before_open
        .into_iter()
        .filter(|id| !used_before.contains(id))
        .collect();
    let added = after_open
        .into_iter()
        .filter(|id| !used_after.contains(id))
        .collect();
    pairs.sort_by(|left, right| {
        left.before
            .cmp(&right.before)
            .then_with(|| left.after.cmp(&right.after))
    });
    TemporalMatching {
        pairs,
        added,
        removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        Action, Rect, RegionFlags, RegionId, RegionParts, Role, SourceMask, UnitInterval,
    };

    fn button(id: &str, label: &str, x: f64, y: f64) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.into(),
            rect: Rect::try_new(x, y, 40.0, 20.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn manifold(regions: Vec<InteractionRegion>) -> InteractionManifold {
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 800.0, 600.0).unwrap(),
            regions,
            0,
        )
        .unwrap()
    }

    #[test]
    fn diff_reports_added_removed_and_changed_in_id_order() {
        let before = manifold(vec![
            button("b", "Beta", 10.0, 10.0),
            button("a", "Alpha", 10.0, 40.0),
        ]);
        let mut after_regions = vec![
            button("b", "Beta moved", 10.0, 80.0),
            button("c", "Gamma", 10.0, 120.0),
        ];
        after_regions[0] = button("b", "Beta", 12.0, 10.0);
        let after = manifold(vec![after_regions.remove(0), after_regions.remove(0)]);
        let delta = diff(&before, &after);
        assert_eq!(
            delta
                .added()
                .iter()
                .map(RegionId::as_str)
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert_eq!(
            delta
                .removed()
                .iter()
                .map(RegionId::as_str)
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(delta.changed().len(), 1);
        assert_eq!(delta.changed()[0].id().as_str(), "b");
        assert_eq!(
            delta.changed()[0].fields(),
            &[ChangedField::Rect, ChangedField::Fingerprint]
        );
        assert!(diff(&before, &before).is_empty());
    }

    #[test]
    fn identical_ids_pair_and_a_rename_matches_by_similarity() {
        let before = manifold(vec![
            button("nav-settings", "Settings", 16.0, 180.0),
            button("main-settings", "Settings", 900.0, 180.0),
        ]);
        let after = manifold(vec![
            button("nav-settings-2", "Settings", 16.0, 182.0),
            button("main-settings", "Settings", 900.0, 180.0),
        ]);
        let matching = match_regions(
            &before,
            &after,
            structural_similarity,
            STRUCTURAL_MATCH_THRESHOLD,
        );
        let renamed = matching
            .pairs()
            .iter()
            .find(|pair| pair.before().as_str() == "nav-settings")
            .unwrap();
        assert_eq!(renamed.after().as_str(), "nav-settings-2");
        assert_eq!(renamed.kind(), MatchKind::Similarity);
        assert!(matching
            .pairs()
            .iter()
            .any(|pair| pair.kind() == MatchKind::IdenticalId
                && pair.before().as_str() == "main-settings"));
        assert!(matching.added().is_empty());
        assert!(matching.removed().is_empty());

        let id_diff = diff(&before, &after);
        assert_eq!(id_diff.removed()[0].as_str(), "nav-settings");
        assert_eq!(id_diff.added()[0].as_str(), "nav-settings-2");
    }

    #[test]
    fn distant_duplicate_labels_do_not_merge() {
        let before = manifold(vec![button("only", "Settings", 16.0, 180.0)]);
        let after = manifold(vec![button("other", "Settings", 900.0, 180.0)]);
        let score = structural_similarity(
            before.get_str("only").unwrap(),
            after.get_str("other").unwrap(),
        );
        assert!(score < STRUCTURAL_MATCH_THRESHOLD, "{score}");
        let matching = match_regions(
            &before,
            &after,
            structural_similarity,
            STRUCTURAL_MATCH_THRESHOLD,
        );
        assert!(matching.pairs().is_empty());
        assert_eq!(matching.removed()[0].as_str(), "only");
        assert_eq!(matching.added()[0].as_str(), "other");
    }

    #[test]
    fn region_id_persists_across_a_move_an_enabled_change_and_a_press() {
        // Identity is the region id, not the rectangle or the enabled bit.
        // A press addresses that id; it does not mint a new one.
        let id = "sign-in";
        let before = manifold(vec![button(id, "Sign in", 16.0, 180.0)]);
        let moved = manifold(vec![button(id, "Sign in", 16.0, 240.0)]);
        let delta = diff(&before, &moved);
        assert!(delta.added().is_empty());
        assert!(delta.removed().is_empty());
        assert_eq!(delta.changed()[0].id().as_str(), id);
        assert!(delta.changed()[0].fields().contains(&ChangedField::Rect));
        let matching = match_regions(
            &before,
            &moved,
            structural_similarity,
            STRUCTURAL_MATCH_THRESHOLD,
        );
        assert_eq!(matching.pairs()[0].kind(), MatchKind::IdenticalId);
        assert_eq!(matching.pairs()[0].before().as_str(), id);
        assert_eq!(matching.pairs()[0].after().as_str(), id);

        let enabled = moved.get_str(id).unwrap().flags().disabled();
        assert!(!enabled);
        let disabled_region = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: "Sign in".into(),
            rect: Rect::try_new(16.0, 240.0, 40.0, 20.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags: {
                let mut flags = RegionFlags::none();
                flags.set_disabled(true);
                flags
            },
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let disabled = manifold(vec![disabled_region]);
        let delta = diff(&moved, &disabled);
        assert!(delta.removed().is_empty());
        assert_eq!(delta.changed()[0].id().as_str(), id);
        assert!(delta.changed()[0].fields().contains(&ChangedField::Flags));
        assert!(!delta.changed()[0].fields().contains(&ChangedField::Rect));

        // A press is an executor event aimed at the same id. The manifold
        // still contains that id afterwards; the id is not a function of the click.
        let pressed = disabled.clone();
        assert!(pressed.get_str(id).is_some());
        assert_eq!(pressed.get_str(id).unwrap().id().as_str(), id);
        assert!(pressed
            .get_str(id)
            .unwrap()
            .actions()
            .contains(&Action::Click));
        assert!(diff(&disabled, &pressed).is_empty());
    }

    #[test]
    fn flag_change_is_visible_without_a_rect_change() {
        let before = manifold(vec![button("a", "Export", 0.0, 0.0)]);
        let disabled = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("a").unwrap(),
            role: Role::Button,
            label: "Export".into(),
            rect: Rect::try_new(0.0, 0.0, 40.0, 20.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags: {
                let mut flags = RegionFlags::none();
                flags.set_disabled(true);
                flags
            },
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let after = manifold(vec![disabled]);
        let delta = diff(&before, &after);
        let fields = &delta.changed()[0];
        assert!(fields.fields().contains(&ChangedField::Flags));
        assert!(!fields.fields().contains(&ChangedField::Rect));
    }

    #[test]
    fn moved_only_rect_is_moved_not_relabeled() {
        let before = manifold(vec![
            button("n1", "Send", 0.0, 0.0),
            button("n2", "Cancel", 80.0, 0.0),
        ]);
        let after = manifold(vec![
            button("n1", "Send", 40.0, 0.0),
            button("n2", "Close", 80.0, 0.0),
        ]);
        let delta = diff(&before, &after);
        let moved: Vec<&str> = delta.moved().map(RegionId::as_str).collect();
        let relabeled: Vec<&str> = delta.relabeled().map(RegionId::as_str).collect();
        assert_eq!(moved, ["n1"]);
        assert_eq!(relabeled, ["n2"]);
        assert!(!relabeled.contains(&"n1"));
        assert!(!moved.contains(&"n2"));
    }
}
