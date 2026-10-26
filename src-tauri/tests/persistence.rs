use wakenote::commands::AppBackend;
use wakenote::persistence::{
    AppPersistence, ListVisibilityKind, ListVisibilityTarget, SetListVisibilityRequest,
};
use wakenote::queue::{QueueIssueCode, QueueIssueSeverity, QueueJobStatus, TranscriptionQueue};
use wakenote::settings::{
    AppSettings, DictionaryEntry, FloatingOverlayPosition, SettingsPatch, SubtitleAnimation,
    ThemeMode,
};

#[test]
fn legacy_settings_keep_the_existing_dictation_chord_when_cycle_default_would_overlap() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AppPersistence::new(tmp.path());
    let mut legacy = serde_json::to_value(AppSettings::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("cycle_microphone_shortcut");
    legacy["dictation_enabled"] = true.into();
    legacy["dictation_shortcut"] = "ctrl+shift".into();
    std::fs::write(tmp.path().join("settings.json"), serde_json::to_vec(&legacy).unwrap()).unwrap();
    let loaded = store.load_settings().unwrap().unwrap();
    assert_eq!(loaded.dictation_shortcut, "ctrl+shift");
    assert!(loaded.cycle_microphone_shortcut.is_empty());
}

#[test]
fn cycle_shortcut_round_trips_and_empty_means_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AppPersistence::new(tmp.path());
    let mut settings = AppSettings::default();
    assert_eq!(settings.cycle_microphone_shortcut, "ctrl+shift+m");
    settings.apply_patch(SettingsPatch { cycle_microphone_shortcut: Some("ctrl+alt+k".into()), ..Default::default() });
    store.save_settings(&settings).unwrap();
    assert_eq!(store.load_settings().unwrap().unwrap().cycle_microphone_shortcut, "ctrl+alt+k");
    settings.apply_patch(SettingsPatch { cycle_microphone_shortcut: Some(String::new()), ..Default::default() });
    store.save_settings(&settings).unwrap();
    assert!(store.load_settings().unwrap().unwrap().cycle_microphone_shortcut.is_empty());
}

#[test]
fn persistence_round_trips_settings_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    let mut settings = AppSettings::default();
    settings.apply_patch(SettingsPatch {
        recording_enabled: Some(false),
        threshold_dbfs: Some(-38.0),
        selected_microphone: Some("input-1-usb-mic".to_string()),
        selected_microphone_label: Some("USB Mic".to_string()),
        save_root: Some("/tmp/wakenote-recordings".to_string()),
        theme_mode: Some(ThemeMode::Light),
        dictionary_enabled: Some(true),
        dictionary: Some(vec![DictionaryEntry {
            id: "wake".to_string(),
            term: " WakeNote ".to_string(),
            aliases: vec![" wake note ".to_string()],
            enabled: true,
        }]),
        permission_onboarding_seen: Some(true),
        ..SettingsPatch::default()
    });

    store.save_settings(&settings).expect("save settings");

    let loaded = store
        .load_settings()
        .expect("load settings")
        .expect("settings");
    assert!(!loaded.recording_enabled);
    assert_eq!(loaded.threshold_dbfs, -38.0);
    assert_eq!(loaded.selected_microphone, "input-1-usb-mic");
    assert_eq!(loaded.selected_microphone_label, "USB Mic");
    assert_eq!(loaded.save_root, "/tmp/wakenote-recordings");
    assert_eq!(loaded.theme_mode, ThemeMode::Light);
    assert_eq!(loaded.dictionary[0].term, "WakeNote");
    assert_eq!(loaded.dictionary[0].aliases, vec!["wake note"]);
    assert!(loaded.save_root_confirmed);
    assert!(loaded.permission_onboarding_seen);
}

#[test]
fn persistence_round_trips_openrouter_api_key_separately_from_settings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());

    assert!(
        !store
            .openrouter_api_key_configured()
            .expect("configured check")
    );

    store
        .save_openrouter_api_key("  sk-or-test-key  ")
        .expect("save api key");

    assert!(
        store
            .openrouter_api_key_configured()
            .expect("configured check")
    );
    assert_eq!(
        store.load_openrouter_api_key().expect("load key"),
        Some("sk-or-test-key".to_string())
    );
    assert!(!tmp.path().join("settings.json").exists());

    store.delete_openrouter_api_key().expect("delete api key");
    assert!(
        !store
            .openrouter_api_key_configured()
            .expect("configured check")
    );
    assert_eq!(store.load_openrouter_api_key().expect("load key"), None);
}

#[test]
fn persistence_rejects_blank_openrouter_api_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());

    assert!(store.save_openrouter_api_key("  ").is_err());
    assert!(
        !store
            .openrouter_api_key_configured()
            .expect("configured check")
    );
}

#[test]
fn persistence_round_trips_openai_api_key_separately_from_other_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());

    assert!(!store.openai_api_key_configured().expect("configured check"));
    store
        .save_openai_api_key("  sk-openai-test-key  ")
        .expect("save api key");

    assert!(store.openai_api_key_configured().expect("configured check"));
    assert_eq!(
        store.load_openai_api_key().expect("load key"),
        Some("sk-openai-test-key".to_string())
    );
    assert!(!tmp.path().join("settings.json").exists());
    assert!(!tmp.path().join("openrouter-secrets.json").exists());

    store.delete_openai_api_key().expect("delete api key");
    store
        .delete_openai_api_key()
        .expect("idempotent delete api key");
    assert!(!store.openai_api_key_configured().expect("configured check"));
}

#[test]
fn persistence_rejects_blank_openai_api_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());

    assert!(store.save_openai_api_key("  ").is_err());
    assert!(!store.openai_api_key_configured().expect("configured check"));
}

#[test]
fn persistence_round_trips_soniox_api_key_separately_from_other_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());

    assert!(!store.soniox_api_key_configured().expect("configured check"));
    store
        .save_soniox_api_key("  soniox-test-key  ")
        .expect("save api key");

    assert!(store.soniox_api_key_configured().expect("configured check"));
    assert_eq!(
        store.load_soniox_api_key().expect("load key"),
        Some("soniox-test-key".to_string())
    );
    assert!(!tmp.path().join("settings.json").exists());
    assert!(!tmp.path().join("openrouter-secrets.json").exists());
    assert!(!tmp.path().join("openai-secrets.json").exists());

    store.delete_soniox_api_key().expect("delete api key");
    store
        .delete_soniox_api_key()
        .expect("idempotent delete api key");
    assert!(!store.soniox_api_key_configured().expect("configured check"));
}

#[test]
fn persistence_rejects_blank_soniox_api_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());

    assert!(store.save_soniox_api_key("  ").is_err());
    assert!(!store.soniox_api_key_configured().expect("configured check"));
}

#[test]
fn persistence_migrates_legacy_settings_with_missing_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    std::fs::write(
        tmp.path().join("settings.json"),
        r##"{
          "recording_enabled": false,
          "threshold_dbfs": -39.5,
          "save_root": "/tmp/legacy-wakenote",
          "floating_overlay_background_color": "#123456",
          "floating_overlay_background_opacity": 68,
          "dictation_bubble_background_color": "#ffffff",
          "dictation_bubble_background_opacity": 41
        }"##,
    )
    .expect("legacy settings");

    let loaded = store
        .load_settings()
        .expect("load settings")
        .expect("settings");

    assert!(!loaded.recording_enabled);
    assert_eq!(loaded.threshold_dbfs, -39.5);
    assert_eq!(loaded.save_root, "/tmp/legacy-wakenote");
    assert!(loaded.save_root_confirmed);
    assert!(loaded.transcription_enabled);
    assert!(!loaded.calibration_completed);
    assert_eq!(loaded.attack_ms, 200);
    assert_eq!(loaded.release_ms, 1_000);
    assert_eq!(loaded.pre_roll_ms, 400);
    assert_eq!(loaded.lead_in_padding_ms, 200);
    assert_eq!(loaded.post_roll_ms, 400);
    assert_eq!(loaded.min_chunk_ms, 800);
    assert_eq!(loaded.max_chunk_ms, 180_000);
    assert_eq!(loaded.selected_model, "whisper-medium");
    assert!(loaded.start_live_input_on_launch);
    assert!(loaded.permission_onboarding_seen);
    assert!(loaded.suppress_low_confidence_transcripts);
    assert_eq!(loaded.theme_mode, ThemeMode::Dark);
    assert_eq!(loaded.theme_primary_color, "#000");
    assert_eq!(loaded.floating_overlay_background_color, "#000000");
    assert_eq!(loaded.floating_overlay_background_opacity, 68);
    assert_eq!(loaded.dictation_bubble_background_color, "#ffffff");
    assert_eq!(loaded.dictation_bubble_background_opacity, 41);
    assert_eq!(
        loaded.floating_overlay_position,
        FloatingOverlayPosition::Top
    );
    assert_eq!(loaded.openrouter_model, "z-ai/glm-5.2");
    assert_eq!(loaded.llm_max_iterations, 3);
    assert!(loaded.dictionary_enabled);
    assert!(loaded.dictionary.is_empty());
    assert!(
        loaded
            .llm_summary_prompt_template
            .contains("{{transcripts}}")
    );
    assert!(loaded.llm_report_prompt_template.contains("# Action Items"));
}

#[test]
fn persistence_round_trips_subtitle_presentation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    let mut settings = AppSettings::default();
    settings.apply_patch(SettingsPatch {
        floating_overlay_position: Some(FloatingOverlayPosition::BottomRight),
        subtitle_duration_seconds: Some(9),
        subtitle_animation: Some(SubtitleAnimation::Dissolve),
        ..SettingsPatch::default()
    });

    store.save_settings(&settings).expect("save settings");

    let loaded = store
        .load_settings()
        .expect("load settings")
        .expect("settings");
    assert_eq!(
        loaded.floating_overlay_position,
        FloatingOverlayPosition::BottomRight
    );
    assert_eq!(loaded.subtitle_duration_seconds, 9);
    assert_eq!(loaded.subtitle_animation, SubtitleAnimation::Dissolve);
}

#[test]
fn persistence_round_trips_queue_and_recovers_running_jobs_as_pending() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    let mut queue = TranscriptionQueue::new();
    let first = queue.enqueue_file("/recordings/20260506/230709.wav", "whisper-medium");
    let second = queue.enqueue_file("/recordings/20260506/230810.wav", "whisper-small");
    queue.start_next().expect("running job");
    queue
        .mark_failed(second, "checksum mismatch")
        .expect("fail job");

    store.save_queue(&queue).expect("save queue");

    let loaded = store.load_queue().expect("load queue").expect("queue");
    assert_eq!(loaded.job(first).unwrap().status, QueueJobStatus::Pending);
    assert_eq!(loaded.job(second).unwrap().status, QueueJobStatus::Failed);

    let third = {
        let mut loaded = loaded;
        loaded.enqueue_file("/recordings/20260506/230911.wav", "whisper-medium")
    };
    assert_eq!(third, 3);
}

#[test]
fn persistence_removes_legacy_non_actionable_activity_outcomes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    std::fs::write(
        tmp.path().join("transcription-queue.json"),
        r#"{
          "jobs": [
            {"id": 1, "audio_path": "/recordings/empty.wav", "model_id": "openai-gpt-transcribe", "status": "failed", "error": "OpenAI returned an empty transcript", "is_read": false},
            {"id": 2, "audio_path": "/recordings/silent.wav", "model_id": "whisper-medium", "status": "failed", "error": "No speech detected", "is_read": true},
            {"id": 3, "audio_path": "/recordings/broken.wav", "model_id": "whisper-medium", "status": "failed", "error": "model missing", "is_read": false}
          ],
          "next_id": 3
        }"#,
    )
    .expect("legacy queue");

    let queue = store.load_queue().expect("load queue").expect("queue");
    assert_eq!(queue.job(1), None);
    assert_eq!(queue.job(2), None);

    let broken = queue.job(3).expect("broken job");
    assert_eq!(
        broken
            .issue
            .as_ref()
            .map(|issue| (issue.severity, issue.code)),
        Some((QueueIssueSeverity::Error, QueueIssueCode::Unknown)),
    );
    assert_eq!(broken.issue.as_ref().expect("issue").occurred_at, None);

    let persisted = std::fs::read_to_string(tmp.path().join("transcription-queue.json"))
        .expect("compacted queue");
    assert!(!persisted.contains("OpenAI returned an empty transcript"));
    assert!(!persisted.contains("No speech detected"));
}

#[test]
fn persistence_does_not_save_non_actionable_activity_outcomes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    let mut queue = TranscriptionQueue::new();
    let silent = queue.enqueue_file("/recordings/silent.wav", "whisper-medium");
    let failed = queue.enqueue_file("/recordings/failed.wav", "whisper-medium");
    queue
        .mark_failed_with_issue(
            silent,
            wakenote::queue::QueueJobIssue::warning(QueueIssueCode::NoSpeech, "No speech detected"),
        )
        .expect("no speech outcome");
    queue
        .mark_failed(failed, "model missing")
        .expect("actionable failure");

    store.save_queue(&queue).expect("save queue");

    let loaded = store.load_queue().expect("load queue").expect("queue");
    assert_eq!(loaded.job(silent), None);
    assert_eq!(
        loaded.job(failed).expect("actionable job").status,
        QueueJobStatus::Failed,
    );
}

#[test]
fn backend_persists_marking_all_activity_outcomes_read() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    let mut queue = TranscriptionQueue::new();
    let failed = queue.enqueue_file("/recordings/failed.wav", "whisper-medium");
    let cancelled = queue.enqueue_file("/recordings/cancelled.wav", "whisper-medium");
    let skipped = queue.enqueue_file("/recordings/skipped.wav", "whisper-medium");
    queue
        .mark_failed(failed, "model missing")
        .expect("fail job");
    assert_eq!(queue.start_next().expect("start job").id, cancelled);
    queue
        .cancel_current("cancelled by user")
        .expect("cancel job");
    queue.skip(skipped).expect("skip job");
    store.save_queue(&queue).expect("save unread outcomes");

    let mut backend = AppBackend::load_from_dir(tmp.path()).expect("load backend");
    let marked = backend.mark_all_activity_read();
    assert!(marked.jobs.iter().all(|job| job.is_read));
    drop(backend);

    let reloaded = AppBackend::load_from_dir(tmp.path()).expect("reload backend");
    assert!(reloaded.queue_snapshot().jobs.iter().all(|job| job.is_read));
}

#[test]
fn persistence_compacts_completed_queue_history_on_load() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path());
    let mut queue = TranscriptionQueue::new();
    let mut ids = Vec::new();
    for index in 0..102 {
        let id = queue.enqueue_file(
            format!("/recordings/20260506/{index:06}.wav"),
            "whisper-medium",
        );
        queue.mark_completed(id).expect("complete job");
        ids.push(id);
    }
    let pending = queue.enqueue_file("/recordings/20260506/pending.wav", "whisper-medium");
    std::fs::write(
        tmp.path().join("transcription-queue.json"),
        serde_json::to_vec_pretty(&queue).expect("queue json"),
    )
    .expect("raw queue json");

    let loaded = store.load_queue().expect("load queue").expect("queue");

    assert_eq!(loaded.job(ids[0]), None);
    assert_eq!(loaded.job(ids[1]), None);
    assert_eq!(
        loaded.job(ids[2]).unwrap().status,
        QueueJobStatus::Completed
    );
    assert_eq!(
        loaded.job(*ids.last().unwrap()).unwrap().status,
        QueueJobStatus::Completed
    );
    assert_eq!(loaded.job(pending).unwrap().status, QueueJobStatus::Pending);
    assert_eq!(loaded.snapshot().jobs.len(), 101);
}

#[test]
fn backend_loaded_from_dir_persists_settings_and_queue_mutations() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let audio_path = tmp
        .path()
        .join("recordings")
        .join("20260506")
        .join("230709.wav");
    std::fs::create_dir_all(audio_path.parent().unwrap()).expect("audio dir");
    std::fs::write(&audio_path, b"wav bytes").expect("audio");
    let mut backend = AppBackend::load_from_dir(tmp.path()).expect("load backend");

    backend.update_settings(SettingsPatch {
        transcription_enabled: Some(false),
        threshold_dbfs: Some(-42.0),
        ..SettingsPatch::default()
    });
    backend.enqueue_audio_file(&audio_path, None);

    let reloaded = AppBackend::load_from_dir(tmp.path()).expect("reload backend");
    assert!(!reloaded.settings().transcription_enabled);
    assert_eq!(reloaded.settings().threshold_dbfs, -42.0);
    assert_eq!(reloaded.queue_snapshot().pending_count, 1);
}

#[test]
fn backend_list_visibility_follows_the_current_save_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app_data = tmp.path().join("app-data");
    let first_root = tmp.path().join("first-save-root");
    let second_root = tmp.path().join("second-save-root");
    let mut backend = AppBackend::load_from_dir(&app_data).expect("load backend");
    backend.update_settings(SettingsPatch {
        save_root: Some(first_root.to_string_lossy().into_owned()),
        ..SettingsPatch::default()
    });

    backend
        .set_list_visibility(SetListVisibilityRequest {
            targets: vec![ListVisibilityTarget {
                kind: ListVisibilityKind::Meeting,
                id: "meeting-1".to_string(),
            }],
            hidden: true,
        })
        .expect("hide meeting");
    assert!(
        backend
            .load_list_visibility()
            .expect("first root visibility")
            .meetings
            .contains("meeting-1")
    );

    backend.update_settings(SettingsPatch {
        save_root: Some(second_root.to_string_lossy().into_owned()),
        ..SettingsPatch::default()
    });
    assert!(
        backend
            .load_list_visibility()
            .expect("second root visibility")
            .meetings
            .is_empty()
    );
}

fn visibility_target(kind: ListVisibilityKind, id: &str) -> ListVisibilityTarget {
    ListVisibilityTarget {
        kind,
        id: id.to_string(),
    }
}

#[test]
fn list_visibility_is_scoped_by_save_root_and_restorable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path().join("app-data"));
    let first_root = tmp.path().join("first-save-root");
    let second_root = tmp.path().join("second-save-root");
    let meeting = visibility_target(ListVisibilityKind::Meeting, "meeting-1");

    store
        .set_list_visibility(
            &first_root,
            &SetListVisibilityRequest {
                targets: vec![meeting.clone()],
                hidden: true,
            },
        )
        .expect("hide meeting");

    assert!(
        store
            .load_list_visibility(&first_root)
            .expect("first root visibility")
            .meetings
            .contains("meeting-1")
    );
    assert!(
        store
            .load_list_visibility(&second_root)
            .expect("second root visibility")
            .meetings
            .is_empty()
    );

    store
        .set_list_visibility(
            &first_root,
            &SetListVisibilityRequest {
                targets: vec![meeting],
                hidden: false,
            },
        )
        .expect("restore meeting");

    assert!(
        store
            .load_list_visibility(&first_root)
            .expect("restored visibility")
            .meetings
            .is_empty()
    );
}

#[test]
fn list_visibility_commits_mixed_targets_together_and_deduplicates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = AppPersistence::new(tmp.path().join("app-data"));
    let save_root = tmp.path().join("save-root");
    let meeting = visibility_target(ListVisibilityKind::Meeting, "meeting-1");
    let request = SetListVisibilityRequest {
        targets: vec![
            meeting.clone(),
            meeting,
            visibility_target(
                ListVisibilityKind::Transcript,
                "/save/2026-07-18/transcript.txt",
            ),
            visibility_target(ListVisibilityKind::ReportRun, "run-1"),
            visibility_target(ListVisibilityKind::LegacyReport, "report-1"),
        ],
        hidden: true,
    };

    let hidden = store
        .set_list_visibility(&save_root, &request)
        .expect("hide mixed targets");

    assert_eq!(hidden.meetings.len(), 1);
    assert!(hidden.meetings.contains("meeting-1"));
    assert!(
        hidden
            .transcripts
            .contains("/save/2026-07-18/transcript.txt")
    );
    assert!(hidden.report_runs.contains("run-1"));
    assert!(hidden.legacy_reports.contains("report-1"));

    let restored = store
        .set_list_visibility(
            &save_root,
            &SetListVisibilityRequest {
                targets: request.targets,
                hidden: false,
            },
        )
        .expect("restore mixed targets");

    assert_eq!(restored, Default::default());
}

#[test]
fn list_visibility_rejects_blank_targets_without_changing_registry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app_data = tmp.path().join("app-data");
    let store = AppPersistence::new(&app_data);
    let save_root = tmp.path().join("save-root");

    let error = store
        .set_list_visibility(
            &save_root,
            &SetListVisibilityRequest {
                targets: vec![
                    visibility_target(ListVisibilityKind::Meeting, "meeting-1"),
                    visibility_target(ListVisibilityKind::Transcript, "   "),
                ],
                hidden: true,
            },
        )
        .expect_err("blank target must fail");

    assert!(error.to_string().contains("cannot be blank"));
    assert!(!app_data.join("list-visibility.json").exists());
}

#[test]
fn list_visibility_normalizes_tilde_and_relative_segments() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let app_data = tmp.path().join("app-data");
    let store = AppPersistence::new(&app_data);

    store
        .set_list_visibility(
            std::path::Path::new("~/WakeNote/../WakeNote/recordings"),
            &SetListVisibilityRequest {
                targets: vec![visibility_target(ListVisibilityKind::Meeting, "meeting-1")],
                hidden: true,
            },
        )
        .expect("hide under tilde root");

    let registry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(app_data.join("list-visibility.json")).expect("visibility registry"),
    )
    .expect("registry json");
    let expected = home
        .join("WakeNote/recordings")
        .to_string_lossy()
        .into_owned();

    assert!(
        registry["roots"]
            .as_object()
            .expect("root map")
            .contains_key(&expected)
    );
}

#[test]
fn list_visibility_corrupt_registry_fails_open_without_rewriting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app_data = tmp.path().join("app-data");
    std::fs::create_dir_all(&app_data).expect("app data dir");
    let registry_path = app_data.join("list-visibility.json");
    let corrupt = b"{not-json";
    std::fs::write(&registry_path, corrupt).expect("corrupt registry");
    let store = AppPersistence::new(app_data);

    assert!(
        store
            .load_list_visibility(tmp.path().join("save-root").as_path())
            .is_err()
    );
    assert_eq!(
        std::fs::read(registry_path).expect("registry remains"),
        corrupt
    );
}

#[test]
fn list_visibility_rejects_unsupported_registry_version() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let app_data = tmp.path().join("app-data");
    std::fs::create_dir_all(&app_data).expect("app data dir");
    std::fs::write(
        app_data.join("list-visibility.json"),
        r#"{"version":2,"roots":{}}"#,
    )
    .expect("future registry");
    let store = AppPersistence::new(app_data);

    let error = store
        .load_list_visibility(tmp.path().join("save-root").as_path())
        .expect_err("future version must fail");

    assert!(
        error
            .to_string()
            .contains("unsupported list visibility version 2")
    );
}

#[cfg(unix)]
#[test]
fn persistence_files_are_private_after_creation_and_overwrite() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app-data");
    let store = AppPersistence::new(&root);
    for _ in 0..2 {
        store.save_settings(&AppSettings::default()).unwrap();
        store.save_openai_api_key("test-only").unwrap();
        store.save_openrouter_api_key("test-only").unwrap();
        store.save_soniox_api_key("test-only").unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in [
            "settings.json",
            "openai-secrets.json",
            "openrouter-secrets.json",
            "soniox-secrets.json",
        ] {
            let path = root.join(name);
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn backend_startup_restricts_legacy_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app-data");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    for name in [
        "openai-secrets.json",
        "openrouter-secrets.json",
        "soniox-secrets.json",
    ] {
        let path = root.join(name);
        std::fs::write(&path, r#"{"api_key":"test-only"}"#).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    AppBackend::load_from_dir(&root).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in [
        "openai-secrets.json",
        "openrouter-secrets.json",
        "soniox-secrets.json",
    ] {
        assert_eq!(
            std::fs::metadata(root.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn atomic_write_never_follows_a_predictable_temporary_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app-data");
    std::fs::create_dir(&root).unwrap();
    let unrelated = temp.path().join("unrelated.txt");
    std::fs::write(&unrelated, "untouched").unwrap();
    std::os::unix::fs::symlink(&unrelated, root.join("settings.tmp")).unwrap();
    AppPersistence::new(&root)
        .save_settings(&AppSettings::default())
        .unwrap();
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "untouched");
}

#[cfg(unix)]
#[test]
fn backend_rejects_a_symlinked_private_storage_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app-data");
    let target = temp.path().join("other");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(target, &root).unwrap();
    assert!(AppBackend::load_from_dir(root).is_err());
}
