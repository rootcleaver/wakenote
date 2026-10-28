//! Voice gate: keep only the enrolled speaker's chunks for auto-type.
//!
//! It answers one question per chunk — "is this my voice?" — by comparing a
//! speaker embedding of the chunk with the mean embedding of chunks the user
//! marked as their own. It never deletes audio or transcripts; a chunk from
//! another speaker is only labelled and skipped by auto-type.
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MODEL_FILE: &str = "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";
pub const MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx";
pub const MODEL_SHA256: &str = "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2";
pub const MODEL_BYTES: u64 = 28_281_164;
pub const PROFILE_FILE: &str = "voice-gate.json";
pub const DEFAULT_THRESHOLD: f32 = 0.45;
/// Chunks shorter than this carry too little voice to judge; they pass through.
pub const MIN_SAMPLES_16K: usize = 16_000;
const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceProfile {
    pub embedding: Vec<f32>,
    pub enrolled: u32,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerKind {
    Me,
    Other,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeakerVerdict {
    pub kind: SpeakerKind,
    pub score: Option<f32>,
    pub reason: Option<String>,
}

impl SpeakerVerdict {
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            kind: SpeakerKind::Unknown,
            score: None,
            reason: Some(reason.into()),
        }
    }

    pub fn is_other(&self) -> bool {
        self.kind == SpeakerKind::Other
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceGateStatus {
    pub model_ready: bool,
    pub runtime_available: bool,
    pub enrolled: u32,
}

fn normalized(values: &[f32]) -> Vec<f32> {
    let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return values.to_vec();
    }
    values.iter().map(|v| v / norm).collect()
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let ln = left.iter().map(|v| v * v).sum::<f32>().sqrt();
    let rn = right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if ln <= f32::EPSILON || rn <= f32::EPSILON {
        return 0.0;
    }
    dot / (ln * rn)
}

impl VoiceProfile {
    /// Running mean of unit-length embeddings, so every enrolled chunk weighs the same.
    pub fn add(&mut self, embedding: &[f32]) -> Result<(), String> {
        let unit = normalized(embedding);
        if self.enrolled == 0 || self.embedding.is_empty() {
            self.embedding = unit;
            self.enrolled = 1;
        } else {
            if self.embedding.len() != unit.len() {
                return Err("Speaker model changed; reset the voice enrollment first.".into());
            }
            let n = self.enrolled as f32;
            for (mean, value) in self.embedding.iter_mut().zip(unit) {
                *mean = (*mean * n + value) / (n + 1.0);
            }
            self.enrolled += 1;
        }
        self.updated_at = chrono::Utc::now().to_rfc3339();
        Ok(())
    }

    pub fn decide(&self, embedding: &[f32], threshold: f32) -> SpeakerVerdict {
        if self.enrolled == 0 || self.embedding.is_empty() {
            return SpeakerVerdict::unknown("no enrolled voice");
        }
        let score = cosine(&self.embedding, embedding);
        SpeakerVerdict {
            kind: if score >= threshold {
                SpeakerKind::Me
            } else {
                SpeakerKind::Other
            },
            score: Some(score),
            reason: None,
        }
    }
}

pub fn load_profile(app_data_dir: &Path) -> Option<VoiceProfile> {
    let text = fs::read_to_string(app_data_dir.join(PROFILE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_profile(app_data_dir: &Path, profile: &VoiceProfile) -> Result<(), String> {
    fs::create_dir_all(app_data_dir).map_err(|e| e.to_string())?;
    let path = app_data_dir.join(PROFILE_FILE);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(profile).map_err(|e| e.to_string())?;
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn reset_profile(app_data_dir: &Path) -> Result<(), String> {
    match fs::remove_file(app_data_dir.join(PROFILE_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn model_path(model_directory: &Path) -> PathBuf {
    model_directory.join("speaker").join(MODEL_FILE)
}

pub fn model_ready(model_directory: &Path) -> bool {
    fs::metadata(model_path(model_directory))
        .map(|m| m.len() == MODEL_BYTES)
        .unwrap_or(false)
}

pub fn runtime_available() -> bool {
    cfg!(feature = "asr-sherpa")
}

pub fn status(app_data_dir: &Path, model_directory: &Path) -> VoiceGateStatus {
    VoiceGateStatus {
        model_ready: model_ready(model_directory),
        runtime_available: runtime_available(),
        enrolled: load_profile(app_data_dir).map(|p| p.enrolled).unwrap_or(0),
    }
}

/// Download the speaker model into `<model_directory>/speaker/`, verifying size and SHA-256.
pub fn download_model(model_directory: &Path) -> Result<PathBuf, String> {
    let target = model_path(model_directory);
    if model_ready(model_directory) {
        return Ok(target);
    }
    let dir = target.parent().expect("speaker dir");
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = target.with_extension("onnx.part");
    let response = ureq::get(MODEL_URL)
        .call()
        .map_err(|e| format!("Speaker model download failed: {e}"))?;
    let mut reader = response.into_reader().take(MODEL_BYTES + 1);
    let mut file = fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    let digest = format!("{:x}", hasher.finalize());
    if total != MODEL_BYTES || digest != MODEL_SHA256 {
        let _ = fs::remove_file(&part);
        return Err("Speaker model download did not match the expected size or checksum.".into());
    }
    fs::rename(&part, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

#[cfg(feature = "asr-sherpa")]
mod runtime {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    type Cached = Option<(PathBuf, sherpa_rs::speaker_id::EmbeddingExtractor)>;
    static EXTRACTOR: OnceLock<Mutex<Cached>> = OnceLock::new();

    pub fn embedding(model: &Path, samples: Vec<f32>) -> Result<Vec<f32>, String> {
        let cell = EXTRACTOR.get_or_init(|| Mutex::new(None));
        let mut guard = cell.lock().map_err(|e| e.to_string())?;
        if guard.as_ref().map(|(path, _)| path != model).unwrap_or(true) {
            let extractor = sherpa_rs::speaker_id::EmbeddingExtractor::new(
                sherpa_rs::speaker_id::ExtractorConfig {
                    model: model.to_string_lossy().to_string(),
                    num_threads: Some(2),
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            *guard = Some((model.to_path_buf(), extractor));
        }
        let (_, extractor) = guard.as_mut().expect("extractor");
        extractor
            .compute_speaker_embedding(samples, SAMPLE_RATE)
            .map_err(|e| e.to_string())
    }
}

/// Speaker embedding of a recorded chunk (any format the transcriber can decode).
pub fn embedding_for_audio(model_directory: &Path, audio_path: &Path) -> Result<Vec<f32>, String> {
    if !runtime_available() {
        return Err("This build has no speaker runtime (asr-sherpa feature off).".into());
    }
    if !model_ready(model_directory) {
        return Err("Speaker model is not downloaded.".into());
    }
    let samples = crate::transcription::decode_audio_for_whisper(audio_path)
        .map_err(|e| e.to_string())?;
    if samples.len() < MIN_SAMPLES_16K {
        return Err("too short to judge".into());
    }
    #[cfg(feature = "asr-sherpa")]
    {
        runtime::embedding(&model_path(model_directory), samples)
    }
    #[cfg(not(feature = "asr-sherpa"))]
    {
        let _ = samples;
        Err("This build has no speaker runtime.".into())
    }
}

/// Judge one chunk. Any failure (no model, too short, decode error) yields `Unknown`,
/// which callers treat as "let it through" so the gate never silently eats speech.
pub fn judge(
    app_data_dir: &Path,
    model_directory: &Path,
    audio_path: &Path,
    threshold: f32,
) -> SpeakerVerdict {
    let Some(profile) = load_profile(app_data_dir) else {
        return SpeakerVerdict::unknown("no enrolled voice");
    };
    match embedding_for_audio(model_directory, audio_path) {
        Ok(embedding) => profile.decide(&embedding, threshold),
        Err(reason) => SpeakerVerdict::unknown(reason),
    }
}

/// Add a chunk the user marked as their own voice to the profile.
pub fn enroll(app_data_dir: &Path, model_directory: &Path, audio_path: &Path) -> Result<u32, String> {
    let embedding = embedding_for_audio(model_directory, audio_path)?;
    let mut profile = load_profile(app_data_dir).unwrap_or_default();
    profile.add(&embedding)?;
    save_profile(app_data_dir, &profile)?;
    Ok(profile.enrolled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn profile_mean_and_decision() {
        let mut profile = VoiceProfile::default();
        assert_eq!(profile.decide(&[1.0, 0.0], 0.5).kind, SpeakerKind::Unknown);
        profile.add(&[3.0, 0.0]).unwrap();
        profile.add(&[0.0, 5.0]).unwrap();
        assert_eq!(profile.enrolled, 2);
        assert!((profile.embedding[0] - 0.5).abs() < 1e-6);
        assert!((profile.embedding[1] - 0.5).abs() < 1e-6);
        let me = profile.decide(&[1.0, 1.0], 0.9);
        assert_eq!(me.kind, SpeakerKind::Me);
        let other = profile.decide(&[1.0, -1.0], 0.5);
        assert!(other.is_other());
        assert!(profile.add(&[1.0, 2.0, 3.0]).is_err());
    }

    #[test]
    fn profile_round_trip_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_profile(dir.path()).is_none());
        let mut profile = VoiceProfile::default();
        profile.add(&[0.6, 0.8]).unwrap();
        save_profile(dir.path(), &profile).unwrap();
        assert_eq!(load_profile(dir.path()).unwrap().enrolled, 1);
        assert_eq!(status(dir.path(), dir.path()).enrolled, 1);
        reset_profile(dir.path()).unwrap();
        reset_profile(dir.path()).unwrap();
        assert!(load_profile(dir.path()).is_none());
    }

    #[test]
    fn judge_without_profile_or_model_lets_speech_through() {
        let dir = tempfile::tempdir().unwrap();
        let verdict = judge(dir.path(), dir.path(), &dir.path().join("x.wav"), 0.45);
        assert_eq!(verdict.kind, SpeakerKind::Unknown);
        let mut profile = VoiceProfile::default();
        profile.add(&[1.0, 0.0]).unwrap();
        save_profile(dir.path(), &profile).unwrap();
        let verdict = judge(dir.path(), dir.path(), &dir.path().join("x.wav"), 0.45);
        assert_eq!(verdict.kind, SpeakerKind::Unknown);
        assert!(!model_ready(dir.path()));
    }
}
