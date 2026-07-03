use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;

fn chat_sse_tool_call() -> String {
    concat!(
        "data: {\"id\":\"chatcmpl-tool\",\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"index\":0}]}\n\n",
        "data: {\"id\":\"chatcmpl-tool\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"printf chat-follow-up\\\"}\"}}]},\"finish_reason\":\"tool_calls\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    )
    .to_string()
}

fn chat_sse_final_message() -> String {
    concat!(
        "data: {\"id\":\"chatcmpl-final\",\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    )
    .to_string()
}

fn chat_sse_second_turn() -> String {
    concat!(
        "data: {\"id\":\"chatcmpl-second-turn\",\"choices\":[{\"delta\":{\"content\":\"remembered\"},\"finish_reason\":\"stop\",\"index\":0}]}\n\n",
        "data: [DONE]\n\n",
    )
    .to_string()
}

fn chat_provider(base_url: String) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "chat-test".into(),
        base_url: Some(base_url),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Chat,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(2000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_provider_continues_after_tool_call_output() {
    skip_if_no_network!();

    let (server, _) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk {
            gate: None,
            body: chat_sse_tool_call(),
        }],
        vec![StreamingSseChunk {
            gate: None,
            body: chat_sse_final_message(),
        }],
        vec![StreamingSseChunk {
            gate: None,
            body: chat_sse_second_turn(),
        }],
    ])
    .await;

    let model_provider = chat_provider(format!("{}/v1", server.uri()));
    let TestCodex { codex, .. } = test_codex()
        .with_config(move |config| {
            config.model = Some("chat-test-model".to_string());
            config.model_provider_id = model_provider.name.clone();
            config
                .model_providers
                .insert(model_provider.name.clone(), model_provider.clone());
            config.model_provider = model_provider;
        })
        .build_with_streaming_server(&server)
        .await
        .unwrap();

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "run the command".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .unwrap();

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    server.wait_for_request_count(2).await;
    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);

    let second_request: Value =
        serde_json::from_slice(&requests[1]).expect("second request body should be JSON");
    let messages = second_request["messages"]
        .as_array()
        .expect("chat request should include messages");
    assert!(
        messages.iter().any(|message| {
            message["role"] == "assistant"
                && message["tool_calls"]
                    .as_array()
                    .is_some_and(|tool_calls| !tool_calls.is_empty())
        }),
        "second chat request should replay the assistant tool call"
    );
    assert!(
        messages.iter().any(|message| {
            message["role"] == "tool"
                && message["tool_call_id"] == "call_1"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("chat-follow-up"))
        }),
        "second chat request should include the tool output"
    );

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "what happened last turn?".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await
        .unwrap();

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(requests.len(), 3);
    let third_request: Value =
        serde_json::from_slice(&requests[2]).expect("third request body should be JSON");
    let messages = third_request["messages"]
        .as_array()
        .expect("chat request should include messages");
    assert!(
        messages.iter().any(|message| {
            message["role"] == "assistant"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("done"))
        }),
        "third chat request should replay the previous final assistant message"
    );

    server.shutdown().await;
}
