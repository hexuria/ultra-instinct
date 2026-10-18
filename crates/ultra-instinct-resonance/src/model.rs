/// Versioned locate weights and penalties.
///
/// Positive weights are integer basis points of the unpenalized score and must
/// sum to exactly [`WeightBasisPoints::SUM_BP`] (100). That is an epsilon of
/// zero: validation does not use `f64`. Accessors divide by 100 only after the
/// integer gate. Penalty basis points are subtracted afterwards. They are
/// independent deductions and are not required to sum to 100.
///
/// `hypervector` multiplies cosine similarity in `[-1, 1]`. The other positive
/// terms multiply a score in `[0, 1]`.
///
/// There is no `From<f32>` and no loose float table. A custom table has to pass
/// [`WeightBasisPoints::try_model`]. [`ResonanceModel::V1`] is the only
/// production table, and a const assert pins its sum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResonanceModel {
    version: u32,
    hypervector_bp: u16,
    semantic_bp: u16,
    source_agreement_bp: u16,
    geometric_bp: u16,
    actionability_bp: u16,
    temporal_stability_bp: u16,
    contextual_consistency_bp: u16,
    penalty_disabled_bp: u16,
    penalty_hidden_bp: u16,
    penalty_occluded_bp: u16,
    penalty_offscreen_bp: u16,
    penalty_stale_bp: u16,
    penalty_ambiguous_bp: u16,
    penalty_detached_bp: u16,
    penalty_zero_size_bp: u16,
}

/// Unvalidated positive weights, in basis points. Constructing this does not
/// make them a model. [`WeightBasisPoints::try_model`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeightBasisPoints {
    pub hypervector: u16,
    pub semantic: u16,
    pub source_agreement: u16,
    pub geometric: u16,
    pub actionability: u16,
    pub temporal_stability: u16,
    pub contextual_consistency: u16,
}

/// Unvalidated penalty deductions, in basis points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PenaltyBasisPoints {
    pub disabled: u16,
    pub hidden: u16,
    pub occluded: u16,
    pub offscreen: u16,
    pub stale: u16,
    pub ambiguous: u16,
    pub detached: u16,
    pub zero_size: u16,
}

impl WeightBasisPoints {
    /// Required sum of the positive weights. 100 basis points is exactly 1.
    pub const SUM_BP: u32 = 100;

    pub fn sum_bp(self) -> u32 {
        u32::from(self.hypervector)
            + u32::from(self.semantic)
            + u32::from(self.source_agreement)
            + u32::from(self.geometric)
            + u32::from(self.actionability)
            + u32::from(self.temporal_stability)
            + u32::from(self.contextual_consistency)
    }

    /// Reject a table whose parts do not sum to [`Self::SUM_BP`].
    pub fn try_model(
        self,
        version: u32,
        penalties: PenaltyBasisPoints,
    ) -> Result<ResonanceModel, crate::error::ResonanceError> {
        let sum = self.sum_bp();
        if sum != Self::SUM_BP {
            return Err(crate::error::ResonanceError::WeightsDoNotSum { sum });
        }
        Ok(ResonanceModel {
            version,
            hypervector_bp: self.hypervector,
            semantic_bp: self.semantic,
            source_agreement_bp: self.source_agreement,
            geometric_bp: self.geometric,
            actionability_bp: self.actionability,
            temporal_stability_bp: self.temporal_stability,
            contextual_consistency_bp: self.contextual_consistency,
            penalty_disabled_bp: penalties.disabled,
            penalty_hidden_bp: penalties.hidden,
            penalty_occluded_bp: penalties.occluded,
            penalty_offscreen_bp: penalties.offscreen,
            penalty_stale_bp: penalties.stale,
            penalty_ambiguous_bp: penalties.ambiguous,
            penalty_detached_bp: penalties.detached,
            penalty_zero_size_bp: penalties.zero_size,
        })
    }
}

impl ResonanceModel {
    pub const V1: Self = Self {
        version: 1,
        hypervector_bp: 35,
        semantic_bp: 20,
        source_agreement_bp: 15,
        geometric_bp: 10,
        actionability_bp: 10,
        temporal_stability_bp: 5,
        contextual_consistency_bp: 5,
        penalty_disabled_bp: 25,
        penalty_hidden_bp: 45,
        penalty_occluded_bp: 20,
        penalty_offscreen_bp: 35,
        penalty_stale_bp: 15,
        penalty_ambiguous_bp: 12,
        penalty_detached_bp: 20,
        penalty_zero_size_bp: 40,
    };

    pub const fn version(self) -> u32 {
        self.version
    }

    pub const fn weight_basis_points(self) -> WeightBasisPoints {
        WeightBasisPoints {
            hypervector: self.hypervector_bp,
            semantic: self.semantic_bp,
            source_agreement: self.source_agreement_bp,
            geometric: self.geometric_bp,
            actionability: self.actionability_bp,
            temporal_stability: self.temporal_stability_bp,
            contextual_consistency: self.contextual_consistency_bp,
        }
    }

    pub const fn penalty_basis_points(self) -> PenaltyBasisPoints {
        PenaltyBasisPoints {
            disabled: self.penalty_disabled_bp,
            hidden: self.penalty_hidden_bp,
            occluded: self.penalty_occluded_bp,
            offscreen: self.penalty_offscreen_bp,
            stale: self.penalty_stale_bp,
            ambiguous: self.penalty_ambiguous_bp,
            detached: self.penalty_detached_bp,
            zero_size: self.penalty_zero_size_bp,
        }
    }

    pub const fn weight_basis_point_sum(self) -> u16 {
        self.hypervector_bp
            + self.semantic_bp
            + self.source_agreement_bp
            + self.geometric_bp
            + self.actionability_bp
            + self.temporal_stability_bp
            + self.contextual_consistency_bp
    }

    pub fn hypervector(self) -> f64 {
        bp(self.hypervector_bp)
    }
    pub fn semantic(self) -> f64 {
        bp(self.semantic_bp)
    }
    pub fn source_agreement(self) -> f64 {
        bp(self.source_agreement_bp)
    }
    pub fn geometric(self) -> f64 {
        bp(self.geometric_bp)
    }
    pub fn actionability(self) -> f64 {
        bp(self.actionability_bp)
    }
    pub fn temporal_stability(self) -> f64 {
        bp(self.temporal_stability_bp)
    }
    pub fn contextual_consistency(self) -> f64 {
        bp(self.contextual_consistency_bp)
    }
    pub fn penalty_disabled(self) -> f64 {
        bp(self.penalty_disabled_bp)
    }
    pub fn penalty_hidden(self) -> f64 {
        bp(self.penalty_hidden_bp)
    }
    pub fn penalty_occluded(self) -> f64 {
        bp(self.penalty_occluded_bp)
    }
    pub fn penalty_offscreen(self) -> f64 {
        bp(self.penalty_offscreen_bp)
    }
    pub fn penalty_stale(self) -> f64 {
        bp(self.penalty_stale_bp)
    }
    pub fn penalty_ambiguous(self) -> f64 {
        bp(self.penalty_ambiguous_bp)
    }
    pub fn penalty_detached(self) -> f64 {
        bp(self.penalty_detached_bp)
    }
    pub fn penalty_zero_size(self) -> f64 {
        bp(self.penalty_zero_size_bp)
    }
}

const _: () = assert!(ResonanceModel::V1.weight_basis_point_sum() == 100);

fn bp(value: u16) -> f64 {
    f64::from(value) / 100.0
}
