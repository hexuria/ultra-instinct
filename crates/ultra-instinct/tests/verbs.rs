//! Shell verb parsing and rendering (`ultra_instinct::verbs`).

use serde_json::{json, Value};
use ultra_instinct::verbs::{parse_verb, render, Request, VERBS};

fn a(raw: &[&str]) -> Vec<String> {
    raw.iter().map(|s| (*s).to_owned()).collect()
}

fn tool(verb: &str, args: &[&str]) -> (&'static str, Value) {
    match parse_verb(verb, &a(args))
        .unwrap_or_else(|err| panic!("{verb} {args:?}: {err}"))
        .request
    {
        Request::Tool { name, arguments } => (name, arguments),
        Request::Stop => ("stop", Value::Null),
    }
}

fn reply(text: &str, is_error: bool) -> String {
    json!({"jsonrpc": "2.0", "id": 1, "result": {
        "content": [{"type": "text", "text": text}], "isError": is_error}})
    .to_string()
}

fn rendered(verb: &str, args: &[&str], body: Value) -> String {
    render(
        &parse_verb(verb, &a(args)).unwrap(),
        &reply(&body.to_string(), false),
    )
    .unwrap()
}

#[test]
fn verbs_map_to_browser_tools() {
    assert_eq!(
        tool("open", &["https://shop.example/"]),
        ("browser_navigate", json!({"url": "https://shop.example/"}))
    );
    assert_eq!(
        tool("snap", &[]),
        ("browser_get_state", json!({"format": "text"}))
    );
    assert_eq!(
        tool("snap", &["--json"]),
        ("browser_get_state", json!({"format": "json"}))
    );
    assert_eq!(
        tool("click", &["@e5", "--confirm", "Pay $18"]),
        (
            "browser_click",
            json!({"index": "@e5", "verify": true, "confirm": "Pay $18"})
        )
    );
    assert_eq!(
        tool("type", &["@e2", "kid@example.com"]),
        (
            "browser_type",
            json!({"index": "@e2", "text": "kid@example.com", "verify": true})
        )
    );
    assert_eq!(
        tool("select", &["3", "New", "Zealand"]),
        (
            "browser_select_dropdown",
            json!({"index": "3", "text": "New Zealand", "verify": true})
        )
    );
    assert_eq!(
        tool("key", &["@e2", "Enter"]),
        (
            "browser_send_key",
            json!({"index": "@e2", "key": "Enter", "verify": true})
        )
    );
    assert_eq!(
        tool("scroll", &["up"]),
        ("browser_scroll", json!({"down": false}))
    );
    assert_eq!(
        tool("scroll", &[]),
        ("browser_scroll", json!({"down": true}))
    );
    assert_eq!(
        tool("wait", &["--text", "Welcome", "--timeout", "2.5"]),
        ("browser_wait", json!({"text": "Welcome", "seconds": 2.5}))
    );
    assert_eq!(
        tool("wait", &["--load", "domcontentloaded"]),
        ("browser_wait", json!({"load": "domcontentloaded"}))
    );
    assert_eq!(
        tool("wait", &["--gone", "Loading…", "--load=load"]),
        ("browser_wait", json!({"gone": "Loading…", "load": "load"}))
    );
    assert_eq!(tool("tabs", &[]), ("browser_list_tabs", json!({})));
    assert_eq!(tool("close", &[]), ("stop", Value::Null));
    let (name, arguments) = tool("shot", &["out.png"]);
    assert_eq!(name, "browser_screenshot");
    let path = arguments["path"].as_str().unwrap();
    assert!(std::path::Path::new(path).is_absolute() && path.ends_with("out.png"));
    assert!(arguments.get("annotate").is_none());
}

#[test]
fn page_verbs_map_to_the_extra_tools() {
    let dialog = |args: &[&str]| tool("dialog", args);
    assert_eq!(dialog(&[]), ("browser_dialog", json!({"action": "status"})));
    assert_eq!(
        dialog(&["status"]),
        ("browser_dialog", json!({"action": "status"}))
    );
    assert_eq!(
        dialog(&["accept"]),
        ("browser_dialog", json!({"action": "accept"}))
    );
    assert_eq!(
        dialog(&["accept", "Kid Smith"]),
        (
            "browser_dialog",
            json!({"action": "accept", "text": "Kid Smith"})
        )
    );
    assert_eq!(
        dialog(&["dismiss"]),
        ("browser_dialog", json!({"action": "dismiss"}))
    );
    assert_eq!(tool("forward", &[]), ("browser_go_forward", json!({})));
    assert_eq!(
        tool("reload", &[]),
        ("browser_reload", json!({"ignore_cache": false}))
    );
    assert_eq!(
        tool("reload", &["--ignore-cache"]),
        ("browser_reload", json!({"ignore_cache": true}))
    );
    assert_eq!(
        tool("check", &["@e3"]),
        ("browser_check", json!({"index": "@e3", "checked": true}))
    );
    assert_eq!(
        tool("uncheck", &["3", "--mouse"]),
        (
            "browser_check",
            json!({"index": "3", "checked": false, "mouse": true})
        )
    );
    assert_eq!(
        tool("focus", &["@e1"]),
        ("browser_focus", json!({"index": "@e1"}))
    );
    assert_eq!(
        tool("keyboard", &["type", "hello"]),
        ("browser_keyboard", json!({"text": "hello", "mode": "type"}))
    );
    assert_eq!(
        tool("keyboard", &["type", "\n", "--confirm", "Pay $18"]),
        (
            "browser_keyboard",
            json!({"text": "\n", "mode": "type", "confirm": "Pay $18"})
        )
    );
    assert_eq!(
        tool("keyboard", &["insert", "日本"]),
        (
            "browser_keyboard",
            json!({"text": "日本", "mode": "insert"})
        )
    );
    for what in ["value", "label", "role", "box"] {
        assert_eq!(
            tool("get", &[what, "@e2"]),
            ("browser_get", json!({"index": "@e2", "what": what}))
        );
    }
    assert_eq!(
        tool("get", &["attr", "@e2", "href"]),
        (
            "browser_get",
            json!({"index": "@e2", "what": "attr", "name": "href"})
        )
    );
    for what in ["visible", "enabled", "checked", "editable"] {
        assert_eq!(
            tool("is", &[what, "4"]),
            ("browser_is", json!({"index": "4", "what": what}))
        );
    }
    assert_eq!(tool("count", &[]), ("browser_count", json!({})));
    assert_eq!(
        tool("count", &["--label", "Delete", "--role", "button"]),
        (
            "browser_count",
            json!({"label": "Delete", "role": "button"})
        )
    );
    let (name, arguments) = tool("shot", &["--annotate", "a.png"]);
    assert_eq!(name, "browser_screenshot");
    assert_eq!(arguments["annotate"], true);
    assert!(std::path::Path::new(arguments["path"].as_str().unwrap()).is_absolute());
}

#[test]
fn state_cookie_and_storage_verbs_map_to_their_tools() {
    let (name, arguments) = tool("state", &["save", "login.json"]);
    assert_eq!(name, "browser_state_save");
    let path = arguments["path"].as_str().unwrap();
    assert!(std::path::Path::new(path).is_absolute() && path.ends_with("login.json"));
    assert_eq!(
        tool("state", &["load", "/x/s.json"]).0,
        "browser_state_load"
    );
    assert_eq!(
        tool("state", &["load", "/x/s.json"]).1,
        json!({"path": "/x/s.json"})
    );
    assert_eq!(
        tool("cookies", &[]),
        ("browser_cookies", json!({"action": "list"}))
    );
    assert_eq!(
        tool(
            "cookies",
            &[
                "list",
                "--name",
                "sid",
                "--domain",
                "shop.example",
                "--reveal"
            ]
        ),
        (
            "browser_cookies",
            json!({"action": "list", "name": "sid", "domain": "shop.example", "reveal": true})
        )
    );
    assert_eq!(
        tool(
            "cookies",
            &[
                "set",
                "sid",
                "abc 1",
                "--domain",
                ".shop.example",
                "--path",
                "/app",
                "--expires",
                "1900000000",
                "--http-only",
                "--secure",
                "--same-site",
                "Lax"
            ]
        ),
        (
            "browser_cookies",
            json!({"action": "set", "name": "sid", "value": "abc 1",
                   "domain": ".shop.example", "path": "/app", "expires": 1900000000.0,
                   "http_only": true, "secure": true, "same_site": "Lax"})
        )
    );
    assert_eq!(
        tool("cookies", &["clear", "--name", "sid"]),
        ("browser_cookies", json!({"action": "clear", "name": "sid"}))
    );
    assert_eq!(
        tool("storage", &["local"]),
        (
            "browser_storage",
            json!({"area": "local", "action": "list"})
        )
    );
    assert_eq!(
        tool("storage", &["session", "list", "--reveal"]),
        (
            "browser_storage",
            json!({"area": "session", "action": "list", "reveal": true})
        )
    );
    assert_eq!(
        tool("storage", &["local", "get", "token"]),
        (
            "browser_storage",
            json!({"area": "local", "action": "get", "key": "token"})
        )
    );
    assert_eq!(
        tool("storage", &["local", "set", "theme", "dark"]),
        (
            "browser_storage",
            json!({"area": "local", "action": "set", "key": "theme", "value": "dark"})
        )
    );
    assert_eq!(
        tool("storage", &["session", "clear"]),
        (
            "browser_storage",
            json!({"area": "session", "action": "clear"})
        )
    );
    assert_eq!(
        tool("storage", &["session", "clear", "cart"]),
        (
            "browser_storage",
            json!({"area": "session", "action": "clear", "key": "cart"})
        )
    );
}

#[test]
fn env_verbs_map_to_set_download_and_network() {
    assert_eq!(
        tool("set", &["viewport", "1280", "720"]),
        (
            "browser_set",
            json!({"what": "viewport", "width": 1280, "height": 720})
        )
    );
    assert_eq!(
        tool("set", &["viewport", "390", "844", "3"]),
        (
            "browser_set",
            json!({"what": "viewport", "width": 390, "height": 844, "scale": 3.0})
        )
    );
    assert_eq!(
        tool("set", &["device", "iPhone 14"]),
        (
            "browser_set",
            json!({"what": "device", "name": "iPhone 14"})
        )
    );
    assert_eq!(
        tool("set", &["media", "dark"]),
        (
            "browser_set",
            json!({"what": "media", "color_scheme": "dark"})
        )
    );
    assert_eq!(
        tool("set", &["offline", "on"]),
        ("browser_set", json!({"what": "offline", "on": true}))
    );
    assert_eq!(
        tool("set", &["offline", "off"]),
        ("browser_set", json!({"what": "offline", "on": false}))
    );
    assert_eq!(
        tool("set", &["headers", "X-Test=1", "Accept-Language=en=US"]),
        (
            "browser_set",
            json!({"what": "headers", "map": {"X-Test": "1", "Accept-Language": "en=US"}})
        )
    );
    assert_eq!(
        tool("set", &["reset"]),
        ("browser_set", json!({"what": "reset"}))
    );
    let (name, arguments) = tool(
        "download",
        &["@e4", "out/r.pdf", "--overwrite", "--timeout-ms", "5000"],
    );
    assert_eq!(name, "browser_download");
    assert_eq!(arguments["index"], "@e4");
    assert_eq!(arguments["overwrite"], true);
    assert_eq!(arguments["timeout_ms"], 5000);
    assert!(std::path::Path::new(arguments["path"].as_str().unwrap()).is_absolute());
    assert_eq!(
        tool("download", &["2", "/tmp/r.pdf"]).1,
        json!({"index": "2", "path": "/tmp/r.pdf"})
    );
    assert_eq!(
        tool("network", &["block", "*.png", "*ads*"]),
        (
            "browser_network",
            json!({"action": "block", "patterns": ["*.png", "*ads*"]})
        )
    );
    assert_eq!(
        tool("network", &["unblock"]),
        ("browser_network", json!({"action": "unblock"}))
    );
    assert_eq!(
        tool("network", &["unblock", "*.png"]),
        (
            "browser_network",
            json!({"action": "unblock", "patterns": ["*.png"]})
        )
    );
    assert_eq!(
        tool("network", &["har", "start"]),
        ("browser_network", json!({"action": "har_start"}))
    );
    let (_, arguments) = tool("network", &["har", "stop", "run.har"]);
    assert_eq!(arguments["action"], "har_stop");
    assert!(std::path::Path::new(arguments["path"].as_str().unwrap()).is_absolute());
}

/// `--snap` prints the page after the input.
#[test]
fn snap_after_an_input_prints_the_page_after_it() {
    for (verb, args) in [
        ("click", &["@e5", "--snap"][..]),
        ("type", &["@e2", "rust", "--snap"][..]),
        ("open", &["https://a/", "--snap"][..]),
        ("scroll", &["--snap"][..]),
    ] {
        assert_eq!(tool(verb, args).1["state"], true, "{verb}");
    }
    let click = parse_verb("click", &a(&["@e5", "--snap"])).unwrap();
    let body = json!({"acted": "click", "ticket": 41, "verification": "navigation",
                      "state": "Done  http://h/\n@e1 link \"Home\"\n1 control · snapshot 8",
                      "snapshot": 8});
    let out = render(&click, &reply(&body.to_string(), false)).unwrap();
    assert_eq!(
        out,
        "✓ ticket 41 · navigation\nDone  http://h/\n@e1 link \"Home\"\n1 control · snapshot 8\n"
    );
    let failed = json!({"acted": "click", "ticket": 41,
                        "state_error": {"code": "dialog_open", "message": "a dialog is open"}});
    let out = render(&click, &reply(&failed.to_string(), false)).unwrap();
    assert!(out.ends_with("(a dialog is open); run aui snap\n"), "{out}");
    assert!(
        !out.contains('✗'),
        "the press ran: not an error line: {out}"
    );
}

#[test]
fn max_output_reaches_only_the_text_tools() {
    assert_eq!(
        tool("snap", &["--max-output", "400"]).1,
        json!({"format": "text", "max_chars": 400})
    );
    assert_eq!(
        tool("click", &["@e1", "--max-output=9"]).1,
        json!({"index": "@e1", "verify": true})
    );
    assert!(parse_verb("snap", &a(&["--max-output", "lots"])).is_err());
}

/// `--script` → `mouse: false`, `--mouse` → `true`, neither → no key, both → error.
#[test]
fn click_flags_pick_the_press() {
    assert_eq!(
        tool("click", &["@e1", "--script"]),
        (
            "browser_click",
            json!({"index": "@e1", "verify": true, "mouse": false})
        )
    );
    assert_eq!(
        tool("click", &["@e1", "--mouse"]),
        (
            "browser_click",
            json!({"index": "@e1", "verify": true, "mouse": true})
        )
    );
    let (_, arguments) = tool("click", &["@e1"]);
    assert_eq!(arguments, json!({"index": "@e1", "verify": true}));
    assert!(arguments.get("mouse").is_none(), "{arguments}");
    let err = parse_verb("click", &a(&["@e1", "--mouse", "--script"])).unwrap_err();
    assert!(err.to_string().contains("pick one"), "{err}");
}

#[test]
fn verb_flags_and_arity_are_checked() {
    let open = parse_verb(
        "open",
        &a(&[
            "https://shop.example/",
            "--session",
            "s1",
            "--cdp=http://127.0.0.1:9333",
            "--allow-domain",
            "shop.example",
            "--confirm-press",
            "Pay*",
        ]),
    )
    .unwrap();
    assert_eq!(open.session.as_str(), "s1");
    assert_eq!(open.cdp.as_deref(), Some("http://127.0.0.1:9333"));
    assert_eq!(open.allow_domains, ["shop.example"]);
    assert_eq!(open.confirm_press, ["Pay*"]);
    assert!(!open.launch);
    assert_eq!(parse_verb("snap", &[]).unwrap().cdp, None);
    let launch = parse_verb("open", &a(&["https://a/", "--launch", "--headed"])).unwrap();
    assert!(launch.launch && launch.headed);
    assert_eq!(launch.profile, None);
    let kept = parse_verb(
        "open",
        &a(&["https://a/", "--launch", "--profile", "/p/shop"]),
    )
    .unwrap();
    assert_eq!(kept.profile.as_deref(), Some("/p/shop"));
    let relative = parse_verb("open", &a(&["https://a/", "--launch", "--profile", "rel"])).unwrap();
    let expect = std::env::current_dir().unwrap().join("rel");
    assert_eq!(relative.profile.as_deref(), Some(expect.to_str().unwrap()));
    let everyday = ultra_instinct_browser::launch::default_user_data_dirs();
    let mut refused = vec![
        (vec!["--profile", "/p"], "--profile needs --launch"),
        (vec!["--launch", "--cdp", "http://x"], "pick one"),
        (
            vec!["--launch", "--cdp", "http://x", "--profile", "/p"],
            "pick one",
        ),
        (vec!["--launch", "--profile", ""], "needs a folder"),
        (vec!["--launch", "--profile", "  "], "needs a folder"),
    ];
    for (args, says) in [
        (&["--attach"][..], "name the sites with --allow-domain"),
        (&["--attach", "--launch"], "--attach and --launch: pick one"),
        (
            &["--attach", "--cdp", "http://x"],
            "--attach and --cdp: pick one",
        ),
        (
            &["--attach", "--profile", "/p"],
            "--attach and --profile: pick one",
        ),
        (&["--attach", "--headed"], "--attach and --headed: pick one"),
    ] {
        let err = parse_verb("open", &a(&[&["https://a/"], args].concat())).unwrap_err();
        assert!(err.to_string().contains(says), "{args:?}: {err}");
    }
    let dir = everyday.first().map(|dir| format!("{}/", dir.display()));
    if let Some(dir) = &dir {
        refused.push((vec!["--launch", "--profile", dir], "separate folder"));
    }
    for (args, says) in refused {
        let err = parse_verb("open", &a(&[&["https://a/"], &args[..]].concat()));
        let err = err.unwrap_err().to_string();
        assert!(err.contains(says), "{args:?}: {err}");
    }
    assert_eq!(launch.idle_secs, None);
    let idle = parse_verb("open", &a(&["https://a/", "--launch", "--idle", "90"])).unwrap();
    assert_eq!(idle.idle_secs, Some(90));
    let max = parse_verb("open", &a(&["https://a/", "--idle=86400"])).unwrap();
    assert_eq!(max.idle_secs, Some(86_400));
    for bad in ["0", "86401", "-1", "1.5", "soon"] {
        let err = parse_verb("open", &a(&["https://a/", "--idle", bad])).unwrap_err();
        assert!(err.to_string().contains("--idle"), "{bad}: {err}");
    }
    for (verb, args) in [
        ("snap", &["--idle", "5"][..]),
        ("open", &["https://a/", "--idle", "5", "--idle", "6"][..]),
        ("open", &[][..]),
        ("open", &["https://a/", "--headed"][..]),
        ("snap", &["--launch"][..]),
        ("click", &[][..]),
        ("type", &["@e1"][..]),
        ("key", &["Enter"][..]),
        ("scroll", &["left"][..]),
        ("wait", &[][..]),
        ("wait", &["--timeout", "3"][..]),
        ("wait", &["--timeout", "x", "--url", "/a"][..]),
        ("wait", &["--load", "networkidle"][..]),
        ("wait", &["--gone"][..]),
        ("wait", &["--url", "/a", "--url", "/b"][..]),
        ("snap", &["--confirm", "x"][..]),
        ("snap", &["--json", "--json"][..]),
        ("click", &["@e1", "--mouse=yes"][..]),
        ("click", &["@e1", "--allow-domain", "x"][..]),
        ("open", &["https://a/", "--allow-domain", "https://a/"][..]),
        ("snap", &["--session", "bad name"][..]),
        ("tabs", &["extra"][..]),
        ("dialog", &["maybe"][..]),
        ("reload", &["--mouse"][..]),
        ("check", &[][..]),
        ("focus", &["@e1", "--mouse"][..]),
        ("snap", &["--snap"][..]),
        ("get", &["value", "@e1", "--snap"][..]),
        ("keyboard", &["type"][..]),
        ("keyboard", &["press", "x"][..]),
        ("get", &["attr", "@e1"][..]),
        ("get", &["text", "@e1"][..]),
        ("is", &["hidden", "@e1"][..]),
        ("count", &["Delete"][..]),
        ("state", &["save"][..]),
        ("state", &["dump", "x"][..]),
        ("cookies", &["set", "sid"][..]),
        ("cookies", &["list", "--path", "/"][..]),
        ("cookies", &["clear", "--reveal"][..]),
        ("cookies", &["set", "a", "b", "--expires", "soon"][..]),
        ("storage", &[][..]),
        ("storage", &["cookie"][..]),
        ("storage", &["local", "get"][..]),
        ("storage", &["local", "get", "k", "--reveal"][..]),
        ("set", &["viewport", "wide", "720"][..]),
        ("set", &["viewport", "1", "2", "3", "4"][..]),
        ("set", &["device"][..]),
        ("set", &["media", "sepia"][..]),
        ("set", &["offline", "yes"][..]),
        ("set", &["headers"][..]),
        ("set", &["headers", "novalue"][..]),
        ("set", &["headers", "=v"][..]),
        ("download", &["@e1"][..]),
        ("download", &["@e1", "x", "--timeout-ms", "-1"][..]),
        ("network", &["block"][..]),
        ("network", &["har"][..]),
        ("network", &["har", "stop"][..]),
    ] {
        assert!(parse_verb(verb, &a(args)).is_err(), "{verb} {args:?}");
    }
    assert!(parse_verb("fly", &[]).is_err());
}

#[test]
fn refusals_render_short_and_fail() {
    let click = parse_verb("click", &a(&["@e5"])).unwrap();
    let ok = render(
        &click,
        &reply(
            r#"{"acted":"click","ticket":41,"verification":"navigation"}"#,
            false,
        ),
    )
    .unwrap();
    assert_eq!(ok, "✓ ticket 41 · navigation\n");
    let stale = render(
        &click,
        &reply(
            r#"{"variant":"TicketInvalid","message":"ticket target-gone"}"#,
            true,
        ),
    )
    .unwrap_err();
    assert_eq!(stale.to_string(), "✗ refused: TargetGone · run aui snap");
    let confirm = render(
        &click,
        &reply(
            r#"{"variant":"TicketInvalid","message":"needs-confirm: \"Pay $18\""}"#,
            true,
        ),
    )
    .unwrap_err();
    assert_eq!(
        confirm.to_string(),
        "✗ refused: NeedsConfirm \"Pay $18\" · rerun with --confirm \"Pay \\$18\""
    );
    let gate = render(
        &click,
        &reply(r#"{"variant":"TicketInvalid","message":"occluded"}"#, true),
    )
    .unwrap_err();
    assert_eq!(gate.to_string(), "✗ refused: Occluded · run aui snap");
    let other = render(
        &click,
        &reply(r#"{"variant":"Browser","message":"socket closed"}"#, true),
    )
    .unwrap_err();
    assert_eq!(other.to_string(), "✗ error: socket closed");
    // A daemon-level error (a failed Chrome launch) is shown unchanged.
    let launch = render(
        &click,
        &json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "no Chrome found"}})
            .to_string(),
    )
    .unwrap_err();
    assert_eq!(launch.to_string(), "✗ error: no Chrome found");
}

/// Over MCP a screenshot answers with the image first, then its
/// metadata as text.
#[test]
fn a_screenshot_reply_renders_from_its_text_item() {
    let shot = parse_verb("shot", &a(&["/x/s.png"])).unwrap();
    let reply = json!({"jsonrpc": "2.0", "id": 1, "result": {"content": [
        {"type": "image", "data": "iVBO", "mimeType": "image/png"},
        {"type": "text", "text": r#"{"format":"png","path":"/x/s.png","bytes":4}"#}],
        "isError": false}});
    assert_eq!(
        render(&shot, &reply.to_string()).unwrap(),
        "✓ saved /x/s.png (4 bytes)\n"
    );
}

#[test]
fn new_tool_replies_render_one_short_line() {
    assert_eq!(
        rendered(
            "dialog",
            &[],
            json!({"type": null, "open": false, "tracked": true})
        ),
        "✓ no dialog open\n"
    );
    assert_eq!(
        rendered(
            "dialog",
            &[],
            json!({"type": "confirm", "message": "Delete?", "open": true, "tracked": true})
        ),
        "✓ confirm dialog open · \"Delete?\"\n"
    );
    assert_eq!(
        rendered(
            "dialog",
            &["accept"],
            json!({"type": "prompt", "handled": "accept", "open": false})
        ),
        "✓ accepted prompt dialog\n"
    );
    assert_eq!(
        rendered("forward", &[], json!({"went_forward": true})),
        "✓ went forward\n"
    );
    assert_eq!(
        rendered(
            "check",
            &["@e3"],
            json!({"changed": true, "checked": true, "index": 3})
        ),
        "✓ checked @e3\n"
    );
    assert_eq!(
        rendered(
            "uncheck",
            &["@e3"],
            json!({"changed": false, "checked": false, "index": 3})
        ),
        "✓ already unchecked @e3\n"
    );
    assert_eq!(
        rendered("focus", &["2"], json!({"focused": 2})),
        "✓ focused @e2\n"
    );
    assert_eq!(
        rendered(
            "keyboard",
            &["insert", "hi"],
            json!({"typed": 2, "mode": "insert"})
        ),
        "✓ inserted 2 chars\n"
    );
    assert_eq!(
        rendered(
            "get",
            &["value", "@e1"],
            json!({"value": "kid@example.com"})
        ),
        "kid@example.com\n"
    );
    assert_eq!(
        rendered("get", &["value", "@e1"], json!({"value": null})),
        "(none)\n"
    );
    assert_eq!(
        rendered(
            "get",
            &["box", "@e1"],
            json!({"value": {"x": 20.0, "y": 20.0, "w": 300.0, "h": 28.0}})
        ),
        "20.0 20.0 300.0 28.0\n"
    );
    assert_eq!(
        rendered("is", &["visible", "@e1"], json!({"value": false})),
        "false\n"
    );
    assert_eq!(
        rendered("count", &[], json!({"count": 2, "indexes": [1, 4]})),
        "2  @e1 @e4\n"
    );
    assert_eq!(
        rendered("count", &[], json!({"count": 0, "indexes": []})),
        "0\n"
    );
    let load = rendered(
        "state",
        &["load", "/s.json"],
        json!({"path": "/s.json", "cookies": 3, "applied_origin": "https://a.example",
               "pending_origins": ["https://b.example"], "hint": "navigate there"}),
    );
    assert_eq!(
        load,
        "✓ loaded state /s.json · 3 cookies · storage for https://a.example\npending origins: https://b.example\nnavigate there\n"
    );
    assert_eq!(
        rendered(
            "cookies",
            &[],
            json!({"count": 1, "cookies": [{"name": "sid", "value": "•••", "domain": "a.example",
                   "path": "/", "http_only": true, "secure": false}]})
        ),
        "sid=•••  a.example/  http-only\n1 cookies\n"
    );
    assert_eq!(
        rendered("cookies", &["clear"], json!({"cleared": "all"})),
        "✓ cleared all cookies\n"
    );
    assert_eq!(
        rendered(
            "storage",
            &["local"],
            json!({"origin": "https://a.example", "area": "local", "count": 1,
                   "entries": {"theme": {"value": "•••", "value_length": 4}}})
        ),
        "theme=•••\n1 local keys · https://a.example\n"
    );
    assert_eq!(
        rendered(
            "set",
            &["viewport", "1280", "720"],
            json!({"set": "viewport", "width": 1280, "height": 720, "scale": 1.0})
        ),
        "✓ viewport 1280x720 @1.0\n"
    );
    assert_eq!(
        rendered(
            "download",
            &["@e2", "/r.pdf"],
            json!({"path": "/r.pdf", "bytes": 5})
        ),
        "✓ saved /r.pdf (5 bytes)\n"
    );
    assert_eq!(
        rendered(
            "network",
            &["block", "*.png"],
            json!({"blocked": ["*.png"]})
        ),
        "✓ blocking *.png\n"
    );
    assert_eq!(
        rendered(
            "network",
            &["har", "stop", "/r.har"],
            json!({"path": "/r.har", "entries": 7})
        ),
        "✓ saved HAR /r.har · 7 entries\n"
    );
}

/// A verb and arguments that parse to the tool, or why the tool has no verb.
type VerbOrReason = Result<(&'static str, &'static [&'static str]), &'static str>;

/// Every `browser_*` tool: the verb call that reaches it, or why it has none.
/// A tool added to `ultra_instinct_mcp::TOOLS` with neither fails
/// `every_browser_tool_has_a_verb_or_a_reason`.
const TOOL_VERBS: [(&str, VerbOrReason); 53] = [
    ("browser_navigate", Ok(("open", &["https://shop.example/"]))),
    ("browser_get_state", Ok(("snap", &[]))),
    ("browser_click", Ok(("click", &["@e1"]))),
    ("browser_type", Ok(("type", &["@e1", "x"]))),
    ("browser_select_dropdown", Ok(("select", &["@e1", "x"]))),
    ("browser_send_key", Ok(("key", &["@e1", "Enter"]))),
    ("browser_scroll", Ok(("scroll", &[]))),
    ("browser_wait", Ok(("wait", &["--text", "x"]))),
    ("browser_screenshot", Ok(("shot", &[]))),
    ("browser_list_tabs", Ok(("tabs", &[]))),
    ("browser_dialog", Ok(("dialog", &[]))),
    ("browser_go_forward", Ok(("forward", &[]))),
    ("browser_reload", Ok(("reload", &[]))),
    ("browser_check", Ok(("check", &["@e1"]))),
    ("browser_focus", Ok(("focus", &["@e1"]))),
    ("browser_keyboard", Ok(("keyboard", &["type", "x"]))),
    ("browser_get", Ok(("get", &["value", "@e1"]))),
    ("browser_is", Ok(("is", &["visible", "@e1"]))),
    ("browser_count", Ok(("count", &[]))),
    (
        "browser_state_save",
        Ok(("state", &["save", "/tmp/s.json"])),
    ),
    (
        "browser_state_load",
        Ok(("state", &["load", "/tmp/s.json"])),
    ),
    ("browser_cookies", Ok(("cookies", &[]))),
    ("browser_storage", Ok(("storage", &["local"]))),
    ("browser_set", Ok(("set", &["offline", "on"]))),
    ("browser_download", Ok(("download", &["@e1", "/tmp/f.bin"]))),
    ("browser_network", Ok(("network", &["har", "start"]))),
    ("browser_go_back", Ok(("back", &[]))),
    ("browser_hover", Ok(("hover", &["@e1"]))),
    ("browser_drag", Ok(("drag", &["@e1", "@e2"]))),
    (
        "browser_file_upload",
        Ok(("upload", &["@e1", "/tmp/f.txt"])),
    ),
    ("browser_get_text", Ok(("text", &[]))),
    ("browser_extract_content", Ok(("read", &[]))),
    ("browser_get_html", Ok(("html", &[]))),
    ("browser_find", Ok(("find", &["sign"]))),
    ("browser_save_as_pdf", Ok(("pdf", &["/tmp/p.pdf"]))),
    ("browser_new_tab", Ok(("tab", &["new"]))),
    ("browser_switch_tab", Ok(("tab", &["switch", "AB12CD34"]))),
    ("browser_close_tab", Ok(("tab", &["close", "AB12CD34"]))),
    ("browser_exec", Ok(("eval", &["1+1"]))),
    ("browser_read_console", Ok(("console", &[]))),
    ("browser_read_network", Ok(("requests", &[]))),
    ("browser_double_click", Ok(("dblclick", &["@e1"]))),
    ("browser_right_click", Ok(("rightclick", &["@e1"]))),
    ("browser_middle_click", Ok(("midclick", &["@e1"]))),
    ("browser_triple_click", Ok(("tripleclick", &["@e1"]))),
    ("browser_hold_key", Ok(("hold", &["ArrowDown"]))),
    ("browser_scroll_to", Ok(("scrollto", &["@e1"]))),
    ("browser_zoom", Ok(("zoom", &["in"]))),
    ("browser_get_dropdown_options", Ok(("options", &["@e1"]))),
    ("browser_get_page_text", Err("alias of browser_get_text")),
    ("browser_form_input", Err("alias of browser_type")),
    ("browser_javascript_exec", Err("alias of browser_exec")),
    ("browser_search", Err("a search URL through `open`")),
];

#[test]
fn every_browser_tool_has_a_verb_or_a_reason() {
    for name in ultra_instinct_mcp::TOOLS {
        if ultra_instinct_mcp::PRODUCT_TOOLS.contains(&name) {
            continue;
        }
        let rows: Vec<_> = TOOL_VERBS
            .iter()
            .filter(|(tool, _)| *tool == name)
            .collect();
        assert_eq!(
            rows.len(),
            1,
            "{name}: add one TOOL_VERBS row (a verb or a reason)"
        );
    }
    for (name, entry) in TOOL_VERBS {
        assert!(
            ultra_instinct_mcp::TOOLS.contains(&name),
            "{name} is not a tool"
        );
        match entry {
            Ok((verb, args)) => assert_eq!(tool(verb, args).0, name, "aui {verb} {args:?}"),
            Err(reason) => assert!(!reason.is_empty(), "{name}: say why it has no verb"),
        }
    }
}

#[test]
fn every_verb_reaches_a_tool_in_the_table() {
    for verb in VERBS {
        let reached = TOOL_VERBS
            .iter()
            .any(|(_, entry)| matches!(entry, Ok((v, _)) if *v == verb));
        // `uncheck` is `check` with `checked: false`; `close` stops the daemon.
        assert!(
            reached || ["uncheck", "close"].contains(&verb),
            "aui {verb}: add its tool to TOOL_VERBS"
        );
    }
}

#[test]
fn back_hover_drag_upload_parse_to_their_tools() {
    assert_eq!(tool("back", &[]), ("browser_go_back", json!({})));
    assert_eq!(
        tool("hover", &["@e2"]),
        ("browser_hover", json!({"index": "@e2"}))
    );
    assert_eq!(
        tool("drag", &["@e3", "@e5", "--confirm", "Bin"]),
        (
            "browser_drag",
            json!({"from_index": "@e3", "to_index": "@e5", "confirm": "Bin"})
        )
    );
    assert_eq!(
        tool("upload", &["@e4", "/tmp/a.txt", "/tmp/b.txt"]),
        (
            "browser_file_upload",
            json!({"index": "@e4", "paths": ["/tmp/a.txt", "/tmp/b.txt"]})
        )
    );
    // A relative path is this shell's: the daemon may run elsewhere.
    let (_, arguments) = tool("upload", &["@e4", "a.txt"]);
    let sent = arguments["paths"][0].as_str().unwrap();
    assert!(
        std::path::Path::new(sent).is_absolute() && sent.ends_with("/a.txt"),
        "{sent}"
    );
    for (verb, args) in [
        ("back", &["x"][..]),
        ("hover", &[]),
        ("drag", &["@e1"]),
        ("upload", &["@e1"]),
    ] {
        let err = parse_verb(verb, &a(args)).err().unwrap().to_string();
        assert!(err.contains(&format!("usage: aui {verb}")), "{err}");
    }
}

#[test]
fn back_hover_drag_upload_render_one_short_line() {
    let line = |verb: &str, args: &[&str], body: Value| {
        let verb = parse_verb(verb, &a(args)).unwrap();
        render(&verb, &reply(&body.to_string(), false)).unwrap()
    };
    assert_eq!(
        line("back", &[], json!({"went_back": true})),
        "✓ went back\n"
    );
    assert_eq!(
        line("hover", &["@e2"], json!({"hovered": 2})),
        "✓ hovering @e2\n"
    );
    assert_eq!(
        line(
            "drag",
            &["@e3", "@e5"],
            json!({"dragged": {"from": 3, "to": 5}})
        ),
        "✓ dragged @e3 to @e5\n"
    );
    assert_eq!(
        line(
            "upload",
            &["@e4", "/tmp/a.txt", "/tmp/b.txt"],
            json!({"acted": true, "index": 4, "ticket": 9})
        ),
        "✓ ticket 9 · 2 files set on @e4\n"
    );
}

#[test]
fn text_read_html_find_pdf_parse_to_their_tools() {
    assert_eq!(tool("text", &[]), ("browser_get_text", json!({})));
    assert_eq!(tool("read", &[]), ("browser_extract_content", json!({})));
    assert_eq!(tool("html", &[]), ("browser_get_html", json!({})));
    assert_eq!(
        tool("find", &["sign", "--role", "button"]),
        ("browser_find", json!({"text": "sign", "role": "button"}))
    );
    assert_eq!(
        tool("find", &["--role", "link"]),
        ("browser_find", json!({"role": "link"}))
    );
    assert_eq!(
        tool("pdf", &["/tmp/p.pdf", "--landscape", "--scale", "0.5"]),
        (
            "browser_save_as_pdf",
            json!({"path": "/tmp/p.pdf", "landscape": true, "print_background": false, "scale": 0.5})
        )
    );
    for (verb, args) in [
        ("text", &["x"][..]),
        ("read", &["x"]),
        ("html", &["x"]),
        ("find", &[]),
        ("find", &["a", "b"]),
        ("pdf", &[]),
    ] {
        let err = parse_verb(verb, &a(args)).err().unwrap().to_string();
        assert!(err.contains(&format!("usage: aui {verb}")), "{err}");
    }
    let err = parse_verb("pdf", &a(&["p.pdf", "--scale", "big"]))
        .err()
        .unwrap();
    assert!(err.to_string().contains("--scale"), "{err}");
}

#[test]
fn text_read_html_find_pdf_print_the_tool_reply() {
    let line = |verb: &str, args: &[&str], body: Value| {
        let verb = parse_verb(verb, &a(args)).unwrap();
        render(&verb, &reply(&body.to_string(), false)).unwrap()
    };
    assert_eq!(
        line("text", &[], json!({"text": "Hello\nthere"})),
        "Hello\nthere\n"
    );
    assert_eq!(
        line("read", &[], json!({"content": "# Hello"})),
        "# Hello\n"
    );
    assert_eq!(
        line("html", &[], json!({"html": "<html></html>"})),
        "<html></html>\n"
    );
    assert_eq!(
        line(
            "find",
            &["sign"],
            json!({"matched": 1, "matches": [{"index": 2, "role": "button", "label": "Sign in"}]})
        ),
        "@e2 button \"Sign in\"\n1 found\n"
    );
    assert_eq!(
        line("find", &["zzz"], json!({"matched": 0, "matches": []})),
        "0 found\n"
    );
    assert_eq!(
        line(
            "pdf",
            &["/tmp/p.pdf"],
            json!({"data": "JVBERg==", "path": "/tmp/p.pdf", "bytes": 4})
        ),
        "✓ saved /tmp/p.pdf (4 bytes)\n"
    );
}

#[test]
fn tab_eval_console_requests_parse_to_their_tools() {
    assert_eq!(tool("tab", &["new"]), ("browser_new_tab", json!({})));
    assert_eq!(
        tool("tab", &["new", "https://example.com"]),
        ("browser_new_tab", json!({"url": "https://example.com"}))
    );
    assert_eq!(
        tool("tab", &["switch", "AB12CD34"]),
        ("browser_switch_tab", json!({"tab_id": "AB12CD34"}))
    );
    assert_eq!(
        tool("tab", &["close", "AB12CD34"]),
        ("browser_close_tab", json!({"tab_id": "AB12CD34"}))
    );
    assert_eq!(
        tool("eval", &["document.title"]),
        ("browser_exec", json!({"script": "document.title"}))
    );
    assert_eq!(tool("console", &[]), ("browser_read_console", json!({})));
    assert_eq!(tool("requests", &[]), ("browser_read_network", json!({})));
    for (verb, args) in [
        ("tab", &[][..]),
        ("tab", &["switch"]),
        ("tab", &["close"]),
        ("tab", &["open", "x"]),
        ("eval", &[]),
        ("eval", &["a", "b"]),
        ("console", &["x"]),
        ("requests", &["x"]),
    ] {
        let err = parse_verb(verb, &a(args)).err().unwrap().to_string();
        assert!(err.contains(&format!("usage: aui {verb}")), "{err}");
    }
}

#[test]
fn tab_eval_console_requests_print_short_lines() {
    let line = |verb: &str, args: &[&str], body: Value| {
        let verb = parse_verb(verb, &a(args)).unwrap();
        render(&verb, &reply(&body.to_string(), false)).unwrap()
    };
    assert_eq!(
        line(
            "tab",
            &["new", "https://example.com"],
            json!({"tab_id": "AB12CD34EF56", "url": "https://example.com", "new_tab": true})
        ),
        "✓ new tab AB12CD34 · https://example.com\n"
    );
    assert_eq!(
        line(
            "tab",
            &["switch", "AB12"],
            json!({"active_tab": "AB12CD34EF56", "url": "https://example.com"})
        ),
        "✓ on tab AB12CD34 · https://example.com\n"
    );
    assert_eq!(
        line(
            "tab",
            &["close", "AB12"],
            json!({"closed": true, "tab_id": "AB12CD34EF56"})
        ),
        "✓ closed tab AB12CD34\n"
    );
    assert_eq!(
        line("eval", &["document.title"], json!({"result": "Home"})),
        "Home\n"
    );
    assert_eq!(
        line("eval", &["({a:1})"], json!({"result": {"a": 1}})),
        "{\"a\":1}\n"
    );
    assert_eq!(line("eval", &["void 0"], json!({"result": null})), "null\n");
    assert_eq!(
        line(
            "console",
            &[],
            json!({"dropped": 0, "entries": [
                {"level": "log", "text": "\"hi\" 2"},
                {"level": "error", "text": "boom", "source": "network"}
            ]})
        ),
        "log  \"hi\" 2\nerror  boom\n2 entries\n"
    );
    assert_eq!(
        line(
            "requests",
            &[],
            json!({"dropped": 0, "entries": [
                {"requestId": "1", "entry": {"method": "GET", "url": "http://a/x", "status": 200}},
                {"requestId": "2", "entry": {"method": "POST", "url": "http://a/y"}},
                {"requestId": "3", "entry": {"method": "GET", "url": "http://a/z", "error": "net::ERR_FAILED"}}
            ]})
        ),
        "GET  200  http://a/x\nPOST  -  http://a/y\nGET  net::ERR_FAILED  http://a/z\n3 requests\n"
    );
    assert_eq!(
        line("requests", &[], json!({"dropped": 0, "entries": []})),
        "0 requests\n"
    );
    assert_eq!(
        line(
            "console",
            &["--max-output", "5"],
            json!({"dropped": 0, "entries": [], "truncated": "… [truncated 40 chars]"})
        ),
        "0 entries · … [truncated 40 chars]\n"
    );
    // Events the buffer lost are named: the list is not the whole story.
    assert_eq!(
        line("requests", &[], json!({"dropped": 7, "entries": []})),
        "0 requests · 7 dropped\n"
    );
    assert_eq!(
        line(
            "console",
            &[],
            json!({"dropped": 3, "entries": [{"level": "log", "text": "hi"}]})
        ),
        "log  hi\n1 entries · 3 dropped\n"
    );
}

#[test]
fn click_key_and_view_verbs_parse_to_their_tools() {
    for (verb, name) in [
        ("dblclick", "browser_double_click"),
        ("rightclick", "browser_right_click"),
        ("midclick", "browser_middle_click"),
        ("tripleclick", "browser_triple_click"),
    ] {
        assert_eq!(tool(verb, &["@e3"]), (name, json!({"index": "@e3"})));
        assert_eq!(
            tool(verb, &["@e3", "--confirm", "Delete"]),
            (name, json!({"index": "@e3", "confirm": "Delete"}))
        );
        let err = parse_verb(verb, &a(&[])).err().unwrap().to_string();
        assert!(err.contains(&format!("usage: aui {verb} <ref>")), "{err}");
    }
    assert_eq!(
        tool("hold", &["ArrowDown"]),
        ("browser_hold_key", json!({"key": "ArrowDown"}))
    );
    assert_eq!(
        tool("hold", &["ArrowDown", "--ms", "900", "--confirm", "Go"]),
        (
            "browser_hold_key",
            json!({"key": "ArrowDown", "hold_ms": 900, "confirm": "Go"})
        )
    );
    assert_eq!(
        tool("scrollto", &["@e9"]),
        ("browser_scroll_to", json!({"index": "@e9"}))
    );
    assert_eq!(
        tool("zoom", &["in"]),
        ("browser_zoom", json!({"direction": "in"}))
    );
    assert_eq!(
        tool("zoom", &["1.5"]),
        ("browser_zoom", json!({"factor": 1.5}))
    );
    assert_eq!(
        tool("options", &["@e5"]),
        ("browser_get_dropdown_options", json!({"index": "@e5"}))
    );
    for (verb, args) in [
        ("hold", &[][..]),
        ("hold", &["a", "b"]),
        ("scrollto", &[]),
        ("zoom", &[]),
        ("zoom", &["big"]),
        ("zoom", &["NaN"]),
        ("zoom", &["1e309"]),
        ("zoom", &["0"]),
        ("zoom", &["-2"]),
        ("options", &[]),
    ] {
        let err = parse_verb(verb, &a(args)).err().unwrap().to_string();
        assert!(err.contains(&format!("usage: aui {verb}")), "{err}");
    }
    let err = parse_verb("hold", &a(&["a", "--ms", "long"]))
        .err()
        .unwrap();
    assert!(err.to_string().contains("--ms"), "{err}");
}

#[test]
fn click_key_and_view_verbs_print_one_short_line() {
    let line = |verb: &str, args: &[&str], body: Value| {
        let verb = parse_verb(verb, &a(args)).unwrap();
        render(&verb, &reply(&body.to_string(), false)).unwrap()
    };
    for verb in ["dblclick", "rightclick", "midclick", "tripleclick"] {
        assert_eq!(
            line(
                verb,
                &["@e3"],
                json!({"acted": true, "index": 3, "ticket": 7})
            ),
            "✓ ticket 7\n"
        );
    }
    assert_eq!(
        line("hold", &["ArrowDown"], json!({"key": "ArrowDown"})),
        "✓ held ArrowDown\n"
    );
    assert_eq!(
        line("scrollto", &["@e9"], json!({"scrolled": true, "index": 9})),
        "✓ scrolled to @e9\n"
    );
    assert_eq!(
        line("zoom", &["in"], json!({"zoomed": 1.25})),
        "✓ zoom 1.25\n"
    );
    assert_eq!(
        line(
            "options",
            &["@e5"],
            json!({"index": 5, "options": ["Small", "Large"], "selected": "Large"})
        ),
        "  Small\n* Large\n2 options\n"
    );
    // A multi-select joins its selected labels; a label may hold ", " itself.
    assert_eq!(
        line(
            "options",
            &["@e5"],
            json!({"index": 5, "options": ["A", "B", "C"], "selected": "A, C"})
        ),
        "* A\n  B\n* C\n3 options\n"
    );
    assert_eq!(
        line(
            "options",
            &["@e5"],
            json!({"index": 5, "options": ["Doe, Jane", "Doe"], "selected": "Doe, Jane"})
        ),
        "* Doe, Jane\n  Doe\n2 options\n"
    );
    assert_eq!(
        line(
            "options",
            &["@e5"],
            json!({"index": 5, "options": [], "selected": null})
        ),
        "0 options\n"
    );
}

/// P2.6: the `core` skill and `docs/tools.md` name every verb, as code.
#[test]
fn the_core_skill_and_the_tools_doc_name_every_verb() {
    let skill = ultra_instinct::execute(&a(&["skills", "get", "core"])).unwrap();
    let doc = include_str!("../../../docs/tools.md");
    for verb in VERBS {
        for (name, text) in [("skills get core", skill.as_str()), ("docs/tools.md", doc)] {
            let named = text.contains(&format!("`{verb}`")) || text.contains(&format!("`{verb} "));
            assert!(named, "{name} does not name `{verb}`");
        }
    }
}
