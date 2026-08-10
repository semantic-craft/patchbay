use std::path::PathBuf;

use anyhow::anyhow;
use app_lib::core::{
    app_dirs, app_state, chain, error::AppError, fleet, instructions, skill_store::SkillStore,
    tool_service,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(name = "patchbay-cli")]
#[command(about = "Shared-core CLI for Patchbay", version)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Where Patchbay keeps its data on this machine
    Status,
    Tools(ToolsArgs),
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
struct ToolsArgs {
    #[command(subcommand)]
    command: ToolsCommand,
}

#[derive(Subcommand, Debug)]
enum ToolsCommand {
    List,
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
    let store = app_state::initialize_cli_store()?;

    match cli.command {
        Commands::Status => {
            print_json(
                &AppStatus {
                    ok: true,
                    data_dir: app_dirs::base_dir().to_string_lossy().to_string(),
                    db_path: app_dirs::db_path().to_string_lossy().to_string(),
                },
                cli.json,
            );
            Ok(())
        }
        Commands::Tools(args) => run_tools(args, &store, cli.json),
        Commands::Chain(args) => run_chain(args, &store, cli.json),
        Commands::Instructions(args) => run_instructions(args, &store, cli.json),
        Commands::Fleet(args) => run_fleet(args, &store, cli.json),
    }
}

// ── repo ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AppStatus {
    ok: bool,
    data_dir: String,
    db_path: String,
}

fn run_tools(args: ToolsArgs, store: &SkillStore, json: bool) -> anyhow::Result<()> {
    match args.command {
        ToolsCommand::List => print_json(&tool_service::list_tool_info(store), json),
    }
    Ok(())
}

// ── skills ────────────────────────────────────────────────────────────────

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
        assert!(apply_outcome_succeeded(&apply_outcome(true, "repaired")));
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
