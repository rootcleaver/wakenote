use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header::AUTHORIZATION};
use tokio_tungstenite::{WebSocketStream, connect_async};

use crate::cloud_realtime::{
    RealtimePartial, RealtimePartialCallback, RealtimeSamplesRequest, RealtimeStoredResult,
    realtime_result_store,
};
use crate::cloud_transcription::{FailureCategory, TranscriptionFailure};
use crate::models::OPENAI_GPT_LIVE_TRANSCRIBE_MODEL;
use crate::transcription::{TranscriptionExecution, TranscriptionUsage};

const REALTIME_URL: &str = "wss://api.openai.com/v1/realtime";
const REALTIME_SAMPLE_RATE: u32 = 24_000;
const FINAL_QUIET_PERIOD: Duration = Duration::from_millis(350);
const PARTIAL_COMPLETION_WAIT: Duration = Duration::from_secs(5);
const COMMIT_COMPLETION_WAIT: Duration = Duration::from_secs(15);
const RECORDED_FRAME_MS: usize = 250;
#[derive(Clone)]
pub struct OpenAiRealtimeManager {
    tx: mpsc::UnboundedSender<ManagerCommand>,
}

impl OpenAiRealtimeManager {
    pub fn new(on_partial: RealtimePartialCallback) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tauri::async_runtime::spawn(manager_loop(rx, on_partial));
        Self { tx }
    }

    pub fn submit_samples(&self, request: RealtimeSamplesRequest) {
        let _ = self.tx.send(ManagerCommand::Samples(Box::new(request)));
    }

    pub fn submit_recorded_samples(&self, request: RealtimeSamplesRequest) {
        let _ = self.tx.send(ManagerCommand::Recorded(Box::new(request)));
    }

    pub fn commit(&self, source_key: String, chunk_id: u64, audio_path: PathBuf) {
        let _ = self.tx.send(ManagerCommand::Commit {
            source_key,
            chunk_id,
            audio_path,
        });
    }

    pub fn close_source(&self, source_key: String) {
        let _ = self.tx.send(ManagerCommand::Close(source_key));
    }
}

enum ManagerCommand {
    Samples(Box<RealtimeSamplesRequest>),
    Recorded(Box<RealtimeSamplesRequest>),
    Commit {
        source_key: String,
        chunk_id: u64,
        audio_path: PathBuf,
    },
    Close(String),
}

enum ConnectionCommand {
    Append(String),
    Commit {
        audio_path: PathBuf,
        audio_duration_ms: u64,
    },
    Close,
}

struct ActiveSession {
    chunk_id: u64,
    model_id: String,
    source_rate: u32,
    sent_source_samples: usize,
    tx: mpsc::UnboundedSender<ConnectionCommand>,
    failure: Arc<Mutex<Option<TranscriptionFailure>>>,
}

async fn manager_loop(
    mut rx: mpsc::UnboundedReceiver<ManagerCommand>,
    on_partial: RealtimePartialCallback,
) {
    let mut sessions: HashMap<String, ActiveSession> = HashMap::new();
    while let Some(command) = rx.recv().await {
        match command {
            ManagerCommand::Samples(request) => {
                let request = *request;
                if request.model_id != "openai-gpt-live-transcribe" {
                    continue;
                }
                let replace = sessions.get(&request.source_key).is_none_or(|session| {
                    session.chunk_id != request.chunk_id
                        || session.model_id != request.model_id
                        || session.source_rate != request.sample_rate
                        || request.samples.len() < session.sent_source_samples
                });
                if replace {
                    if let Some(previous) = sessions.remove(&request.source_key) {
                        let _ = previous.tx.send(ConnectionCommand::Close);
                    }
                    let failure = Arc::new(Mutex::new(None));
                    let (tx, command_rx) = mpsc::unbounded_channel();
                    tauri::async_runtime::spawn(connection_loop(
                        request.clone(),
                        command_rx,
                        on_partial.clone(),
                        failure.clone(),
                    ));
                    sessions.insert(
                        request.source_key.clone(),
                        ActiveSession {
                            chunk_id: request.chunk_id,
                            model_id: request.model_id.clone(),
                            source_rate: request.sample_rate,
                            sent_source_samples: 0,
                            tx,
                            failure,
                        },
                    );
                }
                if let Some(session) = sessions.get_mut(&request.source_key) {
                    let (unseen, sent_source_samples) =
                        unseen_samples(&request.samples, session.sent_source_samples);
                    session.sent_source_samples = sent_source_samples;
                    if !unseen.is_empty() {
                        let audio = encode_pcm24k(unseen, request.sample_rate);
                        let _ = session.tx.send(ConnectionCommand::Append(audio));
                    }
                }
            }
            ManagerCommand::Recorded(request) => {
                let request = *request;
                if request.model_id != "openai-gpt-live-transcribe" {
                    continue;
                }
                if let Some(previous) = sessions.remove(&request.source_key) {
                    let _ = previous.tx.send(ConnectionCommand::Close);
                }
                let failure = Arc::new(Mutex::new(None));
                let (tx, command_rx) = mpsc::unbounded_channel();
                tauri::async_runtime::spawn(connection_loop(
                    request.clone(),
                    command_rx,
                    on_partial.clone(),
                    failure.clone(),
                ));
                let frame_samples = recorded_frame_samples(request.sample_rate);
                for frame in request.samples.chunks(frame_samples) {
                    let _ = tx.send(ConnectionCommand::Append(encode_pcm24k(
                        frame,
                        request.sample_rate,
                    )));
                }
                sessions.insert(
                    request.source_key.clone(),
                    ActiveSession {
                        chunk_id: request.chunk_id,
                        model_id: request.model_id,
                        source_rate: request.sample_rate,
                        sent_source_samples: request.samples.len(),
                        tx,
                        failure,
                    },
                );
            }
            ManagerCommand::Commit {
                source_key,
                chunk_id,
                audio_path,
            } => {
                let Some(session) = sessions.get(&source_key) else {
                    realtime_result_store().publish(
                        audio_path,
                        RealtimeStoredResult::Failed(local_failure(
                            FailureCategory::Transport,
                            "OpenAI live transcription session is unavailable",
                        )),
                    );
                    continue;
                };
                if session.chunk_id != chunk_id
                    || session
                        .tx
                        .send(ConnectionCommand::Commit {
                            audio_path: audio_path.clone(),
                            audio_duration_ms: source_duration_ms(
                                session.sent_source_samples,
                                session.source_rate,
                            ),
                        })
                        .is_err()
                {
                    let failure = session
                        .failure
                        .lock()
                        .ok()
                        .and_then(|failure| failure.clone())
                        .unwrap_or_else(|| {
                            local_failure(
                                FailureCategory::Transport,
                                "OpenAI live transcription session ended before commit",
                            )
                        });
                    realtime_result_store()
                        .publish(audio_path, RealtimeStoredResult::Failed(failure));
                    sessions.remove(&source_key);
                }
            }
            ManagerCommand::Close(source_key) => {
                if let Some(session) = sessions.remove(&source_key) {
                    let _ = session.tx.send(ConnectionCommand::Close);
                }
            }
        }
    }
    for session in sessions.into_values() {
        let _ = session.tx.send(ConnectionCommand::Close);
    }
}

async fn connection_loop(
    request: RealtimeSamplesRequest,
    commands: mpsc::UnboundedReceiver<ConnectionCommand>,
    on_partial: RealtimePartialCallback,
    shared_failure: Arc<Mutex<Option<TranscriptionFailure>>>,
) {
    let Some(api_key) = request.credentials.openai_api_key() else {
        store_shared_failure(
            &shared_failure,
            local_failure(
                FailureCategory::Authentication,
                "OpenAI API key is not configured; add it in Settings > Integrations",
            ),
        );
        return;
    };
    let url = realtime_url();
    let Ok(mut websocket_request) = url.into_client_request() else {
        store_shared_failure(&shared_failure, transport_failure());
        return;
    };
    let Ok(authorization) = HeaderValue::from_str(&format!("Bearer {api_key}")) else {
        store_shared_failure(&shared_failure, transport_failure());
        return;
    };
    websocket_request
        .headers_mut()
        .insert(AUTHORIZATION, authorization);
    let Ok((socket, _)) = connect_async(websocket_request).await else {
        store_shared_failure(&shared_failure, transport_failure());
        return;
    };
    run_connected_session(socket, request, commands, on_partial, shared_failure).await;
}

async fn run_connected_session<S>(
    socket: WebSocketStream<S>,
    request: RealtimeSamplesRequest,
    mut commands: mpsc::UnboundedReceiver<ConnectionCommand>,
    on_partial: RealtimePartialCallback,
    shared_failure: Arc<Mutex<Option<TranscriptionFailure>>>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut writer, mut reader) = socket.split();
    let configuration =
        configuration_message(request.language.whisper_code(), request.dictionary.prompt());
    if writer
        .send(Message::Text(configuration.to_string().into()))
        .await
        .is_err()
    {
        store_shared_failure(&shared_failure, transport_failure());
        return;
    }

    let mut transcript = RealtimeTranscript::default();
    let mut committed_path: Option<PathBuf> = None;
    let mut committed_audio_duration_ms = 0;
    let mut final_deadline: Option<Instant> = None;
    let mut command_channel_open = true;
    loop {
        let deadline = final_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            command = commands.recv(), if command_channel_open => match command {
                Some(ConnectionCommand::Append(audio)) => {
                    let event = json!({ "type": "input_audio_buffer.append", "audio": audio });
                    if writer.send(Message::Text(event.to_string().into())).await.is_err() {
                        store_shared_failure(&shared_failure, transport_failure());
                        return;
                    }
                }
                Some(ConnectionCommand::Commit { audio_path, audio_duration_ms }) => {
                    committed_path = Some(audio_path);
                    committed_audio_duration_ms = audio_duration_ms;
                    final_deadline = Some(Instant::now() + COMMIT_COMPLETION_WAIT);
                    let event = json!({ "type": "input_audio_buffer.commit" });
                    let _ = writer.send(Message::Text(event.to_string().into())).await;
                }
                // A close after commit means "no more audio", not "drop the
                // result": the transcription worker is already waiting for this
                // path, so keep the socket open until the transcript completes
                // or the commit deadline fires. The manager closes a session as
                // soon as the next chunk's samples arrive, which races completion.
                Some(ConnectionCommand::Close) | None => {
                    command_channel_open = false;
                    if committed_path.is_none() {
                        let _ = writer.close().await;
                        return;
                    }
                }
            },
            message = reader.next() => {
                let Some(Ok(Message::Text(message))) = message else {
                    if let Some(path) = committed_path.take() {
                        realtime_result_store().publish(path, RealtimeStoredResult::Failed(transport_failure()));
                    } else {
                        store_shared_failure(&shared_failure, transport_failure());
                    }
                    return;
                };
                let Ok(event) = serde_json::from_str::<Value>(&message) else {
                    continue;
                };
                match parse_server_event(&event) {
                    ServerEvent::ItemCreated { item_id, previous_item_id } => {
                        transcript.note_item(item_id, previous_item_id);
                    }
                    ServerEvent::Delta { item_id, delta } => {
                        transcript.push_delta(&item_id, &delta);
                        on_partial(RealtimePartial {
                            source_key: request.source_key.clone(),
                            source_label: request.source_label.clone(),
                            microphone_slot: request.microphone_slot,
                            chunk_id: request.chunk_id,
                            text: transcript.text(),
                            confirmed_text: String::new(),
                        });
                        if committed_path.is_some() {
                            final_deadline = Some(Instant::now() + PARTIAL_COMPLETION_WAIT);
                        }
                    }
                    ServerEvent::Completed { item_id, text } => {
                        transcript.complete(&item_id, text);
                        if committed_path.is_some() {
                            final_deadline = Some(Instant::now() + FINAL_QUIET_PERIOD);
                        }
                    }
                    ServerEvent::CommitEmpty => {
                        if let Some(path) = committed_path.take() {
                            publish_completed_result(
                                path,
                                &request.model_id,
                                transcript.text(),
                                committed_audio_duration_ms,
                            );
                            let _ = writer.close().await;
                            return;
                        }
                    }
                    ServerEvent::Failed(failure) => {
                        store_shared_failure(&shared_failure, failure.clone());
                        if let Some(path) = committed_path.take() {
                            realtime_result_store().publish(path, RealtimeStoredResult::Failed(failure));
                        }
                        return;
                    }
                    ServerEvent::Ignore => {}
                }
            },
            _ = sleep_until(deadline), if final_deadline.is_some() => {
                let Some(path) = committed_path.take() else { continue; };
                let text = transcript.text();
                publish_completed_result(
                    path,
                    &request.model_id,
                    text,
                    committed_audio_duration_ms,
                );
                let _ = writer.close().await;
                return;
            }
        }
    }
}

fn realtime_url() -> String {
    format!("{REALTIME_URL}?intent=transcription")
}

fn configuration_message(language: Option<&str>, prompt: Option<&str>) -> Value {
    let mut transcription = json!({ "model": OPENAI_GPT_LIVE_TRANSCRIBE_MODEL });
    if let Some(language) = language {
        transcription["languages"] = json!([language]);
    }
    if let Some(prompt) = prompt {
        transcription["prompt"] = Value::String(prompt.to_string());
    }
    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": REALTIME_SAMPLE_RATE },
                    "transcription": transcription,
                    "turn_detection": null
                }
            }
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerEvent {
    ItemCreated {
        item_id: String,
        previous_item_id: Option<String>,
    },
    Delta {
        item_id: String,
        delta: String,
    },
    Completed {
        item_id: String,
        text: String,
    },
    Failed(TranscriptionFailure),
    CommitEmpty,
    Ignore,
}

fn parse_server_event(event: &Value) -> ServerEvent {
    match event.get("type").and_then(Value::as_str) {
        Some("conversation.item.created") => {
            let Some(item_id) = event
                .get("item")
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
            else {
                return ServerEvent::Ignore;
            };
            ServerEvent::ItemCreated {
                item_id: item_id.to_string(),
                previous_item_id: event
                    .get("previous_item_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }
        }
        Some("conversation.item.input_audio_transcription.delta") => {
            string_pair_event(event, "delta").map_or(ServerEvent::Ignore, |(item_id, delta)| {
                ServerEvent::Delta { item_id, delta }
            })
        }
        Some("conversation.item.input_audio_transcription.completed") => {
            string_pair_event(event, "transcript").map_or(ServerEvent::Ignore, |(item_id, text)| {
                ServerEvent::Completed { item_id, text }
            })
        }
        Some("conversation.item.input_audio_transcription.failed") | Some("error") => {
            let error = event.get("error").unwrap_or(event);
            let code = error.get("code").and_then(Value::as_str);
            if code == Some("input_audio_buffer_commit_empty") {
                ServerEvent::CommitEmpty
            } else {
                ServerEvent::Failed(provider_failure_from_code(code))
            }
        }
        _ => ServerEvent::Ignore,
    }
}

fn string_pair_event(event: &Value, value_key: &str) -> Option<(String, String)> {
    Some((
        event.get("item_id")?.as_str()?.to_string(),
        event.get(value_key)?.as_str()?.to_string(),
    ))
}

#[derive(Debug, Default)]
struct RealtimeTranscript {
    items: Vec<TranscriptItem>,
}

#[derive(Debug)]
struct TranscriptItem {
    id: String,
    previous_id: Option<String>,
    text: String,
}

impl RealtimeTranscript {
    fn note_item(&mut self, id: String, previous_id: Option<String>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.previous_id = previous_id;
        } else {
            self.items.push(TranscriptItem {
                id,
                previous_id,
                text: String::new(),
            });
        }
    }

    fn push_delta(&mut self, id: &str, delta: &str) {
        self.item_mut(id).text.push_str(delta);
    }

    fn complete(&mut self, id: &str, text: String) {
        self.item_mut(id).text = text;
    }

    fn item_mut(&mut self, id: &str) -> &mut TranscriptItem {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            return &mut self.items[index];
        }
        self.items.push(TranscriptItem {
            id: id.to_string(),
            previous_id: None,
            text: String::new(),
        });
        self.items.last_mut().expect("just inserted")
    }

    fn text(&self) -> String {
        let mut ordered = Vec::with_capacity(self.items.len());
        let mut remaining = self.items.iter().collect::<Vec<_>>();
        let mut previous: Option<&str> = None;
        while !remaining.is_empty() {
            let index = remaining
                .iter()
                .position(|item| item.previous_id.as_deref() == previous)
                .unwrap_or(0);
            let item = remaining.remove(index);
            previous = Some(&item.id);
            ordered.push(item.text.trim());
        }
        ordered
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn encode_pcm24k(samples: &[f32], source_rate: u32) -> String {
    let resampled = resample(samples, source_rate, REALTIME_SAMPLE_RATE);
    let mut bytes = Vec::with_capacity(resampled.len() * 2);
    for sample in resampled {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&pcm.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unseen_samples(samples: &[f32], sent_samples: usize) -> (&[f32], usize) {
    (&samples[sent_samples.min(samples.len())..], samples.len())
}

fn recorded_frame_samples(sample_rate: u32) -> usize {
    (sample_rate as usize)
        .saturating_mul(RECORDED_FRAME_MS)
        .checked_div(1_000)
        .unwrap_or_default()
        .max(1)
}

fn resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return samples.to_vec();
    }
    let output_len = ((samples.len() as u64 * target_rate as u64) / source_rate as u64) as usize;
    (0..output_len)
        .map(|index| {
            let source = index as f64 * source_rate as f64 / target_rate as f64;
            let left = source.floor() as usize;
            let right = (left + 1).min(samples.len() - 1);
            let fraction = (source - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

fn source_duration_ms(samples: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        0
    } else {
        (samples as u64).saturating_mul(1_000) / sample_rate as u64
    }
}

fn publish_completed_result(
    audio_path: PathBuf,
    model_id: &str,
    text: String,
    audio_duration_ms: u64,
) {
    realtime_result_store().publish(
        audio_path,
        RealtimeStoredResult::Completed(TranscriptionExecution {
            text,
            speaker_turns: Vec::new(),
            requested_model_id: model_id.to_string(),
            effective_model_id: model_id.to_string(),
            fallback_from_model_id: None,
            usage: Some(TranscriptionUsage {
                provider: Some("OpenAI".to_string()),
                audio_duration_ms,
                provider_cost_usd: None,
            }),
            issue: None,
        }),
    );
}

fn provider_failure_from_code(code: Option<&str>) -> TranscriptionFailure {
    let category = if matches!(
        code,
        Some(
            "credit_balance_exhausted"
                | "organization_spend_limit_exceeded"
                | "project_spend_limit_exceeded"
                | "organization_usage_limit_exceeded"
        )
    ) {
        FailureCategory::BillingLimit
    } else {
        FailureCategory::Provider
    };
    TranscriptionFailure {
        provider: Some("OpenAI"),
        http_status: None,
        provider_code: code.map(str::to_string),
        safe_message: "OpenAI live transcription failed".to_string(),
        category,
    }
}

fn local_failure(category: FailureCategory, message: &str) -> TranscriptionFailure {
    TranscriptionFailure {
        provider: Some("OpenAI"),
        http_status: None,
        provider_code: None,
        safe_message: message.to_string(),
        category,
    }
}

fn transport_failure() -> TranscriptionFailure {
    local_failure(
        FailureCategory::Transport,
        "OpenAI live transcription connection failed",
    )
}

fn store_shared_failure(
    shared: &Arc<Mutex<Option<TranscriptionFailure>>>,
    failure: TranscriptionFailure,
) {
    if let Ok(mut slot) = shared.lock() {
        *slot = Some(failure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[test]
    fn openai_realtime_uses_the_transcription_transport() {
        assert_eq!(
            realtime_url(),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
    }

    #[test]
    fn openai_realtime_builds_documented_explicit_commit_configuration() {
        let configuration = configuration_message(Some("ko"), Some("WakeNote"));

        assert_eq!(configuration["type"], "session.update");
        assert_eq!(configuration["session"]["type"], "transcription");
        assert_eq!(
            configuration["session"]["audio"]["input"]["format"],
            json!({ "type": "audio/pcm", "rate": 24_000 })
        );
        assert_eq!(
            configuration["session"]["audio"]["input"]["transcription"]["languages"],
            json!(["ko"])
        );
        assert!(
            configuration["session"]["audio"]["input"]["transcription"]
                .get("language")
                .is_none()
        );
        assert_eq!(
            configuration["session"]["audio"]["input"]["transcription"]["prompt"],
            "WakeNote"
        );
        assert!(configuration["session"]["audio"]["input"]["turn_detection"].is_null());

        let automatic = configuration_message(None, None);
        assert!(
            automatic["session"]["audio"]["input"]["transcription"]
                .get("languages")
                .is_none()
        );
        assert!(
            automatic["session"]["audio"]["input"]["transcription"]
                .get("prompt")
                .is_none()
        );
    }

    #[test]
    fn openai_realtime_encodes_resampled_pcm_without_reencoding_prior_samples() {
        let first = encode_pcm24k(&[0.0, 0.5, -0.5, 1.0], 24_000);
        let tail = encode_pcm24k(&[-1.0, 0.25], 24_000);
        let first_bytes = base64::engine::general_purpose::STANDARD
            .decode(first)
            .expect("first pcm");
        let tail_bytes = base64::engine::general_purpose::STANDARD
            .decode(tail)
            .expect("tail pcm");

        assert_eq!(first_bytes.len(), 8);
        assert_eq!(tail_bytes.len(), 4);
        assert_ne!(first_bytes[..4], tail_bytes[..]);
        assert_eq!(resample(&[0.0; 16_000], 16_000, 24_000).len(), 24_000);

        let cumulative = [0.0, 0.5, -0.5, 1.0, -1.0, 0.25];
        let (unseen, next_sent) = unseen_samples(&cumulative, 4);
        assert_eq!(unseen, &[-1.0, 0.25]);
        assert_eq!(next_sent, cumulative.len());

        let recorded = vec![0.0; 16_001];
        let frames = recorded
            .chunks(recorded_frame_samples(16_000))
            .collect::<Vec<_>>();
        assert_eq!(recorded_frame_samples(16_000), 4_000);
        assert_eq!(frames.len(), 5);
        assert!(frames.iter().all(|frame| frame.len() <= 4_000));
        assert!(frames.iter().all(|frame| {
            base64::engine::general_purpose::STANDARD
                .decode(encode_pcm24k(frame, 16_000))
                .is_ok_and(|bytes| bytes.len() <= 12_000)
        }));
    }

    #[test]
    fn openai_realtime_orders_completed_items_by_relationship() {
        let mut transcript = RealtimeTranscript::default();
        transcript.complete("second", "world".into());
        transcript.note_item("second".into(), Some("first".into()));
        transcript.complete("first", "hello".into());
        transcript.note_item("first".into(), None);

        assert_eq!(transcript.text(), "hello world");
    }

    #[test]
    fn openai_realtime_parses_deltas_completion_and_safe_billing_errors() {
        assert_eq!(
            parse_server_event(&json!({
                "type": "conversation.item.input_audio_transcription.delta",
                "item_id": "item-1",
                "delta": "Wake"
            })),
            ServerEvent::Delta {
                item_id: "item-1".into(),
                delta: "Wake".into(),
            }
        );
        assert_eq!(
            parse_server_event(&json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "item_id": "item-1",
                "transcript": "WakeNote"
            })),
            ServerEvent::Completed {
                item_id: "item-1".into(),
                text: "WakeNote".into(),
            }
        );
        let ServerEvent::Failed(failure) = parse_server_event(&json!({
            "type": "error",
            "error": {
                "code": "credit_balance_exhausted",
                "message": "private provider response"
            }
        })) else {
            panic!("failure event")
        };
        assert_eq!(failure.category, FailureCategory::BillingLimit);
        assert!(!failure.safe_message.contains("private"));
        assert_eq!(
            parse_server_event(&json!({
                "type": "error",
                "error": { "code": "input_audio_buffer_commit_empty" }
            })),
            ServerEvent::CommitEmpty
        );
    }

    #[tokio::test]
    async fn openai_realtime_delivers_committed_result_when_closed_before_completion() {
        // Same race as Soniox: the manager closes the previous chunk's session
        // when the next chunk's samples arrive, which can precede the completed
        // transcript. The committed chunk must still publish its result.
        let audio_path = PathBuf::from("/tmp/wakenote-openai-closed-before-completion.wav");
        let _ = realtime_result_store().try_take(&audio_path);
        let (client_io, server_io) = duplex(1_048_576);
        let client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        command_tx
            .send(ConnectionCommand::Append("AAA=".into()))
            .expect("append command");
        command_tx
            .send(ConnectionCommand::Commit {
                audio_path: audio_path.clone(),
                audio_duration_ms: 20,
            })
            .expect("commit command");
        command_tx
            .send(ConnectionCommand::Close)
            .expect("close command");

        let request = RealtimeSamplesRequest {
            language_hints: vec![crate::settings::TranscriptionLanguage::Ko],
            source_key: "microphone:test".into(),
            source_label: "Test microphone".into(),
            microphone_slot: None,
            chunk_id: 1,
            model_id: OPENAI_GPT_LIVE_TRANSCRIBE_MODEL.into(),
            language: crate::settings::TranscriptionLanguage::Ko,
            dictionary: crate::dictionary::DictionaryContext::default(),
            sample_rate: REALTIME_SAMPLE_RATE,
            samples: Arc::new(vec![0.0]),
            credentials: crate::cloud_transcription::TranscriptionCredentials::default(),
        };
        let shared_failure = Arc::new(Mutex::new(None));
        let client_task = tokio::spawn(run_connected_session(
            client,
            request,
            command_rx,
            Arc::new(|_| {}),
            shared_failure,
        ));
        let server_task = tokio::spawn(async move {
            for expected in [
                "session.update",
                "input_audio_buffer.append",
                "input_audio_buffer.commit",
            ] {
                let Some(Ok(Message::Text(message))) = server.next().await else {
                    panic!("expected {expected} event")
                };
                let event = serde_json::from_str::<Value>(&message).expect("client event");
                assert_eq!(event["type"], expected);
            }
            for event in [
                json!({ "type": "conversation.item.created", "item": { "id": "item-1" } }),
                json!({
                    "type": "conversation.item.input_audio_transcription.completed",
                    "item_id": "item-1",
                    "transcript": "hello there"
                }),
            ] {
                let _ = server.send(Message::Text(event.to_string().into())).await;
            }
            // Stay connected until the client publishes and closes the socket.
            while let Some(Ok(message)) = server.next().await {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
        });

        let (client_result, server_result) = tokio::join!(client_task, server_task);
        client_result.expect("client task");
        server_result.expect("server task");
        let Some(RealtimeStoredResult::Completed(result)) =
            realtime_result_store().try_take(&audio_path)
        else {
            panic!("committed result must be published even when closed before completion")
        };
        assert_eq!(result.text, "hello there");
        assert_eq!(result.usage.map(|usage| usage.audio_duration_ms), Some(20));
    }
}
