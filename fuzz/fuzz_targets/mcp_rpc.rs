#![no_main]

use libfuzzer_sys::fuzz_target;

// The MCP adapter reads untrusted stdin lines from a host process. It must
// never panic on malformed input, and every reply it emits must be
// well-formed JSON.
fuzz_target!(|data: &[u8]| {
    let line = String::from_utf8_lossy(data);
    if let Some(reply) = ultra_instinct_mcp::handle_line(&line) {
        assert!(
            serde_json::from_str::<serde_json::Value>(&reply).is_ok(),
            "mcp reply is not valid JSON"
        );
    }
});
