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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
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
fn init_partial_apply_keeps_item_stdout_and_exits_one_with_error_envelope() {
    let temp = tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config");
    let skills = temp.path().join("skills");
    let projects = temp.path().join("projects");
    let alpha = projects.join("alpha");
    let mirrors = temp.path().join("mirrors");
    for dir in [&home, &config, &skills, &alpha, &mirrors] {
        std::fs::create_dir_all(dir).unwrap();
    }
    git(&alpha, &["init", "-b", "main"]);
    std::fs::write(alpha.join("file.txt"), "base").unwrap();
    git(&alpha, &["add", "-A"]);
    git(&alpha, &["commit", "-m", "base"]);
    git(
        &alpha,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:team/alpha.git",
        ],
    );
    let origin_before = git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]);

    // Inside the hub: `plan_meta_init` refuses a meta repo that sits outside
    // every declared local hub, which is what `init_refuses_a_meta_target_
    // outside_the_declared_local_hub` pins down. Placing it beside the hub
    // made this test assert a refusal it did not mean to exercise.
    let meta_bare = mirrors.join("_patchbay-fleet.git");
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
host_machine = "worker-machine"

[[repo]]
name = "alpha"
hub = "test"
authority = "worker-machine"
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

    let preview = run_cli(
        &home,
        &config,
        &skills,
        &["fleet", "init", "--repo", "alpha", "--repo", "rogue"],
    );
    assert!(preview.status.success());
    let plan: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(plan["items"][0]["status"], "ready");
    assert_eq!(plan["items"][0]["mirror_action"], "create");
    assert_eq!(plan["items"][0]["remote_action"], "add");
    assert_eq!(plan["items"][1]["reason_code"], "repo_not_in_manifest");
    assert!(!mirrors.join("alpha.git").exists());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&alpha)
        .args(["remote", "get-url", "test"])
        .output()
        .unwrap()
        .status
        .code()
        .is_some_and(|code| code != 0));

    let applied = run_cli(
        &home,
        &config,
        &skills,
        &[
            "fleet", "init", "--repo", "alpha", "--repo", "rogue", "--apply",
        ],
    );

    assert_eq!(applied.status.code(), Some(1));
    let outcome: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(outcome["ok"], false);
    assert_eq!(
        outcome["items"][0]["action"], "applied",
        "outcome: {outcome}"
    );
    assert_eq!(outcome["items"][1]["action"], "refused");
    assert_eq!(
        git_stdout(
            &mirrors.join("alpha.git"),
            &["rev-parse", "--is-bare-repository"]
        ),
        "true"
    );
    // `repo_url` joins with `/` because it builds a git URL, so on Windows the
    // remote is `C:\...\mirrors/alpha.git` — git accepts the mixed separator.
    // Normalize before comparing, as `repo_move` already does for roots.
    assert_eq!(
        git_stdout(&alpha, &["remote", "get-url", "test"]).replace('\\', "/"),
        mirrors
            .join("alpha.git")
            .to_string_lossy()
            .replace('\\', "/")
    );
    assert_eq!(
        git_stdout(&alpha, &["config", "--get-regexp", "^remote\\.origin\\."]),
        origin_before
    );
    let envelope: Value = serde_json::from_slice(&applied.stderr).unwrap();
    assert_eq!(envelope["ok"], false);
    assert!(envelope["error"]
        .as_str()
        .unwrap_or_default()
        .contains("fleet init did not fully succeed"));
}

#[test]
fn fleet_init_product_sources_never_spawn_ssh() {
    let sources = [
        include_str!("../src/core/fleet/service.rs"),
        include_str!("../src/core/fleet/repo_ops.rs"),
        include_str!("../src/core/fleet/meta_repo.rs"),
    ]
    .join("\n");

    assert!(!sources.contains("Command::new(\"ssh\")"));
    assert!(!sources.contains("Command::new(\"/usr/bin/ssh\")"));
}
