#![no_main]

use libfuzzer_sys::fuzz_target;

// ReplayTransport::parse consumes the CDP replay grammar — the same grammar
// live sessions record and tests replay. It must only return Err on malformed
// input; a panic or hang here is a DoS on the whole test harness.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = ultra_instinct_browser::ReplayTransport::parse(&text);
});
