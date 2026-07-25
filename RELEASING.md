# Releasing

Patchbay releases are built, signed, notarized, and published entirely on
GitHub-hosted runners — free for this public repository. Source builds on every
push/PR via `test.yml`; the release pipeline runs only on version tags.

## Cutting a release

1. Run the **Prepare Release** workflow (`workflow_dispatch`) from `main`,
   choosing `patch` / `minor` / `major`. It bumps the version across
   `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, the i18n
   files, and both changelogs, commits, tags `vX.Y.Z`, and dispatches the
   release.
2. **Build & Release** (`release.yml`) then runs on the tag:
   - `macos-14` builds the Apple Silicon and (cross-compiled) Intel bundles,
     signs them with the Developer ID cert, notarizes the DMG with `notarytool`,
     and staples it.
   - `windows-latest` builds the NSIS installer (unsigned; auto-update still
     works via minisign).
   - Artifacts publish to this repository as a draft release, which is
     verified (checksums, updater signatures) and then flipped to latest.

Everything runs on ephemeral hosted runners — no self-hosted infrastructure.

## Required repository configuration

Add these under **Settings → Secrets and variables → Actions** before the first
release.

**Secrets**

| Name | Purpose |
|------|---------|
| `TAURI_SIGNING_PRIVATE_KEY` | minisign key for updater signatures |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | password for the above |
| `APPLE_CERTIFICATE` | base64 of the Developer ID `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password for the `.p12` |
| `KEYCHAIN_PASSWORD` | any string; unlocks the throwaway CI keychain |
| `APPLE_ID` | Apple ID for notarization |
| `APPLE_PASSWORD` | app-specific password for notarization |
| `APPLE_TEAM_ID` | Apple Developer Team ID |

**Variables**

| Name | Purpose |
|------|---------|
| `PATCHBAY_GITHUB_APP_CLIENT_ID` | client id of the in-app backup GitHub App |

On a public repository, GitHub withholds these from fork-based pull requests, so
they are exposed only to tag-triggered release runs from this repo.

## Why there is no publisher credential

Releases go to this repository using the workflow's built-in `GITHUB_TOKEN`,
so there is nothing to rotate or renew. Artifacts used to land in a separate
downloads repo — necessary while the source was private — and reaching across
repositories required a GitHub App with its own client id and private key.
Publishing where the source already lives removed that whole surface.
