import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Notice { sequence: number; label: string; detail: string; error: boolean }

export function MicrophoneNotice() {
  const [notice, setNotice] = useState<Notice | null>(null);
  useEffect(() => {
    let mounted = true;
    const accept = (next: Notice) => {
      if (mounted) setNotice(previous => !previous || next.sequence >= previous.sequence ? next : previous);
    };
    const listener = listen<Notice>("microphone-cycle-notice", event => accept(event.payload));
    void listener.then(() => invoke<Notice>("microphone_cycle_notice")).then(accept);
    return () => { mounted = false; void listener.then(unlisten => unlisten()); };
  }, []);
  return (
    <main className={`microphone-notice${notice?.error ? " microphone-notice--error" : ""}`} role="status">
      <strong>{notice?.label || "Switching microphone…"}</strong>
      <span>{notice?.detail || "Primary input"}</span>
    </main>
  );
}
