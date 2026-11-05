import { Check, Copy, PlayIcon, SquareIcon } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { Button } from "@/components/ui/button";
import { shouldCopySelectedTranscriptsOnKeydown } from "@/components/TranscriptsView";
import {
  VoiceMarkButtons,
  speakerTag,
  useVoiceMarks,
} from "@/components/capture/LiveTranscriptList";
import { formatLocalTimestamp } from "@/lib/transcript-history";
import type { TranscriptEntry } from "@/lib/transcript-log";

/** How many phrases the live copy tab keeps on screen. */
export const LIVE_COPY_LIMIT = 200;

const STATUS_LABEL: Record<TranscriptEntry["status"], string> = {
  listening: "Listening",
  partial: "Live",
  queued: "Refining",
  final: "Final",
  failed: "Review",
};

export function liveCopyKey(entry: TranscriptEntry): string {
  const audioPath = entry.audio_path?.trim();
  return audioPath || `${entry.source_key ?? "microphone"}:${entry.chunk_id}`;
}

/** Newest-last list of distinct phrases (latest state per chunk wins). */
export function liveCopyEntries(
  entries: readonly TranscriptEntry[],
  limit = LIVE_COPY_LIMIT,
): TranscriptEntry[] {
  const seen = new Set<string>();
  const visible: TranscriptEntry[] = [];
  for (const entry of [...entries].reverse()) {
    const key = liveCopyKey(entry);
    if (seen.has(key)) continue;
    seen.add(key);
    if (!entry.text.trim()) continue;
    visible.push(entry);
    if (visible.length === limit) break;
  }
  return visible.reverse();
}

/** Text that lands on the clipboard: one phrase per line, in spoken order. */
export function formatLiveCopy(entries: readonly TranscriptEntry[]): string {
  return entries
    .map((entry) => entry.text.trim())
    .filter(Boolean)
    .join("\n");
}

function isCopyIgnoredTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  if (!element || typeof element.closest !== "function") return false;
  return Boolean(
    element.closest("input, textarea, select, [contenteditable='true']"),
  );
}

type DragMode = "select" | "deselect";

export function LiveCopyPage({
  entries,
  liveActive,
  busy,
  onStart,
  onStop,
  voiceGate = false,
  voiceGateThreshold,
}: {
  entries: readonly TranscriptEntry[];
  liveActive: boolean;
  busy: boolean;
  onStart: () => void;
  onStop: () => void;
  /** Show the voice gate verdict and the "This is me" / "Not me" actions. */
  voiceGate?: boolean;
  /** Match threshold shown next to the scores. */
  voiceGateThreshold?: number;
}) {
  const visible = useMemo(() => liveCopyEntries(entries), [entries]);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [copied, setCopied] = useState<"selected" | "all" | null>(null);
  const [follow, setFollow] = useState(true);
  const anchorRef = useRef<string | null>(null);
  const dragRef = useRef<{ mode: DragMode; visited: Set<string> } | null>(null);
  const listRef = useRef<HTMLOListElement | null>(null);
  const voice = useVoiceMarks();
  const enrollError = voice.error;

  // Drop selections whose phrase is no longer on screen.
  useEffect(() => {
    const keys = new Set(visible.map(liveCopyKey));
    setSelected((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const key of prev) {
        if (keys.has(key)) next.add(key);
        else changed = true;
      }
      return changed ? next : prev;
    });
  }, [visible]);

  // Keep the newest phrase in view while following.
  useEffect(() => {
    if (!follow) return;
    const list = listRef.current;
    if (list) list.scrollTop = list.scrollHeight;
  }, [visible, follow]);

  useEffect(() => {
    const stop = () => {
      dragRef.current = null;
    };
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
    return () => {
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
    };
  }, []);

  const selectedEntries = useMemo(
    () => visible.filter((entry) => selected.has(liveCopyKey(entry))),
    [visible, selected],
  );

  const writeClipboard = useCallback(
    async (text: string, kind: "selected" | "all") => {
      if (!text) return;
      try {
        await navigator.clipboard.writeText(text);
        setCopied(kind);
        window.setTimeout(() => setCopied(null), 1500);
      } catch {
        // Clipboard unavailable — no feedback.
      }
    },
    [],
  );

  const copySelected = useCallback(() => {
    void writeClipboard(formatLiveCopy(selectedEntries), "selected");
  }, [selectedEntries, writeClipboard]);

  const copyAll = useCallback(() => {
    void writeClipboard(formatLiveCopy(visible), "all");
  }, [visible, writeClipboard]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && selected.size > 0) {
        setSelected(new Set());
        anchorRef.current = null;
        return;
      }
      if (!shouldCopySelectedTranscriptsOnKeydown(event, selectedEntries.length)) {
        return;
      }
      if (isCopyIgnoredTarget(event.target)) return;
      // Let a native text selection inside a row win over row selection.
      const nativeSelection = window.getSelection?.()?.toString() ?? "";
      if (nativeSelection.trim()) return;
      event.preventDefault();
      copySelected();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [copySelected, selected.size, selectedEntries.length]);

  const beginSelect = (key: string, event: ReactPointerEvent) => {
    if (event.button !== 0) return;
    if (event.shiftKey && anchorRef.current) {
      const keys = visible.map(liveCopyKey);
      const from = keys.indexOf(anchorRef.current);
      const to = keys.indexOf(key);
      if (from >= 0 && to >= 0) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        setSelected((prev) => {
          const next = new Set(prev);
          for (const k of keys.slice(lo, hi + 1)) next.add(k);
          return next;
        });
        event.preventDefault();
        return;
      }
    }
    anchorRef.current = key;
    setFollow(false);
    setSelected((prev) => {
      const next = new Set(prev);
      const mode: DragMode = prev.has(key) ? "deselect" : "select";
      if (mode === "select") next.add(key);
      else next.delete(key);
      dragRef.current = { mode, visited: new Set([key]) };
      return next;
    });
    event.preventDefault();
  };

  const continueSelect = (key: string) => {
    const drag = dragRef.current;
    if (!drag || drag.visited.has(key)) return;
    drag.visited.add(key);
    setSelected((prev) => {
      const next = new Set(prev);
      if (drag.mode === "select") next.add(key);
      else next.delete(key);
      return next;
    });
  };

  const onScroll = () => {
    const list = listRef.current;
    if (!list) return;
    const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 24;
    if (atBottom !== follow) setFollow(atBottom);
  };

  return (
    <div data-slot="live-copy-page" className="live-copy">
      <div className="live-copy__bar">
        {liveActive ? (
          <Button
            type="button"
            size="sm"
            variant="destructive"
            disabled={busy}
            onClick={onStop}
          >
            <SquareIcon data-icon="inline-start" />
            Stop
          </Button>
        ) : (
          <Button type="button" size="sm" disabled={busy} onClick={onStart}>
            <PlayIcon data-icon="inline-start" />
            Start
          </Button>
        )}
        <span className="live-copy__hint">
          {selectedEntries.length > 0
            ? `${selectedEntries.length} selected · ⌘C`
            : "Drag rows · Shift-click · ⌘C"}
        </span>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={selectedEntries.length === 0}
          onClick={copySelected}
          title="Copy selected phrases (⌘C)"
        >
          {copied === "selected" ? <Check /> : <Copy />}
          {copied === "selected" ? "Copied" : "Copy"}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          disabled={visible.length === 0}
          onClick={copyAll}
          title="Copy every phrase on screen"
        >
          {copied === "all" ? "Copied" : "All"}
        </Button>
      </div>
      {visible.length === 0 ? (
        <p className="live-copy__empty">
          {liveActive ? "Listening for speech…" : "Start input to see live phrases here."}
        </p>
      ) : (
        <ol
          ref={listRef}
          className="live-copy__list"
          aria-label="Live phrases"
          onScroll={onScroll}
        >
          {visible.map((entry) => {
            const key = liveCopyKey(entry);
            const isSelected = selected.has(key);
            return (
              <li
                key={key}
                data-status={entry.status}
                data-selected={isSelected ? "true" : undefined}
                aria-selected={isSelected}
                onPointerDown={(event) => beginSelect(key, event)}
                onPointerEnter={() => continueSelect(key)}
              >
                <span className="live-copy__meta">
                  {entry.status === "final"
                    ? formatLocalTimestamp(entry.recorded_at || entry.started_at) || "Final"
                    : STATUS_LABEL[entry.status]}
                  {voiceGate
                    ? speakerTag(entry, voice.markFor(entry), voiceGateThreshold)
                    : null}
                  {voiceGate ? <VoiceMarkButtons entry={entry} voice={voice} /> : null}
                </span>
                <span className="live-copy__text">{entry.text}</span>
              </li>
            );
          })}
        </ol>
      )}
      {voiceGate && enrollError ? (
        <p className="live-copy__empty" role="alert">
          Voice enroll: {enrollError}
        </p>
      ) : null}
      {!follow && visible.length > 0 ? (
        <Button
          type="button"
          size="sm"
          variant="secondary"
          className="live-copy__follow"
          onClick={() => setFollow(true)}
        >
          Jump to newest
        </Button>
      ) : null}
    </div>
  );
}
