/* eslint-disable react-refresh/only-export-components */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  chainDoctorReport,
  getChainDuplicates,
  chainLocateCandidates,
  chainPresetsList,
  chainRepairJournal,
  chainRepoMoves,
  getChainTopology,
  instructionsScan,
} from "../lib/tauri";
import type {
  ChainDoctorReport,
  ChainDuplicatesReport,
  ChainJournalRecord,
  ChainPreset,
  ChainRepairCandidate,
  ChainRepoMove,
  ChainTopology,
  InstructionsScanReport,
} from "../lib/tauri";

/**
 * The single chain scan the whole app reads.
 *
 * Every work area used to run its own scan on mount, so switching areas
 * re-walked the filesystem and each area carried its own `scanned_at`, its own
 * "stale" badge and its own rescan button — four answers to one question. The
 * scan is a property of the machine, not of a screen, so it lives here: one
 * load, one freshness clock, one `reload`.
 *
 * The instructions Doctor report is deliberately NOT here. It is the only
 * multi-megabyte payload and only the Diagnostics area consumes it, so that
 * area fetches it itself rather than making every launch pay for it.
 */
interface ChainState {
  topo: ChainTopology | null;
  doctor: ChainDoctorReport | null;
  instr: InstructionsScanReport | null;
  presets: ChainPreset[];
  journal: ChainJournalRecord[];
  repoMoves: ChainRepoMove[];
  duplicates: ChainDuplicatesReport | null;
  /** Fingerprint → located repair candidates, filled by a non-blocking pass. */
  candidates: Record<string, ChainRepairCandidate[]>;
  loading: boolean;
  error: string | null;
  /** Rescan. Resolves `true` only when a fresh topology landed — callers that
   * act on the inventory must not proceed on a stale one. */
  reload: () => Promise<boolean>;
  /** Refresh just the preset list — not worth a full rescan. */
  reloadPresets: () => Promise<void>;
}

const ChainContext = createContext<ChainState | null>(null);

export function ChainProvider({ children }: { children: ReactNode }) {
  const [topo, setTopo] = useState<ChainTopology | null>(null);
  const [doctor, setDoctor] = useState<ChainDoctorReport | null>(null);
  const [instr, setInstr] = useState<InstructionsScanReport | null>(null);
  const [presets, setPresets] = useState<ChainPreset[]>([]);
  const [journal, setJournal] = useState<ChainJournalRecord[]>([]);
  const [repoMoves, setRepoMoves] = useState<ChainRepoMove[]>([]);
  const [duplicates, setDuplicates] = useState<ChainDuplicatesReport | null>(null);
  const [candidates, setCandidates] = useState<Record<string, ChainRepairCandidate[]>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async (): Promise<boolean> => {
    setLoading(true);
    setError(null);
    try {
      // Only the topology is load-bearing: everything else decorates it, and
      // each failure is tolerated separately so none of them can blank the
      // link view.
      const [topology, report, instructions, presetList, records, storms, dupes] =
        await Promise.all([
          getChainTopology(),
          chainDoctorReport().catch(() => null),
          instructionsScan().catch(() => null),
          chainPresetsList().catch(() => null),
          chainRepairJournal().catch(() => null),
          chainRepoMoves().catch(() => null),
          getChainDuplicates().catch(() => null),
        ]);
      setTopo(topology);
      setDoctor(report);
      setInstr(instructions);
      setPresets(presetList ?? []);
      setJournal(records ?? []);
      setRepoMoves(storms?.groups ?? []);
      setDuplicates(dupes);
      // Broken findings get their candidate evidence located in a second,
      // non-blocking pass — cards render immediately and the candidate row
      // fills in when the lookup lands. Failures just leave it empty.
      const broken = (report?.findings ?? [])
        .filter((finding) => finding.deviation === "broken")
        .map((finding) => finding.fingerprint);
      setCandidates({});
      if (broken.length > 0) {
        chainLocateCandidates(broken)
          .then((located) => setCandidates(located.candidates))
          .catch(() => {});
      }
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setLoading(false);
    }
  }, []);

  const reloadPresets = useCallback(async () => {
    try {
      setPresets(await chainPresetsList());
    } catch {
      // The stale list stays; the next full reload refreshes it.
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const value = useMemo<ChainState>(
    () => ({
      topo,
      doctor,
      instr,
      presets,
      journal,
      repoMoves,
      duplicates,
      candidates,
      loading,
      error,
      reload,
      reloadPresets,
    }),
    [
      topo,
      doctor,
      instr,
      presets,
      journal,
      repoMoves,
      duplicates,
      candidates,
      loading,
      error,
      reload,
      reloadPresets,
    ],
  );

  return <ChainContext.Provider value={value}>{children}</ChainContext.Provider>;
}

export function useChain(): ChainState {
  const ctx = useContext(ChainContext);
  if (!ctx) throw new Error("useChain must be used within ChainProvider");
  return ctx;
}
