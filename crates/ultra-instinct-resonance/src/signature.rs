#![cfg(feature = "hgra")]
use std::collections::{BTreeMap, HashMap};

use ultra_instinct_core::{tokenize, InteractionManifold, InteractionRegion, Relation};
use ultra_instinct_geometry::{normalize, size_class, spatial_relations, zones};
use ultra_instinct_hyper::{bind, bundle, permute, relation_shift, BipolarVector, Encoder};

use crate::error::ResonanceError;

// Relative bundle weights. Not a probability distribution: they do not sum
// to 1, and normalizing them would change the signature. See DECISIONS.md.
const ROLE_WEIGHT: f64 = 2.0;
const LABEL_WEIGHT: f64 = 3.0;
const POSITION_WEIGHT: f64 = 2.0;
const SHAPE_WEIGHT: f64 = 1.0;
const ACTION_WEIGHT: f64 = 1.0;
const PARENT_WEIGHT: f64 = 1.0;
const NEIGHBOR_WEIGHT: f64 = 0.25;
const STATE_WEIGHT: f64 = 0.5;
const SOURCE_WEIGHT: f64 = 0.5;
// Every query probe bundles at the same weight. See `query_vector`.
const QUERY_PART_WEIGHT: f64 = 1.0;

pub(crate) struct Memory<'a> {
    encoder: &'a Encoder,
    cache: HashMap<(String, String), BipolarVector>,
}

impl<'a> Memory<'a> {
    pub(crate) fn new(encoder: &'a Encoder) -> Self {
        Self {
            encoder,
            cache: HashMap::new(),
        }
    }

    fn symbol(&mut self, namespace: &str, symbol: &str) -> Result<BipolarVector, ResonanceError> {
        let key = (namespace.to_owned(), symbol.to_owned());
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit.clone());
        }
        let encoded = self.encoder.encode(namespace, symbol)?;
        self.cache.insert(key, encoded.clone());
        Ok(encoded)
    }

    fn bound(
        &mut self,
        key: &str,
        value_namespace: &str,
        value: &str,
    ) -> Result<BipolarVector, ResonanceError> {
        let key_vec = self.symbol("key", key)?;
        let value_vec = self.symbol(value_namespace, value)?;
        Ok(bind(&key_vec, &value_vec)?)
    }
}

/// Compose a region signature.
///
/// The bundle, in order, is:
/// role, label tokens, position zones, shape, actions, parent (permuted),
/// neighborhood relations (permuted, excluding the parent id), content state,
/// and source bits.
///
/// Penalty flags are intentionally absent. They are applied later as
/// subtractions so a disabled control does not hide inside the hypervector.
pub(crate) fn region_signature(
    manifold: &InteractionManifold,
    region: &InteractionRegion,
    memory: &mut Memory<'_>,
) -> Result<BipolarVector, ResonanceError> {
    let mut parts = Vec::new();
    parts.push((
        memory.bound("role", "role", region.role().as_str())?,
        ROLE_WEIGHT,
    ));
    for token in tokenize(region.label()) {
        parts.push((memory.bound("label", "label", &token)?, LABEL_WEIGHT));
    }
    let normalized = normalize(region.rect(), manifold.viewport())?;
    for zone in zones(normalized) {
        parts.push((
            memory.bound("position", "position", zone.as_str())?,
            POSITION_WEIGHT,
        ));
    }
    parts.push((
        memory.bound("shape", "shape", size_class(normalized).as_str())?,
        SHAPE_WEIGHT,
    ));
    for action in region.actions() {
        parts.push((
            memory.bound("action", "action", action.as_str())?,
            ACTION_WEIGHT,
        ));
    }
    if let Some(parent_id) = region.parent() {
        if let Some(parent) = manifold.get(parent_id) {
            push_permuted(
                &mut parts,
                memory,
                "parent",
                parent.role().as_str(),
                PARENT_WEIGHT,
            )?;
            for token in tokenize(parent.label()) {
                push_permuted(&mut parts, memory, "parent", &token, PARENT_WEIGHT)?;
            }
        }
    }
    // Neighbors that share a relation and a role produce the same bipolar
    // vector. `NEIGHBOR_WEIGHT` is 1/4, so `weight * count` is the same f64
    // as adding `weight` once per neighbor, and the bundle sum stays an exact
    // multiple of 1/4. Collapsing them does not change a sign.
    let mut neighbor_hits: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    for other in manifold.regions() {
        if other.id() == region.id() || Some(other.id()) == region.parent() {
            continue;
        }
        let other_norm = normalize(other.rect(), manifold.viewport())?;
        for relation in spatial_relations(normalized, other_norm) {
            if !is_neighborhood(relation) {
                continue;
            }
            *neighbor_hits
                .entry((relation.as_str(), other.role().as_str()))
                .or_insert(0) += 1;
        }
    }
    for ((relation, role), count) in neighbor_hits {
        push_permuted(
            &mut parts,
            memory,
            relation,
            role,
            NEIGHBOR_WEIGHT * f64::from(count),
        )?;
    }
    let (x, y, w, h) = region.rect().quantize_milli();
    let state = format!(
        "{}|{}|{x}|{y}|{w}|{h}",
        region.role().as_str(),
        region.label()
    );
    parts.push((memory.bound("state", "state", &state)?, STATE_WEIGHT));
    for source in region.sources().iter() {
        let name = source_name(source.bits());
        parts.push((memory.bound("source", "source", name)?, SOURCE_WEIGHT));
    }
    Ok(bundle(&parts)?)
}

fn is_neighborhood(relation: Relation) -> bool {
    matches!(
        relation,
        Relation::Near
            | Relation::Above
            | Relation::Below
            | Relation::AlignedX
            | Relation::AlignedY
            | Relation::Overlaps
    )
}

fn push_permuted(
    parts: &mut Vec<(BipolarVector, f64)>,
    memory: &mut Memory<'_>,
    relation: &str,
    filler_symbol: &str,
    weight: f64,
) -> Result<(), ResonanceError> {
    let shift = relation_shift(relation, memory.encoder.dims());
    let filler = memory.symbol("filler", filler_symbol)?;
    let permuted = permute(&filler, shift);
    let relation_vec = memory.symbol("relation", relation)?;
    parts.push((bind(&relation_vec, &permuted)?, weight));
    Ok(())
}

/// One bound `key * value` probe per query constraint, in the same
/// namespaces the region signature uses: role, each label token, position,
/// action. These are the parts [`query_vector`] bundles.
pub(crate) fn query_probes(
    query: &ultra_instinct_core::LocateQuery,
    memory: &mut Memory<'_>,
) -> Result<Vec<BipolarVector>, ResonanceError> {
    let mut probes = Vec::new();
    if let Some(role) = query.role_ref() {
        probes.push(memory.bound("role", "role", role.as_str())?);
    }
    if let Some(text) = query.text_ref() {
        for token in tokenize(text) {
            probes.push(memory.bound("label", "label", &token)?);
        }
    }
    if let Some(zone) = query.position_ref() {
        probes.push(memory.bound("position", "position", zone.as_str())?);
    }
    if let Some(action) = query.action_ref() {
        probes.push(memory.bound("action", "action", action.as_str())?);
    }
    Ok(probes)
}

/// The query as one hypervector: the probes from [`query_probes`] plus any
/// world-context parent channel from [`ContextScope`], bundled the same way
/// the region signature bundles its parts. The ranker takes a single cosine
/// of this vector against each region signature.
///
/// Context encoding: when `within` or a resolved `near` scope names a
/// container, the query adds the same permuted `parent` bindings the region
/// signature stores for that container's role and label tokens. Cosine then
/// prefers descendants of that container; the hard `ContextScope::score`
/// minimum still zeros out-of-scope regions.
///
/// Equal weight keeps parity with the probe set the old probe-mean scored:
/// every constraint (and every label token) counts once. With an even number
/// of probes a component can sum to zero; [`bundle`] breaks that tie to `+1`,
/// as documented in the algebra crate. `None` when the query has no
/// constraint and no context channel, so the caller keeps the neutral
/// empty-query behaviour.
pub(crate) fn query_vector(
    query: &ultra_instinct_core::LocateQuery,
    scope: &crate::ContextScope,
    manifold: &InteractionManifold,
    memory: &mut Memory<'_>,
) -> Result<Option<BipolarVector>, ResonanceError> {
    let probes = query_probes(query, memory)?;
    let mut parts: Vec<(BipolarVector, f64)> = probes
        .into_iter()
        .map(|probe| (probe, QUERY_PART_WEIGHT))
        .collect();
    // Prefer near_scope when both are set: within already filtered the pool,
    // and near is the cursor-local container (cargo-runner rule).
    let container = scope.near_scope().or(scope.within());
    if let Some(container) = container {
        push_context_parent_channel(&mut parts, memory, manifold, container)?;
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(bundle(&parts)?))
}

/// Match [`region_signature`]'s parent channel so cosine aligns on ancestry.
fn push_context_parent_channel(
    parts: &mut Vec<(BipolarVector, f64)>,
    memory: &mut Memory<'_>,
    manifold: &InteractionManifold,
    container: &ultra_instinct_core::RegionId,
) -> Result<(), ResonanceError> {
    let Some(region) = manifold.get(container) else {
        return Ok(());
    };
    push_permuted(
        parts,
        memory,
        "parent",
        region.role().as_str(),
        PARENT_WEIGHT,
    )?;
    for token in tokenize(region.label()) {
        push_permuted(parts, memory, "parent", &token, PARENT_WEIGHT)?;
    }
    Ok(())
}

fn source_name(bits: u8) -> &'static str {
    match bits {
        0b0001 => "dom",
        0b0010 => "accessibility",
        0b0100 => "screenshot",
        0b1000 => "cua",
        _ => "unknown",
    }
}
