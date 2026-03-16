use crate::auth::AuthProvider;
use crate::common::ResponseStream;
use crate::common::ResponsesApiRequest;
use crate::common::TextControls;
use crate::endpoint::responses::ResponsesOptions;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::requests::headers::build_conversation_headers;
use crate::requests::headers::insert_header;
use crate::requests::headers::subagent_header;
use crate::requests::responses::Compression;
use crate::sse::spawn_chat_completions_stream;
use crate::telemetry::SseTelemetry;
use codex_client::HttpTransport;
use codex_client::RequestCompression;
use codex_client::RequestTelemetry;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::warn;

pub struct ChatCompletionsClient<T: HttpTransport, A: AuthProvider> {
    session: EndpointSession<T, A>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport, A: AuthProvider> ChatCompletionsClient<T, A> {
    pub fn new(transport: T, provider: crate::provider::Provider, auth: A) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    pub async fn stream_request(
        &self,
        request: ResponsesApiRequest,
        options: ResponsesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let ResponsesOptions {
            conversation_id,
            session_source,
            extra_headers,
            compression,
            turn_state,
        } = options;
        let body = translate_responses_request_to_chat_body(request)?;

        let mut headers = extra_headers;
        if let Some(ref conv_id) = conversation_id {
            insert_header(&mut headers, "x-client-request-id", conv_id);
        }
        headers.extend(build_conversation_headers(conversation_id));
        if let Some(subagent) = subagent_header(&session_source) {
            insert_header(&mut headers, "x-openai-subagent", &subagent);
        }

        self.stream(body, headers, compression, turn_state).await
    }

    fn path() -> &'static str {
        "chat/completions"
    }

    pub async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
    ) -> Result<ResponseStream, ApiError> {
        let request_compression = match compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_with(
                Method::POST,
                Self::path(),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(spawn_chat_completions_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
        ))
    }
}

fn translate_responses_request_to_chat_body(
    request: ResponsesApiRequest,
) -> Result<Value, ApiError> {
    let mut body = json!({
        "model": request.model,
        "messages": translate_input_to_chat_messages(&request.instructions, &request.input)?,
        "stream": true,
    });

    let tools = request
        .tools
        .into_iter()
        .map(translate_tool_to_chat_tool)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = Value::String(request.tool_choice);
        body["parallel_tool_calls"] = Value::Bool(request.parallel_tool_calls);
    }
    if let Some(service_tier) = request.service_tier {
        body["service_tier"] = Value::String(service_tier);
    }
    if let Some(response_format) = translate_text_controls_to_response_format(request.text.as_ref())
    {
        body["response_format"] = response_format;
    }

    Ok(body)
}

fn translate_input_to_chat_messages(
    instructions: &str,
    input: &[ResponseItem],
) -> Result<Vec<Value>, ApiError> {
    let mut messages = Vec::new();
    if !instructions.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions,
        }));
    }

    for item in input {
        let translated = match item {
            ResponseItem::Message { role, content, .. } => Some(json!({
                "role": role,
                "content": translate_content_items_to_chat_content(content),
            })),
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => Some(chat_tool_call_message(
                name,
                arguments.clone(),
                call_id.clone(),
            )),
            ResponseItem::CustomToolCall {
                name,
                input,
                call_id,
                ..
            } => Some(chat_tool_call_message(
                name,
                json!({ "input": input }).to_string(),
                call_id.clone(),
            )),
            ResponseItem::LocalShellCall {
                call_id,
                id,
                action,
                ..
            } => {
                let call_id = call_id.clone().or_else(|| id.clone()).ok_or_else(|| {
                    ApiError::InvalidRequest {
                        message: "chat compatibility requires local shell calls to have an id"
                            .to_string(),
                    }
                })?;
                Some(chat_tool_call_message(
                    "local_shell",
                    serde_json::to_string(action).map_err(|err| ApiError::InvalidRequest {
                        message: format!(
                            "failed to encode local shell call for chat compatibility: {err}"
                        ),
                    })?,
                    call_id,
                ))
            }
            ResponseItem::FunctionCallOutput { call_id, output }
            | ResponseItem::CustomToolCallOutput { call_id, output } => Some(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output.body.to_text().unwrap_or_default(),
            })),
            ResponseItem::Reasoning { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::GhostSnapshot { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::Other => None,
        };
        if let Some(message) = translated {
            messages.push(message);
        }
    }

    Ok(messages)
}

fn translate_content_items_to_chat_content(content: &[ContentItem]) -> Value {
    let has_images = content
        .iter()
        .any(|item| matches!(item, ContentItem::InputImage { .. }));
    if !has_images {
        let text = content
            .iter()
            .filter_map(|item| match item {
                ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                    Some(text.as_str())
                }
                ContentItem::InputImage { .. } => None,
            })
            .collect::<String>();
        return Value::String(text);
    }

    Value::Array(
        content
            .iter()
            .filter_map(|item| match item {
                ContentItem::InputText { text } | ContentItem::OutputText { text } => Some(json!({
                    "type": "text",
                    "text": text,
                })),
                ContentItem::InputImage { image_url } => Some(json!({
                    "type": "image_url",
                    "image_url": { "url": image_url },
                })),
            })
            .collect(),
    )
}

fn chat_tool_call_message(name: &str, arguments: String, call_id: String) -> Value {
    json!({
        "role": "assistant",
        "tool_calls": [{
            "id": call_id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": arguments,
            }
        }]
    })
}

fn translate_tool_to_chat_tool(tool: Value) -> Result<Option<Value>, ApiError> {
    let Some(kind) = tool.get("type").and_then(Value::as_str) else {
        return Err(ApiError::InvalidRequest {
            message: "chat compatibility could not translate a tool without a type".to_string(),
        });
    };

    match kind {
        "function" => Ok(Some(json!({
            "type": "function",
            "function": {
                "name": tool.get("name").cloned().unwrap_or(Value::Null),
                "description": tool.get("description").cloned().unwrap_or(Value::Null),
                "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {}, "additionalProperties": false })),
                "strict": tool.get("strict").cloned().unwrap_or(Value::Bool(false)),
            }
        }))),
        "custom" => {
            let name = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                ApiError::InvalidRequest {
                    message: "chat compatibility could not translate a custom tool without a name"
                        .to_string(),
                }
            })?;
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let syntax = tool
                .get("format")
                .and_then(|format| format.get("syntax"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let definition = tool
                .get("format")
                .and_then(|format| format.get("definition"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut translated_description = description.to_string();
            if !syntax.is_empty() || !definition.is_empty() {
                translated_description.push_str(
                    "\n\nProvide the full tool payload in the JSON string field `input`.",
                );
                if !syntax.is_empty() {
                    translated_description.push_str(&format!("\nSyntax: {syntax}"));
                }
                if !definition.is_empty() {
                    translated_description.push_str(&format!("\nDefinition: {definition}"));
                }
            }
            Ok(Some(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": translated_description,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "input": {
                                "type": "string",
                                "description": "Full tool input payload."
                            }
                        },
                        "required": ["input"],
                        "additionalProperties": false
                    },
                    "strict": false,
                }
            })))
        }
        "local_shell" => Ok(Some(json!({
            "type": "function",
            "function": {
                "name": "local_shell",
                "description": "Runs a shell command and returns its output.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The command to execute."
                        },
                        "workdir": {
                            "type": "string",
                            "description": "The working directory to execute the command in."
                        },
                        "timeout_ms": {
                            "type": "number",
                            "description": "The timeout for the command in milliseconds."
                        }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                },
                "strict": false,
            }
        }))),
        "web_search" | "image_generation" => {
            warn!(
                tool_type = kind,
                "dropping Responses-native tool from chat compatibility request"
            );
            Ok(None)
        }
        unsupported => Err(ApiError::InvalidRequest {
            message: format!(
                "wire_api=\"chat\" cannot translate Responses tool type `{unsupported}` yet"
            ),
        }),
    }
}

fn translate_text_controls_to_response_format(text: Option<&TextControls>) -> Option<Value> {
    let format = text?.format.as_ref()?;
    Some(json!({
        "type": "json_schema",
        "json_schema": {
            "name": format.name,
            "schema": format.schema,
            "strict": format.strict,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::translate_responses_request_to_chat_body;
    use crate::common::ResponsesApiRequest;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn chat_compatibility_drops_responses_native_tools() {
        let request = ResponsesApiRequest {
            model: "chat-model".to_string(),
            instructions: "system".to_string(),
            input: Vec::new(),
            tools: vec![
                json!({
                    "type": "web_search",
                    "external_web_access": true
                }),
                json!({
                    "type": "image_generation",
                    "output_format": "png"
                }),
                json!({
                    "type": "function",
                    "name": "echo",
                    "description": "Echoes back text.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        },
                        "required": ["value"],
                        "additionalProperties": false
                    },
                    "strict": true
                }),
            ],
            tool_choice: "auto".to_string(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
        };

        let body = translate_responses_request_to_chat_body(request).expect("request translates");
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], json!("echo"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
    }

    #[test]
    fn chat_compatibility_omits_tool_controls_when_no_tools_survive() {
        let request = ResponsesApiRequest {
            model: "chat-model".to_string(),
            instructions: String::new(),
            input: Vec::new(),
            tools: vec![json!({
                "type": "web_search",
                "external_web_access": false
            })],
            tool_choice: "required".to_string(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
        };

        let body = translate_responses_request_to_chat_body(request).expect("request translates");
        assert_eq!(body.get("tools"), None);
        assert_eq!(body.get("tool_choice"), None);
        assert_eq!(body.get("parallel_tool_calls"), None);
    }
}
