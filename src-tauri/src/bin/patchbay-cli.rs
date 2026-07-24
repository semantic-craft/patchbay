use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, bail};
use app_lib::commands::skills as cmd;
use app_lib::core::{
    app_state, central_repo, chain, error::AppError, fleet, git_backup, git_fetcher, installer,
    instructions, merge, repo_lock::RepoLock, scenario_service, skill_metadata,
    skill_store::SkillStore, skillssh_api, sync_engine, sync_metadata, tool_service,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "patchbay-cli")]
#[command(about = "Shared-core CLI for Patchbay", version)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    skills_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Repo(RepoArgs),
    Tools(ToolsArgs),
    Skills(SkillsArgs),
    #[command(alias = "scenarios")]
    Presets(PresetArgs),
    Git(GitArgs),
    Chain(ChainArgs),
    Instructions(InstructionsArgs),
    Fleet(FleetArgs),
}

#[derive(Args, Debug)]
struct FleetArgs {
    #[command(subcommand)]
    command: FleetCommand,
}

#[derive(Subcommand, Debug)]
enum FleetCommand {
    /// Show this machine's fleet settings, or set the meta repo URL
    Config {
        /// Set the fleet meta repo URL (omit to show the current settings)
        #[arg(long = "meta-url")]
        meta_url: Option<String>,
    },
    /// Status matrix: manifest repos × machines (local column measured live)
    Status,
    /// Git directories under the projects root that the manifest does not manage
    Discover,
    /// Push authority-owned clean repos to their manifest hub (preview by default)
    Push {
        /// Limit the operation to one or more manifest repo names
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Apply the previewed plan
        #[arg(long)]
        apply: bool,
    },
    /// Fast-forward eligible clean repos from their manifest hub (preview by default)
    Pull {
        /// Limit the operation to one or more manifest repo names
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Apply the previewed plan
        #[arg(long)]
        apply: bool,
    },
    /// Create missing local hub mirrors and converge manifest hub remotes
    Init {
        /// Limit the operation to one or more manifest repo names
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Apply the previewed plan
        #[arg(long)]
        apply: bool,
    },
    /// Clone manifest repos that are missing on this machine (preview by default)
    Bootstrap {
        /// Limit the operation to one or more manifest repo names
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Apply the previewed plan
        #[arg(long)]
        apply: bool,
    },
    /// Report this machine's repo states to the fleet meta repo
    Report {
        /// Commit and push the report (default is a read-only preview)
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Args, Debug)]
struct RepoArgs {
    #[command(subcommand)]
    command: RepoCommand,
}

#[derive(Subcommand, Debug)]
enum RepoCommand {
    Status,
    SetPath { path: String },
    ResetPath,
}

#[derive(Args, Debug)]
struct ToolsArgs {
    #[command(subcommand)]
    command: ToolsCommand,
}

#[derive(Subcommand, Debug)]
enum ToolsCommand {
    List,
}

#[derive(Args, Debug)]
struct SkillsArgs {
    #[command(subcommand)]
    command: SkillsCommand,
}

#[derive(Subcommand, Debug)]
enum SkillsCommand {
    List,
    Show {
        reference: String,
    },
    Export {
        reference: String,
        #[arg(long)]
        dest: PathBuf,
    },
    Install {
        /// Ref: local path, git URL, or owner/repo[@skill] / owner/repo/skill
        reference: String,
        #[arg(long, conflicts_with_all = ["git", "skillssh"])]
        local: bool,
        #[arg(long, conflicts_with_all = ["local", "skillssh"])]
        git: bool,
        #[arg(long, conflicts_with_all = ["local", "git"])]
        skillssh: bool,
        #[arg(long)]
        name: Option<String>,
        /// Add to current active preset and sync agents
        #[arg(long, conflicts_with = "sync_preset")]
        sync: bool,
        /// Add to given preset (by id or name) and sync agents
        #[arg(long, alias = "sync-scenario", value_name = "REF")]
        sync_preset: Option<String>,
    },
    Update {
        /// Skill ref (id / name / dir basename / central path). Omit for --all.
        reference: Option<String>,
        #[arg(long)]
        all: bool,
    },
    Check {
        reference: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        force: bool,
    },
    Remove {
        references: Vec<String>,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Deprecated no-op: use presets add-skill to enable a skill in a preset.
    Enable {
        references: Vec<String>,
    },
    /// Deprecated no-op: use presets remove-skill to disable a skill in a preset.
    Disable {
        references: Vec<String>,
    },
    Sync {
        /// Preset id or name (default = current active preset)
        #[arg(long, alias = "scenario")]
        preset: Option<String>,
        /// Tool key (default = all enabled tools)
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Search {
        query: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    Adopt {
        /// Agent skill dirs to scan (e.g. ~/.claude/skills), or a single skill dir
        paths: Vec<PathBuf>,
        /// If set, adopt as git source (only with single adoptable skill)
        #[arg(long)]
        git_url: Option<String>,
        /// Subpath inside the git repo where the adopted skill lives. Required
        /// with --git-url when the URL itself does not encode a subpath. Pass
        /// "" if the skill is at the repo root.
        #[arg(long)]
        git_subpath: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Tag(TagArgs),
}

#[derive(Args, Debug)]
struct TagArgs {
    #[command(subcommand)]
    command: TagCommand,
}

#[derive(Subcommand, Debug)]
enum TagCommand {
    Add {
        reference: String,
        tags: Vec<String>,
    },
    Remove {
        reference: String,
        tags: Vec<String>,
    },
    List {
        reference: Option<String>,
    },
}

#[derive(Args, Debug)]
struct PresetArgs {
    #[command(subcommand)]
    command: PresetCommand,
}

#[derive(Subcommand, Debug)]
enum PresetCommand {
    List,
    Current,
    Preview {
        reference: String,
    },
    #[command(alias = "activate", alias = "enable", alias = "start", alias = "open")]
    Apply {
        reference: String,
    },
    #[command(alias = "disable", alias = "stop", alias = "close", alias = "off")]
    Deactivate {
        reference: String,
    },
    AddSkill {
        preset: String,
        skills: Vec<String>,
    },
    RemoveSkill {
        preset: String,
        skills: Vec<String>,
    },
}

#[derive(Args, Debug)]
struct GitArgs {
    #[command(subcommand)]
    command: GitCommand,
}

#[derive(Subcommand, Debug)]
enum GitCommand {
    Status,
    Init,
    Clone {
        url: String,
    },
    SetRemote {
        url: String,
    },
    Pull,
    Push,
    Commit {
        #[arg(short, long)]
        message: String,
    },
    Versions {
        #[arg(long)]
        limit: Option<usize>,
    },
    Restore {
        tag: String,
    },
    /// Remove refs/patchbay/* that a `git push --mirror`/--all style
    /// operation uploaded to the backup remote. Local sync refs are kept.
    PruneSyncRefs,
}

#[derive(Args, Debug)]
struct ChainArgs {
    #[command(subcommand)]
    command: ChainCommand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum DecisionAction {
    MarkPrivate,
    Ignore,
}

impl DecisionAction {
    fn kind(self) -> &'static str {
        match self {
            Self::MarkPrivate => chain::decisions::KIND_PROJECT_PRIVATE,
            Self::Ignore => chain::decisions::KIND_IGNORED,
        }
    }
}

/// The `chain` command group. The read-only queries (topology, resolution,
/// Doctor, repository health, duplicates) return exactly the Chain Service
/// contract the GUI consumes, so `--json` is a stable schema with no localized
/// prose to parse, and none of them mutate the filesystem, settings, or Git.
///
/// The mutating workflows (link, unlink, remediate, normalize, pull, fork-sync)
/// default to a read-only PREVIEW and require an explicit `--apply` to write
/// (AC1). Every one delegates to the SAME `ChainService` operation the GUI calls
/// — no second implementation — so the registered-project, Original Repository,
/// changed-evidence, and no-history-rewrite guards apply unchanged and `--json`
/// yields the same per-item created/existing/removed/skipped/conflict/error
/// schema (AC2/AC3/AC4).
#[derive(Subcommand, Debug)]
enum ChainCommand {
    /// Full three-tier link topology: roots, Original Repositories, project
    /// chains, and the global-surface guard.
    Topology,
    /// Resolve where a Skill name links across every tier.
    Where {
        /// Skill name (from its `SKILL.md`, falling back to directory name).
        skill: String,
        /// Narrow to one registered project path; returns only that project's
        /// tier-2/3 references (Originals are omitted).
        #[arg(long)]
        project: Option<String>,
    },
    /// Read-only Doctor findings, optionally filtered by severity and deviation.
    Doctor {
        /// Repeatable. One of: violation, warning, advice, notice.
        #[arg(long = "severity", value_name = "SEVERITY")]
        severities: Vec<String>,
        /// Repeatable. One of: broken, direct, copy, project_private, legacy, orphan.
        #[arg(long = "deviation", value_name = "DEVIATION")]
        deviations: Vec<String>,
    },
    /// Preview (default) or persist Doctor decisions for current findings.
    Decide {
        /// Repeatable Doctor finding fingerprint from the latest scan.
        #[arg(long = "fingerprint", required = true)]
        fingerprints: Vec<String>,
        /// Decision to record: classify a physical Skill as private, or ignore
        /// an accepted finding.
        #[arg(long)]
        action: DecisionAction,
        /// Persist the previewed decisions. Omitted, the command is read-only.
        #[arg(long)]
        apply: bool,
    },
    /// Original Repository inventory with Git health.
    #[command(alias = "repository-status", alias = "repos")]
    Repositories,
    /// Duplicate Original Repository checkouts grouped by normalized remote
    /// identity, with evidence and advisory-only guidance (never a delete/merge).
    #[command(alias = "duplicate-checkouts", alias = "dupes")]
    Duplicates,
    /// Preview (default) or, with `--apply`, apply linking Original Skills into a
    /// registered project for the given Agents. Preview prints `plan_link`; apply
    /// enrols the project (the explicit enrolment approval) then applies the plan.
    Link {
        /// Registered chain project the links are written into.
        #[arg(long)]
        project: String,
        /// Repeatable. An Original Skill path (a warehouse-resident Skill) to link.
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Repeatable. Agent key whose surface should expose the linked Skills.
        #[arg(long = "agent")]
        agents: Vec<String>,
        /// Write the previewed plan. Omitted, the command only previews (AC1).
        #[arg(long)]
        apply: bool,
    },
    /// Preview (default) or, with `--apply`, apply removing a Skill from a project
    /// for the given Agents, preserving every access that must survive. An empty
    /// `--agent` set targets every Agent that currently exposes the Skill.
    Unlink {
        /// Registered chain project the Skill is removed from.
        #[arg(long)]
        project: String,
        /// Skill name (from its `SKILL.md`, falling back to directory name).
        #[arg(long)]
        skill: String,
        /// Repeatable. Agent key to unlink from; empty means every exposing Agent.
        #[arg(long = "agent")]
        agents: Vec<String>,
        /// Apply the removal. Omitted, the command only previews (AC1).
        #[arg(long)]
        apply: bool,
    },
    /// Preview (default) or, with `--apply`, apply remediating one Global Guard
    /// violation into a registered project. Apply establishes and verifies the
    /// project-local chain BEFORE retiring the global entry and never deletes a
    /// physical global Skill directory.
    Remediate {
        /// The offending global-surface entry path, as reported by the Guard.
        #[arg(long = "global-path")]
        global_path: String,
        /// Registered chain project to remediate the Skill into.
        #[arg(long)]
        project: String,
        /// Repeatable. Agent key whose project surface should expose the Skill.
        #[arg(long = "agent")]
        agents: Vec<String>,
        /// Apply the remediation. Omitted, the command only previews (AC1).
        #[arg(long)]
        apply: bool,
    },
    /// Preview (default) or, with `--apply`, apply normalizing noncanonical chains
    /// identified by Doctor finding fingerprint (the "repair"/normalize
    /// operation). Only broken/direct/legacy findings are repairable.
    Normalize {
        /// Repeatable. Doctor finding fingerprint to normalize.
        #[arg(long = "fingerprint")]
        fingerprints: Vec<String>,
        /// Apply the normalization. Omitted, the command only previews (AC1).
        #[arg(long)]
        apply: bool,
    },
    /// Preview (default) or, with `--apply`, apply fast-forward-only pulls of
    /// Original Repositories. A dirty, diverged, or up-to-date repository is
    /// skipped, never forced or reset.
    Pull {
        /// Repeatable. Original Repository path to pull.
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Apply the pull. Omitted, the command only previews (AC1).
        #[arg(long)]
        apply: bool,
    },
    /// Preview (default) or, with `--apply`, apply fast-forward-only fork
    /// synchronizations (`upstream` → `origin`) of Original Repositories. History
    /// is never rewritten; a non-fast-forwardable fork is skipped, not forced.
    ForkSync {
        /// Repeatable. Original Repository (fork) path to synchronize.
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// Apply the fork synchronization. Omitted, the command only previews (AC1).
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Args, Debug)]
struct InstructionsArgs {
    #[command(subcommand)]
    command: InstructionsCommand,
}

/// The `instructions` command group (design §5). Both commands return the exact
/// `InstructionsService` read-only contract the GUI will consume, so `--json` is
/// a stable schema with no localized prose, and neither mutates the filesystem,
/// settings, or Git (P0 read-only base — no write path exists in the module).
#[derive(Subcommand, Debug)]
enum InstructionsCommand {
    /// Read-only scan of instructions surfaces: the canonical `AGENTS.md` body,
    /// each installed agent's entry state and resident-set cost (project +
    /// global), the unmanaged personal layer, and the machine's global surfaces
    /// with their reader sets. Scans every registered project, or just
    /// `--project` when given.
    Scan {
        /// Narrow to a single project path (any directory; read-only). Omitted,
        /// every registered project is scanned.
        #[arg(long)]
        project: Option<String>,
    },
    /// Per-agent instructions read chain for one project: each installed agent's
    /// ordered files tagged by role (canonical / entry / append / import /
    /// global / conditional) with import hop depth.
    Where {
        /// Project path to resolve the read chain for.
        #[arg(long)]
        project: String,
        /// Narrow to a single agent key (claude / codex / copilot / opencode /
        /// antigravity). Omitted, every installed agent is reported.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Read-only Doctor findings over the instructions surfaces (design §3),
    /// optionally filtered by severity and rule. Same `DoctorReport` shape as
    /// `chain doctor`; `--rule` replaces chain's `--deviation` axis.
    Doctor {
        /// Repeatable. One of: violation, warning, advice, notice.
        #[arg(long = "severity", value_name = "SEVERITY")]
        severities: Vec<String>,
        /// Repeatable. A rule id — full (`instructions.dual_body`) or short
        /// (`dual_body`).
        #[arg(long = "rule", value_name = "RULE")]
        rules: Vec<String>,
        /// Narrow to a single project path (any directory; read-only). Omitted,
        /// every registered project plus the machine's global surfaces.
        #[arg(long)]
        project: Option<String>,
    },
    /// Preview (default) or, with `--apply`, apply normalizing a project's
    /// instructions to the canonical shape (design §4.1): mechanical merge /
    /// canonicalization / wrapper completion. Preview prints the plan; apply
    /// snapshots originals, writes through the §8 guard stack, then rescans and
    /// verifies. Naming `--project` approves adopting (enrolling) it.
    Normalize {
        /// Registered (or, on apply, hereby enrolled) project to normalize.
        #[arg(long)]
        project: String,
        /// Repeatable. A Doctor finding fingerprint to normalize; omitted, every
        /// fixable finding in the project is planned.
        #[arg(long = "fingerprint")]
        fingerprints: Vec<String>,
        /// Apply the previewed plan. Omitted, the command only previews.
        #[arg(long)]
        apply: bool,
    },
    /// Preview (default) or, with `--apply`, apply scaffolding a project's
    /// instructions (design §4.2): the `AGENTS.md` skeleton (never overwritten),
    /// the per-agent wrapper entries, and — with `--docs-dir` — an empty
    /// `docs/agents/` directory plus a pointer in the skeleton. Create-only and
    /// idempotent. Naming `--project` approves adopting (enrolling) it.
    Init {
        /// Registered (or, on apply, hereby enrolled) project to scaffold.
        #[arg(long)]
        project: String,
        /// Also create an empty `docs/agents/` directory and point the skeleton's
        /// Conventions section at it.
        #[arg(long = "docs-dir")]
        docs_dir: bool,
        /// Apply the previewed plan. Omitted, the command only previews.
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Debug, Serialize)]
struct RepoStatus {
    base_dir: String,
    skills_dir: String,
    db_path: String,
    metadata_dir: String,
    skill_count: usize,
    preset_count: usize,
    active_preset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SkillSummary {
    id: String,
    name: String,
    description: Option<String>,
    path: String,
    enabled: bool,
    tags: Vec<String>,
    source_type: String,
    source_ref: Option<String>,
    presets: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SkillDetail {
    #[serde(flatten)]
    summary: SkillSummary,
    skill_file: String,
    files: Vec<String>,
    markdown: String,
}

#[derive(Debug, Serialize)]
struct PresetInfo {
    id: String,
    name: String,
    description: Option<String>,
    icon: Option<String>,
    sort_order: i32,
    skill_count: usize,
    active: bool,
}

#[derive(Debug, Serialize)]
struct InstallReport {
    ok: bool,
    skill_id: String,
    name: String,
    central_path: String,
    source_type: String,
    synced: bool,
    preset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpdateReport {
    skill_id: String,
    name: String,
    source_type: String,
    refreshed: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CheckReport {
    skill_id: String,
    name: String,
    source_type: String,
    update_status: String,
    last_check_error: Option<String>,
    skipped: bool,
}

#[derive(Debug, Serialize)]
struct RemoveReport {
    ok: bool,
    deleted: usize,
    failed: Vec<String>,
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct DeprecatedEnableReport {
    skill_id: String,
    name: String,
    enabled: bool,
    changed: bool,
    deprecated: bool,
    message: String,
}

#[derive(Debug, Serialize)]
struct SyncReport {
    ok: bool,
    preset_id: String,
    preset_name: String,
    tool: Option<String>,
    dry_run: bool,
    targets: Vec<scenario_service::SyncPreviewTarget>,
}

#[derive(Debug, Serialize)]
struct PresetDeactivateReport {
    ok: bool,
    preset_id: String,
    preset_name: String,
    removed_target_count: usize,
    active_preset_id: Option<String>,
    active_preset_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchHit {
    install_ref: String,
    name: String,
    source: String,
    skill_id: String,
    installs: u64,
    skills_sh_url: String,
}

#[derive(Debug, Serialize)]
struct AdoptCandidate {
    path: String,
    name: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct AdoptReport {
    ok: bool,
    dry_run: bool,
    adopted: Vec<InstallReport>,
    candidates: Vec<AdoptCandidate>,
    skipped: Vec<AdoptCandidate>,
}

#[derive(Debug, Serialize)]
struct TagReport {
    skill_id: String,
    name: String,
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PresetMembershipReport {
    preset_id: String,
    preset_name: String,
    added: Vec<String>,
    removed: Vec<String>,
    missing: Vec<String>,
}

enum InstallKind {
    Local,
    Git,
    Skillssh,
}

enum SyncTarget {
    None,
    Active,
    Specific(String),
}

fn main() {
    let json = std::env::args()
        .skip(1)
        .take_while(|a| a != "--")
        .any(|a| a == "--json" || a.starts_with("--json="));

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            if !e.use_stderr() {
                e.exit();
            }
            if json {
                let envelope = serde_json::json!({"ok": false, "error": e.to_string()});
                eprintln!("{}", serde_json::to_string(&envelope).unwrap());
                std::process::exit(2);
            }
            e.exit();
        }
    };

    if let Err(err) = run(cli) {
        if json {
            let envelope = serde_json::json!({"ok": false, "error": format!("{err:#}")});
            eprintln!("{}", serde_json::to_string(&envelope).unwrap());
        } else {
            eprintln!("error: {err:#}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    if let Some(skills_root) = &cli.skills_root {
        let base = central_repo::external_base_dir(skills_root);
        central_repo::set_runtime_base_dir_override(Some(base));
        central_repo::set_runtime_skills_dir_override(Some(skills_root.clone()));
    }

    let store = app_state::initialize_cli_store()?;

    match cli.command {
        Commands::Repo(args) => run_repo(args, &store, cli.json),
        Commands::Tools(args) => run_tools(args, &store, cli.json),
        Commands::Skills(args) => run_skills(args, &store, cli.json),
        Commands::Presets(args) => run_presets(args, &store, cli.json),
        Commands::Git(args) => run_git(args, &store, cli.skills_root.is_some(), cli.json),
        Commands::Chain(args) => run_chain(args, &store, cli.json),
        Commands::Instructions(args) => run_instructions(args, &store, cli.json),
        Commands::Fleet(args) => run_fleet(args, &store, cli.json),
    }
}

// ── repo ──────────────────────────────────────────────────────────────────

fn run_repo(args: RepoArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    match args.command {
        RepoCommand::Status => print_json(&repo_status(store), json),
        RepoCommand::SetPath { path } => {
            central_repo::set_base_dir_override(Some(path))?;
            let store = app_state::initialize_cli_store()?;
            print_json(&repo_status(&store), json);
        }
        RepoCommand::ResetPath => {
            central_repo::set_base_dir_override(None)?;
            let store = app_state::initialize_cli_store()?;
            print_json(&repo_status(&store), json);
        }
    }
    Ok(())
}

fn repo_status(store: &SkillStore) -> RepoStatus {
    RepoStatus {
        base_dir: central_repo::base_dir().to_string_lossy().to_string(),
        skills_dir: central_repo::skills_dir().to_string_lossy().to_string(),
        db_path: central_repo::db_path().to_string_lossy().to_string(),
        metadata_dir: sync_metadata::metadata_dir().to_string_lossy().to_string(),
        skill_count: store.get_all_skills().unwrap_or_default().len(),
        preset_count: store.get_all_scenarios().unwrap_or_default().len(),
        active_preset_id: store.get_active_scenario_id().unwrap_or(None),
    }
}

// ── tools ─────────────────────────────────────────────────────────────────

fn run_tools(args: ToolsArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    match args.command {
        ToolsCommand::List => print_json(&tool_service::list_tool_info(store), json),
    }
    Ok(())
}

// ── skills ────────────────────────────────────────────────────────────────

fn run_skills(args: SkillsArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    match args.command {
        SkillsCommand::List => print_json(&list_skills(store)?, json),
        SkillsCommand::Show { reference } => print_json(&show_skill(store, &reference)?, json),
        SkillsCommand::Export { reference, dest } => {
            let result = export_skill(store, &reference, &dest)?;
            print_json(
                &serde_json::json!({"ok": true, "destination": result}),
                json,
            );
        }
        SkillsCommand::Install {
            reference,
            local,
            git,
            skillssh,
            name,
            sync,
            sync_preset,
        } => {
            let kind = classify_ref(&reference, local, git, skillssh)?;
            let sync_target = if let Some(ref s) = sync_preset {
                SyncTarget::Specific(s.clone())
            } else if sync {
                SyncTarget::Active
            } else {
                SyncTarget::None
            };
            let report = run_install(store, &reference, name.as_deref(), kind, sync_target)?;
            print_json(&report, json);
        }
        SkillsCommand::Update { reference, all } => {
            let reports = run_update(store, reference.as_deref(), all)?;
            print_json(&reports, json);
        }
        SkillsCommand::Check {
            reference,
            all,
            force,
        } => {
            let reports = run_check(store, reference.as_deref(), all, force)?;
            print_json(&reports, json);
        }
        SkillsCommand::Remove {
            references,
            yes,
            dry_run,
        } => {
            let report = run_remove(store, &references, yes, dry_run)?;
            print_json(&report, json);
        }
        SkillsCommand::Enable { references } => {
            let reports = run_deprecated_set_enabled(store, &references, true)?;
            print_json(&reports, json);
        }
        SkillsCommand::Disable { references } => {
            let reports = run_deprecated_set_enabled(store, &references, false)?;
            print_json(&reports, json);
        }
        SkillsCommand::Sync {
            preset,
            tool,
            dry_run,
        } => {
            let report = run_sync(store, preset.as_deref(), tool.as_deref(), dry_run)?;
            print_json(&report, json);
        }
        SkillsCommand::Search { query, limit } => {
            let hits = run_search(store, &query, limit)?;
            print_json(&hits, json);
        }
        SkillsCommand::Adopt {
            paths,
            git_url,
            git_subpath,
            dry_run,
        } => {
            let report = run_adopt(
                store,
                &paths,
                git_url.as_deref(),
                git_subpath.as_deref(),
                dry_run,
            )?;
            print_json(&report, json);
        }
        SkillsCommand::Tag(args) => run_tag(args, store, json)?,
    }
    Ok(())
}

fn list_skills(store: &SkillStore) -> anyhow::Result<Vec<SkillSummary>> {
    let tags_map = store.get_tags_map()?;
    let scenarios = store.get_all_scenarios()?;
    let scenario_lookup: std::collections::HashMap<String, String> =
        scenarios.into_iter().map(|s| (s.id, s.name)).collect();

    let mut items = Vec::new();
    for skill in store.get_all_skills()? {
        let preset_names = store
            .get_scenarios_for_skill(&skill.id)?
            .into_iter()
            .filter_map(|id| scenario_lookup.get(&id).cloned())
            .collect();
        items.push(SkillSummary {
            id: skill.id.clone(),
            name: skill.name.clone(),
            description: skill.description.clone(),
            path: skill.central_path.clone(),
            enabled: skill.enabled,
            tags: tags_map.get(&skill.id).cloned().unwrap_or_default(),
            source_type: skill.source_type.clone(),
            source_ref: skill.source_ref.clone(),
            presets: preset_names,
        });
    }
    Ok(items)
}

fn show_skill(store: &SkillStore, reference: &str) -> anyhow::Result<SkillDetail> {
    let skill = resolve_skill(store, reference)?;

    let summary = list_skills(store)?
        .into_iter()
        .find(|item| item.id == skill.id)
        .ok_or_else(|| anyhow!("skill summary missing"))?;

    let skill_dir = PathBuf::from(&skill.central_path);
    let skill_file = [skill_dir.join("SKILL.md"), skill_dir.join("skill.md")]
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow!("no SKILL.md found for {}", skill.name))?;
    let markdown = std::fs::read_to_string(&skill_file)?;

    Ok(SkillDetail {
        summary,
        skill_file: skill_file.to_string_lossy().to_string(),
        files: collect_files(&skill_dir)?,
        markdown,
    })
}

fn export_skill(store: &SkillStore, reference: &str, dest: &Path) -> anyhow::Result<String> {
    let skill = resolve_skill(store, reference)?;
    sync_engine::sync_skill(
        Path::new(&skill.central_path),
        dest,
        sync_engine::SyncMode::Copy,
    )?;
    Ok(dest.to_string_lossy().to_string())
}

fn resolve_skill(
    store: &SkillStore,
    reference: &str,
) -> anyhow::Result<app_lib::core::skill_store::SkillRecord> {
    let matches: Vec<_> = store
        .get_all_skills()?
        .into_iter()
        .filter(|skill| {
            skill.id == reference
                || skill.name == reference
                || skill.central_path == reference
                || Path::new(&skill.central_path)
                    .file_name()
                    .and_then(|v| v.to_str())
                    == Some(reference)
        })
        .collect();

    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => Err(anyhow!("skill not found: {reference}")),
        _ => Err(anyhow!("skill reference is ambiguous: {reference}")),
    }
}

fn collect_files(root: &Path) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    collect_files_inner(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_files_inner(root: &Path, current: &Path, out: &mut Vec<String>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_inner(root, &path, out)?;
        } else {
            out.push(path.strip_prefix(root)?.to_string_lossy().to_string());
        }
    }
    Ok(())
}

// ── install ───────────────────────────────────────────────────────────────

/// Whether a reference names a filesystem path rather than something remote.
///
/// The POSIX forms alone left every native Windows path "ambiguous": `.\skill`,
/// `D:\skills\thing` and `~\skills\thing` all fell past this check, past the git
/// and skill.sh checks, and out through `bail!`, which then advised passing
/// `--local` — for a path that is obviously local.
///
/// The Windows forms are recognised on every platform rather than behind
/// `cfg(windows)`: a backslash-rooted path or a `C:\`-style prefix is not a
/// valid git URL or skill.sh shorthand anywhere, so accepting it costs unix
/// nothing and keeps one classifier to reason about.
fn looks_like_local_path(reference: &str) -> bool {
    reference.starts_with("./")
        || reference.starts_with("../")
        || reference.starts_with(".\\")
        || reference.starts_with("..\\")
        || reference.starts_with('/')
        // A leading backslash covers both `\path` and UNC `\\server\share`.
        || reference.starts_with('\\')
        || reference.starts_with("~/")
        || reference.starts_with("~\\")
        || starts_with_drive_letter(reference)
}

/// `C:\...` or `C:/...`. Deliberately requires the separator: without it `x:y`
/// would swallow scp-style git remotes like `alpha:git-mirrors/repo.git`, whose
/// host happened to be one character long.
fn starts_with_drive_letter(reference: &str) -> bool {
    let bytes = reference.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn classify_ref(
    reference: &str,
    force_local: bool,
    force_git: bool,
    force_skillssh: bool,
) -> anyhow::Result<InstallKind> {
    if force_local {
        return Ok(InstallKind::Local);
    }
    if force_git {
        return Ok(InstallKind::Git);
    }
    if force_skillssh {
        return Ok(InstallKind::Skillssh);
    }

    if looks_like_local_path(reference) {
        return Ok(InstallKind::Local);
    }

    if reference.contains("://") || reference.ends_with(".git") || reference.starts_with("git@") {
        return Ok(InstallKind::Git);
    }

    if is_skillssh_shorthand(reference) {
        return Ok(InstallKind::Skillssh);
    }

    bail!(
        "ambiguous ref '{}'; pass --local, --git, or --skillssh to disambiguate",
        reference
    )
}

fn is_skillssh_shorthand(s: &str) -> bool {
    // owner/repo, owner/repo/skill, owner/repo@skill
    fn seg_ok(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
    }
    let (head, _at_skill) = match s.split_once('@') {
        Some((h, t)) if seg_ok(t) => (h, Some(t)),
        Some(_) => return false,
        None => (s, None),
    };
    let parts: Vec<&str> = head.split('/').collect();
    (parts.len() == 2 || parts.len() == 3) && parts.iter().all(|p| seg_ok(p))
}

fn resolve_sync_target(store: &SkillStore, target: &SyncTarget) -> anyhow::Result<Option<String>> {
    match target {
        SyncTarget::None => Ok(None),
        SyncTarget::Active => Ok(store.get_active_scenario_id()?),
        SyncTarget::Specific(ref_) => {
            let scenario = resolve_scenario(store, ref_)?;
            Ok(Some(scenario.id))
        }
    }
}

fn run_install(
    store: &SkillStore,
    reference: &str,
    name: Option<&str>,
    kind: InstallKind,
    sync: SyncTarget,
) -> anyhow::Result<InstallReport> {
    let preset_id = resolve_sync_target(store, &sync)?;
    let synced = preset_id.is_some();

    let (skill_id, install_name, central_path, source_type) = match kind {
        InstallKind::Local => install_local_action(store, reference, name, preset_id.as_deref())?,
        InstallKind::Git => install_git_action(store, reference, name, preset_id.as_deref())?,
        InstallKind::Skillssh => install_skillssh_action(store, reference, preset_id.as_deref())?,
    };

    Ok(InstallReport {
        ok: true,
        skill_id,
        name: install_name,
        central_path,
        source_type,
        synced,
        preset_id,
    })
}

fn install_local_action(
    store: &SkillStore,
    reference: &str,
    name: Option<&str>,
    active_scenario: Option<&str>,
) -> anyhow::Result<(String, String, String, String)> {
    let path = expand_path(reference)?;
    if !path.exists() {
        bail!("local path does not exist: {}", path.display());
    }

    let _lock = RepoLock::acquire_foreground("cli install local")?;
    let result = installer::install_from_local(&path, name)?;
    let metadata = cmd::InstallSourceMetadata {
        source_type: "local".to_string(),
        source_ref: Some(path.to_string_lossy().to_string()),
        source_ref_resolved: None,
        source_subpath: None,
        source_branch: None,
        source_revision: None,
        remote_revision: None,
        update_status: "local_only".to_string(),
    };
    let central_path = result.central_path.to_string_lossy().to_string();
    let install_name = result.name.clone();
    let skill_id = cmd::store_installed_skill_unlocked(store, &result, &metadata, active_scenario)
        .map_err(map_app_err)?;
    Ok((skill_id, install_name, central_path, "local".to_string()))
}

fn install_git_action(
    store: &SkillStore,
    repo_url: &str,
    name: Option<&str>,
    active_scenario: Option<&str>,
) -> anyhow::Result<(String, String, String, String)> {
    git_fetcher::validate_git_url(repo_url)?;
    let proxy_url = store.proxy_url();
    let parsed = git_fetcher::parse_git_source_resolved(repo_url, proxy_url.as_deref());
    let cancel = Arc::new(AtomicBool::new(false));
    let temp_dir = git_fetcher::clone_repo_ref(
        &parsed.clone_url,
        parsed.branch.as_deref(),
        Some(&cancel),
        proxy_url.as_deref(),
    )?;
    let result = (|| -> anyhow::Result<(String, String, String)> {
        let _lock = RepoLock::acquire_foreground("cli install git")?;
        let skill_dir = cmd::resolve_skill_dir(&temp_dir, parsed.subpath.as_deref(), None)
            .map_err(map_app_err)?;
        let revision = git_fetcher::get_head_revision(&temp_dir)?;
        let install_result = installer::install_from_git_dir(&skill_dir, name)?;
        let metadata = cmd::InstallSourceMetadata {
            source_type: "git".to_string(),
            source_ref: Some(parsed.original_url.clone()),
            source_ref_resolved: Some(parsed.clone_url.clone()),
            source_subpath: git_fetcher::relative_subpath(&temp_dir, &skill_dir),
            source_branch: parsed.branch.clone(),
            source_revision: Some(revision.clone()),
            remote_revision: Some(revision),
            update_status: "up_to_date".to_string(),
        };
        let central_path = install_result.central_path.to_string_lossy().to_string();
        let install_name = install_result.name.clone();
        let skill_id =
            cmd::store_installed_skill_unlocked(store, &install_result, &metadata, active_scenario)
                .map_err(map_app_err)?;
        Ok((skill_id, install_name, central_path))
    })();
    git_fetcher::cleanup_temp(&temp_dir);
    let (skill_id, install_name, central_path) = result?;
    Ok((skill_id, install_name, central_path, "git".to_string()))
}

fn install_skillssh_action(
    store: &SkillStore,
    shorthand: &str,
    active_scenario: Option<&str>,
) -> anyhow::Result<(String, String, String, String)> {
    let (source, skill_id_field) = parse_skillssh_shorthand(shorthand)?;
    let proxy_url = store.proxy_url();
    let repo_url = format!("https://github.com/{}.git", source);
    let cancel = Arc::new(AtomicBool::new(false));
    let temp_dir =
        git_fetcher::clone_repo_ref(&repo_url, None, Some(&cancel), proxy_url.as_deref())?;
    let result = (|| -> anyhow::Result<(String, String, String)> {
        let _lock = RepoLock::acquire_foreground("cli install skillssh")?;
        let skill_dir =
            cmd::resolve_skill_dir(&temp_dir, None, Some(&skill_id_field)).map_err(map_app_err)?;
        let revision = git_fetcher::get_head_revision(&temp_dir)?;
        let source_ref = format!("{}/{}", source, skill_id_field);
        let (install_name, destination) =
            cmd::resolve_skillssh_install_target(store, &source_ref, &skill_id_field)
                .map_err(map_app_err)?;
        let install_result =
            installer::install_skill_dir_to_destination(&skill_dir, &install_name, &destination)?;
        let metadata = cmd::InstallSourceMetadata {
            source_type: "skillssh".to_string(),
            source_ref: Some(source_ref),
            source_ref_resolved: Some(repo_url.clone()),
            source_subpath: git_fetcher::relative_subpath(&temp_dir, &skill_dir),
            source_branch: None,
            source_revision: Some(revision.clone()),
            remote_revision: Some(revision),
            update_status: "up_to_date".to_string(),
        };
        let central_path = install_result.central_path.to_string_lossy().to_string();
        let skill_id =
            cmd::store_installed_skill_unlocked(store, &install_result, &metadata, active_scenario)
                .map_err(map_app_err)?;
        Ok((skill_id, install_name, central_path))
    })();
    git_fetcher::cleanup_temp(&temp_dir);
    let (skill_id, install_name, central_path) = result?;
    Ok((skill_id, install_name, central_path, "skillssh".to_string()))
}

/// Parse `owner/repo`, `owner/repo@skill`, or `owner/repo/skill` into
/// (source = "owner/repo", skill_id) — matching SkillsMP / install_from_skillssh.
fn parse_skillssh_shorthand(s: &str) -> anyhow::Result<(String, String)> {
    if let Some((head, skill_id)) = s.split_once('@') {
        if head.split('/').count() != 2 {
            bail!("invalid shorthand: '{s}' (expected owner/repo@skill)");
        }
        return Ok((head.to_string(), skill_id.to_string()));
    }
    let parts: Vec<&str> = s.split('/').collect();
    match parts.len() {
        2 => Ok((s.to_string(), parts[1].to_string())),
        3 => Ok((format!("{}/{}", parts[0], parts[1]), parts[2].to_string())),
        _ => bail!("invalid shorthand: '{s}'"),
    }
}

fn expand_path(s: &str) -> anyhow::Result<PathBuf> {
    // Both separators, as `central_repo::normalize_path` already accepts: a
    // Windows user types `~\Projects`.
    if s.starts_with("~/") || s.starts_with("~\\") {
        return Ok(dirs_home()?.join(&s[2..]));
    }
    if s == "~" {
        return dirs_home();
    }
    Ok(PathBuf::from(s))
}

fn dirs_home() -> anyhow::Result<PathBuf> {
    // Not `$HOME`: that is unset on Windows, which would fail every `~`
    // argument. `dirs` reads the right source per platform, and is what the
    // rest of the crate already uses.
    dirs::home_dir().ok_or_else(|| anyhow!("could not determine the home directory"))
}

// ── update / check ────────────────────────────────────────────────────────

fn run_update(
    store: &SkillStore,
    reference: Option<&str>,
    all: bool,
) -> anyhow::Result<Vec<UpdateReport>> {
    let targets = select_skill_ids(store, reference, all)?;
    let proxy_url = store.proxy_url();
    let mut reports = Vec::new();

    for skill in targets {
        let report = match skill.source_type.as_str() {
            "git" | "skillssh" => {
                match cmd::update_git_skill_internal(store, &skill.id, proxy_url.as_deref(), None) {
                    Ok(r) => UpdateReport {
                        skill_id: skill.id.clone(),
                        name: skill.name.clone(),
                        source_type: skill.source_type.clone(),
                        refreshed: r.content_changed,
                        error: None,
                    },
                    Err(e) => UpdateReport {
                        skill_id: skill.id.clone(),
                        name: skill.name.clone(),
                        source_type: skill.source_type.clone(),
                        refreshed: false,
                        error: Some(e.message.clone()),
                    },
                }
            }
            "local" | "import" => match cmd::reimport_local_skill_internal(store, &skill.id) {
                Ok(_) => UpdateReport {
                    skill_id: skill.id.clone(),
                    name: skill.name.clone(),
                    source_type: skill.source_type.clone(),
                    refreshed: true,
                    error: None,
                },
                Err(e) => UpdateReport {
                    skill_id: skill.id.clone(),
                    name: skill.name.clone(),
                    source_type: skill.source_type.clone(),
                    refreshed: false,
                    error: Some(e.message.clone()),
                },
            },
            other => UpdateReport {
                skill_id: skill.id.clone(),
                name: skill.name.clone(),
                source_type: skill.source_type.clone(),
                refreshed: false,
                error: Some(format!("source type '{other}' cannot be refreshed")),
            },
        };
        reports.push(report);
    }

    Ok(reports)
}

fn run_check(
    store: &SkillStore,
    reference: Option<&str>,
    all: bool,
    force: bool,
) -> anyhow::Result<Vec<CheckReport>> {
    let targets = select_skill_ids(store, reference, all)?;
    let proxy_url = store.proxy_url();
    let mut reports = Vec::new();

    for skill in targets {
        if !matches!(skill.source_type.as_str(), "git" | "skillssh") {
            reports.push(CheckReport {
                skill_id: skill.id.clone(),
                name: skill.name.clone(),
                source_type: skill.source_type.clone(),
                update_status: skill.update_status.clone(),
                last_check_error: skill.last_check_error.clone(),
                skipped: true,
            });
            continue;
        }
        let report =
            match cmd::check_skill_update_internal(store, &skill.id, force, proxy_url.as_deref()) {
                Ok(dto) => CheckReport {
                    skill_id: dto.id,
                    name: dto.name,
                    source_type: dto.source_type,
                    update_status: dto.update_status,
                    last_check_error: dto.last_check_error,
                    skipped: false,
                },
                Err(e) => CheckReport {
                    skill_id: skill.id.clone(),
                    name: skill.name.clone(),
                    source_type: skill.source_type.clone(),
                    update_status: "error".to_string(),
                    last_check_error: Some(e.message.clone()),
                    skipped: false,
                },
            };
        reports.push(report);
    }

    Ok(reports)
}

fn select_skill_ids(
    store: &SkillStore,
    reference: Option<&str>,
    all: bool,
) -> anyhow::Result<Vec<app_lib::core::skill_store::SkillRecord>> {
    if let Some(r) = reference {
        if all {
            bail!("pass either a ref or --all, not both");
        }
        Ok(vec![resolve_skill(store, r)?])
    } else {
        // No ref → treat as --all (the flag is just explicit confirmation)
        let _ = all;
        Ok(store.get_all_skills()?)
    }
}

// ── remove ────────────────────────────────────────────────────────────────

fn run_remove(
    store: &SkillStore,
    references: &[String],
    yes: bool,
    dry_run: bool,
) -> anyhow::Result<RemoveReport> {
    if references.is_empty() {
        bail!("no skill ref provided");
    }
    let mut ids = Vec::new();
    let mut failed = Vec::new();
    for r in references {
        match resolve_skill(store, r) {
            Ok(skill) => ids.push(skill.id),
            Err(e) => failed.push(format!("{r}: {e}")),
        }
    }

    if dry_run {
        return Ok(RemoveReport {
            ok: true,
            deleted: ids.len(),
            failed,
            dry_run: true,
        });
    }
    if !yes {
        bail!("refusing to delete {} skill(s) without --yes", ids.len());
    }

    let result = cmd::delete_managed_skills_by_ids(store, &ids).map_err(map_app_err)?;
    for missing in result.failed {
        failed.push(format!("{missing}: not found"));
    }
    Ok(RemoveReport {
        ok: true,
        deleted: result.deleted,
        failed,
        dry_run: false,
    })
}

// ── enable / disable ──────────────────────────────────────────────────────

fn run_deprecated_set_enabled(
    store: &SkillStore,
    references: &[String],
    requested_enabled: bool,
) -> anyhow::Result<Vec<DeprecatedEnableReport>> {
    if references.is_empty() {
        bail!("no skill ref provided");
    }
    let mut reports = Vec::new();
    for r in references {
        let skill = resolve_skill(store, r)?;
        // `skills enable` repairs legacy enabled=false rows; `skills disable`
        // is a true no-op. Flipping enabled to true on disable would be the
        // opposite of what the user asked for.
        let changed = if requested_enabled && !skill.enabled {
            store.update_skill_enabled(&skill.id, true)?;
            true
        } else {
            false
        };
        let enabled_after = if requested_enabled {
            true
        } else {
            skill.enabled
        };
        let message = if requested_enabled {
            "Deprecated no-op: skills are enabled by adding them to a preset; this command only restores legacy sync inclusion."
        } else {
            "Deprecated no-op: skills are disabled by removing them from a preset; this command does not modify the legacy enabled flag."
        };
        reports.push(DeprecatedEnableReport {
            skill_id: skill.id,
            name: skill.name,
            enabled: enabled_after,
            changed,
            deprecated: true,
            message: message.to_string(),
        });
    }
    if reports.iter().any(|report| report.changed) {
        sync_metadata::write_all_from_db(store)?;
    }
    Ok(reports)
}

// ── sync ──────────────────────────────────────────────────────────────────

fn run_sync(
    store: &SkillStore,
    preset_ref: Option<&str>,
    tool_key: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<SyncReport> {
    let preset = match preset_ref {
        Some(s) => resolve_scenario(store, s)?,
        None => {
            let active = store
                .get_active_scenario_id()?
                .ok_or_else(|| anyhow!("no active preset; pass --preset"))?;
            store
                .get_all_scenarios()?
                .into_iter()
                .find(|s| s.id == active)
                .ok_or_else(|| anyhow!("active preset not found"))?
        }
    };

    let preview =
        scenario_service::preview_scenario_sync(store, &preset.id).map_err(map_app_err)?;

    let filtered: Vec<_> = if let Some(t) = tool_key {
        preview.into_iter().filter(|p| p.tool == t).collect()
    } else {
        preview
    };

    if dry_run {
        return Ok(SyncReport {
            ok: true,
            preset_id: preset.id,
            preset_name: preset.name,
            tool: tool_key.map(|s| s.to_string()),
            dry_run: true,
            targets: filtered,
        });
    }

    // Make preset active if it isn't, then sync.
    let active = store.get_active_scenario_id()?;
    if active.as_deref() != Some(preset.id.as_str()) {
        store.set_active_scenario(&preset.id)?;
    }

    if let Some(t) = tool_key {
        // Build targets locally and filter to the requested tool so we don't
        // fan out to every enabled adapter (which is what
        // sync_active_scenario_to_tool ends up doing via
        // sync_skill_to_active_scenario).
        let all_targets = scenario_service::collect_scenario_sync_targets(store, &preset.id)
            .map_err(map_app_err)?;
        let desired: Vec<_> = all_targets.into_iter().filter(|tg| tg.tool == t).collect();
        scenario_service::sync_desired_targets(store, &desired).map_err(map_app_err)?;
    } else {
        scenario_service::apply_scenario_to_default(store, &preset.id).map_err(map_app_err)?;
    }

    Ok(SyncReport {
        ok: true,
        preset_id: preset.id,
        preset_name: preset.name,
        tool: tool_key.map(|s| s.to_string()),
        dry_run: false,
        targets: filtered,
    })
}

// ── search ────────────────────────────────────────────────────────────────

fn run_search(
    store: &SkillStore,
    query: &str,
    limit: Option<usize>,
) -> anyhow::Result<Vec<SearchHit>> {
    let proxy_url = store.proxy_url();
    let bounded = limit.unwrap_or(60).clamp(1, 300);
    let hits = skillssh_api::search_skills(query, bounded, proxy_url.as_deref())?;
    Ok(hits
        .into_iter()
        .map(|s| {
            let install_ref = format!("{}/{}", s.source, s.skill_id);
            let skills_sh_url = format!("https://skills.sh/{}/{}", s.source, s.skill_id);
            SearchHit {
                install_ref,
                name: s.name,
                source: s.source,
                skill_id: s.skill_id,
                installs: s.installs,
                skills_sh_url,
            }
        })
        .collect())
}

// ── adopt ─────────────────────────────────────────────────────────────────

fn run_adopt(
    store: &SkillStore,
    paths: &[PathBuf],
    git_url: Option<&str>,
    git_subpath: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<AdoptReport> {
    if paths.is_empty() {
        bail!("provide at least one path to scan");
    }
    if git_url.is_some() && paths.len() != 1 {
        bail!("--git-url requires exactly one path");
    }
    if git_subpath.is_some() && git_url.is_none() {
        bail!("--git-subpath requires --git-url");
    }

    // Resolve the source subpath for git-based adopts up front so we fail fast
    // before any filesystem work. parse_git_source pulls a subpath out of GitHub
    // /tree/branch/path URLs; --git-subpath is the explicit override (pass ""
    // to mean "skill lives at the repo root").
    let resolved_git: Option<(String, Option<String>, Option<String>, Option<String>)> =
        if let Some(url) = git_url {
            git_fetcher::validate_git_url(url)?;
            let parsed = git_fetcher::parse_git_source(url);
            let subpath = match git_subpath {
                Some(s) => {
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                }
                None => parsed.subpath.clone(),
            };
            if subpath.is_none() && git_subpath.is_none() {
                bail!(
                    "--git-url has no subpath and --git-subpath was not provided. \
                     Pass --git-subpath \"\" if the skill lives at the repo root, \
                     --git-subpath <path> for a subdirectory, or use a URL like \
                     https://github.com/owner/repo/tree/branch/path/to/skill"
                );
            }
            Some((
                parsed.clone_url,
                subpath,
                parsed.branch,
                Some(url.to_string()),
            ))
        } else {
            None
        };

    // Build exclusion set: existing central paths, sync target paths, canonicals
    let mut excluded: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for skill in store.get_all_skills()? {
        let p = PathBuf::from(&skill.central_path);
        excluded.insert(p.clone());
        if let Ok(c) = p.canonicalize() {
            excluded.insert(c);
        }
    }
    for target in store.get_all_targets()? {
        let p = PathBuf::from(&target.target_path);
        excluded.insert(p.clone());
        if let Ok(c) = p.canonicalize() {
            excluded.insert(c);
        }
    }
    let central_root = central_repo::skills_dir();
    let central_root_canonical = central_root.canonicalize().unwrap_or(central_root.clone());

    let mut candidates: Vec<AdoptCandidate> = Vec::new();
    let mut skipped: Vec<AdoptCandidate> = Vec::new();

    for path in paths {
        let path = expand_path(&path.to_string_lossy())?;
        if !path.is_dir() {
            skipped.push(AdoptCandidate {
                path: path.to_string_lossy().to_string(),
                name: String::new(),
                reason: "not a directory".to_string(),
            });
            continue;
        }

        // If the user pointed directly at a single skill dir, treat it as one
        // candidate rather than scanning its children (which would be the
        // skill's own files/references and miss the SKILL.md at the root).
        if skill_metadata::is_valid_skill_dir(&path) {
            classify_adopt_candidate(
                &path,
                false, // path itself can't be a symlink-into-central in this branch
                &excluded,
                &central_root_canonical,
                &mut candidates,
                &mut skipped,
            );
            continue;
        }

        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let is_symlink = entry.file_type()?.is_symlink();
            classify_adopt_candidate(
                &dir,
                is_symlink,
                &excluded,
                &central_root_canonical,
                &mut candidates,
                &mut skipped,
            );
        }
    }

    if dry_run {
        return Ok(AdoptReport {
            ok: true,
            dry_run: true,
            adopted: Vec::new(),
            candidates,
            skipped,
        });
    }

    if git_url.is_some() && candidates.len() != 1 {
        bail!(
            "--git-url requires exactly one adoptable skill, found {}",
            candidates.len()
        );
    }

    let mut adopted = Vec::new();
    for c in &candidates {
        let dir = PathBuf::from(&c.path);
        let _lock = RepoLock::acquire_foreground("cli adopt")?;
        let result = installer::install_from_local(&dir, None)?;
        let metadata = if let Some((clone_url, subpath, branch, original_url)) = &resolved_git {
            cmd::InstallSourceMetadata {
                source_type: "git".to_string(),
                source_ref: original_url.clone(),
                source_ref_resolved: Some(clone_url.clone()),
                source_subpath: subpath.clone(),
                source_branch: branch.clone(),
                source_revision: None,
                remote_revision: None,
                update_status: "unknown".to_string(),
            }
        } else {
            cmd::InstallSourceMetadata {
                source_type: "local".to_string(),
                source_ref: Some(dir.to_string_lossy().to_string()),
                source_ref_resolved: None,
                source_subpath: None,
                source_branch: None,
                source_revision: None,
                remote_revision: None,
                update_status: "local_only".to_string(),
            }
        };
        let central_path = result.central_path.to_string_lossy().to_string();
        let install_name = result.name.clone();
        let source_type = metadata.source_type.clone();
        let skill_id = cmd::store_installed_skill_unlocked(store, &result, &metadata, None)
            .map_err(map_app_err)?;
        adopted.push(InstallReport {
            ok: true,
            skill_id,
            name: install_name,
            central_path,
            source_type,
            synced: false,
            preset_id: None,
        });
    }

    Ok(AdoptReport {
        ok: true,
        dry_run: false,
        adopted,
        candidates: Vec::new(),
        skipped,
    })
}

fn classify_adopt_candidate(
    dir: &Path,
    is_symlink: bool,
    excluded: &std::collections::HashSet<PathBuf>,
    central_root_canonical: &Path,
    candidates: &mut Vec<AdoptCandidate>,
    skipped: &mut Vec<AdoptCandidate>,
) {
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if excluded.contains(dir) || excluded.contains(&canonical) {
        skipped.push(AdoptCandidate {
            path: dir.to_string_lossy().to_string(),
            name,
            reason: "already managed (in DB or sync target)".to_string(),
        });
        return;
    }

    if is_symlink && canonical.starts_with(central_root_canonical) {
        skipped.push(AdoptCandidate {
            path: dir.to_string_lossy().to_string(),
            name,
            reason: "symlink into central repo (already managed)".to_string(),
        });
        return;
    }

    if !skill_metadata::is_valid_skill_dir(dir) {
        skipped.push(AdoptCandidate {
            path: dir.to_string_lossy().to_string(),
            name,
            reason: "no SKILL.md / skill.md".to_string(),
        });
        return;
    }

    candidates.push(AdoptCandidate {
        path: dir.to_string_lossy().to_string(),
        name,
        reason: "ready".to_string(),
    });
}

// ── tag ───────────────────────────────────────────────────────────────────

fn run_tag(args: TagArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    match args.command {
        TagCommand::Add { reference, tags } => {
            let skill = resolve_skill(store, &reference)?;
            let mut current = store
                .get_tags_map()?
                .get(&skill.id)
                .cloned()
                .unwrap_or_default();
            for t in tags {
                if !current.iter().any(|c| c == &t) {
                    current.push(t);
                }
            }
            store.set_tags_for_skill(&skill.id, &current)?;
            sync_metadata::ensure_skill_metadata(store, &skill.id)?;
            print_json(
                &TagReport {
                    skill_id: skill.id,
                    name: skill.name,
                    tags: current,
                },
                json,
            );
        }
        TagCommand::Remove { reference, tags } => {
            let skill = resolve_skill(store, &reference)?;
            let mut current = store
                .get_tags_map()?
                .get(&skill.id)
                .cloned()
                .unwrap_or_default();
            current.retain(|c| !tags.iter().any(|t| t == c));
            store.set_tags_for_skill(&skill.id, &current)?;
            sync_metadata::ensure_skill_metadata(store, &skill.id)?;
            print_json(
                &TagReport {
                    skill_id: skill.id,
                    name: skill.name,
                    tags: current,
                },
                json,
            );
        }
        TagCommand::List { reference } => {
            if let Some(r) = reference {
                let skill = resolve_skill(store, &r)?;
                let tags = store
                    .get_tags_map()?
                    .get(&skill.id)
                    .cloned()
                    .unwrap_or_default();
                print_json(
                    &TagReport {
                        skill_id: skill.id,
                        name: skill.name,
                        tags,
                    },
                    json,
                );
            } else {
                print_json(&store.get_all_tags()?, json);
            }
        }
    }
    Ok(())
}

// ── presets ───────────────────────────────────────────────────────────────

fn run_presets(args: PresetArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    match args.command {
        PresetCommand::List => print_json(&list_presets(store)?, json),
        PresetCommand::Current => print_json(&current_preset(store)?, json),
        PresetCommand::Preview { reference } => {
            let preset = resolve_scenario(store, &reference)?;
            let preview =
                scenario_service::preview_scenario_sync(store, &preset.id).map_err(map_app_err)?;
            print_json(&preview, json);
        }
        PresetCommand::Apply { reference } => {
            let preset = resolve_scenario(store, &reference)?;
            scenario_service::apply_scenario_to_default(store, &preset.id).map_err(map_app_err)?;
            print_json(&current_preset(store)?, json);
        }
        PresetCommand::Deactivate { reference } => {
            let preset = resolve_scenario(store, &reference)?;
            let active = store.get_active_scenario_id()?;
            let is_active = active.as_deref() == Some(preset.id.as_str());
            let count_before = count_synced_targets_for_preset(store, &preset.id)?;

            if is_active {
                let next_active = replacement_preset_after_deactivate(store, &preset.id)?;
                if let Some(next) = next_active.as_ref() {
                    scenario_service::apply_scenario_to_default(store, &next.id)
                        .map_err(map_app_err)?;
                } else {
                    scenario_service::unsync_scenario_skills(store, &preset.id)
                        .map_err(map_app_err)?;
                    store.clear_active_scenario()?;
                }
            } else {
                // Closing a non-active preset still tears down sync targets for
                // any skills it shares with the active preset. Unsync this
                // preset first, then re-sync the active preset so the shared
                // targets are restored.
                scenario_service::unsync_scenario_skills(store, &preset.id).map_err(map_app_err)?;
                if let Some(active_id) = active.as_deref() {
                    scenario_service::sync_scenario_skills(store, active_id)
                        .map_err(map_app_err)?;
                }
            }

            let count_after = count_synced_targets_for_preset(store, &preset.id)?;
            let removed_target_count = count_before.saturating_sub(count_after);

            let active_after = current_preset(store)?;
            print_json(
                &PresetDeactivateReport {
                    ok: true,
                    preset_id: preset.id,
                    preset_name: preset.name,
                    removed_target_count,
                    active_preset_id: active_after.as_ref().map(|preset| preset.id.clone()),
                    active_preset_name: active_after.map(|preset| preset.name),
                },
                json,
            );
        }
        PresetCommand::AddSkill { preset, skills } => {
            let s = resolve_scenario(store, &preset)?;
            let mut added = Vec::new();
            let mut missing = Vec::new();
            for r in skills {
                match resolve_skill(store, &r) {
                    Ok(skill) => {
                        store.add_skill_to_scenario(&s.id, &skill.id)?;
                        added.push(skill.name);
                    }
                    Err(_) => missing.push(r),
                }
            }
            sync_metadata::write_all_from_db(store)?;
            print_json(
                &PresetMembershipReport {
                    preset_id: s.id,
                    preset_name: s.name,
                    added,
                    removed: Vec::new(),
                    missing,
                },
                json,
            );
        }
        PresetCommand::RemoveSkill { preset, skills } => {
            let s = resolve_scenario(store, &preset)?;
            let mut removed = Vec::new();
            let mut missing = Vec::new();
            for r in skills {
                match resolve_skill(store, &r) {
                    Ok(skill) => {
                        store.remove_skill_from_scenario(&s.id, &skill.id)?;
                        removed.push(skill.name);
                    }
                    Err(_) => missing.push(r),
                }
            }
            sync_metadata::write_all_from_db(store)?;
            print_json(
                &PresetMembershipReport {
                    preset_id: s.id,
                    preset_name: s.name,
                    added: Vec::new(),
                    removed,
                    missing,
                },
                json,
            );
        }
    }
    Ok(())
}

fn list_presets(store: &SkillStore) -> anyhow::Result<Vec<PresetInfo>> {
    let active = store.get_active_scenario_id()?;
    let scenarios = store.get_all_scenarios()?;
    Ok(scenarios
        .into_iter()
        .map(|scenario| PresetInfo {
            skill_count: store
                .get_skill_ids_for_scenario(&scenario.id)
                .unwrap_or_default()
                .len(),
            active: active.as_deref() == Some(scenario.id.as_str()),
            id: scenario.id,
            name: scenario.name,
            description: scenario.description,
            icon: scenario.icon,
            sort_order: scenario.sort_order,
        })
        .collect())
}

fn current_preset(store: &SkillStore) -> anyhow::Result<Option<PresetInfo>> {
    let scenarios = list_presets(store)?;
    Ok(scenarios.into_iter().find(|s| s.active))
}

fn count_synced_targets_for_preset(store: &SkillStore, preset_id: &str) -> anyhow::Result<usize> {
    let skill_ids = store.get_skill_ids_for_scenario(preset_id)?;
    let mut count = 0;
    for skill_id in skill_ids {
        count += store.get_targets_for_skill(&skill_id)?.len();
    }
    Ok(count)
}

fn replacement_preset_after_deactivate(
    store: &SkillStore,
    deactivated_id: &str,
) -> anyhow::Result<Option<app_lib::core::skill_store::ScenarioRecord>> {
    let scenarios = store.get_all_scenarios()?;
    if let Some(default_id) = store.get_setting("default_scenario")? {
        if default_id != deactivated_id {
            if let Some(default) = scenarios.iter().find(|scenario| scenario.id == default_id) {
                return Ok(Some(default.clone()));
            }
        }
    }

    Ok(scenarios
        .into_iter()
        .find(|scenario| scenario.id != deactivated_id))
}

fn resolve_scenario(
    store: &SkillStore,
    reference: &str,
) -> anyhow::Result<app_lib::core::skill_store::ScenarioRecord> {
    let scenarios = store.get_all_scenarios()?;
    if reference == "current" {
        let active = store
            .get_active_scenario_id()?
            .ok_or_else(|| anyhow!("no active preset"))?;
        return scenarios
            .into_iter()
            .find(|scenario| scenario.id == active)
            .ok_or_else(|| anyhow!("active preset not found"));
    }
    let matches: Vec<_> = scenarios
        .into_iter()
        .filter(|s| s.id == reference || s.name == reference)
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => Err(anyhow!("preset not found: {reference}")),
        _ => Err(anyhow!("preset reference is ambiguous: {reference}")),
    }
}

// ── git ───────────────────────────────────────────────────────────────────

fn run_git(
    args: GitArgs,
    store: &SkillStore,
    has_skills_root: bool,
    json: bool,
) -> anyhow::Result<()> {
    match args.command {
        GitCommand::Status => {
            print_json(&git_backup::get_status(&central_repo::skills_dir())?, json)
        }
        GitCommand::Init => {
            // No settings store on this path; the hostname default matches
            // what the GUI derives, and the GUI reconciles the repo identity
            // on its next backup anyway.
            git_backup::init_repo(
                &central_repo::skills_dir(),
                &git_backup::default_device_name(),
            )?;
            print_json(&git_backup::get_status(&central_repo::skills_dir())?, json);
        }
        GitCommand::Clone { url } => {
            let target = central_repo::skills_dir();
            if has_skills_root {
                git_backup::clone_into_strict(&target, &url)?;
            } else {
                git_backup::clone_into(&target, &url)?;
            }
            print_json(&git_backup::get_status(&target)?, json);
        }
        GitCommand::SetRemote { url } => {
            git_backup::set_remote(&central_repo::skills_dir(), &url)?;
            print_json(&git_backup::get_status(&central_repo::skills_dir())?, json);
        }
        GitCommand::Pull => {
            // Same engine gate as the GUI sync (object merge by default,
            // merge_engine=system opts out). A raw line merge from this CLI
            // would read as an old-client violation on other devices (§6).
            let dir = central_repo::skills_dir();
            {
                let _lock = RepoLock::acquire_foreground("git pull")?;
                let device = store
                    .get_setting("backup_device_name")
                    .ok()
                    .flatten()
                    .map(|v| git_backup::sanitize_device_name(&v))
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(git_backup::default_device_name);
                let _ = git_backup::configure_device_identity(&dir, &device);
                merge::gated_pull_unlocked(store, &dir)?;
            }
            // Reconcile the DB from the merged metadata (takes its own lock).
            sync_metadata::reindex_from_metadata(store)?;
            print_json(&git_backup::get_status(&dir)?, json);
        }
        GitCommand::Push => {
            git_backup::push(&central_repo::skills_dir())?;
            print_json(&git_backup::get_status(&central_repo::skills_dir())?, json);
        }
        GitCommand::Commit { message } => {
            git_backup::commit_all(&central_repo::skills_dir(), &message)?;
            let tag = git_backup::create_snapshot_tag(&central_repo::skills_dir())?;
            print_json(&serde_json::json!({"ok": true, "tag": tag}), json);
        }
        GitCommand::Versions { limit } => print_json(
            &git_backup::list_snapshot_versions(&central_repo::skills_dir(), limit)?,
            json,
        ),
        GitCommand::Restore { tag } => {
            git_backup::restore_snapshot_version(&central_repo::skills_dir(), &tag)?;
            print_json(&git_backup::get_status(&central_repo::skills_dir())?, json);
        }
        GitCommand::PruneSyncRefs => {
            let removed = git_backup::prune_hidden_refs_on_remote(&central_repo::skills_dir())?;
            print_json(&serde_json::json!({ "removed": removed }), json);
        }
    }
    Ok(())
}

// ── chain ─────────────────────────────────────────────────────────────────

/// Dispatch a `chain` command straight through the Chain Service, so the CLI and
/// the GUI return identical results and produce equivalent state and audit
/// records for the same on-disk fixture. The read-only queries never mutate
/// anything.
///
/// The mutating commands are a THIN WRAPPER: each defaults to a read-only preview
/// (the plan DTO) and, only with `--apply`, delegates to the exact same
/// `ChainService` plan→apply pair the GUI calls — so the registered-project,
/// Original Repository, changed-evidence, and no-history-rewrite guards, and the
/// per-item audit records, are shared here, not reimplemented (AC2/AC4/AC5).
/// Because the dispatch adds no behavior of its own, the plan→apply equivalence
/// is proven once at the service level (`core::chain::service` tests) rather than
/// by spawning this binary. Apply outcomes route through [`finish_apply`] so a
/// partial failure prints its per-item detail and then exits non-zero (AC6).
fn run_chain(args: ChainArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    let service = chain::ChainService::new(store);
    match args.command {
        ChainCommand::Topology => print_json(&service.scan().map_err(anyhow::Error::msg)?, json),
        ChainCommand::Where { skill, project } => print_json(
            &service
                .resolve(&skill, project.as_deref())
                .map_err(anyhow::Error::msg)?,
            json,
        ),
        ChainCommand::Doctor {
            severities,
            deviations,
        } => {
            let filter = build_doctor_filter(&severities, &deviations)?;
            print_json(&service.doctor(&filter).map_err(anyhow::Error::msg)?, json);
        }
        ChainCommand::Decide {
            fingerprints,
            action,
            apply,
        } => {
            let plan = service
                .plan_decisions(&fingerprints, action.kind())
                .map_err(anyhow::Error::msg)?;
            if apply {
                let outcome = service.apply_decisions(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(&outcome, outcome.ok, "chain decide", json);
            }
            print_json(&plan, json);
            if !plan.ok {
                return Err(anyhow!(
                    "chain decide preview contains errors; see per-item outcomes"
                ));
            }
        }
        ChainCommand::Repositories => print_json(
            &service.repository_status().map_err(anyhow::Error::msg)?,
            json,
        ),
        ChainCommand::Duplicates => print_json(
            &service.duplicate_checkouts().map_err(anyhow::Error::msg)?,
            json,
        ),
        ChainCommand::Link {
            project,
            skills,
            agents,
            apply,
        } => {
            let project_path = PathBuf::from(&project);
            let originals: Vec<PathBuf> = skills.iter().map(PathBuf::from).collect();
            if apply {
                // Apply mirrors the GUI one-shot: enrol the folder (the explicit
                // enrolment approval), then plan and apply the same methods.
                service
                    .enrol_project(&project_path)
                    .map_err(anyhow::Error::msg)?;
                let plan = service
                    .plan_link(&project_path, &originals, &agents)
                    .map_err(anyhow::Error::msg)?;
                let outcome = service.apply_link(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(
                    &outcome,
                    apply_outcome_succeeded(&outcome),
                    "chain link",
                    json,
                );
            }
            // Preview: plan only — never enrol, never write (AC1).
            let plan = service
                .plan_link(&project_path, &originals, &agents)
                .map_err(anyhow::Error::msg)?;
            print_json(&plan, json);
        }
        ChainCommand::Unlink {
            project,
            skill,
            agents,
            apply,
        } => {
            let project_path = PathBuf::from(&project);
            let plan = service
                .plan_unlink(&project_path, &skill, &agents)
                .map_err(anyhow::Error::msg)?;
            if apply {
                let outcome = service.apply_unlink(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(
                    &outcome,
                    unlink_outcome_succeeded(&outcome),
                    "chain unlink",
                    json,
                );
            }
            print_json(&plan, json);
        }
        ChainCommand::Remediate {
            global_path,
            project,
            agents,
            apply,
        } => {
            let project_path = PathBuf::from(&project);
            let plan = service
                .plan_remediate(&global_path, &project_path, &agents)
                .map_err(anyhow::Error::msg)?;
            if apply {
                let outcome = service.apply_remediate(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(
                    &outcome,
                    remediate_outcome_succeeded(&outcome),
                    "chain remediate",
                    json,
                );
            }
            print_json(&plan, json);
        }
        ChainCommand::Normalize {
            fingerprints,
            apply,
        } => {
            let plan = service
                .plan_repair(&fingerprints)
                .map_err(anyhow::Error::msg)?;
            if apply {
                let outcome = service.apply_repair(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(
                    &outcome,
                    repair_outcome_succeeded(&outcome),
                    "chain normalize",
                    json,
                );
            }
            print_json(&plan, json);
        }
        ChainCommand::Pull { repos, apply } => {
            let plan = service.plan_pull(&repos).map_err(anyhow::Error::msg)?;
            if apply {
                let outcome = service.apply_pull(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(
                    &outcome,
                    pull_outcome_succeeded(&outcome),
                    "chain pull",
                    json,
                );
            }
            print_json(&plan, json);
        }
        ChainCommand::ForkSync { repos, apply } => {
            let plan = service.plan_fork_sync(&repos).map_err(anyhow::Error::msg)?;
            if apply {
                let outcome = service.apply_fork_sync(&plan).map_err(anyhow::Error::msg)?;
                return finish_apply(
                    &outcome,
                    fork_sync_outcome_succeeded(&outcome),
                    "chain fork-sync",
                    json,
                );
            }
            print_json(&plan, json);
        }
    }
    Ok(())
}

/// The `instructions` command group. Every command delegates to the same
/// `InstructionsService` the GUI will use — no second implementation. `scan` /
/// `where` / `doctor` are read-only; `normalize` previews by default and writes
/// only with `--apply`, through the §8 guard stack.
fn run_instructions(args: InstructionsArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    let service = instructions::InstructionsService::new(store);
    match args.command {
        InstructionsCommand::Scan { project } => {
            let path = project.as_ref().map(PathBuf::from);
            let report = service.scan(path.as_deref()).map_err(map_app_err)?;
            print_json(&report, json);
        }
        InstructionsCommand::Where { project, agent } => {
            let path = PathBuf::from(&project);
            let chains = service
                .where_chain(&path, agent.as_deref())
                .map_err(map_app_err)?;
            print_json(&chains, json);
        }
        InstructionsCommand::Doctor {
            severities,
            rules,
            project,
        } => {
            let filter = build_instructions_doctor_filter(&severities, &rules)?;
            let path = project.as_ref().map(PathBuf::from);
            let report = service
                .doctor(&filter, path.as_deref())
                .map_err(map_app_err)?;
            print_json(&report, json);
        }
        InstructionsCommand::Normalize {
            project,
            fingerprints,
            apply,
        } => {
            let path = PathBuf::from(&project);
            // Plan first (read-only) so apply consumes an evidence-carrying plan;
            // the guard re-verifies each target before writing.
            let plan = service
                .plan_normalize(&path, &fingerprints)
                .map_err(map_app_err)?;
            if apply {
                let outcome = service.apply_normalize(&path, &plan).map_err(map_app_err)?;
                return finish_apply(
                    &outcome,
                    normalize_outcome_succeeded(&outcome),
                    "instructions normalize",
                    json,
                );
            }
            print_json(&plan, json);
        }
        InstructionsCommand::Init {
            project,
            docs_dir,
            apply,
        } => {
            let path = PathBuf::from(&project);
            let plan = service.plan_init(&path, docs_dir).map_err(map_app_err)?;
            if apply {
                let outcome = service.apply_init(&path, &plan).map_err(map_app_err)?;
                return finish_apply(
                    &outcome,
                    init_outcome_succeeded(&outcome),
                    "instructions init",
                    json,
                );
            }
            print_json(&plan, json);
        }
    }
    Ok(())
}

/// Predicate for a normalize apply (parity with chain's apply predicates): fully
/// succeeded only when a rescan `verified` every fix AND no item was refused.
fn normalize_outcome_succeeded(outcome: &instructions::normalize::NormalizeOutcome) -> bool {
    outcome.verified && outcome.items.iter().all(|i| i.action != "conflict")
}

/// Predicate for an init apply: fully succeeded only when every intended target
/// exists (`verified`) AND no item was refused.
fn init_outcome_succeeded(outcome: &instructions::init::InitOutcome) -> bool {
    outcome.verified && outcome.items.iter().all(|i| i.action != "conflict")
}

/// Build an instructions `DoctorFilter` from repeated `--severity`/`--rule`
/// tokens. Severities are the stable serialized `Severity` names (shared with
/// chain); rules accept the full id or its short suffix. Any unknown token is a
/// hard error, never a silently dropped filter.
fn build_instructions_doctor_filter(
    severities: &[String],
    rules: &[String],
) -> anyhow::Result<instructions::doctor::DoctorFilter> {
    let severities = severities
        .iter()
        .map(|s| parse_enum_token::<chain::doctor::Severity>(s, "severity"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let rules = rules
        .iter()
        .map(|r| {
            instructions::doctor::Rule::from_token(r).ok_or_else(|| anyhow!("unknown rule: {r}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(instructions::doctor::DoctorFilter { severities, rules })
}

/// Item action tokens that mean a single link / unlink / remediate / normalize
/// target was REFUSED rather than settled cleanly. Any one of these — even when
/// a rescan otherwise `verified` the shape — means the operation did not fully
/// succeed (AC6). Both `skip` (repair's vocabulary) and `skipped`
/// (link/unlink's vocabulary) count as refusals.
fn is_refused_item_action(action: &str) -> bool {
    matches!(action, "conflict" | "error" | "skipped" | "skip")
}

/// Predicate for a link apply (AC6): fully succeeded only when a rescan
/// `verified` the requested chain AND every written item settled cleanly.
/// `verified` already implies a clean report inside the service; the item check
/// is kept explicit so the CLI's exit-code gate is legible and cannot drift.
fn apply_outcome_succeeded(outcome: &chain::service::ApplyOutcome) -> bool {
    outcome.verified
        && outcome
            .report
            .skills
            .iter()
            .chain(outcome.report.entries.iter())
            .all(|item| !is_refused_item_action(&item.action))
}

/// Predicate for an unlink apply (AC6): fully succeeded only when the rescan
/// `verified` the intended removal AND no item was refused.
fn unlink_outcome_succeeded(outcome: &chain::service::UnlinkOutcome) -> bool {
    outcome.verified
        && outcome
            .report
            .iter()
            .all(|item| !is_refused_item_action(&item.action))
}

/// Predicate for a normalize (repair) apply (AC6): fully succeeded only when the
/// rescan `verified` the normalized chain AND no item was refused.
fn repair_outcome_succeeded(outcome: &chain::repair::RepairOutcome) -> bool {
    outcome.verified
        && outcome
            .results
            .iter()
            .all(|item| !is_refused_item_action(&item.action))
}

/// Predicate for a remediation apply (AC6): fully succeeded only when the
/// end-to-end `verified` flag is set (the project link verified AND the global
/// entry was retired) AND the nested link apply, if any, had no refused item. A
/// physical global entry (`link == None`) is manual-only and never sets
/// `verified`, so it is correctly reported as not fully succeeded.
fn remediate_outcome_succeeded(outcome: &chain::remediate::RemediationOutcome) -> bool {
    outcome.verified && outcome.link.as_ref().is_none_or(apply_outcome_succeeded)
}

/// Predicate for a pull apply (AC6): fully succeeded when no attempted repository
/// ended in `error`. A `skipped` result (dirty, diverged, or already up to date)
/// is the protective guard working as intended — a refusal, NOT a failure — so
/// only a real `error` fails the operation.
fn pull_outcome_succeeded(outcome: &chain::pull::PullOutcome) -> bool {
    outcome
        .results
        .iter()
        .all(|result| result.action != "error")
}

/// Predicate for a fork-sync apply (AC6): the same rule as
/// [`pull_outcome_succeeded`] — only an `error` result is a failure; a `skipped`
/// is a protected refusal (dirty, diverged, up to date, or not fast-forwardable).
fn fork_sync_outcome_succeeded(outcome: &chain::fork_sync::ForkSyncOutcome) -> bool {
    outcome
        .results
        .iter()
        .all(|result| result.action != "error")
}

/// Emit an APPLY outcome, then enforce the partial-failure contract (AC6). The
/// per-item JSON is printed FIRST so the created/existing/removed/skipped/
/// conflict/error detail is always visible; only then, if `succeeded` is false,
/// is a non-zero exit produced by returning an `Err` that `main` renders as an
/// `ok:false` envelope on stderr. Preview never routes through here — it is
/// read-only and cannot "fail" on an outcome.
fn run_fleet(args: FleetArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    let service = fleet::FleetService::new(store);
    match args.command {
        FleetCommand::Config { meta_url } => {
            let config = match meta_url {
                Some(url) => service.set_meta_url(&url).map_err(map_app_err)?,
                None => service.config().map_err(map_app_err)?,
            };
            print_json(&config, json);
        }
        FleetCommand::Status => print_json(&service.status().map_err(map_app_err)?, json),
        FleetCommand::Discover => print_json(&service.discover().map_err(map_app_err)?, json),
        FleetCommand::Push { repos, apply } => {
            let plan = service.plan_push(&repos).map_err(map_app_err)?;
            if apply {
                let outcome = service.apply_push(&plan).map_err(map_app_err)?;
                return finish_apply(&outcome, outcome.ok, "fleet push", json);
            }
            print_json(&plan, json);
        }
        FleetCommand::Pull { repos, apply } => {
            let plan = service.plan_pull(&repos).map_err(map_app_err)?;
            if apply {
                let outcome = service.apply_pull(&plan).map_err(map_app_err)?;
                return finish_apply(&outcome, outcome.ok, "fleet pull", json);
            }
            print_json(&plan, json);
        }
        FleetCommand::Init { repos, apply } => {
            let plan = service.plan_init(&repos).map_err(map_app_err)?;
            if apply {
                let outcome = service.apply_init(&plan).map_err(map_app_err)?;
                return finish_apply(&outcome, outcome.ok, "fleet init", json);
            }
            print_json(&plan, json);
        }
        FleetCommand::Bootstrap { repos, apply } => {
            let plan = service.plan_bootstrap(&repos).map_err(map_app_err)?;
            if apply {
                let outcome = service.apply_bootstrap(&plan).map_err(map_app_err)?;
                return finish_apply(&outcome, outcome.ok, "fleet bootstrap", json);
            }
            print_json(&plan, json);
        }
        FleetCommand::Report { apply } => {
            if apply {
                let outcome = service.apply_report().map_err(map_app_err)?;
                return finish_apply(&outcome, outcome.ok, "fleet report", json);
            }
            // Preview: the exact report that --apply would push, nothing written.
            print_json(&service.plan_report().map_err(map_app_err)?, json);
        }
    }
    Ok(())
}

fn finish_apply<T: Serialize>(
    outcome: &T,
    succeeded: bool,
    op: &str,
    json: bool,
) -> anyhow::Result<()> {
    print_json(outcome, json);
    if succeeded {
        Ok(())
    } else {
        Err(anyhow!("{op} did not fully succeed; see per-item outcomes"))
    }
}

/// Build a `DoctorFilter` from repeated `--severity`/`--deviation` tokens. Tokens
/// are the stable serialized enum names (never localized prose), so an unknown
/// token is a hard error rather than a silently dropped filter.
fn build_doctor_filter(
    severities: &[String],
    deviations: &[String],
) -> anyhow::Result<chain::doctor::DoctorFilter> {
    let severities = severities
        .iter()
        .map(|s| parse_enum_token::<chain::doctor::Severity>(s, "severity"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let deviations = deviations
        .iter()
        .map(|s| parse_enum_token::<chain::doctor::Deviation>(s, "deviation"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(chain::doctor::DoctorFilter {
        severities,
        deviations,
    })
}

/// Parse one stable, snake_case enum token (e.g. "violation", "project_private")
/// into the corresponding Doctor filter variant via serde, matching the exact
/// wire vocabulary the JSON output uses.
fn parse_enum_token<T: serde::de::DeserializeOwned>(token: &str, axis: &str) -> anyhow::Result<T> {
    serde_json::from_value(serde_json::Value::String(token.to_string()))
        .map_err(|_| anyhow!("unknown {axis}: {token}"))
}

// ── helpers ───────────────────────────────────────────────────────────────

fn map_app_err(e: AppError) -> anyhow::Error {
    anyhow!(e.message)
}

fn print_json<T: Serialize>(value: &T, json: bool) {
    let rendered = if json {
        serde_json::to_string(value).unwrap()
    } else {
        serde_json::to_string_pretty(value).unwrap()
    };
    println!("{rendered}");
}

#[cfg(test)]
mod chain_cli_tests {
    //! Contract tests for the read-only `chain` CLI group: argument wiring and
    //! the Doctor-filter token vocabulary, covering success, empty, and error
    //! cases. The command handlers themselves are thin pass-throughs to the
    //! Chain Service, whose projection logic is covered in `core::chain::resolve`
    //! and `core::chain::doctor`, so CLI and Service results match by construction.
    use super::*;
    use app_lib::core::chain::doctor::{Deviation, Severity};
    use clap::Parser;

    #[test]
    fn chain_subcommands_parse() {
        assert!(matches!(
            Cli::try_parse_from(["cli", "chain", "topology"])
                .unwrap()
                .command,
            Commands::Chain(ChainArgs {
                command: ChainCommand::Topology
            })
        ));

        let cli =
            Cli::try_parse_from(["cli", "chain", "where", "alpha", "--project", "/p"]).unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command: ChainCommand::Where { skill, project },
            }) => {
                assert_eq!(skill, "alpha");
                assert_eq!(project.as_deref(), Some("/p"));
            }
            other => panic!("expected chain where, got {other:?}"),
        }

        // The `repository-status` alias resolves to the Repositories command.
        assert!(matches!(
            Cli::try_parse_from(["cli", "chain", "repository-status"])
                .unwrap()
                .command,
            Commands::Chain(ChainArgs {
                command: ChainCommand::Repositories
            })
        ));

        // Both the canonical `duplicates` and its `duplicate-checkouts` alias
        // resolve to the Duplicates command.
        for arg in ["duplicates", "duplicate-checkouts"] {
            assert!(matches!(
                Cli::try_parse_from(["cli", "chain", arg]).unwrap().command,
                Commands::Chain(ChainArgs {
                    command: ChainCommand::Duplicates
                })
            ));
        }
    }

    #[test]
    fn doctor_filter_parses_valid_tokens() {
        let f = build_doctor_filter(&["violation".into(), "notice".into()], &["broken".into()])
            .unwrap();
        assert_eq!(f.severities, vec![Severity::Violation, Severity::Notice]);
        assert_eq!(f.deviations, vec![Deviation::Broken]);
    }

    #[test]
    fn empty_doctor_filter_constrains_nothing() {
        let f = build_doctor_filter(&[], &[]).unwrap();
        assert!(f.severities.is_empty() && f.deviations.is_empty());
    }

    #[test]
    fn unknown_doctor_token_is_an_error() {
        let err = build_doctor_filter(&["bogus".into()], &[]).unwrap_err();
        assert!(err.to_string().contains("unknown severity: bogus"));
        let err = build_doctor_filter(&[], &["nope".into()]).unwrap_err();
        assert!(err.to_string().contains("unknown deviation: nope"));
    }

    /// Every mutating subcommand parses; `--apply` defaults to false (preview,
    /// AC1) and flips true; repeated `--skill`/`--agent`/`--repo`/`--fingerprint`
    /// collect into vectors. Like the read-only cases, these assert only argument
    /// wiring — the handlers are thin pass-throughs to the Chain Service.
    #[test]
    fn chain_mutation_subcommands_parse() {
        // Link: preview by default; repeated --skill/--agent collect in order.
        let cli = Cli::try_parse_from([
            "cli",
            "chain",
            "link",
            "--project",
            "/p",
            "--skill",
            "/w/a",
            "--skill",
            "/w/b",
            "--agent",
            "claude",
            "--agent",
            "codex",
        ])
        .unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command:
                    ChainCommand::Link {
                        project,
                        skills,
                        agents,
                        apply,
                    },
            }) => {
                assert_eq!(project, "/p");
                assert_eq!(skills, ["/w/a", "/w/b"]);
                assert_eq!(agents, ["claude", "codex"]);
                assert!(!apply, "mutating commands default to preview (AC1)");
            }
            other => panic!("expected chain link, got {other:?}"),
        }

        // --apply flips the preview default to a write.
        assert!(matches!(
            Cli::try_parse_from([
                "cli",
                "chain",
                "link",
                "--project",
                "/p",
                "--skill",
                "/w/a",
                "--apply"
            ])
            .unwrap()
            .command,
            Commands::Chain(ChainArgs {
                command: ChainCommand::Link { apply: true, .. }
            })
        ));

        // Unlink: a single --skill plus repeated --agent.
        let cli = Cli::try_parse_from([
            "cli",
            "chain",
            "unlink",
            "--project",
            "/p",
            "--skill",
            "demo",
            "--agent",
            "claude",
        ])
        .unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command:
                    ChainCommand::Unlink {
                        project,
                        skill,
                        agents,
                        apply,
                    },
            }) => {
                assert_eq!(project, "/p");
                assert_eq!(skill, "demo");
                assert_eq!(agents, ["claude"]);
                assert!(!apply);
            }
            other => panic!("expected chain unlink, got {other:?}"),
        }

        // Remediate: --global-path is the offending Guard entry; --apply set here.
        let cli = Cli::try_parse_from([
            "cli",
            "chain",
            "remediate",
            "--global-path",
            "/g/demo",
            "--project",
            "/p",
            "--agent",
            "claude",
            "--apply",
        ])
        .unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command:
                    ChainCommand::Remediate {
                        global_path,
                        project,
                        agents,
                        apply,
                    },
            }) => {
                assert_eq!(global_path, "/g/demo");
                assert_eq!(project, "/p");
                assert_eq!(agents, ["claude"]);
                assert!(apply);
            }
            other => panic!("expected chain remediate, got {other:?}"),
        }

        // Normalize: repeated --fingerprint collect.
        let cli = Cli::try_parse_from([
            "cli",
            "chain",
            "normalize",
            "--fingerprint",
            "fp1",
            "--fingerprint",
            "fp2",
        ])
        .unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command:
                    ChainCommand::Normalize {
                        fingerprints,
                        apply,
                    },
            }) => {
                assert_eq!(fingerprints, ["fp1", "fp2"]);
                assert!(!apply);
            }
            other => panic!("expected chain normalize, got {other:?}"),
        }

        // Pull: repeated --repo collect; --apply set.
        let cli = Cli::try_parse_from([
            "cli", "chain", "pull", "--repo", "/r1", "--repo", "/r2", "--apply",
        ])
        .unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command: ChainCommand::Pull { repos, apply },
            }) => {
                assert_eq!(repos, ["/r1", "/r2"]);
                assert!(apply);
            }
            other => panic!("expected chain pull, got {other:?}"),
        }

        // ForkSync parses under the kebab-case name `fork-sync`.
        let cli = Cli::try_parse_from(["cli", "chain", "fork-sync", "--repo", "/r1"]).unwrap();
        match cli.command {
            Commands::Chain(ChainArgs {
                command: ChainCommand::ForkSync { repos, apply },
            }) => {
                assert_eq!(repos, ["/r1"]);
                assert!(!apply);
            }
            other => panic!("expected chain fork-sync, got {other:?}"),
        }
    }

    #[test]
    fn chain_decide_parses_preview_and_apply_contract() {
        let preview = Cli::try_parse_from([
            "cli",
            "chain",
            "decide",
            "--fingerprint",
            "fp1",
            "--fingerprint",
            "fp2",
            "--action",
            "mark-private",
        ])
        .unwrap();
        match preview.command {
            Commands::Chain(ChainArgs {
                command:
                    ChainCommand::Decide {
                        fingerprints,
                        action,
                        apply,
                    },
            }) => {
                assert_eq!(fingerprints, ["fp1", "fp2"]);
                assert_eq!(action, DecisionAction::MarkPrivate);
                assert!(!apply, "decide defaults to a read-only preview");
            }
            other => panic!("expected chain decide, got {other:?}"),
        }

        assert!(matches!(
            Cli::try_parse_from([
                "cli",
                "chain",
                "decide",
                "--fingerprint",
                "fp1",
                "--action",
                "ignore",
                "--apply",
            ])
            .unwrap()
            .command,
            Commands::Chain(ChainArgs {
                command: ChainCommand::Decide {
                    action: DecisionAction::Ignore,
                    apply: true,
                    ..
                }
            })
        ));

        assert!(
            Cli::try_parse_from(["cli", "chain", "decide", "--action", "ignore"]).is_err(),
            "at least one fingerprint is required"
        );
        assert!(
            Cli::try_parse_from([
                "cli",
                "chain",
                "decide",
                "--fingerprint",
                "fp1",
                "--action",
                "delete",
            ])
            .is_err(),
            "actions outside the two-ticket vocabulary are rejected by clap"
        );
    }

    fn op_result(action: &str) -> chain::ops::OpResult {
        chain::ops::OpResult {
            name: "demo".to_string(),
            path: "/p/.agents/skills/demo".to_string(),
            action: action.to_string(),
            message: None,
        }
    }

    /// Build a link `ApplyOutcome` with a chosen `verified` flag and skill-item
    /// action; the agent entry is always a clean `created`.
    fn apply_outcome(verified: bool, skill_action: &str) -> chain::service::ApplyOutcome {
        chain::service::ApplyOutcome {
            report: chain::ops::LinkReport {
                agg_dir: "/p/.agents/skills".to_string(),
                skills: vec![op_result(skill_action)],
                entries: vec![op_result("created")],
            },
            verified,
            observed: vec!["demo".to_string()],
            missing: Vec::new(),
        }
    }

    fn pull_outcome(action: &str) -> chain::pull::PullOutcome {
        chain::pull::PullOutcome {
            results: vec![chain::pull::PullResult {
                path: "/r".to_string(),
                name: "r".to_string(),
                action: action.to_string(),
                from: None,
                to: None,
                reason: None,
                message: None,
            }],
            scanned_at: 1,
        }
    }

    /// AC6 predicate for verified-style applies: a verified, all-clean outcome
    /// succeeds; a single conflict/skipped/error item OR `verified == false`
    /// fails.
    #[test]
    fn apply_success_predicate_requires_verified_and_clean_items() {
        assert!(apply_outcome_succeeded(&apply_outcome(true, "created")));
        assert!(apply_outcome_succeeded(&apply_outcome(true, "exists")));
        for bad in ["conflict", "skipped", "error"] {
            assert!(
                !apply_outcome_succeeded(&apply_outcome(true, bad)),
                "a {bad} item must fail the predicate even when verified"
            );
        }
        assert!(
            !apply_outcome_succeeded(&apply_outcome(false, "created")),
            "verified == false is never a success"
        );
    }

    /// AC6 predicate for pull/fork-sync: a `skipped` refusal is the guard working
    /// (success), while an `error` is a real failure.
    #[test]
    fn pull_success_predicate_treats_skip_as_ok_but_error_as_failure() {
        assert!(pull_outcome_succeeded(&pull_outcome("skipped")));
        assert!(pull_outcome_succeeded(&pull_outcome("up_to_date")));
        assert!(pull_outcome_succeeded(&pull_outcome("updated")));
        assert!(
            !pull_outcome_succeeded(&pull_outcome("error")),
            "an error result is a partial failure"
        );
    }
}

#[cfg(test)]
mod install_ref_tests {
    //! `classify_ref` must recognise a native path on the platform it is run
    //! from. The POSIX-only form sent every Windows path to the `bail!` arm.
    use super::*;

    fn kind(reference: &str) -> InstallKind {
        classify_ref(reference, false, false, false)
            .unwrap_or_else(|e| panic!("{reference:?} should classify, got: {e}"))
    }

    #[test]
    fn native_windows_paths_are_local() {
        for reference in [
            ".\\my-skill",
            "..\\my-skill",
            "D:\\Projects\\skills\\thing",
            "C:/Projects/skills/thing",
            "~\\skills\\thing",
            "\\\\server\\share\\skill",
        ] {
            assert!(
                matches!(kind(reference), InstallKind::Local),
                "{reference:?} should be Local"
            );
        }
    }

    #[test]
    fn posix_paths_stay_local_and_remotes_stay_remote() {
        for reference in ["./my-skill", "../my-skill", "/opt/skills/x", "~/skills/x"] {
            assert!(
                matches!(kind(reference), InstallKind::Local),
                "{reference:?} should be Local"
            );
        }
        for reference in [
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://host/srv/r",
        ] {
            assert!(
                matches!(kind(reference), InstallKind::Git),
                "{reference:?} should be Git"
            );
        }
        // The drive-letter rule requires a separator, so a one-character scp
        // host is still a git remote rather than a mis-read drive.
        assert!(matches!(kind("m:git-mirrors/repo.git"), InstallKind::Git));
        assert!(matches!(kind("owner/repo"), InstallKind::Skillssh));
    }
}

#[cfg(test)]
mod fleet_cli_tests {
    //! Argument wiring for the read-only `fleet` group (P0). The handlers are
    //! thin pass-throughs to `FleetService`, covered in `core::fleet`.
    use super::*;
    use clap::Parser;

    #[test]
    fn status_and_discover_parse() {
        match Cli::try_parse_from(["cli", "fleet", "status"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Status,
            }) => {}
            other => panic!("expected fleet status, got {other:?}"),
        }
        match Cli::try_parse_from(["cli", "fleet", "discover"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Discover,
            }) => {}
            other => panic!("expected fleet discover, got {other:?}"),
        }
    }

    #[test]
    fn config_shows_by_default_and_takes_a_meta_url() {
        match Cli::try_parse_from(["cli", "fleet", "config"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Config { meta_url },
            }) => assert!(meta_url.is_none(), "bare config must only read"),
            other => panic!("expected fleet config, got {other:?}"),
        }
        match Cli::try_parse_from([
            "cli",
            "fleet",
            "config",
            "--meta-url",
            "alpha:git-mirrors/projects/_patchbay-fleet.git",
        ])
        .unwrap()
        .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Config { meta_url },
            }) => assert_eq!(
                meta_url.as_deref(),
                Some("alpha:git-mirrors/projects/_patchbay-fleet.git")
            ),
            other => panic!("expected fleet config --meta-url, got {other:?}"),
        }
    }

    #[test]
    fn report_defaults_to_preview_and_takes_apply() {
        match Cli::try_parse_from(["cli", "fleet", "report"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Report { apply },
            }) => assert!(!apply, "report must default to preview"),
            other => panic!("expected fleet report, got {other:?}"),
        }
        match Cli::try_parse_from(["cli", "fleet", "report", "--apply"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Report { apply },
            }) => assert!(apply),
            other => panic!("expected fleet report --apply, got {other:?}"),
        }
    }

    #[test]
    fn push_defaults_to_preview_and_accepts_repeated_repo_selectors() {
        match Cli::try_parse_from(["cli", "fleet", "push", "--repo", "alpha", "--repo", "beta"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Push { repos, apply },
            }) => {
                assert_eq!(repos, vec!["alpha", "beta"]);
                assert!(!apply, "fleet push must default to preview");
            }
            other => panic!("expected fleet push preview, got {other:?}"),
        }

        match Cli::try_parse_from(["cli", "fleet", "push", "--apply"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Push { repos, apply },
            }) => {
                assert!(repos.is_empty(), "no selectors means all manifest repos");
                assert!(apply);
            }
            other => panic!("expected fleet push --apply, got {other:?}"),
        }
    }

    #[test]
    fn pull_defaults_to_preview_and_accepts_repeated_repo_selectors() {
        match Cli::try_parse_from(["cli", "fleet", "pull", "--repo", "alpha", "--repo", "beta"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Pull { repos, apply },
            }) => {
                assert_eq!(repos, vec!["alpha", "beta"]);
                assert!(!apply, "fleet pull must default to preview");
            }
            other => panic!("expected fleet pull preview, got {other:?}"),
        }

        match Cli::try_parse_from(["cli", "fleet", "pull", "--apply"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Pull { repos, apply },
            }) => {
                assert!(repos.is_empty(), "no selectors means all manifest repos");
                assert!(apply);
            }
            other => panic!("expected fleet pull --apply, got {other:?}"),
        }
    }

    #[test]
    fn init_defaults_to_preview_and_accepts_repeated_repo_selectors() {
        match Cli::try_parse_from(["cli", "fleet", "init", "--repo", "alpha", "--repo", "beta"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Init { repos, apply },
            }) => {
                assert_eq!(repos, vec!["alpha", "beta"]);
                assert!(!apply, "fleet init must default to preview");
            }
            other => panic!("expected fleet init preview, got {other:?}"),
        }

        match Cli::try_parse_from(["cli", "fleet", "init", "--apply"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Init { repos, apply },
            }) => {
                assert!(repos.is_empty(), "no selectors means all manifest repos");
                assert!(apply);
            }
            other => panic!("expected fleet init --apply, got {other:?}"),
        }
    }

    #[test]
    fn bootstrap_defaults_to_preview_and_accepts_repeated_repo_selectors() {
        match Cli::try_parse_from([
            "cli",
            "fleet",
            "bootstrap",
            "--repo",
            "alpha",
            "--repo",
            "beta",
        ])
        .unwrap()
        .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Bootstrap { repos, apply },
            }) => {
                assert_eq!(repos, vec!["alpha", "beta"]);
                assert!(!apply, "fleet bootstrap must default to preview");
            }
            other => panic!("expected fleet bootstrap preview, got {other:?}"),
        }

        match Cli::try_parse_from(["cli", "fleet", "bootstrap", "--apply"])
            .unwrap()
            .command
        {
            Commands::Fleet(FleetArgs {
                command: FleetCommand::Bootstrap { repos, apply },
            }) => {
                assert!(repos.is_empty(), "no selectors means all manifest repos");
                assert!(apply);
            }
            other => panic!("expected fleet bootstrap --apply, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod instructions_cli_tests {
    //! Contract tests for the read-only `instructions` CLI group: argument wiring
    //! for `scan` and `where`, covering the optional/required flags and the empty
    //! and error cases. The handlers are thin pass-throughs to
    //! `InstructionsService`, whose scan/where projection is covered in
    //! `core::instructions::scanner`, so CLI and service results match by
    //! construction.
    use super::*;
    use clap::Parser;

    #[test]
    fn scan_parses_with_and_without_project() {
        // No --project: scans every registered project.
        match Cli::try_parse_from(["cli", "instructions", "scan"])
            .unwrap()
            .command
        {
            Commands::Instructions(InstructionsArgs {
                command: InstructionsCommand::Scan { project },
            }) => assert_eq!(project, None),
            other => panic!("expected instructions scan, got {other:?}"),
        }

        // --project narrows to one path.
        match Cli::try_parse_from(["cli", "instructions", "scan", "--project", "/p"])
            .unwrap()
            .command
        {
            Commands::Instructions(InstructionsArgs {
                command: InstructionsCommand::Scan { project },
            }) => assert_eq!(project.as_deref(), Some("/p")),
            other => panic!("expected instructions scan, got {other:?}"),
        }
    }

    #[test]
    fn where_parses_project_and_optional_agent() {
        let cli = Cli::try_parse_from([
            "cli",
            "instructions",
            "where",
            "--project",
            "/p",
            "--agent",
            "claude",
        ])
        .unwrap();
        match cli.command {
            Commands::Instructions(InstructionsArgs {
                command: InstructionsCommand::Where { project, agent },
            }) => {
                assert_eq!(project, "/p");
                assert_eq!(agent.as_deref(), Some("claude"));
            }
            other => panic!("expected instructions where, got {other:?}"),
        }

        // --agent is optional.
        match Cli::try_parse_from(["cli", "instructions", "where", "--project", "/p"])
            .unwrap()
            .command
        {
            Commands::Instructions(InstructionsArgs {
                command: InstructionsCommand::Where { project, agent },
            }) => {
                assert_eq!(project, "/p");
                assert_eq!(agent, None);
            }
            other => panic!("expected instructions where, got {other:?}"),
        }
    }

    #[test]
    fn where_requires_project() {
        // --project is mandatory; omitting it is a usage error (exit 2 at runtime).
        assert!(Cli::try_parse_from(["cli", "instructions", "where"]).is_err());
    }

    #[test]
    fn doctor_parses_filters_and_project() {
        // No filters: every registered project, no severity/rule constraint.
        match Cli::try_parse_from(["cli", "instructions", "doctor"])
            .unwrap()
            .command
        {
            Commands::Instructions(InstructionsArgs {
                command:
                    InstructionsCommand::Doctor {
                        severities,
                        rules,
                        project,
                    },
            }) => {
                assert!(severities.is_empty() && rules.is_empty() && project.is_none());
            }
            other => panic!("expected instructions doctor, got {other:?}"),
        }

        // Repeatable --severity / --rule and an optional --project.
        let cli = Cli::try_parse_from([
            "cli",
            "instructions",
            "doctor",
            "--severity",
            "warning",
            "--severity",
            "violation",
            "--rule",
            "dual_body",
            "--rule",
            "instructions.broken_import",
            "--project",
            "/p",
        ])
        .unwrap();
        match cli.command {
            Commands::Instructions(InstructionsArgs {
                command:
                    InstructionsCommand::Doctor {
                        severities,
                        rules,
                        project,
                    },
            }) => {
                assert_eq!(severities, vec!["warning", "violation"]);
                assert_eq!(rules, vec!["dual_body", "instructions.broken_import"]);
                assert_eq!(project.as_deref(), Some("/p"));
            }
            other => panic!("expected instructions doctor, got {other:?}"),
        }
    }

    #[test]
    fn instructions_doctor_filter_accepts_short_and_full_rule_ids() {
        let filter = build_instructions_doctor_filter(
            &["notice".to_string()],
            &[
                "dual_body".to_string(),
                "instructions.global_cost".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(filter.severities, vec![chain::doctor::Severity::Notice]);
        assert_eq!(
            filter.rules,
            vec![
                instructions::doctor::Rule::DualBody,
                instructions::doctor::Rule::GlobalCost
            ]
        );
    }

    #[test]
    fn instructions_doctor_filter_rejects_unknown_tokens() {
        assert!(build_instructions_doctor_filter(&["bogus".to_string()], &[]).is_err());
        assert!(build_instructions_doctor_filter(&[], &["not_a_rule".to_string()]).is_err());
    }

    #[test]
    fn normalize_parses_project_fingerprints_and_apply() {
        // Preview: required --project, no fingerprints, apply off.
        match Cli::try_parse_from(["cli", "instructions", "normalize", "--project", "/p"])
            .unwrap()
            .command
        {
            Commands::Instructions(InstructionsArgs {
                command:
                    InstructionsCommand::Normalize {
                        project,
                        fingerprints,
                        apply,
                    },
            }) => {
                assert_eq!(project, "/p");
                assert!(fingerprints.is_empty());
                assert!(!apply);
            }
            other => panic!("expected instructions normalize, got {other:?}"),
        }

        // Repeatable --fingerprint and --apply.
        let cli = Cli::try_parse_from([
            "cli",
            "instructions",
            "normalize",
            "--project",
            "/p",
            "--fingerprint",
            "fp1",
            "--fingerprint",
            "fp2",
            "--apply",
        ])
        .unwrap();
        match cli.command {
            Commands::Instructions(InstructionsArgs {
                command:
                    InstructionsCommand::Normalize {
                        project,
                        fingerprints,
                        apply,
                    },
            }) => {
                assert_eq!(project, "/p");
                assert_eq!(fingerprints, ["fp1", "fp2"]);
                assert!(apply);
            }
            other => panic!("expected instructions normalize, got {other:?}"),
        }

        // --project is mandatory.
        assert!(Cli::try_parse_from(["cli", "instructions", "normalize"]).is_err());
    }

    #[test]
    fn normalize_success_predicate_requires_verified_and_no_conflict() {
        use instructions::normalize::{NormalizeItem, NormalizeOutcome};
        use instructions::write_guard::WriteEvidence;
        let item = |action: &str| NormalizeItem {
            fingerprint: "fp".into(),
            rule: "instructions.dual_body".into(),
            project: "/p".into(),
            path: "/p/CLAUDE.md".into(),
            action: action.into(),
            before: WriteEvidence::Absent,
            after_content: None,
            snapshot: false,
            depends_on: None,
            message: None,
        };
        let outcome = |verified: bool, action: &str| NormalizeOutcome {
            items: vec![item(action)],
            snapshot_id: None,
            verified,
            scanned_at: 0,
        };
        assert!(normalize_outcome_succeeded(&outcome(true, "rewrite")));
        // A conflict item fails the gate even if verified were somehow true.
        assert!(!normalize_outcome_succeeded(&outcome(true, "conflict")));
        // Unverified fails regardless.
        assert!(!normalize_outcome_succeeded(&outcome(false, "rewrite")));
    }

    #[test]
    fn init_parses_project_docs_dir_and_apply() {
        // Preview: required --project, docs-dir off, apply off.
        match Cli::try_parse_from(["cli", "instructions", "init", "--project", "/p"])
            .unwrap()
            .command
        {
            Commands::Instructions(InstructionsArgs {
                command:
                    InstructionsCommand::Init {
                        project,
                        docs_dir,
                        apply,
                    },
            }) => {
                assert_eq!(project, "/p");
                assert!(!docs_dir);
                assert!(!apply);
            }
            other => panic!("expected instructions init, got {other:?}"),
        }

        // --docs-dir and --apply flags.
        let cli = Cli::try_parse_from([
            "cli",
            "instructions",
            "init",
            "--project",
            "/p",
            "--docs-dir",
            "--apply",
        ])
        .unwrap();
        match cli.command {
            Commands::Instructions(InstructionsArgs {
                command:
                    InstructionsCommand::Init {
                        project,
                        docs_dir,
                        apply,
                    },
            }) => {
                assert_eq!(project, "/p");
                assert!(docs_dir);
                assert!(apply);
            }
            other => panic!("expected instructions init, got {other:?}"),
        }

        // --project is mandatory.
        assert!(Cli::try_parse_from(["cli", "instructions", "init"]).is_err());
    }

    #[test]
    fn init_success_predicate_requires_verified_and_no_conflict() {
        use instructions::init::{InitItem, InitOutcome};
        use instructions::write_guard::WriteEvidence;
        let item = |action: &str| InitItem {
            path: "/p/AGENTS.md".into(),
            kind: "canonical".into(),
            action: action.into(),
            before: WriteEvidence::Absent,
            after_content: None,
            message: None,
        };
        let outcome = |verified: bool, action: &str| InitOutcome {
            items: vec![item(action)],
            verified,
            scanned_at: 0,
        };
        assert!(init_outcome_succeeded(&outcome(true, "create")));
        assert!(!init_outcome_succeeded(&outcome(true, "conflict")));
        assert!(!init_outcome_succeeded(&outcome(false, "create")));
    }
}
