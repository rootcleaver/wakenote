use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Local, NaiveDate, NaiveTime, TimeZone, Utc};

use crate::audio::{
    InputDevice, LevelMonitor, LevelSnapshot, MIC_NONZERO_DBFS, MicHealthAction, MicHealthMonitor,
    MicHealthVerdict, dbfs_from_samples, list_input_devices,
};
use crate::capture::{CaptureController, CaptureControllerConfig, CaptureControllerEvent};
use crate::cloud_transcription::TranscriptionCredentials;
use crate::debug_log::{append_debug_log, append_debug_log_nonblocking};
use crate::dictation::DictationStage;
use crate::dictionary::DictionaryContext;
use crate::live_capture::AudioFrame;
use crate::manual_meeting_capture::{
    MANUAL_MEETING_SAMPLE_RATE, ManualMeetingFrameSender, ManualMeetingSource, ManualMeetingWriter,
};
use crate::meeting::{MeetingFinishedEvent, MeetingStatus, start_manual_recorded_meeting_capture};
use crate::models::{
    ModelDescriptor, ModelStatus, ModelStore, default_model_registry, model_supports_reprocessing,
    validate_model_options,
};
use crate::multi_capture::{MicrophoneMixMode, MicrophoneMixer};
use crate::persistence::{
    AppPersistence, ListVisibilityState, PersistenceError, SetListVisibilityRequest,
};
use crate::queue::{
    BacklogScan, COMPLETED_JOB_HISTORY_LIMIT, QueueActivityKind, QueueJob, QueueJobStatus,
    QueueSnapshot, TranscriptionQueue, is_importable_audio_path,
};
use crate::recorder::{
    ChunkMetadata, ChunkSource, RecordedChunk, TranscriptionSidecar, TranscriptionStatus,
    microphone_output_label,
};
use crate::settings::{
    AppSettings, MicrophoneSlot, SettingsPatch, TranscriptionLanguage, TranscriptionOptions,
    expand_user_path, microphone_source_key,
};
use crate::storage::copy_uploaded_audio_file;
use crate::transcription::{
    FallbackTranscriber, RuntimeTranscriber, Transcriber, TranscriptionJobOutcome,
    TranscriptionWorker, TranscriptionWorkerOptions, apply_outcome, model_supports_live_partials,
};
use crate::trash::{TrashedRecordingBundle, recording_bundle_paths, restore_recording_bundle};

/// How many recently committed chunk_ids we keep around for audio_path -> chunk_id
/// reverse-lookup after a queue worker finishes. Bounded to avoid unbounded growth
/// over a long session.
const LIVE_CHUNK_HISTORY_LIMIT: usize = 256;

/// Events surfaced from the capture pipeline that the runtime should forward
/// to the frontend (or to the live transcription service).
#[derive(Debug, Clone)]
pub enum LiveTranscriptEvent {
    Discarded {
        source_key: String,
        chunk_id: u64,
    },
    Started {
        source_key: String,
        source_label: String,
        microphone_slot: Option<MicrophoneSlot>,
        chunk_id: u64,
        started_at: DateTime<Utc>,
        overlay_position: crate::settings::FloatingOverlayPosition,
    },
    SamplesReady {
        source_key: String,
        source_label: String,
        microphone_slot: Option<MicrophoneSlot>,
        chunk_id: u64,
        model_id: String,
        language: TranscriptionLanguage,
        suppress_low_confidence_transcripts: bool,
        sample_rate: u32,
        samples: Arc<Vec<f32>>,
    },
    Committed {
        source_key: String,
        source_label: String,
        microphone_slot: Option<MicrophoneSlot>,
        chunk_id: u64,
        model_id: Option<String>,
        audio_path: PathBuf,
        overlay_position: crate::settings::FloatingOverlayPosition,
        will_transcribe: bool,
    },
}

pub type LiveEventHandler = Arc<dyn Fn(LiveTranscriptEvent) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    RecordingAndTranscription,
    RecordingOnly,
    TranscriptionOnly,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayState {
    Idle,
    Listening,
    Recording,
    Transcribing,
    Paused,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayPresentation {
    pub rgba: [u8; 4],
    pub tooltip: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayIconImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayRuntimePresentation {
    pub icon: TrayPresentation,
    pub visible: bool,
}

const TRAY_ICON_NORMAL_RGBA: [u8; 4] = [0, 0, 0, 255];
const TRAY_ICON_RECORDING_RGBA: [u8; 4] = [22, 163, 74, 255];
const TRAY_ICON_TRANSCRIBING_RGBA: [u8; 4] = [217, 119, 6, 255];
const TRAY_ICON_DISCONNECTED_RGBA: [u8; 4] = [220, 38, 38, 255];
const TRAY_ICON_IMAGE_SIZE: u32 = 64;
const TRAY_ICON_DOT_DIAMETER: u32 = TRAY_ICON_IMAGE_SIZE / 2;
const TRANSCRIPT_DAY_INDEX_SCHEMA_VERSION: u32 = 2;
const DICTATION_DUPLICATE_INTERVAL_GRACE_MS: i64 = 2_000;
const DICTATION_DUPLICATE_START_FALLBACK_MS: i64 = 5_000;
static TRANSCRIPT_DAY_INDEX_REFRESH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const TRANSCRIPT_DAY_INDEX_FILE_NAME: &str = "all.json";

pub fn tray_presentation_for_state(state: TrayState) -> TrayPresentation {
    match state {
        TrayState::Idle => TrayPresentation {
            rgba: TRAY_ICON_NORMAL_RGBA,
            tooltip: "WakeNote: Idle",
        },
        TrayState::Listening => TrayPresentation {
            rgba: TRAY_ICON_NORMAL_RGBA,
            tooltip: "WakeNote: Listening",
        },
        TrayState::Recording => TrayPresentation {
            rgba: TRAY_ICON_RECORDING_RGBA,
            tooltip: "WakeNote: Recording",
        },
        TrayState::Transcribing => TrayPresentation {
            rgba: TRAY_ICON_TRANSCRIBING_RGBA,
            tooltip: "WakeNote: Transcribing",
        },
        TrayState::Paused => TrayPresentation {
            rgba: TRAY_ICON_NORMAL_RGBA,
            tooltip: "WakeNote: Paused",
        },
        TrayState::Error => TrayPresentation {
            rgba: TRAY_ICON_DISCONNECTED_RGBA,
            tooltip: "WakeNote: Error",
        },
    }
}

pub fn tray_presentation_for_dictation(stage: DictationStage) -> Option<TrayPresentation> {
    match stage {
        DictationStage::Recording => Some(TrayPresentation {
            rgba: TRAY_ICON_RECORDING_RGBA,
            tooltip: "WakeNote: Dictation recording",
        }),
        DictationStage::Transcribing => Some(TrayPresentation {
            rgba: TRAY_ICON_TRANSCRIBING_RGBA,
            tooltip: "WakeNote: Dictation transcribing",
        }),
        DictationStage::Error => Some(TrayPresentation {
            rgba: TRAY_ICON_DISCONNECTED_RGBA,
            tooltip: "WakeNote: Dictation failed",
        }),
        DictationStage::Idle => None,
    }
}

pub fn tray_icon_image_for_presentation(presentation: TrayPresentation) -> TrayIconImage {
    let width = TRAY_ICON_IMAGE_SIZE;
    let height = TRAY_ICON_IMAGE_SIZE;
    let radius = TRAY_ICON_DOT_DIAMETER as f32 / 2.0;
    let center = TRAY_ICON_IMAGE_SIZE as f32 / 2.0;
    let mut rgba = vec![0; (width * height * 4) as usize];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let distance = dx.mul_add(dx, dy * dy).sqrt();
            let alpha = if distance <= radius - 0.5 {
                255
            } else if distance < radius + 0.5 {
                ((radius + 0.5 - distance) * 255.0).round() as u8
            } else {
                0
            };
            if alpha > 0 {
                let offset = ((y * width + x) * 4) as usize;
                rgba[offset..offset + 3].copy_from_slice(&presentation.rgba[..3]);
                rgba[offset + 3] = alpha;
            }
        }
    }

    TrayIconImage {
        rgba,
        width,
        height,
    }
}

pub fn tray_runtime_presentation(
    settings: &AppSettings,
    status: &AppStatus,
) -> TrayRuntimePresentation {
    let mut icon = tray_presentation_for_state(status.tray_state);
    if tray_status_has_microphone_connection_failure(status) {
        icon.rgba = TRAY_ICON_DISCONNECTED_RGBA;
        icon.tooltip = "WakeNote: Microphone disconnected";
    }

    TrayRuntimePresentation {
        icon,
        visible: settings.show_tray_icon,
    }
}

fn tray_status_has_microphone_connection_failure(status: &AppStatus) -> bool {
    !status.live_input_active
        && (status.microphone_warning.is_some()
            || status
                .microphone_captures
                .iter()
                .any(|microphone| microphone.warning.is_some())
            || status
                .runtime_warning
                .as_deref()
                .map(|warning| warning.starts_with("Live input stream error:"))
                .unwrap_or(false))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuPresentation {
    pub recording_checked: bool,
    pub recording_text: &'static str,
    pub transcription_checked: bool,
    pub subtitles_checked: bool,
    pub auto_type_checked: bool,
    pub auto_type_text: &'static str,
    pub pause_all_checked: bool,
    pub active_model_text: String,
    pub active_microphone_text: String,
    pub threshold_text: String,
}

pub fn tray_menu_presentation(settings: &AppSettings, status: &AppStatus) -> TrayMenuPresentation {
    TrayMenuPresentation {
        recording_checked: settings.recording_enabled,
        recording_text: if settings.recording_enabled {
            "Recording On"
        } else {
            "Recording Off"
        },
        transcription_checked: settings.transcription_enabled,
        subtitles_checked: settings.show_floating_overlay,
        auto_type_checked: settings.auto_transcript_input_enabled,
        auto_type_text: if settings.auto_transcript_input_enabled {
            "Auto Type On"
        } else {
            "Auto Type Off"
        },
        pause_all_checked: settings.pause_all,
        active_model_text: format!("Model: {}", status.active_model),
        active_microphone_text: format!("Microphone: {}", status.active_microphone),
        threshold_text: format!("Threshold: {:.0} dBFS", status.threshold_dbfs),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealSaveFolderRequest {
    pub program: PathBuf,
    pub path: PathBuf,
    pub reveal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainWindowCloseAction {
    HideToTray,
    AllowClose,
}

pub fn main_window_close_action(window_label: &str) -> MainWindowCloseAction {
    if window_label == "main" {
        MainWindowCloseAction::HideToTray
    } else {
        MainWindowCloseAction::AllowClose
    }
}

fn system_capture_meeting_title(source_id: &str) -> Option<&'static str> {
    match source_id {
        "meet" => Some("Google Meet"),
        "zoom" => Some("Zoom"),
        _ => None,
    }
}

/// Decide whether the device we actually opened diverges from the user's
/// pinned selection enough to warrant a "fallback in use" warning. The
/// `cpal` stable id embeds the host enumeration index, which is NOT stable
/// across sleep/wake or USB re-enumeration on macOS — the same physical mic
/// can end up with a different id while keeping its label. The label is the
/// durable identity, so a label match is enough evidence that we are on
/// the user's intended device and no warning should fire.
pub fn pinned_device_mismatch(
    pinned_id: &str,
    pinned_label: &str,
    resolved_id: &str,
    resolved_label: &str,
    resolver_fell_back: bool,
) -> bool {
    if resolver_fell_back {
        return true;
    }
    if pinned_id == resolved_id {
        return false;
    }
    if !pinned_label.is_empty() && pinned_label == resolved_label {
        return false;
    }
    true
}

/// Rewrite any settings entry whose `id == requested_id` to point at
/// `resolved_id` when the label still matches. Closes the drift where
/// `cpal` re-enumeration assigns the same physical mic a different stable
/// id, leaving stale ids in `selected_microphone` and the priority list.
/// Pure — does not persist; the caller is responsible for `persist_settings`.
pub fn reconcile_device_id_in_settings(
    settings: &mut AppSettings,
    requested_id: &str,
    resolved_id: &str,
    resolved_label: &str,
) -> bool {
    if requested_id == resolved_id || resolved_label.is_empty() {
        return false;
    }
    let mut changed = false;
    if settings.selected_microphone == requested_id
        && settings.selected_microphone_label == resolved_label
    {
        settings.selected_microphone = resolved_id.to_string();
        changed = true;
    }
    for entry in settings.microphone_priority.iter_mut() {
        if entry.id == requested_id && entry.label == resolved_label {
            entry.id = resolved_id.to_string();
            changed = true;
        }
    }
    changed
}

pub fn reveal_save_folder_request(settings: &AppSettings) -> RevealSaveFolderRequest {
    RevealSaveFolderRequest {
        program: PathBuf::from("/usr/bin/open"),
        path: expand_user_path(&settings.save_root),
        reveal: false,
    }
}

pub fn open_containing_folder_request(
    path: impl AsRef<Path>,
) -> Result<RevealSaveFolderRequest, String> {
    let expanded = expand_user_path(path.as_ref().to_string_lossy());
    let (path, reveal) = if expanded.is_file() {
        (expanded, true)
    } else if expanded.is_dir() {
        (expanded, false)
    } else {
        (
            expanded
                .parent()
                .ok_or_else(|| "path has no containing folder".to_string())?
                .to_path_buf(),
            false,
        )
    };
    if !reveal && !path.is_dir() {
        return Err("containing folder does not exist".to_string());
    }
    Ok(RevealSaveFolderRequest {
        program: PathBuf::from("/usr/bin/open"),
        path,
        reveal,
    })
}

pub fn audio_playback_content_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("m4a") => Some("audio/mp4"),
        Some("mp3") => Some("audio/mpeg"),
        Some("wav") => Some("audio/wav"),
        _ => None,
    }
}

pub fn validate_audio_playback_file(path: &Path) -> Result<&'static str, String> {
    let content_type = audio_playback_content_type(path)
        .ok_or_else(|| "only mp3, m4a, and wav recordings can be played".to_string())?;
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("recording path is not a file".to_string());
    }
    if metadata.len() == 0 {
        return Err("recording file is empty".to_string());
    }
    ensure_audio_file_readable(path)?;
    Ok(content_type)
}

fn ensure_audio_file_readable(path: &Path) -> Result<(), String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("recording file is not readable: {error}"))?;
    let mut saw_bytes = false;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => saw_bytes = true,
            Err(error) => return Err(format!("recording file is not readable: {error}")),
        }
    }

    if !saw_bytes {
        return Err("recording file is empty".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicrophoneDevice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_type: Option<u32>,
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_audio_uid: Option<String>,
    pub available: bool,
    pub fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SilenceWarning {
    /// User-visible label of the device that has been silent.
    pub device_label: String,
    /// How long the input has been digitally silent, in whole seconds.
    pub seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStatus {
    pub mode: AppMode,
    pub tray_state: TrayState,
    pub live_input_active: bool,
    pub active_model: String,
    pub active_microphone: String,
    pub microphone_warning: Option<String>,
    pub silence_warning: Option<SilenceWarning>,
    pub runtime_warning: Option<String>,
    pub threshold_dbfs: f32,
    pub level: LevelSnapshot,
    pub microphone_captures: Vec<MicrophoneCaptureStatus>,
    pub queue: QueueSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicrophoneCaptureStatus {
    pub slot: MicrophoneSlot,
    pub device_id: String,
    pub label: String,
    pub active: bool,
    pub reconnecting: bool,
    pub warning: Option<String>,
    pub level: LevelSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentTranscript {
    pub transcript_path: String,
    pub audio_path: Option<String>,
    pub recorded_at: String,
    pub text: String,
    pub source: ChunkSource,
    pub source_label: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub microphone_slot: Option<MicrophoneSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptDay {
    pub day: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TranscriptDayIndex {
    pub schema_version: u32,
    pub day: String,
    pub generated_at: String,
    pub source_files: Vec<TranscriptDayIndexSourceFile>,
    pub entries: Vec<RecentTranscript>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct TranscriptDayIndexSourceFile {
    pub path: String,
    pub len: u64,
    pub modified_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadedAudio {
    pub audio_path: String,
    pub original_filename: String,
    pub stored_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashActivityJobFailure {
    pub id: u64,
    pub audio_path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashActivityJobsResult {
    pub queue: QueueSnapshot,
    pub removed_ids: Vec<u64>,
    pub trashed_ids: Vec<u64>,
    pub missing_ids: Vec<u64>,
    pub failures: Vec<TrashActivityJobFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashTranscriptFailure {
    pub transcript_path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashTranscriptsResult {
    pub removed_transcript_paths: Vec<PathBuf>,
    pub trashed_transcript_paths: Vec<PathBuf>,
    pub missing_transcript_paths: Vec<PathBuf>,
    pub failures: Vec<TrashTranscriptFailure>,
}

#[derive(Debug, Clone)]
pub struct StartedTranscriptionJob {
    pub live_metadata: Option<Arc<ChunkMetadata>>,
    pub job: crate::queue::QueueJob,
    pub model_directory: std::path::PathBuf,
    pub language: TranscriptionLanguage,
    pub language_hints: Vec<TranscriptionLanguage>,
    pub suppress_low_confidence_transcripts: bool,
    pub dictionary: DictionaryContext,
    pub transcription_options: TranscriptionOptions,
    pub fallback_supports_diarization: bool,
    pub diarization_required: bool,
    pub credentials: Result<TranscriptionCredentials, String>,
}

#[derive(Debug, Clone)]
pub struct PreparedModelDownload {
    pub model_directory: std::path::PathBuf,
    pub model: ModelDescriptor,
    pub registry: Vec<ModelDescriptor>,
}

/// One-shot device override staged by the recovery watchdog. Carries the
/// stable device id plus the persisted label, so the resolver can still
/// reattach to the same physical mic when its enumeration index has moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicRecoveryOverride {
    pub device_id: String,
    pub label_hint: Option<String>,
}

struct SystemMeetingCapture {
    generation: u64,
    meeting_id: String,
    writer: ManualMeetingWriter,
    microphone_sender: ManualMeetingFrameSender,
    system_sender: ManualMeetingFrameSender,
    received_audio: bool,
}

pub struct AppBackend {
    live_webhook: Option<Arc<crate::live_webhook::WebhookService>>,
    pending_live_metadata: HashMap<u64, Arc<ChunkMetadata>>,
    settings: AppSettings,
    queue: TranscriptionQueue,
    capture: Option<CaptureController>,
    secondary_capture: Option<CaptureController>,
    microphone_mixer: Option<MicrophoneMixer>,
    /// Parallel capture session for system-audio frames. Mirrors `capture`
    /// but has no level/health monitoring and tags chunks `source = System`.
    system_capture: Option<CaptureController>,
    system_meeting_capture: Option<SystemMeetingCapture>,
    finished_system_meetings: Vec<MeetingFinishedEvent>,
    next_system_meeting_generation: u64,
    last_system_audio_frame_at: Option<Instant>,
    level_monitor: LevelMonitor,
    secondary_level_monitor: LevelMonitor,
    active_microphone_id: Option<String>,
    active_microphone_label: Option<String>,
    active_microphone_sample_rate: Option<u32>,
    active_secondary_microphone_id: Option<String>,
    active_secondary_microphone_label: Option<String>,
    active_secondary_microphone_sample_rate: Option<u32>,
    microphone_warning: Option<String>,
    secondary_microphone_warning: Option<String>,
    silence_warning: Option<SilenceWarning>,
    persistence: Option<AppPersistence>,
    live_event_handler: Option<LiveEventHandler>,
    pending_live_events: Vec<LiveTranscriptEvent>,
    chunk_id_history: VecDeque<(PathBuf, u64)>,
    chunk_id_index: HashMap<PathBuf, u64>,
    live_transcription_suspended_for_dictation: bool,
    dictation_suppressed_live_chunks: HashSet<(String, u64)>,
    active_transcription_jobs: BTreeSet<u64>,
    mic_health: MicHealthMonitor,
    /// One-shot device override used by the recovery watchdog to force a
    /// restart onto a specific device (e.g. system default) on the next
    /// capture start. Consumed by `take_microphone_recovery_override`.
    mic_recovery_override: Option<MicRecoveryOverride>,
}

impl std::fmt::Debug for AppBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppBackend")
            .field("settings", &self.settings)
            .field("queue", &self.queue)
            .field("capture", &self.capture)
            .field("secondary_capture", &self.secondary_capture)
            .field("microphone_mixer", &self.microphone_mixer)
            .field("system_capture", &self.system_capture)
            .field(
                "system_meeting_capture",
                &self.system_meeting_capture.is_some(),
            )
            .field("finished_system_meetings", &self.finished_system_meetings)
            .field(
                "last_system_audio_frame_at",
                &self.last_system_audio_frame_at,
            )
            .field("level_monitor", &self.level_monitor)
            .field("secondary_level_monitor", &self.secondary_level_monitor)
            .field("active_microphone_id", &self.active_microphone_id)
            .field("active_microphone_label", &self.active_microphone_label)
            .field(
                "active_microphone_sample_rate",
                &self.active_microphone_sample_rate,
            )
            .field(
                "active_secondary_microphone_label",
                &self.active_secondary_microphone_label,
            )
            .field(
                "active_secondary_microphone_sample_rate",
                &self.active_secondary_microphone_sample_rate,
            )
            .field("microphone_warning", &self.microphone_warning)
            .field(
                "secondary_microphone_warning",
                &self.secondary_microphone_warning,
            )
            .field("silence_warning", &self.silence_warning)
            .field("persistence", &self.persistence)
            .field(
                "live_event_handler",
                &self.live_event_handler.as_ref().map(|_| "<handler>"),
            )
            .field("pending_live_events_len", &self.pending_live_events.len())
            .field("chunk_id_history_len", &self.chunk_id_history.len())
            .field(
                "live_transcription_suspended_for_dictation",
                &self.live_transcription_suspended_for_dictation,
            )
            .field(
                "dictation_suppressed_live_chunks",
                &self.dictation_suppressed_live_chunks,
            )
            .field("active_transcription_jobs", &self.active_transcription_jobs)
            .field("mic_health", &self.mic_health)
            .field("mic_recovery_override", &self.mic_recovery_override)
            .finish()
    }
}

impl Default for AppBackend {
    fn default() -> Self {
        Self {
            live_webhook: None,
            pending_live_metadata: HashMap::new(),
            settings: AppSettings::default(),
            queue: TranscriptionQueue::new(),
            capture: None,
            secondary_capture: None,
            microphone_mixer: None,
            system_capture: None,
            system_meeting_capture: None,
            finished_system_meetings: Vec::new(),
            next_system_meeting_generation: 0,
            last_system_audio_frame_at: None,
            level_monitor: LevelMonitor::default(),
            secondary_level_monitor: LevelMonitor::default(),
            active_microphone_id: None,
            active_microphone_label: None,
            active_microphone_sample_rate: None,
            active_secondary_microphone_id: None,
            active_secondary_microphone_label: None,
            active_secondary_microphone_sample_rate: None,
            microphone_warning: None,
            secondary_microphone_warning: None,
            silence_warning: None,
            persistence: None,
            live_event_handler: None,
            pending_live_events: Vec::new(),
            chunk_id_history: VecDeque::new(),
            chunk_id_index: HashMap::new(),
            live_transcription_suspended_for_dictation: false,
            dictation_suppressed_live_chunks: HashSet::new(),
            active_transcription_jobs: BTreeSet::new(),
            mic_health: MicHealthMonitor::default(),
            mic_recovery_override: None,
        }
    }
}

impl AppBackend {
    pub fn load_from_dir(root: impl AsRef<std::path::Path>) -> Result<Self, PersistenceError> {
        let persistence = AppPersistence::new(root);
        persistence.protect_private_storage()?;
        let mut settings = persistence.load_settings()?.unwrap_or_default();
        let models = model_registry_snapshot(expand_user_path(&settings.model_directory));
        settings.transcription_options = validate_model_options(
            &models,
            &settings.selected_model,
            &settings.transcription_options,
        );
        Ok(Self {
            live_webhook: None,
            pending_live_metadata: HashMap::new(),
            settings,
            queue: persistence.load_queue()?.unwrap_or_default(),
            capture: None,
            secondary_capture: None,
            microphone_mixer: None,
            system_capture: None,
            system_meeting_capture: None,
            finished_system_meetings: Vec::new(),
            next_system_meeting_generation: 0,
            last_system_audio_frame_at: None,
            level_monitor: LevelMonitor::default(),
            secondary_level_monitor: LevelMonitor::default(),
            active_microphone_id: None,
            active_microphone_label: None,
            active_microphone_sample_rate: None,
            active_secondary_microphone_id: None,
            active_secondary_microphone_label: None,
            active_secondary_microphone_sample_rate: None,
            microphone_warning: None,
            secondary_microphone_warning: None,
            silence_warning: None,
            persistence: Some(persistence),
            live_event_handler: None,
            pending_live_events: Vec::new(),
            chunk_id_history: VecDeque::new(),
            chunk_id_index: HashMap::new(),
            live_transcription_suspended_for_dictation: false,
            dictation_suppressed_live_chunks: HashSet::new(),
            active_transcription_jobs: BTreeSet::new(),
            mic_health: MicHealthMonitor::default(),
            mic_recovery_override: None,
        })
    }

    pub fn set_live_event_handler(&mut self, handler: LiveEventHandler) {
        self.live_event_handler = Some(handler);
    }

    pub fn live_event_handler(&self) -> Option<LiveEventHandler> {
        self.live_event_handler.clone()
    }

    pub fn drain_live_events(&mut self) -> Vec<LiveTranscriptEvent> {
        std::mem::take(&mut self.pending_live_events)
    }

    pub fn set_live_transcription_suspended_for_dictation(
        &mut self,
        suspended: bool,
    ) -> Vec<String> {
        self.live_transcription_suspended_for_dictation = suspended;
        if !suspended {
            return Vec::new();
        }

        let mut active_sources = Vec::new();
        if self.microphone_mixer.is_some() {
            if let Some(chunk_id) = self
                .capture
                .as_ref()
                .and_then(CaptureController::active_chunk_id)
            {
                let source_key = format!("microphone:{}", self.merged_microphone_device_id());
                self.dictation_suppressed_live_chunks
                    .insert((source_key.clone(), chunk_id));
                active_sources.push(source_key);
            }
            return active_sources;
        }

        for slot in [MicrophoneSlot::Primary, MicrophoneSlot::Secondary] {
            let chunk_id = match slot {
                MicrophoneSlot::Primary => self
                    .capture
                    .as_ref()
                    .and_then(CaptureController::active_chunk_id),
                MicrophoneSlot::Secondary => self
                    .secondary_capture
                    .as_ref()
                    .and_then(CaptureController::active_chunk_id),
            };
            if let Some(chunk_id) = chunk_id {
                let source_key = self.microphone_source_key_for_slot(slot);
                self.dictation_suppressed_live_chunks
                    .insert((source_key.clone(), chunk_id));
                active_sources.push(source_key);
            }
        }
        active_sources
    }

    pub fn take_finished_system_meetings(&mut self) -> Vec<MeetingFinishedEvent> {
        std::mem::take(&mut self.finished_system_meetings)
    }

    pub fn active_system_meeting_generation(&self) -> Option<u64> {
        self.system_meeting_capture
            .as_ref()
            .map(|capture| capture.generation)
    }

    pub fn set_system_meeting_capture_warning(&mut self, warning: impl Into<String>) {
        if let Some(capture) = self.system_meeting_capture.as_ref() {
            let _ = capture.writer.set_capture_warning(warning);
        }
    }

    pub fn chunk_id_for_audio_path(&self, audio_path: &std::path::Path) -> Option<u64> {
        self.chunk_id_index.get(audio_path).copied()
    }

    pub fn active_capture_sample_rate(&self) -> Option<u32> {
        self.capture.as_ref().map(|capture| capture.sample_rate())
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.clone()
    }

    pub fn set_live_webhook(&mut self, service: Arc<crate::live_webhook::WebhookService>) {
        service.configure(
            self.settings.live_transcription_webhook.clone(),
            self.save_root_path(),
        );
        self.live_webhook = Some(service);
    }

    pub fn update_settings(&mut self, patch: SettingsPatch) -> AppSettings {
        self.try_update_settings(patch).unwrap_or_else(|error| {
            eprintln!("[settings] could not save settings: {error}");
            self.settings.clone()
        })
    }

    pub fn try_update_settings(&mut self, mut patch: SettingsPatch) -> Result<AppSettings, String> {
        if let Some(model_id) = patch
            .auto_transcript_input_model
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let model = self
                .model_registry()
                .into_iter()
                .find(|model| model.id == model_id)
                .ok_or_else(|| format!("Unknown auto-type model: {model_id}"))?;
            if model.provider_runtime != "whisper-rs"
                && !matches!(
                    model_id,
                    "openai-gpt-live-transcribe" | "soniox-realtime-v5"
                )
            {
                return Err("Auto-type requires a Whisper or cloud realtime model".into());
            }
        }
        if let Some(model_id) = patch.selected_model.as_deref() {
            let model_directory = patch
                .model_directory
                .as_deref()
                .unwrap_or(&self.settings.model_directory);
            if !model_is_selectable(model_id, model_directory) {
                patch.selected_model = None;
            }
        }
        let mut candidate = self.settings.clone();
        candidate.apply_patch(patch);
        candidate.live_transcription_webhook.validate()?;
        let models = model_registry_snapshot(expand_user_path(&candidate.model_directory));
        candidate.transcription_options = validate_model_options(
            &models,
            &candidate.selected_model,
            &candidate.transcription_options,
        );
        if let Some(persistence) = &self.persistence {
            persistence
                .save_settings(&candidate)
                .map_err(|error| format!("Could not save settings: {error}"))?;
        }
        self.settings = candidate;
        if let Some(service) = &self.live_webhook {
            service.configure(
                self.settings.live_transcription_webhook.clone(),
                self.save_root_path(),
            );
        }
        self.sync_capture_settings();
        Ok(self.settings.clone())
    }

    fn save_root_path(&self) -> PathBuf {
        expand_user_path(&self.settings.save_root)
    }

    pub fn load_list_visibility(&self) -> Result<ListVisibilityState, PersistenceError> {
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(PersistenceError::PersistenceUnavailable)?;
        persistence.load_list_visibility(&self.save_root_path())
    }

    pub fn set_list_visibility(
        &self,
        request: SetListVisibilityRequest,
    ) -> Result<ListVisibilityState, PersistenceError> {
        let persistence = self
            .persistence
            .as_ref()
            .ok_or(PersistenceError::PersistenceUnavailable)?;
        persistence.set_list_visibility(&self.save_root_path(), &request)
    }

    fn model_directory_path(&self) -> PathBuf {
        expand_user_path(&self.settings.model_directory)
    }

    fn model_store(&self) -> ModelStore {
        ModelStore::new(self.model_directory_path())
    }

    pub fn model_registry(&self) -> Vec<ModelDescriptor> {
        model_registry_snapshot(self.model_directory_path())
    }

    pub fn verify_model(&self, model_id: &str) -> Result<Vec<ModelDescriptor>, String> {
        let store = self.model_store();
        let registry = store
            .load_model_registry()
            .map_err(|error| error.to_string())?;
        if !registry.contains_key(model_id) {
            return Err(format!("unknown model {model_id}"));
        }

        Ok(self.model_registry())
    }

    pub fn download_model(&self, model_id: &str) -> Result<Vec<ModelDescriptor>, String> {
        let prepared = self.prepare_model_download(model_id)?;
        let store = ModelStore::new(prepared.model_directory);
        if prepared.model.provider_runtime == "qwen3-asr" {
            store
                .install_qwen3_asr_model(&prepared.model)
                .map_err(|error| error.to_string())?;
        } else {
            store
                .download_model(&prepared.model)
                .map_err(|error| error.to_string())?;
        }
        Ok(self.model_registry())
    }

    pub fn prepare_model_download(&self, model_id: &str) -> Result<PreparedModelDownload, String> {
        let store = self.model_store();
        let registry = store
            .load_model_registry()
            .map_err(|error| error.to_string())?;
        let model = registry
            .get(model_id)
            .ok_or_else(|| format!("unknown model {model_id}"))?
            .clone();
        if model.download_url.is_none() && model.provider_runtime != "qwen3-asr" {
            return Err(format!("model {model_id} has no download URL"));
        }
        let mut refreshed_models: Vec<ModelDescriptor> = registry.values().cloned().collect();
        store
            .refresh_statuses(&mut refreshed_models)
            .map_err(|error| error.to_string())?;
        let current_status = refreshed_models
            .iter()
            .find(|candidate| candidate.id == model_id)
            .map(|candidate| candidate.status)
            .unwrap_or(model.status);
        match current_status {
            ModelStatus::Downloading | ModelStatus::Verifying | ModelStatus::Extracting => {
                return Err(format!("model {model_id} download is already active"));
            }
            ModelStatus::Installed | ModelStatus::Ready | ModelStatus::Unloaded => {
                return Err(format!("model {model_id} is already installed"));
            }
            ModelStatus::Missing | ModelStatus::Error => {}
        }

        store
            .record_download_progress(
                &model.id,
                0,
                Some(model.size_mb.saturating_mul(1024 * 1024)),
            )
            .map_err(|error| error.to_string())?;

        Ok(PreparedModelDownload {
            model_directory: self.model_directory_path(),
            model,
            registry: self.model_registry(),
        })
    }

    pub fn cancel_model_download(&self, model_id: &str) -> Result<Vec<ModelDescriptor>, String> {
        let store = self.model_store();
        let registry = store
            .load_model_registry()
            .map_err(|error| error.to_string())?;
        if !registry.contains_key(model_id) {
            return Err(format!("unknown model {model_id}"));
        }
        let mut refreshed_models: Vec<ModelDescriptor> = registry.values().cloned().collect();
        store
            .refresh_statuses(&mut refreshed_models)
            .map_err(|error| error.to_string())?;
        let current_status = refreshed_models
            .iter()
            .find(|candidate| candidate.id == model_id)
            .map(|candidate| candidate.status)
            .unwrap_or(ModelStatus::Missing);
        if !matches!(
            current_status,
            ModelStatus::Downloading | ModelStatus::Verifying | ModelStatus::Extracting
        ) {
            return Err(format!("model {model_id} has no active download"));
        }

        store
            .cancel_download(model_id)
            .map_err(|error| error.to_string())?;
        Ok(self.model_registry())
    }

    pub fn delete_model(&self, model_id: &str) -> Result<Vec<ModelDescriptor>, String> {
        if model_id == self.settings.selected_model
            || model_id == self.settings.effective_auto_type_model()
            || model_id == self.settings.effective_dictation_model()
            || model_id == self.settings.effective_meeting_model()
        {
            return Err(format!("cannot delete model {model_id} while it is in use"));
        }

        let store = self.model_store();
        let registry = store
            .load_model_registry()
            .map_err(|error| error.to_string())?;
        if !registry.contains_key(model_id) {
            return Err(format!("unknown model {model_id}"));
        }
        let mut refreshed_models: Vec<ModelDescriptor> = registry.values().cloned().collect();
        store
            .refresh_statuses(&mut refreshed_models)
            .map_err(|error| error.to_string())?;
        let current_status = refreshed_models
            .iter()
            .find(|candidate| candidate.id == model_id)
            .map(|candidate| candidate.status)
            .unwrap_or(ModelStatus::Missing);
        if matches!(
            current_status,
            ModelStatus::Downloading | ModelStatus::Verifying | ModelStatus::Extracting
        ) {
            return Err(format!("model {model_id} download is active"));
        }

        store
            .delete_model(model_id)
            .map_err(|error| error.to_string())?;
        Ok(self.model_registry())
    }

    pub fn queue_snapshot(&self) -> QueueSnapshot {
        self.queue.snapshot()
    }

    pub fn recent_transcripts(&self, limit: usize) -> Vec<RecentTranscript> {
        recent_transcripts_from_save_root(&self.save_root_path(), limit)
    }

    pub fn openrouter_api_key_configured(&self) -> Result<bool, String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .openrouter_api_key_configured()
            .map_err(|error| error.to_string())
    }

    pub fn load_openrouter_api_key(&self) -> Result<Option<String>, String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .load_openrouter_api_key()
            .map_err(|error| error.to_string())
    }

    pub fn save_openrouter_api_key(&self, api_key: &str) -> Result<(), String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .save_openrouter_api_key(api_key)
            .map_err(|error| error.to_string())
    }

    pub fn delete_openrouter_api_key(&self) -> Result<(), String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .delete_openrouter_api_key()
            .map_err(|error| error.to_string())
    }

    pub fn openai_api_key_configured(&self) -> Result<bool, String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .openai_api_key_configured()
            .map_err(|error| error.to_string())
    }

    pub fn load_openai_api_key(&self) -> Result<Option<String>, String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .load_openai_api_key()
            .map_err(|error| error.to_string())
    }

    pub fn save_openai_api_key(&self, api_key: &str) -> Result<(), String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .save_openai_api_key(api_key)
            .map_err(|error| error.to_string())
    }

    pub fn delete_openai_api_key(&self) -> Result<(), String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .delete_openai_api_key()
            .map_err(|error| error.to_string())
    }

    pub fn soniox_api_key_configured(&self) -> Result<bool, String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .soniox_api_key_configured()
            .map_err(|error| error.to_string())
    }

    pub fn load_soniox_api_key(&self) -> Result<Option<String>, String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .load_soniox_api_key()
            .map_err(|error| error.to_string())
    }

    pub fn save_soniox_api_key(&self, api_key: &str) -> Result<(), String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .save_soniox_api_key(api_key)
            .map_err(|error| error.to_string())
    }

    pub fn delete_soniox_api_key(&self) -> Result<(), String> {
        self.persistence
            .as_ref()
            .ok_or_else(|| "App persistence is not configured".to_string())?
            .delete_soniox_api_key()
            .map_err(|error| error.to_string())
    }

    pub fn transcription_credentials(&self) -> Result<TranscriptionCredentials, String> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Ok(TranscriptionCredentials::default());
        };
        Ok(TranscriptionCredentials::new(
            persistence
                .load_openrouter_api_key()
                .map_err(|error| error.to_string())?,
            persistence
                .load_openai_api_key()
                .map_err(|error| error.to_string())?,
            persistence
                .load_soniox_api_key()
                .map_err(|error| error.to_string())?,
        ))
    }

    pub fn upload_audio_file(
        &mut self,
        source_path: impl Into<std::path::PathBuf>,
        timestamp: DateTime<Local>,
    ) -> Result<UploadedAudio, String> {
        let source_path = source_path.into();
        let target = copy_uploaded_audio_file(&self.save_root_path(), &source_path, timestamp)
            .map_err(|error| error.to_string())?;
        Ok(UploadedAudio {
            audio_path: target.to_string_lossy().to_string(),
            original_filename: source_path
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .unwrap_or_default()
                .to_string(),
            stored_at: timestamp.to_rfc3339(),
        })
    }

    pub fn start_capture_session(
        &mut self,
        sample_rate: u32,
        base_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<AppStatus, String> {
        let device_id = self.settings.selected_microphone.clone();
        let device_name = self.settings.selected_microphone_label.clone();
        self.start_capture_session_with_device(
            sample_rate,
            base_time,
            device_id,
            device_name,
            false,
        )
    }

    pub fn start_capture_session_with_device(
        &mut self,
        sample_rate: u32,
        base_time: chrono::DateTime<chrono::Utc>,
        device_id: impl Into<String>,
        device_name: impl Into<String>,
        used_fallback_device: bool,
    ) -> Result<AppStatus, String> {
        self.start_capture_session_for_slot(
            MicrophoneSlot::Primary,
            sample_rate,
            base_time,
            device_id,
            device_name,
            used_fallback_device,
        )
    }

    pub fn start_capture_session_for_slot(
        &mut self,
        slot: MicrophoneSlot,
        sample_rate: u32,
        base_time: chrono::DateTime<chrono::Utc>,
        device_id: impl Into<String>,
        device_name: impl Into<String>,
        used_fallback_device: bool,
    ) -> Result<AppStatus, String> {
        let device_id = device_id.into();
        let device_name = device_name.into();
        if self.should_combine_microphone_inputs() {
            return self.start_merged_capture_slot(
                slot,
                sample_rate,
                base_time,
                device_id,
                device_name,
                used_fallback_device,
            );
        }
        if slot == MicrophoneSlot::Secondary {
            if let Some(capture) = self.secondary_capture.as_mut() {
                let events = capture.flush().map_err(|error| error.to_string())?;
                self.handle_capture_events_for_slot(MicrophoneSlot::Secondary, events);
            }
            self.secondary_level_monitor = LevelMonitor::default();
            self.active_secondary_microphone_id = Some(device_id.clone());
            self.active_secondary_microphone_label = Some(device_name.clone());
            self.active_secondary_microphone_sample_rate = Some(sample_rate);
            self.secondary_microphone_warning = used_fallback_device
                .then(|| format!("Secondary microphone {device_name} is unavailable"));
            let source_key = self.microphone_source_key_for_slot(slot);
            self.clear_suppressed_live_chunks_for_source(&source_key);
            self.secondary_capture = Some(CaptureController::new(CaptureControllerConfig {
                save_root: self.save_root_path(),
                settings: self.settings.clone(),
                sample_rate,
                device_id,
                device_name: device_name.clone(),
                used_fallback_device,
                base_time,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                source: ChunkSource::Microphone,
                source_label: Some(microphone_output_label(slot, &device_name)),
            }));
            return Ok(self.app_status());
        }

        if let Some(capture) = self.capture.as_mut() {
            let events = capture.flush().map_err(|error| error.to_string())?;
            self.handle_capture_events(events);
        }
        self.level_monitor = LevelMonitor::default();
        self.active_microphone_id = Some(device_id.clone());
        self.active_microphone_label = Some(device_name.clone());
        self.active_microphone_sample_rate = Some(sample_rate);
        let source_key = self.microphone_source_key_for_slot(slot);
        self.clear_suppressed_live_chunks_for_source(&source_key);
        self.microphone_warning = if used_fallback_device {
            Some(format!(
                "Pinned microphone {} is unavailable; using {device_name}",
                self.settings.selected_microphone_label
            ))
        } else {
            None
        };
        // Runtime diagnostic: every capture start logs the resolved device id,
        // its label, and whether a fallback warning is being surfaced. Lets
        // a `log show --predicate 'process == "wakenote"'` reader confirm
        // the false-positive 'Pinned X unavailable; using X' bug stays dead.
        eprintln!(
            "[mic-start] device_id={device_id} label={device_name} fallback={used_fallback_device} warning={:?}",
            self.microphone_warning
        );
        append_debug_log_nonblocking(
            self.save_root_path(),
            format!(
                "[mic-start] device_id={} label={} fallback={} warning={:?}",
                device_id, device_name, used_fallback_device, self.microphone_warning
            ),
        );
        let priority_ids = vec![device_id.clone()];
        let active_index = priority_ids
            .iter()
            .position(|id| id == &device_id)
            .unwrap_or(priority_ids.len());
        self.mic_health
            .capture_started(Instant::now(), priority_ids, active_index);
        self.capture = Some(CaptureController::new(CaptureControllerConfig {
            save_root: self.save_root_path(),
            settings: self.settings.clone(),
            sample_rate,
            device_id,
            device_name: device_name.clone(),
            used_fallback_device,
            base_time,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            source: ChunkSource::Microphone,
            source_label: (self.settings.capture_microphones.len() > 1)
                .then(|| microphone_output_label(slot, &device_name)),
        }));
        Ok(self.app_status())
    }

    fn should_combine_microphone_inputs(&self) -> bool {
        (self.settings.merge_microphone_inputs || self.settings.priority_microphone_inputs)
            && self.settings.capture_microphones.len() == 2
    }

    fn start_merged_capture_slot(
        &mut self,
        slot: MicrophoneSlot,
        sample_rate: u32,
        base_time: chrono::DateTime<chrono::Utc>,
        device_id: String,
        device_name: String,
        used_fallback_device: bool,
    ) -> Result<AppStatus, String> {
        match slot {
            MicrophoneSlot::Primary => {
                self.level_monitor = LevelMonitor::default();
                self.active_microphone_id = Some(device_id.clone());
                self.active_microphone_label = Some(device_name.clone());
                self.active_microphone_sample_rate = Some(sample_rate);
                self.microphone_warning = used_fallback_device.then(|| {
                    format!(
                        "Pinned microphone {} is unavailable; using {device_name}",
                        self.settings.selected_microphone_label
                    )
                });
                let priority_ids = vec![device_id.clone()];
                self.mic_health
                    .capture_started(Instant::now(), priority_ids, 0);
            }
            MicrophoneSlot::Secondary => {
                self.secondary_level_monitor = LevelMonitor::default();
                self.active_secondary_microphone_id = Some(device_id);
                self.active_secondary_microphone_label = Some(device_name);
                self.active_secondary_microphone_sample_rate = Some(sample_rate);
                self.secondary_microphone_warning = used_fallback_device.then(|| {
                    "Secondary microphone is unavailable; waiting for the same device".to_string()
                });
            }
        }

        if self.microphone_mixer.is_none() {
            let merged_device_id = self.merged_microphone_device_id();
            let merged_device_name = self.merged_microphone_device_name();
            self.clear_suppressed_live_chunks_for_source(&format!("microphone:{merged_device_id}"));
            let mode = if self.settings.priority_microphone_inputs {
                MicrophoneMixMode::Priority
            } else {
                MicrophoneMixMode::Merge
            };
            self.microphone_mixer = Some(MicrophoneMixer::with_mode(sample_rate, base_time, mode));
            self.capture = Some(CaptureController::new(CaptureControllerConfig {
                save_root: self.save_root_path(),
                settings: self.settings.clone(),
                sample_rate,
                device_id: merged_device_id,
                device_name: merged_device_name,
                used_fallback_device,
                base_time,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                source: ChunkSource::Microphone,
                source_label: Some(
                    if self.settings.priority_microphone_inputs {
                        "mic-priority"
                    } else {
                        "mic-merged"
                    }
                    .to_string(),
                ),
            }));
            self.secondary_capture = None;
        }

        Ok(self.app_status())
    }

    fn merged_microphone_device_id(&self) -> String {
        self.settings
            .capture_microphones
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>()
            .join("+")
    }

    fn merged_microphone_device_name(&self) -> String {
        self.settings
            .capture_microphones
            .iter()
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>()
            .join(" + ")
    }

    pub fn start_capture_session_for_test(&mut self, sample_rate: u32) -> Result<(), String> {
        self.start_capture_session(sample_rate, chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)?;
        Ok(())
    }

    /// Replace the mic-health monitor with one using a custom config.
    /// Call this before `start_capture_session_for_test` so that
    /// `capture_started` is invoked on the new monitor.
    pub fn override_mic_health_config_for_test(&mut self, config: crate::audio::MicHealthConfig) {
        self.mic_health = MicHealthMonitor::with_config(config);
    }

    /// Inject a single audio frame directly into the health monitor.
    pub fn observe_audio_for_test(&mut self, dbfs: f32, at: Instant) {
        self.mic_health.observe_frame(dbfs, at);
    }

    /// Test-only: apply a `SettingsPatch` without going through the Tauri
    /// command boundary. Mirrors `update_settings` but lets integration tests
    /// drive backend state without a Tauri runtime.
    pub fn apply_settings_patch_for_test(&mut self, patch: crate::settings::SettingsPatch) {
        self.update_settings(patch);
    }

    pub fn stop_capture_session(&mut self) -> Result<AppStatus, String> {
        self.stop_capture_slot(MicrophoneSlot::Primary)?;
        self.stop_capture_slot(MicrophoneSlot::Secondary)?;
        Ok(self.app_status())
    }

    pub fn stop_capture_slot(&mut self, slot: MicrophoneSlot) -> Result<AppStatus, String> {
        if self.microphone_mixer.is_some() {
            return self.stop_merged_capture_slot(slot);
        }
        let events = match slot {
            MicrophoneSlot::Primary => self
                .capture
                .as_mut()
                .map(CaptureController::flush)
                .transpose()
                .map_err(|error| error.to_string())?,
            MicrophoneSlot::Secondary => self
                .secondary_capture
                .as_mut()
                .map(CaptureController::flush)
                .transpose()
                .map_err(|error| error.to_string())?,
        };
        if let Some(events) = events {
            self.handle_capture_events_for_slot(slot, events);
        }
        self.clear_capture_slot_state(slot);
        Ok(self.app_status())
    }

    fn stop_merged_capture_slot(&mut self, slot: MicrophoneSlot) -> Result<AppStatus, String> {
        self.clear_microphone_slot_runtime_state(slot);
        let remaining_slots = self.active_microphone_slots();
        let mixed_frames = if remaining_slots.is_empty() {
            self.microphone_mixer
                .as_mut()
                .map(MicrophoneMixer::flush)
                .unwrap_or_default()
        } else {
            self.microphone_mixer
                .as_mut()
                .map(|mixer| mixer.drain(&remaining_slots))
                .unwrap_or_default()
        };
        self.process_mixed_microphone_frames(mixed_frames)?;

        if remaining_slots.is_empty() {
            let events = self
                .capture
                .as_mut()
                .map(CaptureController::flush)
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or_default();
            self.handle_merged_capture_events(events);
            self.capture = None;
            self.secondary_capture = None;
            self.microphone_mixer = None;
        }
        Ok(self.app_status())
    }

    pub fn capture_slot_active(&self, slot: MicrophoneSlot) -> bool {
        match slot {
            MicrophoneSlot::Primary => self.active_microphone_id.is_some(),
            MicrophoneSlot::Secondary => self.active_secondary_microphone_id.is_some(),
        }
    }

    pub fn capture_slot_matches(
        &self,
        slot: MicrophoneSlot,
        device_id: &str,
        _label: &str,
    ) -> bool {
        match slot {
            MicrophoneSlot::Primary => self.active_microphone_id.as_deref() == Some(device_id),
            MicrophoneSlot::Secondary => {
                self.active_secondary_microphone_id.as_deref() == Some(device_id)
            }
        }
    }

    /// Mark the most recent live-capture start as failed. The capture
    /// controller is torn down, but mic_health stays alive in
    /// `AwaitingRestart` so the watchdog keeps retrying the same configured
    /// physical device with backoff until a start succeeds.
    pub fn capture_start_failed(&mut self, warning: impl Into<String>) -> AppStatus {
        if self.microphone_mixer.is_none() {
            self.capture = None;
        }
        self.clear_microphone_slot_runtime_state(MicrophoneSlot::Primary);

        let warning = warning.into();
        let priority_ids: Vec<String> = self
            .settings
            .capture_microphones
            .first()
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        self.mic_health.refresh_priority(priority_ids);
        self.mic_health
            .mark_start_failed(Instant::now(), warning.clone());

        self.microphone_warning = Some(warning);
        self.app_status()
    }

    pub fn capture_slot_start_failed(
        &mut self,
        slot: MicrophoneSlot,
        warning: impl Into<String>,
    ) -> AppStatus {
        if slot == MicrophoneSlot::Primary {
            return self.capture_start_failed(warning);
        }
        self.clear_capture_slot_state(slot);
        self.secondary_microphone_warning = Some(warning.into());
        self.app_status()
    }

    pub fn set_microphone_slot_warning(
        &mut self,
        slot: MicrophoneSlot,
        warning: impl Into<String>,
    ) {
        match slot {
            MicrophoneSlot::Primary => self.microphone_warning = Some(warning.into()),
            MicrophoneSlot::Secondary => {
                self.secondary_microphone_warning = Some(warning.into());
            }
        }
    }

    pub fn clear_microphone_slot_warning(&mut self, slot: MicrophoneSlot) {
        match slot {
            MicrophoneSlot::Primary => self.microphone_warning = None,
            MicrophoneSlot::Secondary => self.secondary_microphone_warning = None,
        }
    }

    /// Inspect microphone-input health and decide whether the watchdog should
    /// trigger a capture recovery. Returns `None` when no action is required.
    /// Side effect: updates `self.silence_warning` based on the verdict so
    /// `app_status()` exposes the warning to the frontend.
    pub fn evaluate_microphone_health(&mut self) -> Option<MicHealthAction> {
        self.evaluate_microphone_health_at(Instant::now())
    }

    pub fn evaluate_microphone_health_at(&mut self, now: Instant) -> Option<MicHealthAction> {
        if self.settings.pause_all || !self.settings.recording_enabled {
            self.silence_warning = None;
            return None;
        }
        // Allow ticking when no capture is running so the watchdog can keep
        // re-attempting starts after a failure (`AwaitingRestart`). When
        // capture is fully stopped (`Idle`) mic_health returns Healthy and
        // we exit cleanly.
        if self.active_microphone_id.is_none() && !self.mic_health.is_awaiting_restart() {
            self.silence_warning = None;
            return None;
        }
        let verdict = self.mic_health.tick(now);
        match verdict {
            MicHealthVerdict::Action(action) => {
                self.silence_warning = None;
                Some(action)
            }
            MicHealthVerdict::SilenceWarning { since } => {
                let device_label = self
                    .active_microphone_label
                    .clone()
                    .unwrap_or_else(|| self.settings.selected_microphone_label.clone());
                self.silence_warning = Some(SilenceWarning {
                    device_label,
                    seconds: since.as_secs() as u32,
                });
                None
            }
            MicHealthVerdict::Healthy
            | MicHealthVerdict::AwaitingFirstFrame
            | MicHealthVerdict::InCooldown => {
                self.silence_warning = None;
                None
            }
        }
    }

    /// True iff recording is desired (recording_enabled && !pause_all). The
    /// watchdog uses this to decide whether to drive an idle-but-pending
    /// recovery loop forward, even when no live stream is currently running.
    pub fn live_capture_should_run(&self) -> bool {
        self.settings.recording_enabled && !self.settings.pause_all
    }

    /// True iff the watchdog is in the middle of recovering from a failed
    /// start (no live capture, but periodic retries pending).
    pub fn mic_recovery_pending(&self) -> bool {
        self.capture.is_none() && self.mic_health.is_awaiting_restart()
    }

    /// Number of consecutive watchdog escalations since the last non-silent
    /// frame. Used by the runtime watchdog to surface a UI warning when
    /// recovery is fighting (and likely losing).
    pub fn mic_escalations_since_first_frame(&self) -> u32 {
        self.mic_health.escalations_since_first_frame()
    }

    /// Forward an out-of-band stall signal (cpal `StreamError`, OS device-change)
    /// to the health monitor so the next watchdog tick triggers a recovery
    /// action immediately instead of waiting for heartbeat/silence thresholds.
    pub fn notify_stream_error(&mut self, reason: impl Into<String>) {
        if self.capture.is_none() {
            return;
        }
        self.mic_health.request_recovery(reason);
    }

    /// Pre-stage a device override for the next live-capture start. Consumed
    /// (cleared) by `take_microphone_recovery_override` at the next start.
    /// The label is looked up from the user's priority list so the resolver
    /// can fall back to label-matching when the cpal index has moved.
    pub fn set_microphone_recovery_override(&mut self, device_id: impl Into<String>) {
        let device_id = device_id.into();
        let label_hint = self
            .settings
            .microphone_priority
            .iter()
            .find(|entry| entry.id == device_id)
            .map(|entry| entry.label.clone())
            .or_else(|| {
                if self.settings.selected_microphone == device_id {
                    Some(self.settings.selected_microphone_label.clone())
                } else {
                    None
                }
            });
        self.mic_recovery_override = Some(MicRecoveryOverride {
            device_id,
            label_hint,
        });
    }

    /// Consume any pending recovery-device override; the next live-capture
    /// start should use the returned device id instead of `settings.selected_microphone`.
    pub fn take_microphone_recovery_override(&mut self) -> Option<MicRecoveryOverride> {
        self.mic_recovery_override.take()
    }

    /// Update settings to point at the actual stable id we just resolved to,
    /// when the label proves we're on the same physical device the user
    /// pinned. Silently reconciles drift after cpal re-enumeration so the
    /// next start has the correct id from the get-go.
    pub fn reconcile_resolved_device_id(
        &mut self,
        requested_id: &str,
        resolved_id: &str,
        resolved_label: &str,
    ) {
        if reconcile_device_id_in_settings(
            &mut self.settings,
            requested_id,
            resolved_id,
            resolved_label,
        ) {
            self.persist_settings();
        }
    }

    pub fn reconcile_capture_microphone_device_id(
        &mut self,
        slot: MicrophoneSlot,
        requested_id: &str,
        resolved_id: &str,
        resolved_label: &str,
    ) {
        let index = match slot {
            MicrophoneSlot::Primary => 0,
            MicrophoneSlot::Secondary => 1,
        };
        let Some(entry) = self.settings.capture_microphones.get_mut(index) else {
            return;
        };
        if entry.id == requested_id && entry.id != resolved_id && entry.label == resolved_label {
            entry.id = resolved_id.to_string();
            if slot == MicrophoneSlot::Primary {
                self.settings.selected_microphone = resolved_id.to_string();
                if let Some(priority) = self.settings.microphone_priority.first_mut() {
                    priority.id = resolved_id.to_string();
                }
            }
            self.persist_settings();
        }
    }

    /// Surface a warning directly (used by the watchdog when recovery is
    /// exhausted) without tearing down the active capture session.
    pub fn set_microphone_warning(&mut self, warning: impl Into<String>) {
        self.microphone_warning = Some(warning.into());
    }

    /// Clear any standing microphone warning (used after a successful recovery
    /// re-establishes audio input).
    pub fn clear_microphone_warning(&mut self) {
        self.microphone_warning = None;
    }

    pub fn process_audio_frame(&mut self, frame: AudioFrame) -> Result<AppStatus, String> {
        self.process_audio_frame_for_slot(MicrophoneSlot::Primary, frame)
    }

    pub fn process_audio_frame_for_slot(
        &mut self,
        slot: MicrophoneSlot,
        frame: AudioFrame,
    ) -> Result<AppStatus, String> {
        if self.microphone_mixer.is_some() {
            return self.process_merged_audio_frame(slot, frame);
        }
        if slot == MicrophoneSlot::Secondary {
            let capture = self
                .secondary_capture
                .as_mut()
                .ok_or_else(|| "secondary capture session is not running".to_string())?;
            self.secondary_level_monitor.observe_samples(&frame.samples);
            if dbfs_from_samples(&frame.samples) > MIC_NONZERO_DBFS {
                self.secondary_microphone_warning = None;
            }
            let events = capture
                .process_samples_at(&frame.samples, frame.duration_ms, frame.captured_at)
                .map_err(|error| error.to_string())?;
            self.handle_capture_events_for_slot(MicrophoneSlot::Secondary, events);
            return Ok(self.app_status());
        }

        let capture = self
            .capture
            .as_mut()
            .ok_or_else(|| "capture session is not running".to_string())?;
        let dbfs = dbfs_from_samples(&frame.samples);
        self.level_monitor.observe_samples(&frame.samples);
        self.mic_health.observe_frame(dbfs, Instant::now());
        if dbfs > MIC_NONZERO_DBFS
            && self
                .active_microphone_label
                .as_deref()
                .map(|label| label == self.settings.selected_microphone_label)
                .unwrap_or(false)
        {
            self.microphone_warning = None;
        }
        let events = capture
            .process_samples_at(&frame.samples, frame.duration_ms, frame.captured_at)
            .map_err(|error| error.to_string())?;
        self.handle_capture_events(events);
        if let Some(sample_rate) = self.active_microphone_sample_rate {
            self.try_send_system_meeting_microphone_frame(frame, sample_rate);
        }
        Ok(self.app_status())
    }

    fn process_merged_audio_frame(
        &mut self,
        slot: MicrophoneSlot,
        frame: AudioFrame,
    ) -> Result<AppStatus, String> {
        let input_sample_rate = match slot {
            MicrophoneSlot::Primary => self.active_microphone_sample_rate,
            MicrophoneSlot::Secondary => self.active_secondary_microphone_sample_rate,
        }
        .ok_or_else(|| format!("{} capture session is not running", slot.as_str()))?;

        match slot {
            MicrophoneSlot::Primary => {
                let dbfs = dbfs_from_samples(&frame.samples);
                self.level_monitor.observe_samples(&frame.samples);
                self.mic_health.observe_frame(dbfs, Instant::now());
                if dbfs > MIC_NONZERO_DBFS
                    && self
                        .active_microphone_label
                        .as_deref()
                        .map(|label| label == self.settings.selected_microphone_label)
                        .unwrap_or(false)
                {
                    self.microphone_warning = None;
                }
            }
            MicrophoneSlot::Secondary => {
                self.secondary_level_monitor.observe_samples(&frame.samples);
                if dbfs_from_samples(&frame.samples) > MIC_NONZERO_DBFS {
                    self.secondary_microphone_warning = None;
                }
            }
        }

        let active_slots = self.active_microphone_slots();
        let mixed_frames = self
            .microphone_mixer
            .as_mut()
            .expect("merged capture checked above")
            .push_frame(slot, input_sample_rate, frame, &active_slots);
        self.process_mixed_microphone_frames(mixed_frames)?;
        Ok(self.app_status())
    }

    fn active_microphone_slots(&self) -> Vec<MicrophoneSlot> {
        [
            (MicrophoneSlot::Primary, self.active_microphone_id.is_some()),
            (
                MicrophoneSlot::Secondary,
                self.active_secondary_microphone_id.is_some(),
            ),
        ]
        .into_iter()
        .filter_map(|(slot, active)| active.then_some(slot))
        .collect()
    }

    fn process_mixed_microphone_frames(&mut self, frames: Vec<AudioFrame>) -> Result<(), String> {
        if frames.is_empty() {
            return Ok(());
        }
        let mut events = Vec::new();
        let mut meeting_frames = Vec::with_capacity(frames.len());
        let sample_rate;
        {
            let capture = self
                .capture
                .as_mut()
                .ok_or_else(|| "merged capture session is not running".to_string())?;
            sample_rate = capture.sample_rate();
            for frame in frames {
                events.extend(
                    capture
                        .process_samples_at(&frame.samples, frame.duration_ms, frame.captured_at)
                        .map_err(|error| error.to_string())?,
                );
                meeting_frames.push(frame);
            }
        }
        self.handle_merged_capture_events(events);
        for frame in meeting_frames {
            self.try_send_system_meeting_microphone_frame(frame, sample_rate);
        }
        Ok(())
    }

    fn try_send_system_meeting_microphone_frame(&mut self, frame: AudioFrame, sample_rate: u32) {
        if let Some(capture) = self.system_meeting_capture.as_mut() {
            capture.microphone_sender.try_send(frame, sample_rate);
            capture.received_audio = true;
        }
    }

    /// Open a parallel capture session for system-audio frames. Unlike the
    /// microphone path this has no level meter, no health watchdog and no
    /// device resolution: the caller (system-audio input) already owns those.
    /// Chunks it produces are tagged `source = System` and slugged with a
    /// source label so they never collide with mic chunks from the same second.
    pub fn start_system_capture_session(
        &mut self,
        sample_rate: u32,
        base_time: chrono::DateTime<chrono::Utc>,
        app_name: String,
        source_id: String,
        source_label: String,
    ) -> Result<AppStatus, String> {
        if let Some(capture) = self.system_capture.as_mut() {
            let events = capture.flush().map_err(|error| error.to_string())?;
            self.handle_system_capture_events(events);
        }
        if let Some(capture) = self.system_meeting_capture.take() {
            self.finish_system_meeting_capture(capture);
        }
        append_debug_log_nonblocking(
            self.save_root_path(),
            format!(
                "[system-capture] start source_id={} label={} app={} sample_rate={}",
                source_id, source_label, app_name, sample_rate
            ),
        );
        self.last_system_audio_frame_at = None;
        self.next_system_meeting_generation =
            self.next_system_meeting_generation.wrapping_add(1).max(1);
        let meeting_generation = self.next_system_meeting_generation;
        let meeting_capture = system_capture_meeting_title(&source_id)
            .map(|title| -> Result<SystemMeetingCapture, String> {
                let save_root = self.save_root_path();
                let recorder = start_manual_recorded_meeting_capture(
                    &save_root,
                    title,
                    "Microphone + System Audio",
                    self.settings.effective_meeting_model(),
                    self.settings.transcription_language,
                    env!("CARGO_PKG_VERSION"),
                    MANUAL_MEETING_SAMPLE_RATE,
                    self.settings.audio_bitrate_kbps,
                    base_time.with_timezone(&Local),
                )?;
                let meeting_id = recorder.id().to_string();
                let writer = ManualMeetingWriter::start(recorder, base_time);
                let microphone_sender = writer.sender(ManualMeetingSource::Microphone);
                let system_sender = writer.sender(ManualMeetingSource::System);
                Ok(SystemMeetingCapture {
                    generation: meeting_generation,
                    meeting_id,
                    writer,
                    microphone_sender,
                    system_sender,
                    received_audio: false,
                })
            })
            .transpose();
        self.system_meeting_capture = match meeting_capture {
            Ok(capture) => capture,
            Err(error) => {
                append_debug_log_nonblocking(
                    self.save_root_path(),
                    format!(
                        "[system-capture] meeting_start_failed source_id={} error={}",
                        source_id, error
                    ),
                );
                None
            }
        };
        self.system_capture = Some(CaptureController::new(CaptureControllerConfig {
            save_root: self.save_root_path(),
            settings: self.settings.clone(),
            sample_rate,
            device_id: source_id.clone(),
            device_name: app_name,
            used_fallback_device: false,
            base_time,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            source: ChunkSource::System,
            source_label: Some(source_label),
        }));
        Ok(self.app_status())
    }

    /// Feed one system-audio frame into the parallel capture session. Mirrors
    /// `process_audio_frame` but routes completed chunks through the same
    /// transcription queue without touching the mic level/health monitors or
    /// emitting mic-specific live-capture events.
    pub fn process_system_audio_frame(&mut self, frame: AudioFrame) -> Result<AppStatus, String> {
        let generation = self.active_system_meeting_generation();
        self.process_system_audio_frame_for_generation(generation, frame)
    }

    pub fn process_system_audio_frame_for_generation(
        &mut self,
        generation: Option<u64>,
        frame: AudioFrame,
    ) -> Result<AppStatus, String> {
        let capture = self
            .system_capture
            .as_mut()
            .ok_or_else(|| "system capture session is not running".to_string())?;
        let sample_rate = capture.sample_rate();
        let events = capture
            .process_samples_at(&frame.samples, frame.duration_ms, frame.captured_at)
            .map_err(|error| error.to_string())?;
        if let Some(capture) = self.system_meeting_capture.as_mut()
            && generation == Some(capture.generation)
        {
            capture.system_sender.try_send(frame, sample_rate);
            capture.received_audio = true;
        }
        self.last_system_audio_frame_at = Some(Instant::now());
        self.handle_system_capture_events(events);
        Ok(self.app_status())
    }

    pub fn has_recent_system_audio_frame(&self, now: Instant, window: Duration) -> bool {
        self.last_system_audio_frame_at
            .map(|last_frame_at| now.saturating_duration_since(last_frame_at) <= window)
            .unwrap_or(false)
    }

    /// Flush the active system chunk and tear down the system capture session.
    /// Mirrors `stop_capture_session`'s flush-then-clear so an in-progress
    /// system recording is finalized and enqueued before the session closes.
    pub fn stop_system_capture_session(&mut self) -> Result<AppStatus, String> {
        if let Some(capture) = self.system_capture.as_mut() {
            let events = capture.flush().map_err(|error| error.to_string())?;
            self.handle_system_capture_events(events);
        }
        self.system_capture = None;
        self.last_system_audio_frame_at = None;
        if let Some(capture) = self.system_meeting_capture.take() {
            self.finish_system_meeting_capture(capture);
        }
        append_debug_log_nonblocking(self.save_root_path(), "[system-capture] stop");
        Ok(self.app_status())
    }

    fn finish_system_meeting_capture(&mut self, capture: SystemMeetingCapture) {
        let SystemMeetingCapture {
            meeting_id,
            writer,
            received_audio,
            ..
        } = capture;
        if !received_audio {
            let _ = writer.abort();
            append_debug_log_nonblocking(
                self.save_root_path(),
                format!(
                    "[system-capture] meeting_discarded id={} reason=no_audio",
                    meeting_id
                ),
            );
            return;
        }

        if writer.overflowed() {
            let _ = writer.set_capture_warning(
                "Some Meeting audio frames were dropped while saving; Live Transcription continued",
            );
        }
        match writer.finish() {
            Ok(record) => {
                debug_assert_eq!(meeting_id, record.id);
                self.finished_system_meetings.push(MeetingFinishedEvent {
                    id: record.id.clone(),
                    status: MeetingStatus::Recorded,
                    error: None,
                });
                append_debug_log_nonblocking(
                    self.save_root_path(),
                    format!(
                        "[system-capture] meeting_recorded id={} duration_ms={}",
                        record.id, record.duration_ms
                    ),
                );
            }
            Err(error) => {
                append_debug_log_nonblocking(
                    self.save_root_path(),
                    format!(
                        "[system-capture] meeting_failed id={} error={}",
                        meeting_id, error
                    ),
                );
                self.finished_system_meetings.push(MeetingFinishedEvent {
                    id: meeting_id,
                    status: MeetingStatus::Failed,
                    error: Some(error),
                });
            }
        }
    }

    /// Enqueue completed system chunks into the shared transcription queue,
    /// honoring the same transcription-enabled + single-insert dedup rules as
    /// the mic path. System chunks also emit the same live transcript events as
    /// microphone chunks so external-app audio appears in the live transcript
    /// stream while it is being captured.
    fn handle_system_capture_events(&mut self, events: Vec<CaptureControllerEvent>) {
        let mut queue_changed = false;
        for event in events {
            match event {
                CaptureControllerEvent::ChunkDiscarded { chunk_id } => {
                    self.emit_live_event(LiveTranscriptEvent::Discarded {
                        source_key: "system".into(),
                        chunk_id,
                    });
                }
                CaptureControllerEvent::ChunkStarted {
                    chunk_id,
                    started_at,
                } => {
                    eprintln!("[wakenote] system-capture: ChunkStarted chunk_id={chunk_id}");
                    self.emit_live_event(LiveTranscriptEvent::Started {
                        source_key: "system".to_string(),
                        source_label: "System".to_string(),
                        microphone_slot: None,
                        chunk_id,
                        started_at,
                        overlay_position: self.settings.effective_floating_overlay_position(),
                    });
                }
                CaptureControllerEvent::LiveSamplesReady {
                    chunk_id,
                    sample_rate,
                    samples,
                } => {
                    eprintln!(
                        "[wakenote] system-capture: LiveSamplesReady chunk_id={chunk_id} samples={} rate={sample_rate}",
                        samples.len()
                    );
                    self.emit_live_event(LiveTranscriptEvent::SamplesReady {
                        source_key: "system".to_string(),
                        source_label: "System".to_string(),
                        microphone_slot: None,
                        chunk_id,
                        model_id: self.settings.selected_model.clone(),
                        language: self.settings.transcription_language,
                        suppress_low_confidence_transcripts: self
                            .settings
                            .suppress_low_confidence_transcripts,
                        sample_rate,
                        samples,
                    });
                }
                CaptureControllerEvent::ChunkCompleted { chunk_id, chunk } => {
                    let model_id = self.transcription_model_for_completed_chunk(&chunk);
                    eprintln!(
                        "[wakenote] system-capture: ChunkCompleted chunk_id={chunk_id} path={} queue_model={:?}",
                        chunk.audio_path.display(),
                        model_id
                    );
                    append_debug_log_nonblocking(
                        self.save_root_path(),
                        format!(
                            "[system-capture] chunk_completed chunk_id={} path={} queue_model={:?}",
                            chunk_id,
                            chunk.audio_path.display(),
                            model_id
                        ),
                    );
                    self.remember_chunk_id(&chunk.audio_path, chunk_id);
                    if let Some(model_id) = model_id.as_ref() {
                        let (job_id, inserted) = self
                            .queue
                            .enqueue_file_if_new(chunk.audio_path.clone(), model_id.clone());
                        if inserted {
                            if let Some(metadata) = &chunk.metadata {
                                self.pending_live_metadata.insert(job_id, metadata.clone());
                            }
                            let _ = self.queue.set_transcription_options(
                                job_id,
                                self.settings.transcription_options.clone(),
                            );
                        }
                        queue_changed |= inserted;
                    }
                    self.emit_live_event(LiveTranscriptEvent::Committed {
                        source_key: "system".to_string(),
                        source_label: "System".to_string(),
                        microphone_slot: None,
                        chunk_id,
                        model_id: model_id.clone(),
                        audio_path: chunk.audio_path.clone(),
                        overlay_position: self.settings.effective_floating_overlay_position(),
                        will_transcribe: self.should_process_transcriptions(),
                    });
                }
            }
        }

        if queue_changed {
            self.persist_queue();
        }
    }

    pub fn process_audio_samples_for_test(
        &mut self,
        samples: &[f32],
        duration_ms: u64,
    ) -> Result<AppStatus, String> {
        let capture = self
            .capture
            .as_mut()
            .ok_or_else(|| "capture session is not running".to_string())?;
        self.level_monitor.observe_samples(samples);
        let events = capture
            .process_samples(samples, duration_ms)
            .map_err(|error| error.to_string())?;
        self.handle_capture_events(events);
        Ok(self.app_status())
    }

    pub fn enqueue_audio_file(
        &mut self,
        audio_path: impl Into<std::path::PathBuf>,
        model_id: Option<String>,
    ) -> QueueSnapshot {
        let audio_path = audio_path.into();
        if !is_importable_audio_path(&audio_path) {
            return self.queue.snapshot();
        }

        let model_id = model_id.unwrap_or_else(|| self.settings.selected_model.clone());
        let (job_id, inserted) = self.queue.enqueue_file_if_new(audio_path, model_id);
        if inserted {
            let _ = self
                .queue
                .set_transcription_options(job_id, self.settings.transcription_options.clone());
            self.persist_queue();
        }
        self.queue.snapshot()
    }

    pub fn enqueue_backlog(
        &mut self,
        save_root: impl AsRef<std::path::Path>,
    ) -> std::io::Result<QueueSnapshot> {
        let save_root = expand_user_path(save_root.as_ref().to_string_lossy());
        let scan = BacklogScan::scan(&save_root)?;
        let enqueued = self
            .queue
            .enqueue_backlog(scan, self.settings.selected_model.clone());
        if !enqueued.is_empty() {
            for job_id in &enqueued {
                let _ = self.queue.set_transcription_options(
                    *job_id,
                    self.settings.transcription_options.clone(),
                );
            }
            let log_root = self.save_root_path();
            for job_id in &enqueued {
                if let Some(job) = self.queue.job(*job_id) {
                    append_debug_log(
                        &log_root,
                        format!(
                            "[queue] state=backfilled job_id={} path={} model={}",
                            job.id,
                            job.audio_path.display(),
                            job.model_id
                        ),
                    );
                }
            }
            self.persist_queue();
        }
        Ok(self.queue.snapshot())
    }

    pub fn retry_job(&mut self, id: u64) -> Result<QueueSnapshot, String> {
        let job = self
            .queue
            .job(id)
            .ok_or_else(|| format!("job {id} not found"))?;
        if !job.is_reprocessable() {
            return Err(format!("job {id} cannot be retried from {:?}", job.status));
        }
        if matches!(
            job.status,
            QueueJobStatus::Completed | QueueJobStatus::Skipped
        ) {
            let chunk = RecordedChunk::from_audio_path(job.audio_path.clone());
            TranscriptionSidecar::reset_for_regenerate(&chunk)
                .map_err(|error| error.to_string())?;
        }
        self.queue.retry(id)?;
        self.persist_queue();
        Ok(self.queue.snapshot())
    }

    pub fn regenerate_transcript(
        &mut self,
        audio_path: impl Into<std::path::PathBuf>,
        model_id: Option<String>,
    ) -> Result<QueueSnapshot, String> {
        let audio_path = audio_path.into();
        if !is_importable_audio_path(&audio_path) {
            return Err("only existing m4a and wav audio files can be regenerated".to_string());
        }
        let model_id = match model_id {
            Some(model_id) => {
                if !model_is_selectable(&model_id, &self.settings.model_directory) {
                    return Err(format!("model {model_id} is not ready"));
                }
                model_id
            }
            None => self.settings.selected_model.clone(),
        };
        let chunk = RecordedChunk::from_audio_path(audio_path.clone());
        TranscriptionSidecar::reset_for_regenerate(&chunk).map_err(|error| error.to_string())?;
        let job_id = self.queue.requeue_file(audio_path, model_id)?;
        self.queue
            .set_transcription_options(job_id, self.settings.transcription_options.clone())?;
        self.persist_queue();
        Ok(self.queue.snapshot())
    }

    pub fn reprocess_jobs(
        &mut self,
        ids: Vec<u64>,
        model_id: String,
    ) -> Result<QueueSnapshot, String> {
        let ids = ids.into_iter().collect::<BTreeSet<_>>();
        if ids.is_empty() {
            return Err("select at least one job to reprocess".to_string());
        }

        let model = self
            .model_registry()
            .into_iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| format!("unknown model {model_id}"))?;
        if !model_supports_reprocessing(&model) {
            return Err(format!(
                "model {model_id} does not support audio reprocessing"
            ));
        }
        if !model_is_selectable(&model_id, &self.settings.model_directory) {
            return Err(format!("model {model_id} is not ready"));
        }

        let mut audio_paths = Vec::with_capacity(ids.len());
        for id in &ids {
            let job = self
                .queue
                .job(*id)
                .ok_or_else(|| format!("job {id} not found"))?;
            if !job.is_reprocessable() {
                return Err(format!(
                    "job {id} cannot be reprocessed from {:?}",
                    job.status
                ));
            }
            if !is_importable_audio_path(&job.audio_path) {
                return Err(format!("job {id} audio is not an existing m4a or wav file"));
            }
            audio_paths.push(job.audio_path.clone());
        }

        for audio_path in &audio_paths {
            let chunk = RecordedChunk::from_audio_path(audio_path.clone());
            TranscriptionSidecar::reset_for_regenerate(&chunk)
                .map_err(|error| error.to_string())?;
        }
        for audio_path in audio_paths {
            let job_id = self.queue.requeue_file(audio_path, model_id.clone())?;
            self.queue
                .set_transcription_options(job_id, self.settings.transcription_options.clone())?;
        }
        self.persist_queue();
        Ok(self.queue.snapshot())
    }

    pub fn trash_activity_jobs_with<F>(
        &mut self,
        ids: Vec<u64>,
        mut move_to_trash: F,
    ) -> Result<TrashActivityJobsResult, String>
    where
        F: FnMut(&Path) -> Result<(), String>,
    {
        let ids = ids.into_iter().collect::<BTreeSet<_>>();
        if ids.is_empty() {
            return Err("select at least one Activity item to move to Trash".to_string());
        }

        let mut jobs = Vec::with_capacity(ids.len());
        for id in ids {
            let job = self
                .queue
                .job(id)
                .ok_or_else(|| format!("job {id} not found"))?
                .clone();
            if job.status == QueueJobStatus::Running {
                return Err(format!("job {id} is currently running"));
            }
            jobs.push(job);
        }

        let mut removed_ids = Vec::new();
        let mut trashed_ids = Vec::new();
        let mut missing_ids = Vec::new();
        let mut failures = Vec::new();
        for job in jobs {
            match job.audio_path.try_exists() {
                Ok(false) => {
                    removed_ids.push(job.id);
                    missing_ids.push(job.id);
                }
                Err(error) => failures.push(TrashActivityJobFailure {
                    id: job.id,
                    audio_path: job.audio_path,
                    error: error.to_string(),
                }),
                Ok(true) if !is_importable_audio_path(&job.audio_path) => {
                    failures.push(TrashActivityJobFailure {
                        id: job.id,
                        audio_path: job.audio_path,
                        error: "only m4a and wav audio files can be moved to Trash".to_string(),
                    });
                }
                Ok(true) => match move_to_trash(&job.audio_path) {
                    Ok(()) => {
                        removed_ids.push(job.id);
                        trashed_ids.push(job.id);
                    }
                    Err(error) => failures.push(TrashActivityJobFailure {
                        id: job.id,
                        audio_path: job.audio_path,
                        error,
                    }),
                },
            }
        }

        if !removed_ids.is_empty() {
            self.queue.remove_jobs(&removed_ids);
            self.persist_queue();
        }

        Ok(TrashActivityJobsResult {
            queue: self.queue.snapshot(),
            removed_ids,
            trashed_ids,
            missing_ids,
            failures,
        })
    }

    pub fn trash_transcripts_with<F>(
        &mut self,
        transcript_paths: Vec<PathBuf>,
        mut move_bundle_to_trash: F,
    ) -> Result<TrashTranscriptsResult, String>
    where
        F: FnMut(&Path) -> Result<Option<TrashedRecordingBundle>, String>,
    {
        let transcript_paths = transcript_paths.into_iter().collect::<BTreeSet<_>>();
        if transcript_paths.is_empty() {
            return Err("select at least one Transcript item to move to Trash".to_string());
        }

        let save_root = self.save_root_path();
        let mut bundle_paths_by_transcript = BTreeMap::new();
        for transcript_path in &transcript_paths {
            validate_transcript_trash_path(&save_root, transcript_path)?;
            let bundle_paths = recording_bundle_paths(transcript_path)?;
            if let Some(job) = self.queue.snapshot().jobs.into_iter().find(|job| {
                self.active_transcription_jobs.contains(&job.id)
                    && bundle_paths.contains(&job.audio_path)
            }) {
                return Err(format!(
                    "Transcript {} belongs to running job {}",
                    transcript_path.display(),
                    job.id
                ));
            }
            bundle_paths_by_transcript.insert(transcript_path.clone(), bundle_paths);
        }

        let mut removed_transcript_paths = Vec::new();
        let mut trashed_transcript_paths = Vec::new();
        let mut missing_transcript_paths = Vec::new();
        let mut failures = Vec::new();
        let mut removed_audio_paths = BTreeSet::new();

        for transcript_path in transcript_paths {
            match move_bundle_to_trash(&transcript_path) {
                Ok(Some(bundle)) => {
                    if let Err(index_error) =
                        refresh_transcript_day_index_for_recording_path(&transcript_path)
                    {
                        let restore_error = restore_recording_bundle(&bundle).err();
                        let _ = refresh_transcript_day_index_for_recording_path(&transcript_path);
                        failures.push(TrashTranscriptFailure {
                            transcript_path,
                            error: match restore_error {
                                Some(restore_error) => format!(
                                    "failed to refresh Transcript index: {index_error}; rollback incomplete: {restore_error}"
                                ),
                                None => format!(
                                    "failed to refresh Transcript index: {index_error}; bundle restored"
                                ),
                            },
                        });
                        continue;
                    }
                    removed_audio_paths.extend(
                        bundle
                            .original_paths
                            .iter()
                            .filter(|path| is_recording_audio_path(path))
                            .cloned(),
                    );
                    removed_transcript_paths.push(transcript_path.clone());
                    trashed_transcript_paths.push(transcript_path);
                }
                Ok(None) => {
                    if let Some(bundle_paths) = bundle_paths_by_transcript.get(&transcript_path) {
                        removed_audio_paths.extend(
                            bundle_paths
                                .iter()
                                .filter(|path| is_recording_audio_path(path))
                                .cloned(),
                        );
                    }
                    let _ = refresh_transcript_day_index_for_recording_path(&transcript_path);
                    removed_transcript_paths.push(transcript_path.clone());
                    missing_transcript_paths.push(transcript_path);
                }
                Err(error) => failures.push(TrashTranscriptFailure {
                    transcript_path,
                    error,
                }),
            }
        }

        if !removed_audio_paths.is_empty() {
            let queue_ids = self
                .queue
                .snapshot()
                .jobs
                .into_iter()
                .filter(|job| removed_audio_paths.contains(&job.audio_path))
                .map(|job| job.id)
                .collect::<Vec<_>>();
            if !queue_ids.is_empty() {
                self.queue.remove_jobs(&queue_ids);
                self.persist_queue();
            }
        }

        Ok(TrashTranscriptsResult {
            removed_transcript_paths,
            trashed_transcript_paths,
            missing_transcript_paths,
            failures,
        })
    }

    pub fn skip_job(&mut self, id: u64) -> Result<QueueSnapshot, String> {
        self.queue.skip(id)?;
        self.persist_queue();
        Ok(self.queue.snapshot())
    }

    pub fn mark_all_activity_read(&mut self) -> QueueSnapshot {
        if self.queue.mark_attention_outcomes_read() > 0 {
            self.persist_queue();
        }
        self.queue.snapshot()
    }

    pub fn cancel_current_transcription(&mut self) -> Result<QueueSnapshot, String> {
        match self.queue.cancel_current("cancelled by user") {
            Ok(()) => self.persist_queue(),
            Err(error) if error == "no running job" => {}
            Err(error) => return Err(error),
        }
        Ok(self.queue.snapshot())
    }

    pub fn cancel_transcription_job(&mut self, id: u64) -> Result<QueueSnapshot, String> {
        self.queue.cancel(id, "cancelled by user")?;
        self.persist_queue();
        Ok(self.queue.snapshot())
    }

    pub fn prepare_activity_jobs_for_deletion(
        &mut self,
        ids: &[u64],
    ) -> Result<QueueSnapshot, String> {
        let ids = ids.iter().copied().collect::<BTreeSet<_>>();
        if ids.is_empty() {
            return Err("select at least one Activity item to move to Trash".to_string());
        }
        for id in &ids {
            if self.queue.job(*id).is_none() {
                return Err(format!("job {id} not found"));
            }
        }
        let mut cancelled = false;
        for id in ids {
            if self
                .queue
                .job(id)
                .is_some_and(|job| job.status == QueueJobStatus::Running)
            {
                self.queue.cancel(id, "cancelled for deletion by user")?;
                cancelled = true;
            }
        }
        if cancelled {
            self.persist_queue();
        }
        Ok(self.queue.snapshot())
    }

    pub fn transcription_job_is_active(&self, id: u64) -> bool {
        self.active_transcription_jobs.contains(&id)
    }

    pub fn transcription_job_was_cancelled(&self, id: u64) -> bool {
        self.queue
            .job(id)
            .is_some_and(|job| job.status == QueueJobStatus::Cancelled)
    }

    pub fn transcription_output_allowed(&self, audio_path: &Path) -> bool {
        self.queue.has_running_audio_path(audio_path)
    }

    pub fn acknowledge_cancelled_transcription(&mut self, id: u64) {
        self.active_transcription_jobs.remove(&id);
    }

    pub fn cancel_current_operation(&mut self) -> Result<(), String> {
        let store = self.model_store();
        if store
            .cancel_active_download()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(());
        }

        match self.queue.cancel_current("cancelled by user") {
            Ok(()) => {
                self.persist_queue();
                Ok(())
            }
            Err(error) if error == "no running job" => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub fn process_next_transcription(&mut self) -> Result<QueueSnapshot, String> {
        let Some(started) = self.start_next_transcription_job() else {
            return Ok(self.queue.snapshot());
        };
        let runtime = match started.credentials.and_then(|credentials| {
            let replay = started.job.replay_recorded_audio.then(|| {
                crate::recorded_realtime::RecordedRealtimeOptions {
                    credentials: credentials.clone(),
                    is_cancelled: std::sync::Arc::new(|| false),
                }
            });
            RuntimeTranscriber::for_archival_with_credentials(&started.model_directory, credentials)
                .map(|runtime| runtime.with_recorded_realtime(replay))
                .map_err(|error| error.to_string())
        }) {
            Ok(runtime) => {
                runtime.with_file_streaming(started.transcription_options.streaming_enabled, None)
            }
            Err(error) => {
                return self.finish_transcription_job(TranscriptionJobOutcome::failed(
                    started.job.id,
                    error,
                ));
            }
        };
        let fallback = FallbackTranscriber::configured(
            runtime.clone(),
            runtime,
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
            .process_started_job_with_final(
                &started.job,
                self.live_final_callback(started.live_metadata),
            )
            .unwrap_or_else(|error| {
                TranscriptionJobOutcome::failed(started.job.id, error.to_string())
            });
        self.finish_transcription_job(outcome)
    }

    pub fn process_next_transcription_with<T: Transcriber>(
        &mut self,
        transcriber: T,
    ) -> Result<QueueSnapshot, String> {
        if self.settings.pause_all || !self.settings.transcription_enabled {
            return Ok(self.queue.snapshot());
        }

        let selectable_model_ids = selectable_model_ids(&self.settings.model_directory);
        let Some(job) = self.queue.start_next_for_model_ids(&selectable_model_ids) else {
            return Ok(self.queue.snapshot());
        };
        self.active_transcription_jobs.insert(job.id);
        let live_metadata = self
            .pending_live_metadata
            .remove(&job.id)
            .filter(|_| !job.replay_recorded_audio);
        let worker = TranscriptionWorker::with_options_and_dictionary(
            transcriber,
            TranscriptionWorkerOptions {
                language: self.settings.transcription_language,
                suppress_low_confidence_transcripts: self
                    .settings
                    .suppress_low_confidence_transcripts,
            },
            DictionaryContext::from_settings(&self.settings),
        )
        .with_language_hints(&self.settings.transcription_language_hints);
        let outcome = worker
            .process_started_job_with_final(&job, self.live_final_callback(live_metadata))
            .unwrap_or_else(|error| TranscriptionJobOutcome::failed(job.id, error.to_string()));
        self.finish_transcription_job(outcome)
    }

    pub fn process_pending_transcriptions_with<T: Clone + Transcriber>(
        &mut self,
        transcriber: T,
    ) -> Result<QueueSnapshot, String> {
        while let Some(started) = self.start_next_transcription_job() {
            let worker = TranscriptionWorker::with_options_and_dictionary(
                transcriber.clone(),
                TranscriptionWorkerOptions {
                    language: started.language,
                    suppress_low_confidence_transcripts: started
                        .suppress_low_confidence_transcripts,
                },
                started.dictionary,
            )
            .with_language_hints(&started.language_hints);
            let outcome = worker
                .process_started_job_with_final(
                    &started.job,
                    self.live_final_callback(started.live_metadata),
                )
                .unwrap_or_else(|error| {
                    TranscriptionJobOutcome::failed(started.job.id, error.to_string())
                });
            self.finish_transcription_job(outcome)?;
        }

        Ok(self.queue.snapshot())
    }

    fn live_final_callback(
        &self,
        metadata: Option<Arc<ChunkMetadata>>,
    ) -> impl FnOnce(&crate::transcription::TranscriptionExecution) + use<> {
        let service = self.live_webhook.clone();
        let transcript = metadata.map(crate::live_webhook::LiveWebhookTranscript::new);
        move |execution| {
            if let (Some(service), Some(transcript)) = (service, transcript) {
                transcript.finalize(&service, execution);
            }
        }
    }

    pub fn should_process_transcriptions(&self) -> bool {
        let selectable_model_ids = selectable_model_ids(&self.settings.model_directory);
        !self.settings.pause_all
            && self.settings.transcription_enabled
            && self.queue.has_pending_for_model_ids(&selectable_model_ids)
    }

    pub fn start_next_transcription_job(&mut self) -> Option<StartedTranscriptionJob> {
        self.start_transcription_jobs_up_to(1).into_iter().next()
    }

    pub fn start_transcription_jobs_up_to(
        &mut self,
        max_running: usize,
    ) -> Vec<StartedTranscriptionJob> {
        self.start_transcription_jobs_matching(max_running, false)
    }

    /// Jobs an update must finish before exit because they cannot resume after
    /// a relaunch: live chunks captured in this session whose realtime result
    /// exists only in memory, or whose live-webhook metadata is not persisted.
    fn unresumable_job_filter(
        &self,
        models: &[ModelDescriptor],
    ) -> impl Fn(&QueueJob) -> bool + use<> {
        let realtime_model_ids: HashSet<String> = models
            .iter()
            .filter(|model| {
                matches!(
                    model.provider_runtime.as_str(),
                    "openai-realtime" | "soniox-realtime"
                )
            })
            .map(|model| model.id.clone())
            .collect();
        let live_job_ids: HashSet<u64> = self.pending_live_metadata.keys().copied().collect();
        let webhook_enabled = self.settings.live_transcription_webhook.enabled;
        move |job| {
            !job.replay_recorded_audio
                && live_job_ids.contains(&job.id)
                && (webhook_enabled || realtime_model_ids.contains(&job.model_id))
        }
    }

    /// Whether an update still has queued work it must finish before exit.
    pub fn has_pending_unresumable_transcription_jobs(&self) -> bool {
        if self.settings.pause_all || !self.settings.transcription_enabled {
            return false;
        }
        let selectable_model_ids = selectable_model_ids(&self.settings.model_directory);
        let cannot_resume = self.unresumable_job_filter(&self.model_registry());
        self.queue.any_job(|job| {
            job.status == QueueJobStatus::Pending
                && selectable_model_ids.contains(&job.model_id)
                && cannot_resume(job)
        })
    }

    /// Recordings added from files can take far longer to transcribe than an
    /// update may wait, unlike live and dictation chunks. Picked files have no
    /// WakeNote sidecar; live chunks do (older ones may lack its `source`), and
    /// chunks captured in this session count as live even without one.
    pub fn is_transcribing_imported_audio(&self) -> bool {
        self.queue.any_job(|job| {
            job.status == QueueJobStatus::Running
                && !self.chunk_id_index.contains_key(&job.audio_path)
                && match job.activity_kind {
                    QueueActivityKind::ImportedAudio => true,
                    QueueActivityKind::Other => !job.audio_path.with_extension("json").exists(),
                    QueueActivityKind::LiveTranscription | QueueActivityKind::Dictation => false,
                }
        })
    }

    /// While an update drains work, start only jobs that cannot resume after a
    /// relaunch. Everything else stays pending on disk and resumes after the
    /// restart.
    pub fn start_unresumable_transcription_jobs_up_to(
        &mut self,
        max_running: usize,
    ) -> Vec<StartedTranscriptionJob> {
        self.start_transcription_jobs_matching(max_running, true)
    }

    fn start_transcription_jobs_matching(
        &mut self,
        max_running: usize,
        unresumable_only: bool,
    ) -> Vec<StartedTranscriptionJob> {
        if self.settings.pause_all || !self.settings.transcription_enabled {
            return Vec::new();
        }

        let selectable_model_ids = selectable_model_ids(&self.settings.model_directory);
        let model_directory = self.model_directory_path();
        let language = self.settings.transcription_language;
        let suppress_low_confidence_transcripts = self.settings.suppress_low_confidence_transcripts;
        let dictionary = DictionaryContext::from_settings(&self.settings);
        let models = self.model_registry();
        let cannot_resume = self.unresumable_job_filter(&models);
        let requested_transcription_options = self.settings.transcription_options.clone();
        let credentials = self.transcription_credentials();
        let mut started_jobs = Vec::new();

        while let Some(job) = self.queue.start_next_matching_up_to(max_running, |job| {
            selectable_model_ids.contains(&job.model_id)
                && (!unresumable_only || cannot_resume(job))
        }) {
            self.active_transcription_jobs.insert(job.id);
            let requested_options = job
                .transcription_options
                .clone()
                .unwrap_or_else(|| requested_transcription_options.clone());
            let transcription_options =
                validate_model_options(&models, &job.model_id, &requested_options);
            let fallback_supports_diarization = transcription_options
                .cost_limit_fallback_model_id
                .as_deref()
                .and_then(|fallback_id| models.iter().find(|model| model.id == fallback_id))
                .is_some_and(|model| model.capabilities.diarization);
            let diarization_required = models
                .iter()
                .find(|model| model.id == job.model_id)
                .is_some_and(|model| model.capabilities.diarization);
            started_jobs.push(StartedTranscriptionJob {
                live_metadata: self
                    .pending_live_metadata
                    .remove(&job.id)
                    .filter(|_| !job.replay_recorded_audio),
                job,
                model_directory: model_directory.clone(),
                language,
                language_hints: self.settings.transcription_language_hints.clone(),
                suppress_low_confidence_transcripts,
                dictionary: dictionary.clone(),
                transcription_options,
                fallback_supports_diarization,
                diarization_required,
                credentials: credentials.clone(),
            });
        }

        if !started_jobs.is_empty() {
            self.persist_queue();
        }
        started_jobs
    }

    pub fn finish_transcription_job(
        &mut self,
        outcome: TranscriptionJobOutcome,
    ) -> Result<QueueSnapshot, String> {
        let id = outcome.id;
        let result = apply_outcome(&mut self.queue, outcome).map_err(|error| error.to_string());
        self.active_transcription_jobs.remove(&id);
        result?;
        self.persist_queue();
        Ok(self.queue.snapshot())
    }

    pub fn list_microphones(&self) -> Vec<MicrophoneDevice> {
        microphone_devices_from_input_devices(
            &self.settings.selected_microphone,
            list_input_devices(),
        )
    }

    pub fn app_status(&self) -> AppStatus {
        let queue = self.queue.snapshot();
        let mode = derive_mode(&self.settings);
        let is_recording = self
            .capture
            .as_ref()
            .map(|capture| capture.is_recording())
            .unwrap_or(false)
            || self
                .secondary_capture
                .as_ref()
                .map(|capture| capture.is_recording())
                .unwrap_or(false)
            || self
                .system_capture
                .as_ref()
                .map(|capture| capture.is_recording())
                .unwrap_or(false);
        let is_monitoring =
            self.active_microphone_id.is_some() || self.active_secondary_microphone_id.is_some();
        let has_active_microphone = is_monitoring;
        let has_error = (!has_active_microphone
            && (self.microphone_warning.is_some() || self.secondary_microphone_warning.is_some()))
            || self.queue.unread_error_count() > 0;
        let tray_state = derive_tray_state(
            mode,
            queue.running_count > 0,
            is_monitoring,
            is_recording,
            has_error,
        );
        let runtime_warning = self.queue_blocked_warning(&queue);

        AppStatus {
            mode,
            tray_state,
            live_input_active: is_monitoring,
            active_model: self.settings.selected_model.clone(),
            active_microphone: self
                .active_microphone_label
                .clone()
                .unwrap_or_else(|| self.settings.selected_microphone_label.clone()),
            microphone_warning: self.microphone_warning.clone(),
            silence_warning: self.silence_warning.clone(),
            runtime_warning,
            threshold_dbfs: self.settings.threshold_dbfs,
            level: self.level_monitor.snapshot(),
            microphone_captures: self.microphone_capture_statuses(),
            queue,
        }
    }

    fn microphone_capture_statuses(&self) -> Vec<MicrophoneCaptureStatus> {
        self.settings
            .capture_microphones
            .iter()
            .enumerate()
            .filter_map(|(index, configured)| {
                let slot = MicrophoneSlot::from_index(index)?;
                let status = match slot {
                    MicrophoneSlot::Primary => MicrophoneCaptureStatus {
                        slot,
                        device_id: self
                            .active_microphone_id
                            .clone()
                            .unwrap_or_else(|| configured.id.clone()),
                        label: self
                            .active_microphone_label
                            .clone()
                            .unwrap_or_else(|| configured.label.clone()),
                        active: self.active_microphone_id.is_some(),
                        reconnecting: self.active_microphone_id.is_none()
                            && self.mic_health.is_awaiting_restart(),
                        warning: self.microphone_warning.clone(),
                        level: self.level_monitor.snapshot(),
                    },
                    MicrophoneSlot::Secondary => MicrophoneCaptureStatus {
                        slot,
                        device_id: self
                            .active_secondary_microphone_id
                            .clone()
                            .unwrap_or_else(|| configured.id.clone()),
                        label: self
                            .active_secondary_microphone_label
                            .clone()
                            .unwrap_or_else(|| configured.label.clone()),
                        active: self.active_secondary_microphone_id.is_some(),
                        reconnecting: self.active_secondary_microphone_id.is_none()
                            && self.secondary_microphone_warning.is_some(),
                        warning: self.secondary_microphone_warning.clone(),
                        level: self.secondary_level_monitor.snapshot(),
                    },
                };
                Some(status)
            })
            .collect()
    }

    /// Surface a runtime warning when the queue has pending jobs but the
    /// selected model is not installed, so the user understands why nothing
    /// is being transcribed.
    fn queue_blocked_warning(&self, queue: &QueueSnapshot) -> Option<String> {
        if queue.pending_count == 0 {
            return None;
        }
        if !self.settings.transcription_enabled || self.settings.pause_all {
            return None;
        }
        let installed = selectable_model_ids(&self.settings.model_directory);
        let stuck_models: HashSet<String> = queue
            .jobs
            .iter()
            .filter(|job| job.status == crate::queue::QueueJobStatus::Pending)
            .filter(|job| !installed.contains(&job.model_id))
            .map(|job| job.model_id.clone())
            .collect();
        if stuck_models.is_empty() {
            None
        } else {
            let mut models: Vec<String> = stuck_models.into_iter().collect();
            models.sort();
            Some(format!(
                "Transcription queue is paused: model {} is not installed. Open Models tab to download it.",
                models.join(", ")
            ))
        }
    }

    fn persist_settings(&self) {
        if let Some(persistence) = &self.persistence {
            let _ = persistence.save_settings(&self.settings);
        }
    }

    fn persist_queue(&mut self) {
        self.pending_live_metadata.retain(|id, _| {
            self.queue.job(*id).is_some_and(|job| {
                job.status == QueueJobStatus::Pending && !job.replay_recorded_audio
            })
        });
        self.queue
            .prune_completed_history(COMPLETED_JOB_HISTORY_LIMIT);
        if let Some(persistence) = &self.persistence {
            let _ = persistence.save_queue(&self.queue);
        }
    }

    fn sync_capture_settings(&mut self) {
        if self.microphone_mixer.is_some() {
            let frames = self
                .microphone_mixer
                .as_mut()
                .map(MicrophoneMixer::flush)
                .unwrap_or_default();
            let _ = self.process_mixed_microphone_frames(frames);
        }
        let merged_capture = self.microphone_mixer.is_some();
        if let Some(capture) = self.capture.as_mut()
            && let Ok(events) = capture.update_settings(self.settings.clone())
        {
            if merged_capture {
                self.handle_merged_capture_events(events);
            } else {
                self.handle_capture_events(events);
            }
        }
        if let Some(capture) = self.secondary_capture.as_mut()
            && let Ok(events) = capture.update_settings(self.settings.clone())
        {
            self.handle_capture_events_for_slot(MicrophoneSlot::Secondary, events);
        }
        if self.settings.pause_all || !self.settings.recording_enabled {
            self.clear_capture_session_state();
        }
    }

    /// Propagate the current settings to an active system-audio capture session
    /// so a settings change mid-capture (e.g. threshold/chunk timing) takes
    /// effect without restarting the stream. Mirrors `sync_capture_settings`'s
    /// controller update for the mic path; tearing the session down on a
    /// disabled/paused state is the caller's job (it also owns the stream
    /// handle), so this is a no-op when no system capture is running.
    pub fn sync_system_capture_settings(&mut self) {
        let Some(capture) = self.system_capture.as_mut() else {
            return;
        };
        if let Ok(events) = capture.update_settings(self.settings.clone()) {
            self.handle_system_capture_events(events);
        }
    }

    /// Whether a system-audio capture session is currently open. Used by the
    /// settings path to decide if it must tear down or sync the session, and by
    /// tests to assert the disable/pause teardown behavior.
    pub fn is_system_capturing(&self) -> bool {
        self.system_capture.is_some()
    }

    fn clear_capture_session_state(&mut self) {
        self.clear_capture_slot_state(MicrophoneSlot::Primary);
        self.clear_capture_slot_state(MicrophoneSlot::Secondary);
        self.microphone_mixer = None;
    }

    fn clear_capture_slot_state(&mut self, slot: MicrophoneSlot) {
        match slot {
            MicrophoneSlot::Primary => self.capture = None,
            MicrophoneSlot::Secondary => self.secondary_capture = None,
        }
        self.clear_microphone_slot_runtime_state(slot);
    }

    fn clear_microphone_slot_runtime_state(&mut self, slot: MicrophoneSlot) {
        match slot {
            MicrophoneSlot::Primary => {
                self.active_microphone_id = None;
                self.active_microphone_label = None;
                self.active_microphone_sample_rate = None;
                self.level_monitor = LevelMonitor::default();
                self.mic_health.capture_stopped();
                self.silence_warning = None;
            }
            MicrophoneSlot::Secondary => {
                self.active_secondary_microphone_id = None;
                self.active_secondary_microphone_label = None;
                self.active_secondary_microphone_sample_rate = None;
                self.secondary_level_monitor = LevelMonitor::default();
            }
        }
    }

    fn handle_capture_events(&mut self, events: Vec<CaptureControllerEvent>) {
        self.handle_capture_events_for_slot(MicrophoneSlot::Primary, events);
    }

    fn handle_capture_events_for_slot(
        &mut self,
        slot: MicrophoneSlot,
        events: Vec<CaptureControllerEvent>,
    ) {
        let configured = self
            .settings
            .capture_microphones
            .get(match slot {
                MicrophoneSlot::Primary => 0,
                MicrophoneSlot::Secondary => 1,
            })
            .cloned();
        let active_device_label = match slot {
            MicrophoneSlot::Primary => self.active_microphone_label.as_ref(),
            MicrophoneSlot::Secondary => self.active_secondary_microphone_label.as_ref(),
        };
        let source_label = active_device_label
            .cloned()
            .or_else(|| configured.as_ref().map(|entry| entry.label.clone()))
            .unwrap_or_else(|| {
                if slot == MicrophoneSlot::Primary {
                    self.settings.selected_microphone_label.clone()
                } else {
                    "Secondary".to_string()
                }
            });
        let source_key = self.microphone_source_key_for_slot(slot);
        self.handle_microphone_capture_events(events, source_key, source_label, Some(slot));
    }

    fn microphone_source_key_for_slot(&self, slot: MicrophoneSlot) -> String {
        let configured = self.settings.capture_microphones.get(match slot {
            MicrophoneSlot::Primary => 0,
            MicrophoneSlot::Secondary => 1,
        });
        let active_device_id = match slot {
            MicrophoneSlot::Primary => self.active_microphone_id.as_deref(),
            MicrophoneSlot::Secondary => self.active_secondary_microphone_id.as_deref(),
        };
        let device_id = active_device_id
            .or_else(|| configured.map(|entry| entry.id.as_str()))
            .unwrap_or(slot.as_str());
        microphone_source_key(device_id, slot, &self.settings.capture_microphones)
    }

    fn live_chunk_is_suppressed_for_dictation(&mut self, source_key: &str, chunk_id: u64) -> bool {
        let key = (source_key.to_string(), chunk_id);
        if self.live_transcription_suspended_for_dictation {
            self.dictation_suppressed_live_chunks.insert(key.clone());
        }
        self.dictation_suppressed_live_chunks.contains(&key)
    }

    fn clear_suppressed_live_chunks_for_source(&mut self, source_key: &str) {
        self.dictation_suppressed_live_chunks
            .retain(|(candidate, _)| candidate != source_key);
    }

    fn finish_live_chunk_suppression(&mut self, source_key: &str, chunk_id: u64) -> bool {
        self.dictation_suppressed_live_chunks
            .remove(&(source_key.to_string(), chunk_id))
            || self.live_transcription_suspended_for_dictation
    }

    fn handle_merged_capture_events(&mut self, events: Vec<CaptureControllerEvent>) {
        let source_key = format!("microphone:{}", self.merged_microphone_device_id());
        let source_label = self.merged_microphone_device_name();
        self.handle_microphone_capture_events(events, source_key, source_label, None);
    }

    fn handle_microphone_capture_events(
        &mut self,
        events: Vec<CaptureControllerEvent>,
        source_key: String,
        source_label: String,
        microphone_slot: Option<MicrophoneSlot>,
    ) {
        let mut queue_changed = false;
        for event in events {
            match event {
                CaptureControllerEvent::ChunkDiscarded { chunk_id } => {
                    self.finish_live_chunk_suppression(&source_key, chunk_id);
                    self.emit_live_event(LiveTranscriptEvent::Discarded {
                        source_key: source_key.clone(),
                        chunk_id,
                    });
                }
                CaptureControllerEvent::ChunkStarted {
                    chunk_id,
                    started_at,
                } => {
                    eprintln!("[wakenote] capture: ChunkStarted chunk_id={chunk_id}");
                    if !self.live_chunk_is_suppressed_for_dictation(&source_key, chunk_id) {
                        self.emit_live_event(LiveTranscriptEvent::Started {
                            source_key: source_key.clone(),
                            source_label: source_label.clone(),
                            microphone_slot,
                            chunk_id,
                            started_at,
                            overlay_position: self.settings.effective_floating_overlay_position(),
                        });
                    }
                }
                CaptureControllerEvent::LiveSamplesReady {
                    chunk_id,
                    sample_rate,
                    samples,
                } => {
                    eprintln!(
                        "[wakenote] capture: LiveSamplesReady chunk_id={chunk_id} samples={} rate={sample_rate}",
                        samples.len()
                    );
                    if !self.live_chunk_is_suppressed_for_dictation(&source_key, chunk_id) {
                        self.emit_live_event(LiveTranscriptEvent::SamplesReady {
                            source_key: source_key.clone(),
                            source_label: source_label.clone(),
                            microphone_slot,
                            chunk_id,
                            model_id: self.settings.effective_auto_type_model().to_string(),
                            language: self.settings.transcription_language,
                            suppress_low_confidence_transcripts: self
                                .settings
                                .suppress_low_confidence_transcripts,
                            sample_rate,
                            samples,
                        });
                    }
                }
                CaptureControllerEvent::ChunkCompleted { chunk_id, chunk } => {
                    let suppressed = self.finish_live_chunk_suppression(&source_key, chunk_id);
                    let model_id = if suppressed {
                        if let Err(error) = TranscriptionSidecar::mark_not_requested(&chunk) {
                            append_debug_log_nonblocking(
                                self.save_root_path(),
                                format!(
                                    "[dictation] failed to suppress overlapping VOR transcription path={} error={error}",
                                    chunk.audio_path.display()
                                ),
                            );
                        }
                        None
                    } else {
                        self.transcription_model_for_completed_chunk(&chunk)
                    };
                    eprintln!(
                        "[wakenote] capture: ChunkCompleted chunk_id={chunk_id} path={} queue_model={:?} dictation_suppressed={suppressed}",
                        chunk.audio_path.display(),
                        model_id
                    );
                    self.remember_chunk_id(&chunk.audio_path, chunk_id);
                    if let Some(model_id) = model_id.as_ref() {
                        let (job_id, inserted) = self
                            .queue
                            .enqueue_file_if_new(chunk.audio_path.clone(), model_id.clone());
                        if inserted {
                            if let Some(metadata) = &chunk.metadata {
                                self.pending_live_metadata.insert(job_id, metadata.clone());
                            }
                            let _ = self.queue.set_transcription_options(
                                job_id,
                                self.settings.transcription_options.clone(),
                            );
                        }
                        queue_changed |= inserted;
                    }
                    self.emit_live_event(LiveTranscriptEvent::Committed {
                        source_key: source_key.clone(),
                        source_label: source_label.clone(),
                        microphone_slot,
                        chunk_id,
                        model_id: model_id.clone(),
                        audio_path: chunk.audio_path.clone(),
                        overlay_position: self.settings.effective_floating_overlay_position(),
                        will_transcribe: model_id.is_some(),
                    });
                }
            }
        }

        if queue_changed {
            self.persist_queue();
        }
    }

    fn emit_live_event(&mut self, event: LiveTranscriptEvent) {
        self.pending_live_events.push(event);
    }

    fn remember_chunk_id(&mut self, audio_path: &std::path::Path, chunk_id: u64) {
        let path = audio_path.to_path_buf();
        if self.chunk_id_index.insert(path.clone(), chunk_id).is_none() {
            self.chunk_id_history.push_back((path, chunk_id));
            while self.chunk_id_history.len() > LIVE_CHUNK_HISTORY_LIMIT {
                if let Some((evicted, _)) = self.chunk_id_history.pop_front() {
                    self.chunk_id_index.remove(&evicted);
                }
            }
        }
    }

    fn transcription_model_for_completed_chunk(&self, chunk: &RecordedChunk) -> Option<String> {
        if let Some(metadata) = &chunk.metadata {
            return (metadata.transcription_status == TranscriptionStatus::Queued)
                .then(|| metadata.model_id.clone());
        }
        let metadata = std::fs::read_to_string(&chunk.metadata_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<ChunkMetadata>(&contents).ok());

        if let Some(metadata) = metadata {
            return (metadata.transcription_status == TranscriptionStatus::Queued)
                .then_some(metadata.model_id);
        }

        self.settings
            .transcription_enabled
            .then(|| self.settings.selected_model.clone())
    }
}

fn validate_transcript_trash_path(save_root: &Path, transcript_path: &Path) -> Result<(), String> {
    if !is_transcript_sidecar(transcript_path) {
        return Err(format!(
            "selected path is not a Transcript text sidecar: {}",
            transcript_path.display()
        ));
    }
    if transcript_path.file_stem().and_then(|stem| stem.to_str()) == Some("all") {
        return Err("the Transcript day index cannot be moved to Trash".to_string());
    }

    let canonical_root = save_root.canonicalize().map_err(|error| {
        format!(
            "failed to resolve save root {}: {error}",
            save_root.display()
        )
    })?;
    let parent = transcript_path
        .parent()
        .ok_or_else(|| "selected Transcript path has no parent directory".to_string())?;
    let canonical_parent = parent.canonicalize().map_err(|error| {
        format!(
            "failed to resolve Transcript directory {}: {error}",
            parent.display()
        )
    })?;
    let relative = canonical_parent
        .strip_prefix(&canonical_root)
        .map_err(|_| {
            format!(
                "selected Transcript is outside the configured save root: {}",
                transcript_path.display()
            )
        })?;
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let valid_day = match components.as_slice() {
        [day] => dashed_day_from_compact(day).is_some(),
        [uploaded, day] if uploaded == "uploaded" => dashed_day_from_compact(day).is_some(),
        _ => false,
    };
    if !valid_day {
        return Err(format!(
            "selected Transcript is not inside a YYYYMMDD recording directory: {}",
            transcript_path.display()
        ));
    }
    Ok(())
}

fn is_recording_audio_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("m4a" | "wav" | "mp3")
    )
}

/// Lists the `YYYY-MM-DD` days that contain at least one non-empty transcript
/// sidecar, with a per-day sidecar count. Scans both top-level `YYYYMMDD`
/// folders and `uploaded/YYYYMMDD`. Uses file size only so the calendar can
/// stay fast and avoid iCloud materialization.
pub fn transcript_days_from_save_root(root: &Path) -> Vec<TranscriptDay> {
    // Counts are size-based (see `sidecar_has_content`): no file content is read
    // here, so this never triggers an iCloud download and works on evicted
    // (dataless) days. `accumulate_day_counts` never inserts zero-count entries,
    // so every entry is a day with at least one sidecar. BTreeMap keeps days sorted.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    accumulate_day_counts(root, &mut counts);
    accumulate_day_counts(&root.join("uploaded"), &mut counts);

    counts
        .into_iter()
        .map(|(day, count)| TranscriptDay { day, count })
        .collect()
}

fn accumulate_day_counts(parent: &Path, counts: &mut BTreeMap<String, usize>) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let Some(day) = dashed_day_from_compact(name.to_str().unwrap_or_default()) else {
            continue;
        };
        let count = count_transcript_sidecars(&entry.path());
        // Skip empty days so the map only ever holds days with sidecars.
        if count > 0 {
            *counts.entry(day).or_insert(0) += count;
        }
    }
}

/// Counts transcript sidecars in a single day directory using file SIZE only
/// (no content read). A sidecar counts when it is a non-`.error.txt` `.txt`
/// larger than one byte. Legacy empty/suppressed transcripts were written as
/// a lone `"\n"` (one byte), so `len > 1` excludes those stubs and keeps the
/// count aligned with what the day view (which trims) renders.
///
/// Size comes from `stat`, which returns the logical size of an iCloud
/// "dataless" (evicted) file WITHOUT downloading it. Reading content here would
/// instead fail under the dataless guard — so evicted days would wrongly count
/// as empty — or force a download of the entire archive.
fn count_transcript_sidecars(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .filter(|path| is_transcript_sidecar(path))
        .filter(|path| sidecar_has_content(path))
        .count()
}

/// Whether a sidecar holds real content, judged from size alone so it works on
/// iCloud dataless files without downloading them. One byte or less is the
/// legacy empty/suppressed `"\n"` stub and counts as contentless.
fn sidecar_has_content(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.len() > 1)
        .unwrap_or(false)
}

/// "20260510" -> Some("2026-05-10"); anything that is not 8 ASCII digits -> None.
fn dashed_day_from_compact(name: &str) -> Option<String> {
    if name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(format!("{}-{}-{}", &name[0..4], &name[4..6], &name[6..8]))
    } else {
        None
    }
}

/// "2026-05-10" -> Some("20260510"); malformed input -> None.
fn compact_day_from_dashed(day: &str) -> Option<String> {
    let bytes = day.as_bytes();
    let well_formed = day.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && day[0..4].bytes().all(|byte| byte.is_ascii_digit())
        && day[5..7].bytes().all(|byte| byte.is_ascii_digit())
        && day[8..10].bytes().all(|byte| byte.is_ascii_digit());
    well_formed.then(|| format!("{}{}{}", &day[0..4], &day[5..7], &day[8..10]))
}

/// Materializes every non-empty transcript sidecar for a single `YYYY-MM-DD`
/// day, reading only that day's `YYYYMMDD` folder (and its `uploaded` twin).
/// Entries are returned in ascending (oldest-first) order.
///
/// Normal navigation (`download == false`) first tries `YYYYMMDD/all.json` when
/// that index exists and its file fingerprints still match the sidecars. Missing
/// indexes fall back to the legacy sidecar scan without writing one, so existing
/// archives are not migrated implicitly.
///
/// When `download` is false (on-navigation load) the dataless guard is held so
/// iCloud-evicted sidecars are skipped instead of downloaded. When `download`
/// is true (an explicit user reload) the guard is dropped so the requested
/// day's evicted sidecars are materialized (fetched), returned, and persisted
/// into `YYYYMMDD/all.json`.
pub fn transcripts_for_day_from_save_root(
    root: &Path,
    day: &str,
    download: bool,
) -> Vec<RecentTranscript> {
    let Some(compact) = compact_day_from_dashed(day) else {
        return Vec::new();
    };

    if !download && let Some(entries) = read_valid_transcript_day_index(root, day, &compact) {
        return entries;
    }

    let _dataless_guard = if download {
        None
    } else {
        Some(DatalessMaterializationGuard::disabled())
    };
    let entries = collect_transcripts_for_compact_day(root, &compact);

    if download {
        let _ = write_transcript_day_index(root, day, &compact, &entries);
    }

    entries
}

pub(crate) fn transcripts_for_day_result_from_save_root(
    root: &Path,
    day: &str,
    download: bool,
) -> Result<Vec<RecentTranscript>, String> {
    let Some(compact) = compact_day_from_dashed(day) else {
        return Ok(Vec::new());
    };

    if !download && let Some(entries) = read_valid_transcript_day_index(root, day, &compact) {
        return Ok(entries);
    }

    let _dataless_guard = if download {
        None
    } else {
        Some(DatalessMaterializationGuard::disabled())
    };
    let entries = collect_transcripts_for_compact_day_result(root, &compact)?;

    if download {
        write_transcript_day_index(root, day, &compact, &entries)?;
    }

    Ok(entries)
}

pub fn rebuild_transcript_day_index_from_save_root(
    root: &Path,
    day: &str,
    download: bool,
) -> Result<Vec<RecentTranscript>, String> {
    let Some(compact) = compact_day_from_dashed(day) else {
        return Ok(Vec::new());
    };

    let _dataless_guard = if download {
        None
    } else {
        Some(DatalessMaterializationGuard::disabled())
    };
    let entries = collect_transcripts_for_compact_day(root, &compact);
    let _ = write_transcript_day_index(root, day, &compact, &entries);
    Ok(entries)
}

pub fn refresh_transcript_day_index_for_recording_path(
    recording_path: &Path,
) -> Result<(), String> {
    let day_dir = recording_path
        .parent()
        .ok_or_else(|| "recording path has no day directory".to_string())?;
    let compact = day_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "recording day directory is not valid UTF-8".to_string())?;
    let day = dashed_day_from_compact(compact)
        .ok_or_else(|| format!("recording is not inside a YYYYMMDD directory: {compact}"))?;
    let parent = day_dir
        .parent()
        .ok_or_else(|| "recording day directory has no save root".to_string())?;
    let root = if parent.file_name().and_then(|name| name.to_str()) == Some("uploaded") {
        parent
            .parent()
            .ok_or_else(|| "uploaded recording directory has no save root".to_string())?
    } else {
        parent
    };
    let _refresh_guard = TRANSCRIPT_DAY_INDEX_REFRESH_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|error| format!("transcript day index refresh lock failed: {error}"))?;
    let _dataless_guard = DatalessMaterializationGuard::disabled();
    let entries = collect_transcripts_for_compact_day(root, compact);
    write_transcript_day_index(root, &day, compact, &entries)
}

fn collect_transcripts_for_compact_day(root: &Path, compact: &str) -> Vec<RecentTranscript> {
    let day_dir = root.join(compact);
    let uploaded_dir = root.join("uploaded").join(compact);
    let mut paths = Vec::new();
    collect_day_sidecar_paths(&day_dir, &mut paths);
    collect_day_sidecar_paths(&uploaded_dir, &mut paths);
    paths.sort_by(|left, right| {
        transcript_path_sort_key(left).cmp(&transcript_path_sort_key(right))
    });

    let candidates = paths
        .iter()
        .filter_map(|path| transcript_display_candidate_from_sidecar(path))
        .collect();
    prefer_dictation_transcripts(candidates)
        .into_iter()
        .map(|candidate| candidate.transcript)
        .collect()
}

fn collect_transcripts_for_compact_day_result(
    root: &Path,
    compact: &str,
) -> Result<Vec<RecentTranscript>, String> {
    let day_dir = root.join(compact);
    let uploaded_dir = root.join("uploaded").join(compact);
    let mut paths = Vec::new();
    collect_day_sidecar_paths(&day_dir, &mut paths);
    collect_day_sidecar_paths(&uploaded_dir, &mut paths);
    paths.sort_by(|left, right| {
        transcript_path_sort_key(left).cmp(&transcript_path_sort_key(right))
    });

    let candidates = paths
        .iter()
        .map(|path| transcript_display_candidate_from_sidecar_result(path))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(prefer_dictation_transcripts(candidates)
        .into_iter()
        .map(|candidate| candidate.transcript)
        .collect())
}

fn transcript_day_index_path(root: &Path, compact: &str) -> PathBuf {
    root.join(compact).join(TRANSCRIPT_DAY_INDEX_FILE_NAME)
}

fn read_valid_transcript_day_index(
    root: &Path,
    day: &str,
    compact: &str,
) -> Option<Vec<RecentTranscript>> {
    let index_path = transcript_day_index_path(root, compact);
    let index = fs::read(&index_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TranscriptDayIndex>(&bytes).ok())?;
    if index.schema_version != TRANSCRIPT_DAY_INDEX_SCHEMA_VERSION || index.day != day {
        return None;
    }
    let source_files = transcript_day_index_source_files(root, compact);
    (index.source_files == source_files).then_some(index.entries)
}

fn write_transcript_day_index(
    root: &Path,
    day: &str,
    compact: &str,
    entries: &[RecentTranscript],
) -> Result<(), String> {
    let index_path = transcript_day_index_path(root, compact);
    let parent = index_path
        .parent()
        .ok_or_else(|| "transcript day index path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let index = TranscriptDayIndex {
        schema_version: TRANSCRIPT_DAY_INDEX_SCHEMA_VERSION,
        day: day.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        source_files: transcript_day_index_source_files(root, compact),
        entries: entries.to_vec(),
    };
    let tmp_path = index_path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&index).map_err(|error| error.to_string())?;
    fs::write(&tmp_path, bytes).map_err(|error| error.to_string())?;
    fs::rename(&tmp_path, &index_path).map_err(|error| error.to_string())
}

fn transcript_day_index_source_files(
    root: &Path,
    compact: &str,
) -> Vec<TranscriptDayIndexSourceFile> {
    let mut paths = BTreeSet::new();
    let mut sidecars = Vec::new();
    collect_day_sidecar_paths(&root.join(compact), &mut sidecars);
    collect_day_sidecar_paths(&root.join("uploaded").join(compact), &mut sidecars);

    for sidecar in sidecars {
        paths.insert(sidecar.clone());
        let metadata_path = sidecar.with_extension("json");
        if metadata_path.exists() {
            paths.insert(metadata_path);
        }
        for extension in ["m4a", "wav"] {
            let audio_path = sidecar.with_extension(extension);
            if audio_path.exists() {
                paths.insert(audio_path);
            }
        }
    }

    paths
        .into_iter()
        .filter_map(|path| transcript_day_index_source_file(&path))
        .collect()
}

fn transcript_day_index_source_file(path: &Path) -> Option<TranscriptDayIndexSourceFile> {
    let metadata = fs::metadata(path).ok()?;
    let modified_at = metadata
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
        .map(|timestamp| timestamp.to_rfc3339())?;
    Some(TranscriptDayIndexSourceFile {
        path: path.to_string_lossy().to_string(),
        len: metadata.len(),
        modified_at,
    })
}

/// Non-recursive: collect non-error `.txt` sidecars directly inside `dir`.
fn collect_day_sidecar_paths(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
            && is_transcript_sidecar(&path)
        {
            paths.push(path);
        }
    }
}

pub fn recent_transcripts_from_save_root(root: &Path, limit: usize) -> Vec<RecentTranscript> {
    recent_transcripts_matching_from_save_root(root, limit, |_| true)
}

pub fn recent_dictations_from_save_root(
    root: &Path,
    limit: usize,
    hidden: &BTreeSet<String>,
) -> Vec<RecentTranscript> {
    recent_transcripts_matching_from_save_root(root, limit, |entry| {
        entry.source == ChunkSource::Microphone
            && transcript_is_dictation(entry)
            && !hidden.contains(&entry.transcript_path)
    })
}

fn recent_transcripts_matching_from_save_root(
    root: &Path,
    limit: usize,
    include: impl Fn(&RecentTranscript) -> bool,
) -> Vec<RecentTranscript> {
    if limit == 0 {
        return Vec::new();
    }

    let mut days = collect_day_keys(root);
    // Newest day first.
    days.sort_by(|left, right| right.cmp(left));

    let _dataless_guard = DatalessMaterializationGuard::disabled();
    let mut transcripts = Vec::new();
    for day in days {
        if transcripts.len() >= limit {
            break;
        }
        // Merge both sources for this day before sorting, mirroring
        // `transcripts_for_day_from_save_root`, so the within-day order (and
        // any `limit` truncation) spans top-level and `uploaded` together.
        let mut paths = Vec::new();
        collect_day_sidecar_paths(&root.join(&day), &mut paths);
        collect_day_sidecar_paths(&root.join("uploaded").join(&day), &mut paths);
        // Newest within the day first.
        paths.sort_by(|left, right| {
            transcript_path_sort_key(right).cmp(&transcript_path_sort_key(left))
        });
        let candidates = paths
            .iter()
            .filter_map(|path| transcript_display_candidate_from_sidecar(path))
            .collect();
        for candidate in prefer_dictation_transcripts(candidates) {
            if transcripts.len() >= limit {
                break;
            }
            if include(&candidate.transcript) {
                transcripts.push(candidate.transcript);
            }
        }
    }
    transcripts
}

/// Unique compact `YYYYMMDD` day codes that have a folder under `root`
/// (top-level) or `root/uploaded`. Order is unspecified (caller sorts).
fn collect_day_keys(root: &Path) -> Vec<String> {
    let mut days = BTreeSet::new();
    push_day_keys(root, &mut days);
    push_day_keys(&root.join("uploaded"), &mut days);
    days.into_iter().collect()
}

/// Inserts each `YYYYMMDD`-named subdirectory of `parent` into `days`.
fn push_day_keys(parent: &Path, days: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit()) {
            days.insert(name.to_string());
        }
    }
}

#[cfg(target_os = "macos")]
struct DatalessMaterializationGuard {
    previous_policy: Option<std::ffi::c_int>,
}

#[cfg(target_os = "macos")]
impl DatalessMaterializationGuard {
    fn disabled() -> Self {
        const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: std::ffi::c_int = 3;
        const IOPOL_SCOPE_THREAD: std::ffi::c_int = 1;
        const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: std::ffi::c_int = 1;

        let previous_policy = unsafe {
            getiopolicy_np(
                IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                IOPOL_SCOPE_THREAD,
            )
        };
        if previous_policy < 0 {
            return Self {
                previous_policy: None,
            };
        }

        let changed = unsafe {
            setiopolicy_np(
                IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                IOPOL_SCOPE_THREAD,
                IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
            )
        } == 0;

        Self {
            previous_policy: changed.then_some(previous_policy),
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for DatalessMaterializationGuard {
    fn drop(&mut self) {
        const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: std::ffi::c_int = 3;
        const IOPOL_SCOPE_THREAD: std::ffi::c_int = 1;

        if let Some(previous_policy) = self.previous_policy {
            let _ = unsafe {
                setiopolicy_np(
                    IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
                    IOPOL_SCOPE_THREAD,
                    previous_policy,
                )
            };
        }
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getiopolicy_np(iotype: std::ffi::c_int, scope: std::ffi::c_int) -> std::ffi::c_int;
    fn setiopolicy_np(
        iotype: std::ffi::c_int,
        scope: std::ffi::c_int,
        policy: std::ffi::c_int,
    ) -> std::ffi::c_int;
}

#[cfg(not(target_os = "macos"))]
struct DatalessMaterializationGuard;

#[cfg(not(target_os = "macos"))]
impl DatalessMaterializationGuard {
    fn disabled() -> Self {
        Self
    }
}

#[derive(Clone)]
struct TranscriptDisplayCandidate {
    transcript: RecentTranscript,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
}

fn transcript_display_candidate_from_sidecar(path: &Path) -> Option<TranscriptDisplayCandidate> {
    transcript_display_candidate_from_sidecar_result(path)
        .ok()
        .flatten()
}

fn transcript_display_candidate_from_sidecar_result(
    path: &Path,
) -> Result<Option<TranscriptDisplayCandidate>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read transcript {}: {error}", path.display()))?
        .trim()
        .to_string();
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(transcript_display_candidate_from_text(path, text)))
}

fn transcript_display_candidate_from_text(path: &Path, text: String) -> TranscriptDisplayCandidate {
    let metadata = metadata_for_transcript(path);

    let source = metadata
        .as_ref()
        .map(|metadata| metadata.source)
        .unwrap_or_default();
    let source_label = metadata
        .as_ref()
        .and_then(|metadata| metadata.source_label.clone());
    let device_id = metadata
        .as_ref()
        .map(|metadata| metadata.device_id.clone())
        .filter(|value| !value.is_empty());
    let device_name = metadata
        .as_ref()
        .map(|metadata| metadata.device_name.clone())
        .filter(|value| !value.is_empty());
    let microphone_slot = metadata
        .as_ref()
        .and_then(|metadata| metadata.microphone_slot);

    let recorded_at = recorded_at_for_transcript(path, metadata.as_ref());
    let started_at = metadata
        .as_ref()
        .map(|metadata| metadata.started_at)
        .or_else(|| parse_recorded_at(&recorded_at));
    let ended_at = metadata.as_ref().map(|metadata| metadata.ended_at);

    TranscriptDisplayCandidate {
        transcript: RecentTranscript {
            transcript_path: path.to_string_lossy().to_string(),
            audio_path: audio_path_for_transcript(path)
                .map(|audio_path| audio_path.to_string_lossy().to_string()),
            recorded_at,
            text,
            source,
            source_label,
            device_id,
            device_name,
            microphone_slot,
        },
        started_at,
        ended_at,
    }
}

fn prefer_dictation_transcripts(
    candidates: Vec<TranscriptDisplayCandidate>,
) -> Vec<TranscriptDisplayCandidate> {
    let dictations: Vec<TranscriptDisplayCandidate> = candidates
        .iter()
        .filter(|candidate| transcript_is_dictation(&candidate.transcript))
        .cloned()
        .collect();
    if dictations.is_empty() {
        return candidates;
    }

    candidates
        .into_iter()
        .filter(|candidate| {
            transcript_is_dictation(&candidate.transcript)
                || candidate.transcript.source != ChunkSource::Microphone
                || !dictations.iter().any(|dictation| {
                    transcript_intervals_overlap(candidate, dictation)
                        && transcript_texts_match(
                            &candidate.transcript.text,
                            &dictation.transcript.text,
                        )
                })
        })
        .collect()
}

fn transcript_is_dictation(transcript: &RecentTranscript) -> bool {
    transcript
        .source_label
        .as_deref()
        .is_some_and(|label| label.eq_ignore_ascii_case("dictation"))
}

fn transcript_intervals_overlap(
    left: &TranscriptDisplayCandidate,
    right: &TranscriptDisplayCandidate,
) -> bool {
    let (Some(left_start), Some(right_start)) = (left.started_at, right.started_at) else {
        return false;
    };
    match (left.ended_at, right.ended_at) {
        (Some(left_end), Some(right_end)) => {
            let grace = chrono::Duration::milliseconds(DICTATION_DUPLICATE_INTERVAL_GRACE_MS);
            left_start <= right_end + grace && right_start <= left_end + grace
        }
        _ => {
            (left_start - right_start).num_milliseconds().abs()
                <= DICTATION_DUPLICATE_START_FALLBACK_MS
        }
    }
}

fn transcript_texts_match(left: &str, right: &str) -> bool {
    let left = normalized_transcript_text(left);
    let right = normalized_transcript_text(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }

    let (shorter, longer) = if left.chars().count() <= right.chars().count() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let shorter_len = shorter.chars().count();
    let longer_len = longer.chars().count();
    if shorter_len >= 4 && longer.contains(shorter) && shorter_len * 100 >= longer_len * 60 {
        return true;
    }

    let left_bigrams = transcript_bigrams(&left);
    let right_bigrams = transcript_bigrams(&right);
    if left_bigrams.is_empty() || right_bigrams.is_empty() {
        return false;
    }
    let shared = left_bigrams.intersection(&right_bigrams).count();
    shared * 200 >= (left_bigrams.len() + right_bigrams.len()) * 65
}

fn normalized_transcript_text(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn transcript_bigrams(text: &str) -> HashSet<(char, char)> {
    let characters: Vec<char> = text.chars().collect();
    characters
        .windows(2)
        .map(|window| (window[0], window[1]))
        .collect()
}

fn parse_recorded_at(recorded_at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(recorded_at)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn transcript_path_sort_key(path: &Path) -> String {
    recorded_at_from_path(path).unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn audio_path_for_transcript(path: &Path) -> Option<PathBuf> {
    ["m4a", "wav"]
        .into_iter()
        .map(|extension| path.with_extension(extension))
        .find(|candidate| candidate.exists())
}

fn metadata_for_transcript(path: &Path) -> Option<ChunkMetadata> {
    metadata_for_recording_path(path)
}

fn metadata_for_recording_path(path: &Path) -> Option<ChunkMetadata> {
    path.with_extension("json")
        .try_exists()
        .ok()
        .filter(|exists| *exists)
        .and_then(|_| fs::read(path.with_extension("json")).ok())
        .and_then(|bytes| serde_json::from_slice::<ChunkMetadata>(&bytes).ok())
}

pub fn recorded_at_for_audio_path(path: &Path) -> String {
    let metadata = metadata_for_recording_path(path);
    recorded_at_for_path(path, metadata.as_ref())
}

fn recorded_at_for_transcript(path: &Path, metadata: Option<&ChunkMetadata>) -> String {
    metadata
        .map(|metadata| metadata.started_at.to_rfc3339())
        .or_else(|| {
            audio_path_for_transcript(path)
                .map(|audio_path| recorded_at_for_audio_path(&audio_path))
                .filter(|recorded_at| !recorded_at.is_empty())
        })
        .or_else(|| recorded_at_from_path(path))
        .or_else(|| modified_at_rfc3339(path))
        .unwrap_or_default()
}

fn recorded_at_for_path(path: &Path, metadata: Option<&ChunkMetadata>) -> String {
    metadata
        .map(|metadata| metadata.started_at.to_rfc3339())
        .or_else(|| recorded_at_from_path(path))
        .or_else(|| modified_at_rfc3339(path))
        .unwrap_or_default()
}

fn modified_at_rfc3339(path: &Path) -> Option<String> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .map(|timestamp| timestamp.to_rfc3339())
        .ok()
}

fn recorded_at_from_path(path: &Path) -> Option<String> {
    let date_part = path.parent()?.file_name()?.to_str()?;
    if date_part.len() != 8
        || !date_part
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return None;
    }

    let stem = path.file_stem()?.to_str()?;
    let time_part = recording_time_part(stem)?;
    if !time_part
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        return None;
    }

    let date = NaiveDate::parse_from_str(date_part, "%Y%m%d").ok()?;
    let time = NaiveTime::parse_from_str(time_part, "%H%M%S").ok()?;
    let local = date.and_time(time);
    match Local.from_local_datetime(&local) {
        chrono::LocalResult::Single(timestamp) => Some(timestamp.to_rfc3339()),
        chrono::LocalResult::Ambiguous(timestamp, _) => Some(timestamp.to_rfc3339()),
        chrono::LocalResult::None => {
            Some(DateTime::<Utc>::from_naive_utc_and_offset(local, Utc).to_rfc3339())
        }
    }
}

fn recording_time_part(stem: &str) -> Option<&str> {
    if let Some(dated_time) = stem.get(0..13)
        && dated_time.as_bytes().get(6) == Some(&b'-')
        && dated_time[..6]
            .chars()
            .all(|character| character.is_ascii_digit())
        && dated_time[7..]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return dated_time.get(7..13);
    }

    let legacy_time = stem.get(0..6)?;
    legacy_time
        .chars()
        .all(|character| character.is_ascii_digit())
        .then_some(legacy_time)
}

fn is_transcript_sidecar(path: &Path) -> bool {
    if path.extension().and_then(|extension| extension.to_str()) != Some("txt") {
        return false;
    }

    !path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .is_some_and(|file_name| file_name.ends_with(".error.txt"))
}

pub fn microphone_devices_from_input_devices(
    selected_microphone: &str,
    devices: Vec<InputDevice>,
) -> Vec<MicrophoneDevice> {
    devices
        .into_iter()
        .map(|device| MicrophoneDevice {
            transport_type: None,
            fallback: device.id == "default" && selected_microphone != "default",
            id: device.id,
            label: device.label,
            core_audio_uid: None,
            available: device.available,
        })
        .collect()
}

pub fn model_registry_snapshot(
    model_directory: impl AsRef<std::path::Path>,
) -> Vec<ModelDescriptor> {
    let store = ModelStore::new(model_directory);
    let download_state = store.load_download_state().unwrap_or_default();
    let mut models: Vec<ModelDescriptor> = store
        .load_model_registry()
        .unwrap_or_else(|_| default_model_registry())
        .into_values()
        .map(|mut model| {
            model.download_progress = None;
            model.download_error = None;

            if model_has_selectable_runtime(&store, &model) {
                model.status = ModelStatus::Ready;
                model.download_progress = Some(100);
                return model;
            }

            if let Some(record) = download_state.downloads.get(&model.id) {
                model.status = record.status;
                model.download_progress = record.download_progress_percent();
                model.download_error = record.error.clone();
            } else {
                model.status = ModelStatus::Missing;
            }

            model
        })
        .collect();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models
}

pub fn live_preview_model_id(
    settings: &AppSettings,
    models: &[ModelDescriptor],
    model_directory: impl AsRef<Path>,
) -> Option<String> {
    let is_ready = |model: &ModelDescriptor| {
        matches!(
            model.status,
            ModelStatus::Ready | ModelStatus::Installed | ModelStatus::Unloaded
        )
    };
    if let Some(selected) = models
        .iter()
        .find(|model| model.id == settings.selected_model)
        .filter(|model| is_ready(model))
        .filter(|model| {
            matches!(
                model.id.as_str(),
                "openai-gpt-live-transcribe" | "soniox-realtime-v5"
            ) || (model.provider_runtime == "whisper-rs"
                && model_supports_live_partials(&model_directory, &model.id))
        })
    {
        return Some(selected.id.clone());
    }

    let mut candidates = models
        .iter()
        .filter(|model| model.offline)
        .filter(|model| model.provider_runtime == "whisper-rs")
        .filter(|model| is_ready(model))
        .filter(|model| model_supports_live_partials(&model_directory, &model.id))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .speed_score
            .cmp(&left.speed_score)
            .then_with(|| right.accuracy_score.cmp(&left.accuracy_score))
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.first().map(|model| model.id.clone())
}

pub fn derive_mode(settings: &AppSettings) -> AppMode {
    if settings.pause_all || (!settings.recording_enabled && !settings.transcription_enabled) {
        return AppMode::Paused;
    }

    match (settings.recording_enabled, settings.transcription_enabled) {
        (true, true) => AppMode::RecordingAndTranscription,
        (true, false) => AppMode::RecordingOnly,
        (false, true) => AppMode::TranscriptionOnly,
        (false, false) => AppMode::Paused,
    }
}

fn derive_tray_state(
    mode: AppMode,
    transcribing: bool,
    monitoring: bool,
    recording: bool,
    has_error: bool,
) -> TrayState {
    if has_error {
        return TrayState::Error;
    }

    if transcribing {
        return TrayState::Transcribing;
    }

    if recording {
        return TrayState::Recording;
    }

    match mode {
        AppMode::RecordingAndTranscription | AppMode::RecordingOnly if monitoring => {
            TrayState::Listening
        }
        AppMode::RecordingAndTranscription | AppMode::RecordingOnly => TrayState::Idle,
        AppMode::TranscriptionOnly => TrayState::Idle,
        AppMode::Paused => TrayState::Paused,
    }
}

fn model_is_selectable(model_id: &str, model_directory: &str) -> bool {
    selectable_model_ids(model_directory).contains(model_id)
}

fn selectable_model_ids(model_directory: &str) -> HashSet<String> {
    let store = ModelStore::new(expand_user_path(model_directory));
    let models = store
        .load_model_registry()
        .unwrap_or_else(|_| default_model_registry())
        .into_values();
    let download_state = store.load_download_state().unwrap_or_default();

    models
        .filter(|model| {
            model_has_selectable_runtime(&store, model)
                && !download_state
                    .downloads
                    .get(&model.id)
                    .is_some_and(download_record_blocks_model_selection)
        })
        .map(|model| model.id)
        .collect()
}

fn model_has_selectable_runtime(store: &ModelStore, model: &ModelDescriptor) -> bool {
    match model.provider_runtime.as_str() {
        "openrouter-stt" | "openai-stt" | "openai-realtime" | "soniox-async-stt"
        | "soniox-realtime" => true,
        "sherpa-onnx" | "external-command" | "qwen3-asr" => store
            .verify_model(model)
            .is_ok_and(|status| matches!(status, ModelStatus::Ready | ModelStatus::Installed)),
        _ => store.model_path(&model.id).exists(),
    }
}

fn download_record_blocks_model_selection(record: &crate::models::ModelDownloadRecord) -> bool {
    matches!(
        record.status,
        ModelStatus::Downloading
            | ModelStatus::Verifying
            | ModelStatus::Extracting
            | ModelStatus::Error
    )
}

pub fn with_runtime_warning(status: AppStatus, dropped_frames: u64) -> AppStatus {
    with_live_runtime_warning(status, dropped_frames, None)
}

pub fn with_live_runtime_warning(
    mut status: AppStatus,
    dropped_frames: u64,
    stream_error: Option<String>,
) -> AppStatus {
    if let Some(stream_error) = stream_error.filter(|error| !error.trim().is_empty()) {
        status.tray_state = TrayState::Error;
        status.runtime_warning = Some(format!("Live input stream error: {stream_error}"));
        return status;
    }

    if dropped_frames > 0 {
        status.runtime_warning = Some(format!(
            "Live input dropped {dropped_frames} stale audio frames while processing was busy"
        ));
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_drain_finishes_only_live_chunks_that_cannot_resume() {
        let tmp = tempfile::tempdir().unwrap();
        write_test_registry(tmp.path(), "local-ready");
        let mut registry: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join("model-registry.json")).unwrap(),
        )
        .unwrap();
        let mut realtime = registry[0].clone();
        realtime["id"] = "live-realtime".into();
        realtime["provider_runtime"] = "soniox-realtime".into();
        registry.as_array_mut().unwrap().push(realtime);
        std::fs::write(tmp.path().join("model-registry.json"), registry.to_string()).unwrap();
        std::fs::write(tmp.path().join("local-ready.bin"), b"model").unwrap();
        let mut backend = AppBackend::default();
        backend.settings.model_directory = tmp.path().to_string_lossy().into();
        backend.settings.selected_model = "local-ready".into();
        let live_chunk = |name: &str, model_id: &str| {
            let mut metadata: ChunkMetadata =
                serde_json::from_str(include_str!("../tests/fixtures/live-transcript.json"))
                    .unwrap();
            metadata.model_id = model_id.into();
            let mut chunk = RecordedChunk::from_audio_path(tmp.path().join(name));
            chunk.metadata = Some(Arc::new(metadata));
            std::fs::write(&chunk.audio_path, b"audio").unwrap();
            chunk
        };
        let job_for = |backend: &AppBackend, path: &std::path::Path| {
            backend.queue.any_job(|job| job.audio_path == path)
        };

        // A picked file has no live result to lose, even on a realtime model.
        let imported = tmp.path().join("imported.wav");
        std::fs::write(&imported, b"audio").unwrap();
        backend.enqueue_audio_file(&imported, Some("live-realtime".into()));
        let local = live_chunk("local.wav", "local-ready");
        backend.handle_capture_events(vec![CaptureControllerEvent::ChunkCompleted {
            chunk_id: 1,
            chunk: local.clone(),
        }]);
        assert!(job_for(&backend, &imported) && job_for(&backend, &local.audio_path));
        assert!(!backend.has_pending_unresumable_transcription_jobs());
        assert!(
            backend
                .start_unresumable_transcription_jobs_up_to(1)
                .is_empty()
        );

        // This session's realtime chunk waits for an in-memory result.
        let realtime = live_chunk("realtime.wav", "live-realtime");
        backend.handle_capture_events(vec![CaptureControllerEvent::ChunkCompleted {
            chunk_id: 2,
            chunk: realtime.clone(),
        }]);
        assert!(backend.has_pending_unresumable_transcription_jobs());
        let started = backend.start_unresumable_transcription_jobs_up_to(1);
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].job.audio_path, realtime.audio_path);
        backend
            .finish_transcription_job(TranscriptionJobOutcome::completed(started[0].job.id))
            .unwrap();

        // Live-webhook metadata is memory-only, so a webhook user's local
        // chunk must also finish before the update exits.
        assert!(!backend.has_pending_unresumable_transcription_jobs());
        backend.settings.live_transcription_webhook.enabled = true;
        assert!(backend.has_pending_unresumable_transcription_jobs());
        let started = backend.start_unresumable_transcription_jobs_up_to(1);
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].job.audio_path, local.audio_path);
        assert!(started[0].live_metadata.is_some());
        assert!(!backend.is_transcribing_imported_audio());
    }

    #[test]
    fn live_webhook_metadata_is_memory_only_and_consumed_by_the_first_live_job() {
        let tmp = tempfile::tempdir().unwrap();
        write_test_registry(tmp.path(), "local-ready");
        std::fs::write(tmp.path().join("local-ready.bin"), b"model").unwrap();
        let mut backend = AppBackend::default();
        backend.settings.model_directory = tmp.path().to_string_lossy().into();
        backend.settings.selected_model = "local-ready".into();
        let mut metadata: ChunkMetadata =
            serde_json::from_str(include_str!("../tests/fixtures/live-transcript.json")).unwrap();
        metadata.model_id = "local-ready".into();
        let mut chunk = RecordedChunk::from_audio_path(tmp.path().join("live.wav"));
        chunk.metadata = Some(Arc::new(metadata));
        std::fs::write(&chunk.audio_path, b"audio").unwrap();
        // No JSON file exists, and duplicate committed callbacks must not
        // manufacture a second live job or a second metadata handle.
        for _ in 0..2 {
            backend.handle_capture_events(vec![CaptureControllerEvent::ChunkCompleted {
                chunk_id: 1,
                chunk: chunk.clone(),
            }]);
        }
        assert_eq!(backend.pending_live_metadata.len(), 1);
        let started = backend.start_next_transcription_job().unwrap();
        assert!(started.live_metadata.is_some());
        assert!(!chunk.metadata_path.exists());
        assert!(backend.pending_live_metadata.is_empty());
        backend
            .finish_transcription_job(TranscriptionJobOutcome::completed(started.job.id))
            .unwrap();
        backend.retry_job(started.job.id).unwrap();
        let retried = backend.start_next_transcription_job().unwrap();
        assert!(retried.live_metadata.is_none());
        backend
            .finish_transcription_job(TranscriptionJobOutcome::completed(retried.job.id))
            .unwrap();
        // File imports and recovered backlog do not acquire live metadata.
        let imported = tmp.path().join("imported.wav");
        std::fs::write(&imported, b"audio").unwrap();
        backend.queue.enqueue_file(&imported, "local-ready");
        let imported_jobs = backend.start_transcription_jobs_up_to(3);
        assert_eq!(imported_jobs.len(), 1);
        assert!(imported_jobs[0].live_metadata.is_none());
    }

    fn write_test_registry(model_directory: &std::path::Path, model_id: &str) {
        std::fs::create_dir_all(model_directory).expect("model dir");
        std::fs::write(
            model_directory.join("model-registry.json"),
            format!(
                r#"[
                  {{
                    "id": "{model_id}",
                    "display_name": "Local Test Model",
                    "engine": "whisper.cpp",
                    "provider_runtime": "whisper-rs",
                    "download_url": null,
                    "checksum_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "size_mb": 1,
                    "languages": ["en"],
                    "speed_score": 1,
                    "accuracy_score": 1,
                    "offline": true
                  }}
                ]"#
            ),
        )
        .expect("registry");
    }

    #[test]
    fn selectable_model_ids_uses_file_existence_without_hashing_model_contents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_test_registry(tmp.path(), "local-ready");
        std::fs::write(
            tmp.path().join("local-ready.bin"),
            b"checksum is intentionally wrong",
        )
        .expect("model file");

        let models = selectable_model_ids(&tmp.path().to_string_lossy());

        assert!(models.contains("local-ready"));
    }

    #[test]
    fn selectable_model_ids_excludes_models_with_active_download_records() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_test_registry(tmp.path(), "local-downloading");
        std::fs::write(tmp.path().join("local-downloading.bin"), b"existing model")
            .expect("model file");
        ModelStore::new(tmp.path())
            .record_download_progress("local-downloading", 1, Some(2))
            .expect("download state");

        let models = selectable_model_ids(&tmp.path().to_string_lossy());

        assert!(!models.contains("local-downloading"));
    }

    #[test]
    fn selectable_model_ids_includes_external_command_models_with_command_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path()).expect("model dir");
        std::fs::write(
            tmp.path().join("model-registry.json"),
            r#"[
              {
                "id": "sensevoice-small",
                "display_name": "SenseVoice Small",
                "engine": "SenseVoice",
                "provider_runtime": "external-command",
                "download_url": null,
                "checksum_sha256": null,
                "size_mb": 1000,
                "languages": ["ko", "en", "multi"],
                "speed_score": 9,
                "accuracy_score": 8,
                "offline": true
              }
            ]"#,
        )
        .expect("registry");
        std::fs::write(tmp.path().join("sensevoice-small.command"), "printf ok")
            .expect("command file");

        let models = selectable_model_ids(&tmp.path().to_string_lossy());

        assert!(models.contains("sensevoice-small"));
    }

    #[test]
    fn selectable_model_ids_includes_cloud_models_without_local_files() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let models = selectable_model_ids(&tmp.path().to_string_lossy());

        assert!(models.contains("openrouter-qwen3-asr-flash"));
        assert!(models.contains("openai-gpt-transcribe"));
        assert!(!tmp.path().join("openrouter-qwen3-asr-flash.bin").exists());
        assert!(!tmp.path().join("openai-gpt-transcribe.bin").exists());
    }

    #[test]
    fn tray_state_prefers_running_transcription_over_active_recording() {
        assert_eq!(
            derive_tray_state(AppMode::RecordingAndTranscription, true, true, true, false),
            TrayState::Transcribing
        );
    }

    #[test]
    fn tray_presentation_for_dictation_overrides_active_stages() {
        assert_eq!(
            tray_presentation_for_dictation(DictationStage::Recording)
                .expect("recording override")
                .tooltip,
            "WakeNote: Dictation recording"
        );
        assert_eq!(
            tray_presentation_for_dictation(DictationStage::Transcribing)
                .expect("transcribing override")
                .tooltip,
            "WakeNote: Dictation transcribing"
        );
        assert_eq!(
            tray_presentation_for_dictation(DictationStage::Error)
                .expect("error override")
                .tooltip,
            "WakeNote: Dictation failed"
        );
        assert!(tray_presentation_for_dictation(DictationStage::Idle).is_none());
    }

    #[test]
    fn pinned_device_mismatch_returns_false_when_label_matches_after_id_drift() {
        // Stable ids embed the cpal enumeration index. After USB re-enumeration
        // the same physical mic has a new stable id but the label is unchanged.
        // Label match alone is enough to say we're on the pinned device.
        assert!(!pinned_device_mismatch(
            "input-3-by-v",
            "BY-V",
            "input-5-by-v",
            "BY-V",
            false,
        ));
    }

    #[test]
    fn pinned_device_mismatch_returns_true_when_label_differs() {
        assert!(pinned_device_mismatch(
            "input-3-by-v",
            "BY-V",
            "default",
            "System Default",
            true,
        ));
    }

    #[test]
    fn pinned_device_mismatch_returns_true_when_resolver_explicitly_fell_back() {
        // Even with matching labels, trust the resolver's explicit fallback signal.
        assert!(pinned_device_mismatch(
            "input-3-by-v",
            "BY-V",
            "input-3-by-v",
            "BY-V",
            true,
        ));
    }

    #[test]
    fn pinned_device_mismatch_returns_false_when_ids_match() {
        assert!(!pinned_device_mismatch(
            "input-3-by-v",
            "BY-V",
            "input-3-by-v",
            "BY-V",
            false,
        ));
    }

    #[test]
    fn pinned_device_mismatch_returns_true_when_pinned_label_empty_and_ids_differ() {
        // Legacy settings might not have a label. Without a label, fall back
        // to id equality only.
        assert!(pinned_device_mismatch(
            "input-3", "", "input-5", "Some Mic", false,
        ));
    }

    fn settings_with_priority(
        selected_id: &str,
        selected_label: &str,
        priority: Vec<(&str, &str)>,
    ) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.selected_microphone = selected_id.to_string();
        settings.selected_microphone_label = selected_label.to_string();
        settings.microphone_priority = priority
            .into_iter()
            .map(|(id, label)| crate::settings::MicrophonePriorityEntry {
                id: id.to_string(),
                label: label.to_string(),
            })
            .collect();
        settings
    }

    #[test]
    fn reconcile_device_id_in_settings_rewrites_selected_and_priority_entries() {
        let mut settings = settings_with_priority(
            "input-3-by-v",
            "BY-V",
            vec![("input-3-by-v", "BY-V"), ("input-7-airpods", "AirPods")],
        );

        let changed =
            reconcile_device_id_in_settings(&mut settings, "input-3-by-v", "input-5-by-v", "BY-V");

        assert!(changed);
        assert_eq!(settings.selected_microphone, "input-5-by-v");
        assert_eq!(settings.selected_microphone_label, "BY-V");
        assert_eq!(settings.microphone_priority[0].id, "input-5-by-v");
        assert_eq!(settings.microphone_priority[1].id, "input-7-airpods");
    }

    #[test]
    fn reconcile_device_id_in_settings_skips_when_label_does_not_match() {
        // If the resolver returned a different label for the same id, we
        // can't be sure it's the same physical device — leave settings alone.
        let mut settings =
            settings_with_priority("input-3-by-v", "BY-V", vec![("input-3-by-v", "BY-V")]);

        let changed = reconcile_device_id_in_settings(
            &mut settings,
            "input-3-by-v",
            "input-5-other",
            "Some Other Mic",
        );

        assert!(!changed);
        assert_eq!(settings.selected_microphone, "input-3-by-v");
        assert_eq!(settings.microphone_priority[0].id, "input-3-by-v");
    }

    #[test]
    fn reconcile_device_id_in_settings_noop_when_ids_equal() {
        let mut settings =
            settings_with_priority("input-3-by-v", "BY-V", vec![("input-3-by-v", "BY-V")]);

        let changed =
            reconcile_device_id_in_settings(&mut settings, "input-3-by-v", "input-3-by-v", "BY-V");

        assert!(!changed);
    }

    #[test]
    fn reconcile_device_id_in_settings_noop_when_resolved_label_empty() {
        // No label means we can't safely identify the device; skip.
        let mut settings =
            settings_with_priority("input-3-by-v", "BY-V", vec![("input-3-by-v", "BY-V")]);

        let changed =
            reconcile_device_id_in_settings(&mut settings, "input-3-by-v", "input-5-by-v", "");

        assert!(!changed);
        assert_eq!(settings.selected_microphone, "input-3-by-v");
    }

    fn cached_source_file(path: &Path) -> serde_json::Value {
        let metadata = std::fs::metadata(path).expect("source metadata");
        let modified = metadata
            .modified()
            .map(DateTime::<Utc>::from)
            .map(|timestamp| timestamp.to_rfc3339())
            .expect("source modified time");
        serde_json::json!({
            "path": path.to_string_lossy(),
            "len": metadata.len(),
            "modified_at": modified,
        })
    }

    #[test]
    fn transcripts_for_day_reads_valid_all_json_before_sidecars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        let sidecar_path = day.join("090000.txt");
        std::fs::write(&sidecar_path, "sidecar transcript").expect("sidecar");
        let cached_entry = serde_json::json!({
            "transcript_path": sidecar_path.to_string_lossy(),
            "audio_path": null,
            "recorded_at": "2026-05-10T09:00:00+09:00",
            "text": "cached transcript",
            "source": "microphone",
            "source_label": null,
        });
        let index = serde_json::json!({
            "schema_version": TRANSCRIPT_DAY_INDEX_SCHEMA_VERSION,
            "day": "2026-05-10",
            "generated_at": "2026-05-10T00:00:00Z",
            "source_files": [cached_source_file(&sidecar_path)],
            "entries": [cached_entry],
        });
        std::fs::write(
            day.join("all.json"),
            serde_json::to_vec_pretty(&index).expect("index json"),
        )
        .expect("index");

        let transcripts = transcripts_for_day_from_save_root(tmp.path(), "2026-05-10", false);

        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0].text, "cached transcript");
    }

    #[test]
    fn transcripts_for_day_reload_writes_all_json_for_manual_rebuild() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        std::fs::write(day.join("090000.txt"), "morning transcript").expect("sidecar");

        let transcripts = transcripts_for_day_from_save_root(tmp.path(), "2026-05-10", true);

        assert_eq!(transcripts.len(), 1);
        let index_path = day.join("all.json");
        assert!(index_path.exists());
        let index: serde_json::Value =
            serde_json::from_slice(&std::fs::read(index_path).expect("index")).expect("index json");
        assert_eq!(index["day"], "2026-05-10");
        assert_eq!(index["entries"][0]["text"], "morning transcript");
    }

    #[test]
    fn refresh_transcript_day_index_tracks_completed_and_removed_transcripts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        let morning_audio = day.join("090000.m4a");
        let noon_audio = day.join("120000.m4a");
        std::fs::write(&morning_audio, b"morning audio").expect("morning audio");
        std::fs::write(morning_audio.with_extension("txt"), "morning transcript\n")
            .expect("morning transcript");

        refresh_transcript_day_index_for_recording_path(&morning_audio).expect("first refresh");

        let index_path = day.join("all.json");
        let first: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&index_path).expect("first index"))
                .expect("first index json");
        assert_eq!(first["entries"].as_array().expect("entries").len(), 1);
        assert_eq!(first["entries"][0]["text"], "morning transcript");

        std::fs::write(&noon_audio, b"noon audio").expect("noon audio");
        std::fs::write(noon_audio.with_extension("txt"), "noon transcript\n")
            .expect("noon transcript");
        refresh_transcript_day_index_for_recording_path(&noon_audio).expect("second refresh");

        let second: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&index_path).expect("second index"))
                .expect("second index json");
        assert_eq!(second["entries"].as_array().expect("entries").len(), 2);

        std::fs::remove_file(morning_audio.with_extension("txt")).expect("remove morning");
        std::fs::write(
            morning_audio.with_extension("error.txt"),
            "No speech detected\n",
        )
        .expect("morning error");
        refresh_transcript_day_index_for_recording_path(&morning_audio).expect("third refresh");

        let third: serde_json::Value =
            serde_json::from_slice(&std::fs::read(index_path).expect("third index"))
                .expect("third index json");
        assert_eq!(third["entries"].as_array().expect("entries").len(), 1);
        assert_eq!(third["entries"][0]["text"], "noon transcript");
    }

    #[test]
    fn rebuild_transcript_day_index_returns_entries_when_cache_write_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        std::fs::write(day.join("090000.txt"), "morning transcript").expect("sidecar");
        std::fs::create_dir(day.join("all.json.tmp")).expect("block temp index write");

        let transcripts =
            rebuild_transcript_day_index_from_save_root(tmp.path(), "2026-05-10", true)
                .expect("reload should return entries even if cache write fails");

        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0].text, "morning transcript");
    }

    #[test]
    fn transcript_days_counts_sidecars_per_day_including_uploaded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let day1 = root.join("20260510");
        std::fs::create_dir_all(&day1).expect("day1");
        std::fs::write(day1.join("010203.txt"), "morning").expect("t1");
        std::fs::write(day1.join("110203.txt"), "noon").expect("t2");
        // error / empty-by-name sidecars must not be counted as transcripts.
        std::fs::write(day1.join("120000.error.txt"), "boom").expect("err");
        // audio without a transcript must not be counted.
        std::fs::write(day1.join("130000.m4a"), b"audio").expect("audio");

        let day2 = root.join("20260511");
        std::fs::create_dir_all(&day2).expect("day2");
        std::fs::write(day2.join("090000.txt"), "second day").expect("t3");

        // Uploaded files live under uploaded/YYYYMMDD and merge into the same day bucket.
        let uploaded = root.join("uploaded").join("20260511");
        std::fs::create_dir_all(&uploaded).expect("uploaded");
        std::fs::write(uploaded.join("memo.txt"), "imported").expect("t4");

        // A non-date directory must be ignored.
        std::fs::create_dir_all(root.join("notes")).expect("notes");

        let days = transcript_days_from_save_root(root);

        assert_eq!(
            days,
            vec![
                TranscriptDay {
                    day: "2026-05-10".to_string(),
                    count: 2
                },
                TranscriptDay {
                    day: "2026-05-11".to_string(),
                    count: 2
                },
            ]
        );
    }

    #[test]
    fn transcript_days_excludes_empty_sidecars_to_match_day_view() {
        // Legacy suppressed (low-confidence) transcripts were written as a
        // lone "\n" (one byte). The size-based count must continue to exclude
        // them, otherwise a day would look selectable but open to "No
        // transcripts for this day".
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Day with ONLY a one-byte suppressed stub -> excluded entirely (count 0).
        let suppressed_only = root.join("20260512");
        std::fs::create_dir_all(&suppressed_only).expect("suppressed_only");
        std::fs::write(suppressed_only.join("100000.txt"), "\n").expect("suppressed");

        // Day with one suppressed stub + one real sidecar -> count == 1.
        let mixed = root.join("20260513");
        std::fs::create_dir_all(&mixed).expect("mixed");
        std::fs::write(mixed.join("090000.txt"), "\n").expect("suppressed");
        std::fs::write(mixed.join("100000.txt"), "real transcript\n").expect("real");

        let days = transcript_days_from_save_root(root);

        assert_eq!(
            days,
            vec![TranscriptDay {
                day: "2026-05-13".to_string(),
                count: 1
            }]
        );
    }

    #[test]
    fn recorded_at_for_audio_path_prefers_recording_metadata_started_at() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let audio_path = tmp.path().join("20260615").join("235959.wav");
        std::fs::create_dir_all(audio_path.parent().expect("audio parent")).expect("audio dir");
        std::fs::write(&audio_path, b"wav bytes").expect("audio");
        let started_at = Utc.with_ymd_and_hms(2026, 5, 9, 8, 0, 0).unwrap();
        let metadata = ChunkMetadata {
            model_id: "whisper-medium".into(),
            requested_model_id: None,
            effective_model_id: None,
            fallback_from_model_id: None,
            device_id: "default".into(),
            device_name: "System Default".into(),
            sample_rate: 16_000,
            threshold_dbfs: -42.0,
            attack_ms: 100,
            release_ms: 1_000,
            pre_roll_ms: 1_000,
            lead_in_padding_ms: 300,
            post_roll_ms: 300,
            min_chunk_ms: 600,
            max_chunk_ms: 120_000,
            started_at,
            ended_at: started_at + chrono::Duration::milliseconds(500),
            duration_ms: 500,
            transcription_status: TranscriptionStatus::Completed,
            app_version: "0.0.0".into(),
            used_fallback_device: false,
            live_capture_chunk_id: Some(42),
            source: ChunkSource::Microphone,
            source_label: None,
            microphone_slot: None,
            microphone_inputs: Vec::new(),
            transcribed_at: None,
            transcript_text: None,
        };
        std::fs::write(
            audio_path.with_extension("json"),
            serde_json::to_vec_pretty(&metadata).expect("metadata json"),
        )
        .expect("metadata");

        assert_eq!(
            recorded_at_for_audio_path(&audio_path),
            started_at.to_rfc3339()
        );
    }

    #[test]
    fn recorded_at_path_fallback_accepts_dated_and_legacy_recording_names() {
        for filename in ["260615-235959.wav", "235959.wav"] {
            let path = Path::new("/tmp/WakeNote/20260615").join(filename);
            let recorded_at = recorded_at_from_path(&path).expect("recorded at from path");
            let parsed = DateTime::parse_from_rfc3339(&recorded_at).expect("RFC3339");

            assert_eq!(
                parsed.date_naive(),
                NaiveDate::from_ymd_opt(2026, 6, 15).unwrap()
            );
            assert_eq!(parsed.time(), NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        }
    }

    #[test]
    fn transcripts_for_day_reads_only_that_day_including_uploaded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let target = root.join("20260510");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::write(target.join("090000.txt"), "morning").expect("t1");
        std::fs::write(target.join("180000.txt"), "evening").expect("t2");
        std::fs::write(target.join("190000.txt"), "  \n").expect("empty");

        let uploaded = root.join("uploaded").join("20260510");
        std::fs::create_dir_all(&uploaded).expect("uploaded");
        std::fs::write(uploaded.join("memo.txt"), "imported").expect("t3");

        let other = root.join("20260511");
        std::fs::create_dir_all(&other).expect("other");
        std::fs::write(other.join("090000.txt"), "other day").expect("t4");

        let result = transcripts_for_day_from_save_root(root, "2026-05-10", false);
        let texts: Vec<&str> = result.iter().map(|item| item.text.as_str()).collect();

        // Empty sidecar skipped; other day excluded; uploaded merged in.
        assert_eq!(result.len(), 3);
        assert!(texts.contains(&"morning"));
        assert!(texts.contains(&"evening"));
        assert!(texts.contains(&"imported"));
        assert!(!texts.contains(&"other day"));

        // Ascending (oldest-first): morning (09:00) precedes evening (18:00).
        // The uploaded "imported" entry sorts by full path, so only assert the
        // two same-folder entries' relative order.
        assert!(
            texts.iter().position(|text| *text == "morning")
                < texts.iter().position(|text| *text == "evening")
        );
    }

    #[test]
    fn transcripts_for_day_rejects_malformed_day() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(transcripts_for_day_from_save_root(tmp.path(), "20260510", false).is_empty());
        assert!(transcripts_for_day_from_save_root(tmp.path(), "not-a-day", false).is_empty());
    }

    #[test]
    fn transcripts_for_day_download_flag_is_equivalent_for_local_files() {
        // For materialized (non-evicted) files the dataless guard has no effect,
        // so the on-navigation (download=false) and reload (download=true) reads
        // return the same entries. The flag only matters for iCloud-evicted files.
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        std::fs::write(day.join("090000.txt"), "morning\n").expect("t1");
        std::fs::write(day.join("180000.txt"), "evening\n").expect("t2");

        let local = transcripts_for_day_from_save_root(tmp.path(), "2026-05-10", false);
        let downloaded = transcripts_for_day_from_save_root(tmp.path(), "2026-05-10", true);
        assert_eq!(local, downloaded);
        assert_eq!(local.len(), 2);
    }

    #[test]
    fn strict_transcripts_for_day_matches_the_local_day_view() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        std::fs::write(day.join("090000.txt"), "morning\n").expect("morning");
        std::fs::write(day.join("180000.txt"), "evening\n").expect("evening");

        let transcripts = transcripts_for_day_result_from_save_root(tmp.path(), "2026-05-10", true)
            .expect("strict transcript day");

        assert_eq!(
            transcripts
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["morning", "evening"],
        );
    }

    #[test]
    fn strict_transcripts_for_day_reports_invalid_utf8_sidecars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260510");
        std::fs::create_dir_all(&day).expect("day");
        std::fs::write(day.join("090000.txt"), [0xff, 0xfe]).expect("invalid UTF-8");

        let error = transcripts_for_day_result_from_save_root(tmp.path(), "2026-05-10", true)
            .expect_err("strict transcript read must fail");

        assert!(error.contains("090000.txt"), "{error}");
        assert!(error.contains("failed to read transcript"), "{error}");
    }

    #[test]
    fn recent_transcripts_orders_newest_day_first_across_buckets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let older = root.join("20260510");
        std::fs::create_dir_all(&older).expect("older");
        std::fs::write(older.join("090000.txt"), "older day").expect("t1");

        let newer = root.join("20260512");
        std::fs::create_dir_all(&newer).expect("newer");
        std::fs::write(newer.join("080000.txt"), "newer morning").expect("t2");
        std::fs::write(newer.join("200000.txt"), "newer evening").expect("t3");

        let all = recent_transcripts_from_save_root(root, 10);
        let texts: Vec<&str> = all.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, vec!["newer evening", "newer morning", "older day"]);

        // Limit stops once filled, newest first.
        let limited = recent_transcripts_from_save_root(root, 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].text, "newer evening");
    }

    #[test]
    fn recent_transcripts_merges_uploaded_into_same_day_before_sorting() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let top = root.join("20260512");
        std::fs::create_dir_all(&top).expect("top day");
        std::fs::write(top.join("090000.txt"), "top morning").expect("t1");

        let uploaded = root.join("uploaded").join("20260512");
        std::fs::create_dir_all(&uploaded).expect("uploaded day");
        std::fs::write(uploaded.join("200000.txt"), "uploaded evening").expect("t2");

        let all = recent_transcripts_from_save_root(root, 10);
        let texts: Vec<&str> = all.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts.len(), 2);
        assert!(texts.contains(&"top morning"));
        assert!(texts.contains(&"uploaded evening"));
        // Within the day, both sources are merged before sorting, so the
        // 20:00 uploaded entry precedes the 09:00 top-level one.
        assert!(
            texts.iter().position(|t| *t == "uploaded evening")
                < texts.iter().position(|t| *t == "top morning")
        );

        // The merge happens before truncation: limit 1 keeps the newest of
        // the merged day, which lives in `uploaded/`.
        let limited = recent_transcripts_from_save_root(root, 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].text, "uploaded evening");
    }

    #[test]
    fn recent_transcripts_fills_limit_after_skipping_empty_sidecars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260528");
        std::fs::create_dir_all(&day).expect("day dir");
        std::fs::write(day.join("000003.txt"), "   \n").expect("empty transcript");
        std::fs::write(day.join("000002.txt"), "second newest").expect("second transcript");
        std::fs::write(day.join("000001.txt"), "oldest").expect("oldest transcript");

        let transcripts = recent_transcripts_from_save_root(tmp.path(), 2);

        assert_eq!(transcripts.len(), 2);
        assert_eq!(transcripts[0].text, "second newest");
        assert_eq!(transcripts[1].text, "oldest");
    }

    #[test]
    fn recent_dictations_filter_before_limiting_and_skip_hidden_records() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let older = tmp.path().join("20260527");
        let newer = tmp.path().join("20260528");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::create_dir_all(&newer).unwrap();
        let base = Utc.with_ymd_and_hms(2026, 5, 27, 9, 0, 0).unwrap();
        write_transcript_fixture(
            &older,
            "090000-dictation",
            "older dictation",
            base,
            base + chrono::Duration::seconds(5),
            Some("Dictation"),
        );
        let base = base + chrono::Duration::days(1);
        write_transcript_fixture(
            &newer,
            "090000-dictation",
            "newer dictation",
            base,
            base + chrono::Duration::seconds(5),
            Some("dictation"),
        );
        std::fs::write(newer.join("100000.txt"), "newest ordinary capture").unwrap();
        let all = recent_dictations_from_save_root(tmp.path(), 2, &BTreeSet::new());
        assert_eq!(
            all.iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["newer dictation", "older dictation"]
        );
        let hidden = BTreeSet::from([all[0].transcript_path.clone()]);
        let visible = recent_dictations_from_save_root(tmp.path(), 1, &hidden);
        assert_eq!(visible[0].text, "older dictation");
        assert!(recent_dictations_from_save_root(tmp.path(), 0, &hidden).is_empty());

        let metadata_path = newer.join("090000-dictation.json");
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
        metadata["source"] = serde_json::json!("system");
        std::fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let visible = recent_dictations_from_save_root(tmp.path(), 10, &BTreeSet::new());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].text, "older dictation");
    }

    fn write_transcript_fixture(
        day: &Path,
        stem: &str,
        text: &str,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        source_label: Option<&str>,
    ) {
        let audio_path = day.join(format!("{stem}.wav"));
        std::fs::write(&audio_path, b"wav").expect("audio");
        std::fs::write(audio_path.with_extension("txt"), text).expect("transcript");
        let metadata = ChunkMetadata {
            model_id: "whisper-medium".into(),
            requested_model_id: None,
            effective_model_id: None,
            fallback_from_model_id: None,
            device_id: "default".into(),
            device_name: "System Default".into(),
            sample_rate: 16_000,
            threshold_dbfs: -42.0,
            attack_ms: 100,
            release_ms: 1_000,
            pre_roll_ms: 1_000,
            lead_in_padding_ms: 300,
            post_roll_ms: 300,
            min_chunk_ms: 600,
            max_chunk_ms: 120_000,
            started_at,
            ended_at,
            duration_ms: (ended_at - started_at).num_milliseconds(),
            transcription_status: TranscriptionStatus::Completed,
            app_version: "0.0.0".into(),
            used_fallback_device: false,
            live_capture_chunk_id: None,
            source: ChunkSource::Microphone,
            source_label: source_label.map(str::to_string),
            microphone_slot: None,
            microphone_inputs: Vec::new(),
            transcribed_at: None,
            transcript_text: Some(text.to_string()),
        };
        std::fs::write(
            audio_path.with_extension("json"),
            serde_json::to_vec_pretty(&metadata).expect("metadata json"),
        )
        .expect("metadata");
    }

    #[test]
    fn transcript_views_prefer_overlapping_dictation_over_the_same_live_transcript() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let day = tmp.path().join("20260528");
        std::fs::create_dir_all(&day).expect("day dir");
        let base = Utc.with_ymd_and_hms(2026, 5, 28, 9, 0, 0).unwrap();
        write_transcript_fixture(
            &day,
            "090000",
            "wake note 오늘 회의를 시작합니다",
            base,
            base + chrono::Duration::seconds(5),
            None,
        );
        write_transcript_fixture(
            &day,
            "090001",
            "WakeNote, 오늘 회의를 시작합니다.",
            base + chrono::Duration::seconds(1),
            base + chrono::Duration::seconds(4),
            Some("dictation"),
        );
        write_transcript_fixture(
            &day,
            "090002",
            "겹치지만 내용은 다른 메모",
            base + chrono::Duration::seconds(2),
            base + chrono::Duration::seconds(3),
            None,
        );
        write_transcript_fixture(
            &day,
            "091000",
            "WakeNote, 오늘 회의를 시작합니다.",
            base + chrono::Duration::minutes(10),
            base + chrono::Duration::minutes(10) + chrono::Duration::seconds(3),
            None,
        );

        let day_entries = transcripts_for_day_from_save_root(tmp.path(), "2026-05-28", false);
        let recent_entries = recent_transcripts_from_save_root(tmp.path(), 10);

        for entries in [&day_entries, &recent_entries] {
            assert_eq!(entries.len(), 3);
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.source_label.as_deref() == Some("dictation"))
            );
            assert!(!entries.iter().any(|entry| {
                entry.source_label.is_none() && entry.text == "wake note 오늘 회의를 시작합니다"
            }));
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.text == "겹치지만 내용은 다른 메모")
            );
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.recorded_at.contains("09:10:00"))
            );
        }
    }

    #[test]
    fn dictation_duplicate_matching_tolerates_small_transcription_differences() {
        assert!(transcript_texts_match(
            "오늘 Wake Note 회의를 시작하겠습니다",
            "오늘 wakenote 회의를 바로 시작하겠습니다.",
        ));
        assert!(!transcript_texts_match(
            "오늘 WakeNote 회의를 시작하겠습니다",
            "제품 배포는 다음 주로 연기합니다",
        ));
    }
}
