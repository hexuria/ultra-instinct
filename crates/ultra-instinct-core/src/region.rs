use crate::element_state::ElementState;
use crate::error::CoreError;
use crate::id::{RegionId, StateFingerprint, UnitInterval};
use crate::rect::Rect;
use crate::vocab::{Action, RegionFlags, Role, SourceMask};

/// One interactive (or explicitly non-interactive) region in a snapshot.
///
/// Fields are private. The fingerprint is derived at construction from the
/// observable state, so callers cannot pair a label with an unrelated digest.
#[derive(Clone, Debug, PartialEq)]
pub struct InteractionRegion {
    id: RegionId,
    role: Role,
    label: String,
    rect: Rect,
    actions: Vec<Action>,
    parent: Option<RegionId>,
    sources: SourceMask,
    flags: RegionFlags,
    temporal_stability: UnitInterval,
    fingerprint: StateFingerprint,
    state: ElementState,
}

/// Owned inputs for [`InteractionRegion::try_new`].
#[derive(Clone, Debug)]
pub struct RegionParts {
    pub id: RegionId,
    pub role: Role,
    pub label: String,
    pub rect: Rect,
    pub actions: Vec<Action>,
    pub parent: Option<RegionId>,
    pub sources: SourceMask,
    pub flags: RegionFlags,
    pub temporal_stability: UnitInterval,
}

impl InteractionRegion {
    pub fn try_new(mut parts: RegionParts) -> Result<Self, CoreError> {
        parts.actions.sort_unstable();
        parts.actions.dedup();
        let fingerprint = fingerprint_of(&parts);
        Ok(Self {
            id: parts.id,
            role: parts.role,
            label: parts.label,
            rect: parts.rect,
            actions: parts.actions,
            parent: parts.parent,
            sources: parts.sources,
            flags: parts.flags,
            temporal_stability: parts.temporal_stability,
            fingerprint,
            state: ElementState::default(),
        })
    }

    pub fn id(&self) -> &RegionId {
        &self.id
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn rect(&self) -> Rect {
        self.rect
    }
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }
    pub fn parent(&self) -> Option<&RegionId> {
        self.parent.as_ref()
    }
    pub fn sources(&self) -> SourceMask {
        self.sources
    }
    pub fn flags(&self) -> RegionFlags {
        self.flags
    }
    pub fn temporal_stability(&self) -> UnitInterval {
        self.temporal_stability
    }
    pub fn fingerprint(&self) -> StateFingerprint {
        self.fingerprint
    }
    pub fn state(&self) -> &ElementState {
        &self.state
    }
    pub fn with_state(mut self, state: ElementState) -> Self {
        self.state = state;
        self
    }

    /// The owned inputs that rebuild this region. The fingerprint is derived
    /// again by [`InteractionRegion::try_new`].
    pub fn to_parts(&self) -> RegionParts {
        RegionParts {
            id: self.id.clone(),
            role: self.role,
            label: self.label.clone(),
            rect: self.rect,
            actions: self.actions.clone(),
            parent: self.parent.clone(),
            sources: self.sources,
            flags: self.flags,
            temporal_stability: self.temporal_stability,
        }
    }
}

fn fingerprint_of(parts: &RegionParts) -> StateFingerprint {
    let mut hash = 0xcbf29ce484222325u64;
    fn mix(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    mix(&mut hash, parts.role.as_str().as_bytes());
    mix(&mut hash, parts.label.as_bytes());
    let (x, y, w, h) = parts.rect.quantize_milli();
    mix(&mut hash, &x.to_le_bytes());
    mix(&mut hash, &y.to_le_bytes());
    mix(&mut hash, &w.to_le_bytes());
    mix(&mut hash, &h.to_le_bytes());
    for action in &parts.actions {
        mix(&mut hash, action.as_str().as_bytes());
    }
    match &parts.parent {
        Some(id) => mix(&mut hash, id.as_str().as_bytes()),
        None => mix(&mut hash, b"-"),
    }
    mix(&mut hash, &[parts.sources.bits()]);
    mix(&mut hash, &parts.flags.bits().to_le_bytes());
    let stability_q = (parts.temporal_stability.get() * 1_000_000.0).round() as u64;
    mix(&mut hash, &stability_q.to_le_bytes());
    StateFingerprint::from_bits(hash)
}
