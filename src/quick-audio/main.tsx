import { createRoot } from "react-dom/client";
import { QuickAudioPanel } from "./QuickAudioPanel";
import { devFixtures } from "../lib/dev-fixtures";
import { isTauriRuntime, seedBrowserFixtures } from "../lib/tauri-client";
import { MicrophoneNotice } from "./MicrophoneNotice";
import "../styles.css";
import "./quick-audio.css";

if (!isTauriRuntime()) seedBrowserFixtures(devFixtures());
const notice = new URLSearchParams(window.location.search).has("notice");
createRoot(document.getElementById("root")!).render(notice ? <MicrophoneNotice /> : <QuickAudioPanel />);
