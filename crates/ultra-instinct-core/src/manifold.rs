use std::collections::BTreeMap;

use crate::error::CoreError;
use crate::id::RegionId;
use crate::rect::Rect;
use crate::region::InteractionRegion;

/// A static snapshot of interactive regions in one viewport.
///
/// Regions are stored in identifier order. Iteration order is part of the
/// locate contract: the same regions always bind neighborhood vectors in the
/// same sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct InteractionManifold {
    viewport: Rect,
    regions: BTreeMap<RegionId, InteractionRegion>,
    captured_at_ms: u64,
}

impl InteractionManifold {
    pub fn try_new(
        viewport: Rect,
        regions: Vec<InteractionRegion>,
        captured_at_ms: u64,
    ) -> Result<Self, CoreError> {
        if viewport.width() <= 0.0 || viewport.height() <= 0.0 {
            return Err(CoreError::NonPositiveViewport);
        }
        let mut map = BTreeMap::new();
        for region in regions {
            let id = region.id().clone();
            if map.insert(id.clone(), region).is_some() {
                return Err(CoreError::DuplicateRegion(id.to_string()));
            }
        }
        Ok(Self {
            viewport,
            regions: map,
            captured_at_ms,
        })
    }

    pub fn viewport(&self) -> Rect {
        self.viewport
    }

    pub fn captured_at_ms(&self) -> u64 {
        self.captured_at_ms
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn get(&self, id: &RegionId) -> Option<&InteractionRegion> {
        self.regions.get(id)
    }

    pub fn get_str(&self, id: &str) -> Option<&InteractionRegion> {
        let id = RegionId::try_new(id).ok()?;
        self.regions.get(&id)
    }

    pub fn regions(&self) -> impl Iterator<Item = &InteractionRegion> {
        self.regions.values()
    }

    pub fn ids(&self) -> impl Iterator<Item = &RegionId> {
        self.regions.keys()
    }

    pub fn insert(&mut self, region: InteractionRegion) -> Result<(), CoreError> {
        let id = region.id().clone();
        if self.regions.contains_key(&id) {
            return Err(CoreError::DuplicateRegion(id.to_string()));
        }
        self.regions.insert(id, region);
        Ok(())
    }

    pub fn remove(&mut self, id: &RegionId) -> Option<InteractionRegion> {
        self.regions.remove(id)
    }

    /// Replace an existing id, or insert it if absent.
    pub fn replace(&mut self, region: InteractionRegion) {
        self.regions.insert(region.id().clone(), region);
    }

    /// Parent chain of `id`, nearest first, not including `id`.
    ///
    /// A parent id that is not a region in this manifold is yielded and ends
    /// the chain, because its own parent is unknown. A cycle (only possible in
    /// a hand-written fixture) ends the chain at the first repeat. An unknown
    /// `id` has no ancestors.
    pub fn ancestors(&self, id: &RegionId) -> Vec<&RegionId> {
        let mut out: Vec<&RegionId> = Vec::new();
        let Some(mut current) = self.regions.get(id) else {
            return out;
        };
        while let Some(parent) = current.parent() {
            if parent == id || out.contains(&parent) {
                break;
            }
            out.push(parent);
            match self.regions.get(parent) {
                Some(next) => current = next,
                None => break,
            }
        }
        out
    }

    /// `id` is a strict descendant of `container` through parent links.
    pub fn is_within(&self, id: &RegionId, container: &RegionId) -> bool {
        self.ancestors(id)
            .into_iter()
            .any(|ancestor| ancestor == container)
    }
}

#[cfg(test)]
mod tests {
    use crate::{parse_fixture, write_fixture, RegionId, Role};

    const ROWS: &str = "viewport w=800 h=600
region id=table role=generic label=\"Servers\" x=0 y=0 w=800 h=300
region id=row role=generic label=\"alpha\" x=0 y=0 w=800 h=40 parent=table
region id=go role=button label=\"Suspend\" x=700 y=4 w=80 h=30 actions=click parent=row
region id=orphan role=button label=\"Help\" x=0 y=500 w=80 h=30 parent=missing
region id=loop-a role=button label=\"A\" x=0 y=400 w=80 h=30 parent=loop-b
region id=loop-b role=button label=\"B\" x=100 y=400 w=80 h=30 parent=loop-a
region id=dlg role=dialog label=\"Confirm\" x=200 y=200 w=300 h=200 flags=modal
";

    fn id(raw: &str) -> RegionId {
        RegionId::try_new(raw).unwrap()
    }

    #[test]
    fn ancestors_walk_the_parent_chain_nearest_first() {
        let m = parse_fixture(ROWS).unwrap();
        let chain: Vec<&str> = m.ancestors(&id("go")).iter().map(|a| a.as_str()).collect();
        assert_eq!(chain, ["row", "table"]);
        assert!(m.is_within(&id("go"), &id("table")));
        assert!(!m.is_within(&id("row"), &id("row")), "strict");
        assert!(!m.is_within(&id("table"), &id("go")));
        // A missing parent is yielded and ends the chain.
        let chain: Vec<&str> = m
            .ancestors(&id("orphan"))
            .iter()
            .map(|a| a.as_str())
            .collect();
        assert_eq!(chain, ["missing"]);
        // A cycle ends at the first repeat.
        let chain: Vec<&str> = m
            .ancestors(&id("loop-a"))
            .iter()
            .map(|a| a.as_str())
            .collect();
        assert_eq!(chain, ["loop-b"]);
        assert!(m.ancestors(&id("nope")).is_empty());
    }

    #[test]
    fn dialog_role_and_modal_flag_round_trip_through_the_fixture_format() {
        let m = parse_fixture(ROWS).unwrap();
        let dlg = m.get(&id("dlg")).unwrap();
        assert_eq!(dlg.role(), Role::Dialog);
        assert!(dlg.flags().modal());
        assert_eq!(Role::parse("alertdialog"), Some(Role::Dialog));
        let again = parse_fixture(&write_fixture(&m).unwrap()).unwrap();
        assert_eq!(again, m);
    }
}
