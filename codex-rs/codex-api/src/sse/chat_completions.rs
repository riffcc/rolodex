use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::rate_limits::parse_all_rate_limits;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

#[cfg(test)]
use codex_client::TransportError;
#[cfg(test)]
use tokio_util::io::ReaderStream;

#[derive(Debug, Default)]
struct ChatStreamState {
    response_id: Option<String>,
    assistant_message_open: bool,
    assistant_text: String,
    tool_calls: BTreeMap<usize, ToolCallState>,
    token_usage: Option<TokenUsage>,
    saw_created: bool,
    saw_server_model: Option<String>,
    saw_terminal_finish_reason: bool,
}

#[derive(Debug, Default)]
struct ToolCallState {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<ChatCompletionChoice>,
    #[serde(default)]
    usage: Option<ChatCompletionUsage>,
    #[serde(default)]
    error: Option<ChatCompletionError>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionChoice {
    #[serde(default)]
    delta: ChatCompletionDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionDelta {
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatCompletionToolCallDelta>>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatCompletionFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionUsage {
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
}

impl From<ChatCompletionUsage> for TokenUsage {
    fn from(value: ChatCompletionUsage) -> Self {
        TokenUsage {
            input_tokens: value.prompt_tokens,
            cached_input_tokens: 0,
            output_tokens: value.completion_tokens,
            reasoning_output_tokens: 0,
            total_tokens: value.total_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionError {
    #[serde(default)]
    message: Option<String>,
}

pub fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(turn_state) = turn_state.as_ref()
        && let Some(header_value) = stream_response
            .headers
            .get("x-codex-turn-state")
            .and_then(|value| value.to_str().ok())
    {
        let _ = turn_state.set(header_value.to_string());
    }

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        for snapshot in rate_limit_snapshots {
            if tx_event
                .send(Ok(ResponseEvent::RateLimits(snapshot)))
                .await
                .is_err()
            {
                return;
            }
        }
        process_chat_sse(stream_response.bytes, tx_event, idle_timeout, telemetry).await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

async fn process_chat_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = stream.eventsource();
    let mut state = ChatStreamState::default();

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(err))) => {
                let _ = tx_event.send(Err(ApiError::Stream(err.to_string()))).await;
                return;
            }
            Ok(None) => {
                if state.saw_terminal_finish_reason {
                    if let Err(err) = flush_and_complete(&mut state, &tx_event).await {
                        let _ = tx_event.send(Err(err)).await;
                    }
                } else {
                    let error = ApiError::Stream("stream closed before completion".to_string());
                    let _ = tx_event.send(Err(error)).await;
                }
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "idle timeout waiting for SSE".to_string(),
                    )))
                    .await;
                return;
            }
        };

        trace!("Chat Completions SSE event: {}", sse.data);
        if sse.data == "[DONE]" {
            if let Err(err) = flush_and_complete(&mut state, &tx_event).await {
                let _ = tx_event.send(Err(err)).await;
            }
            return;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_str(&sse.data) {
            Ok(chunk) => chunk,
            Err(err) => {
                debug!(
                    "Failed to parse Chat Completions SSE chunk: {err}, data: {}",
                    sse.data
                );
                continue;
            }
        };

        if let Err(err) = process_chat_chunk(chunk, &mut state, &tx_event).await {
            let _ = tx_event.send(Err(err)).await;
            return;
        }
    }
}

async fn process_chat_chunk(
    chunk: ChatCompletionChunk,
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> Result<(), ApiError> {
    if !state.saw_created {
        tx_event
            .send(Ok(ResponseEvent::Created))
            .await
            .map_err(|_| {
                ApiError::Stream("failed to forward chat compatibility created event".to_string())
            })?;
        state.saw_created = true;
    }

    if let Some(response_id) = chunk.id {
        state.response_id = Some(response_id);
    }

    if let Some(model) = chunk.model
        && state.saw_server_model.as_deref() != Some(model.as_str())
    {
        tx_event
            .send(Ok(ResponseEvent::ServerModel(model.clone())))
            .await
            .map_err(|_| {
                ApiError::Stream(
                    "failed to forward chat compatibility server model event".to_string(),
                )
            })?;
        state.saw_server_model = Some(model);
    }

    if let Some(usage) = chunk.usage {
        state.token_usage = Some(usage.into());
    }

    if let Some(error) = chunk.error {
        return Err(ApiError::Stream(error.message.unwrap_or_else(|| {
            "chat completions stream returned an error".to_string()
        })));
    }

    for choice in chunk.choices {
        if let Some(content) = choice.delta.content {
            for delta in chat_delta_strings(&content) {
                if !delta.is_empty() {
                    ensure_assistant_message_started(state, tx_event).await?;
                    state.assistant_text.push_str(&delta);
                    tx_event
                        .send(Ok(ResponseEvent::OutputTextDelta(delta)))
                        .await
                        .map_err(|_| {
                            ApiError::Stream(
                                "failed to forward chat compatibility text delta".to_string(),
                            )
                        })?;
                }
            }
        }

        if let Some(tool_calls) = choice.delta.tool_calls {
            for tool_call in tool_calls {
                let entry = state.tool_calls.entry(tool_call.index).or_default();
                if let Some(id) = tool_call.id {
                    entry.id = Some(id);
                }
                if let Some(function) = tool_call.function {
                    if let Some(name) = function.name {
                        entry.name = Some(name);
                    }
                    if let Some(arguments) = function.arguments {
                        entry.arguments.push_str(&arguments);
                    }
                }
            }
        }

        if let Some(finish_reason) = choice.finish_reason {
            flush_for_finish_reason(state, tx_event, finish_reason.as_str()).await?;
        }
    }

    Ok(())
}

async fn ensure_assistant_message_started(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> Result<(), ApiError> {
    if state.assistant_message_open {
        return Ok(());
    }

    tx_event
        .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: Vec::new(),
            phase: None,
        })))
        .await
        .map_err(|_| {
            ApiError::Stream("failed to forward chat compatibility item start".to_string())
        })?;
    state.assistant_message_open = true;
    Ok(())
}

async fn flush_for_finish_reason(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    finish_reason: &str,
) -> Result<(), ApiError> {
    match finish_reason {
        "tool_calls" => {
            flush_assistant_message(state, tx_event).await?;
            flush_tool_calls(state, tx_event).await?;
            state.saw_terminal_finish_reason = true;
        }
        "stop" | "length" | "content_filter" => {
            flush_assistant_message(state, tx_event).await?;
            state.saw_terminal_finish_reason = true;
        }
        _ => {}
    }
    Ok(())
}

async fn flush_assistant_message(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> Result<(), ApiError> {
    if !state.assistant_message_open {
        return Ok(());
    }

    let item = ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: std::mem::take(&mut state.assistant_text),
        }],
        phase: None,
    };
    state.assistant_message_open = false;
    tx_event
        .send(Ok(ResponseEvent::OutputItemDone(item)))
        .await
        .map_err(|_| {
            ApiError::Stream("failed to forward chat compatibility assistant message".to_string())
        })?;
    Ok(())
}

async fn flush_tool_calls(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> Result<(), ApiError> {
    let tool_calls = std::mem::take(&mut state.tool_calls);
    for (_, tool_call) in tool_calls {
        let name = tool_call
            .name
            .ok_or_else(|| ApiError::Stream("tool call missing name".to_string()))?;
        let (namespace, name) = split_chat_function_name(&name);
        let item = ResponseItem::FunctionCall {
            id: None,
            name,
            namespace,
            arguments: tool_call.arguments,
            call_id: tool_call
                .id
                .unwrap_or_else(|| "chat-compat-call".to_string()),
        };
        tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .map_err(|_| {
                ApiError::Stream("failed to forward chat compatibility tool call".to_string())
            })?;
    }
    Ok(())
}

fn split_chat_function_name(name: &str) -> (Option<String>, String) {
    let Some((namespace, tool_name)) = name.rsplit_once("__") else {
        return (None, name.to_string());
    };
    if namespace.is_empty() || tool_name.is_empty() {
        return (None, name.to_string());
    }
    let namespace_is_known = namespace.starts_with("mcp__")
        || matches!(
            namespace,
            "codex_app" | "image_gen" | "multi_agent_v1" | "web"
        );
    if namespace_is_known {
        (Some(namespace.to_string()), tool_name.to_string())
    } else {
        (None, name.to_string())
    }
}

async fn flush_and_complete(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> Result<(), ApiError> {
    flush_assistant_message(state, tx_event).await?;
    flush_tool_calls(state, tx_event).await?;
    tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id: state
                .response_id
                .clone()
                .unwrap_or_else(|| "chat-compat".to_string()),
            token_usage: state.token_usage.clone(),
            end_turn: None,
        }))
        .await
        .map_err(|_| {
            ApiError::Stream("failed to forward chat compatibility completion".to_string())
        })
}

fn chat_delta_strings(content: &Value) -> Vec<String> {
    match content {
        Value::String(text) => vec![text.clone()],
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    part.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;
    use http::HeaderMap;
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use tokio_test::io::Builder as IoBuilder;

    fn idle_timeout() -> Duration {
        Duration::from_millis(1_000)
    }

    async fn collect_events(chunks: &[&[u8]]) -> Vec<ResponseEvent> {
        let mut builder = IoBuilder::new();
        for chunk in chunks {
            builder.read(chunk);
        }
        let reader = builder.build();
        let stream =
            ReaderStream::new(reader).map_err(|err| TransportError::Network(err.to_string()));
        let stream_response = StreamResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            bytes: Box::pin(stream),
        };
        let mut response_stream =
            spawn_chat_completions_stream(stream_response, idle_timeout(), None, None);
        let mut events = Vec::new();
        while let Some(event) = response_stream.rx_event.recv().await {
            events.push(event.expect("chat compatibility event"));
        }
        events
    }

    #[tokio::test]
    async fn chat_completion_text_stream_maps_to_response_events() {
        let chunks = [
            br#"data: {"id":"chatcmpl-1","model":"compat-model","choices":[{"delta":{"content":"Hel"},"finish_reason":null}]}

"# as &[u8],
            br#"data: {"id":"chatcmpl-1","choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}

"#,
            br#"data: [DONE]

"#,
        ];

        let events = collect_events(&chunks).await;
        assert_eq!(events.len(), 8);
        assert!(matches!(events[0], ResponseEvent::RateLimits(_)));
        assert!(matches!(events[1], ResponseEvent::Created));
        assert!(matches!(
            &events[2],
            ResponseEvent::ServerModel(model) if model == "compat-model"
        ));
        assert!(matches!(
            &events[3],
            ResponseEvent::OutputItemAdded(ResponseItem::Message { role, .. }) if role == "assistant"
        ));
        assert!(matches!(
            &events[4],
            ResponseEvent::OutputTextDelta(delta) if delta == "Hel"
        ));
        assert!(matches!(
            &events[5],
            ResponseEvent::OutputTextDelta(delta) if delta == "lo"
        ));
        assert!(matches!(
            &events[6],
            ResponseEvent::OutputItemDone(ResponseItem::Message { role, content, .. })
                if role == "assistant"
                    && content
                        == &vec![ContentItem::OutputText {
                            text: "Hello".to_string(),
                        }]
        ));
        assert!(matches!(
            &events[7],
            ResponseEvent::Completed {
                response_id,
                token_usage,
                ..
            } if response_id == "chatcmpl-1" && token_usage.is_none()
        ));
    }

    #[tokio::test]
    async fn chat_completion_tool_call_stream_maps_to_function_call() {
        let chunks = [
            br#"data: {"id":"chatcmpl-2","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"exec_command","arguments":"{\"cmd\":\"echo"}}]}}]}

"# as &[u8],
            br#"data: {"id":"chatcmpl-2","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":" hi\"}"}}],"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}

"#,
            br#"data: [DONE]

"#,
        ];

        let events = collect_events(&chunks).await;
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], ResponseEvent::RateLimits(_)));
        assert!(matches!(events[1], ResponseEvent::Created));
        assert!(matches!(
            &events[2],
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id,
                name,
                arguments,
                call_id,
                ..
            }) if id.is_none()
                && name == "exec_command"
                && arguments == "{\"cmd\":\"echo hi\"}"
                && call_id == "call_1"
        ));
        assert!(matches!(
            &events[3],
            ResponseEvent::Completed {
                response_id,
                token_usage: Some(TokenUsage {
                    input_tokens: 10,
                    cached_input_tokens: 0,
                    output_tokens: 2,
                    reasoning_output_tokens: 0,
                    total_tokens: 12,
                }),
                ..
            } if response_id == "chatcmpl-2"
        ));
    }

    #[tokio::test]
    async fn chat_completion_namespaced_tool_call_stream_restores_namespace() {
        let chunks = [
            br#"data: {"id":"chatcmpl-namespace","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"mcp__demo__lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}

"# as &[u8],
            br#"data: [DONE]

"#,
        ];

        let events = collect_events(&chunks).await;
        assert!(matches!(
            &events[2],
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                namespace,
                name,
                arguments,
                call_id,
                ..
            }) if namespace.as_deref() == Some("mcp__demo")
                && name == "lookup"
                && arguments == "{}"
                && call_id == "call_1"
        ));
    }

    #[tokio::test]
    async fn chat_completion_stream_without_done_after_stop_completes() {
        let chunks = [
            br#"data: {"id":"chatcmpl-3","choices":[{"delta":{"content":"Hello"},"finish_reason":"stop"}]}

"# as &[u8],
        ];

        let events = collect_events(&chunks).await;
        assert_eq!(events.len(), 6);
        assert!(matches!(events[0], ResponseEvent::RateLimits(_)));
        assert!(matches!(events[1], ResponseEvent::Created));
        assert!(matches!(
            &events[2],
            ResponseEvent::OutputItemAdded(ResponseItem::Message { role, .. }) if role == "assistant"
        ));
        assert!(matches!(
            &events[3],
            ResponseEvent::OutputTextDelta(delta) if delta == "Hello"
        ));
        assert!(matches!(
            &events[4],
            ResponseEvent::OutputItemDone(ResponseItem::Message { role, content, .. })
                if role == "assistant"
                    && content
                        == &vec![ContentItem::OutputText {
                            text: "Hello".to_string(),
                        }]
        ));
        assert!(matches!(
            &events[5],
            ResponseEvent::Completed {
                response_id,
                token_usage,
                ..
            } if response_id == "chatcmpl-3" && token_usage.is_none()
        ));
    }
}
