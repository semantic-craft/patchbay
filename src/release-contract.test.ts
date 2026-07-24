import { existsSync, readFileSync } from "node:fs";
import { execFileSync, spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import en from "./i18n/en.json";
import zhTW from "./i18n/zh-TW.json";
import zh from "./i18n/zh.json";

const read = (path: string) =>
  readFileSync(resolve(process.cwd(), path), "utf8");

const trackedFiles = () =>
  execFileSync("git", ["ls-files", "-z"], { encoding: "utf8" })
    .split("\0")
    .filter(Boolean);

describe("Patchbay release contract", () => {
  it("contains no retired product identity in tracked text files", () => {
    const retiredNames = [
      ["skills", "manager"].join("-"),
      ["skills", "manager"].join("_"),
      ["skills", "manager"].join(" "),
      ["skills", "manager"].join(""),
      [".agent", "skills"].join("-"),
    ];
    const grep = spawnSync(
      "git",
      ["grep", "-FIil", ...retiredNames.flatMap((name) => ["-e", name]), "--", "."],
      { encoding: "utf8" },
    );
    expect([0, 1]).toContain(grep.status);

    const contentOffenders = grep.status === 0
      ? grep.stdout.trim().split("\n").filter(Boolean)
      : [];
    const pathOffenders = trackedFiles().filter((path) => {
      const normalized = path.toLowerCase();
      return retiredNames.some((name) => normalized.includes(name));
    });

    expect([...new Set([...contentOffenders, ...pathOffenders])]).toEqual([]);
  });

  it("uses Patchbay for the Rust package and canonical CLI identity", () => {
    const cargo = read("src-tauri/Cargo.toml");
    const runner = read("scripts/run-rust-cli.mjs");

    expect(cargo).toContain('name = "patchbay"');
    expect(cargo).toContain('default-run = "patchbay"');
    expect(cargo).toContain('description = "Patchbay - AI Agent Skills Control Plane"');
    expect(runner).toContain("'--bin', 'patchbay-cli'");
    expect(existsSync(resolve(process.cwd(), "src-tauri/src/bin/patchbay-cli.rs"))).toBe(true);
  });

  it("uses Patchbay for current storage, backup, and operator-facing documentation", () => {
    const backupView = read("src/views/Backup.tsx");
    const readme = read("README.md");
    const zhReadme = read("README.zh-CN.md");
    const manageSkills = read("skills/manage-skills/SKILL.md");

    expect(backupView).toContain('const DEFAULT_GITHUB_REPO = "patchbay-backup"');
    for (const locale of [en, zh, zhTW]) {
      expect(locale.settings.repoWarning_config_unreadable).toContain("~/.patchbay");
      expect(locale.settings.repoWarning_repo_path_invalid).toContain("~/.patchbay");
    }
    for (const doc of [readme, zhReadme, manageSkills]) {
      expect(doc).toContain("~/.patchbay");
      expect(doc).toContain("patchbay-cli");
    }
  });

  it("uses an independently configured, repository-scoped Patchbay GitHub App", () => {
    const githubApi = read("src-tauri/src/core/github_api.rs");
    const backupView = read("src/views/Backup.tsx");
    const releaseWorkflow = read(".github/workflows/release.yml");

    expect(githubApi).toContain('option_env!("PATCHBAY_GITHUB_APP_CLIENT_ID")');
    expect(githubApi).not.toContain("Ov23li4a3SMdhIiKo7IE");
    expect(githubApi).toContain("connect_github_app_backup_repo");
    expect(githubApi).toContain("GITHUB_APP_REPO_ACCESS");
    expect(githubApi).toContain("GITHUB_APP_REPO_NOT_PRIVATE");
    expect(githubApi).toContain("GITHUB_APP_INSTALLATION_SCOPE");
    expect(githubApi).toContain('"repository_id".to_string()');
    expect(backupView).toContain("handleDeviceFlow");
    expect(backupView).toContain('poll.status === "repository_identified"');
    expect(backupView).toContain("https://github.com/apps/patchbay-backup/installations/new");
    expect(releaseWorkflow).toContain(
      "PATCHBAY_GITHUB_APP_CLIENT_ID: ${{ vars.PATCHBAY_GITHUB_APP_CLIENT_ID }}",
    );
    expect(releaseWorkflow).toContain(
      "PATCHBAY_RELEASE_APP_CLIENT_ID: ${{ vars.PATCHBAY_RELEASE_APP_CLIENT_ID }}",
    );
  });

  it("builds signed updater artifacts for macOS from the base config", () => {
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));

    expect(config.bundle.createUpdaterArtifacts).toBe(true);
    // Base config stays macOS-only. Windows is added by a platform overlay, not
    // by widening this list — that is what keeps the macOS release contract
    // byte-identical while Windows ships.
    expect(config.bundle.targets).toEqual(["app", "dmg"]);
    expect(config.plugins.updater.endpoints).toEqual([
      "https://github.com/semantic-craft/patchbay-releases/releases/latest/download/latest.json",
    ]);
  });

  it("ships Windows through a platform overlay, unsigned but auto-updatable", () => {
    const windows = JSON.parse(read("src-tauri/tauri.windows.conf.json"));

    // Tauri merges tauri.<platform>.conf.json over the base, so this replaces
    // the macOS bundle targets on Windows only.
    expect(windows.bundle.targets).toEqual(["nsis"]);
    // The overlay must not restate schema defaults: webviewInstallMode is
    // already a silent downloadBootstrapper and NSIS installMode is already
    // currentUser, which is what keeps the installer elevation-free.
    expect(windows.bundle.windows).toBeUndefined();
    // Transparency buys nothing off macOS — the glass tier resolves to "none" —
    // and is fragile on WebView2 behind a decorated frame.
    expect(windows.app.windows[0].transparent).toBe(false);
    expect(windows.app.windows[0].decorations).toBe(true);
    // titleBarStyle/hiddenTitle are macOS-only keys; carrying them here would
    // stack the app's own drag strip under a native caption bar.
    expect(windows.app.windows[0].titleBarStyle).toBeUndefined();
    expect(windows.app.windows[0].hiddenTitle).toBeUndefined();
  });

  it("wires automatic signed updates to an explicitly permitted relaunch", () => {
    const pkg = JSON.parse(read("package.json"));
    const cargo = read("src-tauri/Cargo.toml");
    const tauriLib = read("src-tauri/src/lib.rs");
    const capability = JSON.parse(read("src-tauri/capabilities/default.json"));
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));
    const app = read("src/App.tsx");
    const settings = read("src/views/Settings.tsx");
    const updater = read("src/lib/appUpdater.ts");

    expect(pkg.dependencies["@tauri-apps/plugin-process"]).toBeTruthy();
    expect(pkg.dependencies["@tauri-apps/plugin-updater"]).toBeTruthy();
    expect(cargo).toContain('tauri-plugin-process = "2"');
    expect(cargo).toContain('tauri-plugin-updater = "2"');
    expect(tauriLib).toContain(".plugin(tauri_plugin_process::init())");
    expect(tauriLib).toContain(".plugin(tauri_plugin_updater::Builder::new().build())");
    expect(capability.permissions).toContain("process:allow-restart");
    expect(capability.permissions).toContain("updater:default");
    expect(capability.permissions).not.toContain("process:default");
    expect(config.plugins.updater.pubkey).toBeTruthy();
    expect(app).toContain("<AppUpdateNotifier />");
    expect(settings).toContain("APP_UPDATE_ENABLED_SETTING");
    expect(settings).toContain("setAppAutoUpdateEnabled] = useState(true)");
    expect(settings).toContain("handleCheckUpdate");
    expect(updater).toContain("downloadAndInstall");
    expect(updater).toContain("await runtime.relaunch()");
    expect(read("README.md")).toContain("never installed silently");
    expect(read("README.zh-CN.md")).toContain("不会静默安装");
    for (const locale of [en, zh, zhTW]) {
      expect(locale.settings.appUpdate.title).toBeTruthy();
      expect(locale.settings.appUpdate.ready).toContain("{{version}}");
      expect(locale.settings.appUpdate.installAndRestart).toBeTruthy();
    }
  });

  it("uses Developer ID signing, notarization, and Patchbay bundle paths", () => {
    const workflow = read(".github/workflows/release.yml");

    expect(workflow).toContain("Validate required release secrets");
    expect(workflow).toContain("Tag/version mismatch");
    expect(workflow).toContain("Tag commit is not origin/main HEAD");
    expect(workflow).toMatch(/releaseName:\s+["']Patchbay v__VERSION__["']/);
    expect(workflow).toContain(
      "APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}",
    );
    expect(workflow).toContain(
      "TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    );
    expect(workflow).toContain("APPLE_ID: ${{ secrets.APPLE_ID }}");
    expect(workflow).toContain("APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}");
    expect(workflow).toContain("APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}");
    expect(workflow).toContain("bundle/macos/Patchbay.app");
    expect(workflow).toContain('xcrun notarytool submit "$DMG_PATH"');
    expect(workflow).toContain('xcrun stapler staple "$DMG_PATH"');
    expect(workflow).toContain('gh release upload "$GITHUB_REF_NAME"');
    expect(workflow).toContain('gh release download "$GITHUB_REF_NAME"');
    expect(workflow).toContain('gh release view "${{ github.ref_name }}"');
    expect(workflow).toContain(
      "actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1",
    );
    expect(workflow).toContain("RELEASE_REPOSITORY: semantic-craft/patchbay-releases");
    expect(workflow).toContain("repo: patchbay-releases");
    expect(workflow).toContain('GH_TOKEN: ${{ steps.release-token.outputs.token }}');
    expect(workflow).toContain('--repo "$RELEASE_REPOSITORY"');
    expect(workflow).toContain("--clobber");
    expect(workflow).toContain("Uploaded DMG checksum mismatch");
    expect(workflow).not.toContain("/releases/tags/");
    expect(workflow).toContain("xcrun stapler validate");
    expect(workflow).toContain("spctl --assess");
    expect(workflow).toContain("minisign -Vm");
    // Every shipped platform is verified before the release leaves draft, and
    // the Windows artifact is checked from the macOS runner: the loop derives
    // the asset from the metadata URL, so minisign does not care who built it.
    for (const platform of ["darwin-aarch64", "darwin-x86_64", "windows-x86_64"]) {
      expect(workflow).toContain(platform);
    }
    expect(workflow).not.toContain("linux-x86_64");
    expect(workflow).not.toContain("Linux-x64");
    expect(workflow).not.toContain("跨平台构建通过");
    expect(workflow).not.toContain("Cross-platform build passed");
    expect(workflow).not.toContain(
      "APPLE_SIGNING_IDENTITY: ${{ startsWith(matrix.platform, 'macos') && '-' || '' }}",
    );
  });

  it("runs the release pipeline on GitHub-hosted runners", () => {
    const releaseWorkflow = read(".github/workflows/release.yml");
    const prepareWorkflow = read(".github/workflows/prepare-release.yml");

    // Self-hosted runners are retired: nothing routes to our own boxes anymore.
    expect(releaseWorkflow).not.toContain("self-hosted");
    expect(prepareWorkflow).not.toContain("self-hosted");

    // Signing and notarization need macOS; macos-14 is Apple Silicon and builds
    // both the arm64 target natively and the x86_64 target by cross-compile.
    expect(releaseWorkflow).toContain("runs-on: macos-14");
    expect(releaseWorkflow).toContain("runs-on: windows-latest");
    // Both macOS legs upload into one draft release, so they must not race.
    expect(releaseWorkflow).toContain("max-parallel: 1");
    expect(releaseWorkflow).toContain('CI: "true"');
    expect(releaseWorkflow).toContain("retryAttempts: 2");

    // Ephemeral-runner keychain pattern: a throwaway keychain in RUNNER_TEMP,
    // the Developer ID cert imported for codesign. No human login keychain to
    // protect or restore, so the self-hosted keychain dance and caffeinate are
    // gone.
    expect(releaseWorkflow).toContain("$RUNNER_TEMP/patchbay-signing.keychain-db");
    expect(releaseWorkflow).toContain("security create-keychain");
    expect(releaseWorkflow).toContain("security set-keychain-settings -t 21600");
    expect(releaseWorkflow).toContain('grep "Developer ID Application" | head -1');
    expect(releaseWorkflow).not.toContain("caffeinate");
    expect(releaseWorkflow).not.toContain("Restore runner keychain");

    // Version bump / tag / dispatch has no native build, so it runs on Linux.
    expect(prepareWorkflow).toContain("runs-on: ubuntu-latest");
  });

  it("commits every file changed by release preparation", () => {
    const workflow = read(".github/workflows/prepare-release.yml");

    expect(workflow).toContain("Validate release secrets before tagging");
    expect(workflow).toContain("Prepare Release must run from main");
    expect(workflow).toContain("git push --atomic");
    expect(workflow).toContain("actions: write");
    expect(workflow).toContain(
      'gh workflow run release.yml --repo "$GITHUB_REPOSITORY" --ref "v${VERSION}"',
    );
    expect(workflow).toContain(
      "APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}",
    );

    for (const path of [
      "CHANGELOG.md",
      "CHANGELOG-zh.md",
      "package.json",
      "package-lock.json",
      "src-tauri/tauri.conf.json",
      "src/i18n/en.json",
      "src/i18n/zh.json",
      "src/i18n/zh-TW.json",
    ]) {
      expect(workflow).toContain(path);
    }
  });

  it("does not expose private source links in public release notes", () => {
    const workflow = read(".github/workflows/release.yml");
    expect(workflow).not.toContain("github.repository }}/compare");
    expect(workflow).not.toContain("github.repository }}/blob/main/CHANGELOG");
  });

  it("refreshes release metadata from the Patchbay repository", () => {
    expect(read("scripts/gen-star-history.py")).toContain(
      'REPO = "semantic-craft/patchbay-releases"',
    );
  });

  it("documents the notarized official macOS release contract", () => {
    const readme = read("README.md");
    const zhReadme = read("README.zh-CN.md");
    expect(readme).toContain("Developer ID-signed and notarized");
    expect(zhReadme).toContain("Developer ID 签名并通过 Apple 公证");
    expect(readme).not.toContain(
      "release builds are Developer ID-signed but are not notarized",
    );
    expect(zhReadme).not.toContain("使用 ad-hoc 签名，未做 Apple 公证");
  });

  it("keeps every shipped version label in sync", () => {
    const pkg = JSON.parse(read("package.json"));
    const lock = JSON.parse(read("package-lock.json"));
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));

    expect(config.version).toBe(pkg.version);
    expect(lock.version).toBe(pkg.version);
    expect(lock.packages[""].version).toBe(pkg.version);
    for (const locale of [en, zh, zhTW]) {
      expect(locale.settings.version).toBe(`Patchbay ${pkg.version}`);
    }
  });
});
