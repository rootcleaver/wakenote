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
}: {
  entries: readonly TranscriptEntry[];
  liveActive: boolean;
  now?: Date;
  /** Show the voice gate verdict and a "This is me" enrollment action. */
  voiceGate?: boolean;
}) {
  const visibleEntries = newestLiveTranscriptEntries(entries);
  const [enrolled, setEnrolled] = useState<ReadonlySet<string>>(new Set());
  const [enrollError, setEnrollError] = useState<string | null>(null);

  async function enroll(audioPath: string) {
    try {
      await enrollVoiceGate(audioPath);
      setEnrolled((previous) => new Set(previous).add(audioPath));
      setEnrollError(null);
    } catch (error) {
      setEnrollError(String(error));
    }
  }

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
            {voiceGate ? speakerTag(entry) : null}
            {voiceGate && canEnroll(entry) ? (
              <button
                type="button"
                data-slot="voice-enroll"
                disabled={enrolled.has(entry.audio_path ?? "")}
                title="Add this phrase to your enrolled voice"
                onClick={() => void enroll(entry.audio_path ?? "")}
              >
                {enrolled.has(entry.audio_path ?? "") ? "Enrolled" : "This is me"}
              </button>
            ) : null}
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

function canEnroll(entry: TranscriptEntry): boolean {
  return (
    entry.status === "final" &&
    Boolean(entry.audio_path) &&
    (entry.source_key ?? "microphone").startsWith("microphone") &&
    entry.microphone_slot !== "secondary"
  );
}

function speakerTag(entry: TranscriptEntry): ReactNode {
  if (entry.status !== "final" || !entry.speaker) return null;
  const score =
    typeof entry.speaker_score === "number"
      ? ` ${entry.speaker_score.toFixed(2)}`
      : "";
  const label =
    entry.speaker === "other"
      ? `Other speaker${score}`
      : entry.speaker === "me"
        ? `Me${score}`
        : "Unjudged";
  return (
    <span data-slot="speaker-tag" data-speaker={entry.speaker}>
      {label}
    </span>
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
