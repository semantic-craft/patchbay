# Changelog

All notable changes to Patchbay since its first independent release are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Release Overview
- Patchbay is multi-platform again. The 1.31.0 note below records that Windows support "was dropped when the product scope narrowed to macOS-only (#47)" — that is no longer true, and #58 reverses it. The entry is left standing because it was the decision at the time; this section is the reversal.

### User-facing
- **Windows builds ship** — `release.yml` gained a `build-windows` job producing an NSIS installer. It is **unsigned**, so Windows shows a SmartScreen warning on first run; auto-update still works, because the updater verifies minisign rather than Authenticode.

### Developer & Governance
- Windows `cargo test` went from 631 passed / 53 failed to 812 passed / 0 failed. Roughly 95 of those had never executed on Windows at all: every module-level `#[cfg(all(test, unix))]` gate is gone, and the symlink fixtures now run there through a shared `core::test_support` helper (directory links fall back to junctions, so they need no Developer Mode).
- Production defects the port surfaced, each of which was a real Windows bug rather than a test artifact: `resolve_hub_base` used `is_absolute()`; `chain::ops::make_symlink` had no junction fallback; six `remove_file` calls could not remove a directory symlink; the `AGENT_SURFACES` table joined a relative path by hand (now funnelled through `project_links::surface_path`); `patchbay-cli` read only `$HOME`; `Cargo.toml` was pinned at 1.0.0.
- `content_hash` folded the executable bit into the digest only on unix, so an identical skill hashed differently per OS and every mixed-fleet comparison saw a phantom change. Now folded everywhere, defaulting to "not executable" — chosen so existing macOS hashes do not move.
- The AES-256-GCM key file is now restricted on Windows (inheritance dropped, owner plus SYSTEM only), and the restriction is re-asserted whenever the key is loaded, so keys written before this repair themselves.
- `npm test` — including `release-contract.test.ts`, a release gate — now runs on pull requests instead of only inside `release.yml`, where it first executed after the tag was already pushed.

### Known gaps
- Publishing a release now requires the Windows runner to be online: `verify-updater-assets` demands a `windows-x86_64` entry in `latest.json`.
- The Windows installer is unsigned by choice. Signing means adding `bundle.windows.certificateThumbprint` and importing a certificate on the runner; a half-wired signing path that silently no-ops would be worse than its visible absence.
- The `prompt-optimizer` round-trip in #47's P3 gate is only half-proven: a non-authority machine fast-forwarded from the hub, but the commit-and-push leg belongs to the authority machine and cannot be driven from a non-authority machine.

## [1.31.0] - 2026-07-18

### Release Overview
- Delivers the multi-machine repository sync epic (#25): a fleet of machines keeps its project repos aligned through a shared hub, with every machine acting only on its own working copies. Ships P0 (read-only status), P1 (push/pull/bootstrap/init), and P3 (script handover); P2 automatic rounds ship default-off pending an observation window.

### User-facing
- **Fleet page (多机)** — a repo × machine status matrix: each cell shows `branch@head`, uncommitted count, and divergence from the hub. The local column is measured live; other machines replay their last report with an honest relative timestamp, flagged stale past seven days.
- **Guarded sync actions** — push (authority machine → hub), pull (hub → clean non-authority checkout, fast-forward only), and bootstrap (clone a managed repo that is missing here) all run preview → confirm → apply, and refuse rather than guess on dirty, detached, diverged, or drifted state.
- **Manifest editor** — adopt a discovered repo in one click, edit authority/hub/branch, or drop a repo from management. Removing a row only edits the manifest; it never touches a checkout or a mirror.
- **Local hub init and remote convergence** — create missing bare mirrors on the hub host and converge each machine's hub remote, leaving `origin` for your own upstream.
- **Automatic rounds (default off)** — opt in globally *and* per repository before any scheduled round runs; clean repos fast-forward or push on their own, and anything else is reported rather than forced.

### Developer & Governance
- New `core/fleet/` (manifest, meta repo, repo ops, service, auto round) plus `patchbay-cli fleet status|discover|report|push|pull|bootstrap|init`. Design of record: `docs/xw-fleet-sync-design.md`.
- The repo list moved to `manifest.toml` in a hub-side meta repo, ending the two-copies-overwrite-each-other failure mode of the legacy per-machine config. the sync script shrank from 747 lines to a 316-line shell that delegates to the CLI and exits 127 with guidance when it is absent.
- Every mutating verb records plan evidence and re-verifies it under `fleet.lock` before acting; any drift becomes a per-item conflict. No fleet verb can merge, rebase, force, reset, stash, auto-commit, or delete.
- Security: a manifest hub URL reached `git ls-remote` without a `--` separator, so a value beginning with `-` was parsed as an option and `--upload-pack=<cmd>` executed — on the read-only status path (#54). Fixed, and `hub.url` is now validated against a transport allowlist.
- Accepted deviations recorded on the epic: bootstrap is permitted on the authority machine (it only ever writes a path that does not exist, so the rule protecting the source of truth has nothing to protect) (#56); Windows support was dropped when the product scope narrowed to macOS-only (#47).

## [1.30.0] - 2026-07-18

### Release Overview
- Delivers the Liquid Glass workbench round (#26): the workbench becomes the home screen with a native glass window treatment, exception-driven chain maintenance with undoable repairs, and preset-driven onboarding.

### User-facing
- **Liquid Glass workbench** — the project workbench is now the home route, dressed in a light/dark glass skin: a wallpaper-backed window shell, glass sidebar, and floating glass cards.
- **Window-level glass on macOS** — the window turns transparent with real Liquid Glass (NSGlassEffectView) on macOS 26, frosted vibrancy on older macOS, and an automatic opaque fallback when neither applies — features never degrade.
- **Native appearance follows your theme** — picking Light/Dark/System in Settings now also drives the native window material and title bar, so the glass never mismatches the UI theme.
- **Exception-driven chain maintenance** — a calm all-clear screen when links are healthy; broken links surface as evidence cards with candidate paths and git clues, fixed through a deterministic plan/apply repair flow with live step progress and pause/take-over.
- **Repair journal with undo** — every applied repair is journaled with its reverse operation for one-click, guarded undo.
- **Common-cause batch relink** — repository moves that break many links at once aggregate into a single cause card with one-click batch relinking.
- **Feed-upstream hint** — an amber card surfaces uncommitted changes sitting in original skill repositories.
- **Chain presets and onboarding wizard** — save the current skill set as a preset, apply it from the preset bar, and onboard zero-link projects through a three-step wizard (source → skills → entry files).

### Developer & Governance
- Window glass ships behind a three-tier runtime probe (`liquid-glass` / `vibrancy` / `none`) with a pure, unit-tested degradation decision; the CSS wallpaper stays opaque unless a native material is confirmed behind the webview.
- Adopted `tauri-plugin-liquid-glass` (bundled cocoa/objc bindings) to avoid the window-vibrancy 0.6/0.8 duplicate-symbol conflict (tauri#15478). Accepted deviations recorded on #37: unfocused windows always frost; `macOSPrivateApi` is App Store-incompatible.
- The agent-instructions support-matrix research (issue #3) landed in `docs/research/`.

## [1.29.4] - 2026-07-16

### Release Overview
- Adds user-approved desktop auto-updates and completes the remaining repository-governance tickets on the cleaned Patchbay codebase.

### User-facing
- **Automatic signed update checks** — official macOS builds check the public Patchbay release channel once per day after startup and show a persistent prompt when a new version is available.
- **Installation remains under user control** — Patchbay downloads, verifies, installs, and restarts only after the user chooses **Install and restart**; automatic checks can be disabled, and manual checking plus release-page downloads remain available.
- **Version-specific approval** — if the available release changes after a prompt appears, Patchbay asks again for the new target version instead of installing it under the earlier approval.
- **Clearer navigation and three-tier flow** — the sidebar now centers on the library, installation, link topology, project links, source repositories, diagnostics, and backup; redundant dashboard and global-workspace routes were folded into their canonical destinations.
- **Patchbay Central and Qoder Work integration** — centrally installed skills can enter the project-only three-tier chain, including Qoder Work's project skill surface, without treating Qoder's vendor-managed global directory as a policy violation.

### Developer & Governance
- Added a TDD-covered updater coordinator with injected settings, time, updater, and process boundaries, plus rendered startup-notification tests and complete English, Simplified Chinese, and Traditional Chinese copy.
- Registered the Tauri process plugin with restart-only permission and expanded the release contract across updater/process dependencies, registrations, permissions, public endpoint, signing key, and startup wiring.
- Completed the outstanding CLI decision workflow and canonical instructions wrapper contracts, then removed retired generated assets and dead code before release.
- Consolidated duplicate navigation and backup surfaces, removed the unused `@hello-pangea/dnd` dependency, and applied repository-wide Rust formatting enforced by the release checks.

## [1.29.3] - 2026-07-14

### Release Overview
- Verifies Patchbay's complete two-repository release architecture from an independent private source repository.

### User-facing
- **Stable public updates from private development** — signed macOS updates continue through `semantic-craft/patchbay-releases` with no dependency on public source access.

### Developer & Governance
- This release is prepared, tagged, built, signed, notarized, and dispatched entirely from the private standalone source repository.
- The release-only GitHub App continues to mint short-lived tokens scoped exclusively to the public release repository.

## [1.29.2] - 2026-07-14

### Release Overview
- Separates Patchbay's private source repository from its public release channel.

### User-facing
- **Dedicated public update channel** — in-app update checks, release links, and downloads now use `semantic-craft/patchbay-releases`, so signed macOS updates remain publicly accessible while the source repository stays private.

### Developer & Governance
- Release automation now publishes with short-lived tokens from the dedicated Patchbay Release Publisher GitHub App, installed only on the public release repository with Contents write access.
- The private source workflow builds, signs, notarizes, validates, and publishes cross-repository assets without exposing source history or a reusable personal token.

## [1.29.1] - 2026-07-13

### Release Overview
- Completes Patchbay's independent product identity and introduces the branded, repository-scoped Patchbay Backup connection.

### User-facing
- **Patchbay identity is complete** — the desktop app, bundled and standalone CLI, storage directory, database, lock files, backup metadata, documentation, and update routes now use Patchbay exclusively. Fresh installs no longer carry retired product migration paths or browser-state keys.
- **Safer GitHub backup authorization** — Patchbay Backup uses a two-stage Device Flow: the first authorization identifies the selected private repository without storing its token, and the second asks GitHub to issue a final token restricted to that repository. Patchbay rejects public repositories, broad installations, and expanded repository access.
- **Clear recovery when authorization changes** — repository privacy and token scope are revalidated before Git operations and after refresh; revoked or broadened access produces an explicit reconnect action instead of silently falling back to empty credentials.

### Developer & Governance
- Added a release contract that scans every tracked non-binary file and tracked path for retired product identity, preventing old names from returning to shipped surfaces.
- Propagated GitHub App reauthorization failures through system Git, libgit2, Chain pull, and fork synchronization; the discovery credential never enters settings, IPC, logs, URLs, or the OS keychain.
- Removed retired default-storage, configuration, database, metadata, and localStorage migration code. The current user-selected repository path migration remains intact.

## [1.29.0] - 2026-07-13

### Release Overview
- First independent Patchbay release: the project-local three-tier Skills control plane is complete across the desktop app and CLI, with a hardened signed-update and notarized macOS release pipeline.

### User-facing
- **Project-local three-tier management is now the primary workflow** — Link Topology, Project Links, Original Repositories, Doctor, and Global Guard share one vocabulary and one Chain Service. Users can inspect complete resolution chains, enrol projects, preview safe link/unlink/remediation/normalization operations, update clean repositories fast-forward only, and rescan to verify the result.
- **The CLI has parity with the desktop workflows** — `patchbay-cli chain` exposes topology, where, doctor, repository health, duplicate comparison, link, unlink, remediation, normalization, pull, and explicit fork synchronization with stable JSON contracts.
- **Patchbay identity and update routing are consistent** — in-app help leads with the three-tier model, update checks point to `semantic-craft/patchbay`, and official macOS releases are Developer ID-signed and notarized.

### Developer & Governance
- Completed the issue #1 roadmap through #20, including adapter-driven Global Guard, registered-project and multi-root inventories, guarded plan/apply writes, persisted Doctor decisions, repository health, CLI parity, GUI completion, wrapper consolidation, and capability-to-ticket verification.
- Release automation now runs frontend and Rust gates before building both macOS architectures, creates signed updater artifacts with a Patchbay-owned key, validates notarization, stapling, and Gatekeeper, verifies `latest.json` and signatures while the release is draft, and publishes only after every gate succeeds.
- Existing helper commands are thin wrappers over `patchbay-cli chain`; platform-specific wrappers keep policy only and no longer duplicate filesystem or Git mutation rules.
