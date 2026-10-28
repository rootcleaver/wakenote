import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import {
  downloadVoiceGateModel,
  loadVoiceGateStatus,
  resetVoiceGate,
  type VoiceGateStatus,
} from "@/lib/tauri-client";
import type { AppSettings } from "@/lib/types";
import { SettingSlider, SettingSwitch, SettingsCard } from "./settings-controls";

export function VoiceGateSettings({
  settings,
  onPatch,
}: {
  settings: AppSettings;
  onPatch: (patch: Partial<AppSettings>) => void;
}) {
  const [status, setStatus] = useState<VoiceGateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    loadVoiceGateStatus()
      .then((next) => alive && setStatus(next))
      .catch((reason) => alive && setError(String(reason)));
    return () => {
      alive = false;
    };
  }, [settings.voice_gate_enabled]);

  async function run(action: () => Promise<VoiceGateStatus>) {
    setBusy(true);
    try {
      setStatus(await action());
      setError(null);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  const enrolled = status?.enrolled ?? 0;
  const modelLine = !status
    ? "Checking…"
    : !status.runtime_available
      ? "This build has no speaker runtime."
      : status.model_ready
        ? "Speaker model ready (CAM++, on this Mac)."
        : "Speaker model not downloaded (about 28 MB).";

  return (
    <SettingsCard
      title="Voice gate"
      description="Auto-type only your own voice. Other speakers stay in the Capture list, marked, and are never typed. Audio is never deleted."
    >
      <SettingSwitch
        label="Type only my voice"
        checked={settings.voice_gate_enabled}
        description="Compares each finished phrase with your enrolled voice. Turns on final-only typing."
        onCheckedChange={(voice_gate_enabled) => onPatch({ voice_gate_enabled })}
      />
      <SettingSlider
        label="Match threshold"
        description="Higher rejects more. Raise it if a nearby voice still gets typed; lower it if your own phrases are marked as another speaker."
        value={settings.voice_gate_threshold}
        min={0.2}
        max={0.8}
        step={0.05}
        valueLabel={settings.voice_gate_threshold.toFixed(2)}
        disabled={!settings.voice_gate_enabled}
        onValueChange={(voice_gate_threshold) => onPatch({ voice_gate_threshold })}
      />
      <Field orientation="responsive" className="settings-row">
        <FieldContent>
          <FieldLabel>Speaker model</FieldLabel>
          <FieldDescription>{modelLine}</FieldDescription>
        </FieldContent>
        <div className="settings-action-row">
          {status && status.runtime_available && !status.model_ready ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void run(downloadVoiceGateModel)}
            >
              {busy ? "Downloading…" : "Download"}
            </Button>
          ) : null}
        </div>
      </Field>
      <Field orientation="responsive" className="settings-row">
        <FieldContent>
          <FieldLabel>My voice</FieldLabel>
          <FieldDescription>
            {enrolled === 0
              ? "Not enrolled. Turn the gate on, speak a few phrases alone, then press “This is me” on 3–5 of them in the Capture list."
              : `${enrolled} phrase${enrolled === 1 ? "" : "s"} enrolled. Add more from the Capture list to improve matching.`}
          </FieldDescription>
          {error ? <FieldDescription>{error}</FieldDescription> : null}
        </FieldContent>
        <div className="settings-action-row">
          {enrolled > 0 ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void run(resetVoiceGate)}
            >
              Reset
            </Button>
          ) : null}
        </div>
      </Field>
    </SettingsCard>
  );
}
