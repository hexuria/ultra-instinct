#![no_main]

use ultra_instinct_core::{parse_fixture, ActionSpace};
use libfuzzer_sys::fuzz_target;

// Remote-model replies cross a trust boundary: a model could emit off-menu
// ids, selectors, coordinates, scripts, or extra fields. parse_reply must
// refuse all of them without panicking, so fuzz it against a real
// ActionSpace built from a real fixture.
const SPACE_FIXTURE: &str = include_str!("../seeds/remote_reply/space_fixture");

fuzz_target!(|data: &[u8]| {
    let manifold = parse_fixture(SPACE_FIXTURE).expect("seed fixture must parse");
    let space = ActionSpace::from_manifold(&manifold);
    let reply = String::from_utf8_lossy(data);
    let _ = ultra_instinct_policy::parse_reply(&space, &reply);
});
