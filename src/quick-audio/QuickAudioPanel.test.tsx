// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mockSnapshot } from "../lib/app-state";
import { QuickAudioPanel } from "./QuickAudioPanel";
import { readAudioState, readMicrophones, writeAudioSettings } from "./client";

vi.mock("./client", () => ({
  readAudioState: vi.fn(),
  readMicrophones: vi.fn(),
  writeAudioSettings: vi.fn(),
  hideAudioPanel: vi.fn(),
}));

const original = { ...mockSnapshot(), history: { primary: [] as (number | null)[], secondary: [] as (number | null)[] } };

async function openPanel() {
  await act(async () => { render(<QuickAudioPanel />); });
}

async function setThreshold(value: string) {
  const slider = screen.getByRole("slider");
  await act(async () => { fireEvent.change(slider, { target: { value } }); });
  await act(async () => { fireEvent.pointerUp(slider); });
}

describe("quick audio controls", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(readAudioState).mockResolvedValue(structuredClone(original));
    vi.mocked(readMicrophones).mockResolvedValue(original.microphones);
    vi.mocked(writeAudioSettings).mockImplementation(async (patch) => ({ ...original.settings, ...patch }));
  });
  afterEach(() => { cleanup(); vi.useRealTimers(); vi.resetAllMocks(); });

  it("commits threshold on release, rather than restarting capture on every drag event", async () => {
    await openPanel();
    const slider = screen.getByRole("slider");
    await act(async () => { fireEvent.change(slider, { target: { value: "-52" } }); });
    expect(writeAudioSettings).not.toHaveBeenCalled();
    await act(async () => { fireEvent.pointerUp(slider); });
    expect(writeAudioSettings).toHaveBeenCalledExactlyOnceWith({ threshold_dbfs: -52 });
    expect((slider as HTMLInputElement).value).toBe("-52");
  });

  it("discards a stale poll that finishes after a settings write", async () => {
    await openPanel();
    let complete!: (state: typeof original) => void;
    vi.mocked(readAudioState).mockImplementationOnce(() => new Promise((resolve) => { complete = resolve; }));
    await act(async () => { vi.advanceTimersByTime(250); });
    await setThreshold("-51");
    await act(async () => { complete(original); });
    expect((screen.getByRole("slider") as HTMLInputElement).value).toBe("-51");
  });

  it("keeps a failed save visible when later level polls succeed", async () => {
    await openPanel();
    vi.mocked(writeAudioSettings).mockRejectedValueOnce(new Error("Could not save settings"));
    await setThreshold("-50");
    await act(async () => { vi.advanceTimersByTime(500); });
    expect(screen.getByRole("alert").textContent).toContain("Could not save settings");
  });

  it("renders separate Primary and Secondary histories, with an explicit unconfigured slot", async () => {
    const live = { ...original, settings: { ...original.settings, pause_all: false },
      status: { ...original.status, microphone_captures: [{ slot: "primary" as const, device_id: "default", label: "Mic", active: true, reconnecting: false, warning: null, level: original.status.level }] },
      history: { primary: [...Array(30).fill(null), -60, -30], secondary: Array(32).fill(-10) },
    };
    vi.mocked(readAudioState).mockResolvedValue(live);
    await openPanel();
    expect(screen.getByRole("img", { name: /Primary input level/ }).querySelector("path")?.getAttribute("d")).toContain("L");
    expect(screen.getByRole("img", { name: /Secondary input level/ }).querySelector("path")?.getAttribute("d")).toBe("");
    expect(screen.getAllByText(/Not configured/).length).toBeGreaterThan(0);
  });

  it("uses different live histories for two configured microphones", async () => {
    const settings = { ...original.settings, pause_all: false, capture_microphones: [
      { id: "usb", label: "USB", core_audio_uid: "usb" },
      { id: "mac", label: "Mac", core_audio_uid: "mac" },
    ] };
    const runtime = (slot: "primary" | "secondary", id: string, db: number) => ({ slot, device_id: id, label: id, active: true, reconnecting: false, warning: null, level: { ...original.status.level, current_dbfs: db } });
    vi.mocked(readAudioState).mockResolvedValue({ ...original, settings,
      status: { ...original.status, live_input_active: true, microphone_captures: [runtime("primary", "usb", -25), runtime("secondary", "mac", -65)] },
      history: { primary: Array(32).fill(-25), secondary: Array(32).fill(-65) },
    });
    await openPanel();
    const primary = screen.getByRole("img", { name: /Primary input level/ }).querySelector("path")?.getAttribute("d");
    const secondary = screen.getByRole("img", { name: /Secondary input level/ }).querySelector("path")?.getAttribute("d");
    expect(primary).not.toBe(secondary);
    expect(screen.getByText("-25 dBFS")).toBeTruthy();
    expect(screen.getByText("-65 dBFS")).toBeTruthy();
  });

  it("clears the displayed trace when input stops, even if a poll contains old history", async () => {
    const live = { ...original,
      settings: { ...original.settings, pause_all: false },
      status: { ...original.status, live_input_active: true, microphone_captures: [{ slot: "primary" as const, device_id: "default", label: "Mic", active: true, reconnecting: false, warning: null, level: original.status.level }] },
      history: { primary: [...Array(30).fill(null), -60, -30], secondary: [] },
    };
    vi.mocked(readAudioState).mockResolvedValue(live);
    await openPanel();
    const graph = screen.getByRole("img", { name: /Primary input level/ });
    expect(graph.querySelector("path")?.getAttribute("d")).toContain("L");
    vi.mocked(readAudioState).mockResolvedValue({ ...live,
      status: { ...live.status, live_input_active: false, microphone_captures: [] },
    });
    await act(async () => { vi.advanceTimersByTime(250); });
    expect(graph.querySelector("path")?.getAttribute("d")).toBe("");
    expect(screen.getAllByText("Input unavailable")[0]).toBeTruthy();
  });
});
