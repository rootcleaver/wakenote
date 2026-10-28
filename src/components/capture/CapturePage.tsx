import {
  AudioLinesIcon,
  PlayIcon,
  RefreshCwIcon,
  Settings2Icon,
  SquareIcon,
} from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";
import { StatusBadge } from "@/components/ui/status-badge";
import { useWaveformHistory } from "@/hooks/use-waveform-history";
import {
  inputAvailability,
  isLiveInputStreamErrored,
  startLiveCaptureDisabledReason,
  stopLiveCaptureDisabledReason,
} from "@/lib/capture-controls";
import { calibrationSettingsPatch } from "@/lib/calibration";
import { resetRecordingSettingsPatch } from "@/lib/recording-settings";
import {
  captureStatusPresentation,
  trayStateBadgeTone,
} from "@/lib/status-summary";
import { humanizeTrayState } from "@/lib/transcript-history";
import type { TranscriptEntry } from "@/lib/transcript-log";
import type { AppSettings, AppSnapshot } from "@/lib/types";
import { livePreviewPresentation } from "@/lib/live-transcripts";
import type { ActivityAttention } from "@/lib/activity-attention";
import { PageHeader } from "@/components/shell/PageHeader";
import { Onboarding } from "@/components/Onboarding";
import { LiveTranscriptList } from "./LiveTranscriptList";
import { QuickCalibration } from "./QuickCalibration";
import {
  RecorderWaveform,
  type RecorderWaveformState,
} from "./RecorderWaveform";

export interface CapturePageProps {
  snapshot: AppSnapshot;
  activityAttention: ActivityAttention | null;
  transcriptEntries: TranscriptEntry[];
  busy: boolean;
  onStart: () => void;
  onStop: () => void;
  onRefresh: () => void;
  onPatch: (patch: Partial<AppSettings>) => void;
  onOpenAudioSettings: () => void;
}

export function CapturePage({
  snapshot,
  activityAttention,
  transcriptEntries,
  busy,
  onStart,
  onStop,
  onRefresh,
  onPatch,
  onOpenAudioSettings,
}: CapturePageProps) {
  const { settings, status, microphones } = snapshot;
  const recording = status.tray_state === "recording";
  const waveform = useWaveformHistory(recording);
  const input = inputAvailability(settings, microphones);
  const streamErrored = isLiveInputStreamErrored(status);
  const showStop = status.live_input_active && !streamErrored;
  const startReason = startLiveCaptureDisabledReason(settings, status, input.canStart);
  const stopReason = stopLiveCaptureDisabledReason(status);
  const presentation = captureStatusPresentation(snapshot, activityAttention);
  const previewPresentation = livePreviewPresentation(
    settings.selected_model,
    snapshot.models,
  );
  const waveformState: RecorderWaveformState = recording
    ? "recording"
    : waveform.phase === "decaying"
      ? "decaying"
      : status.live_input_active
        ? "listening"
        : "idle";
  const currentDbfs = status.live_input_active ? status.level.current_dbfs : -120;
  const peakDbfs = status.live_input_active ? status.level.peak_dbfs : -120;

  return (
    <div data-slot="capture-page" className="capture-page">
      <PageHeader
        eyebrow="Recorder"
        title="Capture"
        description="Voice-triggered recording and live transcription."
        actions={
          <Button type="button" size="sm" variant="outline" disabled={busy} onClick={onRefresh}>
            {busy ? (
              <Spinner data-icon="inline-start" />
            ) : (
              <RefreshCwIcon data-icon="inline-start" />
            )}
            Refresh
          </Button>
        }
      />

      <Onboarding
        settings={settings}
        models={snapshot.models}
        microphones={microphones}
      />

      {presentation.warning ? (
        <Alert
          data-tone={presentation.warning.tone}
          variant={presentation.warning.tone === "danger" ? "destructive" : "default"}
        >
          <AudioLinesIcon />
          <AlertTitle>Capture attention required</AlertTitle>
          <AlertDescription>{presentation.warning.message}</AlertDescription>
        </Alert>
      ) : null}

      <Card data-slot="recorder-card" className="recorder-card">
        <CardHeader>
          <CardTitle>{presentation.headline}</CardTitle>
          <CardDescription className="overflow-wrap-anywhere">
            {input.activeLabel} · {status.active_model} · {settings.threshold_dbfs} dBFS
          </CardDescription>
          <CardAction>
            <StatusBadge tone={trayStateBadgeTone(status.tray_state)}>
              {humanizeTrayState(status.tray_state)}
            </StatusBadge>
          </CardAction>
        </CardHeader>
        <CardContent>
          <RecorderWaveform
            levels={waveform.levels}
            currentDbfs={currentDbfs}
            peakDbfs={peakDbfs}
            thresholdDbfs={settings.threshold_dbfs}
            state={waveformState}
          />
          {input.warning ? (
            <Alert data-tone={input.warningTone} variant={input.warningTone === "danger" ? "destructive" : "default"}>
              <AudioLinesIcon />
              <AlertTitle>{input.warningTone === "danger" ? "Input unavailable" : "Input fallback"}</AlertTitle>
              <AlertDescription>{input.warning}</AlertDescription>
            </Alert>
          ) : null}
        </CardContent>
        <CardFooter className="recorder-card__footer">
          {showStop ? (
            <Button
              type="button"
              size="lg"
              variant="destructive"
              disabled={busy || stopReason !== null}
              title={busy ? "Capture action in progress" : stopReason ?? undefined}
              onClick={onStop}
            >
              <SquareIcon data-icon="inline-start" />
              Stop Input
            </Button>
          ) : (
            <Button
              type="button"
              size="lg"
              disabled={busy || startReason !== null}
              title={busy ? "Capture action in progress" : startReason ?? undefined}
              onClick={onStart}
            >
              <PlayIcon data-icon="inline-start" />
              Start Input
            </Button>
          )}
          <Button type="button" size="sm" variant="ghost" onClick={onOpenAudioSettings}>
            <Settings2Icon data-icon="inline-start" />
            Audio settings
          </Button>
        </CardFooter>
      </Card>

      <Card size="sm" data-slot="live-transcript-card" className="live-transcript-card">
        <CardHeader>
          <CardTitle>Live transcript</CardTitle>
          <CardDescription>{previewPresentation.description}</CardDescription>
        </CardHeader>
        <CardContent>
          <LiveTranscriptList
            entries={transcriptEntries}
            liveActive={status.live_input_active}
            voiceGate={settings.voice_gate_enabled}
          />
        </CardContent>
      </Card>

      <QuickCalibration
        settings={settings}
        status={status}
        onApplyThreshold={(thresholdDbfs) =>
          onPatch(calibrationSettingsPatch(thresholdDbfs))
        }
        onResetRecordingSettings={() => onPatch(resetRecordingSettingsPatch())}
      />
    </div>
  );
}
