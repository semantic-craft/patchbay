import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";
import { open as dialogOpen } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import {
  chainApplyUnlink,
  chainDoctorReport,
  chainLocateCandidates,
  chainPlanUnlink,
  chainPresetsList,
  chainRegisterProject,
  chainRepairJournal,
  chainRepoMoves,
  getChainTopology,
  instructionsPlanInit,
  instructionsPlanNormalize,
  instructionsScan,
} from "../lib/tauri";
import type {
  ChainDoctorReport,
  ChainFinding,
  ChainJournalRecord,
  ChainPreset,
  ChainPresetSkill,
  ChainProject,
  ChainRepairCandidate,
  ChainRepoMove,
  ChainTopology,
  ChainUnlinkPlan,
  InstructionsInitPlan,
  InstructionsNormalizePlan,
  InstructionsScanReport,
} from "../lib/tauri";
import { projectFindings, SEVERITY_RANK, workbenchState } from "../lib/workbenchState";
import { publishDoctorReport } from "../lib/doctorStore";
import { LinkSkillsDialog } from "../components/LinkSkillsDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { InstructionsWriteDialog } from "../components/InstructionsWriteDialog";
import { WorkbenchHeader } from "../components/workbench/WorkbenchHeader";
import { ProjectSwitcher } from "../components/workbench/ProjectSwitcher";
import { SurfacesCard } from "../components/workbench/SurfacesCard";
import { StatusCard } from "../components/workbench/StatusCard";
import { EvidenceCard } from "../components/workbench/EvidenceCard";
import { RepairRecordCard } from "../components/workbench/RepairRecordCard";
import { RepoMoveCard } from "../components/workbench/RepoMoveCard";
import { DirtyRepoCard } from "../components/workbench/DirtyRepoCard";
import { ChainPresetBar } from "../components/workbench/ChainPresetBar";
import { OnboardingWizard } from "../components/workbench/OnboardingWizard";
import { CollapsedLinks } from "../components/workbench/CollapsedLinks";
import { InstructionsPanel } from "../components/workbench/InstructionsPanel";
import { LinkTable, type LinkRow } from "../components/workbench/LinkTable";

type InstructionsWriteTarget =
  | { operation: "normalize"; name: string; path: string; plan: InstructionsNormalizePlan }
  | { operation: "init"; name: string; path: string; plan: InstructionsInitPlan };

function projectRows(project: ChainProject): LinkRow[] {
  const rows: LinkRow[] = [];
  for (const surface of project.surfaces) {
    if (surface.kind !== "per_entry") continue;
    for (const entry of surface.entries) rows.push({ location: surface.agent, entry });
  }
  if (project.agents_dir) {
    for (const entry of project.agents_dir.entries) rows.push({ location: ".agents", entry });
  }
  return rows;
}

/**
 * 项目工作台（应用主屏）。组合可替换的区块：头部 / 项目切换 /
 * 状态区（入口卡 + 指令面板）/ 链接列表；本视图只保留数据加载与动线编排。
 */
export function ChainProjects() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [topo, setTopo] = useState<ChainTopology | null>(null);
  const [instr, setInstr] = useState<InstructionsScanReport | null>(null);
  const [doctor, setDoctor] = useState<ChainDoctorReport | null>(null);
  // Fingerprint → located candidates for the report's broken findings (#30).
  const [candidates, setCandidates] = useState<Record<string, ChainRepairCandidate[]>>({});
  // Repair journal, newest first — the 修复记录 cards' data source (#31).
  const [journal, setJournal] = useState<ChainJournalRecord[]>([]);
  // Detected repo-move storms (#33) and the groups the user itemized back
  // into individual cards (keyed by old_root→new_root).
  const [repoMoves, setRepoMoves] = useState<ChainRepoMove[]>([]);
  // Chain assembly presets (#35) — the green-state Preset bar's contents.
  const [presets, setPresets] = useState<ChainPreset[]>([]);
  const [itemized, setItemized] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(() => searchParams.get("project"));
  const [linkTarget, setLinkTarget] = useState<{ name: string; path: string } | null>(null);
  const [unlinkPlan, setUnlinkPlan] = useState<ChainUnlinkPlan | null>(null);
  const [planningInstructions, setPlanningInstructions] = useState<"normalize" | "init" | null>(null);
  const [instructionsTarget, setInstructionsTarget] = useState<InstructionsWriteTarget | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // Three independent server-side scans. They run together so the home
      // screen costs one scan's latency instead of three. Only the topology is
      // load-bearing: the instructions and Doctor reports share its registered
      // projects, and each failure is tolerated separately so neither can blank
      // the link view.
      const [topology, instructions, report, records, storms, presetList] = await Promise.all([
        getChainTopology(),
        instructionsScan().catch(() => null),
        chainDoctorReport().catch(() => null),
        chainRepairJournal().catch(() => null),
        chainRepoMoves().catch(() => null),
        chainPresetsList().catch(() => null),
      ]);
      setTopo(topology);
      setInstr(instructions);
      setDoctor(report);
      setJournal(records ?? []);
      setRepoMoves(storms?.groups ?? []);
      setItemized(new Set());
      setPresets(presetList ?? []);
      // The sidebar's health dots consume the same report (#30).
      publishDoctorReport(report);
      // Broken findings get their candidate evidence located in a second,
      // non-blocking pass — the cards render immediately and the candidate
      // row fills in when the lookup lands. Failures just leave it empty.
      const broken = (report?.findings ?? [])
        .filter((finding) => finding.deviation === "broken")
        .map((finding) => finding.fingerprint);
      setCandidates({});
      if (broken.length > 0) {
        chainLocateCandidates(broken)
          .then((located) => setCandidates(located.candidates))
          .catch(() => {});
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    setSelectedPath(searchParams.get("project"));
  }, [searchParams]);

  const selectProject = (path: string) => {
    setSelectedPath(path);
    setSearchParams({ project: path }, { replace: true });
  };

  const project = useMemo(() => {
    if (!topo || topo.projects.length === 0) return null;
    return topo.projects.find((p) => p.path === selectedPath) ?? topo.projects[0];
  }, [topo, selectedPath]);

  const rows = useMemo(() => (project ? projectRows(project) : []), [project]);

  // Which of the exception-driven states the main area renders: "green" is the
  // ✓ card (#29), "attention" is severity-ordered evidence cards (#30), and
  // "unknown" (Doctor unreachable) asserts no health at all — full list only.
  const state = workbenchState(doctor, project);

  // 新项目接入（#36，原型 S6）：0 链接的项目进入工作台自动呈现三步向导，
  // 顶替整个状态区。空与否由拓扑自证，doctor 不可达也不拦；唯有异常态
  // 优先——故障卡永远不被向导遮蔽。
  const onboarding = rows.length === 0 && state !== "attention";

  // The selected project's findings, worst first. Doctor's wire order is
  // explicitly not a contract, so the workbench sorts for itself (#30).
  const findings = useMemo(
    () =>
      [...projectFindings(doctor, project)].sort(
        (a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity],
      ),
    [doctor, project],
  );

  // Repo-move storms scoped to this project (#33): a group card claims the
  // member findings it covers here; itemized groups fall back to per-finding
  // cards. A scoped group needs ≥2 members to stay a storm.
  const storms = useMemo(() => {
    return repoMoves
      .map((group) => {
        const scoped = findings.filter((finding) =>
          group.fingerprints.includes(finding.fingerprint),
        );
        const skills = scoped
          .map(
            (finding) =>
              finding.affected.find((obj) => obj.kind === "skill")?.name ?? "",
          )
          .filter(Boolean)
          .sort();
        return {
          group,
          key: `${group.old_root}→${group.new_root}`,
          fingerprints: scoped.map((finding) => finding.fingerprint),
          skills,
        };
      })
      .filter((storm) => storm.fingerprints.length >= 2 && !itemized.has(storm.key));
  }, [repoMoves, findings, itemized]);

  const stormCovered = useMemo(
    () => new Set(storms.flatMap((storm) => storm.fingerprints)),
    [storms],
  );
  const soloFindings = useMemo(
    () => findings.filter((finding) => !stormCovered.has(finding.fingerprint)),
    [findings, stormCovered],
  );

  // Links no finding names — the「其余 N 条正常」count under the cards.
  const normalCount = useMemo(() => {
    const flagged = new Set(findings.map((finding) => finding.evidence.entry_path));
    return rows.filter((row) => !flagged.has(row.entry.entry_path)).length;
  }, [rows, findings]);

  // Preset 栏改动后只刷新套装列表——不值得为此重扫全拓扑。
  const refreshPresets = useCallback(async () => {
    try {
      setPresets(await chainPresetsList());
    } catch {
      // The stale list stays; the next full load refreshes it.
    }
  }, []);

  // 「把当前 N 个技能存为 Preset」的素材：当前项目链接的去重技能引用
  // （同一技能的聚合层与各 surface 行合并为一条，取解析到的原件路径）。
  const currentSkills = useMemo<ChainPresetSkill[]>(() => {
    const byName = new Map<string, ChainPresetSkill>();
    for (const row of rows) {
      if (!byName.has(row.entry.name)) {
        byName.set(row.entry.name, {
          name: row.entry.name,
          path: row.entry.final_target,
          repo: row.entry.repo ?? null,
        });
      }
    }
    return [...byName.values()];
  }, [rows]);

  // 反哺提示（#34）：本项目引用的 warehouse 仓库里工作区不干净的那些。
  // warning 级、不参与 workbenchState —— 绿态照常是绿，只是多一张暖色卡。
  const dirtyRepos = useMemo(
    () =>
      project && topo
        ? topo.repos.filter(
            (repo) =>
              repo.health.dirty &&
              repo.referenced_by.some((ref) => ref.path === project.path),
          )
        : [],
    [topo, project],
  );

  // The 修复记录 card (#31): the selected project's newest record that is
  // neither undone nor dismissed. journal is newest-first, so `find` is it.
  const record = useMemo(
    () =>
      project
        ? (journal.find(
            (candidate) =>
              candidate.status === "applied" &&
              !candidate.dismissed &&
              candidate.projects.includes(project.path),
          ) ?? null)
        : null,
    [journal, project],
  );

  // The selected project's instructions surface. Registered projects match by
  // path (both the topology and the scan enumerate the registry identically), so
  // a plain path lookup is exact.
  const instrProject = useMemo(
    () => (project && instr ? (instr.projects.find((p) => p.path === project.path) ?? null) : null),
    [project, instr]
  );

  const pickFolder = async () => {
    const picked = await dialogOpen({ directory: true, multiple: false });
    if (typeof picked !== "string") return;
    try {
      // Selecting a folder for ongoing management enrols it as a registered
      // project and persists it, so it joins the topology and survives rescans
      // and restarts. Then open the link dialog to manage its skills.
      const registered = await chainRegisterProject(picked);
      await load();
      selectProject(registered.path);
      setLinkTarget({ name: registered.name, path: registered.path });
    } catch (e) {
      toast.error(String(e));
    }
  };

  // A per-entry surface row carries its Agent key in `location`; the aggregate
  // row (".agents") is the shared surface, so unlinking it targets every Agent.
  const startUnlink = async (name: string, projectPath: string, location: string) => {
    const agents = location === ".agents" ? [] : [location];
    try {
      setUnlinkPlan(await chainPlanUnlink(projectPath, name, agents));
    } catch (e) {
      toast.error(String(e));
    }
  };

  // 手动处理 = 既有 unlink 动线。聚合层条目走 ".agents"（作用于全部 Agent），
  // 否则定位包含该入口的 per-entry surface，按其 Agent 走单面 unlink。
  const manualUnlink = (finding: ChainFinding) => {
    if (!project) return;
    const name = finding.affected.find((obj) => obj.kind === "skill")?.name;
    if (!name) return;
    const surface = project.surfaces.find(
      (s) =>
        s.kind === "per_entry" &&
        s.entries.some((entry) => entry.entry_path === finding.evidence.entry_path),
    );
    void startUnlink(name, project.path, surface ? surface.agent : ".agents");
  };

  const confirmUnlink = async () => {
    if (!unlinkPlan) return;
    const outcome = await chainApplyUnlink(unlinkPlan);
    const conflict = outcome.report.find((r) => r.action === "conflict" || r.action === "error");
    if (conflict) {
      toast.warning(`${conflict.name}: ${conflict.message ?? conflict.action}`);
    } else if (!outcome.verified) {
      toast.warning(t("chain.unlinkUnverified", { name: unlinkPlan.skill }));
    } else {
      toast.success(`${unlinkPlan.skill} ✓`);
    }
    await load();
  };

  const startNormalize = async () => {
    if (!project) return;
    setPlanningInstructions("normalize");
    try {
      const plan = await instructionsPlanNormalize(project.path);
      setInstructionsTarget({ operation: "normalize", name: project.name, path: project.path, plan });
    } catch (e) {
      toast.error(String(e));
    } finally {
      setPlanningInstructions(null);
    }
  };

  const startInit = async () => {
    if (!project) return;
    setPlanningInstructions("init");
    try {
      const plan = await instructionsPlanInit(project.path);
      setInstructionsTarget({ operation: "init", name: project.name, path: project.path, plan });
    } catch (e) {
      toast.error(String(e));
    } finally {
      setPlanningInstructions(null);
    }
  };

  // Built once so the collapsed and expanded states show the same table.
  const linkTable = topo && project && (
    <LinkTable
      rows={rows}
      topo={topo}
      onUnlink={(name, location) => void startUnlink(name, project.path, location)}
    />
  );

  return (
    <div className="app-page">
      <WorkbenchHeader
        loading={loading}
        scannedAt={topo?.scanned_at}
        hasProject={project !== null}
        onPickFolder={() => void pickFolder()}
        onRescan={() => void load()}
        onLink={() => project && setLinkTarget({ name: project.name, path: project.path })}
      />

      {error && (
        <div className="app-panel border-red-500/30 p-4 text-[13px] text-red-400">
          {t("chain.scanFailed")}: {error}
        </div>
      )}
      {loading && !topo && <div className="p-4 text-[13px] text-muted">{t("chain.scanning")}</div>}

      {topo && (
        <ProjectSwitcher projects={topo.projects} activePath={project?.path} onSelect={selectProject} />
      )}

      {topo && project && onboarding && (
        <OnboardingWizard
          projectPath={project.path}
          repos={topo.repos}
          presets={presets}
          onDone={() => void load()}
        />
      )}

      {topo && project && !onboarding && (
        <>
          {/* 状态区（#26 异常驱动三态）。头部这一槽位是「状态」本身：全绿时是
              ✓ 状态卡，故障态是按 severity 排序的证据卡（#30）。修完留痕时
              （#31，原型 S4）修复记录卡顶替 ✓ 卡；折叠行兼任全绿指示。 */}
          {/* Preset 栏（#35）：全绿态的沉淀入口；套装应用归 #36 向导。 */}
          {state === "green" && (
            <ChainPresetBar
              presets={presets}
              currentSkills={currentSkills}
              onChanged={() => void refreshPresets()}
            />
          )}

          {state === "green" && doctor && !record && (
            <StatusCard count={rows.length} scannedAt={doctor.scanned_at} />
          )}
          {state === "attention" && (
            <div data-testid="workbench-attention" className="space-y-2.5">
              {storms.map((storm) => (
                <RepoMoveCard
                  key={storm.key}
                  group={storm.group}
                  fingerprints={storm.fingerprints}
                  skills={storm.skills}
                  onViewDiagnosis={() => navigate("/chain/doctor")}
                  onItemize={() =>
                    setItemized((cur) => new Set([...cur, storm.key]))
                  }
                  onRepaired={() => void load()}
                />
              ))}
              {soloFindings.map((finding) => (
                <EvidenceCard
                  key={finding.fingerprint}
                  finding={finding}
                  candidates={candidates[finding.fingerprint] ?? []}
                  onViewDiagnosis={() => navigate("/chain/doctor")}
                  onManual={
                    finding.deviation === "broken" ? () => manualUnlink(finding) : null
                  }
                  onRepaired={() => void load()}
                />
              ))}
            </div>
          )}
          {record && (
            <RepairRecordCard
              record={record}
              onUndone={() => void load()}
              onDismissed={() => void load()}
            />
          )}

          {/* 反哺提示卡（#34）：排在故障卡/记录卡之后——提示不遮蔽异常。 */}
          {dirtyRepos.map((repo) => (
            <DirtyRepoCard key={repo.path} repo={repo} />
          ))}

          <SurfacesCard project={project} />

          {instrProject && (
            <InstructionsPanel
              instrProject={instrProject}
              planning={planningInstructions}
              onNormalize={() => void startNormalize()}
              onInit={() => void startInit()}
            />
          )}

          {/* 没有需要处理的事就不摆列表；一行折叠交代条数，展开即完整列表。
              故障态同理——卡片之外的正常项折叠为「其余 N 条正常」（#30）；
              只有 unknown（Doctor 取不到）退回完整列表、不做健康断言。 */}
          {state === "green" ? (
            <CollapsedLinks count={rows.length}>{linkTable}</CollapsedLinks>
          ) : state === "attention" ? (
            <CollapsedLinks count={normalCount} labelKey="chain.workbench.otherNormal">
              {linkTable}
            </CollapsedLinks>
          ) : (
            linkTable
          )}
        </>
      )}

      <LinkSkillsDialog
        open={linkTarget !== null}
        projectName={linkTarget?.name ?? ""}
        projectPath={linkTarget?.path ?? ""}
        repos={topo?.repos ?? []}
        presets={presets}
        onClose={() => setLinkTarget(null)}
        onLinked={() => void load()}
      />
      <ConfirmDialog
        open={unlinkPlan !== null}
        title={t("chain.unlinkConfirmTitle")}
        message={
          unlinkPlan?.shared_surface
            ? t("chain.unlinkSharedWarning", { name: unlinkPlan?.skill ?? "" })
            : t("chain.unlinkConfirm", { name: unlinkPlan?.skill ?? "" })
        }
        details={unlinkPlan?.affected_agents ?? []}
        confirmLabel={t("chain.unlink")}
        tone={unlinkPlan?.shared_surface ? "warning" : "danger"}
        onClose={() => setUnlinkPlan(null)}
        onConfirm={confirmUnlink}
      />
      <InstructionsWriteDialog
        open={instructionsTarget !== null}
        operation={instructionsTarget?.operation ?? "normalize"}
        projectName={instructionsTarget?.name ?? ""}
        projectPath={instructionsTarget?.path ?? ""}
        plan={instructionsTarget?.plan ?? null}
        onClose={() => setInstructionsTarget(null)}
        onApplied={load}
      />
    </div>
  );
}
