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

fn install_from(root: &Path, version: Option<&str>) -> Output {
    let mut command = Command::new("sh");
    command
        .arg(SCRIPT)
        .env("AUI_BASE_URL", format!("file://{}", root.display()))
        .env("AUI_INSTALL_DIR", root.join("bin"))
        // No cargo: a failed download must not start a source build here.
        .env("PATH", "/usr/bin:/bin")
        .env_remove("AUI_VERSION");
    if let Some(version) = version {
        command.env("AUI_VERSION", version);
    }
    command.output().unwrap()
}

fn install(root: &Path) -> Output {
    install_from(root, Some("v9.9.9"))
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
fn a_missing_pinned_release_never_falls_back_to_source() {
    let root = release("missing-pinned");
    std::fs::remove_dir_all(root.join("download/v9.9.9")).unwrap();
    let out = install(&root);
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("AUI_VERSION=v9.9.9 was not installed"),
        "{stderr}"
    );
    assert!(!stderr.contains("falling back"), "{stderr}");
    assert!(!root.join("bin").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_missing_latest_release_without_cargo_fails_and_installs_nothing() {
    let root = release("missing");
    let out = install_from(&root, None);
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

/// P1.3: a copy of the built `aui` in a scratch bin directory.
fn installed(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["aui", "ultra-instinct"] {
        std::fs::copy(env!("CARGO_BIN_EXE_aui"), bin.join(name)).unwrap();
    }
    bin
}

fn upgrade(root: &Path, bin: &Path) -> Output {
    Command::new(bin.join("aui"))
        .args(["upgrade", "--version", "v9.9.9"])
        .env("AUI_BASE_URL", format!("file://{}", root.display()))
        .output()
        .unwrap()
}

#[test]
fn upgrade_replaces_the_installed_binaries_in_place() {
    let root = release("upgrade");
    let bin = installed(&root);
    let out = upgrade(&root, &bin);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.starts_with("upgraded aui + ultra-instinct in "),
        "{stdout}"
    );
    assert!(
        stdout.ends_with(&format!(": {} -> 9.9.9\n", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );
    for name in ["aui", "ultra-instinct"] {
        let ran = Command::new(bin.join(name)).output().unwrap();
        assert_eq!(
            String::from_utf8(ran.stdout).unwrap(),
            format!("{name} 9.9.9\n")
        );
    }
    // Nothing but the two binaries is left behind.
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn upgrade_refuses_a_tampered_archive_and_keeps_the_old_binary() {
    let root = release("upgrade-tampered");
    sh(
        "for a in ./*.tar.gz; do printf x >> \"$a\"; done",
        &root.join("download/v9.9.9"),
    );
    let bin = installed(&root);
    let out = upgrade(&root, &bin);
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("checksum mismatch"), "{stderr}");
    assert!(stderr.contains("nothing was replaced"), "{stderr}");
    let ran = Command::new(bin.join("aui"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(ran.stdout).unwrap().trim(),
        format!("ultra-instinct {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 2);
    std::fs::remove_dir_all(root).unwrap();
}
