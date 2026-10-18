//! Live Chrome smoke for world-context gating.
//!
//! Ignored by default. Run via `examples/world-context/smoke.sh`, which starts
//! a throwaway Chrome and sets `ULTRA_INSTINCT_LIVE_CDP` + `ULTRA_INSTINCT_LIVE_SITE`.
//! Tests must run with `--test-threads=1` so they share one tab without racing.
//! No JEV, no Luna: observe → guard only.
//! Rank traces: `RUST_LOG=ultra_instinct_resonance=debug`.

use std::thread;
use std::time::Duration;

use serde_json::json;
use ultra_instinct_browser::{BrowserSession, CdpTransport, WebSocketTransport};
use ultra_instinct_core::{InteractionManifold, LocateQuery, Role};
#[cfg(feature = "hgra")]
use ultra_instinct_guard::guard_with;
use ultra_instinct_guard::{
    blocker, guard, with_front_layer, FrontLayer, GuardDecision, GuardError, GuardReason,
    GuardRequest,
};
#[cfg(feature = "hgra")]
use ultra_instinct_resonance::HgraMatcher;
use ultra_instinct_resonance::RegionState;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}

fn cdp() -> String {
    std::env::var("ULTRA_INSTINCT_LIVE_CDP")
        .expect("ULTRA_INSTINCT_LIVE_CDP (run examples/world-context/smoke.sh)")
}

fn site() -> String {
    std::env::var("ULTRA_INSTINCT_LIVE_SITE")
        .expect("ULTRA_INSTINCT_LIVE_SITE (run examples/world-context/smoke.sh)")
}

fn session_for(path: &str) -> BrowserSession<WebSocketTransport> {
    init_tracing();
    let url = format!("{}{}", site().trim_end_matches('/'), path);
    let transport = WebSocketTransport::connect(&cdp()).expect("connect live Chrome");
    let mut session = BrowserSession::new(transport);
    let t = session.transport_mut();
    let _ = t.call("Page.enable", "{}");
    let _ = t.call("DOM.enable", "{}");
    let _ = t.call("Accessibility.enable", "{}");
    let _ = t.call("Runtime.enable", "{}");
    t.call("Page.navigate", &json!({ "url": url }).to_string())
        .expect("Page.navigate");
    // Static pages; wait for onload scripts (showModal / focus).
    thread::sleep(Duration::from_millis(1000));
    let _ = t.call(
        "Runtime.evaluate",
        &json!({
            "expression": "(() => { const d=document.getElementById('confirm'); if (d && d.showModal && !d.open) d.showModal(); const b=document.getElementById('beta-host'); if (b) { b.focus(); b.select && b.select(); } return location.pathname + '|' + document.title + '|' + (document.activeElement && document.activeElement.id); })()",
            "returnByValue": true
        })
        .to_string(),
    );
    thread::sleep(Duration::from_millis(400));
    session
}

fn visibility(session: &BrowserSession<WebSocketTransport>, id: &str) -> String {
    let manifold = session.manifold().expect("observed");
    let effective = with_front_layer(manifold);
    let region = effective
        .get_str(id)
        .unwrap_or_else(|| panic!("missing {id}"));
    format!(
        "{:?}",
        RegionState::of(effective.viewport(), region).visibility()
    )
    .to_lowercase()
}

fn find_ids_by_label(session: &BrowserSession<WebSocketTransport>, label: &str) -> Vec<String> {
    let manifold = session.manifold().expect("observed");
    manifold
        .regions()
        .filter(|r| r.label() == label)
        .map(|r| r.id().as_str().to_owned())
        .collect()
}

/// Rank+decide with the matcher selected by `ULTRA_INSTINCT_MATCHER` (default weighted).
/// Set `ULTRA_INSTINCT_MATCHER=hgra` and build with `--features hgra` for forced-HGRA remasure.
fn decide(
    manifold: &InteractionManifold,
    request: &GuardRequest,
) -> Result<GuardDecision, GuardError> {
    let matcher = std::env::var("ULTRA_INSTINCT_MATCHER").unwrap_or_else(|_| "weighted".into());
    match matcher.as_str() {
        "weighted" | "" => guard(manifold, request),
        "hgra" => {
            #[cfg(feature = "hgra")]
            {
                eprintln!("matcher=hgra");
                guard_with(manifold, request, &HgraMatcher::default())
            }
            #[cfg(not(feature = "hgra"))]
            {
                let _ = (manifold, request);
                panic!("ULTRA_INSTINCT_MATCHER=hgra requires cargo --features hgra");
            }
        }
        other => panic!("unknown ULTRA_INSTINCT_MATCHER={other} (want weighted|hgra)"),
    }
}

fn dump_regions(session: &BrowserSession<WebSocketTransport>) -> String {
    let manifold = session.manifold().expect("observed");
    manifold
        .regions()
        .map(|r| format!("{}:{}:{:?}", r.id(), r.label(), r.role()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
#[ignore = "live Chrome: examples/world-context/smoke.sh"]
fn live_modal_confirm_refuses_buried_delete_and_allows_dialog_cancel() {
    let mut session = session_for("/modal-confirm.html");
    let manifold = session.observe().expect("observe modal").clone();
    eprintln!("modal regions: {}", dump_regions(&session));

    let layer = FrontLayer::of(&manifold);
    assert!(
        !layer.is_empty(),
        "expected an open dialog front layer; regions={}",
        dump_regions(&session)
    );

    let buried = decide(
        &manifold,
        &GuardRequest::click(
            LocateQuery::new()
                .text("Delete project")
                .unwrap()
                .role(Role::Button),
        ),
    )
    .unwrap();
    match buried {
        GuardDecision::Refuse {
            reason: GuardReason::FrontLayer | GuardReason::Occluded,
            ..
        } => {}
        other => {
            panic!("expected refuse front-layer or occluded for Delete project, got {other:?}")
        }
    }

    let twin = decide(
        &manifold,
        &GuardRequest::click(
            LocateQuery::new()
                .text("Cancel")
                .unwrap()
                .role(Role::Button),
        ),
    )
    .unwrap();
    match twin {
        GuardDecision::Allow { ref target, .. } => {
            let region = manifold.get(&target.id).expect("allow target");
            assert!(
                blocker(&manifold, region).is_none(),
                "allowed Cancel must not be blocked; got {}",
                target.id
            );
        }
        other => panic!("expected allow dialog Cancel, got {other:?}"),
    }
}

#[test]
#[ignore = "live Chrome: examples/world-context/smoke.sh"]
fn live_twin_suspend_near_focus_picks_beta_row() {
    let mut session = session_for("/twin-suspend.html");
    let manifold = session.observe().expect("observe twins").clone();
    let page = session.page().expect("page").clone();
    eprintln!(
        "twins regions: {}; focused={:?}",
        dump_regions(&session),
        page.focused()
    );
    let focused = page
        .focused()
        .cloned()
        .expect("a Hostname field should be focused");
    let focused_region = manifold.get(&focused).expect("focused region");
    assert_eq!(
        focused_region.label(),
        "Hostname",
        "focus should be on a Hostname field, got {:?}",
        focused_region.label()
    );

    let bare = decide(
        &manifold,
        &GuardRequest::click(
            LocateQuery::new()
                .text("Suspend")
                .unwrap()
                .role(Role::Button),
        ),
    )
    .unwrap();
    match bare {
        GuardDecision::Escalate {
            reason: GuardReason::Ambiguous,
            ..
        } => {}
        other => panic!("expected escalate ambiguous without context, got {other:?}"),
    }

    let focused_guard = decide(
        &manifold,
        &GuardRequest::click(
            LocateQuery::new()
                .text("Suspend")
                .unwrap()
                .role(Role::Button)
                .near(Some(focused.clone())),
        ),
    )
    .unwrap();
    match focused_guard {
        GuardDecision::Allow { ref target, .. } => {
            let region = manifold.get(&target.id).expect("target");
            let row = manifold
                .ancestors(&focused)
                .into_iter()
                .find(|ancestor| {
                    manifold
                        .regions()
                        .any(|r| r.label() == "Suspend" && manifold.is_within(r.id(), ancestor))
                })
                .cloned()
                .or_else(|| region.parent().cloned())
                .expect("focused field should sit in a row with Suspend");
            assert!(
                manifold.is_within(region.id(), &row) || region.id() == &row,
                "allowed Suspend {} not under focused row {row}",
                target.id
            );
            let others = find_ids_by_label(&session, "Suspend");
            assert!(
                others.iter().any(|id| id == target.id.as_str()),
                "allowed id missing from Suspend set"
            );
        }
        other => panic!("expected allow Suspend near focus, got {other:?}"),
    }
}

#[test]
#[ignore = "live Chrome: examples/world-context/smoke.sh"]
fn live_cookie_backdrop_hit_test_refuses_save() {
    let mut session = session_for("/cookie-backdrop.html");
    let manifold = session.observe().expect("observe cookie").clone();
    eprintln!("cookie regions: {}", dump_regions(&session));

    let save_ids = find_ids_by_label(&session, "Save");
    assert_eq!(save_ids.len(), 1, "expected one Save, got {save_ids:?}");
    let save_id = &save_ids[0];
    let save = manifold.get_str(save_id).expect("save region");
    assert!(
        save.flags().occluded() || blocker(&manifold, save).is_some(),
        "Save under the cookie backdrop must be occluded or blocked; flags={:?} vis={} regions={}",
        save.flags(),
        visibility(&session, save_id),
        dump_regions(&session)
    );

    let decision = decide(
        &manifold,
        &GuardRequest::click(LocateQuery::new().text("Save").unwrap().role(Role::Button)),
    )
    .unwrap();
    match decision {
        GuardDecision::Refuse {
            reason: GuardReason::Occluded | GuardReason::FrontLayer,
            ..
        } => {}
        other => panic!("expected refuse occluded/front-layer for Save, got {other:?}"),
    }

    let accept = decide(
        &manifold,
        &GuardRequest::click(
            LocateQuery::new()
                .text("Accept all")
                .unwrap()
                .role(Role::Button),
        ),
    )
    .unwrap();
    assert!(
        matches!(accept, GuardDecision::Allow { .. }),
        "Accept all on the banner must allow, got {accept:?}"
    );
}
