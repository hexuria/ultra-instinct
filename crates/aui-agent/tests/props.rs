//! Property tests for the executor boundary and abstention.

use aui_agent::{execute_ticketed, AgentBuilder, AgentOutcome, ExecError, Input, MockBrowser};
use aui_core::{parse_fixture, Action, InteractionManifold, RegionId};
use aui_guard::{gate, TicketLedger};
use aui_policy::InstinctPolicy;
use aui_protocol::{GuardReason, TicketInvalid};
use proptest::prelude::*;

const TICKET_INVALID: [TicketInvalid; 5] = [
    TicketInvalid::WorldChanged,
    TicketInvalid::TargetGone,
    TicketInvalid::TargetChanged,
    TicketInvalid::TicketConsumed,
    TicketInvalid::TicketMismatch,
];

#[derive(Clone, Copy)]
enum Extra {
    None,
    /// Far from every button in this fixture — target-scoped world ignores it.
    FarBanner,
    /// One peer beside each button — must invalidate every target's neighborhood.
    NearPeers,
}

fn page(
    n: usize,
    relabel: Option<(usize, &str)>,
    drop: Option<usize>,
    extra: Extra,
    modal: bool,
) -> InteractionManifold {
    let mut src = String::from("viewport w=800 h=600\n");
    for i in 0..n {
        if drop == Some(i) {
            continue;
        }
        let label = match relabel {
            Some((j, l)) if j == i => l.to_owned(),
            _ => format!("Button {i}"),
        };
        src.push_str(&format!(
            "region id=b{i} role=button label=\"{label}\" x=10 y={} w=80 h=20 actions=click sources=dom,accessibility\n",
            10 + i * 30
        ));
    }
    match extra {
        Extra::None => {}
        Extra::FarBanner => {
            src.push_str(
                "region id=toast role=button label=\"Undo\" x=600 y=500 w=80 h=20 actions=click sources=dom,accessibility\n",
            );
        }
        Extra::NearPeers => {
            for i in 0..n {
                if drop == Some(i) {
                    continue;
                }
                src.push_str(&format!(
                    "region id=near{i} role=button label=\"Peer {i}\" x=100 y={} w=80 h=20 actions=click sources=dom,accessibility\n",
                    10 + i * 30
                ));
            }
        }
    }
    if modal {
        src.push_str("region id=dlg role=dialog label=\"Hold on\" x=300 y=200 w=200 h=150 actions=focus sources=dom,accessibility flags=modal\n");
    }
    parse_fixture(&src).unwrap()
}

proptest! {
    /// The executor dispatches only the ticket's target, and only when the
    /// target and neighborhood (or global front layer) are unchanged since
    /// the ticket was issued. An unrelated far banner is not a world change.
    #[test]
    fn stale_never_executes_and_target_cannot_be_substituted(
        n in 2usize..6,
        pick in 0usize..6,
        mutation in 0u8..7,
        other in 0usize..6,
    ) {
        let pick = pick % n;
        let other = other % n;
        let before = page(n, None, None, Extra::None, false);
        let target = RegionId::try_new(format!("b{pick}")).unwrap();
        let ticket = gate(&before, &target, Action::Click, None, 0).unwrap();
        let after = match mutation {
            0 => before.clone(),
            1 => page(n, Some((pick, "Delete everything")), None, Extra::None, false),
            2 => page(n, None, Some(pick), Extra::None, false),
            3 => page(n, None, None, Extra::FarBanner, false),
            4 => page(n, None, None, Extra::None, true),
            5 => page(n, Some((other, "Renamed")), None, Extra::None, false),
            _ => page(n, None, None, Extra::NearPeers, false),
        };
        let expect_exec = match mutation {
            0 => true,
            // Far banner: target-scoped world does not change.
            3 => true,
            5 => other != pick,
            _ => false,
        };
        let mut browser = MockBrowser::new(after);
        let mut ledger = TicketLedger::new();
        let res = execute_ticketed(&mut browser, &mut ledger, &ticket, &Input::Click);
        prop_assert_eq!(res.is_ok(), expect_exec, "{:?}", res);
        if expect_exec {
            prop_assert_eq!(browser.press_log(), &[(target.clone(), Action::Click)][..]);
            // One-shot.
            prop_assert!(execute_ticketed(&mut browser, &mut ledger, &ticket, &Input::Click).is_err());
            prop_assert_eq!(browser.press_log().len(), 1);
        } else {
            prop_assert!(browser.press_log().is_empty());
        }
    }

    /// Instinct abstention never becomes an input, whatever the twin count/order.
    #[test]
    fn abstain_never_executes(twins in 2usize..6, shift in 0usize..6) {
        let mut src = String::from("viewport w=800 h=600\n");
        for i in 0..twins {
            let k = (i + shift) % twins;
            src.push_str(&format!(
                "region id=t{k} role=button label=\"Archive\" x=10 y={} w=80 h=20 actions=click sources=dom,accessibility\n",
                10 + k * 30
            ));
        }
        let mut agent = AgentBuilder::new(MockBrowser::new(parse_fixture(&src).unwrap()), InstinctPolicy::default())
            .max_steps(3)
            .build("Archive");
        let outcome = agent.run();
        prop_assert!(matches!(outcome, AgentOutcome::Abstained { .. }), "{:?}", outcome);
        prop_assert!(agent.browser_mut().press_log().is_empty());
    }

    /// Stale (observe → decide again) is exactly world / target drift. A spent
    /// lease (`ticket-consumed`), an operation swap (`ticket-mismatch`), a gate
    /// refusal, an observe failure, a page rejection, or a dispatch failure is
    /// never "stale", so the agent never silently retries it.
    #[test]
    fn is_stale_is_exactly_world_or_target_drift(k in 0usize..9) {
        let err = match k {
            0..=4 => ExecError::Ticket(TICKET_INVALID[k]),
            5 => ExecError::Gate(GuardReason::Occluded),
            6 => ExecError::Observe("x".into()),
            7 => ExecError::Rejected("x".into()),
            _ => ExecError::Dispatch("x".into()),
        };
        let expect = matches!(
            err,
            ExecError::Ticket(
                TicketInvalid::WorldChanged | TicketInvalid::TargetChanged | TicketInvalid::TargetGone
            )
        );
        prop_assert_eq!(err.is_stale(), expect, "{:?}", err);
    }

    /// Once the executor consumed a ticket (success or page rejection), any
    /// later use is `ticket-consumed` — even if the world also went stale —
    /// and nothing more is dispatched.
    #[test]
    fn consumed_beats_stale_at_the_executor(
        n in 2usize..6,
        pick in 0usize..6,
        reject: bool,
        mutation in 0u8..4,
    ) {
        let pick = pick % n;
        let before = page(n, None, None, Extra::None, false);
        let target = RegionId::try_new(format!("b{pick}")).unwrap();
        let ticket = gate(&before, &target, Action::Click, None, 0).unwrap();
        let mut browser = MockBrowser::new(before.clone());
        if reject {
            browser.reject_next("disabled at dispatch");
        }
        let mut ledger = TicketLedger::new();
        let first = execute_ticketed(&mut browser, &mut ledger, &ticket, &Input::Click);
        prop_assert_eq!(first.is_ok(), !reject);
        prop_assert!(ledger.is_consumed(ticket.ticket_id));
        let inputs_after_first = browser.input_log().len();
        browser.replace_manifold(match mutation {
            0 => before.clone(),
            1 => page(n, Some((pick, "Delete everything")), None, Extra::None, false),
            2 => page(n, None, Some(pick), Extra::None, false),
            _ => page(n, None, None, Extra::None, true),
        });
        let err = execute_ticketed(&mut browser, &mut ledger, &ticket, &Input::Click).unwrap_err();
        prop_assert_eq!(&err, &ExecError::Ticket(TicketInvalid::TicketConsumed));
        prop_assert!(!err.is_stale());
        prop_assert_eq!(browser.input_log().len(), inputs_after_first);
    }
}

#[test]
fn a_drag_ticket_releases_only_at_its_own_destination() {
    use aui_agent::{execute_ticketed, ExecError, Input, MockBrowser};
    use aui_guard::TicketLedger;
    use aui_protocol::TicketInvalid;
    let m = aui_core::parse_fixture(
        "viewport w=1200 h=900\n\
         region id=card role=generic label=\"Card\" x=10 y=10 w=60 h=40 actions=drag sources=dom\n\
         region id=bin role=generic label=\"Bin\" x=500 y=600 w=120 h=80 sources=dom\n\
         region id=other role=generic label=\"Other\" x=900 y=600 w=120 h=80 sources=dom\n",
    )
    .unwrap();
    let card = aui_core::RegionId::try_new("card").unwrap();
    let bin = aui_core::RegionId::try_new("bin").unwrap();
    let other = aui_core::RegionId::try_new("other").unwrap();
    let ticket = aui_guard::gate_drag(&m, &card, &bin, None, 0).unwrap();
    let mut browser = MockBrowser::new(m.clone());
    let mut ledger = TicketLedger::default();
    // Swapping the release point is a mismatch, never a drag elsewhere.
    let swapped = execute_ticketed(
        &mut browser,
        &mut ledger,
        &ticket,
        &Input::Drag { to: other },
    );
    assert!(matches!(
        swapped,
        Err(ExecError::Ticket(TicketInvalid::TicketMismatch))
    ));
    // A drag ticket never drives a click.
    let click = execute_ticketed(&mut browser, &mut ledger, &ticket, &Input::Click);
    assert!(matches!(
        click,
        Err(ExecError::Ticket(TicketInvalid::TicketMismatch))
    ));
    assert!(browser.input_log().is_empty());
    // The right destination runs once.
    execute_ticketed(
        &mut browser,
        &mut ledger,
        &ticket,
        &Input::Drag { to: bin.clone() },
    )
    .unwrap();
    assert_eq!(browser.input_log(), &[(card, Input::Drag { to: bin })]);
}

#[test]
fn a_cell_ticket_clicks_only_its_own_cell() {
    use aui_agent::{execute_ticketed, ExecError, Input, MockBrowser};
    use aui_guard::TicketLedger;
    use aui_protocol::TicketInvalid;
    let m = aui_core::parse_fixture(
        "viewport w=800 h=600\n\
         region id=pad role=image label=\"canvas\" x=0 y=0 w=160 h=160 actions=point,click sources=dom\n",
    )
    .unwrap();
    let pad = aui_core::RegionId::try_new("pad").unwrap();
    assert!(
        aui_guard::gate_point(&m, &pad, [8, 0], None, 0).is_err(),
        "off-grid refused"
    );
    let ticket = aui_guard::gate_point(&m, &pad, [3, 4], None, 0).unwrap();
    let mut browser = MockBrowser::new(m.clone());
    let mut ledger = TicketLedger::default();
    for wrong in [Input::ClickAt { cell: [4, 3] }, Input::Click] {
        assert!(matches!(
            execute_ticketed(&mut browser, &mut ledger, &ticket, &wrong),
            Err(ExecError::Ticket(TicketInvalid::TicketMismatch))
        ));
    }
    // A plain click ticket never clicks a cell.
    let plain = aui_guard::gate(&m, &pad, aui_core::Action::Click, None, 0).unwrap();
    assert!(matches!(
        execute_ticketed(
            &mut browser,
            &mut ledger,
            &plain,
            &Input::ClickAt { cell: [3, 4] }
        ),
        Err(ExecError::Ticket(TicketInvalid::TicketMismatch))
    ));
    execute_ticketed(
        &mut browser,
        &mut ledger,
        &ticket,
        &Input::ClickAt { cell: [3, 4] },
    )
    .unwrap();
    assert_eq!(
        browser.input_log(),
        &[(pad, Input::ClickAt { cell: [3, 4] })]
    );
}
