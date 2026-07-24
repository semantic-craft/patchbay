use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use app_lib::core::fleet::service::{MACHINE_ID_KEY, META_URL_KEY, PROJECTS_ROOT_KEY};
use app_lib::core::skill_store::SkillStore;
use serde_json::Value;
use tempfile::tempdir;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn run_cli(home: &Path, config: &Path, skills: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_patchbay-cli"))
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", config)
        .env("USERPROFILE", home)
        .env("APPDATA", config)
        .env("LOCALAPPDATA", config)
        .args(["--json", "--skills-root", skills.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn bootstrap_preview_is_read_only_and_partial_apply_keeps_json_outcome() {
    let temp = tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config");
    let skills = temp.path().join("skills");
    let projects = temp.path().join("projects");
    let source = temp.path().join("source");
    for dir in [&home, &config, &skills, &projects, &source] {
        std::fs::create_dir_all(dir).unwrap();
    }
    git(&source, &["init", "-b", "main"]);
    std::fs::write(source.join("file.txt"), "from hub").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-m", "seed"]);
    let target_oid = git_stdout(&source, &["rev-parse", "HEAD"]);

    let mirrors = temp.path().join("mirrors");
    std::fs::create_dir_all(&mirrors).unwrap();
    let hub = mirrors.join("alpha.git");
    assert!(Command::new("git")
        .args(["clone", "--bare"])
        .arg(&source)
        .arg(&hub)
        .output()
        .unwrap()
        .status
        .success());

    let meta_bare = temp.path().join("_patchbay-fleet.git");
    assert!(Command::new("git")
        .args(["init", "--bare", "--initial-branch=main"])
        .arg(&meta_bare)
        .output()
        .unwrap()
        .status
        .success());
    let meta_seed = temp.path().join("meta-seed");
    std::fs::create_dir_all(&meta_seed).unwrap();
    git(&meta_seed, &["init", "-b", "main"]);
    git(
        &meta_seed,
        &["remote", "add", "origin", meta_bare.to_str().unwrap()],
    );
    std::fs::write(
        meta_seed.join("manifest.toml"),
        format!(
            r#"
[hub.test]
url = '{}'

[[repo]]
name = "alpha"
hub = "test"
authority = "authority-machine"
branch = "main"
"#,
            mirrors.display()
        ),
    )
    .unwrap();
    git(&meta_seed, &["add", "-A"]);
    git(&meta_seed, &["commit", "-m", "seed manifest"]);
    git(&meta_seed, &["push", "origin", "main"]);

    let initialized = run_cli(&home, &config, &skills, &["repo", "status"]);
    assert!(initialized.status.success());
    let repo_status: Value = serde_json::from_slice(&initialized.stdout).unwrap();
    let db_path = PathBuf::from(repo_status["db_path"].as_str().unwrap());
    let store = SkillStore::new(&db_path).unwrap();
    store.set_setting(MACHINE_ID_KEY, "worker-machine").unwrap();
    store
        .set_setting(META_URL_KEY, meta_bare.to_str().unwrap())
        .unwrap();
    store
        .set_setting(PROJECTS_ROOT_KEY, projects.to_str().unwrap())
        .unwrap();

    let status = run_cli(&home, &config, &skills, &["fleet", "status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let target = projects.join("alpha");

    let preview = run_cli(
        &home,
        &config,
        &skills,
        &["fleet", "bootstrap", "--repo", "alpha", "--repo", "rogue"],
    );
    assert!(preview.status.success());
    let plan: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(plan["items"][0]["status"], "ready");
    assert_eq!(plan["items"][0]["evidence"]["target_oid"], target_oid);
    assert_eq!(plan["items"][1]["reason_code"], "repo_not_in_manifest");
    assert!(!target.exists(), "preview must not create the target");
    assert!(store.list_audit(None).unwrap().is_empty());

    let applied = run_cli(
        &home,
        &config,
        &skills,
        &[
            "fleet",
            "bootstrap",
            "--repo",
            "alpha",
            "--repo",
            "rogue",
            "--apply",
        ],
    );
    assert_eq!(applied.status.code(), Some(1));
    let outcome: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(outcome["ok"], false);
    assert_eq!(outcome["items"][0]["action"], "bootstrapped");
    assert_eq!(outcome["items"][1]["action"], "refused");
    assert_eq!(git_stdout(&target, &["branch", "--show-current"]), "main");
    assert_eq!(git_stdout(&target, &["rev-parse", "HEAD"]), target_oid);
    assert_eq!(git_stdout(&target, &["remote"]), "test");
    let envelope: Value = serde_json::from_slice(&applied.stderr).unwrap();
    assert_eq!(envelope["ok"], false);
    assert!(envelope["error"]
        .as_str()
        .unwrap_or_default()
        .contains("fleet bootstrap did not fully succeed"));
}

#[test]
fn fleet_bootstrap_product_sources_never_force_or_reset_a_branch() {
    let source = include_str!("../src/core/fleet/repo_ops.rs");

    assert!(!source.contains("&[\"checkout\", \"-B\""));
    assert!(!source.contains("&[\"reset\""));
}
