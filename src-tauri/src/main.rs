use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
#[cfg(target_os = "macos")]
use objc2_core_graphics::{CGEventFlags, CGEventSource, CGEventSourceStateID};
use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{
    AboutMetadata, CheckMenuItem, HELP_SUBMENU_ID, Menu, MenuItem, PredefinedMenuItem, Submenu,
    WINDOW_SUBMENU_ID,
};
use tauri::path::BaseDirectory;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, State, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use wakenote::audio::{MicHealthAction, list_input_devices};
use wakenote::audio_analysis::AudioWaveform;
use wakenote::audio_merge::{AudioMergeResult, MergeAudioRequest, merge_audio_to_m4a};
use wakenote::caption_layout::StableCaptionPager;
use wakenote::cloud_realtime::{RealtimePartial, RealtimeSamplesRequest};
use wakenote::cloud_transcription::TranscriptionCredentials;
use wakenote::commands::{
    AppBackend, AppStatus, LiveEventHandler, LiveTranscriptEvent, MainWindowCloseAction,
    MicrophoneDevice, RecentTranscript, StartedTranscriptionJob, TrashActivityJobsResult,
    TrashTranscriptsResult, TrayMenuPresentation, TrayRuntimePresentation, TrayState,
    UploadedAudio, live_preview_model_id, main_window_close_action,
    microphone_devices_from_input_devices, open_containing_folder_request,
    recorded_at_for_audio_path, refresh_transcript_day_index_for_recording_path,
    reveal_save_folder_request, tray_icon_image_for_presentation, tray_menu_presentation,
    tray_presentation_for_dictation, tray_presentation_for_state, tray_runtime_presentation,
    validate_audio_playback_file, with_live_runtime_warning,
};
use wakenote::debug_log::append_debug_log_nonblocking as append_debug_log;
use wakenote::dictation::{
    DICTATION_MAX_RECORDING_DURATION, DICTATION_STATE_EVENT, DictationAction,
    DictationCaptureHealth, DictationCaptureResult, DictationHotkeyMode, DictationMicrophoneInput,
    DictationModifierShortcuts, DictationRecording, DictationRuntime, DictationShortcutEvent,
    DictationSlotFailure, DictationStage, DictationStatePayload, ModifierShortcut,
    PhysicalModifierKey, active_dictation_shortcuts, archive_dictation_recording,
    candidate_dictation_settings, modifier_shortcut, normalize_dictation_patch,
    prepare_dictation_audio, transcribe_dictation_recording_execution, validate_dictation_shortcut,
};
use wakenote::dictionary::DictionaryContext;
use wakenote::dictionary_file::{
    DictionaryFileError, DictionaryFileStatus, DictionaryFileStore, DictionaryPoll,
};
use wakenote::input_monitor::InputMonitorRuntime;
use wakenote::live_capture::{
    AudioFrame, AudioInputBackend, AudioInputConfig, AudioStreamHandle, CpalAudioInput,
    LiveCaptureError, LiveCaptureRuntime, ResolvedCpalInputDevice, microphone_labels_match,
};
use wakenote::live_transcription::{
    LivePartialEvent, LivePartialRequest, LiveTranscriptionService,
};
use wakenote::live_webhook::history::{HistoryDetail, HistoryFilter, HistoryPage, RetryResult};
use wakenote::live_webhook::{LiveWebhookTranscript, WebhookService, WebhookStats};
use wakenote::llm_runs::{LlmReportRunSnapshot, LlmRunRuntime, LlmRunStore};
use wakenote::manual_meeting_capture::{
    MANUAL_MEETING_SAMPLE_RATE, ManualMeetingSource, ManualMeetingWriter,
};
use wakenote::meeting::{
    MeetingDetail, MeetingEvent, MeetingFinishedEvent, MeetingRealtimeReplay,
    MeetingRealtimeReplayRequest, MeetingSummary, MeetingTranscriptionRequest,
    cancel_pending_meeting, start_manual_recorded_meeting_capture,
    start_recorded_meeting_transcription,
};
use wakenote::microphone_level::macos::PlatformVolumeBackend;
use wakenote::microphone_level::{
    MicrophoneInputLevel, MicrophoneLevelService, MicrophoneVolumeBackend,
};
use wakenote::models::{
    ModelDescriptor, ModelStatus, ModelStore, StreamingCapability, TranscriptionContext,
    model_supports_context, validate_model_options,
};
use wakenote::multi_capture::MultiCaptureRuntime;
use wakenote::openai_realtime::OpenAiRealtimeManager;
use wakenote::overlay;
use wakenote::overlay_caption::{
    OVERLAY_CAPTION_HIDDEN_EVENT, OVERLAY_CAPTION_UPDATED_EVENT, OverlayCaptionRuntime,
    OverlayCaptionSnapshot, OverlayCaptionSource, adaptive_caption_hold,
};
use wakenote::permission_drag::{self, PermissionDragTarget};
use wakenote::permissions::{
    self, AppPermissions, PERMISSION_GUIDANCE_EVENT, PermissionGrantStatus,
    PermissionGuidancePayload,
};
use wakenote::persistence::{ListVisibilityState, SetListVisibilityRequest};
use wakenote::queue::QueueSnapshot;
use wakenote::recorder::{ChunkMetadata, TranscriptionSidecar};
use wakenote::settings::{
    AppSettings, CaptureMicrophoneEntry, DictationCueSound, DictationCueVolume,
    FloatingOverlayPosition, LaunchAtLoginAction, LiveCaptureRuntimeAction, MicrophoneSlot,
    SettingsPatch, SubtitleAnimation, TranscriptionLanguage, TrayClickAction,
    clamp_llm_max_iterations, expand_user_path, launch_at_login_action_for_patch,
    live_capture_runtime_action_for_patch, live_capture_should_start_on_launch,
    microphone_source_key, resolve_auto_prompt,
};
use wakenote::soniox_realtime::SonioxRealtimeManager;
use wakenote::source_watcher::{
    DetectedSource, SOURCE_MISSING_GRACE_POLLS, SourceTransition,
    compute_source_transition_with_missing_grace_and_auto_preemption, should_auto_capture_source,
    should_defer_source_end_for_recent_audio,
};
use wakenote::sources::source_definitions;
use wakenote::system_audio::{PIPELINE_SAMPLE_RATE, SystemAudioInput, enumerate_windows};
use wakenote::transcription::{
    FallbackTranscriber, RuntimeTranscriber, TranscriptionJobOutcome, TranscriptionJobStatus,
    TranscriptionPartialCallback, TranscriptionWorker, TranscriptionWorkerOptions,
    model_supports_live_partials,
};
use wakenote::transcription_cost::{
    TranscriptionCostDetails, TranscriptionCostEntry, TranscriptionCostLedger,
    TranscriptionCostSnapshot, estimated_provider_cost_usd, ledger_path,
};
use wakenote::updater::RESTART_GATE;
use wakenote::voice_leveling::{
    ServiceHardwareLevelControl, VoiceAwareMicrophoneProcessor, VoiceLevelingPolicy,
};

mod app_updater;
mod microphone_shortcut;
mod autotype_shortcut;
mod input_level_query;

type BackendState = Arc<Mutex<AppBackend>>;
type DictionaryFileState = Arc<Mutex<DictionaryFileStore>>;
type LiveCaptureState = Mutex<MultiCaptureRuntime<CpalAudioInput>>;
const UPDATE_PAUSE_MEETING_MIC_WAIT: Duration = Duration::from_secs(10);
type DictationState = Arc<Mutex<DictationRuntime<CpalAudioInput>>>;
type DictationShortcutDispatcher = mpsc::Sender<(DictationShortcutEvent, DictationHotkeyMode)>;
type ModifierShortcutState = Arc<Mutex<DictationModifierShortcuts>>;
#[derive(Default)]
struct DictationShortcutSuspension(AtomicBool);
type InputMonitorState = Arc<Mutex<InputMonitorRuntime>>;
type AutoTranscriptionState = Arc<AtomicBool>;
type LiveTranscriberState = Arc<Mutex<Option<Arc<LiveTranscriptionService>>>>;
type AutoTypeState = Mutex<wakenote::auto_type::AutoTypeSession>;
type SubtitleTranslationState =
    Arc<Mutex<wakenote::subtitle_translation::SubtitleTranslationQueue>>;

fn apply_subtitle_translation(
    app: &AppHandle,
    snapshot: &mut OverlayCaptionSnapshot,
    max_width: Option<f64>,
) {
    let Some(state) = app.try_state::<SubtitleTranslationState>() else {
        return;
    };
    let settings = app
        .state::<BackendState>()
        .lock()
        .map(|backend| backend.settings())
        .unwrap_or_default();
    let request =
        wakenote::subtitle_translation::CaptionTranslationRequest::new(snapshot, &settings);
    let (start, translated, waiting) = {
        let Ok(mut queue) = state.lock() else {
            return;
        };
        if let Some(request) = request {
            let start = queue.submit(request.clone());
            (
                start,
                queue.translated_text(&request).map(str::to_string),
                queue.is_waiting(&request),
            )
        } else {
            queue.clear();
            (false, None, false)
        }
    };
    if let Some(captions) = app.try_state::<OverlayCaptionState>()
        && let Ok(mut runtime) = captions.lock()
        && runtime.set_translation_pending(waiting)
        && snapshot.source != OverlayCaptionSource::Preview
    {
        *snapshot = runtime.snapshot();
    }
    if let Some(text) = translated {
        snapshot.text = StableCaptionPager::default()
            .update_for_max_width(
                &text,
                &snapshot.style,
                max_width.unwrap_or(snapshot.style.max_width_px as f64),
            )
            .text;
    }
    if start {
        let app = app.clone();
        let state = state.inner().clone();
        tauri::async_runtime::spawn(async move {
            let client = wakenote::llm::ReqwestOpenRouterClient::default();
            loop {
                tokio::time::sleep(Duration::from_millis(350)).await;
                let next = state.lock().ok().and_then(|mut queue| queue.take_next());
                let Some((request, cancellation)) = next else {
                    break;
                };
                let key = app
                    .state::<BackendState>()
                    .lock()
                    .map_err(|error| error.to_string())
                    .and_then(|backend| backend.load_openrouter_api_key())
                    .map(|key| key.unwrap_or_default());
                let result = match key {
                    Ok(key) => wakenote::text_transform::transform_text_with_client(
                        &client,
                        &request.settings,
                        &key,
                        &wakenote::text_transform::TextTransformRequest {
                            kind: wakenote::text_transform::TextTransformKind::Translate,
                            text: request.text.clone(),
                            target_language: Some(request.settings.subtitle_translation_language),
                        },
                        &cancellation,
                    )
                    .await
                    .map(|result| result.text),
                    Err(error) => Err(error),
                };
                let accepted = state
                    .lock()
                    .map(|mut queue| queue.complete(&request, result.clone()))
                    .unwrap_or(false);
                if accepted {
                    if let Err(error) = result {
                        let _ = app.emit("text-transform-error", format!("Subtitle translation failed: {error}. The original text remains available."));
                    }
                    if let Some(snapshot) = current_caption_snapshot(&app) {
                        publish_overlay_caption_snapshot(&app, snapshot, "subtitle translation");
                    }
                }
            }
        });
    }
}
type TextTransformJobs =
    Mutex<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>;

#[tauri::command]
async fn transform_text(
    state: State<'_, BackendState>,
    jobs: State<'_, TextTransformJobs>,
    request_id: String,
    request: wakenote::text_transform::TextTransformRequest,
) -> Result<wakenote::text_transform::TextTransformResult, String> {
    let _update_activity = RESTART_GATE.activity()?;
    if request_id.is_empty() || request_id.len() > 128 {
        return Err("Invalid text-processing request id".into());
    }
    let (settings, key) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        (
            backend.settings(),
            backend.load_openrouter_api_key()?.unwrap_or_default(),
        )
    };
    let cancellation = tokio_util::sync::CancellationToken::new();
    {
        let mut pending = jobs.lock().map_err(|error| error.to_string())?;
        if pending.contains_key(&request_id) {
            return Err("This text-processing request is already running".into());
        }
        if pending.len() >= 8 {
            return Err("Too many text-processing requests. Try again shortly.".into());
        }
        pending.insert(request_id.clone(), cancellation.clone());
    }
    let result = wakenote::text_transform::transform_text_with_client(
        &wakenote::llm::ReqwestOpenRouterClient::default(),
        &settings,
        &key,
        &request,
        &cancellation,
    )
    .await;
    if let Ok(mut pending) = jobs.lock() {
        pending.remove(&request_id);
    }
    result
}

#[tauri::command]
fn cancel_text_transform(
    jobs: State<'_, TextTransformJobs>,
    request_id: String,
) -> Result<(), String> {
    if let Some(token) = jobs
        .lock()
        .map_err(|error| error.to_string())?
        .get(&request_id)
    {
        token.cancel();
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
enum AutoTypeUpdate {
    Started,
    Partial(String),
    /// Provider-confirmed words of the chunk so far.
    Confirmed(String),
    Final(String),
    Failed,
}

/// Realtime partials carry provider-confirmed words when the provider has them.
fn realtime_auto_type_update(partial: &RealtimePartial) -> AutoTypeUpdate {
    if partial.confirmed_text.is_empty() {
        AutoTypeUpdate::Partial(partial.text.clone())
    } else {
        AutoTypeUpdate::Confirmed(partial.confirmed_text.clone())
    }
}

/// Updates that wait for the whole chunk under these settings. The voice gate decides
/// per final chunk, so it holds both partials and confirmed words. Final-only holds
/// partials; confirmed words are final already, so they still go through.
fn auto_type_update_is_held(update: &AutoTypeUpdate, settings: &AppSettings) -> bool {
    match update {
        AutoTypeUpdate::Partial(_) => {
            settings.auto_transcript_input_final_only || settings.voice_gate_enabled
        }
        AutoTypeUpdate::Confirmed(_) => settings.voice_gate_enabled,
        _ => false,
    }
}

fn clear_auto_type(app: &AppHandle) {
    if let Some(state) = app.try_state::<AutoTypeState>()
        && let Ok(mut session) = state.lock()
    {
        session.clear();
    }
}

fn dispatch_auto_type(
    app: &AppHandle,
    source_key: &str,
    slot: Option<MicrophoneSlot>,
    chunk_id: u64,
    update: AutoTypeUpdate,
) {
    if !source_key.starts_with("microphone") || slot == Some(MicrophoneSlot::Secondary) {
        return;
    }
    let input_app = app.clone();
    let scheduled = app.run_on_main_thread(move || {
        let Some(state) = input_app.try_state::<AutoTypeState>() else {
            return;
        };
        let Ok(mut session) = state.lock() else {
            return;
        };
        let settings = input_app
            .try_state::<BackendState>()
            .and_then(|backend| backend.lock().ok().map(|backend| backend.settings()))
            .unwrap_or_default();
        if !settings.auto_transcript_input_enabled
            || settings.pause_all
            || !settings.recording_enabled
            || !settings.transcription_enabled
            || dictation_suppresses_vor_live_transcription(&input_app)
        {
            session.clear();
            return;
        }
        let held = auto_type_update_is_held(&update, &settings);
        let observed = match update {
            _ if held => Ok(()),
            AutoTypeUpdate::Started => session.start(chunk_id),
            AutoTypeUpdate::Partial(text) => {
                session.partial(chunk_id, &text);
                Ok(())
            }
            AutoTypeUpdate::Confirmed(text) => {
                let dictionary = DictionaryContext::from_settings(&settings);
                session.confirm(chunk_id, &text, |raw| dictionary.correct(raw));
                Ok(())
            }
            AutoTypeUpdate::Final(text) => {
                session.finish(chunk_id, &text);
                Ok(())
            }
            AutoTypeUpdate::Failed => {
                session.discard(chunk_id);
                Ok(())
            }
        };
        let result = observed.and_then(|()| {
            session.flush(settings.auto_transcript_input_trailing_space, |text| {
                wakenote::text_input::type_text_into_focused_cursor_with_clipboard(
                    text,
                    wakenote::text_input::ClipboardAfterInput::PreservePrevious,
                )
            })
        });
        for id in session.take_dropped() {
            append_runtime_debug_log(
                &settings,
                format!("[auto-type] final text changed confirmed words; skipped the rest of chunk {id}"),
            );
        }
        if let Err(error) = result {
            session.clear();
            append_runtime_debug_log(&settings, format!("[auto-type] {error}"));
            let _ = input_app.emit("auto-type-error", error);
        }
    });
    if let Err(error) = scheduled {
        clear_auto_type(app);
        let _ = app.emit(
            "auto-type-error",
            format!("Could not schedule auto-type: {error}"),
        );
    }
}
type OverlayCaptionState = Arc<Mutex<OverlayCaptionRuntime>>;
type SubtitlePreviewState = Arc<AtomicU64>;
type IntentionalQuitState = Arc<AtomicBool>;
/// Open system-audio stream handle (dropping it stops capture). Held alongside
/// the mic runtime so the two capture paths are independent. Managed as an
/// `Arc<Mutex<…>>` so the watcher thread and IPC commands share one handle.
type SystemCaptureState = Arc<Mutex<Option<Box<dyn AudioStreamHandle>>>>;
type SourceCaptureLifecycleState = Arc<Mutex<SourceCaptureLifecycle>>;
/// The recognized source currently detected on screen, threaded across polls by
/// the watcher and read by the capture commands.
type DetectedSourceState = Arc<Mutex<Option<DetectedSource>>>;
/// Source ids the user paused manually while the source remains detected.
type SourceCapturePauseState = Arc<Mutex<HashSet<String>>>;
type SourceMeetingMicrophoneLeaseState = Arc<Mutex<Option<SourceMeetingMicrophoneLease>>>;
/// Tracks the single in-flight long-form meeting job and its cancel flag.
/// Only one meeting transcribes at a time (one shared GPU context).
type MeetingState = Arc<Mutex<MeetingRuntime>>;
type ManualMeetingRecordingState = Arc<Mutex<ManualMeetingRecordingRuntime>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceMeetingMicrophoneLease {
    source_id: String,
    generation: u64,
    auto_started: bool,
    user_owned: bool,
}
type TranscriptionCostState = Arc<Mutex<TranscriptionCostLedger>>;
type LlmRunState = Arc<Mutex<LlmRunRuntime>>;
type MicrophoneLevelState = Arc<MicrophoneLevelService<PlatformVolumeBackend>>;
type TrayPresentationCache = Mutex<Option<TrayPresentationSnapshot>>;

const EVENT_DICTIONARY_CHANGED: &str = "dictionary-changed";
const EVENT_DICTIONARY_FILE_ERROR: &str = "dictionary-file-error";
const MICROPHONE_INPUT_LEVELS_CHANGED_EVENT: &str = "microphone-input-levels-changed";
#[derive(Debug, Clone, PartialEq)]
enum DictionaryApplyOutcome {
    Unchanged,
    Changed(Box<AppSettings>),
    Invalid(DictionaryFileError),
}

fn apply_dictionary_poll(backend: &mut AppBackend, poll: DictionaryPoll) -> DictionaryApplyOutcome {
    match poll {
        DictionaryPoll::Unchanged => DictionaryApplyOutcome::Unchanged,
        DictionaryPoll::Changed(dictionary) => {
            let settings = backend.update_settings(SettingsPatch {
                dictionary: Some(dictionary),
                ..Default::default()
            });
            DictionaryApplyOutcome::Changed(Box::new(settings))
        }
        DictionaryPoll::Invalid(error) => DictionaryApplyOutcome::Invalid(error),
    }
}

fn initialize_dictionary_file(
    backend: &mut AppBackend,
    path: PathBuf,
) -> (DictionaryFileStore, DictionaryApplyOutcome) {
    let mut store = DictionaryFileStore::new(path);
    let current = backend.settings().dictionary;
    let outcome = match store.ensure(&current) {
        Ok(poll) => apply_dictionary_poll(backend, poll),
        Err(error) => DictionaryApplyOutcome::Invalid(error),
    };
    (store, outcome)
}

fn emit_dictionary_outcome(
    app: &AppHandle,
    outcome: &DictionaryApplyOutcome,
    status: &DictionaryFileStatus,
) {
    let event = match outcome {
        DictionaryApplyOutcome::Unchanged => return,
        DictionaryApplyOutcome::Changed(_) => EVENT_DICTIONARY_CHANGED,
        DictionaryApplyOutcome::Invalid(_) => EVENT_DICTIONARY_FILE_ERROR,
    };
    if let Err(error) = app.emit(event, status.clone()) {
        eprintln!("[dictionary-file] could not emit {event}: {error}");
    }
}

fn spawn_dictionary_file_watcher(
    app: AppHandle,
    backend_state: BackendState,
    dictionary_state: DictionaryFileState,
) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            let current = match backend_state.lock() {
                Ok(backend) => backend.settings().dictionary,
                Err(error) => {
                    eprintln!("[dictionary-file] backend lock failed: {error}");
                    continue;
                }
            };
            let (poll, status) = match dictionary_state.lock() {
                Ok(mut store) => {
                    let poll = store.poll(&current);
                    let status = store.status();
                    (poll, status)
                }
                Err(error) => {
                    eprintln!("[dictionary-file] store lock failed: {error}");
                    continue;
                }
            };
            let outcome = match backend_state.lock() {
                Ok(mut backend) => apply_dictionary_poll(&mut backend, poll),
                Err(error) => {
                    eprintln!("[dictionary-file] backend apply lock failed: {error}");
                    continue;
                }
            };
            emit_dictionary_outcome(&app, &outcome, &status);
        }
    });
}

#[derive(Default)]
struct MeetingRuntime {
    /// Id of the meeting currently being processed, if any.
    current: Option<String>,
    /// Cancel flag for the current job, checked between segments.
    cancel: Option<Arc<AtomicBool>>,
    /// Meeting jobs waiting for the shared transcription worker.
    queued: VecDeque<MeetingJobSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManualMeetingRecordingPhase {
    Off,
    Recording,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManualMeetingStopReason {
    Manual,
    MaximumDuration,
    ApplicationQuit,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ManualMeetingRecordingStatus {
    generation: u64,
    state: ManualMeetingRecordingPhase,
    meeting_id: Option<String>,
    started_at: Option<String>,
    elapsed_ms: u64,
    remaining_ms: u64,
    inputs: Vec<String>,
    stop_reason: Option<ManualMeetingStopReason>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct StartMeetingTranscriptionRequest {
    model_id: String,
    language: TranscriptionLanguage,
    #[serde(default)]
    language_hints: Option<Vec<TranscriptionLanguage>>,
    streaming_enabled: bool,
    speaker_separation_enabled: bool,
}

#[derive(Default)]
struct ManualMeetingRecordingRuntime {
    generation: u64,
    session: Option<ManualMeetingSession>,
    last_meeting_id: Option<String>,
    last_stop_reason: Option<ManualMeetingStopReason>,
    error: Option<String>,
}

struct ManualMeetingSession {
    generation: u64,
    meeting_id: String,
    started_at: DateTime<Utc>,
    started_instant: Instant,
    microphone: LiveCaptureRuntime<CpalAudioInput>,
    system: LiveCaptureRuntime<SystemAudioInput>,
    writer: Option<ManualMeetingWriter>,
}

impl ManualMeetingRecordingRuntime {
    fn status(&self, now: Instant) -> ManualMeetingRecordingStatus {
        let (state, meeting_id, started_at, elapsed_ms, remaining_ms) =
            if let Some(session) = self.session.as_ref() {
                let elapsed = now.saturating_duration_since(session.started_instant);
                (
                    if session
                        .writer
                        .as_ref()
                        .is_some_and(ManualMeetingWriter::overflowed)
                    {
                        ManualMeetingRecordingPhase::Error
                    } else {
                        ManualMeetingRecordingPhase::Recording
                    },
                    Some(session.meeting_id.clone()),
                    Some(session.started_at.to_rfc3339()),
                    elapsed.as_millis().min(u64::MAX as u128) as u64,
                    MAX_MANUAL_MEETING_DURATION
                        .saturating_sub(elapsed)
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                )
            } else {
                (
                    if self.error.is_some() {
                        ManualMeetingRecordingPhase::Error
                    } else {
                        ManualMeetingRecordingPhase::Off
                    },
                    self.last_meeting_id.clone(),
                    None,
                    0,
                    MAX_MANUAL_MEETING_DURATION.as_millis() as u64,
                )
            };
        ManualMeetingRecordingStatus {
            generation: self.generation,
            state,
            meeting_id,
            started_at,
            elapsed_ms,
            remaining_ms,
            inputs: vec!["Microphone".to_string(), "System Audio".to_string()],
            stop_reason: self.last_stop_reason,
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeetingJobSpec {
    id: String,
    save_root: PathBuf,
    model_directory: PathBuf,
    model_id: String,
    suppress_low_confidence: bool,
    dictionary: DictionaryContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeetingJobScheduleOutcome {
    Started,
    Queued,
    AlreadyScheduled,
}

fn schedule_meeting_job(
    runtime: &mut MeetingRuntime,
    job: MeetingJobSpec,
    queue_if_busy: bool,
) -> Result<(MeetingJobScheduleOutcome, Option<Arc<AtomicBool>>), String> {
    if runtime.current.as_deref() == Some(job.id.as_str())
        || runtime.queued.iter().any(|queued| queued.id == job.id)
    {
        return Ok((MeetingJobScheduleOutcome::AlreadyScheduled, None));
    }

    if let Some(current) = runtime.current.as_ref() {
        if !queue_if_busy {
            return Err(format!(
                "Another meeting ({current}) is being processed. Try again after it finishes."
            ));
        }
        runtime.queued.push_back(job);
        return Ok((MeetingJobScheduleOutcome::Queued, None));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    runtime.current = Some(job.id);
    runtime.cancel = Some(cancel.clone());
    Ok((MeetingJobScheduleOutcome::Started, Some(cancel)))
}

fn complete_meeting_job(
    runtime: &mut MeetingRuntime,
    completed_id: &str,
) -> Option<(MeetingJobSpec, Arc<AtomicBool>)> {
    if runtime.current.as_deref() != Some(completed_id) {
        return None;
    }

    let Some(next_job) = runtime.queued.pop_front() else {
        runtime.current = None;
        runtime.cancel = None;
        return None;
    };
    let next_cancel = Arc::new(AtomicBool::new(false));
    runtime.current = Some(next_job.id.clone());
    runtime.cancel = Some(next_cancel.clone());
    Some((next_job, next_cancel))
}

#[derive(Debug, Clone)]
struct TrayPresentationUpdate {
    settings: AppSettings,
    status: AppStatus,
    dictation_stage: DictationStage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayPresentationSnapshot {
    runtime: TrayRuntimePresentation,
    menu: TrayMenuPresentation,
    show_menu_on_left_click: bool,
    dictation_stage: DictationStage,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum SourceCaptureLifecycle {
    #[default]
    Idle,
    Starting {
        source_id: String,
        attempt: u8,
    },
    Running {
        source_id: String,
    },
    Stopping {
        source_id: String,
    },
    Failed {
        source_id: String,
        failed_at: Instant,
        attempts: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceCaptureStartDecision {
    Start { attempt: u8 },
    AlreadyStarting,
    AlreadyRunning,
    Stopping,
    BackingOff { retry_after: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceCaptureAttemptOutcome {
    Started,
    AlreadyCapturing,
    Deferred,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceCaptureAttemptResult {
    status: AppStatus,
    outcome: SourceCaptureAttemptOutcome,
}

trait SourceCaptureStreamStarter {
    fn start_source_capture_stream(
        &mut self,
        source: &DetectedSource,
        sample_rate: u32,
        on_frame: Arc<dyn Fn(AudioFrame) + Send + Sync>,
    ) -> Result<Box<dyn AudioStreamHandle>, LiveCaptureError>;
}

struct SystemAudioStreamStarter;

impl SourceCaptureStreamStarter for SystemAudioStreamStarter {
    fn start_source_capture_stream(
        &mut self,
        source: &DetectedSource,
        sample_rate: u32,
        on_frame: Arc<dyn Fn(AudioFrame) + Send + Sync>,
    ) -> Result<Box<dyn AudioStreamHandle>, LiveCaptureError> {
        let mut input = SystemAudioInput::new();
        input.set_target_app(source.pid, source.app_name.clone());
        let mut runtime = LiveCaptureRuntime::new(input);
        runtime.start(
            AudioInputConfig {
                device_id: source.source_id.clone(),
                sample_rate: Some(sample_rate),
                label_hint: None,
                core_audio_uid: None,
            },
            move |frame| on_frame(frame),
        )?;
        Ok(Box::new(runtime))
    }
}

const EVENT_LIVE_STARTED: &str = "live-transcript-started";
const EVENT_LIVE_PARTIAL: &str = "live-transcript-partial";
const EVENT_LIVE_COMMITTED: &str = "live-transcript-committed";
const EVENT_LIVE_FINAL: &str = "live-transcript-final";
const EVENT_LIVE_FAILED: &str = "live-transcript-failed";
const EVENT_TRANSCRIPTION_MODEL_FALLBACK: &str = "transcription-model-fallback";
const EVENT_TRANSCRIPTION_COST_UPDATED: &str = "transcription-cost-updated";
const EVENT_MANUAL_MEETING_RECORDING_STATE: &str = "manual-meeting-recording-state";
const MAX_MANUAL_MEETING_DURATION: Duration = Duration::from_secs(5 * 60 * 60);
const ACTIVITY_DELETE_WORKER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const EVENT_SOURCE_DETECTED: &str = "source-detected";
const EVENT_SOURCE_ENDED: &str = "source-ended";
const EVENT_SOURCE_CAPTURE_STARTED: &str = "source-capture-started";
const EVENT_SOURCE_CAPTURE_STOPPED: &str = "source-capture-stopped";
const EVENT_SOURCE_CAPTURE_ERROR: &str = "source-capture-error";
const EVENT_MEETING_PROGRESS: &str = "meeting-progress";
const EVENT_MEETING_SEGMENT: &str = "meeting-segment-committed";
const EVENT_MEETING_FINISHED: &str = "meeting-finished";
const EVENT_LLM_REPORT_RUN_UPDATED: &str = "llm-report-run-updated";
/// How often the watcher re-enumerates windows while the feature is enabled.
const SOURCE_WATCH_INTERVAL: Duration = Duration::from_secs(5);
const SOURCE_WATCH_DIAGNOSTIC_INTERVAL: Duration = Duration::from_secs(60);
const MAIN_WINDOW_LABEL: &str = "main";
const MAIN_WINDOW_TITLE: &str = "WakeNote";
const CLOSE_SETTINGS_WINDOW_MENU_ID: &str = "close-settings-window";
const MAIN_WINDOW_WIDTH: f64 = 1040.0;
const MAIN_WINDOW_HEIGHT: f64 = 922.0;
const MAIN_WINDOW_MIN_WIDTH: f64 = 980.0;
const MAIN_WINDOW_MIN_HEIGHT: f64 = 640.0;
/// How long we wait for cpal's `host.input_devices()` enumeration to
/// finish before giving up with "audio device lookup did not finish
/// within N seconds". cpal blocks on CoreAudio on macOS, which can
/// take several seconds the first time after a sleep/wake cycle or
/// when many Bluetooth devices are advertising. 2s was too aggressive
/// — bumped to 5s.
const AUDIO_DEVICE_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
const LAUNCH_AUTO_START_RETRY_DELAY_SECS: [u64; 6] = [2, 5, 10, 20, 30, 60];
const OVERLAY_LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(50);
const MAX_PARALLEL_TRANSCRIPTIONS: usize = 1;
const MIC_RECOVERY_TICK_INTERVAL: Duration = Duration::from_millis(500);
const SOURCE_CAPTURE_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
];
const SOURCE_CAPTURE_AUDIO_LIVENESS_HOLD: Duration = Duration::from_secs(20);
/// Sleep inserted between a watchdog-driven `stop` and the immediately
/// following `start`. macOS CoreAudio occasionally retains wedged state
/// when a device is reopened the instant after it's released; a brief
/// pause lets the driver drop its handles and avoids re-opening into the
/// same wedged stream.
const MIC_RECOVERY_SETTLING_DELAY: Duration = Duration::from_millis(200);
/// After this many consecutive watchdog escalations without a single
/// non-silent frame, surface a UI warning so the user knows the watchdog
/// is fighting (and that pressing Refresh or changing priority may help).
const MIC_WEDGE_WARNING_THRESHOLD: u32 = 4;
const DEFAULT_RECENT_TRANSCRIPT_LIMIT: usize = 50;
const MAX_RECENT_TRANSCRIPT_LIMIT: usize = 5_000;
#[cfg(test)]
const TRAY_MENU_ORDER: &[&str] = &[
    "open-settings",
    "separator",
    "toggle-recording",
    "toggle-transcription",
    "toggle-subtitles",
    "toggle-auto-type",
    "active-model",
    "active-microphone",
    "threshold",
    "separator",
    "reveal-save-folder",
    "pause-all",
    "separator",
    "cancel-current-operation",
    "separator",
    "quit",
];

#[derive(Debug, Clone, Serialize)]
struct LiveStartedPayload {
    source_key: String,
    source_label: String,
    microphone_slot: Option<MicrophoneSlot>,
    chunk_id: u64,
    started_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct LivePartialPayload {
    source_key: String,
    source_label: String,
    microphone_slot: Option<MicrophoneSlot>,
    chunk_id: u64,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct LiveCommittedPayload {
    source_key: String,
    source_label: String,
    microphone_slot: Option<MicrophoneSlot>,
    chunk_id: u64,
    audio_path: String,
    will_transcribe: bool,
}

#[derive(Debug, Clone, Serialize)]
struct LiveFinalPayload {
    source_key: String,
    source_label: String,
    microphone_slot: Option<MicrophoneSlot>,
    chunk_id: Option<u64>,
    audio_path: String,
    recorded_at: String,
    text: String,
    /// Voice gate verdict: "me" | "other" | "unknown"; absent when the gate is off.
    speaker: Option<wakenote::voice_gate::SpeakerKind>,
    speaker_score: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
struct LiveFailedPayload {
    source_key: String,
    source_label: String,
    microphone_slot: Option<MicrophoneSlot>,
    chunk_id: Option<u64>,
    audio_path: String,
    recorded_at: String,
    error: String,
}

#[derive(Debug, Clone, Serialize)]
struct TranscriptionModelFallbackPayload {
    requested_model_id: String,
    effective_model_id: String,
}

fn dictation_allows_caption_window_mutation(
    stage: DictationStage,
    source: OverlayCaptionSource,
) -> bool {
    matches!(stage, DictationStage::Idle)
        || matches!(
            (stage, source),
            (
                DictationStage::Transcribing,
                OverlayCaptionSource::Dictation
            )
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DictationCaptionUpdate {
    Partial(String),
    Final { audio_path: PathBuf, text: String },
}

fn apply_dictation_caption_update<B: AudioInputBackend + Default>(
    dictation_state: &Arc<Mutex<DictationRuntime<B>>>,
    caption_state: &OverlayCaptionState,
    operation_id: u64,
    update: DictationCaptionUpdate,
    settings: &AppSettings,
) -> Option<OverlayCaptionSnapshot> {
    if !settings.dictation_subtitles_enabled() {
        return None;
    }
    let update_has_text = match &update {
        DictationCaptionUpdate::Partial(text) | DictationCaptionUpdate::Final { text, .. } => {
            text.split_whitespace().next().is_some()
        }
    };
    if !update_has_text {
        return None;
    }
    let dictation = dictation_state.lock().ok()?;
    if dictation.stage() != DictationStage::Transcribing
        || !dictation.is_operation_active(operation_id)
    {
        return None;
    }

    let position = settings.effective_dictation_subtitle_position();
    let style = settings.floating_overlay_caption_style();
    let mut runtime = caption_state.lock().ok()?;
    let changed = match update {
        DictationCaptionUpdate::Partial(text) => runtime.show_partial_for_source(
            OverlayCaptionSource::Dictation,
            operation_id,
            text,
            position,
            style,
        ),
        DictationCaptionUpdate::Final { audio_path, text } => runtime.show_final_for_source(
            OverlayCaptionSource::Dictation,
            Some(operation_id),
            audio_path,
            text,
            position,
            style,
        ),
    };
    changed.then(|| runtime.snapshot())
}

fn current_dictation_caption_settings(app: &AppHandle, fallback: &AppSettings) -> AppSettings {
    app.try_state::<BackendState>()
        .and_then(|state| state.lock().ok().map(|backend| backend.settings()))
        .unwrap_or_else(|| fallback.clone())
}

fn publish_dictation_partial(
    app: &AppHandle,
    operation_id: u64,
    text: impl Into<String>,
    fallback_settings: &AppSettings,
) -> bool {
    let Some(dictation_state) = app.try_state::<DictationState>() else {
        return false;
    };
    let Some(caption_state) = app.try_state::<OverlayCaptionState>() else {
        return false;
    };
    let settings = current_dictation_caption_settings(app, fallback_settings);
    let snapshot = apply_dictation_caption_update(
        dictation_state.inner(),
        caption_state.inner(),
        operation_id,
        DictationCaptionUpdate::Partial(text.into()),
        &settings,
    );
    let Some(snapshot) = snapshot else {
        return false;
    };
    publish_overlay_caption_snapshot(app, snapshot, "Dictation partial caption");
    true
}

fn publish_dictation_final(
    app: &AppHandle,
    operation_id: u64,
    audio_path: PathBuf,
    text: impl Into<String>,
    fallback_settings: &AppSettings,
) -> bool {
    let Some(dictation_state) = app.try_state::<DictationState>() else {
        return false;
    };
    let Some(caption_state) = app.try_state::<OverlayCaptionState>() else {
        return false;
    };
    let settings = current_dictation_caption_settings(app, fallback_settings);
    let snapshot = apply_dictation_caption_update(
        dictation_state.inner(),
        caption_state.inner(),
        operation_id,
        DictationCaptionUpdate::Final {
            audio_path,
            text: text.into(),
        },
        &settings,
    );
    let Some(snapshot) = snapshot else {
        return false;
    };
    publish_overlay_caption_snapshot(app, snapshot, "Dictation final caption");
    true
}

fn hide_dictation_caption(app: &AppHandle) -> bool {
    let Some(caption_state) = app.try_state::<OverlayCaptionState>() else {
        return false;
    };
    let snapshot = caption_state.lock().ok().and_then(|mut runtime| {
        runtime
            .hide_source(OverlayCaptionSource::Dictation)
            .then(|| runtime.snapshot())
    });
    let Some(snapshot) = snapshot else {
        return false;
    };
    publish_overlay_caption_snapshot(app, snapshot, "Dictation caption cleanup");
    true
}

fn begin_subtitle_preview(state: &AtomicU64) -> u64 {
    loop {
        let current = state.load(Ordering::SeqCst);
        let token = (current & !1).wrapping_add(2) | 1;
        if state
            .compare_exchange(current, token, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return token;
        }
    }
}

fn finish_subtitle_preview(state: &AtomicU64, token: u64) -> bool {
    state
        .compare_exchange(
            token,
            token.wrapping_add(1),
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn subtitle_preview_is_active(state: &AtomicU64) -> bool {
    state.load(Ordering::SeqCst) & 1 == 1
}

fn caption_snapshot_for_publication(
    requested: OverlayCaptionSnapshot,
    current: Option<OverlayCaptionSnapshot>,
) -> OverlayCaptionSnapshot {
    if requested.source == OverlayCaptionSource::Preview {
        requested
    } else {
        current.unwrap_or(requested)
    }
}

fn current_caption_snapshot(app: &AppHandle) -> Option<OverlayCaptionSnapshot> {
    app.try_state::<OverlayCaptionState>()
        .and_then(|state| state.lock().ok().map(|runtime| runtime.snapshot()))
}

fn latest_caption_snapshot_for_publication(
    app: &AppHandle,
    requested: OverlayCaptionSnapshot,
) -> OverlayCaptionSnapshot {
    if requested.source == OverlayCaptionSource::Preview {
        requested
    } else {
        caption_snapshot_for_publication(requested, current_caption_snapshot(app))
    }
}

fn publish_overlay_caption_snapshot(
    app: &AppHandle,
    snapshot: OverlayCaptionSnapshot,
    context: &'static str,
) {
    let app_for_task = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let mut snapshot = latest_caption_snapshot_for_publication(&app_for_task, snapshot);
        let needs_caption_layout = snapshot.visible
            && !snapshot.text.is_empty()
            && !matches!(snapshot.position, FloatingOverlayPosition::Off);
        let layout_context = needs_caption_layout
            .then(|| overlay::resolve_caption_layout_context(&app_for_task, &snapshot.style))
            .flatten();

        if needs_caption_layout && snapshot.source == OverlayCaptionSource::Preview {
            let mut pager = StableCaptionPager::default();
            snapshot.text = pager
                .update_for_max_width(
                    &snapshot.text,
                    &snapshot.style,
                    layout_context
                        .map(|context| context.effective_max_width_px)
                        .unwrap_or(snapshot.style.max_width_px as f64),
                )
                .text;
        } else if needs_caption_layout
            && let Some(state) = app_for_task.try_state::<OverlayCaptionState>()
            && let Ok(mut runtime) = state.lock()
        {
            runtime.set_effective_max_width(
                layout_context.map(|context| context.effective_max_width_px),
            );
            snapshot = runtime.snapshot();
        }

        snapshot = latest_caption_snapshot_for_publication(&app_for_task, snapshot);

        apply_subtitle_translation(&app_for_task, &mut snapshot,
            layout_context.map(|context| context.effective_max_width_px));

        let preview_protects_visible_window = !snapshot.visible
            && app_for_task
                .try_state::<SubtitlePreviewState>()
                .is_some_and(|state| subtitle_preview_is_active(state.inner()));
        if preview_protects_visible_window {
            return;
        }

        let expiry = app_for_task.try_state::<OverlayCaptionState>().and_then(|state| {
            let runtime_state = state.inner().clone();
            let generation = runtime_state
                .lock()
                .ok()
                .and_then(|mut runtime| runtime.take_expiry_generation_to_schedule());
            generation.map(|generation| (runtime_state, generation))
        });
        if let Some((runtime_state, generation)) = expiry {
            schedule_overlay_caption_hide(app_for_task.clone(), runtime_state, generation);
        }
        let is_visible = snapshot.visible
            && !snapshot.text.is_empty()
            && !matches!(snapshot.position, FloatingOverlayPosition::Off);
        let event_name = if is_visible {
            OVERLAY_CAPTION_UPDATED_EVENT
        } else {
            OVERLAY_CAPTION_HIDDEN_EVENT
        };
        if let Some(window) = app_for_task.get_webview_window(overlay::OVERLAY_LABEL) {
            let _ = window.emit(event_name, snapshot.clone());
        }
        let _ = app_for_task.emit(event_name, snapshot.clone());

        let dictation_stage = app_for_task
            .try_state::<DictationState>()
            .and_then(|state| state.lock().ok().map(|runtime| runtime.stage()))
            .unwrap_or(DictationStage::Idle);
        if !dictation_allows_caption_window_mutation(dictation_stage, snapshot.source) {
            return;
        }

        if is_visible {
            if let Err(error) = overlay::show_caption_overlay(
                &app_for_task,
                snapshot.position,
                &snapshot.text,
                &snapshot.style,
                layout_context.map(|context| context.target_monitor),
            ) {
                eprintln!("[overlay-caption] {context} show failed: {error}");
                if let Err(hide_error) = overlay::hide_overlay(&app_for_task) {
                    eprintln!(
                        "[overlay-caption] {context} cleanup after show failure failed: {hide_error}"
                    );
                }
            }
        } else {
            let exit_duration = subtitle_exit_duration(snapshot.style.animation);
            if exit_duration.is_zero() {
                if let Err(error) = overlay::hide_overlay(&app_for_task) {
                    eprintln!("[overlay-caption] {context} hide failed: {error}");
                }
            } else {
                schedule_subtitle_window_hide(
                    app_for_task.clone(),
                    snapshot.generation,
                    snapshot.source,
                    exit_duration,
                    context,
                );
            }
        }
    }) {
        eprintln!("[overlay-caption] {context} schedule failed: {error}");
        if let Err(hide_error) = overlay::hide_overlay_on_main_thread(app, context) {
            eprintln!(
                "[overlay-caption] {context} cleanup after schedule failure failed: {hide_error}"
            );
        }
    }
}

fn subtitle_exit_duration(animation: SubtitleAnimation) -> Duration {
    match animation {
        SubtitleAnimation::Instant => Duration::ZERO,
        SubtitleAnimation::Fade => Duration::from_millis(180),
        SubtitleAnimation::Dissolve => Duration::from_millis(240),
    }
}

fn schedule_subtitle_window_hide(
    app: AppHandle,
    generation: u64,
    source: OverlayCaptionSource,
    delay: Duration,
    context: &'static str,
) {
    thread::spawn(move || {
        thread::sleep(delay);
        let should_hide = app
            .try_state::<OverlayCaptionState>()
            .and_then(|state| state.lock().ok().map(|runtime| runtime.snapshot()))
            .is_some_and(|snapshot| {
                snapshot.generation == generation && snapshot.source == source && !snapshot.visible
            });
        let dictation_allows_hide = app
            .try_state::<DictationState>()
            .and_then(|state| state.lock().ok().map(|runtime| runtime.stage()))
            .is_none_or(|stage| dictation_allows_caption_window_mutation(stage, source));
        if !should_hide || !dictation_allows_hide {
            return;
        }
        let app_for_task = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if let Err(error) = overlay::hide_overlay(&app_for_task) {
                eprintln!("[overlay-caption] {context} delayed hide failed: {error}");
            }
        }) {
            eprintln!("[overlay-caption] {context} delayed hide schedule failed: {error}");
        }
    });
}

fn schedule_overlay_caption_hide(
    app: AppHandle,
    overlay_caption_state: OverlayCaptionState,
    generation: u64,
) {
    thread::spawn(move || {
        loop {
            let delay = overlay_caption_state
                .lock()
                .ok()
                .and_then(|runtime| runtime.hide_delay_for_generation(generation, Instant::now()));
            let Some(delay) = delay else {
                return;
            };
            if !delay.is_zero() {
                thread::sleep(delay);
                continue;
            }
            break;
        }
        let snapshot = overlay_caption_state.lock().ok().and_then(|mut runtime| {
            runtime
                .hide_if_generation(generation)
                .then(|| runtime.snapshot())
        });
        if let Some(snapshot) = snapshot {
            publish_overlay_caption_snapshot(&app, snapshot, "final hold elapsed");
        }
    });
}

/// Emitted on `source-detected` / `source-ended`, and reused inside the capture
/// status snapshot to describe the recognized source on screen.
#[derive(Debug, Clone, Serialize)]
struct SourcePayload {
    source_id: String,
    label: String,
    app_name: String,
}

impl From<&DetectedSource> for SourcePayload {
    fn from(source: &DetectedSource) -> Self {
        Self {
            source_id: source.source_id.clone(),
            label: source.label.clone(),
            app_name: source.app_name.clone(),
        }
    }
}

fn recording_source_label(source: &DetectedSource) -> String {
    if source.source_id.starts_with("custom-source-") {
        source.label.clone()
    } else {
        source.source_id.clone()
    }
}

fn source_capture_scope(_source: &DetectedSource) -> &'static str {
    "target-app"
}

/// Emitted on `source-capture-started` / `source-capture-stopped`.
#[derive(Debug, Clone, Serialize)]
struct SourceCapturePayload {
    source_id: String,
    label: String,
}

/// Emitted on `source-capture-error`.
#[derive(Debug, Clone, Serialize)]
struct SourceCaptureErrorPayload {
    source_id: String,
    error: String,
}

/// One recognized source returned by `list_recognized_sources`, with its
/// effective auto-prompt preference resolved against current settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RecognizedSourceInfo {
    id: String,
    label: String,
    description: String,
    auto_prompt: bool,
    title_patterns: Vec<String>,
    custom: bool,
}

/// Snapshot returned by `source_capture_status`.
#[derive(Debug, Clone, Serialize)]
struct SourceCaptureStatus {
    detected: Option<SourcePayload>,
    capturing: bool,
}

/// Map the built-in recognized sources to `RecognizedSourceInfo`, resolving each
/// source's effective auto-prompt against the given settings. Pure so it can be
/// unit-tested without a backend.
fn recognized_source_infos(settings: &AppSettings) -> Vec<RecognizedSourceInfo> {
    source_definitions(settings)
        .into_iter()
        .map(|source| RecognizedSourceInfo {
            auto_prompt: resolve_auto_prompt(settings, &source.id),
            id: source.id,
            label: source.label,
            description: source.description,
            title_patterns: source.title_patterns,
            custom: source.custom,
        })
        .collect()
}

#[derive(Clone)]
struct TrayMenuItems {
    recording: CheckMenuItem<Wry>,
    transcription: CheckMenuItem<Wry>,
    subtitles: CheckMenuItem<Wry>,
    auto_type: CheckMenuItem<Wry>,
    active_model: MenuItem<Wry>,
    active_mic: MenuItem<Wry>,
    threshold: MenuItem<Wry>,
    pause_all: CheckMenuItem<Wry>,
    cancel: MenuItem<Wry>,
}

#[derive(Debug, Clone, Serialize)]
struct OpenRouterKeyStatus {
    configured: bool,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiKeyStatus {
    configured: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SonioxKeyStatus {
    configured: bool,
}

#[tauri::command]
fn get_settings(state: State<'_, BackendState>) -> Result<AppSettings, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(backend.settings())
}

#[tauri::command]
fn get_live_webhook_stats(service: State<'_, Arc<WebhookService>>) -> WebhookStats {
    service.stats()
}

#[tauri::command]
async fn list_webhook_history(
    service: State<'_, Arc<WebhookService>>,
    page: u32,
    filter: HistoryFilter,
) -> Result<HistoryPage, String> {
    service.list_history(page, filter).await
}

#[tauri::command]
async fn get_webhook_history_detail(
    service: State<'_, Arc<WebhookService>>,
    event_id: String,
) -> Result<Option<HistoryDetail>, String> {
    service.history_detail(event_id).await
}

#[tauri::command]
async fn retry_webhook_history(
    service: State<'_, Arc<WebhookService>>,
    event_ids: Vec<String>,
) -> Result<RetryResult, String> {
    service.retry_history(event_ids).await
}

#[tauri::command]
fn dictionary_file_status(
    state: State<'_, DictionaryFileState>,
) -> Result<DictionaryFileStatus, String> {
    state
        .lock()
        .map(|store| store.status())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_dictionary_file(state: State<'_, DictionaryFileState>) -> Result<(), String> {
    state
        .lock()
        .map_err(|error| error.to_string())?
        .open_in_default_editor()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn reload_dictionary_file(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    dictionary_state: State<'_, DictionaryFileState>,
) -> Result<DictionaryFileStatus, String> {
    let current = backend_state
        .lock()
        .map_err(|error| error.to_string())?
        .settings()
        .dictionary;
    let (poll, status) = {
        let mut store = dictionary_state.lock().map_err(|error| error.to_string())?;
        let poll = store.reload(&current);
        let status = store.status();
        (poll, status)
    };
    let outcome = {
        let mut backend = backend_state.lock().map_err(|error| error.to_string())?;
        apply_dictionary_poll(&mut backend, poll)
    };
    emit_dictionary_outcome(&app, &outcome, &status);
    Ok(status)
}

#[tauri::command]
fn load_list_visibility(state: State<'_, BackendState>) -> Result<ListVisibilityState, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend
        .load_list_visibility()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_list_visibility(
    state: State<'_, BackendState>,
    request: SetListVisibilityRequest,
) -> Result<ListVisibilityState, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend
        .set_list_visibility(request)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn openrouter_key_status(state: State<'_, BackendState>) -> Result<OpenRouterKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(OpenRouterKeyStatus {
        configured: backend.openrouter_api_key_configured()?,
    })
}

#[tauri::command]
fn save_openrouter_api_key(
    state: State<'_, BackendState>,
    api_key: String,
) -> Result<OpenRouterKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.save_openrouter_api_key(&api_key)?;
    Ok(OpenRouterKeyStatus { configured: true })
}

#[tauri::command]
fn delete_openrouter_api_key(
    state: State<'_, BackendState>,
) -> Result<OpenRouterKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.delete_openrouter_api_key()?;
    Ok(OpenRouterKeyStatus { configured: false })
}

#[tauri::command]
fn openai_key_status(state: State<'_, BackendState>) -> Result<OpenAiKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(OpenAiKeyStatus {
        configured: backend.openai_api_key_configured()?,
    })
}

#[tauri::command]
fn save_openai_api_key(
    state: State<'_, BackendState>,
    api_key: String,
) -> Result<OpenAiKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.save_openai_api_key(&api_key)?;
    Ok(OpenAiKeyStatus { configured: true })
}

#[tauri::command]
fn delete_openai_api_key(state: State<'_, BackendState>) -> Result<OpenAiKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.delete_openai_api_key()?;
    Ok(OpenAiKeyStatus { configured: false })
}

#[tauri::command]
fn soniox_key_status(state: State<'_, BackendState>) -> Result<SonioxKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(SonioxKeyStatus {
        configured: backend.soniox_api_key_configured()?,
    })
}

#[tauri::command]
fn save_soniox_api_key(
    state: State<'_, BackendState>,
    api_key: String,
) -> Result<SonioxKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.save_soniox_api_key(&api_key)?;
    Ok(SonioxKeyStatus { configured: true })
}

#[tauri::command]
fn delete_soniox_api_key(state: State<'_, BackendState>) -> Result<SonioxKeyStatus, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.delete_soniox_api_key()?;
    Ok(SonioxKeyStatus { configured: false })
}

#[tauri::command]
fn overlay_caption_snapshot(state: State<'_, OverlayCaptionState>) -> OverlayCaptionSnapshot {
    state
        .lock()
        .map(|runtime| runtime.snapshot())
        .unwrap_or_else(|_| OverlayCaptionRuntime::default().snapshot())
}

#[tauri::command]
fn preview_subtitle(
    app: AppHandle,
    state: State<'_, OverlayCaptionState>,
    backend_state: State<'_, BackendState>,
    preview_state: State<'_, SubtitlePreviewState>,
    patch: SettingsPatch,
) -> Result<OverlayCaptionSnapshot, String> {
    let mut settings = backend_state
        .lock()
        .map_err(|error| error.to_string())?
        .settings();
    settings.apply_patch(patch);
    let position = match settings.floating_overlay_position {
        FloatingOverlayPosition::Off => FloatingOverlayPosition::Top,
        position => position,
    };
    let style = settings.floating_overlay_caption_style();
    let preview_raw_text = "Subtitle preview · 자막 미리보기";
    let preview_hold = adaptive_caption_hold(preview_raw_text, style.duration_seconds);
    let preview_token = begin_subtitle_preview(preview_state.inner());
    let preview = OverlayCaptionSnapshot {
        generation: preview_token,
        visible: true,
        source: OverlayCaptionSource::Preview,
        phase: wakenote::overlay_caption::OverlayCaptionPhase::Partial,
        chunk_id: None,
        audio_path: None,
        text: preview_raw_text.to_string(),
        position,
        final_hold_ms: Some(preview_hold.as_millis() as u64),
        style,
    };
    publish_overlay_caption_snapshot(&app, preview.clone(), "subtitle preview");

    let app_for_restore = app.clone();
    let state_for_restore = state.inner().clone();
    let preview_state_for_restore = preview_state.inner().clone();
    thread::spawn(move || {
        thread::sleep(preview_hold);
        if !finish_subtitle_preview(&preview_state_for_restore, preview_token) {
            return;
        }
        let snapshot = state_for_restore
            .lock()
            .ok()
            .map(|runtime| runtime.snapshot());
        if let Some(snapshot) = snapshot {
            publish_overlay_caption_snapshot(
                &app_for_restore,
                snapshot,
                "subtitle preview elapsed",
            );
        }
    });

    Ok(preview)
}

#[tauri::command]
fn debug_show_overlay_caption(
    app: AppHandle,
    state: State<'_, OverlayCaptionState>,
    backend_state: State<'_, BackendState>,
    text: String,
) -> Result<OverlayCaptionSnapshot, String> {
    let style = backend_state
        .lock()
        .map_err(|error| error.to_string())?
        .settings()
        .floating_overlay_caption_style();
    let snapshot = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        runtime.show_partial(0, text, FloatingOverlayPosition::Top, style);
        runtime.snapshot()
    };
    publish_overlay_caption_snapshot(&app, snapshot.clone(), "debug show caption");
    Ok(snapshot)
}

#[tauri::command]
fn debug_hide_overlay_caption(
    app: AppHandle,
    state: State<'_, OverlayCaptionState>,
) -> Result<OverlayCaptionSnapshot, String> {
    let snapshot = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        runtime.hide();
        runtime.snapshot()
    };
    publish_overlay_caption_snapshot(&app, snapshot.clone(), "debug hide caption");
    Ok(snapshot)
}

fn maybe_show_debug_overlay_caption_from_env(app: &AppHandle) {
    let Ok(text) = std::env::var("WAKENOTE_DEBUG_OVERLAY_CAPTION") else {
        return;
    };
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<OverlayCaptionState>() else {
        return;
    };
    let style = app
        .try_state::<BackendState>()
        .and_then(|state| {
            state
                .lock()
                .ok()
                .map(|backend| backend.settings().floating_overlay_caption_style())
        })
        .unwrap_or_else(|| AppSettings::default().floating_overlay_caption_style());
    let snapshot = state.lock().ok().map(|mut runtime| {
        runtime.show_partial(0, text, FloatingOverlayPosition::Top, style);
        runtime.snapshot()
    });
    if let Some(snapshot) = snapshot {
        publish_overlay_caption_snapshot(app, snapshot, "debug env caption");
    }
}

fn emit_dictation_state(app: &AppHandle, payload: DictationStatePayload) {
    let _ = app.emit(DICTATION_STATE_EVENT, payload);
}

fn emit_permission_guidance(app: &AppHandle, payload: PermissionGuidancePayload) {
    if let Err(error) = app.emit(PERMISSION_GUIDANCE_EVENT, payload) {
        eprintln!("[permissions] could not emit guidance: {error}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DictationCue {
    Start,
    Stop,
    End,
}

fn dictation_cue_resource_name(cue: DictationCue, sound: DictationCueSound) -> &'static str {
    match (cue, sound) {
        (DictationCue::Start, DictationCueSound::Original) => "dictation-start.wav",
        (DictationCue::Stop, DictationCueSound::Original) => "dictation-stop.wav",
        (DictationCue::End, DictationCueSound::Original) => "dictation-stop.wav",
        (DictationCue::Start, DictationCueSound::Alternative) => "dict-start-0.mp3",
        (DictationCue::Stop, DictationCueSound::Alternative) => "dict-start-0.mp3",
        (DictationCue::End, DictationCueSound::Alternative) => "dict-end-0.mp3",
    }
}

fn dictation_cue_sound(cue: DictationCue, settings: &AppSettings) -> DictationCueSound {
    match cue {
        DictationCue::Start => settings.dictation_start_sound,
        DictationCue::Stop => settings.dictation_stop_sound,
        DictationCue::End => settings.dictation_end_sound,
    }
}

fn dictation_cue_volume_arg(volume: DictationCueVolume) -> Option<&'static str> {
    match volume {
        DictationCueVolume::Muted => None,
        DictationCueVolume::Small => Some("0.55"),
        DictationCueVolume::Medium => Some("1.0"),
        DictationCueVolume::Large => Some("1.45"),
    }
}

fn play_dictation_cue(
    app: &AppHandle,
    cue: DictationCue,
    settings: &AppSettings,
) -> Result<(), String> {
    let Some(volume) = dictation_cue_volume_arg(settings.dictation_cue_volume) else {
        return Ok(());
    };
    let sound = dictation_cue_sound(cue, settings);
    let path = app
        .path()
        .resolve(
            dictation_cue_resource_name(cue, sound),
            BaseDirectory::Resource,
        )
        .map_err(|error| error.to_string())?;
    let status = Command::new("/usr/bin/afplay")
        .args(["-v", volume])
        .arg(path)
        .status()
        .map_err(|error| error.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("afplay exited with {status}"))
}

fn log_dictation_runtime(app: &AppHandle, message: impl AsRef<str>) {
    let message = message.as_ref();
    if let Some(settings) = app
        .try_state::<BackendState>()
        .and_then(|state| state.lock().ok().map(|backend| backend.settings()))
    {
        append_runtime_debug_log(&settings, message);
    }
}

fn spawn_dictation_cue_task(task: impl FnOnce() + Send + 'static) -> Result<(), String> {
    thread::Builder::new()
        .name("dictation-audio-cue".to_string())
        .spawn(task)
        .map(|_| ())
        .map_err(|error| format!("could not start dictation cue: {error}"))
}

fn play_dictation_cue_nonblocking_on_failure(
    app: &AppHandle,
    cue: DictationCue,
    settings: &AppSettings,
) {
    let app = app.clone();
    let settings = settings.clone();
    if let Err(error) = spawn_dictation_cue_task(move || {
        let cue_name = match cue {
            DictationCue::Start => "start",
            DictationCue::Stop => "stop",
            DictationCue::End => "end",
        };
        match play_dictation_cue(&app, cue, &settings) {
            Ok(()) => log_dictation_runtime(&app, format!("[dictation] cue={cue_name} completed")),
            Err(error) => {
                let message = format!("[dictation] cue={cue_name} failed error={error}");
                eprintln!("{message}");
                log_dictation_runtime(&app, message);
            }
        }
    }) {
        eprintln!("[dictation] cue worker failed: {error}");
    }
}

fn dispatch_dictation_shortcut_event(
    app: &AppHandle,
    event: DictationShortcutEvent,
    mode: DictationHotkeyMode,
) {
    if app
        .state::<DictationShortcutSuspension>()
        .0
        .load(Ordering::SeqCst)
    {
        return;
    }
    if let Some(dispatcher) = app.try_state::<DictationShortcutDispatcher>()
        && let Err(error) = dispatcher.send((event, mode))
    {
        eprintln!("[dictation] shortcut dispatcher unavailable: {error}");
    }
}

fn spawn_dictation_shortcut_worker(app: AppHandle) -> Result<DictationShortcutDispatcher, String> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("dictation-shortcut-events".to_string())
        .spawn(move || {
            while let Ok((event, mode)) = receiver.recv() {
                if let Err(error) = handle_dictation_shortcut_event(&app, event, mode) {
                    finish_dictation(&app, Some(error));
                }
            }
        })
        .map_err(|error| format!("could not start the dictation shortcut worker: {error}"))?;
    Ok(sender)
}

enum ParsedDictationShortcut {
    Keyed(Shortcut),
    Modifiers(ModifierShortcut),
}

fn parse_dictation_shortcut(raw: &str) -> Result<(String, ParsedDictationShortcut), String> {
    let normalized = validate_dictation_shortcut(raw)?;
    if let Some(shortcut) = modifier_shortcut(&normalized)? {
        return Ok((normalized, ParsedDictationShortcut::Modifiers(shortcut)));
    }
    let shortcut = normalized
        .parse::<Shortcut>()
        .map_err(|error| format!("invalid dictation shortcut: {error}"))?;
    Ok((normalized, ParsedDictationShortcut::Keyed(shortcut)))
}

fn register_dictation_shortcut_for_mode(
    app: &AppHandle,
    raw: &str,
    mode: DictationHotkeyMode,
) -> Result<(), String> {
    let (normalized, shortcut) = parse_dictation_shortcut(raw)?;
    let shortcut = match shortcut {
        ParsedDictationShortcut::Keyed(shortcut) => shortcut,
        ParsedDictationShortcut::Modifiers(shortcut) => {
            if mode == DictationHotkeyMode::Enhanced {
                return Err("Enhanced Prompt requires a shortcut with a main key".into());
            }
            return app
                .state::<ModifierShortcutState>()
                .lock()
                .map_err(|error| error.to_string())?
                .register(shortcut, mode)
                .map_err(|error| {
                    format!("could not register dictation shortcut '{normalized}': {error}")
                });
        }
    };
    if app.global_shortcut().is_registered(shortcut) {
        return Err(format!(
            "dictation shortcut '{normalized}' is already registered"
        ));
    }
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, registered, event| {
            if registered != &shortcut {
                return;
            }
            let event = match event.state {
                ShortcutState::Pressed => DictationShortcutEvent::Pressed,
                ShortcutState::Released => DictationShortcutEvent::Released,
            };
            dispatch_dictation_shortcut_event(app, event, mode);
        })
        .map_err(|error| format!("could not register dictation shortcut '{normalized}': {error}"))
}

fn unregister_dictation_shortcut(app: &AppHandle, raw: &str) -> Result<(), String> {
    let (normalized, shortcut) = parse_dictation_shortcut(raw)?;
    let shortcut = match shortcut {
        ParsedDictationShortcut::Keyed(shortcut) => shortcut,
        ParsedDictationShortcut::Modifiers(shortcut) => {
            app.state::<ModifierShortcutState>()
                .lock()
                .map_err(|error| error.to_string())?
                .unregister(shortcut);
            return Ok(());
        }
    };
    if !app.global_shortcut().is_registered(shortcut) {
        return Ok(());
    }
    app.global_shortcut()
        .unregister(shortcut)
        .map_err(|error| format!("could not unregister dictation shortcut '{normalized}': {error}"))
}

#[tauri::command]
fn suspend_dictation_shortcut(
    app: AppHandle,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    let settings = state.lock().map_err(|error| error.to_string())?.settings();
    app.state::<DictationShortcutSuspension>()
        .0
        .store(true, Ordering::SeqCst);
    cancel_dictation_runtime(&app)?;
    microphone_shortcut::register(&app, "")?;
    autotype_shortcut::register(&app, "")?;
    for (shortcut, _) in active_dictation_shortcuts(&settings) {
        unregister_dictation_shortcut(&app, &shortcut)?;
    }
    app.state::<DictationState>()
        .lock()
        .map_err(|error| error.to_string())?
        .reset_hotkeys();
    Ok(())
}

#[tauri::command]
fn resume_dictation_shortcut(app: AppHandle, state: State<'_, BackendState>) -> Result<(), String> {
    let settings = state.lock().map_err(|error| error.to_string())?.settings();
    microphone_shortcut::register(&app, &settings.cycle_microphone_shortcut)?;
    autotype_shortcut::register(&app, &settings.auto_type_toggle_shortcut)?;
    for (shortcut, mode) in active_dictation_shortcuts(&settings) {
        if !is_dictation_shortcut_registered(&app, &shortcut)? {
            register_dictation_shortcut_for_mode(&app, &shortcut, mode)?;
        }
    }
    app.state::<DictationShortcutSuspension>()
        .0
        .store(false, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn enhanced_dictation_event(app: AppHandle, pressed: bool) -> Result<(), String> {
    let settings = app
        .state::<BackendState>()
        .lock()
        .map_err(|error| error.to_string())?
        .settings();
    if !settings.dictation_enabled || !settings.enhance_prompt_enabled {
        return Err("Enable Dictation and Enhanced Prompt in Settings first".into());
    }
    app.state::<DictationShortcutDispatcher>()
        .send((
            if pressed {
                DictationShortcutEvent::Pressed
            } else {
                DictationShortcutEvent::Released
            },
            DictationHotkeyMode::Enhanced,
        ))
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn current_pressed_modifier_shortcut() -> Option<String> {
    let flags = CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState);
    ModifierShortcut::new(
        flags.contains(CGEventFlags::MaskControl),
        flags.contains(CGEventFlags::MaskAlternate),
        flags.contains(CGEventFlags::MaskShift),
        flags.contains(CGEventFlags::MaskCommand),
    )
    .canonical_string()
}

#[cfg(not(target_os = "macos"))]
fn current_pressed_modifier_shortcut() -> Option<String> {
    None
}

#[tauri::command]
fn pressed_modifier_shortcut() -> Option<String> {
    current_pressed_modifier_shortcut()
}

fn is_dictation_shortcut_registered(app: &AppHandle, raw: &str) -> Result<bool, String> {
    let (_, shortcut) = parse_dictation_shortcut(raw)?;
    match shortcut {
        ParsedDictationShortcut::Keyed(shortcut) => {
            Ok(app.global_shortcut().is_registered(shortcut))
        }
        ParsedDictationShortcut::Modifiers(shortcut) => app
            .state::<ModifierShortcutState>()
            .lock()
            .map_err(|error| error.to_string())
            .map(|runtime| runtime.is_registered(shortcut)),
    }
}

#[cfg(target_os = "macos")]
fn handle_modifier_shortcut_event(
    app: &AppHandle,
    runtime: &ModifierShortcutState,
    flags: CGEventFlags,
) {
    let logical = ModifierShortcut::new(
        flags.contains(CGEventFlags::MaskControl),
        flags.contains(CGEventFlags::MaskAlternate),
        flags.contains(CGEventFlags::MaskShift),
        flags.contains(CGEventFlags::MaskCommand),
    );
    let physical = [
        (PhysicalModifierKey::LeftControl, 59_u16),
        (PhysicalModifierKey::RightControl, 62_u16),
        (PhysicalModifierKey::LeftAlt, 58_u16),
        (PhysicalModifierKey::RightAlt, 61_u16),
        (PhysicalModifierKey::LeftShift, 56_u16),
        (PhysicalModifierKey::RightShift, 60_u16),
        (PhysicalModifierKey::LeftCommand, 55_u16),
        (PhysicalModifierKey::RightCommand, 54_u16),
    ]
    .into_iter()
    .filter_map(|(key, key_code)| {
        CGEventSource::key_state(CGEventSourceStateID::CombinedSessionState, key_code)
            .then_some(key)
    })
    .collect::<Vec<_>>();
    let pressed = match physical.as_slice() {
        [key] => ModifierShortcut::Physical(*key),
        _ => logical,
    };
    let events = runtime
        .lock()
        .map(|mut runtime| runtime.handle_modifiers(pressed))
        .unwrap_or_default();
    for (event, mode) in events {
        if let Some(settings) = app
            .try_state::<BackendState>()
            .and_then(|state| state.lock().ok().map(|backend| backend.settings()))
        {
            append_runtime_debug_log(
                &settings,
                format!("[dictation] modifier shortcut event={event:?}"),
            );
        }
        dispatch_dictation_shortcut_event(app, event, mode);
    }
}

#[cfg(target_os = "macos")]
fn install_modifier_shortcut_monitors(
    app: &AppHandle,
    runtime: ModifierShortcutState,
) -> Result<(), String> {
    let app = app.clone();
    thread::Builder::new()
        .name("dictation-modifier-shortcut".to_string())
        .spawn(move || {
            loop {
                let flags = CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState);
                handle_modifier_shortcut_event(&app, &runtime, flags);
                thread::sleep(Duration::from_millis(15));
            }
        })
        .map(|_| ())
        .map_err(|error| format!("could not start the modifier shortcut monitor: {error}"))
}

#[cfg(not(target_os = "macos"))]
fn install_modifier_shortcut_monitors(
    _app: &AppHandle,
    _runtime: ModifierShortcutState,
) -> Result<(), String> {
    Ok(())
}

fn reconcile_dictation_shortcut_registration(
    app: &AppHandle,
    previous: &AppSettings,
    next: &AppSettings,
) -> Result<(), String> {
    let old = active_dictation_shortcuts(previous);
    let new = active_dictation_shortcuts(next);
    if old == new {
        return Ok(());
    }
    app.state::<DictationState>()
        .lock()
        .map_err(|error| error.to_string())?
        .reset_hotkeys();
    let mut installed: Vec<String> = Vec::new();
    let result = (|| {
        for (shortcut, _) in &old {
            unregister_dictation_shortcut(app, shortcut)?;
        }
        for (shortcut, mode) in &new {
            register_dictation_shortcut_for_mode(app, shortcut, *mode)?;
            installed.push(shortcut.clone());
        }
        Ok(())
    })();
    if let Err(error) = result {
        for shortcut in installed {
            let _ = unregister_dictation_shortcut(app, &shortcut);
        }
        let mut failures = Vec::new();
        for (shortcut, mode) in old {
            if !is_dictation_shortcut_registered(app, &shortcut).unwrap_or(false)
                && let Err(restore_error) =
                    register_dictation_shortcut_for_mode(app, &shortcut, mode)
            {
                failures.push(restore_error);
            }
        }
        return Err(if failures.is_empty() {
            error
        } else {
            format!("{error}; shortcut restore failed: {}", failures.join("; "))
        });
    }
    Ok(())
}

fn show_dictation_overlay(
    app: &AppHandle,
    state: overlay::DictationOverlayState,
    message: Option<String>,
) {
    let (position, style) = app
        .try_state::<BackendState>()
        .and_then(|state| {
            state.lock().ok().map(|backend| {
                let settings = backend.settings();
                (
                    settings.dictation_bubble_position,
                    settings.dictation_overlay_style(),
                )
            })
        })
        .unwrap_or_default();
    let app_for_task = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if let Err(error) =
            overlay::show_dictation_overlay(&app_for_task, state, message, position, style)
        {
            eprintln!("[dictation] overlay update failed: {error}");
        }
    }) {
        eprintln!("[dictation] overlay scheduling failed: {error}");
    }
}

fn dictation_operation_is_active(app: &AppHandle, operation_id: u64) -> bool {
    app.try_state::<DictationState>()
        .and_then(|state| {
            state
                .lock()
                .ok()
                .map(|runtime| runtime.is_operation_active(operation_id))
        })
        .unwrap_or(false)
}

fn dictation_suppresses_vor_live_transcription(app: &AppHandle) -> bool {
    app.try_state::<DictationState>()
        .and_then(|state| state.lock().ok().map(|runtime| runtime.stage()))
        .is_some_and(|stage| {
            matches!(
                stage,
                DictationStage::Recording | DictationStage::Transcribing
            )
        })
}

fn set_vor_live_transcription_suspended_for_dictation(app: &AppHandle, suspended: bool) {
    let source_keys = app
        .try_state::<BackendState>()
        .and_then(|state| {
            state.lock().ok().map(|mut backend| {
                backend.set_live_transcription_suspended_for_dictation(suspended)
            })
        })
        .unwrap_or_default();
    if !suspended {
        return;
    }
    for source_key in source_keys {
        if let Some(manager) = app.try_state::<OpenAiRealtimeManager>() {
            manager.close_source(source_key.clone());
        }
        if let Some(manager) = app.try_state::<SonioxRealtimeManager>() {
            manager.close_source(source_key);
        }
    }
}

fn type_dictation_text_on_main_thread(
    app: &AppHandle,
    operation_id: u64,
    text: String,
    settings: AppSettings,
    started: Instant,
) {
    let app_for_task = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if !dictation_operation_is_active(&app_for_task, operation_id) {
            return;
        }
        let clipboard_after_input = wakenote::text_input::dictation_clipboard_after_input(
            settings.dictation_copy_to_clipboard,
        );
        if let Some(payload) = dictation_insertion_permission_guidance(
            permissions::permission_snapshot().accessibility.status,
        ) {
            let error = match wakenote::text_input::copy_dictation_text_to_clipboard(&text) {
                Ok(()) => wakenote::text_input::accessibility_permission_error().to_string(),
                Err(error) => error,
            };
            emit_permission_guidance(&app_for_task, payload);
            show_dictation_operation_error(&app_for_task, operation_id, error);
            return;
        }
        if let Err(error) =
            wakenote::text_input::type_dictation_text_into_focused_cursor_with_clipboard(
                &text,
                clipboard_after_input,
            )
        {
            show_dictation_operation_error(&app_for_task, operation_id, error);
            return;
        }
        log_dictation_runtime(
            &app_for_task,
            format!(
                "[dictation] text_input=completed total_ms={}",
                started.elapsed().as_millis()
            ),
        );
        play_dictation_cue_nonblocking_on_failure(&app_for_task, DictationCue::End, &settings);
        complete_dictation_operation(&app_for_task, operation_id);
    }) {
        show_dictation_operation_error(
            app,
            operation_id,
            format!("Could not schedule Dictation text input: {error}"),
        );
    }
}

fn refresh_tray_from_backend(app: &AppHandle) {
    let snapshot = app.try_state::<BackendState>().and_then(|state| {
        state
            .lock()
            .ok()
            .map(|backend| (backend.settings(), backend.app_status()))
    });
    if let Some((settings, status)) = snapshot {
        update_tray_presentation(app, &settings, &status);
    }
}

fn restore_overlay_after_dictation(app: &AppHandle) {
    let _ = overlay::hide_dictation_overlay_state(app);
    let caption = app
        .try_state::<OverlayCaptionState>()
        .and_then(|state| state.lock().ok().map(|runtime| runtime.snapshot()));
    if caption.as_ref().is_some_and(|snapshot| {
        snapshot.visible
            && !snapshot.text.is_empty()
            && !matches!(snapshot.position, FloatingOverlayPosition::Off)
    }) {
        publish_overlay_caption_snapshot(
            app,
            caption.expect("visible caption snapshot"),
            "dictation finish",
        );
        return;
    }

    let restored = app.try_state::<BackendState>().and_then(|state| {
        state.lock().ok().map(|backend| {
            let settings = backend.settings();
            let status = backend.app_status();
            (settings, status)
        })
    });
    if let Some((settings, status)) = restored
        && matches!(status.tray_state, TrayState::Recording)
    {
        if let Err(error) = overlay::show_overlay_on_main_thread(
            app,
            overlay::OverlayState::Recording,
            settings.effective_floating_overlay_position(),
            "dictation restore",
        ) {
            eprintln!("[dictation] overlay restore failed: {error}");
        }
        return;
    }

    if let Some(caption) = caption {
        publish_overlay_caption_snapshot(app, caption, "dictation finish");
    } else if let Err(error) = overlay::hide_overlay_on_main_thread(app, "dictation finish") {
        eprintln!("[dictation] overlay hide failed: {error}");
    }
}

fn complete_dictation(app: &AppHandle) {
    let payload = app
        .try_state::<DictationState>()
        .and_then(|state| {
            state.lock().ok().map(|mut runtime| {
                runtime.finish();
                runtime.payload(None)
            })
        })
        .unwrap_or(DictationStatePayload {
            state: DictationStage::Idle,
            error: None,
        });
    set_vor_live_transcription_suspended_for_dictation(app, false);
    emit_dictation_state(app, payload);
    refresh_tray_from_backend(app);
    restore_overlay_after_dictation(app);
}

fn complete_dictation_operation(app: &AppHandle, operation_id: u64) -> bool {
    let payload = app.try_state::<DictationState>().and_then(|state| {
        state.lock().ok().and_then(|mut runtime| {
            runtime
                .finish_if_active(operation_id)
                .then(|| runtime.payload(None))
        })
    });
    let Some(payload) = payload else {
        return false;
    };
    set_vor_live_transcription_suspended_for_dictation(app, false);
    emit_dictation_state(app, payload);
    refresh_tray_from_backend(app);
    restore_overlay_after_dictation(app);
    true
}

fn show_dictation_error(app: &AppHandle, error: String) {
    eprintln!("[dictation] {error}");
    log_dictation_runtime(app, format!("[dictation] error={error}"));
    let payload = app
        .try_state::<DictationState>()
        .and_then(|state| {
            state.lock().ok().map(|mut runtime| {
                runtime.fail();
                runtime.payload(Some(error.clone()))
            })
        })
        .unwrap_or(DictationStatePayload {
            state: DictationStage::Error,
            error: Some(error.clone()),
        });
    set_vor_live_transcription_suspended_for_dictation(app, false);
    hide_dictation_caption(app);
    emit_dictation_state(app, payload);
    let message = if error == "No speech detected" {
        error
    } else {
        "Dictation failed".to_string()
    };
    show_dictation_overlay(app, overlay::DictationOverlayState::Error, Some(message));
    refresh_tray_from_backend(app);

    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1_500));
        let reset = app.try_state::<DictationState>().and_then(|state| {
            state
                .lock()
                .ok()
                .and_then(|mut runtime| runtime.reset_error().then(|| runtime.payload(None)))
        });
        if let Some(payload) = reset {
            emit_dictation_state(&app, payload);
            refresh_tray_from_backend(&app);
            restore_overlay_after_dictation(&app);
        }
    });
}

fn show_dictation_operation_error(app: &AppHandle, operation_id: u64, error: String) -> bool {
    let transitioned = app.try_state::<DictationState>().and_then(|state| {
        state.lock().ok().and_then(|mut runtime| {
            runtime
                .fail_if_active(operation_id)
                .then(|| runtime.payload(Some(error.clone())))
        })
    });
    let Some(payload) = transitioned else {
        return false;
    };

    eprintln!("[dictation] {error}");
    log_dictation_runtime(app, format!("[dictation] error={error}"));
    set_vor_live_transcription_suspended_for_dictation(app, false);
    hide_dictation_caption(app);
    emit_dictation_state(app, payload);
    let message = if error == "No speech detected" {
        error
    } else {
        "Dictation failed".to_string()
    };
    show_dictation_overlay(app, overlay::DictationOverlayState::Error, Some(message));
    refresh_tray_from_backend(app);

    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1_500));
        let reset = app.try_state::<DictationState>().and_then(|state| {
            state
                .lock()
                .ok()
                .and_then(|mut runtime| runtime.reset_error().then(|| runtime.payload(None)))
        });
        if let Some(payload) = reset {
            emit_dictation_state(&app, payload);
            refresh_tray_from_backend(&app);
            restore_overlay_after_dictation(&app);
        }
    });
    true
}

fn cancel_dictation_runtime(app: &AppHandle) -> Result<bool, String> {
    let Some(state) = app.try_state::<DictationState>() else {
        return Ok(false);
    };
    let payload = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        runtime.cancel_active().then(|| runtime.payload(None))
    };
    let Some(payload) = payload else {
        return Ok(false);
    };

    if let Some(manager) = app.try_state::<OpenAiRealtimeManager>() {
        manager.close_source("dictation".to_string());
    }
    if let Some(manager) = app.try_state::<SonioxRealtimeManager>() {
        manager.close_source("dictation".to_string());
    }
    log_dictation_runtime(app, "[dictation] cancelled by user");
    set_vor_live_transcription_suspended_for_dictation(app, false);
    hide_dictation_caption(app);
    emit_dictation_state(app, payload);
    refresh_tray_from_backend(app);
    restore_overlay_after_dictation(app);
    Ok(true)
}

fn finish_dictation(app: &AppHandle, error: Option<String>) {
    match error {
        Some(error) => show_dictation_error(app, error),
        None => complete_dictation(app),
    }
}

fn transform_dictation_output(
    app: &AppHandle,
    operation_id: u64,
    settings: &AppSettings,
    text: &str,
) -> Result<String, String> {
    let Some(request) = wakenote::text_transform::dictation_transform_request(settings, text)
    else {
        return Ok(text.into());
    };
    if !dictation_operation_is_active(app, operation_id) {
        return Err("Dictation cancelled".into());
    }
    let key = app
        .state::<BackendState>()
        .lock()
        .map_err(|error| error.to_string())?
        .load_openrouter_api_key()?
        .unwrap_or_default();
    show_dictation_overlay(
        app,
        overlay::DictationOverlayState::Transcribing,
        Some(
            if settings.dictation_enhance_prompt {
                "Enhancing prompt…"
            } else {
                "Translating…"
            }
            .into(),
        ),
    );
    let cancellation = tokio_util::sync::CancellationToken::new();
    tauri::async_runtime::block_on(async {
        let client = wakenote::llm::ReqwestOpenRouterClient::default();
        tokio::select! {
            result = wakenote::text_transform::transform_text_with_client(
                &client, settings, &key, &request, &cancellation,
            ) => result.map(|result| result.text),
            _ = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if !dictation_operation_is_active(app, operation_id) { break; }
                }
            } => { cancellation.cancel(); Err("Dictation cancelled".into()) }
        }
    })
}

fn process_dictation_recording(
    app: &AppHandle,
    operation_id: u64,
    settings: AppSettings,
    recording: DictationRecording,
) {
    let started = Instant::now();
    let dictation_model = settings.effective_dictation_model().to_string();
    let (credentials, models) = match app.try_state::<BackendState>() {
        Some(state) => match state.lock() {
            Ok(backend) => match backend.transcription_credentials() {
                Ok(credentials) => (credentials, backend.model_registry()),
                Err(error) => {
                    show_dictation_operation_error(app, operation_id, error);
                    return;
                }
            },
            Err(error) => {
                show_dictation_operation_error(app, operation_id, error.to_string());
                return;
            }
        },
        None => (TranscriptionCredentials::default(), Vec::new()),
    };
    let transcriber = match RuntimeTranscriber::for_dictation_with_credentials(
        &settings.model_directory,
        credentials.clone(),
    ) {
        Ok(transcriber) => transcriber,
        Err(error) => {
            show_dictation_operation_error(app, operation_id, error.to_string());
            return;
        }
    };
    let transcription_options =
        validate_model_options(&models, &dictation_model, &settings.transcription_options);
    let uses_realtime_manager = matches!(
        dictation_model.as_str(),
        "openai-gpt-live-transcribe" | "soniox-realtime-v5"
    );
    let partial_callback: Option<TranscriptionPartialCallback> =
        (settings.dictation_subtitles_enabled() && !uses_realtime_manager).then(|| {
            let callback_app = app.clone();
            let callback_settings = settings.clone();
            Arc::new(move |text| {
                publish_dictation_partial(&callback_app, operation_id, text, &callback_settings);
            }) as TranscriptionPartialCallback
        });
    let transcriber =
        transcriber.with_file_streaming(transcription_options.streaming_enabled, partial_callback);
    let fallback_model_id = if transcription_options.cost_limit_fallback_enabled {
        transcription_options.cost_limit_fallback_model_id.clone()
    } else {
        None
    };
    let fallback_supports_diarization = fallback_model_id
        .as_deref()
        .and_then(|model_id| models.iter().find(|model| model.id == model_id))
        .is_some_and(|model| model.capabilities.diarization);
    let diarization_required = models
        .iter()
        .find(|model| model.id == dictation_model)
        .is_some_and(|model| model.capabilities.diarization);
    let transcriber = FallbackTranscriber::configured(
        transcriber.clone(),
        transcriber,
        fallback_model_id,
        fallback_supports_diarization,
        diarization_required,
    );
    let dictionary = DictionaryContext::from_settings(&settings);
    let ((archive_result, archive_elapsed), result, transcription_elapsed) = if matches!(
        dictation_model.as_str(),
        "openai-gpt-live-transcribe" | "soniox-realtime-v5"
    ) {
        let archive_started = Instant::now();
        let archive_result =
            archive_dictation_recording(&recording, &settings, env!("CARGO_PKG_VERSION"));
        let archive_elapsed = archive_started.elapsed();
        let transcription_started = Instant::now();
        let result = match &archive_result {
            Ok(archive) => {
                let source_key = "dictation".to_string();
                let chunk_id = operation_id;
                let request = RealtimeSamplesRequest {
                    source_key: source_key.clone(),
                    source_label: "Dictation".to_string(),
                    microphone_slot: None,
                    chunk_id,
                    model_id: dictation_model.clone(),
                    language: settings.dictation_language,
                    language_hints: settings.transcription_language_hints.clone(),
                    dictionary: dictionary.clone(),
                    sample_rate: recording.sample_rate,
                    samples: Arc::new(recording.samples.clone()),
                    credentials: credentials.clone(),
                };
                let submitted = match dictation_model.as_str() {
                    "openai-gpt-live-transcribe" => {
                        app.try_state::<OpenAiRealtimeManager>().map(|manager| {
                            manager.submit_samples(request);
                            manager.commit(source_key, chunk_id, archive.audio_path.clone());
                        })
                    }
                    "soniox-realtime-v5" => {
                        app.try_state::<SonioxRealtimeManager>().map(|manager| {
                            manager.submit_samples(request);
                            manager.commit(source_key, chunk_id, archive.audio_path.clone());
                        })
                    }
                    _ => None,
                };
                if submitted.is_some() {
                    transcribe_dictation_recording_execution(
                        &recording,
                        &dictation_model,
                        settings.dictation_language,
                        &settings.transcription_language_hints,
                        settings.suppress_low_confidence_transcripts,
                        &dictionary,
                        transcriber,
                    )
                } else {
                    Err("Real-time transcription is unavailable".to_string())
                }
            }
            Err(error) => Err(format!("Could not save dictation: {error}")),
        };
        (
            (archive_result, archive_elapsed),
            result,
            transcription_started.elapsed(),
        )
    } else {
        thread::scope(|scope| {
            let archive_started = Instant::now();
            let recording_for_archive = &recording;
            let settings_for_archive = &settings;
            let archive_task = scope.spawn(move || {
                let result = archive_dictation_recording(
                    recording_for_archive,
                    settings_for_archive,
                    env!("CARGO_PKG_VERSION"),
                );
                (result, archive_started.elapsed())
            });
            let transcription_started = Instant::now();
            let result = transcribe_dictation_recording_execution(
                &recording,
                &dictation_model,
                settings.dictation_language,
                &settings.transcription_language_hints,
                settings.suppress_low_confidence_transcripts,
                &dictionary,
                transcriber,
            );
            let transcription_elapsed = transcription_started.elapsed();
            let archive = archive_task.join().unwrap_or_else(|_| {
                (
                    Err("Dictation archive worker panicked".to_string()),
                    archive_started.elapsed(),
                )
            });
            (archive, result, transcription_elapsed)
        })
    };
    let archive = match archive_result {
        Ok(chunk) => chunk,
        Err(error) => {
            show_dictation_operation_error(
                app,
                operation_id,
                format!("Could not save dictation: {error}"),
            );
            return;
        }
    };
    log_dictation_runtime(
        app,
        format!(
            "[dictation] archive=created elapsed_ms={} audio_path={}",
            archive_elapsed.as_millis(),
            archive.audio_path.display(),
        ),
    );

    if !dictation_operation_is_active(app, operation_id) {
        log_dictation_runtime(
            app,
            format!("[dictation] stale result ignored operation_id={operation_id}"),
        );
        return;
    }

    match result {
        Ok(Some(mut execution)) => {
            if let Some(usage) = execution.usage.as_mut() {
                if usage.audio_duration_ms == 0 {
                    usage.audio_duration_ms = (recording.samples.len() as u64)
                        .saturating_mul(1_000)
                        / u64::from(recording.sample_rate.max(1));
                }
                if usage.provider_cost_usd.is_none()
                    && let Some(provider) = usage.provider.as_deref()
                {
                    usage.provider_cost_usd = estimated_provider_cost_usd(
                        provider,
                        &execution.effective_model_id,
                        usage.audio_duration_ms,
                    );
                }
            }
            let cost_entry = execution.usage.as_ref().and_then(|usage| {
                usage
                    .provider
                    .as_ref()
                    .map(|provider| TranscriptionCostEntry {
                        source_id: format!("dictation:{}", recording.started_at.timestamp_micros()),
                        recorded_at: recording.started_at,
                        provider: provider.clone(),
                        model_id: execution.effective_model_id.clone(),
                        audio_duration_ms: usage.audio_duration_ms,
                        estimated_cost_usd: usage.provider_cost_usd,
                        request_count: 1,
                        unpriced_request_count: u64::from(usage.provider_cost_usd.is_none()),
                    })
            });
            if let Some(entry) = cost_entry {
                upsert_transcription_cost(app, entry);
            }
            let text = execution.text;
            if let Err(error) = TranscriptionSidecar::write_success(&archive, &text) {
                show_dictation_operation_error(
                    app,
                    operation_id,
                    format!("Could not save dictation transcript: {error}"),
                );
                return;
            }
            let output_text = match transform_dictation_output(app, operation_id, &settings, &text)
            {
                Ok(output) => output,
                Err(error) => {
                    refresh_transcript_day_index(&archive.audio_path);
                    if dictation_operation_is_active(app, operation_id) {
                        show_dictation_operation_error(
                            app,
                            operation_id,
                            format!("{error}. The original transcript is saved in Transcripts."),
                        );
                    }
                    return;
                }
            };
            if !dictation_operation_is_active(app, operation_id) {
                return;
            }
            let Some(input_text) = wakenote::text_input::dictation_input_text(
                &output_text,
                settings.dictation_remove_trailing_space,
            ) else {
                let _ = TranscriptionSidecar::write_error(&archive, "No speech detected");
                refresh_transcript_day_index(&archive.audio_path);
                show_dictation_operation_error(app, operation_id, "No speech detected".to_string());
                return;
            };
            refresh_transcript_day_index(&archive.audio_path);
            log_dictation_runtime(
                app,
                format!(
                    "[dictation] transcription=completed elapsed_ms={} total_ms={}",
                    transcription_elapsed.as_millis(),
                    started.elapsed().as_millis(),
                ),
            );
            publish_dictation_final(
                app,
                operation_id,
                archive.audio_path.clone(),
                &output_text,
                &settings,
            );
            type_dictation_text_on_main_thread(app, operation_id, input_text, settings, started);
        }
        Ok(None) => {
            let _ = TranscriptionSidecar::write_error(&archive, "No speech detected");
            refresh_transcript_day_index(&archive.audio_path);
            show_dictation_operation_error(app, operation_id, "No speech detected".to_string());
        }
        Err(error) => {
            let _ = TranscriptionSidecar::write_error(&archive, &error);
            refresh_transcript_day_index(&archive.audio_path);
            show_dictation_operation_error(app, operation_id, error);
        }
    }
}

fn spawn_dictation_processing_task(
    app: AppHandle,
    operation_id: u64,
    settings: AppSettings,
    recording: DictationRecording,
) -> Result<(), String> {
    thread::Builder::new()
        .name(format!("dictation-transcription-{operation_id}"))
        .spawn(move || process_dictation_recording(&app, operation_id, settings, recording))
        .map(|_| ())
        .map_err(|error| format!("Could not start Dictation transcription: {error}"))
}

fn refresh_transcript_day_index(recording_path: &Path) {
    if let Err(error) = refresh_transcript_day_index_for_recording_path(recording_path) {
        eprintln!(
            "[wakenote] transcript day index refresh failed path={} error={error}",
            recording_path.display()
        );
    }
}

fn dictation_model_to_preload(settings: &AppSettings) -> Option<String> {
    let model_id = settings.effective_dictation_model();
    (settings.dictation_enabled
        && model_supports_live_partials(&settings.model_directory, model_id))
    .then(|| model_id.to_string())
}

fn preload_dictation_model(app: &AppHandle, settings: &AppSettings) {
    let Some(model_id) = dictation_model_to_preload(settings) else {
        return;
    };
    let Some(transcriber_state) = app.try_state::<LiveTranscriberState>() else {
        return;
    };
    if let Ok(slot) = transcriber_state.lock()
        && let Some(service) = slot.as_ref()
    {
        service.preload(model_id);
    }
}

fn dictation_recording_permission_guidance(
    event: DictationShortcutEvent,
    stage: DictationStage,
    status: PermissionGrantStatus,
) -> Option<PermissionGuidancePayload> {
    (event == DictationShortcutEvent::Pressed
        && stage == DictationStage::Idle
        && status != PermissionGrantStatus::Granted)
        .then(PermissionGuidancePayload::dictation_recording)
}

fn dictation_insertion_permission_guidance(
    status: PermissionGrantStatus,
) -> Option<PermissionGuidancePayload> {
    (status != PermissionGrantStatus::Granted).then(PermissionGuidancePayload::dictation_insertion)
}

struct ResolvedDictationInput {
    input: DictationMicrophoneInput,
    config: AudioInputConfig,
}

struct DictationStartResolution {
    ready: Vec<ResolvedDictationInput>,
    failures: Vec<DictationSlotFailure>,
}

fn resolve_dictation_inputs_with<F>(
    settings: &AppSettings,
    mut resolver: F,
) -> DictationStartResolution
where
    F: FnMut(&CaptureMicrophoneEntry) -> Result<ResolvedCpalInputDevice, String>,
{
    let configured = if settings.capture_microphones.is_empty() {
        vec![CaptureMicrophoneEntry {
            id: settings.selected_microphone.clone(),
            label: settings.selected_microphone_label.clone(),
            core_audio_uid: None,
        }]
    } else {
        settings
            .capture_microphones
            .iter()
            .take(2)
            .cloned()
            .collect()
    };
    let mut resolution = DictationStartResolution {
        ready: Vec::new(),
        failures: Vec::new(),
    };
    for (index, configured_input) in configured.iter().enumerate() {
        let Some(slot) = MicrophoneSlot::from_index(index) else {
            continue;
        };
        match resolver(configured_input) {
            Ok(resolved) if resolved.used_fallback_device => {
                resolution.failures.push(DictationSlotFailure {
                    slot,
                    error: format!(
                        "{} is disconnected; dictation is waiting for the same device",
                        configured_input.label
                    ),
                });
            }
            Ok(resolved) => {
                let input = DictationMicrophoneInput {
                    slot,
                    device: CaptureMicrophoneEntry {
                        id: resolved.device_id.clone(),
                        label: resolved.device_name.clone(),
                        core_audio_uid: resolved.core_audio_uid.clone(),
                    },
                };
                resolution.ready.push(ResolvedDictationInput {
                    config: AudioInputConfig {
                        device_id: resolved.device_id,
                        sample_rate: Some(resolved.sample_rate),
                        label_hint: (!resolved.device_name.is_empty())
                            .then_some(resolved.device_name),
                        core_audio_uid: resolved.core_audio_uid,
                    },
                    input,
                });
            }
            Err(error) => resolution
                .failures
                .push(DictationSlotFailure { slot, error }),
        }
    }
    resolution
}

fn resolve_dictation_inputs(settings: &AppSettings) -> DictationStartResolution {
    resolve_dictation_inputs_with(settings, |input| {
        resolve_capture_device_with_timeout(
            input.id.clone(),
            (!input.label.is_empty()).then_some(input.label.clone()),
            input.core_audio_uid.clone(),
        )
        .map_err(|error| error.to_string())
    })
}

fn dictation_slot_label(slot: MicrophoneSlot) -> &'static str {
    match slot {
        MicrophoneSlot::Primary => "Primary",
        MicrophoneSlot::Secondary => "Secondary",
    }
}

fn dictation_start_error(failures: &[DictationSlotFailure]) -> String {
    if failures.is_empty() {
        return "No Dictation microphone is available".to_string();
    }
    failures
        .iter()
        .map(|failure| format!("{}: {}", dictation_slot_label(failure.slot), failure.error))
        .collect::<Vec<_>>()
        .join("; ")
}

fn dictation_listening_message(active_slots: &[MicrophoneSlot]) -> Result<String, String> {
    match active_slots {
        [MicrophoneSlot::Primary, MicrophoneSlot::Secondary] => Ok("Listening…".to_string()),
        [MicrophoneSlot::Primary] => Ok("Listening · Primary only".to_string()),
        [MicrophoneSlot::Secondary] => Ok("Listening · Secondary only".to_string()),
        _ => Err("Dictation did not start a microphone".to_string()),
    }
}

fn show_dictation_degraded(
    app: &AppHandle,
    active_slots: &[MicrophoneSlot],
    failures: &[DictationSlotFailure],
) {
    for failure in failures {
        log_dictation_runtime(
            app,
            format!(
                "[dictation] slot={} state=failed error={}",
                failure.slot.as_str(),
                failure.error,
            ),
        );
    }
    let Ok(message) = dictation_listening_message(active_slots) else {
        return;
    };
    show_dictation_overlay(
        app,
        overlay::DictationOverlayState::Recording,
        Some(message),
    );
}

fn start_dictation_capture(
    app: &AppHandle,
    state: &DictationState,
    settings: AppSettings,
) -> Result<u64, String> {
    let _update_activity = RESTART_GATE.activity()?;
    clear_auto_type(app);
    let resolution = resolve_dictation_inputs(&settings);
    if resolution.ready.is_empty() {
        return Err(dictation_start_error(&resolution.failures));
    }
    let operation_id = state
        .lock()
        .map_err(|error| error.to_string())?
        .begin_recording(
            settings.clone(),
            Utc::now(),
            resolution.ready.iter().map(|input| input.input.slot),
        )?;
    let mut failures = resolution.failures;
    {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        for failure in &failures {
            runtime.mark_slot_start_failed(operation_id, failure.slot, failure.error.clone());
        }
    }

    let callback_throttle = Arc::new(Mutex::new(Instant::now() - OVERLAY_LEVEL_EMIT_INTERVAL));
    let voice_leveling_policy = app
        .try_state::<VoiceLevelingPolicy>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "Voice-aware Auto Level policy is unavailable".to_string())?;
    for resolved in resolution.ready {
        let slot = resolved.input.slot;
        let configured_microphone = resolved.input.device.clone();
        let callback_app = app.clone();
        let callback_throttle = callback_throttle.clone();
        let emit_waveform = move |frame: &AudioFrame| {
            if !overlay_level_emit_due(&callback_throttle) {
                return;
            }
            let levels = overlay::waveform_levels_from_samples(
                &frame.samples,
                overlay::OVERLAY_WAVEFORM_BAR_COUNT,
            );
            overlay::emit_dictation_waveform_levels(&callback_app, levels);
        };
        // A second slot on an already-open device must not build another
        // voice-leveling processor. That processor would read and later write
        // the same hardware gain.
        let shares_open_device = state
            .lock()
            .map_err(|error| error.to_string())?
            .open_capture_sample_rate(&resolved.config)
            .is_some();
        let start_result = if shares_open_device {
            state
                .lock()
                .map_err(|error| error.to_string())?
                .attach_shared_slot(operation_id, resolved.input, resolved.config, emit_waveform)
        } else {
            let hardware_control = app
                .try_state::<MicrophoneLevelState>()
                .map(|state| state.inner().clone())
                .and_then(|service| {
                    service
                        .level_for(&configured_microphone)
                        .ok()
                        .filter(|level| level.writable && level.volume_percent.is_some())
                        .map(|_| {
                            ServiceHardwareLevelControl::new(service, configured_microphone.clone())
                        })
                });
            let sample_rate = resolved
                .config
                .sample_rate
                .ok_or_else(|| "Dictation microphone sample rate is unavailable".to_string())?;
            let processor = VoiceAwareMicrophoneProcessor::new(
                sample_rate,
                voice_leveling_policy.clone(),
                hardware_control,
            );
            state
                .lock()
                .map_err(|error| error.to_string())?
                .start_slot_with_processor(
                    operation_id,
                    resolved.input,
                    resolved.config,
                    processor,
                    emit_waveform,
                )
        };
        if let Err(error) = start_result {
            let failure = DictationSlotFailure { slot, error };
            state
                .lock()
                .map_err(|error| error.to_string())?
                .mark_slot_start_failed(operation_id, failure.slot, failure.error.clone());
            failures.push(failure);
        }
    }

    let active_slots = state
        .lock()
        .map_err(|error| error.to_string())?
        .active_slots(operation_id);
    if active_slots.is_empty() {
        return Err(dictation_start_error(&failures));
    }
    if failures.is_empty() {
        show_dictation_overlay(
            app,
            overlay::DictationOverlayState::Recording,
            Some(dictation_listening_message(&active_slots)?),
        );
    } else {
        show_dictation_degraded(app, &active_slots, &failures);
    }
    log_dictation_runtime(
        app,
        format!(
            "[dictation] capture=started recording_id={operation_id} slots={}",
            active_slots
                .iter()
                .map(|slot| slot.as_str())
                .collect::<Vec<_>>()
                .join("+")
        ),
    );
    Ok(operation_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DictationStopCause {
    ShortcutRelease,
    DurationLimit,
}

fn finish_dictation_capture(
    app: &AppHandle,
    operation_id: u64,
    cause: DictationStopCause,
) -> Result<bool, String> {
    let Some(state) = app.try_state::<DictationState>() else {
        return Ok(false);
    };
    let claimed = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        let result = match cause {
            DictationStopCause::ShortcutRelease => {
                if runtime.current_operation_id() != Some(operation_id) {
                    return Ok(false);
                }
                Some(runtime.stop_recording())
            }
            DictationStopCause::DurationLimit => runtime.stop_if_recording(operation_id),
        };
        result.map(|result| (result, runtime.payload(None)))
    };
    let Some((result, payload)) = claimed else {
        return Ok(false);
    };
    let result = result?;
    log_dictation_runtime(
        app,
        format!(
            "[dictation] capture=stopped cause={} samples={} slots={}",
            match cause {
                DictationStopCause::ShortcutRelease => "shortcut_release",
                DictationStopCause::DurationLimit => "duration_limit",
            },
            result.recording.samples.len(),
            result
                .recording
                .microphone_inputs
                .iter()
                .map(|input| input.slot.as_str())
                .collect::<Vec<_>>()
                .join("+")
        ),
    );
    emit_dictation_state(app, payload);
    show_dictation_overlay(
        app,
        overlay::DictationOverlayState::Transcribing,
        Some("Transcribing…".to_string()),
    );
    refresh_tray_from_backend(app);
    play_dictation_cue_nonblocking_on_failure(app, DictationCue::Stop, &result.settings);
    spawn_dictation_processing_task(app.clone(), operation_id, result.settings, result.recording)?;
    Ok(true)
}

fn finish_exhausted_dictation(
    app: &AppHandle,
    result: DictationCaptureResult,
) -> Result<(), String> {
    let error = dictation_start_error(&result.failures);
    log_dictation_runtime(
        app,
        format!("[dictation] capture=partial reason=device_failure error={error}"),
    );
    emit_dictation_state(
        app,
        DictationStatePayload {
            state: DictationStage::Transcribing,
            error: None,
        },
    );
    play_dictation_cue_nonblocking_on_failure(app, DictationCue::Stop, &result.settings);
    if prepare_dictation_audio(&result.recording.samples, result.recording.sample_rate)?.is_some() {
        show_dictation_overlay(
            app,
            overlay::DictationOverlayState::Transcribing,
            Some("Transcribing…".to_string()),
        );
        refresh_tray_from_backend(app);
        spawn_dictation_processing_task(
            app.clone(),
            result.operation_id,
            result.settings,
            result.recording,
        )
    } else {
        show_dictation_operation_error(app, result.operation_id, error);
        Ok(())
    }
}

async fn monitor_dictation_capture_health(app: AppHandle, operation_id: u64) {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        interval.tick().await;
        let outcome = app
            .try_state::<DictationState>()
            .and_then(|state| state.lock().ok()?.poll_capture_health(operation_id));
        match outcome {
            None | Some(DictationCaptureHealth::Healthy) => {}
            Some(DictationCaptureHealth::Degraded {
                active_slots,
                failures,
            }) => show_dictation_degraded(&app, &active_slots, &failures),
            Some(DictationCaptureHealth::Exhausted(result)) => {
                if let Err(error) = finish_exhausted_dictation(&app, *result) {
                    show_dictation_operation_error(&app, operation_id, error);
                }
                break;
            }
        }
        if !dictation_operation_is_active(&app, operation_id) {
            break;
        }
    }
}

async fn stop_dictation_after_limit(app: AppHandle, operation_id: u64) {
    tokio::time::sleep(DICTATION_MAX_RECORDING_DURATION).await;
    if let Err(error) =
        finish_dictation_capture(&app, operation_id, DictationStopCause::DurationLimit)
    {
        show_dictation_operation_error(&app, operation_id, error);
    }
}

fn handle_dictation_shortcut_event(
    app: &AppHandle,
    event: DictationShortcutEvent,
    mode: DictationHotkeyMode,
) -> Result<(), String> {
    if app
        .state::<DictationShortcutSuspension>()
        .0
        .load(Ordering::SeqCst)
    {
        return Ok(());
    }
    let mut settings = app
        .try_state::<BackendState>()
        .ok_or_else(|| "dictation backend is unavailable".to_string())?
        .lock()
        .map_err(|error| error.to_string())?
        .settings();
    let enhanced = mode == DictationHotkeyMode::Enhanced;
    if !settings.dictation_enabled
        || (enhanced && !settings.enhance_prompt_enabled)
        || (mode == DictationHotkeyMode::Toggle && settings.dictation_toggle_shortcut.is_empty())
    {
        return Ok(());
    }
    settings.dictation_enhance_prompt = enhanced;
    let event_name = match event {
        DictationShortcutEvent::Pressed => "pressed",
        DictationShortcutEvent::Released => "released",
    };
    log_dictation_runtime(app, format!("[dictation] shortcut={event_name}"));
    let state = app
        .try_state::<DictationState>()
        .ok_or_else(|| "dictation runtime is unavailable".to_string())?;
    let stage = state.lock().map_err(|error| error.to_string())?.stage();
    if let Some(payload) = dictation_recording_permission_guidance(
        event,
        stage,
        permissions::permission_snapshot().microphone.status,
    ) {
        emit_permission_guidance(app, payload);
        return Ok(());
    }
    let action = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        runtime.handle_hotkey_event(event, mode)
    };
    match action {
        DictationAction::Ignore => Ok(()),
        DictationAction::Cancel => {
            cancel_dictation_runtime(app)?;
            Ok(())
        }
        DictationAction::PlayStopCue => {
            play_dictation_cue_nonblocking_on_failure(app, DictationCue::Stop, &settings);
            Ok(())
        }
        DictationAction::StartRecording => {
            set_vor_live_transcription_suspended_for_dictation(app, true);
            preload_dictation_model(app, &settings);
            let payload = state
                .lock()
                .map_err(|error| error.to_string())?
                .payload(None);
            emit_dictation_state(app, payload);
            show_dictation_overlay(
                app,
                overlay::DictationOverlayState::Recording,
                Some("Starting…".to_string()),
            );
            refresh_tray_from_backend(app);
            play_dictation_cue_nonblocking_on_failure(app, DictationCue::Start, &settings);
            let recording_id = start_dictation_capture(app, state.inner(), settings)?;
            let app_for_limit = app.clone();
            tauri::async_runtime::spawn(async move {
                stop_dictation_after_limit(app_for_limit, recording_id).await;
            });
            let app_for_health = app.clone();
            tauri::async_runtime::spawn(async move {
                monitor_dictation_capture_health(app_for_health, recording_id).await;
            });
            Ok(())
        }
        DictationAction::StopAndTranscribe => {
            let operation_id = state
                .lock()
                .map_err(|error| error.to_string())?
                .current_operation_id()
                .ok_or_else(|| "dictation operation is unavailable".to_string())?;
            finish_dictation_capture(app, operation_id, DictationStopCause::ShortcutRelease)?;
            Ok(())
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn update_settings(
    app: AppHandle,
    state: State<'_, BackendState>,
    dictionary_state: State<'_, DictionaryFileState>,
    live_state: State<'_, LiveCaptureState>,
    transcription_state: State<'_, AutoTranscriptionState>,
    live_transcriber_state: State<'_, LiveTranscriberState>,
    system_capture_state: State<'_, SystemCaptureState>,
    source_capture_lifecycle_state: State<'_, SourceCaptureLifecycleState>,
    detected_source_state: State<'_, DetectedSourceState>,
    meeting_state: State<'_, MeetingState>,
    voice_leveling_policy: State<'_, VoiceLevelingPolicy>,
    mut patch: SettingsPatch,
) -> Result<AppSettings, String> {
    let _update_activity = RESTART_GATE.activity()?;
    normalize_dictation_patch(&mut patch)?;
    if let Some(raw) = patch.cycle_microphone_shortcut.as_mut() {
        *raw = microphone_shortcut::normalize(raw)?;
    }
    if let Some(raw) = patch.auto_type_toggle_shortcut.as_mut() {
        *raw = autotype_shortcut::normalize(raw)?;
    }
    let dictionary_was_patched = patch.dictionary.is_some();
    let (
        previous_settings,
        launch_at_login_action,
        live_capture_action,
        previous_model_directory,
        previous_show_dock_icon,
    ) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        let settings = backend.settings();
        (
            settings.clone(),
            launch_at_login_action_for_patch(&settings, &patch),
            live_capture_runtime_action_for_patch(&settings, &patch),
            settings.model_directory.clone(),
            settings.show_dock_icon,
        )
    };
    let candidate_settings = candidate_dictation_settings(&previous_settings, &patch)?;
    if active_dictation_shortcuts(&previous_settings)
        != active_dictation_shortcuts(&candidate_settings)
    {
        cancel_dictation_runtime(&app)?;
    }
    autotype_shortcut::validate(&candidate_settings)?;
    apply_launch_at_login_action(&app, launch_at_login_action)?;
    microphone_shortcut::reconcile(&app, &previous_settings, &candidate_settings)?;
    if let Err(error) = autotype_shortcut::reconcile(&app, &candidate_settings) {
        let cycle_restore =
            microphone_shortcut::reconcile(&app, &candidate_settings, &previous_settings);
        return Err(format!("{error}; restore: cycle={cycle_restore:?}"));
    }

    let (settings, live_event_handler, live_events) = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        let settings = match backend.try_update_settings(patch) {
            Ok(settings) => settings,
            Err(error) => {
                let shortcut_rollback = microphone_shortcut::reconcile(
                    &app,
                    &candidate_settings,
                    &previous_settings,
                )
                .and(autotype_shortcut::reconcile(&app, &previous_settings));
                let launch_rollback = apply_launch_at_login_action(
                    &app,
                    launch_at_login_action_for_patch(
                        &candidate_settings,
                        &SettingsPatch {
                            launch_at_login: Some(previous_settings.launch_at_login),
                            ..Default::default()
                        },
                    ),
                );
                return Err(format!(
                    "{error}{}{}",
                    shortcut_rollback
                        .err()
                        .map(|error| format!("; shortcut restore failed: {error}"))
                        .unwrap_or_default(),
                    launch_rollback
                        .err()
                        .map(|error| format!("; login setting restore failed: {error}"))
                        .unwrap_or_default(),
                ));
            }
        };
        let (handler, events) = live_events_for_dispatch(&mut backend);
        (settings, handler, events)
    };
    if previous_settings.auto_transcript_input_enabled != settings.auto_transcript_input_enabled
        || previous_settings.auto_transcript_input_model != settings.auto_transcript_input_model
        || settings.pause_all
        || !settings.transcription_enabled
        || !settings.recording_enabled
    {
        clear_auto_type(&app);
    }
    voice_leveling_policy.set_enabled(settings.voice_auto_level_enabled);
    if dictionary_was_patched {
        match dictionary_state.lock() {
            Ok(mut store) => {
                let outcome = match store.save_entries(&settings.dictionary) {
                    Ok(()) => DictionaryApplyOutcome::Changed(Box::new(settings.clone())),
                    Err(error) => DictionaryApplyOutcome::Invalid(error),
                };
                let status = store.status();
                emit_dictionary_outcome(&app, &outcome, &status);
            }
            Err(error) => {
                eprintln!("[dictionary-file] store lock failed after settings save: {error}");
            }
        }
    }
    if previous_settings.dictation_enabled && !settings.dictation_enabled {
        let _ = cancel_dictation_runtime(&app);
    }
    if settings.model_directory != previous_model_directory
        && let Ok(slot) = live_transcriber_state.lock()
        && let Some(service) = slot.as_ref()
    {
        service.update_model_directory(&settings.model_directory);
    }
    preload_dictation_model(&app, &settings);
    if settings.auto_transcript_input_enabled
        && let Ok(slot) = live_transcriber_state.lock()
        && let Some(service) = slot.as_ref()
        && model_supports_live_partials(
            &settings.model_directory,
            settings.effective_auto_type_model(),
        )
    {
        service.preload(settings.effective_auto_type_model());
    }
    if settings.show_dock_icon != previous_show_dock_icon {
        apply_dock_icon_visibility(&app, settings.show_dock_icon)
            .map_err(|error| error.to_string())?;
    }
    if settings_window_should_open_on_launch(&settings) {
        ensure_main_window_visible(&app, settings.show_dock_icon)
            .map_err(|error| error.to_string())?;
    }
    dispatch_live_events(live_event_handler, live_events);
    apply_live_capture_runtime_action(
        &app,
        state.inner(),
        live_state.inner(),
        transcription_state.inner().clone(),
        live_capture_action,
    )?;
    apply_input_monitor_settings(&app, state.inner())?;
    apply_system_capture_settings_action(
        &app,
        &settings,
        state.inner(),
        system_capture_state.inner(),
        source_capture_lifecycle_state.inner(),
        detected_source_state.inner(),
        meeting_state.inner(),
    )?;
    kick_transcription_worker_if_needed(
        app.clone(),
        state.inner().clone(),
        transcription_state.inner().clone(),
    );

    if overlay_presentation_settings_changed(&previous_settings, &settings) {
        apply_overlay_settings_change(&app, &settings);
    }

    Ok(settings)
}

fn overlay_presentation_settings_changed(previous: &AppSettings, next: &AppSettings) -> bool {
    previous.subtitle_translation_enabled != next.subtitle_translation_enabled
        || previous.subtitle_translation_language != next.subtitle_translation_language
        || previous.effective_text_transform_model() != next.effective_text_transform_model()
        || previous.effective_floating_overlay_position()
            != next.effective_floating_overlay_position()
        || previous.effective_dictation_subtitle_position()
            != next.effective_dictation_subtitle_position()
        || previous.floating_overlay_caption_style() != next.floating_overlay_caption_style()
        || previous.dictation_bubble_position != next.dictation_bubble_position
        || previous.dictation_overlay_style() != next.dictation_overlay_style()
}

fn dictation_overlay_presentation(
    stage: DictationStage,
) -> Option<(overlay::DictationOverlayState, &'static str)> {
    match stage {
        DictationStage::Recording => {
            Some((overlay::DictationOverlayState::Recording, "Listening…"))
        }
        DictationStage::Transcribing => Some((
            overlay::DictationOverlayState::Transcribing,
            "Transcribing…",
        )),
        DictationStage::Error => Some((overlay::DictationOverlayState::Error, "Dictation failed")),
        DictationStage::Idle => None,
    }
}

fn refresh_active_dictation_overlay(app: &AppHandle) -> bool {
    let presentation = app.try_state::<DictationState>().and_then(|state| {
        state
            .lock()
            .ok()
            .and_then(|runtime| dictation_overlay_presentation(runtime.stage()))
    });
    let Some((state, message)) = presentation else {
        return false;
    };

    show_dictation_overlay(app, state, Some(message.to_string()));
    true
}

fn apply_overlay_settings_change(app: &AppHandle, settings: &AppSettings) {
    let mut published_caption = false;
    if let Some(caption_state) = app.try_state::<OverlayCaptionState>() {
        let snapshot = caption_state.lock().ok().map(|mut runtime| {
            let position = match runtime.snapshot().source {
                OverlayCaptionSource::LiveTranscription => {
                    settings.effective_floating_overlay_position()
                }
                OverlayCaptionSource::Dictation => settings.effective_dictation_subtitle_position(),
                OverlayCaptionSource::Preview => settings.floating_overlay_position,
            };
            runtime.set_position(position);
            runtime.set_style(settings.floating_overlay_caption_style());
            runtime.snapshot()
        });
        if let Some(snapshot) = snapshot {
            publish_overlay_caption_snapshot(app, snapshot, "overlay settings change");
            published_caption = true;
        }
    }

    let refreshed_dictation = refresh_active_dictation_overlay(app);
    if !published_caption
        && !refreshed_dictation
        && let Err(error) =
            overlay::hide_overlay_on_main_thread(app, "position change hide inactive")
    {
        eprintln!("[overlay] position change failed: {error}");
    }
}

/// What to do with an active system-audio capture session after a settings
/// change. `None` means no session is open (nothing to do).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemCaptureSettingsAction {
    /// Feature/recording disabled or paused: tear the session down.
    Stop,
    /// Keep capturing; propagate the new settings to the live controller.
    Sync,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMonitorRuntimeAction {
    Start,
    Stop,
    Unchanged,
}

fn input_monitor_runtime_action(
    settings: &AppSettings,
    live_input_running: bool,
    monitor_running: bool,
) -> InputMonitorRuntimeAction {
    let should_run = settings.input_monitoring_enabled && live_input_running;
    match (monitor_running, should_run) {
        (false, true) => InputMonitorRuntimeAction::Start,
        (true, false) => InputMonitorRuntimeAction::Stop,
        _ => InputMonitorRuntimeAction::Unchanged,
    }
}

/// Pure decision for `apply_system_capture_settings_action`. The session must be
/// stopped when the feature is disabled, recording is off, or pause-all is on
/// (§7/§2 pause semantics); otherwise the live settings are synced so the
/// running capture never operates on a frozen snapshot.
fn system_capture_settings_action(
    settings: &AppSettings,
    capturing: bool,
) -> SystemCaptureSettingsAction {
    if !capturing {
        return SystemCaptureSettingsAction::None;
    }
    if !settings.system_audio_enabled || !settings.recording_enabled || settings.pause_all {
        SystemCaptureSettingsAction::Stop
    } else {
        SystemCaptureSettingsAction::Sync
    }
}

fn apply_input_monitor_settings(
    app: &AppHandle,
    backend_state: &BackendState,
) -> Result<(), String> {
    let Some(input_monitor_state) = app.try_state::<InputMonitorState>() else {
        return Ok(());
    };
    let input_monitor_state = input_monitor_state.inner().clone();
    let (settings, sample_rate) = {
        let backend = backend_state.lock().map_err(|error| error.to_string())?;
        (backend.settings(), backend.active_capture_sample_rate())
    };

    let mut monitor = input_monitor_state
        .lock()
        .map_err(|error| error.to_string())?;
    if let Some(error) = monitor.runtime_error() {
        append_runtime_debug_log(
            &settings,
            format!("[input-monitor] output_stream_error error={error}"),
        );
        monitor.stop();
    }

    match input_monitor_runtime_action(&settings, sample_rate.is_some(), monitor.is_running()) {
        InputMonitorRuntimeAction::Start => {
            if let Some(sample_rate) = sample_rate {
                match monitor.start(sample_rate) {
                    Ok(()) => append_runtime_debug_log(
                        &settings,
                        format!("[input-monitor] start sample_rate={sample_rate}"),
                    ),
                    Err(error) => {
                        monitor.stop();
                        append_runtime_debug_log(
                            &settings,
                            format!("[input-monitor] start_failed error={error}"),
                        );
                    }
                }
            }
        }
        InputMonitorRuntimeAction::Stop => {
            monitor.stop();
            append_runtime_debug_log(&settings, "[input-monitor] stop");
        }
        InputMonitorRuntimeAction::Unchanged => {}
    }
    Ok(())
}

fn source_capture_retry_delay(attempts: u8) -> Duration {
    let index = attempts.saturating_sub(1) as usize;
    SOURCE_CAPTURE_RETRY_DELAYS[index.min(SOURCE_CAPTURE_RETRY_DELAYS.len() - 1)]
}

fn source_capture_start_decision(
    lifecycle: &SourceCaptureLifecycle,
    source_id: &str,
    now: Instant,
) -> SourceCaptureStartDecision {
    match lifecycle {
        SourceCaptureLifecycle::Idle => SourceCaptureStartDecision::Start { attempt: 1 },
        SourceCaptureLifecycle::Starting { .. } => SourceCaptureStartDecision::AlreadyStarting,
        SourceCaptureLifecycle::Running { .. } => SourceCaptureStartDecision::AlreadyRunning,
        SourceCaptureLifecycle::Stopping { .. } => SourceCaptureStartDecision::Stopping,
        SourceCaptureLifecycle::Failed {
            source_id: failed_source_id,
            failed_at,
            attempts,
        } if failed_source_id == source_id => {
            let delay = source_capture_retry_delay(*attempts);
            let retry_at = *failed_at + delay;
            match retry_at.checked_duration_since(now) {
                Some(retry_after) if !retry_after.is_zero() => {
                    SourceCaptureStartDecision::BackingOff { retry_after }
                }
                _ => SourceCaptureStartDecision::Start {
                    attempt: attempts.saturating_add(1).max(1),
                },
            }
        }
        SourceCaptureLifecycle::Failed { .. } => SourceCaptureStartDecision::Start { attempt: 1 },
    }
}

fn source_capture_mark_starting(
    lifecycle: &mut SourceCaptureLifecycle,
    source_id: &str,
    attempt: u8,
) {
    *lifecycle = SourceCaptureLifecycle::Starting {
        source_id: source_id.to_string(),
        attempt: attempt.max(1),
    };
}

fn source_capture_mark_running(lifecycle: &mut SourceCaptureLifecycle, source_id: &str) {
    *lifecycle = SourceCaptureLifecycle::Running {
        source_id: source_id.to_string(),
    };
}

fn source_capture_mark_stopping(lifecycle: &mut SourceCaptureLifecycle, source_id: &str) {
    *lifecycle = SourceCaptureLifecycle::Stopping {
        source_id: source_id.to_string(),
    };
}

fn source_capture_mark_failed(
    lifecycle: &mut SourceCaptureLifecycle,
    source_id: &str,
    failed_at: Instant,
) {
    let attempts = match lifecycle {
        SourceCaptureLifecycle::Starting {
            source_id: active_source,
            attempt,
        } if active_source == source_id => *attempt,
        SourceCaptureLifecycle::Failed {
            source_id: failed_source,
            attempts,
            ..
        } if failed_source == source_id => *attempts,
        _ => 1,
    };
    *lifecycle = SourceCaptureLifecycle::Failed {
        source_id: source_id.to_string(),
        failed_at,
        attempts: attempts.max(1),
    };
}

fn source_capture_mark_idle(lifecycle: &mut SourceCaptureLifecycle) {
    *lifecycle = SourceCaptureLifecycle::Idle;
}

fn source_capture_recovery_pending(lifecycle: &SourceCaptureLifecycle, source_id: &str) -> bool {
    matches!(
        lifecycle,
        SourceCaptureLifecycle::Failed {
            source_id: failed_source_id,
            ..
        } if failed_source_id == source_id
    )
}

fn source_capture_failure_diagnostic(
    source_id: &str,
    attempt: u8,
    screen_recording_status: permissions::PermissionGrantStatus,
    error: &str,
) -> String {
    format!(
        "[source-capture] error source_id={} attempt={} screen_recording_status={:?} next_retry_ms={} error={}",
        source_id,
        attempt.max(1),
        screen_recording_status,
        source_capture_retry_delay(attempt.max(1)).as_millis(),
        error
    )
}

/// Reconcile an active system-audio capture session with newly applied settings.
/// If the feature was disabled, recording turned off, or pause-all turned on,
/// tear the session down the same way `stop_source_capture` does (drop the
/// stream handle, flush/stop the backend session, clear detection, emit
/// `source-capture-stopped`). Otherwise propagate the new settings to the live
/// system `CaptureController` so threshold/chunk-timing changes take effect
/// mid-capture without restarting the stream.
fn apply_system_capture_settings_action(
    app: &AppHandle,
    settings: &AppSettings,
    backend_state: &BackendState,
    system_capture_state: &SystemCaptureState,
    source_capture_lifecycle_state: &SourceCaptureLifecycleState,
    detected_source_state: &DetectedSourceState,
    meeting_state: &MeetingState,
) -> Result<(), String> {
    let capturing = system_capture_state
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or(false);
    match system_capture_settings_action(settings, capturing) {
        SystemCaptureSettingsAction::Stop => {
            stop_source_capture_runtime(
                app,
                backend_state,
                system_capture_state,
                source_capture_lifecycle_state,
                detected_source_state,
                meeting_state,
            )?;
        }
        SystemCaptureSettingsAction::Sync => {
            let mut backend = backend_state.lock().map_err(|error| error.to_string())?;
            backend.sync_system_capture_settings();
        }
        SystemCaptureSettingsAction::None => {}
    }
    Ok(())
}

fn live_events_for_dispatch(
    backend: &mut AppBackend,
) -> (Option<LiveEventHandler>, Vec<LiveTranscriptEvent>) {
    let handler = backend.live_event_handler();
    let events = backend.drain_live_events();
    (handler, events)
}

fn dispatch_live_events(handler: Option<LiveEventHandler>, events: Vec<LiveTranscriptEvent>) {
    if events.is_empty() {
        return;
    }

    let Some(handler) = handler else {
        eprintln!("[wakenote] WARN: live events generated before handler was set");
        return;
    };

    for event in events {
        handler(event);
    }
}

fn overlay_level_emit_due(last_emit: &Mutex<Instant>) -> bool {
    let now = Instant::now();
    let Ok(mut last_emit_at) = last_emit.lock() else {
        return false;
    };
    if now.duration_since(*last_emit_at) < OVERLAY_LEVEL_EMIT_INTERVAL {
        return false;
    }
    *last_emit_at = now;
    true
}

async fn resolve_microphones_for_ui(settings: AppSettings) -> Vec<MicrophoneDevice> {
    let fallback_settings = settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (sender, receiver) = mpsc::channel();
        let selected_for_worker = settings.selected_microphone.clone();
        thread::spawn(move || {
            let mut devices =
                microphone_devices_from_input_devices(&selected_for_worker, list_input_devices());
            let volume_backend = PlatformVolumeBackend::new();
            let system_devices = volume_backend.input_devices().unwrap_or_default();
            let default_uid = volume_backend.default_input_uid().ok().flatten();
            for device in &mut devices {
                device.core_audio_uid = if device.id == "default" {
                    default_uid.clone()
                } else {
                    system_devices
                        .iter()
                        .find(|candidate| {
                            candidate.legacy_cpal_id.as_deref() == Some(device.id.as_str())
                                && candidate.label == device.label
                        })
                        .map(|candidate| candidate.uid.clone())
                };
                device.transport_type = device.core_audio_uid.as_deref().and_then(|uid| {
                    wakenote::microphone_level::macos::input_transport_type(uid).ok()
                });
            }
            let _ = sender.send(devices);
        });

        match receiver.recv_timeout(Duration::from_millis(600)) {
            Ok(devices) => stable_microphone_options(&settings, devices),
            Err(_) => fallback_microphones(&settings),
        }
    })
    .await
    .unwrap_or_else(|_| fallback_microphones(&fallback_settings))
}

fn stable_microphone_options(
    settings: &AppSettings,
    mut devices: Vec<MicrophoneDevice>,
) -> Vec<MicrophoneDevice> {
    for configured in &settings.capture_microphones {
        if devices.iter().any(|device| device.id == configured.id) {
            continue;
        }
        let same_name_available = devices
            .iter()
            .filter(|device| {
                device.available && microphone_labels_match(&device.label, &configured.label)
            })
            .count();
        if same_name_available == 1 {
            continue;
        }
        devices.push(MicrophoneDevice {
            transport_type: None,
            id: configured.id.clone(),
            label: configured.label.clone(),
            core_audio_uid: configured.core_audio_uid.clone(),
            available: false,
            fallback: false,
        });
    }
    devices
}

fn fallback_microphones(settings: &AppSettings) -> Vec<MicrophoneDevice> {
    stable_microphone_options(
        settings,
        vec![MicrophoneDevice {
            transport_type: None,
            id: "default".to_string(),
            label: "System Default".to_string(),
            core_audio_uid: None,
            available: true,
            fallback: settings.selected_microphone != "default",
        }],
    )
}

fn resolve_capture_device_with_timeout(
    device_id: String,
    label_hint: Option<String>,
    core_audio_uid: Option<String>,
) -> Result<ResolvedCpalInputDevice, LiveCaptureError> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(CpalAudioInput::resolve_device_with_uid(
            &device_id,
            label_hint.as_deref(),
            core_audio_uid.as_deref(),
        ));
    });

    match receiver.recv_timeout(AUDIO_DEVICE_RESOLVE_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(LiveCaptureError::Cpal(format!(
            "audio device lookup did not finish within {} seconds",
            AUDIO_DEVICE_RESOLVE_TIMEOUT.as_secs()
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(LiveCaptureError::Cpal(
            "audio device lookup disconnected".to_string(),
        )),
    }
}

#[tauri::command]
fn app_status(
    app: AppHandle,
    state: State<'_, BackendState>,
    live_state: State<'_, LiveCaptureState>,
) -> Result<AppStatus, String> {
    let (dropped_frames, stream_error) = live_state
        .lock()
        .map(|live_capture| {
            (
                live_capture.total_dropped_frame_count(),
                live_capture.first_runtime_error(),
            )
        })
        .unwrap_or((0, None));
    let (settings, status) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        (
            backend.settings(),
            with_live_runtime_warning(backend.app_status(), dropped_frames, stream_error),
        )
    };
    update_tray_presentation(&app, &settings, &status);
    Ok(status)
}

#[tauri::command]
async fn list_microphones(state: State<'_, BackendState>) -> Result<Vec<MicrophoneDevice>, String> {
    let settings = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        backend.settings()
    };
    Ok(resolve_microphones_for_ui(settings).await)
}

fn input_levels_for_entries(
    levels: &MicrophoneLevelService<PlatformVolumeBackend>,
    entries: &[CaptureMicrophoneEntry],
) -> Vec<MicrophoneInputLevel> {
    entries
        .iter()
        .map(|entry| {
            levels
                .level_for(entry)
                .unwrap_or_else(|error| MicrophoneInputLevel {
                    device_id: entry.id.clone(),
                    label: entry.label.clone(),
                    volume_percent: None,
                    writable: false,
                    available: false,
                    error: Some(error.user_message()),
                })
        })
        .collect()
}

#[tauri::command]
async fn microphone_input_levels(
    backend: State<'_, BackendState>,
    levels: State<'_, MicrophoneLevelState>,
    gate: State<'_, input_level_query::HardwareQueryGate>,
) -> Result<Vec<MicrophoneInputLevel>, String> {
    let entries = backend
        .lock()
        .map_err(|error| error.to_string())?
        .settings()
        .capture_microphones;
    let fallback = entries.clone();
    let levels = levels.inner().clone();
    Ok(gate.run(move || Ok(input_levels_for_entries(levels.as_ref(), &entries))).await
        .unwrap_or_else(|error| unavailable_input_levels(&fallback, error)))
}

fn unavailable_input_levels(entries: &[CaptureMicrophoneEntry], error: String) -> Vec<MicrophoneInputLevel> {
    entries.iter().map(|entry| MicrophoneInputLevel {
        device_id: entry.id.clone(), label: entry.label.clone(), volume_percent: None,
        writable: false, available: false, error: Some(error.clone()),
    }).collect()
}

#[tauri::command]
async fn set_microphone_input_volume(
    device_id: String,
    volume_percent: u8,
    backend: State<'_, BackendState>,
    levels: State<'_, MicrophoneLevelState>,
    gate: State<'_, input_level_query::HardwareQueryGate>,
) -> Result<Vec<MicrophoneInputLevel>, String> {
    let entries = backend
        .lock()
        .map_err(|error| error.to_string())?
        .settings()
        .capture_microphones;
    let configured = entries
        .iter()
        .find(|entry| entry.id == device_id)
        .cloned()
        .ok_or_else(|| format!("Unknown configured microphone: {device_id}"))?;
    let levels = levels.inner().clone();
    gate.run(move || {
        levels.set_volume(&configured, volume_percent.min(100)).map_err(|error| error.user_message())?;
        Ok(input_levels_for_entries(levels.as_ref(), &entries))
    }).await
}

fn spawn_microphone_input_level_observer(
    app: AppHandle,
    backend: BackendState,
    levels: MicrophoneLevelState,
) {
    let query_gate = app.state::<input_level_query::HardwareQueryGate>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let mut previous = Vec::new();
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            interval.tick().await;
            let entries = match backend.lock() {
                Ok(backend) => backend.settings().capture_microphones,
                Err(error) => {
                    eprintln!("[microphone-level] settings lock failed: {error}");
                    continue;
                }
            };
            let fallback = entries.clone();
            let levels_for_query = levels.clone();
            let next = query_gate.run(move || Ok(input_levels_for_entries(levels_for_query.as_ref(), &entries))).await
                .unwrap_or_else(|error| unavailable_input_levels(&fallback, error));
            if wakenote::microphone_level::levels_changed(&previous, &next) {
                previous = next.clone();
                let _ = app.emit(MICROPHONE_INPUT_LEVELS_CHANGED_EVENT, next);
            }
        }
    });
}

#[tauri::command]
async fn merge_transcript_audio(
    app: AppHandle,
    request: MergeAudioRequest,
) -> Result<AudioMergeResult, String> {
    let _update_activity = RESTART_GATE.long_activity()?;
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        merge_audio_to_m4a(request, move |progress| {
            let _ = progress_app.emit("audio-merge-progress", progress);
        })
    })
    .await
    .map_err(|error| format!("audio merge task failed: {error}"))?
}

#[tauri::command]
fn list_models(state: State<'_, BackendState>) -> Result<Vec<ModelDescriptor>, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(backend.model_registry())
}

#[tauri::command]
fn verify_model(
    state: State<'_, BackendState>,
    model_id: String,
) -> Result<Vec<ModelDescriptor>, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.verify_model(&model_id)
}

#[tauri::command]
fn download_model(
    state: State<'_, BackendState>,
    model_id: String,
) -> Result<Vec<ModelDescriptor>, String> {
    let update_activity = RESTART_GATE.long_activity()?;
    let prepared = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        backend.prepare_model_download(&model_id)?
    };
    let model_directory = prepared.model_directory.clone();
    let model = prepared.model.clone();
    thread::spawn(move || {
        let _update_activity = update_activity;
        let store = ModelStore::new(model_directory);
        // sherpa-onnx models download a .tar.bz2 and extract in place; whisper
        // models download a single .bin. Both paths record an Error status into
        // the store on failure (surfaced via the registry snapshot); log here so
        // a failed background download is observable in the app log too.
        let result = match model.provider_runtime.as_str() {
            "sherpa-onnx" => store.download_and_extract_sherpa_model(&model),
            "qwen3-asr" => store.install_qwen3_asr_model(&model),
            _ => store.download_model(&model),
        };
        if let Err(error) = result {
            eprintln!("[models] download {} failed: {error}", model.id);
        }
    });

    Ok(prepared.registry)
}

#[tauri::command]
fn cancel_model_download(
    state: State<'_, BackendState>,
    model_id: String,
) -> Result<Vec<ModelDescriptor>, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.cancel_model_download(&model_id)
}

#[tauri::command]
fn delete_model(
    state: State<'_, BackendState>,
    model_id: String,
) -> Result<Vec<ModelDescriptor>, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    backend.delete_model(&model_id)
}

#[tauri::command]
fn queue_snapshot(state: State<'_, BackendState>) -> Result<QueueSnapshot, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(backend.queue_snapshot())
}

#[tauri::command]
fn permission_snapshot() -> AppPermissions {
    permissions::permission_snapshot()
}

#[tauri::command]
async fn request_accessibility_permission() -> Result<AppPermissions, String> {
    tauri::async_runtime::spawn_blocking(permissions::request_accessibility_permission)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_accessibility_permission_settings() -> Result<(), String> {
    permissions::open_accessibility_permission_settings()
}

#[tauri::command]
async fn request_microphone_permission() -> Result<AppPermissions, String> {
    tauri::async_runtime::spawn_blocking(permissions::request_microphone_permission)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_microphone_permission_settings() -> Result<(), String> {
    permissions::open_microphone_permission_settings()
}

#[tauri::command]
async fn request_screen_recording_permission() -> Result<AppPermissions, String> {
    tauri::async_runtime::spawn_blocking(permissions::request_screen_recording_permission)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn open_screen_recording_settings() -> Result<(), String> {
    permissions::open_screen_recording_permission_settings()
}

#[tauri::command]
fn open_permission_drag_shelf(
    app: AppHandle,
    target: PermissionDragTarget,
) -> Result<AppPermissions, String> {
    permission_drag::show_permission_drag_shelf(&app, target)?;
    Ok(permissions::permission_snapshot())
}

#[tauri::command]
async fn recent_transcripts(
    state: State<'_, BackendState>,
    limit: Option<usize>,
) -> Result<Vec<RecentTranscript>, String> {
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(backend.settings().save_root)
    };
    let limit = normalize_recent_transcript_limit(limit);

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::commands::recent_transcripts_from_save_root(&save_root, limit)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn recent_dictations(
    state: State<'_, BackendState>,
    limit: Option<usize>,
) -> Result<Vec<RecentTranscript>, String> {
    let (save_root, hidden) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        (
            expand_user_path(backend.settings().save_root),
            backend
                .load_list_visibility()
                .map_err(|error| error.to_string())?
                .transcripts,
        )
    };
    let limit = normalize_recent_transcript_limit(Some(limit.unwrap_or(10)));
    tauri::async_runtime::spawn_blocking(move || {
        wakenote::commands::recent_dictations_from_save_root(&save_root, limit, &hidden)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn transcript_days(
    state: State<'_, BackendState>,
) -> Result<Vec<wakenote::commands::TranscriptDay>, String> {
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(backend.settings().save_root)
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::commands::transcript_days_from_save_root(&save_root)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn transcripts_for_day(
    state: State<'_, BackendState>,
    day: String,
    download: bool,
) -> Result<Vec<RecentTranscript>, String> {
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(backend.settings().save_root)
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::commands::transcripts_for_day_from_save_root(&save_root, &day, download)
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn export_all_transcript_days(
    state: State<'_, BackendState>,
    destination_parent: String,
) -> Result<wakenote::transcript_export::TranscriptExportResult, String> {
    let _update_activity = RESTART_GATE.long_activity()?;
    let (save_root, custom_sources) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        let settings = backend.settings();
        (
            expand_user_path(settings.save_root),
            settings.custom_sources,
        )
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::transcript_export::export_all_transcript_days(
            &save_root,
            Path::new(&destination_parent),
            &custom_sources,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn export_transcript_day(
    state: State<'_, BackendState>,
    day: String,
    destination_path: String,
    overwrite: Option<bool>,
) -> Result<wakenote::transcript_export::TranscriptExportResult, String> {
    let _update_activity = RESTART_GATE.long_activity()?;
    let (save_root, custom_sources) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        let settings = backend.settings();
        (
            expand_user_path(settings.save_root),
            settings.custom_sources,
        )
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::transcript_export::export_transcript_day(
            &save_root,
            &day,
            Path::new(&destination_path),
            &custom_sources,
            overwrite.unwrap_or(false),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn rebuild_transcript_day_index(
    state: State<'_, BackendState>,
    day: String,
    download: bool,
) -> Result<Vec<RecentTranscript>, String> {
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(backend.settings().save_root)
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::commands::rebuild_transcript_day_index_from_save_root(&save_root, &day, download)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn llm_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn llm_settings_and_key(backend_state: &BackendState) -> Result<(AppSettings, String), String> {
    let backend = backend_state.lock().map_err(|error| error.to_string())?;
    let api_key = backend
        .load_openrouter_api_key()?
        .ok_or_else(|| "OpenRouter API key is not configured".to_string())?;
    Ok((backend.settings(), api_key))
}

fn emit_llm_run_update(app: &AppHandle, snapshot: &LlmReportRunSnapshot) {
    let _ = app.emit(EVENT_LLM_REPORT_RUN_UPDATED, snapshot.clone());
}

fn emit_terminal_run_result(
    app: &AppHandle,
    result: Result<Option<LlmReportRunSnapshot>, String>,
    context: &str,
) {
    match result {
        Ok(Some(snapshot)) => emit_llm_run_update(app, &snapshot),
        Ok(None) => {}
        Err(error) => eprintln!("[llm-report] {context}: {error}"),
    }
}

fn spawn_registered_llm_report_run(
    app: AppHandle,
    run_state: LlmRunState,
    settings: AppSettings,
    api_key: String,
    snapshot: LlmReportRunSnapshot,
    request: wakenote::llm::LlmGenerateRequest,
    cancellation: tokio_util::sync::CancellationToken,
) {
    let store = LlmRunStore::new(expand_user_path(&settings.save_root));
    tauri::async_runtime::spawn(async move {
        let progress_app = app.clone();
        let progress_state = run_state.clone();
        let progress_store = store.clone();
        let result = wakenote::llm::generate_transcript_report_with_client_progress_and_cancel(
            &settings,
            &api_key,
            request,
            &wakenote::llm::ReqwestOpenRouterClient::default(),
            &cancellation,
            move |event| {
                let update = progress_state
                    .lock()
                    .map_err(|error| error.to_string())?
                    .progress(&progress_store, event, &llm_now())?;
                if let Some(update) = update {
                    emit_llm_run_update(&progress_app, &update);
                }
                Ok(())
            },
        )
        .await;

        if cancellation.is_cancelled() {
            if let Ok(response) = &result
                && let Some(report_path) = &response.report_path
            {
                wakenote::llm::remove_persisted_report(report_path);
            }
            let cancelled = run_state
                .lock()
                .map_err(|error| error.to_string())
                .and_then(|mut runtime| {
                    runtime.finish_cancelled(&store, &snapshot.run_id, &llm_now())
                });
            emit_terminal_run_result(&app, cancelled, "could not persist cancellation");
            return;
        }

        match result {
            Ok(response) => {
                let report_path = response.report_path.clone();
                let completion = run_state
                    .lock()
                    .map_err(|error| error.to_string())
                    .and_then(|mut runtime| {
                        runtime.complete(&store, &snapshot.run_id, &response, &llm_now())
                    });
                match completion {
                    Ok(Some(completed)) => {
                        emit_llm_run_update(&app, &completed);
                    }
                    Ok(None) => {
                        if let Some(report_path) = report_path {
                            wakenote::llm::remove_persisted_report(&report_path);
                        }
                    }
                    Err(error) => {
                        if let Some(report_path) = report_path {
                            wakenote::llm::remove_persisted_report(&report_path);
                        }
                        let failed = run_state
                            .lock()
                            .map_err(|lock_error| lock_error.to_string())
                            .and_then(|mut runtime| {
                                runtime.fail(&store, &snapshot.run_id, &error, &llm_now())
                            });
                        emit_terminal_run_result(&app, failed, "completion persistence failed");
                    }
                }
            }
            Err(wakenote::llm::LlmGenerateError::Cancelled) => {
                let cancelled = run_state
                    .lock()
                    .map_err(|error| error.to_string())
                    .and_then(|mut runtime| {
                        runtime.finish_cancelled(&store, &snapshot.run_id, &llm_now())
                    });
                emit_terminal_run_result(&app, cancelled, "could not persist cancellation");
            }
            Err(wakenote::llm::LlmGenerateError::Failed(error)) => {
                let failed = run_state
                    .lock()
                    .map_err(|lock_error| lock_error.to_string())
                    .and_then(|mut runtime| {
                        runtime.fail(&store, &snapshot.run_id, &error, &llm_now())
                    });
                emit_terminal_run_result(&app, failed, "could not persist failure");
            }
        }
    });
}

#[tauri::command]
async fn start_llm_report(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    run_state: State<'_, LlmRunState>,
    request: wakenote::llm::LlmGenerateRequest,
) -> Result<LlmReportRunSnapshot, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let (settings, api_key) = llm_settings_and_key(backend_state.inner())?;
    let store = LlmRunStore::new(expand_user_path(&settings.save_root));
    let max_iterations = clamp_llm_max_iterations(settings.llm_max_iterations);
    let (snapshot, private_request, cancellation) = {
        let mut runtime = run_state.lock().map_err(|error| error.to_string())?;
        let snapshot = runtime.start(
            &store,
            request,
            None,
            &settings.openrouter_model,
            max_iterations,
            &llm_now(),
        )?;
        let private_request = store.load(&snapshot.run_id)?.request;
        let cancellation = runtime
            .active()
            .filter(|active| active.run_id == snapshot.run_id)
            .ok_or_else(|| "Report run did not become active".to_string())?
            .cancellation;
        (snapshot, private_request, cancellation)
    };
    emit_llm_run_update(&app, &snapshot);
    spawn_registered_llm_report_run(
        app,
        run_state.inner().clone(),
        settings,
        api_key,
        snapshot.clone(),
        private_request,
        cancellation,
    );
    Ok(snapshot)
}

#[tauri::command]
async fn list_llm_report_runs(
    backend_state: State<'_, BackendState>,
) -> Result<Vec<LlmReportRunSnapshot>, String> {
    let save_root = {
        let backend = backend_state.lock().map_err(|error| error.to_string())?;
        expand_user_path(backend.settings().save_root)
    };
    LlmRunStore::new(save_root).list().map(|records| {
        records
            .into_iter()
            .map(|record| record.snapshot())
            .collect()
    })
}

#[tauri::command]
async fn cancel_llm_report(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    run_state: State<'_, LlmRunState>,
    run_id: String,
) -> Result<LlmReportRunSnapshot, String> {
    let save_root = {
        let backend = backend_state.lock().map_err(|error| error.to_string())?;
        expand_user_path(backend.settings().save_root)
    };
    let snapshot = run_state
        .lock()
        .map_err(|error| error.to_string())?
        .request_cancel(&LlmRunStore::new(save_root), &run_id, &llm_now())?;
    emit_llm_run_update(&app, &snapshot);
    Ok(snapshot)
}

#[tauri::command]
async fn retry_llm_report(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    run_state: State<'_, LlmRunState>,
    run_id: String,
) -> Result<LlmReportRunSnapshot, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let (settings, api_key) = llm_settings_and_key(backend_state.inner())?;
    let store = LlmRunStore::new(expand_user_path(&settings.save_root));
    let max_iterations = clamp_llm_max_iterations(settings.llm_max_iterations);
    let (snapshot, private_request, cancellation) = {
        let mut runtime = run_state.lock().map_err(|error| error.to_string())?;
        let snapshot = runtime.retry(
            &store,
            &run_id,
            &settings.openrouter_model,
            max_iterations,
            &llm_now(),
        )?;
        let private_request = store.load(&snapshot.run_id)?.request;
        let cancellation = runtime
            .active()
            .filter(|active| active.run_id == snapshot.run_id)
            .ok_or_else(|| "Report run did not become active".to_string())?
            .cancellation;
        (snapshot, private_request, cancellation)
    };
    emit_llm_run_update(&app, &snapshot);
    spawn_registered_llm_report_run(
        app,
        run_state.inner().clone(),
        settings,
        api_key,
        snapshot.clone(),
        private_request,
        cancellation,
    );
    Ok(snapshot)
}

#[tauri::command]
async fn list_llm_report_history(
    state: State<'_, BackendState>,
) -> Result<Vec<wakenote::llm::LlmReportHistoryItem>, String> {
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        backend.settings().save_root
    };

    tauri::async_runtime::spawn_blocking(move || wakenote::llm::list_llm_report_history(&save_root))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn load_llm_report_history_detail(
    state: State<'_, BackendState>,
    report_id: String,
) -> Result<wakenote::llm::LlmReportHistoryDetail, String> {
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        backend.settings().save_root
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::llm::load_llm_report_history_detail(&save_root, &report_id)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn export_llm_report(
    state: State<'_, BackendState>,
    report_id: String,
    destination_path: String,
) -> Result<String, String> {
    let _update_activity = RESTART_GATE.long_activity()?;
    let save_root = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        backend.settings().save_root
    };

    tauri::async_runtime::spawn_blocking(move || {
        wakenote::llm::export_llm_report(&save_root, &report_id, &destination_path)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn allow_audio_playback(app: AppHandle, audio_path: String) -> Result<(), String> {
    let path = PathBuf::from(audio_path);
    let validated_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || validate_audio_playback_file(&validated_path))
        .await
        .map_err(|error| error.to_string())??;
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn upload_audio_file(
    state: State<'_, BackendState>,
    source_path: String,
) -> Result<UploadedAudio, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.upload_audio_file(source_path, chrono::Local::now())
}

#[tauri::command]
async fn analyze_audio_waveform(
    audio_path: String,
    bucket_count: Option<usize>,
) -> Result<AudioWaveform, String> {
    let path = PathBuf::from(audio_path);
    validate_audio_playback_file(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        wakenote::audio_analysis::analyze_audio_waveform(&path, bucket_count)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn enqueue_audio_file(
    app: AppHandle,
    state: State<'_, BackendState>,
    transcription_state: State<'_, AutoTranscriptionState>,
    audio_path: String,
    model_id: Option<String>,
) -> Result<QueueSnapshot, String> {
    let snapshot = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend.enqueue_audio_file(audio_path, model_id)
    };
    kick_transcription_worker_if_needed(
        app,
        state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(snapshot)
}

#[tauri::command]
fn enqueue_backlog(
    app: AppHandle,
    state: State<'_, BackendState>,
    transcription_state: State<'_, AutoTranscriptionState>,
    save_root: String,
) -> Result<QueueSnapshot, String> {
    let snapshot = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend
            .enqueue_backlog(save_root)
            .map_err(|error| error.to_string())?
    };
    kick_transcription_worker_if_needed(
        app,
        state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(snapshot)
}

#[tauri::command]
fn retry_job(
    app: AppHandle,
    state: State<'_, BackendState>,
    transcription_state: State<'_, AutoTranscriptionState>,
    id: u64,
) -> Result<QueueSnapshot, String> {
    let snapshot = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend.retry_job(id)?
    };
    kick_transcription_worker_if_needed(
        app,
        state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(snapshot)
}

#[tauri::command]
fn regenerate_transcript(
    app: AppHandle,
    state: State<'_, BackendState>,
    transcription_state: State<'_, AutoTranscriptionState>,
    audio_path: String,
    model_id: Option<String>,
) -> Result<QueueSnapshot, String> {
    let snapshot = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend.regenerate_transcript(audio_path, model_id)?
    };
    kick_transcription_worker_if_needed(
        app,
        state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(snapshot)
}

#[tauri::command]
fn reprocess_jobs(
    app: AppHandle,
    state: State<'_, BackendState>,
    transcription_state: State<'_, AutoTranscriptionState>,
    ids: Vec<u64>,
    model_id: String,
) -> Result<QueueSnapshot, String> {
    let snapshot = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend.reprocess_jobs(ids, model_id)?
    };
    kick_transcription_worker_if_needed(
        app,
        state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(snapshot)
}

#[tauri::command]
fn trash_activity_jobs(
    state: State<'_, BackendState>,
    ids: Vec<u64>,
) -> Result<TrashActivityJobsResult, String> {
    {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend.prepare_activity_jobs_for_deletion(&ids)?;
    }

    let deadline = Instant::now() + ACTIVITY_DELETE_WORKER_TIMEOUT;
    loop {
        let active = {
            let backend = state.lock().map_err(|error| error.to_string())?;
            ids.iter()
                .copied()
                .filter(|id| backend.transcription_job_is_active(*id))
                .collect::<Vec<_>>()
        };
        if active.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for cancelled transcription workers: {}",
                active
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }

    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.trash_activity_jobs_with(ids, move_recording_bundle_to_trash_and_refresh)
}

#[tauri::command]
fn trash_transcripts(
    state: State<'_, BackendState>,
    transcript_paths: Vec<String>,
) -> Result<TrashTranscriptsResult, String> {
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.trash_transcripts_with(
        transcript_paths.into_iter().map(PathBuf::from).collect(),
        wakenote::trash::move_recording_bundle_to_trash,
    )
}

fn move_recording_bundle_to_trash_and_refresh(path: &Path) -> Result<(), String> {
    let Some(bundle) = wakenote::trash::move_recording_bundle_to_trash(path)? else {
        return Ok(());
    };
    if let Err(index_error) = refresh_transcript_day_index_for_recording_path(path) {
        return match wakenote::trash::restore_recording_bundle(&bundle) {
            Ok(()) => Err(format!(
                "failed to refresh Transcript index: {index_error}; bundle restored"
            )),
            Err(restore_error) => Err(format!(
                "failed to refresh Transcript index: {index_error}; rollback incomplete: {restore_error}"
            )),
        };
    }
    Ok(())
}

#[tauri::command]
fn open_transcript_folder(path: String) -> Result<(), String> {
    let request = open_containing_folder_request(PathBuf::from(path))?;
    let mut command = Command::new(request.program);
    if request.reveal {
        command.arg("-R");
    }
    command
        .arg(request.path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn skip_job(state: State<'_, BackendState>, id: u64) -> Result<QueueSnapshot, String> {
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.skip_job(id)
}

#[tauri::command]
fn mark_all_activity_read(state: State<'_, BackendState>) -> Result<QueueSnapshot, String> {
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    Ok(backend.mark_all_activity_read())
}

#[tauri::command]
fn cancel_current_transcription(state: State<'_, BackendState>) -> Result<QueueSnapshot, String> {
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.cancel_current_transcription()
}

#[tauri::command]
fn cancel_activity_job(state: State<'_, BackendState>, id: u64) -> Result<QueueSnapshot, String> {
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.cancel_transcription_job(id)
}

#[tauri::command]
fn dictation_state(state: State<'_, DictationState>) -> Result<DictationStatePayload, String> {
    state
        .lock()
        .map(|runtime| runtime.payload(None))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_dictation(app: AppHandle) -> Result<DictationStatePayload, String> {
    let _ = cancel_dictation_runtime(&app)?;
    app.try_state::<DictationState>()
        .ok_or_else(|| "dictation runtime is unavailable".to_string())?
        .lock()
        .map(|runtime| runtime.payload(None))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_current_operation(app: AppHandle, state: State<'_, BackendState>) -> Result<(), String> {
    if cancel_dictation_runtime(&app)? {
        return Ok(());
    }
    let (settings, status) = {
        let mut backend = state.lock().map_err(|error| error.to_string())?;
        backend.cancel_current_operation()?;
        (backend.settings(), backend.app_status())
    };
    update_tray_presentation(&app, &settings, &status);
    Ok(())
}

#[tauri::command]
fn reveal_save_folder(state: State<'_, BackendState>) -> Result<(), String> {
    let request = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        reveal_save_folder_request(&backend.settings())
    };
    Command::new(request.program)
        .arg(request.path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn process_next_transcription(state: State<'_, BackendState>) -> Result<QueueSnapshot, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let mut backend = state.lock().map_err(|error| error.to_string())?;
    backend.process_next_transcription()
}

#[tauri::command]
fn transcription_cost_snapshot(
    state: State<'_, TranscriptionCostState>,
) -> Result<TranscriptionCostSnapshot, String> {
    state
        .lock()
        .map(|ledger| ledger.snapshot(Local::now()))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn transcription_cost_details(
    state: State<'_, TranscriptionCostState>,
) -> Result<TranscriptionCostDetails, String> {
    state
        .lock()
        .map(|ledger| ledger.details())
        .map_err(|error| error.to_string())
}

fn upsert_transcription_cost(app: &AppHandle, entry: TranscriptionCostEntry) {
    let Some(state) = app.try_state::<TranscriptionCostState>() else {
        return;
    };
    let snapshot = state.lock().ok().and_then(|mut ledger| {
        ledger
            .upsert(entry)
            .map_err(|error| {
                eprintln!("[wakenote] WARN transcription cost ledger update failed: {error}");
            })
            .ok()?;
        Some(ledger.snapshot(Local::now()))
    });
    if let Some(snapshot) = snapshot {
        let _ = app.emit(EVENT_TRANSCRIPTION_COST_UPDATED, snapshot);
    }
}

fn record_job_transcription_cost(app: &AppHandle, outcome: &TranscriptionJobOutcome) {
    let Some(usage) = outcome.usage.as_ref() else {
        return;
    };
    let Some(provider) = usage.provider.as_ref() else {
        return;
    };
    let Some(model_id) = outcome.effective_model_id.as_ref() else {
        return;
    };
    upsert_transcription_cost(
        app,
        TranscriptionCostEntry {
            source_id: format!("transcription-job:{}", outcome.id),
            recorded_at: Utc::now(),
            provider: provider.clone(),
            model_id: model_id.clone(),
            audio_duration_ms: usage.audio_duration_ms,
            estimated_cost_usd: usage.provider_cost_usd,
            request_count: 1,
            unpriced_request_count: u64::from(usage.provider_cost_usd.is_none()),
        },
    );
}

fn record_meeting_transcription_cost(app: &AppHandle, save_root: &Path, id: &str) {
    let Ok(detail) = wakenote::meeting::meeting_detail(save_root, id) else {
        return;
    };
    let record = detail.record;
    if record.api_request_count == 0 {
        return;
    }
    let provider = if record.model_id.starts_with("openai-") {
        "OpenAI"
    } else if record.model_id.starts_with("openrouter-") {
        "OpenRouter"
    } else if record.model_id.starts_with("soniox-") {
        "Soniox"
    } else {
        return;
    };
    upsert_transcription_cost(
        app,
        TranscriptionCostEntry {
            source_id: format!("meeting:{}", record.id),
            recorded_at: record.updated_at,
            provider: provider.to_string(),
            model_id: record.model_id,
            audio_duration_ms: record.api_audio_duration_ms,
            estimated_cost_usd: (record.api_cost_microusd > 0
                || record.api_unpriced_request_count == 0)
                .then_some(record.api_cost_microusd as f64 / 1_000_000.0),
            request_count: record.api_request_count,
            unpriced_request_count: record.api_unpriced_request_count,
        },
    );
}

// --- Manual Meeting Mode -------------------------------------------------

fn emit_manual_meeting_status(app: &AppHandle, status: &ManualMeetingRecordingStatus) {
    if let Err(error) = app.emit(EVENT_MANUAL_MEETING_RECORDING_STATE, status.clone()) {
        eprintln!("[wakenote] WARN emit manual meeting state failed: {error}");
    }
}

fn finalize_manual_meeting_recording(
    app: &AppHandle,
    state: &ManualMeetingRecordingState,
    generation: u64,
    reason: ManualMeetingStopReason,
) -> Result<ManualMeetingRecordingStatus, String> {
    let _update_activity = RESTART_GATE.long_activity()?;
    let mut session = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        if runtime.session.as_ref().map(|session| session.generation) != Some(generation) {
            return Ok(runtime.status(Instant::now()));
        }
        runtime.session.take().expect("generation matched session")
    };
    session.microphone.stop();
    session.system.stop();
    let result = session
        .writer
        .take()
        .ok_or_else(|| "manual meeting writer is unavailable".to_string())?
        .finish();
    let status = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        runtime.last_meeting_id = Some(session.meeting_id);
        match result {
            Ok(record) => {
                runtime.last_meeting_id = Some(record.id);
                runtime.last_stop_reason = Some(reason);
                runtime.error = None;
            }
            Err(error) => {
                runtime.last_stop_reason = Some(ManualMeetingStopReason::Error);
                runtime.error = Some(error);
            }
        }
        runtime.status(Instant::now())
    };
    emit_manual_meeting_status(app, &status);
    Ok(status)
}

#[tauri::command]
fn manual_meeting_recording_status(
    app: AppHandle,
    state: State<'_, ManualMeetingRecordingState>,
) -> Result<ManualMeetingRecordingStatus, String> {
    let state = state.inner().clone();
    let overflow_generation = state.lock().ok().and_then(|runtime| {
        runtime.session.as_ref().and_then(|session| {
            session
                .writer
                .as_ref()
                .is_some_and(ManualMeetingWriter::overflowed)
                .then_some(session.generation)
        })
    });
    if let Some(generation) = overflow_generation {
        return finalize_manual_meeting_recording(
            &app,
            &state,
            generation,
            ManualMeetingStopReason::Error,
        );
    }
    state
        .lock()
        .map(|runtime| runtime.status(Instant::now()))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn start_manual_meeting_recording(
    app: AppHandle,
    state: State<'_, ManualMeetingRecordingState>,
    backend_state: State<'_, BackendState>,
) -> Result<ManualMeetingRecordingStatus, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let permission_snapshot = permissions::permission_snapshot();
    if permission_snapshot.microphone.status != permissions::PermissionGrantStatus::Granted {
        return Err("Microphone permission is required for Meeting Mode".to_string());
    }
    if permission_snapshot.screen_recording.status != permissions::PermissionGrantStatus::Granted {
        return Err("Screen & System Audio permission is required for Meeting Mode".to_string());
    }
    let settings = backend_state
        .lock()
        .map_err(|error| error.to_string())?
        .settings();
    let state = state.inner().clone();
    let generation = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        if runtime.session.is_some() {
            return Err("Meeting Mode is already recording".to_string());
        }
        runtime.generation = runtime.generation.saturating_add(1);
        runtime.error = None;
        runtime.last_stop_reason = None;
        runtime.generation
    };
    let configured = settings
        .capture_microphones
        .first()
        .cloned()
        .unwrap_or_else(|| wakenote::settings::CaptureMicrophoneEntry {
            id: settings.selected_microphone.clone(),
            label: settings.selected_microphone_label.clone(),
            core_audio_uid: None,
        });
    let resolved = CpalAudioInput::resolve_device_with_uid(
        &configured.id,
        Some(&configured.label),
        configured.core_audio_uid.as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let configured_microphone = CaptureMicrophoneEntry {
        id: resolved.device_id.clone(),
        label: resolved.device_name.clone(),
        core_audio_uid: resolved.core_audio_uid.clone(),
    };
    let microphone_rate = resolved.sample_rate;
    let started_at = Utc::now();
    let title = format!(
        "Meeting {}",
        started_at.with_timezone(&Local).format("%Y-%m-%d %H:%M")
    );
    let recorder = start_manual_recorded_meeting_capture(
        &expand_user_path(&settings.save_root),
        &title,
        "Microphone + System Audio",
        settings.effective_meeting_model(),
        settings.transcription_language,
        env!("CARGO_PKG_VERSION"),
        MANUAL_MEETING_SAMPLE_RATE,
        settings.audio_bitrate_kbps,
        started_at.with_timezone(&Local),
    )?;
    let meeting_id = recorder.id().to_string();
    let writer = ManualMeetingWriter::start(recorder, started_at);
    let microphone_sender = writer.sender(ManualMeetingSource::Microphone);
    let system_sender = writer.sender(ManualMeetingSource::System);
    let voice_leveling_policy = app
        .try_state::<VoiceLevelingPolicy>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "Voice-aware Auto Level policy is unavailable".to_string())?;
    let hardware_control = app
        .try_state::<MicrophoneLevelState>()
        .map(|state| state.inner().clone())
        .and_then(|service| {
            service
                .level_for(&configured_microphone)
                .ok()
                .filter(|level| level.writable && level.volume_percent.is_some())
                .map(|_| ServiceHardwareLevelControl::new(service, configured_microphone.clone()))
        });
    let processor = VoiceAwareMicrophoneProcessor::new(
        microphone_rate,
        voice_leveling_policy,
        hardware_control,
    );

    let mut microphone = LiveCaptureRuntime::new(CpalAudioInput);
    if let Err(error) = microphone.start_processed(
        AudioInputConfig {
            device_id: resolved.device_id,
            sample_rate: Some(microphone_rate),
            label_hint: Some(resolved.device_name),
            core_audio_uid: resolved.core_audio_uid,
        },
        processor,
        move |frame| microphone_sender.try_send(frame, microphone_rate),
    ) {
        let _ = writer.abort();
        return Err(error.to_string());
    }
    let mut system = LiveCaptureRuntime::new(SystemAudioInput::new());
    if let Err(error) = system.start(
        AudioInputConfig {
            device_id: "system".to_string(),
            sample_rate: Some(MANUAL_MEETING_SAMPLE_RATE),
            label_hint: None,
            core_audio_uid: None,
        },
        move |frame| system_sender.try_send(frame, MANUAL_MEETING_SAMPLE_RATE),
    ) {
        microphone.stop();
        let _ = writer.abort();
        return Err(error.to_string());
    }

    let status = {
        let mut runtime = state.lock().map_err(|error| error.to_string())?;
        runtime.session = Some(ManualMeetingSession {
            generation,
            meeting_id,
            started_at,
            started_instant: Instant::now(),
            microphone,
            system,
            writer: Some(writer),
        });
        runtime.status(Instant::now())
    };
    emit_manual_meeting_status(&app, &status);
    let deadline_app = app.clone();
    let deadline_state = state.clone();
    thread::spawn(move || {
        thread::sleep(MAX_MANUAL_MEETING_DURATION);
        let _ = finalize_manual_meeting_recording(
            &deadline_app,
            &deadline_state,
            generation,
            ManualMeetingStopReason::MaximumDuration,
        );
    });
    Ok(status)
}

#[tauri::command]
fn stop_manual_meeting_recording(
    app: AppHandle,
    state: State<'_, ManualMeetingRecordingState>,
) -> Result<ManualMeetingRecordingStatus, String> {
    let state = state.inner().clone();
    let generation = state
        .lock()
        .map_err(|error| error.to_string())?
        .session
        .as_ref()
        .map(|session| session.generation);
    match generation {
        Some(generation) => finalize_manual_meeting_recording(
            &app,
            &state,
            generation,
            ManualMeetingStopReason::Manual,
        ),
        None => state
            .lock()
            .map(|runtime| runtime.status(Instant::now()))
            .map_err(|error| error.to_string()),
    }
}

// --- Long-form meeting transcription -------------------------------------

/// Build the sink that forwards meeting-pipeline events to the frontend.
fn meeting_event_emitter(app: AppHandle) -> wakenote::meeting::MeetingEventCallback {
    Arc::new(move |event: MeetingEvent| match event {
        MeetingEvent::Progress(payload) => {
            let _ = app.emit(EVENT_MEETING_PROGRESS, payload);
        }
        MeetingEvent::SegmentCommitted(payload) => {
            let _ = app.emit(EVENT_MEETING_SEGMENT, payload);
        }
        MeetingEvent::Finished(payload) => {
            let _ = app.emit(EVENT_MEETING_FINISHED, payload);
        }
    })
}

fn meeting_save_root(state: &State<'_, BackendState>) -> Result<PathBuf, String> {
    let backend = state.lock().map_err(|error| error.to_string())?;
    Ok(expand_user_path(backend.settings().save_root))
}

fn schedule_and_spawn_meeting_job(
    app: AppHandle,
    meeting_state: MeetingState,
    job: MeetingJobSpec,
    queue_if_busy: bool,
) -> Result<MeetingJobScheduleOutcome, String> {
    let (outcome, cancel) = {
        let mut runtime = meeting_state.lock().map_err(|error| error.to_string())?;
        schedule_meeting_job(&mut runtime, job.clone(), queue_if_busy)?
    };
    let Some(cancel) = cancel else {
        return Ok(outcome);
    };

    let meeting_state_for_thread = meeting_state.clone();
    thread::spawn(move || {
        let mut current_job = job;
        let mut current_cancel = cancel;
        loop {
            let emit = meeting_event_emitter(app.clone());
            let credentials = app
                .try_state::<BackendState>()
                .and_then(|state| {
                    state.lock().ok().and_then(|backend| {
                        backend
                            .transcription_credentials()
                            .map_err(|error| {
                                eprintln!("[wakenote] meeting credentials unavailable: {error}");
                            })
                            .ok()
                    })
                })
                .unwrap_or_default();
            let realtime_replay = meeting_realtime_replay(&app);
            if let Err(error) = wakenote::meeting::run_meeting_job(
                &current_job.save_root,
                &current_job.model_directory,
                &current_job.id,
                current_job.suppress_low_confidence,
                &current_job.dictionary,
                credentials,
                realtime_replay,
                current_cancel,
                emit,
            ) {
                eprintln!("[wakenote] meeting job {} error: {error}", current_job.id);
            }
            record_meeting_transcription_cost(&app, &current_job.save_root, &current_job.id);

            let next = meeting_state_for_thread
                .lock()
                .ok()
                .and_then(|mut runtime| complete_meeting_job(&mut runtime, &current_job.id));
            let Some((next_job, next_cancel)) = next else {
                break;
            };
            append_debug_log(
                &next_job.save_root,
                format!(
                    "[meeting-capture] state=transcribing id={} resumed_from_queue=true",
                    next_job.id
                ),
            );
            current_job = next_job;
            current_cancel = next_cancel;
        }
    });
    Ok(outcome)
}

fn meeting_realtime_replay(app: &AppHandle) -> Option<MeetingRealtimeReplay> {
    let openai = app
        .try_state::<OpenAiRealtimeManager>()
        .map(|state| state.inner().clone());
    let soniox = app
        .try_state::<SonioxRealtimeManager>()
        .map(|state| state.inner().clone());
    if openai.is_none() && soniox.is_none() {
        return None;
    }
    let openai_for_submit = openai.clone();
    let soniox_for_submit = soniox.clone();
    let submit = Arc::new(move |request: MeetingRealtimeReplayRequest| {
        let source_key = request.source_key.clone();
        let chunk_id = request.chunk_id;
        let audio_path = request.audio_path.clone();
        let samples_request = RealtimeSamplesRequest {
            source_key: request.source_key,
            source_label: request.source_label,
            microphone_slot: None,
            chunk_id,
            model_id: request.model_id.clone(),
            language: request.language,
            language_hints: request.language_hints,
            dictionary: request.dictionary,
            sample_rate: request.sample_rate,
            samples: request.samples,
            credentials: request.credentials,
        };
        match request.model_id.as_str() {
            "openai-gpt-live-transcribe" => {
                let manager = openai_for_submit
                    .as_ref()
                    .ok_or_else(|| "OpenAI realtime manager is unavailable".to_string())?;
                manager.submit_recorded_samples(samples_request);
                manager.commit(source_key, chunk_id, audio_path);
            }
            "soniox-realtime-v5" => {
                let manager = soniox_for_submit
                    .as_ref()
                    .ok_or_else(|| "Soniox realtime manager is unavailable".to_string())?;
                manager.submit_recorded_samples(samples_request);
                manager.commit(source_key, chunk_id, audio_path);
            }
            _ => return Err("selected model is not a realtime Meeting model".to_string()),
        }
        Ok(())
    });
    let close = Arc::new(move |source_key: String| {
        if let Some(manager) = openai.as_ref() {
            manager.close_source(source_key.clone());
        }
        if let Some(manager) = soniox.as_ref() {
            manager.close_source(source_key);
        }
    });
    Some(MeetingRealtimeReplay::new(submit, close))
}

/// Schedule one saved meeting on the shared worker, optionally behind the
/// current job. A started job owns the worker thread that drains queued work.
fn schedule_saved_meeting_job(
    app: AppHandle,
    backend_state: BackendState,
    meeting_state: MeetingState,
    id: String,
    queue_if_busy: bool,
) -> Result<MeetingJobScheduleOutcome, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let settings = {
        let backend = backend_state.lock().map_err(|error| error.to_string())?;
        backend.settings()
    };
    let job = meeting_job_spec_for_saved_record(id, &settings)?;
    schedule_and_spawn_meeting_job(app, meeting_state, job, queue_if_busy)
}

fn meeting_job_spec_for_saved_record(
    id: String,
    settings: &AppSettings,
) -> Result<MeetingJobSpec, String> {
    let save_root = expand_user_path(&settings.save_root);
    let detail = wakenote::meeting::meeting_detail(&save_root, &id)?;
    Ok(MeetingJobSpec {
        id,
        save_root,
        model_directory: expand_user_path(&settings.model_directory),
        model_id: detail.record.model_id,
        suppress_low_confidence: settings.suppress_low_confidence_transcripts,
        dictionary: DictionaryContext::from_settings(settings),
    })
}

#[tauri::command]
fn list_meetings(state: State<'_, BackendState>) -> Result<Vec<MeetingSummary>, String> {
    Ok(wakenote::meeting::list_meetings(&meeting_save_root(
        &state,
    )?))
}

#[tauri::command]
fn import_meeting_recording(
    state: State<'_, BackendState>,
    source_path: String,
) -> Result<MeetingSummary, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let (save_root, model_id, language) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        let settings = backend.settings();
        (
            expand_user_path(&settings.save_root),
            settings.effective_meeting_model().to_string(),
            settings.transcription_language,
        )
    };
    let record = wakenote::meeting::import_meeting(
        &save_root,
        Path::new(&source_path),
        &model_id,
        language,
        env!("CARGO_PKG_VERSION"),
        chrono::Local::now(),
    )?;
    Ok(record.summary())
}

#[tauri::command]
fn meeting_detail(state: State<'_, BackendState>, id: String) -> Result<MeetingDetail, String> {
    wakenote::meeting::meeting_detail(&meeting_save_root(&state)?, &id)
}

fn validate_meeting_transcription_request(
    model: &ModelDescriptor,
    model_directory: &Path,
    request: &StartMeetingTranscriptionRequest,
) -> Result<MeetingTranscriptionRequest, String> {
    if !model_supports_context(model, TranscriptionContext::Meeting, false)
        || !(model.capabilities.file_transcription || model.capabilities.realtime)
    {
        return Err("selected model does not support saved Meeting transcription".to_string());
    }
    let runnable = match model.status {
        ModelStatus::Installed | ModelStatus::Ready => true,
        ModelStatus::Unloaded if model.offline => ModelStore::new(model_directory)
            .verify_model(model)
            .is_ok_and(|status| matches!(status, ModelStatus::Installed | ModelStatus::Ready)),
        ModelStatus::Unloaded => true,
        ModelStatus::Missing
        | ModelStatus::Downloading
        | ModelStatus::Verifying
        | ModelStatus::Extracting
        | ModelStatus::Error => false,
    };
    if !runnable {
        return Err("selected Meeting model is not currently runnable".to_string());
    }
    if request.speaker_separation_enabled && !model.capabilities.diarization {
        return Err("selected model does not support speaker separation".to_string());
    }
    let streaming_enabled = match model.capabilities.streaming {
        StreamingCapability::Required => true,
        StreamingCapability::Optional => request.streaming_enabled,
        StreamingCapability::Unsupported => false,
    };
    Ok(MeetingTranscriptionRequest {
        model_id: model.id.clone(),
        language: request.language,
        language_hints: wakenote::settings::normalize_language_hints(
            &request
                .language_hints
                .clone()
                .unwrap_or_else(wakenote::settings::default_transcription_language_hints),
        ),
        streaming_enabled,
        speaker_separation_enabled: request.speaker_separation_enabled,
    })
}

#[tauri::command]
fn start_meeting_transcription(
    app: AppHandle,
    state: State<'_, BackendState>,
    meeting_state: State<'_, MeetingState>,
    id: String,
    mut request: StartMeetingTranscriptionRequest,
) -> Result<MeetingSummary, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let (save_root, model_directory, model) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        let settings = backend.settings();
        if request.language_hints.is_none() {
            request.language_hints = Some(settings.transcription_language_hints.clone());
        }
        let model = backend
            .model_registry()
            .into_iter()
            .find(|model| model.id == request.model_id)
            .ok_or_else(|| "selected meeting transcription model was not found".to_string())?;
        (
            expand_user_path(&settings.save_root),
            expand_user_path(&settings.model_directory),
            model,
        )
    };
    let transcription_request =
        validate_meeting_transcription_request(&model, &model_directory, &request)?;
    let record = start_recorded_meeting_transcription(&save_root, &id, transcription_request)?;
    let outcome = schedule_saved_meeting_job(
        app,
        state.inner().clone(),
        meeting_state.inner().clone(),
        record.id.clone(),
        true,
    )?;
    if outcome == MeetingJobScheduleOutcome::AlreadyScheduled {
        return Err("meeting job is already scheduled".to_string());
    }
    Ok(record.summary())
}

#[tauri::command]
fn cancel_meeting(
    app: AppHandle,
    meeting_state: State<'_, MeetingState>,
    id: String,
) -> Result<(), String> {
    let canceled = {
        let mut runtime = meeting_state.lock().map_err(|error| error.to_string())?;
        if runtime.current.as_deref() == Some(id.as_str()) {
            if let Some(cancel) = runtime.cancel.as_ref() {
                cancel.store(true, Ordering::Release);
            }
            return Ok(());
        }
        let Some(index) = runtime.queued.iter().position(|job| job.id == id) else {
            return Ok(());
        };
        let job = runtime.queued[index].clone();
        let record = cancel_pending_meeting(&job.save_root, &id)?;
        runtime.queued.remove(index);
        MeetingFinishedEvent {
            id: record.id,
            status: record.status,
            error: None,
        }
    };
    let _ = app.emit(EVENT_MEETING_FINISHED, canceled);
    Ok(())
}

#[tauri::command]
fn resume_meeting(
    app: AppHandle,
    state: State<'_, BackendState>,
    meeting_state: State<'_, MeetingState>,
    id: String,
) -> Result<MeetingSummary, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let save_root = meeting_save_root(&state)?;
    let outcome = schedule_saved_meeting_job(
        app,
        state.inner().clone(),
        meeting_state.inner().clone(),
        id.clone(),
        false,
    )?;
    if outcome != MeetingJobScheduleOutcome::Started {
        return Err("meeting job could not be started".to_string());
    }
    let detail = wakenote::meeting::meeting_detail(&save_root, &id)?;
    Ok(detail.record.summary())
}

#[tauri::command]
fn start_live_capture(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    live_state: State<'_, LiveCaptureState>,
    transcription_state: State<'_, AutoTranscriptionState>,
) -> Result<AppStatus, String> {
    let status = start_live_capture_runtime(
        &app,
        backend_state.inner(),
        live_state.inner(),
        transcription_state.inner().clone(),
    )
    .map_err(|error| error.to_string())?;
    if status.live_input_active {
        promote_source_meeting_microphone_lease(&app);
    }
    Ok(status)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveCaptureStartError {
    MicrophonePermission(PermissionGrantStatus),
    Runtime(String),
}

impl std::fmt::Display for LiveCaptureStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MicrophonePermission(status) => write!(
                formatter,
                "Microphone permission is required to start Live Input (status: {status:?}). Open WakeNote Settings > Audio to continue."
            ),
            Self::Runtime(error) => formatter.write_str(error),
        }
    }
}

impl From<String> for LiveCaptureStartError {
    fn from(error: String) -> Self {
        Self::Runtime(error)
    }
}

fn live_capture_permission_result(
    status: PermissionGrantStatus,
) -> Result<(), LiveCaptureStartError> {
    if status == PermissionGrantStatus::Granted {
        Ok(())
    } else {
        Err(LiveCaptureStartError::MicrophonePermission(status))
    }
}

fn launch_auto_start_should_retry(error: &LiveCaptureStartError) -> bool {
    !matches!(error, LiveCaptureStartError::MicrophonePermission(_))
}

fn start_live_capture_runtime(
    app: &AppHandle,
    backend_state: &BackendState,
    live_state: &LiveCaptureState,
    transcription_state: AutoTranscriptionState,
) -> Result<AppStatus, LiveCaptureStartError> {
    let _update_activity = RESTART_GATE
        .capture_activity()
        .map_err(LiveCaptureStartError::Runtime)?;
    let settings = {
        let backend = backend_state
            .lock()
            .map_err(|error| LiveCaptureStartError::Runtime(error.to_string()))?;
        let settings = backend.settings();
        if settings.pause_all || !settings.recording_enabled {
            return Ok(backend.app_status());
        }
        settings
    };
    live_capture_permission_result(permissions::permission_snapshot().microphone.status)?;

    let desired_slots = settings
        .capture_microphones
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| MicrophoneSlot::from_index(index).map(|slot| (slot, entry)))
        .collect::<Vec<_>>();
    let active_slots = live_state
        .lock()
        .map_err(|error| LiveCaptureStartError::Runtime(error.to_string()))?
        .active_slots();
    for slot in active_slots {
        let keep = desired_slots.iter().any(|(desired_slot, entry)| {
            *desired_slot == slot
                && backend_state
                    .lock()
                    .map(|backend| backend.capture_slot_matches(slot, &entry.id, &entry.label))
                    .unwrap_or(false)
        });
        if !keep {
            stop_live_capture_slot_runtime(backend_state, live_state, slot)?;
        }
    }

    for (slot, entry) in desired_slots {
        let healthy = live_state
            .lock()
            .map_err(|error| LiveCaptureStartError::Runtime(error.to_string()))?
            .diagnostic(slot)
            .is_some_and(|diagnostic| diagnostic.running && diagnostic.runtime_error.is_none());
        if healthy {
            continue;
        }
        if live_state
            .lock()
            .map_err(|error| LiveCaptureStartError::Runtime(error.to_string()))?
            .is_running(slot)
        {
            stop_live_capture_slot_runtime(backend_state, live_state, slot)?;
        }
        if let Err(error) = start_live_capture_slot_runtime(
            app,
            backend_state,
            live_state,
            transcription_state.clone(),
            slot,
            entry.clone(),
        ) {
            let mut backend = backend_state
                .lock()
                .map_err(|error| LiveCaptureStartError::Runtime(error.to_string()))?;
            backend.capture_slot_start_failed(
                slot,
                format!("{} microphone unavailable: {error}", slot.as_str()),
            );
        }
    }
    apply_input_monitor_settings(app, backend_state)?;

    // Warm the live-transcription model now that capture is live. whisper
    // (esp. medium, 1.5 GB) takes 5-15 s to load; preloading here means the
    // first utterance produces captions immediately instead of being dropped
    // while the model loads. Best-effort — a failure just falls back to the
    // original lazy load on the first partial request.
    let preview_model = backend_state.lock().ok().and_then(|backend| {
        let mut settings = backend.settings();
        settings.selected_model = settings.effective_auto_type_model().to_string();
        let models = backend.model_registry();
        live_preview_model_id(&settings, &models, &settings.model_directory)
            .filter(|model_id| model_supports_live_partials(&settings.model_directory, model_id))
    });
    if let (Some(transcriber_state), Some(model_id)) =
        (app.try_state::<LiveTranscriberState>(), preview_model)
        && let Ok(slot) = transcriber_state.lock()
        && let Some(service) = slot.as_ref()
    {
        service.preload(model_id);
    }

    let (dropped_frames, runtime_error) = live_state
        .lock()
        .map(|live_capture| {
            (
                live_capture.total_dropped_frame_count(),
                live_capture.first_runtime_error(),
            )
        })
        .unwrap_or((0, None));
    let backend = backend_state
        .lock()
        .map_err(|error| LiveCaptureStartError::Runtime(error.to_string()))?;
    let runtime_error = (!backend.app_status().live_input_active)
        .then_some(runtime_error)
        .flatten();
    Ok(with_live_runtime_warning(
        backend.app_status(),
        dropped_frames,
        runtime_error,
    ))
}

fn start_live_capture_slot_runtime(
    app: &AppHandle,
    backend_state: &BackendState,
    live_state: &LiveCaptureState,
    transcription_state: AutoTranscriptionState,
    slot: MicrophoneSlot,
    requested_device: CaptureMicrophoneEntry,
) -> Result<(), String> {
    let _update_activity = RESTART_GATE.capture_activity()?;
    let CaptureMicrophoneEntry {
        id: requested_device_id,
        label: requested_label,
        core_audio_uid: requested_core_audio_uid,
    } = requested_device;
    let requested_label_hint = Some(requested_label.clone()).filter(|label| !label.is_empty());
    let resolved = resolve_capture_device_with_timeout(
        requested_device_id.clone(),
        requested_label_hint.clone(),
        requested_core_audio_uid.clone(),
    )
    .map_err(|error| error.to_string())?;
    let configured_microphone = CaptureMicrophoneEntry {
        id: resolved.device_id.clone(),
        label: resolved.device_name.clone(),
        core_audio_uid: resolved.core_audio_uid.clone(),
    };
    if requested_device_id != "default" && resolved.used_fallback_device {
        return Err(format!(
            "{} is disconnected; waiting for the same device",
            requested_label
        ));
    }

    let capture_config = AudioInputConfig {
        device_id: resolved.device_id.clone(),
        sample_rate: Some(resolved.sample_rate),
        label_hint: requested_label_hint.clone(),
        core_audio_uid: resolved.core_audio_uid.clone(),
    };
    let shared_sample_rate = live_state
        .lock()
        .map_err(|error| error.to_string())?
        .open_device_sample_rate(&capture_config)
        .filter(|sample_rate| *sample_rate > 0);
    let sample_rate = shared_sample_rate.unwrap_or(resolved.sample_rate);
    let shares_open_device = shared_sample_rate.is_some();
    let capture_config = AudioInputConfig {
        sample_rate: Some(sample_rate),
        ..capture_config
    };

    let (handler, events) = {
        let mut backend = backend_state.lock().map_err(|error| error.to_string())?;
        backend.reconcile_capture_microphone_device_id(
            slot,
            &requested_device_id,
            &resolved.device_id,
            &resolved.device_name,
        );
        backend.start_capture_session_for_slot(
            slot,
            sample_rate,
            chrono::Utc::now(),
            resolved.device_id.clone(),
            resolved.device_name,
            false,
        )?;
        backend.clear_microphone_slot_warning(slot);
        live_events_for_dispatch(&mut backend)
    };
    dispatch_live_events(handler, events);

    let callback_backend = Arc::clone(backend_state);
    let callback_transcription = transcription_state.clone();
    let callback_app = app.clone();
    let callback_input_monitor = (slot == MicrophoneSlot::Primary)
        .then(|| {
            app.try_state::<InputMonitorState>()
                .map(|state| state.inner().clone())
        })
        .flatten();
    let callback_overlay_level_throttle = Arc::new(Mutex::new(
        Instant::now()
            .checked_sub(Duration::from_millis(100))
            .unwrap_or_else(Instant::now),
    ));
    let voice_leveling_policy = app
        .try_state::<VoiceLevelingPolicy>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "Voice-aware Auto Level policy is unavailable".to_string())?;
    let warning_backend = Arc::clone(backend_state);
    let frame_callback = move |frame: AudioFrame| {
        if let Some(input_monitor) = callback_input_monitor.as_ref()
            && let Ok(monitor) = input_monitor.try_lock()
        {
            monitor.feed(&frame.samples);
        }
        let waveform_levels = overlay::waveform_levels_from_samples(
            &frame.samples,
            overlay::OVERLAY_WAVEFORM_BAR_COUNT,
        );
        let (should_kick, handler, events, emit_waveform, tray_status) =
            if let Ok(mut backend) = callback_backend.lock() {
                let status = backend.process_audio_frame_for_slot(slot, frame);
                let emit_waveform = slot == MicrophoneSlot::Primary
                    && status
                        .as_ref()
                        .map(|status| matches!(status.tray_state, TrayState::Recording))
                        .unwrap_or(false);
                let should_kick = status
                    .map(|_| backend.should_process_transcriptions())
                    .unwrap_or(false);
                let tray_status = Some((backend.settings(), backend.app_status()));
                let (handler, events) = live_events_for_dispatch(&mut backend);
                (should_kick, handler, events, emit_waveform, tray_status)
            } else {
                (false, None, Vec::new(), false, None)
            };
        dispatch_live_events(handler, events);
        if let Some((settings, status)) = tray_status {
            update_tray_presentation(&callback_app, &settings, &status);
        }
        if emit_waveform && overlay_level_emit_due(&callback_overlay_level_throttle) {
            overlay::emit_waveform_levels(&callback_app, waveform_levels);
        }
        if should_kick {
            kick_transcription_worker(
                callback_app.clone(),
                callback_backend.clone(),
                callback_transcription.clone(),
            );
        }
    };
    let start_result = if shares_open_device {
        live_state
            .lock()
            .map_err(|error| error.to_string())?
            .attach_slot(slot, &capture_config, frame_callback)
    } else {
        let hardware_control = app
            .try_state::<MicrophoneLevelState>()
            .map(|state| state.inner().clone())
            .and_then(|service| {
                service
                    .level_for(&configured_microphone)
                    .ok()
                    .filter(|level| level.writable && level.volume_percent.is_some())
                    .map(|_| ServiceHardwareLevelControl::new(service, configured_microphone))
            });
        let processor = VoiceAwareMicrophoneProcessor::new(
            sample_rate,
            voice_leveling_policy,
            hardware_control,
        );
        live_state
            .lock()
            .map_err(|error| error.to_string())?
            .start_slot_processed_with_warnings(
                slot,
                capture_config,
                processor,
                frame_callback,
                move |warning| {
                    if let Ok(mut backend) = warning_backend.lock() {
                        backend.set_microphone_slot_warning(slot, warning);
                    }
                },
            )
    };
    if let Err(error) = start_result {
        let mut backend = backend_state.lock().map_err(|lock| lock.to_string())?;
        backend.stop_capture_slot(slot)?;
        return Err(format!("capture failed: {error}"));
    }
    Ok(())
}

fn stop_live_capture_slot_runtime(
    backend_state: &BackendState,
    live_state: &LiveCaptureState,
    slot: MicrophoneSlot,
) -> Result<(), String> {
    let _update_activity = RESTART_GATE.activity()?;
    live_state
        .lock()
        .map_err(|error| error.to_string())?
        .stop_slot(slot);
    let (handler, events) = {
        let mut backend = backend_state.lock().map_err(|error| error.to_string())?;
        backend.stop_capture_slot(slot)?;
        live_events_for_dispatch(&mut backend)
    };
    dispatch_live_events(handler, events);
    Ok(())
}

#[tauri::command]
fn stop_live_capture(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    live_state: State<'_, LiveCaptureState>,
    transcription_state: State<'_, AutoTranscriptionState>,
) -> Result<AppStatus, String> {
    let status = stop_live_capture_runtime(&app, backend_state.inner(), live_state.inner())?;
    note_source_meeting_microphone_stopped(&app, backend_state.inner());
    kick_transcription_worker_if_needed(
        app,
        backend_state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(status)
}

fn stop_live_capture_runtime(
    app: &AppHandle,
    backend_state: &BackendState,
    live_state: &LiveCaptureState,
) -> Result<AppStatus, String> {
    let _update_activity = RESTART_GATE.activity()?;
    live_state
        .lock()
        .map_err(|error| error.to_string())?
        .stop_all();
    let (status, handler, events) = {
        let mut backend = backend_state.lock().map_err(|error| error.to_string())?;
        let status = backend.stop_capture_session()?;
        let (handler, events) = live_events_for_dispatch(&mut backend);
        (status, handler, events)
    };
    apply_input_monitor_settings(app, backend_state)?;
    dispatch_live_events(handler, events);
    let queue_idle = backend_state
        .lock()
        .map(|backend| {
            let snapshot = backend.queue_snapshot();
            snapshot.running_count == 0 && snapshot.pending_count == 0
        })
        .unwrap_or(true);
    if queue_idle && let Err(error) = overlay::hide_overlay_on_main_thread(app, "hide") {
        eprintln!("[overlay] hide failed: {error}");
    }
    Ok(status)
}

fn source_uses_automatic_meeting_capture(source_id: &str) -> bool {
    matches!(source_id, "meet" | "zoom")
}

fn source_meeting_microphone_should_stop(lease: &SourceMeetingMicrophoneLease) -> bool {
    lease.auto_started && !lease.user_owned
}

fn primary_live_capture_is_healthy(live_state: &LiveCaptureState) -> bool {
    live_state
        .lock()
        .ok()
        .and_then(|runtime| runtime.diagnostic(MicrophoneSlot::Primary))
        .is_some_and(|diagnostic| diagnostic.running && diagnostic.runtime_error.is_none())
}

fn ensure_source_meeting_microphone(
    app: &AppHandle,
    source_id: &str,
    backend_state: &BackendState,
    transcription_state: AutoTranscriptionState,
) {
    if !source_uses_automatic_meeting_capture(source_id) {
        return;
    }
    let Some(live_state) = app.try_state::<LiveCaptureState>() else {
        return;
    };
    let generation = backend_state
        .lock()
        .ok()
        .and_then(|backend| backend.active_system_meeting_generation())
        .unwrap_or_default();
    if generation == 0 {
        return;
    }

    let mut was_running = primary_live_capture_is_healthy(live_state.inner());
    if !was_running
        && (RESTART_GATE.is_capture_suspended() || RESTART_GATE.is_capture_restore_pending())
    {
        was_running = wait_for_capture_after_update_pause(live_state.inner());
    }
    let mut auto_started = false;
    if !was_running {
        match start_live_capture_runtime(
            app,
            backend_state,
            live_state.inner(),
            transcription_state,
        ) {
            Ok(_) => {
                auto_started = primary_live_capture_is_healthy(live_state.inner());
            }
            Err(error) => {
                append_runtime_debug_log(
                    &backend_state
                        .lock()
                        .map(|backend| backend.settings())
                        .unwrap_or_default(),
                    format!(
                        "[meeting-capture] microphone_unavailable source_id={source_id} error={error}"
                    ),
                );
            }
        }
    }
    let microphone_available = primary_live_capture_is_healthy(live_state.inner());
    if !microphone_available && let Ok(mut backend) = backend_state.lock() {
        backend.set_system_meeting_capture_warning(
            "Microphone audio was unavailable; this Meeting contains system audio only",
        );
    }
    if let Some(lease_state) = app.try_state::<SourceMeetingMicrophoneLeaseState>()
        && let Ok(mut lease) = lease_state.lock()
    {
        *lease = Some(SourceMeetingMicrophoneLease {
            source_id: source_id.to_string(),
            generation,
            auto_started,
            user_owned: was_running,
        });
    }
}

/// A meeting that started just before an update paused Capture input makes
/// the update abandon the install and restore capture. Wait for that restore
/// instead of racing it or recording the meeting as system audio only.
fn wait_for_capture_after_update_pause(live_state: &LiveCaptureState) -> bool {
    let deadline = Instant::now() + UPDATE_PAUSE_MEETING_MIC_WAIT;
    while (RESTART_GATE.is_capture_suspended() || RESTART_GATE.is_capture_restore_pending())
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(50));
    }
    primary_live_capture_is_healthy(live_state)
}

fn promote_source_meeting_microphone_lease(app: &AppHandle) {
    if let Some(lease_state) = app.try_state::<SourceMeetingMicrophoneLeaseState>()
        && let Ok(mut lease) = lease_state.lock()
        && let Some(lease) = lease.as_mut()
    {
        lease.auto_started = false;
        lease.user_owned = true;
    }
}

fn note_source_meeting_microphone_stopped(app: &AppHandle, backend_state: &BackendState) {
    let mut meeting_active = false;
    if let Some(lease_state) = app.try_state::<SourceMeetingMicrophoneLeaseState>()
        && let Ok(mut lease) = lease_state.lock()
        && let Some(lease) = lease.as_mut()
    {
        lease.auto_started = false;
        lease.user_owned = false;
        meeting_active = true;
    }
    if meeting_active && let Ok(mut backend) = backend_state.lock() {
        backend.set_system_meeting_capture_warning(
            "Microphone audio stopped during this Meeting; later audio may contain system sound only",
        );
    }
}

fn release_source_meeting_microphone(app: &AppHandle, backend_state: &BackendState) {
    let lease = app
        .try_state::<SourceMeetingMicrophoneLeaseState>()
        .and_then(|state| state.lock().ok().and_then(|mut lease| lease.take()));
    let Some(lease) = lease else {
        return;
    };
    if !source_meeting_microphone_should_stop(&lease) {
        return;
    }
    if let Some(live_state) = app.try_state::<LiveCaptureState>()
        && let Err(error) = stop_live_capture_runtime(app, backend_state, live_state.inner())
    {
        eprintln!("[meeting-capture] could not release automatic microphone: {error}");
    }
}

#[tauri::command]
fn list_recognized_sources(backend_state: State<'_, BackendState>) -> Vec<RecognizedSourceInfo> {
    let settings = backend_state
        .lock()
        .map(|backend| backend.settings())
        .unwrap_or_default();
    recognized_source_infos(&settings)
}

fn source_capture_stream_is_healthy(system_capture_state: &SystemCaptureState) -> bool {
    system_capture_state
        .lock()
        .map(|slot| {
            slot.as_ref()
                .map(|stream| stream.runtime_error().is_none())
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Remove a ScreenCaptureKit handle that reported an asynchronous terminal
/// error so the watcher can finalize the session and retry it.
fn take_source_capture_runtime_error(system_capture_state: &SystemCaptureState) -> Option<String> {
    let mut slot = system_capture_state.lock().ok()?;
    let error = slot.as_ref()?.runtime_error()?;
    slot.take();
    Some(error)
}

#[tauri::command]
fn source_capture_status(
    system_capture_state: State<'_, SystemCaptureState>,
    detected_source_state: State<'_, DetectedSourceState>,
) -> SourceCaptureStatus {
    let detected = detected_source_state
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(SourcePayload::from));
    let capturing = source_capture_stream_is_healthy(system_capture_state.inner());
    SourceCaptureStatus {
        detected,
        capturing,
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn start_source_capture(
    source_id: String,
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    system_capture_state: State<'_, SystemCaptureState>,
    source_capture_lifecycle_state: State<'_, SourceCaptureLifecycleState>,
    detected_source_state: State<'_, DetectedSourceState>,
    source_capture_pause_state: State<'_, SourceCapturePauseState>,
    transcription_state: State<'_, AutoTranscriptionState>,
) -> Result<AppStatus, String> {
    if let Ok(mut paused) = source_capture_pause_state.lock() {
        paused.remove(&source_id);
    }
    start_source_capture_runtime(
        &app,
        &source_id,
        backend_state.inner(),
        system_capture_state.inner(),
        source_capture_lifecycle_state.inner(),
        detected_source_state.inner(),
        transcription_state.inner().clone(),
    )
}

/// Start system-audio capture for a detected source. The source must currently
/// be detected (its pid is the ScreenCaptureKit target) and its id must match.
#[allow(clippy::too_many_arguments)]
fn attempt_source_capture_start(
    source: &DetectedSource,
    backend_state: &BackendState,
    system_capture_state: &SystemCaptureState,
    source_capture_lifecycle_state: &SourceCaptureLifecycleState,
    on_frame: Arc<dyn Fn(AudioFrame) + Send + Sync>,
    frame_generation: &AtomicU64,
    starter: &mut impl SourceCaptureStreamStarter,
    now: Instant,
    screen_recording_status: permissions::PermissionGrantStatus,
) -> Result<SourceCaptureAttemptResult, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let stream_running = source_capture_stream_is_healthy(system_capture_state);
    if stream_running {
        if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
            source_capture_mark_running(&mut lifecycle, &source.source_id);
        }
        let backend = backend_state.lock().map_err(|e| e.to_string())?;
        return Ok(SourceCaptureAttemptResult {
            status: backend.app_status(),
            outcome: SourceCaptureAttemptOutcome::AlreadyCapturing,
        });
    }

    let start_attempt = {
        let mut lifecycle = source_capture_lifecycle_state
            .lock()
            .map_err(|e| e.to_string())?;
        match source_capture_start_decision(&lifecycle, &source.source_id, now) {
            SourceCaptureStartDecision::Start { attempt } => {
                source_capture_mark_starting(&mut lifecycle, &source.source_id, attempt);
                attempt
            }
            SourceCaptureStartDecision::AlreadyStarting
            | SourceCaptureStartDecision::AlreadyRunning
            | SourceCaptureStartDecision::Stopping => {
                let backend = backend_state.lock().map_err(|e| e.to_string())?;
                return Ok(SourceCaptureAttemptResult {
                    status: backend.app_status(),
                    outcome: SourceCaptureAttemptOutcome::Deferred,
                });
            }
            SourceCaptureStartDecision::BackingOff { retry_after } => {
                if let Ok(backend) = backend_state.lock() {
                    append_runtime_debug_log(
                        &backend.settings(),
                        format!(
                            "[source-capture] backoff source_id={} retry_after_ms={}",
                            source.source_id,
                            retry_after.as_millis()
                        ),
                    );
                    return Ok(SourceCaptureAttemptResult {
                        status: backend.app_status(),
                        outcome: SourceCaptureAttemptOutcome::Deferred,
                    });
                }
                return Err("source capture state is unavailable".to_string());
            }
        }
    };

    let sample_rate = system_audio_sample_rate();
    let source_label = recording_source_label(source);
    {
        let mut backend = backend_state.lock().map_err(|e| e.to_string())?;
        append_runtime_debug_log(
            &backend.settings(),
            format!(
                "[source-capture] start source_id={} label={} recording_label={} app={} pid={} sample_rate={} attempt={}",
                source.source_id,
                source.label,
                source_label,
                source.app_name,
                source.pid,
                sample_rate,
                start_attempt
            ),
        );
        if let Err(error) = backend.start_system_capture_session(
            sample_rate,
            chrono::Utc::now(),
            source.app_name.clone(),
            source.source_id.clone(),
            source_label,
        ) {
            if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
                source_capture_mark_failed(&mut lifecycle, &source.source_id, now);
            }
            let diagnostic = source_capture_failure_diagnostic(
                &source.source_id,
                start_attempt,
                screen_recording_status,
                &error,
            );
            append_runtime_debug_log(
                &backend.settings(),
                format!(
                    "{} label={} app={} pid={}",
                    diagnostic, source.label, source.app_name, source.pid
                ),
            );
            return Err(error);
        }
        frame_generation.store(
            backend
                .active_system_meeting_generation()
                .unwrap_or_default(),
            Ordering::Release,
        );
    }

    let capture_scope = source_capture_scope(source);
    if let Ok(backend) = backend_state.lock() {
        append_runtime_debug_log(
            &backend.settings(),
            format!(
                "[source-capture] input_scope source_id={} scope={} app={} pid={}",
                source.source_id, capture_scope, source.app_name, source.pid
            ),
        );
    }

    let stream = match starter.start_source_capture_stream(source, sample_rate, on_frame) {
        Ok(stream) => stream,
        Err(error) => {
            let message = format!("system-audio capture failed: {error}");
            let diagnostic = source_capture_failure_diagnostic(
                &source.source_id,
                start_attempt,
                screen_recording_status,
                &message,
            );
            // Roll back the capture session we opened above.
            if let Ok(mut backend) = backend_state.lock() {
                append_runtime_debug_log(
                    &backend.settings(),
                    format!(
                        "{} label={} app={} pid={}",
                        diagnostic, source.label, source.app_name, source.pid
                    ),
                );
                let _ = backend.stop_system_capture_session();
            }
            if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
                source_capture_mark_failed(&mut lifecycle, &source.source_id, now);
            }
            return Err(message);
        }
    };

    *system_capture_state.lock().map_err(|e| e.to_string())? = Some(stream);
    if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
        source_capture_mark_running(&mut lifecycle, &source.source_id);
    }

    let backend = backend_state.lock().map_err(|e| e.to_string())?;
    Ok(SourceCaptureAttemptResult {
        status: backend.app_status(),
        outcome: SourceCaptureAttemptOutcome::Started,
    })
}

fn start_source_capture_runtime(
    app: &AppHandle,
    source_id: &str,
    backend_state: &BackendState,
    system_capture_state: &SystemCaptureState,
    source_capture_lifecycle_state: &SourceCaptureLifecycleState,
    detected_source_state: &DetectedSourceState,
    transcription_state: AutoTranscriptionState,
) -> Result<AppStatus, String> {
    let _update_activity = RESTART_GATE.source_capture_activity()?;
    let source = {
        let slot = detected_source_state.lock().map_err(|e| e.to_string())?;
        match slot.as_ref() {
            Some(source) if source.source_id == source_id => source.clone(),
            _ => {
                let error = format!("source '{source_id}' is not currently detected");
                emit_source_capture_error(app, source_id, &error);
                return Err(error);
            }
        }
    };

    let callback_backend = Arc::clone(backend_state);
    let callback_app = app.clone();
    let callback_transcription = transcription_state.clone();
    let frame_generation = Arc::new(AtomicU64::new(0));
    let callback_generation = frame_generation.clone();
    let on_frame = move |frame: AudioFrame| {
        let (should_kick, handler, events, tray_status) =
            if let Ok(mut backend) = callback_backend.lock() {
                let generation = callback_generation.load(Ordering::Acquire);
                let status = backend.process_system_audio_frame_for_generation(
                    (generation != 0).then_some(generation),
                    frame,
                );
                let should_kick = status
                    .map(|_| backend.should_process_transcriptions())
                    .unwrap_or(false);
                let tray_status = Some((backend.settings(), backend.app_status()));
                let (handler, events) = live_events_for_dispatch(&mut backend);
                (should_kick, handler, events, tray_status)
            } else {
                (false, None, Vec::new(), None)
            };
        dispatch_live_events(handler, events);
        if let Some((settings, status)) = tray_status {
            update_tray_presentation(&callback_app, &settings, &status);
        }
        if should_kick {
            kick_transcription_worker(
                callback_app.clone(),
                callback_backend.clone(),
                callback_transcription.clone(),
            );
        }
    };

    let mut starter = SystemAudioStreamStarter;
    let screen_recording_status = permissions::permission_snapshot().screen_recording.status;
    let result = match attempt_source_capture_start(
        &source,
        backend_state,
        system_capture_state,
        source_capture_lifecycle_state,
        Arc::new(on_frame),
        frame_generation.as_ref(),
        &mut starter,
        Instant::now(),
        screen_recording_status,
    ) {
        Ok(result) => result,
        Err(error) => {
            emit_source_capture_error(app, source_id, &error);
            return Err(error);
        }
    };

    if result.outcome == SourceCaptureAttemptOutcome::Started {
        ensure_source_meeting_microphone(
            app,
            &source.source_id,
            backend_state,
            transcription_state,
        );
        let _ = app.emit(
            EVENT_SOURCE_CAPTURE_STARTED,
            SourceCapturePayload {
                source_id: source.source_id.clone(),
                label: source.label.clone(),
            },
        );

        let settings = backend_state.lock().map_err(|e| e.to_string())?.settings();
        update_tray_presentation(app, &settings, &result.status);
    }
    Ok(result.status)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn stop_source_capture(
    app: AppHandle,
    backend_state: State<'_, BackendState>,
    system_capture_state: State<'_, SystemCaptureState>,
    source_capture_lifecycle_state: State<'_, SourceCaptureLifecycleState>,
    detected_source_state: State<'_, DetectedSourceState>,
    meeting_state: State<'_, MeetingState>,
    source_capture_pause_state: State<'_, SourceCapturePauseState>,
    transcription_state: State<'_, AutoTranscriptionState>,
) -> Result<AppStatus, String> {
    if let Some(source_id) = detected_source_state
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|source| source.source_id.clone()))
        && let Ok(mut paused) = source_capture_pause_state.lock()
    {
        paused.insert(source_id);
    }
    let status = stop_source_capture_runtime(
        &app,
        backend_state.inner(),
        system_capture_state.inner(),
        source_capture_lifecycle_state.inner(),
        detected_source_state.inner(),
        meeting_state.inner(),
    )?;
    kick_transcription_worker_if_needed(
        app,
        backend_state.inner().clone(),
        transcription_state.inner().clone(),
    );
    Ok(status)
}

fn stop_source_capture_runtime(
    app: &AppHandle,
    backend_state: &BackendState,
    system_capture_state: &SystemCaptureState,
    source_capture_lifecycle_state: &SourceCaptureLifecycleState,
    detected_source_state: &DetectedSourceState,
    _meeting_state: &MeetingState,
) -> Result<AppStatus, String> {
    let _update_activity = RESTART_GATE.activity()?;
    let stopped_source_id = detected_source_state
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|source| source.source_id.clone()));
    if let Some(source_id) = stopped_source_id.as_deref()
        && let Ok(mut lifecycle) = source_capture_lifecycle_state.lock()
    {
        source_capture_mark_stopping(&mut lifecycle, source_id);
    }
    // Dropping the handle stops the ScreenCaptureKit stream.
    *system_capture_state.lock().map_err(|e| e.to_string())? = None;
    let (settings, status, finished_meetings) = {
        let mut backend = backend_state.lock().map_err(|e| e.to_string())?;
        append_runtime_debug_log(&backend.settings(), "[source-capture] stop");
        let status = backend.stop_system_capture_session()?;
        let finished_meetings = backend.take_finished_system_meetings();
        (backend.settings(), status, finished_meetings)
    };
    let emit_meeting = meeting_event_emitter(app.clone());
    for payload in finished_meetings {
        emit_meeting(MeetingEvent::Finished(payload));
    }
    release_source_meeting_microphone(app, backend_state);
    update_tray_presentation(app, &settings, &status);
    let payload = detected_source_state
        .lock()
        .ok()
        .and_then(|slot| {
            slot.as_ref().map(|source| SourceCapturePayload {
                source_id: source.source_id.clone(),
                label: source.label.clone(),
            })
        })
        .unwrap_or(SourceCapturePayload {
            source_id: String::new(),
            label: String::new(),
        });
    let _ = app.emit(EVENT_SOURCE_CAPTURE_STOPPED, payload);
    if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
        source_capture_mark_idle(&mut lifecycle);
    }
    Ok(status)
}

fn emit_source_capture_error(app: &AppHandle, source_id: &str, error: &str) {
    let _ = app.emit(
        EVENT_SOURCE_CAPTURE_ERROR,
        SourceCaptureErrorPayload {
            source_id: source_id.to_string(),
            error: error.to_string(),
        },
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FinishedSystemMeetingJobAction {
    Start(MeetingJobSpec),
    Pending {
        job: MeetingJobSpec,
        reason: &'static str,
    },
}

struct StartupDocumentRecovery {
    recovered_llm_runs: Vec<LlmReportRunSnapshot>,
    reconciled_meetings: Vec<MeetingFinishedEvent>,
    meeting_actions: Vec<FinishedSystemMeetingJobAction>,
}

#[cfg(test)]
fn pending_meeting_job_actions(settings: &AppSettings) -> Vec<FinishedSystemMeetingJobAction> {
    pending_meeting_job_actions_with_cutoff(settings, None)
}

fn pending_meeting_job_actions_before(
    settings: &AppSettings,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<FinishedSystemMeetingJobAction> {
    pending_meeting_job_actions_with_cutoff(settings, Some(cutoff))
}

fn pending_meeting_job_actions_with_cutoff(
    settings: &AppSettings,
    cutoff: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<FinishedSystemMeetingJobAction> {
    let save_root = expand_user_path(&settings.save_root);
    let model_directory = expand_user_path(&settings.model_directory);
    let mut meetings = wakenote::meeting::list_meetings(&save_root)
        .into_iter()
        .filter(|meeting| meeting.status == wakenote::meeting::MeetingStatus::Pending)
        .filter(|meeting| {
            cutoff
                .as_ref()
                .map(|cutoff| meeting.created_at <= *cutoff)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    meetings.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    meetings
        .into_iter()
        .map(|meeting| {
            let job = MeetingJobSpec {
                id: meeting.id,
                save_root: save_root.clone(),
                model_directory: model_directory.clone(),
                model_id: meeting.model_id,
                suppress_low_confidence: settings.suppress_low_confidence_transcripts,
                dictionary: DictionaryContext::from_settings(settings),
            };
            if !settings.transcription_enabled {
                FinishedSystemMeetingJobAction::Pending {
                    job,
                    reason: "transcription_disabled",
                }
            } else if !meeting_job_model_is_ready(&job) {
                FinishedSystemMeetingJobAction::Pending {
                    job,
                    reason: "model_not_ready",
                }
            } else {
                FinishedSystemMeetingJobAction::Start(job)
            }
        })
        .collect()
}

fn spawn_startup_meeting_recovery_worker<T, Scan, Apply>(
    scan: Scan,
    apply: Apply,
) -> thread::JoinHandle<()>
where
    T: Send + 'static,
    Scan: FnOnce() -> T + Send + 'static,
    Apply: FnOnce(T) + Send + 'static,
{
    thread::spawn(move || apply(scan()))
}

fn spawn_startup_document_recovery(
    app: AppHandle,
    meeting_state: MeetingState,
    settings: AppSettings,
    cutoff: chrono::DateTime<chrono::Utc>,
) {
    let _ = spawn_startup_meeting_recovery_worker(
        move || {
            let custom_sources = settings
                .custom_sources
                .iter()
                .map(|source| format!("{}:{}", source.id, source.label))
                .collect::<Vec<_>>()
                .join(",");
            append_runtime_debug_log(
                &settings,
                format!(
                    "[app] launched version={} system_audio_enabled={} custom_sources=[{}]",
                    env!("CARGO_PKG_VERSION"),
                    settings.system_audio_enabled,
                    custom_sources
                ),
            );

            let save_root = expand_user_path(&settings.save_root);
            let llm_store = LlmRunStore::new(save_root.clone());
            let recovered_llm_runs = llm_store
                .recover_interrupted_before(&llm_now(), &cutoff.to_rfc3339())
                .unwrap_or_else(|error| {
                    eprintln!("[llm-report] could not recover interrupted runs: {error}");
                    Vec::new()
                });

            // A previous worker dies with the app. Reconcile it before
            // discovering pending records so all resumable work is visible.
            let reconciled_meetings =
                wakenote::meeting::reconcile_interrupted_before(&save_root, cutoff);
            let meeting_actions = pending_meeting_job_actions_before(&settings, cutoff);
            StartupDocumentRecovery {
                recovered_llm_runs,
                reconciled_meetings,
                meeting_actions,
            }
        },
        move |recovery| {
            for snapshot in recovery.recovered_llm_runs {
                emit_llm_run_update(&app, &snapshot);
            }
            let emit_meeting = meeting_event_emitter(app.clone());
            for payload in recovery.reconciled_meetings {
                emit_meeting(MeetingEvent::Finished(payload));
            }
            start_finished_system_meeting_jobs(&app, &meeting_state, recovery.meeting_actions);
        },
    );
}

fn meeting_job_model_is_ready(job: &MeetingJobSpec) -> bool {
    if job
        .model_directory
        .join(format!("{}.bin", job.model_id))
        .is_file()
    {
        return true;
    }
    let store = ModelStore::new(&job.model_directory);
    store
        .load_model_registry()
        .ok()
        .and_then(|registry| registry.get(&job.model_id).cloned())
        .is_some_and(|model| {
            store.verify_model(&model).is_ok_and(|status| {
                matches!(
                    status,
                    wakenote::models::ModelStatus::Ready | wakenote::models::ModelStatus::Installed
                )
            })
        })
}

fn start_finished_system_meeting_jobs(
    app: &AppHandle,
    meeting_state: &MeetingState,
    actions: Vec<FinishedSystemMeetingJobAction>,
) {
    for action in actions {
        let job = match action {
            FinishedSystemMeetingJobAction::Start(job) => job,
            FinishedSystemMeetingJobAction::Pending { job, reason } => {
                append_debug_log(
                    &job.save_root,
                    format!(
                        "[meeting-capture] state=pending id={} reason={}",
                        job.id, reason
                    ),
                );
                continue;
            }
        };
        append_debug_log(
            &job.save_root,
            format!("[meeting-capture] state=queued id={}", job.id),
        );
        match schedule_and_spawn_meeting_job(app.clone(), meeting_state.clone(), job.clone(), true)
        {
            Ok(MeetingJobScheduleOutcome::Started) => append_debug_log(
                &job.save_root,
                format!("[meeting-capture] state=transcribing id={}", job.id),
            ),
            Ok(MeetingJobScheduleOutcome::Queued) => append_debug_log(
                &job.save_root,
                format!(
                    "[meeting-capture] state=queued id={} reason=worker_busy",
                    job.id
                ),
            ),
            Ok(MeetingJobScheduleOutcome::AlreadyScheduled) => append_debug_log(
                &job.save_root,
                format!("[meeting-capture] state=already_scheduled id={}", job.id),
            ),
            Err(error) => append_debug_log(
                &job.save_root,
                format!(
                    "[meeting-capture] state=pending id={} reason=schedule_failed error={}",
                    job.id, error
                ),
            ),
        }
    }
}

/// ScreenCaptureKit is asked to deliver audio at the transcription pipeline rate
/// so no resampling is needed; mirror that here for the capture session.
fn system_audio_sample_rate() -> u32 {
    PIPELINE_SAMPLE_RATE
}

fn append_runtime_debug_log(settings: &AppSettings, message: impl AsRef<str>) {
    append_debug_log(expand_user_path(&settings.save_root), message);
}

fn source_watcher_should_enumerate_windows(
    settings: &AppSettings,
    screen_recording_status: permissions::PermissionGrantStatus,
) -> bool {
    settings.system_audio_enabled
        && matches!(
            screen_recording_status,
            permissions::PermissionGrantStatus::Granted
        )
}

fn source_watch_diagnostic_due(last: Option<Instant>, now: Instant) -> bool {
    last.map(|last| now.duration_since(last) >= SOURCE_WATCH_DIAGNOSTIC_INTERVAL)
        .unwrap_or(true)
}

fn redacted_source_window_summary(windows: &[wakenote::source_watcher::WindowSnapshot]) -> String {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for window in windows {
        let app = window.app_name.trim();
        if app.is_empty() || !seen.insert(app.to_string()) {
            continue;
        }
        let normalized_title = window.title.to_lowercase();
        let title_shape = if normalized_title.trim().is_empty() {
            "untitled"
        } else if normalized_title.contains("meet") || normalized_title.contains("google") {
            "meet-marker"
        } else {
            "other-title"
        };
        candidates.push(format!("app={app:?},title={title_shape}"));
        if candidates.len() == 6 {
            break;
        }
    }
    format!(
        "windows={} candidates=[{}]",
        windows.len(),
        candidates.join("; ")
    )
}

/// Background watcher: while `system_audio_enabled`, poll the on-screen windows
/// every [`SOURCE_WATCH_INTERVAL`] and react to source transitions. Cheap when
/// the feature is off (it just re-reads the flag and sleeps). Notifications are
/// posted at most once per source per app session via an in-memory snooze set.
#[allow(clippy::too_many_arguments)]
fn spawn_source_watcher(
    app: AppHandle,
    backend_state: BackendState,
    system_capture_state: SystemCaptureState,
    source_capture_lifecycle_state: SourceCaptureLifecycleState,
    detected_source_state: DetectedSourceState,
    meeting_state: MeetingState,
    source_capture_pause_state: SourceCapturePauseState,
    transcription_state: AutoTranscriptionState,
) {
    thread::spawn(move || {
        let mut missing_source_polls = 0;
        let mut last_source_watch_diagnostic = None;
        loop {
            thread::sleep(SOURCE_WATCH_INTERVAL);

            let settings = match backend_state.lock() {
                Ok(backend) => backend.settings(),
                Err(_) => continue,
            };
            if !settings.system_audio_enabled {
                continue;
            }

            if let Some(error) = take_source_capture_runtime_error(&system_capture_state) {
                let source_id = detected_source_state
                    .lock()
                    .ok()
                    .and_then(|slot| slot.as_ref().map(|source| source.source_id.clone()))
                    .unwrap_or_else(|| "unknown".to_string());
                append_runtime_debug_log(
                    &settings,
                    format!(
                        "[source-capture] runtime_error source_id={} error={} action=restart",
                        source_id, error
                    ),
                );
                let stop_result = stop_source_capture_runtime(
                    &app,
                    &backend_state,
                    &system_capture_state,
                    &source_capture_lifecycle_state,
                    &detected_source_state,
                    &meeting_state,
                );
                if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
                    source_capture_mark_failed(&mut lifecycle, &source_id, Instant::now());
                }
                emit_source_capture_error(&app, &source_id, &error);
                kick_transcription_worker_if_needed(
                    app.clone(),
                    backend_state.clone(),
                    transcription_state.clone(),
                );
                if let Err(stop_error) = stop_result {
                    eprintln!(
                        "[source-watch] runtime-error cleanup failed for {source_id}: {stop_error}"
                    );
                }
                continue;
            }
            let screen_recording_status =
                permissions::permission_snapshot().screen_recording.status;
            if !source_watcher_should_enumerate_windows(&settings, screen_recording_status) {
                continue;
            }

            let now = Instant::now();
            let windows = match enumerate_windows() {
                Ok(windows) => windows,
                Err(error) => {
                    if source_watch_diagnostic_due(last_source_watch_diagnostic, now) {
                        append_runtime_debug_log(
                            &settings,
                            format!(
                                "[source-watch] enumerate_error screen_recording_status={screen_recording_status:?} error={error}"
                            ),
                        );
                        last_source_watch_diagnostic = Some(now);
                    }
                    continue;
                }
            };
            let mut source_defs = source_definitions(&settings);
            for source in &mut source_defs {
                source.default_auto_prompt = resolve_auto_prompt(&settings, &source.id);
            }
            let previous = match detected_source_state.lock() {
                Ok(slot) => slot.clone(),
                Err(_) => continue,
            };
            let recovery_pending = previous
                .as_ref()
                .and_then(|source| {
                    source_capture_lifecycle_state.lock().ok().map(|lifecycle| {
                        source_capture_recovery_pending(&lifecycle, &source.source_id)
                    })
                })
                .unwrap_or(false);
            let allow_auto_preemption =
                !source_capture_stream_is_healthy(&system_capture_state) && !recovery_pending;
            let (transition, next_missing_source_polls) =
                compute_source_transition_with_missing_grace_and_auto_preemption(
                    previous.as_ref(),
                    &windows,
                    &source_defs,
                    missing_source_polls,
                    SOURCE_MISSING_GRACE_POLLS,
                    allow_auto_preemption,
                );
            missing_source_polls = next_missing_source_polls;
            if previous.is_none()
                && matches!(&transition, SourceTransition::Unchanged)
                && source_watch_diagnostic_due(last_source_watch_diagnostic, now)
            {
                append_runtime_debug_log(
                    &settings,
                    format!(
                        "[source-watch] no_match screen_recording_status={screen_recording_status:?} {}",
                        redacted_source_window_summary(&windows)
                    ),
                );
                last_source_watch_diagnostic = Some(now);
            }
            match transition {
                SourceTransition::Detected(source) => {
                    if let Ok(mut slot) = detected_source_state.lock() {
                        *slot = Some(source.clone());
                    }
                    append_runtime_debug_log(
                        &settings,
                        format!(
                            "[source-watch] detected source_id={} label={} app={} pid={}",
                            source.source_id, source.label, source.app_name, source.pid
                        ),
                    );
                    let _ = app.emit(EVENT_SOURCE_DETECTED, SourcePayload::from(&source));

                    let already_capturing = source_capture_stream_is_healthy(&system_capture_state);
                    let auto_capture = resolve_auto_prompt(&settings, &source.source_id);
                    let paused_this_session = source_capture_pause_state
                        .lock()
                        .map(|paused| paused.contains(&source.source_id))
                        .unwrap_or(false);
                    if should_auto_capture_source(
                        settings.system_audio_enabled,
                        auto_capture,
                        already_capturing,
                        paused_this_session,
                    ) && let Err(error) = start_source_capture_runtime(
                        &app,
                        &source.source_id,
                        &backend_state,
                        &system_capture_state,
                        &source_capture_lifecycle_state,
                        &detected_source_state,
                        transcription_state.clone(),
                    ) {
                        eprintln!("[source-watch] auto-capture failed: {error}");
                    }
                }
                SourceTransition::Ended(source) => {
                    let capturing = source_capture_stream_is_healthy(&system_capture_state);
                    let recent_audio = backend_state
                        .lock()
                        .map(|backend| {
                            backend.has_recent_system_audio_frame(
                                Instant::now(),
                                SOURCE_CAPTURE_AUDIO_LIVENESS_HOLD,
                            )
                        })
                        .unwrap_or(false);
                    let ended_transition = SourceTransition::Ended(source.clone());
                    if should_defer_source_end_for_recent_audio(
                        &ended_transition,
                        capturing,
                        recent_audio,
                    ) {
                        missing_source_polls = SOURCE_MISSING_GRACE_POLLS.saturating_sub(1);
                        append_runtime_debug_log(
                            &settings,
                            format!(
                                "[source-watch] hold-ended source_id={} label={} app={} pid={} audio_liveness_window_ms={}",
                                source.source_id,
                                source.label,
                                source.app_name,
                                source.pid,
                                SOURCE_CAPTURE_AUDIO_LIVENESS_HOLD.as_millis()
                            ),
                        );
                        continue;
                    }

                    append_runtime_debug_log(
                        &settings,
                        format!(
                            "[source-watch] ended source_id={} label={} app={} pid={}",
                            source.source_id, source.label, source.app_name, source.pid
                        ),
                    );
                    if let Ok(mut paused) = source_capture_pause_state.lock() {
                        paused.remove(&source.source_id);
                    }
                    let _ = app.emit(EVENT_SOURCE_ENDED, SourcePayload::from(&source));

                    if capturing {
                        if let Err(error) = stop_source_capture_runtime(
                            &app,
                            &backend_state,
                            &system_capture_state,
                            &source_capture_lifecycle_state,
                            &detected_source_state,
                            &meeting_state,
                        ) {
                            eprintln!("[source-watch] auto-stop failed: {error}");
                        }
                        // Auto-stop can flush a final system chunk after the
                        // queue worker has gone idle; make that tail job run.
                        kick_transcription_worker_if_needed(
                            app.clone(),
                            backend_state.clone(),
                            transcription_state.clone(),
                        );
                    } else if let Ok(mut lifecycle) = source_capture_lifecycle_state.lock() {
                        source_capture_mark_idle(&mut lifecycle);
                    }
                    if let Ok(mut slot) = detected_source_state.lock() {
                        *slot = None;
                    }
                }
                SourceTransition::Unchanged => {
                    let source = detected_source_state
                        .lock()
                        .ok()
                        .and_then(|slot| slot.clone());
                    let Some(source) = source else {
                        continue;
                    };
                    let already_capturing = source_capture_stream_is_healthy(&system_capture_state);
                    let auto_capture = resolve_auto_prompt(&settings, &source.source_id);
                    let paused_this_session = source_capture_pause_state
                        .lock()
                        .map(|paused| paused.contains(&source.source_id))
                        .unwrap_or(false);
                    if should_auto_capture_source(
                        settings.system_audio_enabled,
                        auto_capture || recovery_pending,
                        already_capturing,
                        paused_this_session,
                    ) && let Err(error) = start_source_capture_runtime(
                        &app,
                        &source.source_id,
                        &backend_state,
                        &system_capture_state,
                        &source_capture_lifecycle_state,
                        &detected_source_state,
                        transcription_state.clone(),
                    ) {
                        eprintln!("[source-watch] auto-capture retry failed: {error}");
                    }
                }
            }
        }
    });
}

fn kick_transcription_worker_if_needed(
    app: AppHandle,
    backend_state: BackendState,
    transcription_state: AutoTranscriptionState,
) {
    let should_process = backend_state
        .lock()
        .map(|backend| backend.should_process_transcriptions())
        .unwrap_or(false);
    if should_process {
        kick_transcription_worker(app, backend_state, transcription_state);
    }
}

fn emit_realtime_partial(app: &AppHandle, backend_state: &BackendState, partial: RealtimePartial) {
    dispatch_auto_type(
        app,
        &partial.source_key,
        partial.microphone_slot,
        partial.chunk_id,
        realtime_auto_type_update(&partial),
    );
    if partial.source_key.starts_with("meeting:") {
        return;
    }
    if partial.source_key != "dictation" && dictation_suppresses_vor_live_transcription(app) {
        return;
    }
    let caption_source = if partial.source_key == "dictation" {
        OverlayCaptionSource::Dictation
    } else {
        OverlayCaptionSource::LiveTranscription
    };
    match caption_source {
        OverlayCaptionSource::Dictation => {
            let settings = backend_state
                .lock()
                .ok()
                .map(|backend| backend.settings())
                .unwrap_or_default();
            publish_dictation_partial(app, partial.chunk_id, &partial.text, &settings);
        }
        OverlayCaptionSource::LiveTranscription
            if partial.microphone_slot != Some(MicrophoneSlot::Secondary) =>
        {
            if let Some(caption_state) = app.try_state::<OverlayCaptionState>() {
                let (overlay_position, caption_style) = backend_state
                    .lock()
                    .ok()
                    .map(|backend| {
                        let settings = backend.settings();
                        (
                            settings.effective_floating_overlay_position(),
                            settings.floating_overlay_caption_style(),
                        )
                    })
                    .unwrap_or_else(|| {
                        (
                            FloatingOverlayPosition::Off,
                            AppSettings::default().floating_overlay_caption_style(),
                        )
                    });
                let snapshot = caption_state.lock().ok().and_then(|mut runtime| {
                    runtime
                        .show_partial_for_source(
                            OverlayCaptionSource::LiveTranscription,
                            partial.chunk_id,
                            &partial.text,
                            overlay_position,
                            caption_style,
                        )
                        .then(|| runtime.snapshot())
                });
                if let Some(snapshot) = snapshot {
                    publish_overlay_caption_snapshot(app, snapshot, "cloud realtime partial");
                }
            }
        }
        OverlayCaptionSource::LiveTranscription | OverlayCaptionSource::Preview => {}
    }
    if let Err(error) = app.emit(
        EVENT_LIVE_PARTIAL,
        LivePartialPayload {
            source_key: partial.source_key,
            source_label: partial.source_label,
            microphone_slot: partial.microphone_slot,
            chunk_id: partial.chunk_id,
            text: partial.text,
        },
    ) {
        eprintln!("[wakenote] WARN failed to emit cloud realtime partial: {error}");
    }
}

fn wire_live_transcription(
    app_handle: AppHandle,
    backend_state: BackendState,
    live_transcriber_state: LiveTranscriberState,
) {
    let model_directory = backend_state
        .lock()
        .map(|backend| backend.settings().model_directory)
        .unwrap_or_default();
    eprintln!("[wakenote] wire_live_transcription: model_dir={model_directory}");

    let app_for_partial = app_handle.clone();
    let backend_for_partial = backend_state.clone();
    let on_partial: Arc<dyn Fn(LivePartialEvent) + Send + Sync> = Arc::new(
        move |event| match event {
            LivePartialEvent::Text(result) => {
                if dictation_suppresses_vor_live_transcription(&app_for_partial) {
                    return;
                }
                dispatch_auto_type(
                    &app_for_partial,
                    &result.source_key,
                    result.microphone_slot,
                    result.chunk_id,
                    AutoTypeUpdate::Partial(result.text.clone()),
                );
                eprintln!(
                    "[wakenote] live partial -> FE chunk_id={} text_len={}",
                    result.chunk_id,
                    result.text.len()
                );
                if result.microphone_slot != Some(MicrophoneSlot::Secondary)
                    && let Some(caption_state) = app_for_partial.try_state::<OverlayCaptionState>()
                {
                    let (overlay_position, caption_style) = backend_for_partial
                        .lock()
                        .ok()
                        .map(|backend| {
                            let settings = backend.settings();
                            (
                                settings.effective_floating_overlay_position(),
                                settings.floating_overlay_caption_style(),
                            )
                        })
                        .unwrap_or_else(|| {
                            (
                                FloatingOverlayPosition::Off,
                                AppSettings::default().floating_overlay_caption_style(),
                            )
                        });
                    let snapshot = caption_state.lock().ok().map(|mut runtime| {
                        runtime.show_partial(
                            result.chunk_id,
                            &result.text,
                            overlay_position,
                            caption_style,
                        );
                        runtime.snapshot()
                    });
                    if let Some(snapshot) = snapshot {
                        publish_overlay_caption_snapshot(
                            &app_for_partial,
                            snapshot,
                            "live partial caption",
                        );
                    }
                }
                if let Err(error) = app_for_partial.emit(
                    EVENT_LIVE_PARTIAL,
                    LivePartialPayload {
                        source_key: result.source_key,
                        source_label: result.source_label,
                        microphone_slot: result.microphone_slot,
                        chunk_id: result.chunk_id,
                        text: result.text,
                    },
                ) {
                    eprintln!("[wakenote] WARN failed to emit partial: {error}");
                }
            }
            LivePartialEvent::ModelMissing {
                source_key,
                source_label,
                microphone_slot,
                chunk_id,
                model_id,
            } => {
                if dictation_suppresses_vor_live_transcription(&app_for_partial) {
                    return;
                }
                eprintln!(
                    "[wakenote] live partial: model missing chunk_id={chunk_id} model={model_id}"
                );
                let error = format!(
                    "Live transcription model {model_id} is not installed. Open Models tab to download it.",
                );
                if let Some(caption_state) = app_for_partial.try_state::<OverlayCaptionState>() {
                    let snapshot = caption_state.lock().ok().map(|mut runtime| {
                        runtime.hide_source(OverlayCaptionSource::LiveTranscription);
                        runtime.snapshot()
                    });
                    if let Some(snapshot) = snapshot {
                        publish_overlay_caption_snapshot(
                            &app_for_partial,
                            snapshot,
                            "live partial model missing",
                        );
                    }
                }
                if let Err(emit_error) = app_for_partial.emit(
                    EVENT_LIVE_FAILED,
                    LiveFailedPayload {
                        source_key,
                        source_label,
                        microphone_slot,
                        chunk_id: Some(chunk_id),
                        audio_path: String::new(),
                        recorded_at: chrono::Utc::now().to_rfc3339(),
                        error,
                    },
                ) {
                    eprintln!("[wakenote] WARN failed to emit live-failed: {emit_error}");
                }
            }
            LivePartialEvent::EngineError {
                source_key,
                source_label,
                microphone_slot,
                chunk_id,
                message,
            } => {
                if dictation_suppresses_vor_live_transcription(&app_for_partial) {
                    return;
                }
                eprintln!(
                    "[wakenote] live partial: engine error chunk_id={chunk_id} message={message}"
                );
                if let Some(caption_state) = app_for_partial.try_state::<OverlayCaptionState>() {
                    let snapshot = caption_state.lock().ok().map(|mut runtime| {
                        runtime.hide_source(OverlayCaptionSource::LiveTranscription);
                        runtime.snapshot()
                    });
                    if let Some(snapshot) = snapshot {
                        publish_overlay_caption_snapshot(
                            &app_for_partial,
                            snapshot,
                            "live partial engine error",
                        );
                    }
                }
                if let Err(emit_error) = app_for_partial.emit(
                    EVENT_LIVE_FAILED,
                    LiveFailedPayload {
                        source_key,
                        source_label,
                        microphone_slot,
                        chunk_id: Some(chunk_id),
                        audio_path: String::new(),
                        recorded_at: chrono::Utc::now().to_rfc3339(),
                        error: format!("Live partial decode failed: {message}"),
                    },
                ) {
                    eprintln!("[wakenote] WARN failed to emit live-failed: {emit_error}");
                }
            }
        },
    );

    let service = Arc::new(LiveTranscriptionService::new(&model_directory, on_partial));
    if let Ok(mut slot) = live_transcriber_state.lock() {
        *slot = Some(service.clone());
    }
    if let Ok(backend) = backend_state.lock() {
        preload_dictation_model(&app_handle, &backend.settings());
    }

    let app_for_openai_realtime = app_handle.clone();
    let backend_for_openai_realtime = backend_state.clone();
    let openai_realtime = OpenAiRealtimeManager::new(Arc::new(move |partial| {
        emit_realtime_partial(
            &app_for_openai_realtime,
            &backend_for_openai_realtime,
            partial,
        );
    }));
    app_handle.manage(openai_realtime.clone());

    let app_for_soniox_realtime = app_handle.clone();
    let backend_for_soniox_realtime = backend_state.clone();
    let soniox_realtime = SonioxRealtimeManager::new(Arc::new(move |partial| {
        emit_realtime_partial(
            &app_for_soniox_realtime,
            &backend_for_soniox_realtime,
            partial,
        );
    }));
    app_handle.manage(soniox_realtime.clone());

    let app_for_handler = app_handle.clone();
    let model_directory_for_handler = model_directory.clone();
    let service_for_handler = service.clone();
    let openai_realtime_for_handler = openai_realtime.clone();
    let soniox_realtime_for_handler = soniox_realtime.clone();
    let handler: wakenote::commands::LiveEventHandler = Arc::new(move |event| match event {
        LiveTranscriptEvent::Discarded {
            source_key,
            chunk_id,
        } => {
            dispatch_auto_type(
                &app_for_handler,
                &source_key,
                None,
                chunk_id,
                AutoTypeUpdate::Failed,
            );
            let _ = app_for_handler.emit(
                "live-transcript-discarded",
                serde_json::json!({ "source_key": source_key, "chunk_id": chunk_id }),
            );
        }
        LiveTranscriptEvent::Started {
            source_key,
            source_label,
            microphone_slot,
            chunk_id,
            started_at,
            overlay_position,
        } => {
            dispatch_auto_type(
                &app_for_handler,
                &source_key,
                microphone_slot,
                chunk_id,
                AutoTypeUpdate::Started,
            );
            eprintln!("[wakenote] handler: emit started chunk_id={chunk_id}");
            if microphone_slot != Some(MicrophoneSlot::Secondary)
                && let Some(caption_state) = app_for_handler.try_state::<OverlayCaptionState>()
            {
                let caption_style = app_for_handler
                    .try_state::<BackendState>()
                    .and_then(|state| {
                        state
                            .lock()
                            .ok()
                            .map(|backend| backend.settings().floating_overlay_caption_style())
                    })
                    .unwrap_or_else(|| AppSettings::default().floating_overlay_caption_style());
                let snapshot = caption_state.lock().ok().map(|mut runtime| {
                    runtime.start_chunk(chunk_id, overlay_position, caption_style);
                    runtime.snapshot()
                });
                if let Some(snapshot) = snapshot {
                    publish_overlay_caption_snapshot(
                        &app_for_handler,
                        snapshot,
                        "live started caption",
                    );
                }
            }
            if let Err(error) = app_for_handler.emit(
                EVENT_LIVE_STARTED,
                LiveStartedPayload {
                    source_key,
                    source_label,
                    microphone_slot,
                    chunk_id,
                    started_at: started_at.to_rfc3339(),
                },
            ) {
                eprintln!("[wakenote] WARN emit started failed: {error}");
            }
        }
        LiveTranscriptEvent::SamplesReady {
            source_key,
            source_label,
            microphone_slot,
            chunk_id,
            model_id,
            language,
            suppress_low_confidence_transcripts,
            sample_rate,
            samples,
        } => {
            let preview_model_id = app_for_handler
                .try_state::<BackendState>()
                .and_then(|state| {
                    state.lock().ok().and_then(|backend| {
                        let mut settings = backend.settings();
                        settings.selected_model = model_id.clone();
                        live_preview_model_id(
                            &settings,
                            &backend.model_registry(),
                            &model_directory_for_handler,
                        )
                    })
                });
            let Some(preview_model_id) = preview_model_id else {
                eprintln!(
                    "[wakenote] handler: no ready live preview model for final model={model_id}"
                );
                return;
            };
            if matches!(
                preview_model_id.as_str(),
                "openai-gpt-live-transcribe" | "soniox-realtime-v5"
            ) {
                let (dictionary, credentials, language_hints) = app_for_handler
                    .try_state::<BackendState>()
                    .and_then(|state| {
                        state.lock().ok().map(|backend| {
                            (
                                DictionaryContext::from_settings(&backend.settings()),
                                backend.transcription_credentials().unwrap_or_default(),
                                backend.settings().transcription_language_hints.clone(),
                            )
                        })
                    })
                    .unwrap_or_default();
                let request = RealtimeSamplesRequest {
                    source_key,
                    source_label,
                    microphone_slot,
                    chunk_id,
                    model_id: preview_model_id.clone(),
                    language,
                    language_hints,
                    dictionary,
                    sample_rate,
                    samples,
                    credentials,
                };
                match preview_model_id.as_str() {
                    "openai-gpt-live-transcribe" => {
                        openai_realtime_for_handler.submit_samples(request)
                    }
                    "soniox-realtime-v5" => soniox_realtime_for_handler.submit_samples(request),
                    _ => {}
                }
                return;
            }
            if !model_supports_live_partials(&model_directory_for_handler, &preview_model_id) {
                eprintln!(
                    "[wakenote] handler: skip unsupported live preview model={preview_model_id} final_model={model_id}"
                );
                return;
            }
            eprintln!(
                "[wakenote] handler: submit live partial chunk_id={chunk_id} preview_model={preview_model_id} final_model={model_id} samples={} rate={sample_rate}",
                samples.len()
            );
            service_for_handler.submit(LivePartialRequest {
                source_key,
                source_label,
                microphone_slot,
                chunk_id,
                model_id: preview_model_id,
                language,
                suppress_low_confidence_transcripts,
                dictionary: app_for_handler
                    .try_state::<BackendState>()
                    .and_then(|state| {
                        state
                            .lock()
                            .ok()
                            .map(|backend| DictionaryContext::from_settings(&backend.settings()))
                    })
                    .unwrap_or_default(),
                sample_rate,
                samples,
            });
        }
        LiveTranscriptEvent::Committed {
            source_key,
            source_label,
            microphone_slot,
            chunk_id,
            model_id,
            audio_path,
            overlay_position: _,
            will_transcribe,
        } => {
            if !will_transcribe {
                dispatch_auto_type(
                    &app_for_handler,
                    &source_key,
                    microphone_slot,
                    chunk_id,
                    AutoTypeUpdate::Failed,
                );
            }
            eprintln!(
                "[wakenote] handler: emit committed chunk_id={chunk_id} path={}",
                audio_path.display()
            );
            match model_id.as_deref() {
                Some("openai-gpt-live-transcribe") => openai_realtime_for_handler.commit(
                    source_key.clone(),
                    chunk_id,
                    audio_path.clone(),
                ),
                Some("soniox-realtime-v5") => soniox_realtime_for_handler.commit(
                    source_key.clone(),
                    chunk_id,
                    audio_path.clone(),
                ),
                _ => {}
            }
            if microphone_slot != Some(MicrophoneSlot::Secondary)
                && let Some(caption_state) = app_for_handler.try_state::<OverlayCaptionState>()
            {
                let snapshot = caption_state.lock().ok().map(|mut runtime| {
                    runtime.mark_committed(chunk_id, audio_path.clone(), will_transcribe);
                    runtime.snapshot()
                });
                if let Some(snapshot) = snapshot {
                    publish_overlay_caption_snapshot(
                        &app_for_handler,
                        snapshot,
                        "live committed caption",
                    );
                }
            }
            if let Err(error) = app_for_handler.emit(
                EVENT_LIVE_COMMITTED,
                LiveCommittedPayload {
                    source_key,
                    source_label,
                    microphone_slot,
                    chunk_id,
                    audio_path: audio_path.to_string_lossy().to_string(),
                    will_transcribe,
                },
            ) {
                eprintln!("[wakenote] WARN emit committed failed: {error}");
            }
        }
    });

    if let Ok(mut backend) = backend_state.lock() {
        backend.set_live_event_handler(handler);
    }
}

fn kick_transcription_worker(
    app: AppHandle,
    backend_state: BackendState,
    transcription_state: AutoTranscriptionState,
) {
    let Ok(update_activity) = RESTART_GATE.activity() else {
        return;
    };
    if transcription_state
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    thread::spawn(move || {
        let _update_activity = update_activity;
        eprintln!(
            "[wakenote] queue worker thread started max_parallel={MAX_PARALLEL_TRANSCRIPTIONS}"
        );
        let (outcome_tx, outcome_rx) = mpsc::channel();
        let mut active_jobs = 0usize;

        loop {
            let started_jobs = match backend_state.lock() {
                // An update is draining work: finish only what cannot resume
                // after the relaunch so the restart is not held by a backlog.
                Ok(mut backend) if RESTART_GATE.is_capture_suspended() => {
                    backend.start_unresumable_transcription_jobs_up_to(MAX_PARALLEL_TRANSCRIPTIONS)
                }
                Ok(mut backend) => {
                    backend.start_transcription_jobs_up_to(MAX_PARALLEL_TRANSCRIPTIONS)
                }
                Err(_) => break,
            };
            if started_jobs.is_empty() && active_jobs == 0 {
                eprintln!("[wakenote] queue worker: no pending jobs, exiting loop");
                break;
            }

            for started in started_jobs {
                spawn_transcription_job(
                    started,
                    outcome_tx.clone(),
                    app.clone(),
                    backend_state.clone(),
                );
                active_jobs = active_jobs.saturating_add(1);
            }

            if active_jobs == 0 {
                continue;
            }

            let Ok((audio_path, outcome)) = outcome_rx.recv() else {
                eprintln!("[wakenote] queue worker: outcome channel closed");
                break;
            };
            eprintln!(
                "[wakenote] queue worker: job id={} outcome={:?}",
                outcome.id, outcome.status
            );

            let cancelled = match backend_state.lock() {
                Ok(backend) if backend.transcription_job_was_cancelled(outcome.id) => true,
                Ok(mut backend) => {
                    if let Err(error) = backend.finish_transcription_job(outcome.clone()) {
                        eprintln!(
                            "[wakenote] queue worker: failed to commit job {} outcome: {error}",
                            outcome.id
                        );
                        active_jobs = active_jobs.saturating_sub(1);
                        continue;
                    }
                    false
                }
                Err(_) => break,
            };
            if cancelled {
                let chunk = wakenote::recorder::RecordedChunk::from_audio_path(audio_path.clone());
                if let Err(error) = TranscriptionSidecar::discard_cancelled_output(&chunk) {
                    eprintln!(
                        "[wakenote] queue worker: failed to discard cancelled job {} output: {error}",
                        outcome.id
                    );
                }
                refresh_transcript_day_index(&audio_path);
                if let Ok(mut backend) = backend_state.lock() {
                    backend.acknowledge_cancelled_transcription(outcome.id);
                }
            } else {
                refresh_transcript_day_index(&audio_path);
                emit_outcome_to_frontend(&app, &backend_state, &audio_path, &outcome);
                record_job_transcription_cost(&app, &outcome);
            }
            active_jobs = active_jobs.saturating_sub(1);
        }

        transcription_state.store(false, Ordering::Release);

        // Pending jobs left behind by an update drain are kicked again if the
        // update is abandoned, or resume after the relaunch.
        if !RESTART_GATE.is_capture_suspended() {
            kick_transcription_worker_if_needed(app, backend_state, transcription_state);
        }
    });
}

fn spawn_transcription_job(
    started: StartedTranscriptionJob,
    outcome_tx: mpsc::Sender<(PathBuf, TranscriptionJobOutcome)>,
    app: AppHandle,
    backend_state: BackendState,
) {
    thread::spawn(move || {
        let audio_path = started.job.audio_path.clone();
        let live_transcript = started
            .live_metadata
            .clone()
            .map(LiveWebhookTranscript::new);
        eprintln!(
            "[wakenote] queue worker: processing job id={} path={} model={}",
            started.job.id,
            audio_path.display(),
            started.job.model_id
        );

        let cancel_backend = backend_state.clone();
        let job_id = started.job.id;
        let transcriber = match started.credentials.and_then(|credentials| {
            let replay = started.job.replay_recorded_audio.then(|| {
                wakenote::recorded_realtime::RecordedRealtimeOptions {
                    credentials: credentials.clone(),
                    is_cancelled: Arc::new(move || {
                        cancel_backend
                            .lock()
                            .map(|backend| backend.transcription_job_was_cancelled(job_id))
                            .unwrap_or(true)
                    }),
                }
            });
            RuntimeTranscriber::for_archival_with_credentials(started.model_directory, credentials)
                .map(|runtime| runtime.with_recorded_realtime(replay))
                .map_err(|error| error.to_string())
        }) {
            Ok(transcriber) => transcriber,
            Err(error) => {
                let _ = outcome_tx.send((
                    audio_path,
                    TranscriptionJobOutcome::failed(started.job.id, error),
                ));
                return;
            }
        };
        let partial_audio_path = audio_path.clone();
        let partial_app = app.clone();
        let partial_backend = backend_state.clone();
        let partial_callback: TranscriptionPartialCallback = Arc::new(move |text| {
            emit_file_stream_partial(&partial_app, &partial_backend, &partial_audio_path, text);
        });
        let transcriber = transcriber.with_file_streaming(
            started.transcription_options.streaming_enabled,
            Some(partial_callback),
        );
        let fallback = FallbackTranscriber::configured(
            transcriber.clone(),
            transcriber,
            if started.transcription_options.cost_limit_fallback_enabled {
                started
                    .transcription_options
                    .cost_limit_fallback_model_id
                    .clone()
            } else {
                None
            },
            started.fallback_supports_diarization,
            started.diarization_required,
        );
        let worker = TranscriptionWorker::with_options_and_dictionary(
            fallback,
            TranscriptionWorkerOptions {
                language: started.language,
                suppress_low_confidence_transcripts: started.suppress_low_confidence_transcripts,
            },
            started.dictionary,
        )
        .with_language_hints(&started.language_hints);
        let outcome = worker
            .process_started_job_with_final(&started.job, |execution| {
                if let Some(transcript) = &live_transcript
                    && let Some(service) = app.try_state::<Arc<WebhookService>>()
                {
                    // Serialize setting changes, cancellation, and finalization
                    // at the same boundary. No disk/network work in this lock.
                    if let Ok(backend) = backend_state.lock()
                        && !backend.transcription_job_was_cancelled(started.job.id)
                    {
                        transcript.finalize(&service, execution);
                    }
                }
            })
            .unwrap_or_else(|error| {
                eprintln!("[wakenote] queue worker: process_started_job error: {error}");
                TranscriptionJobOutcome::failed(started.job.id, error.to_string())
            });

        let _ = outcome_tx.send((audio_path, outcome));
    });
}

fn emit_file_stream_partial(
    app: &AppHandle,
    backend_state: &BackendState,
    audio_path: &Path,
    text: String,
) {
    if !backend_state
        .lock()
        .map(|backend| backend.transcription_output_allowed(audio_path))
        .unwrap_or(false)
    {
        return;
    }
    let (chunk_id, capture_microphones) = backend_state
        .lock()
        .ok()
        .map(|backend| {
            (
                backend
                    .chunk_id_for_audio_path(audio_path)
                    .or_else(|| chunk_id_from_metadata(audio_path)),
                backend.settings().capture_microphones,
            )
        })
        .unwrap_or_else(|| (chunk_id_from_metadata(audio_path), Vec::new()));
    let Some(chunk_id) = chunk_id else {
        return;
    };
    let source = capture_source_identity_from_metadata(audio_path, &capture_microphones);
    if source.microphone_slot != Some(MicrophoneSlot::Secondary)
        && let Some(caption_state) = app.try_state::<OverlayCaptionState>()
    {
        let (overlay_position, caption_style) = backend_state
            .lock()
            .ok()
            .map(|backend| {
                let settings = backend.settings();
                (
                    settings.effective_floating_overlay_position(),
                    settings.floating_overlay_caption_style(),
                )
            })
            .unwrap_or_else(|| {
                (
                    FloatingOverlayPosition::Off,
                    AppSettings::default().floating_overlay_caption_style(),
                )
            });
        let snapshot = caption_state.lock().ok().map(|mut runtime| {
            runtime.show_partial(chunk_id, &text, overlay_position, caption_style);
            runtime.snapshot()
        });
        if let Some(snapshot) = snapshot {
            publish_overlay_caption_snapshot(app, snapshot, "OpenAI file stream partial");
        }
    }
    if let Err(error) = app.emit(
        EVENT_LIVE_PARTIAL,
        LivePartialPayload {
            source_key: source.source_key,
            source_label: source.source_label,
            microphone_slot: source.microphone_slot,
            chunk_id,
            text,
        },
    ) {
        eprintln!("[wakenote] WARN failed to emit OpenAI stream partial: {error}");
    }
}

/// Voice gate verdict for a finished microphone chunk; `None` when the gate is off
/// or the chunk is not from the Primary/merged microphone.
fn voice_gate_verdict(
    app: &AppHandle,
    settings: &AppSettings,
    source_key: &str,
    slot: Option<MicrophoneSlot>,
    audio_path: &Path,
) -> Option<wakenote::voice_gate::SpeakerVerdict> {
    if !settings.voice_gate_enabled
        || !source_key.starts_with("microphone")
        || slot == Some(MicrophoneSlot::Secondary)
    {
        return None;
    }
    let app_data_dir = app.path().app_data_dir().ok()?;
    let started = Instant::now();
    let verdict = wakenote::voice_gate::judge(
        &app_data_dir,
        &expand_user_path(&settings.model_directory),
        audio_path,
        settings.voice_gate_threshold,
    );
    append_runtime_debug_log(
        settings,
        format!(
            "[voice-gate] kind={:?} score={:?} reason={:?} took_ms={} path={}",
            verdict.kind,
            verdict.score,
            verdict.reason,
            started.elapsed().as_millis(),
            audio_path.display()
        ),
    );
    Some(verdict)
}

#[tauri::command]
async fn voice_gate_status(
    app: AppHandle,
    state: State<'_, BackendState>,
) -> Result<wakenote::voice_gate::VoiceGateStatus, String> {
    let model_dir = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(&backend.settings().model_directory)
    };
    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    Ok(wakenote::voice_gate::status(&app_data_dir, &model_dir))
}

#[tauri::command]
async fn voice_gate_download_model(
    app: AppHandle,
    state: State<'_, BackendState>,
) -> Result<wakenote::voice_gate::VoiceGateStatus, String> {
    let model_dir = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(&backend.settings().model_directory)
    };
    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || -> Result<wakenote::voice_gate::VoiceGateStatus, String> {
        wakenote::voice_gate::download_model(&model_dir)?;
        Ok(wakenote::voice_gate::status(&app_data_dir, &model_dir))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn voice_gate_enroll(
    app: AppHandle,
    state: State<'_, BackendState>,
    audio_path: String,
) -> Result<wakenote::voice_gate::VoiceGateStatus, String> {
    let (model_dir, save_root) = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        let settings = backend.settings();
        (
            expand_user_path(&settings.model_directory),
            expand_user_path(&settings.save_root),
        )
    };
    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    let audio = PathBuf::from(&audio_path)
        .canonicalize()
        .map_err(|error| format!("Recording not found: {error}"))?;
    let root = save_root
        .canonicalize()
        .map_err(|error| format!("Save folder not found: {error}"))?;
    if !audio.starts_with(&root) {
        return Err("Only recordings inside the WakeNote save folder can be enrolled.".into());
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<wakenote::voice_gate::VoiceGateStatus, String> {
        wakenote::voice_gate::enroll(&app_data_dir, &model_dir, &audio)?;
        Ok(wakenote::voice_gate::status(&app_data_dir, &model_dir))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn voice_gate_reset(
    app: AppHandle,
    state: State<'_, BackendState>,
) -> Result<wakenote::voice_gate::VoiceGateStatus, String> {
    let model_dir = {
        let backend = state.lock().map_err(|error| error.to_string())?;
        expand_user_path(&backend.settings().model_directory)
    };
    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    wakenote::voice_gate::reset_profile(&app_data_dir)?;
    Ok(wakenote::voice_gate::status(&app_data_dir, &model_dir))
}

fn emit_outcome_to_frontend(
    app: &AppHandle,
    backend_state: &BackendState,
    audio_path: &Path,
    outcome: &TranscriptionJobOutcome,
) {
    let (chunk_id, settings_for_log) = backend_state
        .lock()
        .ok()
        .map(|backend| {
            (
                backend
                    .chunk_id_for_audio_path(audio_path)
                    .or_else(|| chunk_id_from_metadata(audio_path)),
                backend.settings(),
            )
        })
        .unwrap_or_else(|| (chunk_id_from_metadata(audio_path), AppSettings::default()));
    let audio_path_str = audio_path.to_string_lossy().to_string();
    let recorded_at = recorded_at_for_audio_path(audio_path);
    let source_identity =
        capture_source_identity_from_metadata(audio_path, &settings_for_log.capture_microphones);

    if outcome.fallback_from_model_id.is_some()
        && let (Some(requested_model_id), Some(effective_model_id)) = (
            outcome.requested_model_id.clone(),
            outcome.effective_model_id.clone(),
        )
        && let Err(error) = app.emit(
            EVENT_TRANSCRIPTION_MODEL_FALLBACK,
            TranscriptionModelFallbackPayload {
                requested_model_id,
                effective_model_id,
            },
        )
    {
        eprintln!("[wakenote] WARN emit model fallback failed: {error}");
    }

    match &outcome.status {
        TranscriptionJobStatus::Completed => {
            let transcript_path = audio_path.with_extension("txt");
            let text = std::fs::read_to_string(&transcript_path)
                .map(|content| content.trim_end().to_string())
                .unwrap_or_default();
            if text.is_empty() {
                if let Some(id) = chunk_id {
                    dispatch_auto_type(
                        app,
                        &source_identity.source_key,
                        source_identity.microphone_slot,
                        id,
                        AutoTypeUpdate::Failed,
                    );
                }
                eprintln!(
                    "[wakenote] emit_outcome_to_frontend: empty sidecar at {}",
                    transcript_path.display()
                );
                return;
            }
            eprintln!(
                "[wakenote] emit final chunk_id={:?} path={} text_len={}",
                chunk_id,
                audio_path_str,
                text.len()
            );
            if chunk_id.is_some()
                && source_identity.microphone_slot != Some(MicrophoneSlot::Secondary)
                && let Some(caption_state) = app.try_state::<OverlayCaptionState>()
            {
                let snapshot = caption_state.lock().ok().and_then(|mut runtime| {
                    if runtime.show_final_at(
                        chunk_id,
                        audio_path.to_path_buf(),
                        &text,
                        settings_for_log.effective_floating_overlay_position(),
                        settings_for_log.floating_overlay_caption_style(),
                    ) {
                        Some(runtime.snapshot())
                    } else {
                        None
                    }
                });
                if let Some(snapshot) = snapshot {
                    publish_overlay_caption_snapshot(app, snapshot, "live final caption");
                }
            }
            let verdict = voice_gate_verdict(
                app,
                &settings_for_log,
                &source_identity.source_key,
                source_identity.microphone_slot,
                audio_path,
            );
            if let Some(id) = chunk_id {
                let other_speaker = verdict.as_ref().is_some_and(|v| v.is_other());
                dispatch_auto_type(
                    app,
                    &source_identity.source_key,
                    source_identity.microphone_slot,
                    id,
                    if other_speaker {
                        AutoTypeUpdate::Failed
                    } else {
                        AutoTypeUpdate::Final(text.clone())
                    },
                );
            }
            if let Err(error) = app.emit(
                EVENT_LIVE_FINAL,
                LiveFinalPayload {
                    source_key: source_identity.source_key,
                    source_label: source_identity.source_label,
                    microphone_slot: source_identity.microphone_slot,
                    chunk_id,
                    audio_path: audio_path_str,
                    recorded_at,
                    text,
                    speaker: verdict.as_ref().map(|v| v.kind.clone()),
                    speaker_score: verdict.as_ref().and_then(|v| v.score),
                },
            ) {
                eprintln!("[wakenote] WARN emit final failed: {error}");
            }
        }
        TranscriptionJobStatus::Failed(error) => {
            if let Some(id) = chunk_id {
                dispatch_auto_type(
                    app,
                    &source_identity.source_key,
                    source_identity.microphone_slot,
                    id,
                    AutoTypeUpdate::Failed,
                );
            }
            eprintln!(
                "[wakenote] emit failed chunk_id={:?} path={} error={}",
                chunk_id, audio_path_str, error
            );
            if source_identity.microphone_slot != Some(MicrophoneSlot::Secondary)
                && let Some(caption_state) = app.try_state::<OverlayCaptionState>()
            {
                let snapshot = caption_state.lock().ok().map(|mut runtime| {
                    runtime.hide_source(OverlayCaptionSource::LiveTranscription);
                    runtime.snapshot()
                });
                if let Some(snapshot) = snapshot {
                    publish_overlay_caption_snapshot(app, snapshot, "live final failed");
                }
            }
            if let Err(emit_error) = app.emit(
                EVENT_LIVE_FAILED,
                LiveFailedPayload {
                    source_key: source_identity.source_key,
                    source_label: source_identity.source_label,
                    microphone_slot: source_identity.microphone_slot,
                    chunk_id,
                    audio_path: audio_path_str,
                    recorded_at,
                    error: error.clone(),
                },
            ) {
                eprintln!("[wakenote] WARN emit failed event failed: {emit_error}");
            }
        }
    }
}

fn chunk_id_from_metadata(audio_path: &Path) -> Option<u64> {
    let metadata_path = audio_path.with_extension("json");
    let bytes = std::fs::read(&metadata_path).ok()?;
    let metadata: ChunkMetadata = serde_json::from_slice(&bytes).ok()?;
    metadata.live_capture_chunk_id
}

struct CaptureSourceIdentity {
    source_key: String,
    source_label: String,
    microphone_slot: Option<MicrophoneSlot>,
}

fn capture_source_identity_from_metadata(
    audio_path: &Path,
    capture_microphones: &[CaptureMicrophoneEntry],
) -> CaptureSourceIdentity {
    let metadata = std::fs::read(audio_path.with_extension("json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ChunkMetadata>(&bytes).ok());
    let Some(metadata) = metadata else {
        return CaptureSourceIdentity {
            source_key: "microphone".to_string(),
            source_label: "Mic".to_string(),
            microphone_slot: None,
        };
    };
    if metadata.source == wakenote::recorder::ChunkSource::System {
        let label = metadata
            .source_label
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| "System".to_string());
        return CaptureSourceIdentity {
            source_key: format!("system:{label}"),
            source_label: label,
            microphone_slot: None,
        };
    }
    let source_label = if metadata.device_name.trim().is_empty() {
        "Mic".to_string()
    } else {
        metadata.device_name
    };
    let device_id = metadata.device_id.trim();
    let source_key = match metadata.microphone_slot {
        Some(slot) => microphone_source_key(device_id, slot, capture_microphones),
        None if device_id.is_empty() => "microphone".to_string(),
        None => format!("microphone:{device_id}"),
    };
    CaptureSourceIdentity {
        source_key,
        source_label,
        microphone_slot: metadata.microphone_slot,
    }
}

fn apply_launch_at_login_action(
    app: &AppHandle,
    action: LaunchAtLoginAction,
) -> Result<(), String> {
    match action {
        LaunchAtLoginAction::Enable => apply_launch_at_login_preference(app, true),
        LaunchAtLoginAction::Disable => apply_launch_at_login_preference(app, false),
        LaunchAtLoginAction::Unchanged => Ok(()),
    }
}

/// Spawn the microphone-input recovery watchdog. The watchdog runs for the
/// lifetime of the process: every [`MIC_RECOVERY_TICK_INTERVAL`] it polls the
/// backend for a [`MicHealthAction`] and orchestrates a capture restart (or
/// fallback to the system default device) when the stream is wedged.
fn spawn_mic_recovery_watchdog(
    app: AppHandle,
    backend_state: BackendState,
    transcription_state: AutoTranscriptionState,
) {
    thread::spawn(move || {
        loop {
            thread::sleep(MIC_RECOVERY_TICK_INTERVAL);
            // An update stopped Capture input on purpose, or an abandoned one
            // is reopening it; recovery would only race it.
            if RESTART_GATE.is_capture_suspended() || RESTART_GATE.is_capture_restore_pending() {
                continue;
            }
            let (live_running, runtime_error) = match app.try_state::<LiveCaptureState>() {
                Some(state) => match state.lock() {
                    Ok(live) => live
                        .diagnostic(MicrophoneSlot::Primary)
                        .map(|diagnostic| (diagnostic.running, diagnostic.runtime_error))
                        .unwrap_or((false, None)),
                    Err(_) => continue,
                },
                None => continue,
            };
            let secondary_config = backend_state.lock().ok().and_then(|backend| {
                let settings = backend.settings();
                (settings.recording_enabled && !settings.pause_all)
                    .then(|| settings.capture_microphones.get(1).cloned())
                    .flatten()
            });
            if let Some(configured) = secondary_config {
                let secondary_healthy = app
                    .try_state::<LiveCaptureState>()
                    .and_then(|state| {
                        state
                            .lock()
                            .ok()
                            .and_then(|live| live.diagnostic(MicrophoneSlot::Secondary))
                    })
                    .is_some_and(|diagnostic| {
                        diagnostic.running && diagnostic.runtime_error.is_none()
                    });
                if !secondary_healthy && let Some(state) = app.try_state::<LiveCaptureState>() {
                    let _ = stop_live_capture_slot_runtime(
                        &backend_state,
                        state.inner(),
                        MicrophoneSlot::Secondary,
                    );
                    if let Err(error) = start_live_capture_slot_runtime(
                        &app,
                        &backend_state,
                        state.inner(),
                        transcription_state.clone(),
                        MicrophoneSlot::Secondary,
                        configured,
                    ) && let Ok(mut backend) = backend_state.lock()
                    {
                        backend.set_microphone_slot_warning(
                            MicrophoneSlot::Secondary,
                            format!(
                                "Secondary microphone disconnected; waiting for the same device: {error}"
                            ),
                        );
                    }
                }
            }
            // Decide whether this tick should do anything at all. We act when
            // the stream is running (normal heartbeat/silence watching) OR
            // when no stream is running but the user still wants recording
            // AND the backend is mid-recovery from a previous start failure.
            let (should_evaluate, recovery_pending) = match backend_state.lock() {
                Ok(backend) => (
                    backend.live_capture_should_run() || live_running,
                    backend.mic_recovery_pending(),
                ),
                Err(_) => continue,
            };
            if !live_running && !recovery_pending {
                continue;
            }
            if !should_evaluate {
                continue;
            }
            // If cpal explicitly told us the stream broke, latch that as a
            // pending recovery so the same tick fires an action immediately —
            // no need to wait for heartbeat/silence thresholds.
            if let Some(error) = runtime_error
                && let Ok(mut backend) = backend_state.lock()
            {
                backend.notify_stream_error(format!("audio stream error: {error}"));
            }
            let action = match backend_state.lock() {
                Ok(mut backend) => backend.evaluate_microphone_health(),
                Err(_) => continue,
            };
            // Surface a UI warning once the watchdog has been escalating
            // without recovery long enough that the user is likely to
            // benefit from intervention (Refresh / change priority).
            // Static text — the warning key on the frontend hashes the
            // content, and we want it stable across ticks so a dismiss
            // sticks until conditions change.
            if let Ok(mut backend) = backend_state.lock() {
                let escalations = backend.mic_escalations_since_first_frame();
                if escalations >= MIC_WEDGE_WARNING_THRESHOLD {
                    backend.set_microphone_warning(
                        "Microphone has not produced audio after several recovery attempts. \
                         Try Refresh or change the microphone priority.",
                    );
                }
            }
            let Some(action) = action else {
                continue;
            };
            apply_mic_recovery_action(&app, &backend_state, &transcription_state, action);
        }
    });
}

fn apply_mic_recovery_action(
    app: &AppHandle,
    backend_state: &BackendState,
    transcription_state: &AutoTranscriptionState,
    action: MicHealthAction,
) {
    let live_state = match app.try_state::<LiveCaptureState>() {
        Some(state) => state,
        None => return,
    };
    match action {
        MicHealthAction::SwitchTo { device_id, reason } => {
            eprintln!("[mic-watchdog] restarting primary {device_id}: {reason}");
            if let Err(error) = stop_live_capture_slot_runtime(
                backend_state,
                live_state.inner(),
                MicrophoneSlot::Primary,
            ) {
                eprintln!("[mic-watchdog] stop primary before restart failed: {error}");
            }
            // CoreAudio frequently retains wedged state for ~tens of ms
            // after a device is released. A brief settling delay before
            // reopening avoids handing us back the same wedged stream.
            thread::sleep(MIC_RECOVERY_SETTLING_DELAY);
            let configured = backend_state
                .lock()
                .ok()
                .and_then(|backend| backend.settings().capture_microphones.first().cloned());
            let restart = configured
                .ok_or_else(|| "Primary microphone is not configured".to_string())
                .and_then(|entry| {
                    start_live_capture_slot_runtime(
                        app,
                        backend_state,
                        live_state.inner(),
                        transcription_state.clone(),
                        MicrophoneSlot::Primary,
                        entry,
                    )
                });
            if let Err(error) = restart {
                eprintln!("[mic-watchdog] primary restart failed: {error}");
                if let Ok(mut backend) = backend_state.lock() {
                    backend.capture_slot_start_failed(
                        MicrophoneSlot::Primary,
                        format!(
                            "Primary microphone recovery failed: {error}. Try Refresh or reconnect the same device."
                        ),
                    );
                }
            }
        }
    }
}

fn apply_live_capture_runtime_action(
    app: &AppHandle,
    backend_state: &BackendState,
    live_state: &LiveCaptureState,
    transcription_state: AutoTranscriptionState,
    action: LiveCaptureRuntimeAction,
) -> Result<(), String> {
    match action {
        LiveCaptureRuntimeAction::Start => {
            start_live_capture_runtime(app, backend_state, live_state, transcription_state)
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        LiveCaptureRuntimeAction::Stop => {
            stop_live_capture_runtime(app, backend_state, live_state)?;
            Ok(())
        }
        LiveCaptureRuntimeAction::Restart => {
            stop_live_capture_runtime(app, backend_state, live_state)?;
            start_live_capture_runtime(app, backend_state, live_state, transcription_state)
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        LiveCaptureRuntimeAction::Reconcile => {
            start_live_capture_runtime(app, backend_state, live_state, transcription_state)
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        LiveCaptureRuntimeAction::Unchanged => Ok(()),
    }
}

fn launch_auto_start_retry_delay_secs(settings: &AppSettings) -> &'static [u64] {
    if live_capture_should_start_on_launch(settings) {
        &LAUNCH_AUTO_START_RETRY_DELAY_SECS
    } else {
        &[]
    }
}

fn normalize_recent_transcript_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_RECENT_TRANSCRIPT_LIMIT)
        .min(MAX_RECENT_TRANSCRIPT_LIMIT)
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn apply_launch_at_login_preference(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;

    let autostart = app.autolaunch();
    if enabled {
        autostart.enable()
    } else {
        autostart.disable()
    }
    .map_err(|error| error.to_string())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn apply_launch_at_login_preference(_app: &AppHandle, _enabled: bool) -> Result<(), String> {
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DockIconRuntimeMode {
    Visible,
    Hidden,
}

fn dock_icon_runtime_mode(settings: &AppSettings) -> DockIconRuntimeMode {
    if settings.show_dock_icon {
        DockIconRuntimeMode::Visible
    } else {
        DockIconRuntimeMode::Hidden
    }
}

fn settings_window_should_open_on_launch(settings: &AppSettings) -> bool {
    !settings.show_tray_icon
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMenuAction {
    HideSettingsWindow,
    IntentionalQuit,
    Noop,
}

fn app_menu_action(menu_id: &str) -> AppMenuAction {
    match menu_id {
        CLOSE_SETTINGS_WINDOW_MENU_ID => AppMenuAction::HideSettingsWindow,
        "quit" => AppMenuAction::IntentionalQuit,
        _ => AppMenuAction::Noop,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitRequestDecision {
    Allow,
    Prevent,
}

fn exit_request_decision(
    code: Option<i32>,
    show_tray_icon: bool,
    runtime_active: bool,
    intentional_quit: bool,
) -> ExitRequestDecision {
    if code.is_some() || intentional_quit || !show_tray_icon || !runtime_active {
        ExitRequestDecision::Allow
    } else {
        ExitRequestDecision::Prevent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsWindowRequest {
    Show { show_dock_icon: bool },
}

fn settings_window_request_for_reopen(settings: Option<&AppSettings>) -> SettingsWindowRequest {
    SettingsWindowRequest::Show {
        show_dock_icon: settings
            .map(|settings| settings.show_dock_icon)
            .unwrap_or(true),
    }
}

fn settings_window_request_from_app(app: &AppHandle) -> SettingsWindowRequest {
    let settings = app
        .try_state::<BackendState>()
        .and_then(|state| state.lock().ok().map(|backend| backend.settings()));
    settings_window_request_for_reopen(settings.as_ref())
}

fn handle_app_menu_event(app: &AppHandle, menu_id: &str) {
    match app_menu_action(menu_id) {
        AppMenuAction::HideSettingsWindow => {
            if let Err(error) = hide_settings_window(app) {
                eprintln!("[window] failed to close settings window: {error}");
            }
        }
        AppMenuAction::IntentionalQuit => {
            mark_intentional_quit(app, "app menu");
            finalize_manual_meeting_before_exit(app);
            app.exit(0);
        }
        AppMenuAction::Noop => {}
    }
}

fn finalize_manual_meeting_before_exit(app: &AppHandle) {
    let Some(state) = app.try_state::<ManualMeetingRecordingState>() else {
        return;
    };
    let state = state.inner().clone();
    let generation = state
        .lock()
        .ok()
        .and_then(|runtime| runtime.session.as_ref().map(|session| session.generation));
    let Some(generation) = generation else {
        return;
    };
    if let Err(error) = finalize_manual_meeting_recording(
        app,
        &state,
        generation,
        ManualMeetingStopReason::ApplicationQuit,
    ) {
        eprintln!("[wakenote] WARN could not finalize Meeting Mode before exit: {error}");
    }
}

fn mark_intentional_quit(app: &AppHandle, reason: &'static str) {
    if let Some(state) = app.try_state::<IntentionalQuitState>() {
        state.store(true, Ordering::SeqCst);
    }
    append_app_lifecycle_debug_log(app, format!("[app] quit requested reason={reason}"));
}

fn append_app_lifecycle_debug_log(app: &AppHandle, message: impl AsRef<str>) {
    if let Some(settings) = app
        .try_state::<BackendState>()
        .and_then(|state| state.lock().ok().map(|backend| backend.settings()))
    {
        append_runtime_debug_log(&settings, message);
    }
}

fn hide_settings_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL)
        && main_window_close_action(window.label()) == MainWindowCloseAction::HideToTray
    {
        window.hide()?;
    }
    Ok(())
}

fn schedule_settings_window_for_reopen(app: &AppHandle, reason: &'static str) {
    let app_handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if let Err(error) = open_settings_window_for_reopen(&app_handle) {
            eprintln!("[window] failed to open settings window on {reason}: {error}");
        }
    }) {
        eprintln!("[window] failed to schedule settings window on {reason}: {error}");
    }
}

fn open_settings_window_for_reopen(app: &AppHandle) -> tauri::Result<()> {
    match settings_window_request_from_app(app) {
        SettingsWindowRequest::Show { show_dock_icon } => {
            ensure_main_window_visible(app, show_dock_icon)
        }
    }
}

fn prepare_main_window_for_launch(
    app: &AppHandle,
    settings: &AppSettings,
    show_dock_icon: bool,
) -> tauri::Result<()> {
    if settings_window_should_open_on_launch(settings) {
        ensure_main_window_visible(app, show_dock_icon)?;
    } else {
        apply_dock_icon_visibility(app, show_dock_icon)?;
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            window.hide()?;
        }
    }
    Ok(())
}

fn apply_dock_icon_visibility(app: &AppHandle, show_dock_icon: bool) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mode = if show_dock_icon {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        app.set_activation_policy(mode)?;
        app.set_dock_visibility(show_dock_icon)?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, show_dock_icon);
    }

    Ok(())
}

fn ensure_main_window_visible(app: &AppHandle, show_dock_icon: bool) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        apply_dock_icon_visibility(app, show_dock_icon)?;
        app.show()?;
    }

    let window = if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window
    } else {
        tauri::WebviewWindowBuilder::new(
            app,
            MAIN_WINDOW_LABEL,
            tauri::WebviewUrl::App("index.html".into()),
        )
        .title(MAIN_WINDOW_TITLE)
        .inner_size(MAIN_WINDOW_WIDTH, MAIN_WINDOW_HEIGHT)
        .min_inner_size(MAIN_WINDOW_MIN_WIDTH, MAIN_WINDOW_MIN_HEIGHT)
        .resizable(true)
        .visible(true)
        .focused(true)
        .build()?
    };

    window.set_min_size(Some(LogicalSize::new(
        MAIN_WINDOW_MIN_WIDTH,
        MAIN_WINDOW_MIN_HEIGHT,
    )))?;
    if window.is_minimized().unwrap_or(false) {
        window.unminimize()?;
    }
    window.show()?;
    let size = window.inner_size()?;
    if main_window_needs_size_restore(size.width, size.height) {
        window.set_size(LogicalSize::new(MAIN_WINDOW_WIDTH, MAIN_WINDOW_HEIGHT))?;
        if let Err(error) = window.center() {
            eprintln!("[window] failed to center settings window: {error}");
        }
    }
    if let Err(error) = window.set_focus() {
        eprintln!("[window] failed to focus settings window: {error}");
    }
    Ok(())
}

fn main_window_needs_size_restore(width: u32, height: u32) -> bool {
    width < MAIN_WINDOW_MIN_WIDTH as u32 || height < MAIN_WINDOW_MIN_HEIGHT as u32
}

fn build_app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let pkg_info = app.package_info();
    let config = app.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config
            .bundle
            .publisher
            .clone()
            .map(|publisher| vec![publisher]),
        ..Default::default()
    };
    let close_window = MenuItem::with_id(
        app,
        CLOSE_SETTINGS_WINDOW_MENU_ID,
        "Close Window",
        true,
        Some("CmdOrCtrl+W"),
    )?;
    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &close_window,
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let window_menu = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?;
    let help_menu = Submenu::with_id_and_items(
        app,
        HELP_SUBMENU_ID,
        "Help",
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::about(app, None, Some(about_metadata))?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_metadata))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &file_menu,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window_menu,
            &help_menu,
        ],
    )
}

fn main() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            schedule_settings_window_for_reopen(app, "single-instance launch");
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init());

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .menu(build_app_menu)
        .on_menu_event(|app, event| {
            handle_app_menu_event(app, event.id().as_ref());
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && main_window_close_action(window.label()) == MainWindowCloseAction::HideToTray
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
            app.handle().plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None,
            ))?;

            let app_data_dir = app.path().app_data_dir()?;
            let mut backend =
                AppBackend::load_from_dir(&app_data_dir).unwrap_or_default();
            let (dictionary_store, dictionary_startup_outcome) = initialize_dictionary_file(
                &mut backend,
                app_data_dir.join("dictionary.txt"),
            );
            let initial_settings_for_runtime = backend.settings();
            let initial_dock_mode = dock_icon_runtime_mode(&initial_settings_for_runtime);
            let show_dock_icon = initial_dock_mode == DockIconRuntimeMode::Visible;
            prepare_main_window_for_launch(
                app.handle(),
                &initial_settings_for_runtime,
                show_dock_icon,
            )?;
            let backend_state = Arc::new(Mutex::new(backend));
            let dictionary_state: DictionaryFileState =
                Arc::new(Mutex::new(dictionary_store));
            let transcription_state = Arc::new(AtomicBool::new(false));
            let live_transcriber_state: LiveTranscriberState = Arc::new(Mutex::new(None));
            let overlay_caption_state: OverlayCaptionState =
                Arc::new(Mutex::new(OverlayCaptionRuntime::default()));
            let subtitle_preview_state: SubtitlePreviewState = Arc::new(AtomicU64::new(0));
            let intentional_quit_state: IntentionalQuitState = Arc::new(AtomicBool::new(false));
            let system_capture_state: SystemCaptureState = Arc::new(Mutex::new(None));
            let source_capture_lifecycle_state: SourceCaptureLifecycleState =
                Arc::new(Mutex::new(SourceCaptureLifecycle::Idle));
            let detected_source_state: DetectedSourceState = Arc::new(Mutex::new(None));
            let source_capture_pause_state: SourceCapturePauseState =
                Arc::new(Mutex::new(HashSet::new()));
            let source_meeting_microphone_lease_state: SourceMeetingMicrophoneLeaseState =
                Arc::new(Mutex::new(None));
            let meeting_state: MeetingState = Arc::new(Mutex::new(MeetingRuntime::default()));
            let manual_meeting_recording_state: ManualMeetingRecordingState =
                Arc::new(Mutex::new(ManualMeetingRecordingRuntime::default()));
            let transcription_cost_state: TranscriptionCostState = Arc::new(Mutex::new(
                TranscriptionCostLedger::load(ledger_path(&app_data_dir)),
            ));
            let llm_run_state: LlmRunState =
                Arc::new(Mutex::new(LlmRunRuntime::default()));
            let microphone_level_state: MicrophoneLevelState = Arc::new(
                MicrophoneLevelService::new(PlatformVolumeBackend::new()),
            );
            let voice_leveling_policy = VoiceLevelingPolicy::new(
                initial_settings_for_runtime.voice_auto_level_enabled,
            );
            let input_monitor_state: InputMonitorState =
                Arc::new(Mutex::new(InputMonitorRuntime::new()));
            let dictation_state: DictationState =
                Arc::new(Mutex::new(DictationRuntime::default()));
            let modifier_shortcut_state: ModifierShortcutState =
                Arc::new(Mutex::new(DictationModifierShortcuts::default()));
            let dictation_shortcut_dispatcher =
                spawn_dictation_shortcut_worker(app.handle().clone())?;
            let live_webhook = Arc::new(WebhookService::new(
                initial_settings_for_runtime.live_transcription_webhook.clone(),
                wakenote::settings::expand_user_path(&initial_settings_for_runtime.save_root),
                app_data_dir.join("webhook-history.sqlite3"),
            ));
            if let Ok(mut backend) = backend_state.lock() {
                backend.set_live_webhook(live_webhook.clone());
            }
            app.manage(live_webhook);
            app.manage(backend_state.clone());
            app.manage(dictionary_state.clone());
            app.manage(transcription_state.clone());
            app.manage(Mutex::new(
                MultiCaptureRuntime::<CpalAudioInput>::default(),
            ));
            app.manage(input_monitor_state);
            app.manage(dictation_state);
            app.manage(dictation_shortcut_dispatcher);
            app.manage(DictationShortcutSuspension::default());
            app.manage(microphone_shortcut::Runtime::default());
            app.manage(autotype_shortcut::Runtime::default());
            app.manage(modifier_shortcut_state.clone());
            app.manage(live_transcriber_state.clone());
            app.manage(AutoTypeState::default());
            app.manage(TextTransformJobs::default());
            app.manage(SubtitleTranslationState::default());
            app.manage(overlay_caption_state);
            app.manage(subtitle_preview_state);
            app.manage(intentional_quit_state);
            app.manage(system_capture_state.clone());
            app.manage(source_capture_lifecycle_state.clone());
            app.manage(detected_source_state.clone());
            app.manage(source_capture_pause_state.clone());
            app.manage(source_meeting_microphone_lease_state);
            app.manage(meeting_state.clone());
            app.manage(manual_meeting_recording_state);
            app.manage(transcription_cost_state);
            app.manage(llm_run_state);
            app.manage(microphone_level_state.clone());
            app.manage(input_level_query::HardwareQueryGate::default());
            app.manage(voice_leveling_policy);
            spawn_microphone_input_level_observer(
                app.handle().clone(),
                backend_state.clone(),
                microphone_level_state,
            );

            if let Ok(store) = dictionary_state.lock() {
                emit_dictionary_outcome(
                    app.handle(),
                    &dictionary_startup_outcome,
                    &store.status(),
                );
            }
            spawn_dictionary_file_watcher(
                app.handle().clone(),
                backend_state.clone(),
                dictionary_state,
            );

            install_modifier_shortcut_monitors(app.handle(), modifier_shortcut_state)?;

            for (shortcut, mode) in active_dictation_shortcuts(&initial_settings_for_runtime) {
                if let Err(error) = register_dictation_shortcut_for_mode(app.handle(), &shortcut, mode) {
                    append_runtime_debug_log(&initial_settings_for_runtime,
                        format!("[dictation] startup registration failed mode={mode:?}: {error}"));
                    eprintln!("[dictation] startup registration failed mode={mode:?}: {error}");
                }
            }

            if let Err(error) = microphone_shortcut::validate(&initial_settings_for_runtime)
                .and_then(|_| microphone_shortcut::register(app.handle(), &initial_settings_for_runtime.cycle_microphone_shortcut))
            {
                eprintln!("[microphone-cycle] startup shortcut registration failed: {error}");
            }
            if let Err(error) = autotype_shortcut::validate(&initial_settings_for_runtime)
                .and_then(|_| autotype_shortcut::register(app.handle(), &initial_settings_for_runtime.auto_type_toggle_shortcut))
            {
                eprintln!("[auto-type-toggle] startup shortcut registration failed: {error}");
            }

            let startup_recovery_cutoff = chrono::Utc::now();
            spawn_startup_document_recovery(
                app.handle().clone(),
                meeting_state.clone(),
                initial_settings_for_runtime.clone(),
                startup_recovery_cutoff,
            );

            spawn_source_watcher(
                app.handle().clone(),
                backend_state.clone(),
                system_capture_state,
                source_capture_lifecycle_state,
                detected_source_state,
                meeting_state,
                source_capture_pause_state,
                transcription_state.clone(),
            );

            wire_live_transcription(
                app.handle().clone(),
                backend_state.clone(),
                live_transcriber_state.clone(),
            );
            spawn_mic_recovery_watchdog(
                app.handle().clone(),
                backend_state.clone(),
                transcription_state.clone(),
            );
            if let Err(error) = overlay::create_overlay_window(app.handle()) {
                eprintln!("[overlay] initial hidden overlay creation failed: {error}");
            }
            maybe_show_debug_overlay_caption_from_env(app.handle());
            let initial = backend_state
                .lock()
                .ok()
                .map(|backend| (backend.settings(), backend.app_status()));
            let (initial_settings, initial_status) = match initial {
                Some((settings, status)) => (Some(settings), Some(status)),
                None => (None, None),
            };
            app.manage(Mutex::new(
                initial_settings
                    .as_ref()
                    .zip(initial_status.as_ref())
                    .map(|(settings, status)| {
                        tray_presentation_snapshot(settings, status, DictationStage::Idle)
                    }),
            ));
            let tray_menu_items =
                setup_tray(app, initial_settings.as_ref(), initial_status.as_ref())?;
            wakenote::quick_audio::setup(app.handle())?;
            if let Some((settings, status)) = initial_settings.as_ref().zip(initial_status.as_ref()) {
                wakenote::quick_audio::update(app.handle(), settings, status);
            }
            if let Some(settings) = initial_settings.as_ref() {
                let _ = apply_launch_at_login_preference(app.handle(), settings.launch_at_login);
            }
            app.manage(tray_menu_items);
            let app_handle_for_initial = app.handle().clone();
            let launch_auto_start_delays = initial_settings
                .as_ref()
                .map(launch_auto_start_retry_delay_secs)
                .unwrap_or(&[])
                .to_vec();
            if !launch_auto_start_delays.is_empty() {
                let app_handle_for_capture = app.handle().clone();
                let backend_state_for_capture = backend_state.clone();
                let transcription_state_for_capture = transcription_state.clone();
                thread::spawn(move || {
                    for delay_secs in launch_auto_start_delays {
                        thread::sleep(Duration::from_secs(delay_secs));
                        let should_start = backend_state_for_capture
                            .lock()
                            .map(|backend| {
                                live_capture_should_start_on_launch(&backend.settings())
                            })
                            .unwrap_or(false);
                        if !should_start {
                            eprintln!(
                                "[capture] launch auto-start stopped because settings no longer allow live input"
                            );
                            break;
                        }

                        let live_state = app_handle_for_capture.state::<LiveCaptureState>();
                        match start_live_capture_runtime(
                            &app_handle_for_capture,
                            &backend_state_for_capture,
                            live_state.inner(),
                            transcription_state_for_capture.clone(),
                        ) {
                            Ok(status) => {
                                let settings = backend_state_for_capture
                                    .lock()
                                    .map(|backend| backend.settings())
                                    .ok();
                                if let Some(settings) = settings {
                                    update_tray_presentation(
                                        &app_handle_for_capture,
                                        &settings,
                                        &status,
                                    );
                                }
                                if status.live_input_active {
                                    eprintln!("[capture] launch auto-start succeeded");
                                    break;
                                }
                                eprintln!(
                                    "[capture] launch auto-start did not activate input; retrying if attempts remain"
                                );
                            }
                            Err(error) => {
                                eprintln!(
                                    "[capture] launch auto-start attempt failed: {error}"
                                );
                                if !launch_auto_start_should_retry(&error) {
                                    eprintln!(
                                        "[capture] launch auto-start stopped because Microphone permission is unavailable"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                });
            }
            kick_transcription_worker_if_needed(
                app_handle_for_initial,
                backend_state,
                transcription_state,
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            wakenote::quick_audio::hide_quick_audio,
            wakenote::quick_audio::quick_audio_history,
            microphone_shortcut::cycle_primary_microphone,
            microphone_shortcut::microphone_cycle_notice,
            autotype_shortcut::toggle_auto_type,
            app_updater::check_for_update,
            app_updater::update_install_readiness,
            app_updater::open_update_release,
            app_updater::download_and_install_update,
            enhanced_dictation_event,
            transform_text,
            cancel_text_transform,
            get_settings,
            get_live_webhook_stats,
            list_webhook_history,
            get_webhook_history_detail,
            retry_webhook_history,
            dictionary_file_status,
            open_dictionary_file,
            reload_dictionary_file,
            load_list_visibility,
            set_list_visibility,
            openrouter_key_status,
            save_openrouter_api_key,
            delete_openrouter_api_key,
            openai_key_status,
            save_openai_api_key,
            delete_openai_api_key,
            soniox_key_status,
            save_soniox_api_key,
            delete_soniox_api_key,
            overlay_caption_snapshot,
            preview_subtitle,
            debug_show_overlay_caption,
            debug_hide_overlay_caption,
            suspend_dictation_shortcut,
            resume_dictation_shortcut,
            pressed_modifier_shortcut,
            update_settings,
            app_status,
            list_microphones,
            microphone_input_levels,
            set_microphone_input_volume,
            merge_transcript_audio,
            list_models,
            verify_model,
            download_model,
            cancel_model_download,
            delete_model,
            queue_snapshot,
            permission_snapshot,
            request_accessibility_permission,
            open_accessibility_permission_settings,
            request_microphone_permission,
            open_microphone_permission_settings,
            request_screen_recording_permission,
            open_screen_recording_settings,
            open_permission_drag_shelf,
            recent_transcripts,
            recent_dictations,
            voice_gate_status,
            voice_gate_download_model,
            voice_gate_enroll,
            voice_gate_reset,
            transcript_days,
            transcripts_for_day,
            export_all_transcript_days,
            export_transcript_day,
            rebuild_transcript_day_index,
            start_llm_report,
            list_llm_report_runs,
            cancel_llm_report,
            retry_llm_report,
            list_llm_report_history,
            load_llm_report_history_detail,
            export_llm_report,
            allow_audio_playback,
            upload_audio_file,
            analyze_audio_waveform,
            enqueue_audio_file,
            enqueue_backlog,
            retry_job,
            regenerate_transcript,
            reprocess_jobs,
            trash_activity_jobs,
            trash_transcripts,
            open_transcript_folder,
            skip_job,
            mark_all_activity_read,
            cancel_current_transcription,
            cancel_activity_job,
            dictation_state,
            cancel_dictation,
            cancel_current_operation,
            reveal_save_folder,
            process_next_transcription,
            transcription_cost_snapshot,
            transcription_cost_details,
            start_live_capture,
            stop_live_capture,
            list_recognized_sources,
            source_capture_status,
            start_source_capture,
            stop_source_capture,
            manual_meeting_recording_status,
            start_manual_meeting_recording,
            stop_manual_meeting_recording,
            list_meetings,
            import_meeting_recording,
            meeting_detail,
            start_meeting_transcription,
            cancel_meeting,
            resume_meeting
        ])
        .build(tauri::generate_context!())
        .expect("failed to build WakeNote")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit)
                && let Some(service) = app.try_state::<Arc<WebhookService>>()
            {
                service.shutdown();
            }
            #[cfg(target_os = "macos")]
            match event {
                tauri::RunEvent::Reopen { .. } => {
                    schedule_settings_window_for_reopen(app, "macOS reopen");
                }
                tauri::RunEvent::ExitRequested { code, api, .. } => {
                    let (settings, mut runtime_active) = app
                        .try_state::<BackendState>()
                        .and_then(|state| {
                            state.lock().ok().map(|backend| {
                                let status = backend.app_status();
                                let runtime_active = status.live_input_active
                                    || status.queue.running_count > 0
                                    || backend.live_capture_should_run();
                                (backend.settings(), runtime_active)
                            })
                        })
                        .unwrap_or_else(|| (AppSettings::default(), false));
                    let manual_meeting_active = app
                        .try_state::<ManualMeetingRecordingState>()
                        .and_then(|state| {
                            state
                                .lock()
                                .ok()
                                .map(|runtime| runtime.session.is_some())
                        })
                        .unwrap_or(false);
                    runtime_active |= manual_meeting_active;
                    let show_tray_icon = settings
                        .show_tray_icon;
                    let intentional_quit = app
                        .try_state::<IntentionalQuitState>()
                        .map(|state| state.load(Ordering::SeqCst))
                        .unwrap_or(false);
                    let decision =
                        exit_request_decision(code, show_tray_icon, runtime_active, intentional_quit);
                    append_runtime_debug_log(
                        &settings,
                        format!(
                            "[app] exit_requested code={code:?} show_tray_icon={show_tray_icon} runtime_active={runtime_active} intentional_quit={intentional_quit} decision={decision:?}"
                        ),
                    );
                    if matches!(decision, ExitRequestDecision::Prevent) {
                        api.prevent_exit();
                    } else {
                        finalize_manual_meeting_before_exit(app);
                    }
                }
                // whisper.cpp's GGML Metal backend aborts inside its
                // static destructor (`ggml_metal_rsets_free` -> `ggml_abort`)
                // when the process tears down via libc `exit()` ->
                // `__cxa_finalize`. The Metal device is already gone by then,
                // so the cleanup trips an internal assertion and SIGABRTs on
                // every quit that ran a transcription. Bypass the C++ static
                // destructors entirely with `_exit`: the kernel reclaims the
                // GPU resources anyway, and all durable state (settings,
                // transcription queue) is already persisted on write.
                tauri::RunEvent::Exit => {
                    append_app_lifecycle_debug_log(app, "[app] exit");
                    unsafe { libc::_exit(0) };
                }
                _ => {}
            }

            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

fn setup_tray(
    app: &mut tauri::App,
    initial_settings: Option<&AppSettings>,
    initial_status: Option<&AppStatus>,
) -> tauri::Result<TrayMenuItems> {
    let initial_menu = initial_settings
        .zip(initial_status)
        .map(|(settings, status)| tray_menu_presentation(settings, status));
    let recording = CheckMenuItem::with_id(
        app,
        "toggle-recording",
        initial_menu
            .as_ref()
            .map(|menu| menu.recording_text)
            .unwrap_or("Recording On"),
        true,
        initial_menu
            .as_ref()
            .map(|menu| menu.recording_checked)
            .unwrap_or(true),
        None::<&str>,
    )?;
    let transcription = CheckMenuItem::with_id(
        app,
        "toggle-transcription",
        "Transcription On",
        true,
        initial_menu
            .as_ref()
            .map(|menu| menu.transcription_checked)
            .unwrap_or(true),
        None::<&str>,
    )?;
    let subtitles = CheckMenuItem::with_id(
        app,
        "toggle-subtitles",
        "Subtitles On",
        true,
        initial_menu
            .as_ref()
            .map(|menu| menu.subtitles_checked)
            .unwrap_or(false),
        None::<&str>,
    )?;
    let auto_type = CheckMenuItem::with_id(
        app,
        "toggle-auto-type",
        initial_menu
            .as_ref()
            .map(|menu| menu.auto_type_text)
            .unwrap_or("Auto Type Off"),
        true,
        initial_menu
            .as_ref()
            .map(|menu| menu.auto_type_checked)
            .unwrap_or(false),
        None::<&str>,
    )?;
    let active_model = MenuItem::with_id(
        app,
        "active-model",
        initial_menu
            .as_ref()
            .map(|menu| menu.active_model_text.as_str())
            .unwrap_or("Model: whisper-medium"),
        false,
        None::<&str>,
    )?;
    let active_mic = MenuItem::with_id(
        app,
        "active-microphone",
        initial_menu
            .as_ref()
            .map(|menu| menu.active_microphone_text.as_str())
            .unwrap_or("Microphone: System Default"),
        false,
        None::<&str>,
    )?;
    let threshold = MenuItem::with_id(
        app,
        "threshold",
        initial_menu
            .as_ref()
            .map(|menu| menu.threshold_text.as_str())
            .unwrap_or("Threshold: -45 dBFS"),
        false,
        None::<&str>,
    )?;
    let reveal = MenuItem::with_id(
        app,
        "reveal-save-folder",
        "Reveal Save Folder",
        true,
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, "open-settings", "Open Settings", true, None::<&str>)?;
    let pause_all = CheckMenuItem::with_id(
        app,
        "pause-all",
        "Pause All",
        true,
        initial_menu
            .as_ref()
            .map(|menu| menu.pause_all_checked)
            .unwrap_or(false),
        None::<&str>,
    )?;
    let cancel = MenuItem::with_id(
        app,
        "cancel-current-operation",
        "Cancel Current Operation",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, Some("CmdOrCtrl+Q"))?;
    let separator_one = PredefinedMenuItem::separator(app)?;
    let separator_two = PredefinedMenuItem::separator(app)?;
    let separator_three = PredefinedMenuItem::separator(app)?;
    let separator_four = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &separator_one,
            &recording,
            &transcription,
            &subtitles,
            &auto_type,
            &active_model,
            &active_mic,
            &threshold,
            &separator_two,
            &reveal,
            &pause_all,
            &separator_three,
            &cancel,
            &separator_four,
            &quit,
        ],
    )?;

    let presentation = initial_settings
        .zip(initial_status)
        .map(|(settings, status)| tray_runtime_presentation(settings, status))
        .unwrap_or_else(|| wakenote::commands::TrayRuntimePresentation {
            icon: tray_presentation_for_state(wakenote::commands::TrayState::Listening),
            visible: true,
        });
    let icon_image = tray_icon_image_for_presentation(presentation.icon);
    let icon = Image::new_owned(icon_image.rgba, icon_image.width, icon_image.height);
    let tray = TrayIconBuilder::with_id("wakenote")
        .tooltip(presentation.icon.tooltip)
        .icon(icon)
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(
            initial_settings
                .map(tray_show_menu_on_left_click)
                .unwrap_or(false),
        )
        .on_menu_event(|app, event| {
            handle_tray_menu(app, event.id().as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            handle_tray_icon_event(tray.app_handle(), event);
        })
        .build(app)?;
    tray.set_visible(presentation.visible)?;

    Ok(TrayMenuItems {
        recording,
        transcription,
        subtitles,
        auto_type,
        active_model,
        active_mic,
        threshold,
        pause_all,
        cancel,
    })
}

fn tray_presentation_update_payload(
    settings: &AppSettings,
    status: &AppStatus,
    dictation_stage: DictationStage,
) -> TrayPresentationUpdate {
    TrayPresentationUpdate {
        settings: settings.clone(),
        status: status.clone(),
        dictation_stage,
    }
}

fn tray_presentation_snapshot(
    settings: &AppSettings,
    status: &AppStatus,
    dictation_stage: DictationStage,
) -> TrayPresentationSnapshot {
    TrayPresentationSnapshot {
        runtime: tray_runtime_presentation(settings, status),
        menu: tray_menu_presentation(settings, status),
        show_menu_on_left_click: tray_show_menu_on_left_click(settings),
        dictation_stage,
    }
}

fn next_tray_presentation_update(
    cache: &mut Option<TrayPresentationSnapshot>,
    settings: &AppSettings,
    status: &AppStatus,
    dictation_stage: DictationStage,
) -> Option<TrayPresentationUpdate> {
    let snapshot = tray_presentation_snapshot(settings, status, dictation_stage);
    if cache.as_ref() == Some(&snapshot) {
        return None;
    }

    *cache = Some(snapshot);
    Some(tray_presentation_update_payload(
        settings,
        status,
        dictation_stage,
    ))
}

fn update_tray_presentation(app: &tauri::AppHandle, settings: &AppSettings, status: &AppStatus) {
    wakenote::quick_audio::update(app, settings, status);
    let app_handle = app.clone();
    let dictation_stage = app
        .try_state::<DictationState>()
        .and_then(|state| state.lock().ok().map(|runtime| runtime.stage()))
        .unwrap_or(DictationStage::Idle);
    let payload = if let Some(cache) = app.try_state::<TrayPresentationCache>() {
        match cache.lock() {
            Ok(mut cache) => {
                next_tray_presentation_update(&mut cache, settings, status, dictation_stage)
            }
            Err(_) => Some(tray_presentation_update_payload(
                settings,
                status,
                dictation_stage,
            )),
        }
    } else {
        Some(tray_presentation_update_payload(
            settings,
            status,
            dictation_stage,
        ))
    };
    let Some(payload) = payload else {
        return;
    };
    let log_settings = payload.settings.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        apply_tray_presentation(
            &app_handle,
            &payload.settings,
            &payload.status,
            payload.dictation_stage,
        );
    }) {
        append_runtime_debug_log(
            &log_settings,
            format!("[tray] failed to schedule presentation update: {error}"),
        );
        eprintln!("[tray] failed to schedule presentation update: {error}");
    }
}

fn apply_tray_presentation(
    app: &tauri::AppHandle,
    settings: &AppSettings,
    status: &AppStatus,
    dictation_stage: DictationStage,
) {
    let Some(tray) = app.tray_by_id("wakenote") else {
        return;
    };
    let presentation = tray_runtime_presentation(settings, status);
    let icon = tray_presentation_for_dictation(dictation_stage).unwrap_or(presentation.icon);
    let icon_image = tray_icon_image_for_presentation(icon);
    let _ = tray.set_icon(Some(Image::new_owned(
        icon_image.rgba,
        icon_image.width,
        icon_image.height,
    )));
    let _ = tray.set_tooltip(Some(icon.tooltip));
    let _ = tray.set_visible(presentation.visible);
    let _ = tray.set_show_menu_on_left_click(tray_show_menu_on_left_click(settings));

    if let Some(items) = app.try_state::<TrayMenuItems>() {
        let menu = tray_menu_presentation(settings, status);
        let _ = items.recording.set_checked(menu.recording_checked);
        let _ = items.recording.set_text(menu.recording_text);
        let _ = items.transcription.set_checked(menu.transcription_checked);
        let _ = items.subtitles.set_checked(menu.subtitles_checked);
        let _ = items.auto_type.set_checked(menu.auto_type_checked);
        let _ = items.auto_type.set_text(menu.auto_type_text);
        let _ = items.pause_all.set_checked(menu.pause_all_checked);
        let _ = items.active_model.set_text(menu.active_model_text);
        let _ = items.active_mic.set_text(menu.active_microphone_text);
        let _ = items.threshold.set_text(menu.threshold_text);
        let _ = items.cancel.set_text(
            if matches!(
                dictation_stage,
                DictationStage::Recording | DictationStage::Transcribing
            ) {
                "Cancel Dictation"
            } else {
                "Cancel Current Operation"
            },
        );
    }
}

#[cfg(test)]
fn tray_menu_order() -> &'static [&'static str] {
    TRAY_MENU_ORDER
}

fn tray_show_menu_on_left_click(settings: &AppSettings) -> bool {
    settings.tray_left_click_action == TrayClickAction::OpenMenu
}

fn tray_click_action_for_button(
    settings: &AppSettings,
    button: MouseButton,
) -> Option<TrayClickAction> {
    match button {
        MouseButton::Left => Some(settings.tray_left_click_action),
        MouseButton::Right => Some(settings.tray_right_click_action()),
        MouseButton::Middle => None,
    }
}

fn handle_tray_icon_event(app: &tauri::AppHandle, event: TrayIconEvent) {
    let TrayIconEvent::Click {
        button,
        button_state,
        ..
    } = event
    else {
        return;
    };
    if button_state != MouseButtonState::Up {
        return;
    }

    let action = app
        .try_state::<BackendState>()
        .and_then(|state| {
            state
                .lock()
                .ok()
                .and_then(|backend| tray_click_action_for_button(&backend.settings(), button))
        })
        .unwrap_or(TrayClickAction::OpenMenu);

    if action == TrayClickAction::TogglePause {
        patch_from_tray(app, |settings| SettingsPatch {
            pause_all: Some(!settings.pause_all),
            ..SettingsPatch::default()
        });
    }
}

fn handle_tray_menu(app: &tauri::AppHandle, id: &str) {
    match id {
        "toggle-recording" => patch_from_tray(app, |settings| SettingsPatch {
            recording_enabled: Some(!settings.recording_enabled),
            ..SettingsPatch::default()
        }),
        "toggle-transcription" => patch_from_tray(app, |settings| SettingsPatch {
            transcription_enabled: Some(!settings.transcription_enabled),
            ..SettingsPatch::default()
        }),
        "toggle-subtitles" => patch_from_tray(app, |settings| SettingsPatch {
            show_floating_overlay: Some(!settings.show_floating_overlay),
            ..SettingsPatch::default()
        }),
        "toggle-auto-type" => patch_from_tray(app, |settings| SettingsPatch {
            auto_transcript_input_enabled: Some(!settings.auto_transcript_input_enabled),
            ..SettingsPatch::default()
        }),
        "pause-all" => patch_from_tray(app, |settings| SettingsPatch {
            pause_all: Some(!settings.pause_all),
            ..SettingsPatch::default()
        }),
        "reveal-save-folder" => {
            let state = app.state::<BackendState>();
            let request = state
                .lock()
                .map(|backend| reveal_save_folder_request(&backend.settings()))
                .ok();
            if let Some(request) = request {
                let _ = Command::new(request.program).arg(request.path).spawn();
            }
        }
        "cancel-current-operation" => {
            if cancel_dictation_runtime(app).unwrap_or(false) {
                return;
            }
            let state = app.state::<BackendState>();
            let presentation = if let Ok(mut backend) = state.lock() {
                let _ = backend.cancel_current_operation();
                Some((backend.settings(), backend.app_status()))
            } else {
                None
            };
            if let Some((settings, status)) = presentation {
                update_tray_presentation(app, &settings, &status);
            }
        }
        "open-settings" => {
            let show_dock_icon = app
                .try_state::<BackendState>()
                .and_then(|state| {
                    state
                        .lock()
                        .ok()
                        .map(|backend| backend.settings().show_dock_icon)
                })
                .unwrap_or(true);
            if let Err(error) = ensure_main_window_visible(app, show_dock_icon) {
                eprintln!("[window] failed to open settings window: {error}");
            }
        }
        "quit" => {
            mark_intentional_quit(app, "tray menu");
            finalize_manual_meeting_before_exit(app);
            app.exit(0);
        }
        _ => {}
    }
}

fn patch_from_tray(app: &tauri::AppHandle, patch: impl FnOnce(AppSettings) -> SettingsPatch) {
    let state = app.state::<BackendState>();
    let live_state = app.state::<LiveCaptureState>();
    let transcription_state = app.state::<AutoTranscriptionState>();
    let (live_capture_action, handler, events, overlay_settings) =
        if let Ok(mut backend) = state.lock() {
            let current = backend.settings();
            let patch = patch(current.clone());
            let live_capture_action = live_capture_runtime_action_for_patch(&current, &patch);
            backend.update_settings(patch);
            let next = backend.settings();
            let overlay_settings =
                overlay_presentation_settings_changed(&current, &next).then_some(next);
            let (handler, events) = live_events_for_dispatch(&mut backend);
            (live_capture_action, handler, events, overlay_settings)
        } else {
            return;
        };
    dispatch_live_events(handler, events);

    let _ = apply_live_capture_runtime_action(
        app,
        state.inner(),
        live_state.inner(),
        transcription_state.inner().clone(),
        live_capture_action,
    );
    let _ = apply_input_monitor_settings(app, state.inner());
    if let Some(settings) = overlay_settings {
        apply_overlay_settings_change(app, &settings);
    }
    let presentation = state
        .lock()
        .map(|backend| (backend.settings(), backend.app_status()))
        .ok();
    if let Some((settings, status)) = presentation {
        update_tray_presentation(app, &settings, &status);
    }
    kick_transcription_worker_if_needed(
        app.clone(),
        state.inner().clone(),
        transcription_state.inner().clone(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realtime_partial(text: &str, confirmed: &str) -> RealtimePartial {
        RealtimePartial {
            source_key: "microphone".into(),
            source_label: "Mic".into(),
            microphone_slot: Some(MicrophoneSlot::Primary),
            chunk_id: 7,
            text: text.into(),
            confirmed_text: confirmed.into(),
        }
    }

    #[test]
    fn realtime_partials_route_confirmed_words_when_the_provider_has_them() {
        assert_eq!(
            realtime_auto_type_update(&realtime_partial("hello wor", "")),
            AutoTypeUpdate::Partial("hello wor".into())
        );
        assert_eq!(
            realtime_auto_type_update(&realtime_partial("hello wor", "hello")),
            AutoTypeUpdate::Confirmed("hello".into())
        );
        // Same combined text, more of it confirmed: the confirmed words still move on.
        assert_eq!(
            realtime_auto_type_update(&realtime_partial("hello wor", "hello wor")),
            AutoTypeUpdate::Confirmed("hello wor".into())
        );
    }

    #[test]
    fn voice_gate_holds_partials_and_confirmed_words_but_final_only_holds_partials_only() {
        let mut settings = AppSettings::default();
        let partial = AutoTypeUpdate::Partial("a".into());
        let confirmed = AutoTypeUpdate::Confirmed("a".into());
        let final_text = AutoTypeUpdate::Final("a".into());
        assert!(!auto_type_update_is_held(&partial, &settings));
        assert!(!auto_type_update_is_held(&confirmed, &settings));
        settings.auto_transcript_input_final_only = true;
        assert!(auto_type_update_is_held(&partial, &settings));
        assert!(!auto_type_update_is_held(&confirmed, &settings));
        settings.voice_gate_enabled = true;
        assert!(auto_type_update_is_held(&partial, &settings));
        assert!(auto_type_update_is_held(&confirmed, &settings));
        assert!(!auto_type_update_is_held(&final_text, &settings));
    }
    use chrono::TimeZone;
    use wakenote::settings::{DictionaryEntry, SourceAutoPromptEntry};

    struct CaptionTestStream;

    impl AudioStreamHandle for CaptionTestStream {}

    #[derive(Default)]
    struct CaptionTestInput;

    impl wakenote::live_capture::AudioInputBackend for CaptionTestInput {
        fn start(
            &mut self,
            _config: AudioInputConfig,
            _on_frame: Arc<dyn Fn(AudioFrame) + Send + Sync>,
        ) -> Result<Box<dyn AudioStreamHandle>, LiveCaptureError> {
            Ok(Box::new(CaptionTestStream))
        }
    }

    fn dictation_resolution_settings() -> AppSettings {
        AppSettings {
            capture_microphones: vec![
                CaptureMicrophoneEntry {
                    id: "primary".to_string(),
                    label: "Wired".to_string(),
                    core_audio_uid: Some("primary-uid".to_string()),
                },
                CaptureMicrophoneEntry {
                    id: "secondary".to_string(),
                    label: "Wireless".to_string(),
                    core_audio_uid: Some("secondary-uid".to_string()),
                },
            ],
            ..AppSettings::default()
        }
    }

    fn resolved_dictation_device(entry: &CaptureMicrophoneEntry) -> ResolvedCpalInputDevice {
        ResolvedCpalInputDevice {
            device_id: entry.id.clone(),
            device_name: entry.label.clone(),
            used_fallback_device: false,
            core_audio_uid: entry.core_audio_uid.clone(),
            sample_rate: 48_000,
        }
    }

    #[test]
    fn dictation_accepts_the_same_physical_microphone_on_both_slots() {
        let mut settings = dictation_resolution_settings();
        settings.capture_microphones[0].id = "default".into();
        let distinct =
            resolve_dictation_inputs_with(&settings, |entry| Ok(resolved_dictation_device(entry)));
        assert_eq!(distinct.ready.len(), 2);
        assert!(distinct.failures.is_empty());
        let duplicate = resolve_dictation_inputs_with(&settings, |entry| {
            let mut resolved = resolved_dictation_device(entry);
            resolved.core_audio_uid = Some("same-hardware".into());
            Ok(resolved)
        });
        assert_eq!(duplicate.ready.len(), 2);
        assert!(duplicate.failures.is_empty());
        assert_eq!(
            duplicate
                .ready
                .iter()
                .map(|input| input.input.slot)
                .collect::<Vec<_>>(),
            vec![MicrophoneSlot::Primary, MicrophoneSlot::Secondary],
        );
    }

    #[test]
    fn dictation_start_message_matches_the_slots_that_really_started() {
        assert_eq!(
            dictation_listening_message(&[MicrophoneSlot::Primary, MicrophoneSlot::Secondary])
                .as_deref(),
            Ok("Listening…"),
        );
        assert_eq!(
            dictation_listening_message(&[MicrophoneSlot::Primary]).as_deref(),
            Ok("Listening · Primary only"),
        );
        assert_eq!(
            dictation_listening_message(&[MicrophoneSlot::Secondary]).as_deref(),
            Ok("Listening · Secondary only"),
        );
        assert!(dictation_listening_message(&[]).is_err());
    }

    #[test]
    fn dictation_start_resolution_keeps_ready_secondary_when_primary_fails() {
        let result = resolve_dictation_inputs_with(&dictation_resolution_settings(), |entry| {
            if entry.id == "primary" {
                Err("Primary disconnected".to_string())
            } else {
                Ok(resolved_dictation_device(entry))
            }
        });

        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].input.slot, MicrophoneSlot::Secondary);
        assert_eq!(result.failures[0].slot, MicrophoneSlot::Primary);
    }

    #[test]
    fn dictation_start_resolution_keeps_ready_primary_when_secondary_fails() {
        let result = resolve_dictation_inputs_with(&dictation_resolution_settings(), |entry| {
            if entry.id == "secondary" {
                Err("Secondary disconnected".to_string())
            } else {
                Ok(resolved_dictation_device(entry))
            }
        });

        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].input.slot, MicrophoneSlot::Primary);
        assert_eq!(result.failures[0].slot, MicrophoneSlot::Secondary);
    }

    #[test]
    fn dictation_start_resolution_preserves_slot_order() {
        let result = resolve_dictation_inputs_with(&dictation_resolution_settings(), |entry| {
            Ok(resolved_dictation_device(entry))
        });

        assert_eq!(
            result
                .ready
                .iter()
                .map(|input| input.input.slot)
                .collect::<Vec<_>>(),
            vec![MicrophoneSlot::Primary, MicrophoneSlot::Secondary],
        );
    }

    #[test]
    fn dictation_start_resolution_combines_two_safe_failures() {
        let result = resolve_dictation_inputs_with(&dictation_resolution_settings(), |entry| {
            Err(format!("{} disconnected", entry.label))
        });

        assert_eq!(
            dictation_start_error(&result.failures),
            "Primary: Wired disconnected; Secondary: Wireless disconnected",
        );
    }

    #[test]
    fn dictation_start_resolution_rejects_fallback_devices() {
        let result = resolve_dictation_inputs_with(
            &AppSettings {
                capture_microphones: vec![CaptureMicrophoneEntry {
                    id: "primary".to_string(),
                    label: "Wired".to_string(),
                    core_audio_uid: Some("primary-uid".to_string()),
                }],
                ..AppSettings::default()
            },
            |entry| {
                let mut resolved = resolved_dictation_device(entry);
                resolved.used_fallback_device = true;
                Ok(resolved)
            },
        );

        assert!(result.ready.is_empty());
        assert_eq!(result.failures[0].slot, MicrophoneSlot::Primary);
        assert!(result.failures[0].error.contains("same device"));
    }

    fn active_caption_test_dictation_state() -> (Arc<Mutex<DictationRuntime<CaptionTestInput>>>, u64)
    {
        let mut runtime = DictationRuntime::<CaptionTestInput>::default();
        assert_eq!(
            runtime.handle_shortcut_event(DictationShortcutEvent::Pressed),
            DictationAction::StartRecording
        );
        let operation_id = runtime
            .start_recording(AudioInputConfig {
                device_id: "test".to_string(),
                sample_rate: Some(16_000),
                label_hint: None,
                core_audio_uid: None,
            })
            .expect("test Dictation recording starts");
        assert_eq!(
            runtime.handle_shortcut_event(DictationShortcutEvent::Released),
            DictationAction::StopAndTranscribe
        );
        runtime.stop_recording().expect("test recording stops");
        (Arc::new(Mutex::new(runtime)), operation_id)
    }

    #[test]
    fn dictation_caption_updates_require_an_active_enabled_operation() {
        let (dictation_state, operation_id) = active_caption_test_dictation_state();
        let caption_state = Arc::new(Mutex::new(OverlayCaptionRuntime::default()));
        let mut settings = AppSettings {
            show_floating_overlay: true,
            floating_overlay_position: FloatingOverlayPosition::Top,
            ..AppSettings::default()
        };

        let partial = apply_dictation_caption_update(
            &dictation_state,
            &caption_state,
            operation_id,
            DictationCaptionUpdate::Partial("  딕테이션 중간 자막  ".to_string()),
            &settings,
        )
        .expect("active Dictation partial is shown");
        assert_eq!(partial.source, OverlayCaptionSource::Dictation);
        assert_eq!(partial.text, "딕테이션 중간 자막");

        assert!(
            apply_dictation_caption_update(
                &dictation_state,
                &caption_state,
                operation_id + 1,
                DictationCaptionUpdate::Partial("stale".to_string()),
                &settings,
            )
            .is_none()
        );
        assert!(
            apply_dictation_caption_update(
                &dictation_state,
                &caption_state,
                operation_id,
                DictationCaptionUpdate::Partial("  ".to_string()),
                &settings,
            )
            .is_none()
        );
        assert!(
            apply_dictation_caption_update(
                &dictation_state,
                &caption_state,
                operation_id,
                DictationCaptionUpdate::Final {
                    audio_path: PathBuf::from("/tmp/empty.m4a"),
                    text: " \n ".to_string(),
                },
                &settings,
            )
            .is_none()
        );

        settings.subtitle_source_mode = wakenote::settings::SubtitleSourceMode::LiveTranscription;
        assert!(
            apply_dictation_caption_update(
                &dictation_state,
                &caption_state,
                operation_id,
                DictationCaptionUpdate::Partial("disabled".to_string()),
                &settings,
            )
            .is_none()
        );
        settings.subtitle_source_mode = wakenote::settings::SubtitleSourceMode::Both;

        let final_caption = apply_dictation_caption_update(
            &dictation_state,
            &caption_state,
            operation_id,
            DictationCaptionUpdate::Final {
                audio_path: PathBuf::from("/tmp/dictation.m4a"),
                text: "최종 딕테이션 자막".to_string(),
            },
            &settings,
        )
        .expect("active Dictation final is shown");
        assert_eq!(
            final_caption.phase,
            wakenote::overlay_caption::OverlayCaptionPhase::Final
        );
        assert_eq!(final_caption.text, "최종 딕테이션 자막");
        assert!(
            apply_dictation_caption_update(
                &dictation_state,
                &caption_state,
                operation_id,
                DictationCaptionUpdate::Partial("late partial".to_string()),
                &settings,
            )
            .is_none()
        );

        assert!(dictation_state.lock().unwrap().cancel_active());
        assert!(
            apply_dictation_caption_update(
                &dictation_state,
                &caption_state,
                operation_id,
                DictationCaptionUpdate::Partial("cancelled".to_string()),
                &settings,
            )
            .is_none()
        );
    }

    #[test]
    fn subtitle_preview_tokens_keep_only_the_latest_preview_active() {
        let state = AtomicU64::new(0);
        assert!(!subtitle_preview_is_active(&state));

        let first = begin_subtitle_preview(&state);
        assert!(subtitle_preview_is_active(&state));
        let second = begin_subtitle_preview(&state);
        assert_ne!(first, second);

        assert!(!finish_subtitle_preview(&state, first));
        assert!(subtitle_preview_is_active(&state));
        assert!(finish_subtitle_preview(&state, second));
        assert!(!subtitle_preview_is_active(&state));
    }

    #[test]
    fn stale_hidden_caption_publication_selects_the_current_visible_react_payload() {
        let mut runtime = OverlayCaptionRuntime::default();
        let style = AppSettings::default().floating_overlay_caption_style();
        runtime.show_partial(
            7,
            "older caption",
            FloatingOverlayPosition::Top,
            style.clone(),
        );
        runtime.hide();
        let stale_hidden = runtime.snapshot();

        runtime.show_partial(8, "newer caption", FloatingOverlayPosition::Top, style);
        let current_visible = runtime.snapshot();
        let published =
            caption_snapshot_for_publication(stale_hidden.clone(), Some(current_visible.clone()));

        assert_eq!(published, current_visible);
        assert!(published.visible);
        assert_eq!(published.text, "newer caption");
        assert!(!runtime.hide_if_generation(stale_hidden.generation));
        assert_eq!(runtime.snapshot(), current_visible);
    }

    fn dictionary_entry(id: &str, term: &str, aliases: &[&str]) -> DictionaryEntry {
        DictionaryEntry {
            id: id.to_string(),
            term: term.to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            enabled: true,
        }
    }

    #[test]
    fn dictionary_file_state_applies_valid_poll_as_complete_patch() {
        let mut backend = AppBackend::default();
        let entries = vec![dictionary_entry("dictionary-1", "WakeNote", &["wake note"])];

        let outcome = apply_dictionary_poll(&mut backend, DictionaryPoll::Changed(entries.clone()));

        assert!(matches!(outcome, DictionaryApplyOutcome::Changed(_)));
        assert_eq!(backend.settings().dictionary, entries);
    }

    #[test]
    fn dictionary_file_state_invalid_poll_preserves_backend_settings() {
        let existing = vec![dictionary_entry("dictionary-1", "WakeNote", &["wake note"])];
        let mut backend = AppBackend::default();
        backend.update_settings(SettingsPatch {
            dictionary: Some(existing.clone()),
            ..Default::default()
        });
        let error = wakenote::dictionary_file::parse_dictionary_text("WakeNote\nwakenote\n", &[])
            .unwrap_err();

        let outcome = apply_dictionary_poll(&mut backend, DictionaryPoll::Invalid(error));

        assert!(matches!(outcome, DictionaryApplyOutcome::Invalid(_)));
        assert_eq!(backend.settings().dictionary, existing);
    }

    #[test]
    fn dictionary_file_state_keeps_saved_settings_when_file_rewrite_fails() {
        let directory = tempfile::tempdir().unwrap();
        let blocking_parent = directory.path().join("not-a-directory");
        std::fs::write(&blocking_parent, "file").unwrap();
        let mut store = DictionaryFileStore::new(blocking_parent.join("dictionary.txt"));
        let mut backend = AppBackend::default();
        let entries = vec![dictionary_entry("dictionary-1", "WakeNote", &["wake note"])];

        let settings = backend.update_settings(SettingsPatch {
            dictionary: Some(entries.clone()),
            ..Default::default()
        });
        assert!(store.save_entries(&settings.dictionary).is_err());

        assert_eq!(backend.settings().dictionary, entries);
        assert!(!store.status().in_sync);
    }

    #[test]
    fn dictionary_file_state_startup_applies_existing_valid_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictionary.txt");
        std::fs::write(&path, "WakeNote = wake note, wake-note\n").unwrap();
        let mut backend = AppBackend::default();

        let (store, outcome) = initialize_dictionary_file(&mut backend, path);

        assert!(matches!(outcome, DictionaryApplyOutcome::Changed(_)));
        assert_eq!(backend.settings().dictionary[0].term, "WakeNote");
        assert!(store.status().in_sync);
    }

    #[test]
    fn dictionary_file_state_startup_invalid_file_keeps_settings_and_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictionary.txt");
        std::fs::write(&path, "WakeNote\nwakenote\n").unwrap();
        let existing = vec![dictionary_entry(
            "dictionary-9",
            "Qwen3 ASR",
            &["qwen 3 asr"],
        )];
        let mut backend = AppBackend::default();
        backend.update_settings(SettingsPatch {
            dictionary: Some(existing.clone()),
            ..Default::default()
        });

        let (store, outcome) = initialize_dictionary_file(&mut backend, path);

        assert!(matches!(outcome, DictionaryApplyOutcome::Invalid(_)));
        assert_eq!(backend.settings().dictionary, existing);
        assert_eq!(store.status().error_line, Some(2));
    }

    #[test]
    fn overlay_settings_change_detects_active_dictation_opacity_and_position() {
        let previous = AppSettings::default();
        let mut next = previous.clone();
        next.dictation_bubble_background_opacity = 24;
        assert!(overlay_presentation_settings_changed(&previous, &next));

        let mut repositioned = previous.clone();
        repositioned.dictation_bubble_position =
            wakenote::settings::DictationBubblePosition::BottomRight;
        assert!(overlay_presentation_settings_changed(
            &previous,
            &repositioned
        ));
    }

    #[test]
    fn overlay_settings_change_ignores_unrelated_preferences() {
        let previous = AppSettings::default();
        let mut next = previous.clone();
        next.autoplay_next_transcript = !previous.autoplay_next_transcript;
        assert!(!overlay_presentation_settings_changed(&previous, &next));
    }

    #[test]
    fn active_dictation_stage_maps_to_a_refreshable_overlay_presentation() {
        assert_eq!(
            dictation_overlay_presentation(DictationStage::Recording),
            Some((overlay::DictationOverlayState::Recording, "Listening…"))
        );
        assert_eq!(
            dictation_overlay_presentation(DictationStage::Transcribing),
            Some((
                overlay::DictationOverlayState::Transcribing,
                "Transcribing…"
            ))
        );
        assert_eq!(dictation_overlay_presentation(DictationStage::Idle), None);
    }

    #[test]
    fn recognized_source_infos_use_default_auto_prompt() {
        let settings = AppSettings::default();
        let infos = recognized_source_infos(&settings);
        assert_eq!(infos.len(), 3);
        let meet = infos.iter().find(|i| i.id == "meet").expect("meet listed");
        let zoom = infos.iter().find(|i| i.id == "zoom").expect("zoom listed");
        let youtube = infos
            .iter()
            .find(|i| i.id == "youtube")
            .expect("youtube listed");
        assert_eq!(meet.label, "Google Meet");
        assert_eq!(
            meet.description,
            "Google Meet tabs and floating meet.google.com call windows"
        );
        assert!(meet.auto_prompt);
        assert_eq!(
            meet.title_patterns,
            vec!["google meet", "meet - ", "meet.google.com"]
        );
        assert_eq!(zoom.label, "Zoom");
        assert!(zoom.auto_prompt);
        assert_eq!(zoom.title_patterns, vec!["zoom", "zoom meeting"]);
        assert!(youtube.auto_prompt);
        assert_eq!(youtube.title_patterns, vec!["- youtube", "youtube"]);
        assert!(!youtube.custom);
    }

    #[test]
    fn recognized_source_infos_honor_user_override() {
        let settings = AppSettings {
            source_auto_prompt: vec![SourceAutoPromptEntry {
                source_id: "youtube".into(),
                auto_prompt: true,
            }],
            ..Default::default()
        };
        let infos = recognized_source_infos(&settings);
        let youtube = infos
            .iter()
            .find(|i| i.id == "youtube")
            .expect("youtube listed");
        assert!(youtube.auto_prompt);
    }

    #[test]
    fn recognized_source_infos_include_custom_sources() {
        let mut settings = AppSettings::default();
        settings.custom_sources = vec![wakenote::settings::CustomSourceEntry {
            id: "spotify".into(),
            label: "Spotify".into(),
            title_patterns: vec!["Spotify".into()],
            auto_prompt: true,
        }];

        let infos = recognized_source_infos(&settings);
        let spotify = infos
            .iter()
            .find(|i| i.id == "spotify")
            .expect("spotify listed");

        assert_eq!(spotify.label, "Spotify");
        assert!(spotify.custom);
        assert_eq!(spotify.title_patterns, vec!["Spotify"]);
        assert!(spotify.auto_prompt);
    }

    #[test]
    fn recording_source_label_uses_custom_label_and_builtin_id() {
        let custom = DetectedSource {
            source_id: "custom-source-2".into(),
            label: "Spotify".into(),
            app_name: "Spotify".into(),
            pid: 42,
        };
        let builtin = DetectedSource {
            source_id: "youtube".into(),
            label: "YouTube".into(),
            app_name: "Google Chrome".into(),
            pid: 43,
        };

        assert_eq!(recording_source_label(&custom), "Spotify");
        assert_eq!(recording_source_label(&builtin), "youtube");
    }

    #[test]
    fn browser_sources_use_target_app_capture_scope() {
        let youtube = DetectedSource {
            source_id: "youtube".into(),
            label: "YouTube".into(),
            app_name: "Google Chrome".into(),
            pid: 42,
        };
        let meet = DetectedSource {
            source_id: "meet".into(),
            label: "Google Meet".into(),
            app_name: "Google Chrome".into(),
            pid: 43,
        };
        let zoom = DetectedSource {
            source_id: "zoom".into(),
            label: "Zoom".into(),
            app_name: "zoom.us".into(),
            pid: 45,
        };
        let spotify = DetectedSource {
            source_id: "custom-source-2".into(),
            label: "Spotify".into(),
            app_name: "Spotify".into(),
            pid: 44,
        };

        assert_eq!(source_capture_scope(&youtube), "target-app");
        assert_eq!(source_capture_scope(&meet), "target-app");
        assert_eq!(source_capture_scope(&zoom), "target-app");
        assert_eq!(source_capture_scope(&spotify), "target-app");
    }

    #[test]
    fn system_capture_action_is_none_without_active_session() {
        let settings = AppSettings::default();
        assert_eq!(
            system_capture_settings_action(&settings, false),
            SystemCaptureSettingsAction::None
        );
    }

    #[test]
    fn system_capture_action_syncs_while_enabled_and_recording() {
        let settings = AppSettings {
            system_audio_enabled: true,
            ..Default::default()
        };
        assert!(settings.recording_enabled);
        assert!(!settings.pause_all);
        assert_eq!(
            system_capture_settings_action(&settings, true),
            SystemCaptureSettingsAction::Sync
        );
    }

    #[test]
    fn system_capture_action_stops_when_feature_disabled() {
        let settings = AppSettings::default();
        assert_eq!(
            system_capture_settings_action(&settings, true),
            SystemCaptureSettingsAction::Stop
        );
    }

    #[test]
    fn system_capture_action_stops_when_recording_off_or_paused() {
        let recording_off = AppSettings {
            recording_enabled: false,
            ..Default::default()
        };
        assert_eq!(
            system_capture_settings_action(&recording_off, true),
            SystemCaptureSettingsAction::Stop
        );

        let paused = AppSettings {
            pause_all: true,
            ..Default::default()
        };
        assert_eq!(
            system_capture_settings_action(&paused, true),
            SystemCaptureSettingsAction::Stop
        );
    }

    #[test]
    fn input_monitor_action_starts_only_when_enabled_and_live_input_running() {
        let settings = AppSettings {
            input_monitoring_enabled: true,
            ..Default::default()
        };

        assert_eq!(
            input_monitor_runtime_action(&settings, true, false),
            InputMonitorRuntimeAction::Start
        );
        assert_eq!(
            input_monitor_runtime_action(&settings, false, false),
            InputMonitorRuntimeAction::Unchanged
        );
    }

    #[test]
    fn input_monitor_action_stops_when_disabled_or_live_input_stops() {
        let mut settings = AppSettings::default();
        assert_eq!(
            input_monitor_runtime_action(&settings, true, true),
            InputMonitorRuntimeAction::Stop
        );

        settings.input_monitoring_enabled = true;
        assert_eq!(
            input_monitor_runtime_action(&settings, false, true),
            InputMonitorRuntimeAction::Stop
        );
    }

    #[test]
    fn source_watcher_enumerates_windows_only_after_screen_recording_grant() {
        let mut settings = AppSettings {
            system_audio_enabled: true,
            ..Default::default()
        };

        assert!(!source_watcher_should_enumerate_windows(
            &settings,
            permissions::PermissionGrantStatus::NotDetermined
        ));
        assert!(!source_watcher_should_enumerate_windows(
            &settings,
            permissions::PermissionGrantStatus::Denied
        ));
        assert!(source_watcher_should_enumerate_windows(
            &settings,
            permissions::PermissionGrantStatus::Granted
        ));

        settings.system_audio_enabled = false;
        assert!(!source_watcher_should_enumerate_windows(
            &settings,
            permissions::PermissionGrantStatus::Granted
        ));
    }

    #[test]
    fn automatic_meeting_sources_and_microphone_lease_ownership_are_explicit() {
        assert!(source_uses_automatic_meeting_capture("meet"));
        assert!(source_uses_automatic_meeting_capture("zoom"));
        assert!(!source_uses_automatic_meeting_capture("youtube"));

        let automatic = SourceMeetingMicrophoneLease {
            source_id: "meet".into(),
            generation: 1,
            auto_started: true,
            user_owned: false,
        };
        assert!(source_meeting_microphone_should_stop(&automatic));
        assert!(!source_meeting_microphone_should_stop(
            &SourceMeetingMicrophoneLease {
                user_owned: true,
                ..automatic.clone()
            }
        ));
        assert!(!source_meeting_microphone_should_stop(
            &SourceMeetingMicrophoneLease {
                auto_started: false,
                user_owned: true,
                ..automatic
            }
        ));
    }

    #[test]
    fn source_watcher_diagnostics_retain_app_shape_without_meeting_titles() {
        let windows = vec![wakenote::source_watcher::WindowSnapshot {
            title: "Confidential customer weekly sync".into(),
            app_name: "Google Chrome".into(),
            pid: 42,
        }];

        let summary = redacted_source_window_summary(&windows);

        assert!(summary.contains("Google Chrome"));
        assert!(summary.contains("other-title"));
        assert!(!summary.contains("Confidential"));
        assert!(!summary.contains("customer"));
    }

    #[test]
    fn source_capture_start_decision_blocks_duplicate_starts() {
        let now = Instant::now();
        let starting = SourceCaptureLifecycle::Starting {
            source_id: "meet".into(),
            attempt: 1,
        };
        let running = SourceCaptureLifecycle::Running {
            source_id: "meet".into(),
        };

        assert_eq!(
            source_capture_start_decision(&starting, "meet", now),
            SourceCaptureStartDecision::AlreadyStarting
        );
        assert_eq!(
            source_capture_start_decision(&running, "meet", now),
            SourceCaptureStartDecision::AlreadyRunning
        );
    }

    #[test]
    fn source_capture_start_decision_backs_off_after_failed_start() {
        let now = Instant::now();
        let failed = SourceCaptureLifecycle::Failed {
            source_id: "meet".into(),
            failed_at: now,
            attempts: 1,
        };

        assert_eq!(
            source_capture_start_decision(&failed, "meet", now + Duration::from_secs(1)),
            SourceCaptureStartDecision::BackingOff {
                retry_after: Duration::from_secs(1),
            }
        );
        assert_eq!(
            source_capture_start_decision(&failed, "meet", now + Duration::from_secs(2)),
            SourceCaptureStartDecision::Start { attempt: 2 }
        );
    }

    #[test]
    fn source_capture_retry_delay_steps_then_caps() {
        assert_eq!(source_capture_retry_delay(1), Duration::from_secs(2));
        assert_eq!(source_capture_retry_delay(2), Duration::from_secs(5));
        assert_eq!(source_capture_retry_delay(3), Duration::from_secs(10));
        assert_eq!(source_capture_retry_delay(8), Duration::from_secs(10));
    }

    #[test]
    fn source_capture_lifecycle_preserves_failed_attempt_count() {
        let now = Instant::now();
        let mut lifecycle = SourceCaptureLifecycle::Idle;

        source_capture_mark_starting(&mut lifecycle, "meet", 1);
        assert_eq!(
            lifecycle,
            SourceCaptureLifecycle::Starting {
                source_id: "meet".into(),
                attempt: 1,
            }
        );

        source_capture_mark_failed(&mut lifecycle, "meet", now);
        assert_eq!(
            lifecycle,
            SourceCaptureLifecycle::Failed {
                source_id: "meet".into(),
                failed_at: now,
                attempts: 1,
            }
        );

        source_capture_mark_starting(&mut lifecycle, "meet", 2);
        source_capture_mark_failed(&mut lifecycle, "meet", now + Duration::from_secs(3));
        assert_eq!(
            source_capture_start_decision(&lifecycle, "meet", now + Duration::from_secs(7)),
            SourceCaptureStartDecision::BackingOff {
                retry_after: Duration::from_secs(1),
            }
        );
    }

    #[test]
    fn failed_source_capture_requests_recovery_only_for_the_same_source() {
        let lifecycle = SourceCaptureLifecycle::Failed {
            source_id: "youtube".into(),
            failed_at: Instant::now(),
            attempts: 1,
        };

        assert!(source_capture_recovery_pending(&lifecycle, "youtube"));
        assert!(should_auto_capture_source(
            true,
            source_capture_recovery_pending(&lifecycle, "youtube"),
            false,
            false,
        ));
        assert!(!source_capture_recovery_pending(&lifecycle, "meet"));
        assert!(!source_capture_recovery_pending(
            &SourceCaptureLifecycle::Idle,
            "youtube"
        ));
    }

    #[test]
    fn source_capture_failure_diagnostic_includes_permission_and_retry_context() {
        let message = source_capture_failure_diagnostic(
            "meet",
            2,
            permissions::PermissionGrantStatus::Granted,
            "system-audio capture failed: cpal error: timed out querying shareable content elapsed_ms=10000",
        );

        assert_eq!(
            message,
            "[source-capture] error source_id=meet attempt=2 screen_recording_status=Granted next_retry_ms=5000 error=system-audio capture failed: cpal error: timed out querying shareable content elapsed_ms=10000"
        );
    }

    #[test]
    fn dictation_preloads_the_effective_dictation_whisper_model() {
        let mut settings = AppSettings::default();
        assert_eq!(dictation_model_to_preload(&settings), None);

        settings.dictation_enabled = true;
        settings.selected_model = "whisper-medium".to_string();
        assert_eq!(
            dictation_model_to_preload(&settings),
            Some("whisper-medium".to_string())
        );

        settings.selected_model = "qwen3-asr-0.6b".to_string();
        settings.dictation_model = "whisper-small".to_string();
        assert_eq!(
            dictation_model_to_preload(&settings),
            Some("whisper-small".to_string())
        );

        settings.selected_model = "whisper-medium".to_string();
        settings.dictation_model = "qwen3-asr-0.6b".to_string();
        assert_eq!(dictation_model_to_preload(&settings), None);
    }

    #[test]
    fn dictation_permission_guidance_blocks_only_the_relevant_boundaries() {
        let recording = dictation_recording_permission_guidance(
            DictationShortcutEvent::Pressed,
            DictationStage::Idle,
            PermissionGrantStatus::Denied,
        );
        assert_eq!(
            recording,
            Some(PermissionGuidancePayload::dictation_recording())
        );
        assert_eq!(
            dictation_recording_permission_guidance(
                DictationShortcutEvent::Pressed,
                DictationStage::Idle,
                PermissionGrantStatus::Granted,
            ),
            None
        );
        assert_eq!(
            dictation_recording_permission_guidance(
                DictationShortcutEvent::Released,
                DictationStage::Idle,
                PermissionGrantStatus::Denied,
            ),
            None
        );
        assert_eq!(
            dictation_recording_permission_guidance(
                DictationShortcutEvent::Pressed,
                DictationStage::Recording,
                PermissionGrantStatus::Denied,
            ),
            None
        );

        assert_eq!(
            dictation_insertion_permission_guidance(PermissionGrantStatus::Restricted),
            Some(PermissionGuidancePayload::dictation_insertion())
        );
        assert_eq!(
            dictation_insertion_permission_guidance(PermissionGrantStatus::Granted),
            None
        );
    }

    struct FakeSystemAudioStream;

    impl AudioStreamHandle for FakeSystemAudioStream {}

    struct FailedSystemAudioStream;

    impl AudioStreamHandle for FailedSystemAudioStream {
        fn runtime_error(&self) -> Option<String> {
            Some("ScreenCaptureKit stream stopped".to_string())
        }
    }

    #[test]
    fn failed_system_audio_stream_is_removed_for_watcher_recovery() {
        let state: SystemCaptureState =
            Arc::new(Mutex::new(Some(Box::new(FailedSystemAudioStream))));

        assert!(!source_capture_stream_is_healthy(&state));
        assert_eq!(
            take_source_capture_runtime_error(&state).as_deref(),
            Some("ScreenCaptureKit stream stopped")
        );
        assert!(state.lock().expect("stream state").is_none());
    }

    struct FakeSourceCaptureStarter {
        calls: usize,
        outcomes: std::collections::VecDeque<Result<(), LiveCaptureError>>,
    }

    impl FakeSourceCaptureStarter {
        fn new(outcomes: Vec<Result<(), LiveCaptureError>>) -> Self {
            Self {
                calls: 0,
                outcomes: outcomes.into(),
            }
        }
    }

    impl SourceCaptureStreamStarter for FakeSourceCaptureStarter {
        fn start_source_capture_stream(
            &mut self,
            source: &DetectedSource,
            sample_rate: u32,
            _on_frame: Arc<dyn Fn(AudioFrame) + Send + Sync>,
        ) -> Result<Box<dyn AudioStreamHandle>, LiveCaptureError> {
            self.calls += 1;
            assert_eq!(source.source_id, "meet");
            assert_eq!(source.app_name, "Google Chrome");
            assert_eq!(sample_rate, PIPELINE_SAMPLE_RATE);
            match self.outcomes.pop_front().expect("fake outcome") {
                Ok(()) => Ok(Box::new(FakeSystemAudioStream)),
                Err(error) => Err(error),
            }
        }
    }

    #[test]
    fn source_capture_start_failure_backoff_retry_preserves_existing_queue() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let queued_audio = tmp.path().join("existing.wav");
        std::fs::write(&queued_audio, b"existing audio").expect("queued audio");

        let mut backend = AppBackend::default();
        backend.update_settings(SettingsPatch {
            save_root: Some(tmp.path().to_string_lossy().to_string()),
            audio_format: Some(wakenote::settings::AudioFormat::Wav),
            transcription_enabled: Some(true),
            ..SettingsPatch::default()
        });
        let queued_before = backend.enqueue_audio_file(&queued_audio, None);

        let backend_state: BackendState = Arc::new(Mutex::new(backend));
        let system_capture_state: SystemCaptureState = Arc::new(Mutex::new(None));
        let lifecycle_state: SourceCaptureLifecycleState =
            Arc::new(Mutex::new(SourceCaptureLifecycle::Idle));
        let source = DetectedSource {
            source_id: "meet".into(),
            label: "Google Meet".into(),
            app_name: "Google Chrome".into(),
            pid: 42,
        };
        let on_frame: Arc<dyn Fn(AudioFrame) + Send + Sync> = Arc::new(|_| {});
        let frame_generation = AtomicU64::new(0);
        let now = Instant::now();
        let mut starter = FakeSourceCaptureStarter::new(vec![
            Err(LiveCaptureError::Cpal(
                "timed out starting system-audio capture elapsed_ms=10000".into(),
            )),
            Ok(()),
        ]);

        let error = attempt_source_capture_start(
            &source,
            &backend_state,
            &system_capture_state,
            &lifecycle_state,
            on_frame.clone(),
            &frame_generation,
            &mut starter,
            now,
            permissions::PermissionGrantStatus::Granted,
        )
        .expect_err("first ScreenCaptureKit start fails");

        assert!(error.contains("system-audio capture failed"));
        assert_eq!(starter.calls, 1);
        assert!(system_capture_state.lock().expect("stream state").is_none());
        {
            let backend = backend_state.lock().expect("backend");
            assert!(!backend.is_system_capturing());
            assert_eq!(backend.queue_snapshot(), queued_before);
        }
        assert!(wakenote::meeting::list_meetings(tmp.path()).is_empty());
        assert!(matches!(
            &*lifecycle_state.lock().expect("lifecycle"),
            SourceCaptureLifecycle::Failed { source_id, attempts, .. }
                if source_id == "meet" && *attempts == 1
        ));

        let deferred = attempt_source_capture_start(
            &source,
            &backend_state,
            &system_capture_state,
            &lifecycle_state,
            on_frame.clone(),
            &frame_generation,
            &mut starter,
            now + Duration::from_secs(1),
            permissions::PermissionGrantStatus::Granted,
        )
        .expect("retry inside backoff is deferred");

        assert_eq!(deferred.outcome, SourceCaptureAttemptOutcome::Deferred);
        assert_eq!(starter.calls, 1);
        assert!(system_capture_state.lock().expect("stream state").is_none());
        assert_eq!(
            backend_state.lock().expect("backend").queue_snapshot(),
            queued_before
        );

        let started = attempt_source_capture_start(
            &source,
            &backend_state,
            &system_capture_state,
            &lifecycle_state,
            on_frame,
            &frame_generation,
            &mut starter,
            now + Duration::from_secs(2),
            permissions::PermissionGrantStatus::Granted,
        )
        .expect("retry after backoff starts capture");

        assert_eq!(started.outcome, SourceCaptureAttemptOutcome::Started);
        assert_eq!(starter.calls, 2);
        assert!(system_capture_state.lock().expect("stream state").is_some());
        {
            let backend = backend_state.lock().expect("backend");
            assert!(backend.is_system_capturing());
            assert_eq!(backend.queue_snapshot(), queued_before);
        }
        assert!(matches!(
            &*lifecycle_state.lock().expect("lifecycle"),
            SourceCaptureLifecycle::Running { source_id } if source_id == "meet"
        ));
    }

    #[test]
    fn finalized_meet_capture_is_recorded_once_without_auto_transcription() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut backend = AppBackend::default();
        backend.update_settings(SettingsPatch {
            save_root: Some(tmp.path().to_string_lossy().to_string()),
            transcription_enabled: Some(true),
            ..SettingsPatch::default()
        });
        let base_time = chrono::Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap();

        backend
            .start_system_capture_session(
                16_000,
                base_time,
                "Google Chrome".into(),
                "meet".into(),
                "meet".into(),
            )
            .expect("start meet system capture");
        backend
            .process_system_audio_frame(AudioFrame {
                samples: vec![0.5; 1_600],
                duration_ms: 100,
                captured_at: base_time + chrono::Duration::milliseconds(100),
            })
            .expect("meet audio frame");
        backend
            .stop_system_capture_session()
            .expect("stop meet system capture");

        let meetings = wakenote::meeting::list_meetings(tmp.path());
        let finished = backend.take_finished_system_meetings();

        assert_eq!(meetings.len(), 1);
        assert_eq!(
            meetings[0].status,
            wakenote::meeting::MeetingStatus::Recorded
        );
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].id, meetings[0].id);
        assert_eq!(
            finished[0].status,
            wakenote::meeting::MeetingStatus::Recorded
        );
        assert!(backend.take_finished_system_meetings().is_empty());
        assert!(pending_meeting_job_actions(&backend.settings()).is_empty());
    }

    #[test]
    fn finalized_meet_capture_stays_recorded_when_meeting_model_is_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut backend = AppBackend::default();
        backend.update_settings(SettingsPatch {
            save_root: Some(tmp.path().to_string_lossy().to_string()),
            model_directory: Some(tmp.path().join("models").to_string_lossy().to_string()),
            transcription_enabled: Some(true),
            ..SettingsPatch::default()
        });
        let base_time = chrono::Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap();

        backend
            .start_system_capture_session(
                16_000,
                base_time,
                "Google Chrome".into(),
                "meet".into(),
                "meet".into(),
            )
            .expect("start meet system capture");
        backend
            .process_system_audio_frame(AudioFrame {
                samples: vec![0.5; 1_600],
                duration_ms: 100,
                captured_at: base_time + chrono::Duration::milliseconds(100),
            })
            .expect("meet audio frame");
        backend
            .stop_system_capture_session()
            .expect("stop meet system capture");

        let meetings = wakenote::meeting::list_meetings(tmp.path());
        assert_eq!(meetings.len(), 1);
        assert_eq!(
            meetings[0].status,
            wakenote::meeting::MeetingStatus::Recorded
        );
        assert_eq!(backend.take_finished_system_meetings().len(), 1);
        assert!(pending_meeting_job_actions(&backend.settings()).is_empty());
    }

    #[test]
    fn busy_meeting_worker_queues_auto_capture_and_claims_it_on_completion() {
        let mut runtime = MeetingRuntime::default();
        let job = |id: &str| MeetingJobSpec {
            id: id.into(),
            save_root: PathBuf::from("/tmp/meetings"),
            model_directory: PathBuf::from("/tmp/models"),
            model_id: "whisper-medium".into(),
            suppress_low_confidence: false,
            dictionary: DictionaryContext::default(),
        };

        let (first_outcome, first_cancel) = schedule_meeting_job(&mut runtime, job("first"), false)
            .expect("idle worker accepts first meeting");
        assert_eq!(first_outcome, MeetingJobScheduleOutcome::Started);
        assert!(first_cancel.is_some());

        let (second_outcome, second_cancel) =
            schedule_meeting_job(&mut runtime, job("second"), true)
                .expect("automatic meeting queues behind busy worker");
        assert_eq!(second_outcome, MeetingJobScheduleOutcome::Queued);
        assert!(second_cancel.is_none());
        assert_eq!(runtime.current.as_deref(), Some("first"));

        let (next_job, next_cancel) = complete_meeting_job(&mut runtime, "first")
            .expect("queued meeting is claimed when current worker finishes");
        assert_eq!(next_job.id, "second");
        assert!(!next_cancel.load(Ordering::Acquire));
        assert_eq!(runtime.current.as_deref(), Some("second"));
        assert!(runtime.cancel.is_some());

        assert!(complete_meeting_job(&mut runtime, "second").is_none());
        assert!(runtime.current.is_none());
        assert!(runtime.cancel.is_none());
    }

    #[test]
    fn cloud_meeting_models_are_ready_without_local_artifacts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for model_id in ["openrouter-qwen3-asr-flash", "openai-gpt-transcribe"] {
            let job = MeetingJobSpec {
                id: "cloud".into(),
                save_root: tmp.path().to_path_buf(),
                model_directory: tmp.path().join("models"),
                model_id: model_id.into(),
                suppress_low_confidence: true,
                dictionary: DictionaryContext::default(),
            };
            assert!(meeting_job_model_is_ready(&job));
        }
    }

    #[test]
    fn meeting_request_validation_accepts_realtime_and_artifact_backed_unloaded_models() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let backend = AppBackend::default();
        let models = backend.model_registry();
        let realtime = models
            .iter()
            .find(|model| model.id == "soniox-realtime-v5")
            .expect("realtime model");
        let mut unready = models
            .iter()
            .find(|model| model.id == "whisper-medium")
            .expect("local model")
            .clone();
        unready.status = ModelStatus::Missing;
        let request = StartMeetingTranscriptionRequest {
            language_hints: None,
            model_id: realtime.id.clone(),
            language: TranscriptionLanguage::Auto,
            streaming_enabled: true,
            speaker_separation_enabled: false,
        };

        assert_eq!(
            validate_meeting_transcription_request(realtime, tmp.path(), &request),
            Ok(MeetingTranscriptionRequest {
                language_hints: wakenote::settings::default_transcription_language_hints(),
                model_id: realtime.id.clone(),
                language: TranscriptionLanguage::Auto,
                streaming_enabled: true,
                speaker_separation_enabled: false,
            })
        );
        assert!(
            validate_meeting_transcription_request(
                &unready,
                tmp.path(),
                &StartMeetingTranscriptionRequest {
                    language_hints: None,
                    model_id: unready.id.clone(),
                    ..request.clone()
                },
            )
            .expect_err("unready local must fail")
            .contains("not currently runnable")
        );

        let mut unloaded = unready;
        unloaded.status = ModelStatus::Unloaded;
        unloaded.checksum_sha256 = None;
        std::fs::create_dir_all(tmp.path()).expect("model directory");
        std::fs::write(tmp.path().join(format!("{}.bin", unloaded.id)), b"artifact")
            .expect("model artifact");
        assert!(
            validate_meeting_transcription_request(
                &unloaded,
                tmp.path(),
                &StartMeetingTranscriptionRequest {
                    language_hints: None,
                    model_id: unloaded.id.clone(),
                    ..request
                },
            )
            .is_ok()
        );
    }

    #[test]
    fn meeting_request_validation_preserves_supported_diarization() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let backend = AppBackend::default();
        let model = backend
            .model_registry()
            .into_iter()
            .find(|model| model.id == "openai-gpt-4o-transcribe-diarize")
            .expect("diarization model");
        let request = StartMeetingTranscriptionRequest {
            language_hints: None,
            model_id: model.id.clone(),
            language: TranscriptionLanguage::Ko,
            streaming_enabled: true,
            speaker_separation_enabled: true,
        };

        assert_eq!(
            validate_meeting_transcription_request(&model, tmp.path(), &request),
            Ok(MeetingTranscriptionRequest {
                language_hints: wakenote::settings::default_transcription_language_hints(),
                model_id: model.id,
                language: TranscriptionLanguage::Ko,
                streaming_enabled: true,
                speaker_separation_enabled: true,
            })
        );
    }

    #[test]
    fn saved_meeting_job_uses_its_request_model_not_the_live_model() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = tmp.path().join("meeting.wav");
        std::fs::write(&source, b"fake wav").expect("source");
        let imported = wakenote::meeting::import_meeting(
            tmp.path(),
            &source,
            "soniox-realtime-v5",
            TranscriptionLanguage::Auto,
            "test",
            chrono::Local
                .with_ymd_and_hms(2026, 8, 10, 12, 0, 0)
                .unwrap(),
        )
        .expect("import");
        wakenote::meeting::start_recorded_meeting_transcription(
            tmp.path(),
            &imported.id,
            MeetingTranscriptionRequest {
                language_hints: wakenote::settings::default_transcription_language_hints(),
                model_id: "soniox-async-v5".into(),
                language: TranscriptionLanguage::Ko,
                streaming_enabled: false,
                speaker_separation_enabled: false,
            },
        )
        .expect("configure");
        let settings = AppSettings {
            save_root: tmp.path().to_string_lossy().to_string(),
            selected_model: "soniox-realtime-v5".into(),
            ..AppSettings::default()
        };

        let job = meeting_job_spec_for_saved_record(imported.id, &settings).expect("job");

        assert_eq!(job.model_id, "soniox-async-v5");
    }

    #[test]
    fn queued_meeting_keeps_its_own_capture_configuration() {
        let mut runtime = MeetingRuntime::default();
        let first = MeetingJobSpec {
            id: "first".into(),
            save_root: PathBuf::from("/tmp/first-root"),
            model_directory: PathBuf::from("/tmp/first-models"),
            model_id: "first-model".into(),
            suppress_low_confidence: false,
            dictionary: DictionaryContext::default(),
        };
        let second = MeetingJobSpec {
            id: "second".into(),
            save_root: PathBuf::from("/tmp/second-root"),
            model_directory: PathBuf::from("/tmp/second-models"),
            model_id: "second-model".into(),
            suppress_low_confidence: true,
            dictionary: DictionaryContext::default(),
        };

        schedule_meeting_job(&mut runtime, first, false).expect("start first");
        schedule_meeting_job(&mut runtime, second, true).expect("queue second");

        let (next, _) = complete_meeting_job(&mut runtime, "first").expect("promote second");
        assert_eq!(next.id, "second");
        assert_eq!(next.save_root, PathBuf::from("/tmp/second-root"));
        assert_eq!(next.model_directory, PathBuf::from("/tmp/second-models"));
        assert_eq!(next.model_id, "second-model");
        assert!(next.suppress_low_confidence);
    }

    #[test]
    fn automatic_meeting_queue_is_fifo_deduplicated_and_ignores_stale_completion() {
        let mut runtime = MeetingRuntime::default();
        let job = |id: &str| MeetingJobSpec {
            id: id.into(),
            save_root: PathBuf::from("/tmp/meetings"),
            model_directory: PathBuf::from("/tmp/models"),
            model_id: "whisper-medium".into(),
            suppress_low_confidence: false,
            dictionary: DictionaryContext::default(),
        };

        schedule_meeting_job(&mut runtime, job("first"), false).expect("start first");
        schedule_meeting_job(&mut runtime, job("second"), true).expect("queue second");
        schedule_meeting_job(&mut runtime, job("third"), true).expect("queue third");
        let (duplicate, cancel) =
            schedule_meeting_job(&mut runtime, job("second"), true).expect("deduplicate second");
        assert_eq!(duplicate, MeetingJobScheduleOutcome::AlreadyScheduled);
        assert!(cancel.is_none());
        assert_eq!(runtime.queued.len(), 2);

        assert!(complete_meeting_job(&mut runtime, "stale").is_none());
        assert_eq!(runtime.current.as_deref(), Some("first"));

        let (second, _) = complete_meeting_job(&mut runtime, "first").expect("promote second");
        assert_eq!(second.id, "second");
        let (third, _) = complete_meeting_job(&mut runtime, "second").expect("promote third");
        assert_eq!(third.id, "third");
    }

    #[test]
    fn startup_recovery_queues_pending_meetings_oldest_first_using_record_model() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let model_dir = tmp.path().join("models");
        std::fs::create_dir_all(&model_dir).expect("model dir");
        std::fs::write(model_dir.join("record-model.bin"), b"fake model").expect("model file");
        let source = tmp.path().join("capture.wav");
        std::fs::write(&source, b"fake wav").expect("source");

        let older = wakenote::meeting::import_meeting(
            tmp.path(),
            &source,
            "record-model",
            wakenote::settings::TranscriptionLanguage::Ko,
            "test",
            chrono::Local
                .with_ymd_and_hms(2026, 7, 21, 10, 0, 0)
                .unwrap(),
        )
        .expect("older meeting");
        let newer = wakenote::meeting::import_meeting(
            tmp.path(),
            &source,
            "record-model",
            wakenote::settings::TranscriptionLanguage::Ko,
            "test",
            chrono::Local
                .with_ymd_and_hms(2026, 7, 21, 11, 0, 0)
                .unwrap(),
        )
        .expect("newer meeting");
        for id in [&older.id, &newer.id] {
            wakenote::meeting::start_recorded_meeting_transcription(
                tmp.path(),
                id,
                MeetingTranscriptionRequest {
                    language_hints: wakenote::settings::default_transcription_language_hints(),
                    model_id: "record-model".into(),
                    language: wakenote::settings::TranscriptionLanguage::Ko,
                    streaming_enabled: false,
                    speaker_separation_enabled: false,
                },
            )
            .expect("queue imported meeting");
        }
        let settings = AppSettings {
            save_root: tmp.path().to_string_lossy().to_string(),
            model_directory: model_dir.to_string_lossy().to_string(),
            selected_model: "different-selected-model".into(),
            transcription_enabled: true,
            ..AppSettings::default()
        };

        assert_eq!(
            pending_meeting_job_actions(&settings),
            vec![
                FinishedSystemMeetingJobAction::Start(MeetingJobSpec {
                    id: older.id,
                    save_root: tmp.path().to_path_buf(),
                    model_directory: model_dir.clone(),
                    model_id: "record-model".into(),
                    suppress_low_confidence: settings.suppress_low_confidence_transcripts,
                    dictionary: DictionaryContext::default(),
                }),
                FinishedSystemMeetingJobAction::Start(MeetingJobSpec {
                    id: newer.id,
                    save_root: tmp.path().to_path_buf(),
                    model_directory: model_dir,
                    model_id: "record-model".into(),
                    suppress_low_confidence: settings.suppress_low_confidence_transcripts,
                    dictionary: DictionaryContext::default(),
                }),
            ]
        );
    }

    #[test]
    fn delayed_startup_scan_ignores_pending_meetings_created_after_launch_cutoff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let model_dir = tmp.path().join("models");
        std::fs::create_dir_all(&model_dir).expect("model dir");
        std::fs::write(model_dir.join("record-model.bin"), b"fake model").expect("model file");
        let source = tmp.path().join("capture.wav");
        std::fs::write(&source, b"fake wav").expect("source");

        let before_launch = wakenote::meeting::import_meeting(
            tmp.path(),
            &source,
            "record-model",
            wakenote::settings::TranscriptionLanguage::Ko,
            "test",
            chrono::Local
                .with_ymd_and_hms(2026, 7, 21, 10, 0, 0)
                .unwrap(),
        )
        .expect("meeting before launch");
        wakenote::meeting::import_meeting(
            tmp.path(),
            &source,
            "record-model",
            wakenote::settings::TranscriptionLanguage::Ko,
            "test",
            chrono::Local
                .with_ymd_and_hms(2026, 7, 21, 11, 0, 0)
                .unwrap(),
        )
        .and_then(|record| {
            wakenote::meeting::start_recorded_meeting_transcription(
                tmp.path(),
                &record.id,
                MeetingTranscriptionRequest {
                    language_hints: wakenote::settings::default_transcription_language_hints(),
                    model_id: "record-model".into(),
                    language: wakenote::settings::TranscriptionLanguage::Ko,
                    streaming_enabled: false,
                    speaker_separation_enabled: false,
                },
            )
        })
        .expect("meeting after launch");
        wakenote::meeting::start_recorded_meeting_transcription(
            tmp.path(),
            &before_launch.id,
            MeetingTranscriptionRequest {
                language_hints: wakenote::settings::default_transcription_language_hints(),
                model_id: "record-model".into(),
                language: wakenote::settings::TranscriptionLanguage::Ko,
                streaming_enabled: false,
                speaker_separation_enabled: false,
            },
        )
        .expect("queue meeting before launch");
        let settings = AppSettings {
            save_root: tmp.path().to_string_lossy().to_string(),
            model_directory: model_dir.to_string_lossy().to_string(),
            transcription_enabled: true,
            ..AppSettings::default()
        };

        let actions = pending_meeting_job_actions_before(
            &settings,
            chrono::Utc.with_ymd_and_hms(2026, 7, 21, 1, 30, 0).unwrap(),
        );

        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            FinishedSystemMeetingJobAction::Start(job) if job.id == before_launch.id
        ));
    }

    #[test]
    fn startup_meeting_recovery_does_not_block_the_launch_thread() {
        let (scan_started_tx, scan_started_rx) = mpsc::channel();
        let (release_scan_tx, release_scan_rx) = mpsc::channel();
        let (caller_returned_tx, caller_returned_rx) = mpsc::channel();
        let (actions_applied_tx, actions_applied_rx) = mpsc::channel();

        let caller = thread::spawn(move || {
            let worker = spawn_startup_meeting_recovery_worker(
                move || {
                    scan_started_tx.send(()).expect("announce blocked scan");
                    release_scan_rx.recv().expect("release blocked scan");
                    vec!["pending-meeting"]
                },
                move |actions| {
                    actions_applied_tx
                        .send(actions)
                        .expect("publish recovered actions");
                },
            );
            caller_returned_tx
                .send(worker)
                .expect("announce caller return");
        });

        scan_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("recovery scan started");
        let returned = caller_returned_rx.recv_timeout(Duration::from_millis(100));
        release_scan_tx.send(()).expect("release recovery scan");

        let worker = returned.expect("launch caller returned while scan remained blocked");
        assert_eq!(
            actions_applied_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("recovered actions applied"),
            vec!["pending-meeting"]
        );
        worker.join().expect("recovery worker completed");
        caller.join().expect("caller completed");
    }

    #[test]
    fn tray_presentation_update_payload_is_send_static_for_main_thread_dispatch() {
        fn assert_send_static<T: Send + 'static>(_: &T) {}

        let tmp = tempfile::tempdir().expect("tempdir");
        let backend = AppBackend::load_from_dir(tmp.path()).expect("backend");
        let settings = backend.settings();
        let status = backend.app_status();
        let payload = tray_presentation_update_payload(&settings, &status, DictationStage::Idle);

        assert_send_static(&payload);
        assert_eq!(payload.settings, settings);
        assert_eq!(payload.status, status);
    }

    #[test]
    fn dictation_cues_use_selected_bundled_resources_and_volume() {
        assert_eq!(
            dictation_cue_resource_name(DictationCue::Start, DictationCueSound::Original),
            "dictation-start.wav"
        );
        assert_eq!(
            dictation_cue_resource_name(DictationCue::Stop, DictationCueSound::Original),
            "dictation-stop.wav"
        );
        assert_eq!(
            dictation_cue_resource_name(DictationCue::End, DictationCueSound::Original),
            "dictation-stop.wav"
        );
        assert_eq!(
            dictation_cue_resource_name(DictationCue::Start, DictationCueSound::Alternative),
            "dict-start-0.mp3"
        );
        assert_eq!(
            dictation_cue_resource_name(DictationCue::Stop, DictationCueSound::Alternative),
            "dict-start-0.mp3"
        );
        assert_eq!(
            dictation_cue_resource_name(DictationCue::End, DictationCueSound::Alternative),
            "dict-end-0.mp3"
        );
        assert_eq!(dictation_cue_volume_arg(DictationCueVolume::Muted), None);
        assert_eq!(
            dictation_cue_volume_arg(DictationCueVolume::Small),
            Some("0.55")
        );
        assert_eq!(
            dictation_cue_volume_arg(DictationCueVolume::Medium),
            Some("1.0")
        );
        assert_eq!(
            dictation_cue_volume_arg(DictationCueVolume::Large),
            Some("1.45")
        );

        let settings = AppSettings {
            dictation_end_sound: DictationCueSound::Alternative,
            ..Default::default()
        };
        assert_eq!(
            dictation_cue_sound(DictationCue::End, &settings),
            DictationCueSound::Alternative
        );
    }

    #[test]
    fn dictation_cue_task_returns_before_audio_playback_finishes() {
        let (task_started_tx, task_started_rx) = mpsc::channel();
        let (release_task_tx, release_task_rx) = mpsc::channel();
        let (caller_returned_tx, caller_returned_rx) = mpsc::channel();

        let caller = thread::spawn(move || {
            spawn_dictation_cue_task(move || {
                task_started_tx.send(()).expect("announce cue start");
                release_task_rx.recv().expect("release cue task");
            })
            .expect("cue task starts");
            caller_returned_tx.send(()).expect("announce caller return");
        });

        task_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("cue task started");
        let returned_before_playback_finished =
            caller_returned_rx.recv_timeout(Duration::from_millis(100));
        release_task_tx.send(()).expect("release cue task");
        caller.join().expect("cue caller completed");

        assert!(
            returned_before_playback_finished.is_ok(),
            "shortcut worker must not wait for cue playback"
        );
    }

    #[test]
    fn dictation_stage_and_caption_source_gate_window_mutation() {
        assert!(dictation_allows_caption_window_mutation(
            DictationStage::Transcribing,
            OverlayCaptionSource::Dictation,
        ));
        assert!(!dictation_allows_caption_window_mutation(
            DictationStage::Transcribing,
            OverlayCaptionSource::LiveTranscription,
        ));
        assert!(!dictation_allows_caption_window_mutation(
            DictationStage::Recording,
            OverlayCaptionSource::Dictation,
        ));
        assert!(!dictation_allows_caption_window_mutation(
            DictationStage::Error,
            OverlayCaptionSource::Dictation,
        ));
        assert!(dictation_allows_caption_window_mutation(
            DictationStage::Idle,
            OverlayCaptionSource::LiveTranscription,
        ));
    }

    #[test]
    fn unchanged_tray_presentation_is_not_dispatched_again() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let backend = AppBackend::load_from_dir(tmp.path()).expect("backend");
        let settings = backend.settings();
        let status = backend.app_status();
        let mut cache = None;

        assert!(
            next_tray_presentation_update(&mut cache, &settings, &status, DictationStage::Idle,)
                .is_some()
        );
        assert!(
            next_tray_presentation_update(&mut cache, &settings, &status, DictationStage::Idle,)
                .is_none()
        );
        assert!(
            next_tray_presentation_update(
                &mut cache,
                &settings,
                &status,
                DictationStage::Recording,
            )
            .is_some()
        );
    }

    #[test]
    fn tray_menu_order_starts_with_open_settings_then_separator() {
        assert_eq!(
            tray_menu_order(),
            &[
                "open-settings",
                "separator",
                "toggle-recording",
                "toggle-transcription",
                "toggle-subtitles",
                "toggle-auto-type",
                "active-model",
                "active-microphone",
                "threshold",
                "separator",
                "reveal-save-folder",
                "pause-all",
                "separator",
                "cancel-current-operation",
                "separator",
                "quit",
            ]
        );
    }

    #[test]
    fn tray_left_click_menu_flag_follows_click_action_setting() {
        let mut settings = AppSettings::default();
        assert!(tray_show_menu_on_left_click(&settings));

        settings.tray_left_click_action = wakenote::settings::TrayClickAction::TogglePause;
        assert!(!tray_show_menu_on_left_click(&settings));
    }

    #[test]
    fn main_window_size_restore_detects_zero_sized_window() {
        assert!(main_window_needs_size_restore(0, 0));
    }

    #[test]
    fn main_window_size_restore_detects_too_small_window() {
        assert!(main_window_needs_size_restore(
            100,
            MAIN_WINDOW_MIN_HEIGHT as u32
        ));
        assert!(main_window_needs_size_restore(
            MAIN_WINDOW_MIN_WIDTH as u32,
            100
        ));
    }

    #[test]
    fn main_window_size_restore_keeps_usable_window_size() {
        assert!(!main_window_needs_size_restore(
            MAIN_WINDOW_MIN_WIDTH as u32,
            MAIN_WINDOW_MIN_HEIGHT as u32,
        ));
    }

    #[test]
    fn main_window_default_size_matches_current_settings_window() {
        assert_eq!(MAIN_WINDOW_WIDTH as u32, 1040);
        assert_eq!(MAIN_WINDOW_HEIGHT as u32, 922);
    }

    #[test]
    fn dock_icon_runtime_mode_follows_setting() {
        let mut settings = AppSettings::default();
        assert_eq!(
            dock_icon_runtime_mode(&settings),
            DockIconRuntimeMode::Visible
        );

        settings.show_dock_icon = false;
        assert_eq!(
            dock_icon_runtime_mode(&settings),
            DockIconRuntimeMode::Hidden
        );
    }

    #[test]
    fn hidden_menu_bar_icon_requires_settings_window_on_launch() {
        let mut settings = AppSettings::default();
        assert!(!settings_window_should_open_on_launch(&settings));

        settings.show_tray_icon = false;
        assert!(settings_window_should_open_on_launch(&settings));
    }

    #[test]
    fn close_window_shortcut_hides_settings_window() {
        assert_eq!(
            app_menu_action(CLOSE_SETTINGS_WINDOW_MENU_ID),
            AppMenuAction::HideSettingsWindow
        );
        assert_eq!(app_menu_action("open-settings"), AppMenuAction::Noop);
    }

    #[test]
    fn quit_menu_action_marks_intentional_quit() {
        assert_eq!(app_menu_action("quit"), AppMenuAction::IntentionalQuit);
    }

    #[test]
    fn unexpected_exit_request_is_prevented_for_tray_app() {
        assert_eq!(
            exit_request_decision(None, true, true, false),
            ExitRequestDecision::Prevent
        );
    }

    #[test]
    fn explicit_or_intentional_exit_request_is_allowed() {
        assert_eq!(
            exit_request_decision(Some(0), true, true, false),
            ExitRequestDecision::Allow
        );
        assert_eq!(
            exit_request_decision(None, true, true, true),
            ExitRequestDecision::Allow
        );
    }

    #[test]
    fn exit_request_is_allowed_when_tray_icon_is_disabled() {
        assert_eq!(
            exit_request_decision(None, false, true, false),
            ExitRequestDecision::Allow
        );
    }

    #[test]
    fn exit_request_is_allowed_when_runtime_is_idle() {
        assert_eq!(
            exit_request_decision(None, true, false, false),
            ExitRequestDecision::Allow
        );
    }

    #[test]
    fn reopen_request_always_shows_settings_window() {
        let mut settings = AppSettings::default();
        assert_eq!(
            settings_window_request_for_reopen(Some(&settings)),
            SettingsWindowRequest::Show {
                show_dock_icon: true
            }
        );

        settings.show_dock_icon = false;
        assert_eq!(
            settings_window_request_for_reopen(Some(&settings)),
            SettingsWindowRequest::Show {
                show_dock_icon: false
            }
        );
        assert_eq!(
            settings_window_request_for_reopen(None),
            SettingsWindowRequest::Show {
                show_dock_icon: true
            }
        );
    }

    #[test]
    fn launch_auto_start_uses_retry_delays_only_when_enabled() {
        let settings = AppSettings {
            permission_onboarding_seen: true,
            ..AppSettings::default()
        };

        assert_eq!(
            launch_auto_start_retry_delay_secs(&settings),
            &[2, 5, 10, 20, 30, 60]
        );

        assert!(
            launch_auto_start_retry_delay_secs(&AppSettings {
                start_live_input_on_launch: false,
                permission_onboarding_seen: true,
                ..AppSettings::default()
            })
            .is_empty()
        );
        assert!(
            launch_auto_start_retry_delay_secs(&AppSettings {
                recording_enabled: false,
                permission_onboarding_seen: true,
                ..AppSettings::default()
            })
            .is_empty()
        );
        assert!(
            launch_auto_start_retry_delay_secs(&AppSettings {
                pause_all: true,
                permission_onboarding_seen: true,
                ..AppSettings::default()
            })
            .is_empty()
        );
    }

    #[test]
    fn live_capture_permission_allows_only_a_granted_microphone() {
        assert_eq!(
            live_capture_permission_result(PermissionGrantStatus::Granted),
            Ok(())
        );

        for status in [
            PermissionGrantStatus::Unknown,
            PermissionGrantStatus::NotDetermined,
            PermissionGrantStatus::Denied,
            PermissionGrantStatus::Restricted,
            PermissionGrantStatus::Unsupported,
        ] {
            let error = live_capture_permission_result(status).expect_err("unavailable");
            assert_eq!(error, LiveCaptureStartError::MicrophonePermission(status));
            assert!(!launch_auto_start_should_retry(&error));
            assert!(error.to_string().contains("WakeNote Settings > Audio"));
        }
    }

    #[test]
    fn launch_auto_start_retries_non_permission_capture_failures() {
        assert!(launch_auto_start_should_retry(
            &LiveCaptureStartError::Runtime("device warming up".to_string())
        ));
    }

    #[test]
    fn recent_transcript_limit_defaults_and_clamps_for_runtime_command() {
        assert_eq!(
            normalize_recent_transcript_limit(None),
            DEFAULT_RECENT_TRANSCRIPT_LIMIT
        );
        assert_eq!(normalize_recent_transcript_limit(Some(0)), 0);
        assert_eq!(normalize_recent_transcript_limit(Some(12)), 12);
        assert_eq!(
            normalize_recent_transcript_limit(Some(MAX_RECENT_TRANSCRIPT_LIMIT + 1)),
            MAX_RECENT_TRANSCRIPT_LIMIT
        );
    }

    #[test]
    fn fallback_microphones_returns_default_device_without_fallback_for_default_selection() {
        let devices = fallback_microphones(&AppSettings::default());

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "default");
        assert!(!devices[0].fallback);
    }

    #[test]
    fn fallback_microphones_marks_default_as_fallback_for_pinned_selection() {
        let devices = fallback_microphones(&AppSettings {
            selected_microphone: "input-0-external".to_string(),
            ..AppSettings::default()
        });

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "default");
        assert!(devices[0].fallback);
    }

    #[test]
    fn fallback_microphones_preserves_configured_capture_devices() {
        let settings = AppSettings {
            selected_microphone: "input-primary".to_string(),
            selected_microphone_label: "Studio Mic".to_string(),
            capture_microphones: vec![
                wakenote::settings::CaptureMicrophoneEntry {
                    id: "input-primary".to_string(),
                    label: "Studio Mic".to_string(),
                    core_audio_uid: None,
                },
                wakenote::settings::CaptureMicrophoneEntry {
                    id: "input-secondary".to_string(),
                    label: "Desk Mic".to_string(),
                    core_audio_uid: None,
                },
            ],
            ..AppSettings::default()
        };

        let devices = fallback_microphones(&settings);

        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].id, "default");
        assert_eq!(devices[1].id, "input-primary");
        assert_eq!(devices[1].label, "Studio Mic");
        assert!(!devices[1].available);
        assert_eq!(devices[2].id, "input-secondary");
        assert_eq!(devices[2].label, "Desk Mic");
        assert!(!devices[2].available);
    }

    #[test]
    fn stable_microphones_hide_a_stale_duplicate_when_one_same_name_device_is_active() {
        let settings = AppSettings {
            capture_microphones: vec![wakenote::settings::CaptureMicrophoneEntry {
                id: "input-4-boya-cm40".to_string(),
                label: "BOYA CM40".to_string(),
                core_audio_uid: Some("uid-stale".to_string()),
            }],
            ..AppSettings::default()
        };
        let devices = stable_microphone_options(
            &settings,
            vec![MicrophoneDevice {
                transport_type: None,
                id: "input-2-boya-cm40".to_string(),
                label: "BOYA CM40".to_string(),
                core_audio_uid: Some("uid-active".to_string()),
                available: true,
                fallback: false,
            }],
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "input-2-boya-cm40");
        assert!(devices[0].available);
    }

    #[test]
    fn stable_microphones_keep_a_stale_entry_when_same_name_rebinding_is_ambiguous() {
        let settings = AppSettings {
            capture_microphones: vec![wakenote::settings::CaptureMicrophoneEntry {
                id: "input-4-boya-cm40".to_string(),
                label: "BOYA CM40".to_string(),
                core_audio_uid: Some("uid-stale".to_string()),
            }],
            ..AppSettings::default()
        };
        let devices = stable_microphone_options(
            &settings,
            vec![
                MicrophoneDevice {
                    transport_type: None,
                    id: "input-1-boya-cm40".to_string(),
                    label: "BOYA CM40".to_string(),
                    core_audio_uid: Some("uid-a".to_string()),
                    available: true,
                    fallback: false,
                },
                MicrophoneDevice {
                    transport_type: None,
                    id: "input-2-boya-cm40".to_string(),
                    label: "BOYA CM40".to_string(),
                    core_audio_uid: Some("uid-b".to_string()),
                    available: true,
                    fallback: false,
                },
            ],
        );

        assert_eq!(devices.len(), 3);
        assert_eq!(devices[2].id, "input-4-boya-cm40");
        assert!(!devices[2].available);
    }
}
