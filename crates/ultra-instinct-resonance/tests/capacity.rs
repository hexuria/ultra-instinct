#![cfg(feature = "hgra")]
//! Closed-load smoke test. Not a regression gate.
//!
//! Load model: closed, single thread, N = 2000 regions, one locate.
//! Success: top-1 id is the golden target and the call returns.
//! Statistic: one-shot debug latency. The ceiling is a flake guard, not a
//! performance budget. Instruction counts are deferred (no valgrind).

use std::time::{Duration, Instant};

use ultra_instinct_core::{
    Action, InteractionManifold, InteractionRegion, LocateQuery, Rect, RegionFlags, RegionId,
    RegionParts, Role, SourceMask, UnitInterval, Zone,
};
use ultra_instinct_hyper::{Dims, Encoder};
use ultra_instinct_resonance::{locate_with, ResonanceModel};

fn button(id: &str, label: &str, x: f64, y: f64) -> InteractionRegion {
    InteractionRegion::try_new(RegionParts {
        id: RegionId::try_new(id).unwrap(),
        role: Role::Button,
        label: label.into(),
        rect: Rect::try_new(x, y, 40.0, 16.0).unwrap(),
        actions: vec![Action::Click],
        parent: None,
        sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
        flags: RegionFlags::none(),
        temporal_stability: UnitInterval::ONE,
    })
    .unwrap()
}

#[test]
fn two_thousand_regions_rank_the_golden_target() {
    const N: usize = 2000;
    // Measured debug one-shot on this shared box was about 2.0s after
    // identical neighbor vectors were coalesced (about 100s before).
    // 500ms fails that debug build. 10s is 5x the measurement: a flake
    // guard, not the PRD p95 and not a regression gate.
    const CEILING: Duration = Duration::from_secs(10);

    let mut regions = Vec::with_capacity(N);
    regions.push(button("target", "Settings", 16.0, 40.0));
    for index in 1..N {
        let column = (index % 40) as f64;
        let row = (index / 40) as f64;
        regions.push(button(
            &format!("d{index:04}"),
            "Other",
            400.0 + column * 24.0,
            40.0 + row * 16.0,
        ));
    }
    let manifold = InteractionManifold::try_new(
        Rect::try_viewport(0.0, 0.0, 1440.0, 900.0).unwrap(),
        regions,
        0,
    )
    .unwrap();
    assert_eq!(manifold.len(), N);
    let query = LocateQuery::new()
        .text("Settings")
        .unwrap()
        .role(Role::Button)
        .position(Zone::Left);
    let started = Instant::now();
    let ranked = locate_with(
        &manifold,
        &query,
        &Encoder::new(Dims::D512),
        ResonanceModel::V1,
    )
    .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(ranked[0].id().as_str(), "target", "golden top-1");
    assert_eq!(ranked[0].rank(), 1);
    assert_eq!(ranked.len(), N);
    assert!(ranked[0].score().total() > ranked[1].score().total());
    assert!(
        elapsed < CEILING,
        "smoke ceiling {CEILING:?} exceeded by {elapsed:?}; this is a flake guard, not a regression gate"
    );
}
