//! Instructions Doctor: the fourteen §3 rules, stable fingerprints, and ignore.
//!
//! Mirrors `chain::doctor`'s *shape* — a stable, never-localized rule id, a
//! `Severity` (the very enum chain defines, reused so the GUI's shared severity
//! axis stays one type), evidence, affected objects, advertised action codes, and
//! a fingerprint — but the rule set is `instructions.*`, the type-filter axis is
//! the rule id (this module has no chain-style `deviation` enum), and the
//! evidence is `{primary_path, counterpart_path?, metrics, locations[]}` instead
//! of traced symlink hops (design §3, §7).
//!
//! Unlike chain Doctor — which derives everything from an already-built topology
//! and never touches disk — instructions Doctor consumes the cost-scan's
//! classification (canonical state, per-agent entry state, resident sizes, global
//! surfaces) and performs *targeted, read-only* supplementary inspection for the
//! evidence the cost scan does not carry: `@import` resolution, block overlap,
//! the skill whitelist, and gitignore status. It never writes and never evaluates
//! instructions content (design §8).
//!
//! Fingerprint discipline (design §3): a finding's fingerprint hashes the rule id
//! plus *identity-level* evidence only — paths and names, never content or sizes.
//! Editing a body must not refresh the fingerprint (so an ignore survives edits);
//! relocating a file or renaming a skill must (so a stale ignore is reconsidered).
//! Each rule's identity fields are exactly the `→` column of the §3 table.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::core::chain::decisions::{self, FindingDecision};
// Reuse chain's Severity verbatim: the GUI filters both modules on one enum.
pub use crate::core::chain::doctor::Severity;

use super::blocks;
use super::import_resolver::{self, Import};
use super::scanner::{GlobalFile, ProjectScan, ScanReport};
use super::surfaces::Agent;

// ── rule catalogue (design §3) ──────────────────────────────────────────────

/// One of the fourteen instructions Doctor rules. The stable `id()` is the wire
/// contract and the `--rule` filter axis; `Deserialize` accepts that full id so a
/// GUI or Tauri filter is unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Rule {
    Uninitialized,
    MissingCanonical,
    DualBody,
    DuplicateContent,
    MissingEntry,
    SymlinkEntry,
    BrokenImport,
    ImportInCanonical,
    OversizedBody,
    HardCapRisk,
    SkillMissing,
    SkillUnmentioned,
    EntryGitignored,
    GlobalCost,
}

impl Rule {
    /// Every rule, in stable presentation order (roughly severity-major).
    pub const ALL: [Rule; 14] = [
        Rule::Uninitialized,
        Rule::MissingCanonical,
        Rule::DualBody,
        Rule::DuplicateContent,
        Rule::MissingEntry,
        Rule::SymlinkEntry,
        Rule::BrokenImport,
        Rule::ImportInCanonical,
        Rule::OversizedBody,
        Rule::HardCapRisk,
        Rule::SkillMissing,
        Rule::SkillUnmentioned,
        Rule::EntryGitignored,
        Rule::GlobalCost,
    ];

    /// Stable, namespaced, never-localized rule identifier (design §3).
    pub fn id(self) -> &'static str {
        match self {
            Rule::Uninitialized => "instructions.uninitialized",
            Rule::MissingCanonical => "instructions.missing_canonical",
            Rule::DualBody => "instructions.dual_body",
            Rule::DuplicateContent => "instructions.duplicate_content",
            Rule::MissingEntry => "instructions.missing_entry",
            Rule::SymlinkEntry => "instructions.symlink_entry",
            Rule::BrokenImport => "instructions.broken_import",
            Rule::ImportInCanonical => "instructions.import_in_canonical",
            Rule::OversizedBody => "instructions.oversized_body",
            Rule::HardCapRisk => "instructions.hard_cap_risk",
            Rule::SkillMissing => "instructions.skill_missing",
            Rule::SkillUnmentioned => "instructions.skill_unmentioned",
            Rule::EntryGitignored => "instructions.entry_gitignored",
            Rule::GlobalCost => "instructions.global_cost",
        }
    }

    fn severity(self) -> Severity {
        match self {
            Rule::BrokenImport => Severity::Violation,
            Rule::MissingCanonical
            | Rule::DualBody
            | Rule::DuplicateContent
            | Rule::MissingEntry
            | Rule::HardCapRisk
            | Rule::SkillMissing => Severity::Warning,
            Rule::ImportInCanonical | Rule::OversizedBody | Rule::EntryGitignored => {
                Severity::Advice
            }
            Rule::Uninitialized
            | Rule::SymlinkEntry
            | Rule::SkillUnmentioned
            | Rule::GlobalCost => Severity::Notice,
        }
    }

    /// Default action codes advertised for this rule (never localized; nothing is
    /// executed here). `broken_import` overrides per-finding (`init` only when the
    /// missing target is the canonical body).
    fn default_actions(self) -> &'static [&'static str] {
        match self {
            Rule::Uninitialized => &["init"],
            Rule::MissingCanonical | Rule::DualBody | Rule::MissingEntry | Rule::SymlinkEntry => {
                &["normalize"]
            }
            Rule::GlobalCost => &["open"],
            // Report-only rules (design §3 "动作" column): duplicate_content,
            // import_in_canonical, oversized_body, hard_cap_risk, skill_missing,
            // skill_unmentioned, entry_gitignored, and broken_import's default.
            _ => &[],
        }
    }

    /// Parse the full stable id (`instructions.dual_body`). Used by `Deserialize`.
    pub fn from_id(s: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.id() == s)
    }

    /// Parse a CLI `--rule` token: the full id or its short suffix
    /// (`dual_body`), so the CLI is forgiving while the wire id stays stable.
    pub fn from_token(s: &str) -> Option<Rule> {
        Rule::from_id(s).or_else(|| {
            Rule::ALL
                .into_iter()
                .find(|r| r.id().strip_prefix("instructions.") == Some(s))
        })
    }
}

impl TryFrom<String> for Rule {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Rule::from_id(&s).ok_or_else(|| format!("unknown instructions rule: {s}"))
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Violation => 0,
        Severity::Warning => 1,
        Severity::Advice => 2,
        Severity::Notice => 3,
    }
}

// ── finding shape (design §3, §7) ───────────────────────────────────────────

/// One affected object referenced by a finding. Structurally identical to
/// chain's so the GUI can render both modules' findings uniformly.
#[derive(Debug, Clone, Serialize)]
pub struct AffectedObject {
    /// "project" | "canonical" | "entry" | "skill" | "agent" | "global".
    pub kind: String,
    pub name: String,
    pub path: String,
}

/// A concrete file position behind a finding (an import line, a skill mention, an
/// oversized section's heading). Empty when a finding has no line-level anchor.
#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub path: String,
    pub line: usize,
}

/// The instructions-flavoured evidence: the object the finding is about, an
/// optional counterpart (the other body in a two-file rule), a free-form metric
/// bag (sizes, ratios, caps, reader sets — deterministic, BTree-ordered), and any
/// line-level locations. Replaces chain's traced-hop evidence (design §7).
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub primary_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterpart_path: Option<String>,
    pub metrics: BTreeMap<String, Value>,
    pub locations: Vec<Location>,
}

/// A single Doctor finding. Shares chain's field structure
/// (rule/severity/evidence/affected/actions/fingerprint) so the GUI treats both
/// alike; `evidence` renders per-module.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub evidence: Evidence,
    pub affected: Vec<AffectedObject>,
    pub actions: Vec<String>,
    pub fingerprint: String,
}

impl Finding {
    /// Build a finding, fingerprinting the rule id over exactly its identity
    /// fields (design §3's `→` column). `identity` must be identity-level only —
    /// paths and names — never content or sizes.
    fn new(
        rule: Rule,
        identity: &[&str],
        evidence: Evidence,
        affected: Vec<AffectedObject>,
    ) -> Self {
        Finding {
            rule: rule.id().to_string(),
            severity: rule.severity(),
            evidence,
            affected,
            actions: rule
                .default_actions()
                .iter()
                .map(|a| a.to_string())
                .collect(),
            fingerprint: fingerprint(rule.id(), identity),
        }
    }

    fn with_actions(mut self, actions: Vec<&str>) -> Self {
        self.actions = actions.into_iter().map(|a| a.to_string()).collect();
        self
    }
}

/// Deterministic hash over a rule id and its identity fields, hex-encoded and
/// NUL-separated so `["a","bc"]` and `["ab","c"]` never collide. Same scheme as
/// chain Doctor's fingerprint, kept local so the two modules stay decoupled.
fn fingerprint(rule: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rule.as_bytes());
    for part in parts {
        hasher.update([0u8]);
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn metrics(pairs: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

// ── filter + report (chain-compatible contract, design §5) ──────────────────

/// Which findings to keep. Empty vectors mean "no constraint"; the two axes
/// (severity, rule) combine with AND. `--rule` replaces chain's `--deviation`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DoctorFilter {
    #[serde(default)]
    pub severities: Vec<Severity>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl DoctorFilter {
    fn keeps(&self, f: &Finding) -> bool {
        (self.severities.is_empty() || self.severities.contains(&f.severity))
            && (self.rules.is_empty() || self.rules.iter().any(|r| r.id() == f.rule))
    }

    pub fn apply(&self, findings: Vec<Finding>) -> Vec<Finding> {
        findings.into_iter().filter(|f| self.keeps(f)).collect()
    }
}

/// Same shape as chain `DoctorReport`: visible findings, the ignored set (kept
/// for the restore panel), the pre-filter visible count, and a scan timestamp.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub findings: Vec<Finding>,
    pub ignored: Vec<Finding>,
    pub total: usize,
    pub scanned_at: i64,
}

/// Split findings into `(visible, ignored)` by matching each finding's
/// `(rule, fingerprint)` against persisted decisions — reusing chain's shared
/// decision store (disjoint rule prefixes mean `chain.*` decisions never match an
/// `instructions.*` finding). A stale fingerprint leaves its finding visible.
pub fn partition_by_decisions(
    findings: Vec<Finding>,
    decisions: &[FindingDecision],
) -> (Vec<Finding>, Vec<Finding>) {
    let mut visible = Vec::new();
    let mut ignored = Vec::new();
    for f in findings {
        if decisions::is_ignored(decisions, &f.rule, &f.fingerprint) {
            ignored.push(f);
        } else {
            visible.push(f);
        }
    }
    (visible, ignored)
}

// ── top-level diagnose ──────────────────────────────────────────────────────

/// Derive every finding from a cost `ScanReport` plus targeted read-only
/// inspection. `home` resolves `~/`-prefixed imports. Deterministic apart from
/// the on-disk state it inspects; never writes.
pub fn diagnose(scan: &ScanReport, home: &Path) -> Vec<Finding> {
    let installed: Vec<Agent> = scan
        .agents
        .iter()
        .filter_map(|k| Agent::from_key(k))
        .collect();
    let mut findings = Vec::new();
    for project in &scan.projects {
        diagnose_project(project, &installed, home, &mut findings);
    }
    for global in &scan.globals {
        findings.push(global_cost_finding(global));
    }
    sort_findings(&mut findings);
    findings
}

fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then_with(|| a.rule.cmp(&b.rule))
            .then_with(|| a.evidence.primary_path.cmp(&b.evidence.primary_path))
    });
}

fn diagnose_project(
    project: &ProjectScan,
    installed: &[Agent],
    home: &Path,
    out: &mut Vec<Finding>,
) {
    let root = PathBuf::from(&project.path);
    let claude_installed = installed.contains(&Agent::Claude);

    check_uninitialized(project, out);
    check_canonical_and_entries(project, claude_installed, out);
    check_imports(project, home, out);
    check_oversized(project, out);
    check_hard_cap(project, installed, &root, out);
    check_duplicate_content(project, installed, &root, out);
    check_skills(project, &root, out);
    if claude_installed {
        check_entry_gitignored(&root, out);
    }
}

// ── read helpers ────────────────────────────────────────────────────────────

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn project_object(project: &ProjectScan) -> AffectedObject {
    AffectedObject {
        kind: "project".to_string(),
        name: basename(&project.path),
        path: project.path.clone(),
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

// ── uninitialized ───────────────────────────────────────────────────────────

fn check_uninitialized(project: &ProjectScan, out: &mut Vec<Finding>) {
    let any_entry_file = project.entries.iter().any(|e| e.state != "missing");
    if project.canonical.exists || any_entry_file {
        return;
    }
    out.push(Finding::new(
        Rule::Uninitialized,
        &[&project.path],
        Evidence {
            primary_path: project.path.clone(),
            counterpart_path: None,
            metrics: metrics(vec![]),
            locations: vec![],
        },
        vec![project_object(project)],
    ));
}

// ── canonical / entry rules ─────────────────────────────────────────────────

fn check_canonical_and_entries(
    project: &ProjectScan,
    claude_installed: bool,
    out: &mut Vec<Finding>,
) {
    let canonical = &project.canonical;
    let claude_entries: Vec<_> = project
        .entries
        .iter()
        .filter(|e| e.agent == "claude")
        .collect();

    // missing_entry: Claude installed, canonical exists, no entry file at all.
    if claude_installed && canonical.exists && claude_entries.iter().all(|e| e.state == "missing") {
        if let Some(expected) = claude_entries.first() {
            out.push(Finding::new(
                Rule::MissingEntry,
                &[&project.path, "claude"],
                Evidence {
                    primary_path: expected.path.clone(),
                    counterpart_path: Some(canonical.path.clone()),
                    metrics: metrics(vec![]),
                    locations: vec![],
                },
                vec![
                    project_object(project),
                    agent_object("claude", &expected.path),
                ],
            ));
        }
    }

    for entry in claude_entries.iter().filter(|e| e.state != "missing") {
        match entry.state.as_str() {
            "body" if canonical.exists => {
                out.push(dual_body_finding(project, entry, canonical));
            }
            "body" => {
                // No canonical: content is trapped in a single agent's entry.
                out.push(Finding::new(
                    Rule::MissingCanonical,
                    &[&entry.path],
                    Evidence {
                        primary_path: entry.path.clone(),
                        counterpart_path: Some(canonical.path.clone()),
                        metrics: metrics(vec![("entry_bytes", json!(entry.bytes))]),
                        locations: vec![],
                    },
                    vec![project_object(project), entry_object(&entry.path)],
                ));
            }
            "symlink" if resolves_to_canonical(&entry.path, &canonical.path) => {
                let target = std::fs::read_link(&entry.path)
                    .map(|t| t.to_string_lossy().to_string())
                    .unwrap_or_default();
                out.push(Finding::new(
                    Rule::SymlinkEntry,
                    &[&entry.path],
                    Evidence {
                        primary_path: entry.path.clone(),
                        counterpart_path: Some(canonical.path.clone()),
                        metrics: metrics(vec![("link_target", json!(target))]),
                        locations: vec![],
                    },
                    vec![project_object(project), entry_object(&entry.path)],
                ));
            }
            _ => {}
        }
    }
}

fn dual_body_finding(
    project: &ProjectScan,
    entry: &super::scanner::EntryInfo,
    canonical: &super::scanner::CanonicalInfo,
) -> Finding {
    let entry_text = read_text(Path::new(&entry.path)).unwrap_or_default();
    let canon_text = read_text(Path::new(&canonical.path)).unwrap_or_default();
    let canon_set = blocks::fingerprint_set(&canon_text);
    let entry_blocks = blocks::blockize(&entry_text);
    let overlap = entry_blocks
        .iter()
        .filter(|b| canon_set.contains(&b.fingerprint))
        .count();
    Finding::new(
        Rule::DualBody,
        &[&entry.path, &canonical.path],
        Evidence {
            primary_path: entry.path.clone(),
            counterpart_path: Some(canonical.path.clone()),
            metrics: metrics(vec![
                ("entry_bytes", json!(entry.bytes)),
                ("canonical_bytes", json!(canonical.bytes)),
                ("entry_blocks", json!(entry_blocks.len())),
                ("canonical_blocks", json!(canon_set.len())),
                ("overlap_blocks", json!(overlap)),
            ]),
            locations: vec![],
        },
        vec![project_object(project), entry_object(&entry.path)],
    )
}

fn entry_object(path: &str) -> AffectedObject {
    AffectedObject {
        kind: "entry".to_string(),
        name: basename(path),
        path: path.to_string(),
    }
}

fn agent_object(agent: &str, path: &str) -> AffectedObject {
    AffectedObject {
        kind: "agent".to_string(),
        name: agent.to_string(),
        path: path.to_string(),
    }
}

/// Whether an entry path is a symlink resolving to the canonical body.
fn resolves_to_canonical(entry: &str, canonical: &str) -> bool {
    match (
        std::fs::canonicalize(entry),
        std::fs::canonicalize(canonical),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

// ── imports (broken_import / import_in_canonical) ───────────────────────────

fn check_imports(project: &ProjectScan, home: &Path, out: &mut Vec<Finding>) {
    let canonical = &project.canonical;
    // Files Claude reads and expands: the canonical body + existing Claude
    // entries. Other agents read `@path` as literal text, so imports are scoped
    // to the Claude surface (matrix conclusion ②).
    let mut sources: Vec<PathBuf> = Vec::new();
    if canonical.exists {
        sources.push(PathBuf::from(&canonical.path));
    }
    for e in project
        .entries
        .iter()
        .filter(|e| e.agent == "claude" && e.state != "missing")
    {
        sources.push(PathBuf::from(&e.path));
    }

    let imports = collect_imports(&sources, home);
    for imp in &imports {
        if !imp.exists {
            let is_canonical_target = imp.target == canonical.path;
            let finding = Finding::new(
                Rule::BrokenImport,
                &[&imp.source, &imp.target],
                Evidence {
                    primary_path: imp.source.clone(),
                    counterpart_path: Some(imp.target.clone()),
                    metrics: metrics(vec![
                        ("target", json!(imp.target)),
                        ("import", json!(imp.raw)),
                        ("target_is_canonical", json!(is_canonical_target)),
                    ]),
                    locations: vec![Location {
                        path: imp.source.clone(),
                        line: imp.line,
                    }],
                },
                vec![entry_object(&imp.source)],
            );
            // Special case (design §3): a wrapper/import pointing at a missing
            // canonical is fixable by `init`.
            out.push(if is_canonical_target {
                finding.with_actions(vec!["init"])
            } else {
                finding
            });
        }
        // import_in_canonical: any `@import` written IN the canonical body.
        if imp.source == canonical.path {
            out.push(Finding::new(
                Rule::ImportInCanonical,
                &[&canonical.path, &imp.target],
                Evidence {
                    primary_path: canonical.path.clone(),
                    counterpart_path: Some(imp.target.clone()),
                    metrics: metrics(vec![
                        ("target", json!(imp.target)),
                        ("import", json!(imp.raw)),
                    ]),
                    locations: vec![Location {
                        path: imp.source.clone(),
                        line: imp.line,
                    }],
                },
                vec![AffectedObject {
                    kind: "canonical".to_string(),
                    name: basename(&canonical.path),
                    path: canonical.path.clone(),
                }],
            ));
        }
    }
}

/// Resolve imports from every source, de-duplicated by `(source, target)` so a
/// diamond or a wrapper→canonical→target path counts each broken/canonical
/// import once.
fn collect_imports(sources: &[PathBuf], home: &Path) -> Vec<Import> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for src in sources {
        for imp in import_resolver::resolve(src, home).imports {
            if seen.insert((imp.source.clone(), imp.target.clone())) {
                out.push(imp);
            }
        }
    }
    out
}

// ── oversized_body ──────────────────────────────────────────────────────────

const OVERSIZED_LINES: usize = 200;
const OVERSIZED_BYTES: u64 = 8 * 1024;
const SECTION_LINE_LIMIT: usize = 80;
const FENCE_LINE_LIMIT: usize = 40;

fn check_oversized(project: &ProjectScan, out: &mut Vec<Finding>) {
    let canonical = &project.canonical;
    if !canonical.exists {
        return;
    }
    if canonical.lines <= OVERSIZED_LINES && canonical.bytes <= OVERSIZED_BYTES {
        return;
    }
    let text = read_text(Path::new(&canonical.path)).unwrap_or_default();
    let candidates = oversized_candidates(&text, &canonical.path);
    out.push(Finding::new(
        Rule::OversizedBody,
        &[&canonical.path],
        Evidence {
            primary_path: canonical.path.clone(),
            counterpart_path: None,
            metrics: metrics(vec![
                ("lines", json!(canonical.lines)),
                ("bytes", json!(canonical.bytes)),
                ("candidate_count", json!(candidates.len())),
            ]),
            locations: candidates,
        },
        vec![AffectedObject {
            kind: "canonical".to_string(),
            name: basename(&canonical.path),
            path: canonical.path.clone(),
        }],
    ));
}

/// Extraction candidates for a §3 pointer-style rewrite: a single `##`/`###`
/// section longer than 80 lines, or a fenced block longer than 40 lines. Reported
/// as guidance only (advice), so approximate span accounting is acceptable.
fn oversized_candidates(text: &str, path: &str) -> Vec<Location> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    // Oversized `##`/`###` sections: from the heading to the next heading of the
    // same or higher level.
    let mut i = 0;
    while i < lines.len() {
        if let Some(level) = heading_level(lines[i].trim_start()) {
            if level == 2 || level == 3 {
                let start = i;
                let mut j = i + 1;
                while j < lines.len() {
                    if let Some(next) = heading_level(lines[j].trim_start()) {
                        if next <= level {
                            break;
                        }
                    }
                    j += 1;
                }
                if j - start > SECTION_LINE_LIMIT {
                    out.push(Location {
                        path: path.to_string(),
                        line: start + 1,
                    });
                }
            }
        }
        i += 1;
    }

    // Oversized fenced blocks.
    for block in blocks::blockize(text) {
        let trimmed = block.text.trim_start();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if is_fence && block.text.lines().count() > FENCE_LINE_LIMIT {
            out.push(Location {
                path: path.to_string(),
                line: block.start_line,
            });
        }
    }
    out
}

/// ATX heading level (1–6) of a left-trimmed line, or `None`.
fn heading_level(trimmed: &str) -> Option<usize> {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    match trimmed[hashes..].chars().next() {
        None => Some(hashes),
        Some(c) if c == ' ' || c == '\t' => Some(hashes),
        _ => None,
    }
}

// ── hard_cap_risk ───────────────────────────────────────────────────────────

const CODEX_CAP_BYTES: u64 = 32 * 1024;
const ANTIGRAVITY_CAP_CHARS: usize = 12_000;

fn check_hard_cap(project: &ProjectScan, installed: &[Agent], root: &Path, out: &mut Vec<Finding>) {
    // Codex: merged resident chain over 32 KiB is truncated.
    if installed.contains(&Agent::Codex) {
        if let Some(res) = project.resident.iter().find(|r| r.agent == "codex") {
            let actual = res.project_bytes + res.global_bytes;
            if actual > CODEX_CAP_BYTES {
                out.push(Finding::new(
                    Rule::HardCapRisk,
                    &[&project.path, "codex"],
                    Evidence {
                        primary_path: project.path.clone(),
                        counterpart_path: None,
                        metrics: metrics(vec![
                            ("cap_bytes", json!(CODEX_CAP_BYTES)),
                            ("actual_bytes", json!(actual)),
                        ]),
                        locations: vec![],
                    },
                    vec![
                        project_object(project),
                        agent_object("codex", &project.path),
                    ],
                ));
            }
        }
    }

    // Antigravity: any single `.agents/rules/` file over 12,000 characters.
    if installed.contains(&Agent::Antigravity) {
        let mut offenders = Vec::new();
        if let Ok(rd) = std::fs::read_dir(root.join(".agents/rules")) {
            let mut paths: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            paths.sort();
            for p in paths {
                if let Some(text) = read_text(&p) {
                    let chars = text.chars().count();
                    if chars > ANTIGRAVITY_CAP_CHARS {
                        offenders.push((p.to_string_lossy().to_string(), chars));
                    }
                }
            }
        }
        if !offenders.is_empty() {
            let files: Vec<Value> = offenders
                .iter()
                .map(|(p, c)| json!({"path": p, "chars": c}))
                .collect();
            let locations = offenders
                .iter()
                .map(|(p, _)| Location {
                    path: p.clone(),
                    line: 1,
                })
                .collect();
            out.push(Finding::new(
                Rule::HardCapRisk,
                &[&project.path, "antigravity"],
                Evidence {
                    primary_path: project.path.clone(),
                    counterpart_path: None,
                    metrics: metrics(vec![
                        ("cap_chars", json!(ANTIGRAVITY_CAP_CHARS)),
                        ("files", json!(files)),
                    ]),
                    locations,
                },
                vec![
                    project_object(project),
                    agent_object("antigravity", &project.path),
                ],
            ));
        }
    }
}

// ── duplicate_content ───────────────────────────────────────────────────────

const DUPLICATE_RATIO: f64 = 0.5;

fn check_duplicate_content(
    project: &ProjectScan,
    installed: &[Agent],
    root: &Path,
    out: &mut Vec<Finding>,
) {
    let canonical = &project.canonical;
    if !canonical.exists {
        return;
    }
    let canon_text = read_text(Path::new(&canonical.path)).unwrap_or_default();
    let canon_blocks = blocks::blockize(&canon_text);
    if canon_blocks.is_empty() {
        return;
    }

    // Native dual-read companion files: only meaningful when their agent is
    // installed (an unread file costs nothing).
    let companions: [(Agent, &str); 2] = [
        (Agent::Antigravity, "GEMINI.md"),
        (Agent::Copilot, ".github/copilot-instructions.md"),
    ];
    for (agent, rel) in companions {
        if !installed.contains(&agent) {
            continue;
        }
        let path = root.join(rel);
        let Some(text) = read_text(&path) else {
            continue;
        };
        let file_set = blocks::fingerprint_set(&text);
        let present = canon_blocks
            .iter()
            .filter(|b| file_set.contains(&b.fingerprint))
            .count();
        let ratio = present as f64 / canon_blocks.len() as f64;
        if ratio >= DUPLICATE_RATIO {
            let path_str = path.to_string_lossy().to_string();
            out.push(Finding::new(
                Rule::DuplicateContent,
                &[&path_str, &canonical.path],
                Evidence {
                    primary_path: path_str.clone(),
                    counterpart_path: Some(canonical.path.clone()),
                    metrics: metrics(vec![
                        ("overlap_ratio", json!((ratio * 1000.0).round() / 1000.0)),
                        ("canonical_blocks", json!(canon_blocks.len())),
                        ("overlap_blocks", json!(present)),
                    ]),
                    locations: vec![],
                },
                vec![project_object(project), entry_object(&path_str)],
            ));
        }
    }
}

// ── skills reconciliation (design §3 对账语义) ──────────────────────────────

struct SkillEntry {
    /// Directory basename (display + one identity candidate).
    dir_name: String,
    /// Lowercased name candidates: directory name and `SKILL.md` `name:`.
    candidates: Vec<String>,
}

fn check_skills(project: &ProjectScan, root: &Path, out: &mut Vec<Finding>) {
    let whitelist = skill_whitelist(root);

    // The managed surface: canonical + entries + append layers (design §3 scope).
    // Personal-layer files are excluded. Each is (path, text).
    let managed = managed_surface_files(project, root);
    let joined_lc: String = managed
        .iter()
        .map(|(_, t)| t.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");

    // skill_unmentioned: a whitelist skill whose name appears nowhere.
    for skill in &whitelist {
        let mentioned = skill
            .candidates
            .iter()
            .any(|name| mentions(&joined_lc, name));
        if !mentioned {
            out.push(Finding::new(
                Rule::SkillUnmentioned,
                &[&project.path, &skill.dir_name],
                Evidence {
                    primary_path: root
                        .join(".agents/skills")
                        .join(&skill.dir_name)
                        .to_string_lossy()
                        .to_string(),
                    counterpart_path: None,
                    metrics: metrics(vec![("skill", json!(skill.dir_name))]),
                    locations: vec![],
                },
                vec![project_object(project), skill_object(&skill.dir_name, root)],
            ));
        }
    }

    // skill_missing: an instructions reference to a skill not in the whitelist.
    // v1 detects slash-form multi-segment-kebab references (`/skill-name`) that
    // are not embedded in a path — a low-false-positive signal; the design left
    // the reference-detection mechanism unspecified, so this is narrowed
    // deliberately and false positives route to ignore (§3). (Ticket deviation.)
    let candidate_names: Vec<String> = whitelist
        .iter()
        .flat_map(|s| s.candidates.clone())
        .collect();
    let mut reported: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (path, text) in &managed {
        for (line, name) in slash_skill_refs(text) {
            let name_lc = name.to_lowercase();
            if candidate_names.contains(&name_lc) {
                continue; // resolves to a whitelist skill
            }
            if !reported.insert(name.clone()) {
                continue; // one finding per referenced name
            }
            out.push(Finding::new(
                Rule::SkillMissing,
                &[&project.path, &name],
                Evidence {
                    primary_path: path.clone(),
                    counterpart_path: None,
                    metrics: metrics(vec![("skill", json!(name))]),
                    locations: vec![Location {
                        path: path.clone(),
                        line,
                    }],
                },
                vec![project_object(project)],
            ));
        }
    }
}

fn skill_object(dir_name: &str, root: &Path) -> AffectedObject {
    AffectedObject {
        kind: "skill".to_string(),
        name: dir_name.to_string(),
        path: root
            .join(".agents/skills")
            .join(dir_name)
            .to_string_lossy()
            .to_string(),
    }
}

/// The `.agents/skills/` whitelist: one entry per top-level item (dir or
/// symlink), with lowercased name candidates from the directory name and the
/// `SKILL.md` `name:` front-matter field.
fn skill_whitelist(root: &Path) -> Vec<SkillEntry> {
    let dir = root.join(".agents/skills");
    let mut entries = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return entries;
    };
    let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for item in items {
        // Skills are directories or symlinks to directories; skip stray files.
        if !item.is_dir() {
            continue;
        }
        let dir_name = match item.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };
        let mut candidates = vec![dir_name.to_lowercase()];
        if let Some(name) = read_skill_name(&item.join("SKILL.md")) {
            let name_lc = name.to_lowercase();
            if !candidates.contains(&name_lc) {
                candidates.push(name_lc);
            }
        }
        entries.push(SkillEntry {
            dir_name,
            candidates,
        });
    }
    entries
}

/// Extract the `name:` value from a `SKILL.md` YAML front-matter block.
fn read_skill_name(path: &Path) -> Option<String> {
    let text = read_text(path)?;
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(rest) = t.strip_prefix("name:") {
            let name = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// The managed-surface files that exist, as `(path, text)`: canonical body, all
/// installed agents' entry files, and native append layers. Personal-layer files
/// are excluded (design §3 scope).
fn managed_surface_files(project: &ProjectScan, root: &Path) -> Vec<(String, String)> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if project.canonical.exists {
        paths.push(PathBuf::from(&project.canonical.path));
    }
    for e in project.entries.iter().filter(|e| e.state != "missing") {
        paths.push(PathBuf::from(&e.path));
    }
    // Native append layers (present regardless of whether scan listed an entry).
    for rel in ["GEMINI.md", ".github/copilot-instructions.md"] {
        let p = root.join(rel);
        if p.is_file() {
            paths.push(p);
        }
    }
    // De-duplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in paths {
        if !seen.insert(p.clone()) {
            continue;
        }
        if let Some(text) = read_text(&p) {
            out.push((p.to_string_lossy().to_string(), text));
        }
    }
    out
}

/// Whether `name_lc` appears in `text_lc` at a word boundary (boundary char =
/// `[^a-z0-9-]`), case-insensitive (both already lowercased). Matches inside code
/// spans and fenced blocks too (design §3 对账语义).
fn mentions(text_lc: &str, name_lc: &str) -> bool {
    if name_lc.is_empty() {
        return false;
    }
    let bytes = text_lc.as_bytes();
    let name = name_lc.as_bytes();
    let mut i = 0;
    while let Some(pos) = find_from(bytes, name, i) {
        let before_ok = pos == 0 || !is_name_byte(bytes[pos - 1]);
        let after = pos + name.len();
        let after_ok = after >= bytes.len() || !is_name_byte(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        i = pos + 1;
    }
    false
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// A name byte for word-boundary purposes: `[a-z0-9-]` (names are kebab-case).
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// Slash-form multi-segment-kebab skill references in `text`, as `(line, name)`.
/// A reference is `/<kebab-with-hyphen>` where the `/` is at a word boundary and
/// the token is not embedded in a path or filename (the following character is
/// not `/`, `.`, or another name character). Deliberately conservative.
fn slash_skill_refs(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'/' {
                // `/` must be at a word boundary (start or a non-path char before).
                let boundary = i == 0 || !is_ref_boundary_byte(bytes[i - 1]);
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && is_name_byte(bytes[end]) {
                    end += 1;
                }
                let token = &line[start..end];
                let has_hyphen = token.contains('-');
                let not_path_or_file = end >= bytes.len()
                    || (bytes[end] != b'/' && bytes[end] != b'.' && !is_name_byte(bytes[end]));
                if boundary && has_hyphen && not_path_or_file && is_kebab(token) {
                    out.push((idx + 1, token.to_string()));
                }
                i = end.max(i + 1);
                continue;
            }
            i += 1;
        }
    }
    out
}

/// Characters that, immediately before a `/`, mean it is part of a path rather
/// than a slash-command boundary.
fn is_ref_boundary_byte(b: u8) -> bool {
    is_name_byte(b) || b == b'/' || b == b'.' || b == b'_' || b == b'~'
}

/// Whether a token is a clean kebab name: lowercase alphanumerics and hyphens,
/// no leading/trailing/double hyphen.
fn is_kebab(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with('-')
        && !token.ends_with('-')
        && !token.contains("--")
        && token.bytes().all(is_name_byte)
}

// ── entry_gitignored ────────────────────────────────────────────────────────

fn check_entry_gitignored(root: &Path, out: &mut Vec<Finding>) {
    // The would-be wrapper entry, whether or not it exists yet (design §3).
    let entry = root.join("CLAUDE.md");
    let entry_str = entry.to_string_lossy().to_string();
    let Ok(repo) = git2::Repository::discover(root) else {
        return;
    };
    // `is_path_ignored` is authoritative (respects nested and global excludes).
    if !repo.is_path_ignored(&entry).unwrap_or(false) {
        return;
    }
    let (source, line, pattern) = gitignore_match_evidence(root, "CLAUDE.md");
    let mut m = vec![("ignored", json!(true))];
    if let Some(p) = &pattern {
        m.push(("pattern", json!(p)));
    }
    if let Some(s) = &source {
        m.push(("source", json!(s)));
    }
    let locations = match (source, line) {
        (Some(s), Some(l)) => vec![Location { path: s, line: l }],
        _ => vec![],
    };
    out.push(Finding::new(
        Rule::EntryGitignored,
        &[&entry_str],
        Evidence {
            primary_path: entry_str.clone(),
            counterpart_path: None,
            metrics: metrics(m),
            locations,
        },
        vec![entry_object(&entry_str)],
    ));
}

/// Best-effort evidence for which `.gitignore` line ignores `basename`: scan the
/// project-root `.gitignore` for a matching pattern. The *trigger* is already
/// git2-authoritative; this only enriches the report (an ignore via a nested or
/// global exclude yields no line here).
fn gitignore_match_evidence(
    root: &Path,
    basename: &str,
) -> (Option<String>, Option<usize>, Option<String>) {
    let gitignore = root.join(".gitignore");
    let Some(text) = read_text(&gitignore) else {
        return (None, None, None);
    };
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        if simple_gitignore_match(line, basename) {
            return (
                Some(".gitignore".to_string()),
                Some(idx + 1),
                Some(line.to_string()),
            );
        }
    }
    (None, None, None)
}

/// A minimal gitignore-pattern test against a bare basename: handles exact names,
/// a leading `/` anchor, and simple `*` globs. Evidence-only, not a full matcher.
fn simple_gitignore_match(pattern: &str, basename: &str) -> bool {
    let pat = pattern.trim_end_matches('/');
    let pat = pat.strip_prefix('/').unwrap_or(pat);
    if pat == basename {
        return true;
    }
    if pat.contains('*') {
        return glob_match(pat, basename);
    }
    false
}

/// A tiny `*`-glob matcher (no `**`, no character classes) for evidence use.
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(idx) => {
                if i == 0 && idx != 0 {
                    return false; // leading literal must anchor at the start
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }
    // A trailing non-`*` segment must reach the end.
    if let Some(last) = parts.last() {
        if !last.is_empty() && !pattern.ends_with('*') {
            return text.ends_with(last);
        }
    }
    true
}

// ── global_cost ─────────────────────────────────────────────────────────────

fn global_cost_finding(global: &GlobalFile) -> Finding {
    Finding::new(
        Rule::GlobalCost,
        &[&global.path],
        Evidence {
            primary_path: global.path.clone(),
            counterpart_path: None,
            metrics: metrics(vec![
                ("bytes", json!(global.bytes)),
                ("est_tokens", json!(global.est_tokens)),
                ("readers", json!(global.readers)),
            ]),
            locations: vec![],
        },
        vec![AffectedObject {
            kind: "global".to_string(),
            name: basename(&global.path),
            path: global.path.clone(),
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::instructions::scanner;
    use std::fs;
    use std::path::Path;
    use tempfile::{tempdir, TempDir};

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Run the real scan → diagnose pipeline against a project root, home, and
    /// installed set. Matches production: Doctor consumes the cost scan.
    fn diag(root: &Path, home: &Path, installed: &[Agent]) -> Vec<Finding> {
        let scan = scanner::scan_with(&[root.to_path_buf()], home, installed, 0);
        diagnose(&scan, home)
    }

    fn has(findings: &[Finding], id: &str) -> bool {
        findings.iter().any(|f| f.rule == id)
    }

    fn get<'a>(findings: &'a [Finding], id: &str) -> &'a Finding {
        findings
            .iter()
            .find(|f| f.rule == id)
            .unwrap_or_else(|| panic!("missing {id} in {:?}", ids(findings)))
    }

    fn ids(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule.as_str()).collect()
    }

    /// An empty home so `globals` is empty and project rules are isolated.
    fn empty_home() -> TempDir {
        tempdir().unwrap()
    }

    // ── rule id / severity / filter contract ────────────────────────────────

    #[test]
    fn rule_ids_are_stable_and_prefixed() {
        for rule in Rule::ALL {
            assert!(rule.id().starts_with("instructions."));
            assert_eq!(Rule::from_id(rule.id()), Some(rule));
            let short = rule.id().strip_prefix("instructions.").unwrap();
            assert_eq!(Rule::from_token(short), Some(rule));
            assert_eq!(Rule::from_token(rule.id()), Some(rule));
        }
        assert_eq!(Rule::from_id("instructions.nope"), None);
        assert_eq!(Rule::from_token("nope"), None);
    }

    // ── 1. uninitialized ────────────────────────────────────────────────────

    #[test]
    fn uninitialized_triggers_on_empty_project() {
        let root = tempdir().unwrap();
        let home = empty_home();
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.uninitialized"));

        // Non-trigger: a canonical body present.
        write(&root.path().join("AGENTS.md"), "# body\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.uninitialized"));
    }

    // ── 2. missing_canonical ────────────────────────────────────────────────

    #[test]
    fn missing_canonical_triggers_when_body_lives_in_entry() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(
            &root.path().join("CLAUDE.md"),
            "# real instructions\nbody\n",
        );
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.missing_canonical"));

        // Non-trigger: canonical exists.
        write(&root.path().join("AGENTS.md"), "# body\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.missing_canonical"));
    }

    // ── 3. dual_body ────────────────────────────────────────────────────────

    #[test]
    fn dual_body_triggers_with_body_entry_and_canonical() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# shared\ncommon\n");
        write(
            &root.path().join("CLAUDE.md"),
            "# shared\ncommon\nclaude-only\n",
        );
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.dual_body"));
        let m = &get(&f, "instructions.dual_body").evidence.metrics;
        assert!(m.contains_key("overlap_blocks"));

        // Non-trigger: a pure wrapper entry.
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.dual_body"));
    }

    // ── 4. duplicate_content ────────────────────────────────────────────────

    #[test]
    fn duplicate_content_triggers_on_overlapping_companion() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# one\n\npara two\n");
        // GEMINI.md repeats both canonical blocks (ratio 1.0 ≥ 0.5).
        write(
            &root.path().join("GEMINI.md"),
            "# one\n\npara two\n\nextra\n",
        );
        let f = diag(root.path(), home.path(), &[Agent::Antigravity]);
        assert!(has(&f, "instructions.duplicate_content"));

        // Non-trigger: distinct companion content.
        write(&root.path().join("GEMINI.md"), "wholly different\n");
        let f = diag(root.path(), home.path(), &[Agent::Antigravity]);
        assert!(!has(&f, "instructions.duplicate_content"));
    }

    // ── 5. missing_entry ────────────────────────────────────────────────────

    #[test]
    fn missing_entry_triggers_when_claude_has_no_entry() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# body\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.missing_entry"));

        // Non-trigger: a wrapper entry exists.
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.missing_entry"));
    }

    // ── 6. symlink_entry ────────────────────────────────────────────────────

    #[test]
    fn symlink_entry_triggers_on_symlink_to_canonical() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# body\n");
        crate::core::test_support::expect_symlink_file(
            std::path::Path::new("AGENTS.md"),
            &root.path().join("CLAUDE.md"),
        );
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.symlink_entry"));

        // Non-trigger: a wrapper file instead of a symlink.
        fs::remove_file(root.path().join("CLAUDE.md")).unwrap();
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.symlink_entry"));
    }

    // ── 7. broken_import ────────────────────────────────────────────────────

    #[test]
    fn broken_import_triggers_and_flags_canonical_target() {
        let root = tempdir().unwrap();
        let home = empty_home();
        // Wrapper points at a missing canonical → broken_import with `init`.
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        let broken = get(&f, "instructions.broken_import");
        assert_eq!(broken.severity, Severity::Violation);
        assert_eq!(broken.actions, vec!["init".to_string()]);

        // A non-canonical broken import carries no auto-fix action.
        write(&root.path().join("AGENTS.md"), "@nowhere.md\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        let broken = f
            .iter()
            .find(|f| {
                f.rule == "instructions.broken_import"
                    && f.evidence.counterpart_path.as_deref()
                        == Some(root.path().join("nowhere.md").to_string_lossy().as_ref())
            })
            .expect("non-canonical broken import present");
        assert!(broken.actions.is_empty());

        // Non-trigger: every import resolves.
        write(&root.path().join("AGENTS.md"), "# body\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.broken_import"));
    }

    // ── 8. import_in_canonical ──────────────────────────────────────────────

    #[test]
    fn import_in_canonical_triggers_on_at_import() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("sidecar.md"), "extra\n");
        write(&root.path().join("AGENTS.md"), "# body\n\n@sidecar.md\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.import_in_canonical"));

        // Non-trigger: no imports in the canonical.
        write(&root.path().join("AGENTS.md"), "# body\nno imports\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.import_in_canonical"));
    }

    // ── 9. oversized_body ───────────────────────────────────────────────────

    #[test]
    fn oversized_body_triggers_over_line_limit() {
        let root = tempdir().unwrap();
        let home = empty_home();
        let big = "line\n".repeat(250);
        write(&root.path().join("AGENTS.md"), &big);
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.oversized_body"));

        // Non-trigger: a short body.
        write(&root.path().join("AGENTS.md"), "# short\nbody\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.oversized_body"));
    }

    // ── 10. hard_cap_risk ───────────────────────────────────────────────────

    #[test]
    fn hard_cap_risk_triggers_for_codex_over_32kib() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), &"a".repeat(33 * 1024));
        let f = diag(root.path(), home.path(), &[Agent::Codex]);
        let cap = get(&f, "instructions.hard_cap_risk");
        assert_eq!(cap.severity, Severity::Warning);

        // Non-trigger: a small canonical.
        write(&root.path().join("AGENTS.md"), "# small\n");
        let f = diag(root.path(), home.path(), &[Agent::Codex]);
        assert!(!has(&f, "instructions.hard_cap_risk"));
    }

    #[test]
    fn hard_cap_risk_triggers_for_antigravity_rule_file() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# body\n");
        write(
            &root.path().join(".agents/rules/big.md"),
            &"x".repeat(12_001),
        );
        let f = diag(root.path(), home.path(), &[Agent::Antigravity]);
        assert!(has(&f, "instructions.hard_cap_risk"));
    }

    // ── 11. skill_missing ───────────────────────────────────────────────────

    #[test]
    fn skill_missing_triggers_on_unknown_slash_reference() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(
            &root.path().join("AGENTS.md"),
            "# body\n\nRun /ghost-skill to proceed.\n",
        );
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.skill_missing"));

        // Non-trigger: the referenced skill is whitelisted.
        make_skill(
            &root.path().join(".agents/skills/ghost-skill"),
            "ghost-skill",
        );
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.skill_missing"));
    }

    #[test]
    fn skill_missing_ignores_file_paths() {
        let root = tempdir().unwrap();
        let home = empty_home();
        // Path segments (`/path/to/proj`) must not read as skill references.
        write(
            &root.path().join("AGENTS.md"),
            "# body\n\nSee /path/to/some-dir/file.md and /usr/local/bin.\n",
        );
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.skill_missing"));
    }

    // ── 12. skill_unmentioned ───────────────────────────────────────────────

    #[test]
    fn skill_unmentioned_triggers_for_unreferenced_whitelist_skill() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# body\nnothing here\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        make_skill(&root.path().join(".agents/skills/foo-bar"), "foo-bar");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&f, "instructions.skill_unmentioned"));

        // Non-trigger: the skill is mentioned (in a code span, boundary-matched).
        write(
            &root.path().join("AGENTS.md"),
            "# body\nUse the `foo-bar` skill.\n",
        );
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.skill_unmentioned"));
    }

    // ── 13. entry_gitignored ────────────────────────────────────────────────

    #[test]
    fn entry_gitignored_triggers_when_wrapper_is_ignored() {
        let root = tempdir().unwrap();
        let home = empty_home();
        git2::Repository::init(root.path()).unwrap();
        write(&root.path().join(".gitignore"), "CLAUDE.md\n");
        write(&root.path().join("AGENTS.md"), "# body\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        let gi = get(&f, "instructions.entry_gitignored");
        assert_eq!(gi.evidence.metrics.get("pattern").unwrap(), "CLAUDE.md");

        // Non-trigger: not ignored.
        write(&root.path().join(".gitignore"), "other.txt\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.entry_gitignored"));
    }

    // ── 14. global_cost ─────────────────────────────────────────────────────

    #[test]
    fn global_cost_triggers_for_a_global_surface() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&home.path().join(".claude/CLAUDE.md"), "global prefs\n");
        write(&root.path().join("AGENTS.md"), "# body\n");
        let f = diag(root.path(), home.path(), &[Agent::Claude]);
        let gc = get(&f, "instructions.global_cost");
        assert_eq!(gc.severity, Severity::Notice);
        assert!(gc.evidence.metrics.contains_key("readers"));

        // Non-trigger: no global files.
        let f = diag(root.path(), empty_home().path(), &[Agent::Claude]);
        assert!(!has(&f, "instructions.global_cost"));
    }

    // ── fingerprint stability (design §3, AC2) ──────────────────────────────

    #[test]
    fn fingerprint_survives_content_edits() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# shared\ncommon\n");
        write(&root.path().join("CLAUDE.md"), "# shared\nclaude body\n");
        let fp1 = get(
            &diag(root.path(), home.path(), &[Agent::Claude]),
            "instructions.dual_body",
        )
        .fingerprint
        .clone();

        // Edit BOTH bodies' content — identity (the two paths) is unchanged.
        write(
            &root.path().join("AGENTS.md"),
            "# shared\ncommon\nnew canonical line\n",
        );
        write(
            &root.path().join("CLAUDE.md"),
            "# shared\nedited claude body\n",
        );
        let fp2 = get(
            &diag(root.path(), home.path(), &[Agent::Claude]),
            "instructions.dual_body",
        )
        .fingerprint
        .clone();
        assert_eq!(fp1, fp2, "editing content must not refresh the fingerprint");
    }

    #[test]
    fn fingerprint_changes_when_project_relocates() {
        let home = empty_home();
        let mk = |root: &Path| {
            write(&root.join("AGENTS.md"), "# shared\ncommon\n");
            write(&root.join("CLAUDE.md"), "# shared\nclaude body\n");
        };
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        mk(a.path());
        mk(b.path());
        let fp_a = get(
            &diag(a.path(), home.path(), &[Agent::Claude]),
            "instructions.dual_body",
        )
        .fingerprint
        .clone();
        let fp_b = get(
            &diag(b.path(), home.path(), &[Agent::Claude]),
            "instructions.dual_body",
        )
        .fingerprint
        .clone();
        assert_ne!(fp_a, fp_b, "a different path must refresh the fingerprint");
    }

    #[test]
    fn fingerprint_changes_when_skill_is_renamed() {
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("AGENTS.md"), "# body\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        make_skill(&root.path().join(".agents/skills/old-name"), "old-name");
        let fp1 = get(
            &diag(root.path(), home.path(), &[Agent::Claude]),
            "instructions.skill_unmentioned",
        )
        .fingerprint
        .clone();

        fs::remove_dir_all(root.path().join(".agents/skills/old-name")).unwrap();
        make_skill(&root.path().join(".agents/skills/new-name"), "new-name");
        let fp2 = get(
            &diag(root.path(), home.path(), &[Agent::Claude]),
            "instructions.skill_unmentioned",
        )
        .fingerprint
        .clone();
        assert_ne!(fp1, fp2, "renaming a skill must refresh the fingerprint");
    }

    fn make_skill(dir: &Path, name: &str) {
        write(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: fixture\n---\nbody\n"),
        );
    }

    // ── ignore round-trip over the shared decision store (AC3) ──────────────

    #[test]
    fn ignore_hides_from_visible_then_restore_reappears() {
        use crate::core::skill_store::SkillStore;

        let root = tempdir().unwrap();
        let home = empty_home();
        write(
            &root.path().join("CLAUDE.md"),
            "# real instructions\nbody\n",
        );
        let all = diag(root.path(), home.path(), &[Agent::Claude]);
        let target = get(&all, "instructions.missing_canonical").clone();

        let db = tempdir().unwrap();
        let store = SkillStore::new(&db.path().join("patchbay.db")).unwrap();

        // Nothing ignored yet: the finding is visible.
        let decisions = decisions::load(&store).unwrap();
        let (visible, ignored) = partition_by_decisions(all.clone(), &decisions);
        assert!(has(&visible, "instructions.missing_canonical"));
        assert!(ignored.is_empty());

        // Ignore it via the shared store, then it moves to `ignored`.
        decisions::add(
            &store,
            FindingDecision {
                rule: target.rule.clone(),
                fingerprint: target.fingerprint.clone(),
                kind: decisions::KIND_IGNORED.to_string(),
                note: None,
                created_at: 1,
            },
        )
        .unwrap();
        let decisions = decisions::load(&store).unwrap();
        let (visible, ignored) = partition_by_decisions(all.clone(), &decisions);
        assert!(!has(&visible, "instructions.missing_canonical"));
        assert!(ignored.iter().any(|f| f.fingerprint == target.fingerprint));

        // Restore: it returns to the visible set.
        decisions::remove(&store, &target.rule, &target.fingerprint).unwrap();
        let decisions = decisions::load(&store).unwrap();
        let (visible, _) = partition_by_decisions(all, &decisions);
        assert!(has(&visible, "instructions.missing_canonical"));
    }

    #[test]
    fn a_chain_decision_never_hides_an_instructions_finding() {
        // Disjoint rule prefixes: a `chain.*` decision must not match an
        // `instructions.*` finding sharing the same fingerprint string.
        let root = tempdir().unwrap();
        let home = empty_home();
        write(&root.path().join("CLAUDE.md"), "# body content\n");
        let all = diag(root.path(), home.path(), &[Agent::Claude]);
        let fp = get(&all, "instructions.missing_canonical")
            .fingerprint
            .clone();

        let decisions = vec![FindingDecision {
            rule: "chain.broken_link".to_string(),
            fingerprint: fp,
            kind: decisions::KIND_IGNORED.to_string(),
            note: None,
            created_at: 1,
        }];
        let (visible, ignored) = partition_by_decisions(all, &decisions);
        assert!(has(&visible, "instructions.missing_canonical"));
        assert!(ignored.is_empty());
    }

    // ── filter ──────────────────────────────────────────────────────────────

    #[test]
    fn filter_narrows_by_severity_and_rule() {
        let root = tempdir().unwrap();
        let home = empty_home();
        // A violation (broken non-canonical import) and a warning (a body entry
        // alongside the canonical).
        write(&root.path().join("AGENTS.md"), "# body\n\n@nowhere.md\n");
        write(&root.path().join("CLAUDE.md"), "# real\nclaude body\n");
        let all = diag(root.path(), home.path(), &[Agent::Claude]);
        assert!(has(&all, "instructions.broken_import")); // violation
        assert!(has(&all, "instructions.dual_body")); // warning

        let by_sev = DoctorFilter {
            severities: vec![Severity::Violation],
            rules: vec![],
        }
        .apply(all.clone());
        assert!(by_sev.iter().all(|f| f.severity == Severity::Violation));
        assert!(has(&by_sev, "instructions.broken_import"));

        let by_rule = DoctorFilter {
            severities: vec![],
            rules: vec![Rule::DualBody],
        }
        .apply(all.clone());
        assert_eq!(ids(&by_rule), vec!["instructions.dual_body"]);

        // Empty filter is a no-op; AND across axes with no overlap yields nothing.
        assert_eq!(DoctorFilter::default().apply(all.clone()).len(), all.len());
        let none = DoctorFilter {
            severities: vec![Severity::Notice],
            rules: vec![Rule::BrokenImport],
        }
        .apply(all);
        assert!(none.is_empty());
    }

    #[test]
    fn filter_deserializes_rules_from_stable_ids() {
        let filter: DoctorFilter = serde_json::from_str(
            r#"{"severities":["warning"],"rules":["instructions.dual_body"]}"#,
        )
        .unwrap();
        assert_eq!(filter.severities, vec![Severity::Warning]);
        assert_eq!(filter.rules, vec![Rule::DualBody]);
        // An unknown id is rejected, never silently dropped.
        assert!(
            serde_json::from_str::<DoctorFilter>(r#"{"rules":["instructions.nope"]}"#).is_err()
        );
    }
}
