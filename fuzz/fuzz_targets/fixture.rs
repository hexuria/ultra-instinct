#![no_main]

use libfuzzer_sys::fuzz_target;

// The manifold fixture grammar is loaded from disk by tests, evals, and the
// replay path. A panic in parse_fixture is a DoS on every consumer; a
// parse/write/parse round-trip failure means the grammar cannot round-trip
// its own output.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(manifold) = ultra_instinct_core::parse_fixture(&text) {
        if let Ok(written) = ultra_instinct_core::write_fixture(&manifold) {
            ultra_instinct_core::parse_fixture(&written).expect("write_fixture output must re-parse");
        }
    }
});
