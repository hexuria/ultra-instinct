#![no_main]

use libfuzzer_sys::fuzz_target;

// Battle diaries cross a trust boundary on replay: a torn final line, a
// wrong schema, unknown types, or hostile field shapes must all fail the
// field-validated parse without panicking.
fuzz_target!(|data: &[u8]| {
    let line = String::from_utf8_lossy(data);
    let _ = ultra_instinct_dojo::parse_line(&line, 1);
});
