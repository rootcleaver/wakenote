export type AudioFormat = "m4a" | "mp3" | "wav";

export type FloatingOverlayPosition =
  | "off"
  | "top_left"
  | "top"
  | "top_right"
  | "bottom_left"
  | "bottom"
  | "bottom_right";
export type SubtitleAnimation = "instant" | "fade" | "dissolve";
export type SubtitleSourceMode = "live_transcription" | "dictation" | "both";
export type DictationCueSound = "original" | "alternative";
export type DictationCueVolume = "muted" | "small" | "medium" | "large";
export type DictationStage = "idle" | "recording" | "transcribing" | "error";
export interface DictationStatePayload {
  state: DictationStage;
  error: string | null;
}
export type DictationBubblePosition =
  | "top_left"
  | "top_center"
  | "top_right"
  | "bottom_left"
  | "bottom_center"
  | "bottom_right";
export type ThemeMode = "light" | "dark";
export type TrayClickAction = "toggle_pause" | "open_menu";
export type TranscriptionLanguage =
  | "auto"
  | "ko"
  | "en"
  | "ja"
  | "zh"
  | "es"
  | "fr"
  | "de";

export type AppMode =
  | "recording_and_transcription"
  | "recording_only"
  | "transcription_only"
  | "paused";

export type TrayState =
  | "idle"
  | "listening"
  | "recording"
  | "transcribing"
  | "paused"
  | "error";

export type ModelStatus =
  | "installed"
  | "missing"
  | "downloading"
  | "verifying"
  | "extracting"
  | "ready"
  | "unloaded"
  | "error";

export type StreamingCapability = "unsupported" | "optional" | "required";
export type CostReportingCapability =
  | "none"
  | "provider_actual"
  | "duration_estimate";
export type TranscriptionContext =
  | "file"
  | "realtime"
  | "dictation"
  | "meeting";

export interface ModelCapabilities {
  file_transcription: boolean;
  realtime: boolean;
  streaming: StreamingCapability;
  diarization: boolean;
  cost_reporting: CostReportingCapability;
  maximum_request_bytes: number | null;
  selectable_contexts: TranscriptionContext[];
}

export interface TranscriptionOptions {
  streaming_enabled: boolean;
  cost_limit_fallback_enabled: boolean;
  cost_limit_fallback_model_id: string | null;
}

export type QueueJobStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "skipped";

export type QueueActivityKind =
  | "dictation"
  | "live_transcription"
  | "imported_audio"
  | "other";

export type QueueIssueSeverity = "warning" | "error";

export type QueueIssueCode =
  | "empty_transcript"
  | "no_speech"
  | "low_confidence"
  | "transcript_artifact"
  | "authentication"
  | "billing_limit"
  | "rate_limit"
  | "provider"
  | "transport"
  | "invalid_response"
  | "model"
  | "audio_decode"
  | "local_io"
  | "cancelled"
  | "skipped"
  | "unknown";

export interface QueueJobIssue {
  severity: QueueIssueSeverity;
  code: QueueIssueCode;
  message: string;
  occurred_at?: string | null;
}

export interface MicrophonePriorityEntry {
  id: string;
  label: string;
}

export interface CaptureMicrophoneEntry {
  id: string;
  label: string;
  core_audio_uid?: string | null;
}

export type MicrophoneSlot = "primary" | "secondary";

export interface DictionaryEntry {
  id: string;
  term: string;
  aliases: string[];
  enabled: boolean;
}

export interface DictionaryFileStatus {
  path: string;
  revision: string;
  error: string | null;
  error_line: number | null;
  in_sync: boolean;
}

export interface LiveTranscriptionWebhookSettings {
  enabled: boolean;
  endpoint_url: string;
  payload_format: "text_only" | "json";
  auto_delete_history: boolean;
}

export interface AppSettings {
  live_transcription_webhook: LiveTranscriptionWebhookSettings;
  recording_enabled: boolean;
  transcription_enabled: boolean;
  transcription_language: TranscriptionLanguage;
  transcription_language_hints: TranscriptionLanguage[];
  suppress_low_confidence_transcripts: boolean;
  pause_all: boolean;
  selected_microphone: string;
  selected_microphone_label: string;
  microphone_priority: MicrophonePriorityEntry[];
  capture_microphones: CaptureMicrophoneEntry[];
  merge_microphone_inputs: boolean;
  priority_microphone_inputs: boolean;
  save_root: string;
  save_root_confirmed: boolean;
  audio_format: AudioFormat;
  audio_bitrate_kbps: number;
  threshold_dbfs: number;
  calibration_completed: boolean;
  attack_ms: number;
  release_ms: number;
  pre_roll_ms: number;
  lead_in_padding_ms: number;
  post_roll_ms: number;
  min_chunk_ms: number;
  max_chunk_ms: number;
  selected_model: string;
  meeting_model: string;
  transcription_options: TranscriptionOptions;
  model_directory: string;
  dictionary_enabled: boolean;
  dictionary: DictionaryEntry[];
  vad_enabled: boolean;
  voice_auto_level_enabled: boolean;
  launch_at_login: boolean;
  start_live_input_on_launch: boolean;
  permission_onboarding_seen: boolean;
  input_monitoring_enabled: boolean;
  auto_transcript_input_enabled: boolean;
  auto_transcript_input_trailing_space: boolean;
  auto_transcript_input_model: string;
  auto_transcript_input_final_only: boolean;
  voice_gate_enabled: boolean;
  voice_gate_threshold: number;
  text_transform_model: string;
  subtitle_translation_enabled: boolean;
  subtitle_translation_language: TranscriptionLanguage;
  transcription_translation_enabled: boolean;
  transcription_translation_language: TranscriptionLanguage | null;
  dictation_translation_enabled: boolean;
  dictation_translation_language: TranscriptionLanguage;
  enhance_prompt_enabled: boolean;
  enhance_prompt_shortcut: string;
  enhance_prompt_system_prompt: string;
  dictation_enabled: boolean;
  dictation_shortcut: string;
  dictation_toggle_shortcut: string;
  cycle_microphone_shortcut: string;
  dictation_language: TranscriptionLanguage;
  dictation_start_sound: DictationCueSound;
  dictation_stop_sound: DictationCueSound;
  dictation_end_sound: DictationCueSound;
  dictation_cue_volume: DictationCueVolume;
  dictation_bubble_position: DictationBubblePosition;
  dictation_bubble_background_color: string;
  dictation_bubble_background_opacity: number;
  dictation_model: string;
  dictation_copy_to_clipboard: boolean;
  dictation_remove_trailing_space: boolean;
  show_dock_icon: boolean;
  show_tray_icon: boolean;
  tray_left_click_action: TrayClickAction;
  show_floating_overlay: boolean;
  subtitle_source_mode: SubtitleSourceMode;
  floating_overlay_position: FloatingOverlayPosition;
  floating_overlay_font_size_px: number;
  floating_overlay_text_color: string;
  floating_overlay_background_color: string;
  floating_overlay_background_opacity: number;
  subtitle_duration_seconds: number;
  subtitle_animation: SubtitleAnimation;
  subtitle_padding_horizontal_px: number;
  subtitle_padding_vertical_px: number;
  subtitle_border_width_px: number;
  subtitle_border_color: string;
  subtitle_border_radius_px: number;
  subtitle_max_width_px: number;
  subtitle_min_height_px: number;
  subtitle_max_height_px: number;
  theme_mode: ThemeMode;
  theme_primary_color: string;
  system_audio_enabled: boolean;
  autoplay_next_transcript: boolean;
  openrouter_model: string;
  llm_summary_prompt_template: string;
  llm_report_prompt_template: string;
  llm_max_iterations: number;
  source_auto_prompt: SourceAutoPromptEntry[];
  custom_sources: CustomSourceEntry[];
}

export type SettingsPatch = Partial<AppSettings>;

/** Audio source a recorded chunk came from. Mirrors the Rust `ChunkSource`. */
export type ChunkSource = "microphone" | "system";

/** Per-source "auto-prompt on detection" override. */
export interface SourceAutoPromptEntry {
  source_id: string;
  auto_prompt: boolean;
}

/** User-defined system-audio source matched by window title or app name. */
export interface CustomSourceEntry {
  id: string;
  label: string;
  title_patterns: string[];
  auto_prompt: boolean;
}

/** A recognized capture source with its resolved auto-capture setting. */
export interface RecognizedSourceInfo {
  id: string;
  label: string;
  description: string;
  auto_prompt: boolean;
  title_patterns: string[];
  custom: boolean;
}

/** A recognized source currently detected on screen (source-detected/ended). */
export interface SourcePayload {
  source_id: string;
  label: string;
  app_name: string;
}

/** Current system-audio detection/capture state (source_capture_status). */
export interface SourceCaptureStatus {
  detected: SourcePayload | null;
  capturing: boolean;
}

export interface MicrophoneDevice {
  transport_type?: number | null;
  id: string;
  label: string;
  core_audio_uid?: string | null;
  available: boolean;
  fallback: boolean;
}

export interface MicrophoneInputLevel {
  device_id: string;
  label: string;
  volume_percent: number | null;
  writable: boolean;
  available: boolean;
  error: string | null;
}

export interface ModelDescriptor {
  id: string;
  display_name: string;
  engine: string;
  provider_runtime: string;
  download_url?: string | null;
  checksum_sha256?: string | null;
  size_mb: number;
  languages: string[];
  speed_score: number;
  accuracy_score: number;
  offline: boolean;
  status: ModelStatus;
  download_progress?: number | null;
  download_error?: string | null;
  capabilities: ModelCapabilities;
}

export interface QueueJob {
  id: number;
  audio_path: string;
  model_id: string;
  status: QueueJobStatus;
  error?: string | null;
  issue?: QueueJobIssue | null;
  is_read?: boolean;
  transcription_options?: TranscriptionOptions | null;
  activity_kind?: QueueActivityKind;
  replay_recorded_audio?: boolean;
}

export interface QueueSnapshot {
  jobs: QueueJob[];
  pending_count: number;
  running_count: number;
  failed_count: number;
}

export interface TrashActivityJobFailure {
  id: number;
  audio_path: string;
  error: string;
}

export interface TrashActivityJobsResult {
  queue: QueueSnapshot;
  removed_ids: number[];
  trashed_ids: number[];
  missing_ids: number[];
  failures: TrashActivityJobFailure[];
}

export interface TrashActivityJobsOutcome extends TrashActivityJobsResult {
  snapshot: AppSnapshot;
}

export interface TrashTranscriptFailure {
  transcript_path: string;
  error: string;
}

export interface TrashTranscriptsResult {
  removed_transcript_paths: string[];
  trashed_transcript_paths: string[];
  missing_transcript_paths: string[];
  failures: TrashTranscriptFailure[];
}

export type ListVisibilityKind =
  | "meeting"
  | "transcript"
  | "report_run"
  | "legacy_report";

export interface ListVisibilityTarget {
  kind: ListVisibilityKind;
  id: string;
}

export interface SetListVisibilityRequest {
  targets: ListVisibilityTarget[];
  hidden: boolean;
}

export interface ListVisibilityState {
  meetings: string[];
  transcripts: string[];
  report_runs: string[];
  legacy_reports: string[];
}

export interface RecentTranscript {
  transcript_path: string;
  audio_path: string | null;
  recorded_at: string;
  text: string;
  source?: ChunkSource;
  source_label?: string | null;
  device_id?: string | null;
  device_name?: string | null;
  microphone_slot?: MicrophoneSlot | null;
}

export interface MergeAudioInput {
  audio_path: string;
  recorded_at: string;
}

export interface MergeAudioRequest {
  operation_id: string;
  destination_path: string;
  inputs: MergeAudioInput[];
}

export type AudioMergeStage =
  | "preparing"
  | "converting"
  | "combining"
  | "encoding"
  | "saved";

export interface AudioMergeProgress {
  operation_id: string;
  stage: AudioMergeStage;
  completed_inputs: number;
  total_inputs: number;
  percent: number;
  current_input_label: string | null;
  destination_path: string | null;
}

export interface AudioMergeResult {
  operation_id: string;
  destination_path: string;
  input_count: number;
}

export type LlmReportKind = "summary" | "detailed_report";

export type LlmCompletionReason =
  | "success_criteria_met"
  | "max_iterations_reached";

export type LlmProgressStage =
  | "preparing"
  | "generating"
  | "evaluating"
  | "refining"
  | "saving"
  | "completed"
  | "max_iterations_reached"
  | "failed"
  | "cancelled";

export interface LlmGenerateRequest {
  kind: LlmReportKind;
  transcripts: RecentTranscript[];
  run_id?: string | null;
}

export interface LlmProgressEvent {
  run_id: string;
  stage: LlmProgressStage;
  iteration: number;
  max_iterations: number;
  message: string;
  detail?: string | null;
}

export type LlmReportRunStatus =
  | "queued"
  | "running"
  | "stopping"
  | "cancelled"
  | "failed"
  | "completed";

export interface LlmReportRunSnapshot {
  run_id: string;
  parent_run_id: string | null;
  revision: number;
  status: LlmReportRunStatus;
  stage: LlmProgressStage | null;
  kind: LlmReportKind;
  created_at: string;
  updated_at: string;
  started_at: string | null;
  finished_at: string | null;
  iteration: number;
  max_iterations: number;
  message: string;
  detail: string | null;
  error: string | null;
  progress: LlmProgressEvent[];
  model: string;
  selected_count: number;
  date_range: string;
  report_id: string | null;
  report_path: string | null;
  completion_reason: LlmCompletionReason | null;
  success_criteria_met: boolean | null;
  quality_feedback: string | null;
  usage: LlmUsageTotals | null;
}

export interface LlmUsageTotals {
  request_count: number;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
  cost: number | null;
}

export interface LlmReportHistoryItem {
  report_id: string;
  kind: LlmReportKind;
  created_at: string;
  file_name: string;
  report_path: string;
  model: string | null;
  iterations_used: number | null;
  max_iterations: number | null;
  success_criteria_met: boolean | null;
  completion_reason: LlmCompletionReason | null;
  quality_feedback: string | null;
  selected_count: number | null;
  date_range: string | null;
  usage: LlmUsageTotals | null;
  legacy: boolean;
}

export interface LlmReportHistoryDetail {
  item: LlmReportHistoryItem;
  content: string;
}

export interface ApiKeyStatus {
  configured: boolean;
}

export type OpenRouterKeyStatus = ApiKeyStatus;

export interface TranscriptDay {
  day: string;
  count: number;
}

export interface TranscriptExportResult {
  destination_path: string;
  day_count: number;
  transcript_count: number;
}

export interface UploadedAudio {
  audio_path: string;
  original_filename: string;
  stored_at: string;
}

// --- Long-form meeting transcription -------------------------------------

export type MeetingStatus =
  | "recorded"
  | "pending"
  | "processing"
  | "completed"
  | "failed"
  | "canceled";

export type MeetingSegmentStatus = "pending" | "completed" | "failed";

export type MeetingPhase =
  | "preparing_audio"
  | "splitting_audio"
  | "transcribing"
  | "finalizing";

export type ManualMeetingRecordingPhase = "off" | "recording" | "error";

export type ManualMeetingStopReason =
  | "manual"
  | "maximum_duration"
  | "application_quit"
  | "error";

export interface ManualMeetingRecordingStatus {
  generation: number;
  state: ManualMeetingRecordingPhase;
  meeting_id: string | null;
  started_at: string | null;
  elapsed_ms: number;
  remaining_ms: number;
  inputs: string[];
  stop_reason: ManualMeetingStopReason | null;
  error: string | null;
}

export interface MeetingProgress {
  phase?: MeetingPhase | null;
  segments_total: number;
  segments_done: number;
  processed_ms: number;
  elapsed_ms: number;
}

export interface MeetingSegment {
  index: number;
  start_ms: number;
  end_ms: number;
  status: MeetingSegmentStatus;
  text: string;
  no_speech: boolean;
}

export interface SpeakerTurn {
  speaker_id: string;
  part_index: number;
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface MeetingTranscriptionRequest {
  model_id: string;
  language: TranscriptionLanguage;
  language_hints?: TranscriptionLanguage[];
  streaming_enabled: boolean;
  speaker_separation_enabled: boolean;
}

export interface MeetingSummary {
  id: string;
  title: string;
  source_filename: string;
  status: MeetingStatus;
  duration_ms: number;
  created_at: string;
  updated_at: string;
  progress: MeetingProgress;
  model_id: string;
  language: TranscriptionLanguage;
  error: string | null;
  capture_warning: string | null;
}

export interface MeetingRecord extends MeetingSummary {
  audio_file: string;
  audio_format: string;
  app_version: string;
  segments: MeetingSegment[];
  failed_phase?: MeetingPhase | null;
  failed_segments?: number;
  transcription_request: MeetingTranscriptionRequest | null;
  speaker_turns: SpeakerTurn[];
  api_audio_duration_ms: number;
  api_cost_microusd: number;
  api_request_count: number;
  api_unpriced_request_count: number;
}

export interface TranscriptionCostPeriod {
  estimated_cost_usd: number;
  audio_duration_ms: number;
  request_count: number;
  unpriced_request_count: number;
}

export interface TranscriptionCostEntry {
  source_id: string;
  recorded_at: string;
  provider: string;
  model_id: string;
  audio_duration_ms: number;
  estimated_cost_usd: number | null;
  request_count: number;
  unpriced_request_count: number;
}

export interface TranscriptionCostDetails {
  currency: "USD";
  generated_at: string;
  entries: TranscriptionCostEntry[];
  entry_limit: number;
}

export interface TranscriptionCostSnapshot {
  currency: "USD";
  generated_at: string;
  today: TranscriptionCostPeriod;
  week: TranscriptionCostPeriod;
  month: TranscriptionCostPeriod;
  entry_count: number;
  disclosure: string;
}

export interface MeetingDetail {
  record: MeetingRecord;
  transcript: string;
  audio_path: string;
}

export interface MeetingProgressPayload {
  id: string;
  status: MeetingStatus;
  phase?: MeetingPhase;
  segments_total: number;
  segments_done: number;
  processed_ms: number;
  duration_ms: number;
  elapsed_ms: number;
  eta_ms: number;
}

export interface MeetingSegmentPayload {
  id: string;
  index: number;
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface MeetingFinishedPayload {
  id: string;
  status: MeetingStatus;
  error: string | null;
}

export interface AudioRange {
  start: number;
  end: number;
}

export interface AudioWaveform {
  duration_seconds: number;
  sample_rate: number;
  /** Absolute (positive) per-bucket peaks 0..1, used for skip-silence and
   * fallback rendering when signed peaks are unavailable. */
  peaks: number[];
  /** Positive per-bucket peak 0..1. Drawn upward from the centerline. */
  peaks_max?: number[];
  /** Negative per-bucket peak -1..0. Drawn downward from the centerline. */
  peaks_min?: number[];
  audible_ranges: AudioRange[];
}

export type PermissionGrantStatus =
  | "unknown"
  | "not_determined"
  | "granted"
  | "denied"
  | "restricted"
  | "unsupported";

export interface PermissionState {
  status: PermissionGrantStatus;
  label: string;
  detail: string;
  can_request: boolean;
  can_open_settings: boolean;
}

export interface AppPermissions {
  accessibility: PermissionState;
  microphone: PermissionState;
  screen_recording: PermissionState;
}

export type PermissionDragTarget = "accessibility" | "screen_recording";

export interface LevelSnapshot {
  current_dbfs: number;
  peak_dbfs: number;
  noise_floor_dbfs: number;
  suggested_threshold_dbfs: number;
}

export interface SilenceWarning {
  device_label: string;
  seconds: number;
}

export interface AppStatus {
  mode: AppMode;
  tray_state: TrayState;
  live_input_active: boolean;
  active_model: string;
  active_microphone: string;
  microphone_warning?: string | null;
  silence_warning?: SilenceWarning | null;
  runtime_warning?: string | null;
  threshold_dbfs: number;
  level: LevelSnapshot;
  microphone_captures: MicrophoneCaptureStatus[];
  queue: QueueSnapshot;
}

export interface MicrophoneCaptureStatus {
  slot: MicrophoneSlot;
  device_id: string;
  label: string;
  active: boolean;
  reconnecting: boolean;
  warning?: string | null;
  level: LevelSnapshot;
}

export interface AppSnapshot {
  settings: AppSettings;
  status: AppStatus;
  microphones: MicrophoneDevice[];
  microphone_input_levels: MicrophoneInputLevel[];
  models: ModelDescriptor[];
  queue: QueueSnapshot;
  recent_transcripts: RecentTranscript[];
  permissions: AppPermissions;
  openrouter_key_configured: boolean;
  openai_key_configured: boolean;
  soniox_key_configured: boolean;
  dictionary_file_status: DictionaryFileStatus;
}
