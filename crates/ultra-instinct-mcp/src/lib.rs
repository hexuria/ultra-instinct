//! stdio MCP server for ultra-instinct.
//!
//! Product tools: observe, guard, verify. The locate, inspect, act and diff
//! helpers are removed (ADR 0012).
//!
//! `browser_*` tools are the browser-use-compatible host surface (navigate,
//! get_state, click, type, scroll, screenshot, tabs). Target-bound input still
//! goes through gate → ticket → `execute_ticketed`, so a stale page refuses
//! instead of clicking the wrong element; page-level calls are plain CDP.
//! They live in `ultra-instinct-browser-tools` and run on [`Server`] through
//! a crate-private `BrowserHost` adapter. `Server` itself does not implement
//! the trait, so its ledgers and buffers are not public API:
//!
//! ```compile_fail,E0277
//! fn host<T: ultra_instinct_browser_tools::BrowserHost>(_: T) {}
//! host(ultra_instinct_mcp::Server::default());
//! ```

#![forbid(unsafe_code)]

#[cfg(unix)]
mod pipe;
mod rpc;
mod server;
mod tools;

#[cfg(unix)]
pub use pipe::{PipeBackend, PIPE_ENDPOINT};
pub use rpc::{handle_line, serve_stdio};
pub use server::{CdpConnector, Server, ENV_ALLOW_DOMAINS, ENV_CONFIRM_PRESS, MAX_LIVE_SESSIONS};
pub use tools::call_tool;
pub use ultra_instinct_browser_tools::{
    sensitive, HttpBackend, PageTab, TargetBackend, ToolError, ERROR_CODES, MASK,
};

pub const TOOL_OBSERVE: &str = "observe";
pub const TOOL_GUARD: &str = "guard";
pub const TOOL_VERIFY: &str = "verify";

pub const TOOLS: [&str; 56] = [
    TOOL_OBSERVE,
    TOOL_GUARD,
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
    "browser_set",
    "browser_download",
    "browser_network",
    "browser_state_save",
    "browser_state_load",
    "browser_cookies",
    "browser_storage",
    "browser_dialog",
    "browser_go_forward",
    "browser_reload",
    "browser_check",
    "browser_focus",
    "browser_keyboard",
    "browser_get",
    "browser_is",
    "browser_count",
];

pub const PRODUCT_TOOLS: [&str; 3] = [TOOL_OBSERVE, TOOL_GUARD, TOOL_VERIFY];

pub use ultra_instinct_browser_tools::{BROWSER_TOOLS, ENV_TOOLS, EXTRA_TOOLS, STATE_TOOLS};

/// Tool name for a legacy protocol phase. The locate, inspect and diff tools
/// are removed, so their phases map onto the product tool that now covers
/// them: Locate and Inspect read the page with `observe` (regions carry id,
/// label, parent and state), Act is `guard`, and Diff is `verify` with
/// `before_snapshot` / `after_snapshot`.
pub fn tool_for_phase(phase: ultra_instinct_protocol::LoopPhase) -> &'static str {
    use ultra_instinct_protocol::LoopPhase;
    match phase {
        LoopPhase::Observe | LoopPhase::Locate | LoopPhase::Inspect => TOOL_OBSERVE,
        LoopPhase::Act => TOOL_GUARD,
        LoopPhase::Diff | LoopPhase::Verify => TOOL_VERIFY,
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
        for name in crate::BROWSER_TOOLS.into_iter().chain(crate::EXTRA_TOOLS) {
            assert!(TOOLS.contains(&name));
            assert!(!PRODUCT_TOOLS.contains(&name));
        }
        for name in crate::ENV_TOOLS {
            assert!(TOOLS.contains(&name));
            assert!(!PRODUCT_TOOLS.contains(&name));
        }
    }

    #[test]
    fn tools_are_product_tools_then_every_browser_tool_once() {
        // TOOLS lives here; the browser tool lists live in
        // ultra-instinct-browser-tools. Each browser tool appears exactly once.
        let browser: Vec<&str> = BROWSER_TOOLS
            .into_iter()
            .chain(EXTRA_TOOLS)
            .chain(ENV_TOOLS)
            .chain(STATE_TOOLS)
            .collect();
        let mut distinct = browser.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), browser.len(), "a browser tool listed twice");
        assert_eq!(TOOLS[..3], PRODUCT_TOOLS);
        assert_eq!(TOOLS.len(), PRODUCT_TOOLS.len() + browser.len());
        for name in &browser {
            assert_eq!(TOOLS.iter().filter(|n| *n == name).count(), 1, "{name}");
        }
    }

    #[test]
    fn firewall_phases_match_product_tools() {
        let names: Vec<_> = FIREWALL_ORDER.iter().map(|p| p.as_str()).collect();
        assert_eq!(names, PRODUCT_TOOLS);
        assert_eq!(tool_for_phase(LoopPhase::Act), TOOL_GUARD);
        assert_eq!(tool_for_phase(LoopPhase::Observe), TOOL_OBSERVE);
        assert_eq!(tool_for_phase(LoopPhase::Locate), TOOL_OBSERVE);
        assert_eq!(tool_for_phase(LoopPhase::Inspect), TOOL_OBSERVE);
        assert_eq!(tool_for_phase(LoopPhase::Diff), TOOL_VERIFY);
        assert_eq!(tool_for_phase(LoopPhase::Verify), TOOL_VERIFY);
        for removed in ["locate", "inspect", "act", "diff"] {
            assert!(!TOOLS.contains(&removed), "{removed}");
        }
        let _ = LOOP_ORDER;
        let _ = FirewallPhase::Guard;
    }
}
