import { AudioLinesIcon } from "lucide-react";
import { useState, type ReactNode } from "react";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { newestLiveTranscriptEntries } from "@/lib/live-transcripts";
import { enrollVoiceGate } from "@/lib/tauri-client";
import {
  appendRecentAge,
  fileUrlFromPath,
  formatLocalTimestamp,
} from "@/lib/transcript-history";
import type {
  TranscriptEntry,
  TranscriptEntryStatus,
} from "@/lib/transcript-log";

const STATUS_LABEL: Record<TranscriptEntryStatus, string> = {
  listening: "Listening",
  partial: "Live",
  queued: "Refining",
  final: "Final",
  failed: "Review",
};

export function LiveTranscriptList({
  entries,
  liveActive,
  now = new Date(),
  voiceGate = false,
  voiceGateThreshold,
}: {
  entries: readonly TranscriptEntry[];
  liveActive: boolean;
  now?: Date;
  /** Show the voice gate verdict and the "This is me" / "Not me" actions. */
  voiceGate?: boolean;
  /** Match threshold shown next to the scores. */
  voiceGateThreshold?: number;
}) {
  const visibleEntries = newestLiveTranscriptEntries(entries);
  const voice = useVoiceMarks();
  const enrollError = voice.error;

  if (visibleEntries.length === 0) {
    return (
      <Empty className="live-transcript-empty">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <AudioLinesIcon />
          </EmptyMedia>
          <EmptyTitle>
            {liveActive ? "Listening for speech" : "No speech captured yet"}
          </EmptyTitle>
          <EmptyDescription>
            {liveActive
              ? "The latest decoded phrase will appear here."
              : "Start input to build a live transcript."}
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }

  return (
    <ol data-slot="live-transcript-list" aria-label="Live transcription">
      {visibleEntries.map((entry) => (
        <li
          key={`${entry.source_key ?? "microphone"}:${entry.chunk_id}:${entry.audio_path ?? entry.started_at}`}
          data-status={entry.status}
        >
          <span data-slot="transcript-meta">
            {entry.source_label ? (
              <span data-slot="transcript-source">{entry.source_label}</span>
            ) : null}
            {entry.source_label ? " · " : null}
            {labelForEntry(entry)}
          </span>
          <span data-slot="transcript-text">
            {textForEntry(entry, now)}
            {voiceGate
              ? speakerTag(entry, voice.markFor(entry), voiceGateThreshold)
              : null}
            {voiceGate ? <VoiceMarkButtons entry={entry} voice={voice} /> : null}
          </span>
        </li>
      ))}
      {voiceGate && enrollError ? (
        <li data-status="failed">
          <span data-slot="transcript-meta">Voice enroll</span>
          <span data-slot="transcript-text">{enrollError}</span>
        </li>
      ) : null}
    </ol>
  );
}

export type VoiceMark = "me" | "other";

export interface VoiceMarks {
  error: string | null;
  markFor: (entry: TranscriptEntry) => VoiceMark | undefined;
  mark: (audioPath: string, kind: VoiceMark) => Promise<void>;
}

/** Per-list memory of "This is me" / "Not me" presses (the backend keeps the learning). */
export function useVoiceMarks(): VoiceMarks {
  const [marks, setMarks] = useState<ReadonlyMap<string, VoiceMark>>(new Map());
  const [error, setError] = useState<string | null>(null);
  return {
    error,
    markFor: (entry) => (entry.audio_path ? marks.get(entry.audio_path) : undefined),
    mark: async (audioPath, kind) => {
      try {
        await enrollVoiceGate(audioPath, { notMe: kind === "other" });
        setMarks((previous) => new Map(previous).set(audioPath, kind));
        setError(null);
      } catch (caught) {
        setError(String(caught));
      }
    },
  };
}

/** "This is me" / "Not me" pair; once pressed, the pressed one shows as done. */
export function VoiceMarkButtons({
  entry,
  voice,
}: {
  entry: TranscriptEntry;
  voice: VoiceMarks;
}) {
  if (!canEnroll(entry)) return null;
  const audioPath = entry.audio_path ?? "";
  const marked = voice.markFor(entry);
  // Keep row drag-selection (Live copy) from starting on these buttons.
  const stop = (event: { stopPropagation: () => void }) => event.stopPropagation();
  return (
    <>
      <button
        type="button"
        data-slot="voice-enroll"
        data-kind="me"
        disabled={marked !== undefined}
        title="Add this phrase to your enrolled voice"
        onPointerDown={stop}
        onClick={(event) => {
          stop(event);
          void voice.mark(audioPath, "me");
        }}
      >
        {marked === "me" ? "Enrolled" : "This is me"}
      </button>
      <button
        type="button"
        data-slot="voice-enroll"
        data-kind="other"
        disabled={marked !== undefined}
        title="Remember this phrase as someone else's voice"
        onPointerDown={stop}
        onClick={(event) => {
          stop(event);
          void voice.mark(audioPath, "other");
        }}
      >
        {marked === "other" ? "Marked not me" : "Not me"}
      </button>
    </>
  );
}

export function canEnroll(entry: TranscriptEntry): boolean {
  return (
    entry.status === "final" &&
    Boolean(entry.audio_path) &&
    (entry.source_key ?? "microphone").startsWith("microphone") &&
    entry.microphone_slot !== "secondary"
  );
}

/** "you 0.83 · not-me 0.41 · cut 0.45" — the numbers the verdict came from. */
export function speakerScoreText(
  entry: TranscriptEntry,
  threshold?: number,
): string | null {
  const parts: string[] = [];
  if (typeof entry.speaker_score === "number") {
    parts.push(`you ${entry.speaker_score.toFixed(2)}`);
  }
  if (typeof entry.speaker_other_score === "number") {
    parts.push(`not-me ${entry.speaker_other_score.toFixed(2)}`);
  }
  if (parts.length > 0 && typeof threshold === "number") {
    parts.push(`cut ${threshold.toFixed(2)}`);
  }
  if (parts.length === 0 && entry.speaker === "unknown" && entry.speaker_reason) {
    parts.push(entry.speaker_reason);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

export function speakerTag(
  entry: TranscriptEntry,
  marked?: VoiceMark,
  threshold?: number,
): ReactNode {
  if (entry.status !== "final") return null;
  if (!marked && !entry.speaker) return null;
  const kind = marked ?? entry.speaker;
  const label = marked
    ? marked === "me"
      ? "Me (you)"
      : "Other (you)"
    : kind === "other"
      ? "Other speaker"
      : kind === "me"
        ? "Me"
        : "Unjudged";
  const scores = speakerScoreText(entry, threshold);
  return (
    <>
      <span
        data-slot="speaker-tag"
        data-speaker={kind}
        title={entry.speaker_reason ?? undefined}
      >
        {label}
      </span>
      {scores ? <span data-slot="speaker-score">{scores}</span> : null}
    </>
  );
}

function labelForEntry(entry: TranscriptEntry): ReactNode {
  if (entry.status !== "final") {
    return STATUS_LABEL[entry.status];
  }

  const timestamp =
    formatLocalTimestamp(entry.recorded_at || entry.started_at) || "Final";
  if (!entry.audio_path) {
    return timestamp;
  }
  return (
    <a href={fileUrlFromPath(entry.audio_path)} title={entry.audio_path}>
      {timestamp}
    </a>
  );
}

function textForEntry(entry: TranscriptEntry, now: Date): string {
  if (entry.status === "listening" && !entry.text.trim()) {
    return "Waiting for speech";
  }
  if (entry.status === "failed" && !entry.text.trim()) {
    return "Final transcript needs review";
  }
  if (entry.status !== "final") {
    return entry.text;
  }
  return appendRecentAge(
    entry.text,
    entry.recorded_at || entry.started_at,
    now,
  );
}
