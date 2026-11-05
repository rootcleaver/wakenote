export type PrimaryRoute =
  | "capture"
  | "live"
  | "meetings"
  | "transcripts"
  | "reports"
  | "activity"
  | "webhooks"
  | "costs"
  | "settings";

export type SettingsSection =
  | "general"
  | "audio"
  | "dictation"
  | "subtitles"
  | "models"
  | "storage"
  | "integrations"
  | "advanced";

export const PRIMARY_NAV = [
  { id: "capture", label: "Capture" },
  { id: "live", label: "Live copy" },
  { id: "meetings", label: "Meetings" },
  { id: "transcripts", label: "Transcripts" },
  { id: "reports", label: "Reports" },
  { id: "activity", label: "Activity" },
  { id: "webhooks", label: "Webhooks" },
] as const satisfies ReadonlyArray<{
  id: Exclude<PrimaryRoute, "settings">;
  label: string;
}>;

export const SETTINGS_SECTIONS = [
  { id: "general", label: "General" },
  { id: "audio", label: "Audio" },
  { id: "dictation", label: "Dictation" },
  { id: "subtitles", label: "Subtitles" },
  { id: "models", label: "Models" },
  { id: "storage", label: "Storage" },
  { id: "integrations", label: "Integrations" },
  { id: "advanced", label: "Advanced" },
] as const satisfies ReadonlyArray<{ id: SettingsSection; label: string }>;
