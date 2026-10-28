use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cloud_transcription::{FailureCategory, TranscriptionCredentials, TranscriptionFailure};
use crate::dictionary::DictionaryContext;
use crate::settings::{MicrophoneSlot, TranscriptionLanguage};
use crate::transcription::TranscriptionExecution;

const RESULT_WAIT_TIMEOUT: Duration = Duration::from_secs(75);
const RESULT_CANCEL_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeStoredResult {
    Completed(TranscriptionExecution),
    Failed(TranscriptionFailure),
}

#[derive(Clone, Default)]
pub struct RealtimeResultStore {
    inner: Arc<(Mutex<HashMap<PathBuf, RealtimeStoredResult>>, Condvar)>,
}

impl RealtimeResultStore {
    pub fn publish(&self, audio_path: PathBuf, result: RealtimeStoredResult) {
        let (lock, ready) = &*self.inner;
        if let Ok(mut results) = lock.lock() {
            results.insert(audio_path, result);
            ready.notify_all();
        }
    }

    pub fn wait(&self, audio_path: &Path) -> RealtimeStoredResult {
        let (lock, ready) = &*self.inner;
        let Ok(results) = lock.lock() else {
            return RealtimeStoredResult::Failed(result_store_failure(
                FailureCategory::Transport,
                "Live transcription result store is unavailable",
            ));
        };
        let Ok((mut results, timeout)) =
            ready.wait_timeout_while(results, RESULT_WAIT_TIMEOUT, |results| {
                !results.contains_key(audio_path)
            })
        else {
            return RealtimeStoredResult::Failed(result_store_failure(
                FailureCategory::Transport,
                "Live transcription result wait failed",
            ));
        };
        if timeout.timed_out() {
            return RealtimeStoredResult::Failed(result_store_failure(
                FailureCategory::Transport,
                "Live transcription timed out",
            ));
        }
        results.remove(audio_path).unwrap_or_else(|| {
            RealtimeStoredResult::Failed(result_store_failure(
                FailureCategory::InvalidResponse,
                "Live transcription result is missing",
            ))
        })
    }

    pub fn wait_cancellable(
        &self,
        audio_path: &Path,
        cancel: &AtomicBool,
    ) -> Option<RealtimeStoredResult> {
        self.wait_until_cancelled(audio_path, || cancel.load(Ordering::Acquire))
    }

    pub fn wait_until_cancelled(
        &self,
        audio_path: &Path,
        is_cancelled: impl Fn() -> bool,
    ) -> Option<RealtimeStoredResult> {
        let (lock, ready) = &*self.inner;
        let mut results = match lock.lock() {
            Ok(results) => results,
            Err(_) => {
                return Some(RealtimeStoredResult::Failed(result_store_failure(
                    FailureCategory::Transport,
                    "Live transcription result store is unavailable",
                )));
            }
        };
        let deadline = Instant::now() + RESULT_WAIT_TIMEOUT;
        loop {
            if is_cancelled() {
                results.remove(audio_path);
                return None;
            }
            if let Some(result) = results.remove(audio_path) {
                return Some(result);
            }
            let now = Instant::now();
            if now >= deadline {
                return Some(RealtimeStoredResult::Failed(result_store_failure(
                    FailureCategory::Transport,
                    "Live transcription timed out",
                )));
            }
            let wait_for = RESULT_CANCEL_POLL.min(deadline.saturating_duration_since(now));
            let Ok((next_results, _)) = ready.wait_timeout(results, wait_for) else {
                return Some(RealtimeStoredResult::Failed(result_store_failure(
                    FailureCategory::Transport,
                    "Live transcription result wait failed",
                )));
            };
            results = next_results;
        }
    }

    pub fn discard(&self, audio_path: &Path) {
        let (lock, _) = &*self.inner;
        if let Ok(mut results) = lock.lock() {
            results.remove(audio_path);
        }
    }

    #[cfg(test)]
    pub(crate) fn try_take(&self, audio_path: &Path) -> Option<RealtimeStoredResult> {
        let (lock, _) = &*self.inner;
        lock.lock().ok()?.remove(audio_path)
    }
}

pub fn realtime_result_store() -> &'static RealtimeResultStore {
    static STORE: OnceLock<RealtimeResultStore> = OnceLock::new();
    STORE.get_or_init(RealtimeResultStore::default)
}

#[derive(Debug, Clone)]
pub struct RealtimeSamplesRequest {
    pub source_key: String,
    pub source_label: String,
    pub microphone_slot: Option<MicrophoneSlot>,
    pub chunk_id: u64,
    pub model_id: String,
    pub language: TranscriptionLanguage,
    pub language_hints: Vec<TranscriptionLanguage>,
    pub dictionary: DictionaryContext,
    pub sample_rate: u32,
    pub samples: Arc<Vec<f32>>,
    pub credentials: TranscriptionCredentials,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimePartial {
    pub source_key: String,
    pub source_label: String,
    pub microphone_slot: Option<MicrophoneSlot>,
    pub chunk_id: u64,
    pub text: String,
    /// Leading words the provider marked final; they are never revised.
    /// Empty when the provider has no such notion.
    pub confirmed_text: String,
}

pub type RealtimePartialCallback = Arc<dyn Fn(RealtimePartial) + Send + Sync>;

fn result_store_failure(category: FailureCategory, message: &str) -> TranscriptionFailure {
    TranscriptionFailure {
        provider: None,
        http_status: None,
        provider_code: None,
        safe_message: message.to_string(),
        category,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_result_store_delivers_once_by_audio_path() {
        let store = RealtimeResultStore::default();
        let path = PathBuf::from("/tmp/wakenote-live.wav");
        store.publish(
            path.clone(),
            RealtimeStoredResult::Completed(TranscriptionExecution {
                text: "hello".into(),
                speaker_turns: Vec::new(),
                requested_model_id: "realtime-model".into(),
                effective_model_id: "realtime-model".into(),
                fallback_from_model_id: None,
                usage: None,
                issue: None,
            }),
        );

        let RealtimeStoredResult::Completed(result) = store.wait(&path) else {
            panic!("completed result")
        };
        assert_eq!(result.text, "hello");
    }

    #[test]
    fn realtime_result_store_cancels_and_discards_stale_results() {
        let store = RealtimeResultStore::default();
        let path = PathBuf::from("/tmp/wakenote-meeting-segment.wav");
        let cancel = AtomicBool::new(true);

        store.publish(
            path.clone(),
            RealtimeStoredResult::Failed(result_store_failure(
                FailureCategory::Transport,
                "completed after cancellation",
            )),
        );
        assert!(store.wait_cancellable(&path, &cancel).is_none());
        store.publish(
            path.clone(),
            RealtimeStoredResult::Failed(result_store_failure(FailureCategory::Transport, "stale")),
        );
        store.discard(&path);
        assert!(store.try_take(&path).is_none());
    }
}
