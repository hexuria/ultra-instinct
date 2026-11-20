//! `scripts/install.sh` (plan P1.2) against a release laid out on disk: it
//! installs the archive for this machine, and refuses a tampered one.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");

fn sh(script: &str, dir: &Path) -> String {
    let out = Command::new("sh")
        .args(["-euc", script])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{script}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A scratch directory holding release `v9.9.9` with one archive per target
/// this machine could ask for, each with stub binaries.
fn release(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aui-install-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join("download/v9.9.9");
    std::fs::create_dir_all(&dir).unwrap();
    sh(
        r#"for os in apple-darwin unknown-linux-gnu unknown-linux-musl; do
  for arch in x86_64 aarch64; do
    stage="aui-$arch-$os"
    mkdir "$stage"
    for bin in aui ultra-instinct; do
      printf '#!/bin/sh\necho "%s 9.9.9"\n' "$bin" > "$stage/$bin"
      chmod 755 "$stage/$bin"
    done
    tar -czf "$stage.tar.gz" "$stage"
    rm -r "$stage"
  done
done
if command -v sha256sum >/dev/null; then sha256sum ./*.tar.gz; else shasum -a 256 ./*.tar.gz; fi \
  | sed 's|\./||' > SHA256SUMS"#,
        &dir,
    );
    root
}

fn install(root: &Path) -> Output {
    Command::new("sh")
        .arg(SCRIPT)
        .env("AUI_BASE_URL", format!("file://{}", root.display()))
        .env("AUI_VERSION", "v9.9.9")
        .env("AUI_INSTALL_DIR", root.join("bin"))
        // No cargo: a failed download must not start a source build here.
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap()
}

#[test]
fn installs_both_binaries_after_the_checksum_matches() {
    let root = release("ok");
    let out = install(&root);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with("installed aui 9.9.9 to "), "{stdout}");
    for bin in ["aui", "ultra-instinct"] {
        let ran = Command::new(root.join("bin").join(bin)).output().unwrap();
        assert_eq!(
            String::from_utf8(ran.stdout).unwrap(),
            format!("{bin} 9.9.9\n")
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_tampered_archive_installs_nothing() {
    let root = release("tampered");
    sh(
        "for a in ./*.tar.gz; do printf x >> \"$a\"; done",
        &root.join("download/v9.9.9"),
    );
    let out = install(&root);
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("checksum mismatch"), "{stderr}");
    assert!(stderr.contains("nothing was installed"), "{stderr}");
    assert!(!root.join("bin").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_missing_release_without_cargo_fails_and_installs_nothing() {
    let root = release("missing");
    std::fs::remove_dir_all(root.join("download/v9.9.9")).unwrap();
    let out = install(&root);
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("falling back to the source build"),
        "{stderr}"
    );
    assert!(stderr.contains("cargo is not on PATH"), "{stderr}");
    assert!(!root.join("bin").exists());
    std::fs::remove_dir_all(root).unwrap();
}
