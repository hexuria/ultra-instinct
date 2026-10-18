//! Carry region ids forward across observations of one session.
//!
//! Fusion names a region after the node it saw (`n{backendNodeId}`). A
//! framework that re-renders a control mints a new DOM node, so that name
//! changes even though the control did not. [`IdentityMap::assign`] keeps a
//! stable id instead:
//!
//! 1. A fused id seen last time keeps the stable id it had.
//! 2. Remaining regions are paired with the previous observation by
//!    [`match_regions`] with [`structural_similarity`] at
//!    [`STRUCTURAL_MATCH_THRESHOLD`]. A pair inherits the previous stable id.
//! 3. Anything left gets its fused id, or `{fused}-{k}` if that id was already
//!    used by another control in this session.
//!
//! A first observation therefore has exactly the fused ids. Stable ids are
//! opaque: after a re-render, `n100` may name a node whose backend id is 900.
//! The binding for actuation always holds the current node and backend ids.
//! This is the only identity service. `match_regions` is the only pairing.

use std::collections::{BTreeMap, BTreeSet};

use ultra_instinct_core::{InteractionManifold, InteractionRegion, RegionId};
use ultra_instinct_observe::{match_regions, structural_similarity, STRUCTURAL_MATCH_THRESHOLD};

use crate::error::BrowserError;
use crate::fusion::NodeBinding;

const PROVISIONAL_PREFIX: char = '~';

#[derive(Clone, Debug, Default)]
pub(crate) struct IdentityMap {
    /// Fused id at the last observation, to the stable id it was given.
    by_fused: BTreeMap<RegionId, RegionId>,
    /// Every stable id ever issued in this session. Never reused for another control.
    minted: BTreeSet<RegionId>,
}

impl IdentityMap {
    pub(crate) fn assign(
        &mut self,
        previous: Option<&InteractionManifold>,
        fresh: InteractionManifold,
        bindings: BTreeMap<RegionId, NodeBinding>,
    ) -> Result<(InteractionManifold, BTreeMap<RegionId, NodeBinding>), BrowserError> {
        // Step 1: known fused ids keep their stable id. Others get a
        // provisional id that cannot collide with a stable one.
        let mut stable_of: BTreeMap<RegionId, RegionId> = BTreeMap::new();
        let mut provisional = Vec::new();
        for region in fresh.regions() {
            let fused = region.id().clone();
            let id = match self.by_fused.get(&fused) {
                Some(stable) => stable.clone(),
                None => provisional_id(&fused)?,
            };
            stable_of.insert(fused, id.clone());
            provisional.push(rename(region, id, None)?);
        }
        let staged =
            InteractionManifold::try_new(fresh.viewport(), provisional, fresh.captured_at_ms())
                .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))?;

        // Step 2: pair provisional regions with the previous observation.
        if let Some(previous) = previous {
            let matching = match_regions(
                previous,
                &staged,
                structural_similarity,
                STRUCTURAL_MATCH_THRESHOLD,
            );
            let taken: BTreeSet<&RegionId> = stable_of.values().collect();
            let mut inherit = BTreeMap::new();
            for pair in matching.pairs() {
                if is_provisional(pair.after()) && !taken.contains(pair.before()) {
                    inherit.insert(pair.after().clone(), pair.before().clone());
                }
            }
            for id in stable_of.values_mut() {
                if let Some(previous_id) = inherit.get(id) {
                    *id = previous_id.clone();
                }
            }
        }

        // Step 3: mint what is left.
        for (fused, id) in stable_of.iter_mut() {
            if is_provisional(id) {
                *id = self.mint(fused)?;
            }
            self.minted.insert(id.clone());
        }

        let mut regions = Vec::with_capacity(fresh.len());
        let mut stable_bindings = BTreeMap::new();
        for region in fresh.regions() {
            let id = stable_of[region.id()].clone();
            let parent = region
                .parent()
                .map(|parent| stable_of.get(parent).cloned().unwrap_or(parent.clone()));
            regions.push(rename(region, id.clone(), Some(parent))?);
            if let Some(binding) = bindings.get(region.id()) {
                stable_bindings.insert(id, binding.clone());
            }
        }
        let manifold =
            InteractionManifold::try_new(fresh.viewport(), regions, fresh.captured_at_ms())
                .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))?;
        self.by_fused = stable_of;
        Ok((manifold, stable_bindings))
    }

    fn mint(&self, fused: &RegionId) -> Result<RegionId, BrowserError> {
        if !self.minted.contains(fused) {
            return Ok(fused.clone());
        }
        let mut k = 2u32;
        loop {
            let candidate = RegionId::try_new(format!("{fused}-{k}"))
                .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))?;
            if !self.minted.contains(&candidate) {
                return Ok(candidate);
            }
            k += 1;
        }
    }
}

fn provisional_id(fused: &RegionId) -> Result<RegionId, BrowserError> {
    RegionId::try_new(format!("{PROVISIONAL_PREFIX}{fused}"))
        .map_err(|err| BrowserError::DuplicateRegion(err.to_string()))
}

fn is_provisional(id: &RegionId) -> bool {
    id.as_str().starts_with(PROVISIONAL_PREFIX)
}

/// Rebuild `region` under `id`. `parent` of `None` keeps the original parent.
fn rename(
    region: &InteractionRegion,
    id: RegionId,
    parent: Option<Option<RegionId>>,
) -> Result<InteractionRegion, BrowserError> {
    let mut parts = region.to_parts();
    parts.id = id;
    if let Some(parent) = parent {
        parts.parent = parent;
    }
    InteractionRegion::try_new(parts).map_err(|err| BrowserError::DuplicateRegion(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ultra_instinct_core::{
        Action, Rect, RegionFlags, RegionParts, Role, SourceMask, UnitInterval,
    };

    fn button(id: &str, label: &str, x: f64) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.into(),
            rect: Rect::try_new(x, 100.0, 80.0, 30.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn snapshot(
        regions: Vec<InteractionRegion>,
    ) -> (InteractionManifold, BTreeMap<RegionId, NodeBinding>) {
        let mut bindings = BTreeMap::new();
        for region in &regions {
            let backend: i64 = region.id().as_str()[1..].parse().unwrap();
            bindings.insert(
                region.id().clone(),
                NodeBinding {
                    dom_node_id: Some(backend / 10),
                    backend_node_id: Some(backend),
                    center_x: region.rect().center().x(),
                    center_y: region.rect().center().y(),
                },
            );
        }
        let manifold = InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 1280.0, 720.0).unwrap(),
            regions,
            0,
        )
        .unwrap();
        (manifold, bindings)
    }

    fn ids(manifold: &InteractionManifold) -> Vec<String> {
        manifold.ids().map(|id| id.to_string()).collect()
    }

    #[test]
    fn first_observation_keeps_fused_ids() {
        let mut map = IdentityMap::default();
        let (fresh, bindings) = snapshot(vec![button("n100", "Sign in", 400.0)]);
        let (stable, _) = map.assign(None, fresh, bindings).unwrap();
        assert_eq!(ids(&stable), ["n100"]);
    }

    #[test]
    fn a_rerender_inherits_and_a_reused_backend_id_is_minted_fresh() {
        let mut map = IdentityMap::default();
        let (first, bindings) = snapshot(vec![button("n100", "Sign in", 400.0)]);
        let (first, _) = map.assign(None, first, bindings).unwrap();

        let (second, bindings) = snapshot(vec![button("n900", "Sign in", 402.0)]);
        let (second, second_bindings) = map.assign(Some(&first), second, bindings).unwrap();
        assert_eq!(ids(&second), ["n100"]);
        let binding = &second_bindings[&RegionId::try_new("n100").unwrap()];
        assert_eq!(binding.backend_node_id, Some(900));

        // A different control now has backend 100. n100 is taken.
        let (third, bindings) = snapshot(vec![
            button("n900", "Sign in", 402.0),
            button("n100", "Help", 1100.0),
        ]);
        let (third, _) = map.assign(Some(&second), third, bindings).unwrap();
        assert_eq!(ids(&third), ["n100", "n100-2"]);
        assert_eq!(third.get_str("n100").unwrap().label(), "Sign in");
        assert_eq!(third.get_str("n100-2").unwrap().label(), "Help");
    }

    #[test]
    fn distant_same_label_does_not_inherit() {
        let mut map = IdentityMap::default();
        let (first, bindings) = snapshot(vec![button("n100", "Settings", 16.0)]);
        let (first, _) = map.assign(None, first, bindings).unwrap();
        let (second, bindings) = snapshot(vec![button("n900", "Settings", 1100.0)]);
        let (second, _) = map.assign(Some(&first), second, bindings).unwrap();
        assert_eq!(ids(&second), ["n900"]);
    }

    #[test]
    fn a_third_control_on_a_taken_fused_id_is_minted_with_suffix_three() {
        let mut map = IdentityMap::default();
        let (first, bindings) = snapshot(vec![button("n100", "Sign in", 400.0)]);
        let (first, _) = map.assign(None, first, bindings).unwrap();
        let (rerender, bindings) = snapshot(vec![button("n900", "Sign in", 402.0)]);
        let (rerender, _) = map.assign(Some(&first), rerender, bindings).unwrap();
        let (second, bindings) = snapshot(vec![
            button("n900", "Sign in", 402.0),
            button("n100", "Help", 1100.0),
        ]);
        let (second, _) = map.assign(Some(&rerender), second, bindings).unwrap();
        assert_eq!(ids(&second), ["n100", "n100-2"]);
        // Help re-renders onto n500 and inherits n100-2 by similarity.
        let (third, bindings) = snapshot(vec![
            button("n900", "Sign in", 402.0),
            button("n500", "Help", 1100.0),
        ]);
        let (third, _) = map.assign(Some(&second), third, bindings).unwrap();
        assert_eq!(ids(&third), ["n100", "n100-2"]);
        // A new control takes backend 100 again. n100 and n100-2 are taken.
        let (fourth, bindings) = snapshot(vec![
            button("n900", "Sign in", 402.0),
            button("n500", "Help", 1100.0),
            button("n100", "Export", 700.0),
        ]);
        let (fourth, _) = map.assign(Some(&third), fourth, bindings).unwrap();
        assert_eq!(ids(&fourth), ["n100", "n100-2", "n100-3"]);
        assert_eq!(fourth.get_str("n100-3").unwrap().label(), "Export");
    }

    #[test]
    fn a_kept_stable_id_is_not_replaced_by_a_similarity_pair() {
        let mut map = IdentityMap::default();
        let (first, bindings) = snapshot(vec![button("n100", "Sign in", 400.0)]);
        map.assign(None, first, bindings).unwrap();
        // A previous observation this map did not issue: its region x1 is
        // similar to n100 but is not n100. Step 1 keeps n100; step 2 must
        // not rename it to x1.
        let (unrelated, _) = snapshot(vec![button("n1", "Sign in", 402.0)]);
        let unrelated = InteractionManifold::try_new(
            unrelated.viewport(),
            vec![rename(
                unrelated.get_str("n1").unwrap(),
                RegionId::try_new("x1").unwrap(),
                None,
            )
            .unwrap()],
            0,
        )
        .unwrap();
        let (fresh, bindings) = snapshot(vec![button("n100", "Sign in", 400.0)]);
        let (fresh, _) = map.assign(Some(&unrelated), fresh, bindings).unwrap();
        assert_eq!(ids(&fresh), ["n100"]);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn reobserving_an_identical_manifold_keeps_every_id(
            labels in proptest::collection::vec("[A-Z][a-z]{2,6}", 1..6)
        ) {
            let regions: Vec<_> = labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    button(&format!("n{}", 100 + index), label, 40.0 + 150.0 * index as f64)
                })
                .collect();
            let mut map = IdentityMap::default();
            let (fresh, bindings) = snapshot(regions.clone());
            let (first, _) = map.assign(None, fresh, bindings).unwrap();
            let (fresh, bindings) = snapshot(regions);
            let (second, _) = map.assign(Some(&first), fresh, bindings).unwrap();
            prop_assert_eq!(ids(&first), ids(&second));
            prop_assert_eq!(first, second);
        }
    }
}
