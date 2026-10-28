/**
 * Transcript log state machine for the live-transcription footer.
 *
 * The backend fires five Tauri events per chunk lifecycle:
 *   started -> partial(s) -> committed -> final | failed
 * The footer wants to show one entry per chunk, transitioning through:
 *   listening -> partial -> queued (chunk closed, awaiting full pass) -> final
 * Final replaces partial text once the queue worker's accurate decode lands.
 */

export type TranscriptEntryStatus =
  | "listening"
  | "partial"
  | "queued"
  | "final"
  | "failed";

export interface TranscriptEntry {
  source_key?: string;
  source_label?: string;
  microphone_slot?: "primary" | "secondary" | null;
  chunk_id: number;
  status: TranscriptEntryStatus;
  text: string;
  started_at: string;
  recorded_at: string;
  audio_path: string | null;
  error: string | null;
  /** Voice gate verdict on the final chunk; absent when the gate is off. */
  speaker?: SpeakerKind | null;
  speaker_score?: number | null;
}

export type SpeakerKind = "me" | "other" | "unknown";

export type TranscriptEvent =
  | (SourceIdentity & { type: "started"; chunk_id: number; started_at: string })
  | (SourceIdentity & { type: "partial"; chunk_id: number; text: string })
  | (SourceIdentity & {
      type: "committed";
      chunk_id: number;
      audio_path: string;
      will_transcribe?: boolean;
    })
  | ({
      type: "final";
      chunk_id: number | null;
      audio_path: string;
      text: string;
      recorded_at?: string;
      speaker?: SpeakerKind | null;
      speaker_score?: number | null;
    } & SourceIdentity)
  | ({
      type: "failed";
      chunk_id: number | null;
      audio_path: string;
      error: string;
      recorded_at?: string;
    } & SourceIdentity);

interface SourceIdentity {
  source_key?: string;
  source_label?: string;
  microphone_slot?: "primary" | "secondary" | null;
}

export const TRANSCRIPT_LOG_LIMIT = 12;

export function reduceTranscriptLog(
  entries: TranscriptEntry[],
  event: TranscriptEvent,
): TranscriptEntry[] {
  switch (event.type) {
    case "started":
      return appendOrReplace(entries, event.chunk_id, () => ({
        ...sourceIdentity(event),
        chunk_id: event.chunk_id,
        status: "listening",
        text: "",
        started_at: event.started_at,
        recorded_at: event.started_at,
        audio_path: null,
        error: null,
      }));

    case "partial":
      if (!entries.some((entry) => sameChunk(entry, event))) {
        const timestamp = new Date().toISOString();
        return appendOrReplace(entries, event.chunk_id, () => ({
          ...sourceIdentity(event),
          chunk_id: event.chunk_id,
          status: "partial",
          text: event.text,
          started_at: timestamp,
          recorded_at: timestamp,
          audio_path: null,
          error: null,
        }));
      }
      return updateEntry(entries, event, (entry) => {
        if (entry.status === "final" || entry.status === "failed") {
          return entry;
        }
        return {
          ...entry,
          status: entry.status === "queued" ? "queued" : "partial",
          text: event.text,
        };
      });

    case "committed":
      if (event.will_transcribe === false) {
        return entries.filter((entry) => !sameChunk(entry, event));
      }
      return updateEntry(entries, event, (entry) =>
        entry.status === "final" || entry.status === "failed"
          ? entry
          : {
              ...entry,
              status: "queued",
              audio_path: event.audio_path,
            },
      );

    case "final":
      return updateOrAppendByAudio(
        entries,
        event.chunk_id,
        event.audio_path,
        event,
        (entry) => {
          const recordedAt =
            entry?.recorded_at ||
            entry?.started_at ||
            event.recorded_at ||
            new Date().toISOString();
          return {
            ...(entry ??
              defaultEntry(
                event.chunk_id,
                event.audio_path,
                event.recorded_at,
              )),
            ...sourceIdentity(event),
            chunk_id: event.chunk_id ?? entry?.chunk_id ?? -1,
            status: "final",
            text: event.text,
            recorded_at: recordedAt,
            audio_path: event.audio_path,
            error: null,
            speaker: event.speaker ?? null,
            speaker_score: event.speaker_score ?? null,
          };
        },
      );

    case "failed":
      return updateOrAppendByAudio(
        entries,
        event.chunk_id,
        event.audio_path,
        event,
        (entry) => {
          const recordedAt =
            entry?.recorded_at ||
            entry?.started_at ||
            event.recorded_at ||
            new Date().toISOString();
          return {
            ...(entry ??
              defaultEntry(
                event.chunk_id,
                event.audio_path,
                event.recorded_at,
              )),
            ...sourceIdentity(event),
            chunk_id: event.chunk_id ?? entry?.chunk_id ?? -1,
            status: "failed",
            text: entry?.text ?? "",
            recorded_at: recordedAt,
            audio_path: event.audio_path,
            error: event.error,
          };
        },
      );
  }
}

function defaultEntry(
  chunk_id: number | null,
  audio_path: string,
  recordedAt?: string,
): TranscriptEntry {
  const timestamp = recordedAt || new Date().toISOString();
  return {
    chunk_id: chunk_id ?? -1,
    status: "queued",
    text: "",
    started_at: timestamp,
    recorded_at: timestamp,
    audio_path,
    error: null,
  };
}

function appendOrReplace(
  entries: TranscriptEntry[],
  chunk_id: number,
  build: () => TranscriptEntry,
): TranscriptEntry[] {
  const nextEntry = build();
  const existingIndex = entries.findIndex(
    (entry) =>
      entry.chunk_id === chunk_id &&
      (entry.source_key ?? "microphone") ===
        (nextEntry.source_key ?? "microphone"),
  );
  const next =
    existingIndex >= 0
      ? entries.map((entry, index) =>
          index === existingIndex ? nextEntry : entry,
        )
      : [...entries, nextEntry];
  return trimToLimit(next);
}

function updateEntry(
  entries: TranscriptEntry[],
  event: SourceIdentity & { chunk_id: number },
  patch: (entry: TranscriptEntry) => TranscriptEntry,
): TranscriptEntry[] {
  if (!entries.some((entry) => sameChunk(entry, event))) {
    return entries;
  }
  return entries.map((entry) =>
    sameChunk(entry, event) ? patch(entry) : entry,
  );
}

function updateOrAppendByAudio(
  entries: TranscriptEntry[],
  chunk_id: number | null,
  audio_path: string,
  identity: SourceIdentity,
  patch: (entry: TranscriptEntry | undefined) => TranscriptEntry,
): TranscriptEntry[] {
  let index = -1;
  if (chunk_id != null) {
    index = entries.findIndex(
      (entry) =>
        entry.chunk_id === chunk_id &&
        (entry.source_key ?? "microphone") ===
          (identity.source_key ?? "microphone"),
    );
  }
  if (index < 0) {
    index = entries.findIndex((entry) => entry.audio_path === audio_path);
  }

  if (index >= 0) {
    const existing = entries[index];
    return entries.map((entry, i) => (i === index ? patch(existing) : entry));
  }

  return trimToLimit([...entries, patch(undefined)]);
}

function sourceIdentity(event: SourceIdentity): SourceIdentity {
  return {
    source_key: event.source_key ?? "microphone",
    source_label: event.source_label ?? "Mic",
    microphone_slot: event.microphone_slot ?? null,
  };
}

function sameChunk(
  entry: TranscriptEntry,
  event: SourceIdentity & { chunk_id: number },
): boolean {
  return (
    entry.chunk_id === event.chunk_id &&
    (entry.source_key ?? "microphone") === (event.source_key ?? "microphone")
  );
}

function trimToLimit(entries: TranscriptEntry[]): TranscriptEntry[] {
  if (entries.length <= TRANSCRIPT_LOG_LIMIT) {
    return entries;
  }
  return entries.slice(entries.length - TRANSCRIPT_LOG_LIMIT);
}
