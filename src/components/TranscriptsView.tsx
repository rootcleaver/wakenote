import {
  Check,
  ChevronLeft,
  ChevronRight,
  CloudDownload,
  Copy,
  Download,
  Eye,
  EyeOff,
  FileText,
  FileAudio,
  FolderOpen,
  Loader2,
  Mic,
  MonitorSpeaker,
  MoreHorizontal,
  Pause,
  Play,
  RotateCw,
  Trash2,
  Video,
  Youtube,
} from "lucide-react";
import type {
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
} from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { TranslatedText } from "./TranslatedText";
import type { TranslationPreferences } from "@/lib/text-translation";
import {
  fileUrlFromPath,
  formatLocalTimestamp,
  formatTranscriptsForCopy,
  isDictationTranscript,
  transcriptSourceLabel,
} from "../lib/transcript-history";
import { formatModelLabel } from "../lib/models";
import { isActiveLlmReportRun } from "../lib/llm-report-runs";
import type {
  LlmReportKind,
  LlmReportRunSnapshot,
  ModelDescriptor,
  RecentTranscript,
  TranscriptDay,
  TranscriptExportResult,
} from "../lib/types";
import {
  ListVisibilityToolbar,
  type ListVisibilityMode,
} from "./ListVisibilityToolbar";
import { Button } from "./ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "./ui/alert-dialog";
import { Checkbox } from "./ui/checkbox";
import { EmptyState } from "./ui/empty-state";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import { TranscriptPlayerDock } from "./transcripts/TranscriptPlayerDock";
import { Progress } from "./ui/progress";
import {
  eligibleAudioMergeEntries,
  type AudioMergeUiState,
} from "../lib/audio-merge";
import {
  addDays,
  DatePagePicker,
  formatLocalDay,
  type HistorySortOrder,
  isYearMonthDayLabel,
  weekStartFor,
} from "./DatePagePicker";

export {
  addDays,
  formatLocalDay,
  nextWeekDisabledReason,
  previousWeekDisabledReason,
  weekStartFor,
} from "./DatePagePicker";

type CopyToastKind = "all" | "selected";
type DragMode = "select" | "deselect";
type RegenerationModel = Pick<
  ModelDescriptor,
  "id" | "display_name" | "status"
>;

const ALL_SOURCE_FILTER = "all";
export const TRANSCRIPT_PAGE_SIZE = 50;
const TRANSCRIPT_PAGE_SIZE_OPTIONS = [30, 50, 100, 200] as const;

export interface TranscriptSourceFilterOption {
  id: string;
  label: string;
  count: number;
}

export function transcriptPage(
  entries: readonly RecentTranscript[],
  requestedPage: number,
  pageSize = TRANSCRIPT_PAGE_SIZE,
) {
  const total = entries.length;
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const page = Math.min(Math.max(1, Math.floor(requestedPage)), pageCount);
  const offset = (page - 1) * pageSize;
  const pageEntries = entries.slice(offset, offset + pageSize);

  return {
    entries: pageEntries,
    page,
    pageCount,
    rangeStart: total === 0 ? 0 : offset + 1,
    rangeEnd: offset + pageEntries.length,
    total,
  };
}

interface DragState {
  mode: DragMode;
  visited: Set<string>;
}

interface TranscriptContextMenu {
  targets: RecentTranscript[];
  regenerationTargets: RecentTranscript[];
  x: number;
  y: number;
}

export interface TranscriptPlaybackState {
  playingTranscriptPath: string | null;
  playbackPaused: boolean;
}

export type TranscriptExportUiState =
  | { status: "idle" }
  | { status: "running"; scope: "all" | "day"; day?: string }
  | { status: "success"; result: TranscriptExportResult }
  | { status: "error"; message: string };

export function transcriptPlaybackStateAfterToggle(
  state: TranscriptPlaybackState,
  entry: RecentTranscript,
): TranscriptPlaybackState & { shouldChangeActiveTranscript: boolean } {
  if (!entry.audio_path) {
    return { ...state, shouldChangeActiveTranscript: false };
  }
  if (state.playingTranscriptPath === entry.transcript_path) {
    return {
      playingTranscriptPath: state.playingTranscriptPath,
      playbackPaused: !state.playbackPaused,
      shouldChangeActiveTranscript: false,
    };
  }
  return {
    playingTranscriptPath: entry.transcript_path,
    playbackPaused: false,
    shouldChangeActiveTranscript: true,
  };
}

/**
 * Returns the transcript path of the first playable entry (one with audio) after
 * `currentPath` in `entries`, or null when the current item is last/absent. Used to
 * auto-advance playback through the visible list.
 */
export function nextPlayableTranscriptPath(
  entries: readonly RecentTranscript[],
  currentPath: string | null,
): string | null {
  if (currentPath === null) {
    return null;
  }
  const currentIndex = entries.findIndex(
    (entry) => entry.transcript_path === currentPath,
  );
  if (currentIndex === -1) {
    return null;
  }
  for (let index = currentIndex + 1; index < entries.length; index += 1) {
    if (entries[index].audio_path) {
      return entries[index].transcript_path;
    }
  }
  return null;
}

export function TranscriptsView({
  translation,
  days,
  entriesByDay,
  loadingDay = null,
  sourceLabels = {},
  models = [],
  selectedModelId = "",
  onActiveDayChange,
  onRegenerate,
  onOpenFolder,
  onMergeAudio,
  onOpenMergedAudioFolder,
  onGenerateReport,
  onOpenReports,
  onReload,
  onExportAllDays,
  onExportDay,
  onOpenExportFolder,
  exportState = { status: "idle" },
  autoPlayNext = false,
  openrouterKeyConfigured = false,
  reportError = null,
  reportRun = null,
  audioMergeState = { status: "idle" },
  visibilityMode = "visible",
  visibleCountByDay = new Map(),
  hiddenCountByDay = new Map(),
  visibilityMutating = false,
  visibilityStatus = "",
  visibilityError = null,
  trashMutating = false,
  trashError = null,
  onVisibilityModeChange,
  onSetTranscriptsHidden,
  onTrashTranscripts,
  initialPlayingTranscriptPath = null,
  initialSourceFilter = ALL_SOURCE_FILTER,
  initialActiveDay = null,
  today = new Date(),
  onEnrollVoice,
}: {
  days: TranscriptDay[];
  translation?: TranslationPreferences;
  entriesByDay: ReadonlyMap<string, RecentTranscript[]>;
  loadingDay?: string | null;
  sourceLabels?: Readonly<Record<string, string>>;
  models?: readonly RegenerationModel[];
  selectedModelId?: string;
  onActiveDayChange?: (day: string) => void;
  onRegenerate?: (
    entries: readonly RecentTranscript[],
    modelId?: string,
  ) => void | Promise<void>;
  onOpenFolder?: (entry: RecentTranscript) => void | Promise<void>;
  onMergeAudio?: (entries: readonly RecentTranscript[]) => void | Promise<void>;
  onOpenMergedAudioFolder?: (destinationPath: string) => void | Promise<void>;
  onGenerateReport?: (
    entries: readonly RecentTranscript[],
    kind: LlmReportKind,
  ) => void | Promise<void>;
  onOpenReports?: () => void;
  onReload?: (day: string) => void;
  onExportAllDays?: () => void | Promise<void>;
  onExportDay?: (day: string) => void | Promise<void>;
  onOpenExportFolder?: (destinationPath: string) => void | Promise<void>;
  exportState?: TranscriptExportUiState;
  autoPlayNext?: boolean;
  openrouterKeyConfigured?: boolean;
  reportError?: string | null;
  reportRun?: LlmReportRunSnapshot | null;
  audioMergeState?: AudioMergeUiState;
  visibilityMode?: ListVisibilityMode;
  visibleCountByDay?: ReadonlyMap<string, number>;
  hiddenCountByDay?: ReadonlyMap<string, number>;
  visibilityMutating?: boolean;
  visibilityStatus?: string;
  visibilityError?: string | null;
  trashMutating?: boolean;
  trashError?: string | null;
  onVisibilityModeChange?: (mode: ListVisibilityMode) => void;
  onSetTranscriptsHidden?: (
    entries: readonly RecentTranscript[],
    hidden: boolean,
  ) => boolean | Promise<boolean>;
  onTrashTranscripts?: (
    entries: readonly RecentTranscript[],
  ) => readonly string[] | Promise<readonly string[]>;
  initialPlayingTranscriptPath?: string | null;
  initialSourceFilter?: string;
  initialActiveDay?: string | null;
  today?: Date;
  /** Add this past microphone clip to the enrolled voice (voice gate). */
  onEnrollVoice?: (entry: RecentTranscript) => Promise<void>;
}) {
  const [enrolledVoicePaths, setEnrolledVoicePaths] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [voiceEnrollError, setVoiceEnrollError] = useState<string | null>(null);
  const enrollVoice = onEnrollVoice
    ? async (entry: RecentTranscript) => {
        try {
          await onEnrollVoice(entry);
          setEnrolledVoicePaths((previous) =>
            new Set(previous).add(entry.transcript_path),
          );
          setVoiceEnrollError(null);
        } catch (error) {
          setVoiceEnrollError(String(error));
        }
      }
    : undefined;
  const todayDay = formatLocalDay(today);

  const availableDays = useMemo(() => {
    const set = new Set<string>([todayDay]);
    if (
      initialActiveDay &&
      isYearMonthDayLabel(initialActiveDay) &&
      initialActiveDay <= todayDay
    ) {
      set.add(initialActiveDay);
    }
    for (const entry of days) {
      if (isYearMonthDayLabel(entry.day) && entry.day <= todayDay) {
        set.add(entry.day);
      }
    }
    return set;
  }, [days, todayDay, initialActiveDay]);

  const earliestDay = useMemo(() => {
    const sorted = [...availableDays].filter(isYearMonthDayLabel).sort();
    return sorted[0] ?? todayDay;
  }, [availableDays, todayDay]);

  const [activeDay, setActiveDay] = useState<string | null>(initialActiveDay);
  const [viewWeekStart, setViewWeekStart] = useState<string | null>(null);
  const [playingTranscriptPath, setPlayingTranscriptPath] = useState<
    string | null
  >(initialPlayingTranscriptPath);
  const [playbackPaused, setPlaybackPaused] = useState(false);
  const [sourceFilter, setSourceFilter] = useState(initialSourceFilter);
  const [sortOrder, setSortOrder] = useState<HistorySortOrder>("newest");
  const [requestedPage, setRequestedPage] = useState(1);
  const [pageSize, setPageSize] = useState<number>(TRANSCRIPT_PAGE_SIZE);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(
    () => new Set(),
  );
  const [copyToast, setCopyToast] = useState<CopyToastKind | null>(null);
  const [contextMenu, setContextMenu] = useState<TranscriptContextMenu | null>(
    null,
  );
  const dragStateRef = useRef<DragState | null>(null);
  const lastSelectionAnchorRef = useRef<string | null>(null);
  const effectiveActiveDay = activeDay ?? todayDay;

  const activeEntries = useMemo(() => {
    const entries = entriesByDay.get(effectiveActiveDay) ?? [];
    const chronological = [...entries].sort(
      (left, right) =>
        timestampValue(left.recorded_at) - timestampValue(right.recorded_at) ||
        left.transcript_path.localeCompare(right.transcript_path),
    );
    return sortOrder === "newest" ? chronological.reverse() : chronological;
  }, [entriesByDay, effectiveActiveDay, sortOrder]);

  const sourceFilterOptions = useMemo(
    () => transcriptSourceFilterOptions(activeEntries, sourceLabels),
    [activeEntries, sourceLabels],
  );

  const effectiveSourceFilter = sourceFilterOptions.some(
    (option) => option.id === sourceFilter,
  )
    ? sourceFilter
    : ALL_SOURCE_FILTER;
  const selectedSourceOption = sourceFilterOptions.find(
    (option) => option.id === effectiveSourceFilter,
  );

  const filteredEntries = useMemo(
    () => filterTranscriptsBySource(activeEntries, effectiveSourceFilter),
    [activeEntries, effectiveSourceFilter],
  );
  const pagination = transcriptPage(filteredEntries, requestedPage, pageSize);
  const pageEntries = pagination.entries;

  const usableRegenerationModels = useMemo(
    () =>
      orderRegenerationModelOptions(
        regenerationModelOptions(models),
        selectedModelId,
      ),
    [models, selectedModelId],
  );

  const playingTranscript = useMemo(() => {
    for (const list of entriesByDay.values()) {
      const match = list.find(
        (entry) => entry.transcript_path === playingTranscriptPath,
      );
      if (match) {
        return match;
      }
    }
    return undefined;
  }, [entriesByDay, playingTranscriptPath]);

  useEffect(() => {
    onActiveDayChange?.(effectiveActiveDay);
  }, [onActiveDayChange, effectiveActiveDay]);

  useEffect(() => {
    if (
      activeDay !== null &&
      !availableDays.has(activeDay) &&
      activeDay !== todayDay
    ) {
      setActiveDay(null);
    }
  }, [availableDays, activeDay, todayDay]);

  useEffect(() => {
    setSelectedPaths(new Set());
    lastSelectionAnchorRef.current = null;
    setCopyToast(null);
  }, [
    effectiveActiveDay,
    visibilityMode,
    sortOrder,
    pagination.page,
    pageSize,
  ]);

  useEffect(() => {
    if (!sourceFilterOptions.some((option) => option.id === sourceFilter)) {
      setSourceFilter(ALL_SOURCE_FILTER);
    }
  }, [sourceFilter, sourceFilterOptions]);

  useEffect(() => {
    const visiblePaths = new Set(
      pageEntries.map((entry) => entry.transcript_path),
    );
    if (
      lastSelectionAnchorRef.current &&
      !visiblePaths.has(lastSelectionAnchorRef.current)
    ) {
      lastSelectionAnchorRef.current = null;
    }
    setSelectedPaths((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const path of prev) {
        if (visiblePaths.has(path)) {
          next.add(path);
        } else {
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [pageEntries]);

  useEffect(() => {
    const endDrag = () => {
      dragStateRef.current = null;
    };
    window.addEventListener("pointerup", endDrag);
    window.addEventListener("pointercancel", endDrag);
    return () => {
      window.removeEventListener("pointerup", endDrag);
      window.removeEventListener("pointercancel", endDrag);
    };
  }, []);

  useEffect(() => {
    if (!contextMenu) {
      return;
    }
    const close = () => setContextMenu(null);
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        close();
      }
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("keydown", closeOnEscape);
    };
  }, [contextMenu]);

  const beginDragSelection = useCallback(
    (path: string, extendRange = false) => {
      if (extendRange) {
        dragStateRef.current = null;
        const anchorPath = lastSelectionAnchorRef.current;
        if (!anchorPath) {
          lastSelectionAnchorRef.current = path;
        }
        setSelectedPaths((prev) =>
          selectTranscriptPathsAfterShiftClick(
            pageEntries,
            prev,
            anchorPath,
            path,
          ),
        );
        return;
      }
      lastSelectionAnchorRef.current = path;
      setSelectedPaths((prev) => {
        const next = new Set(prev);
        const mode: DragMode = prev.has(path) ? "deselect" : "select";
        if (mode === "select") {
          next.add(path);
        } else {
          next.delete(path);
        }
        dragStateRef.current = { mode, visited: new Set([path]) };
        return next;
      });
    },
    [pageEntries],
  );

  const continueDragSelection = useCallback((path: string) => {
    const drag = dragStateRef.current;
    if (!drag) return;
    if (drag.visited.has(path)) return;
    drag.visited.add(path);
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (drag.mode === "select") {
        next.add(path);
      } else {
        next.delete(path);
      }
      return next;
    });
  }, []);

  const handleSelectDay = (day: string) => {
    setActiveDay(day);
    setViewWeekStart(null);
    setRequestedPage(1);
  };

  const handleSourceFilterChange = (nextFilter: string) => {
    setSourceFilter(nextFilter);
    setRequestedPage(1);
    setSelectedPaths(new Set());
    lastSelectionAnchorRef.current = null;
  };

  const handleSortOrderChange = (nextOrder: HistorySortOrder) => {
    setSortOrder(nextOrder);
    setRequestedPage(1);
    setSelectedPaths(new Set());
    lastSelectionAnchorRef.current = null;
  };

  const handlePageSizeChange = (value: string) => {
    const nextSize = TRANSCRIPT_PAGE_SIZE_OPTIONS.find(
      (size) => String(size) === value,
    );
    if (nextSize === undefined) return;
    setPageSize(nextSize);
    setRequestedPage(1);
  };

  const writeToClipboard = useCallback(
    async (text: string, kind: CopyToastKind) => {
      if (!text) return;
      try {
        await navigator.clipboard.writeText(text);
        setCopyToast(kind);
        setSelectedPaths(new Set());
        lastSelectionAnchorRef.current = null;
        window.setTimeout(() => setCopyToast(null), 1500);
      } catch {
        // Clipboard API unavailable — silently ignore; UI feedback simply won't toggle.
      }
    },
    [],
  );

  const handleCopyAll = useCallback(() => {
    void writeToClipboard(
      formatTranscriptsForCopy(filteredEntries, sourceLabels),
      "all",
    );
  }, [writeToClipboard, filteredEntries, sourceLabels]);

  const handleCopySelected = useCallback(() => {
    const selected = pageEntries.filter((entry) =>
      selectedPaths.has(entry.transcript_path),
    );
    void writeToClipboard(
      formatTranscriptsForCopy(selected, sourceLabels),
      "selected",
    );
  }, [writeToClipboard, pageEntries, selectedPaths, sourceLabels]);

  const selectedEntries = useMemo(
    () =>
      pageEntries.filter((entry) => selectedPaths.has(entry.transcript_path)),
    [pageEntries, selectedPaths],
  );
  const selectedAudioMergeEntries = useMemo(
    () => eligibleAudioMergeEntries(selectedEntries),
    [selectedEntries],
  );

  const handleGenerateReport = useCallback(
    (entries: readonly RecentTranscript[], kind: LlmReportKind) => {
      if (
        !openrouterKeyConfigured ||
        (reportRun && isActiveLlmReportRun(reportRun))
      ) {
        return;
      }
      void onGenerateReport?.(entries, kind);
    },
    [onGenerateReport, openrouterKeyConfigured, reportRun],
  );

  const handleClearSelection = useCallback(() => {
    setSelectedPaths(new Set());
    lastSelectionAnchorRef.current = null;
  }, []);

  const handleTrashSelected = useCallback(async () => {
    if (!onTrashTranscripts || selectedEntries.length === 0 || trashMutating) {
      return;
    }
    const removedPaths = new Set(await onTrashTranscripts(selectedEntries));
    if (removedPaths.size === 0) {
      return;
    }
    setSelectedPaths((current) => {
      const next = new Set(current);
      for (const path of removedPaths) {
        next.delete(path);
      }
      return next;
    });
    if (playingTranscriptPath && removedPaths.has(playingTranscriptPath)) {
      setPlayingTranscriptPath(null);
      setPlaybackPaused(false);
    }
    if (
      lastSelectionAnchorRef.current &&
      removedPaths.has(lastSelectionAnchorRef.current)
    ) {
      lastSelectionAnchorRef.current = null;
    }
  }, [
    onTrashTranscripts,
    playingTranscriptPath,
    selectedEntries,
    trashMutating,
  ]);

  const handleSelectAllVisible = useCallback(() => {
    setSelectedPaths(selectTranscriptPathsForEntries(pageEntries));
    lastSelectionAnchorRef.current = pageEntries[0]?.transcript_path ?? null;
  }, [pageEntries]);

  const handleTranscriptSelectionChange = useCallback(
    (path: string, selected: boolean) => {
      lastSelectionAnchorRef.current = path;
      setSelectedPaths((current) => {
        const next = new Set(current);
        if (selected) {
          next.add(path);
        } else {
          next.delete(path);
        }
        return next;
      });
    },
    [],
  );

  const applyTranscriptVisibility = useCallback(
    async (entries: readonly RecentTranscript[]) => {
      if (!onSetTranscriptsHidden || entries.length === 0) {
        return false;
      }
      const succeeded = await onSetTranscriptsHidden(
        entries,
        visibilityMode === "visible",
      );
      if (succeeded) {
        handleClearSelection();
      }
      return succeeded;
    },
    [handleClearSelection, onSetTranscriptsHidden, visibilityMode],
  );

  const handleTogglePlayback = useCallback(
    (entry: RecentTranscript) => {
      const next = transcriptPlaybackStateAfterToggle(
        { playingTranscriptPath, playbackPaused },
        entry,
      );
      if (
        next.playingTranscriptPath === playingTranscriptPath &&
        next.playbackPaused === playbackPaused
      ) {
        return;
      }
      setPlayingTranscriptPath(next.playingTranscriptPath);
      setPlaybackPaused(next.playbackPaused);
    },
    [playingTranscriptPath, playbackPaused],
  );

  const handlePlaybackEnded = useCallback(() => {
    if (autoPlayNext) {
      const nextPath = nextPlayableTranscriptPath(
        pageEntries,
        playingTranscriptPath,
      );
      if (nextPath) {
        setPlayingTranscriptPath(nextPath);
        setPlaybackPaused(false);
        return;
      }
    }
    setPlaybackPaused(true);
  }, [autoPlayNext, pageEntries, playingTranscriptPath]);

  const openContextMenu = useCallback(
    (entry: RecentTranscript, x: number, y: number) => {
      const canOpenVisibilityMenu = Boolean(onSetTranscriptsHidden);
      const canOpenRegenerationMenu =
        Boolean(onRegenerate) &&
        usableRegenerationModels.length > 0 &&
        Boolean(entry.audio_path);
      if (!canOpenVisibilityMenu && !canOpenRegenerationMenu) {
        return;
      }
      const targets = transcriptVisibilityTargetsForContextMenu(
        entry,
        pageEntries,
        selectedPaths,
      );
      if (targets.length === 0) {
        return;
      }
      const regenerationTargets = transcriptRegenerationTargetsForContextMenu(
        entry,
        pageEntries,
        selectedPaths,
      );
      if (!selectedPaths.has(entry.transcript_path)) {
        setSelectedPaths(new Set([entry.transcript_path]));
        lastSelectionAnchorRef.current = entry.transcript_path;
      }
      const menuWidth = 424;
      const menuHeight = 88;
      const maxX = Math.max(8, window.innerWidth - menuWidth - 8);
      const maxY = Math.max(8, window.innerHeight - menuHeight - 8);
      setContextMenu({
        targets,
        regenerationTargets,
        x: Math.min(Math.max(8, x), maxX),
        y: Math.min(Math.max(8, y), maxY),
      });
    },
    [
      pageEntries,
      onRegenerate,
      onSetTranscriptsHidden,
      selectedPaths,
      usableRegenerationModels.length,
    ],
  );

  const handleRegenerateFromMenu = useCallback(
    (modelId: string) => {
      if (!contextMenu) {
        return;
      }
      const targets = contextMenu.regenerationTargets;
      setContextMenu(null);
      void onRegenerate?.(targets, modelId);
    },
    [contextMenu, onRegenerate],
  );

  const handleVisibilityFromMenu = useCallback(() => {
    if (!contextMenu) {
      return;
    }
    const targets = contextMenu.targets;
    setContextMenu(null);
    void applyTranscriptVisibility(targets);
  }, [applyTranscriptVisibility, contextMenu]);

  const selectionCount = pageEntries.filter((entry) =>
    selectedPaths.has(entry.transcript_path),
  ).length;
  const hasEntries = pageEntries.length > 0;
  const hasAnyEntries = activeEntries.length > 0;
  const isLoadingActive = loadingDay === effectiveActiveDay;
  const canRegenerateFromContext =
    Boolean(onRegenerate) && usableRegenerationModels.length > 0;
  const canOpenContextMenu =
    canRegenerateFromContext || Boolean(onSetTranscriptsHidden);
  const canGenerateReports = Boolean(onGenerateReport) && hasEntries;
  const reportDisabledReason = !openrouterKeyConfigured
    ? "Save an OpenRouter API key in Advanced settings first"
    : reportRun && isActiveLlmReportRun(reportRun)
      ? "A report is already running"
      : undefined;

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!shouldCopySelectedTranscriptsOnKeydown(event, selectionCount)) {
        return;
      }
      const target = event.target as HTMLElement | null;
      if (isKeyboardCopyIgnoredTarget(target)) {
        return;
      }
      event.preventDefault();
      handleCopySelected();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleCopySelected, selectionCount]);

  // The calendar count (size-based) can exceed the entries we managed to load
  // when some of the day's sidecars are iCloud-evicted (skipped on the
  // no-download navigation read). The gap is what an explicit reload would fetch.
  const availableCount = useMemo(
    () => days.find((entry) => entry.day === effectiveActiveDay)?.count ?? 0,
    [days, effectiveActiveDay],
  );
  const visibleCount =
    visibleCountByDay.get(effectiveActiveDay) ??
    (visibilityMode === "visible" ? activeEntries.length : 0);
  const hiddenCount =
    hiddenCountByDay.get(effectiveActiveDay) ??
    (visibilityMode === "hidden" ? activeEntries.length : 0);
  const pendingCount = Math.max(0, availableCount - visibleCount - hiddenCount);
  const hasPending = pendingCount > 0 && !isLoadingActive;
  const transcriptCountText =
    effectiveSourceFilter === ALL_SOURCE_FILTER
      ? `${activeEntries.length} transcript${activeEntries.length === 1 ? "" : "s"}`
      : `${filteredEntries.length} / ${activeEntries.length} transcripts`;
  const effectiveWeekStart = viewWeekStart ?? weekStartFor(effectiveActiveDay);
  const handlePrevWeek = () =>
    setViewWeekStart(addDays(effectiveWeekStart, -7));
  const handleNextWeek = () => setViewWeekStart(addDays(effectiveWeekStart, 7));
  const exportRunning = exportState.status === "running";
  const allPageEntriesSelected =
    pageEntries.length > 0 && selectionCount === pageEntries.length;
  const selectionChecked =
    selectionCount === 0
      ? false
      : allPageEntriesSelected
        ? true
        : "indeterminate";

  return (
    <div className="transcripts-panel">
      <section
        className="transcript-archive-rail"
        aria-label="Transcript archive"
      >
        <DatePagePicker
          activeDay={effectiveActiveDay}
          availableDays={availableDays}
          todayDay={todayDay}
          weekStart={effectiveWeekStart}
          earliestDay={earliestDay}
          itemLabel="Transcript"
          onSelectDay={handleSelectDay}
          onPrevWeek={handlePrevWeek}
          onNextWeek={handleNextWeek}
        />
        <Button
          aria-label="Export all transcript days"
          disabled={exportRunning || days.length === 0 || !onExportAllDays}
          onClick={() => void onExportAllDays?.()}
          type="button"
        >
          {exportState.status === "running" && exportState.scope === "all" ? (
            <Loader2 className="loading-spin" />
          ) : (
            <Download />
          )}
          Export all
        </Button>
      </section>
      {exportState.status !== "idle" ? (
        <section
          aria-live="polite"
          className={`transcript-export-status${
            exportState.status === "error"
              ? " warning-banner warning-banner--danger"
              : ""
          }`}
          data-status={exportState.status}
          role="status"
        >
          <span>
            {exportState.status === "running"
              ? exportState.scope === "all"
                ? "Exporting all transcript days…"
                : `Exporting transcripts for ${exportState.day ?? effectiveActiveDay}…`
              : exportState.status === "success"
                ? `Exported ${exportState.result.transcript_count} transcript${
                    exportState.result.transcript_count === 1 ? "" : "s"
                  } across ${exportState.result.day_count} day${
                    exportState.result.day_count === 1 ? "" : "s"
                  }`
                : `Transcript export unavailable: ${exportState.message}`}
          </span>
          {exportState.status === "success" && onOpenExportFolder ? (
            <Button
              aria-label="Show export in Finder"
              onClick={() =>
                void onOpenExportFolder(exportState.result.destination_path)
              }
              size="sm"
              type="button"
              variant="secondary"
            >
              <FolderOpen /> Show in Finder
            </Button>
          ) : null}
        </section>
      ) : null}
      <div className="transcript-browse-row">
        <ListVisibilityToolbar
          mode={visibilityMode}
          visibleCount={visibleCount}
          hiddenCount={hiddenCount}
          selectedCount={selectionCount}
          totalInMode={pageEntries.length}
          mutating={visibilityMutating}
          statusMessage={visibilityStatus}
          selectionPlacement="external"
          onModeChange={(mode) => onVisibilityModeChange?.(mode)}
          onSelectAll={handleSelectAllVisible}
          onClearSelection={handleClearSelection}
          onApplySelection={() =>
            void applyTranscriptVisibility(selectedEntries)
          }
        />
        <div className="transcript-browse-row__filters">
          <label className="transcript-source-filter">
            <span>Source</span>
            <Select
              value={effectiveSourceFilter}
              onValueChange={handleSourceFilterChange}
            >
              <SelectTrigger size="sm" aria-label="Transcript source">
                <SelectValue>
                  {selectedSourceOption
                    ? `${selectedSourceOption.label} (${selectedSourceOption.count})`
                    : "All sources"}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                {sourceFilterOptions.map((option) => (
                  <SelectItem key={option.id} value={option.id}>
                    {option.label} ({option.count})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </label>
          <label className="transcript-source-filter">
            <span>Order</span>
            <Select
              value={sortOrder}
              onValueChange={(value) =>
                handleSortOrderChange(value as HistorySortOrder)
              }
            >
              <SelectTrigger size="sm" aria-label="Transcript order">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="newest">Newest first</SelectItem>
                <SelectItem value="oldest">Oldest first</SelectItem>
              </SelectContent>
            </Select>
          </label>
          <label className="transcript-source-filter">
            <span>Per page</span>
            <Select
              value={String(pageSize)}
              onValueChange={handlePageSizeChange}
            >
              <SelectTrigger size="sm" aria-label="Transcripts per page">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {TRANSCRIPT_PAGE_SIZE_OPTIONS.map((size) => (
                  <SelectItem key={size} value={String(size)}>
                    {size}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </label>
        </div>
      </div>
      <article className="transcript-day transcript-day--condensed">
        <header>
          <div>
            <span>{effectiveActiveDay}</span>
            <strong>{transcriptCountText}</strong>
          </div>
          <div
            data-slot="transcript-toolbar"
            className="transcript-day__actions transcript-toolbar"
          >
            {hasPending && hasAnyEntries ? (
              <span className="transcript-day__icloud-hint">
                {pendingCount} more in iCloud
              </span>
            ) : null}
            <Button
              aria-label={`Export transcripts for ${effectiveActiveDay}`}
              disabled={
                exportRunning ||
                (availableCount === 0 && !hasAnyEntries) ||
                !onExportDay
              }
              onClick={() => void onExportDay?.(effectiveActiveDay)}
              size="sm"
              type="button"
              variant="secondary"
            >
              {exportState.status === "running" &&
              exportState.scope === "day" ? (
                <Loader2 className="loading-spin" />
              ) : (
                <Download />
              )}
              Export day
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  aria-label="More day actions"
                  size="sm"
                  type="button"
                  variant="ghost"
                >
                  <MoreHorizontal /> More
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {hasEntries ? (
                  <DropdownMenuItem onSelect={handleCopyAll}>
                    <Copy /> {copyToast === "all" ? "Copied" : "Copy all"}
                  </DropdownMenuItem>
                ) : null}
                {canGenerateReports ? (
                  <>
                    <DropdownMenuItem
                      disabled={Boolean(reportDisabledReason)}
                      title={reportDisabledReason}
                      onSelect={() =>
                        handleGenerateReport(filteredEntries, "summary")
                      }
                    >
                      <FileText /> Summary all
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      disabled={Boolean(reportDisabledReason)}
                      title={reportDisabledReason}
                      onSelect={() =>
                        handleGenerateReport(filteredEntries, "detailed_report")
                      }
                    >
                      <FileText /> Report all
                    </DropdownMenuItem>
                  </>
                ) : null}
                {(hasEntries || canGenerateReports) && (
                  <DropdownMenuSeparator />
                )}
                <DropdownMenuItem
                  disabled={isLoadingActive}
                  onSelect={() => onReload?.(effectiveActiveDay)}
                >
                  <RotateCw /> Reload
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </header>
        {hasEntries && selectionCount > 0 ? (
          <section
            aria-label="Selected transcript actions"
            className="transcript-selection-shelf"
          >
            <label className="transcript-selection-shelf__select-all">
              <Checkbox
                aria-label={
                  allPageEntriesSelected
                    ? "Clear transcript selection"
                    : "Select all visible transcripts"
                }
                checked={selectionChecked}
                disabled={visibilityMutating}
                onCheckedChange={() => {
                  if (allPageEntriesSelected) {
                    handleClearSelection();
                  } else {
                    handleSelectAllVisible();
                  }
                }}
              />
              <strong>{selectionCount} selected</strong>
            </label>
            <Button
              disabled={visibilityMutating}
              onClick={handleClearSelection}
              size="sm"
              type="button"
              variant="ghost"
            >
              Clear
            </Button>
            <Button
              aria-label="Copy selected transcripts"
              onClick={handleCopySelected}
              size="sm"
              type="button"
              variant="secondary"
            >
              {copyToast === "selected" ? <Check /> : <Copy />}
              {copyToast === "selected" ? "Copied" : "Copy selection"}
            </Button>
            {onSetTranscriptsHidden ? (
              <Button
                disabled={visibilityMutating}
                onClick={() => void applyTranscriptVisibility(selectedEntries)}
                size="sm"
                type="button"
                variant="outline"
              >
                {visibilityMode === "visible" ? <EyeOff /> : <Eye />}
                {visibilityMode === "visible"
                  ? "Hide selected"
                  : "Restore selected"}
              </Button>
            ) : null}
            {onMergeAudio && selectedAudioMergeEntries.length >= 2 ? (
              <Button
                aria-label={`Merge Audio · ${selectedAudioMergeEntries.length}`}
                disabled={audioMergeState.status === "running"}
                onClick={() => onMergeAudio(selectedAudioMergeEntries)}
                size="sm"
                type="button"
                variant="secondary"
              >
                {audioMergeState.status === "running" ? (
                  <Loader2 className="loading-spin" />
                ) : (
                  <FileAudio />
                )}
                Merge Audio · {selectedAudioMergeEntries.length}
              </Button>
            ) : null}
            {canGenerateReports ? (
              <>
                <Button
                  aria-label="Summarize selected transcripts"
                  disabled={Boolean(reportDisabledReason)}
                  title={reportDisabledReason}
                  onClick={() =>
                    handleGenerateReport(selectedEntries, "summary")
                  }
                  size="sm"
                  type="button"
                  variant="secondary"
                >
                  <FileText /> Summary
                </Button>
                <Button
                  aria-label="Create detailed report from selected transcripts"
                  disabled={Boolean(reportDisabledReason)}
                  title={reportDisabledReason}
                  onClick={() =>
                    handleGenerateReport(selectedEntries, "detailed_report")
                  }
                  size="sm"
                  type="button"
                  variant="secondary"
                >
                  <FileText /> Report
                </Button>
              </>
            ) : null}
            {onTrashTranscripts ? (
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button
                    aria-label="Move selected transcript bundles to Trash"
                    disabled={trashMutating}
                    size="sm"
                    type="button"
                    variant="destructive"
                  >
                    {trashMutating ? (
                      <Loader2 className="loading-spin" />
                    ) : (
                      <Trash2 />
                    )}
                    {trashMutating ? "Moving…" : "Trash"}
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>
                      Move {selectionCount} transcript
                      {selectionCount === 1 ? "" : "s"} to Trash?
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                      Each selected recording moves as one recoverable bundle
                      containing its audio, transcript text, metadata JSON, and
                      error text when present. Nothing is permanently deleted
                      until you empty the macOS Trash.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction
                      variant="destructive"
                      onClick={() => void handleTrashSelected()}
                    >
                      Move bundles to Trash
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            ) : null}
          </section>
        ) : null}
        {audioMergeState.status !== "idle" ? (
          <AudioMergeStatus
            state={audioMergeState}
            onOpenFolder={onOpenMergedAudioFolder}
          />
        ) : null}
        {voiceEnrollError ? (
          <p className="transcript-voice-enroll-error" role="alert">
            Voice enroll: {voiceEnrollError}
          </p>
        ) : null}
        {hasEntries ? (
          <div
            data-slot="transcript-list"
            className="transcript-entry-list transcript-entry-list--condensed"
          >
            {pageEntries.map((entry) => (
              <TranscriptEntryRow
                translation={translation}
                entry={entry}
                isPlaybackActive={
                  entry.transcript_path === playingTranscriptPath
                }
                isPlaying={
                  entry.transcript_path === playingTranscriptPath &&
                  !playbackPaused
                }
                isSelected={selectedPaths.has(entry.transcript_path)}
                key={entry.transcript_path}
                sourceLabels={sourceLabels}
                onOpenContextMenu={openContextMenu}
                onOpenFolder={onOpenFolder}
                onPlay={() => handleTogglePlayback(entry)}
                onPointerDownSelect={beginDragSelection}
                onPointerEnterSelect={continueDragSelection}
                onSelectionChange={handleTranscriptSelectionChange}
                canOpenContextMenu={canOpenContextMenu}
                canRegenerateFromContext={canRegenerateFromContext}
                visibilityMutating={visibilityMutating}
                voiceEnrolled={enrolledVoicePaths.has(entry.transcript_path)}
                onEnrollVoice={
                  enrollVoice && canEnrollVoice(entry)
                    ? () => void enrollVoice(entry)
                    : undefined
                }
              />
            ))}
          </div>
        ) : isLoadingActive ? (
          <EmptyState
            className="transcripts-empty"
            icon={Loader2}
            iconClassName="loading-spin"
            title="Loading…"
          />
        ) : hasPending ? (
          <EmptyState
            className="transcripts-empty"
            icon={CloudDownload}
            title={`${pendingCount} in iCloud — press Reload`}
            description="These captures are stored in iCloud and not on this Mac yet. Reload downloads them."
          />
        ) : visibilityMode === "hidden" ? (
          <EmptyState
            className="transcripts-empty"
            icon={EyeOff}
            title={
              effectiveSourceFilter === "dictation"
                ? "No hidden dictations for this day · Files remain on disk"
                : "No hidden transcripts for this day · Files remain on disk"
            }
            description="Hiding a capture only removes it from this list. Audio and transcript files are never deleted."
          />
        ) : (
          <EmptyState
            className="transcripts-empty"
            icon={Mic}
            title={
              effectiveSourceFilter === "dictation"
                ? "No dictations for this day"
                : "No transcripts for this day"
            }
            description={
              effectiveSourceFilter === "dictation"
                ? "Pick another day, or use a dictation hotkey to record. Choose All sources to see other captures."
                : "Captures land here automatically once recording picks up speech. Pick another day above, or start input from Capture."
            }
          />
        )}
        {pagination.pageCount > 1 ? (
          <nav className="queue-pagination" aria-label="Transcript pages">
            <span className="queue-pagination__range">
              {pagination.rangeStart}–{pagination.rangeEnd} of{" "}
              {pagination.total}
            </span>
            <div className="queue-pagination__controls">
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label="Previous Transcript page"
                disabled={pagination.page === 1}
                onClick={() => setRequestedPage(pagination.page - 1)}
              >
                <ChevronLeft data-icon="solo" />
              </Button>
              <span aria-live="polite">
                Page {pagination.page} of {pagination.pageCount}
              </span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label="Next Transcript page"
                disabled={pagination.page === pagination.pageCount}
                onClick={() => setRequestedPage(pagination.page + 1)}
              >
                <ChevronRight data-icon="solo" />
              </Button>
            </div>
          </nav>
        ) : null}
        {visibilityError ? (
          <div className="warning-banner warning-banner--danger">
            List visibility unavailable: {visibilityError}
          </div>
        ) : null}
        {trashError ? (
          <div className="warning-banner warning-banner--danger transcript-trash-error">
            Move to Trash incomplete: {trashError}
          </div>
        ) : null}
        {reportError ? (
          <div className="warning-banner warning-banner--danger transcript-report-error">
            {reportError}
          </div>
        ) : null}
        {reportRun ? (
          <section
            aria-live="polite"
            className="transcript-report-status"
            data-status={reportRun.status}
          >
            <div>
              <strong>{reportRun.message}</strong>
              <span>
                {reportRun.kind === "summary" ? "Summary" : "Detailed report"}
                {" · "}
                {reportRun.iteration > 0
                  ? `Iteration ${reportRun.iteration} of ${reportRun.max_iterations}`
                  : "Preparing"}
              </span>
            </div>
            <Button
              aria-label="Open Reports"
              onClick={onOpenReports}
              size="sm"
              type="button"
              variant="secondary"
            >
              Open Reports
            </Button>
          </section>
        ) : null}
        {playingTranscript?.audio_path ? (
          <TranscriptPlayerDock
            entry={playingTranscript}
            paused={playbackPaused}
            autoPlayNext={autoPlayNext}
            onClose={() => {
              setPlayingTranscriptPath(null);
              setPlaybackPaused(false);
            }}
            onPausedChange={setPlaybackPaused}
            onEnded={handlePlaybackEnded}
          />
        ) : null}
      </article>
      {contextMenu ? (
        <div
          className="transcript-context-menu"
          role="menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            className="transcript-context-menu__item"
            onClick={handleVisibilityFromMenu}
            role="menuitem"
            type="button"
          >
            {visibilityMode === "visible" ? (
              <EyeOff aria-hidden="true" />
            ) : (
              <Eye aria-hidden="true" />
            )}
            <span className="transcript-context-menu__label">
              {visibilityMode === "visible"
                ? "Hide selected"
                : "Restore selected"}
            </span>
          </button>
          {contextMenu.regenerationTargets.length > 0 &&
          usableRegenerationModels.length > 0 ? (
            <div
              aria-haspopup="menu"
              className="transcript-context-menu__item transcript-context-menu__item--has-submenu"
              role="menuitem"
              tabIndex={0}
            >
              <RotateCw aria-hidden="true" />
              <span className="transcript-context-menu__label">
                Regenerate with...
              </span>
              <ChevronRight
                aria-hidden="true"
                className="transcript-context-menu__chevron"
              />
              <div className="transcript-context-menu__submenu" role="menu">
                {usableRegenerationModels.map((model) => (
                  <button
                    key={model.id}
                    onClick={() => handleRegenerateFromMenu(model.id)}
                    role="menuitem"
                    type="button"
                  >
                    {formatModelLabel(model.id, usableRegenerationModels)}
                  </button>
                ))}
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function AudioMergeStatus({
  state,
  onOpenFolder,
}: {
  state: Exclude<AudioMergeUiState, { status: "idle" }>;
  onOpenFolder?: (destinationPath: string) => void | Promise<void>;
}) {
  if (state.status === "running") {
    return (
      <section
        aria-live="polite"
        className="transcript-audio-merge-status"
        data-status="running"
      >
        <div className="transcript-audio-merge-status__copy">
          <strong>Merging audio · {state.percent}%</strong>
          <span>
            {audioMergeStageLabel(state.stage)} · {state.completed_inputs} of{" "}
            {state.total_inputs} files
            {state.current_input_label ? ` · ${state.current_input_label}` : ""}
          </span>
        </div>
        <Progress aria-label="Audio merge progress" value={state.percent} />
      </section>
    );
  }
  if (state.status === "saved") {
    return (
      <section
        aria-live="polite"
        className="transcript-audio-merge-status"
        data-status="saved"
      >
        <div className="transcript-audio-merge-status__copy">
          <strong>Audio merged · {state.input_count} files</strong>
          <code title={state.destination_path}>{state.destination_path}</code>
        </div>
        {onOpenFolder ? (
          <Button
            aria-label="Open merged audio folder"
            onClick={() => onOpenFolder(state.destination_path)}
            size="sm"
            type="button"
            variant="secondary"
          >
            <FolderOpen /> Open Folder
          </Button>
        ) : null}
      </section>
    );
  }
  return (
    <section
      aria-live="polite"
      className="transcript-audio-merge-status"
      data-status="error"
    >
      <div className="transcript-audio-merge-status__copy">
        <strong>Audio merge failed</strong>
        <span>{state.message}</span>
        <span>Selection preserved. Try again.</span>
      </div>
    </section>
  );
}

function audioMergeStageLabel(stage: string) {
  switch (stage) {
    case "preparing":
      return "Preparing";
    case "converting":
      return "Converting";
    case "combining":
      return "Combining";
    case "encoding":
      return "Encoding M4A";
    case "saved":
      return "Saved";
    default:
      return stage;
  }
}

function TranscriptEntryRow({
  translation,
  canOpenContextMenu,
  canRegenerateFromContext,
  entry,
  isPlaybackActive,
  isPlaying,
  isSelected,
  sourceLabels,
  onOpenContextMenu,
  onOpenFolder,
  onPlay,
  onPointerDownSelect,
  onPointerEnterSelect,
  onSelectionChange,
  visibilityMutating,
  voiceEnrolled = false,
  onEnrollVoice,
}: {
  canOpenContextMenu: boolean;
  translation?: TranslationPreferences;
  canRegenerateFromContext: boolean;
  entry: RecentTranscript;
  isPlaybackActive: boolean;
  isPlaying: boolean;
  isSelected: boolean;
  sourceLabels: Readonly<Record<string, string>>;
  onOpenContextMenu: (entry: RecentTranscript, x: number, y: number) => void;
  onOpenFolder?: (entry: RecentTranscript) => void | Promise<void>;
  onPlay: () => void;
  onPointerDownSelect: (path: string, extendRange?: boolean) => void;
  onPointerEnterSelect: (path: string) => void;
  onSelectionChange: (path: string, selected: boolean) => void;
  visibilityMutating: boolean;
  voiceEnrolled?: boolean;
  onEnrollVoice?: () => void;
}) {
  const timestamp = formatLocalTimestamp(entry.recorded_at);
  const regenerateAvailable = Boolean(
    entry.audio_path && canRegenerateFromContext,
  );
  const folderAvailable = Boolean(onOpenFolder);
  const playLabel = isPlaying
    ? timestamp
      ? `Pause recording from ${timestamp}`
      : "Pause recording"
    : timestamp
      ? `Play recording from ${timestamp}`
      : "Play recording";
  const folderLabel = timestamp
    ? `Open recording folder for ${timestamp}`
    : "Open recording folder";

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest("a, button")) return;
    event.preventDefault();
    onPointerDownSelect(entry.transcript_path, event.shiftKey);
  };

  const handlePointerEnter = () => {
    onPointerEnterSelect(entry.transcript_path);
  };

  const handleContextMenu = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (!canOpenContextMenu) {
      return;
    }
    const target = event.target as HTMLElement | null;
    if (target?.closest("a, button")) {
      return;
    }
    event.preventDefault();
    onOpenContextMenu(entry, event.clientX, event.clientY);
  };

  return (
    <div
      className="transcript-entry transcript-entry--condensed"
      data-audio-path={
        regenerateAvailable ? (entry.audio_path ?? undefined) : undefined
      }
      data-regenerate-available={regenerateAvailable ? "true" : undefined}
      data-selected={isSelected ? "true" : undefined}
      onContextMenu={handleContextMenu}
      onPointerDown={handlePointerDown}
      onPointerEnter={handlePointerEnter}
    >
      <Checkbox
        checked={isSelected}
        disabled={visibilityMutating}
        aria-label={`Select transcript ${timestamp || entry.transcript_path}`}
        onCheckedChange={(checked) =>
          onSelectionChange(entry.transcript_path, checked === true)
        }
      />
      <a
        className="transcript-entry__timestamp"
        href={fileUrlFromPath(entry.transcript_path)}
        title={entry.transcript_path}
      >
        <span>{timestamp || entry.transcript_path}</span>
      </a>
      <TranscriptSourceBadge entry={entry} sourceLabels={sourceLabels} />
      {translation ? (
        <div className="transcript-entry__text">
          <TranslatedText text={entry.text} preferences={translation} />
        </div>
      ) : (
        <p className="transcript-entry__text">{entry.text}</p>
      )}
      <div className="transcript-entry__actions">
        <Button
          aria-label={playLabel}
          aria-pressed={isPlaybackActive}
          className="transcript-entry__play"
          disabled={!entry.audio_path}
          onClick={onPlay}
          size="icon"
          title={
            entry.audio_path
              ? isPlaying
                ? "Pause recording"
                : "Play recording"
              : "No recording file"
          }
          type="button"
          variant={isPlaybackActive ? "default" : "secondary"}
        >
          {isPlaying ? <Pause /> : <Play />}
        </Button>
        {onEnrollVoice ? (
          <Button
            className="transcript-entry__voice-enroll"
            data-slot="voice-enroll"
            disabled={voiceEnrolled}
            onClick={onEnrollVoice}
            size="sm"
            title="Add this clip to your enrolled voice"
            type="button"
            variant="outline"
          >
            {voiceEnrolled ? "Enrolled" : "This is me"}
          </Button>
        ) : null}
        {folderAvailable ? (
          <Button
            aria-label={folderLabel}
            className="transcript-entry__folder"
            onClick={() => void onOpenFolder?.(entry)}
            size="icon"
            title={entry.audio_path ?? entry.transcript_path}
            type="button"
            variant="ghost"
          >
            <FolderOpen />
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/** Past Primary/merged microphone clips with audio can be enrolled as "me". */
export function canEnrollVoice(entry: RecentTranscript): boolean {
  return (
    Boolean(entry.audio_path) &&
    (entry.source ?? "microphone") === "microphone" &&
    entry.microphone_slot !== "secondary"
  );
}

function TranscriptSourceBadge({
  entry,
  sourceLabels,
}: {
  entry: RecentTranscript;
  sourceLabels: Readonly<Record<string, string>>;
}) {
  const presentation = transcriptSourcePresentation(entry, sourceLabels);
  const Icon = presentation.icon;
  return (
    <span
      className={`transcript-source-badge transcript-source-badge--${presentation.tone}`}
      title={presentation.title}
    >
      <Icon aria-hidden="true" />
      {presentation.label}
    </span>
  );
}

export function transcriptSourceFilterOptions(
  entries: readonly RecentTranscript[],
  sourceLabels: Readonly<Record<string, string>> = {},
): TranscriptSourceFilterOption[] {
  const byId = new Map<string, TranscriptSourceFilterOption>();
  for (const entry of entries) {
    const id = transcriptSourceFilterKey(entry);
    const existing = byId.get(id);
    if (existing) {
      existing.count += 1;
      continue;
    }
    byId.set(id, {
      id,
      label: transcriptSourceLabel(entry, sourceLabels),
      count: 1,
    });
  }

  const microphone = byId.get("microphone");
  const dictation: TranscriptSourceFilterOption = {
    id: "dictation",
    label: "Dictations",
    count: byId.get("dictation")?.count ?? 0,
  };
  const microphoneDevices = [...byId.values()].filter((option) =>
    option.id.startsWith("microphone:"),
  );
  const ordered = [
    dictation,
    ...(microphone ? [microphone] : []),
    ...microphoneDevices,
    ...[...byId.values()].filter(
      (option) =>
        option.id !== "microphone" &&
        option.id !== "dictation" &&
        !option.id.startsWith("microphone:"),
    ),
  ];

  return [
    {
      id: ALL_SOURCE_FILTER,
      label: "All sources",
      count: entries.length,
    },
    ...ordered,
  ];
}

export function filterTranscriptsBySource(
  entries: readonly RecentTranscript[],
  sourceFilter: string,
): RecentTranscript[] {
  if (sourceFilter === ALL_SOURCE_FILTER) {
    return [...entries];
  }
  return entries.filter(
    (entry) => transcriptSourceFilterKey(entry) === sourceFilter,
  );
}

export function selectTranscriptPathsForEntries(
  entries: readonly RecentTranscript[],
): Set<string> {
  return new Set(entries.map((entry) => entry.transcript_path));
}

export function selectTranscriptPathsAfterShiftClick(
  entries: readonly RecentTranscript[],
  selectedPaths: ReadonlySet<string>,
  anchorPath: string | null,
  targetPath: string,
): Set<string> {
  const next = new Set(selectedPaths);
  const targetIndex = entries.findIndex(
    (entry) => entry.transcript_path === targetPath,
  );
  if (targetIndex < 0) {
    return next;
  }

  const anchorIndex = anchorPath
    ? entries.findIndex((entry) => entry.transcript_path === anchorPath)
    : -1;
  if (anchorIndex < 0) {
    next.add(targetPath);
    return next;
  }

  const start = Math.min(anchorIndex, targetIndex);
  const end = Math.max(anchorIndex, targetIndex);
  for (let index = start; index <= end; index += 1) {
    next.add(entries[index].transcript_path);
  }
  return next;
}

export function transcriptRegenerationTargetsForContextMenu(
  clickedEntry: RecentTranscript,
  entries: readonly RecentTranscript[],
  selectedPaths: ReadonlySet<string>,
): RecentTranscript[] {
  if (selectedPaths.has(clickedEntry.transcript_path)) {
    const selectedEntries = entries.filter(
      (entry) => selectedPaths.has(entry.transcript_path) && entry.audio_path,
    );
    if (selectedEntries.length > 0) {
      return selectedEntries;
    }
  }
  return clickedEntry.audio_path ? [clickedEntry] : [];
}

export function transcriptVisibilityTargetsForContextMenu(
  clickedEntry: RecentTranscript,
  entries: readonly RecentTranscript[],
  selectedPaths: ReadonlySet<string>,
): RecentTranscript[] {
  if (selectedPaths.has(clickedEntry.transcript_path)) {
    const selectedEntries = entries.filter((entry) =>
      selectedPaths.has(entry.transcript_path),
    );
    if (selectedEntries.length > 0) {
      return selectedEntries;
    }
  }
  return [clickedEntry];
}

export function shouldCopySelectedTranscriptsOnKeydown(
  event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey">,
  selectedCount: number,
): boolean {
  return (
    selectedCount > 0 &&
    !event.altKey &&
    (event.metaKey || event.ctrlKey) &&
    event.key.toLowerCase() === "c"
  );
}

function isKeyboardCopyIgnoredTarget(target: HTMLElement | null): boolean {
  if (!target) {
    return false;
  }
  return Boolean(
    target.closest("input, textarea, select, [contenteditable='true']"),
  );
}

function transcriptSourceFilterKey(entry: RecentTranscript): string {
  if (isDictationTranscript(entry)) {
    return "dictation";
  }
  if ((entry.source ?? "microphone") !== "system") {
    return entry.device_id ? `microphone:${entry.device_id}` : "microphone";
  }
  return `system:${entry.source_label ?? ""}`;
}

function regenerationModelOptions(
  models: readonly RegenerationModel[],
): RegenerationModel[] {
  return models.filter((model) =>
    ["ready", "installed", "unloaded"].includes(model.status),
  );
}

function orderRegenerationModelOptions(
  models: RegenerationModel[],
  selectedModelId: string,
): RegenerationModel[] {
  if (!selectedModelId) {
    return models;
  }
  const selectedIndex = models.findIndex(
    (model) => model.id === selectedModelId,
  );
  if (selectedIndex <= 0) {
    return models;
  }
  const selected = models[selectedIndex];
  return [
    selected,
    ...models.slice(0, selectedIndex),
    ...models.slice(selectedIndex + 1),
  ];
}

function transcriptSourcePresentation(
  entry: RecentTranscript,
  sourceLabels: Readonly<Record<string, string>>,
): {
  label: string;
  title: string;
  tone: "dictation" | "microphone" | "youtube" | "meet" | "system";
  icon: typeof Mic;
} {
  if (isDictationTranscript(entry)) {
    return {
      label: "Dict",
      title: "Dictation transcript",
      tone: "dictation",
      icon: Mic,
    };
  }
  if ((entry.source ?? "microphone") !== "system") {
    const label = transcriptSourceLabel(entry, sourceLabels);
    const mergedDeviceName =
      entry.source_label === "mic-merged" ||
      entry.source_label === "mic-priority"
        ? entry.device_name?.trim()
        : "";
    return {
      label,
      title: mergedDeviceName
        ? `${label} · ${mergedDeviceName}`
        : `${label} microphone transcript`,
      tone: "microphone",
      icon: Mic,
    };
  }

  const label = transcriptSourceLabel(entry, sourceLabels);
  if (entry.source_label === "youtube") {
    return {
      label,
      title: "YouTube system-audio transcript",
      tone: "youtube",
      icon: Youtube,
    };
  }
  if (entry.source_label === "meet") {
    return {
      label,
      title: "Google Meet system-audio transcript",
      tone: "meet",
      icon: Video,
    };
  }
  return {
    label,
    title: `${label} system-audio transcript`,
    tone: "system",
    icon: MonitorSpeaker,
  };
}

function timestampValue(value: string): number {
  const time = new Date(value).getTime();
  return Number.isNaN(time) ? Number.MAX_SAFE_INTEGER : time;
}
