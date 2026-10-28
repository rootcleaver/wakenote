import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { TranslationPreferences } from "@/lib/text-translation";
import { useLlmReportRuns } from "../hooks/use-llm-report-runs";
import { useListVisibility } from "../hooks/use-list-visibility";
import { projectListItems } from "../lib/list-visibility";
import {
  exportAllTranscriptDays,
  exportTranscriptDay,
  loadTranscriptDays,
  loadTranscriptsForDay,
  mergeTranscriptAudio,
  openTranscriptFolder,
  rebuildTranscriptDayIndex,
  regenerateTranscript,
  enrollVoiceGate,
  startLlmReport,
  trashTranscripts,
} from "../lib/tauri-client";
import {
  nextAudioMergeProgress,
  type AudioMergeUiState,
} from "../lib/audio-merge";
import { transcriptDayFromRecordingReference } from "../lib/transcript-history";
import type {
  CustomSourceEntry,
  LlmReportKind,
  ModelDescriptor,
  RecentTranscript,
  TranscriptDay,
} from "../lib/types";
import {
  TranscriptsView,
  formatLocalDay,
  type TranscriptExportUiState,
} from "./TranscriptsView";

export function TranscriptsPanel({
  customSources = [],
  models = [],
  selectedModelId = "",
  autoPlayNext = false,
  openrouterKeyConfigured = false,
  voiceGate = false,
  onOpenReports,
  translation,
  initialSourceFilter,
  initialActiveDay,
}: {
  customSources?: readonly CustomSourceEntry[];
  models?: readonly Pick<ModelDescriptor, "id" | "display_name" | "status">[];
  selectedModelId?: string;
  autoPlayNext?: boolean;
  openrouterKeyConfigured?: boolean;
  /** Voice gate on: offer "This is me" on past microphone clips. */
  voiceGate?: boolean;
  onOpenReports?: () => void;
  translation?: TranslationPreferences;
  initialSourceFilter?: string;
  initialActiveDay?: string | null;
}) {
  const [days, setDays] = useState<TranscriptDay[]>([]);
  const [entriesByDay, setEntriesByDay] = useState<
    Map<string, RecentTranscript[]>
  >(() => new Map());
  const [loadingDay, setLoadingDay] = useState<string | null>(null);
  const { activeRun, runs } = useLlmReportRuns();
  const [startedRunId, setStartedRunId] = useState<string | null>(null);
  const [reportError, setReportError] = useState<string | null>(null);
  const [audioMergeState, setAudioMergeState] = useState<AudioMergeUiState>({
    status: "idle",
  });
  const [trashMutating, setTrashMutating] = useState(false);
  const [trashError, setTrashError] = useState<string | null>(null);
  const [exportState, setExportState] = useState<TranscriptExportUiState>({
    status: "idle",
  });
  const [visibilityMode, setVisibilityMode] = useState<"visible" | "hidden">(
    "visible",
  );
  const visibility = useListVisibility();
  const requestedRef = useRef<Set<string>>(new Set());
  const startedRun = runs.find((run) => run.run_id === startedRunId);
  const visibleRun = activeRun ?? startedRun ?? null;

  const projectedEntries = useMemo(() => {
    const visible = new Map<string, RecentTranscript[]>();
    const hidden = new Map<string, RecentTranscript[]>();
    for (const [day, entries] of entriesByDay) {
      const projection = projectListItems(
        entries,
        visibility.state,
        (entry) => ({
          kind: "transcript",
          id: entry.transcript_path,
        }),
      );
      visible.set(day, projection.visible);
      hidden.set(day, projection.hidden);
    }
    return { visible, hidden };
  }, [entriesByDay, visibility.state]);

  const visibilityCounts = useMemo(() => {
    const visible = new Map<string, number>();
    const hidden = new Map<string, number>();
    for (const [day, entries] of projectedEntries.visible) {
      visible.set(day, entries.length);
    }
    for (const [day, entries] of projectedEntries.hidden) {
      hidden.set(day, entries.length);
    }
    return { visible, hidden };
  }, [projectedEntries]);

  const refreshDays = useCallback(async () => {
    try {
      setDays(await loadTranscriptDays());
    } catch {
      // Day listing is best-effort; the calendar still shows today.
    }
  }, []);

  // `download` is false for on-navigation loads (iCloud-evicted sidecars are
  // skipped, no network) and true for an explicit reload (evicted sidecars for
  // that day are fetched).
  const loadDay = useCallback(async (day: string, download = false) => {
    setLoadingDay(day);
    try {
      const entries = await loadTranscriptsForDay(day, download);
      setEntriesByDay((prev) => {
        const next = new Map(prev);
        next.set(day, entries);
        return next;
      });
    } catch {
      // Leave any previously loaded entries in place on failure.
    } finally {
      setLoadingDay((current) => (current === day ? null : current));
    }
  }, []);

  const rebuildDay = useCallback(async (day: string, download = true) => {
    setLoadingDay(day);
    try {
      const entries = await rebuildTranscriptDayIndex(day, download);
      setEntriesByDay((prev) => {
        const next = new Map(prev);
        next.set(day, entries);
        return next;
      });
    } catch {
      // Leave any previously loaded entries in place on failure.
    } finally {
      setLoadingDay((current) => (current === day ? null : current));
    }
  }, []);

  // TranscriptsView fires onActiveDayChange(effectiveActiveDay) on mount (with
  // today) and whenever the active day changes. ensureDayLoaded's requestedRef
  // guard is what keeps each day loaded at most once across those effect
  // re-fires — do not remove it, or navigating back and forth re-loads days.
  // First-nav loads stay local (download=false); reloadDay and the
  // live-transcript-final handler intentionally bypass the guard to force a
  // re-read (reload additionally downloads evicted sidecars).
  const ensureDayLoaded = useCallback(
    (day: string) => {
      if (requestedRef.current.has(day)) {
        return;
      }
      requestedRef.current.add(day);
      void loadDay(day, false);
    },
    [loadDay],
  );

  const reloadDay = useCallback(
    (day: string) => {
      requestedRef.current.add(day);
      void rebuildDay(day, true);
      void refreshDays();
    },
    [rebuildDay, refreshDays],
  );

  const regenerateEntries = useCallback(
    async (entries: readonly RecentTranscript[], modelId?: string) => {
      const daysToReload = new Set<string>();
      for (const entry of entries) {
        if (!entry.audio_path) {
          continue;
        }
        await regenerateTranscript(entry.audio_path, modelId);
        daysToReload.add(formatLocalDay(new Date(entry.recorded_at)));
      }
      for (const day of daysToReload) {
        requestedRef.current.add(day);
        await loadDay(day, true);
      }
      if (daysToReload.size > 0) {
        void refreshDays();
      }
    },
    [loadDay, refreshDays],
  );

  const openEntryFolder = useCallback(async (entry: RecentTranscript) => {
    await openTranscriptFolder(entry.audio_path ?? entry.transcript_path);
  }, []);

  const mergeAudioEntries = useCallback(
    async (entries: readonly RecentTranscript[]) => {
      let operationId = "merge-pending";
      try {
        const result = await mergeTranscriptAudio(entries, (progress) => {
          operationId = progress.operation_id;
          setAudioMergeState((state) =>
            nextAudioMergeProgress(state, progress),
          );
        });
        if (result) {
          setAudioMergeState({
            status: "saved",
            operation_id: result.operation_id,
            destination_path: result.destination_path,
            input_count: result.input_count,
          });
        }
      } catch (error) {
        setAudioMergeState({
          status: "error",
          operation_id: operationId,
          message: error instanceof Error ? error.message : String(error),
        });
      }
    },
    [],
  );

  const openMergedAudioFolder = useCallback(async (destinationPath: string) => {
    await openTranscriptFolder(destinationPath);
  }, []);

  const exportAllDays = useCallback(async () => {
    setExportState({ status: "running", scope: "all" });
    try {
      const result = await exportAllTranscriptDays();
      setExportState(
        result ? { status: "success", result } : { status: "idle" },
      );
    } catch (error) {
      setExportState({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  const exportDay = useCallback(async (day: string) => {
    setExportState({ status: "running", scope: "day", day });
    try {
      const result = await exportTranscriptDay(day);
      setExportState(
        result ? { status: "success", result } : { status: "idle" },
      );
    } catch (error) {
      setExportState({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  const openExportFolder = useCallback(async (destinationPath: string) => {
    await openTranscriptFolder(destinationPath);
  }, []);

  const trashEntries = useCallback(
    async (entries: readonly RecentTranscript[]) => {
      if (entries.length === 0 || trashMutating) {
        return [];
      }
      setTrashMutating(true);
      setTrashError(null);
      try {
        const outcome = await trashTranscripts(entries);
        const removedPaths = new Set(outcome.removed_transcript_paths);
        if (removedPaths.size > 0) {
          setEntriesByDay((current) => {
            const next = new Map(current);
            for (const [day, dayEntries] of next) {
              next.set(
                day,
                dayEntries.filter(
                  (entry) => !removedPaths.has(entry.transcript_path),
                ),
              );
            }
            return next;
          });
        }
        const affectedDays = new Set(
          entries
            .filter((entry) => removedPaths.has(entry.transcript_path))
            .map((entry) => formatLocalDay(new Date(entry.recorded_at))),
        );
        for (const day of affectedDays) {
          requestedRef.current.add(day);
          await loadDay(day, false);
        }
        if (affectedDays.size > 0) {
          await refreshDays();
        }
        if (outcome.failures.length > 0) {
          const first = outcome.failures[0];
          setTrashError(
            `${outcome.failures.length} bundle${outcome.failures.length === 1 ? "" : "s"} could not be moved. ${first.error}`,
          );
        }
        return outcome.removed_transcript_paths;
      } catch (error) {
        setTrashError(error instanceof Error ? error.message : String(error));
        return [];
      } finally {
        setTrashMutating(false);
      }
    },
    [loadDay, refreshDays, trashMutating],
  );

  const generateReport = useCallback(
    async (entries: readonly RecentTranscript[], kind: LlmReportKind) => {
      if (entries.length === 0) {
        return;
      }
      setReportError(null);
      try {
        const run = await startLlmReport({
          kind,
          transcripts: [...entries],
        });
        setStartedRunId(run.run_id);
      } catch (error) {
        setReportError(error instanceof Error ? error.message : String(error));
      }
    },
    [],
  );

  useEffect(() => {
    void refreshDays();
  }, [refreshDays]);

  // Auto-refresh the recorded day as new transcripts are finalized (live or queue).
  // Queued jobs can finish long after recording, so the event's recorded_at
  // value, not wall-clock "today", chooses the bucket to reload.
  useEffect(() => {
    if (typeof window === "undefined" || !window.__TAURI_INTERNALS__) {
      return;
    }
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const stop = await listen("live-transcript-final", (event) => {
        const payload = event.payload as {
          recorded_at?: string | null;
          audio_path?: string | null;
        };
        const todayDay = formatLocalDay(new Date());
        const day = transcriptDayFromRecordingReference(
          payload.recorded_at,
          payload.audio_path,
        );
        requestedRef.current.add(day);
        void loadDay(day, false);
        if (day !== todayDay) {
          void refreshDays();
        }
      });
      if (cancelled) {
        stop();
      } else {
        unlisten = stop;
      }
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadDay, refreshDays]);

  return (
    <TranscriptsView
      onEnrollVoice={
        voiceGate
          ? async (entry) => {
              if (entry.audio_path) await enrollVoiceGate(entry.audio_path);
            }
          : undefined
      }
      initialSourceFilter={initialSourceFilter}
      initialActiveDay={initialActiveDay}
      translation={translation}
      days={days}
      entriesByDay={
        visibilityMode === "visible"
          ? projectedEntries.visible
          : projectedEntries.hidden
      }
      visibleCountByDay={visibilityCounts.visible}
      hiddenCountByDay={visibilityCounts.hidden}
      visibilityMode={visibilityMode}
      visibilityMutating={visibility.loading || visibility.mutating}
      visibilityStatus={visibility.announcement}
      visibilityError={visibility.error}
      trashMutating={trashMutating}
      trashError={trashError}
      loadingDay={loadingDay}
      models={models}
      selectedModelId={selectedModelId}
      sourceLabels={sourceLabelsFromCustomSources(customSources)}
      autoPlayNext={autoPlayNext}
      openrouterKeyConfigured={openrouterKeyConfigured}
      reportError={reportError}
      reportRun={visibleRun}
      audioMergeState={audioMergeState}
      exportState={exportState}
      onActiveDayChange={ensureDayLoaded}
      onOpenFolder={openEntryFolder}
      onGenerateReport={generateReport}
      onMergeAudio={mergeAudioEntries}
      onOpenMergedAudioFolder={openMergedAudioFolder}
      onExportAllDays={exportAllDays}
      onExportDay={exportDay}
      onOpenExportFolder={openExportFolder}
      onOpenReports={onOpenReports}
      onRegenerate={regenerateEntries}
      onReload={reloadDay}
      onVisibilityModeChange={setVisibilityMode}
      onSetTranscriptsHidden={(entries, hidden) =>
        visibility.setTargetsHidden(
          entries.map((entry) => ({
            kind: "transcript",
            id: entry.transcript_path,
          })),
          hidden,
        )
      }
      onTrashTranscripts={trashEntries}
    />
  );
}

function sourceLabelsFromCustomSources(
  customSources: readonly CustomSourceEntry[],
): Record<string, string> {
  return Object.fromEntries(
    customSources.map((source) => [source.id, source.label]),
  );
}
