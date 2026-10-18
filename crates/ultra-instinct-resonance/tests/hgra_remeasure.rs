#![cfg(feature = "hgra")]
//! HGRA vs WeightedMatcher on the locate eval corpus (`evals/locate/cases.tsv`).
//!
//! Prints one row per case and matcher: top id, top total, runner-up margin,
//! and whether the product act gate (0.55 top, 0.05 margin) would refuse.
//! Run with `--nocapture` to see the table. The numbers are recorded in
//! `crates/ultra-instinct-resonance/HGRA_REMEASURE.md`. Totals are not calibrated
//! across matchers; compare tops, margins, and gate decisions, not totals.

use ultra_instinct_core::{parse_fixture, InteractionManifold, LocateQuery, Role, Zone};
use ultra_instinct_hyper::{Dims, Encoder};
use ultra_instinct_resonance::{
    locate_with, HgraMatcher, Match, RegionMatcher, ResonanceModel, WeightedMatcher,
};

// Mirrors ultra_instinct_guard::{MIN_ALLOW_CONFIDENCE, MIN_ALLOW_MARGIN}. The
// resonance crate does not depend on the guard crate.
const GATE_TOP: f64 = 0.55;
const GATE_MARGIN: f64 = 0.05;

struct Case {
    name: &'static str,
    manifold: &'static str,
    text: &'static str,
    role: Option<Role>,
    position: Option<Zone>,
    expected: &'static str,
}

const SEND: &str = include_str!("../../../fixtures/send-buttons.manifold");
const SIDEBAR: &str = include_str!("../../../fixtures/sidebar.manifold");
const TWINS: &str = include_str!("../../../evals/locate/twins.manifold");

fn corpus() -> Vec<Case> {
    vec![
        Case {
            name: "send",
            manifold: SEND,
            text: "Send",
            role: Some(Role::Button),
            position: None,
            expected: "z-send",
        },
        Case {
            name: "sidebar",
            manifold: SIDEBAR,
            text: "Settings",
            role: Some(Role::Button),
            position: Some(Zone::Left),
            expected: "nav-settings",
        },
        Case {
            name: "export",
            manifold: TWINS,
            text: "Export",
            role: Some(Role::Button),
            position: None,
            expected: "z-export",
        },
        Case {
            name: "admin",
            manifold: TWINS,
            text: "Admin",
            role: Some(Role::Button),
            position: None,
            expected: "z-admin",
        },
        Case {
            name: "undo",
            manifold: TWINS,
            text: "Undo",
            role: Some(Role::Button),
            position: None,
            expected: "z-undo",
        },
    ]
}

fn query(case: &Case) -> LocateQuery {
    let mut query = LocateQuery::new().text(case.text).unwrap();
    if let Some(role) = case.role {
        query = query.role(role);
    }
    if let Some(position) = case.position {
        query = query.position(position);
    }
    query
}

struct Row {
    top: String,
    confidence: f64,
    margin: f64,
    refused: bool,
}

fn row(ranked: &[Match]) -> Row {
    let top = &ranked[0];
    let margin = ranked.get(1).map_or(f64::INFINITY, |second| {
        top.confidence() - second.confidence()
    });
    let refused = top.confidence() < GATE_TOP || margin < GATE_MARGIN - 1e-9;
    Row {
        top: top.id().as_str().to_owned(),
        confidence: top.confidence(),
        margin,
        refused,
    }
}

#[test]
fn remeasure_hgra_against_weighted_on_the_locate_corpus() {
    let mut hgra_hits = 0;
    let mut agree = 0;
    let cases = corpus();
    println!("| case | matcher | top | top total | margin | gate |");
    println!("|---|---|---|---|---|---|");
    for case in &cases {
        let manifold: InteractionManifold = parse_fixture(case.manifold).unwrap();
        let query = query(case);
        let weighted = row(&WeightedMatcher::default().rank(&query, &manifold).unwrap());
        let hgra = row(&HgraMatcher::default().rank(&query, &manifold).unwrap());
        for (name, r) in [("weighted", &weighted), ("hgra", &hgra)] {
            println!(
                "| {} | {name} | {} | {:.4} | {:.4} | {} |",
                case.name,
                r.top,
                r.confidence,
                r.margin,
                if r.refused { "refuse" } else { "allow" }
            );
        }
        assert_eq!(weighted.top, case.expected, "weighted {}", case.name);
        if hgra.top == case.expected {
            hgra_hits += 1;
        }
        if hgra.top == weighted.top {
            agree += 1;
        }
    }
    println!(
        "hgra top-1 {hgra_hits}/{n}, top agreement {agree}/{n}",
        n = cases.len()
    );

    // Score parts for the Send case, both regions that matter.
    let manifold = parse_fixture(SEND).unwrap();
    let query = LocateQuery::new().text("Send").unwrap().role(Role::Button);
    let ranked = locate_with(
        &manifold,
        &query,
        &Encoder::new(Dims::DEFAULT),
        ResonanceModel::V1,
    )
    .unwrap();
    println!("| send region | hypervector | semantic | total |");
    println!("|---|---|---|---|");
    for candidate in &ranked {
        let score = candidate.score();
        println!(
            "| {} | {:.4} | {:.4} | {:.4} |",
            candidate.id().as_str(),
            score.hypervector(),
            score.semantic(),
            score.total()
        );
    }
}
