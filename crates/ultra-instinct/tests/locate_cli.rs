use std::process::Command;

#[test]
fn binary_locate_json_prefers_the_left_settings_button() {
    let exe = env!("CARGO_BIN_EXE_ultra-instinct");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sidebar.manifold");
    let output = Command::new(exe)
        .args([
            "locate",
            "--fixture",
            fixture.to_str().unwrap(),
            "--text",
            "Settings",
            "--role",
            "button",
            "--position",
            "left",
            "--json",
        ])
        .output()
        .expect("spawn ultra-instinct");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let nav = stdout.find("\"id\": \"nav-settings\"").expect(&stdout);
    let main = stdout.find("\"id\": \"main-settings\"").expect(&stdout);
    assert!(nav < main);
    assert!(!stdout.contains("hgra"));
}

#[test]
fn locate_send_prefers_the_exact_label() {
    let exe = env!("CARGO_BIN_EXE_ultra-instinct");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/send-buttons.manifold");
    let output = Command::new(exe)
        .args([
            "locate",
            "--fixture",
            fixture.to_str().unwrap(),
            "--text",
            "Send",
            "--role",
            "button",
            "--json",
        ])
        .output()
        .expect("spawn ultra-instinct");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let exact = stdout.find("\"id\": \"z-send\"").expect(&stdout);
    let feedback = stdout.find("\"id\": \"a-feedback\"").expect(&stdout);
    assert!(exact < feedback, "{stdout}");
}
