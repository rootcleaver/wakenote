use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Duration, Utc};

use crate::audio::{GateConfig, GateDecision, SpeechGate, dbfs_from_samples};
use crate::recorder::{ChunkSource, RecordedChunk, Recorder, RecorderError, RecordingRequest};
use crate::settings::AppSettings;
use crate::soniox_realtime::SONIOX_REALTIME_MODEL_ID;

/// How often the live transcription tap fires while a chunk is recording.
/// Smaller values = snappier preview, larger values = lighter CPU. The live
/// transcription worker is single-flight (latest pending request replaces
/// any older one), so a faster cadence never stacks queued work — the worker
/// just drops stale frames when it can't keep up.
const LIVE_PARTIAL_INTERVAL_MS: u64 = 750;

/// Don't emit a partial until the active buffer has this much audio. Whisper
/// returns nothing below ~1 s, so submitting earlier wastes a decode cycle
/// the worker could spend on the next, longer slice.
const LIVE_PARTIAL_MIN_DURATION_MS: u64 = 1_000;

/// Committed chunks reach their consumers through `ChunkCompleted` events, so
/// keep only a few recent ones for inspection instead of every chunk of an
/// always-on capture session.
const RECENT_COMPLETED_CHUNK_LIMIT: usize = 16;
// Unique across devices and stream restarts so late ASR results cannot target a new utterance.
static NEXT_CHUNK_ID: OnceLock<AtomicU64> = OnceLock::new();

fn next_chunk_id() -> u64 {
    NEXT_CHUNK_ID
        .get_or_init(|| AtomicU64::new(Utc::now().timestamp_micros().unsigned_abs()))
        .fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct CaptureProcessorConfig {
    pub save_root: PathBuf,
    pub settings: AppSettings,
    pub sample_rate: u32,
    pub device_id: String,
    pub device_name: String,
    pub used_fallback_device: bool,
    pub base_time: DateTime<Utc>,
    pub app_version: String,
    /// Audio source these chunks are tagged with (mic vs. system audio).
    pub source: ChunkSource,
    /// Optional slug (e.g. app name) that suffixes the chunk filename so a
    /// system chunk never collides with a mic chunk from the same second.
    pub source_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CaptureControllerConfig {
    pub save_root: PathBuf,
    pub settings: AppSettings,
    pub sample_rate: u32,
    pub device_id: String,
    pub device_name: String,
    pub used_fallback_device: bool,
    pub base_time: DateTime<Utc>,
    pub app_version: String,
    /// Audio source these chunks are tagged with (mic vs. system audio).
    pub source: ChunkSource,
    /// Optional slug (e.g. app name) that suffixes the chunk filename so a
    /// system chunk never collides with a mic chunk from the same second.
    pub source_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureControllerEvent {
    ChunkStarted {
        chunk_id: u64,
        started_at: DateTime<Utc>,
    },
    LiveSamplesReady {
        chunk_id: u64,
        sample_rate: u32,
        samples: Arc<Vec<f32>>,
    },
    ChunkCompleted {
        chunk_id: u64,
        chunk: RecordedChunk,
    },
    ChunkDiscarded {
        chunk_id: u64,
    },
}

#[derive(Debug)]
pub struct CaptureController {
    config: CaptureControllerConfig,
    processor: CaptureProcessor,
}

#[derive(Debug, Clone)]
struct BufferedFrame {
    samples: Vec<f32>,
    duration_ms: u64,
}

#[derive(Debug)]
pub struct CaptureProcessor {
    config: CaptureProcessorConfig,
    gate: SpeechGate,
    elapsed_ms: u64,
    pre_roll: VecDeque<BufferedFrame>,
    active_samples: Vec<f32>,
    active_started_at_ms: Option<u64>,
    active_has_signal: bool,
    current_chunk_id: Option<u64>,
    last_partial_emit_offset_ms: u64,
    completed_chunks: Vec<RecordedChunk>,
    pending_events: Vec<CaptureControllerEvent>,
}

impl CaptureController {
    pub fn new(config: CaptureControllerConfig) -> Self {
        let processor = CaptureProcessor::new(processor_config(&config));
        Self { config, processor }
    }

    pub fn process_samples(
        &mut self,
        samples: &[f32],
        duration_ms: u64,
    ) -> Result<Vec<CaptureControllerEvent>, RecorderError> {
        if !self.is_listening() {
            return Ok(Vec::new());
        }

        self.processor.process_samples(samples, duration_ms)?;
        Ok(self.drain_new_events())
    }

    pub fn process_samples_at(
        &mut self,
        samples: &[f32],
        duration_ms: u64,
        captured_at: DateTime<Utc>,
    ) -> Result<Vec<CaptureControllerEvent>, RecorderError> {
        if !self.is_listening() {
            return Ok(Vec::new());
        }

        self.processor
            .process_samples_at(samples, duration_ms, captured_at)?;
        Ok(self.drain_new_events())
    }

    pub fn update_settings(
        &mut self,
        settings: AppSettings,
    ) -> Result<Vec<CaptureControllerEvent>, RecorderError> {
        let was_recording = self.processor.is_recording();
        let should_stop = settings.pause_all || !settings.recording_enabled;
        let mut events = Vec::new();

        if was_recording {
            self.processor.flush()?;
            events.extend(self.drain_new_events());
        }

        self.config.settings = settings;
        if should_stop {
            return Ok(events);
        }

        self.processor.reconfigure(processor_config(&self.config));
        Ok(events)
    }

    pub fn flush(&mut self) -> Result<Vec<CaptureControllerEvent>, RecorderError> {
        self.processor.flush()?;
        Ok(self.drain_new_events())
    }

    pub fn is_listening(&self) -> bool {
        self.config.settings.recording_enabled && !self.config.settings.pause_all
    }

    pub fn is_recording(&self) -> bool {
        self.is_listening() && self.processor.is_recording()
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn completed_chunks(&self) -> &[RecordedChunk] {
        self.processor.completed_chunks()
    }

    pub fn active_chunk_id(&self) -> Option<u64> {
        self.processor.current_chunk_id
    }

    fn drain_new_events(&mut self) -> Vec<CaptureControllerEvent> {
        self.processor.take_pending_events()
    }
}

impl CaptureProcessor {
    pub fn new(config: CaptureProcessorConfig) -> Self {
        let gate = SpeechGate::new(GateConfig {
            threshold_dbfs: config.settings.threshold_dbfs,
            attack_ms: config.settings.attack_ms,
            release_ms: config.settings.release_ms,
            pre_roll_ms: config.settings.pre_roll_ms,
            post_roll_ms: config.settings.post_roll_ms,
            min_chunk_ms: config.settings.min_chunk_ms,
            max_chunk_ms: config.settings.max_chunk_ms,
            rollover_grace_ms: rollover_grace_ms(&config),
        });

        Self {
            config,
            gate,
            elapsed_ms: 0,
            pre_roll: VecDeque::new(),
            active_samples: Vec::new(),
            active_started_at_ms: None,
            active_has_signal: false,
            current_chunk_id: None,
            last_partial_emit_offset_ms: 0,
            completed_chunks: Vec::new(),
            pending_events: Vec::new(),
        }
    }

    pub fn process_samples(
        &mut self,
        samples: &[f32],
        duration_ms: u64,
    ) -> Result<(), RecorderError> {
        let frame_start_ms = self.elapsed_ms;
        let frame_end_ms = frame_start_ms.saturating_add(duration_ms);
        self.process_samples_window(samples, duration_ms, frame_start_ms, frame_end_ms)
    }

    pub fn process_samples_at(
        &mut self,
        samples: &[f32],
        duration_ms: u64,
        captured_at: DateTime<Utc>,
    ) -> Result<(), RecorderError> {
        let observed_end_ms = offset_from_base_ms(self.config.base_time, captured_at);
        // `captured_at` is the end timestamp for this frame. Use it as the
        // canonical timeline so real wall-clock gaps before speech are preserved,
        // while still keeping the timeline monotonic if a frame arrives early.
        let frame_end_ms = observed_end_ms.max(self.elapsed_ms);
        let frame_start_ms = frame_end_ms.saturating_sub(duration_ms);
        self.process_samples_window(samples, duration_ms, frame_start_ms, frame_end_ms)
    }

    fn process_samples_window(
        &mut self,
        samples: &[f32],
        duration_ms: u64,
        frame_start_ms: u64,
        frame_end_ms: u64,
    ) -> Result<(), RecorderError> {
        if self.config.settings.pause_all || !self.config.settings.recording_enabled {
            self.elapsed_ms = self.elapsed_ms.max(frame_end_ms);
            return Ok(());
        }

        let adjusted_samples = input_samples_for_source(samples, self.config.source);
        let frame = BufferedFrame {
            samples: adjusted_samples,
            duration_ms,
        };
        let dbfs = dbfs_from_samples(&frame.samples);
        let above_threshold = dbfs >= self.config.settings.threshold_dbfs;
        let decision = self.gate.observe_frame(dbfs, frame_start_ms, frame_end_ms);

        match decision {
            GateDecision::Idle => {
                self.push_pre_roll(frame);
            }
            GateDecision::Start { started_at_ms } => {
                let chunk_id = next_chunk_id();
                self.current_chunk_id = Some(chunk_id);
                self.active_started_at_ms = Some(started_at_ms);
                self.active_samples.clear();
                self.active_has_signal = true;
                self.last_partial_emit_offset_ms = 0;
                self.drain_pre_roll_into_active(started_at_ms);
                self.active_samples.extend_from_slice(&frame.samples);

                self.pending_events
                    .push(CaptureControllerEvent::ChunkStarted {
                        chunk_id,
                        started_at: self.time_at(started_at_ms),
                    });
                self.maybe_emit_live_partial();
            }
            GateDecision::Recording => {
                if self.gate.is_recording() {
                    self.active_has_signal |= above_threshold;
                    self.active_samples.extend_from_slice(&frame.samples);
                    self.maybe_emit_live_partial();
                } else {
                    self.push_pre_roll(frame);
                }
            }
            GateDecision::End { ended_at_ms } => {
                self.active_samples.extend_from_slice(&frame.samples);
                self.commit_active_chunk(ended_at_ms)?;
                self.push_pre_roll(frame);
            }
            GateDecision::Rollover {
                ended_at_ms,
                next_started_at_ms,
            } => {
                self.active_samples.extend_from_slice(&frame.samples);
                let overlap_samples = self.rollover_overlap_samples();
                let overlap_started_at_ms = next_started_at_ms.saturating_sub(active_duration_ms(
                    overlap_samples.len(),
                    self.config.sample_rate,
                ));
                self.commit_active_chunk(ended_at_ms)?;
                let chunk_id = next_chunk_id();
                self.current_chunk_id = Some(chunk_id);
                self.active_started_at_ms = Some(overlap_started_at_ms);
                self.active_samples = overlap_samples;
                self.active_has_signal = false;
                self.last_partial_emit_offset_ms = 0;

                self.pending_events
                    .push(CaptureControllerEvent::ChunkStarted {
                        chunk_id,
                        started_at: self.time_at(overlap_started_at_ms),
                    });
            }
        }

        self.elapsed_ms = frame_end_ms;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), RecorderError> {
        if self.gate.is_recording() && !self.active_samples.is_empty() {
            self.commit_active_chunk(self.elapsed_ms)?;
        }
        Ok(())
    }

    pub fn completed_chunks(&self) -> &[RecordedChunk] {
        &self.completed_chunks
    }

    pub fn pending_events(&self) -> &[CaptureControllerEvent] {
        &self.pending_events
    }

    pub fn take_pending_events(&mut self) -> Vec<CaptureControllerEvent> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn is_recording(&self) -> bool {
        self.gate.is_recording()
    }

    fn reconfigure(&mut self, config: CaptureProcessorConfig) {
        self.config = config;
        self.gate = SpeechGate::new(GateConfig {
            threshold_dbfs: self.config.settings.threshold_dbfs,
            attack_ms: self.config.settings.attack_ms,
            release_ms: self.config.settings.release_ms,
            pre_roll_ms: self.config.settings.pre_roll_ms,
            post_roll_ms: self.config.settings.post_roll_ms,
            min_chunk_ms: self.config.settings.min_chunk_ms,
            max_chunk_ms: self.config.settings.max_chunk_ms,
            rollover_grace_ms: rollover_grace_ms(&self.config),
        });
        self.pre_roll.clear();
        self.active_samples.clear();
        self.active_started_at_ms = None;
        self.active_has_signal = false;
        self.current_chunk_id = None;
        self.last_partial_emit_offset_ms = 0;
    }

    fn push_pre_roll(&mut self, frame: BufferedFrame) {
        self.pre_roll.push_back(frame);
        let mut total_ms: u64 = self.pre_roll.iter().map(|frame| frame.duration_ms).sum();
        // Cap includes attack window so pre-onset frames aren't evicted by Idle frames pushed during attack.
        let cap_ms = self
            .config
            .settings
            .pre_roll_ms
            .saturating_add(self.config.settings.attack_ms);
        while total_ms > cap_ms {
            let Some(removed) = self.pre_roll.pop_front() else {
                break;
            };
            total_ms = total_ms.saturating_sub(removed.duration_ms);
        }
    }

    fn drain_pre_roll_into_active(&mut self, _started_at_ms: u64) {
        while let Some(frame) = self.pre_roll.pop_front() {
            self.active_samples.extend_from_slice(&frame.samples);
        }
    }

    fn rollover_overlap_samples(&self) -> Vec<f32> {
        if auto_type_shapes_chunks(&self.config.settings, self.config.source) {
            return Vec::new();
        }
        let overlap_count =
            sample_count_for_duration_ms(self.config.settings.pre_roll_ms, self.config.sample_rate);
        if overlap_count == 0 || self.active_samples.is_empty() {
            return Vec::new();
        }

        let keep_count = overlap_count.min(self.active_samples.len());
        self.active_samples[self.active_samples.len() - keep_count..].to_vec()
    }

    fn maybe_emit_live_partial(&mut self) {
        let Some(chunk_id) = self.current_chunk_id else {
            return;
        };
        let active_duration_ms =
            active_duration_ms(self.active_samples.len(), self.config.sample_rate);
        if active_duration_ms < LIVE_PARTIAL_MIN_DURATION_MS {
            return;
        }
        if active_duration_ms.saturating_sub(self.last_partial_emit_offset_ms)
            < LIVE_PARTIAL_INTERVAL_MS
        {
            return;
        }
        self.last_partial_emit_offset_ms = active_duration_ms;
        self.pending_events
            .push(CaptureControllerEvent::LiveSamplesReady {
                chunk_id,
                sample_rate: self.config.sample_rate,
                samples: Arc::new(self.active_samples.clone()),
            });
    }

    fn commit_active_chunk(&mut self, ended_at_ms: u64) -> Result<(), RecorderError> {
        let started_at_ms = self.active_started_at_ms.unwrap_or(self.elapsed_ms);
        let chunk_id = self.current_chunk_id;
        if self.active_samples.is_empty() || !self.active_has_signal {
            if let Some(chunk_id) = chunk_id {
                self.pending_events
                    .push(CaptureControllerEvent::ChunkDiscarded { chunk_id });
            }
            self.active_samples.clear();
            self.active_started_at_ms = None;
            self.active_has_signal = false;
            self.current_chunk_id = None;
            self.last_partial_emit_offset_ms = 0;
            return Ok(());
        }

        let chunk = Recorder::write_chunk(RecordingRequest {
            save_root: &self.config.save_root,
            settings: &self.config.settings,
            samples: &self.active_samples,
            sample_rate: self.config.sample_rate,
            started_at: self.time_at(started_at_ms),
            ended_at: self.time_at(ended_at_ms),
            device_id: &self.config.device_id,
            device_name: &self.config.device_name,
            used_fallback_device: self.config.used_fallback_device,
            transcription_enabled: self.config.settings.transcription_enabled,
            app_version: &self.config.app_version,
            live_capture_chunk_id: chunk_id,
            source: self.config.source,
            source_label: self.config.source_label.as_deref(),
            microphone_inputs: None,
        })?;

        if self.completed_chunks.len() == RECENT_COMPLETED_CHUNK_LIMIT {
            self.completed_chunks.remove(0);
        }
        self.completed_chunks.push(chunk.clone());
        if let Some(chunk_id) = chunk_id {
            let final_duration_ms =
                active_duration_ms(self.active_samples.len(), self.config.sample_rate);
            if final_duration_ms > self.last_partial_emit_offset_ms {
                self.pending_events
                    .push(CaptureControllerEvent::LiveSamplesReady {
                        chunk_id,
                        sample_rate: self.config.sample_rate,
                        samples: Arc::new(self.active_samples.clone()),
                    });
            }
            self.pending_events
                .push(CaptureControllerEvent::ChunkCompleted { chunk_id, chunk });
        }
        self.active_samples.clear();
        self.active_started_at_ms = None;
        self.active_has_signal = false;
        self.current_chunk_id = None;
        self.last_partial_emit_offset_ms = 0;
        Ok(())
    }

    fn time_at(&self, offset_ms: u64) -> DateTime<Utc> {
        self.config.base_time + Duration::milliseconds(offset_ms as i64)
    }
}

fn active_duration_ms(sample_count: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    ((sample_count as u128 * 1_000) / sample_rate as u128) as u64
}

fn sample_count_for_duration_ms(duration_ms: u64, sample_rate: u32) -> usize {
    if duration_ms == 0 || sample_rate == 0 {
        return 0;
    }
    ((duration_ms as u128 * sample_rate as u128) / 1_000) as usize
}

fn input_samples_for_source(samples: &[f32], _source: ChunkSource) -> Vec<f32> {
    samples.to_vec()
}

fn offset_from_base_ms(base_time: DateTime<Utc>, captured_at: DateTime<Utc>) -> u64 {
    captured_at
        .signed_duration_since(base_time)
        .num_milliseconds()
        .try_into()
        .unwrap_or(0)
}

/// Auto-type microphone chunks are capped at five seconds; give a phrase up to
/// three more seconds to reach a pause so the cut does not split a word. The
/// hard limit (8 s from the chunk start, pre-roll included) stays inside the ten-second
/// live decoder window.
const AUTO_TYPE_ROLLOVER_GRACE_MS: u64 = 3_000;

/// Auto-type reshapes microphone chunks only for models that type from partial
/// agreement. Soniox real-time confirms words as they arrive, so with it the
/// chunks follow the same rules as with auto-type off.
fn auto_type_shapes_chunks(settings: &AppSettings, source: ChunkSource) -> bool {
    settings.auto_transcript_input_enabled
        && source == ChunkSource::Microphone
        && settings.effective_auto_type_model() != SONIOX_REALTIME_MODEL_ID
}

fn rollover_grace_ms(config: &CaptureProcessorConfig) -> u64 {
    if auto_type_shapes_chunks(&config.settings, config.source) {
        AUTO_TYPE_ROLLOVER_GRACE_MS
    } else {
        0
    }
}

fn processor_config(config: &CaptureControllerConfig) -> CaptureProcessorConfig {
    let mut settings = config.settings.clone();
    if auto_type_shapes_chunks(&settings, config.source) {
        // Keep each hypothesis inside the ten-second live decoder window, even
        // when the archival chunk setting is several minutes. Settings on disk stay unchanged.
        settings.max_chunk_ms = settings.max_chunk_ms.min(5_000);
        settings.min_chunk_ms = settings.min_chunk_ms.min(1_000);
        settings.release_ms = settings.release_ms.min(750);
    }
    if settings.auto_transcript_input_enabled
        && config.source == ChunkSource::Microphone
        && !settings.auto_transcript_input_model.is_empty()
    {
        settings.selected_model = settings.auto_transcript_input_model.clone();
    }
    CaptureProcessorConfig {
        save_root: config.save_root.clone(),
        settings,
        sample_rate: config.sample_rate,
        device_id: config.device_id.clone(),
        device_name: config.device_name.clone(),
        used_fallback_device: config.used_fallback_device,
        base_time: config.base_time,
        app_version: config.app_version.clone(),
        source: config.source,
        source_label: config.source_label.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::ChunkMetadata;
    use crate::settings::{AppSettings, AudioFormat};
    use chrono::TimeZone;

    fn settings() -> AppSettings {
        AppSettings {
            audio_format: AudioFormat::Wav,
            transcription_enabled: false,
            threshold_dbfs: -45.0,
            attack_ms: 300,
            release_ms: 1_500,
            pre_roll_ms: 200,
            post_roll_ms: 200,
            min_chunk_ms: 500,
            max_chunk_ms: 30_000,
            ..AppSettings::default()
        }
    }

    #[test]
    fn system_source_controller_writes_chunk_with_system_source_and_slug() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_time = Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap();
        let stem = base_time
            .with_timezone(&chrono::Local)
            .format("%y%m%d-%H%M%S")
            .to_string();
        let mut controller = CaptureController::new(CaptureControllerConfig {
            save_root: tmp.path().to_path_buf(),
            settings: settings(),
            sample_rate: 10,
            device_id: "google-meet".to_string(),
            device_name: "Google Meet".to_string(),
            used_fallback_device: false,
            base_time,
            app_version: "0.1.0".to_string(),
            source: ChunkSource::System,
            source_label: Some("meet".to_string()),
        });

        for _ in 0..5 {
            controller.process_samples(&[0.8; 1], 100).expect("speech");
        }
        controller.flush().expect("flush");

        let chunks = controller.completed_chunks();
        assert_eq!(chunks.len(), 1);
        assert!(
            chunks[0].audio_path.ends_with(format!("{stem}-meet.wav")),
            "audio path should carry the source slug: {}",
            chunks[0].audio_path.display()
        );

        let metadata: ChunkMetadata =
            serde_json::from_str(&std::fs::read_to_string(&chunks[0].metadata_path).expect("meta"))
                .expect("parse metadata");
        assert_eq!(metadata.source, ChunkSource::System);
        assert_eq!(metadata.source_label.as_deref(), Some("meet"));
        assert_eq!(metadata.device_name, "Google Meet");
    }

    #[test]
    fn process_samples_at_never_stamps_a_chunk_in_the_future() {
        // Synthetic audio-sample clock runs 10x ahead of wall-clock: every frame
        // claims 1000ms of audio but only 100ms of real time elapses between
        // deliveries (the system-audio over-delivery case). The committed chunk's
        // started_at/ended_at must stay anchored to wall-clock (captured_at) and
        // never exceed the latest observed time — i.e. never land in the future.
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_time = Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap();
        let mut controller = CaptureController::new(CaptureControllerConfig {
            save_root: tmp.path().to_path_buf(),
            settings: AppSettings {
                attack_ms: 0,
                release_ms: 600_000,
                pre_roll_ms: 0,
                post_roll_ms: 0,
                min_chunk_ms: 0,
                max_chunk_ms: 600_000,
                ..settings()
            },
            sample_rate: 16_000,
            device_id: "mic".to_string(),
            device_name: "Mic".to_string(),
            used_fallback_device: false,
            base_time,
            app_version: "0.1.0".to_string(),
            source: ChunkSource::Microphone,
            source_label: None,
        });

        let mut last_captured_at = base_time;
        for index in 1..=6i64 {
            let captured_at = base_time + Duration::milliseconds(index * 100);
            last_captured_at = captured_at;
            controller
                .process_samples_at(&[0.8; 1], 1_000, captured_at)
                .expect("frame");
        }
        controller.flush().expect("flush");

        let chunks = controller.completed_chunks();
        assert_eq!(chunks.len(), 1, "expected exactly one committed chunk");
        let metadata: ChunkMetadata =
            serde_json::from_str(&std::fs::read_to_string(&chunks[0].metadata_path).expect("meta"))
                .expect("parse metadata");

        assert!(
            metadata.ended_at <= last_captured_at,
            "ended_at {} must not exceed the latest wall-clock {last_captured_at}",
            metadata.ended_at
        );
        assert!(
            metadata.started_at <= last_captured_at,
            "started_at {} must not exceed the latest wall-clock {last_captured_at}",
            metadata.started_at
        );
    }

    #[test]
    fn rollover_grace_only_applies_to_auto_type_microphone_chunks() {
        let config = |auto_type: bool, source: ChunkSource| CaptureProcessorConfig {
            save_root: std::path::PathBuf::new(),
            settings: AppSettings {
                auto_transcript_input_enabled: auto_type,
                ..settings()
            },
            sample_rate: 10,
            device_id: "mic".to_string(),
            device_name: "Mic".to_string(),
            used_fallback_device: false,
            base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
            app_version: "0.1.0".to_string(),
            source,
            source_label: None,
        };
        assert_eq!(
            rollover_grace_ms(&config(true, ChunkSource::Microphone)),
            AUTO_TYPE_ROLLOVER_GRACE_MS
        );
        assert_eq!(rollover_grace_ms(&config(false, ChunkSource::Microphone)), 0);
        assert_eq!(rollover_grace_ms(&config(true, ChunkSource::System)), 0);
        let mut soniox = config(true, ChunkSource::Microphone);
        soniox.settings.auto_transcript_input_model = SONIOX_REALTIME_MODEL_ID.into();
        assert_eq!(rollover_grace_ms(&soniox), 0);
    }

    #[test]
    fn completed_chunk_history_keeps_only_recent_chunks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut processor = CaptureProcessor::new(CaptureProcessorConfig {
            save_root: tmp.path().to_path_buf(),
            settings: AppSettings {
                attack_ms: 100,
                max_chunk_ms: 500,
                ..settings()
            },
            sample_rate: 10,
            device_id: "mic".to_string(),
            device_name: "Mic".to_string(),
            used_fallback_device: false,
            base_time: Utc.with_ymd_and_hms(2026, 5, 6, 23, 7, 9).unwrap(),
            app_version: "0.1.0".to_string(),
            source: ChunkSource::Microphone,
            source_label: None,
        });

        // Each 500 ms rollover commits one chunk.
        for _ in 0..(RECENT_COMPLETED_CHUNK_LIMIT + 4) * 5 {
            processor.process_samples(&[0.8; 1], 100).expect("speech");
        }

        let committed: Vec<_> = processor
            .take_pending_events()
            .into_iter()
            .filter_map(|event| match event {
                CaptureControllerEvent::ChunkCompleted { chunk, .. } => Some(chunk.audio_path),
                _ => None,
            })
            .collect();
        assert!(committed.len() > RECENT_COMPLETED_CHUNK_LIMIT);
        let retained: Vec<_> = processor
            .completed_chunks()
            .iter()
            .map(|chunk| chunk.audio_path.clone())
            .collect();
        assert_eq!(
            retained,
            committed[committed.len() - RECENT_COMPLETED_CHUNK_LIMIT..]
        );
    }

    #[test]
    fn microphone_samples_reach_capture_without_a_second_gain_stage() {
        let input = [-0.75, -0.25, 0.25, 0.75];
        assert_eq!(
            input_samples_for_source(&input, ChunkSource::Microphone),
            input
        );
    }

    #[test]
    fn system_audio_samples_remain_unchanged() {
        let input = [-0.75, 0.75];
        assert_eq!(input_samples_for_source(&input, ChunkSource::System), input);
    }
}
