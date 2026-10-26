import { invoke } from "@tauri-apps/api/core";
import {
  isTauriRuntime,
  loadSnapshot,
  saveSettingsPatch,
} from "../lib/tauri-client";
import type {
  AppSettings,
  AppStatus,
  MicrophoneDevice,
  SettingsPatch,
} from "../lib/types";

export interface QuickAudioState {
  settings: AppSettings;
  status: AppStatus;
  history: { primary: (number | null)[]; secondary: (number | null)[] };
}

export async function readAudioState(): Promise<QuickAudioState> {
  if (!isTauriRuntime()) {
    const snapshot = await loadSnapshot();
    return { ...snapshot, history: {
      primary: Array(32).fill(snapshot.status.microphone_captures.find(r => r.slot === "primary")?.level.current_dbfs ?? null),
      secondary: Array(32).fill(snapshot.status.microphone_captures.find(r => r.slot === "secondary")?.level.current_dbfs ?? null),
    } };
  }
  const [settings, status, history] = await Promise.all([
    invoke<AppSettings>("get_settings"),
    invoke<AppStatus>("app_status"),
    invoke<QuickAudioState["history"]>("quick_audio_history"),
  ]);
  return { settings, status, history };
}

export async function readMicrophones(): Promise<MicrophoneDevice[]> {
  if (!isTauriRuntime()) return (await loadSnapshot()).microphones;
  return invoke("list_microphones");
}

export async function writeAudioSettings(
  patch: SettingsPatch,
): Promise<AppSettings> {
  if (!isTauriRuntime()) return (await saveSettingsPatch(patch)).settings;
  return invoke("update_settings", { patch });
}

export async function hideAudioPanel() {
  if (isTauriRuntime()) await invoke("hide_quick_audio");
}
