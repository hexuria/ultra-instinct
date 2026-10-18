#![no_main]

use libfuzzer_sys::fuzz_target;

// The compact snapshot is a `Runtime.evaluate` reply: CDP-controlled, but the
// node records inside it are shaped by the live page (tag collisions, forged
// `data-hu-k` values, hostile globals). The parser must return an error for
// anything malformed and must never panic.
fuzz_target!(|data: &[u8]| {
    let json = String::from_utf8_lossy(data);
    let _ = ultra_instinct_browser::compact_parse_fuzzable(&json);
});
