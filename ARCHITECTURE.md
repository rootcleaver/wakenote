# WakeNote Architecture

## Overview

WakeNote is a macOS Tauri 2 application with two user interfaces and one Rust backend:

- the main React workspace for capture, history, reports, queue activity, and settings;
- a separate click-through React overlay for live Subtitle text and dictation feedback;
- the Rust process that owns audio devices, recording, transcription runtimes, persistence, permissions, tray behavior, and Tauri commands.

The central architectural boundary is the filesystem. A capture becomes useful only after its audio and metadata are committed. Transcription then adds a success or error sidecar, while the persistent queue records processing state. This keeps recordings recoverable even if transcription, a cloud provider, or the UI fails.

## Components

### Frontend workspace

`src/main.tsx` mounts `src/App.tsx`. Outside Tauri it also installs the browser-only fixtures from `src/lib/dev-fixtures.ts`; inside Tauri all state comes through `src/lib/tauri-client.ts`.

The primary routes are defined in `src/lib/navigation.ts`:

| Route | Responsibility |
| --- | --- |
| Capture | Live levels, recorder state, calibration, and recent partial/final text |
| Meetings | Imported or manually recorded long-form audio and resumable transcription |
| Transcripts | Day-indexed archive, order/filter controls, 50-row pages, playback, recoverable bundle deletion, and report entry points |
| Reports | Report composition, run progress, history, and rendered Markdown |
| Activity | Day-indexed persistent queue, warnings/errors, running cancellation, retry, reprocessing, and recoverable bundle deletion |
| Settings | General, Audio, Dictation, Models, Storage, Integrations, and Advanced configuration |

`src/lib/app-state.ts` reduces snapshots and events into frontend state. Domain helpers in `src/lib/` keep capability filtering, queue presentation, meeting progress, report composition, and browser mocks out of page components. `DatePagePicker.tsx` supplies the shared local-day navigation used by Transcripts and Activity; each view owns its order, filter, selection, and page-reset semantics. Activity's issue views bypass that day scope so warnings and errors are reviewed and cleared across every date.

### Subtitle overlay

`src/overlay/main.tsx` mounts `RecordingOverlay.tsx` into the separate Tauri overlay window. The backend emits Subtitle and dictation state events; the overlay never owns capture or transcription state. It is a click-through presentation surface, while the main window remains the control surface. One normalized style contract carries pixel padding, border, radius, and width/height bounds through settings, native window sizing, and CSS. Generation-tagged expiry timers give both previews and live text the configured lifetime without allowing stale timers to hide newer content.

### Menu-bar audio controls

`quick_audio.rs` adds an eight-second scrolling line graph alongside the existing tray icon. It samples the existing `AppStatus.level` four times per second, applies display-only exponential smoothing, and retains 32 points. Missing intervals become gaps, and stopping input clears the trace. Time runs left to right, input level runs bottom to top, and a gold horizontal line marks the threshold. The title keeps only the threshold to avoid flickering numbers and changing item widths. Its click opens a separate `quick-audio.html` React window, positioned within the clicked monitor's bounds. The existing tray's configured click actions remain available.

`src/quick-audio/` polls capture state and the same in-memory trace through `quick_audio_history` only while visible, reuses the Audio settings microphone selectors, and writes through `update_settings`. Threshold dragging commits on release, key-up, or blur. A revision guard prevents an older poll from overwriting a completed settings write, and save errors remain visible across successful level polls. The panel hides on focus loss or Escape. Display smoothing never affects recording or voice detection. No additional audio stream or persistence format is introduced.

`microphone_shortcut.rs` registers the configurable Primary input cycle chord independently of Dictation. It ignores key repeat, serializes switch requests, deduplicates physical devices by Core Audio UID, and uses the existing settings/runtime update path while retaining Secondary. A failed active-input start attempts to restore the previous selection, without overwriting a newer manual selection. Shortcut registration changes are reconciled with Dictation and rolled back if registration or persistence fails. Legacy settings retain their existing Dictation chord when the new default would overlap it. The separate, non-focusable `input-notice` window uses the same HTML entry and displays a generation-tagged 2.5-second notice, so a previous timer cannot hide a later switch.

Hardware input-volume reads, writes, and observation run off the UI thread through `input_level_query.rs`. A shared gate admits one in-flight call and a 600ms deadline releases the caller if Core Audio stops responding. The gate remains held until the actual hardware call exits, preventing repeated polls from accumulating blocked workers. Read failures return explicit unavailable-volume metadata; they do not claim that capture hardware is absent.

### Tauri command and event bridge

`src-tauri/src/main.rs` builds the Tauri application, manages shared runtime state, registers commands, starts background workers, and emits events. `src/lib/tauri-client.ts` is the frontend adapter for those commands and provides a compatible in-memory backend for browser development.

Commands cover settings, credentials, permissions, models, queue actions, capture, transcript indexes, reports, costs, source capture, and meetings. Events carry high-frequency or asynchronous updates such as live transcript partials, report runs, meeting progress, source detection, audio merge progress, and cost snapshots.

### Permission drag shelf

`permission_drag.rs` exposes a closed `PermissionDragTarget` boundary for Accessibility and Screen & System Audio Recording. The `open_permission_drag_shelf` Tauri command accepts only that enum, opens the corresponding System Settings deep link, and returns the existing permission snapshot shape. Browser development treats both assistant actions as no-ops, while Microphone keeps its native request path.

On macOS, `permission_drag/macos.rs` validates the running `.app` ancestor before publishing it as a native file drag. A main-thread AppKit controller owns one non-activating `NSPanel`; Window Server metadata locates and tracks System Settings without requiring Accessibility access. Generation tokens cancel stale trackers, and the panel tears down on a grant, either window closing, replacement, or process exit. System Settings performs the actual drop and remains the only component that changes TCC state.

### Audio capture and recording

| Module | Responsibility |
| --- | --- |
| `audio.rs` | Speech gate, level monitoring, calibration inputs, and capture timing |
| `live_capture.rs` | CPAL input runtime and bounded frame dispatch |
| `multi_capture.rs` | Independent Primary/Secondary streams, recovery, resampling, adaptive drift-aware merge, and Priority Audio selection |
| `system_audio.rs` | ScreenCaptureKit-backed system-audio capture |
| `source_watcher.rs` / `sources.rs` | Recognized application/window sources and lifecycle detection |
| `capture.rs` | Converts gated frames into completed chunks |
| `recorder.rs` | Writes WAV directly, M4A through `afconvert`, MP3 through `ffmpeg`, plus metadata and sidecars |
| `storage.rs` | Allocates dated, collision-safe output paths and uploaded-audio paths |
| `trash.rs` | Stages exact-stem artifact bundles, moves them to recoverable macOS Trash, and rolls back partial failures |
| `audio_analysis.rs` / `audio_merge.rs` | Waveform analysis and non-destructive audio merge operations |

Live callbacks push frames into bounded queues so filesystem or inference work does not block the audio callback. Each selected microphone has its own stream and health state. Separate mode preserves both streams. Adaptive Merge uses a 220 ms bounded buffer to track fractional delay, gain, polarity, and clock drift; it blends coherent inputs and falls back toward the cleaner source when confidence drops. Priority Audio never sums room responses: it scores sufficiently voiced input quality, requires three windows before switching, and changes source through a 50 ms equal-power crossfade. Metadata retains both physical devices.

### Transcription runtimes

`src-tauri/src/models.rs` is the capability registry and local model store. A descriptor declares whether a model supports file transcription, realtime use, streaming, diarization, cost reporting, request-size limits, and each selectable context.

`src-tauri/src/transcription.rs` routes a request by `provider_runtime` and returns a common `TranscriptionExecution` with text, requested/effective model provenance, optional fallback provenance, provider usage, and typed failures. The available runtime families are:

- local `whisper-rs` with Metal on macOS;
- bundled sherpa-onnx models;
- isolated Qwen3-ASR Python environments;
- advanced external commands;
- OpenRouter file transcription;
- OpenAI file, realtime, and diarization endpoints;
- Soniox async REST and realtime WebSocket endpoints.

`cloud_transcription.rs` owns shared cloud credentials and failure categories. `openai_realtime.rs` and `soniox_realtime.rs` keep provider protocols separate while sharing the provider-neutral request, partial, and final-result store in `cloud_realtime.rs`. `soniox_async.rs` owns Soniox upload, polling, result retrieval, and best-effort remote cleanup.

### Queue and Activity

`queue.rs` defines the durable queue, job lifecycle, issue severity, acknowledgement state, and history pruning. `commands.rs` connects captures and imports to the queue and persists every meaningful transition.

The production worker runs at most one transcription job at a time. A job retains its chosen model and transcription options. On startup, a persisted `running` job is rewritten to `pending`, allowing the worker to retry work interrupted by an app exit. Completed no-speech, empty, low-confidence, and artifact outcomes can carry warnings without being represented as processing failures.

Running cancellation is cooperative at the worker boundary and idempotent at the queue boundary. Once cancelled, partial events, sidecar commits, and late final outcomes are suppressed. Running deletion is two-phase: mark the job cancelled and pending deletion, wait for the active worker to acknowledge exit, then move the exact audio/text/metadata/error bundle to Trash and remove the queue record. A failed move rolls the staged bundle back and keeps the record recoverable.

Activity acknowledgement is separate from job lifecycle: marking an outcome resolved sets its read state but does not delete the queue record or its audio. Reprocessing creates a new pending attempt with the selected model.

### Dictionary

`dictionary.rs` compiles enabled canonical terms and aliases into provider context plus deterministic post-transcription correction. `dictionary_file.rs` synchronizes the same entries through `dictionary.txt`, preserving the last valid configuration when a file edit is invalid.

Provider-native context is used only when supported. Deterministic correction remains the common final step, so local and cloud results follow the same canonical-term contract.

### Dictation

`dictation.rs` owns shortcut validation, independent hold/toggle event handling, dedicated microphone capture, the ten-minute guard, and temporary inference audio. The existing `dictation_shortcut` setting remains the hold hotkey; `dictation_toggle_shortcut` defaults to empty. Repeated key events are ignored, and only the hotkey that started a recording can finish it. Active hotkeys, including Enhanced Prompt, are validated together for overlaps. Shortcut capture suspends native handlers until editing completes. `text_input.rs` inserts a non-empty final transcript into the focused macOS application and follows the configured clipboard retention policy.

Dictation selects the cleanest signal from the configured Primary and optional Secondary microphones without interrupting archival voice-activated recording. VOR chunks that overlap Dictation remain as audio-only records and skip live/final transcription so the same speech is not stored twice. Dictation audio, metadata, transcript, or recoverable error sidecar is stored like any other capture; only temporary inference files are discarded.

The `recent_dictations` command scans daily transcript storage on a blocking worker, filtering microphone entries with a case-insensitive `dictation` label and excluding hidden records before applying the limit. Settings requests ten entries and links them into the Transcripts daily browser with the selected date and source filter. The Dictations source option remains available with a zero count so changing dates cannot silently clear the filter.

### Meetings

`meeting.rs` keeps long-form work separate from short capture chunks. Imported `mp3`, `m4a`, or `wav` files are copied into `meetings/<id>/` without changing their durable codec, normalized for inference, split into bounded segments, and checkpointed to `meeting.json` after each segment. Manual Meeting capture streams PCM into a hidden WAV, finalizes `audio.m4a` through the same native encoder and configured bitrate as transcript recordings, and removes the temporary file after success. Encoding failure preserves recoverable audio. Interrupted transcription work can resume from persisted progress.

OpenAI GPT-4o Transcribe Diarize can persist speaker turns when speaker separation is requested. Other compatible models use the standard segment transcript path.

### Reports

`llm.rs` builds transcript evidence, calls the configured OpenRouter model, evaluates/refines the result, and atomically writes Markdown plus metadata. `llm_runs.rs` persists active and terminal run state under `reports/.runs/`, enabling cancellation, retry, and startup reconciliation.

The frontend renders report Markdown without enabling raw HTML. Hiding a report changes list visibility only; it does not remove the report files.

## Data flow

### Voice-activated capture and queued transcription

```text
physical microphones / supported system source
  -> one capture runtime per physical microphone and bounded frame queues
  -> level monitor + SpeechGate
  -> Separated Audio / adaptive Merge Audio / Priority Audio
  -> Recorder
  -> YYYYMMDD audio + metadata sidecar
  -> persistent TranscriptionQueue
  -> capability-selected RuntimeTranscriber
  -> Dictionary correction + quality classification
  -> transcript or recoverable error sidecar
  -> snapshot/events -> Transcripts and Activity
```

The audio and metadata are committed before the queue processes them. The transcript sidecar is therefore additive rather than the only durable representation of a capture.

### Realtime Subtitles

```text
cumulative live samples
  -> selected OpenAI or Soniox realtime manager
  -> send only unseen PCM samples
  -> provider partials -> shared callback -> overlay event
  -> capture commit -> provider final result
  -> path-keyed shared result store
  -> normal transcription worker and artifact pipeline
```

Provider-specific protocol state does not leak into the recorder or frontend. A final result still goes through the common provenance, Dictionary, warning/error, and sidecar contracts.

### Shortcut dictation

```text
global shortcut pressed
  -> dedicated microphone capture + feedback overlay
  -> hold released or toggle pressed again -> saved dictation artifact
  -> selected dictation model
  -> non-empty final text -> transcript sidecar
  -> focused-cursor insertion + configured clipboard retention
```

### Meetings

```text
imported audio (original supported codec) or manual PCM capture
  -> imported audio.* or finalized meetings/<id>/audio.m4a + meeting.json
  -> normalization and segmentation
  -> compatible model per segment or provider request
  -> atomic meeting.json progress checkpoints
  -> completed transcript / speaker turns -> Meetings UI
```

### Reports

```text
selected transcript evidence
  -> persisted report run
  -> OpenRouter draft
  -> grading and bounded refinement
  -> reports/*.md + reports/*.json
  -> rendered document and run history
```

## Persistence boundaries

WakeNote deliberately separates private application state from the user-visible archive.

### Private app-data directory

Tauri resolves the platform app-data directory for `com.chann.wakenote`. On macOS this is normally under `~/Library/Application Support/com.chann.wakenote/`.

| File | Contents |
| --- | --- |
| `settings.json` | Non-secret application settings |
| `transcription-queue.json` | Queue jobs, status, issues, selected model/options, and read state |
| `openrouter-secrets.json` | OpenRouter API key only |
| `openai-secrets.json` | OpenAI API key only |
| `soniox-secrets.json` | Soniox API key only |
| `dictionary.txt` | Human-editable canonical terms and aliases |
| `list-visibility.json` | Non-destructive hidden-item state per normalized save root |
| `transcription-costs.json` | Bounded provider usage and local cost estimates |

Secret values are loaded only for backend dispatch. Frontend snapshots expose configured booleans, never the stored keys.

### User-selected save root

The default is `~/Documents/WakeNote`.

```text
<save_root>/
├── YYYYMMDD/
│   ├── YYMMDD-HHMMSS[-source][-N].{m4a|mp3|wav}
│   ├── YYMMDD-HHMMSS[-source][-N].json
│   ├── YYMMDD-HHMMSS[-source][-N].txt
│   ├── YYMMDD-HHMMSS[-source][-N].error.txt
│   └── all.json
├── uploaded/YYYYMMDD/
├── meetings/<meeting-id>/
│   ├── audio.m4a or imported audio.*
│   └── meeting.json
└── reports/
    ├── *.md
    ├── *.json
    └── .runs/*.json
```

`all.json` is a derived per-day transcript index; audio and individual sidecars remain the source artifacts. Collisions use `-2`, `-3`, and so on rather than overwriting an existing basename.

## Directory structure

```text
wakenote/
├── README.md / README.en.md    Project front doors
├── USAGE.md                    Operational and development reference
├── ARCHITECTURE.md             This document
├── package.json                pnpm scripts and frontend dependencies
├── scripts/                    Build, install, and DMG orchestration
├── src/
│   ├── App.tsx                 Main workspace composition
│   ├── main.tsx                Main browser/Tauri frontend entry
│   ├── components/             Product pages, settings, shell, and UI primitives
│   ├── lib/                    State, Tauri adapter, fixtures, and domain helpers
│   ├── overlay/                Separate caption/dictation window
│   ├── quick-audio/            Menu-bar audio control window
│   └── styles/                 Shared tokens and page/component styles
└── src-tauri/
    ├── Cargo.toml              Rust dependencies and optional asr-sherpa feature
    ├── tauri.conf.json         Window, bundle, resources, and macOS minimum version
    ├── src/                    Backend domains and Tauri runtime
    ├── tests/                  Rust integration tests
    └── vendor/                 Patched sherpa-rs / sherpa-rs-sys / coreaudio-rs sources
```

## Design decisions

### Files before inference

WakeNote commits recoverable audio and metadata before transcription. Queue or provider failure cannot erase the original recording, and reprocessing can reuse the same artifact.

### Capabilities, not provider-name conditionals in the UI

Model descriptors define selectable contexts, streaming, diarization, and fallback compatibility. The frontend filters by those capabilities, and the backend normalizes unsupported options again before dispatch.

### Explicit network use

Local models are the default. Cloud runtimes require a saved provider key and retain the selected model on failure. WakeNote does not silently switch between cloud providers. A user-enabled cost-limit fallback is narrower: it retries once with a selected ready on-device model only for a classified billing-limit failure.

### Provider isolation with shared result contracts

OpenAI and Soniox protocols have separate adapters and error parsing. They converge only at credential redaction, typed failures, realtime partial/final contracts, usage accounting, Dictionary correction, and artifact persistence.

### Recoverable history controls

Resolved Activity outcomes and hidden transcripts, meetings, or reports remain stored. Deletion is explicit and separate from acknowledgement or visibility state. Transcript and Activity deletion operates on exact-stem bundles, moves audio plus `.txt`, `.json`, and `.error.txt` siblings to macOS Trash, rolls back on failure, and rebuilds the affected per-day index. Running Activity deletion also waits for worker acknowledgement so late output cannot recreate the bundle.

Transcripts and Activity both navigate at the existing local-day index boundary and default to newest-first with an oldest-first option. Transcripts limits rendered results to 50 rows per page and resets page-local selection when the day, filter, or order changes. This bounds DOM and audio-control work; `all.json` still loads one day at a time, so the design does not claim backend cursor pagination.

Activity's issue views are the exception: they select on outcome rather than date, so they page over every matching warning and error with day headings, and the existing select-all and bundle Trash actions apply to that whole cross-date set. The queue snapshot is already fully in memory, so this adds no additional disk read.

### Atomic small-state writes

Settings, queue state, meeting progress, report metadata, Dictionary synchronization, and visibility registries use temporary-file replacement patterns. Startup repair is bounded to known recoverable states such as `running` queue jobs and interrupted report/meeting work.

### Browser development is a UI simulator

The browser fallback intentionally uses fixtures and a mock Tauri adapter. It is useful for visual and interaction development but cannot prove microphone capture, filesystem behavior, permissions, native focused-cursor insertion, or provider networking.
