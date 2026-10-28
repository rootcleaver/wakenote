import { invoke } from "@tauri-apps/api/core";
import { webhookUrlError } from "./live-webhook";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  normalizeDictationShortcut,
  normalizeEnhancedPromptShortcut,
  normalizeCycleMicrophoneShortcut,
  validateDictationHotkeys,
} from "./dictation-shortcut";
import { modelSupportsReprocessing } from "./models";
import {
  activityIssue,
  isActivityAttentionOutcome,
  isReprocessableActivityJob,
} from "./activity-attention";
import {
  audioMergeDefaultFileName,
  createAudioMergeOperationId,
  eligibleAudioMergeEntries,
} from "./audio-merge";
import {
  formatLocalTimestamp,
  formatTranscriptsForTextExport,
  isDictationTranscript,
} from "./transcript-history";
import {
  applyListVisibilityRequest,
  emptyListVisibilityState,
} from "./list-visibility";
import {
  defaultSettings,
  defaultLevelSnapshot,
  deriveProductMode,
  deriveTrayState,
  emptyQueue,
  mockModels,
  mockSnapshot,
  defaultPermissions,
} from "./app-state";
import type {
  AppSettings,
  AppSnapshot,
  AppStatus,
  MicrophoneDevice,
  MicrophoneInputLevel,
  ModelDescriptor,
  QueueJob,
  QueueSnapshot,
  RecentTranscript,
  SettingsPatch,
  TranscriptDay,
  TranscriptExportResult,
  UploadedAudio,
  AudioWaveform,
  AppPermissions,
  PermissionDragTarget,
  RecognizedSourceInfo,
  SourceCaptureStatus,
  SourcePayload,
  MeetingSummary,
  MeetingDetail,
  MeetingTranscriptionRequest,
  TranscriptionCostSnapshot,
  TranscriptionCostDetails,
  TranscriptionCostEntry,
  ManualMeetingRecordingStatus,
  LlmGenerateRequest,
  LlmProgressEvent,
  LlmReportHistoryDetail,
  LlmReportHistoryItem,
  LlmReportRunSnapshot,
  ApiKeyStatus,
  ListVisibilityState,
  SetListVisibilityRequest,
  DictionaryFileStatus,
  AudioMergeProgress,
  AudioMergeResult,
  MergeAudioRequest,
  TrashActivityJobsOutcome,
  TrashActivityJobsResult,
  TrashTranscriptsResult,
  DictationStatePayload,
} from "./types";
import type { DevFixtures } from "./dev-fixtures";
import {
  aggregateCosts,
  defaultCostFilters,
  localDateKey,
} from "./transcription-costs";

let browserCostEntries: TranscriptionCostEntry[] = [];

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

let browserSnapshot = mockSnapshot();
let browserCaptureSessionId = 0;
let browserQueuedCaptureSessionId: number | null = null;
let browserCaptureSessionTranscriptionRequested = false;
// Simulated system-audio capture state for non-Tauri dev (locked-behaviors §10).
// `startSourceCapture` flips it on (and simulates detecting that source);
// `stopSourceCapture` flips it off so dev reflects session active/inactive.
let browserSourceCapturing = false;
let browserDetectedSource: SourcePayload | null = null;
let browserManualMeetingStartedAt: number | null = null;
let browserManualMeetingGeneration = 0;
let browserManualMeetingId: string | null = null;
let browserOpenRouterApiKey: string | null = null;
let browserOpenAiApiKey: string | null = null;
let browserSonioxApiKey: string | null = null;
let browserDictationState: DictationStatePayload = {
  state: "idle",
  error: null,
};
let browserListVisibility = emptyListVisibilityState();
let browserLlmReportRunSequence = 0;
const browserLlmReportHistory: LlmReportHistoryDetail[] = [];
const browserMeetingDetails = new Map<string, MeetingDetail>();
interface BrowserLlmReportRunRecord {
  snapshot: LlmReportRunSnapshot;
  request: LlmGenerateRequest;
  timer: ReturnType<typeof setTimeout> | null;
  nextStage: number;
}
const browserLlmReportRuns = new Map<string, BrowserLlmReportRunRecord>();
const browserLlmReportRunListeners = new Set<
  (snapshot: LlmReportRunSnapshot) => void
>();
const browserVerificationPreviousStatuses = new Map<
  string,
  ModelDescriptor["status"]
>();
const defaultRecentTranscriptLimit = 50;

function permissionSnapshotFromBrowser(): AppPermissions {
  return browserSnapshot.permissions ?? defaultPermissions();
}

export function isTauriRuntime() {
  return typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);
}

export interface TextTransformResult {
  kind: "translate" | "enhance_prompt";
  text: string;
  model: string;
  target_language: AppSettings["transcription_language"] | null;
}

export async function transformText(requestId: string, request: {
  kind: TextTransformResult["kind"];
  text: string;
  target_language: AppSettings["transcription_language"] | null;
}): Promise<TextTransformResult> {
  if (!isTauriRuntime()) throw new Error("Translation and Enhanced Prompt require the WakeNote desktop app and an OpenRouter API key.");
  return invoke("transform_text", { requestId, request });
}

export async function cancelTextTransform(requestId: string): Promise<void> {
  if (isTauriRuntime()) await invoke("cancel_text_transform", { requestId });
}

function normalizeSubtitlePatch(
  patch: SettingsPatch,
  current?: AppSettings,
): SettingsPatch {
  const safePatch = { ...patch };
  const clampInteger = (value: number, maximum: number) =>
    Math.max(0, Math.min(maximum, Math.round(value)));

  if (typeof safePatch.floating_overlay_font_size_px === "number") {
    safePatch.floating_overlay_font_size_px = Math.max(
      10,
      Math.min(48, Math.round(safePatch.floating_overlay_font_size_px)),
    );
  }
  if (typeof safePatch.subtitle_duration_seconds === "number") {
    safePatch.subtitle_duration_seconds = Math.max(
      1,
      Math.min(10, Math.round(safePatch.subtitle_duration_seconds)),
    );
  }
  for (const key of [
    "subtitle_padding_horizontal_px",
    "subtitle_padding_vertical_px",
  ] as const) {
    if (typeof safePatch[key] === "number") {
      safePatch[key] = clampInteger(safePatch[key], 64);
    }
  }
  if (typeof safePatch.subtitle_border_width_px === "number") {
    safePatch.subtitle_border_width_px = clampInteger(
      safePatch.subtitle_border_width_px,
      8,
    );
  }
  if (typeof safePatch.subtitle_border_radius_px === "number") {
    safePatch.subtitle_border_radius_px = clampInteger(
      safePatch.subtitle_border_radius_px,
      48,
    );
  }
  if (typeof safePatch.subtitle_max_width_px === "number") {
    safePatch.subtitle_max_width_px = clampInteger(
      safePatch.subtitle_max_width_px,
      1_600,
    );
  }
  for (const key of [
    "subtitle_min_height_px",
    "subtitle_max_height_px",
  ] as const) {
    if (typeof safePatch[key] === "number") {
      safePatch[key] = clampInteger(safePatch[key], 1_200);
    }
  }
  if (typeof safePatch.subtitle_border_color === "string") {
    const color = safePatch.subtitle_border_color.trim();
    safePatch.subtitle_border_color = /^#[0-9a-fA-F]{6}$/.test(color)
      ? color.toLowerCase()
      : "#ffffff";
  }
  if (
    typeof safePatch.subtitle_min_height_px === "number" &&
    typeof safePatch.subtitle_max_height_px === "number" &&
    safePatch.subtitle_min_height_px > safePatch.subtitle_max_height_px
  ) {
    [safePatch.subtitle_min_height_px, safePatch.subtitle_max_height_px] = [
      safePatch.subtitle_max_height_px,
      safePatch.subtitle_min_height_px,
    ];
  }
  if (
    current &&
    typeof safePatch.subtitle_min_height_px === "number" &&
    typeof safePatch.subtitle_max_height_px !== "number" &&
    safePatch.subtitle_min_height_px > current.subtitle_max_height_px
  ) {
    safePatch.subtitle_max_height_px = safePatch.subtitle_min_height_px;
  } else if (
    current &&
    typeof safePatch.subtitle_max_height_px === "number" &&
    typeof safePatch.subtitle_min_height_px !== "number" &&
    safePatch.subtitle_max_height_px < current.subtitle_min_height_px
  ) {
    safePatch.subtitle_min_height_px = safePatch.subtitle_max_height_px;
  }
  return safePatch;
}

export async function suspendDictationShortcut(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("suspend_dictation_shortcut");
  }
}

export async function resumeDictationShortcut(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("resume_dictation_shortcut");
  }
}

export async function pressedModifierShortcut(): Promise<string | null> {
  if (!isTauriRuntime()) {
    return null;
  }
  return invoke<string | null>("pressed_modifier_shortcut");
}

export interface VoiceGateStatus {
  model_ready: boolean;
  runtime_available: boolean;
  enrolled: number;
}

let browserVoiceGate: VoiceGateStatus = {
  model_ready: false,
  runtime_available: false,
  enrolled: 0,
};

export async function loadVoiceGateStatus(): Promise<VoiceGateStatus> {
  if (!isTauriRuntime()) return browserVoiceGate;
  return invoke<VoiceGateStatus>("voice_gate_status");
}

export async function downloadVoiceGateModel(): Promise<VoiceGateStatus> {
  if (!isTauriRuntime()) {
    browserVoiceGate = { ...browserVoiceGate, model_ready: true };
    return browserVoiceGate;
  }
  return invoke<VoiceGateStatus>("voice_gate_download_model");
}

export async function enrollVoiceGate(audioPath: string): Promise<VoiceGateStatus> {
  if (!isTauriRuntime()) {
    browserVoiceGate = {
      ...browserVoiceGate,
      enrolled: browserVoiceGate.enrolled + 1,
    };
    return browserVoiceGate;
  }
  return invoke<VoiceGateStatus>("voice_gate_enroll", { audioPath });
}

export async function resetVoiceGate(): Promise<VoiceGateStatus> {
  if (!isTauriRuntime()) {
    browserVoiceGate = { ...browserVoiceGate, enrolled: 0 };
    return browserVoiceGate;
  }
  return invoke<VoiceGateStatus>("voice_gate_reset");
}

export async function loadDictationState(): Promise<DictationStatePayload> {
  if (!isTauriRuntime()) {
    return browserDictationState;
  }
  return invoke<DictationStatePayload>("dictation_state");
}

export async function cancelDictation(): Promise<DictationStatePayload> {
  if (!isTauriRuntime()) {
    browserDictationState = { state: "idle", error: null };
    return browserDictationState;
  }
  return invoke<DictationStatePayload>("cancel_dictation");
}

/**
 * Loads sample content into the browser mock so the UI can be reviewed with
 * realistic notes, meetings, reports, and an in-flight run.
 *
 * Opt-in by design: only the browser entrypoint calls this, and only outside
 * Tauri. Tests import this module directly and keep the empty mock state, so
 * seeding never changes their fixtures. No-op inside Tauri, where real backend
 * data is authoritative.
 */
export function seedBrowserFixtures(fixtures: DevFixtures) {
  if (isTauriRuntime()) {
    return;
  }

  browserSnapshot = {
    ...browserSnapshot,
    settings: { ...browserSnapshot.settings, ...fixtures.settings },
    models: fixtures.models,
    recent_transcripts: fixtures.transcripts,
  };
  browserOpenRouterApiKey = fixtures.openrouterApiKey;
  browserOpenAiApiKey = fixtures.openaiApiKey;
  browserCostEntries = fixtures.costEntries ?? [];
  browserLlmReportHistory.splice(
    0,
    browserLlmReportHistory.length,
    ...fixtures.reports,
  );
  browserLlmReportRuns.clear();
  for (const snapshot of fixtures.runs) {
    browserLlmReportRuns.set(snapshot.run_id, {
      snapshot,
      request: {
        kind: snapshot.kind,
        transcripts: [],
        run_id: snapshot.run_id,
      },
      timer: null,
      // Seeded runs are terminal or paused mid-flight; nothing left to advance.
      nextStage: browserLlmProgressStages.length,
    });
  }
  browserMeetingDetails.clear();
  for (const detail of fixtures.meetings) {
    browserMeetingDetails.set(detail.record.id, detail);
  }
}

export async function loadListVisibility(): Promise<ListVisibilityState> {
  if (!isTauriRuntime()) {
    return {
      meetings: [...browserListVisibility.meetings],
      transcripts: [...browserListVisibility.transcripts],
      report_runs: [...browserListVisibility.report_runs],
      legacy_reports: [...browserListVisibility.legacy_reports],
    };
  }
  return invoke<ListVisibilityState>("load_list_visibility");
}

export async function setListVisibility(
  request: SetListVisibilityRequest,
): Promise<ListVisibilityState> {
  if (!isTauriRuntime()) {
    browserListVisibility = applyListVisibilityRequest(
      browserListVisibility,
      request,
    );
    return loadListVisibility();
  }
  return invoke<ListVisibilityState>("set_list_visibility", { request });
}

function queueFromJobs(jobs: QueueJob[]): QueueSnapshot {
  return {
    jobs,
    pending_count: jobs.filter((job) => job.status === "pending").length,
    running_count: jobs.filter((job) => job.status === "running").length,
    failed_count: jobs.filter((job) => job.status === "failed").length,
  };
}

function isBrowserImportableAudioPath(audioPath: string) {
  return /\.(m4a|wav)$/i.test(audioPath);
}

function isBrowserUploadableAudioPath(audioPath: string) {
  return /\.(mp3|m4a|wav)$/i.test(audioPath);
}

function isUsableBrowserModel(modelId: string, models: ModelDescriptor[]) {
  const model = models.find((candidate) => candidate.id === modelId);
  return Boolean(
    model && ["ready", "installed", "unloaded"].includes(model.status),
  );
}

function settledBrowserModels(models: ModelDescriptor[]): ModelDescriptor[] {
  return models.map((model) => {
    if (model.status === "downloading") {
      return {
        ...model,
        status: "ready",
        download_progress: 100,
        download_error: null,
      };
    }

    if (model.status !== "verifying") {
      return model;
    }

    const previousStatus =
      browserVerificationPreviousStatuses.get(model.id) ?? "missing";
    browserVerificationPreviousStatuses.delete(model.id);
    if (["ready", "installed", "unloaded"].includes(previousStatus)) {
      return {
        ...model,
        status: "ready",
        download_progress: 100,
        download_error: null,
      };
    }

    return {
      ...model,
      status: previousStatus === "error" ? "error" : "missing",
      download_progress: null,
      download_error:
        previousStatus === "error"
          ? (model.download_error ?? "model verification failed")
          : null,
    };
  });
}

function browserLevelSnapshot(activeCapture: boolean): AppStatus["level"] {
  if (!activeCapture) {
    return defaultLevelSnapshot();
  }

  const now = Date.now();
  const currentDbfs = Math.max(
    -78,
    Math.min(-62, -70 + Math.sin(now / 320) * 7 + Math.sin(now / 95) * 2),
  );

  return {
    current_dbfs: currentDbfs,
    peak_dbfs: Math.max(
      currentDbfs,
      browserSnapshot.status?.level.peak_dbfs ?? currentDbfs,
    ),
    noise_floor_dbfs: -73,
    suggested_threshold_dbfs: -61,
  };
}

function maybeQueueBrowserCapture(
  settings: AppSettings,
  queue: QueueSnapshot,
  status: AppStatus,
) {
  if (
    browserCaptureSessionId === 0 ||
    browserQueuedCaptureSessionId === browserCaptureSessionId ||
    !browserCaptureSessionTranscriptionRequested ||
    status.tray_state !== "recording"
  ) {
    return queue;
  }

  const audioPath = `${settings.save_root}/browser-capture-${String(
    browserCaptureSessionId,
  ).padStart(3, "0")}.${settings.audio_format}`;
  browserQueuedCaptureSessionId = browserCaptureSessionId;
  if (queue.jobs.some((job) => job.audio_path === audioPath)) {
    return queue;
  }

  return queueFromJobs([
    ...queue.jobs,
    {
      id: queue.jobs.length + 1,
      audio_path: audioPath,
      model_id: settings.selected_model,
      status: "pending",
      error: null,
    },
  ]);
}

function statusFrom(
  settings: AppSettings,
  queue: QueueSnapshot,
  liveInputActive = browserSnapshot.status?.live_input_active ?? false,
): AppStatus {
  const mode = deriveProductMode(settings);
  const canCapture =
    mode === "recording_and_transcription" || mode === "recording_only";
  const activeCapture = canCapture && liveInputActive;
  const level = browserLevelSnapshot(activeCapture);
  const isRecording =
    activeCapture && level.current_dbfs >= settings.threshold_dbfs;

  return {
    mode,
    tray_state: deriveTrayState(
      mode,
      queue.running_count > 0,
      queue.jobs.some(
        (job) =>
          job.is_read !== true && activityIssue(job)?.severity === "error",
      ),
      activeCapture,
      isRecording,
    ),
    live_input_active: activeCapture,
    active_model: settings.selected_model,
    active_microphone: settings.selected_microphone_label,
    microphone_warning: browserSnapshot.status?.microphone_warning ?? null,
    runtime_warning: browserSnapshot.status?.runtime_warning ?? null,
    threshold_dbfs: settings.threshold_dbfs,
    level,
    microphone_captures: settings.capture_microphones.map(
      (microphone, index) => ({
        slot: index === 0 ? "primary" : "secondary",
        device_id: microphone.id,
        label: microphone.label,
        active: activeCapture,
        reconnecting: false,
        warning: null,
        level,
      }),
    ),
    queue,
  };
}

export async function loadSnapshot(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const currentQueue = browserSnapshot.queue ?? emptyQueue();
    const models = settledBrowserModels(browserSnapshot.models ?? mockModels());
    const currentStatus = statusFrom(settings, currentQueue);
    const queue = maybeQueueBrowserCapture(
      settings,
      currentQueue,
      currentStatus,
    );
    browserSnapshot = {
      ...browserSnapshot,
      settings,
      models,
      queue,
      status: statusFrom(settings, queue),
      openrouter_key_configured: Boolean(browserOpenRouterApiKey),
      openai_key_configured: Boolean(browserOpenAiApiKey),
      soniox_key_configured: Boolean(browserSonioxApiKey),
    };
    return browserSnapshot;
  }

  const [
    settings,
    status,
    microphones,
    microphoneInputLevels,
    models,
    queue,
    permissions,
    openRouterKeyStatus,
    openAiKeyStatus,
    sonioxKeyStatus,
    dictionaryFileStatus,
  ] = await Promise.all([
    invoke<AppSettings>("get_settings"),
    invoke<AppStatus>("app_status"),
    invoke<MicrophoneDevice[]>("list_microphones"),
    invoke<MicrophoneInputLevel[]>("microphone_input_levels"),
    invoke<ModelDescriptor[]>("list_models"),
    invoke<QueueSnapshot>("queue_snapshot"),
    invoke<AppPermissions>("permission_snapshot"),
    invoke<ApiKeyStatus>("openrouter_key_status"),
    invoke<ApiKeyStatus>("openai_key_status"),
    invoke<ApiKeyStatus>("soniox_key_status"),
    invoke<DictionaryFileStatus>("dictionary_file_status"),
  ]);

  return {
    settings,
    status,
    microphones,
    microphone_input_levels: microphoneInputLevels,
    models,
    queue,
    permissions,
    recent_transcripts: [],
    openrouter_key_configured: openRouterKeyStatus.configured,
    openai_key_configured: openAiKeyStatus.configured,
    soniox_key_configured: sonioxKeyStatus.configured,
    dictionary_file_status: dictionaryFileStatus,
  };
}

export async function loadDictionaryFileStatus(): Promise<DictionaryFileStatus> {
  if (!isTauriRuntime()) {
    return { ...browserSnapshot.dictionary_file_status };
  }
  return invoke<DictionaryFileStatus>("dictionary_file_status");
}

export async function openDictionaryFile(): Promise<AppSnapshot> {
  if (isTauriRuntime()) {
    await invoke("open_dictionary_file");
  }
  return loadSnapshot();
}

export async function reloadDictionaryFile(): Promise<AppSnapshot> {
  if (isTauriRuntime()) {
    await invoke<DictionaryFileStatus>("reload_dictionary_file");
  }
  return loadSnapshot();
}

export async function loadPermissionSnapshot(): Promise<AppPermissions> {
  if (!isTauriRuntime()) {
    return permissionSnapshotFromBrowser();
  }

  return invoke<AppPermissions>("permission_snapshot");
}

export function loadPermissions(): Promise<AppPermissions> {
  return loadPermissionSnapshot();
}

export async function loadRecentTranscripts(
  limit = defaultRecentTranscriptLimit,
): Promise<RecentTranscript[]> {
  if (!isTauriRuntime()) {
    return browserSnapshot.recent_transcripts ?? [];
  }

  return invoke<RecentTranscript[]>("recent_transcripts", { limit });
}

export async function loadRecentDictations(
  limit = 10,
): Promise<RecentTranscript[]> {
  if (!isTauriRuntime()) {
    const visibility = await loadListVisibility();
    const hidden = new Set(visibility.transcripts);
    return (browserSnapshot.recent_transcripts ?? [])
      .filter(
        (entry) =>
          isDictationTranscript(entry) && !hidden.has(entry.transcript_path),
      )
      .sort(
        (left, right) =>
          Date.parse(right.recorded_at) - Date.parse(left.recorded_at) ||
          right.transcript_path.localeCompare(left.transcript_path),
      )
      .slice(0, Math.max(0, Math.min(5000, Math.trunc(limit))));
  }
  return invoke<RecentTranscript[]>("recent_dictations", { limit });
}

function transcriptDayFromBrowser(transcript: RecentTranscript): string {
  return formatLocalTimestamp(transcript.recorded_at).slice(0, 10);
}

// Returns days in ascending order; the backend (BTreeMap) and this browser
// fallback agree. Consumers that need newest-first should sort themselves
// (TranscriptsView derives its own ordering).
export async function loadTranscriptDays(): Promise<TranscriptDay[]> {
  if (!isTauriRuntime()) {
    const counts = new Map<string, number>();
    for (const transcript of browserSnapshot.recent_transcripts ?? []) {
      const day = transcriptDayFromBrowser(transcript);
      if (day) {
        counts.set(day, (counts.get(day) ?? 0) + 1);
      }
    }
    return [...counts]
      .map(([day, count]) => ({ day, count }))
      .sort((left, right) => left.day.localeCompare(right.day));
  }

  return invoke<TranscriptDay[]>("transcript_days");
}

// `download` lets an explicit reload fetch iCloud-evicted sidecars; the default
// (on-navigation load) leaves them on the server and shows only local files.
export async function loadTranscriptsForDay(
  day: string,
  download = false,
): Promise<RecentTranscript[]> {
  if (!isTauriRuntime()) {
    return (browserSnapshot.recent_transcripts ?? []).filter(
      (transcript) => transcriptDayFromBrowser(transcript) === day,
    );
  }

  return invoke<RecentTranscript[]>("transcripts_for_day", { day, download });
}

export async function rebuildTranscriptDayIndex(
  day: string,
  download = true,
): Promise<RecentTranscript[]> {
  if (!isTauriRuntime()) {
    return loadTranscriptsForDay(day, download);
  }

  return invoke<RecentTranscript[]>("rebuild_transcript_day_index", {
    day,
    download,
  });
}

function browserTranscriptSourceLabels(): Record<string, string> {
  const settings = browserSnapshot.settings ?? defaultSettings();
  return Object.fromEntries(
    settings.custom_sources.map((source) => [source.id, source.label]),
  );
}

function downloadBrowserTranscriptFile(
  fileName: string,
  entries: readonly RecentTranscript[],
) {
  if (
    typeof document === "undefined" ||
    typeof URL.createObjectURL !== "function"
  ) {
    return;
  }
  const content = formatTranscriptsForTextExport(
    entries,
    browserTranscriptSourceLabels(),
  );
  const url = URL.createObjectURL(
    new Blob([content], { type: "text/plain;charset=utf-8" }),
  );
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.click();
  URL.revokeObjectURL(url);
}

function sortedBrowserTranscriptsForDay(day: string): RecentTranscript[] {
  return (browserSnapshot.recent_transcripts ?? [])
    .filter((transcript) => transcriptDayFromBrowser(transcript) === day)
    .sort((left, right) => left.recorded_at.localeCompare(right.recorded_at));
}

export async function exportAllTranscriptDays(): Promise<TranscriptExportResult | null> {
  if (!isTauriRuntime()) {
    const days = await loadTranscriptDays();
    let transcriptCount = 0;
    let dayCount = 0;
    for (const { day } of days) {
      const entries = sortedBrowserTranscriptsForDay(day);
      if (entries.length === 0) continue;
      downloadBrowserTranscriptFile(`WakeNote-${day}.txt`, entries);
      transcriptCount += entries.length;
      dayCount += 1;
    }
    if (dayCount === 0) {
      throw new Error("No transcripts to export");
    }
    return {
      destination_path: "Downloads",
      day_count: dayCount,
      transcript_count: transcriptCount,
    };
  }

  const destinationParent = await open({
    directory: true,
    multiple: false,
    title: "Export all transcript days",
  });
  if (typeof destinationParent !== "string") return null;
  return invoke<TranscriptExportResult>("export_all_transcript_days", {
    destinationParent,
  });
}

export async function exportTranscriptDay(
  day: string,
): Promise<TranscriptExportResult | null> {
  if (!isTauriRuntime()) {
    const entries = sortedBrowserTranscriptsForDay(day);
    if (entries.length === 0) {
      throw new Error(`No transcripts to export for ${day}`);
    }
    const fileName = `WakeNote-${day}.txt`;
    downloadBrowserTranscriptFile(fileName, entries);
    return {
      destination_path: fileName,
      day_count: 1,
      transcript_count: entries.length,
    };
  }

  const destinationPath = await save({
    defaultPath: `WakeNote-${day}.txt`,
    filters: [{ name: "Text", extensions: ["txt"] }],
  });
  if (!destinationPath) return null;
  return invoke<TranscriptExportResult>("export_transcript_day", {
    day,
    destinationPath,
    // The native save dialog confirms replacement before returning an existing path.
    overwrite: true,
  });
}

export async function saveSettingsPatch(
  patch: SettingsPatch,
): Promise<AppSnapshot> {
  const normalizedPatch = normalizeSubtitlePatch(patch);
  if (!isTauriRuntime()) {
    const previousSettings = browserSnapshot.settings ?? defaultSettings();
    const models = browserSnapshot.models ?? mockModels();
    const safePatch = normalizeSubtitlePatch(patch, previousSettings);
    if (
      typeof safePatch.selected_model === "string" &&
      !isUsableBrowserModel(safePatch.selected_model, models)
    ) {
      delete safePatch.selected_model;
    }
    if (typeof safePatch.save_root === "string") {
      safePatch.save_root_confirmed = safePatch.save_root.trim().length > 0;
    }
    if (typeof safePatch.llm_max_iterations === "number") {
      safePatch.llm_max_iterations = Math.max(
        1,
        Math.min(30, Math.round(safePatch.llm_max_iterations)),
      );
    }
    if (typeof safePatch.dictation_shortcut === "string") {
      safePatch.dictation_shortcut = normalizeDictationShortcut(
        safePatch.dictation_shortcut,
      );
    }
    if (typeof safePatch.dictation_toggle_shortcut === "string") {
      safePatch.dictation_toggle_shortcut =
        safePatch.dictation_toggle_shortcut.trim()
          ? normalizeDictationShortcut(safePatch.dictation_toggle_shortcut)
          : "";
    }
    if (typeof safePatch.enhance_prompt_shortcut === "string") {
      safePatch.enhance_prompt_shortcut = normalizeEnhancedPromptShortcut(
        safePatch.enhance_prompt_shortcut,
      );
    }
    if (typeof safePatch.cycle_microphone_shortcut === "string") {
      safePatch.cycle_microphone_shortcut = normalizeCycleMicrophoneShortcut(safePatch.cycle_microphone_shortcut);
    }
    let queue = browserSnapshot.queue ?? emptyQueue();
    if (
      safePatch.recording_enabled === false ||
      safePatch.pause_all === true ||
      safePatch.transcription_enabled === false
    ) {
      queue = maybeQueueBrowserCapture(
        previousSettings,
        queue,
        statusFrom(previousSettings, queue),
      );
    }

    const settings = { ...previousSettings, ...safePatch };
    if (safePatch.live_transcription_webhook) {
      settings.live_transcription_webhook = {
        ...safePatch.live_transcription_webhook,
        endpoint_url: safePatch.live_transcription_webhook.endpoint_url.trim(),
      };
    }
    const webhookError = webhookUrlError(settings.live_transcription_webhook);
    if (webhookError) throw new Error(webhookError);
    validateDictationHotkeys(settings);
    const dictionaryFileStatus = safePatch.dictionary
      ? {
          ...browserSnapshot.dictionary_file_status,
          revision: `browser-${Date.now()}`,
          error: null,
          error_line: null,
          in_sync: true,
        }
      : browserSnapshot.dictionary_file_status;
    browserSnapshot = {
      ...browserSnapshot,
      settings,
      models,
      status: statusFrom(settings, queue),
      queue,
      openrouter_key_configured: Boolean(browserOpenRouterApiKey),
      openai_key_configured: Boolean(browserOpenAiApiKey),
      soniox_key_configured: Boolean(browserSonioxApiKey),
      dictionary_file_status: dictionaryFileStatus,
    };
    return browserSnapshot;
  }

  await invoke<AppSettings>("update_settings", { patch: normalizedPatch });
  return loadSnapshot();
}

export async function previewSubtitle(patch: SettingsPatch): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }
  await invoke("preview_subtitle", { patch: normalizeSubtitlePatch(patch) });
}

export async function setMicrophoneInputVolume(
  deviceId: string,
  volumePercent: number,
): Promise<AppSnapshot> {
  const normalized = Math.max(0, Math.min(100, Math.round(volumePercent)));
  if (!isTauriRuntime()) {
    browserSnapshot = {
      ...browserSnapshot,
      microphone_input_levels: browserSnapshot.microphone_input_levels.map(
        (level) =>
          level.device_id === deviceId
            ? { ...level, volume_percent: normalized }
            : level,
      ),
    };
    return loadSnapshot();
  }

  await invoke<MicrophoneInputLevel[]>("set_microphone_input_volume", {
    deviceId,
    volumePercent: normalized,
  });
  return loadSnapshot();
}

export async function saveOpenRouterApiKey(
  apiKey: string,
): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      throw new Error("OpenRouter API key cannot be blank");
    }
    browserOpenRouterApiKey = trimmed;
    browserSnapshot = {
      ...browserSnapshot,
      openrouter_key_configured: true,
    };
    return loadSnapshot();
  }

  await invoke<ApiKeyStatus>("save_openrouter_api_key", { apiKey });
  return loadSnapshot();
}

export async function deleteOpenRouterApiKey(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserOpenRouterApiKey = null;
    browserSnapshot = {
      ...browserSnapshot,
      openrouter_key_configured: false,
    };
    return loadSnapshot();
  }

  await invoke<ApiKeyStatus>("delete_openrouter_api_key");
  return loadSnapshot();
}

export async function saveOpenAiApiKey(apiKey: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      throw new Error("OpenAI API key cannot be blank");
    }
    browserOpenAiApiKey = trimmed;
    browserSnapshot = {
      ...browserSnapshot,
      openai_key_configured: true,
    };
    return loadSnapshot();
  }

  await invoke<ApiKeyStatus>("save_openai_api_key", { apiKey });
  return loadSnapshot();
}

export async function deleteOpenAiApiKey(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserOpenAiApiKey = null;
    browserSnapshot = {
      ...browserSnapshot,
      openai_key_configured: false,
    };
    return loadSnapshot();
  }

  await invoke<ApiKeyStatus>("delete_openai_api_key");
  return loadSnapshot();
}

export async function saveSonioxApiKey(apiKey: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      throw new Error("Soniox API key cannot be blank");
    }
    browserSonioxApiKey = trimmed;
    browserSnapshot = {
      ...browserSnapshot,
      soniox_key_configured: true,
    };
    return loadSnapshot();
  }

  await invoke<ApiKeyStatus>("save_soniox_api_key", { apiKey });
  return loadSnapshot();
}

export async function deleteSonioxApiKey(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserSonioxApiKey = null;
    browserSnapshot = {
      ...browserSnapshot,
      soniox_key_configured: false,
    };
    return loadSnapshot();
  }

  await invoke<ApiKeyStatus>("delete_soniox_api_key");
  return loadSnapshot();
}

const browserLlmProgressStages: ReadonlyArray<{
  stage: LlmProgressEvent["stage"];
  message: string;
}> = [
  { stage: "preparing", message: "Preparing transcript context" },
  { stage: "generating", message: "Drafting report" },
  { stage: "evaluating", message: "Checking success criteria" },
  { stage: "saving", message: "Saving report" },
];

function copyLlmGenerateRequest(
  request: LlmGenerateRequest,
): LlmGenerateRequest {
  return {
    ...request,
    transcripts: request.transcripts.map((transcript) => ({ ...transcript })),
  };
}

function copyLlmReportRunSnapshot(
  snapshot: LlmReportRunSnapshot,
): LlmReportRunSnapshot {
  return {
    ...snapshot,
    progress: snapshot.progress.map((event) => ({ ...event })),
    usage: snapshot.usage ? { ...snapshot.usage } : null,
  };
}

function publishBrowserLlmReportRun(record: BrowserLlmReportRunRecord) {
  const snapshot = copyLlmReportRunSnapshot(record.snapshot);
  for (const listener of browserLlmReportRunListeners) {
    listener(snapshot);
  }
}

function activeBrowserLlmReportRun(): BrowserLlmReportRunRecord | undefined {
  return [...browserLlmReportRuns.values()].find((record) =>
    ["queued", "running", "stopping"].includes(record.snapshot.status),
  );
}

function browserLlmProgressEvent(
  record: BrowserLlmReportRunRecord,
  stage: LlmProgressEvent["stage"],
  message: string,
  detail: string | null = null,
): LlmProgressEvent {
  return {
    run_id: record.snapshot.run_id,
    stage,
    iteration: stage === "preparing" ? 0 : 1,
    max_iterations: record.snapshot.max_iterations,
    message,
    detail,
  };
}

function updateBrowserLlmRun(
  record: BrowserLlmReportRunRecord,
  updates: Partial<LlmReportRunSnapshot>,
  progress?: LlmProgressEvent,
) {
  const now = new Date().toISOString();
  record.snapshot = {
    ...record.snapshot,
    ...updates,
    revision: record.snapshot.revision + 1,
    updated_at: now,
    progress: progress
      ? [...record.snapshot.progress, progress]
      : record.snapshot.progress,
  };
  publishBrowserLlmReportRun(record);
}

function completeBrowserLlmReportRun(record: BrowserLlmReportRunRecord) {
  if (record.snapshot.status !== "running") {
    return;
  }
  const settings = browserSnapshot.settings ?? defaultSettings();
  const label =
    record.request.kind === "summary" ? "Summary" : "Detailed report";
  const content = `# ${label}\n\nGenerated from ${record.request.transcripts.length} transcript${record.request.transcripts.length === 1 ? "" : "s"}.`;
  const promptTokens = Math.max(
    1,
    Math.round(
      record.request.transcripts.reduce(
        (total, transcript) => total + transcript.text.length,
        0,
      ) / 4,
    ),
  );
  const completionTokens = Math.max(1, Math.round(content.length / 4));
  const usage = {
    request_count: 2,
    prompt_tokens: promptTokens,
    completion_tokens: completionTokens,
    total_tokens: promptTokens + completionTokens,
    cost: Number(((promptTokens + completionTokens) * 0.000001).toFixed(6)),
  };
  const reportId = `browser-${Date.now()}-${browserLlmReportRunSequence}-${record.request.kind}`;
  const fileName = `${reportId}.md`;
  const reportPath = `${settings.save_root}/reports/${fileName}`;
  const qualityFeedback =
    "Browser preview completed its simulated quality check.";
  const progress = browserLlmProgressEvent(
    record,
    "completed",
    "Success criteria met",
    qualityFeedback,
  );
  updateBrowserLlmRun(
    record,
    {
      status: "completed",
      stage: "completed",
      iteration: 1,
      message: progress.message,
      detail: qualityFeedback,
      finished_at: new Date().toISOString(),
      report_id: reportId,
      report_path: reportPath,
      completion_reason: "success_criteria_met",
      success_criteria_met: true,
      quality_feedback: qualityFeedback,
      usage,
    },
    progress,
  );
  record.timer = null;
  browserLlmReportHistory.unshift({
    item: {
      report_id: reportId,
      kind: record.request.kind,
      created_at: record.snapshot.created_at,
      file_name: fileName,
      report_path: reportPath,
      model: record.snapshot.model,
      iterations_used: 1,
      max_iterations: record.snapshot.max_iterations,
      success_criteria_met: true,
      completion_reason: "success_criteria_met",
      quality_feedback: qualityFeedback,
      selected_count: record.snapshot.selected_count,
      date_range: record.snapshot.date_range || null,
      usage,
      legacy: false,
    },
    content,
  });
}

function advanceBrowserLlmReportRun(runId: string) {
  const record = browserLlmReportRuns.get(runId);
  if (!record || !["queued", "running"].includes(record.snapshot.status)) {
    return;
  }
  const stage = browserLlmProgressStages[record.nextStage];
  if (!stage) {
    completeBrowserLlmReportRun(record);
    return;
  }
  const progress = browserLlmProgressEvent(record, stage.stage, stage.message);
  updateBrowserLlmRun(
    record,
    {
      status: "running",
      stage: stage.stage,
      started_at: record.snapshot.started_at ?? new Date().toISOString(),
      iteration: progress.iteration,
      message: stage.message,
      detail: null,
    },
    progress,
  );
  record.nextStage += 1;
  record.timer = setTimeout(() => advanceBrowserLlmReportRun(runId), 10);
}

function createBrowserLlmReportRun(
  request: LlmGenerateRequest,
  parentRunId: string | null,
): LlmReportRunSnapshot {
  if (!browserOpenRouterApiKey) {
    throw new Error("OpenRouter API key is not configured");
  }
  if (request.transcripts.length === 0) {
    throw new Error("Select at least one transcript");
  }
  const active = activeBrowserLlmReportRun();
  if (active) {
    throw new Error(`Report run ${active.snapshot.run_id} is already active`);
  }
  const settings = browserSnapshot.settings ?? defaultSettings();
  const runId = nextLlmReportRunId();
  const now = new Date().toISOString();
  const privateRequest = copyLlmGenerateRequest({
    ...request,
    run_id: runId,
  });
  const record: BrowserLlmReportRunRecord = {
    snapshot: {
      run_id: runId,
      parent_run_id: parentRunId,
      revision: 1,
      status: "queued",
      stage: null,
      kind: privateRequest.kind,
      created_at: now,
      updated_at: now,
      started_at: null,
      finished_at: null,
      iteration: 0,
      max_iterations: settings.llm_max_iterations,
      message: "Queued for report generation",
      detail: null,
      error: null,
      progress: [],
      model: settings.openrouter_model,
      selected_count: privateRequest.transcripts.length,
      date_range: browserReportDateRange(privateRequest.transcripts) ?? "",
      report_id: null,
      report_path: null,
      completion_reason: null,
      success_criteria_met: null,
      quality_feedback: null,
      usage: null,
    },
    request: privateRequest,
    timer: null,
    nextStage: 0,
  };
  browserLlmReportRuns.set(runId, record);
  publishBrowserLlmReportRun(record);
  record.timer = setTimeout(() => advanceBrowserLlmReportRun(runId), 10);
  return copyLlmReportRunSnapshot(record.snapshot);
}

export async function startLlmReport(
  request: LlmGenerateRequest,
): Promise<LlmReportRunSnapshot> {
  if (!isTauriRuntime()) {
    return createBrowserLlmReportRun(request, null);
  }
  return invoke<LlmReportRunSnapshot>("start_llm_report", { request });
}

export async function listLlmReportRuns(): Promise<LlmReportRunSnapshot[]> {
  if (!isTauriRuntime()) {
    return [...browserLlmReportRuns.values()]
      .map((record) => copyLlmReportRunSnapshot(record.snapshot))
      .sort((left, right) => right.created_at.localeCompare(left.created_at));
  }
  return invoke<LlmReportRunSnapshot[]>("list_llm_report_runs");
}

export async function cancelLlmReport(
  runId: string,
): Promise<LlmReportRunSnapshot> {
  if (isTauriRuntime()) {
    return invoke<LlmReportRunSnapshot>("cancel_llm_report", { runId });
  }
  const record = browserLlmReportRuns.get(runId);
  if (!record) {
    throw new Error(`Report run not found: ${runId}`);
  }
  if (!["queued", "running"].includes(record.snapshot.status)) {
    throw new Error(`Report run ${runId} is not active`);
  }
  if (record.timer) {
    clearTimeout(record.timer);
    record.timer = null;
  }
  updateBrowserLlmRun(record, {
    status: "stopping",
    message: "Stopping report generation",
    detail: null,
  });
  const stopping = copyLlmReportRunSnapshot(record.snapshot);
  record.timer = setTimeout(() => {
    if (record.snapshot.status !== "stopping") {
      return;
    }
    const progress = browserLlmProgressEvent(
      record,
      "cancelled",
      "Report generation cancelled",
    );
    updateBrowserLlmRun(
      record,
      {
        status: "cancelled",
        stage: "cancelled",
        message: progress.message,
        detail: null,
        error: null,
        finished_at: new Date().toISOString(),
      },
      progress,
    );
    record.timer = null;
  }, 0);
  return stopping;
}

export async function retryLlmReport(
  runId: string,
): Promise<LlmReportRunSnapshot> {
  if (isTauriRuntime()) {
    return invoke<LlmReportRunSnapshot>("retry_llm_report", { runId });
  }
  const source = browserLlmReportRuns.get(runId);
  if (!source) {
    throw new Error(`Report run not found: ${runId}`);
  }
  if (!["cancelled", "failed", "completed"].includes(source.snapshot.status)) {
    throw new Error(`Report run ${runId} is not finished`);
  }
  return createBrowserLlmReportRun(
    copyLlmGenerateRequest(source.request),
    runId,
  );
}

export async function subscribeLlmReportRuns(
  onRun: (snapshot: LlmReportRunSnapshot) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    browserLlmReportRunListeners.add(onRun);
    return () => browserLlmReportRunListeners.delete(onRun);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<LlmReportRunSnapshot>("llm-report-run-updated", (event) =>
    onRun(event.payload),
  );
}

export async function listLlmReportHistory(): Promise<LlmReportHistoryItem[]> {
  if (!isTauriRuntime()) {
    return browserLlmReportHistory.map(({ item }) => ({ ...item }));
  }
  return invoke<LlmReportHistoryItem[]>("list_llm_report_history");
}

export async function loadLlmReportHistoryDetail(
  reportId: string,
): Promise<LlmReportHistoryDetail> {
  if (!isTauriRuntime()) {
    const detail = browserLlmReportHistory.find(
      ({ item }) => item.report_id === reportId,
    );
    if (!detail) {
      throw new Error(`Report not found: ${reportId}`);
    }
    return { item: { ...detail.item }, content: detail.content };
  }
  return invoke<LlmReportHistoryDetail>("load_llm_report_history_detail", {
    reportId,
  });
}

export async function downloadLlmReport(
  reportId: string,
  fileName: string,
): Promise<string | null> {
  if (!isTauriRuntime()) {
    const detail = await loadLlmReportHistoryDetail(reportId);
    if (
      typeof document === "undefined" ||
      typeof URL.createObjectURL !== "function"
    ) {
      return detail.item.file_name;
    }
    const url = URL.createObjectURL(
      new Blob([detail.content], { type: "text/markdown;charset=utf-8" }),
    );
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = detail.item.file_name;
    anchor.click();
    URL.revokeObjectURL(url);
    return detail.item.file_name;
  }

  const destinationPath = await save({
    defaultPath: fileName,
    filters: [{ name: "Markdown", extensions: ["md"] }],
  });
  if (!destinationPath) {
    return null;
  }
  return invoke<string>("export_llm_report", { reportId, destinationPath });
}

function browserReportDateRange(
  transcripts: readonly RecentTranscript[],
): string | null {
  const timestamps = transcripts
    .map((transcript) => transcript.recorded_at)
    .filter(Boolean)
    .sort();
  if (timestamps.length === 0) return null;
  if (timestamps[0] === timestamps[timestamps.length - 1]) return timestamps[0];
  return `${timestamps[0]} - ${timestamps[timestamps.length - 1]}`;
}

function nextLlmReportRunId(): string {
  browserLlmReportRunSequence += 1;
  return `llm-report-${Date.now()}-${browserLlmReportRunSequence}`;
}

export async function enqueueBacklog(saveRoot: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const audioPath = `${saveRoot}/20260506/230709.m4a`;
    if (
      browserSnapshot.queue.jobs.some((job) => job.audio_path === audioPath)
    ) {
      return browserSnapshot;
    }

    const id = browserSnapshot.queue.jobs.length + 1;
    const queue: QueueSnapshot = {
      jobs: [
        ...browserSnapshot.queue.jobs,
        {
          id,
          audio_path: audioPath,
          model_id: settings.selected_model,
          status: "pending",
          error: null,
        },
      ],
      pending_count: browserSnapshot.queue.pending_count + 1,
      running_count: browserSnapshot.queue.running_count,
      failed_count: browserSnapshot.queue.failed_count,
    };
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("enqueue_backlog", { saveRoot });
  return loadSnapshot();
}

export async function enqueueAudioFiles(
  audioPaths: string[],
): Promise<AppSnapshot> {
  if (audioPaths.length === 0) {
    return loadSnapshot();
  }

  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const firstId = browserSnapshot.queue.jobs.length + 1;
    const queuedPaths = new Set(
      browserSnapshot.queue.jobs.map((job) => job.audio_path),
    );
    const importedJobs = audioPaths
      .filter((audioPath) => {
        if (!isBrowserImportableAudioPath(audioPath)) {
          return false;
        }
        if (queuedPaths.has(audioPath)) {
          return false;
        }
        queuedPaths.add(audioPath);
        return true;
      })
      .map((audioPath, index) => ({
        id: firstId + index,
        audio_path: audioPath,
        model_id: settings.selected_model,
        status: "pending" as const,
        error: null,
      }));
    if (importedJobs.length === 0) {
      return browserSnapshot;
    }

    const queue: QueueSnapshot = {
      jobs: [...browserSnapshot.queue.jobs, ...importedJobs],
      pending_count: browserSnapshot.queue.pending_count + importedJobs.length,
      running_count: browserSnapshot.queue.running_count,
      failed_count: browserSnapshot.queue.failed_count,
    };
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  for (const audioPath of audioPaths) {
    await invoke<QueueSnapshot>("enqueue_audio_file", { audioPath });
  }
  return loadSnapshot();
}

export async function chooseAudioFiles(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return browserSnapshot;
  }

  const selected = await open({
    multiple: true,
    title: "Choose Audio Files",
    filters: [
      {
        name: "Audio",
        extensions: ["m4a", "wav"],
      },
    ],
  });
  if (!Array.isArray(selected)) {
    return loadSnapshot();
  }

  return enqueueAudioFiles(selected);
}

export async function requestMicrophonePermission(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserSnapshot = {
      ...browserSnapshot,
      permissions: {
        ...(browserSnapshot.permissions ?? defaultPermissions()),
        microphone: {
          status: "granted",
          label: "Allowed",
          detail: "WakeNote can use the microphone.",
          can_request: false,
          can_open_settings: true,
        },
      },
    };
    return loadSnapshot();
  }

  await invoke<AppPermissions>("request_microphone_permission");
  return loadSnapshot();
}

export async function requestAccessibilityPermission(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserSnapshot = {
      ...browserSnapshot,
      permissions: {
        ...(browserSnapshot.permissions ?? defaultPermissions()),
        accessibility: {
          status: "granted",
          label: "Allowed",
          detail: "WakeNote can type Dictation results into the focused app.",
          can_request: false,
          can_open_settings: true,
        },
      },
    };
    return loadSnapshot();
  }

  await invoke<AppPermissions>("request_accessibility_permission");
  return loadSnapshot();
}

export async function openAccessibilityPermissionSettings(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return loadSnapshot();
  }

  await invoke("open_accessibility_permission_settings");
  return loadSnapshot();
}

export async function openMicrophonePermissionSettings(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return loadSnapshot();
  }

  await invoke("open_microphone_permission_settings");
  return loadSnapshot();
}

export async function requestScreenRecordingPermission(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserSnapshot = {
      ...browserSnapshot,
      permissions: {
        ...(browserSnapshot.permissions ?? defaultPermissions()),
        screen_recording: {
          status: "granted",
          label: "Allowed",
          detail:
            "WakeNote can capture system audio (Google Meet, Zoom, YouTube).",
          can_request: false,
          can_open_settings: true,
        },
      },
    };
    return loadSnapshot();
  }

  await invoke<AppPermissions>("request_screen_recording_permission");
  return loadSnapshot();
}

export async function openScreenRecordingSettings(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return loadSnapshot();
  }

  await invoke("open_screen_recording_settings");
  return loadSnapshot();
}

async function openPermissionDragShelf(
  target: PermissionDragTarget,
): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return loadSnapshot();
  }

  await invoke<AppPermissions>("open_permission_drag_shelf", { target });
  return loadSnapshot();
}

export function openAccessibilityPermissionAssistant(): Promise<AppSnapshot> {
  return openPermissionDragShelf("accessibility");
}

export function openScreenRecordingPermissionAssistant(): Promise<AppSnapshot> {
  return openPermissionDragShelf("screen_recording");
}

// Mirrors the Rust built-in recognized-source list so browser/mock dev keeps
// parity with `list_recognized_sources` (locked-behaviors §10).
const BROWSER_RECOGNIZED_SOURCES: ReadonlyArray<{
  id: string;
  label: string;
  description: string;
  titlePatterns: string[];
  defaultAutoPrompt: boolean;
  custom: boolean;
}> = [
  {
    id: "meet",
    label: "Google Meet",
    description: "Google Meet tabs and floating meet.google.com call windows",
    titlePatterns: ["google meet", "meet - ", "meet.google.com"],
    defaultAutoPrompt: true,
    custom: false,
  },
  {
    id: "zoom",
    label: "Zoom",
    description: "Zoom desktop app or windows titled Zoom Meeting",
    titlePatterns: ["zoom", "zoom meeting"],
    defaultAutoPrompt: true,
    custom: false,
  },
  {
    id: "youtube",
    label: "YouTube",
    description: "YouTube tabs, videos, and YouTube Music windows",
    titlePatterns: ["- youtube", "youtube"],
    defaultAutoPrompt: true,
    custom: false,
  },
];

function recognizedSourcesFromBrowser(
  settings: AppSettings,
): RecognizedSourceInfo[] {
  const builtIns = BROWSER_RECOGNIZED_SOURCES.map((source) => {
    const override = settings.source_auto_prompt.find(
      (entry) => entry.source_id === source.id,
    );
    return {
      id: source.id,
      label: source.label,
      description: source.description,
      auto_prompt: override ? override.auto_prompt : source.defaultAutoPrompt,
      title_patterns: source.titlePatterns,
      custom: source.custom,
    };
  });
  return [
    ...builtIns,
    ...settings.custom_sources.map((source) => {
      const override = settings.source_auto_prompt.find(
        (entry) => entry.source_id === source.id,
      );
      return {
        id: source.id,
        label: source.label,
        description: source.title_patterns.join(", "),
        auto_prompt: override ? override.auto_prompt : source.auto_prompt,
        title_patterns: source.title_patterns,
        custom: true,
      };
    }),
  ];
}

export async function loadRecognizedSources(): Promise<RecognizedSourceInfo[]> {
  if (!isTauriRuntime()) {
    return recognizedSourcesFromBrowser(
      browserSnapshot.settings ?? defaultSettings(),
    );
  }
  return invoke<RecognizedSourceInfo[]>("list_recognized_sources");
}

export async function startSourceCapture(
  sourceId: string,
): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const source = recognizedSourcesFromBrowser(settings).find(
      (entry) => entry.id === sourceId,
    );
    if (source) {
      browserDetectedSource = {
        source_id: source.id,
        label: source.label,
        app_name: source.label,
      };
      browserSourceCapturing = true;
    }
    return loadSnapshot();
  }
  await invoke("start_source_capture", { sourceId });
  return loadSnapshot();
}

export async function stopSourceCapture(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    browserSourceCapturing = false;
    return loadSnapshot();
  }
  await invoke("stop_source_capture");
  return loadSnapshot();
}

export async function loadSourceCaptureStatus(): Promise<SourceCaptureStatus> {
  if (!isTauriRuntime()) {
    return {
      detected: browserDetectedSource,
      capturing: browserSourceCapturing,
    };
  }
  return invoke<SourceCaptureStatus>("source_capture_status");
}

export function browserUploadedPathFromSource(
  sourcePath: string,
  saveRoot: string,
  today = new Date(),
): string {
  const fileName = sourcePath.split("/").filter(Boolean).pop() ?? "audio.mp3";
  const day = [
    today.getFullYear(),
    String(today.getMonth() + 1).padStart(2, "0"),
    String(today.getDate()).padStart(2, "0"),
  ].join("");
  return `${saveRoot}/uploaded/${day}/${fileName}`;
}

export async function uploadAudioFile(
  sourcePath: string,
): Promise<UploadedAudio> {
  if (!isTauriRuntime()) {
    if (!isBrowserUploadableAudioPath(sourcePath)) {
      throw new Error("only mp3, m4a, and wav audio files can be uploaded");
    }
    const settings = browserSnapshot.settings ?? defaultSettings();
    return {
      audio_path: browserUploadedPathFromSource(sourcePath, settings.save_root),
      original_filename:
        sourcePath.split("/").filter(Boolean).pop() ?? "audio.mp3",
      stored_at: new Date().toISOString(),
    };
  }

  return invoke<UploadedAudio>("upload_audio_file", { sourcePath });
}

export async function chooseUploadedAudioFile(): Promise<UploadedAudio | null> {
  if (!isTauriRuntime()) {
    return uploadAudioFile("/tmp/browser-upload/meeting.mp3");
  }

  const selected = await open({
    multiple: false,
    title: "Upload Audio",
    filters: [
      {
        name: "Audio",
        extensions: ["mp3", "m4a", "wav"],
      },
    ],
  });
  if (typeof selected !== "string") {
    return null;
  }

  return uploadAudioFile(selected);
}

export async function loadAudioWaveform(
  audioPath: string,
  bucketCount = 4096,
): Promise<AudioWaveform> {
  if (!isTauriRuntime()) {
    const count = Math.min(bucketCount, 512);
    const peaks: number[] = [];
    const peaksMax: number[] = [];
    const peaksMin: number[] = [];
    for (let index = 0; index < count; index += 1) {
      const envelope = Math.abs(Math.sin(index / 7) * Math.cos(index / 19));
      const wobble = Math.sin(index / 3.1) * 0.18;
      const positive = Number(
        (0.06 + envelope * 0.78 + Math.max(0, wobble)).toFixed(3),
      );
      const negative = Number(
        (-0.05 - envelope * 0.66 + Math.min(0, wobble)).toFixed(3),
      );
      peaks.push(Math.min(1, Math.max(positive, Math.abs(negative))));
      peaksMax.push(Math.min(1, positive));
      peaksMin.push(Math.max(-1, negative));
    }
    return {
      duration_seconds: 30 * 60,
      sample_rate: 8000,
      peaks,
      peaks_max: peaksMax,
      peaks_min: peaksMin,
      audible_ranges: [
        { start: 2, end: 320 },
        { start: 370, end: 880 },
        { start: 930, end: 1780 },
      ],
    };
  }

  return invoke<AudioWaveform>("analyze_audio_waveform", {
    audioPath,
    bucketCount,
  });
}

export async function cancelCurrentTranscription(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    let cancelled = false;
    const jobs = browserSnapshot.queue.jobs.map((job) => {
      if (cancelled || job.status !== "running") {
        return job;
      }

      cancelled = true;
      return {
        ...job,
        status: "cancelled" as const,
        error: "cancelled by user",
        issue: {
          severity: "warning" as const,
          code: "cancelled" as const,
          message: "cancelled by user",
          occurred_at: new Date().toISOString(),
        },
        is_read: false,
      };
    });
    if (!cancelled) {
      return browserSnapshot;
    }

    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("cancel_current_transcription");
  return loadSnapshot();
}

export async function cancelActivityJob(id: number): Promise<AppSnapshot> {
  if (!Number.isSafeInteger(id)) {
    throw new Error("A valid Activity job id is required.");
  }
  if (!isTauriRuntime()) {
    const target = browserSnapshot.queue.jobs.find((job) => job.id === id);
    if (!target) {
      throw new Error(`Job ${id} not found.`);
    }
    if (target.status !== "running") {
      throw new Error(`Job ${id} is not running.`);
    }
    const occurredAt = new Date().toISOString();
    const jobs = browserSnapshot.queue.jobs.map((job) =>
      job.id === id
        ? {
            ...job,
            status: "cancelled" as const,
            error: "cancelled by user",
            issue: {
              severity: "warning" as const,
              code: "cancelled" as const,
              message: "cancelled by user",
              occurred_at: occurredAt,
            },
            is_read: false,
          }
        : job,
    );
    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("cancel_activity_job", { id });
  return loadSnapshot();
}

export async function markAllActivityRead(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    let marked = false;
    const jobs = browserSnapshot.queue.jobs.map((job) => {
      if (job.is_read === true || !isActivityAttentionOutcome(job)) {
        return job;
      }

      marked = true;
      return { ...job, is_read: true };
    });
    if (!marked) {
      return browserSnapshot;
    }

    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("mark_all_activity_read");
  return loadSnapshot();
}

export async function cancelCurrentOperation(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const activeModel = browserSnapshot.models.find((model) =>
      ["downloading", "verifying", "extracting"].includes(model.status),
    );
    if (activeModel) {
      browserVerificationPreviousStatuses.delete(activeModel.id);
      browserSnapshot = {
        ...browserSnapshot,
        models: browserSnapshot.models.map((model) =>
          model.id === activeModel.id
            ? {
                ...model,
                status: "error",
                download_progress: model.download_progress ?? 0,
                download_error: "cancelled by user",
              }
            : model,
        ),
      };
      return browserSnapshot;
    }

    return cancelCurrentTranscription();
  }

  await invoke("cancel_current_operation");
  return loadSnapshot();
}

export async function processNextTranscription(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const models = browserSnapshot.models ?? mockModels();
    if (settings.pause_all || !settings.transcription_enabled) {
      return browserSnapshot;
    }
    if (browserSnapshot.queue.jobs.some((job) => job.status === "running")) {
      return browserSnapshot;
    }

    let processedAudioPath = "";
    const jobs = browserSnapshot.queue.jobs.map((job) => {
      if (
        processedAudioPath ||
        job.status !== "pending" ||
        !isUsableBrowserModel(job.model_id, models)
      ) {
        return job;
      }

      processedAudioPath = job.audio_path;
      return {
        ...job,
        status: "completed" as const,
        error: null,
        issue: null,
      };
    });
    if (processedAudioPath.length === 0) {
      return browserSnapshot;
    }

    const queue = queueFromJobs(jobs);
    const transcriptPath = processedAudioPath.replace(/\.(m4a|wav)$/i, ".txt");
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
      recent_transcripts: [
        {
          transcript_path: transcriptPath,
          audio_path: processedAudioPath,
          recorded_at: new Date().toISOString(),
          text: `Browser fallback transcript for ${processedAudioPath.split("/").pop()}`,
        },
        ...(browserSnapshot.recent_transcripts ?? []),
      ],
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("process_next_transcription");
  return loadSnapshot();
}

export async function regenerateTranscript(
  audioPath: string,
  modelId?: string,
): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    if (!isBrowserImportableAudioPath(audioPath)) {
      return browserSnapshot;
    }
    const settings = browserSnapshot.settings ?? defaultSettings();
    const models = browserSnapshot.models ?? mockModels();
    const regenerationModelId =
      modelId && isUsableBrowserModel(modelId, models)
        ? modelId
        : settings.selected_model;
    let matched = false;
    let blocked = false;
    let changed = false;
    const jobs = browserSnapshot.queue.jobs.map((job) => {
      if (job.audio_path !== audioPath) {
        return job;
      }
      matched = true;
      if (job.status === "running") {
        blocked = true;
        return job;
      }
      changed = true;
      return {
        ...job,
        model_id: regenerationModelId,
        status: "pending" as const,
        error: null,
        issue: null,
      };
    });
    if (blocked) {
      return browserSnapshot;
    }
    if (!matched) {
      const nextId = Math.max(0, ...jobs.map((job) => job.id)) + 1;
      jobs.push({
        id: nextId,
        audio_path: audioPath,
        model_id: regenerationModelId,
        status: "pending",
        error: null,
        issue: null,
      });
      changed = true;
    }
    if (!changed) {
      return browserSnapshot;
    }

    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
      recent_transcripts: (browserSnapshot.recent_transcripts ?? []).filter(
        (entry) => entry.audio_path !== audioPath,
      ),
    };
    return browserSnapshot;
  }

  const payload: { audioPath: string; modelId?: string } = { audioPath };
  if (modelId) {
    payload.modelId = modelId;
  }
  await invoke<QueueSnapshot>("regenerate_transcript", payload);
  return loadSnapshot();
}

export async function reprocessJobs(
  ids: number[],
  modelId: string,
): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const uniqueIds = [
      ...new Set(ids.filter((id) => Number.isSafeInteger(id))),
    ];
    const models = browserSnapshot.models ?? mockModels();
    const model = models.find((candidate) => candidate.id === modelId);
    if (
      uniqueIds.length === 0 ||
      !model ||
      !isUsableBrowserModel(modelId, models) ||
      !modelSupportsReprocessing(model)
    ) {
      return browserSnapshot;
    }

    const selectedIds = new Set(uniqueIds);
    const selectedJobs = browserSnapshot.queue.jobs.filter((job) =>
      selectedIds.has(job.id),
    );
    if (
      selectedJobs.length !== selectedIds.size ||
      selectedJobs.some(
        (job) =>
          !isReprocessableActivityJob(job) ||
          !isBrowserImportableAudioPath(job.audio_path),
      )
    ) {
      return browserSnapshot;
    }

    const settings = browserSnapshot.settings ?? defaultSettings();
    const selectedAudioPaths = new Set(
      selectedJobs.map((job) => job.audio_path),
    );
    const jobs = browserSnapshot.queue.jobs.map((job) =>
      selectedIds.has(job.id)
        ? {
            ...job,
            model_id: modelId,
            status: "pending" as const,
            error: null,
            issue: null,
            is_read: false,
            transcription_options: settings.transcription_options,
            replay_recorded_audio: true,
          }
        : job,
    );
    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
      recent_transcripts: (browserSnapshot.recent_transcripts ?? []).filter(
        (entry) =>
          !entry.audio_path || !selectedAudioPaths.has(entry.audio_path),
      ),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("reprocess_jobs", { ids, modelId });
  return loadSnapshot();
}

export async function trashActivityJobs(
  ids: number[],
): Promise<TrashActivityJobsOutcome> {
  if (!isTauriRuntime()) {
    const uniqueIds = [
      ...new Set(ids.filter((id) => Number.isSafeInteger(id))),
    ];
    const selectedIds = new Set(uniqueIds);
    const selectedJobs = browserSnapshot.queue.jobs.filter((job) =>
      selectedIds.has(job.id),
    );
    if (uniqueIds.length === 0) {
      throw new Error("Select at least one Activity item to move to Trash.");
    }
    if (selectedJobs.length !== selectedIds.size) {
      throw new Error("One or more selected Activity items no longer exist.");
    }
    const running = selectedJobs.find((job) => job.status === "running");
    if (running) {
      throw new Error(`Job ${running.id} is currently running.`);
    }

    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(
      browserSnapshot.queue.jobs.filter((job) => !selectedIds.has(job.id)),
    );
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return {
      queue,
      removed_ids: uniqueIds,
      trashed_ids: uniqueIds,
      missing_ids: [],
      failures: [],
      snapshot: browserSnapshot,
    };
  }

  const result = await invoke<TrashActivityJobsResult>("trash_activity_jobs", {
    ids,
  });
  return { ...result, snapshot: await loadSnapshot() };
}

export async function trashTranscripts(
  entries: readonly RecentTranscript[],
): Promise<TrashTranscriptsResult> {
  const transcriptPaths = [
    ...new Set(
      entries
        .map((entry) => entry.transcript_path.trim())
        .filter((path) => path.length > 0),
    ),
  ];
  if (transcriptPaths.length === 0) {
    throw new Error("Select at least one Transcript item to move to Trash.");
  }

  if (!isTauriRuntime()) {
    const selectedPaths = new Set(transcriptPaths);
    const selectedAudioPaths = new Set(
      entries
        .filter((entry) => selectedPaths.has(entry.transcript_path))
        .flatMap((entry) => (entry.audio_path ? [entry.audio_path] : [])),
    );
    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(
      browserSnapshot.queue.jobs.filter(
        (job) => !selectedAudioPaths.has(job.audio_path),
      ),
    );
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
      recent_transcripts: (browserSnapshot.recent_transcripts ?? []).filter(
        (entry) => !selectedPaths.has(entry.transcript_path),
      ),
    };
    return {
      removed_transcript_paths: transcriptPaths,
      trashed_transcript_paths: transcriptPaths,
      missing_transcript_paths: [],
      failures: [],
    };
  }

  return invoke<TrashTranscriptsResult>("trash_transcripts", {
    transcriptPaths,
  });
}

export async function openTranscriptFolder(path: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return loadSnapshot();
  }

  await invoke("open_transcript_folder", { path });
  return loadSnapshot();
}

export async function mergeTranscriptAudio(
  entries: readonly RecentTranscript[],
  onProgress: (progress: AudioMergeProgress) => void,
): Promise<AudioMergeResult | null> {
  const eligible = eligibleAudioMergeEntries(entries);
  if (eligible.length < 2) {
    throw new Error("Select at least two transcripts with audio to merge.");
  }
  const defaultFileName = audioMergeDefaultFileName(eligible);

  if (!isTauriRuntime()) {
    const operationId = createAudioMergeOperationId(0, "browser");
    const destinationPath = `/tmp/${defaultFileName}`;
    const base = {
      operation_id: operationId,
      total_inputs: eligible.length,
      current_input_label: null,
      destination_path: null,
    };
    onProgress({
      ...base,
      stage: "preparing",
      completed_inputs: 0,
      percent: 0,
    });
    onProgress({
      ...base,
      stage: "combining",
      completed_inputs: eligible.length,
      percent: 85,
    });
    onProgress({
      ...base,
      stage: "encoding",
      completed_inputs: eligible.length,
      percent: 90,
    });
    onProgress({
      ...base,
      stage: "saved",
      completed_inputs: eligible.length,
      percent: 100,
      destination_path: destinationPath,
    });
    return {
      operation_id: operationId,
      destination_path: destinationPath,
      input_count: eligible.length,
    };
  }

  const destinationPath = await save({
    defaultPath: defaultFileName,
    filters: [{ name: "M4A Audio", extensions: ["m4a"] }],
  });
  if (!destinationPath) {
    return null;
  }

  const operationId = createAudioMergeOperationId();
  const request: MergeAudioRequest = {
    operation_id: operationId,
    destination_path: destinationPath,
    inputs: eligible.map((entry) => ({
      audio_path: entry.audio_path as string,
      recorded_at: entry.recorded_at,
    })),
  };
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<AudioMergeProgress>(
    "audio-merge-progress",
    (event) => onProgress(event.payload),
  );
  try {
    onProgress({
      operation_id: operationId,
      stage: "preparing",
      completed_inputs: 0,
      total_inputs: eligible.length,
      percent: 0,
      current_input_label: null,
      destination_path: null,
    });
    const result = await invoke<AudioMergeResult>("merge_transcript_audio", {
      request,
    });
    onProgress({
      operation_id: result.operation_id,
      stage: "saved",
      completed_inputs: result.input_count,
      total_inputs: result.input_count,
      percent: 100,
      current_input_label: null,
      destination_path: result.destination_path,
    });
    return result;
  } finally {
    unlisten();
  }
}

export async function startLiveCapture(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = browserSnapshot.queue ?? emptyQueue();
    if (browserSnapshot.status?.live_input_active) {
      return loadSnapshot();
    }
    const mode = deriveProductMode(settings);
    const canCapture =
      mode === "recording_and_transcription" || mode === "recording_only";
    if (!canCapture) {
      browserSnapshot = {
        ...browserSnapshot,
        settings,
        queue,
        status: statusFrom(settings, queue, false),
      };
      return browserSnapshot;
    }

    browserCaptureSessionId += 1;
    browserCaptureSessionTranscriptionRequested =
      settings.transcription_enabled;
    browserSnapshot = {
      ...browserSnapshot,
      settings,
      queue,
      status: statusFrom(settings, queue, true),
    };
    return browserSnapshot;
  }

  await invoke<AppStatus>("start_live_capture");
  return loadSnapshot();
}

export async function stopLiveCapture(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const currentQueue = browserSnapshot.queue ?? emptyQueue();
    const queue = maybeQueueBrowserCapture(
      settings,
      currentQueue,
      statusFrom(settings, currentQueue),
    );
    browserSnapshot = {
      ...browserSnapshot,
      settings,
      queue,
      status: statusFrom(settings, queue, false),
    };
    return browserSnapshot;
  }

  await invoke<AppStatus>("stop_live_capture");
  return loadSnapshot();
}

export async function chooseSaveRoot(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return browserSnapshot;
  }

  const selected = await open({
    directory: true,
    multiple: false,
    title: "Choose WakeNote Save Folder",
  });
  if (typeof selected !== "string") {
    return loadSnapshot();
  }

  await invoke<AppSettings>("update_settings", {
    patch: { save_root: selected },
  });
  return loadSnapshot();
}

export async function chooseModelDirectory(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return browserSnapshot;
  }

  const selected = await open({
    directory: true,
    multiple: false,
    title: "Choose Model Directory",
  });
  if (typeof selected !== "string") {
    return loadSnapshot();
  }

  await invoke<AppSettings>("update_settings", {
    patch: { model_directory: selected },
  });
  return loadSnapshot();
}

export async function revealSaveFolder(): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    return browserSnapshot;
  }

  await invoke("reveal_save_folder");
  return loadSnapshot();
}

export async function verifyModel(modelId: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const target = browserSnapshot.models.find((model) => model.id === modelId);
    if (
      !target ||
      ["downloading", "verifying", "extracting"].includes(target.status)
    ) {
      return browserSnapshot;
    }

    browserSnapshot = {
      ...browserSnapshot,
      models: browserSnapshot.models.map((model) =>
        model.id === modelId
          ? (() => {
              browserVerificationPreviousStatuses.set(modelId, model.status);
              return {
                ...model,
                status: "verifying",
                download_progress: null,
                download_error: null,
              };
            })()
          : model,
      ),
    };
    return browserSnapshot;
  }

  await invoke("verify_model", { modelId });
  return loadSnapshot();
}

export async function downloadModel(modelId: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const target = browserSnapshot.models.find((model) => model.id === modelId);
    if (
      !target?.download_url ||
      !["missing", "error"].includes(target.status)
    ) {
      return browserSnapshot;
    }

    browserVerificationPreviousStatuses.delete(modelId);
    browserSnapshot = {
      ...browserSnapshot,
      models: browserSnapshot.models.map((model) =>
        model.id === modelId
          ? {
              ...model,
              status: "downloading",
              download_progress: 0,
              download_error: null,
            }
          : model,
      ),
    };
    return browserSnapshot;
  }

  await invoke("download_model", { modelId });
  return loadSnapshot();
}

export async function cancelModelDownload(
  modelId: string,
): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const target = browserSnapshot.models.find((model) => model.id === modelId);
    if (
      !target ||
      !["downloading", "verifying", "extracting"].includes(target.status)
    ) {
      return browserSnapshot;
    }

    browserVerificationPreviousStatuses.delete(modelId);
    browserSnapshot = {
      ...browserSnapshot,
      models: browserSnapshot.models.map((model) =>
        model.id === modelId
          ? {
              ...model,
              status: "error",
              download_progress: model.download_progress ?? 0,
              download_error: "cancelled by user",
            }
          : model,
      ),
    };
    return browserSnapshot;
  }

  await invoke("cancel_model_download", { modelId });
  return loadSnapshot();
}

export async function deleteModel(modelId: string): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    const settings = browserSnapshot.settings ?? defaultSettings();
    const target = browserSnapshot.models.find((model) => model.id === modelId);
    if (
      !target ||
      settings.selected_model === modelId ||
      target.status === "missing" ||
      ["downloading", "verifying", "extracting"].includes(target.status)
    ) {
      return browserSnapshot;
    }

    browserVerificationPreviousStatuses.delete(modelId);
    browserSnapshot = {
      ...browserSnapshot,
      models: browserSnapshot.models.map((model) =>
        model.id === modelId
          ? {
              ...model,
              status: "missing",
              download_progress: null,
              download_error: null,
            }
          : model,
      ),
    };
    return browserSnapshot;
  }

  await invoke("delete_model", { modelId });
  return loadSnapshot();
}

export async function retryJob(id: number): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    let retried = false;
    const jobs = browserSnapshot.queue.jobs.map((job) => {
      if (job.id !== id || !isReprocessableActivityJob(job)) {
        return job;
      }

      retried = true;
      return {
        ...job,
        status: "pending" as const,
        error: null,
        issue: null,
        is_read: false,
      };
    });
    if (!retried) {
      return browserSnapshot;
    }

    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("retry_job", { id });
  return loadSnapshot();
}

export async function skipJob(id: number): Promise<AppSnapshot> {
  if (!isTauriRuntime()) {
    let skipped = false;
    const jobs = browserSnapshot.queue.jobs.map((job) => {
      if (
        job.id !== id ||
        !["pending", "failed", "cancelled"].includes(job.status)
      ) {
        return job;
      }

      skipped = true;
      return {
        ...job,
        status: "skipped" as const,
        error: null,
        issue: {
          severity: "warning" as const,
          code: "skipped" as const,
          message: "Skipped",
          occurred_at: new Date().toISOString(),
        },
        is_read: false,
      };
    });
    if (!skipped) {
      return browserSnapshot;
    }

    const settings = browserSnapshot.settings ?? defaultSettings();
    const queue = queueFromJobs(jobs);
    browserSnapshot = {
      ...browserSnapshot,
      queue,
      status: statusFrom(settings, queue),
    };
    return browserSnapshot;
  }

  await invoke<QueueSnapshot>("skip_job", { id });
  return loadSnapshot();
}

// --- Long-form meeting transcription -------------------------------------

function browserMeetingSummary(detail: MeetingDetail): MeetingSummary {
  const { record } = detail;
  return {
    id: record.id,
    title: record.title,
    source_filename: record.source_filename,
    status: record.status,
    duration_ms: record.duration_ms,
    created_at: record.created_at,
    updated_at: record.updated_at,
    progress: record.progress,
    model_id: record.model_id,
    language: record.language,
    error: record.error,
    capture_warning: record.capture_warning,
  };
}

export async function listMeetings(): Promise<MeetingSummary[]> {
  if (!isTauriRuntime()) {
    return [...browserMeetingDetails.values()].map(browserMeetingSummary);
  }
  return invoke<MeetingSummary[]>("list_meetings");
}

export async function listTranscriptionModels(): Promise<ModelDescriptor[]> {
  if (!isTauriRuntime()) {
    return settledBrowserModels(browserSnapshot.models ?? mockModels());
  }
  return invoke<ModelDescriptor[]>("list_models");
}

function browserManualMeetingRecordingStatus(): ManualMeetingRecordingStatus {
  const elapsedMs =
    browserManualMeetingStartedAt === null
      ? 0
      : Math.min(18_000_000, Date.now() - browserManualMeetingStartedAt);
  return {
    generation: browserManualMeetingGeneration,
    state: browserManualMeetingStartedAt === null ? "off" : "recording",
    meeting_id: browserManualMeetingId,
    started_at:
      browserManualMeetingStartedAt === null
        ? null
        : new Date(browserManualMeetingStartedAt).toISOString(),
    elapsed_ms: elapsedMs,
    remaining_ms: Math.max(0, 18_000_000 - elapsedMs),
    inputs: ["Microphone", "System Audio"],
    stop_reason: null,
    error: null,
  };
}

export async function loadManualMeetingRecordingStatus(): Promise<ManualMeetingRecordingStatus> {
  if (!isTauriRuntime()) {
    return browserManualMeetingRecordingStatus();
  }
  return invoke<ManualMeetingRecordingStatus>(
    "manual_meeting_recording_status",
  );
}

export async function startManualMeetingRecording(): Promise<ManualMeetingRecordingStatus> {
  if (!isTauriRuntime()) {
    if (browserManualMeetingStartedAt === null) {
      browserManualMeetingStartedAt = Date.now();
      browserManualMeetingGeneration += 1;
      browserManualMeetingId = `meeting-manual-${browserManualMeetingGeneration}`;
    }
    return browserManualMeetingRecordingStatus();
  }
  return invoke<ManualMeetingRecordingStatus>("start_manual_meeting_recording");
}

export async function stopManualMeetingRecording(): Promise<ManualMeetingRecordingStatus> {
  if (!isTauriRuntime()) {
    browserManualMeetingStartedAt = null;
    return browserManualMeetingRecordingStatus();
  }
  return invoke<ManualMeetingRecordingStatus>("stop_manual_meeting_recording");
}

/** Open a file picker and save a meeting recording for explicit configuration. */
export async function importMeetingRecording(): Promise<MeetingSummary | null> {
  if (!isTauriRuntime()) {
    return null;
  }
  const selected = await open({
    multiple: false,
    title: "Choose meeting recording",
    filters: [{ name: "Audio", extensions: ["mp3", "m4a", "wav"] }],
  });
  if (typeof selected !== "string") {
    return null;
  }
  return invoke<MeetingSummary>("import_meeting_recording", {
    sourcePath: selected,
  });
}

export async function meetingDetail(id: string): Promise<MeetingDetail> {
  if (!isTauriRuntime()) {
    const detail = browserMeetingDetails.get(id);
    if (!detail) {
      throw new Error(`Meeting ${id} was not found.`);
    }
    return detail;
  }
  return invoke<MeetingDetail>("meeting_detail", { id });
}

export async function startMeetingTranscription(
  id: string,
  request: MeetingTranscriptionRequest,
): Promise<MeetingSummary> {
  if (!isTauriRuntime()) {
    const detail = browserMeetingDetails.get(id);
    if (!detail) {
      throw new Error(`Meeting ${id} was not found.`);
    }
    const next: MeetingDetail = {
      ...detail,
      transcript: "",
      record: {
        ...detail.record,
        status: "pending",
        updated_at: new Date().toISOString(),
        progress: {
          segments_total: 0,
          segments_done: 0,
          processed_ms: 0,
          elapsed_ms: 0,
        },
        model_id: request.model_id,
        language: request.language,
        error: null,
        transcription_request: request,
        segments: [],
        speaker_turns: [],
        api_audio_duration_ms: 0,
        api_cost_microusd: 0,
        api_request_count: 0,
        api_unpriced_request_count: 0,
      },
    };
    browserMeetingDetails.set(id, next);
    return browserMeetingSummary(next);
  }
  return invoke<MeetingSummary>("start_meeting_transcription", {
    id,
    request,
  });
}

function updateBrowserMeetingStatus(
  id: string,
  status: MeetingSummary["status"],
): MeetingSummary {
  const detail = browserMeetingDetails.get(id);
  if (!detail) {
    throw new Error(`Meeting ${id} was not found.`);
  }
  const next: MeetingDetail = {
    ...detail,
    record: {
      ...detail.record,
      status,
      updated_at: new Date().toISOString(),
      progress:
        status === "processing"
          ? {
              phase: "preparing_audio",
              segments_total: detail.record.progress.segments_total,
              segments_done: detail.record.progress.segments_done,
              processed_ms: detail.record.progress.processed_ms,
              elapsed_ms: detail.record.progress.elapsed_ms,
            }
          : detail.record.progress,
    },
  };
  browserMeetingDetails.set(id, next);
  return browserMeetingSummary(next);
}

function browserTranscriptionCostSnapshot(): TranscriptionCostSnapshot {
  const now = new Date();
  const week = new Date(now);
  week.setDate(week.getDate() - ((week.getDay() + 6) % 7));
  const period = (start: string) => {
    const { total } = aggregateCosts(browserCostEntries, {
      ...defaultCostFilters(now),
      start,
      includeReferenceEstimates: false,
    });
    return {
      estimated_cost_usd: total.cost,
      audio_duration_ms: total.duration,
      request_count: total.requests,
      unpriced_request_count: total.unpriced,
    };
  };
  return {
    currency: "USD",
    generated_at: new Date().toISOString(),
    today: period(localDateKey(now)),
    week: period(localDateKey(week)),
    month: period(localDateKey(new Date(now.getFullYear(), now.getMonth(), 1))),
    entry_count: browserCostEntries.length,
    disclosure:
      "Local estimate; verify final charges in your provider billing dashboard.",
  };
}

export async function loadTranscriptionCostSnapshot(): Promise<TranscriptionCostSnapshot> {
  if (!isTauriRuntime()) {
    return browserTranscriptionCostSnapshot();
  }
  return invoke<TranscriptionCostSnapshot>("transcription_cost_snapshot");
}

export async function loadTranscriptionCostDetails(): Promise<TranscriptionCostDetails> {
  if (!isTauriRuntime()) {
    return {
      currency: "USD",
      generated_at: new Date().toISOString(),
      entries: browserCostEntries,
      entry_limit: 10_000,
    };
  }
  return invoke<TranscriptionCostDetails>("transcription_cost_details");
}

export async function subscribeTranscriptionCostUpdates(
  callback: (snapshot: TranscriptionCostSnapshot) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => {};
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<TranscriptionCostSnapshot>(
    "transcription-cost-updated",
    (event) => callback(event.payload),
  );
}

export async function cancelMeeting(id: string): Promise<void> {
  if (!isTauriRuntime()) {
    updateBrowserMeetingStatus(id, "canceled");
    return;
  }
  await invoke("cancel_meeting", { id });
}

export async function resumeMeeting(id: string): Promise<MeetingSummary> {
  if (!isTauriRuntime()) {
    return updateBrowserMeetingStatus(id, "processing");
  }
  return invoke<MeetingSummary>("resume_meeting", { id });
}
