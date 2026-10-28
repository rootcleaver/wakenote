import { CustomSourceSettings } from "./CustomSourceSettings";
import { LiveWebhookSettings } from "./LiveWebhookSettings";
import { VoiceGateSettings } from "./VoiceGateSettings";
import {
  SettingSelect,
  SettingSwitch,
  SettingsCard,
  SettingsGrid,
} from "./settings-controls";
import { vadGateDisabledReason } from "./settings-helpers";
import type { AppSettings, ModelDescriptor } from "@/lib/types";

export function AdvancedSettings({
  settings,
  onPatch,
  onSavePatch,
  models = [],
}: {
  settings: AppSettings;
  onPatch: (patch: Partial<AppSettings>) => void;
  onSavePatch?: (patch: Partial<AppSettings>) => void | Promise<void>;
  models?: ModelDescriptor[];
}) {
  const compatibleModels = models.filter(
    (model) =>
      model.provider_runtime === "whisper-rs" ||
      ["openai-gpt-live-transcribe", "soniox-realtime-v5"].includes(model.id),
  );
  const modelOptions = [
    { value: "__default__", label: "Default transcription model" },
    ...compatibleModels.map((model) => ({
      value: model.id,
      label: model.display_name,
      disabled: !["ready", "installed", "unloaded"].includes(model.status),
    })),
  ];
  if (
    settings.auto_transcript_input_model &&
    !modelOptions.some(
      (model) => model.value === settings.auto_transcript_input_model,
    )
  ) {
    modelOptions.push({
      value: settings.auto_transcript_input_model,
      label: `${settings.auto_transcript_input_model} (Unavailable)`,
    });
  }
  return (
    <SettingsGrid maxColumns={2}>
      <LiveWebhookSettings
        settings={settings.live_transcription_webhook}
        onSave={onSavePatch ?? onPatch}
      />
      <SettingsCard
        title="Text output"
        description="Type stable speech into the active application while you speak."
      >
        <SettingSwitch
          label="Auto-type transcripts into cursor"
          checked={settings.auto_transcript_input_enabled}
          description="Uses Primary or merged microphones, never app audio or imported files. Requires running input, transcription, and Accessibility permission."
          onCheckedChange={(auto_transcript_input_enabled) =>
            onPatch({ auto_transcript_input_enabled })
          }
        />
        <SettingSelect
          label="Auto-type model"
          description="Choose an installed Whisper model or a configured cloud realtime model. Smaller local models usually respond faster. Final corrections stay in Activity; WakeNote never erases text you have edited."
          value={settings.auto_transcript_input_model || "__default__"}
          options={modelOptions}
          onValueChange={(model) =>
            onPatch({
              auto_transcript_input_model: model === "__default__" ? "" : model,
            })
          }
        />
        <SettingSwitch
          label="Type final transcript only"
          checked={settings.auto_transcript_input_final_only}
          disabled={!settings.auto_transcript_input_enabled}
          description="Types the same text the Capture list shows, once each phrase is final. Slower than typing stable partial words, but never stops on model corrections."
          onCheckedChange={(auto_transcript_input_final_only) =>
            onPatch({ auto_transcript_input_final_only })
          }
        />
        <SettingSwitch
          label="Add trailing space after auto-type"
          checked={settings.auto_transcript_input_trailing_space}
          disabled={!settings.auto_transcript_input_enabled}
          title={
            settings.auto_transcript_input_enabled
              ? "Append one space after each transcript."
              : "Enable auto-type before adding a trailing space."
          }
          onCheckedChange={(auto_transcript_input_trailing_space) =>
            onPatch({ auto_transcript_input_trailing_space })
          }
        />
      </SettingsCard>

      <VoiceGateSettings settings={settings} onPatch={onPatch} />

      <SettingsCard
        title="Transcript filtering"
        description="Expert transcript quality controls."
      >
        <SettingSwitch
          label="Hide low-confidence transcripts"
          checked={settings.suppress_low_confidence_transcripts}
          onCheckedChange={(suppress_low_confidence_transcripts) =>
            onPatch({ suppress_low_confidence_transcripts })
          }
        />
        <SettingSwitch
          label="VAD gate"
          description="Additional neural speech detection before transcription."
          checked={false}
          disabled
          title={vadGateDisabledReason()}
          onCheckedChange={() => {}}
        />
      </SettingsCard>

      <SettingsCard
        title="Custom system-audio sources"
        description="Window-title patterns and automatic capture behavior."
      >
        <CustomSourceSettings settings={settings} onPatch={onPatch} />
      </SettingsCard>
    </SettingsGrid>
  );
}
