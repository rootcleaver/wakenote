import { CircleAlertIcon } from "lucide-react";
import type { ReactNode } from "react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { AppPageRouter } from "./components/AppPageRouter";
import { MeetingTranscriptionPanel } from "./components/MeetingTranscriptionPanel";
import { QueuePanel } from "./components/QueuePanel";
import { WebhooksPanel } from "./components/WebhooksPanel";
import { ReportHistoryPanel } from "./components/ReportHistoryPanel";
import { TranscriptsPanel } from "./components/TranscriptsPanel";
import { CapturePage } from "./components/capture/CapturePage";
import { AppFrame } from "./components/shell/AppFrame";
import { PageHeader } from "./components/shell/PageHeader";
import { RecordingStatusRail } from "./components/shell/RecordingStatusRail";
import { SettingsPage } from "./components/settings/SettingsPage";
import { EmptyState } from "./components/ui/empty-state";
import { Alert, AlertDescription, AlertTitle } from "./components/ui/alert";
import { newestTranscriptTextEntries } from "./lib/live-transcripts";
import { formatLocalTimestamp } from "./lib/transcript-history";
import { useActivityAttention } from "./hooks/use-activity-attention";
import type { PrimaryRoute, SettingsSection } from "./lib/navigation";
import {
  reduceTranscriptLog,
  type TranscriptEntry,
  type TranscriptEvent,
} from "./lib/transcript-log";
import {
  cancelModelDownload,
  cancelCurrentTranscription,
  cancelActivityJob,
  cancelDictation,
  chooseModelDirectory,
  chooseSaveRoot,
  revealSaveFolder,
  chooseAudioFiles,
  deleteModel,
  downloadModel,
  enqueueBacklog,
  loadRecentTranscripts,
  loadDictationState,
  loadPermissionSnapshot,
  loadSnapshot,
  isTauriRuntime,
  markAllActivityRead,
  openAccessibilityPermissionAssistant,
  openTranscriptFolder,
  requestMicrophonePermission,
  openMicrophonePermissionSettings,
  pressedModifierShortcut,
  resumeDictationShortcut,
  openScreenRecordingPermissionAssistant,
  openDictionaryFile,
  processNextTranscription,
  previewSubtitle,
  reloadDictionaryFile,
  reprocessJobs,
  trashActivityJobs,
  retryJob,
  saveOpenRouterApiKey,
  saveSettingsPatch,
  setMicrophoneInputVolume,
  skipJob,
  startLiveCapture,
  stopLiveCapture,
  suspendDictationShortcut,
  deleteOpenRouterApiKey,
  saveOpenAiApiKey,
  deleteOpenAiApiKey,
  saveSonioxApiKey,
  deleteSonioxApiKey,
  verifyModel,
} from "./lib/tauri-client";
import {
  mockSnapshot,
  pollSnapshotDependencyKey,
  shouldRefreshSnapshotForTauriEvent,
  shouldPollSnapshot,
} from "./lib/app-state";
import type {
  AppSnapshot,
  AppSettings,
  DictationStatePayload,
} from "./lib/types";
import {
  missingPermissions,
  PERMISSION_GUIDANCE_EVENT,
  permissionOnboardingNeedsGuidance,
  type NativePermissionGuidancePayload,
  type PermissionFeature,
} from "./lib/permission-guidance";
import { shouldHandleFrontendHideShortcut } from "./lib/window-shortcuts";

const TranscriptionCostsPage = lazy(
  () => import("./components/TranscriptionCostsPage"),
);

const launchAutoStartPollWindowMs = 130_000;

function preserveRecentTranscripts(
  current: AppSnapshot,
  next: AppSnapshot,
): AppSnapshot {
  if (next.recent_transcripts.length > 0) {
    return next;
  }

  return {
    ...next,
    recent_transcripts: current.recent_transcripts,
  };
}

function transcriptEntriesFromRecent(
  recentTranscripts: AppSnapshot["recent_transcripts"],
): TranscriptEntry[] {
  return [...recentTranscripts].reverse().map((transcript, index) => ({
    chunk_id: -1 - index,
    status: "final",
    text: transcript.text,
    started_at: transcript.recorded_at,
    recorded_at: transcript.recorded_at,
    audio_path: transcript.audio_path,
    error: null,
  }));
}

function WorkspacePage({
  slot,
  eyebrow,
  title,
  description,
  children,
}: {
  slot: string;
  eyebrow: string;
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <div data-slot={`${slot}-page`} className="primary-workspace">
      <PageHeader eyebrow={eyebrow} title={title} description={description} />
      {children}
    </div>
  );
}

export default function App() {
  const [activeRoute, setActiveRoute] = useState<PrimaryRoute>("capture");
  const [dictationHistory, setDictationHistory] = useState<{
    day: string | null;
  } | null>(null);
  const [activeSettingsSection, setActiveSettingsSection] =
    useState<SettingsSection>("general");
  const [snapshot, setSnapshot] = useState<AppSnapshot>(mockSnapshot());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const settingsWrites = useRef({
    tail: Promise.resolve(),
    pending: 0,
    revision: 0,
  });
  const [transcriptLog, setTranscriptLog] = useState<TranscriptEntry[]>([]);
  const [dictationState, setDictationState] = useState<DictationStatePayload>({
    state: "idle",
    error: null,
  });
  const activityAttention = useActivityAttention(snapshot.queue.jobs);
  const launchAutoStartPollUntilMs = useRef(
    Date.now() + launchAutoStartPollWindowMs,
  );
  const permissionOnboardingHandled = useRef(false);
  const launchAutoStartSuppressed = useRef(false);
  const transcriptDispatch = useRef((event: TranscriptEvent) => {
    setTranscriptLog((entries) => reduceTranscriptLog(entries, event));
  });

  async function refresh() {
    const revision = settingsWrites.current.revision;
    setBusy(true);
    setError(null);
    try {
      const next = await loadSnapshot();
      acceptSnapshot(next, revision);
      await handlePermissionOnboarding(next);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setBusy(false);
    }
  }

  async function handlePermissionOnboarding(next: AppSnapshot) {
    if (!isTauriRuntime() || permissionOnboardingHandled.current) {
      return;
    }
    permissionOnboardingHandled.current = true;
    if (next.settings.permission_onboarding_seen) {
      return;
    }

    launchAutoStartSuppressed.current = true;
    if (permissionOnboardingNeedsGuidance(next.permissions)) {
      openSettings("audio");
    }

    try {
      await commitSettingsPatch({
        permission_onboarding_seen: true,
      });
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }

  async function refreshTranscripts() {
    try {
      // Footer's recent strip only needs the most recent handful; the
      // Transcripts panel now loads per-day on its own.
      const recentTranscripts = await loadRecentTranscripts();
      setSnapshot((current) => ({
        ...current,
        recent_transcripts: recentTranscripts,
      }));
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }

  useEffect(() => {
    void refresh();
    void refreshTranscripts();
    void loadDictationState()
      .then(setDictationState)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (typeof window === "undefined" || !window.__TAURI_INTERNALS__) {
      return;
    }
    const handleHideShortcut = (event: KeyboardEvent) => {
      if (!shouldHandleFrontendHideShortcut(event, navigator.platform)) {
        return;
      }
      event.preventDefault();
      void (async () => {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().hide();
      })();
    };
    window.addEventListener("keydown", handleHideShortcut);
    return () => window.removeEventListener("keydown", handleHideShortcut);
  }, []);

  useEffect(() => {
    if (typeof window === "undefined" || !window.__TAURI_INTERNALS__) {
      return;
    }
    let cancelled = false;
    const unlisteners: Array<() => void> = [];

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const dispatch = transcriptDispatch.current;
      const subscriptions: Array<
        [string, (payload: unknown) => TranscriptEvent | null]
      > = [
        [
          "live-transcript-started",
          (payload) => {
            const data = payload as {
              source_key: string;
              source_label: string;
              microphone_slot?: "primary" | "secondary" | null;
              chunk_id: number;
              started_at: string;
            };
            return {
              type: "started",
              source_key: data.source_key,
              source_label: data.source_label,
              microphone_slot: data.microphone_slot,
              chunk_id: data.chunk_id,
              started_at: data.started_at,
            };
          },
        ],
        [
          "live-transcript-partial",
          (payload) => {
            const data = payload as {
              source_key: string;
              source_label: string;
              microphone_slot?: "primary" | "secondary" | null;
              chunk_id: number;
              text: string;
            };
            return { type: "partial", ...data };
          },
        ],
        [
          "live-transcript-committed",
          (payload) => {
            const data = payload as {
              source_key: string;
              source_label: string;
              microphone_slot?: "primary" | "secondary" | null;
              chunk_id: number;
              audio_path: string;
              will_transcribe: boolean;
            };
            return {
              type: "committed",
              source_key: data.source_key,
              source_label: data.source_label,
              microphone_slot: data.microphone_slot,
              chunk_id: data.chunk_id,
              audio_path: data.audio_path,
              will_transcribe: data.will_transcribe,
            };
          },
        ],
        [
          "live-transcript-final",
          (payload) => {
            const data = payload as {
              source_key: string;
              source_label: string;
              microphone_slot?: "primary" | "secondary" | null;
              chunk_id: number | null;
              audio_path: string;
              text: string;
              recorded_at?: string;
              speaker?: "me" | "other" | "unknown" | null;
              speaker_score?: number | null;
            };
            return {
              type: "final",
              source_key: data.source_key,
              source_label: data.source_label,
              microphone_slot: data.microphone_slot,
              chunk_id: data.chunk_id,
              audio_path: data.audio_path,
              text: data.text,
              recorded_at: data.recorded_at,
              speaker: data.speaker ?? null,
              speaker_score: data.speaker_score ?? null,
            };
          },
        ],
        [
          "live-transcript-failed",
          (payload) => {
            const data = payload as {
              source_key: string;
              source_label: string;
              microphone_slot?: "primary" | "secondary" | null;
              chunk_id: number | null;
              audio_path: string;
              error: string;
              recorded_at?: string;
            };
            return {
              type: "failed",
              source_key: data.source_key,
              source_label: data.source_label,
              microphone_slot: data.microphone_slot,
              chunk_id: data.chunk_id,
              audio_path: data.audio_path,
              error: data.error,
              recorded_at: data.recorded_at,
            };
          },
        ],
        ["source-capture-started", () => null],
        ["source-capture-stopped", () => null],
        ["source-capture-error", () => null],
        ["dictionary-changed", () => null],
        ["dictionary-file-error", () => null],
        ["microphone-input-levels-changed", () => null],
        ["live-input-changed", () => null],
        [
          "text-transform-error",
          (payload) => {
            setError(String(payload));
            return null;
          },
        ],
        [
          "auto-type-error",
          (payload) => {
            setError(String(payload));
            return null;
          },
        ],
        [
          "live-transcript-discarded",
          (payload) => {
            const data = payload as { source_key: string; chunk_id: number };
            return {
              type: "committed",
              ...data,
              audio_path: "",
              will_transcribe: false,
            };
          },
        ],
      ];

      for (const [eventName, parse] of subscriptions) {
        const unlisten = await listen(eventName, (rawEvent) => {
          // Event payloads include private transcripts; never log their contents.
          const next = parse(rawEvent.payload);
          if (next) {
            dispatch(next);
          }
          if (eventName === "live-transcript-final") {
            void refreshTranscripts();
          }
          if (shouldRefreshSnapshotForTauriEvent(eventName)) {
            void refreshQuietly();
          }
        });
        if (cancelled) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      }
      const unlistenDictation = await listen("dictation-state", (rawEvent) => {
        setDictationState(rawEvent.payload as DictationStatePayload);
      });
      if (cancelled) {
        unlistenDictation();
      } else {
        unlisteners.push(unlistenDictation);
      }
      const unlistenPermissionGuidance =
        await listen<NativePermissionGuidancePayload>(
          PERMISSION_GUIDANCE_EVENT,
          async () => {
            if (cancelled) {
              return;
            }
            openSettings("audio");
            try {
              const { getCurrentWindow } = await import(
                "@tauri-apps/api/window"
              );
              const currentWindow = getCurrentWindow();
              await currentWindow.show();
              await currentWindow.setFocus();
            } catch (caught) {
              setError(
                caught instanceof Error ? caught.message : String(caught),
              );
            }
          },
        );
      if (cancelled) {
        unlistenPermissionGuidance();
      } else {
        unlisteners.push(unlistenPermissionGuidance);
      }
      // eslint-disable-next-line no-console
      console.log("[wakenote FE] live transcription listeners registered");
    })();

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);

  async function refreshQuietly() {
    if (settingsWrites.current.pending > 0) return;
    const revision = settingsWrites.current.revision;
    try {
      const next = await loadSnapshot();
      acceptSnapshot(next, revision);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }

  async function cancelActiveDictation() {
    setError(null);
    try {
      setDictationState(await cancelDictation());
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }

  useEffect(() => {
    if (
      dictationState.state !== "recording" &&
      dictationState.state !== "transcribing"
    ) {
      return;
    }
    const cancelOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.repeat) {
        return;
      }
      event.preventDefault();
      void cancelActiveDictation();
    };
    window.addEventListener("keydown", cancelOnEscape);
    return () => window.removeEventListener("keydown", cancelOnEscape);
  }, [dictationState.state]);

  const launchAutoStartPending =
    !launchAutoStartSuppressed.current &&
    snapshot.settings.permission_onboarding_seen &&
    snapshot.settings.start_live_input_on_launch &&
    snapshot.settings.recording_enabled &&
    !snapshot.settings.pause_all &&
    !snapshot.status.live_input_active &&
    Date.now() <= launchAutoStartPollUntilMs.current;

  const pollingDependencyKey = pollSnapshotDependencyKey(
    snapshot.status,
    snapshot.queue,
    snapshot.models,
    { launchAutoStartPending },
  );

  useEffect(() => {
    if (
      !shouldPollSnapshot(snapshot.status, snapshot.queue, snapshot.models, {
        launchAutoStartPending,
      })
    ) {
      return;
    }

    const timer = window.setInterval(() => {
      void refreshQuietly();
    }, 100);
    return () => window.clearInterval(timer);
  }, [pollingDependencyKey]);

  function acceptSnapshot(next: AppSnapshot, revision: number) {
    setSnapshot((current) =>
      preserveRecentTranscripts(current, {
        ...next,
        settings:
          settingsWrites.current.pending > 0 ||
          settingsWrites.current.revision !== revision
            ? current.settings
            : next.settings,
      }),
    );
  }

  function commitSettingsPatch(patch: Partial<AppSettings>): Promise<void> {
    const writes = settingsWrites.current;
    writes.pending += 1;
    writes.revision += 1;
    setBusy(true);
    setError(null);
    const pending = writes.tail.then(async () => {
      const next = await saveSettingsPatch(patch);
      setSnapshot((current) => preserveRecentTranscripts(current, next));
    });
    // One failed write must not poison subsequent retries or unrelated fields.
    writes.tail = pending.catch(() => {});
    return pending.finally(() => {
      writes.pending -= 1;
      if (writes.pending === 0) setBusy(false);
    });
  }

  async function patchSettings(patch: Partial<AppSettings>) {
    try {
      await commitSettingsPatch(patch);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  }

  async function runAction(
    action: () => Promise<AppSnapshot>,
    propagateError = false,
  ) {
    const revision = settingsWrites.current.revision;
    setBusy(true);
    setError(null);
    try {
      const next = await action();
      acceptSnapshot(next, revision);
      return true;
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      if (propagateError) throw caught;
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function ensurePermission(feature: PermissionFeature) {
    setBusy(true);
    setError(null);
    try {
      const permissions = await loadPermissionSnapshot();
      setSnapshot((current) => ({ ...current, permissions }));
      if (missingPermissions(feature, permissions).length > 0) {
        openSettings("audio");
        return false;
      }
      return true;
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      openSettings("audio");
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function runPermissionAction(
    feature: PermissionFeature,
    action: () => Promise<AppSnapshot>,
  ) {
    if (!(await ensurePermission(feature))) {
      return false;
    }
    return runAction(action);
  }

  async function moveActivityJobsToTrash(ids: number[]): Promise<number[]> {
    setBusy(true);
    setError(null);
    try {
      const outcome = await trashActivityJobs(ids);
      setSnapshot((current) =>
        preserveRecentTranscripts(current, outcome.snapshot),
      );
      if (outcome.failures.length > 0) {
        const detail = outcome.failures
          .slice(0, 3)
          .map((failure) => `${failure.audio_path}: ${failure.error}`)
          .join("; ");
        setError(
          `${outcome.failures.length} selected ${outcome.failures.length === 1 ? "recording was" : "recordings were"} not moved to Trash. ${detail}`,
        );
      }
      return outcome.removed_ids;
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      return [];
    } finally {
      setBusy(false);
    }
  }

  const themeMode = snapshot.settings.theme_mode === "light" ? "light" : "dark";

  useEffect(() => {
    if (typeof document === "undefined") {
      return;
    }
    document.documentElement.dataset.theme = themeMode;
  }, [themeMode]);

  const transcriptEntries = [
    ...transcriptEntriesFromRecent(snapshot.recent_transcripts),
    ...transcriptLog,
  ];
  const latestTranscriptText =
    newestTranscriptTextEntries(transcriptEntries).at(-1)?.text ?? "";
  const usableModelIds = new Set(
    snapshot.models
      .filter((model) =>
        ["ready", "installed", "unloaded"].includes(model.status),
      )
      .map((model) => model.id),
  );
  const canProcessTranscription =
    !snapshot.settings.pause_all &&
    snapshot.settings.transcription_enabled &&
    snapshot.queue.jobs.some(
      (job) => job.status === "pending" && usableModelIds.has(job.model_id),
    );

  function openSettings(section: SettingsSection) {
    setActiveSettingsSection(section);
    setActiveRoute("settings");
  }

  const pages: Record<PrimaryRoute, ReactNode> = {
    capture: (
      <CapturePage
        snapshot={snapshot}
        activityAttention={activityAttention.attention}
        transcriptEntries={transcriptEntries}
        busy={busy}
        onStart={() => void runPermissionAction("live_input", startLiveCapture)}
        onStop={() => void runAction(stopLiveCapture)}
        onRefresh={() => void refresh()}
        onPatch={(patch) => void patchSettings(patch)}
        onOpenAudioSettings={() => openSettings("audio")}
      />
    ),
    meetings: (
      <WorkspacePage
        slot="meetings"
        eyebrow="Long-form audio"
        title="Meetings"
        description="Import, monitor, resume, and review long meeting recordings."
      >
        <MeetingTranscriptionPanel
          onPermissionRequired={ensurePermission}
          models={snapshot.models}
          meetingModelId={snapshot.settings.meeting_model}
          selectedModelId={snapshot.settings.selected_model}
          transcriptionLanguageHints={
            snapshot.settings.transcription_language_hints
          }
          onMeetingModelChange={(meeting_model) =>
            void patchSettings({ meeting_model })
          }
        />
      </WorkspacePage>
    ),
    transcripts: (
      <WorkspacePage
        slot="transcripts"
        eyebrow="Short captures"
        title="Transcripts"
        description="Browse daily voice clips, play audio, and create reports."
      >
        <TranscriptsPanel
          initialSourceFilter={dictationHistory ? "dictation" : "all"}
          initialActiveDay={dictationHistory?.day}
          translation={{
            enabled: snapshot.settings.transcription_translation_enabled,
            language: snapshot.settings.transcription_translation_language,
            model:
              snapshot.settings.text_transform_model ||
              snapshot.settings.openrouter_model,
            configured: snapshot.openrouter_key_configured,
          }}
          customSources={snapshot.settings.custom_sources}
          models={snapshot.models}
          selectedModelId={snapshot.settings.selected_model}
          autoPlayNext={snapshot.settings.autoplay_next_transcript}
          openrouterKeyConfigured={snapshot.openrouter_key_configured}
          onOpenReports={() => setActiveRoute("reports")}
        />
      </WorkspacePage>
    ),
    reports: (
      <WorkspacePage
        slot="reports"
        eyebrow="Generated output"
        title="Reports"
        description="Review and download LLM-generated transcript reports."
      >
        <ReportHistoryPanel
          model={snapshot.settings.openrouter_model}
          maxIterations={snapshot.settings.llm_max_iterations}
          openrouterKeyConfigured={snapshot.openrouter_key_configured}
          onOpenIntegrationSettings={() => openSettings("integrations")}
        />
      </WorkspacePage>
    ),
    activity: (
      <WorkspacePage
        slot="activity"
        eyebrow="Processing queue"
        title="Activity"
        description="Track pending, active, completed, and failed transcription jobs."
      >
        <QueuePanel
          nowMs={activityAttention.nowMs}
          queue={snapshot.queue}
          models={snapshot.models}
          selectedModelId={snapshot.settings.selected_model}
          canProcessTranscription={canProcessTranscription}
          onImportAudioFiles={() => void runAction(chooseAudioFiles)}
          onEnqueueBacklog={() =>
            void runAction(() => enqueueBacklog(snapshot.settings.save_root))
          }
          onMarkAllRead={() => void runAction(markAllActivityRead)}
          onCancelCurrent={() => void runAction(cancelCurrentTranscription)}
          onCancelJob={(id) => void runAction(() => cancelActivityJob(id))}
          onProcessNext={() => void runAction(processNextTranscription)}
          onRetry={(id) => void runAction(() => retryJob(id))}
          onSkip={(id) => void runAction(() => skipJob(id))}
          onOpenFolder={(path) =>
            void runAction(() => openTranscriptFolder(path))
          }
          onTrash={moveActivityJobsToTrash}
          onReprocess={(ids, modelId) =>
            runAction(() => reprocessJobs(ids, modelId))
          }
        />
      </WorkspacePage>
    ),
    webhooks: (
      <WorkspacePage
        slot="webhooks-page"
        eyebrow="Delivery history"
        title="Webhooks"
        description="Review deliveries from the last 24 hours and retry failed requests."
      >
        <WebhooksPanel
          settings={snapshot.settings.live_transcription_webhook}
          onOpenSettings={() => openSettings("advanced")}
        />
      </WorkspacePage>
    ),
    costs: (
      <Suspense fallback={<EmptyState title="Loading API usage…" />}>
        <TranscriptionCostsPage onBack={() => openSettings("models")} />
      </Suspense>
    ),
    settings: (
      <SettingsPage
        section={activeSettingsSection}
        onSectionChange={setActiveSettingsSection}
        snapshot={snapshot}
        actions={{
          onPatch: patchSettings,
          onSavePatch: commitSettingsPatch,
          onOpenTranscriptionCosts: () => setActiveRoute("costs"),
          onPermissionRequired: ensurePermission,
          onPreviewSubtitle: (patch) => previewSubtitle(patch),
          onSetMicrophoneInputVolume: (deviceId, volumePercent) =>
            void runAction(() =>
              setMicrophoneInputVolume(deviceId, volumePercent),
            ),
          onSuspendDictationShortcut: suspendDictationShortcut,
          onResumeDictationShortcut: resumeDictationShortcut,
          onPressedModifierShortcut: pressedModifierShortcut,
          onOpenDictationHistory: (entry) => {
            setDictationHistory({
              day: entry
                ? formatLocalTimestamp(entry.recorded_at).slice(0, 10)
                : null,
            });
            setActiveRoute("transcripts");
          },
          onChooseSaveRoot: () => void runAction(chooseSaveRoot),
          onRevealSaveFolder: () => void runAction(revealSaveFolder),
          onChooseModelDirectory: () => void runAction(chooseModelDirectory),
          onOpenDictionaryFile: () => void runAction(openDictionaryFile),
          onReloadDictionaryFile: () => void runAction(reloadDictionaryFile),
          onRequestAccessibilityPermission: () =>
            void runAction(openAccessibilityPermissionAssistant),
          onRequestMicrophonePermission: () =>
            void runAction(
              snapshot.permissions.microphone.can_request
                ? requestMicrophonePermission
                : openMicrophonePermissionSettings,
            ),
          onRequestScreenRecordingPermission: () =>
            void runAction(openScreenRecordingPermissionAssistant),
          onVerifyModel: (modelId) =>
            void runAction(() => verifyModel(modelId)),
          onDownloadModel: (modelId) =>
            void runAction(() => downloadModel(modelId)),
          onCancelModelDownload: (modelId) =>
            void runAction(() => cancelModelDownload(modelId)),
          onDeleteModel: (modelId) =>
            void runAction(() => deleteModel(modelId)),
          onSaveOpenRouterApiKey: async (apiKey) => {
            await runAction(() => saveOpenRouterApiKey(apiKey), true);
          },
          onDeleteOpenRouterApiKey: async () => {
            await runAction(deleteOpenRouterApiKey, true);
          },
          onSaveOpenAiApiKey: async (apiKey) => {
            await runAction(() => saveOpenAiApiKey(apiKey), true);
          },
          onDeleteOpenAiApiKey: async () => {
            await runAction(deleteOpenAiApiKey, true);
          },
          onSaveSonioxApiKey: async (apiKey) => {
            await runAction(() => saveSonioxApiKey(apiKey), true);
          },
          onDeleteSonioxApiKey: async () => {
            await runAction(deleteSonioxApiKey, true);
          },
        }}
      />
    ),
  };

  return (
    <AppFrame
      activeRoute={activeRoute}
      queueAttentionCount={activityAttention.attention?.count ?? 0}
      queueAttentionTone={activityAttention.attention?.tone ?? "warning"}
      onNavigate={(route) => {
        setDictationHistory(null);
        setActiveRoute(route);
      }}
      theme={themeMode}
      statusRail={
        activeRoute === "capture" &&
        dictationState.state === "idle" ? undefined : (
          <RecordingStatusRail
            liveActive={snapshot.status.live_input_active}
            latestText={latestTranscriptText}
            dictationState={dictationState.state}
            onCancelDictation={() => void cancelActiveDictation()}
            onReturnToCapture={() => setActiveRoute("capture")}
          />
        )
      }
    >
      {error ? (
        <Alert variant="destructive" className="mb-2.5">
          <CircleAlertIcon />
          <AlertTitle>WakeNote could not complete the last action</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AppPageRouter route={activeRoute} pages={pages} />
    </AppFrame>
  );
}
