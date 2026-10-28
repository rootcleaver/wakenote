use chrono::{TimeZone, Utc};
use wakenote::audio::{
    GateConfig, GateDecision, MicHealthConfig, ROLLOVER_PAUSE_MS, SpeechGate, TOP_PRIORITY_RECHECK,
    dbfs_from_rms,
    input_devices_from_labels,
};
use wakenote::commands::{AppBackend, pinned_device_mismatch};
use wakenote::live_capture::AudioFrame;
use wakenote::models::{ModelStatus, default_model_registry};
use wakenote::settings::{
    AppSettings, AudioFormat, CaptureMicrophoneEntry, LaunchAtLoginAction,
    LiveCaptureRuntimeAction, MicrophonePriorityEntry, MicrophoneSlot, SettingsPatch, ThemeMode,
    TranscriptionLanguage, capture_microphones_share_device, expand_user_path,
    launch_at_login_action_for_patch, live_capture_runtime_action_for_patch,
    live_capture_should_run, live_capture_should_start_on_launch, microphone_source_key,
    normalize_capture_microphones,
};
use wakenote::storage::{OutputBasename, next_available_output};

#[test]
fn dbfs_conversion_clamps_silence_and_reports_full_scale() {
    assert_eq!(dbfs_from_rms(0.0), -120.0);
    assert_eq!(dbfs_from_rms(-1.0), -120.0);
    assert!((dbfs_from_rms(1.0) - 0.0).abs() < 0.001);
    assert!((dbfs_from_rms(0.5) - -6.0206).abs() < 0.001);
}

#[test]
fn input_device_list_marks_system_default_unavailable_without_default_input() {
    let devices = input_devices_from_labels(false, ["USB Mic", "Built-in Microphone"]);

    assert_eq!(devices[0].id, "default");
    assert_eq!(devices[0].label, "System Default");
    assert!(!devices[0].available);
    assert_eq!(devices[1].id, "input-0-usb-mic");
    assert!(devices[1].available);
}

#[test]
fn speech_gate_rejects_transient_spikes_shorter_than_attack() {
    let mut gate = SpeechGate::new(GateConfig {
        threshold_dbfs: -45.0,
        attack_ms: 300,
        release_ms: 1_500,
        pre_roll_ms: 300,
        post_roll_ms: 300,
        min_chunk_ms: 500,
        max_chunk_ms: 30_000,
        rollover_grace_ms: 0,
    });

    assert_eq!(gate.observe(-80.0, 0), GateDecision::Idle);
    assert_eq!(gate.observe(-30.0, 100), GateDecision::Idle);
    assert_eq!(gate.observe(-80.0, 250), GateDecision::Idle);
    assert!(!gate.is_recording());
}

#[test]
fn speech_gate_starts_after_attack_and_ends_after_release() {
    let mut gate = SpeechGate::new(GateConfig {
        threshold_dbfs: -45.0,
        attack_ms: 300,
        release_ms: 1_500,
        pre_roll_ms: 300,
        post_roll_ms: 300,
        min_chunk_ms: 500,
        max_chunk_ms: 30_000,
        rollover_grace_ms: 0,
    });

    assert_eq!(gate.observe(-30.0, 0), GateDecision::Idle);
    assert_eq!(gate.observe(-31.0, 299), GateDecision::Idle);
    assert_eq!(
        gate.observe(-32.0, 300),
        GateDecision::Start { started_at_ms: 0 }
    );
    assert_eq!(gate.observe(-80.0, 1_000), GateDecision::Recording);
    assert_eq!(gate.observe(-82.0, 2_499), GateDecision::Recording);
    assert_eq!(gate.observe(-82.0, 2_500), GateDecision::Recording);
    assert_eq!(
        gate.observe(-82.0, 2_800),
        GateDecision::End { ended_at_ms: 2_800 }
    );
    assert!(!gate.is_recording());
}

#[test]
fn speech_gate_rolls_over_at_max_chunk_length_without_stopping() {
    let mut gate = SpeechGate::new(GateConfig {
        threshold_dbfs: -45.0,
        attack_ms: 100,
        release_ms: 1_500,
        pre_roll_ms: 0,
        post_roll_ms: 300,
        min_chunk_ms: 500,
        max_chunk_ms: 1_000,
        rollover_grace_ms: 0,
    });

    assert_eq!(gate.observe(-30.0, 0), GateDecision::Idle);
    assert_eq!(
        gate.observe(-30.0, 100),
        GateDecision::Start { started_at_ms: 0 }
    );
    assert_eq!(
        gate.observe(-30.0, 1_100),
        GateDecision::Rollover {
            ended_at_ms: 1_100,
            next_started_at_ms: 1_100
        }
    );
    assert!(gate.is_recording());
}

#[test]
fn speech_gate_with_rollover_grace_waits_for_a_pause_after_max_chunk_length() {
    let mut gate = SpeechGate::new(GateConfig {
        threshold_dbfs: -45.0,
        attack_ms: 100,
        release_ms: 750,
        pre_roll_ms: 0,
        post_roll_ms: 0,
        min_chunk_ms: 500,
        max_chunk_ms: 1_000,
        rollover_grace_ms: 3_000,
    });

    assert_eq!(gate.observe(-30.0, 0), GateDecision::Idle);
    assert_eq!(
        gate.observe(-30.0, 100),
        GateDecision::Start { started_at_ms: 0 }
    );
    // Past the soft limit while still speaking: keep the word together.
    assert_eq!(gate.observe(-30.0, 1_100), GateDecision::Recording);
    // A dip shorter than the pause span does not cut.
    assert_eq!(gate.observe(-80.0, 1_200), GateDecision::Recording);
    assert_eq!(gate.observe(-80.0, 1_250), GateDecision::Recording);
    assert_eq!(gate.observe(-30.0, 1_300), GateDecision::Recording);
    // A pause of ROLLOVER_PAUSE_MS rolls over inside the quiet span.
    assert_eq!(gate.observe(-80.0, 1_400), GateDecision::Recording);
    assert_eq!(
        gate.observe(-80.0, 1_400 + ROLLOVER_PAUSE_MS),
        GateDecision::Rollover {
            ended_at_ms: 1_400 + ROLLOVER_PAUSE_MS,
            next_started_at_ms: 1_400 + ROLLOVER_PAUSE_MS
        }
    );
    assert!(gate.is_recording());
}

#[test]
fn speech_gate_with_rollover_grace_still_cuts_at_the_hard_limit() {
    let mut gate = SpeechGate::new(GateConfig {
        threshold_dbfs: -45.0,
        attack_ms: 100,
        release_ms: 750,
        pre_roll_ms: 0,
        post_roll_ms: 0,
        min_chunk_ms: 500,
        max_chunk_ms: 1_000,
        rollover_grace_ms: 3_000,
    });

    assert_eq!(gate.observe(-30.0, 0), GateDecision::Idle);
    assert_eq!(
        gate.observe(-30.0, 100),
        GateDecision::Start { started_at_ms: 0 }
    );
    assert_eq!(gate.observe(-30.0, 3_999), GateDecision::Recording);
    assert_eq!(
        gate.observe(-30.0, 4_000),
        GateDecision::Rollover {
            ended_at_ms: 4_000,
            next_started_at_ms: 4_000
        }
    );
}

#[test]
fn output_paths_use_date_time_format_and_collision_suffix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let timestamp = Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap();
    let local = timestamp.with_timezone(&chrono::Local);
    let dir = local.format("%Y%m%d").to_string();
    let stem = local.format("%y%m%d-%H%M%S").to_string();

    let first =
        next_available_output(tmp.path(), timestamp, AudioFormat::M4a, None).expect("first path");
    assert_eq!(first.basename, OutputBasename::new(stem.clone()));
    assert!(first.audio_path.ends_with(format!("{dir}/{stem}.m4a")));
    assert!(first.metadata_path.ends_with(format!("{dir}/{stem}.json")));
    assert!(first.transcript_path.ends_with(format!("{dir}/{stem}.txt")));
    assert!(
        first
            .error_path
            .ends_with(format!("{dir}/{stem}.error.txt"))
    );

    std::fs::create_dir_all(first.audio_path.parent().unwrap()).expect("date dir");
    std::fs::write(&first.audio_path, b"existing").expect("collision file");

    let second =
        next_available_output(tmp.path(), timestamp, AudioFormat::M4a, None).expect("second path");
    assert_eq!(second.basename, OutputBasename::new(format!("{stem}-2")));
    assert!(second.audio_path.ends_with(format!("{dir}/{stem}-2.m4a")));

    // A pre-existing sidecar (e.g. transcript or error file) alone should also
    // force a new basename so we never silently clobber a partial chunk.
    std::fs::write(&second.audio_path, b"second existing").expect("collision file");
    std::fs::write(&second.transcript_path, b"transcript").expect("collision sidecar");

    let third =
        next_available_output(tmp.path(), timestamp, AudioFormat::M4a, None).expect("third path");
    assert_eq!(third.basename, OutputBasename::new(format!("{stem}-3")));
    assert!(third.audio_path.ends_with(format!("{dir}/{stem}-3.m4a")));
}

#[test]
fn output_paths_support_mp3_recording_format() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let timestamp = Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap();
    let local = timestamp.with_timezone(&chrono::Local);
    let dir = local.format("%Y%m%d").to_string();
    let stem = local.format("%y%m%d-%H%M%S").to_string();

    let target =
        next_available_output(tmp.path(), timestamp, AudioFormat::Mp3, None).expect("mp3 path");

    assert!(target.audio_path.ends_with(format!("{dir}/{stem}.mp3")));
    assert!(target.metadata_path.ends_with(format!("{dir}/{stem}.json")));
    assert!(
        target
            .transcript_path
            .ends_with(format!("{dir}/{stem}.txt"))
    );
}

#[test]
fn user_paths_expand_current_home_tilde_without_rewriting_other_paths() {
    let home = std::env::var_os("HOME").expect("HOME should be set for path expansion tests");

    assert_eq!(
        expand_user_path("~/Documents/WakeNote"),
        std::path::PathBuf::from(&home).join("Documents/WakeNote")
    );
    assert_eq!(expand_user_path("~"), std::path::PathBuf::from(&home));
    assert_eq!(
        expand_user_path("/tmp/wakenote"),
        std::path::PathBuf::from("/tmp/wakenote")
    );
    assert_eq!(
        expand_user_path("relative/wakenote"),
        std::path::PathBuf::from("relative/wakenote")
    );
    assert_eq!(
        expand_user_path("~other/wakenote"),
        std::path::PathBuf::from("~other/wakenote")
    );
}

#[test]
fn default_settings_match_prd_mvp_defaults() {
    let settings = AppSettings::default();

    assert!(settings.recording_enabled);
    assert!(settings.transcription_enabled);
    assert!(!settings.save_root_confirmed);
    assert!(!settings.calibration_completed);
    assert_eq!(settings.selected_microphone, "default");
    assert_eq!(settings.audio_format, AudioFormat::M4a);
    assert_eq!(settings.threshold_dbfs, -40.0);
    assert_eq!(settings.transcription_language, TranscriptionLanguage::Ko);
    assert!(settings.suppress_low_confidence_transcripts);
    assert_eq!(settings.attack_ms, 200);
    assert_eq!(settings.release_ms, 1_000);
    assert_eq!(settings.pre_roll_ms, 400);
    assert_eq!(settings.lead_in_padding_ms, 200);
    assert_eq!(settings.post_roll_ms, 400);
    assert_eq!(settings.min_chunk_ms, 800);
    assert_eq!(settings.max_chunk_ms, 180_000);
    assert_eq!(settings.selected_model, "whisper-medium");
    assert_eq!(settings.save_root, "~/Documents/WakeNote");
    assert_eq!(
        settings.model_directory,
        "~/Library/Application Support/WakeNote/models"
    );
    assert!(settings.show_dock_icon);
    assert!(settings.show_tray_icon);
    assert!(settings.start_live_input_on_launch);
    assert!(!settings.permission_onboarding_seen);
    assert_eq!(settings.theme_mode, ThemeMode::Dark);
    assert_eq!(settings.theme_primary_color, "#000");
    assert_eq!(settings.openrouter_model, "z-ai/glm-5.2");
    assert_eq!(settings.llm_max_iterations, 3);
    assert!(
        settings
            .llm_summary_prompt_template
            .contains("{{transcripts}}")
    );
    assert!(
        settings
            .llm_report_prompt_template
            .contains("# Action Items")
    );
}

#[test]
fn settings_patch_updates_theme_mode() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        theme_mode: Some(ThemeMode::Light),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.theme_mode, ThemeMode::Light);
}

#[test]
fn settings_patch_updates_dock_icon_visibility() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        show_dock_icon: Some(false),
        ..SettingsPatch::default()
    });

    assert!(!settings.show_dock_icon);
}

#[test]
fn settings_patch_updates_start_live_input_on_launch() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        start_live_input_on_launch: Some(false),
        ..SettingsPatch::default()
    });

    assert!(!settings.start_live_input_on_launch);
}

#[test]
fn settings_patch_marks_permission_onboarding_seen() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        permission_onboarding_seen: Some(true),
        ..SettingsPatch::default()
    });

    assert!(settings.permission_onboarding_seen);
}

#[test]
fn settings_patch_updates_transcription_language() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        transcription_language: Some(TranscriptionLanguage::Ko),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.transcription_language, TranscriptionLanguage::Ko);
}

#[test]
fn settings_patch_updates_low_confidence_transcript_suppression() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        suppress_low_confidence_transcripts: Some(false),
        ..SettingsPatch::default()
    });

    assert!(!settings.suppress_low_confidence_transcripts);
}

#[test]
fn settings_patch_clamps_voice_gate_values_to_ui_ranges() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        threshold_dbfs: Some(12.0),
        attack_ms: Some(0),
        release_ms: Some(10_000),
        pre_roll_ms: Some(2_000),
        lead_in_padding_ms: Some(10_000),
        post_roll_ms: Some(4_000),
        min_chunk_ms: Some(0),
        max_chunk_ms: Some(1_000_000),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.threshold_dbfs, -10.0);
    assert_eq!(settings.attack_ms, 50);
    assert_eq!(settings.release_ms, 5_000);
    assert_eq!(settings.pre_roll_ms, 1_500);
    assert_eq!(settings.lead_in_padding_ms, 2_000);
    assert_eq!(settings.post_roll_ms, 2_000);
    assert_eq!(settings.min_chunk_ms, 100);
    assert_eq!(settings.max_chunk_ms, 900_000);
}

#[test]
fn settings_patch_marks_non_empty_save_root_confirmed() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        save_root: Some("/tmp/wakenote-recordings".to_string()),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.save_root, "/tmp/wakenote-recordings");
    assert!(settings.save_root_confirmed);

    settings.apply_patch(SettingsPatch {
        save_root: Some("   ".to_string()),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.save_root, "   ");
    assert!(!settings.save_root_confirmed);
}

#[test]
fn settings_patch_keeps_max_chunk_at_least_min_chunk() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        min_chunk_ms: Some(5_000),
        max_chunk_ms: Some(2_000),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.min_chunk_ms, 5_000);
    assert_eq!(settings.max_chunk_ms, 10_000);
}

#[test]
fn default_settings_seed_microphone_priority_with_system_default() {
    let settings = AppSettings::default();
    assert_eq!(
        settings.microphone_priority,
        vec![MicrophonePriorityEntry {
            id: "default".to_string(),
            label: "System Default".to_string(),
        }],
    );
    // Legacy mirrors stay consistent with priority[0].
    assert_eq!(settings.selected_microphone, "default");
    assert_eq!(settings.selected_microphone_label, "System Default");
}

#[test]
fn default_settings_seed_one_system_default_capture_microphone() {
    let settings = AppSettings::default();

    assert!(settings.merge_microphone_inputs);
    assert!(!settings.priority_microphone_inputs);
    assert_eq!(
        settings.capture_microphones,
        vec![CaptureMicrophoneEntry {
            id: "default".to_string(),
            label: "System Default".to_string(),
            core_audio_uid: None,
        }],
    );
}

#[test]
fn priority_microphone_input_patch_is_mutually_exclusive_and_reconciles_capture() {
    let active = AppSettings {
        capture_microphones: vec![
            CaptureMicrophoneEntry {
                id: "input-1-wired".to_string(),
                label: "Wired".to_string(),
                core_audio_uid: None,
            },
            CaptureMicrophoneEntry {
                id: "input-2-wireless".to_string(),
                label: "Wireless".to_string(),
                core_audio_uid: None,
            },
        ],
        ..AppSettings::default()
    };
    let patch = SettingsPatch {
        merge_microphone_inputs: Some(false),
        priority_microphone_inputs: Some(true),
        ..SettingsPatch::default()
    };

    assert_eq!(
        live_capture_runtime_action_for_patch(&active, &patch),
        LiveCaptureRuntimeAction::Reconcile,
    );
    let mut updated = active;
    updated.apply_patch(patch);
    assert!(!updated.merge_microphone_inputs);
    assert!(updated.priority_microphone_inputs);
}

#[test]
fn legacy_settings_without_microphone_processing_fields_use_merge_mode() {
    let mut value = serde_json::to_value(AppSettings::default()).expect("settings json");
    let object = value.as_object_mut().expect("settings object");
    object.remove("merge_microphone_inputs");
    object.remove("priority_microphone_inputs");

    let settings: AppSettings = serde_json::from_value(value).expect("legacy settings");

    assert!(settings.merge_microphone_inputs);
    assert!(!settings.priority_microphone_inputs);
}

#[test]
fn legacy_single_microphone_migrates_without_enabling_priority_fallback() {
    let mut value = serde_json::to_value(AppSettings {
        selected_microphone: "input-3-boya".to_string(),
        selected_microphone_label: "BOYA".to_string(),
        microphone_priority: vec![
            MicrophonePriorityEntry {
                id: "input-3-boya".to_string(),
                label: "BOYA".to_string(),
            },
            MicrophonePriorityEntry {
                id: "input-7-airpods".to_string(),
                label: "AirPods".to_string(),
            },
        ],
        ..AppSettings::default()
    })
    .expect("settings json");
    value
        .as_object_mut()
        .expect("settings object")
        .remove("capture_microphones");

    let mut settings: AppSettings = serde_json::from_value(value).expect("legacy settings");
    settings.normalize_capture_microphones();

    assert_eq!(
        settings.capture_microphones,
        vec![CaptureMicrophoneEntry {
            id: "input-3-boya".to_string(),
            label: "BOYA".to_string(),
            core_audio_uid: None,
        }],
    );
}

#[test]
fn capture_microphones_keep_the_same_device_on_both_slots() {
    let entry = |id: &str, label: &str| CaptureMicrophoneEntry {
        id: id.to_string(),
        label: label.to_string(),
        core_audio_uid: None,
    };

    assert_eq!(
        normalize_capture_microphones(vec![
            entry("input-1-wired", "Wired"),
            entry("input-1-wired", "Duplicate"),
            entry("input-2-wireless", "Wireless"),
            entry("input-3-third", "Third"),
        ]),
        vec![
            entry("input-1-wired", "Wired"),
            entry("input-1-wired", "Duplicate"),
        ],
    );
    assert_eq!(
        normalize_capture_microphones(vec![entry("", "Missing"), entry("input-1-wired", "Wired"),]),
        vec![entry("input-1-wired", "Wired")],
    );
}

#[test]
fn microphone_source_key_includes_the_slot_only_for_a_shared_device() {
    let studio = CaptureMicrophoneEntry {
        id: "studio".to_string(),
        label: "Studio".to_string(),
        core_audio_uid: Some("uid-studio".to_string()),
    };
    let shared = vec![studio.clone(), studio.clone()];
    assert!(capture_microphones_share_device(&shared));
    assert_eq!(
        microphone_source_key("studio", MicrophoneSlot::Primary, &shared),
        "microphone:primary:studio"
    );
    assert_eq!(
        microphone_source_key("studio", MicrophoneSlot::Secondary, &shared),
        "microphone:secondary:studio"
    );

    let distinct = vec![
        studio,
        CaptureMicrophoneEntry {
            id: "wireless".to_string(),
            label: "Wireless".to_string(),
            core_audio_uid: Some("uid-wireless".to_string()),
        },
    ];
    assert!(!capture_microphones_share_device(&distinct));
    assert_eq!(
        microphone_source_key("studio", MicrophoneSlot::Primary, &distinct),
        "microphone:studio"
    );
    assert_eq!(
        microphone_source_key("wireless", MicrophoneSlot::Secondary, &distinct),
        "microphone:wireless"
    );

    let same_uid = vec![
        CaptureMicrophoneEntry {
            id: "default".to_string(),
            label: "System Default".to_string(),
            core_audio_uid: Some("built-in".to_string()),
        },
        CaptureMicrophoneEntry {
            id: "internal".to_string(),
            label: "Built-in".to_string(),
            core_audio_uid: Some("built-in".to_string()),
        },
    ];
    assert!(capture_microphones_share_device(&same_uid));
    assert_eq!(
        microphone_source_key("internal", MicrophoneSlot::Secondary, &same_uid),
        "microphone:secondary:internal"
    );
}

#[test]
fn system_default_can_be_combined_with_an_explicit_capture_microphone() {
    let entry = |id: &str, label: &str| CaptureMicrophoneEntry {
        id: id.to_string(),
        label: label.to_string(),
        core_audio_uid: None,
    };

    assert_eq!(
        normalize_capture_microphones(vec![
            entry("default", "System Default"),
            entry("input-2-wireless", "Wireless"),
        ]),
        vec![
            entry("default", "System Default"),
            entry("input-2-wireless", "Wireless")
        ],
    );
    assert_eq!(
        normalize_capture_microphones(vec![
            entry("input-1-wired", "Wired"),
            entry("default", "System Default"),
        ]),
        vec![
            entry("input-1-wired", "Wired"),
            entry("default", "System Default")
        ],
    );
}

#[test]
fn legacy_only_settings_migrate_into_priority_list() {
    // Simulates loading a settings.json saved by an older release that only
    // wrote `selected_microphone[_label]` and has no `microphone_priority`.
    let mut settings = AppSettings::default();
    settings.apply_patch(SettingsPatch {
        selected_microphone: Some("input-3-usb-mic".to_string()),
        selected_microphone_label: Some("USB Mic".to_string()),
        ..SettingsPatch::default()
    });

    assert_eq!(
        settings.microphone_priority.first(),
        Some(&MicrophonePriorityEntry {
            id: "input-3-usb-mic".to_string(),
            label: "USB Mic".to_string(),
        }),
    );
    // Legacy fields stay in sync with priority[0].
    assert_eq!(settings.selected_microphone, "input-3-usb-mic");
    assert_eq!(settings.selected_microphone_label, "USB Mic");
}

#[test]
fn microphone_priority_patch_drives_legacy_selected_microphone() {
    // The new UI sends `microphone_priority`. The backend must mirror its
    // first entry into the legacy single-mic fields so every consumer that
    // still reads them stays consistent.
    let mut settings = AppSettings::default();
    settings.apply_patch(SettingsPatch {
        microphone_priority: Some(vec![
            MicrophonePriorityEntry {
                id: "input-3-usb-mic".to_string(),
                label: "USB Mic".to_string(),
            },
            MicrophonePriorityEntry {
                id: "default".to_string(),
                label: "System Default".to_string(),
            },
        ]),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.microphone_priority.len(), 2);
    assert_eq!(settings.selected_microphone, "input-3-usb-mic");
    assert_eq!(settings.selected_microphone_label, "USB Mic");
}

#[test]
fn microphone_priority_normalize_drops_duplicates_and_empty_ids() {
    let mut settings = AppSettings::default();
    settings.apply_patch(SettingsPatch {
        microphone_priority: Some(vec![
            MicrophonePriorityEntry {
                id: "input-3-usb-mic".to_string(),
                label: "USB Mic".to_string(),
            },
            MicrophonePriorityEntry {
                id: "input-3-usb-mic".to_string(),
                label: "USB Mic (duplicate)".to_string(),
            },
            MicrophonePriorityEntry {
                id: String::new(),
                label: "Bogus".to_string(),
            },
            MicrophonePriorityEntry {
                id: "default".to_string(),
                label: "System Default".to_string(),
            },
        ]),
        ..SettingsPatch::default()
    });

    assert_eq!(
        settings.microphone_priority,
        vec![
            MicrophonePriorityEntry {
                id: "input-3-usb-mic".to_string(),
                label: "USB Mic".to_string(),
            },
            MicrophonePriorityEntry {
                id: "default".to_string(),
                label: "System Default".to_string(),
            },
        ],
    );
}

#[test]
fn legacy_selected_microphone_patch_moves_to_top_of_existing_priority() {
    // Users on the new UI may also occasionally trigger code paths that send
    // only the legacy single field (tray menu shortcuts etc.). When that
    // happens we promote the named device to position 0 of the existing
    // priority list rather than throwing the priority away.
    let mut settings = AppSettings::default();
    settings.apply_patch(SettingsPatch {
        microphone_priority: Some(vec![
            MicrophonePriorityEntry {
                id: "default".to_string(),
                label: "System Default".to_string(),
            },
            MicrophonePriorityEntry {
                id: "input-3-usb-mic".to_string(),
                label: "USB Mic".to_string(),
            },
        ]),
        ..SettingsPatch::default()
    });
    settings.apply_patch(SettingsPatch {
        selected_microphone: Some("input-3-usb-mic".to_string()),
        selected_microphone_label: Some("USB Mic".to_string()),
        ..SettingsPatch::default()
    });

    assert_eq!(
        settings.microphone_priority,
        vec![
            MicrophonePriorityEntry {
                id: "input-3-usb-mic".to_string(),
                label: "USB Mic".to_string(),
            },
            MicrophonePriorityEntry {
                id: "default".to_string(),
                label: "System Default".to_string(),
            },
        ],
    );
    assert_eq!(settings.selected_microphone, "input-3-usb-mic");
}

#[test]
fn microphone_priority_reorder_triggers_live_capture_restart() {
    // Changing priority[0] is functionally a microphone change — the live
    // capture must restart so the new top device gets opened.
    let active = AppSettings::default();

    let new_priority = vec![
        MicrophonePriorityEntry {
            id: "input-3-usb-mic".to_string(),
            label: "USB Mic".to_string(),
        },
        MicrophonePriorityEntry {
            id: "default".to_string(),
            label: "System Default".to_string(),
        },
    ];

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &active,
            &SettingsPatch {
                microphone_priority: Some(new_priority),
                ..SettingsPatch::default()
            },
        ),
        LiveCaptureRuntimeAction::Restart,
    );
}

#[test]
fn capture_microphone_patch_reconciles_live_capture_slots() {
    let active = AppSettings::default();

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &active,
            &SettingsPatch {
                capture_microphones: Some(vec![
                    CaptureMicrophoneEntry {
                        id: "input-1-wired".to_string(),
                        label: "Wired".to_string(),
                        core_audio_uid: None,
                    },
                    CaptureMicrophoneEntry {
                        id: "input-2-wireless".to_string(),
                        label: "Wireless".to_string(),
                        core_audio_uid: None,
                    },
                ]),
                ..SettingsPatch::default()
            },
        ),
        LiveCaptureRuntimeAction::Reconcile,
    );
}

#[test]
fn merge_microphone_input_patch_reconciles_live_capture_slots() {
    let active = AppSettings {
        capture_microphones: vec![
            CaptureMicrophoneEntry {
                id: "input-1-wired".to_string(),
                label: "Wired".to_string(),
                core_audio_uid: None,
            },
            CaptureMicrophoneEntry {
                id: "input-2-wireless".to_string(),
                label: "Wireless".to_string(),
                core_audio_uid: None,
            },
        ],
        ..AppSettings::default()
    };

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &active,
            &SettingsPatch {
                merge_microphone_inputs: Some(false),
                ..SettingsPatch::default()
            },
        ),
        LiveCaptureRuntimeAction::Reconcile,
    );
}

#[test]
fn backend_dual_microphone_slots_process_and_stop_independently() {
    let mut backend = AppBackend::default();
    backend.apply_settings_patch_for_test(SettingsPatch {
        capture_microphones: Some(vec![
            CaptureMicrophoneEntry {
                id: "input-1-wired".to_string(),
                label: "Wired".to_string(),
                core_audio_uid: None,
            },
            CaptureMicrophoneEntry {
                id: "input-2-wireless".to_string(),
                label: "Wireless".to_string(),
                core_audio_uid: None,
            },
        ]),
        ..SettingsPatch::default()
    });
    let base_time = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
    backend
        .start_capture_session_for_slot(
            MicrophoneSlot::Primary,
            16_000,
            base_time,
            "input-1-wired",
            "Wired",
            false,
        )
        .expect("start primary");
    backend
        .start_capture_session_for_slot(
            MicrophoneSlot::Secondary,
            16_000,
            base_time,
            "input-2-wireless",
            "Wireless",
            false,
        )
        .expect("start secondary");

    let frame = || AudioFrame {
        samples: vec![0.25; 320],
        duration_ms: 20,
        captured_at: base_time + chrono::Duration::milliseconds(20),
    };
    backend
        .process_audio_frame_for_slot(MicrophoneSlot::Primary, frame())
        .expect("primary frame");
    backend
        .process_audio_frame_for_slot(MicrophoneSlot::Secondary, frame())
        .expect("secondary frame");

    let status = backend.app_status();
    assert_eq!(status.microphone_captures.len(), 2);
    assert!(status.microphone_captures.iter().all(|mic| mic.active));

    backend
        .stop_capture_slot(MicrophoneSlot::Secondary)
        .expect("stop secondary");
    assert!(backend.capture_slot_active(MicrophoneSlot::Primary));
    assert!(!backend.capture_slot_active(MicrophoneSlot::Secondary));
}

#[test]
fn settings_patch_records_completed_threshold_calibration() {
    let mut settings = AppSettings::default();

    settings.apply_patch(SettingsPatch {
        threshold_dbfs: Some(-43.0),
        calibration_completed: Some(true),
        ..SettingsPatch::default()
    });

    assert_eq!(settings.threshold_dbfs, -43.0);
    assert!(settings.calibration_completed);
}

#[test]
fn launch_at_login_action_follows_explicit_settings_patch() {
    let settings = AppSettings::default();

    assert_eq!(
        launch_at_login_action_for_patch(&settings, &SettingsPatch::default()),
        LaunchAtLoginAction::Unchanged
    );
    assert_eq!(
        launch_at_login_action_for_patch(
            &settings,
            &SettingsPatch {
                launch_at_login: Some(true),
                ..SettingsPatch::default()
            }
        ),
        LaunchAtLoginAction::Enable
    );

    let enabled = AppSettings {
        launch_at_login: true,
        ..AppSettings::default()
    };

    assert_eq!(
        launch_at_login_action_for_patch(
            &enabled,
            &SettingsPatch {
                launch_at_login: Some(false),
                ..SettingsPatch::default()
            }
        ),
        LaunchAtLoginAction::Disable
    );
}

#[test]
fn live_capture_runtime_action_stops_when_recording_is_disabled_or_paused() {
    let settings = AppSettings::default();

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &settings,
            &SettingsPatch {
                recording_enabled: Some(false),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Stop
    );
    assert_eq!(
        live_capture_runtime_action_for_patch(
            &settings,
            &SettingsPatch {
                pause_all: Some(true),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Stop
    );
    assert_eq!(
        live_capture_runtime_action_for_patch(&settings, &SettingsPatch::default()),
        LiveCaptureRuntimeAction::Unchanged
    );

    let already_paused = AppSettings {
        pause_all: true,
        ..AppSettings::default()
    };

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &already_paused,
            &SettingsPatch {
                pause_all: Some(true),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Unchanged
    );
}

#[test]
fn live_capture_runtime_action_starts_and_restarts_for_active_recording_settings() {
    let disabled = AppSettings {
        recording_enabled: false,
        ..AppSettings::default()
    };

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &disabled,
            &SettingsPatch {
                recording_enabled: Some(true),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Start
    );

    let paused = AppSettings {
        pause_all: true,
        ..AppSettings::default()
    };

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &paused,
            &SettingsPatch {
                pause_all: Some(false),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Start
    );

    let active = AppSettings::default();

    assert_eq!(
        live_capture_runtime_action_for_patch(
            &active,
            &SettingsPatch {
                selected_microphone: Some("usb-mic".to_string()),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Restart
    );
    assert_eq!(
        live_capture_runtime_action_for_patch(
            &active,
            &SettingsPatch {
                selected_microphone: Some(active.selected_microphone.clone()),
                ..SettingsPatch::default()
            }
        ),
        LiveCaptureRuntimeAction::Unchanged
    );
}

#[test]
fn live_capture_should_run_only_when_recording_is_enabled_and_not_paused() {
    assert!(live_capture_should_run(&AppSettings::default()));
    assert!(!live_capture_should_run(&AppSettings {
        recording_enabled: false,
        ..AppSettings::default()
    }));
    assert!(!live_capture_should_run(&AppSettings {
        pause_all: true,
        ..AppSettings::default()
    }));
}

#[test]
fn live_capture_should_start_on_launch_respects_auto_start_toggle() {
    assert!(!live_capture_should_start_on_launch(&AppSettings::default()));
    assert!(live_capture_should_start_on_launch(&AppSettings {
        permission_onboarding_seen: true,
        ..AppSettings::default()
    }));
    assert!(!live_capture_should_start_on_launch(&AppSettings {
        start_live_input_on_launch: false,
        permission_onboarding_seen: true,
        ..AppSettings::default()
    }));
    assert!(!live_capture_should_start_on_launch(&AppSettings {
        recording_enabled: false,
        permission_onboarding_seen: true,
        ..AppSettings::default()
    }));
    assert!(!live_capture_should_start_on_launch(&AppSettings {
        pause_all: true,
        permission_onboarding_seen: true,
        ..AppSettings::default()
    }));
}

#[test]
fn default_model_registry_includes_whisper_medium_and_fast_local_fallback() {
    let registry = default_model_registry();

    let small = registry.get("whisper-small").expect("small model");
    assert_eq!(small.display_name, "Whisper Small");
    assert!(small.languages.iter().any(|language| language == "ko"));
    assert_eq!(small.size_mb, 465);
    assert!(small.offline);
    assert_eq!(small.status, ModelStatus::Missing);
    assert!(small.checksum_sha256.is_some());

    let medium = registry.get("whisper-medium").expect("medium model");
    assert_eq!(medium.display_name, "Whisper Medium");
    assert!(medium.offline);
    assert_eq!(medium.status, ModelStatus::Missing);
    assert!(medium.checksum_sha256.is_some());

    let turbo = registry.get("whisper-turbo").expect("turbo model");
    assert_eq!(turbo.display_name, "Whisper Turbo");
    assert!(turbo.offline);
    assert_eq!(turbo.status, ModelStatus::Missing);
    assert!(turbo.speed_score > medium.speed_score);
    assert!(turbo.accuracy_score < medium.accuracy_score);
    assert!(turbo.languages.iter().any(|language| language == "ko"));

    let large = registry.get("whisper-large").expect("large model");
    assert_eq!(large.display_name, "Whisper Large");
    assert!(large.languages.iter().any(|language| language == "ko"));
    assert_eq!(large.size_mb, 1_031);
    assert!(large.offline);
    assert_eq!(large.status, ModelStatus::Missing);
    assert!(large.accuracy_score >= medium.accuracy_score);
    assert!(large.checksum_sha256.is_some());
}

#[test]
fn default_registry_uses_pinned_remote_sha256() {
    // Pin the exact SHA256 checksums for each bundled whisper.cpp model.
    // Bumping or changing any of these strings means the binary content has
    // shifted; verify against `curl -L <url> | shasum -a 256` before updating.
    const EXPECTED: &[(&str, &str)] = &[
        (
            "whisper-small",
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        ),
        (
            "whisper-medium",
            "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        ),
        (
            "whisper-turbo",
            "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        ),
        (
            "whisper-large",
            "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
        ),
    ];

    let registry = default_model_registry();
    for (model_id, expected_hash) in EXPECTED {
        let model = registry
            .get(*model_id)
            .unwrap_or_else(|| panic!("registry missing {model_id}"));
        let actual = model
            .checksum_sha256
            .as_deref()
            .unwrap_or_else(|| panic!("{model_id} missing checksum_sha256"));
        assert_eq!(actual, *expected_hash, "{model_id} checksum mismatch");
        assert_eq!(actual.len(), 64, "{model_id} checksum must be 64 hex chars");
        assert!(
            actual
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{model_id} checksum must be lowercase hex"
        );
    }
}

#[test]
fn silence_warning_verdict_populates_app_status() {
    use std::time::{Duration, Instant};

    let mut backend = AppBackend::default();
    backend.apply_settings_patch_for_test(SettingsPatch {
        recording_enabled: Some(true),
        selected_microphone: Some("mic-a".to_string()),
        selected_microphone_label: Some("Mic A".to_string()),
        microphone_priority: Some(vec![MicrophonePriorityEntry {
            id: "mic-a".to_string(),
            label: "Mic A".to_string(),
        }]),
        ..SettingsPatch::default()
    });
    backend.override_mic_health_config_for_test(MicHealthConfig {
        startup_grace: Duration::from_millis(10),
        stall_threshold: Duration::from_millis(100),
        heartbeat_threshold: Duration::from_secs(30),
        recovery_cooldown: Duration::from_millis(50),
        max_recovery_cooldown: Duration::from_millis(200),
        top_priority_recheck: TOP_PRIORITY_RECHECK,
        force_default_after_escalations: u32::MAX,
    });
    backend.start_capture_session_for_test(48_000).unwrap();

    // Feed silent frames over >1 s so that the silence_age reported in
    // `since` converts to at least 1 whole second.
    let mut clock = Instant::now() + Duration::from_millis(20);
    for _ in 0..12 {
        backend.observe_audio_for_test(-120.0, clock);
        clock += Duration::from_millis(100);
    }
    let _ = backend.evaluate_microphone_health_at(clock);

    let warning = backend
        .app_status()
        .silence_warning
        .expect("silence_warning should be Some after silence exceeds stall_threshold");
    assert!(!warning.device_label.is_empty());
    assert!(
        warning.seconds >= 1,
        "seconds should be at least 1, got {}",
        warning.seconds
    );

    // A non-silent frame followed by another tick should clear the warning.
    clock += Duration::from_millis(30);
    backend.observe_audio_for_test(-30.0, clock);
    let _ = backend.evaluate_microphone_health_at(clock);
    assert!(
        backend.app_status().silence_warning.is_none(),
        "silence_warning should be None after healthy audio"
    );
}

#[test]
fn stale_stable_id_with_matching_label_does_not_set_fallback_warning() {
    // End-to-end guard for the 'Pinned BY-V is unavailable; using BY-V' bug.
    // Reproduces the in-the-wild flow:
    //   1. Settings persist an old stable id (cpal enumerated BY-V at index 3 once).
    //   2. After a USB blip / sleep cycle, cpal now enumerates BY-V at index 5.
    //   3. The resolver matches the same physical mic via label-hint, returning
    //      the new stable id with `used_fallback_device = false`.
    //   4. main.rs computes `used_fallback_device` via `pinned_device_mismatch`
    //      and starts the capture session.
    // The fix: label match counts as 'we are on the pinned device', so no
    // warning fires; and `reconcile_resolved_device_id` rewrites settings
    // to the new id so the next start matches by id without needing the
    // label-hint fallback.
    let mut backend = AppBackend::default();
    backend.apply_settings_patch_for_test(SettingsPatch {
        recording_enabled: Some(true),
        selected_microphone: Some("input-3-by-v".to_string()),
        selected_microphone_label: Some("BY-V".to_string()),
        microphone_priority: Some(vec![
            MicrophonePriorityEntry {
                id: "input-3-by-v".to_string(),
                label: "BY-V".to_string(),
            },
            MicrophonePriorityEntry {
                id: "default".to_string(),
                label: "System Default".to_string(),
            },
        ]),
        ..SettingsPatch::default()
    });

    // Simulate the resolver's output after label-hint matching: same physical
    // device, new stable id, NOT a true fallback.
    let resolved_id = "input-5-by-v";
    let resolved_label = "BY-V";
    let resolver_fell_back = false;

    let settings = backend.settings();
    let used_fallback_device = pinned_device_mismatch(
        &settings.selected_microphone,
        &settings.selected_microphone_label,
        resolved_id,
        resolved_label,
        resolver_fell_back,
    );

    assert!(
        !used_fallback_device,
        "id drift with matching label must NOT be flagged as a fallback"
    );

    backend.reconcile_resolved_device_id(
        &settings.selected_microphone,
        resolved_id,
        resolved_label,
    );
    backend
        .start_capture_session_with_device(
            48_000,
            Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 0).unwrap(),
            resolved_id.to_string(),
            resolved_label.to_string(),
            used_fallback_device,
        )
        .expect("start should succeed");

    assert!(
        backend.app_status().microphone_warning.is_none(),
        "microphone_warning must be None after stale-id drift with matching label, got {:?}",
        backend.app_status().microphone_warning
    );

    // Settings have been reconciled to the new stable id — next start will
    // hit the exact-id match path, no more label-hint round-trip.
    let after = backend.settings();
    assert_eq!(after.selected_microphone, resolved_id);
    assert_eq!(after.selected_microphone_label, resolved_label);
    assert_eq!(after.microphone_priority[0].id, resolved_id);
    assert_eq!(after.microphone_priority[0].label, resolved_label);
    // The second priority entry (System Default) must be untouched.
    assert_eq!(after.microphone_priority[1].id, "default");
}

#[test]
fn true_device_fallback_still_produces_warning() {
    // Negative guard: when the resolver fell back to a genuinely different
    // device (label differs), the warning must still fire so the user
    // knows their pinned mic is unavailable.
    let mut backend = AppBackend::default();
    backend.apply_settings_patch_for_test(SettingsPatch {
        recording_enabled: Some(true),
        selected_microphone: Some("input-3-by-v".to_string()),
        selected_microphone_label: Some("BY-V".to_string()),
        microphone_priority: Some(vec![MicrophonePriorityEntry {
            id: "input-3-by-v".to_string(),
            label: "BY-V".to_string(),
        }]),
        ..SettingsPatch::default()
    });

    // Resolver fell back to System Default because BY-V was disconnected.
    let resolved_id = "default";
    let resolved_label = "System Default";
    let resolver_fell_back = true;

    let settings = backend.settings();
    let used_fallback_device = pinned_device_mismatch(
        &settings.selected_microphone,
        &settings.selected_microphone_label,
        resolved_id,
        resolved_label,
        resolver_fell_back,
    );

    assert!(used_fallback_device, "real fallback must be flagged");

    backend
        .start_capture_session_with_device(
            48_000,
            Utc.with_ymd_and_hms(2026, 5, 27, 0, 0, 0).unwrap(),
            resolved_id.to_string(),
            resolved_label.to_string(),
            used_fallback_device,
        )
        .expect("start should succeed");

    let warning = backend
        .app_status()
        .microphone_warning
        .expect("warning should be set for genuine device fallback");
    assert!(warning.contains("Pinned microphone BY-V"));
    assert!(warning.contains("System Default"));
}
