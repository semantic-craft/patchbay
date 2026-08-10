---
name: manage-skills
description: Wire agent skills into a project through Patchbay's three-tier chain via the local patchbay-cli — inspect the chain, link and unlink skills per project, diagnose and repair broken or noncanonical links, keep global skill surfaces empty, govern CLAUDE.md / AGENTS.md instructions, and pull source repositories. Use this whenever the user wants to give a project a skill, see which skills a project exposes to which agent, fix a broken skill link, find out why an agent can't see a skill, clear a skill out of a global surface, or check the health of their skill repositories. Triggers include "add/link a skill to this project", "which skills does this project have", "why can't Claude see this skill", "fix the broken skill link", "my global skills folder has stuff in it", "check my skill chain", "update my skill repos", "normalize AGENTS.md".
---

## Before doing anything

1. Confirm the CLI is available: `command -v patchbay-cli`. If it's not on PATH, this skill doesn't apply — tell the user to install Patchbay (`npm run cli:install` from the repo).
2. **Always pass `--json` when you parse output yourself.** Pretty output is for the user; JSON is for you. Errors come back as `{"ok": false, "error": "..."}` on stderr with a non-zero exit code.

```bash
patchbay-cli --json chain topology
```

## Mental model

Skills live in **the user's own Git repositories**. Patchbay never copies or hosts them. It maintains a chain:

```
① source repo  ──▶  ② <project>/.agents/skills  ──▶  ③ <project>/.claude/skills, .codex/skills, …
```

- **Tier 1** is a Git checkout under a configured *Original Repository root*.
- **Tier 2** is the project's allowlist: one symlink per skill, pointing back at the source. This is what decides "which skills does this project get".
- **Tier 3** is each agent's entry, normally a directory link to tier 2 so one allowlist serves every agent.

A skill is only correctly wired when a tier-3 entry resolves *through* tier 2 into a source repo. Two shapes are wrong and Doctor reports both: **direct** (an agent entry linked straight at the source, bypassing the allowlist) and **broken** (the target no longer exists).

**Global surfaces must stay empty.** `~/.claude/skills`, `~/.codex/skills` and their siblings are guarded, not managed. Anything found there is a violation to remediate into a project, never something to leave alone.

## Preview then apply

Every mutating command is read-only by default and prints a plan. Nothing is written until you add `--apply`. Show the user the plan before applying it.

## Inspect

```bash
# Full topology: roots, source repos, project chains, global-surface guard
patchbay-cli --json chain topology

# Where one skill name resolves across every tier (skill name is positional)
patchbay-cli --json chain where react-best-practices
patchbay-cli --json chain where react-best-practices --project /path/to/proj

# Source repository inventory with Git health (dirty tree, ahead/behind)
patchbay-cli --json chain repositories

# Duplicate checkouts of the same remote (advisory only — never deletes)
patchbay-cli --json chain duplicates
```

## Diagnose and repair

```bash
# All findings; filter by severity or deviation
patchbay-cli --json chain doctor
patchbay-cli --json chain doctor --severity violation --deviation broken

# Repair a broken/direct/legacy finding by fingerprint
patchbay-cli --json chain normalize --fingerprint <fp>            # preview
patchbay-cli --json chain normalize --fingerprint <fp> --apply

# Record a decision instead of repairing (mark-private | ignore)
patchbay-cli --json chain decide --fingerprint <fp> --action ignore --apply
```

`normalize` only repairs `broken`, `direct`, and `legacy` findings. Anything else needs a link/unlink or a human decision.

## Link and unlink

```bash
# Give a project some skills, exposed through the named agents
patchbay-cli --json chain link --project /path/to/proj \
  --skill /path/to/repo/skills/react-best-practices --agent claude
patchbay-cli --json chain link --project /path/to/proj \
  --skill /path/to/repo/skills/react-best-practices --agent claude --apply

# Take a skill back out. An empty --agent set targets every agent exposing it.
patchbay-cli --json chain unlink --project /path/to/proj --skill react-best-practices
patchbay-cli --json chain unlink --project /path/to/proj --skill react-best-practices --apply
```

Naming a project in `link` enrols it — that is the explicit approval to manage it.

## Clear a global surface

```bash
# Move a guard violation into a project; --global-path is the offending entry
# path exactly as the guard reports it. The global entry is retired only after
# the project-local chain is established AND verified.
patchbay-cli --json chain remediate --global-path ~/.codex/skills/leaked-skill \
  --project /path/to/proj --agent codex
patchbay-cli --json chain remediate --global-path ~/.codex/skills/leaked-skill \
  --project /path/to/proj --agent codex --apply
```

A physical global skill directory is never deleted — worst case the entry stays and you tell the user.

## Update source repositories

```bash
# Fast-forward-only. Dirty, diverged, or up-to-date repos are skipped, never forced.
patchbay-cli --json chain pull
patchbay-cli --json chain pull --apply

# upstream → origin for forks, also fast-forward-only
patchbay-cli --json chain fork-sync --apply
```

## Instructions governance

```bash
patchbay-cli --json instructions scan                                   # canonical body, entries, token cost
patchbay-cli --json instructions where --project /path/to/proj --agent claude
patchbay-cli --json instructions doctor --severity warning --rule dual_body
patchbay-cli --json instructions normalize --project /path/to/proj      # preview
patchbay-cli --json instructions normalize --project /path/to/proj --fingerprint <fp> --apply
patchbay-cli --json instructions init --project /path/to/proj --docs-dir --apply
```

`normalize` snapshots before writing and never rewrites the canonical file. `init` is create-only and idempotent.

## Other groups

```bash
patchbay-cli --json status        # this machine's data directory and database path
patchbay-cli --json tools list    # detected agents and their skills directories
patchbay-cli --json fleet status  # cross-machine repo matrix (see the fleet docs)
```

Patchbay's own state (the SQLite database, cache, logs) lives under `~/.patchbay`. Skills never do — they stay in the user's Git repositories.

## Typical workflows

### "Give this project skill X"

1. `chain repositories` to find the skill's source path.
2. `chain link --project … --skill … --agent …` to preview, show the plan, then re-run with `--apply`.
3. `chain topology` to confirm the entry resolves through `.agents/skills`.

### "Why can't my agent see skill X?"

1. `chain where X` — read the hops.
2. If the status is `broken`, run `chain doctor` and `chain normalize --fingerprint <fp>`.
3. If the status is `direct`, the entry bypasses the allowlist — `chain normalize` fixes that too.
4. If the skill simply isn't in the project's allowlist, `chain link` it.

### "My global skills folder has stuff in it"

1. `chain topology` — read `guard` for the violating skills.
2. For each, `chain remediate --global-path … --project … --agent …`, preview, then `--apply`.

## Pitfalls

- **Never write to a global skills directory** to "fix" something. That creates the exact violation the guard exists to catch.
- **Never `git reset`, force-push, or rewrite history** in a source repository. `pull` and `fork-sync` are fast-forward-only by design; a skipped repo is the correct outcome, not a problem to work around.
- **Don't skip the preview.** The plan is what you show the user before `--apply`; applying blind hides conflicts the plan would have surfaced.
- **The CLI and desktop app share one SQLite database.** After a CLI write, the running app needs a **Rescan** before its display is accurate.
