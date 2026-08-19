use std::io;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Receiver, channel};
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};

use crate::runtime::{RuntimeBootstrap, TOOL_LOG_HINT, TOOL_READ_EDITOR};

const LIVE_WEBSOCKET_ENDPOINT: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
const SETUP_TIMEOUT: Duration = Duration::from_secs(15);
/// Per attempt, so three tries plus backoff stay inside `REPORT_TIMEOUT`.
const REPORT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(12);
const REPORT_RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];
/// Gemini streams audio in 20ms-ish chunks, so this is a few seconds of slack
/// for a main loop that is briefly busy publishing or writing a report.
const GEMINI_EVENT_QUEUE: usize = 256;

pub struct GeminiLiveSession {
    writer: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    reader: JoinHandle<()>,
    events: Receiver<GeminiEvent>,
}

impl GeminiLiveSession {
    pub async fn send_text(
        &mut self,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.send_json(realtime_text_message(text)).await
    }

    pub async fn send_audio_pcm_16khz(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.send_json(realtime_audio_message(bytes)).await
    }

    pub async fn send_video_frame(
        &mut self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.send_json(realtime_video_message(bytes, mime_type))
            .await
    }

    pub async fn send_tool_response(
        &mut self,
        call: &GeminiFunctionCall,
        response: Value,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.send_json(tool_response_message(call, response)).await
    }

    pub async fn next_event(&mut self) -> Option<GeminiEvent> {
        self.events.recv().await
    }

    pub async fn close(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.shutdown().await
    }

    /// Same teardown as [`Self::close`], for callers that only hold a mutable
    /// borrow and cannot give up ownership.
    pub async fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.writer.close().await?;
        self.reader.abort();
        Ok(())
    }

    async fn send_json(
        &mut self,
        message: Value,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.writer
            .send(Message::Text(serde_json::to_string(&message)?.into()))
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeminiEvent {
    Audio { bytes: Vec<u8>, mime_type: String },
    Text(String),
    InputTranscript(String),
    OutputTranscript(String),
    TurnComplete,
    Interrupted,
    ToolCall(Vec<GeminiFunctionCall>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiFunctionCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

pub async fn open_live_session(
    api_key: &str,
    boot: &RuntimeBootstrap<'_>,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    open_live_session_at(&gemini_live_websocket_url(api_key), boot).await
}

/// Gemini answers 503 often enough that a single attempt loses reports for a
/// reason that clears in seconds. The candidate is waiting, so the budget stays
/// small: three tries, short backoff, bounded by `REPORT_TIMEOUT` upstream.
pub async fn generate_report(
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut attempt = 0;
    loop {
        let error = match generate_report_once(api_key, model, prompt).await {
            Ok(report) => return Ok(report),
            Err(error) => error,
        };
        let Some(backoff) = REPORT_RETRY_BACKOFF
            .get(attempt)
            .copied()
            .filter(|_| is_retryable(error.as_ref()))
        else {
            return Err(error);
        };
        eprintln!(
            "gemini report attempt {} failed, retrying in {}s: {}",
            attempt + 1,
            backoff.as_secs(),
            redact_api_key(&error.to_string(), api_key)
        );
        tokio::time::sleep(backoff).await;
        attempt += 1;
    }
}

/// Transient upstream conditions only. A bad key or a bad model is answered the
/// same way every time, so retrying it just makes the candidate wait longer.
fn is_retryable(error: &(dyn std::error::Error + 'static)) -> bool {
    let Some(error) = error.downcast_ref::<reqwest::Error>() else {
        return false;
    };
    if error.is_timeout() || error.is_connect() {
        return true;
    }
    error.status().is_some_and(|status| {
        status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    })
}

async fn generate_report_once(
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let response = crate::http_client()
        .post(gemini_generate_content_url(model))
        .header("x-goog-api-key", api_key)
        .timeout(REPORT_ATTEMPT_TIMEOUT)
        .json(&generate_report_request(prompt))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let Some(text) = gemini_text(&response) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Gemini report response had no text",
        )
        .into());
    };
    let Some(json_text) = extract_json_object(&text) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Gemini report response had no JSON object",
        )
        .into());
    };
    Ok(serde_json::from_str(json_text)?)
}

async fn open_live_session_at(
    url: &str,
    boot: &RuntimeBootstrap<'_>,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    let (mut stream, _) = connect_async(url).await?;
    stream
        .send(Message::Text(
            serde_json::to_string(&live_setup_message(boot))?.into(),
        ))
        .await?;
    tokio::time::timeout(SETUP_TIMEOUT, wait_for_setup_complete(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Gemini setup timed out"))??;
    let (writer, mut reader) = stream.split();
    let (sender, events) = channel(GEMINI_EVENT_QUEUE);
    let reader = tokio::spawn(async move {
        while let Some(message) = reader.next().await {
            let Ok(message) = message else { break };
            let Some(text) = websocket_message_text(message) else {
                continue;
            };
            for event in parse_server_events(&text) {
                // Bounded on purpose: a stalled main loop must slow the socket
                // down, not let inbound audio pile up without a ceiling.
                if sender.send(event).await.is_err() {
                    return;
                }
            }
        }
    });
    Ok(GeminiLiveSession {
        writer,
        reader,
        events,
    })
}

fn gemini_live_websocket_url(api_key: &str) -> String {
    format!(
        "{LIVE_WEBSOCKET_ENDPOINT}?key={}",
        percent_encode_query_value(api_key)
    )
}

/// No `?key=` here on purpose. A `reqwest` error Displays the URL it was built
/// from, and this call's errors reach the candidate's browser in the report
/// failure note, so the credential travels in a header instead.
fn gemini_generate_content_url(model: &str) -> String {
    format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        gemini_model_id(model)
    )
}

/// Model names are accepted both bare and resource-qualified; the REST path
/// wants the bare id, the live socket wants the qualified one.
fn gemini_model_id(model: &str) -> &str {
    model.trim_start_matches("models/")
}

fn gemini_text(value: &Value) -> Option<String> {
    let text = value
        .get("candidates")?
        .as_array()?
        .first()?
        .get("content")?
        .get("parts")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

/// Gemini wraps report JSON in markdown fences often enough that trusting the
/// response mime type alone loses reports.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start <= end).then_some(&text[start..=end])
}

pub fn redact_api_key(text: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return text.to_string();
    }
    text.replace(api_key, "[REDACTED]")
        .replace(&percent_encode_query_value(api_key), "[REDACTED]")
}

fn live_setup_message(boot: &RuntimeBootstrap<'_>) -> Value {
    json!({
        "setup": {
            "model": format!("models/{}", gemini_model_id(boot.live_model)),
            "generationConfig": {
                "temperature": 0.7,
                "responseModalities": ["AUDIO"],
                "speechConfig": {
                    "voiceConfig": {
                        "prebuiltVoiceConfig": {
                            "voiceName": boot.voice
                        }
                    }
                }
            },
            "systemInstruction": {
                "parts": [
                    { "text": boot.instructions }
                ]
            },
            "tools": [
                {
                    "functionDeclarations": [
                        {
                            "name": TOOL_READ_EDITOR,
                            "description": "Return the current editor language, numbered code, and latest test run summary."
                        },
                        {
                            "name": TOOL_LOG_HINT,
                            "description": "Record that the interviewer gave the candidate a hint."
                        }
                    ]
                }
            ],
            "inputAudioTranscription": {},
            "outputAudioTranscription": {},
            "realtimeInputConfig": {
                "activityHandling": "START_OF_ACTIVITY_INTERRUPTS"
            },
            "contextWindowCompression": {
                "slidingWindow": {}
            },
            "sessionResumption": {}
        }
    })
}

fn realtime_text_message(text: &str) -> Value {
    json!({ "realtimeInput": { "text": text } })
}

fn realtime_audio_message(bytes: &[u8]) -> Value {
    json!({
        "realtimeInput": {
            "audio": {
                "data": STANDARD.encode(bytes),
                "mimeType": "audio/pcm;rate=16000"
            }
        }
    })
}

fn realtime_video_message(bytes: &[u8], mime_type: &str) -> Value {
    json!({
        "realtimeInput": {
            "video": {
                "data": STANDARD.encode(bytes),
                "mimeType": mime_type
            }
        }
    })
}

fn tool_response_message(call: &GeminiFunctionCall, response: Value) -> Value {
    json!({
        "toolResponse": {
            "functionResponses": [
                {
                    "name": call.name,
                    "id": call.id,
                    "response": response
                }
            ]
        }
    })
}

fn generate_report_request(prompt: &str) -> Value {
    json!({
        "contents": [
            {
                "parts": [
                    { "text": prompt }
                ]
            }
        ],
        "generationConfig": {
            "responseMimeType": "application/json",
            "temperature": 0.3
        }
    })
}

async fn wait_for_setup_complete(
    stream: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    while let Some(message) = stream.next().await {
        let message = message?;
        match message {
            Message::Text(text) if is_setup_complete_text(text.as_str()) => return Ok(()),
            Message::Binary(bytes) if is_setup_complete_text(std::str::from_utf8(&bytes)?) => {
                return Ok(());
            }
            Message::Close(frame) => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    format!("Gemini closed before setupComplete: {frame:?}"),
                )
                .into());
            }
            _ => {}
        }
    }

    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "Gemini WebSocket ended before setupComplete",
    )
    .into())
}

fn is_setup_complete_text(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .is_some_and(|message| message.get("setupComplete").is_some())
}

fn websocket_message_text(message: Message) -> Option<String> {
    match message {
        Message::Text(text) => Some(text.to_string()),
        Message::Binary(bytes) => String::from_utf8(bytes.to_vec()).ok(),
        _ => None,
    }
}

fn parse_server_events(text: &str) -> Vec<GeminiEvent> {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let mut events = Vec::new();

    if let Some(parts) = message
        .pointer("/serverContent/modelTurn/parts")
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(inline_data) = part.get("inlineData") {
                let Some(data) = inline_data.get("data").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(bytes) = STANDARD.decode(data) else {
                    continue;
                };
                let mime_type = inline_data
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("audio/pcm")
                    .to_string();
                events.push(GeminiEvent::Audio { bytes, mime_type });
            }
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                events.push(GeminiEvent::Text(text.to_string()));
            }
        }
    }
    if let Some(text) = message
        .pointer("/serverContent/inputTranscription/text")
        .and_then(Value::as_str)
    {
        events.push(GeminiEvent::InputTranscript(text.to_string()));
    }
    if let Some(text) = message
        .pointer("/serverContent/outputTranscription/text")
        .and_then(Value::as_str)
    {
        events.push(GeminiEvent::OutputTranscript(text.to_string()));
    }
    if message
        .pointer("/serverContent/turnComplete")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        events.push(GeminiEvent::TurnComplete);
    }
    if message
        .pointer("/serverContent/interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        events.push(GeminiEvent::Interrupted);
    }
    if let Some(calls) = message
        .pointer("/toolCall/functionCalls")
        .and_then(Value::as_array)
    {
        let calls = calls
            .iter()
            .filter_map(|call| {
                Some(GeminiFunctionCall {
                    id: call.get("id")?.as_str()?.to_string(),
                    name: call.get("name")?.as_str()?.to_string(),
                    args: call.get("args").cloned().unwrap_or_else(|| json!({})),
                })
            })
            .collect::<Vec<_>>();
        if !calls.is_empty() {
            events.push(GeminiEvent::ToolCall(calls));
        }
    }

    events
}

fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_from_pairs;
    use crate::runtime::bootstrap;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    /// Every Live test needs the same credentials and differs only in what it
    /// overrides, so the shared half is written once. Later pairs win, which is
    /// how an override works.
    fn live_config(overrides: &[(&str, &str)]) -> crate::config::AgentConfig {
        let mut pairs = vec![
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "google"),
            ("GEMINI_LIVE_MODEL", "gemini-live"),
        ];
        pairs.extend_from_slice(overrides);
        load_from_pairs(pairs).unwrap()
    }

    #[test]
    fn live_setup_uses_native_audio_voice_tools_and_transcription() {
        let config = live_config(&[("GEMINI_VOICE", "Kore")]);
        let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);

        let message = live_setup_message(&boot);
        let setup = &message["setup"];

        assert_eq!(setup["model"], "models/gemini-live");
        assert_eq!(setup["generationConfig"]["temperature"], 0.7);
        assert_eq!(setup["generationConfig"]["responseModalities"][0], "AUDIO");
        assert_eq!(
            setup["generationConfig"]["responseModalities"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            setup["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
            "Kore"
        );
        assert!(
            setup["systemInstruction"]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("45-minute technical coding interview")
        );
        assert_eq!(
            setup["tools"][0]["functionDeclarations"][0]["name"],
            TOOL_READ_EDITOR
        );
        assert!(
            setup["tools"][0]["functionDeclarations"][0]
                .get("parameters")
                .is_none()
        );
        assert_eq!(
            setup["tools"][0]["functionDeclarations"][1]["name"],
            TOOL_LOG_HINT
        );
        assert!(
            setup["tools"][0]["functionDeclarations"][1]
                .get("parameters")
                .is_none()
        );
        assert_eq!(setup["inputAudioTranscription"], json!({}));
        assert_eq!(setup["outputAudioTranscription"], json!({}));
        assert_eq!(
            setup["realtimeInputConfig"]["activityHandling"],
            "START_OF_ACTIVITY_INTERRUPTS"
        );
        assert_eq!(
            setup["contextWindowCompression"]["slidingWindow"],
            json!({})
        );
        assert_eq!(setup["sessionResumption"], json!({}));
    }

    #[test]
    fn live_setup_accepts_model_names_with_resource_prefix() {
        let config = live_config(&[("GEMINI_LIVE_MODEL", "models/gemini-live")]);
        let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);

        assert_eq!(
            live_setup_message(&boot)["setup"]["model"],
            "models/gemini-live"
        );
    }

    /// Spins a server that answers `status` once, and hands back the real
    /// `reqwest` error, since these cannot be constructed by hand.
    async fn status_error(status: &str) -> reqwest::Error {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response =
            format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let _ = socket.read(&mut [0u8; 2048]).await;
            let _ = socket.write_all(response.as_bytes()).await;
        });
        crate::http_client()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err()
    }

    /// Gemini answers 503 often enough to lose reports over it, but a bad key
    /// answers the same way every time and retrying only makes the candidate
    /// wait longer for the same failure.
    #[tokio::test]
    async fn only_transient_upstream_failures_are_retried() {
        for status in [
            "503 Service Unavailable",
            "500 Internal Server Error",
            "429 Too Many Requests",
        ] {
            let error = status_error(status).await;
            assert!(is_retryable(&error), "{status} should retry");
        }
        for status in ["400 Bad Request", "401 Unauthorized", "404 Not Found"] {
            let error = status_error(status).await;
            assert!(!is_retryable(&error), "{status} should not retry");
        }
    }

    #[test]
    fn generate_content_url_carries_no_credential() {
        // A `reqwest` error Displays the URL it was built from, and that error
        // reaches the candidate's browser in the report failure note, so the
        // credential has to travel in a header instead.
        let url = gemini_generate_content_url("models/gemini-report");

        assert!(!url.contains("key="), "{url}");
        assert!(!url.contains('?'), "{url}");
    }

    #[test]
    fn gemini_live_websocket_url_escapes_api_key_query_value() {
        assert_eq!(
            gemini_live_websocket_url("abc+123/="),
            format!("{LIVE_WEBSOCKET_ENDPOINT}?key=abc%2B123%2F%3D")
        );
    }

    #[test]
    fn report_generation_request_matches_python_report_model_config() {
        assert_eq!(
            gemini_generate_content_url("models/gemini-report"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-report:generateContent"
        );

        assert_eq!(
            generate_report_request("score this"),
            json!({
                "contents": [
                    {
                        "parts": [
                            { "text": "score this" }
                        ]
                    }
                ],
                "generationConfig": {
                    "responseMimeType": "application/json",
                    "temperature": 0.3
                }
            })
        );
    }

    #[test]
    fn gemini_text_extracts_first_candidate_parts() {
        let value = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Got it. "},
                        {"text": "What invariant are you maintaining?"}
                    ]
                }
            }]
        });

        assert_eq!(
            gemini_text(&value).as_deref(),
            Some("Got it. What invariant are you maintaining?")
        );
    }

    #[test]
    fn extract_json_object_ignores_markdown_wrapper() {
        let text = "```json\n{\"decision\":\"HIRE\"}\n```";

        assert_eq!(extract_json_object(text), Some("{\"decision\":\"HIRE\"}"));
    }

    #[test]
    fn gemini_model_id_accepts_listed_model_names() {
        assert_eq!(
            gemini_model_id("models/gemini-flash-latest"),
            "gemini-flash-latest"
        );
        assert_eq!(
            gemini_model_id("gemini-flash-latest"),
            "gemini-flash-latest"
        );
    }

    #[test]
    fn setup_complete_parser_accepts_only_setup_complete_messages() {
        assert!(is_setup_complete_text(r#"{"setupComplete":{}}"#));
        assert!(!is_setup_complete_text(
            r#"{"serverContent":{"turnComplete":true}}"#
        ));
        assert!(!is_setup_complete_text("not json"));
    }

    #[test]
    fn realtime_messages_match_live_websocket_shapes() {
        assert_eq!(
            realtime_text_message("hello"),
            json!({"realtimeInput":{"text":"hello"}})
        );
        assert_eq!(
            realtime_audio_message(&[0, 1, 2])["realtimeInput"]["audio"],
            json!({"data":"AAEC","mimeType":"audio/pcm;rate=16000"})
        );
        assert_eq!(
            realtime_video_message(&[3, 4], "image/jpeg")["realtimeInput"]["video"],
            json!({"data":"AwQ=","mimeType":"image/jpeg"})
        );
    }

    #[test]
    fn tool_response_message_matches_live_websocket_shape() {
        let call = GeminiFunctionCall {
            id: "call-1".to_string(),
            name: TOOL_READ_EDITOR.to_string(),
            args: json!({}),
        };

        assert_eq!(
            tool_response_message(&call, json!({"result":"code"})),
            json!({
                "toolResponse": {
                    "functionResponses": [
                        {
                            "name": TOOL_READ_EDITOR,
                            "id": "call-1",
                            "response": {"result":"code"}
                        }
                    ]
                }
            })
        );
    }

    #[test]
    fn parse_server_events_extracts_audio_transcripts_and_tool_calls() {
        let events = parse_server_events(
            r#"{
                "serverContent": {
                    "modelTurn": {
                        "parts": [
                            {"inlineData": {"data": "AAE=", "mimeType": "audio/pcm;rate=24000"}},
                            {"text": "text output"}
                        ]
                    },
                    "inputTranscription": {"text": "candidate"},
                    "outputTranscription": {"text": "interviewer"},
                    "turnComplete": true,
                    "interrupted": true
                },
                "toolCall": {
                    "functionCalls": [
                        {"id": "1", "name": "read_editor", "args": {"a": 1}}
                    ]
                }
            }"#,
        );

        assert_eq!(
            events,
            vec![
                GeminiEvent::Audio {
                    bytes: vec![0, 1],
                    mime_type: "audio/pcm;rate=24000".to_string(),
                },
                GeminiEvent::Text("text output".to_string()),
                GeminiEvent::InputTranscript("candidate".to_string()),
                GeminiEvent::OutputTranscript("interviewer".to_string()),
                GeminiEvent::TurnComplete,
                GeminiEvent::Interrupted,
                GeminiEvent::ToolCall(vec![GeminiFunctionCall {
                    id: "1".to_string(),
                    name: TOOL_READ_EDITOR.to_string(),
                    args: json!({"a": 1}),
                }]),
            ]
        );
    }

    #[test]
    fn redact_api_key_removes_raw_and_escaped_key() {
        assert_eq!(
            redact_api_key("raw abc+123/= escaped abc%2B123%2F%3D", "abc+123/="),
            "raw [REDACTED] escaped [REDACTED]"
        );
    }

    #[tokio::test]
    async fn open_live_session_sends_setup_and_waits_for_setup_complete() {
        let config = live_config(&[]);
        let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(socket).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap().into_text().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            setup
        });

        let session = open_live_session_at(&format!("ws://{address}"), &boot)
            .await
            .unwrap();
        let setup: Value = serde_json::from_str(&server.await.unwrap()).unwrap();

        assert_eq!(setup["setup"]["model"], "models/gemini-live");
        assert_eq!(
            setup["setup"]["generationConfig"]["responseModalities"][0],
            "AUDIO"
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn open_live_session_delivers_server_events_from_reader_task() {
        let config = live_config(&[]);
        let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(socket).await.unwrap();
            let _ = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    r#"{"serverContent":{"outputTranscription":{"text":"hello"}}}"#.into(),
                ))
                .await
                .unwrap();
        });

        let mut session = open_live_session_at(&format!("ws://{address}"), &boot)
            .await
            .unwrap();

        assert_eq!(
            session.next_event().await,
            Some(GeminiEvent::OutputTranscript("hello".to_string()))
        );
        session.close().await.unwrap();
        server.await.unwrap();
    }
}
