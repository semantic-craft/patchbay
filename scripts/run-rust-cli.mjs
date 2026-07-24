import { existsSync } from 'node:fs';
import { delimiter, dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';

function canRun(command, args = ['--version']) {
  const result = spawnSync(command, args, { stdio: 'ignore' });
  return result.status === 0;
}

function resolveCargo() {
  if (process.env.CARGO && existsSync(process.env.CARGO)) {
    return process.env.CARGO;
  }

  if (canRun('cargo')) {
    return 'cargo';
  }

  const rustupCheck = spawnSync('rustup', ['which', 'rustc'], { encoding: 'utf8' });
  if (rustupCheck.status === 0) {
    const rustcPath = rustupCheck.stdout.trim();
    if (rustcPath) {
      // `rustup which rustc` reports `...\.cargo\bin\rustc.exe` on Windows, so
      // the sibling to look for is `cargo.exe`. Without the suffix this branch
      // never resolved on Windows and fell through to the exit-127 message
      // even with a working rustup.
      const exe = process.platform === 'win32' ? '.exe' : '';
      const cargoPath = join(dirname(rustcPath), `cargo${exe}`);
      if (existsSync(cargoPath)) {
        return cargoPath;
      }
    }
  }

  console.error('cargo not found. Install Rust or ensure cargo/rustup is on PATH.');
  process.exit(127);
}

const mode = process.argv[2];
const extraArgs = process.argv.slice(3);
const cargo = resolveCargo();

const baseArgs = ['--manifest-path', 'src-tauri/Cargo.toml', '--bin', 'patchbay-cli'];
const cargoArgs =
  mode === 'cli'
    ? ['run', '--quiet', ...baseArgs, '--', ...extraArgs]
    : mode === 'build'
      ? ['build', ...baseArgs]
      : mode === 'install'
        ? ['install', '--path', 'src-tauri', '--bin', 'patchbay-cli', '--locked', '--force']
        : null;

if (!cargoArgs) {
  console.error(`unknown mode: ${mode}`);
  process.exit(2);
}

// Only extend PATH when cargo was resolved to a real location. When it came
// back as the bare name `cargo`, `dirname` yields `.` — which would prepend the
// *current working directory* to the child's PATH and let a stray ./cargo,
// ./rustc or ./git in the repo take precedence over the real tools.
const cargoDir = cargo.includes('/') || cargo.includes('\\') ? dirname(cargo) : null;
const childEnv = cargoDir
  ? { ...process.env, PATH: `${cargoDir}${delimiter}${process.env.PATH ?? ''}` }
  : process.env;

const result = spawnSync(cargo, cargoArgs, {
  stdio: 'inherit',
  env: childEnv,
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);
