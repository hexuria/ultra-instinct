//! stdio MCP server for ultra-instinct.
//!
//! Product tools: observe, guard, verify. Locate, inspect, and diff remain as
//! transitional helpers. `act` is accepted as a deprecated alias of `guard` and
//! never clicks.
//!
//! `browser_*` tools are the browser-use-compatible host surface (navigate,
//! get_state, click, type, scroll, screenshot, tabs). Target-bound input still
//! goes through gate → ticket → `execute_ticketed`, so a stale page refuses
//! instead of clicking the wrong element; page-level calls are plain CDP.

#![forbid(unsafe_code)]

mod error;
mod repeat;
mod rpc;
mod server;
mod tools;

pub use error::ToolError;
pub use repeat::{REPEAT_THRESHOLD, REPEAT_WINDOW};
pub use rpc::{handle_line, serve_stdio};
pub use server::{CdpConnector, Server, MAX_LIVE_SESSIONS};
pub use tools::call_tool;

pub const TOOL_OBSERVE: &str = "observe";
pub const TOOL_LOCATE: &str = "locate";
pub const TOOL_INSPECT: &str = "inspect";
pub const TOOL_GUARD: &str = "guard";
/// Deprecated alias of [`TOOL_GUARD`]. Never clicks. Kept only because the
/// historical bench arms (A5/A6, `bench/arms/`) call it over MCP; see ADR 0003.
#[deprecated(since = "0.1.0", note = "use TOOL_GUARD; `act` never clicks")]
pub const TOOL_ACT: &str = "act";
pub const TOOL_DIFF: &str = "diff";
pub const TOOL_VERIFY: &str = "verify";

#[allow(deprecated)]
pub const TOOLS: [&str; 44] = [
    TOOL_OBSERVE,
    TOOL_LOCATE,
    TOOL_INSPECT,
    TOOL_GUARD,
    TOOL_ACT,
    TOOL_DIFF,
    TOOL_VERIFY,
    "browser_navigate",
    "browser_new_tab",
    "browser_go_back",
    "browser_wait",
    "browser_get_state",
    "browser_get_html",
    "browser_get_text",
    "browser_get_page_text",
    "browser_screenshot",
    "browser_scroll",
    "browser_scroll_to",
    "browser_click",
    "browser_right_click",
    "browser_middle_click",
    "browser_double_click",
    "browser_triple_click",
    "browser_hover",
    "browser_drag",
    "browser_type",
    "browser_form_input",
    "browser_send_key",
    "browser_hold_key",
    "browser_file_upload",
    "browser_save_as_pdf",
    "browser_get_dropdown_options",
    "browser_select_dropdown",
    "browser_search",
    "browser_zoom",
    "browser_find",
    "browser_list_tabs",
    "browser_switch_tab",
    "browser_close_tab",
    "browser_exec",
    "browser_javascript_exec",
    "browser_extract_content",
    "browser_read_console",
    "browser_read_network",
];

pub const PRODUCT_TOOLS: [&str; 3] = [TOOL_OBSERVE, TOOL_GUARD, TOOL_VERIFY];

mod browser_tools;
pub use browser_tools::BROWSER_TOOLS;

/// Tool name for a legacy protocol phase.
pub fn tool_for_phase(phase: ultra_instinct_protocol::LoopPhase) -> &'static str {
    use ultra_instinct_protocol::LoopPhase;
    match phase {
        LoopPhase::Observe => TOOL_OBSERVE,
        LoopPhase::Locate => TOOL_LOCATE,
        LoopPhase::Inspect => TOOL_INSPECT,
        LoopPhase::Act => TOOL_GUARD,
        LoopPhase::Diff => TOOL_DIFF,
        LoopPhase::Verify => TOOL_VERIFY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_protocol::{FirewallPhase, LoopPhase, FIREWALL_ORDER, LOOP_ORDER};

    #[test]
    fn product_tools_are_observe_guard_verify() {
        assert_eq!(PRODUCT_TOOLS, ["observe", "guard", "verify"]);
        for name in PRODUCT_TOOLS {
            assert!(TOOLS.contains(&name));
        }
        assert!(!PRODUCT_TOOLS.contains(&"navigate"));
        assert!(!PRODUCT_TOOLS.contains(&"act"));
        // The `browser_*` host surface sits beside the firewall tools; it
        // never enters PRODUCT_TOOLS.
        for name in crate::browser_tools::BROWSER_TOOLS {
            assert!(TOOLS.contains(&name));
            assert!(!PRODUCT_TOOLS.contains(&name));
        }
    }

    #[test]
    fn firewall_phases_match_product_tools() {
        let names: Vec<_> = FIREWALL_ORDER.iter().map(|p| p.as_str()).collect();
        assert_eq!(names, PRODUCT_TOOLS);
        assert_eq!(tool_for_phase(LoopPhase::Act), TOOL_GUARD);
        assert_eq!(tool_for_phase(LoopPhase::Observe), TOOL_OBSERVE);
        let _ = LOOP_ORDER;
        let _ = FirewallPhase::Guard;
    }
}
