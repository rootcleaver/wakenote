# WakeNote Usage

## Installation

### Install a published release

1. Download `WakeNote_<version>_aarch64.dmg` and `SHA256SUMS.txt` from the [latest GitHub release](https://github.com/channprj/wakenote/releases/latest). Published builds target Apple Silicon Macs running macOS 15.5 or later; Intel Macs need a source build.
2. Optionally verify the download from the folder that holds both files:

```bash
shasum -a 256 -c SHA256SUMS.txt
```

3. Open the DMG and drag **WakeNote** into **Applications**.
4. On the first install, remove the macOS quarantine flag once before opening the app:

```bash
xattr -dr com.apple.quarantine /Applications/WakeNote.app
```

WakeNote is ad-hoc signed and not notarized, so Gatekeeper blocks the first launch of a copy downloaded in a web browser, which carries the macOS quarantine flag. Removing the flag once lets it open. Updates installed from inside WakeNote download, verify, and replace the bundle without adding the flag, so they do not need the command again. Run it again only after replacing the app with a DMG you downloaded yourself.

The rest of this section covers building from source.

### Requirements

- macOS 15.5 or later for app bundles built with `pnpm build` or `pnpm release:build`, which bundle a prebuilt ONNX Runtime that targets macOS 15.5. A development build (`pnpm tauri dev`) links ScreenCaptureKit and needs macOS 12.3 or later. System-audio capture needs macOS 13 or later. The bundle configuration and native capture paths are macOS-specific.
- Node.js 20+ and `pnpm` 10. The repository pins `pnpm@10.33.4` in `package.json`.
- The stable Rust toolchain; the crate uses Rust edition 2024.
- Xcode Command Line Tools for Tauri builds and the native `/usr/bin/afconvert` M4A encoder.
- CMake for compiling the bundled Whisper runtime (`brew install cmake` on macOS).
- `ffmpeg` when recording MP3 or processing imported media that needs conversion. WakeNote checks common Homebrew and `~/.local/bin` locations before `PATH`.
- Microphone permission for live microphone capture.
- Screen Recording permission when system-audio capture is enabled.
- Accessibility permission when WakeNote must type dictation or automatic transcripts into the focused application.

Qwen3-ASR setup additionally needs `uv` or Python 3.10+ and enough disk space for PyTorch plus the selected model. WakeNote's bundled Whisper and sherpa-onnx paths do not need an external Python environment.

### Install dependencies

```bash
brew install cmake
pnpm install
```

### Run the desktop app in development

```bash
pnpm tauri dev
```

Tauri starts the Vite development server through `beforeDevCommand` and opens the native application. Use this path to test microphones, permissions, filesystem persistence, the tray, the overlay, focused-cursor typing, and provider integrations.

### Run only the frontend

```bash
pnpm dev
```

This serves Vite at `http://127.0.0.1:1420` and installs browser-only fixtures. It is the fastest UI development path, but all native calls use the mock adapter.

### Build and install the macOS app

```bash
# Frontend bundle only
pnpm build

# Release Tauri app
pnpm build release

# Release build, install to /Applications, and optionally open it
pnpm build install
pnpm build install open

# Debug equivalents
pnpm build debug install
pnpm build debug install open
```

The build orchestrator enables the `asr-sherpa` Cargo feature, places its native runtime libraries in `WakeNote.app/Contents/Frameworks`, adds the required runtime search path, and applies an ad-hoc deep signature to the assembled bundle.

Use a different installation directory with either form:

```bash
WAKENOTE_INSTALL_PATH="$HOME/Applications" pnpm build install
pnpm build install --path "$HOME/Applications"
```

### Build a DMG

```bash
# Leave the DMG under src-tauri/target/release/bundle/dmg/
pnpm dmg

# Build and move the newest versioned DMG into ~/Downloads
pnpm dmg:downloads
```

`pnpm dmg:downloads` is a standalone script. Do not pass `dmg:downloads` as an argument to `pnpm build`.

### Local build and manual GitHub Release

GitHub Actions is disabled in the repository settings to avoid hosted build and release costs. There are no hosted workflows in this checkout. Pushes and tags do not build or publish the app; maintainers run the following commands on a Mac.

1. Commit the functional changes and run the relevant local tests. Use the installed Headatever skill's `scripts/headatever.sh patch --no-git` to advance `VERSION`. Synchronize the same value in `package.json`, `src-tauri/Cargo.toml`, the `wakenote` entry of `src-tauri/Cargo.lock`, and `src-tauri/tauri.conf.json`.
2. Create a separate `chore(release): v<version>` commit containing the five version files, then create an annotated `v<version>` tag on that commit. The worktree must be clean, including untracked source files.
3. Push the branch and tag before publication:

```bash
git push
git push origin "v$(tr -d '\n' < VERSION)"
```

`release:publish` reuses local artifacts only when their source commit, version, architecture and checksums match the current release. Missing, stale, incomplete or corrupted artifacts trigger a local build before publication. You can also prepare the files separately with `pnpm release:build`.

Both commands use the existing native build pipeline, include the sherpa-onnx runtime, check the app signature, version and architecture, and verify the DMG when building. They do not install or launch the app. Builds create these files under `release/v<version>/aarch64/` on Apple Silicon, or `release/v<version>/x64/` on Intel:

- `WakeNote_<version>_<architecture>.dmg`
- `SHA256SUMS.txt`
- `release.json` with the source commit, version, architecture and DMG hash

The generated directory is ignored by Git. A build targets only the current Mac; it is not a universal or cross-architecture build. Native dependencies may still be downloaded during the local build. Automatic rebuilds preserve the old output until the new build passes validation, then move the old directory to `release/v<version>/.previous-<suffix>/<architecture>/`. A failed build never publishes or discards the old output. The standalone `release:build` command still refuses to replace existing output; move that exact version/architecture directory aside to rebuild it manually.

4. Install GitHub CLI (`brew install gh`) and authenticate with `gh auth login` if needed. The account must be able to create releases and read the repository's Actions settings. `origin` must point to the target GitHub repository and the current branch must track `origin`.
5. Validate and explicitly publish:

```bash
# Read-only validation and build/reuse plan, including live branch/tag and Actions checks
pnpm release:publish --dry-run

# Reuse a current build or build first, then create the release and upload its files
pnpm release:publish

# Alternatively, upload as a draft and publish later in the GitHub UI
pnpm release:publish --draft
```

Publication checks all five versions, the clean source commit, checksums, the annotated tag, live remote branch/tag parity, and that Actions remains disabled. Remote prerequisites are checked before an automatic build and again afterward. `--dry-run` checks these prerequisites and reports whether artifacts will be reused or built; it never builds, changes local artifacts, or creates a release. `--draft` uses the same automatic build behavior. Publication refuses an existing release, including a draft, and never pushes refs or overwrites release assets. If an upload fails, inspect the release in GitHub before retrying; a partially uploaded draft may already exist. A draft created with `--draft` is published through GitHub, not by rerunning `release:publish`.

The app uses **ad-hoc signing and is not notarized**. The release notes state this as well. Downloads can therefore require macOS approval before opening; verify their source and checksum first, then clear the quarantine flag once as described in [Install a published release](#install-a-published-release). This local workflow does not configure Developer ID signing or notarization.

To inspect the cost-control setting:

```bash
gh api repos/channprj/wakenote/actions/permissions --jq .enabled
```

It should return `false`. The setting is managed under **Settings → Actions → General → Disable actions**; restoring hosted automation is a separate, explicit decision.

## Quick start

### First launch

1. Open **Settings › Audio** and grant the requested Microphone permission.
2. Select one Primary microphone. `System Default` can be combined with a distinct Secondary device. WakeNote prevents capturing the same physical device twice, including when the default input resolves to it. Speaker/app audio is configured separately under System Audio.
3. Speak and use **Calibration** to apply a threshold that stays above the room noise floor and below normal speech.
4. Open **Settings › Storage**, choose a save root, and click **Confirm Save Root**.
5. Open **Settings › Models**. Install an on-device model or select a configured cloud model.
6. Return to **Capture**. With Recording and Transcription enabled, crossing the threshold creates a recording and queues it for transcription.

The initial defaults are `~/Documents/WakeNote`, `whisper-medium`, Korean transcription, M4A at 96 kbps, and automatic live input on launch.

### Grant app-list permissions

Accessibility and Screen & System Audio Recording use an app-list workflow. In **Settings › Audio**, press **Open System Settings** for either permission. WakeNote opens the matching Privacy & Security pane and places a small native shelf beside the System Settings window. Drag the WakeNote icon from the shelf into the open list, then enable its switch. macOS remains the authority that adds and grants the permission; WakeNote does not edit the TCC database.

The shelf follows System Settings and closes when the permission is granted, System Settings closes, or you close the helper. Microphone permission continues to use the standard macOS request prompt. The drag shelf is available only when WakeNote is running from an installed `.app` bundle; a direct development executable still opens the correct settings pane but cannot provide an application drag item.

### Workspace

| Screen | Use it for |
| --- | --- |
| Capture | Live microphone state, recording waveform, calibration, and newest decoded phrases |
| Meetings | Manual long-form recording, audio import, resumable transcription, and optional speaker separation |
| Transcripts | Per-day capture history, newest/oldest ordering, 50-item pages, playback, selection, bundle Trash, regeneration, and report creation |
| Reports | Summary or detailed-report composition, progress, history, and rendered Markdown |
| Activity | Per-day queue status, an all-dates warning/error review, newest/oldest ordering, retry, skip, safe running-job cancellation, bulk reprocessing, and Trash actions |
| Settings | General behavior, Audio, Dictation, Models, Storage, Integrations, and Advanced sources |

### Menu-bar audio controls

The two small line graphs beside WakeNote's tray icon show the last eight seconds of Primary (`P`, blue) and Secondary (`S`, green) input without opening the app. Each uses its own microphone level; an unconfigured Secondary shows `S —`, and a stopped or disconnected slot shows `Off` instead of borrowing the other slot's trace. New samples arrive on the right and move left; higher means louder. A gold horizontal line marks the threshold. The trace updates four times per second with gentle display smoothing, while the title keeps the threshold (`T-54`, for example) instead of rapidly changing level numbers. `Off` means live input is stopped or paused. It follows **Settings › General › Show menu bar icon**. Click either graph to see both histories at a larger size and adjust the threshold or microphones. These display changes do not affect recording or voice detection.

Click the meter to open **Audio controls**. Adjust the **Threshold** slider or select the **Primary microphone** and optional **Secondary microphone** directly. Changes use the same saved settings as **Settings › Audio**. The slider saves when released, after a keyboard adjustment, or when it loses focus. Current device names and connection state appear below the selectors. Press Escape, use the close button, or click outside to dismiss the panel.

For a browser-only preview with fixture data, open `/quick-audio.html` on the Vite development server. Real menu-bar placement, audio levels, and device switching require native testing.

Press **Ctrl+Shift+M** to cycle the Primary microphone through available physical inputs. System Default aliases, duplicate device identities, and iPhone Continuity microphones are skipped; Secondary keeps its selection. macOS transport identity excludes renamed wired and wireless iPhones before capture is attempted. iPhone inputs remain available for manual selection in Audio settings. A small notice on the pointer's display shows the selected device for 2.5 seconds without taking keyboard focus. If only one input is available, the notice identifies it without restarting capture. During active capture, inputs that fail to start are skipped; if no alternative starts, WakeNote attempts to restore the previous selection and shows an error. Change or disable the shortcut in **Settings › Audio › Cycle Primary microphone shortcut**; an empty value disables it. Conflicting Dictation shortcuts are rejected.

### Capture lifecycle

The speech gate starts a chunk only after the input remains above Threshold for the Attack duration. It closes only after the input remains below Threshold for Release, while Pre-roll, Lead-in, and Post-roll protect the edges. Max Chunk splits continuous audio into bounded recordings.

With two configured microphones, **Microphone processing** offers three modes. **Separated Audio** writes independent Primary and Secondary artifacts. **Merge Audio** is the default: it buffers up to 220 ms, estimates fractional delay and clock drift, corrects gain and polarity, and blends only while alignment confidence is sufficient. **Priority Audio** instead writes one sufficiently loud, clean source at a time, using switching hysteresis and a 50 ms equal-power crossfade so two room responses are never summed. Every mode retains physical-device identity in metadata and transcript filters. Both slots may name the same physical microphone. That device is opened and leveled once, and both slots use that capture.

Recording, Transcription, and Pause are independent controls. You can retain audio without creating new transcription jobs, process an existing backlog without recording, or pause both.

### Output layout

The default save root is `~/Documents/WakeNote`.

```text
~/Documents/WakeNote/
├── 20260805/
│   ├── 260805-091530-mic-merged.m4a
│   ├── 260805-091530-mic-merged.json
│   ├── 260805-091530-mic-merged.txt
│   ├── 260805-092201.json
│   ├── 260805-092201.mp3
│   ├── 260805-092201.error.txt
│   └── all.json
├── uploaded/20260805/
├── meetings/<meeting-id>/
│   ├── audio.m4a
│   └── meeting.json
└── reports/
```

| Artifact | Meaning |
| --- | --- |
| `*.m4a`, `*.mp3`, `*.wav` | Captured or copied audio |
| `*.json` beside a chunk | Model, device, timing, source, status, and provenance metadata |
| `*.txt` | Successful transcript |
| `*.error.txt` | Recoverable transcription error |
| `YYYYMMDD/all.json` | Derived transcript-day index; individual sidecars remain authoritative |
| `uploaded/YYYYMMDD/` | Audio imported into the normal transcription queue |
| `meetings/<id>/audio.m4a` | Durable audio created by manual Meeting recording; imported meeting audio keeps its original supported codec |
| `meetings/<id>/meeting.json` | Long-form meeting state, segments, progress, and optional speaker turns |
| `reports/*.md` and `*.json` | Persisted report body and metadata |
| `reports/.runs/*.json` | Active and terminal report-run state |

WakeNote never overwrites an existing basename. A collision adds `-2`, then `-3`, and so on.

## Command reference

### Project commands

| Command | Purpose |
| --- | --- |
| `pnpm install` | Install locked JavaScript dependencies |
| `pnpm dev` | Run browser-only Vite UI with fixtures |
| `pnpm tauri dev` | Run the native app in development |
| `pnpm build` | Run TypeScript compilation and build the frontend bundle |
| `pnpm build release` | Build the release macOS app bundle |
| `pnpm build debug` | Build the debug macOS app bundle |
| `pnpm build install` | Build and install the release app |
| `pnpm build install open` | Build, install, and launch the release app |
| `pnpm dmg` | Build a release DMG |
| `pnpm dmg:debug` | Build a debug DMG |
| `pnpm dmg:downloads` | Build a release DMG and move it to `~/Downloads` |
| `pnpm release:build` | Build and verify a local DMG, checksum file and source manifest under `release/` |
| `pnpm release:build --dry-run` | Validate source/version prerequisites and show the local output path without building |
| `pnpm release:publish --dry-run` | Validate publication prerequisites and report build/reuse without building or publishing |
| `pnpm release:publish` | Reuse verified current artifacts or build first, then create a GitHub Release |
| `pnpm release:publish --draft` | Reuse verified current artifacts or build first, then upload a draft GitHub Release |
| `pnpm preview` | Preview the already-built frontend bundle |
| `pnpm test` | Run the complete frontend Vitest suite once |
| `pnpm exec tsc --noEmit` | Type-check without emitting files |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Run the default-feature Rust test suite |
| `cargo test --manifest-path src-tauri/Cargo.toml --all-features` | Run Rust tests including bundled sherpa-onnx integration |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Check Rust formatting |

### Build options

`scripts/build.mjs` accepts these positional arguments and flags through `pnpm build`:

| Argument | Effect |
| --- | --- |
| `debug`, `--debug` | Use the Tauri debug profile |
| `release`, `--release` | Use the release profile; this is the default when packaging |
| `app` | Package the `.app` bundle |
| `dmg` | Package a DMG |
| `install` | Copy the built app into the installation directory |
| `open`, `--open`, `--launch` | Install and launch the app |
| `no-build`, `--no-build` | Install an already-built bundle |
| `--path <dir>` | Override the installation directory |
| `-h`, `--help`, `help` | Print build help |

Examples:

```bash
pnpm build release app
pnpm build debug install open
pnpm build --no-build --path "$HOME/Applications"
```

## Configuration

### Settings and private state

Use the Settings UI for normal configuration. The backend persists non-secret settings and queue state in Tauri's app-data directory for `com.chann.wakenote`, normally:

```text
~/Library/Application Support/com.chann.wakenote/
```

| File | Purpose |
| --- | --- |
| `settings.json` | Non-secret application settings |
| `transcription-queue.json` | Durable queue and Activity state |
| `dictionary.txt` | Human-editable shared Dictionary |
| `openrouter-secrets.json` | OpenRouter API key |
| `openai-secrets.json` | OpenAI API key |
| `soniox-secrets.json` | Soniox API key |
| `list-visibility.json` | Hidden-item state without deleting source files |
| `transcription-costs.json` | Bounded provider usage and local estimates |

Do not put API keys in `settings.json`. The frontend receives only a configured/missing boolean for each provider.

### Capture defaults

Settings patches are clamped or normalized by the Rust backend.

| Setting | Default | Supported range or values |
| --- | --- | --- |
| Recording | `on` | `on` / `off` |
| Transcription | `on` | `on` / `off` |
| Language | `ko` | `auto`, `ko`, `en`, `ja`, `zh`, `es`, `fr`, `de` |
| Soniox language hints | English, Korean | Select several languages in Settings › Audio. Shared with Soniox Dictation; Meetings can override them per transcription. Clear all hints for automatic detection. |
| Transcript translation language | Not set | Select a target language before enabling transcript translation. |
| Primary microphone | `System Default` | one available input |
| Secondary microphone | none | one input, including the same physical microphone as Primary |
| Microphone processing | Merge Audio | Separated Audio / Merge Audio / Priority Audio |
| Threshold | `-40 dBFS` | `-90 … -10` |
| Voice-aware Auto Level | `on` | Local RNN speech detection, adaptive digital gain, clipping protection, and hardware input-volume recommendations when the microphone allows them |
| Mic input volume | Current macOS value | `0 … 100%`; system-authoritative and not reset with recording defaults |
| Attack | `200 ms` | `50 … 2,000 ms` |
| Release | `1,000 ms` | `250 … 5,000 ms` |
| Pre-roll | `400 ms` | `0 … 1,500 ms` |
| Lead-in | `200 ms` | `0 … 2,000 ms` |
| Post-roll | `400 ms` | `0 … 2,000 ms` |
| Min Chunk | `800 ms` | `100 … 5,000 ms` |
| Max Chunk | `180,000 ms` | `10,000 … 900,000 ms` |
| Audio format | `m4a` | `m4a`, `mp3`, `wav` |
| Compressed bitrate | `96 kbps` | `64`, `96`, `128 kbps`; ignored for WAV |
| Save root | `~/Documents/WakeNote` | user-selected directory |
| Selected model | `whisper-medium` | compatible ready model |
| Model directory | `~/Library/Application Support/WakeNote/models` | user-selected directory |
| Shared Dictionary | `on` | `on` / `off` |
| Hide low-confidence transcripts | `on` | `on` / `off` |
| Start live input on launch | `on` | `on` / `off` |
| Input monitoring | `off` | `on` / `off`; Primary only |
| Shortcut dictation | `off` | `on` / `off` |
| Hold-to-dictate hotkey | `Option+Space` | supported key, key chord, or physical modifier combination |
| Toggle dictation hotkey | not set | optional separate hotkey; press to start, press again to finish |
| Dictation language | `auto` | same language set as archival transcription |
| Subtitle | `off` | `on` / `off` |
| Subtitle position | top center | off; top/bottom × left/center/right |
| Subtitle font size | `24 px` | `10 … 48 px` |
| Subtitle background | black at `82%` | black/white, `0 … 100%` |
| Subtitle horizontal padding | `18 px` | `0 … 64 px` |
| Subtitle vertical padding | `14 px` | `0 … 64 px` |
| Subtitle border | `1 px`, `#ffffff` | `0 … 8 px`, six-digit hex color |
| Subtitle corner radius | `8 px` | `0 … 48 px` |
| Subtitle maximum width | `720 px` | `0 … 1,600 px` |
| Subtitle height | `58 … 1,000 px` | minimum/maximum `0 … 1,200 px`; normalized so minimum ≤ maximum |
| Minimum subtitle duration | `5 sec` | `1 … 10 sec` |
| Subtitle animation | fade | instant / fade / dissolve |
| Theme | dark | light / dark |
| OpenRouter report model | `z-ai/glm-5.2` | provider model ID |
| Maximum report iterations | `3` | `1 … 30` |

### Model capabilities

The model picker filters by context. A model that is valid for a saved file is not automatically valid for realtime preview or diarization.

| WakeNote model | Runtime | Selectable contexts | Setup |
| --- | --- | --- | --- |
| Whisper Small / Medium / Turbo / Large | `whisper-rs` with Metal on macOS | File, Dictation, Meeting | Download and verify in Models |
| Parakeet TDT 0.6B V3 | bundled sherpa-onnx | File, Dictation, Meeting | Download and verify in Models |
| SenseVoice Small | bundled sherpa-onnx | File, Dictation, Meeting | Download and verify in Models |
| Nemotron 3.5 ASR Streaming 0.6B | bundled sherpa-onnx | File, Realtime, Dictation, Meeting | Download and verify in Models |
| Qwen3-ASR 0.6B / 1.7B | isolated Transformers runtime | File, Dictation, Meeting | Run **Set up Qwen3-ASR** in Models |
| OpenRouter · Qwen3 ASR Flash | `qwen/qwen3-asr-flash-2026-02-10` | File, Dictation, Meeting | OpenRouter key |
| OpenAI · GPT Transcribe | `gpt-transcribe` | File, Realtime, Dictation, Meeting | OpenAI key |
| OpenAI · GPT Live Transcribe | `gpt-live-transcribe` | Realtime, Dictation | OpenAI key; streaming required |
| OpenAI · GPT-4o Transcribe Diarize | `gpt-4o-transcribe-diarize` | File, Meeting | OpenAI key; supports speaker separation |
| Soniox · Async V5 | `stt-async-v5` | File, Dictation, Meeting | Soniox key |
| Soniox · Real-time V5 | `stt-rt-v5` | Realtime, Dictation | Soniox key; streaming required |

Plain `cargo test` leaves the optional `asr-sherpa` feature disabled to keep local Rust iteration lighter. The repository packaging path enables and bundles it automatically.

### Cloud credentials and network behavior

Save or delete keys in **Settings › Integrations › External AI API keys**:

| Provider | Used for |
| --- | --- |
| OpenRouter | Qwen3 ASR Flash, transcript summaries, and detailed reports |
| OpenAI | GPT file, realtime, and diarization transcription |
| Soniox | Async V5 and Real-time V5 transcription |

Selecting a cloud model sends audio and supported context to that provider. Connectivity, retention, rate limits, privacy, and billing follow the provider account. Missing keys and classified provider failures remain visible on the selected job; WakeNote does not silently switch cloud providers.

**Cost-limit fallback** is opt-in. For a cloud model, choose an installed compatible on-device model and enable the switch. WakeNote retries once only when the primary API response is classified as an exhausted balance or spend limit. Other authentication, provider, transport, decoding, and content failures do not trigger fallback. Requested, effective, and fallback model IDs are stored in provenance metadata.

### Transcription API usage and cost

Open **Settings › Models › Transcription API cost › Details** for the dedicated
**API usage & cost** page. Choose start/end dates and **Daily**, **Weekly**, or
**Monthly** grouping, then filter by API provider and model. Quick ranges cover
the last 7, 30, or 90 days and the last year. Weeks start on Monday in this Mac's
time zone; ranges can span up to 732 days.

The stacked bar chart shows estimated cost by provider. The line chart switches
between cumulative estimated cost and audio minutes. The comparison table shows
requests, audio time, estimated USD, cost per audio hour, and unpriced requests
for each API/model. The paginated period table exposes exact chart values.

**Include Soniox reference estimates** starts on in Details. It estimates missing
Soniox V5 prices from saved duration using the published approximate equivalents
of $0.10/hour for async and $0.12/hour for realtime, checked September 11, 2026.
Soniox bills by tokens, so these are comparison estimates rather than invoices.
Turn it off to match recorded-only costs in the compact Settings card. Original
usage records are never rewritten. Unsupported or missing prices remain **Not
priced**; partial known charges are marked **+ unknown**. Cost per audio hour is
omitted when a model group still contains unpriced requests.

WakeNote retains up to 10,000 local usage records. The page warns when the selected
range reaches the oldest day of a full ledger. Other applications, unrecorded
failures and charges without usage records are excluded.

### Shared Dictionary

Use **Settings › Models › Dictionary** or edit `dictionary.txt` through the provided action. The text format is:

```text
# Canonical term = aliases
WakeNote = wake note, 웨이크 노트
Qwen3-ASR = qwen 3 asr
Soniox
```

Blank lines and `#` comments are ignored. The file accepts at most 1 MiB and 1,000 active lines. A malformed edit reports the line and keeps the last valid Dictionary active.

WakeNote sends canonical terms as native provider context where supported, then performs deterministic alias-to-canonical correction before saving, captioning, or typing. ASCII aliases match case-insensitively at alphanumeric boundaries; non-ASCII aliases match exactly. There is no fuzzy replacement.

### Subtitle presentation

Enable **Settings › Integrations › Subtitle** and choose any top/bottom × left/center/right position. Corner positions use a smaller display-edge margin than the recording waveform, while the centered positions keep their existing anchor.

Padding, border width/color/radius, maximum width, and minimum/maximum height are numeric pixel settings. Short subtitles remain content-sized; long subtitles never grow beyond the configured maximum width. The backend balances long lines from the current full caption, softly preferring sentence endings when the visual result is otherwise similar. The same explicit lines size the native window and render in the web content. Unbreakable URLs and long tokens remain intact and may wrap anywhere only as an overflow safeguard.

Changing a presentation option shows the real overlay as a preview. Each edit restarts the preview for at least the configured 1–10 second minimum. Longer live, dictation, and preview text stays visible for its estimated reading time, capped at 30 seconds. All caption sources share the same generation-safe expiry, so an older timer cannot hide newer text and a final subtitle does not remain on screen indefinitely.

### History navigation and Trash

**Transcripts** and **Activity** open newest-first and can switch to oldest-first. Both navigate one local calendar day at a time. Transcripts renders at most 50 filtered rows per page; changing the day, filter, or order returns to the first page and clears stale off-page selection. This bounds browser DOM and audio-control work, while the existing per-day `all.json` remains the disk index rather than a server-style cursor.

Activity's **Warnings & errors**, **Needs attention**, and **Resolved** views review outcomes rather than one day's work, so they gather every match across all dates and hide the date picker. Rows keep a day heading, the header checkbox selects every match across dates and pages, and Move to Trash acts on that whole selection.

Selected Transcript rows can be moved to Trash together. WakeNote stages every exact-stem audio, `.txt`, `.json`, and `.error.txt` sibling, rolls the bundle back if a move fails, then refreshes that day's `all.json`. Activity uses the same bundle transaction. Cancelling a running Activity job is idempotent and suppresses late partial/final writes; deleting one requests cancellation and waits for its worker to exit before moving the bundle and removing the queue record. Trash remains recoverable through macOS Finder.

### Shortcut dictation

Enable **Settings › Dictation › Shortcut dictation**, choose a compatible model and language, then keep the cursor in the destination application:

1. Use **Hold-to-dictate hotkey** (default **Option+Space**) to record while holding the keys; release to stop and transcribe.
2. Alternatively, set **Toggle dictation hotkey**. Press it once to start recording, release the keys while speaking, then press it again to stop and transcribe.
3. WakeNote saves the dictation and types non-empty final text at the focused cursor.

Click either hotkey field and press the desired keys. Release modifier-only combinations to save them. The two hotkeys and the enabled Enhanced Prompt shortcut must not overlap. **Clear** removes the optional toggle hotkey; existing hold hotkeys are preserved when updating WakeNote.

Dictation uses the Primary and optional Secondary microphones selected in Audio and automatically selects the cleanest signal. Its stream is independent from voice-activated archival capture. VOR keeps recording during Dictation, but overlapping VOR chunks stay audio-only and skip transcription to avoid saving the same speech twice. Each Dictation attempt is stored with a `dictation` source label. Very quiet or empty output is not typed, and recording stops automatically after ten minutes. Press **Escape** to cancel, or press a dictation hotkey during transcription to cancel processing. **Copy result to clipboard** keeps the inserted result by default; turn it off to restore the previous clipboard after insertion.

The top-center feedback bubble remains available even when Subtitle is off. Microphone permission is required for capture; Accessibility permission is required for focused-cursor typing.

### Recent Dictations

**Settings › Dictation › Recent Dictations** shows the ten latest saved dictation transcripts across dates. Expand an entry to read the full original transcript, check its microphone, or **Copy text**. Hidden transcripts are excluded. Use the refresh button to reload the list.

**View in Transcripts** opens that entry's date with the dictation filter selected. **View all dictations** opens the same daily browser at the latest result. You can also choose **Source › Dictations** directly on the **Transcripts** page. The filter stays selected while changing dates or switching visible/hidden records, including days with no dictations. Choose **All sources** to show other captures again.

### Live Transcription Webhook

Use **Settings › Advanced › Live Transcription Webhook** to send newly finalized
live transcript chunks to one HTTP(S) endpoint. Choose **Text Only** or **JSON**,
enable the switch, and changes save automatically. URL edits save after a 700ms
pause; successive settings writes are spaced at least one second apart. JSON shares the saved
recording metadata format; neither mode uploads audio. The feature starts off,
does not send test requests on save, and never automatically replays past transcripts.
The status indicates pending, saving, saved, or failed changes. Failed edits stay
available with **Retry saving**. Valid pending edits are queued when leaving the page.

Delivery runs in the background with a 15-second timeout per attempt and up to
three retries after 1, 2, and 4 seconds. Up to four requests can be sent at once
across new deliveries and all retries; additional requests wait in the queue.
HTTPS is recommended. Delivery may fail,
arrive out of order, or be duplicated; receivers can deduplicate using
`X-Transcription-Event-Id`. Disabling or changing the endpoint/format cancels
unfinished deliveries.

Open **Webhooks**, below **Activity**, to review the last 24 hours of delivery
results. The page has status filters, summary counts, and 50 requests per page.
Each request shows a one-line transcription preview. Choose **More** to expand
the full text with its original line breaks, and **Less** to collapse it.
**Details** shows the transcription alongside the delivery status, HTTP result,
attempts, creation time, format, and event ID. JSON requests also offer an
expandable **Request payload (JSON)** section with the saved request body.
Select failed or queue-full requests on the current page and choose **Retry
selected**. Retries preserve the original request content and event ID; enable
the webhook with the original endpoint and format before retrying.

**Automatically delete old webhook history**, in the same Advanced settings
card, defaults to on and removes history and saved request content after 24 hours.
Turn it off and save to retain older records locally. Results at least 24 hours
old are always hidden and cannot be retried; manual retry does not reset their
age. Changing automatic deletion does not interrupt delivery.

### Realtime auto-type

Enable **Settings › Advanced › Auto-type transcripts into cursor** and select an **Auto-type model** independently from archival transcription. Choose an installed Whisper model or a configured compatible realtime cloud model; leaving the selection at the default follows the transcription model.

Auto-type appends the stable shared prefix of successive partial results, then the remaining final text. While enabled, microphone chunks roll over within five seconds so final-only recognition does not wait for a long recording. Actual latency depends on the selected model and provider. Imports, system/app audio, and a separate Secondary stream never type into the cursor. Dictation temporarily suppresses auto-type to avoid duplicate input.

WakeNote does not backspace over text already inserted. If a model revises that prefix or native typing fails, an error is shown and the corrected source remains available in Activity/Transcripts. Keep the destination focused and grant Accessibility permission before enabling auto-type.

### Translation and Enhanced Prompt

Save an OpenRouter key under **Settings › Integrations**. **Translation & prompt model** accepts a separate compatible OpenRouter model ID; blank uses the report model. These features send text to OpenRouter, can incur provider charges, and are disabled by default. Local ASR does not make this optional text processing local.

- **Subtitles:** enable **Translate subtitles** and choose the target language in Settings › Subtitles. Original captions appear while translation is pending or unavailable. Requests are coalesced and stale results discarded; a completed translated caption receives its normal reading time.
- **Transcripts:** choose a target language in Settings › Audio, then enable **Translate transcripts** for automatic translation of displayed rows. The target defaults to **Not set**; choosing **Not set** again turns translation off and cancels pending requests. Existing saved targets are preserved. Translation is shown separately with its own copy/retry controls. Audio and original transcript files are never replaced; translations are cached only for the current app session.
- **Dictation:** enable **Translate dictation** in Settings › Dictation. When recording finishes, the translated text is inserted at the cursor. Failed processing does not paste an untranslated substitute; the original transcript remains saved.
- **Enhanced Prompt:** enable the mode in Settings › Dictation and hold its separate shortcut (default **Control+Option+Space**) while speaking. Release to rewrite the draft as a usable prompt without executing it. Normal Dictation keeps its own shortcut and behavior. Overlapping shortcuts are rejected. If Dictation translation is enabled, the enhanced prompt uses that target language too.

The editable **Enhanced Prompt system prompt** starts with instructions to preserve intent, names, constraints, and uncertainty, avoid invented requirements, and use Goal/Context/Inputs/Requirements/Output format sections only where useful. It restructures your request; it does not answer it. Native automation can use `enhanced_dictation_event` with a `pressed` boolean, while `transform_text` / `cancel_text_transform` expose cancellable text processing.

### Editing prompts and settings

Prompt/model editors keep a local draft. **Save** persists the complete value; **Cancel** restores the saved value; **Reset** loads the default into the draft for review before saving. Background refreshes do not overwrite an unsaved draft, and failed saves retain it with an error. API keys and Dictionary dialogs also retain failed drafts. Sliders persist on release; numeric fields persist on blur/Enter and cancel with Escape.

### Reports

Reports use OpenRouter and the configured report model. Start from **Reports › New report** or select captures in **Transcripts** and choose Summary or Report.

- Summary produces a concise Markdown structure with key points, decisions, actions, and open questions.
- Detailed report adds context, chronology, risks, and evidence notes, with grading/refinement up to the configured iteration limit.
- Only one report run is active at a time. Active work can be cancelled; failed or cancelled runs can be retried.
- Report Markdown and metadata remain under `<save_root>/reports/`. Hiding a report changes list visibility and does not delete files.

Prompt templates in **Settings › Integrations › OpenRouter reports** support `{{transcripts}}`, `{{date_range}}`, and `{{selected_count}}`.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `WAKENOTE_INSTALL_PATH` | Default app installation directory for the build script |
| `WAKENOTE_QWEN3_ASR_UV` | Explicit `uv` executable for Qwen setup |
| `WAKENOTE_QWEN3_ASR_PYTHON` | Explicit Python executable for Qwen setup |

Advanced external-command models receive runtime variables from WakeNote rather than reading global configuration. See the example below.

## Examples

### Develop the workspace without rebuilding Rust

```bash
pnpm dev
```

The browser fixture includes captures, reports, runs, queue outcomes, and installed models so the main UI is populated. It does not exercise native behavior.

### Keep two microphone identities separate

1. Select Primary and Secondary devices in **Settings › Audio**. The same physical microphone may fill both slots.
2. Choose **Separated Audio** under **Microphone processing**.
3. Record normally.

WakeNote writes separate source-labelled files and interleaves their transcript entries chronologically. Input monitoring still uses Primary only.

### Import and transcribe a meeting

1. Open **Meetings** and import an `mp3`, `m4a`, or `wav` file.
2. Choose a model that supports Meeting.
3. Enable speaker separation only when the selected model advertises diarization.
4. Start transcription and leave the source file in place until WakeNote has copied it into the meeting directory.

Progress is checkpointed to `meeting.json` after each segment so interrupted work can resume.

Manual Meeting recording is streamed through a hidden temporary WAV and finalized as `audio.m4a` with the configured compressed bitrate. The temporary file is removed after successful encoding. If finalization fails, WakeNote keeps recoverable audio instead of discarding the recording. Imported meeting files keep their original `mp3`, `m4a`, or `wav` codec.

### Add an external-command model

Place a command in `<model_directory>/<model-id>.command`. WakeNote passes:

- `WAKENOTE_AUDIO_PATH`
- `WAKENOTE_MODEL_ID`
- `WAKENOTE_MODEL_DIRECTORY`
- `WAKENOTE_LANGUAGE`
- `WAKENOTE_DICTIONARY_TERMS`
- `WAKENOTE_DICTIONARY_JSON`

The command must write only the final transcript to stdout and exit successfully. Treat every path and Dictionary value as untrusted input, and never print credentials.

### Verify a packaged DMG

After `pnpm dmg:downloads`:

```bash
hdiutil verify "$HOME/Downloads/WakeNote_<version>_aarch64.dmg"
shasum -a 256 "$HOME/Downloads/WakeNote_<version>_aarch64.dmg"
```

Use the actual generated filename for `<version>`.

## Troubleshooting

### macOS will not open a downloaded WakeNote

A copy installed from a DMG downloaded in a web browser keeps the macOS quarantine flag, and Gatekeeper blocks quarantined apps that are ad-hoc signed and not notarized. After checking the download against `SHA256SUMS.txt`, run once:

```bash
xattr -dr com.apple.quarantine /Applications/WakeNote.app
```

Use the actual path if WakeNote is installed somewhere other than `/Applications`.

### The app cannot hear a microphone

- Confirm **Settings › Audio › Microphone Permission** is granted.
- Verify that the pinned device is present. WakeNote reconnects to the same device rather than silently replacing it with another physical input.
- Both slots may use the same physical microphone. WakeNote opens that device once and feeds both slots from that capture. `System Default` is supported as Primary even when it routes to the Secondary device.
- Watch each microphone status row for Active, Waiting, or Reconnecting state.
- The input-volume row mirrors macOS. `Digital auto level only` means the device exposes no writable hardware volume; capture remains available and the row is intentionally read-only.
- Some USB interfaces manage gain with a physical knob and do not expose a macOS input-volume property.
- With **Voice-aware Auto Level** enabled, WakeNote detects speech locally and may move a writable macOS input slider during capture. Each physical microphone is leveled once. Slots that share a microphone use that same pass.
- `Hardware + digital auto level` combines macOS input-volume recommendations with adaptive digital gain. `Digital auto level only` keeps the local digital protection when the device is read-only.
- Disable **Voice-aware Auto Level** to use `Manual system volume`. Samples already clipped by the microphone's analog-to-digital converter cannot be reconstructed; lower the physical gain when clipping persists.

### The level moves but no chunk starts

- Use Calibration and compare the speech level with the noise floor.
- A more negative Threshold is more sensitive and may capture background noise.
- Confirm Recording is on and Pause is off.
- Check Attack and Min Chunk if short utterances are missing.

### A queue job stays pending

- Confirm Transcription is on and Pause is off.
- Open **Settings › Models**. Local jobs wait when their selected model is not installed or verified.
- Open **Settings › Integrations**. Cloud jobs require the matching provider key.
- Activity jobs retain the model chosen when they were queued; selecting a different global model does not rewrite existing jobs. Use reprocessing when you intend to choose a new model.

### Activity shows a warning instead of an error

No-speech, empty-transcript, low-confidence, and transcript-artifact outcomes are completed quality warnings. They preserve the recording and remain available for review or model-specific reprocessing. **Mark all resolved** acknowledges them without deleting history.

### A running Activity job does not disappear immediately after Delete

Deletion waits until the active transcription worker acknowledges cancellation so it cannot recreate a transcript or metadata sidecar after the bundle reaches Trash. A non-interruptible cloud request may therefore leave the row visible briefly. Do not force-quit the app during that handoff; WakeNote removes the queue record only after worker exit and a successful recoverable bundle move.

### Cloud transcription failed

- Verify the matching provider key and account status without pasting the key into logs or issue reports.
- Check network connectivity, provider rate limits, request-size limits, and billing state.
- Retry explicitly from Activity after correcting the cause.
- WakeNote preserves the selected provider on ordinary failures. Only an enabled on-device cost-limit fallback handles a classified billing-limit response.

### Dictation records but does not type

- Grant **Accessibility** permission for WakeNote/System Events in **System Settings › Privacy & Security › Accessibility**.
- Keep the destination cursor focused until transcription finishes.
- Empty or low-signal results are deliberately not inserted.
- Pressing a dictation hotkey during transcription cancels processing; wait for completion if you want the result inserted.

### MP3 recording fails

Install `ffmpeg` and ensure it is available in `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, or `PATH`. MP3 encoding uses `libmp3lame`. Select M4A or WAV if MP3 is not required.

### M4A recording fails

Confirm `/usr/bin/afconvert` is available. It is provided by macOS/Xcode Command Line Tools. WAV bypasses the native compressed-audio encoder.

### The browser UI works but native behavior does not

`pnpm dev` uses fixtures. Reproduce through `pnpm tauri dev` or an installed app before drawing conclusions about microphones, ScreenCaptureKit, filesystem writes, permissions, the tray, focused-cursor insertion, or real provider calls.

### DMG packaging reports an unknown argument

Run `pnpm dmg:downloads` directly. `pnpm build dmg:downloads` is invalid because `dmg:downloads` is a package script, not a build CLI argument.

### Known limitations

- The product and packaging workflow are macOS-specific.
- MP3 depends on `ffmpeg`; M4A depends on `/usr/bin/afconvert`.
- Large Whisper and Qwen models can be slow or memory-heavy on lower-end Macs.
- Whisper loads only the official model bytes pinned by WakeNote. Custom or modified `.bin` files are rejected before native parsing; a local registry checksum cannot override this check. Initial loading temporarily uses additional memory equal to the model file size.
- Cloud models send audio to their provider and are subject to external availability, retention, rate-limit, and billing policies.
- Browser fixtures prove frontend behavior only.
- WakeNote is licensed under the [MIT License](LICENSE). Bundled third-party components retain their own licenses.
