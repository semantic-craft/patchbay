import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { RefreshCw, ShieldCheck, ShieldAlert, FileText } from "lucide-react";
import { cn } from "../utils";
import { getChainTopology, instructionsScan } from "../lib/tauri";
import type {
  ChainGuardViolation,
  ChainTopology,
  ChainTracedEntry,
  InstructionsScanReport,
} from "../lib/tauri";
import { RemediateDialog } from "../components/RemediateDialog";
import { ChainScanStatus } from "../components/ChainScanStatus";
import {
  STATUS_TONE,
  TONE_BADGE,
  TONE_DOT,
  TONE_STROKE,
  shortenPath,
  type ChainTone,
} from "../lib/chainUi";
import { agentGlobalTokens, formatTokens } from "../lib/instructionsUi";

interface Node {
  id: string;
  col: 1 | 2 | 3;
  title: string;
  sub: string;
  tone: ChainTone;
  badges: { text: string; tone: ChainTone }[];
  entries: ChainTracedEntry[];
}

interface Edge {
  from: string;
  to: string;
  tone: ChainTone;
  label?: string;
}

interface WirePath {
  key: string;
  d: string;
  tone: ChainTone;
  from: string;
  to: string;
  label?: string;
  lx: number;
  ly: number;
}

function countByStatus(entries: ChainTracedEntry[]): Map<string, number> {
  const m = new Map<string, number>();
  for (const e of entries) m.set(e.status, (m.get(e.status) ?? 0) + 1);
  return m;
}

export function ChainOverview() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [topo, setTopo] = useState<ChainTopology | null>(null);
  const [instr, setInstr] = useState<InstructionsScanReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [wires, setWires] = useState<WirePath[]>([]);
  const [remediate, setRemediate] = useState<{ violation: ChainGuardViolation; agent: string } | null>(
    null
  );

  const containerRef = useRef<HTMLDivElement | null>(null);
  const nodeRefs = useRef(new Map<string, HTMLDivElement>());

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setTopo(await getChainTopology());
      // Best-effort: a failed instructions scan must not blank the topology
      // graph, which is this page's primary content.
      setInstr(await instructionsScan().catch(() => null));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const statusLabel = useCallback(
    (status: string, count: number) => `${t(`chain.status.${status}`)} ×${count}`,
    [t]
  );

  const { nodes, edges } = useMemo(() => {
    const nodes: Node[] = [];
    const edges: Edge[] = [];
    if (!topo) return { nodes, edges };

    // Worth attributing each repo to its source root only when there is more
    // than one configured root.
    const multiRoot = topo.warehouse_roots.length > 1;

    for (const repo of topo.repos) {
      const badges: Node["badges"] = [];
      if (multiRoot) {
        badges.push({ text: repo.root.split("/").pop() || repo.root, tone: "dim" });
      }
      if (repo.health.dirty) badges.push({ text: t("chain.dirty"), tone: "warn" });
      badges.push(
        repo.referenced_by.length > 0
          ? { text: t("chain.referencedBy", { count: repo.referenced_by.length }), tone: "ok" }
          : { text: t("chain.noRefs"), tone: "dim" }
      );
      nodes.push({
        id: `repo:${repo.name}`,
        col: 1,
        title: repo.name,
        sub: t("chain.skillsCount", { count: repo.skills.length }),
        tone: repo.health.dirty ? "warn" : repo.referenced_by.length > 0 ? "ok" : "dim",
        badges,
        entries: [],
      });
    }

    for (const project of topo.projects) {
      if (project.agents_dir) {
        const counts = countByStatus(project.agents_dir.entries);
        const badges: Node["badges"] = [...counts.entries()].map(([status, count]) => ({
          text: statusLabel(status, count),
          tone: STATUS_TONE[status as keyof typeof STATUS_TONE] ?? "dim",
        }));
        const tone: ChainTone = counts.has("broken") ? "err" : counts.has("link_repo") ? "ok" : "dim";
        nodes.push({
          id: `agg:${project.name}`,
          col: 2,
          title: project.name,
          sub: t("chain.entriesCount", { count: project.agents_dir.entries.length }),
          tone,
          badges,
          entries: project.agents_dir.entries,
        });
        const byRepo = new Map<string, number>();
        for (const e of project.agents_dir.entries) {
          if (e.status === "link_repo" && e.repo) byRepo.set(e.repo, (byRepo.get(e.repo) ?? 0) + 1);
        }
        for (const [repo, count] of byRepo) {
          edges.push({ from: `repo:${repo}`, to: `agg:${project.name}`, tone: "ok", label: `×${count}` });
        }
      }

      for (const surface of project.surfaces) {
        if (surface.kind === "absent") continue;
        const id = `surf:${project.name}:${surface.agent}`;
        if (surface.kind === "dir_link") {
          const tone: ChainTone = surface.dir_link_ok ? "ok" : "err";
          nodes.push({
            id,
            col: 3,
            title: `${project.name} · ${surface.agent}`,
            sub: surface.dir_link_target
              ? `→ ${shortenPath(surface.dir_link_target, topo.warehouse_roots.map((r) => r.root), topo.projects_root)}`
              : "",
            tone,
            badges: [{ text: t(surface.dir_link_ok ? "chain.dirLinkOk" : "chain.dirLinkBad"), tone }],
            entries: [],
          });
          if (project.agents_dir) {
            edges.push({ from: `agg:${project.name}`, to: id, tone, label: t("chain.dirLinkOk") });
          }
          continue;
        }
        if (surface.entries.length === 0) continue;
        const counts = countByStatus(surface.entries);
        const badges: Node["badges"] = [...counts.entries()].map(([status, count]) => ({
          text: statusLabel(status, count),
          tone: STATUS_TONE[status as keyof typeof STATUS_TONE] ?? "dim",
        }));
        const tone: ChainTone = counts.has("broken")
          ? "err"
          : counts.has("direct") || counts.has("copy")
            ? "warn"
            : "ok";
        nodes.push({
          id,
          col: 3,
          title: `${project.name} · ${surface.agent}`,
          sub: t("chain.entriesCount", { count: surface.entries.length }),
          tone,
          badges,
          entries: surface.entries,
        });
        const directByRepo = new Map<string, number>();
        let viaAgents = 0;
        for (const e of surface.entries) {
          if (e.status === "direct" && e.repo)
            directByRepo.set(e.repo, (directByRepo.get(e.repo) ?? 0) + 1);
          if (e.status === "via_agents") viaAgents += 1;
        }
        for (const [repo, count] of directByRepo) {
          edges.push({ from: `repo:${repo}`, to: id, tone: "warn", label: `×${count}` });
        }
        if (viaAgents > 0 && project.agents_dir) {
          edges.push({ from: `agg:${project.name}`, to: id, tone: "ok", label: `×${viaAgents}` });
        }
      }
    }

    const ids = new Set(nodes.map((n) => n.id));
    return { nodes, edges: edges.filter((e) => ids.has(e.from) && ids.has(e.to)) };
  }, [topo, t, statusLabel]);

  const connected = useMemo(() => {
    if (!selected) return null;
    const seen = new Set([selected]);
    let grew = true;
    while (grew) {
      grew = false;
      for (const e of edges) {
        if (seen.has(e.from) && !seen.has(e.to)) {
          seen.add(e.to);
          grew = true;
        }
        if (seen.has(e.to) && !seen.has(e.from)) {
          seen.add(e.from);
          grew = true;
        }
      }
    }
    return seen;
  }, [selected, edges]);

  const drawWires = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const crect = container.getBoundingClientRect();
    const next: WirePath[] = [];
    edges.forEach((edge, i) => {
      const a = nodeRefs.current.get(edge.from);
      const b = nodeRefs.current.get(edge.to);
      if (!a || !b) return;
      const ra = a.getBoundingClientRect();
      const rb = b.getBoundingClientRect();
      const x1 = ra.right - crect.left + container.scrollLeft;
      const y1 = ra.top + ra.height / 2 - crect.top + container.scrollTop;
      const x2 = rb.left - crect.left + container.scrollLeft;
      const y2 = rb.top + rb.height / 2 - crect.top + container.scrollTop;
      const mx = (x1 + x2) / 2;
      next.push({
        key: `${edge.from}->${edge.to}:${i}`,
        d: `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`,
        tone: edge.tone,
        from: edge.from,
        to: edge.to,
        label: edge.label,
        lx: mx,
        ly: (y1 + y2) / 2 - 5,
      });
    });
    setWires(next);
  }, [edges]);

  useLayoutEffect(() => {
    drawWires();
    window.addEventListener("resize", drawWires);
    return () => window.removeEventListener("resize", drawWires);
  }, [drawWires]);

  const selectedNode = selected ? nodes.find((n) => n.id === selected) : null;

  const guardAllOk = topo?.guard.every((g) => g.state !== "violation") ?? true;
  const GuardIcon = guardAllOk ? ShieldCheck : ShieldAlert;

  const renderColumn = (col: 1 | 2 | 3, heading: string) => (
    <div className="min-w-[280px] flex-1">
      <div className="app-section-title mb-3">{heading}</div>
      <div className="space-y-2">
        {nodes
          .filter((n) => n.col === col)
          .map((node) => {
            const dimmed = connected ? !connected.has(node.id) : false;
            const hot = connected?.has(node.id) ?? false;
            return (
              <div
                key={node.id}
                ref={(el) => {
                  if (el) nodeRefs.current.set(node.id, el);
                  else nodeRefs.current.delete(node.id);
                }}
                onClick={() => setSelected(selected === node.id ? null : node.id)}
                className={cn(
                  "relative z-10 cursor-pointer rounded-lg border bg-surface px-3 py-2 transition-all",
                  hot ? "border-accent-border shadow-sm" : "border-border-subtle hover:border-border",
                  dimmed && "opacity-30"
                )}
              >
                <div className="flex items-center gap-2">
                  <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", TONE_DOT[node.tone])} />
                  <span className="truncate text-[13px] font-medium text-secondary">{node.title}</span>
                </div>
                <div className="mt-0.5 truncate font-mono text-[11px] text-muted">{node.sub}</div>
                {node.badges.length > 0 && (
                  <div className="mt-1.5 flex flex-wrap gap-1">
                    {node.badges.map((badge, i) => (
                      <span
                        key={i}
                        className={cn(
                          "rounded-full border px-1.5 py-px text-[10.5px] font-medium",
                          TONE_BADGE[badge.tone]
                        )}
                      >
                        {badge.text}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
      </div>
    </div>
  );

  return (
    <div className="app-page">
      <div className="app-page-header app-toolbar">
        <div>
          <h1 className="app-page-title">{t("chain.overviewTitle")}</h1>
          <p className="app-page-subtitle">{t("chain.overviewSubtitle")}</p>
          <ChainScanStatus scannedAt={topo?.scanned_at} loading={loading} />
        </div>
        <button className="app-button-secondary" onClick={() => void load()} disabled={loading}>
          <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
          {t("chain.rescan")}
        </button>
      </div>

      {topo && (
        <div
          className={cn(
            "flex flex-wrap items-center gap-x-4 gap-y-1 rounded-xl border px-4 py-2.5 text-[12.5px]",
            guardAllOk
              ? "border-emerald-500/25 bg-emerald-500/[0.06] text-emerald-400"
              : "border-red-500/30 bg-red-500/[0.07] text-red-400"
          )}
        >
          <span className="flex items-center gap-1.5 font-semibold">
            <GuardIcon className="h-4 w-4" />
            {t(guardAllOk ? "chain.guardOk" : "chain.guardBad")}
          </span>
          {topo.guard
            .filter((g) => g.state !== "absent")
            .map((g) => (
              <span key={g.path} title={g.path} className="font-mono text-[11.5px] text-muted">
                <span className="text-tertiary">{g.agent}</span>{" "}
                ~/{g.path.split("/").slice(-2).join("/")}{" "}
                {g.state === "violation" ? (
                  <span className="inline-flex flex-wrap gap-1">
                    {g.violations.map((v) => (
                      <button
                        key={v.path}
                        onClick={() => setRemediate({ violation: v, agent: g.agent })}
                        title={`${v.final_target}${v.is_link ? " (symlink)" : ""} — ${t("chain.remediate.action")}`}
                        className="rounded border border-red-500/40 px-1.5 py-px font-semibold text-red-400 outline-none transition-colors hover:bg-red-500/10"
                      >
                        {v.skill}
                      </button>
                    ))}
                  </span>
                ) : (
                  <span className="text-tertiary">{t(`chain.${g.state}`)}</span>
                )}
              </span>
            ))}
        </div>
      )}

      {topo && instr && (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 rounded-xl border border-border-subtle bg-surface-hover px-4 py-2.5 text-[12.5px]">
          <span className="flex items-center gap-1.5 font-semibold text-secondary">
            <FileText className="h-4 w-4" />
            {t("instructions.costBarTitle")}
          </span>
          {instr.agents.length === 0 || instr.globals.length === 0 ? (
            <span className="text-muted">{t("instructions.costBarEmpty")}</span>
          ) : (
            instr.agents.map((agent) => {
              const tokens = agentGlobalTokens(instr.globals, agent);
              return (
                <button
                  key={agent}
                  onClick={() => navigate("/chain/projects")}
                  title={t("instructions.costBarHint")}
                  className={cn(
                    "font-mono text-[11.5px] outline-none transition-colors hover:text-secondary",
                    tokens > 0 ? "text-muted" : "text-faint"
                  )}
                >
                  <span className="text-tertiary">{agent}</span> {formatTokens(tokens)}
                </button>
              );
            })
          )}
        </div>
      )}

      {topo &&
        (topo.warehouse_roots.length > 1 ||
          topo.warehouse_roots.some((r) => r.status !== "ok")) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-xl border border-border-subtle bg-surface-hover px-4 py-2.5 text-[12.5px]">
            <span className="font-semibold text-secondary">{t("chain.rootsTitle")}</span>
            {topo.warehouse_roots.map((r) => {
              const bad = r.status !== "ok";
              return (
                <span
                  key={r.root}
                  title={r.error ?? undefined}
                  className={cn(
                    "flex items-center gap-1.5 font-mono text-[11.5px]",
                    bad ? "text-red-400" : "text-muted"
                  )}
                >
                  {shortenPath(r.root, [], topo.projects_root)}
                  <span
                    className={cn(
                      "font-sans",
                      bad ? "font-semibold text-red-400" : "text-tertiary"
                    )}
                  >
                    {r.status === "ok"
                      ? t("chain.reposCount", { count: r.repo_count })
                      : t(r.status === "missing" ? "chain.rootMissing" : "chain.rootUnreadable")}
                  </span>
                </span>
              );
            })}
          </div>
        )}

      {error && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("chain.scanFailed")}: {error}
        </div>
      )}
      {loading && !topo && <div className="p-4 text-[13px] text-muted">{t("chain.scanning")}</div>}

      {topo && (
        <>
          <div ref={containerRef} className="relative overflow-x-auto">
            <svg className="pointer-events-none absolute inset-0 z-0 h-full w-full">
              {wires.map((w) => {
                const hot = connected ? connected.has(w.from) && connected.has(w.to) : false;
                const cold = connected ? !hot : false;
                return (
                  <g key={w.key}>
                    <path
                      d={w.d}
                      fill="none"
                      stroke={TONE_STROKE[w.tone]}
                      strokeWidth={hot ? 2.5 : 1.5}
                      opacity={cold ? 0.08 : hot ? 1 : 0.45}
                    />
                    {w.label && !cold && (
                      <text x={w.lx} y={w.ly} textAnchor="middle" className="fill-current font-mono text-[10px] text-muted">
                        {w.label}
                      </text>
                    )}
                  </g>
                );
              })}
            </svg>
            <div className="flex gap-14 pb-2">
              {renderColumn(1, t("chain.tierRepos"))}
              {renderColumn(2, t("chain.tierAgg"))}
              {renderColumn(3, t("chain.tierSurfaces"))}
            </div>
          </div>

          <div className="app-panel-muted p-4">
            <div className="app-section-title mb-2">{selectedNode?.title ?? t("chain.detailHint")}</div>
            {selectedNode && selectedNode.entries.length > 0 && (
              <div className="space-y-1 font-mono text-[11.5px]">
                {selectedNode.entries.slice(0, 10).map((e) => (
                  <div key={e.entry_path} className="flex flex-wrap items-baseline gap-2">
                    <span className="text-secondary">{e.name}</span>
                    <span className="text-faint">→</span>
                    <span className="min-w-0 flex-1 break-all text-muted">
                      {shortenPath(e.final_target, topo.warehouse_roots.map((r) => r.root), topo.projects_root)}
                    </span>
                    <span
                      className={cn(
                        "rounded-full border px-1.5 py-px text-[10px] font-sans font-medium",
                        TONE_BADGE[STATUS_TONE[e.status]]
                      )}
                    >
                      {t(`chain.status.${e.status}`)}
                    </span>
                  </div>
                ))}
                {selectedNode.entries.length > 10 && (
                  <div className="text-muted">… +{selectedNode.entries.length - 10}</div>
                )}
              </div>
            )}
          </div>
        </>
      )}

      <RemediateDialog
        open={remediate !== null}
        violation={remediate?.violation ?? null}
        agent={remediate?.agent ?? ""}
        projects={topo?.projects ?? []}
        onClose={() => setRemediate(null)}
        onDone={() => void load()}
      />
    </div>
  );
}
