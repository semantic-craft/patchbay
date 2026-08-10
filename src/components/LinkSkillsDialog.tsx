import { useMemo, useState } from "react";
import { X, Link2, ArrowLeft, ShieldCheck, ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { cn } from "../utils";
import {
  chainApplyLink,
  chainApplyUnlink,
  chainPlanLink,
  chainPlanUnlink,
} from "../lib/tauri";
import type {
  ChainLinkPlan,
  ChainPreset,
  ChainProject,
  ChainRepo,
  ChainUnlinkPlan,
} from "../lib/tauri";
import { TONE_BADGE } from "../lib/chainUi";
import { CHAIN_AGENTS, SkillPicker } from "./SkillPicker";

interface Props {
  open: boolean;
  /** The project whose whitelist is being edited; null while closed. */
  project: ChainProject | null;
  repos: ChainRepo[];
  /** Preset 起步 pills（#36：与接入向导共用同一挑选流程）。 */
  presets?: ChainPreset[];
  onClose: () => void;
  onLinked: () => void;
}

const ACTION_TONE: Record<string, keyof typeof TONE_BADGE> = {
  created: "ok",
  repaired: "ok",
  exists: "dim",
  removed: "ok",
  absent: "dim",
  skipped: "warn",
  conflict: "warn",
  error: "err",
};

/** A single previewed target or applied result — both carry name/path/action/message. */
interface ItemLike {
  name: string;
  path: string;
  action: string;
  message: string | null;
}

function ItemRow({ item }: { item: ItemLike }) {
  return (
    <div className="flex items-baseline gap-2 font-mono text-[11.5px]">
      <span
        className={cn(
          "shrink-0 rounded-full border px-1.5 py-px font-sans text-[10.5px] font-medium",
          TONE_BADGE[ACTION_TONE[item.action] ?? "dim"]
        )}
      >
        {item.action}
      </span>
      <span className="shrink-0 text-secondary">{item.name}</span>
      <span className="break-all text-faint">{item.path}</span>
      {item.message && <span className="break-all text-muted">· {item.message}</span>}
    </div>
  );
}

/** The Originals this project currently exposes, by resolved Original path. */
function linkedPaths(project: ChainProject | null): Set<string> {
  const paths = new Set<string>();
  if (!project) return paths;
  for (const entry of project.agents_dir?.entries ?? []) paths.add(entry.final_target);
  for (const surface of project.surfaces) {
    if (surface.kind !== "per_entry") continue;
    for (const entry of surface.entries) paths.add(entry.final_target);
  }
  return paths;
}

/**
 * The Agent surfaces the topology actually observes for this project — the
 * baseline an edit is a diff against. Only agents the picker can target are
 * kept: a surface outside CHAIN_AGENTS must not ride invisibly into a plan.
 *
 * This is deliberately not defaulted. A project with a populated
 * `.agents/skills` but no surfaces has an EMPTY baseline, so enabling an agent
 * counts as the change it is; folding a default in here would mark those
 * agents as already present and make that edit impossible to express.
 */
function observedAgents(project: ChainProject | null): Set<string> {
  const known = new Set<string>(CHAIN_AGENTS);
  const agents = new Set<string>();
  for (const surface of project?.surfaces ?? []) {
    if (surface.kind !== "absent" && known.has(surface.agent)) agents.add(surface.agent);
  }
  return agents;
}

/** What the editor opens with: the observed surfaces, or — when there are none
 * — a suggestion the user can accept or change. A suggestion is not a baseline. */
function initialAgents(project: ChainProject | null): Set<string> {
  const observed = observedAgents(project);
  return observed.size > 0 ? observed : new Set(["claude", "codex"]);
}

interface Preview {
  link: ChainLinkPlan | null;
  unlink: ChainUnlinkPlan[];
}

/**
 * Edit one project's Skill whitelist.
 *
 * It replaced an add-only dialog whose checkboxes were always empty even for a
 * project with 36 Skills already linked — so the whitelist could be added to
 * here but only removed from a table elsewhere. The picker now opens on the
 * project's current selection and the confirm step previews the difference in
 * both directions before anything is written.
 */
export function LinkSkillsDialog({ open, project, repos, presets, onClose, onLinked }: Props) {
  const { t } = useTranslation();
  const linked = useMemo(() => linkedPaths(project), [project]);
  // Seeded from the project's current whitelist. The caller keys this dialog by
  // project path, so opening it on another project mounts a fresh editor rather
  // than syncing state in an effect — and a background rescan can never wipe
  // edits in progress.
  const [selected, setSelected] = useState<Set<string>>(linked);
  const [agents, setAgents] = useState<Set<string>>(() => initialAgents(project));
  const [loading, setLoading] = useState(false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [applied, setApplied] = useState<{
    verified: boolean;
    items: ItemLike[];
    /** True when the link half failed and the removals were left unapplied. */
    removalsSkipped: boolean;
  } | null>(null);

  const added = useMemo(
    () => [...selected].filter((path) => !linked.has(path)),
    [selected, linked],
  );
  // Agents are part of the whitelist too: enabling a surface the project does
  // not have yet is a change even with the Skill set untouched. Disabling a
  // baseline agent is not — linking only ever creates, so an unchecked pill
  // just scopes which surfaces receive NEW Skills, exactly as before.
  const newAgents = useMemo(() => {
    const observed = observedAgents(project);
    return [...agents].filter((agent) => !observed.has(agent));
  }, [agents, project]);
  // Removals are addressed by Skill NAME, because that is what unlink takes —
  // and a name is not always one Original. When a project exposes two
  // same-named Originals (an aggregate `foo` from one repo, a direct `foo`
  // from another), unlinking by that name would take both, including the one
  // still checked. That edit cannot be expressed through the name-addressed
  // API, so it is refused here rather than silently over-removing: this path
  // has no link half, so the apply-time abort guard would never see it.
  const { removed, ambiguous } = useMemo(() => {
    const byPath = new Map<string, string>();
    for (const repo of repos) {
      for (const skill of repo.skills) byPath.set(skill.path, skill.name);
    }
    const keptNames = new Set(
      [...selected]
        .map((path) => byPath.get(path))
        .filter((name): name is string => Boolean(name)),
    );
    const removed = new Set<string>();
    const ambiguous = new Set<string>();
    for (const path of linked) {
      if (selected.has(path)) continue;
      const name = byPath.get(path);
      if (!name) continue;
      if (keptNames.has(name)) ambiguous.add(name);
      else removed.add(name);
    }
    return { removed: [...removed], ambiguous: [...ambiguous] };
  }, [selected, linked, repos]);

  const changeCount = added.length + removed.length + newAgents.length;

  if (!open || !project) return null;

  const handleClose = () => {
    setPreview(null);
    setApplied(null);
    onClose();
  };

  const toggleAgent = (agent: string) => {
    const next = new Set(agents);
    if (next.has(agent)) next.delete(agent);
    else next.add(agent);
    setAgents(next);
  };

  // Step 1 → 2: a read-only preview of the whole change set, both directions.
  const buildPreview = async () => {
    if (changeCount === 0 || ambiguous.length > 0) return;
    setLoading(true);
    try {
      // A new agent surface needs the WHOLE selection, not just the additions —
      // otherwise a pre-existing per-entry surface would receive only the new
      // Skills. Without new agents, planning just the additions keeps the
      // preview free of dozens of "exists" rows.
      const linkPaths = newAgents.length > 0 ? [...selected] : added;
      const link =
        linkPaths.length > 0 ? await chainPlanLink(project.path, linkPaths, [...agents]) : null;
      const unlink: ChainUnlinkPlan[] = [];
      for (const name of removed) {
        unlink.push(await chainPlanUnlink(project.path, name, []));
      }
      setPreview({ link, unlink });
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  // Step 2 → 3: apply exactly what was previewed. Links go first, and the
  // removals only run once the link half verified: a same-named swap whose
  // link conflicts must NOT proceed to unlink the Original the project still
  // depends on — that would leave it with neither.
  const apply = async () => {
    if (!preview) return;
    setLoading(true);
    try {
      const items: ItemLike[] = [];
      let verified = true;
      if (preview.link) {
        const outcome = await chainApplyLink(preview.link);
        verified = verified && outcome.verified;
        items.push(...outcome.report.skills, ...outcome.report.entries);
        const conflicted = [...outcome.report.skills, ...outcome.report.entries].some(
          (item) => item.action === "conflict" || item.action === "error",
        );
        if ((!outcome.verified || conflicted) && preview.unlink.length > 0) {
          setApplied({ verified: false, items, removalsSkipped: true });
          toast.warning(t("chain.removalsSkipped"));
          onLinked();
          return;
        }
      }
      for (const plan of preview.unlink) {
        const outcome = await chainApplyUnlink(plan);
        verified = verified && outcome.verified;
        items.push(...outcome.report);
      }
      setApplied({ verified, items, removalsSkipped: false });
      if (verified) toast.success(t("chain.applyVerified", { count: items.length }));
      else toast.warning(t("chain.applyUnverified"));
      onLinked();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={handleClose} />
      <div className="relative flex max-h-[84vh] w-full max-w-3xl flex-col rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-primary">
            <Link2 className="h-4 w-4 text-accent" />
            {t("chain.linkDialogTitle", { project: project.name })}
          </h2>
          <button
            onClick={handleClose}
            className="rounded p-1 text-muted transition-colors outline-none hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Step 1: edit the whitelist */}
        {!preview && !applied && (
          <>
            <div className="mb-3 flex flex-wrap items-center gap-2">
              <span className="text-[12px] text-muted">{t("chain.agentsLabel")}</span>
              {CHAIN_AGENTS.map((agent) => (
                <button
                  key={agent}
                  onClick={() => toggleAgent(agent)}
                  className={cn(
                    "rounded-full border px-2.5 py-1 text-[12px] font-medium transition-colors outline-none",
                    agents.has(agent)
                      ? "border-accent-border bg-surface-active text-secondary"
                      : "border-border-subtle text-muted hover:text-tertiary"
                  )}
                >
                  {agent}
                </button>
              ))}
              <span data-testid="link-diff" className="ml-auto text-[12px] text-muted">
                {t("chain.changeSummary", { added: added.length, removed: removed.length })}
                {newAgents.length > 0 &&
                  ` · ${t("chain.agentAddition", { agents: newAgents.join(", ") })}`}
              </span>
            </div>

            <SkillPicker
              repos={repos}
              selected={selected}
              onChange={setSelected}
              linked={linked}
              presets={presets}
            />

            {ambiguous.length > 0 && (
              <p
                data-testid="ambiguous-removal"
                className="mt-3 text-[12px] text-amber-400"
              >
                {t("chain.ambiguousRemoval", { names: ambiguous.join(", ") })}
              </p>
            )}

            <div className="mt-4 flex justify-end gap-2">
              <button onClick={handleClose} className="app-button-secondary">
                {t("common.cancel")}
              </button>
              <button
                data-testid="link-preview"
                onClick={() => void buildPreview()}
                disabled={
                  loading || changeCount === 0 || agents.size === 0 || ambiguous.length > 0
                }
                className="app-button-primary"
              >
                {loading ? t("chain.planning") : t("chain.previewPlan")}
              </button>
            </div>
          </>
        )}

        {/* Step 2: preview every write before making it */}
        {preview && !applied && (
          <>
            <div className="mb-2">
              <div className="app-section-title">{t("chain.planTitle")}</div>
              <p className="mt-0.5 text-[12px] text-muted">{t("chain.planHint")}</p>
            </div>
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto rounded-lg border border-border-subtle p-3">
              {preview.link && (
                <>
                  <div>
                    <div className="app-section-title mb-1.5">{t("chain.resultSkills")}</div>
                    <div className="space-y-1">
                      {preview.link.skills.map((item) => (
                        <ItemRow key={item.path} item={item} />
                      ))}
                    </div>
                  </div>
                  <div>
                    <div className="app-section-title mb-1.5">{t("chain.resultEntries")}</div>
                    <div className="space-y-1">
                      {preview.link.entries.map((item) => (
                        <ItemRow key={item.path} item={item} />
                      ))}
                    </div>
                  </div>
                </>
              )}
              {preview.unlink.length > 0 && (
                <div data-testid="preview-unlink">
                  <div className="app-section-title mb-1.5">{t("chain.resultRemoved")}</div>
                  {preview.unlink.some((plan) => plan.shared_surface) && (
                    <p className="mb-1 text-[11.5px] text-amber-400">
                      {t("chain.unlinkSharedNotice")}
                    </p>
                  )}
                  <div className="space-y-1">
                    {preview.unlink.flatMap((plan) =>
                      plan.items.map((item) => (
                        <ItemRow key={`${plan.skill}:${item.path}`} item={item} />
                      )),
                    )}
                  </div>
                </div>
              )}
            </div>
            <div className="mt-4 flex justify-between gap-2">
              <button onClick={() => setPreview(null)} className="app-button-secondary">
                <ArrowLeft className="h-4 w-4" />
                {t("chain.back")}
              </button>
              <button
                data-testid="link-apply"
                onClick={() => void apply()}
                disabled={loading}
                className="app-button-primary"
              >
                {loading ? t("chain.applying") : t("chain.apply")}
              </button>
            </div>
          </>
        )}

        {/* Step 3: what actually happened, per the rescan verdict */}
        {applied && (
          <>
            <div
              className={cn(
                "mb-3 flex items-center gap-2 rounded-lg border px-3 py-2 text-[12px]",
                applied.verified
                  ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-400"
                  : "border-amber-500/25 bg-amber-500/10 text-amber-400"
              )}
            >
              {applied.verified ? (
                <ShieldCheck className="h-4 w-4" />
              ) : (
                <ShieldAlert className="h-4 w-4" />
              )}
              <span>
                {applied.verified
                  ? t("chain.applyVerified", { count: applied.items.length })
                  : t("chain.applyUnverified")}
              </span>
            </div>
            {applied.removalsSkipped && (
              <p
                data-testid="removals-skipped"
                className="mb-3 text-[12px] text-amber-400"
              >
                {t("chain.removalsSkipped")}
              </p>
            )}
            <div className="min-h-0 flex-1 space-y-1 overflow-y-auto">
              {applied.items.map((item, index) => (
                <ItemRow key={`${item.path}:${index}`} item={item} />
              ))}
            </div>
            <div className="mt-4 flex justify-end">
              <button onClick={handleClose} className="app-button-secondary">
                {t("chain.close")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
