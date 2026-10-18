//! A bounded, in-memory ring of recent snapshots.
//!
//! Ids are assigned by [`SnapshotRing::push`] in strictly increasing order and
//! are never reused, even after the entry is evicted. Nothing is written to
//! disk. This is not crash recovery and not a persistence layer.

use std::collections::VecDeque;
use std::fmt;

use ultra_instinct_core::{InteractionManifold, Role};

/// Identity of one stored snapshot. Monotonic within one ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(u64);

impl SnapshotId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HistoryError {
    /// The id was issued but has been evicted. `oldest` is the oldest id kept.
    Evicted { id: SnapshotId, oldest: SnapshotId },
    /// The id was never issued by this ring.
    Unknown(SnapshotId),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evicted { id, oldest } => {
                write!(f, "snapshot {id} was evicted; oldest kept is {oldest}")
            }
            Self::Unknown(id) => write!(f, "snapshot {id} was never recorded"),
        }
    }
}

impl std::error::Error for HistoryError {}

#[derive(Clone, Debug)]
pub struct SnapshotRing<T> {
    cap: usize,
    next: u64,
    entries: VecDeque<(SnapshotId, T)>,
}

impl<T> SnapshotRing<T> {
    pub const DEFAULT_CAP: usize = 16;

    /// A ring that keeps at most `cap` entries. A `cap` of zero is raised to one.
    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            cap,
            next: 1,
            entries: VecDeque::with_capacity(cap),
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Store `value` and return its new id. Evicts the oldest entry when full.
    pub fn push(&mut self, value: T) -> SnapshotId {
        let id = SnapshotId(self.next);
        self.next += 1;
        if self.entries.len() == self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back((id, value));
        id
    }

    pub fn get(&self, id: SnapshotId) -> Result<&T, HistoryError> {
        if id.0 == 0 || id.0 >= self.next {
            return Err(HistoryError::Unknown(id));
        }
        match self.entries.front() {
            Some((oldest, _)) if id < *oldest => Err(HistoryError::Evicted {
                id,
                oldest: *oldest,
            }),
            Some((oldest, _)) => {
                let offset = usize::try_from(id.0 - oldest.0).expect("offset fits in usize");
                Ok(&self.entries[offset].1)
            }
            None => Err(HistoryError::Unknown(id)),
        }
    }

    pub fn latest(&self) -> Option<(SnapshotId, &T)> {
        self.entries.back().map(|(id, value)| (*id, value))
    }

    /// Entries from newest to oldest.
    pub fn iter_recent(&self) -> impl Iterator<Item = (SnapshotId, &T)> {
        self.entries.iter().rev().map(|(id, value)| (*id, value))
    }
}

/// Sorted `(role, label)` multiset of one observation.
///
/// Two regions with the same role and label both count. Rectangles, ids, and
/// flags are not part of the signature. [`detect`] treats equal signatures as
/// the same state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateSignature {
    entries: Vec<(Role, String)>,
}

impl StateSignature {
    pub fn from_manifold(manifold: &InteractionManifold) -> Self {
        let mut entries: Vec<(Role, String)> = manifold
            .regions()
            .map(|region| (region.role(), region.label().to_owned()))
            .collect();
        entries.sort();
        Self { entries }
    }
}

/// What an act did to the signature history. Data only. Not a retry policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemporalSignal {
    /// `after` has the same signature as `before`.
    NoOp,
    /// `after` matches an older same-origin signature in the four snapshots
    /// immediately before it, other than `before`.
    LoopDetected { matches: Vec<SnapshotId> },
}

/// A ring entry [`detect`] can read. The MCP snapshot implements this.
pub trait SignedSnapshot {
    fn origin(&self) -> &str;
    fn state_signature(&self) -> StateSignature;
}

/// Compare `after` with `before` and with the four older ring entries.
///
/// `NoOp` is signature equality of the act pair. `LoopDetected` is signature
/// equality with an older snapshot of the same origin inside that window.
/// A match on `before` is `NoOp`, not a loop. Both can be returned. This does
/// not click, navigate, or choose a retry.
pub fn detect<T: SignedSnapshot>(
    ring: &SnapshotRing<T>,
    before: SnapshotId,
    after: SnapshotId,
) -> Result<Vec<TemporalSignal>, HistoryError> {
    let before_state = ring.get(before)?;
    let after_state = ring.get(after)?;
    let before_signature = before_state.state_signature();
    let after_signature = after_state.state_signature();
    let origin = after_state.origin().to_owned();

    let mut signals = Vec::new();
    if before_signature == after_signature {
        signals.push(TemporalSignal::NoOp);
    }

    let mut matches = Vec::new();
    let mut window = 0usize;
    for (id, state) in ring.iter_recent() {
        if id >= after {
            continue;
        }
        if window == 4 {
            break;
        }
        window += 1;
        if id == before {
            continue;
        }
        if state.origin() != origin {
            continue;
        }
        if state.state_signature() == after_signature {
            matches.push(id);
        }
    }
    if !matches.is_empty() {
        matches.sort();
        signals.push(TemporalSignal::LoopDetected { matches });
    }
    Ok(signals)
}

impl<T> Default for SnapshotRing<T> {
    fn default() -> Self {
        Self::with_capacity(Self::DEFAULT_CAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ultra_instinct_core::{
        Action, InteractionRegion, Rect, RegionFlags, RegionId, RegionParts, SourceMask,
        UnitInterval,
    };

    #[test]
    fn ring_is_empty_until_the_first_push() {
        let mut ring: SnapshotRing<u8> = SnapshotRing::with_capacity(2);
        assert!(ring.is_empty());
        ring.push(1);
        assert!(!ring.is_empty());
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn ring_evicts_oldest_and_ids_never_repeat() {
        let mut ring = SnapshotRing::with_capacity(2);
        let a = ring.push("a");
        let b = ring.push("b");
        let c = ring.push("c");
        assert_eq!((a.get(), b.get(), c.get()), (1, 2, 3));
        assert_eq!(ring.len(), 2);
        assert_eq!(
            ring.get(a).unwrap_err(),
            HistoryError::Evicted { id: a, oldest: b }
        );
        assert_eq!(
            ring.get(a).unwrap_err().to_string(),
            "snapshot 1 was evicted; oldest kept is 2"
        );
        assert_eq!(ring.get(b), Ok(&"b"));
        assert_eq!(ring.get(c), Ok(&"c"));
        assert_eq!(
            ring.get(SnapshotId::new(9)).unwrap_err(),
            HistoryError::Unknown(SnapshotId::new(9))
        );
        assert_eq!(
            ring.get(SnapshotId::new(0)).unwrap_err(),
            HistoryError::Unknown(SnapshotId::new(0))
        );
        assert_eq!(ring.latest(), Some((c, &"c")));
        let recent: Vec<_> = ring.iter_recent().map(|(id, _)| id.get()).collect();
        assert_eq!(recent, [3, 2]);
        assert_eq!(SnapshotRing::<u8>::with_capacity(0).capacity(), 1);
        assert_eq!(SnapshotRing::<u8>::default().capacity(), 16);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn snapshot_ids_are_strictly_increasing(cap in 1usize..6, pushes in 1usize..40) {
            let mut ring = SnapshotRing::with_capacity(cap);
            let mut last = 0;
            for value in 0..pushes {
                let id = ring.push(value);
                prop_assert!(id.get() > last);
                last = id.get();
                prop_assert_eq!(ring.get(id), Ok(&value));
                prop_assert!(ring.len() <= cap);
            }
        }
    }

    struct Sample {
        origin: &'static str,
        signature: StateSignature,
    }

    impl SignedSnapshot for Sample {
        fn origin(&self) -> &str {
            self.origin
        }

        fn state_signature(&self) -> StateSignature {
            self.signature.clone()
        }
    }

    #[test]
    fn no_op_when_after_equals_before() {
        let send = signature(&["Send"]);
        let other = signature(&["Cancel"]);
        let mut ring = SnapshotRing::with_capacity(8);
        let before = ring.push(sample("page", send.clone()));
        let after = ring.push(sample("page", send));
        assert_eq!(
            detect(&ring, before, after).unwrap(),
            vec![TemporalSignal::NoOp]
        );
        let changed = ring.push(sample("page", other));
        assert_eq!(detect(&ring, after, changed).unwrap(), vec![]);
    }

    #[test]
    fn loop_when_after_equals_an_older_snapshot() {
        let send = signature(&["Send"]);
        let other = signature(&["Cancel"]);
        let mut ring = SnapshotRing::with_capacity(8);
        let older = ring.push(sample("page", send.clone()));
        let before = ring.push(sample("page", other.clone()));
        let after = ring.push(sample("page", send.clone()));
        assert_eq!(
            detect(&ring, before, after).unwrap(),
            vec![TemporalSignal::LoopDetected {
                matches: vec![older]
            }]
        );

        // Five snapshots sit between the duplicate and `after`, so it is
        // outside the last four and is not a loop.
        let mut far = SnapshotRing::with_capacity(16);
        let outside = far.push(sample("page", send.clone()));
        for _ in 0..4 {
            far.push(sample("page", other.clone()));
        }
        let before = far.push(sample("page", other.clone()));
        let after = far.push(sample("page", send.clone()));
        assert_eq!(detect(&far, before, after).unwrap(), vec![]);
        assert_ne!(outside, before);

        // Same signature on another origin is not this page's loop.
        let mut other_origin = SnapshotRing::with_capacity(8);
        other_origin.push(sample("other", send.clone()));
        let before = other_origin.push(sample("page", other));
        let after = other_origin.push(sample("page", send));
        assert_eq!(detect(&other_origin, before, after).unwrap(), vec![]);
    }

    #[test]
    fn loop_window_is_exactly_four_snapshots_before_after() {
        let send = signature(&["Send"]);
        let other = signature(&["Cancel"]);
        // Window: before, filler, filler, older. older is the 4th and matches.
        let mut ring = SnapshotRing::with_capacity(16);
        let older = ring.push(sample("page", send.clone()));
        ring.push(sample("page", other.clone()));
        ring.push(sample("page", other.clone()));
        let before = ring.push(sample("page", other.clone()));
        let after = ring.push(sample("page", send.clone()));
        assert_eq!(
            detect(&ring, before, after).unwrap(),
            vec![TemporalSignal::LoopDetected {
                matches: vec![older]
            }]
        );

        // One more filler pushes older out of the window of four.
        let mut ring = SnapshotRing::with_capacity(16);
        let outside = ring.push(sample("page", send.clone()));
        for _ in 0..3 {
            ring.push(sample("page", other.clone()));
        }
        let before = ring.push(sample("page", other.clone()));
        let after = ring.push(sample("page", send.clone()));
        assert_eq!(detect(&ring, before, after).unwrap(), vec![]);
        assert_ne!(outside, before);

        // A different-origin snapshot between before and after still counts
        // toward the four, so an older same-origin match can fall out.
        let mut mixed = SnapshotRing::with_capacity(16);
        let older = mixed.push(sample("page", send.clone()));
        mixed.push(sample("other", other.clone()));
        mixed.push(sample("other", other.clone()));
        mixed.push(sample("other", other.clone()));
        let before = mixed.push(sample("page", other.clone()));
        let after = mixed.push(sample("page", send));
        assert_eq!(detect(&mixed, before, after).unwrap(), vec![]);
        assert_ne!(older, before);
    }

    fn sample(origin: &'static str, signature: StateSignature) -> Sample {
        Sample { origin, signature }
    }

    fn signature(labels: &[&str]) -> StateSignature {
        let regions = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                InteractionRegion::try_new(RegionParts {
                    id: RegionId::try_new(format!("n{index}")).unwrap(),
                    role: Role::Button,
                    label: (*label).into(),
                    rect: Rect::try_new(0.0, index as f64, 40.0, 20.0).unwrap(),
                    actions: vec![Action::Click],
                    parent: None,
                    sources: SourceMask::DOM,
                    flags: RegionFlags::none(),
                    temporal_stability: UnitInterval::ONE,
                })
                .unwrap()
            })
            .collect();
        let manifold = InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 800.0, 600.0).unwrap(),
            regions,
            0,
        )
        .unwrap();
        StateSignature::from_manifold(&manifold)
    }
}
