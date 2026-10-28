import enhancePromptDefault from "../assets/enhance-prompt.txt?raw";
import type {
  AppMode,
  AppSettings,
  AppSnapshot,
  AppStatus,
  AppPermissions,
  LevelSnapshot,
  ModelCapabilities,
  ModelDescriptor,
  QueueSnapshot,
  TrayState,
} from "./types";
import { RECORDING_DEFAULTS } from "./recording-settings";

type ModeInput = Pick<
  AppSettings,
  "recording_enabled" | "transcription_enabled" | "pause_all"
>;

export function deriveProductMode(settings: ModeInput): AppMode {
  if (
    settings.pause_all ||
    (!settings.recording_enabled && !settings.transcription_enabled)
  ) {
    return "paused";
  }

  if (settings.recording_enabled && settings.transcription_enabled) {
    return "recording_and_transcription";
  }

  if (settings.recording_enabled) {
    return "recording_only";
  }

  return "transcription_only";
}

export function deriveTrayState(
  mode: AppMode,
  hasRunningTranscription: boolean,
  hasError = false,
  hasActiveCapture = false,
  isRecording = false,
): TrayState {
  if (isRecording) {
    return "recording";
  }

  if (hasRunningTranscription) {
    return "transcribing";
  }

  if (hasError) {
    return "error";
  }

  switch (mode) {
    case "recording_and_transcription":
    case "recording_only":
      return hasActiveCapture ? "listening" : "idle";
    case "transcription_only":
      return "idle";
    case "paused":
      return "paused";
  }
}

function hasProcessablePendingTranscription(
  queue: QueueSnapshot,
  models: ModelDescriptor[],
) {
  const usableModelIds = new Set(
    models
      .filter((model) =>
        ["ready", "installed", "unloaded"].includes(model.status),
      )
      .map((model) => model.id),
  );

  return queue.jobs.some(
    (job) => job.status === "pending" && usableModelIds.has(job.model_id),
  );
}

export function shouldPollSnapshot(
  status: AppStatus,
  queue: QueueSnapshot,
  models: ModelDescriptor[] = [],
  options: { launchAutoStartPending?: boolean } = {},
) {
  const canProcessPendingTranscription =
    status.mode === "recording_and_transcription" ||
    status.mode === "transcription_only";

  return (
    options.launchAutoStartPending ||
    ["listening", "recording", "transcribing"].includes(status.tray_state) ||
    status.live_input_active ||
    status.runtime_warning?.startsWith("Live input stream error:") ||
    (canProcessPendingTranscription &&
      hasProcessablePendingTranscription(queue, models)) ||
    queue.running_count > 0 ||
    models.some((model) =>
      ["downloading", "verifying", "extracting"].includes(model.status),
    )
  );
}

export function pollSnapshotDependencyKey(
  status: AppStatus,
  queue: QueueSnapshot,
  models: ModelDescriptor[] = [],
  options: { launchAutoStartPending?: boolean } = {},
) {
  return JSON.stringify({
    mode: status.mode,
    trayState: status.tray_state,
    liveInputActive: status.live_input_active,
    runtimeWarning: status.runtime_warning ?? null,
    launchAutoStartPending: Boolean(options.launchAutoStartPending),
    pendingCount: queue.pending_count,
    runningCount: queue.running_count,
    pendingJobs: queue.jobs
      .filter((job) => job.status === "pending")
      .map((job) => ({ id: job.id, modelId: job.model_id })),
    models: models.map((model) => ({
      id: model.id,
      status: model.status,
      downloadProgress: model.download_progress ?? null,
    })),
  });
}

export function shouldRefreshSnapshotForTauriEvent(eventName: string) {
  return (
    eventName === "source-capture-started" ||
    eventName === "source-capture-stopped" ||
    eventName === "source-capture-error" ||
    eventName === "dictionary-changed" ||
    eventName === "dictionary-file-error" ||
    eventName === "microphone-input-levels-changed" ||
    eventName === "live-input-changed"
  );
}

export function defaultSettings(): AppSettings {
  return {
    recording_enabled: true,
    transcription_enabled: true,
    transcription_language: "ko",
    transcription_language_hints: ["en", "ko"],
    suppress_low_confidence_transcripts: true,
    pause_all: false,
    selected_microphone: "default",
    selected_microphone_label: "System Default",
    microphone_priority: [{ id: "default", label: "System Default" }],
    capture_microphones: [{ id: "default", label: "System Default" }],
    merge_microphone_inputs: true,
    priority_microphone_inputs: false,
    save_root: "~/Documents/WakeNote",
    save_root_confirmed: false,
    audio_format: "m4a",
    audio_bitrate_kbps: 96,
    ...RECORDING_DEFAULTS,
    calibration_completed: false,
    selected_model: "whisper-medium",
    meeting_model: "whisper-medium",
    transcription_options: {
      streaming_enabled: false,
      cost_limit_fallback_enabled: false,
      cost_limit_fallback_model_id: null,
    },
    model_directory: "~/Library/Application Support/WakeNote/models",
    dictionary_enabled: true,
    dictionary: [],
    vad_enabled: false,
    voice_auto_level_enabled: true,
    launch_at_login: false,
    start_live_input_on_launch: true,
    permission_onboarding_seen: true,
    input_monitoring_enabled: false,
    auto_transcript_input_enabled: false,
    auto_transcript_input_trailing_space: false,
    auto_transcript_input_model: "",
    auto_transcript_input_final_only: false,
    voice_gate_enabled: false,
    voice_gate_threshold: 0.45,
    live_transcription_webhook: {
      enabled: false,
      endpoint_url: "",
      payload_format: "text_only",
      auto_delete_history: true,
    },
    text_transform_model: "",
    subtitle_translation_enabled: false,
    subtitle_translation_language: "ko",
    transcription_translation_enabled: false,
    transcription_translation_language: null,
    dictation_translation_enabled: false,
    dictation_translation_language: "ko",
    enhance_prompt_enabled: false,
    enhance_prompt_shortcut: "ctrl+alt+space",
    enhance_prompt_system_prompt: enhancePromptDefault.trim(),
    dictation_enabled: false,
    dictation_shortcut: "alt+space",
    dictation_toggle_shortcut: "",
    cycle_microphone_shortcut: "ctrl+shift+m",
    auto_type_toggle_shortcut: "",
    dictation_language: "auto",
    dictation_start_sound: "original",
    dictation_stop_sound: "original",
    dictation_end_sound: "original",
    dictation_cue_volume: "medium",
    dictation_bubble_position: "top_center",
    dictation_bubble_background_color: "#000000",
    dictation_bubble_background_opacity: 88,
    dictation_model: "",
    dictation_copy_to_clipboard: true,
    dictation_remove_trailing_space: false,
    show_dock_icon: true,
    show_tray_icon: true,
    tray_left_click_action: "open_menu",
    show_floating_overlay: false,
    subtitle_source_mode: "both",
    floating_overlay_position: "top",
    floating_overlay_font_size_px: 24,
    floating_overlay_text_color: "#ffffff",
    floating_overlay_background_color: "#000000",
    floating_overlay_background_opacity: 82,
    subtitle_duration_seconds: 5,
    subtitle_animation: "fade",
    subtitle_padding_horizontal_px: 18,
    subtitle_padding_vertical_px: 14,
    subtitle_border_width_px: 1,
    subtitle_border_color: "#ffffff",
    subtitle_border_radius_px: 8,
    subtitle_max_width_px: 720,
    subtitle_min_height_px: 58,
    subtitle_max_height_px: 1_000,
    theme_mode: "dark",
    theme_primary_color: "#000",
    system_audio_enabled: false,
    autoplay_next_transcript: false,
    openrouter_model: "z-ai/glm-5.2",
    llm_summary_prompt_template: [
      "You are WakeNote's transcript summary assistant.",
      "Do not invent facts that are not present in the transcript. Mark uncertainty clearly.",
      "Write in the transcript's dominant language; if Korean is present, write in Korean.",
      "",
      "Date range: {{date_range}}",
      "Selected transcripts: {{selected_count}}",
      "",
      "Return this structure:",
      "- One-line summary",
      "- Key points",
      "- Decisions",
      "- Action items",
      "- Open questions",
      "",
      "Transcript:",
      "{{transcripts}}",
    ].join("\n"),
    llm_report_prompt_template: [
      "You are WakeNote's detailed transcript report writer.",
      "Use only the supplied transcript as evidence. Do not add unsupported assumptions.",
      "Preserve important time/source context where it helps the reader verify the report.",
      "",
      "Date range: {{date_range}}",
      "Selected transcripts: {{selected_count}}",
      "",
      "Write a Markdown report with these sections:",
      "# Summary",
      "# Context",
      "# Chronological Details",
      "# Main Discussion Points",
      "# Decisions",
      "# Action Items",
      "# Risks and Issues",
      "# Open Questions",
      "# Evidence Notes",
      "",
      "Transcript:",
      "{{transcripts}}",
    ].join("\n"),
    llm_max_iterations: 3,
    source_auto_prompt: [],
    custom_sources: [],
  };
}

function mockModelCapabilities(
  model: Pick<ModelDescriptor, "id" | "offline">,
): ModelCapabilities {
  if (model.id === "openai-gpt-live-transcribe") {
    return {
      file_transcription: false,
      realtime: true,
      streaming: "required",
      diarization: false,
      cost_reporting: "duration_estimate",
      maximum_request_bytes: null,
      selectable_contexts: ["realtime", "dictation", "meeting"],
    };
  }
  if (model.id === "soniox-realtime-v5") {
    return {
      file_transcription: false,
      realtime: true,
      streaming: "required",
      diarization: false,
      cost_reporting: "duration_estimate",
      maximum_request_bytes: null,
      selectable_contexts: ["realtime", "dictation", "meeting"],
    };
  }
  if (model.id === "soniox-async-v5") {
    return {
      file_transcription: true,
      realtime: false,
      streaming: "unsupported",
      diarization: false,
      cost_reporting: "duration_estimate",
      maximum_request_bytes: null,
      selectable_contexts: ["file", "dictation", "meeting"],
    };
  }
  if (model.id === "openai-gpt-transcribe") {
    return {
      file_transcription: true,
      realtime: true,
      streaming: "optional",
      diarization: false,
      cost_reporting: "duration_estimate",
      maximum_request_bytes: 25_000_000,
      selectable_contexts: ["file", "realtime", "dictation", "meeting"],
    };
  }
  if (model.id === "openai-gpt-4o-transcribe-diarize") {
    return {
      file_transcription: true,
      realtime: false,
      streaming: "optional",
      diarization: true,
      cost_reporting: "duration_estimate",
      maximum_request_bytes: 25_000_000,
      selectable_contexts: ["file", "meeting"],
    };
  }
  if (model.id === "openrouter-qwen3-asr-flash") {
    return {
      file_transcription: true,
      realtime: false,
      streaming: "unsupported",
      diarization: false,
      cost_reporting: "none",
      maximum_request_bytes: null,
      selectable_contexts: ["file", "dictation", "meeting"],
    };
  }
  return {
    file_transcription: model.offline,
    realtime: model.id === "nemotron-3.5-asr-streaming-0.6b",
    streaming: "unsupported",
    diarization: false,
    cost_reporting: "none",
    maximum_request_bytes: null,
    selectable_contexts:
      model.id === "nemotron-3.5-asr-streaming-0.6b"
        ? ["file", "realtime", "dictation", "meeting"]
        : ["file", "dictation", "meeting"],
  };
}

export function mockModels(): ModelDescriptor[] {
  const models: Omit<ModelDescriptor, "capabilities">[] = [
    {
      id: "whisper-large",
      display_name: "Whisper Large",
      engine: "whisper.cpp",
      provider_runtime: "whisper-rs",
      download_url:
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin",
      checksum_sha256:
        "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
      size_mb: 1031,
      languages: ["ko", "en", "multi"],
      speed_score: 3,
      accuracy_score: 9,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "whisper-medium",
      display_name: "Whisper Medium",
      engine: "whisper.cpp",
      provider_runtime: "whisper-rs",
      download_url:
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
      checksum_sha256:
        "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
      size_mb: 1465,
      languages: ["ko", "en", "multi"],
      speed_score: 5,
      accuracy_score: 8,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "whisper-small",
      display_name: "Whisper Small",
      engine: "whisper.cpp",
      provider_runtime: "whisper-rs",
      download_url:
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
      checksum_sha256:
        "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
      size_mb: 465,
      languages: ["ko", "en", "multi"],
      speed_score: 8,
      accuracy_score: 6,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "whisper-turbo",
      display_name: "Whisper Turbo",
      engine: "whisper.cpp",
      provider_runtime: "whisper-rs",
      download_url:
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
      checksum_sha256:
        "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
      size_mb: 1550,
      languages: ["ko", "en", "multi"],
      speed_score: 8,
      accuracy_score: 7,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "parakeet-tdt-0.6b-v3",
      display_name: "Parakeet TDT 0.6B V3",
      engine: "NVIDIA",
      provider_runtime: "sherpa-onnx",
      download_url:
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
      checksum_sha256: null,
      size_mb: 660,
      languages: ["en", "multi"],
      speed_score: 8,
      accuracy_score: 8,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "sensevoice-small",
      display_name: "SenseVoice Small",
      engine: "SenseVoice",
      provider_runtime: "sherpa-onnx",
      download_url:
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2",
      checksum_sha256: null,
      size_mb: 250,
      languages: ["ko", "en", "ja", "zh", "multi"],
      speed_score: 9,
      accuracy_score: 8,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "nemotron-3.5-asr-streaming-0.6b",
      display_name: "Nemotron 3.5 ASR Streaming 0.6B",
      engine: "NVIDIA",
      provider_runtime: "sherpa-onnx",
      download_url:
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-1120ms-int8-2026-06-11.tar.bz2",
      checksum_sha256: null,
      size_mb: 650,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 8,
      accuracy_score: 8,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "qwen3-asr-0.6b",
      display_name: "Qwen3-ASR 0.6B",
      engine: "Qwen",
      provider_runtime: "qwen3-asr",
      download_url: null,
      checksum_sha256: null,
      size_mb: 1505,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 7,
      accuracy_score: 9,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "qwen3-asr-1.7b",
      display_name: "Qwen3-ASR 1.7B",
      engine: "Qwen",
      provider_runtime: "qwen3-asr",
      download_url: null,
      checksum_sha256: null,
      size_mb: 3900,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 4,
      accuracy_score: 10,
      offline: true,
      status: "missing",
      download_progress: null,
      download_error: null,
    },
    {
      id: "openrouter-qwen3-asr-flash",
      display_name: "OpenRouter · Qwen3 ASR Flash",
      engine: "OpenRouter · Qwen",
      provider_runtime: "openrouter-stt",
      download_url: null,
      checksum_sha256: null,
      size_mb: 0,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 9,
      accuracy_score: 9,
      offline: false,
      status: "ready",
      download_progress: null,
      download_error: null,
    },
    {
      id: "openai-gpt-live-transcribe",
      display_name: "OpenAI · GPT Live Transcribe",
      engine: "OpenAI",
      provider_runtime: "openai-realtime",
      download_url: null,
      checksum_sha256: null,
      size_mb: 0,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 10,
      accuracy_score: 9,
      offline: false,
      status: "ready",
      download_progress: null,
      download_error: null,
    },
    {
      id: "openai-gpt-transcribe",
      display_name: "OpenAI · GPT Transcribe",
      engine: "OpenAI",
      provider_runtime: "openai-stt",
      download_url: null,
      checksum_sha256: null,
      size_mb: 0,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 9,
      accuracy_score: 9,
      offline: false,
      status: "ready",
      download_progress: null,
      download_error: null,
    },
    {
      id: "openai-gpt-4o-transcribe-diarize",
      display_name: "OpenAI · GPT-4o Transcribe Diarize",
      engine: "OpenAI",
      provider_runtime: "openai-stt",
      download_url: null,
      checksum_sha256: null,
      size_mb: 0,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 8,
      accuracy_score: 9,
      offline: false,
      status: "ready",
      download_progress: null,
      download_error: null,
    },
    {
      id: "soniox-async-v5",
      display_name: "Soniox · Async V5",
      engine: "Soniox",
      provider_runtime: "soniox-async-stt",
      download_url: null,
      checksum_sha256: null,
      size_mb: 0,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 9,
      accuracy_score: 9,
      offline: false,
      status: "ready",
      download_progress: null,
      download_error: null,
    },
    {
      id: "soniox-realtime-v5",
      display_name: "Soniox · Real-time V5",
      engine: "Soniox",
      provider_runtime: "soniox-realtime",
      download_url: null,
      checksum_sha256: null,
      size_mb: 0,
      languages: ["ko", "en", "ja", "zh", "es", "fr", "de", "multi"],
      speed_score: 10,
      accuracy_score: 9,
      offline: false,
      status: "ready",
      download_progress: null,
      download_error: null,
    },
  ];
  return models.map((model) => ({
    ...model,
    capabilities: mockModelCapabilities(model),
  }));
}

export function emptyQueue(): QueueSnapshot {
  return {
    jobs: [],
    pending_count: 0,
    running_count: 0,
    failed_count: 0,
  };
}

export function defaultLevelSnapshot(): LevelSnapshot {
  return {
    current_dbfs: -120,
    peak_dbfs: -120,
    noise_floor_dbfs: -120,
    suggested_threshold_dbfs: -90,
  };
}

export function defaultPermissions(): AppPermissions {
  return {
    accessibility: {
      status: "granted",
      label: "Allowed",
      detail: "WakeNote can type Dictation results into the focused app.",
      can_request: false,
      can_open_settings: true,
    },
    microphone: {
      status: "granted",
      label: "Allowed",
      detail: "WakeNote can use the microphone.",
      can_request: false,
      can_open_settings: true,
    },
    screen_recording: {
      status: "granted",
      label: "Allowed",
      detail: "WakeNote can capture system audio (Google Meet, Zoom, YouTube).",
      can_request: false,
      can_open_settings: true,
    },
  };
}

export function mockSnapshot(): AppSnapshot {
  const settings = defaultSettings();
  const queue = emptyQueue();
  const mode = deriveProductMode(settings);
  const trayState = deriveTrayState(mode, queue.running_count > 0);

  return {
    settings,
    status: {
      mode,
      tray_state: trayState,
      live_input_active: false,
      active_model: settings.selected_model,
      active_microphone: settings.selected_microphone_label,
      microphone_warning: null,
      runtime_warning: null,
      threshold_dbfs: settings.threshold_dbfs,
      level: defaultLevelSnapshot(),
      microphone_captures: settings.capture_microphones.map(
        (microphone, index) => ({
          slot: index === 0 ? "primary" : "secondary",
          device_id: microphone.id,
          label: microphone.label,
          active: false,
          reconnecting: false,
          warning: null,
          level: defaultLevelSnapshot(),
        }),
      ),
      queue,
    },
    microphones: [
      {
        id: "default",
        label: "System Default",
        available: true,
        fallback: false,
      },
      {
        id: "builtin-input",
        label: "Built-in Microphone",
        available: true,
        fallback: false,
      },
    ],
    microphone_input_levels: [
      {
        device_id: "default",
        label: "System Default",
        volume_percent: 100,
        writable: true,
        available: true,
        error: null,
      },
    ],
    models: mockModels(),
    queue,
    recent_transcripts: [],
    permissions: defaultPermissions(),
    openrouter_key_configured: false,
    openai_key_configured: false,
    soniox_key_configured: false,
    dictionary_file_status: {
      path: "~/Library/Application Support/WakeNote/dictionary.txt",
      revision: "browser",
      error: null,
      error_line: null,
      in_sync: true,
    },
  };
}
