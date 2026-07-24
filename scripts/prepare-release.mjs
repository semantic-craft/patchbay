#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';

const root = process.cwd();
const args = process.argv.slice(2);

const releaseArg = args.find((arg) => !arg.startsWith('--'));
const dryRun = args.includes('--dry-run');

if (!releaseArg) {
  console.error('Usage: npm run release:prepare -- <patch|minor|major|x.y.z> [--dry-run]');
  process.exit(1);
}

const dateStr = new Date().toISOString().slice(0, 10);

const packagePath = path.join(root, 'package.json');
const packageLockPath = path.join(root, 'package-lock.json');
const tauriConfPath = path.join(root, 'src-tauri', 'tauri.conf.json');
const cargoTomlPath = path.join(root, 'src-tauri', 'Cargo.toml');
const cargoLockPath = path.join(root, 'src-tauri', 'Cargo.lock');
const enI18nPath = path.join(root, 'src', 'i18n', 'en.json');
const zhI18nPath = path.join(root, 'src', 'i18n', 'zh.json');
const zhTwI18nPath = path.join(root, 'src', 'i18n', 'zh-TW.json');
const changelogPath = path.join(root, 'CHANGELOG.md');
const changelogZhPath = path.join(root, 'CHANGELOG-zh.md');

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

/**
 * Bump the crate's own version in a Cargo manifest or lockfile.
 *
 * Windows embeds the *Cargo* version into the exe's version resource and into
 * `patchbay-cli --version`, so leaving it behind makes a shipped build report
 * the wrong number. macOS never reads it — Info.plist takes tauri.conf.json —
 * which is why it silently sat at 1.0.0 through thirty releases.
 *
 * Anchored on the preceding `name = "patchbay"` line: a bare `^version =`
 * would be ambiguous the moment anyone adds `[package.metadata]`, and in
 * Cargo.lock every dependency has a `version` of its own.
 */
function bumpCargoVersion(text, nextVersion, fileLabel) {
  const re = /(name = "patchbay"\r?\nversion = ")\d+\.\d+\.\d+(")/;
  if (!re.test(text)) {
    throw new Error(`Missing patchbay version entry in ${fileLabel}`);
  }
  return text.replace(re, `$1${nextVersion}$2`);
}

function parseSemver(version) {
  const m = version.match(/^(\d+)\.(\d+)\.(\d+)$/);
  if (!m) return null;
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]) };
}

function bumpVersion(current, releaseType) {
  const parsed = parseSemver(current);
  if (!parsed) {
    throw new Error(`Current package version is not SemVer: ${current}`);
  }

  if (releaseType === 'patch') {
    return `${parsed.major}.${parsed.minor}.${parsed.patch + 1}`;
  }
  if (releaseType === 'minor') {
    return `${parsed.major}.${parsed.minor + 1}.0`;
  }
  if (releaseType === 'major') {
    return `${parsed.major + 1}.0.0`;
  }

  if (parseSemver(releaseType)) {
    return releaseType;
  }

  throw new Error(`Invalid release type/version: ${releaseType}`);
}

function updateSettingsVersion(i18nObj, nextVersion, fileLabel) {
  if (!i18nObj.settings || typeof i18nObj.settings.version !== 'string') {
    throw new Error(`Missing settings.version in ${fileLabel}`);
  }
  i18nObj.settings.version = i18nObj.settings.version.replace(/\d+\.\d+\.\d+/, nextVersion);
}

function ensureChangelogEntry(changelog, nextVersion, { zh = false } = {}) {
  const heading = `## [${nextVersion}] - ${dateStr}`;
  if (changelog.includes(heading) || changelog.includes(`## [${nextVersion}] -`)) {
    return changelog;
  }

  const sections = zh
    ? ['### 发布概览', '- ', '', '### 用户可见更新', '- ', '', '### 开发者与治理更新', '- ']
    : ['### Release Overview', '- ', '', '### User-facing', '- ', '', '### Developer & Governance', '- '];

  const entry = [heading, '', ...sections, ''].join('\n');

  const firstReleaseHeading = changelog.search(/^## \[/m);
  if (firstReleaseHeading === -1) {
    return `${changelog.trimEnd()}\n\n${entry}\n`;
  }

  return `${changelog.slice(0, firstReleaseHeading)}${entry}${changelog.slice(firstReleaseHeading)}`;
}

function main() {
  const pkg = readJson(packagePath);
  const packageLock = readJson(packageLockPath);
  const tauriConf = readJson(tauriConfPath);
  const en = readJson(enI18nPath);
  const zh = readJson(zhI18nPath);
  const zhTw = readJson(zhTwI18nPath);
  const changelog = fs.readFileSync(changelogPath, 'utf8');
  const changelogZh = fs.readFileSync(changelogZhPath, 'utf8');
  const cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
  const cargoLock = fs.readFileSync(cargoLockPath, 'utf8');

  const currentVersion = pkg.version;
  const nextVersion = bumpVersion(currentVersion, releaseArg);

  pkg.version = nextVersion;
  packageLock.version = nextVersion;
  packageLock.packages[''].version = nextVersion;
  tauriConf.version = nextVersion;
  updateSettingsVersion(en, nextVersion, 'src/i18n/en.json');
  updateSettingsVersion(zh, nextVersion, 'src/i18n/zh.json');
  updateSettingsVersion(zhTw, nextVersion, 'src/i18n/zh-TW.json');
  const nextChangelog = ensureChangelogEntry(changelog, nextVersion);
  const nextChangelogZh = ensureChangelogEntry(changelogZh, nextVersion, { zh: true });
  // The lockfile moves with the manifest or `cargo --locked` fails, which is
  // how the CLI is installed.
  const nextCargoToml = bumpCargoVersion(cargoToml, nextVersion, 'src-tauri/Cargo.toml');
  const nextCargoLock = bumpCargoVersion(cargoLock, nextVersion, 'src-tauri/Cargo.lock');

  if (dryRun) {
    console.log(`[dry-run] ${currentVersion} -> ${nextVersion}`);
    return;
  }

  writeJson(packagePath, pkg);
  writeJson(packageLockPath, packageLock);
  writeJson(tauriConfPath, tauriConf);
  writeJson(enI18nPath, en);
  writeJson(zhI18nPath, zh);
  writeJson(zhTwI18nPath, zhTw);
  fs.writeFileSync(changelogPath, nextChangelog);
  fs.writeFileSync(changelogZhPath, nextChangelogZh);
  fs.writeFileSync(cargoTomlPath, nextCargoToml);
  fs.writeFileSync(cargoLockPath, nextCargoLock);

  console.log(`Prepared release ${nextVersion}`);
  console.log('Updated:');
  console.log('- CHANGELOG.md');
  console.log('- CHANGELOG-zh.md');
  console.log('- package.json');
  console.log('- package-lock.json');
  console.log('- src-tauri/tauri.conf.json');
  console.log('- src-tauri/Cargo.toml');
  console.log('- src-tauri/Cargo.lock');
  console.log('- src/i18n/en.json');
  console.log('- src/i18n/zh.json');
  console.log('- src/i18n/zh-TW.json');
}

main();
