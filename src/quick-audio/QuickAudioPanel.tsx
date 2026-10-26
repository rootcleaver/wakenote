import { useCallback, useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MicrophoneSelectors } from "../components/settings/AudioSettings";
import { normalizeCaptureMicrophones } from "../lib/capture-controls";
import { isTauriRuntime } from "../lib/tauri-client";
import type { MicrophoneDevice, SettingsPatch } from "../lib/types";
import {
  hideAudioPanel,
  readAudioState,
  readMicrophones,
  writeAudioSettings,
  type QuickAudioState,
} from "./client";

export function QuickAudioPanel() {
  const [state, setState] = useState<QuickAudioState | null>(null);
  const [microphones, setMicrophones] = useState<MicrophoneDevice[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [threshold, setThreshold] = useState<number | null>(null);
  const draft = useRef<number | null>(null);
  const busy = useRef(false);
  const mounted = useRef(false);
  const revision = useRef(0);

  useEffect(() => {
    mounted.current = true;
    let reading = false;
    let enumerating = false;
    let visible = true;
    const refresh = async () => {
      if (
        !visible ||
        document.visibilityState === "hidden" ||
        reading ||
        busy.current
      )
        return;
      reading = true;
      const startedAtRevision = revision.current;
      try {
        const next = await readAudioState();
        if (
          mounted.current &&
          !busy.current &&
          startedAtRevision === revision.current
        ) {
          setState(next);
          setReadError(null);
        }
      } catch (reason) {
        if (mounted.current) setReadError(String(reason));
      } finally {
        reading = false;
      }
    };
    const devices = async () => {
      if (!visible || document.visibilityState === "hidden" || enumerating)
        return;
      enumerating = true;
      try {
        const next = await readMicrophones();
        if (mounted.current) setMicrophones(next);
      } catch (reason) {
        if (mounted.current) setError(String(reason));
      } finally {
        enumerating = false;
      }
    };
    const shown = () => {
      void refresh();
      void devices();
    };
    shown();
    const timer = window.setInterval(() => void refresh(), 250);
    const deviceTimer = window.setInterval(() => void devices(), 5_000);
    document.addEventListener("visibilitychange", shown);
    window.addEventListener("focus", shown);
    const focusListener = isTauriRuntime()
      ? getCurrentWindow().onFocusChanged(({ payload }) => {
          visible = payload;
          if (payload) shown();
        })
      : null;
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
      window.clearInterval(deviceTimer);
      document.removeEventListener("visibilitychange", shown);
      window.removeEventListener("focus", shown);
      void focusListener?.then((unlisten) => unlisten());
    };
  }, []);

  const save = useCallback(async (patch: SettingsPatch) => {
    if (busy.current) return;
    busy.current = true;
    revision.current += 1;
    setSaving(true);
    try {
      const settings = await writeAudioSettings(patch);
      if (mounted.current) {
        setState((previous) =>
          previous ? { ...previous, settings } : previous,
        );
        setError(null);
      }
    } catch (reason) {
      if (mounted.current) setError(String(reason));
    } finally {
      busy.current = false;
      if (mounted.current) setSaving(false);
    }
  }, []);

  const saveMicrophones = useCallback(
    (
      capture_microphones: NonNullable<SettingsPatch["capture_microphones"]>,
    ) => {
      void save({ capture_microphones });
    },
    [save],
  );

  const commitThreshold = () => {
    if (draft.current === null || busy.current) return;
    const value = draft.current;
    draft.current = null;
    void save({ threshold_dbfs: value }).then(() => setThreshold(null));
  };

  const selectedThreshold = threshold ?? state?.settings.threshold_dbfs ?? -40;

  return (
    <main
      className="quick-audio"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          void hideAudioPanel();
        }
      }}
    >
      <header>
        <div>
          <p>WakeNote</p>
          <h1>Audio controls</h1>
        </div>
        <button
          type="button"
          aria-label="Close audio controls"
          onClick={() => void hideAudioPanel()}
        >
          <X size={18} />
        </button>
      </header>
      {(error || readError) && (
        <p role="alert" className="quick-audio__error">
          {error || readError}
        </p>
      )}
      {!state ? (
        <p role="status">Loading audio…</p>
      ) : (
        <>
          <section aria-label="Live microphone levels">
            {(["primary", "secondary"] as const).map((slot, index) => {
              const configured = state.settings.capture_microphones[index];
              const runtime = state.status.microphone_captures.find(item => item.slot === slot);
              const active = Boolean(configured && runtime?.active && !runtime.reconnecting && !state.settings.pause_all && !readError);
              const name = slot === "primary" ? "Primary" : "Secondary";
              const current = state.history[slot].at(-1) ?? runtime?.level.current_dbfs ?? -120;
              return <div key={slot} className={`quick-audio__slot quick-audio__slot--${slot}`}>
                <div className="quick-audio__readout">
                  <span>{name} · {configured?.label ?? "Not configured"}</span>
                  <strong>{active ? `${Math.round(current)} dBFS` : "—"}</strong>
                </div>
                <LevelHistory levels={active ? state.history[slot] : []} threshold={selectedThreshold} name={name} />
                <div className="quick-audio__time-axis">
                  <span>{!configured ? "Not configured" : active ? "8s ago" : runtime?.reconnecting ? "Reconnecting" : "Input unavailable"}</span><span>Now</span>
                </div>
              </div>;
            })}
            <label className="quick-audio__threshold" htmlFor="threshold">
              Threshold <strong>{selectedThreshold} dBFS</strong>
            </label>
            <input
              id="threshold"
              type="range"
              min={-90}
              max={-10}
              step={1}
              value={selectedThreshold}
              disabled={saving}
              onChange={(event) => {
                const value = Number(event.target.value);
                draft.current = value;
                setThreshold(value);
              }}
              onPointerUp={commitThreshold}
              onKeyUp={commitThreshold}
              onBlur={commitThreshold}
            />
          </section>
          <fieldset disabled={saving}>
            <MicrophoneSelectors
              value={state.settings.capture_microphones}
              microphones={microphones}
              onChange={saveMicrophones}
            />
          </fieldset>
          <div
            className="quick-audio__devices"
            aria-label="Input connection status"
          >
            {normalizeCaptureMicrophones(
              state.settings.capture_microphones,
            ).map((configured, index) => {
              const slot = index === 0 ? "primary" : "secondary";
              const runtime = state.status.microphone_captures.find(
                (item) => item.slot === slot,
              );
              const connection = runtime?.reconnecting
                ? "Reconnecting"
                : runtime?.active
                  ? "Active"
                  : "Waiting";
              return (
                <div key={slot}>
                  <span>
                    {index === 0 ? "Primary" : "Secondary"} · {configured.label}
                  </span>
                  <strong>{connection}</strong>
                </div>
              );
            })}
          </div>
          <p role="status" className="quick-audio__save">
            {saving ? "Saving…" : "Changes save automatically"}
            {state.settings.cycle_microphone_shortcut && ` · Cycle input: ${state.settings.cycle_microphone_shortcut}`}
          </p>
        </>
      )}
    </main>
  );
}

function LevelHistory({ levels, threshold, name }: {
  name: string;
  levels: (number | null)[];
  threshold: number;
}) {
  const y = (db: number) => 3 + (1 - Math.max(0, Math.min(1, (db + 90) / 90))) * 50;
  let connected = false;
  const path = levels.map((db, index) => {
    if (db === null || !Number.isFinite(db)) { connected = false; return ""; }
    const command = connected ? "L" : "M";
    connected = true;
    return `${command}${(index / 31 * 320).toFixed(1)},${y(db).toFixed(1)}`;
  }).join(" ");
  return (
    <svg className="quick-audio__history" viewBox="0 0 320 56" preserveAspectRatio="none"
      role="img" aria-label={`${name} input level over the last 8 seconds; newest on the right`}>
      <title>Sound level in dBFS over time. Gold line: threshold.</title>
      <line x1="0" x2="320" y1={y(threshold)} y2={y(threshold)} className="quick-audio__threshold-line" />
      <path d={path} className="quick-audio__level-line" />
    </svg>
  );
}
