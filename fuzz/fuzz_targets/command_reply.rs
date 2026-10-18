#![no_main]

use libfuzzer_sys::fuzz_target;

// CommandTextModel consumes one JSON line from a user-supplied program's
// stdout — another trust boundary. parse_command_reply must only return Err,
// never panic.
fuzz_target!(|data: &[u8]| {
    let _ = ultra_instinct_policy::parse_command_reply(data);
});
