//! Two locate matchers behind one trait.
//!
//! [`WeightedMatcher`] is the product default. [`HgraMatcher`] is the
//! hyperdimensional ranker (`locate_with`). They are not two copies of one
//! score: the weighted matcher never calls the encoder. There is no benchmark
//! that picks a winner.

use ultra_instinct_core::{
    token_precision, token_recall, InteractionManifold, InteractionRegion, LocateQuery, RegionId,
};
#[cfg(feature = "hgra")]
use ultra_instinct_hyper::{Dims, Encoder};

#[cfg(feature = "hgra")]
use crate::locate_with;
use crate::{
    actionability_score, cap_text_miss, geometric_score, penalty_total, ContextScope,
    ResonanceError, ResonanceModel,
};

/// One ranked region. `rank` is 1-based. `confidence` is that matcher's total,
/// not a probability and not comparable across matchers.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    rank: usize,
    id: RegionId,
    confidence: f64,
}

impl Match {
    pub fn rank(&self) -> usize {
        self.rank
    }

    pub fn id(&self) -> &RegionId {
        &self.id
    }

    pub fn confidence(&self) -> f64 {
        self.confidence
    }
}

/// Rank every region. Implementations must be pure: same query and manifold,
/// same order, including the region-id tie-break.
pub trait RegionMatcher {
    fn rank(
        &self,
        query: &LocateQuery,
        manifold: &InteractionManifold,
    ) -> Result<Vec<Match>, ResonanceError>;
}

/// Positive weights for [`WeightedMatcher`]. Basis points must sum to 100.
/// Penalties are not stored here; the matcher subtracts [`ResonanceModel::V1`]'s
/// penalty table and ignores that model's positive weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightedModel {
    semantic_bp: u16,
    geometric_bp: u16,
    actionability_bp: u16,
}

/// Unvalidated weighted parts. [`WeightedBasisPoints::try_model`] is the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightedBasisPoints {
    pub semantic: u16,
    pub geometric: u16,
    pub actionability: u16,
}

impl WeightedBasisPoints {
    pub const SUM_BP: u32 = 100;

    pub fn sum_bp(self) -> u32 {
        u32::from(self.semantic) + u32::from(self.geometric) + u32::from(self.actionability)
    }

    pub fn try_model(self) -> Result<WeightedModel, ResonanceError> {
        let sum = self.sum_bp();
        if sum != Self::SUM_BP {
            return Err(ResonanceError::WeightsDoNotSum { sum });
        }
        Ok(WeightedModel {
            semantic_bp: self.semantic,
            geometric_bp: self.geometric,
            actionability_bp: self.actionability,
        })
    }
}

impl WeightedModel {
    /// Semantic 0.50, geometric 0.30, actionability 0.20. No hypervector term.
    pub const V1: Self = Self {
        semantic_bp: 50,
        geometric_bp: 30,
        actionability_bp: 20,
    };

    pub const fn semantic(self) -> f64 {
        self.semantic_bp as f64 / 100.0
    }

    pub const fn geometric(self) -> f64 {
        self.geometric_bp as f64 / 100.0
    }

    pub const fn actionability(self) -> f64 {
        self.actionability_bp as f64 / 100.0
    }

    pub const fn basis_point_sum(self) -> u16 {
        self.semantic_bp + self.geometric_bp + self.actionability_bp
    }
}

const _: () = assert!(WeightedModel::V1.basis_point_sum() == 100);

/// Deterministic baseline. Semantic text, role, and world context
/// ([`ContextScope`]: `within` and `near`) combine by minimum, so a role hit
/// cannot hide a text miss and a text hit cannot hide a context miss. The text term is
/// `recall * (0.5 + 0.5 * precision)`: every query token must be in the label
/// for full credit, and label tokens the query did not ask for cost up to half
/// of it, so "Send" outranks "Send feedback". An absent constraint scores `1` (it was
/// not asked). Penalties match [`ResonanceModel::V1`] and are subtracted after.
/// A text miss is then clamped to [`crate::TEXT_MISS_CAP`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightedMatcher {
    model: WeightedModel,
}

impl WeightedMatcher {
    pub const fn new(model: WeightedModel) -> Self {
        Self { model }
    }
}

impl Default for WeightedMatcher {
    fn default() -> Self {
        Self {
            model: WeightedModel::V1,
        }
    }
}

impl RegionMatcher for WeightedMatcher {
    fn rank(
        &self,
        query: &LocateQuery,
        manifold: &InteractionManifold,
    ) -> Result<Vec<Match>, ResonanceError> {
        let _span = tracing::debug_span!(
            "locate",
            matcher = "weighted",
            query_text = ?query.text_ref(),
            query_role = ?query.role_ref().map(|role| role.as_str()),
            near = ?query.near_ref().map(|id| id.as_str()),
            within = ?query.within_ref().map(|id| id.as_str()),
        )
        .entered();
        let scope = ContextScope::resolve(query, manifold);
        let mut ranked = Vec::with_capacity(manifold.len());
        for region in manifold.regions() {
            let parts = weighted_parts(self.model, manifold, region, query, &scope);
            debug_assert!(parts.total.is_finite());
            tracing::debug!(
                target: "ultra_instinct_resonance::rank",
                id = %region.id(),
                label = %region.label(),
                role = %region.role().as_str(),
                semantic = parts.semantic,
                geometric = parts.geometric,
                actionability = parts.actionability,
                penalty = parts.penalty,
                total = parts.total,
                "candidate"
            );
            ranked.push(Match {
                rank: 0,
                id: region.id().clone(),
                confidence: parts.total,
            });
        }
        sort_matches(&mut ranked);
        log_top("weighted", &ranked);
        Ok(ranked)
    }
}

#[cfg(feature = "hgra")]
/// The hyperdimensional ranker. `locate` / `locate_with` remain the
/// implementation; this type only adapts them to [`RegionMatcher`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HgraMatcher {
    dims: Dims,
    model: ResonanceModel,
}

#[cfg(feature = "hgra")]
impl HgraMatcher {
    pub const fn new(dims: Dims, model: ResonanceModel) -> Self {
        Self { dims, model }
    }
}

#[cfg(feature = "hgra")]
impl Default for HgraMatcher {
    fn default() -> Self {
        Self {
            dims: Dims::DEFAULT,
            model: ResonanceModel::V1,
        }
    }
}

#[cfg(feature = "hgra")]
impl RegionMatcher for HgraMatcher {
    fn rank(
        &self,
        query: &LocateQuery,
        manifold: &InteractionManifold,
    ) -> Result<Vec<Match>, ResonanceError> {
        let ranked = locate_with(manifold, query, &Encoder::new(self.dims), self.model)?;
        let mut out = Vec::with_capacity(ranked.len());
        for candidate in ranked {
            out.push(Match {
                rank: candidate.rank(),
                id: candidate.id().clone(),
                confidence: candidate.score().total(),
            });
        }
        Ok(out)
    }
}

/// Library default. The product matcher is weighted; HGRA is feature-gated.
pub fn default_matcher() -> WeightedMatcher {
    WeightedMatcher::default()
}

struct WeightedParts {
    semantic: f64,
    geometric: f64,
    actionability: f64,
    penalty: f64,
    total: f64,
}

fn weighted_parts(
    model: WeightedModel,
    manifold: &InteractionManifold,
    region: &InteractionRegion,
    query: &LocateQuery,
    scope: &ContextScope,
) -> WeightedParts {
    // Context (within / near) is a constraint like text and role: minimum.
    let semantic = weighted_semantic(query, region).min(scope.score(manifold, region));
    let geometric = geometric_score(manifold.viewport(), region.rect(), query);
    let actionability = actionability_score(region, query);
    let penalty = penalty_total(manifold.viewport(), region, ResonanceModel::V1);
    let total = model.semantic() * semantic
        + model.geometric() * geometric
        + model.actionability() * actionability
        - penalty;
    WeightedParts {
        semantic,
        geometric,
        actionability,
        penalty,
        total: cap_text_miss(query, region.label(), total),
    }
}
fn log_top(matcher: &str, ranked: &[Match]) {
    let top = ranked.first();
    let runner = ranked.get(1);
    let margin = match (top, runner) {
        (Some(top), Some(runner)) => Some(top.confidence() - runner.confidence()),
        _ => None,
    };
    tracing::debug!(
        target: "ultra_instinct_resonance::rank",
        matcher,
        top_id = top.map(|m| m.id().as_str()),
        top_total = top.map(|m| m.confidence()),
        runner_id = runner.map(|m| m.id().as_str()),
        runner_total = runner.map(|m| m.confidence()),
        margin,
        "rank_top"
    );
}

/// Minimum of the constraints that were actually set. Absent text and role
/// score `1`. A text miss is `0` even when the role matches.
///
/// Shared with HGRA (`semantic_score` in the crate root), so both matchers
/// score text precision and role the same way.
/// Label/role agreement in \[0, 1\] for a query against one region.
///
/// Shared by Weighted and HGRA. The guard uses this to refuse when a buried
/// control matches the query better than the ranked top (so an occlusion
/// penalty cannot quietly reroute a click onto a weaker dialog label).
pub fn weighted_semantic(query: &LocateQuery, region: &InteractionRegion) -> f64 {
    let mut present = Vec::new();
    if let Some(text) = query.text_ref() {
        present.push(weighted_text(text, region.label()));
    }
    if let Some(role) = query.role_ref() {
        present.push(if region.role() == role { 1.0 } else { 0.0 });
    }
    present.into_iter().fold(1.0, f64::min)
}

fn weighted_text(text: &str, label: &str) -> f64 {
    token_recall(text, label) * (0.5 + 0.5 * token_precision(text, label))
}

fn sort_matches(ranked: &mut [Match]) {
    ranked.sort_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then_with(|| left.id.cmp(&right.id))
    });
    for (index, candidate) in ranked.iter_mut().enumerate() {
        candidate.rank = index + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        Action, InteractionRegion, Rect, RegionFlags, RegionId, RegionParts, Role, SourceMask,
        UnitInterval,
    };

    fn button(id: &str, label: &str, flags: RegionFlags) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.into(),
            rect: Rect::try_new(16.0, 40.0, 80.0, 20.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM,
            flags,
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    #[test]
    fn weighted_weights_reject_a_bad_sum_and_disabled_drops_by_the_penalty() {
        let err = WeightedBasisPoints {
            semantic: 40,
            geometric: 30,
            actionability: 20,
        }
        .try_model()
        .unwrap_err();
        assert_eq!(err, ResonanceError::WeightsDoNotSum { sum: 90 });
        assert_eq!(
            err.to_string(),
            "positive weights sum to 90 basis points, expected 100"
        );
        assert_eq!(WeightedModel::V1.basis_point_sum(), 100);

        let viewport = Rect::try_viewport(0.0, 0.0, 1440.0, 900.0).unwrap();
        let clean = InteractionManifold::try_new(
            viewport,
            vec![button("only", "Settings", RegionFlags::none())],
            0,
        )
        .unwrap();
        let mut flags = RegionFlags::none();
        flags.set_disabled(true);
        let disabled =
            InteractionManifold::try_new(viewport, vec![button("only", "Settings", flags)], 0)
                .unwrap();
        let query = LocateQuery::new()
            .text("Settings")
            .unwrap()
            .role(Role::Button);
        let matcher = WeightedMatcher::default();
        let clean_score = matcher.rank(&query, &clean).unwrap()[0].confidence();
        let disabled_score = matcher.rank(&query, &disabled).unwrap()[0].confidence();
        assert!((clean_score - 1.0).abs() < 1e-12, "{clean_score}");
        assert!(
            (clean_score - disabled_score - ResonanceModel::V1.penalty_disabled()).abs() < 1e-12,
            "{clean_score} {disabled_score}"
        );
    }

    #[test]
    fn exact_label_outranks_superset_labels_with_lower_ids() {
        let manifold = ultra_instinct_core::parse_fixture(include_str!(
            "../../../fixtures/send-buttons.manifold"
        ))
        .unwrap();
        let query = LocateQuery::new().text("Send").unwrap().role(Role::Button);
        let ranked = WeightedMatcher::default().rank(&query, &manifold).unwrap();
        let ids: Vec<_> = ranked.iter().map(|m| m.id().as_str()).collect();
        assert_eq!(ids, ["z-send", "a-feedback", "b-device"]);
        assert!((ranked[0].confidence() - 1.0).abs() < 1e-12);
        assert!((ranked[1].confidence() - 0.875).abs() < 1e-12);
        assert!((ranked[2].confidence() - 0.833_333_333_333_333_4).abs() < 1e-12);
    }

    #[test]
    fn text_miss_is_not_rescued_by_a_role_hit() {
        let viewport = Rect::try_viewport(0.0, 0.0, 200.0, 200.0).unwrap();
        let manifold = InteractionManifold::try_new(
            viewport,
            vec![button("help", "Help", RegionFlags::none())],
            0,
        )
        .unwrap();
        let query = LocateQuery::new()
            .text("Settings")
            .unwrap()
            .role(Role::Button);
        let score = WeightedMatcher::default().rank(&query, &manifold).unwrap()[0].confidence();
        // semantic min(0, 1) = 0, geometric unconstrained 1, actionability 1:
        // 0.50 before the cap, then clamped to TEXT_MISS_CAP.
        let uncapped = WeightedModel::V1.geometric() + WeightedModel::V1.actionability();
        assert!((uncapped - 0.5).abs() < 1e-12);
        assert_eq!(score, crate::TEXT_MISS_CAP);
    }

    #[test]
    fn a_label_hit_with_the_wrong_role_outranks_a_nameless_region() {
        // Live drive t7: "Send" asked as a link. Every candidate missed, they
        // all tied at 0.50, and an unlabeled AX node won on region id.
        let viewport = Rect::try_viewport(0.0, 0.0, 400.0, 400.0).unwrap();
        let mut nameless = button("ax11", "", RegionFlags::none()).to_parts();
        nameless.role = Role::Image;
        let manifold = InteractionManifold::try_new(
            viewport,
            vec![
                InteractionRegion::try_new(nameless).unwrap(),
                button("n714", "Send", RegionFlags::none()),
            ],
            0,
        )
        .unwrap();
        let query = LocateQuery::new().text("Send").unwrap().role(Role::Link);
        let ranked = WeightedMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(ranked[0].id().as_str(), "n714");
        assert!((ranked[0].confidence() - 0.5).abs() < 1e-12);
        assert_eq!(ranked[1].id().as_str(), "ax11");
        assert_eq!(ranked[1].confidence(), crate::TEXT_MISS_CAP);
    }

    #[test]
    fn the_cap_only_lowers_and_only_on_a_text_miss() {
        let text = LocateQuery::new().text("Send").unwrap();
        assert_eq!(cap_text_miss(&text, "", 0.9), crate::TEXT_MISS_CAP);
        assert_eq!(cap_text_miss(&text, "Archive", 0.3), 0.3);
        assert_eq!(cap_text_miss(&text, "Send feedback", 0.9), 0.9);
        assert_eq!(cap_text_miss(&LocateQuery::new(), "", 0.9), 0.9);
        assert_eq!(
            cap_text_miss(&LocateQuery::new().role(Role::Button), "", 0.9),
            0.9
        );
    }

    #[test]
    fn rank_trace_emits_candidate_and_top_under_subscriber() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        let viewport = Rect::try_viewport(0.0, 0.0, 400.0, 300.0).unwrap();
        let manifold = InteractionManifold::try_new(
            viewport,
            vec![
                button("exact", "Send", RegionFlags::none()),
                button("long", "Send feedback", RegionFlags::none()),
            ],
            0,
        )
        .unwrap();
        let query = LocateQuery::new().text("Send").unwrap();
        let ranked = WeightedMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(ranked[0].id().as_str(), "exact");
    }
}
