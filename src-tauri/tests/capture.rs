use chrono::{TimeZone, Utc};
use wakenote::capture::{
    CaptureController, CaptureControllerConfig, CaptureControllerEvent, CaptureProcessor,
    CaptureProcessorConfig,
};
use wakenote::recorder::{ChunkMetadata, ChunkSource};
use wakenote::settings::{AppSettings, AudioFormat, MicrophoneSlot};

fn settings() -> AppSettings {
    AppSettings {
        audio_format: AudioFormat::Wav,
        transcription_enabled: false,
        threshold_dbfs: -45.0,
        attack_ms: 300,
        release_ms: 1_500,
        pre_roll_ms: 200,
        lead_in_padding_ms: 0,
        post_roll_ms: 200,
        min_chunk_ms: 500,
        max_chunk_ms: 30_000,
        ..AppSettings::default()
    }
}

#[test]
fn auto_type_bounds_microphone_chunks_and_snapshots_its_model_without_changing_archival_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let mut configured = settings();
    configured.auto_transcript_input_enabled = true;
    configured.auto_transcript_input_model = "whisper-small".into();
    configured.transcription_enabled = true;
    let config = |source| CaptureControllerConfig {
        save_root: tmp.path().to_path_buf(),
        settings: configured.clone(),
        sample_rate: 10,
        device_id: "mic".into(),
        device_name: "Mic".into(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 9, 6, 0, 0, 0).unwrap(),
        app_version: "test".into(),
        source,
        source_label: (source == ChunkSource::System).then(|| "system".into()),
    };
    let mut mic = CaptureController::new(config(ChunkSource::Microphone));
    let mut system = CaptureController::new(config(ChunkSource::System));
    // Continuous speech past the five-second soft limit is not cut mid-word...
    for _ in 0..65 {
        mic.process_samples(&[0.8], 100).unwrap();
        system.process_samples(&[0.8], 100).unwrap();
    }
    assert!(mic.completed_chunks().is_empty());
    // ...but the chunk is still bounded by the hard limit (soft limit + grace).
    for _ in 65..90 {
        mic.process_samples(&[0.8], 100).unwrap();
        system.process_samples(&[0.8], 100).unwrap();
    }
    assert!(!mic.completed_chunks().is_empty());
    assert!(system.completed_chunks().is_empty());
    let metadata: ChunkMetadata =
        serde_json::from_slice(&std::fs::read(&mic.completed_chunks()[0].metadata_path).unwrap())
            .unwrap();
    assert_eq!(metadata.model_id, "whisper-small");
    assert_eq!(metadata.max_chunk_ms, 5_000);
    assert_eq!(configured.max_chunk_ms, 30_000);
}

fn wav_sample_count(path: &std::path::Path) -> usize {
    let reader = hound::WavReader::open(path).expect("wav");
    reader.into_samples::<i16>().count()
}

fn wav_samples(path: &std::path::Path) -> Vec<i16> {
    let reader = hound::WavReader::open(path).expect("wav");
    reader
        .into_samples::<i16>()
        .map(|s| s.expect("sample"))
        .collect()
}

#[test]
fn capture_processor_rejects_spikes_shorter_than_attack() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: settings(),
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    processor.process_samples(&[0.0; 1], 100).expect("silence");
    processor.process_samples(&[0.9; 1], 100).expect("spike");
    processor.process_samples(&[0.0; 1], 100).expect("silence");
    processor.flush().expect("flush");

    assert!(processor.completed_chunks().is_empty());
}

#[test]
fn capture_processor_writes_chunk_after_attack_and_release() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base_time = Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap();
    let local = base_time.with_timezone(&chrono::Local);
    let dir = local.format("%Y%m%d").to_string();
    let stem = local.format("%y%m%d-%H%M%S").to_string();
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: settings(),
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time,
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..5 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }
    for _ in 0..18 {
        processor.process_samples(&[0.0; 1], 100).expect("silence");
    }

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].audio_path.ends_with(format!("{dir}/{stem}.wav")));
    assert!(chunks[0].metadata_path.exists());
    assert!(chunks[0].audio_path.exists());
}

#[test]
fn capture_processor_starts_after_exact_attack_window() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: settings(),
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..3 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }
    for _ in 0..17 {
        processor.process_samples(&[0.0; 1], 100).expect("silence");
    }

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 1);
    assert_eq!(wav_sample_count(&chunks[0].audio_path), 20);
}

#[test]
fn capture_processor_includes_pre_onset_audio_in_chunk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config_settings = settings();
    config_settings.attack_ms = 200;
    config_settings.pre_roll_ms = 200;
    config_settings.release_ms = 200;
    config_settings.min_chunk_ms = 0;
    config_settings.post_roll_ms = 0;
    config_settings.threshold_dbfs = -45.0;
    config_settings.audio_format = AudioFormat::Wav;
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: config_settings,
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..5 {
        processor.process_samples(&[0.0; 1], 100).expect("silence");
    }
    for _ in 0..4 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }
    for _ in 0..4 {
        processor
            .process_samples(&[0.0; 1], 100)
            .expect("trailing silence");
    }
    processor.flush().expect("flush");

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 1);
    let samples = wav_samples(&chunks[0].audio_path);
    assert!(samples.len() >= 2, "chunk too short: {}", samples.len());
    assert_eq!(samples[0], 0, "first sample should be pre-onset silence");
    assert_eq!(samples[1], 0, "second sample should be pre-onset silence");
}

#[test]
fn capture_processor_writes_lead_in_padding_before_chunk_audio() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config_settings = settings();
    config_settings.audio_format = AudioFormat::Wav;
    config_settings.attack_ms = 100;
    config_settings.release_ms = 200;
    config_settings.pre_roll_ms = 0;
    config_settings.lead_in_padding_ms = 300;
    config_settings.post_roll_ms = 0;
    config_settings.min_chunk_ms = 0;
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: config_settings,
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..2 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }
    for _ in 0..4 {
        processor.process_samples(&[0.0; 1], 100).expect("silence");
    }

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 1);
    let samples = wav_samples(&chunks[0].audio_path);
    assert_eq!(&samples[..3], &[0, 0, 0]);
    assert_ne!(samples[3], 0, "speech should follow lead-in padding");
}

#[test]
fn capture_processor_writes_configured_post_roll_audio_samples() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config_settings = settings();
    config_settings.attack_ms = 0;
    config_settings.release_ms = 200;
    config_settings.pre_roll_ms = 0;
    config_settings.post_roll_ms = 300;
    config_settings.min_chunk_ms = 0;
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: config_settings,
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..2 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }
    for _ in 0..6 {
        processor
            .process_samples(&[0.0; 1], 100)
            .expect("silence and post-roll");
    }

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 1);
    assert_eq!(wav_sample_count(&chunks[0].audio_path), 7);
}

#[test]
fn capture_processor_rolls_over_at_max_chunk_without_dropping_stream() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config_settings = settings();
    config_settings.attack_ms = 100;
    config_settings.max_chunk_ms = 500;
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: config_settings,
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..10 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 2);
    assert!(processor.is_recording());
}

#[test]
fn capture_processor_rollover_repeats_tail_context_for_next_chunk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config_settings = settings();
    config_settings.attack_ms = 100;
    config_settings.pre_roll_ms = 200;
    config_settings.max_chunk_ms = 500;
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: config_settings,
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for index in 0..10 {
        let sample = 0.2 + (index as f32 * 0.05);
        processor.process_samples(&[sample], 100).expect("speech");
    }

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 2);
    let first = wav_samples(&chunks[0].audio_path);
    let second = wav_samples(&chunks[1].audio_path);

    assert_eq!(first.len(), 5);
    assert_eq!(second.len(), 7);
    assert_eq!(
        &second[..2],
        &first[3..],
        "next chunk should begin with the previous chunk tail"
    );
}

#[test]
fn capture_processor_rollover_does_not_write_silence_only_chunk_when_speech_stops() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config_settings = settings();
    config_settings.attack_ms = 100;
    config_settings.release_ms = 200;
    config_settings.pre_roll_ms = 0;
    config_settings.post_roll_ms = 0;
    config_settings.min_chunk_ms = 0;
    config_settings.max_chunk_ms = 500;
    let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
        save_root: tmp.path().to_path_buf(),
        settings: config_settings,
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..5 {
        processor.process_samples(&[0.8; 1], 100).expect("speech");
    }
    for _ in 0..5 {
        processor.process_samples(&[0.0; 1], 100).expect("silence");
    }
    processor.flush().expect("flush");

    let chunks = processor.completed_chunks();
    assert_eq!(chunks.len(), 1);
    assert_eq!(wav_sample_count(&chunks[0].audio_path), 5);
    assert!(
        processor
            .pending_events()
            .iter()
            .any(|event| matches!(event, CaptureControllerEvent::ChunkDiscarded { .. })),
        "discarded silence must release pending realtime output"
    );
}

#[test]
fn capture_controller_flushes_active_chunk_when_recording_is_disabled() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut controller = CaptureController::new(CaptureControllerConfig {
        save_root: tmp.path().to_path_buf(),
        settings: settings(),
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..5 {
        controller.process_samples(&[0.8; 1], 100).expect("speech");
    }
    assert!(controller.is_recording());

    let mut disabled = settings();
    disabled.recording_enabled = false;
    let events = controller.update_settings(disabled).expect("disable");

    assert!(!controller.is_listening());
    assert!(!controller.is_recording());
    assert_eq!(completed_event_count(&events), 1);
    assert!(controller.completed_chunks()[0].audio_path.exists());
}

#[test]
fn capture_controller_flushes_active_chunk_when_paused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut controller = CaptureController::new(CaptureControllerConfig {
        save_root: tmp.path().to_path_buf(),
        settings: settings(),
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..5 {
        controller.process_samples(&[0.8; 1], 100).expect("speech");
    }

    let mut paused = settings();
    paused.pause_all = true;
    let events = controller.update_settings(paused).expect("pause");

    assert!(!controller.is_listening());
    assert_eq!(completed_event_count(&events), 1);
    assert!(controller.completed_chunks()[0].audio_path.exists());
}

#[test]
fn capture_controller_flushes_active_chunk_before_threshold_settings_change() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut controller = CaptureController::new(CaptureControllerConfig {
        save_root: tmp.path().to_path_buf(),
        settings: settings(),
        sample_rate: 10,
        device_id: "default".to_string(),
        device_name: "System Default".to_string(),
        used_fallback_device: false,
        base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
        app_version: "0.1.0".to_string(),
        source: ChunkSource::Microphone,
        source_label: None,
    });

    for _ in 0..5 {
        controller.process_samples(&[0.8; 1], 100).expect("speech");
    }
    assert!(controller.is_recording());

    let mut changed = settings();
    changed.threshold_dbfs = -35.0;
    let events = controller
        .update_settings(changed)
        .expect("change threshold");

    assert_eq!(completed_event_count(&events), 1);
    assert!(!controller.is_recording());
    assert!(controller.completed_chunks()[0].audio_path.exists());
}

#[test]
fn dual_microphone_chunks_have_distinct_paths_and_slot_metadata() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base_time = Utc.with_ymd_and_hms(2026, 5, 6, 12, 0, 0).unwrap();
    let mut capture_settings = settings();
    capture_settings.attack_ms = 0;
    capture_settings.min_chunk_ms = 0;

    let make_processor = |device_id: &str, device_name: &str, source_label: &str| {
        CaptureProcessor::new(CaptureProcessorConfig {
            save_root: tmp.path().to_path_buf(),
            settings: capture_settings.clone(),
            sample_rate: 10,
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            used_fallback_device: false,
            base_time,
            app_version: "0.1.0".to_string(),
            source: ChunkSource::Microphone,
            source_label: Some(source_label.to_string()),
        })
    };
    let mut primary = make_processor("input-1-wired", "Wired", "mic-primary-wired");
    let mut secondary = make_processor("input-2-wireless", "Wireless", "mic-secondary-wireless");

    primary
        .process_samples(&[0.8], 100)
        .expect("primary speech");
    secondary
        .process_samples(&[0.8], 100)
        .expect("secondary speech");
    primary.flush().expect("primary flush");
    secondary.flush().expect("secondary flush");

    let primary_chunk = &primary.completed_chunks()[0];
    let secondary_chunk = &secondary.completed_chunks()[0];
    assert_ne!(primary_chunk.audio_path, secondary_chunk.audio_path);
    assert!(
        primary_chunk
            .audio_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("mic-primary-wired")
    );
    assert!(
        secondary_chunk
            .audio_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("mic-secondary-wireless")
    );

    let primary_metadata: ChunkMetadata = serde_json::from_slice(
        &std::fs::read(&primary_chunk.metadata_path).expect("primary metadata"),
    )
    .expect("parse primary metadata");
    let secondary_metadata: ChunkMetadata = serde_json::from_slice(
        &std::fs::read(&secondary_chunk.metadata_path).expect("secondary metadata"),
    )
    .expect("parse secondary metadata");
    assert_eq!(
        primary_metadata.microphone_slot,
        Some(MicrophoneSlot::Primary)
    );
    assert_eq!(
        secondary_metadata.microphone_slot,
        Some(MicrophoneSlot::Secondary)
    );
}

fn completed_event_count(events: &[CaptureControllerEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, CaptureControllerEvent::ChunkCompleted { .. }))
        .count()
}
