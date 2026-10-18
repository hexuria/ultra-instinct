//! Resonance ranking for a static interaction manifold.
//!
//! The product default is [`WeightedMatcher`]: semantic match, geometry,
//! actionability, and the versioned penalties. It does not build hypervectors.
//!
//! The hyperdimensional ranker (`locate` / [`HgraMatcher`]) lives behind the
//! `hgra` feature and the algebra crate under `experiments/hgra/`. It is not
//! the product default and has not been shown to beat WeightedMatcher.
//!
//! Text-miss cap: when the query has text and a region's label shares no
//! token with it, both matchers clamp that region's total to at most
//! [`TEXT_MISS_CAP`] (0.45).

#![forbid(unsafe_code)]

mod context;
mod error;
mod matcher;
mod model;
#[cfg(feature = "hgra")]
mod signature;
mod state;

#[cfg(feature = "hgra")]
pub use matcher::HgraMatcher;
pub use matcher::{
    default_matcher, weighted_semantic, Match, RegionMatcher, WeightedBasisPoints, WeightedMatcher,
    WeightedModel,
};

use ultra_instinct_core::{token_recall, InteractionRegion, LocateQuery, Rect};
#[cfg(feature = "hgra")]
use ultra_instinct_core::{InteractionManifold, RegionId, SourceMask};
use ultra_instinct_geometry::{is_fully_offscreen, normalize, zones};
#[cfg(feature = "hgra")]
use ultra_instinct_hyper::{cosine, Encoder};

pub use context::ContextScope;
pub use error::ResonanceError;
pub use model::{PenaltyBasisPoints, ResonanceModel, WeightBasisPoints};
pub use state::{separating_zone, Availability, RegionState, Visibility};
#[cfg(feature = "hgra")]
pub use ultra_instinct_hyper::{BipolarVector, Dims};

#[cfg(all(test, feature = "hgra"))]
use signature::query_probes;
#[cfg(feature = "hgra")]
use signature::{query_vector, region_signature as compose_signature, Memory};

/// Highest total any matcher gives a region whose label shares no token with
/// a non-empty text query.
///
/// Why 0.45: it is strictly below the executor's 0.55 act gate, so a nameless
/// or unrelated region can never be clicked from a text query, whatever the
/// weights. It is also 0.05 below the 0.50 that the weighted V1 model gives a
/// label hit with the wrong role, so under V1 a right-named control of the
/// wrong kind still ranks above every text miss instead of tying with them
/// and losing on region id.
pub const TEXT_MISS_CAP: f64 = 0.45;

/// Clamp `total` to [`TEXT_MISS_CAP`] when the query has text and `label`
/// shares none of its tokens. A query without text, or text with no tokens,
/// is unchanged.
pub(crate) fn cap_text_miss(query: &LocateQuery, label: &str, total: f64) -> f64 {
    match query.text_ref() {
        Some(text) if token_recall(text, label) == 0.0 => total.min(TEXT_MISS_CAP),
        _ => total,
    }
}

/// One ranked region. `rank` is 1-based after the deterministic sort.
#[derive(Clone, Debug, PartialEq)]
#[cfg(feature = "hgra")]
pub struct RankedCandidate {
    rank: usize,
    id: RegionId,
    score: ResonanceScore,
}

#[cfg(feature = "hgra")]
impl RankedCandidate {
    #[cfg(feature = "hgra")]
    pub fn rank(&self) -> usize {
        self.rank
    }
    #[cfg(feature = "hgra")]
    pub fn id(&self) -> &RegionId {
        &self.id
    }
    #[cfg(feature = "hgra")]
    pub fn score(&self) -> &ResonanceScore {
        &self.score
    }
}

/// Breakdown of one region's resonance. `total = weighted positives - penalty`,
/// then clamped to [`TEXT_MISS_CAP`] on a text miss.
#[derive(Clone, Debug, PartialEq)]
#[cfg(feature = "hgra")]
pub struct ResonanceScore {
    hypervector: f64,
    semantic: f64,
    source_agreement: f64,
    geometric: f64,
    actionability: f64,
    temporal_stability: f64,
    contextual_consistency: f64,
    penalty: f64,
    total: f64,
}

#[cfg(feature = "hgra")]
impl ResonanceScore {
    #[cfg(feature = "hgra")]
    pub fn hypervector(&self) -> f64 {
        self.hypervector
    }
    #[cfg(feature = "hgra")]
    pub fn semantic(&self) -> f64 {
        self.semantic
    }
    #[cfg(feature = "hgra")]
    pub fn source_agreement(&self) -> f64 {
        self.source_agreement
    }
    #[cfg(feature = "hgra")]
    pub fn geometric(&self) -> f64 {
        self.geometric
    }
    #[cfg(feature = "hgra")]
    pub fn actionability(&self) -> f64 {
        self.actionability
    }
    #[cfg(feature = "hgra")]
    pub fn temporal_stability(&self) -> f64 {
        self.temporal_stability
    }
    #[cfg(feature = "hgra")]
    pub fn contextual_consistency(&self) -> f64 {
        self.contextual_consistency
    }
    #[cfg(feature = "hgra")]
    pub fn penalty(&self) -> f64 {
        self.penalty
    }
    #[cfg(feature = "hgra")]
    pub fn total(&self) -> f64 {
        self.total
    }
}

/// Locate with the default 2048-dimensional encoder and [`ResonanceModel::V1`].
#[cfg(feature = "hgra")]
pub fn locate(
    manifold: &InteractionManifold,
    query: &LocateQuery,
) -> Result<Vec<RankedCandidate>, ResonanceError> {
    locate_with(
        manifold,
        query,
        &Encoder::new(Dims::DEFAULT),
        ResonanceModel::V1,
    )
}

#[cfg(feature = "hgra")]
pub fn locate_with(
    manifold: &InteractionManifold,
    query: &LocateQuery,
    encoder: &Encoder,
    model: ResonanceModel,
) -> Result<Vec<RankedCandidate>, ResonanceError> {
    let _span = tracing::debug_span!(
        "locate",
        matcher = "hgra",
        query_text = ?query.text_ref(),
        query_role = ?query.role_ref().map(|role| role.as_str()),
        near = ?query.near_ref().map(|id| id.as_str()),
        within = ?query.within_ref().map(|id| id.as_str()),
    )
    .entered();
    let mut memory = Memory::new(encoder);
    let scope = ContextScope::resolve(query, manifold);
    let query_hv = query_vector(query, &scope, manifold, &mut memory)?;
    let mut ranked = Vec::with_capacity(manifold.len());
    for region in manifold.regions() {
        let signature = compose_signature(manifold, region, &mut memory)?;
        let hypervector = query_similarity(query_hv.as_ref(), &signature)?;
        let score = score_parts(manifold, region, query, &scope, hypervector, model);
        debug_assert!(score.total.is_finite());
        tracing::debug!(
            target: "ultra_instinct_resonance::rank",
            id = %region.id(),
            label = %region.label(),
            role = %region.role().as_str(),
            semantic = score.semantic,
            hypervector = score.hypervector,
            source_agreement = score.source_agreement,
            geometric = score.geometric,
            actionability = score.actionability,
            temporal_stability = score.temporal_stability,
            contextual_consistency = score.contextual_consistency,
            penalty = score.penalty,
            total = score.total,
            "candidate"
        );
        ranked.push(RankedCandidate {
            rank: 0,
            id: region.id().clone(),
            score,
        });
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .total
            .total_cmp(&left.score.total)
            .then_with(|| left.id.cmp(&right.id))
    });
    for (index, candidate) in ranked.iter_mut().enumerate() {
        candidate.rank = index + 1;
    }
    let top = ranked.first();
    let runner = ranked.get(1);
    let margin = match (top, runner) {
        (Some(top), Some(runner)) => Some(top.score.total - runner.score.total),
        _ => None,
    };
    tracing::debug!(
        target: "ultra_instinct_resonance::rank",
        matcher = "hgra",
        top_id = top.map(|c| c.id.as_str()),
        top_total = top.map(|c| c.score.total),
        top_semantic = top.map(|c| c.score.semantic),
        top_hypervector = top.map(|c| c.score.hypervector),
        runner_id = runner.map(|c| c.id.as_str()),
        runner_total = runner.map(|c| c.score.total),
        margin,
        "rank_top"
    );
    Ok(ranked)
}

/// Public signature so other crates can compare regions without re-ranking.
#[cfg(feature = "hgra")]
pub fn region_signature(
    manifold: &InteractionManifold,
    region: &InteractionRegion,
    encoder: &Encoder,
) -> Result<BipolarVector, ResonanceError> {
    let mut memory = Memory::new(encoder);
    compose_signature(manifold, region, &mut memory)
}

/// Hypervector term: one cosine of the bundled query against the region
/// signature. A query with no constraint scores `1` (nothing was asked).
///
/// This replaced the mean of per-probe cosines. See `HGRA_REMEASURE.md`.
#[cfg(feature = "hgra")]
fn query_similarity(
    query_hv: Option<&BipolarVector>,
    signature: &BipolarVector,
) -> Result<f64, ResonanceError> {
    match query_hv {
        None => Ok(1.0),
        Some(query_hv) => Ok(cosine(query_hv, signature)?),
    }
}

/// The retired hypervector term, kept only so tests can ablate it against
/// [`query_similarity`]: the mean of one cosine per probe.
#[cfg(all(test, feature = "hgra"))]
fn probe_mean_similarity(
    probes: &[BipolarVector],
    signature: &BipolarVector,
) -> Result<f64, ResonanceError> {
    if probes.is_empty() {
        return Ok(1.0);
    }
    let mut sum = 0.0;
    for probe in probes {
        sum += cosine(probe, signature)?;
    }
    Ok(sum / probes.len() as f64)
}

#[cfg(feature = "hgra")]
fn score_parts(
    manifold: &InteractionManifold,
    region: &InteractionRegion,
    query: &LocateQuery,
    scope: &ContextScope,
    hypervector: f64,
    model: ResonanceModel,
) -> ResonanceScore {
    // World context (within / near) folds into semantic by minimum, the same
    // way as in the weighted matcher. The hypervector also encodes the
    // resolved container as parent-channel probes (see `query_vector`).
    let semantic = semantic_score(query, region).min(scope.score(manifold, region));
    let source_agreement = f64::from(region.sources().count()) / f64::from(SourceMask::KNOWN_COUNT);
    // Exact semantic match: zone is a ranking preference via the hypervector,
    // not a veto. A wrong LocateQuery `position` must not alone pull an exact
    // label below the executor allow gate (0.55). Live-drive t3 (Add Cc +
    // position=top) scored ~0.516 and refused while still ranked #1. Weighted
    // is unchanged: it keeps raw `geometric_score` (its 0.50 semantic weight
    // already clears 0.55 at geo=0).
    let mut geometric = geometric_score(manifold.viewport(), region.rect(), query);
    if semantic == 1.0 {
        geometric = 1.0;
    }
    let actionability = actionability_score(region, query);
    let temporal_stability = region.temporal_stability().get();
    let contextual_consistency = contextual_score(manifold, region);
    let penalty = penalty_total(manifold.viewport(), region, model);
    let positive = model.hypervector() * hypervector
        + model.semantic() * semantic
        + model.source_agreement() * source_agreement
        + model.geometric() * geometric
        + model.actionability() * actionability
        + model.temporal_stability() * temporal_stability
        + model.contextual_consistency() * contextual_consistency;
    ResonanceScore {
        hypervector,
        semantic,
        source_agreement,
        geometric,
        actionability,
        temporal_stability,
        contextual_consistency,
        penalty,
        total: cap_text_miss(query, region.label(), positive - penalty),
    }
}

/// HGRA semantic term. Identical to the weighted matcher's: text is
/// `recall * (0.5 + 0.5 * precision)`, combined with the role hit by minimum,
/// and an absent constraint scores `1`. Before parity the HGRA text term was
/// recall alone, averaged with the role, so "Send" and "Send feedback" both
/// scored 1.0 for "Send".
#[cfg(feature = "hgra")]
fn semantic_score(query: &LocateQuery, region: &InteractionRegion) -> f64 {
    matcher::weighted_semantic(query, region)
}

pub(crate) fn geometric_score(viewport: Rect, rect: Rect, query: &LocateQuery) -> f64 {
    let Some(wanted) = query.position_ref() else {
        return 1.0;
    };
    match normalize(rect, viewport) {
        Ok(normalized) => {
            if zones(normalized).contains(&wanted) {
                1.0
            } else {
                0.0
            }
        }
        Err(_) => 0.0,
    }
}

pub(crate) fn actionability_score(region: &InteractionRegion, query: &LocateQuery) -> f64 {
    match query.action_ref() {
        Some(action) => {
            if region.actions().contains(&action) {
                1.0
            } else {
                0.0
            }
        }
        None => {
            if region.actions().is_empty() {
                0.0
            } else {
                1.0
            }
        }
    }
}

#[cfg(feature = "hgra")]
pub(crate) fn contextual_score(manifold: &InteractionManifold, region: &InteractionRegion) -> f64 {
    if region.flags().detached() {
        return 0.0;
    }
    match region.parent() {
        None => 1.0,
        Some(parent) => {
            if manifold.get(parent).is_some() {
                1.0
            } else {
                0.5
            }
        }
    }
}

pub(crate) fn penalty_total(
    viewport: Rect,
    region: &InteractionRegion,
    model: ResonanceModel,
) -> f64 {
    let flags = region.flags();
    let mut penalty = 0.0;
    if flags.disabled() {
        penalty += model.penalty_disabled();
    }
    if flags.hidden() {
        penalty += model.penalty_hidden();
    }
    if flags.occluded() {
        penalty += model.penalty_occluded();
    }
    if flags.offscreen() || is_fully_offscreen(region.rect(), viewport) {
        penalty += model.penalty_offscreen();
    }
    if flags.stale() {
        penalty += model.penalty_stale();
    }
    if flags.ambiguous() {
        penalty += model.penalty_ambiguous();
    }
    if flags.detached() {
        penalty += model.penalty_detached();
    }
    if region.rect().is_zero_area() {
        penalty += model.penalty_zero_size();
    }
    penalty
}

#[cfg(all(test, feature = "hgra"))]
mod tests {
    use super::*;
    use ultra_instinct_core::{
        parse_fixture, Action, InteractionRegion, Rect, RegionFlags, RegionId, RegionParts, Role,
        SourceMask, UnitInterval, Zone,
    };
    #[cfg(feature = "hgra")]
    use ultra_instinct_hyper::HyperError;

    fn sample(id: &str, label: &str, x: f64, flags: RegionFlags, width: f64) -> InteractionRegion {
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: label.into(),
            rect: Rect::try_new(x, 40.0, width, if width == 0.0 { 0.0 } else { 20.0 }).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags,
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap()
    }

    fn manifold(regions: Vec<InteractionRegion>) -> InteractionManifold {
        InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 1440.0, 900.0).unwrap(),
            regions,
            0,
        )
        .unwrap()
    }

    fn settings_query() -> LocateQuery {
        LocateQuery::new()
            .text("Settings")
            .unwrap()
            .role(Role::Button)
            .position(Zone::Left)
    }

    #[test]
    fn weights_are_versioned_and_sum_to_one() {
        let model = ResonanceModel::V1;
        assert_eq!(model.version(), 1);
        assert_eq!(model.weight_basis_point_sum(), 100);
        let rebuilt = model
            .weight_basis_points()
            .try_model(model.version(), model.penalty_basis_points())
            .unwrap();
        assert_eq!(rebuilt, model);
        let mut bad = model.weight_basis_points();
        bad.hypervector = 34;
        let err = bad.try_model(1, model.penalty_basis_points()).unwrap_err();
        assert_eq!(err, ResonanceError::WeightsDoNotSum { sum: 99 });
        assert_eq!(
            err.to_string(),
            "positive weights sum to 99 basis points, expected 100"
        );
        let overflow = WeightBasisPoints {
            hypervector: u16::MAX,
            semantic: u16::MAX,
            source_agreement: u16::MAX,
            geometric: u16::MAX,
            actionability: u16::MAX,
            temporal_stability: u16::MAX,
            contextual_consistency: u16::MAX,
        };
        let err = overflow
            .try_model(1, model.penalty_basis_points())
            .unwrap_err();
        assert_eq!(
            err,
            ResonanceError::WeightsDoNotSum {
                sum: 7 * u32::from(u16::MAX)
            }
        );
        assert_eq!(model.hypervector(), 0.35);
        assert_eq!(model.semantic(), 0.20);
        assert_eq!(model.source_agreement(), 0.15);
        assert_eq!(model.geometric(), 0.10);
        assert_eq!(model.actionability(), 0.10);
        assert_eq!(model.temporal_stability(), 0.05);
        assert_eq!(model.contextual_consistency(), 0.05);
        assert_eq!(model.penalty_disabled(), 0.25);
        assert_eq!(model.penalty_hidden(), 0.45);
        assert_eq!(model.penalty_zero_size(), 0.40);
    }

    #[test]
    fn same_manifold_and_query_rank_identically() {
        let manifold = manifold(vec![
            sample("b", "Settings", 16.0, RegionFlags::none(), 80.0),
            sample("a", "Settings", 16.0, RegionFlags::none(), 80.0),
            sample("c", "Help", 900.0, RegionFlags::none(), 80.0),
        ]);
        let query = settings_query();
        let encoder = Encoder::new(Dims::D512);
        let once = locate_with(&manifold, &query, &encoder, ResonanceModel::V1).unwrap();
        let twice = locate_with(&manifold, &query, &encoder, ResonanceModel::V1).unwrap();
        assert_eq!(once.len(), twice.len());
        for (left, right) in once.iter().zip(twice.iter()) {
            assert_eq!(left.id(), right.id());
            assert_eq!(left.rank(), right.rank());
            assert_eq!(
                left.score().total().to_bits(),
                right.score().total().to_bits()
            );
        }
        let ids: Vec<_> = once
            .iter()
            .map(|candidate| candidate.id().as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn penalty_flags_lower_the_score_by_the_versioned_amount() {
        let query = settings_query();
        let encoder = Encoder::new(Dims::D512);
        let model = ResonanceModel::V1;
        let cases: [(&str, RegionFlags, f64); 7] = [
            (
                "disabled",
                with_flag(|flags| flags.set_disabled(true)),
                model.penalty_disabled(),
            ),
            (
                "hidden",
                with_flag(|flags| flags.set_hidden(true)),
                model.penalty_hidden(),
            ),
            (
                "occluded",
                with_flag(|flags| flags.set_occluded(true)),
                model.penalty_occluded(),
            ),
            (
                "stale",
                with_flag(|flags| flags.set_stale(true)),
                model.penalty_stale(),
            ),
            (
                "ambiguous",
                with_flag(|flags| flags.set_ambiguous(true)),
                model.penalty_ambiguous(),
            ),
            (
                "offscreen",
                with_flag(|flags| flags.set_offscreen(true)),
                model.penalty_offscreen(),
            ),
            (
                "detached",
                with_flag(|flags| flags.set_detached(true)),
                model.penalty_detached() + model.contextual_consistency(),
            ),
        ];
        for (name, flags, expected_drop) in cases {
            let clean = manifold(vec![sample(
                "only",
                "Settings",
                16.0,
                RegionFlags::none(),
                80.0,
            )]);
            let penalized = manifold(vec![sample("only", "Settings", 16.0, flags, 80.0)]);
            let clean_score = locate_with(&clean, &query, &encoder, model).unwrap()[0]
                .score()
                .total();
            let penalized_candidate = &locate_with(&penalized, &query, &encoder, model).unwrap()[0];
            let drop = clean_score - penalized_candidate.score().total();
            assert!(
                (drop - expected_drop).abs() < 1e-9,
                "{name} drop {drop} expected {expected_drop}"
            );
            assert!(penalized_candidate.score().penalty() > 0.0, "{name}");
        }
    }

    fn with_flag(set: impl FnOnce(&mut RegionFlags)) -> RegionFlags {
        let mut flags = RegionFlags::none();
        set(&mut flags);
        flags
    }

    #[test]
    fn zero_size_and_geometric_offscreen_are_penalized() {
        let query = LocateQuery::new().text("Ghost").unwrap().role(Role::Button);
        let encoder = Encoder::new(Dims::D512);
        let visible = manifold(vec![sample("g", "Ghost", 16.0, RegionFlags::none(), 40.0)]);
        let zero = manifold(vec![sample("g", "Ghost", 16.0, RegionFlags::none(), 0.0)]);
        let visible_score = locate_with(&visible, &query, &encoder, ResonanceModel::V1).unwrap();
        let zero_score = locate_with(&zero, &query, &encoder, ResonanceModel::V1).unwrap();
        assert!(
            (zero_score[0].score().penalty() - ResonanceModel::V1.penalty_zero_size()).abs()
                < 1e-12
        );
        assert!(zero_score[0].score().total() < visible_score[0].score().total());

        let off = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("off").unwrap(),
            role: Role::Button,
            label: "Undo".into(),
            rect: Rect::try_new(16.0, 2000.0, 80.0, 20.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::SCREENSHOT,
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let on = sample("off", "Undo", 16.0, RegionFlags::none(), 80.0);
        let off_ranked = locate_with(
            &manifold(vec![off]),
            &LocateQuery::new().text("Undo").unwrap(),
            &encoder,
            ResonanceModel::V1,
        )
        .unwrap();
        let on_ranked = locate_with(
            &manifold(vec![on]),
            &LocateQuery::new().text("Undo").unwrap(),
            &encoder,
            ResonanceModel::V1,
        )
        .unwrap();
        assert!(off_ranked[0].score().penalty() >= ResonanceModel::V1.penalty_offscreen() - 1e-12);
        assert!(off_ranked[0].score().total() < on_ranked[0].score().total());
    }

    #[test]
    fn sidebar_settings_ranks_first_and_is_deterministic() {
        let manifold = parse_fixture(include_str!("../../../fixtures/sidebar.manifold")).unwrap();
        let query = settings_query();
        let once = locate(&manifold, &query).unwrap();
        let twice = locate(&manifold, &query).unwrap();
        assert_eq!(once[0].id().as_str(), "nav-settings");
        assert_eq!(once[0].rank(), 1);
        assert!(once[0].score().geometric() == 1.0);
        assert!(once[0].score().semantic() == 1.0);
        let main = once
            .iter()
            .find(|c| c.id().as_str() == "main-settings")
            .unwrap();
        assert!(once[0].score().total() > main.score().total());
        assert!(main.rank() > 1);
        let ids_once: Vec<_> = once.iter().map(|c| c.id().as_str()).collect();
        let ids_twice: Vec<_> = twice.iter().map(|c| c.id().as_str()).collect();
        assert_eq!(ids_once, ids_twice);
        for (left, right) in once.iter().zip(twice.iter()) {
            assert_eq!(
                left.score().total().to_bits(),
                right.score().total().to_bits()
            );
        }
        assert_eq!(once.len(), manifold.len());
        let help = relations_help_above(&manifold);
        assert!(help);
    }

    fn relations_help_above(manifold: &InteractionManifold) -> bool {
        let help = manifold.get_str("nav-help").unwrap().rect();
        let settings = manifold.get_str("nav-settings").unwrap().rect();
        help.bottom() <= settings.y()
            && manifold.get_str("nav-profile").unwrap().rect().y() >= settings.bottom()
    }

    #[test]
    fn disabled_hidden_and_zero_size_rank_below_a_clean_match() {
        let manifold = parse_fixture(include_str!("../../../fixtures/sidebar.manifold")).unwrap();
        let query = LocateQuery::new().role(Role::Button).position(Zone::Left);
        let ranked = locate_with(
            &manifold,
            &query,
            &Encoder::new(Dims::D512),
            ResonanceModel::V1,
        )
        .unwrap();
        let score = |id: &str| {
            ranked
                .iter()
                .find(|candidate| candidate.id().as_str() == id)
                .unwrap()
                .score()
                .total()
        };
        assert!(score("nav-help") > score("nav-disabled"));
        assert!(score("nav-help") > score("nav-hidden"));
        assert!(score("nav-help") > score("zero-ghost"));
        assert!(ranked.iter().any(|c| c.id().as_str() == "nav-hidden"));
    }

    #[test]
    fn signature_is_deterministic_and_dim_mismatch_surfaces() {
        let manifold = manifold(vec![sample(
            "a",
            "Settings",
            16.0,
            RegionFlags::none(),
            80.0,
        )]);
        let region = manifold.get_str("a").unwrap();
        let encoder = Encoder::new(Dims::D512);
        let once = region_signature(&manifold, region, &encoder).unwrap();
        let twice = region_signature(&manifold, region, &encoder).unwrap();
        assert_eq!(once, twice);
        let other = Encoder::new(Dims::D1024);
        let wide = region_signature(&manifold, region, &other).unwrap();
        let err = cosine(&once, &wide).unwrap_err();
        assert!(matches!(err, HyperError::DimMismatch { .. }));
    }

    #[test]
    fn hgra_exact_label_outranks_superset_labels_with_lower_ids() {
        // Semantic parity: HGRA's semantic term is the weighted one, so the
        // exact label wins on text precision and not only on the hypervector.
        let manifold =
            parse_fixture(include_str!("../../../fixtures/send-buttons.manifold")).unwrap();
        let query = LocateQuery::new().text("Send").unwrap().role(Role::Button);
        let ranked = locate(&manifold, &query).unwrap();
        let ids: Vec<_> = ranked.iter().map(|c| c.id().as_str()).collect();
        assert_eq!(ids, ["z-send", "a-feedback", "b-device"]);
        for candidate in &ranked {
            let region = manifold.get(candidate.id()).unwrap();
            assert_eq!(
                candidate.score().semantic(),
                matcher::weighted_semantic(&query, region),
                "{}",
                candidate.id().as_str()
            );
        }
        assert_eq!(ranked[0].score().semantic(), 1.0);
        assert_eq!(ranked[1].score().semantic(), 0.75);
        assert!((ranked[2].score().semantic() - 2.0 / 3.0).abs() < 1e-12);
        // Before parity and the bundled query this margin was 0.0277 and the
        // 0.05 act margin refused it. It is now above that margin.
        let margin = ranked[0].score().total() - ranked[1].score().total();
        assert!(margin > 0.05, "{margin}");
        let matched = HgraMatcher::default().rank(&query, &manifold).unwrap();
        let matched_ids: Vec<_> = matched.iter().map(|m| m.id().as_str()).collect();
        assert_eq!(matched_ids, ids);
    }

    #[test]
    fn hgra_label_hit_with_the_wrong_role_outranks_a_nameless_region() {
        // Live drive t7 under HGRA. With min(text, role) the wrong-role label
        // hit has semantic 0, the same as the nameless node; the text-miss
        // cap and the label token in the hypervector must still separate them.
        let viewport = Rect::try_viewport(0.0, 0.0, 400.0, 400.0).unwrap();
        let mut nameless = sample("ax11", "", 16.0, RegionFlags::none(), 80.0).to_parts();
        nameless.role = Role::Image;
        let manifold = InteractionManifold::try_new(
            viewport,
            vec![
                InteractionRegion::try_new(nameless).unwrap(),
                sample("n714", "Send", 16.0, RegionFlags::none(), 80.0),
            ],
            0,
        )
        .unwrap();
        let query = LocateQuery::new().text("Send").unwrap().role(Role::Link);
        let ranked = HgraMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(ranked[0].id().as_str(), "n714");
        assert!(ranked[0].confidence() > ranked[1].confidence());
        assert!(ranked[1].confidence() <= TEXT_MISS_CAP);
    }

    /// Ablation on the same semantic term: the bundled query against the
    /// retired probe mean. The bundle is one cosine and a different number;
    /// on `send-buttons.manifold` it widens the exact-vs-superset margin.
    #[test]
    fn bundled_query_is_one_cosine_and_widens_the_send_margin_over_probe_mean() {
        let manifold =
            parse_fixture(include_str!("../../../fixtures/send-buttons.manifold")).unwrap();
        let query = LocateQuery::new().text("Send").unwrap().role(Role::Button);
        let encoder = Encoder::new(Dims::DEFAULT);
        let model = ResonanceModel::V1;
        let mut memory = Memory::new(&encoder);
        let probes = query_probes(&query, &mut memory).unwrap();
        assert_eq!(probes.len(), 2);
        let scope = ContextScope::resolve(&query, &manifold);
        let bundled = query_vector(&query, &scope, &manifold, &mut memory)
            .unwrap()
            .unwrap();
        let ranked = locate_with(&manifold, &query, &encoder, model).unwrap();
        let mut bundled_total = std::collections::BTreeMap::new();
        let mut mean_total = std::collections::BTreeMap::new();
        for candidate in &ranked {
            let region = manifold.get(candidate.id()).unwrap();
            let signature = compose_signature(&manifold, region, &mut memory).unwrap();
            let hv = cosine(&bundled, &signature).unwrap();
            assert_eq!(candidate.score().hypervector(), hv);
            let mean = probe_mean_similarity(&probes, &signature).unwrap();
            let id = candidate.id().as_str();
            bundled_total.insert(
                id,
                score_parts(&manifold, region, &query, &scope, hv, model).total,
            );
            mean_total.insert(
                id,
                score_parts(&manifold, region, &query, &scope, mean, model).total,
            );
        }
        let bundled_margin = bundled_total["z-send"] - bundled_total["a-feedback"];
        let mean_margin = mean_total["z-send"] - mean_total["a-feedback"];
        assert!(
            bundled_margin > mean_margin,
            "bundled {bundled_margin} probe-mean {mean_margin}"
        );
        assert!(mean_total["z-send"] > mean_total["a-feedback"]);
    }

    #[test]
    fn hgra_query_vector_encodes_within_as_parent_channel() {
        let manifold =
            parse_fixture(include_str!("../../../fixtures/twin-suspend-rows.manifold")).unwrap();
        let query = LocateQuery::new()
            .text("Suspend")
            .unwrap()
            .role(Role::Button)
            .within(RegionId::try_new("row-alpha").unwrap());
        let encoder = Encoder::new(Dims::DEFAULT);
        let mut memory = Memory::new(&encoder);
        let scope = ContextScope::resolve(&query, &manifold);
        assert_eq!(scope.within().map(RegionId::as_str), Some("row-alpha"));
        let with_ctx = query_vector(&query, &scope, &manifold, &mut memory)
            .unwrap()
            .unwrap();
        let bare = LocateQuery::new()
            .text("Suspend")
            .unwrap()
            .role(Role::Button);
        let bare_scope = ContextScope::resolve(&bare, &manifold);
        let without = query_vector(&bare, &bare_scope, &manifold, &mut memory)
            .unwrap()
            .unwrap();
        assert_ne!(with_ctx.as_slice(), without.as_slice());

        let sig_a = compose_signature(
            &manifold,
            manifold.get_str("alpha-suspend").unwrap(),
            &mut memory,
        )
        .unwrap();
        let sig_b = compose_signature(
            &manifold,
            manifold.get_str("beta-suspend").unwrap(),
            &mut memory,
        )
        .unwrap();
        let cos_a = cosine(&with_ctx, &sig_a).unwrap();
        let cos_b = cosine(&with_ctx, &sig_b).unwrap();
        assert!(
            cos_a > cos_b,
            "within(row-alpha) should cosine-prefer alpha-suspend ({cos_a}) over beta-suspend ({cos_b})"
        );

        let ranked = HgraMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(ranked[0].id().as_str(), "alpha-suspend");
    }

    #[test]
    fn hgra_near_encodes_resolved_scope_into_the_query_vector() {
        let manifold =
            parse_fixture(include_str!("../../../fixtures/twin-suspend-rows.manifold")).unwrap();
        let query = LocateQuery::new()
            .text("Suspend")
            .unwrap()
            .role(Role::Button)
            .near(Some(RegionId::try_new("beta-host").unwrap()));
        let encoder = Encoder::new(Dims::DEFAULT);
        let mut memory = Memory::new(&encoder);
        let scope = ContextScope::resolve(&query, &manifold);
        assert_eq!(scope.near_scope().map(RegionId::as_str), Some("row-beta"));
        let hv = query_vector(&query, &scope, &manifold, &mut memory)
            .unwrap()
            .unwrap();
        let sig_b = compose_signature(
            &manifold,
            manifold.get_str("beta-suspend").unwrap(),
            &mut memory,
        )
        .unwrap();
        let sig_a = compose_signature(
            &manifold,
            manifold.get_str("alpha-suspend").unwrap(),
            &mut memory,
        )
        .unwrap();
        assert!(
            cosine(&hv, &sig_b).unwrap() > cosine(&hv, &sig_a).unwrap(),
            "near(beta-host) query should cosine-prefer beta-suspend"
        );
        let ranked = HgraMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(ranked[0].id().as_str(), "beta-suspend");
    }

    #[test]
    fn weighted_and_hgra_both_rank_sidebar_settings_first() {
        let manifold = parse_fixture(include_str!("../../../fixtures/sidebar.manifold")).unwrap();
        let query = settings_query();
        let weighted = WeightedMatcher::default().rank(&query, &manifold).unwrap();
        let hgra = HgraMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(weighted[0].id().as_str(), "nav-settings");
        assert_eq!(weighted[0].rank(), 1);
        assert_eq!(hgra[0].id().as_str(), "nav-settings");
        assert_eq!(hgra[0].rank(), 1);
        assert_eq!(
            default_matcher().rank(&query, &manifold).unwrap()[0]
                .id()
                .as_str(),
            "nav-settings"
        );
        // No benchmark compares these totals. Do not treat a higher number as a win.
        assert!(weighted[0].confidence().is_finite());
        assert!(hgra[0].confidence().is_finite());
    }

    /// Every HGRA score part pinned, so a changed weight, operator, or
    /// accessor shows up. Two regions: one with a role miss, a dangling parent,
    /// and an action miss; one on the wrong side with the asked action.
    #[test]
    fn hgra_score_parts_are_exact_and_the_total_is_their_weighted_sum() {
        let region =
            |id: &str, x: f64, actions: Vec<Action>, sources, parent: Option<&str>, stability| {
                InteractionRegion::try_new(RegionParts {
                    id: RegionId::try_new(id).unwrap(),
                    role: Role::Button,
                    label: "Settings".into(),
                    rect: Rect::try_new(x, 40.0, 80.0, 20.0).unwrap(),
                    actions,
                    parent: parent.map(|p| RegionId::try_new(p).unwrap()),
                    sources,
                    flags: RegionFlags::none(),
                    temporal_stability: UnitInterval::try_new(stability).unwrap(),
                })
                .unwrap()
            };
        let both = SourceMask::DOM.union(SourceMask::ACCESSIBILITY);
        let manifold = manifold(vec![
            region("left", 16.0, vec![Action::Click], both, Some("gone"), 0.9),
            region(
                "right",
                1300.0,
                vec![Action::Click, Action::Type],
                SourceMask::DOM,
                None,
                0.6,
            ),
        ]);
        let query = LocateQuery::new()
            .text("Settings")
            .unwrap()
            .role(Role::Link)
            .position(Zone::Left)
            .action(Action::Type);
        let encoder = Encoder::new(Dims::D512);
        let ranked = locate_with(&manifold, &query, &encoder, ResonanceModel::V1).unwrap();
        let score = |id: &str| {
            ranked
                .iter()
                .find(|candidate| candidate.id().as_str() == id)
                .unwrap()
                .score()
                .clone()
        };
        let mut memory = Memory::new(&encoder);
        let probes = query_probes(&query, &mut memory).unwrap();
        assert_eq!(probes.len(), 4);
        let scope = ContextScope::resolve(&query, &manifold);
        let bundled = query_vector(&query, &scope, &manifold, &mut memory)
            .unwrap()
            .unwrap();
        let model = ResonanceModel::V1;
        // (id, semantic, source, geometric, actionability, temporal, contextual)
        for (id, semantic, source, geometric, actionability, temporal, contextual) in [
            // Semantic is min(text, role): the Link query misses the Button
            // role, so both score 0 even though the label matches.
            ("left", 0.0, 0.5, 1.0, 0.0, 0.9, 0.5),
            ("right", 0.0, 0.25, 0.0, 1.0, 0.6, 1.0),
        ] {
            let parts = score(id);
            assert_eq!(parts.semantic(), semantic, "{id}");
            assert_eq!(parts.source_agreement(), source, "{id}");
            assert_eq!(parts.geometric(), geometric, "{id}");
            assert_eq!(parts.actionability(), actionability, "{id}");
            assert!((parts.temporal_stability() - temporal).abs() < 1e-6, "{id}");
            assert_eq!(parts.contextual_consistency(), contextual, "{id}");
            assert_eq!(parts.penalty(), 0.0, "{id}");
            let hv = parts.hypervector();
            assert!((-1.0..=1.0).contains(&hv), "{id} {hv}");
            // One probe per constraint, bundled into one query vector; the
            // hypervector term is a single cosine against the signature, not
            // the mean of the four probe cosines.
            let signature =
                compose_signature(&manifold, manifold.get_str(id).unwrap(), &mut memory).unwrap();
            assert_eq!(hv, cosine(&bundled, &signature).unwrap(), "{id}");
            let mean = probe_mean_similarity(&probes, &signature).unwrap();
            assert_ne!(hv, mean, "{id}");
            let expected = model.hypervector() * hv
                + model.semantic() * semantic
                + model.source_agreement() * source
                + model.geometric() * geometric
                + model.actionability() * actionability
                + model.temporal_stability() * parts.temporal_stability()
                + model.contextual_consistency() * contextual;
            assert!(
                (parts.total() - expected).abs() < 1e-12,
                "{id} {} {expected}",
                parts.total()
            );
        }
    }

    /// Live-drive t3: exact "Add Cc" with a wrong `position: top` while the
    /// control sits in the bottom sheet. Geometric alone used to pull HGRA
    /// under the 0.55 allow gate (~0.516) even though the region stayed #1.
    #[test]
    fn hgra_exact_label_wrong_position_stays_above_allow_gate() {
        let viewport = Rect::try_viewport(0.0, 0.0, 1280.0, 800.0).unwrap();
        let add_cc = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("add-cc").unwrap(),
            role: Role::Button,
            label: "Add Cc".into(),
            // Bottom-right compose chrome — not Top.
            rect: Rect::try_new(1190.0, 720.0, 60.0, 30.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let decoy = InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new("send").unwrap(),
            role: Role::Button,
            label: "Send".into(),
            rect: Rect::try_new(740.0, 752.0, 80.0, 36.0).unwrap(),
            actions: vec![Action::Click],
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::none(),
            temporal_stability: UnitInterval::ONE,
        })
        .unwrap();
        let manifold = InteractionManifold::try_new(viewport, vec![add_cc, decoy], 0).unwrap();
        let query = LocateQuery::new()
            .text("Add Cc")
            .unwrap()
            .role(Role::Button)
            .position(Zone::Top);
        assert_eq!(
            geometric_score(viewport, manifold.get_str("add-cc").unwrap().rect(), &query),
            0.0,
            "raw geometry must still miss Top"
        );

        let ranked = locate(&manifold, &query).unwrap();
        assert_eq!(ranked[0].id().as_str(), "add-cc");
        assert_eq!(ranked[0].score().semantic(), 1.0);
        assert_eq!(
            ranked[0].score().geometric(),
            1.0,
            "exact semantic ignores the zone miss in the geometric term"
        );
        assert!(
            ranked[0].score().total() >= 0.55,
            "HGRA total {} must clear MIN_ALLOW_CONFIDENCE",
            ranked[0].score().total()
        );

        // Weighted keeps applying raw geo=0; with V1 weights that is still 0.70.
        let weighted = WeightedMatcher::default().rank(&query, &manifold).unwrap();
        assert_eq!(weighted[0].id().as_str(), "add-cc");
        assert!(
            (weighted[0].confidence() - 0.7).abs() < 1e-12,
            "{}",
            weighted[0].confidence()
        );
    }
}
