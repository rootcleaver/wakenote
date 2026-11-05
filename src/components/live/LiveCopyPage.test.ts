import { describe, expect, it } from "vitest";
import type { TranscriptEntry } from "@/lib/transcript-log";
import { clearLiveCopy, formatLiveCopy, liveCopyEntries, withoutCleared } from "./LiveCopyPage";

function entry(chunk: number, status: TranscriptEntry["status"], text: string, audio: string | null = null): TranscriptEntry {
  return { chunk_id: chunk, status, text, started_at: "", recorded_at: "", audio_path: audio, error: null };
}

describe("live copy", () => {
  it("keeps the latest state per chunk, drops empty phrases, newest last", () => {
    const list = liveCopyEntries([
      entry(1, "partial", "hel"),
      entry(1, "final", "hello"),
      entry(2, "listening", ""),
      entry(3, "partial", "world"),
    ]);
    expect(list.map((e) => e.text)).toEqual(["hello", "world"]);
  });

  it("caps the list to the newest phrases", () => {
    const many = Array.from({ length: 5 }, (_, i) => entry(i, "final", `p${i}`));
    expect(liveCopyEntries(many, 2).map((e) => e.text)).toEqual(["p3", "p4"]);
  });

  it("copies one phrase per line in spoken order", () => {
    expect(formatLiveCopy([entry(1, "final", " a "), entry(2, "final", "b")])).toBe("a\nb");
  });

  it("clears settled phrases from view only, keeps in-progress and later phrases", () => {
    const cleared = new Set<string>();
    const before = [entry(1, "final", "one", "/a/1.wav"), entry(2, "partial", "tw")];
    expect(clearLiveCopy(before, cleared)).toBe(1);
    expect(withoutCleared(before, cleared).map((e) => e.text)).toEqual(["tw"]);
    const later = [...before, entry(2, "final", "two", "/a/2.wav"), entry(3, "final", "three", "/a/3.wav")];
    expect(withoutCleared(later, cleared).map((e) => e.text)).toEqual(["tw", "two", "three"]);
    expect(later).toHaveLength(4);
  });
});
