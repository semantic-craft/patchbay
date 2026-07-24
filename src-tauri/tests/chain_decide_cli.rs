use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn decide_preview_reports_item_error_and_exits_nonzero() {
    let temp = tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config");
    let skills = temp.path().join("skills");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&skills).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_patchbay-cli"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("USERPROFILE", &home)
        .env("APPDATA", &config)
        .env("LOCALAPPDATA", &config)
        .args([
            "--json",
            "--skills-root",
            skills.to_str().unwrap(),
            "chain",
            "decide",
            "--fingerprint",
            "missing-fingerprint",
            "--action",
            "ignore",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));

    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["ok"], false);
    assert_eq!(preview["items"][0]["fingerprint"], "missing-fingerprint");
    assert_eq!(preview["items"][0]["action"], "error");
    assert_eq!(preview["items"][0]["error_code"], "finding_not_found");

    let envelope: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(envelope["ok"], false);
    assert!(envelope["error"]
        .as_str()
        .unwrap_or_default()
        .contains("preview contains errors"));
}
