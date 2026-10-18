//! World context for locate: `within` (ancestry) and `near` (anchor scope).
//!
//! Text and role describe the target. Context describes where the target sits
//! in the current page, and it changes when the page changes. Two "Suspend"
//! buttons in two rows have the same text and role; only their parent tells
//! them apart. Both matchers fold the context term into the semantic term by
//! minimum, so a context miss cannot be rescued by a text hit.
//!
//! - `within(container)`: `1` when the region is a strict descendant of the
//!   container through parent links, else `0`.
//! - `near(anchor)`: resolve a scope once per ranking. Take the regions with
//!   the best text/role/within score (above zero). Walk the anchor and then
//!   its ancestors, nearest first. The first one that is, or contains, one of
//!   those regions is the scope. Regions inside the scope (or the scope
//!   itself) score `1`, others `0`. With no anchor, an anchor that is not in
//!   the manifold, or no ancestor that contains a best match, there is no
//!   scope and every region scores `1` (the default ranking).

use ultra_instinct_core::{InteractionManifold, InteractionRegion, LocateQuery, RegionId};

use crate::matcher::weighted_semantic;

/// Tolerance when picking the best base score. Scores are sums of a few
/// products of exact fractions, so equal labels give bit-equal scores; this
/// only guards against reordering noise.
const BEST_EPSILON: f64 = 1e-12;

/// The resolved context for one query on one manifold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextScope {
    within: Option<RegionId>,
    near_scope: Option<RegionId>,
}

impl ContextScope {
    /// Resolve `query`'s context against `manifold`. Pure.
    pub fn resolve(query: &LocateQuery, manifold: &InteractionManifold) -> Self {
        let within = query.within_ref().cloned();
        let near_scope = query
            .near_ref()
            .and_then(|anchor| resolve_near(query, manifold, within.as_ref(), anchor));
        Self { within, near_scope }
    }

    /// The container the target must be under, if the query named one.
    pub fn within(&self) -> Option<&RegionId> {
        self.within.as_ref()
    }

    /// The scope chosen from the `near` anchor. `None` means the default,
    /// unscoped ranking.
    pub fn near_scope(&self) -> Option<&RegionId> {
        self.near_scope.as_ref()
    }

    /// `1` when the region satisfies every context constraint, else `0`.
    pub fn score(&self, manifold: &InteractionManifold, region: &InteractionRegion) -> f64 {
        if !within_ok(manifold, self.within.as_ref(), region) {
            return 0.0;
        }
        if let Some(scope) = &self.near_scope {
            if !in_subtree(manifold, region.id(), scope) {
                return 0.0;
            }
        }
        1.0
    }
}

fn within_ok(
    manifold: &InteractionManifold,
    within: Option<&RegionId>,
    region: &InteractionRegion,
) -> bool {
    match within {
        None => true,
        Some(container) => manifold.is_within(region.id(), container),
    }
}

fn in_subtree(manifold: &InteractionManifold, id: &RegionId, root: &RegionId) -> bool {
    id == root || manifold.is_within(id, root)
}

fn resolve_near(
    query: &LocateQuery,
    manifold: &InteractionManifold,
    within: Option<&RegionId>,
    anchor: &RegionId,
) -> Option<RegionId> {
    manifold.get(anchor)?;
    let base = |region: &InteractionRegion| {
        if within_ok(manifold, within, region) {
            weighted_semantic(query, region)
        } else {
            0.0
        }
    };
    let best = manifold.regions().map(base).fold(0.0, f64::max);
    if best <= 0.0 {
        return None;
    }
    let matches: Vec<&RegionId> = manifold
        .regions()
        .filter(|region| base(region) >= best - BEST_EPSILON)
        .map(InteractionRegion::id)
        .collect();
    let mut chain = vec![anchor];
    chain.extend(manifold.ancestors(anchor));
    chain
        .into_iter()
        .find(|scope| matches.iter().any(|id| in_subtree(manifold, id, scope)))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        Action, Rect, RegionFlags, RegionParts, Role, SourceMask, UnitInterval,
    };

    fn region(id: &str, role: Role, label: &str, parent: Option<&str>) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role,
            label: label.into(),
            rect: Rect::try_new(10.0, 10.0, 80.0, 24.0).unwrap(),
            actions: vec![Action::Click],
            parent: parent.map(|p| RegionId::try_new(p).unwrap()),
            sources: SourceMask::DOM,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn id(raw: &str) -> RegionId {
        RegionId::try_new(raw).unwrap()
    }

    fn rows() -> InteractionManifold {
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 800.0, 600.0).unwrap(),
            vec![
                region("table", Role::Generic, "Servers", None),
                region("row-a", Role::Generic, "alpha", Some("table")),
                region("row-b", Role::Generic, "beta", Some("table")),
                region("suspend-a", Role::Button, "Suspend", Some("row-a")),
                region("suspend-b", Role::Button, "Suspend", Some("row-b")),
                region("name-b", Role::TextField, "Name", Some("row-b")),
                region("footer", Role::Button, "Help", None),
            ],
            0,
        )
        .unwrap()
    }

    fn suspend() -> LocateQuery {
        LocateQuery::new()
            .text("Suspend")
            .unwrap()
            .role(Role::Button)
    }

    #[test]
    fn near_scopes_to_the_innermost_ancestor_of_the_anchor_with_a_best_match() {
        let manifold = rows();
        let scope = ContextScope::resolve(&suspend().near(Some(id("name-b"))), &manifold);
        assert_eq!(scope.near_scope(), Some(&id("row-b")));
        let a = manifold.get(&id("suspend-a")).unwrap();
        let b = manifold.get(&id("suspend-b")).unwrap();
        assert_eq!(scope.score(&manifold, a), 0.0);
        assert_eq!(scope.score(&manifold, b), 1.0);
    }

    #[test]
    fn near_without_a_containing_ancestor_is_the_default_ranking() {
        let manifold = rows();
        // The footer is a root: no ancestor of it contains a Suspend.
        let scope = ContextScope::resolve(&suspend().near(Some(id("footer"))), &manifold);
        assert_eq!(scope.near_scope(), None);
        // An anchor that vanished from the page is also the default.
        let scope = ContextScope::resolve(&suspend().near(Some(id("gone"))), &manifold);
        assert_eq!(scope.near_scope(), None);
        // No anchor at all.
        let scope = ContextScope::resolve(&suspend(), &manifold);
        assert_eq!(
            scope,
            ContextScope {
                within: None,
                near_scope: None
            }
        );
    }

    #[test]
    fn within_is_a_strict_descendant_test() {
        let manifold = rows();
        let scope = ContextScope::resolve(&suspend().within(id("row-a")), &manifold);
        let a = manifold.get(&id("suspend-a")).unwrap();
        let b = manifold.get(&id("suspend-b")).unwrap();
        let row = manifold.get(&id("row-a")).unwrap();
        assert_eq!(scope.score(&manifold, a), 1.0);
        assert_eq!(scope.score(&manifold, b), 0.0);
        assert_eq!(
            scope.score(&manifold, row),
            0.0,
            "the container is not under itself"
        );
        // Deeper ancestors count.
        let scope = ContextScope::resolve(&suspend().within(id("table")), &manifold);
        assert_eq!(scope.score(&manifold, a), 1.0);
        assert_eq!(scope.score(&manifold, b), 1.0);
    }
}
