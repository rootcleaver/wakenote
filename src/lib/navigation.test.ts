import { describe, expect, it } from "vitest";
import { PRIMARY_NAV, SETTINGS_SECTIONS } from "./navigation";

describe("navigation", () => {
  it("keeps Meetings and Transcripts as separate primary destinations", () => {
    expect(PRIMARY_NAV.map((item) => item.id)).toEqual([
      "capture",
      "live",
      "meetings",
      "transcripts",
      "reports",
      "activity",
      "webhooks",
    ]);
    const labels: readonly string[] = PRIMARY_NAV.map((item) => item.label);
    expect(labels).not.toContain("Library");
  });

  it("keeps configuration categories out of the primary task list", () => {
    expect(SETTINGS_SECTIONS.map((item) => item.id)).toEqual([
      "general",
      "audio",
      "dictation",
      "subtitles",
      "models",
      "storage",
      "integrations",
      "advanced",
    ]);
  });
});
