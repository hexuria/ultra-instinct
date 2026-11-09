//! `browser_file_upload` hands the page only files the host allows: every
//! path absolute (`path_not_absolute`), none at all under `Uploads::Off`
//! (`uploads_off`), and under `Uploads::Dir` only files inside that folder
//! once `..` and symlinks are resolved (`path_not_allowed`). Each refusal
//! comes before any CDP call that drives the page.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::{Server, ToolError, Uploads};

const CDP: &str = "http://127.0.0.1:9222";

type Log = Rc<RefCell<Vec<(String, Value)>>>;

struct Logged {
    inner: ReplayTransport,
    log: Log,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let params = serde_json::from_str(params_json).unwrap_or(Value::Null);
        self.log.borrow_mut().push((method.to_owned(), params));
        self.inner.call(method, params_json)
    }
}

/// A file input @e1; the script observes it and sets its files.
fn replay(uploads: Uploads) -> (Server, Log) {
    let page = PageSpec::of(
        &[Control::button(
            10,
            100,
            "Resume",
            (20.0, 20.0, 200.0, 28.0),
        )],
        "http://127.0.0.1/apply",
        "Apply",
    );
    let script = ScriptBuilder::new()
        .observe_compact(&page)
        .call("DOM.setFileInputFiles", json!({}));
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::clone(&log);
    let mut script = Some(script);
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    })
    .with_uploads(uploads);
    (server, log)
}

fn upload(server: &mut Server, paths: &[&Path]) -> Result<Value, ToolError> {
    let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    server.call_tool(
        "browser_file_upload",
        &json!({"cdp": CDP, "index": 1, "paths": paths}),
    )
}

/// The calls that drive the page: everything but the Page domain each
/// browser_* call enables.
fn driving(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .map(|(m, _)| m.clone())
        .filter(|m| m != "Page.enable")
        .collect()
}

fn files_set(log: &Log) -> Vec<Value> {
    log.borrow()
        .iter()
        .filter(|(m, _)| m == "DOM.setFileInputFiles")
        .map(|(_, params)| params["files"].clone())
        .collect()
}

/// A fresh folder `name` with `inside/cv.pdf`, `outside.pdf` next to
/// `inside`, and `inside/link.pdf` → `outside.pdf`.
fn tree(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("ui-uploads-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let inside = root.join("inside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::write(inside.join("cv.pdf"), b"%PDF").unwrap();
    std::fs::write(root.join("outside.pdf"), b"%PDF").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("outside.pdf"), inside.join("link.pdf")).unwrap();
    (root.clone(), inside, root.join("outside.pdf"))
}

#[test]
fn a_relative_path_is_refused_before_any_cdp_call() {
    let (mut server, log) = replay(Uploads::Anywhere);
    for paths in [
        json!({"path": "cv.pdf"}),
        json!({"paths": ["/tmp/a.pdf", "../b.pdf"]}),
    ] {
        let mut arguments = json!({"cdp": CDP, "index": 1});
        arguments
            .as_object_mut()
            .unwrap()
            .extend(paths.as_object().unwrap().clone());
        let err = server
            .call_tool("browser_file_upload", &arguments)
            .expect_err("relative");
        assert_eq!(err.code(), "path_not_absolute", "{err:?}");
        assert!(driving(&log).is_empty(), "{:?}", log.borrow());
    }
}

#[test]
fn an_absolute_path_is_set_as_given_anywhere() {
    let (mut server, log) = replay(Uploads::Anywhere);
    let out = upload(&mut server, &[Path::new("/tmp/f.pdf")]).unwrap();
    assert_eq!(out["acted"], true, "{out}");
    assert_eq!(files_set(&log), [json!(["/tmp/f.pdf"])]);
}

#[test]
fn uploads_off_refuses_before_any_cdp_call() {
    let (mut server, log) = replay(Uploads::Off);
    let err = upload(&mut server, &[Path::new("/tmp/f.pdf")]).expect_err("off");
    assert_eq!(err, ToolError::UploadsOff);
    assert_eq!(err.to_value()["code"], "uploads_off");
    assert!(driving(&log).is_empty(), "{:?}", log.borrow());
}

#[test]
fn a_dir_allows_files_inside_and_refuses_escapes() {
    let (root, inside, outside) = tree("dir");
    let mut refused = vec![
        outside.clone(),
        inside.join("../outside.pdf"),
        inside.join("missing.pdf"),
        inside.clone(),
    ];
    if cfg!(unix) {
        refused.push(inside.join("link.pdf"));
    }
    for path in &refused {
        let (mut server, log) = replay(Uploads::Dir(inside.clone()));
        let err = upload(&mut server, &[&inside.join("cv.pdf"), path]).expect_err("escape");
        assert_eq!(
            err.code(),
            "path_not_allowed",
            "{}: {err:?}",
            path.display()
        );
        assert_eq!(err.to_value()["dir"], inside.display().to_string());
        assert!(driving(&log).is_empty(), "{:?}", log.borrow());
    }
    // Inside (also through `..` that stays inside): the canonical path goes
    // to Chrome.
    let (mut server, log) = replay(Uploads::Dir(inside.clone()));
    let roundabout = inside.join("../inside/cv.pdf");
    let out = upload(&mut server, &[&roundabout]).unwrap();
    assert_eq!(out["acted"], true, "{out}");
    let canonical = inside.join("cv.pdf").canonicalize().unwrap();
    assert_eq!(files_set(&log), [json!([canonical.display().to_string()])]);
    let _ = std::fs::remove_dir_all(root);
}
