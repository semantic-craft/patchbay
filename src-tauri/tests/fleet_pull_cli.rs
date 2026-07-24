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

fn head(dir: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
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
fn pull_partial_failure_keeps_item_stdout_and_exits_one_with_error_envelope() {
    let temp = tempdir().unwrap();
    let home = temp.path().join("home");
    let config = temp.path().join("config");
    let skills = temp.path().join("skills");
    let projects = temp.path().join("projects");
    let alpha = projects.join("alpha");
    for dir in [&home, &config, &skills, &alpha] {
        std::fs::create_dir_all(dir).unwrap();
    }

    git(&alpha, &["init", "-b", "main"]);
    std::fs::write(alpha.join("file.txt"), "base").unwrap();
    git(&alpha, &["add", "-A"]);
    git(&alpha, &["commit", "-m", "base"]);
    let before = head(&alpha);

    let mirrors = temp.path().join("mirrors");
    std::fs::create_dir_all(&mirrors).unwrap();
    let hub = mirrors.join("alpha.git");
    assert!(Command::new("git")
        .args(["clone", "--bare"])
        .arg(&alpha)
        .arg(&hub)
        .output()
        .unwrap()
        .status
        .success());
    let publisher = temp.path().join("publisher");
    assert!(Command::new("git")
        .args(["clone"])
        .arg(&hub)
        .arg(&publisher)
        .output()
        .unwrap()
        .status
        .success());
    std::fs::write(publisher.join("file.txt"), "from hub").unwrap();
    git(&publisher, &["add", "-A"]);
    git(&publisher, &["commit", "-m", "hub update"]);
    git(&publisher, &["push", "origin", "main"]);
    let target = head(&publisher);

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

    // Seed the meta cache through the normal status surface. Pull preview itself
    // intentionally refuses to clone/fetch the cache.
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
        &["fleet", "pull", "--repo", "alpha", "--repo", "rogue"],
    );
    assert!(preview.status.success());
    let plan: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(plan["items"][0]["status"], "ready");
    assert_eq!(plan["items"][1]["reason_code"], "repo_not_in_manifest");
    assert_eq!(head(&alpha), before, "preview must not move HEAD");

    let applied = run_cli(
        &home,
        &config,
        &skills,
        &[
            "fleet", "pull", "--repo", "alpha", "--repo", "rogue", "--apply",
        ],
    );

    assert_eq!(applied.status.code(), Some(1));
    let outcome: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(outcome["ok"], false);
    assert_eq!(outcome["items"][0]["action"], "pulled");
    assert_eq!(outcome["items"][1]["action"], "refused");
    assert_eq!(head(&alpha), target);
    let envelope: Value = serde_json::from_slice(&applied.stderr).unwrap();
    assert_eq!(envelope["ok"], false);
    assert!(envelope["error"]
        .as_str()
        .unwrap_or_default()
        .contains("fleet pull did not fully succeed"));
}
